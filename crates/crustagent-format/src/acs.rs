//! Parser for the Microsoft Agent 2.0 compiled binary format (`.acs`).
//!
//! Reverse-engineered from the original compiled binary format.

use crate::decode::decode_data;
use crate::error::{Error, Result};
use crate::model::*;
use crate::reader::Cursor;

/// First DWORD of an ACS 2.0 file.
pub const ACS_SIGNATURE: u32 = 0xABCD_ABC3;

/// A `{file offset, byte length}` descriptor from the block directory.
#[derive(Clone, Copy, Debug)]
struct Block {
    offset: usize,
    len: usize,
}

impl Block {
    fn range(&self) -> std::ops::Range<usize> {
        self.offset..self.offset + self.len
    }
}

/// A parsed ACS 2.0 character file.
///
/// The header, names, states and all animations are parsed eagerly; images and sounds
/// are decoded on demand via [`AcsFile::image`] / [`AcsFile::sound`].
pub struct AcsFile {
    data: Vec<u8>,
    pub header: FileHeader,
    pub tts: Option<Tts>,
    pub balloon: Option<Balloon>,
    pub names: Vec<Name>,
    pub states: Vec<State>,
    /// Animation names in file order (parallel to `animations`).
    pub gesture_names: Vec<String>,
    pub animations: Vec<Animation>,
    image_index: Vec<Block>,
    sound_index: Vec<Block>,
}

/// Return the leading signature DWORD, or `None` if the buffer is too short.
pub fn signature(bytes: &[u8]) -> Option<u32> {
    (bytes.len() >= 4).then(|| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

impl AcsFile {
    /// Open and parse an `.acs` file from disk.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<AcsFile> {
        let data = std::fs::read(path)?;
        AcsFile::parse(data)
    }

    /// Parse an in-memory `.acs` byte buffer.
    pub fn parse(data: Vec<u8>) -> Result<AcsFile> {
        let sig = signature(&data).ok_or(Error::UnexpectedEof {
            context: "signature",
            offset: 0,
            needed: 4,
            available: data.len(),
        })?;
        if sig != ACS_SIGNATURE {
            return Err(Error::BadSignature { found: sig });
        }

        // Block directory: four {offset,length} descriptors at 4, 12, 20, 28.
        let blocks = {
            let mut c = Cursor::at(&data, 4);
            let mut b = [Block { offset: 0, len: 0 }; 4];
            for slot in &mut b {
                let v = c.u64()?;
                *slot = Block {
                    offset: (v & 0xFFFF_FFFF) as usize,
                    len: (v >> 32) as usize,
                };
            }
            b
        };
        let (header_blk, gestures_blk, images_blk, sounds_blk) =
            (blocks[0], blocks[1], blocks[2], blocks[3]);

        let hb = parse_header_block(&data, header_blk)?;
        let (gesture_names, animations) = parse_gestures(&data, gestures_blk)?;
        let image_index = parse_index(&data, images_blk)?;
        let sound_index = parse_index(&data, sounds_blk)?;

        Ok(AcsFile {
            data,
            header: hb.header,
            tts: hb.tts,
            balloon: hb.balloon,
            names: hb.names,
            states: hb.states,
            gesture_names,
            animations,
            image_index,
            sound_index,
        })
    }

    /// Find the character name for a Windows `LANGID`, mirroring the original's name lookup:
    /// prefer an exact match, then any name sharing the same primary language (low 10
    /// bits), then fall back to the first name in the file.
    pub fn name(&self, langid: u16) -> Option<&Name> {
        self.names
            .iter()
            .find(|n| n.language == langid)
            .or_else(|| {
                let primary = langid & 0x03FF;
                self.names.iter().find(|n| n.language & 0x03FF == primary)
            })
            .or_else(|| self.names.first())
    }

    /// The default character name. OS-agnostic: prefers US English, else the first
    /// stored name. (At runtime, higher layers should call [`AcsFile::name`] with the
    /// user's actual `LANGID`.)
    pub fn default_name(&self) -> Option<&Name> {
        self.name(0x0409)
    }

    /// Number of images in the image table.
    pub fn image_count(&self) -> usize {
        self.image_index.len()
    }

    /// Decode image `index` to its 8-bpp palette-index bits.
    pub fn image(&self, index: usize) -> Result<Image> {
        let blk = self
            .image_index
            .get(index)
            .copied()
            .ok_or(Error::BadImage { index })?;
        read_image(&self.data, blk.offset, index)
    }

    /// Number of sounds in the sound table.
    pub fn sound_count(&self) -> usize {
        self.sound_index.len()
    }

    /// Borrow the raw bytes of sound `index` (a complete standalone WAV file).
    pub fn sound(&self, index: usize) -> Option<&[u8]> {
        let blk = self.sound_index.get(index)?;
        self.data.get(blk.range())
    }

    /// Find an animation by name. Matching is **case-insensitive** to mirror the engine
    /// (`FindGesture`/`FindAnimation`): state definitions often reference animations in a
    /// different case than they are authored (e.g. state `"IDLINGLEVEL1"` lists
    /// `"IDLE1_1"` while the animation is named `"Idle1_1"`).
    pub fn animation(&self, name: &str) -> Option<&Animation> {
        self.gesture_names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
            .map(|i| &self.animations[i])
    }

    /// Composite one frame into a top-down palette-indexed image the size of the
    /// character. This is the core compositor.
    ///
    /// Mirrors the original compositor (8-bit path): the frame's base images are drawn
    /// back-to-front (highest image index is the base layer, lower indices over it),
    /// then, if `mouth` is given and the frame has a matching overlay, that mouth image
    /// is drawn on top. A `replace` overlay suppresses base image index 0. The canvas is
    /// pre-filled with the transparency index and transparent source pixels are skipped,
    /// so lower layers (and the background) show through.
    pub fn composite_frame_indexed(
        &self,
        frame: &Frame,
        mouth: Option<MouthOverlay>,
    ) -> Result<Indexed> {
        let (w, h) = self.header.image_size;
        let mut canvas = Indexed::filled(w as u32, h as u32, self.header.transparency);

        let overlay = mouth.and_then(|m| frame.overlays.iter().find(|o| o.overlay_type == m));
        let replace_base = overlay.is_some_and(|o| o.replace);

        // Base image stack: highest index (bottom) first, down to index 0 (topmost image).
        for i in (0..frame.images.len()).rev() {
            if i == 0 && replace_base {
                continue;
            }
            let fi = frame.images[i];
            let img = self.image(fi.image_ndx as usize)?;
            self.blit(&mut canvas, &img, fi.offset);
        }

        // Mouth overlay on top.
        if let Some(o) = overlay {
            let img = self.image(o.image_ndx as usize)?;
            self.blit(&mut canvas, &img, o.offset);
        }

        Ok(canvas)
    }

    /// Composite one frame to top-down RGBA (transparency index → transparent pixel).
    pub fn composite_frame(&self, frame: &Frame, mouth: Option<MouthOverlay>) -> Result<Rgba> {
        Ok(self
            .composite_frame_indexed(frame, mouth)?
            .to_rgba(&self.header.palette))
    }

    /// Blit one 8-bpp image onto the (bottom-up-addressed) canvas at `offset`, skipping
    /// transparent-index pixels. The image and the character canvas are both bottom-up
    /// DIBs; `offset` is in that bottom-up space. We write into the top-down `Indexed`
    /// buffer by flipping the row on store.
    fn blit(&self, canvas: &mut Indexed, img: &Image, offset: (i16, i16)) {
        let transparency = self.header.transparency;
        let stride = img.stride();
        let cw = canvas.width as i32;
        let ch = canvas.height as i32;
        let (off_x, off_y) = (offset.0 as i32, offset.1 as i32);

        for src_y in 0..img.height as i32 {
            let cy_bottom_up = src_y + off_y;
            if cy_bottom_up < 0 || cy_bottom_up >= ch {
                continue;
            }
            let dst_row_top_down = (ch - 1 - cy_bottom_up) as usize;
            let src_row = (src_y as usize) * stride;
            for src_x in 0..img.width as i32 {
                let cx = src_x + off_x;
                if cx < 0 || cx >= cw {
                    continue;
                }
                let idx = img.bits[src_row + src_x as usize];
                if idx == transparency {
                    continue;
                }
                canvas.indices[dst_row_top_down * canvas.width as usize + cx as usize] = idx;
            }
        }
    }
}

/// The fully-parsed contents of the header block (block[0]).
struct HeaderBlock {
    header: FileHeader,
    tts: Option<Tts>,
    balloon: Option<Balloon>,
    names: Vec<Name>,
    states: Vec<State>,
}

fn parse_header_block(data: &[u8], blk: Block) -> Result<HeaderBlock> {
    let block = data.get(blk.range()).ok_or(Error::UnexpectedEof {
        context: "header block",
        offset: blk.offset,
        needed: blk.len,
        available: data.len().saturating_sub(blk.offset),
    })?;
    let mut c = Cursor::new(block);

    let version_minor = c.u16()?;
    let version_major = c.u16()?;
    let names_offset_abs = c.u32()? as usize;
    let _names_size = c.u32()?;
    let guid = c.guid()?;
    let width = c.u16()?;
    let height = c.u16()?;
    let transparency = c.u8()?;
    let style = c.u32()?;
    let _unknown = c.u32()?; // always 2

    let tts = if style & char_style::TTS != 0 {
        Some(parse_tts(&mut c)?)
    } else {
        None
    };
    let balloon = if style & char_style::BALLOON != 0 {
        Some(parse_balloon(&mut c)?)
    } else {
        None
    };
    let palette = parse_palette(&mut c)?;
    skip_icon(&mut c)?;

    // States occupy the cursor position through the start of names.
    let states = parse_states(&mut c)?;

    // Names live at an absolute file offset; rebase into the header block.
    let names_offset = names_offset_abs
        .checked_sub(blk.offset)
        .ok_or_else(|| Error::InvalidData("names offset precedes header block".into()))?;
    let names = {
        let mut nc = Cursor::at(block, names_offset);
        parse_names(&mut nc)?
    };

    let header = FileHeader {
        version_major,
        version_minor,
        guid,
        image_size: (width, height),
        transparency,
        style,
        palette,
    };
    Ok(HeaderBlock {
        header,
        tts,
        balloon,
        names,
        states,
    })
}

fn parse_tts(c: &mut Cursor) -> Result<Tts> {
    let engine = c.guid()?;
    let mode = c.guid()?;
    let speed = c.i32()?;
    let pitch = c.i16()?;
    let has_extra = c.u8()? != 0;
    if has_extra {
        let language = c.u16()?;
        let _unknown = c.string(true)?;
        let gender = c.u16()?;
        let age = c.u16()?;
        let style = c.string(true)?;
        Ok(Tts {
            engine,
            mode,
            speed,
            pitch,
            language: Some(language),
            gender,
            age,
            style,
        })
    } else {
        Ok(Tts {
            engine,
            mode,
            speed,
            pitch,
            language: None,
            gender: 0,
            age: 0,
            style: String::new(),
        })
    }
}

fn parse_balloon(c: &mut Cursor) -> Result<Balloon> {
    let lines = c.u8()?;
    let per_line = c.u8()?;
    let fg_color = c.color()?;
    let bg_color = c.color()?;
    let border_color = c.color()?;
    let font_name = c.string(true)?;
    let font_height = c.i32()?;
    let weight = c.u16()?;
    let strikeout = c.u16()?;
    let italic = c.u16()?;
    Ok(Balloon {
        lines,
        per_line,
        fg_color,
        bg_color,
        border_color,
        font_name,
        font_height,
        bold: weight >= 700, // FW_BOLD
        strikeout: strikeout != 0,
        italic: italic != 0,
    })
}

fn parse_palette(c: &mut Cursor) -> Result<Vec<Color>> {
    let count = c.u32()? as usize;
    let mut palette = Vec::with_capacity(count.min(256));
    for i in 0..count {
        let color = c.color()?;
        if i < 256 {
            palette.push(color);
        }
    }
    Ok(palette)
}

fn skip_icon(c: &mut Cursor) -> Result<()> {
    let has_icon = c.u8()?;
    if has_icon != 0 {
        let mask_size = c.u32()? as usize;
        c.skip(mask_size)?;
        let color_size = c.u32()? as usize;
        c.skip(color_size)?;
    }
    Ok(())
}

fn parse_states(c: &mut Cursor) -> Result<Vec<State>> {
    let count = c.u16()?;
    let mut states = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = c.string(true)?;
        let gesture_count = c.u16()?;
        let mut animations = Vec::with_capacity(gesture_count as usize);
        for _ in 0..gesture_count {
            animations.push(c.string(true)?);
        }
        states.push(State { name, animations });
    }
    Ok(states)
}

fn parse_names(c: &mut Cursor) -> Result<Vec<Name>> {
    let count = c.u16()?;
    let mut names = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let language = c.u16()?;
        let name = first_letter_caps(c.string(true)?);
        let desc1 = c.string(true)?;
        let desc2 = c.string(true)?;
        names.push(Name {
            language,
            name,
            desc1,
            desc2,
        });
    }
    Ok(names)
}

fn first_letter_caps(s: String) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) if first.is_lowercase() => {
            first.to_uppercase().collect::<String>() + chars.as_str()
        }
        _ => s,
    }
}

fn parse_index(data: &[u8], blk: Block) -> Result<Vec<Block>> {
    if blk.len == 0 {
        return Ok(Vec::new());
    }
    let slice = data.get(blk.range()).ok_or(Error::UnexpectedEof {
        context: "index block",
        offset: blk.offset,
        needed: blk.len,
        available: data.len().saturating_sub(blk.offset),
    })?;
    let mut c = Cursor::new(slice);
    let count = c.u32()? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let offset = c.u32()? as usize;
        let len = c.u32()? as usize;
        let _checksum = c.u32()?;
        entries.push(Block { offset, len });
    }
    Ok(entries)
}

fn parse_gestures(data: &[u8], blk: Block) -> Result<(Vec<String>, Vec<Animation>)> {
    if blk.len == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let slice = data.get(blk.range()).ok_or(Error::UnexpectedEof {
        context: "gesture block",
        offset: blk.offset,
        needed: blk.len,
        available: data.len().saturating_sub(blk.offset),
    })?;
    let mut c = Cursor::new(slice);
    let count = c.u32()? as usize;
    let mut names = Vec::with_capacity(count);
    let mut animations = Vec::with_capacity(count);
    for _ in 0..count {
        let name = c.string(true)?;
        let anim_offset = c.u32()? as usize;
        let anim_size = c.u32()? as usize;
        let anim = parse_animation(data, anim_offset, anim_size, &name)?;
        names.push(name);
        animations.push(anim);
    }
    Ok((names, animations))
}

fn parse_animation(data: &[u8], offset: usize, size: usize, name_hint: &str) -> Result<Animation> {
    let end = offset.saturating_add(size);
    let slice = data.get(offset..end.min(data.len())).ok_or(Error::UnexpectedEof {
        context: "animation record",
        offset,
        needed: size,
        available: data.len().saturating_sub(offset),
    })?;
    let mut c = Cursor::new(slice);

    let name = c.string(true)?;
    let return_kind = ReturnKind::from_u8(c.u8()?);
    let return_name = c.string(true)?;
    let frame_count = c.u16()?;

    let mut frames = Vec::with_capacity(frame_count as usize);
    for _ in 0..frame_count {
        frames.push(parse_frame(&mut c)?);
    }

    let _ = name_hint; // name inside the record is authoritative; hint kept for debugging
    Ok(Animation {
        name,
        return_kind,
        return_name,
        frames,
    })
}

fn parse_frame(c: &mut Cursor) -> Result<Frame> {
    let image_count = c.u16()?;
    let mut images = Vec::with_capacity(image_count as usize);
    for _ in 0..image_count {
        let image_ndx = c.u32()?;
        let off_x = c.i16()?;
        let off_y = c.i16()?;
        images.push(FrameImage {
            image_ndx,
            offset: (off_x, off_y),
        });
    }

    let sound_ndx = c.i16()?;
    let duration = c.u16()?;
    let exit_frame = c.i16()?;

    let branch_count = c.u8()? as usize;
    let mut branching = Vec::new();
    for i in 0..branch_count {
        let raw = c.u32()?;
        if i < 3 {
            branching.push(Branch {
                frame_ndx: (raw & 0xFFFF) as i16,
                probability: (raw >> 16) as u16,
            });
        }
    }

    let overlay_count = c.u8()? as usize;
    let mut overlays = Vec::with_capacity(overlay_count);
    for _ in 0..overlay_count {
        let overlay_type = MouthOverlay::from_u8(c.u8()?);
        let replace = c.u8()? != 0;
        let image_ndx = c.u16()?;
        let _unknown = c.u8()?;
        let _rgn_flag = c.u8()?;
        let off_x = c.i16()?;
        let off_y = c.i16()?;
        let _something_x = c.i16()?;
        let _something_y = c.i16()?;
        overlays.push(FrameOverlay {
            overlay_type,
            image_ndx,
            replace,
            offset: (off_x, off_y),
        });
    }

    Ok(Frame {
        duration,
        sound_ndx,
        exit_frame,
        branching,
        images,
        overlays,
    })
}

fn read_image(data: &[u8], offset: usize, index: usize) -> Result<Image> {
    let mut c = Cursor::at(data, offset);
    let first_byte = c.u8()?;
    let width = c.u16()?;
    let height = c.u16()?;
    let compressed = c.u8()?;
    let byte_count = c.u32()? as usize;

    if first_byte == 0 || width == 0 || height == 0 {
        return Err(Error::BadImage { index });
    }

    let payload = c.bytes(byte_count)?;
    let bits = if compressed != 0 {
        let expected = Image::expected_len(width, height);
        decode_data(payload, expected)?
    } else {
        payload.to_vec()
    };

    Ok(Image {
        index,
        width,
        height,
        bits,
    })
}
