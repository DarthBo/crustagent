//! Parser for the Microsoft Agent 2.0 compiled binary format (`.acs`).
//!
//! Reverse-engineered from the original compiled binary format.

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
    /// Pre-decoded image/sound pools (ACS 1.5, whose data comes from OLE2 streams rather
    /// than a single mmap'd blob). When `Some`, they take precedence over the lazy
    /// index-into-`data` path used by ACS 2.0.
    images: Option<Vec<Image>>,
    sounds: Option<Vec<Vec<u8>>>,
    /// A pre-decoded **RGBA** image pool for characters built in memory from already-RGBA
    /// art (see [`AcsFile::from_parts_rgba`]) rather than an 8-bpp palette. When `Some`,
    /// [`composite_frame`](AcsFile::composite_frame) alpha-blits these directly, bypassing
    /// the palette/indexed path entirely.
    rgba_images: Option<Vec<Rgba>>,
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
        if sig == crate::acs_v15::OLE2_SIGNATURE {
            return crate::acs_v15::parse_v15(data);
        }
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
            images: None,
            sounds: None,
            rgba_images: None,
        })
    }

    /// Assemble an [`AcsFile`] from already-decoded parts (used by the ACS 1.5 reader,
    /// which pulls images/sounds out of OLE2 streams rather than a flat blob).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_v15(
        header: FileHeader,
        tts: Option<Tts>,
        balloon: Option<Balloon>,
        names: Vec<Name>,
        states: Vec<State>,
        gesture_names: Vec<String>,
        animations: Vec<Animation>,
        images: Vec<Image>,
        sounds: Vec<Vec<u8>>,
    ) -> AcsFile {
        AcsFile {
            data: Vec::new(),
            header,
            tts,
            balloon,
            names,
            states,
            gesture_names,
            animations,
            image_index: Vec::new(),
            sound_index: Vec::new(),
            images: Some(images),
            sounds: Some(sounds),
            rgba_images: None,
        }
    }

    /// Assemble an [`AcsFile`] from already-decoded parts with an **8-bpp palette-indexed**
    /// image pool — the public, in-memory equivalent of a parsed file. Use this to build a
    /// character programmatically (e.g. a synthetic or app-supplied character) that flows
    /// through the same [`Agent`](../crustagent/struct.Agent.html)/compositor path as a real
    /// `.acs`. For already-RGBA art, prefer [`from_parts_rgba`](AcsFile::from_parts_rgba).
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        header: FileHeader,
        tts: Option<Tts>,
        balloon: Option<Balloon>,
        names: Vec<Name>,
        states: Vec<State>,
        gesture_names: Vec<String>,
        animations: Vec<Animation>,
        images: Vec<Image>,
        sounds: Vec<Vec<u8>>,
    ) -> AcsFile {
        AcsFile::from_v15(
            header,
            tts,
            balloon,
            names,
            states,
            gesture_names,
            animations,
            images,
            sounds,
        )
    }

    /// Assemble an [`AcsFile`] from already-decoded parts with an **RGBA** image pool. Each
    /// [`FrameImage::image_ndx`] then indexes `images` (this RGBA pool), and
    /// [`composite_frame`](AcsFile::composite_frame) alpha-blits them directly — so
    /// anti-aliased, soft-alpha art stays crisp (no palette quantization, no 1-bit
    /// transparency key). The palette-indexed helpers ([`image`](AcsFile::image),
    /// [`composite_frame_indexed`](AcsFile::composite_frame_indexed)) are not available on a
    /// file built this way. `header.image_size` sets the canvas size; `header.palette`/
    /// `transparency` are unused.
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts_rgba(
        header: FileHeader,
        tts: Option<Tts>,
        balloon: Option<Balloon>,
        names: Vec<Name>,
        states: Vec<State>,
        gesture_names: Vec<String>,
        animations: Vec<Animation>,
        images: Vec<Rgba>,
        sounds: Vec<Vec<u8>>,
    ) -> AcsFile {
        AcsFile {
            data: Vec::new(),
            header,
            tts,
            balloon,
            names,
            states,
            gesture_names,
            animations,
            image_index: Vec::new(),
            sound_index: Vec::new(),
            images: None,
            sounds: Some(sounds),
            rgba_images: Some(images),
        }
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
        if let Some(rgba) = &self.rgba_images {
            return rgba.len();
        }
        match &self.images {
            Some(imgs) => imgs.len(),
            None => self.image_index.len(),
        }
    }

    /// Decode image `index` to its 8-bpp palette-index bits.
    pub fn image(&self, index: usize) -> Result<Image> {
        if let Some(imgs) = &self.images {
            return imgs.get(index).cloned().ok_or(Error::BadImage { index });
        }
        let blk = self
            .image_index
            .get(index)
            .copied()
            .ok_or(Error::BadImage { index })?;
        read_image(&self.data, blk.offset, index, self.header.transparency)
    }

    /// Number of sounds in the sound table.
    pub fn sound_count(&self) -> usize {
        match &self.sounds {
            Some(snds) => snds.len(),
            None => self.sound_index.len(),
        }
    }

    /// Borrow the raw bytes of sound `index` (a complete standalone WAV file).
    pub fn sound(&self, index: usize) -> Option<&[u8]> {
        if let Some(snds) = &self.sounds {
            return snds.get(index).map(|v| v.as_slice());
        }
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
    ///
    /// For a file built via [`from_parts_rgba`](AcsFile::from_parts_rgba) this alpha-blits
    /// the RGBA pool directly (source-over); otherwise it composites through the 8-bpp
    /// palette path and maps the result through the palette.
    pub fn composite_frame(&self, frame: &Frame, mouth: Option<MouthOverlay>) -> Result<Rgba> {
        if let Some(pool) = &self.rgba_images {
            return self.composite_frame_rgba(frame, mouth, pool);
        }
        Ok(self
            .composite_frame_indexed(frame, mouth)?
            .to_rgba(&self.header.palette))
    }

    /// RGBA compositing (mirrors [`composite_frame_indexed`]'s layering, but source-over on
    /// true RGBA instead of index-keying): base images back-to-front (highest index is the
    /// bottom layer), then the matching mouth overlay on top; a `replace` overlay suppresses
    /// base image 0. `image_ndx` indexes the RGBA `pool`.
    fn composite_frame_rgba(
        &self,
        frame: &Frame,
        mouth: Option<MouthOverlay>,
        pool: &[Rgba],
    ) -> Result<Rgba> {
        let (w, h) = self.header.image_size;
        let mut canvas = Rgba::transparent(w as u32, h as u32);

        let overlay = mouth.and_then(|m| frame.overlays.iter().find(|o| o.overlay_type == m));
        let replace_base = overlay.is_some_and(|o| o.replace);

        for i in (0..frame.images.len()).rev() {
            if i == 0 && replace_base {
                continue;
            }
            let fi = frame.images[i];
            let src = pool.get(fi.image_ndx as usize).ok_or(Error::BadImage {
                index: fi.image_ndx as usize,
            })?;
            alpha_over(&mut canvas, src, fi.offset);
        }

        if let Some(o) = overlay {
            let src = pool.get(o.image_ndx as usize).ok_or(Error::BadImage {
                index: o.image_ndx as usize,
            })?;
            alpha_over(&mut canvas, src, o.offset);
        }

        Ok(canvas)
    }

    /// Blit one 8-bpp image onto the top-down `Indexed` canvas at `offset`, skipping
    /// transparent-index pixels.
    ///
    /// `offset` is the image's top-left position in **top-down** canvas space (matching
    /// the original compositor, where a source pixel at visual row `v` lands at canvas
    /// row `v + offset.y`). The image bits are a bottom-up DIB, so visual row `v` is stored
    /// at scanline `height-1-v`. (Full-frame images use `offset ≈ (0,0)`, but smaller
    /// sub-images — e.g. a separate head layer — depend on this being top-down.)
    fn blit(&self, canvas: &mut Indexed, img: &Image, offset: (i16, i16)) {
        let transparency = self.header.transparency;
        let stride = img.stride();
        let cw = canvas.width as i32;
        let ch = canvas.height as i32;
        let (off_x, off_y) = (offset.0 as i32, offset.1 as i32);

        for v in 0..img.height as i32 {
            let cy = v + off_y; // top-down canvas row
            if cy < 0 || cy >= ch {
                continue;
            }
            let src_row = (img.height as i32 - 1 - v) as usize * stride; // bottom-up scanline
            for u in 0..img.width as i32 {
                let cx = u + off_x;
                if cx < 0 || cx >= cw {
                    continue;
                }
                // Tolerate empty/truncated image data (some characters ship a 0-byte
                // placeholder image): treat missing source pixels as transparent.
                let Some(&idx) = img.bits.get(src_row + u as usize) else {
                    continue;
                };
                if idx == transparency {
                    continue;
                }
                canvas.indices[cy as usize * canvas.width as usize + cx as usize] = idx;
            }
        }
    }
}

/// Source-over composite a top-down RGBA `src` onto a top-down RGBA `canvas` at `offset`
/// (non-premultiplied straight alpha). Pixels outside the canvas are clipped.
fn alpha_over(canvas: &mut Rgba, src: &Rgba, offset: (i16, i16)) {
    let cw = canvas.width as i32;
    let ch = canvas.height as i32;
    let (off_x, off_y) = (offset.0 as i32, offset.1 as i32);

    for v in 0..src.height as i32 {
        let cy = v + off_y;
        if cy < 0 || cy >= ch {
            continue;
        }
        for u in 0..src.width as i32 {
            let cx = u + off_x;
            if cx < 0 || cx >= cw {
                continue;
            }
            let s = ((v * src.width as i32 + u) as usize) * 4;
            let sa = src.pixels[s + 3] as u32;
            if sa == 0 {
                continue;
            }
            let d = ((cy * cw + cx) as usize) * 4;
            if sa == 255 {
                canvas.pixels[d..d + 4].copy_from_slice(&src.pixels[s..s + 4]);
                continue;
            }
            // out = src + dst * (1 - src_a), straight alpha.
            let da = canvas.pixels[d + 3] as u32;
            let inv = 255 - sa;
            let out_a = sa + da * inv / 255;
            for k in 0..3 {
                let sc = src.pixels[s + k] as u32;
                let dc = canvas.pixels[d + k] as u32;
                // Composite in straight-alpha space; guard the zero-alpha case.
                let num = sc * sa + dc * da * inv / 255;
                canvas.pixels[d + k] = if out_a == 0 { 0 } else { (num / out_a) as u8 };
            }
            canvas.pixels[d + 3] = out_a as u8;
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
    // The character-info block should open with a small version (Agent 2.x). A wild value
    // means the block isn't the plain layout we expect — in practice a handful of
    // third-party files ship this region encrypted/obfuscated. Fail clearly rather than
    // cascading into a bogus multi-gigabyte string length.
    if version_major == 0 || version_major > 99 {
        return Err(Error::InvalidData(format!(
            "character-info block is unreadable (version reads as {version_major}.{version_minor}); \
             it appears encrypted or is an unsupported variant"
        )));
    }
    let names_offset_abs = c.u32()? as usize;
    let _names_size = c.u32()?;
    let guid = c.guid()?;
    let width = c.u16()?;
    let height = c.u16()?;
    let transparency = c.u8()?;
    let style = c.u32()?;
    let _unknown = c.u32()?; // always 2

    let tts = if style & char_style::TTS != 0 {
        Some(crate::blocks::tts(&mut c, true)?)
    } else {
        None
    };
    let balloon = if style & char_style::BALLOON != 0 {
        Some(crate::blocks::balloon(&mut c, true)?)
    } else {
        None
    };
    let palette = crate::blocks::palette(&mut c)?;
    crate::blocks::skip_icon(&mut c)?;

    // States occupy the cursor position through the start of names.
    let states = crate::blocks::states(&mut c, true)?;

    // Names live at an absolute file offset; rebase into the header block.
    let names_offset = names_offset_abs
        .checked_sub(blk.offset)
        .ok_or_else(|| Error::InvalidData("names offset precedes header block".into()))?;
    let names = {
        let mut nc = Cursor::at(block, names_offset);
        crate::blocks::names(&mut nc, true)?
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
    let slice = data
        .get(offset..end.min(data.len()))
        .ok_or(Error::UnexpectedEof {
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

fn read_image(data: &[u8], offset: usize, index: usize, transparency: u8) -> Result<Image> {
    // A blank, zero-size image — the graceful result for empty/placeholder or malformed
    // slots (some characters ship 1-byte image entries). Composites to nothing.
    let blank = || Image {
        index,
        width: 0,
        height: 0,
        bits: Vec::new(),
    };

    let mut c = Cursor::at(data, offset);
    let first_byte = c.u8()?;
    let width = c.u16()?;
    let height = c.u16()?;
    let compressed = c.u8()?;
    let byte_count = c.u32()? as usize;

    // Placeholder/empty slot (leading flag byte 0, or degenerate dimensions).
    if first_byte == 0 || width == 0 || height == 0 {
        return Ok(blank());
    }
    // Truncated/garbage record whose payload runs past the file.
    let Ok(payload) = c.bytes(byte_count) else {
        return Ok(blank());
    };

    let expected = Image::expected_len(width, height);
    let mut bits = if compressed != 0 {
        crate::decode::decode_run(payload, expected)
    } else {
        payload.to_vec()
    };
    // A stream that ends early (or an under-long uncompressed record) is padded with the
    // transparent index; the original engine likewise tolerates a short decode.
    bits.resize(expected, transparency);

    Ok(Image {
        index,
        width,
        height,
        bits,
    })
}
