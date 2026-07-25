//! Parser for the Microsoft Agent 2.0 compiled binary format (`.acs`).
//!
//! An `.acs` is a flat container: a 36-byte file header of four `(offset, size)` directory
//! pointers, followed by the character info block (identity, palette, voice, balloon,
//! states, localized names), the animation directory, and the image and sound tables. The
//! bulk art sits between the file header and the directories, so everything is reached
//! through those pointers rather than by walking the file. See
//! [`docs/acs-format.md`](../../../docs/acs-format.md) §1–§4.
//!
//! Images stay compressed in the file buffer and are decoded on demand by
//! [`AcsFile::image`]; sounds are borrowed straight out of it.

use crate::decode::decode_run;
use crate::error::{Error, Result};
use crate::model::*;
use crate::reader::{Cursor, StringForm};

/// First DWORD of an ACS 2.0 file.
pub const ACS_SIGNATURE: u32 = 0xABCD_ABC3;

/// The flat `.acs` writes the UTF-16 NUL terminator of every string to disk.
const FLAT_STRINGS: StringForm = StringForm::Utf16Terminated;

/// Shared message for the temporarily-stubbed frame compositor (see the clean-room note on
/// [`AcsFile::composite_frame`]).
const COMPOSITOR_STUBBED: &str =
    "the frame compositor is being reimplemented clean-room (docs/acs-format.md §3.2.1) \
     and is not yet available";

/// The character-header versions this reader accepts, from the oldest 1.x stamp to 2.0
/// (Appendix A). Files carrying anything else are either a newer format or — far more
/// often — damaged: the corrupt characters in the wild keep a valid container magic and
/// in-bounds directory offsets but hold garbage from the version dword on, so this is the
/// gate that stops the reader chasing nonsense lengths through the art.
const HEADER_VERSIONS: std::ops::RangeInclusive<u32> = 0x0001_001C..=0x0002_0001;

/// A parsed ACS 2.0 character file.
///
/// Images and sounds come either from the parsed file's own byte buffer (decoded lazily)
/// or from the pre-decoded pools of [`AcsFile::from_parts`] / [`AcsFile::from_parts_rgba`]
/// for characters assembled in memory.
pub struct AcsFile {
    pub header: FileHeader,
    pub tts: Option<Tts>,
    pub balloon: Option<Balloon>,
    pub names: Vec<Name>,
    pub states: Vec<State>,
    /// Animation names in file order (parallel to `animations`).
    pub gesture_names: Vec<String>,
    pub animations: Vec<Animation>,
    /// The file's own bytes plus its image/sound tables, for a character read from an
    /// `.acs` blob. Images are decompressed on demand and sounds borrowed in place.
    blob: Option<AcsBlob>,
    /// Pre-decoded 8-bpp image / WAV sound pools, for a character assembled in memory
    /// (also how the ACS 1.5 reader, whose art lives in separate streams, delivers its).
    images: Option<Vec<Image>>,
    sounds: Option<Vec<Vec<u8>>>,
    /// A pre-decoded **RGBA** image pool for characters built in memory from already-RGBA
    /// art (see [`AcsFile::from_parts_rgba`]) rather than an 8-bpp palette. When `Some`,
    /// [`composite_frame`](AcsFile::composite_frame) alpha-blits these directly, bypassing
    /// the palette/indexed path entirely.
    rgba_images: Option<Vec<Rgba>>,
}

/// A `(offset, size)` window into the file — the file header's directory pointers and the
/// image/sound table entries are all this shape.
#[derive(Clone, Copy, Debug)]
struct Extent {
    offset: usize,
    size: usize,
}

impl Extent {
    /// Read an `{u32 offset, u32 size}` pair and check it lies inside a `file_len`-byte file.
    fn read(cursor: &mut Cursor<'_>, file_len: usize, what: &'static str) -> Result<Extent> {
        let offset = cursor.u32()? as usize;
        let size = cursor.u32()? as usize;
        let extent = Extent { offset, size };
        if offset.checked_add(size).is_none_or(|end| end > file_len) {
            return Err(Error::InvalidData(format!(
                "{what} at {offset}+{size} runs past the end of the {file_len}-byte file"
            )));
        }
        Ok(extent)
    }

    fn end(&self) -> usize {
        self.offset + self.size
    }
}

/// The image and sound tables of a file-backed character, over the file's own bytes.
struct AcsBlob {
    bytes: Vec<u8>,
    images: Vec<Extent>,
    sounds: Vec<Extent>,
    /// Palette index to pad a short/corrupt image decode with (the color key).
    transparency: u8,
}

impl AcsBlob {
    /// Decompress image `index` into 8-bpp palette indices.
    ///
    /// A record can legitimately be marked absent (a fully transparent placeholder), which
    /// yields an image with no bits; the compositor treats missing pixels as transparent.
    /// A decode that comes up short is padded with the color key rather than failing the
    /// whole frame — a handful of third-party characters ship individually damaged art.
    fn image(&self, index: usize) -> Result<Image> {
        let extent = *self.images.get(index).ok_or(Error::BadImage { index })?;
        let mut cursor = Cursor::at(&self.bytes, extent.offset);
        let mut image = Image {
            index,
            width: 0,
            height: 0,
            bits: Vec::new(),
        };
        if cursor.u8()? == 0 {
            return Ok(image);
        }
        image.width = cursor.u16()?;
        image.height = cursor.u16()?;
        let compressed = cursor.u8()? != 0;
        let stored_len = cursor.u32()? as usize;
        let raster_len = Image::expected_len(image.width, image.height);

        image.bits = if compressed {
            let available = extent.end().saturating_sub(cursor.pos());
            let mut packed = cursor.bytes(stored_len.min(available))?.to_vec();
            // The decoder ends on the stream's own terminator and expects to be able to
            // read a little past it (the file pads with 0xFF); the region data that
            // follows the pixels on disk is not part of the stream, so pad rather than
            // hand it bytes that belong to the next structure.
            packed.extend_from_slice(&[0xFF; 8]);
            let mut bits = decode_run(&packed, raster_len);
            if !bits.is_empty() && bits.len() < raster_len {
                bits.resize(raster_len, self.transparency);
            }
            bits
        } else {
            cursor.bytes(raster_len)?.to_vec()
        };
        Ok(image)
    }

    /// Borrow sound `index` (a complete RIFF/WAVE file) out of the file buffer.
    fn sound(&self, index: usize) -> Option<&[u8]> {
        let extent = self.sounds.get(index)?;
        self.bytes.get(extent.offset..extent.end())
    }
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
    ///
    /// Both generations are accepted: a flat 2.0 file is read here, an OLE2 compound
    /// document is handed to the [1.5 reader](crate::acs_v15), which normalizes it into the
    /// same shape.
    pub fn parse(data: Vec<u8>) -> Result<AcsFile> {
        let sig = signature(&data).ok_or(Error::UnexpectedEof {
            context: "signature",
            offset: 0,
            needed: 4,
            available: data.len(),
        })?;
        match sig {
            ACS_SIGNATURE => AcsFile::parse_flat(data),
            crate::acs_v15::OLE2_SIGNATURE => crate::acs_v15::parse_v15(data),
            found => Err(Error::BadSignature { found }),
        }
    }

    /// Read the flat 2.0 container: four directory pointers, then the blocks they name.
    fn parse_flat(data: Vec<u8>) -> Result<AcsFile> {
        let file_len = data.len();
        let mut dir = Cursor::at(&data, 4);
        let char_info = Extent::read(&mut dir, file_len, "character info block")?;
        let animations = Extent::read(&mut dir, file_len, "animation directory")?;
        let images = Extent::read(&mut dir, file_len, "image directory")?;
        let sounds = Extent::read(&mut dir, file_len, "sound directory")?;

        let info = read_char_info(&data, char_info)?;
        let animations = read_animations(&data, animations)?;
        let (gesture_names, animations) = animations.into_iter().unzip();

        let transparency = info.header.transparency;
        Ok(AcsFile {
            header: info.header,
            tts: info.tts,
            balloon: info.balloon,
            names: info.names,
            states: info.states,
            gesture_names,
            animations,
            blob: Some(AcsBlob {
                images: read_media_table(&data, images, "image directory")?,
                sounds: read_media_table(&data, sounds, "sound directory")?,
                bytes: data,
                transparency,
            }),
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
            header,
            tts,
            balloon,
            names,
            states,
            gesture_names,
            animations,
            blob: None,
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
            header,
            tts,
            balloon,
            names,
            states,
            gesture_names,
            animations,
            blob: None,
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
        if let Some(blob) = &self.blob {
            return blob.images.len();
        }
        self.images.as_ref().map_or(0, Vec::len)
    }

    /// Fetch image `index` as 8-bpp palette indices, decompressing it from the file on
    /// demand (or cloning it out of a pre-decoded pool).
    ///
    /// Not available for a character built by [`from_parts_rgba`](AcsFile::from_parts_rgba):
    /// its art never had a palette to index into.
    pub fn image(&self, index: usize) -> Result<Image> {
        if let Some(images) = &self.images {
            return images.get(index).cloned().ok_or(Error::BadImage { index });
        }
        match &self.blob {
            Some(blob) => blob.image(index),
            None => Err(Error::Unsupported(
                "this character was built from RGBA art; it has no palette-indexed images",
            )),
        }
    }

    /// Number of sounds in the sound table.
    pub fn sound_count(&self) -> usize {
        if let Some(blob) = &self.blob {
            return blob.sounds.len();
        }
        self.sounds.as_ref().map_or(0, Vec::len)
    }

    /// Borrow the raw bytes of sound `index` (a complete standalone WAV file).
    pub fn sound(&self, index: usize) -> Option<&[u8]> {
        match &self.blob {
            Some(blob) => blob.sound(index),
            None => self.sounds.as_ref()?.get(index).map(|v| v.as_slice()),
        }
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

    /// Composite one frame into a top-down palette-indexed image the size of the character.
    ///
    /// **Temporarily stubbed.** The compositor was carried over from the pre-relicense
    /// GPL-derived code and is being reimplemented clean-room from the layering
    /// rules in `docs/acs-format.md` §3.2.1; it currently returns [`Error::Unsupported`].
    pub fn composite_frame_indexed(
        &self,
        frame: &Frame,
        mouth: Option<MouthOverlay>,
    ) -> Result<Indexed> {
        let _ = (frame, mouth);
        Err(Error::Unsupported(COMPOSITOR_STUBBED))
    }

    /// Composite one frame to top-down RGBA (transparency index → transparent pixel).
    ///
    /// **Temporarily stubbed** during the clean-room rewrite of the compositor (see
    /// [`composite_frame_indexed`](AcsFile::composite_frame_indexed)).
    pub fn composite_frame(&self, frame: &Frame, mouth: Option<MouthOverlay>) -> Result<Rgba> {
        let _ = (frame, mouth);
        Err(Error::Unsupported(COMPOSITOR_STUBBED))
    }
}

/// Everything the character info block carries (§2), before it is split across [`AcsFile`].
struct CharInfo {
    header: FileHeader,
    tts: Option<Tts>,
    balloon: Option<Balloon>,
    names: Vec<Name>,
    states: Vec<State>,
}

/// Read the character info block: fixed header, then the optional voice and balloon
/// sub-blocks, the palette, the tray icon, the state map, and the localized name table.
///
/// The sub-blocks are variable-length and packed, so this is a single forward walk. It runs
/// over the whole file rather than a slice of the block: some authoring tools omit the
/// terminator of the very last string, and the original memory-mapped loader read that
/// terminator out of the zero-filled tail of the mapping (§F.3).
fn read_char_info(data: &[u8], block: Extent) -> Result<CharInfo> {
    let mut c = Cursor::at(data, block.offset);
    let version_minor = c.u16()?;
    let version_major = c.u16()?;
    let version = ((version_major as u32) << 16) | version_minor as u32;
    check_header_version(version)?;
    // The localized name table is also reachable by this pointer; we walk to it instead,
    // which keeps damaged files from sending the reader off into the middle of the art.
    c.skip(8)?;
    let guid = c.guid()?;
    let width = c.u16()?;
    let height = c.u16()?;
    let transparency = c.u8()?;
    let style = c.u32()?;
    c.skip(4)?; // reserved; 0x00000002 in every sampled character

    let tts = (style & char_style::TTS != 0)
        .then(|| read_tts(&mut c, FLAT_STRINGS))
        .transpose()?;
    let balloon = (style & char_style::BALLOON != 0)
        .then(|| read_balloon(&mut c, FLAT_STRINGS, true))
        .transpose()?;

    let palette = read_palette(&mut c)?;
    read_tray_icon(&mut c)?;
    let states = read_states(&mut c, FLAT_STRINGS)?;
    let names = read_names(&mut c, FLAT_STRINGS)?;
    if c.pos() > block.end() + 2 {
        return Err(Error::InvalidData(format!(
            "character info block overran its {} bytes by {}",
            block.size,
            c.pos() - block.end()
        )));
    }

    Ok(CharInfo {
        header: FileHeader {
            version_major,
            version_minor,
            guid,
            image_size: (width, height),
            transparency,
            style,
            palette,
        },
        tts,
        balloon,
        names,
        states,
    })
}

/// Reject a character header whose version stamp is outside [`HEADER_VERSIONS`].
pub(crate) fn check_header_version(version: u32) -> Result<()> {
    if !HEADER_VERSIONS.contains(&version) {
        return Err(Error::InvalidData(format!(
            "character header version 0x{version:08X} is not one this reader supports"
        )));
    }
    Ok(())
}

/// Read the voice block (§2.2): the SAPI engine and mode ids plus the speaking voice's
/// numeric settings, and — when the extension flag is set — its language and display name.
pub(crate) fn read_tts(c: &mut Cursor<'_>, form: StringForm) -> Result<Tts> {
    let engine = c.guid()?;
    let mode = c.guid()?;
    let speed = c.i32()?;
    let pitch = c.u16()? as i16;
    let extended = c.u8()? != 0;
    let mut tts = Tts {
        engine,
        mode,
        speed,
        pitch,
        language: None,
        gender: 0,
        age: 0,
        style: String::new(),
    };
    if extended {
        tts.language = Some(c.u16()?);
        // The language's display name — empty in Microsoft's own characters (which is what
        // makes their voice tail look like a fixed 10-byte prefix), but a real string like
        // "US English" in many third-party ones.
        let _language_name = c.text(form)?;
        tts.gender = c.u16()?;
        tts.age = c.u16()?;
        tts.style = c.text(form)?;
    }
    Ok(tts)
}

/// Read the word-balloon block (§2.3): sizing, colors, and the `LOGFONT` metrics of the
/// text face.
///
/// The trailing `LOGFONT` style flags are `lfItalic` and `lfStrikeOut`; the older header
/// versions of the split/1.5 formats stop after the first of them, hence `strikeout_flag`.
pub(crate) fn read_balloon(
    c: &mut Cursor<'_>,
    form: StringForm,
    strikeout_flag: bool,
) -> Result<Balloon> {
    let lines = c.u8()?;
    let per_line = c.u8()?;
    let fg_color = c.colorref()?;
    let bg_color = c.colorref()?;
    let border_color = c.colorref()?;
    let font_name = c.text(form)?;
    let font_height = c.i32()?;
    let weight = c.u32()?;
    let italic = c.u8()? != 0;
    let strikeout = if strikeout_flag { c.u8()? != 0 } else { false };
    Ok(Balloon {
        lines,
        per_line,
        fg_color,
        bg_color,
        border_color,
        font_name,
        font_height,
        // LOGFONT: anything heavier than normal reads as bold.
        bold: weight > 400,
        italic,
        strikeout,
    })
}

/// Read the character's global palette (§2.4). The count is authoritative — there is no
/// `0` ⇒ 256 sentinel here, unlike the older Actor format.
pub(crate) fn read_palette(c: &mut Cursor<'_>) -> Result<Vec<Color>> {
    let count = c.u32()? as usize;
    let mut palette = Vec::with_capacity(count.min(256));
    for _ in 0..count {
        palette.push(c.color()?);
    }
    Ok(palette)
}

/// Step over the optional tray icon (§2.5): a color DIB and its AND mask, neither of which
/// anything in this crate draws.
pub(crate) fn read_tray_icon(c: &mut Cursor<'_>) -> Result<()> {
    if c.u8()? != 0 {
        let color_len = c.u32()? as usize;
        c.skip(color_len)?;
        let mask_len = c.u32()? as usize;
        c.skip(mask_len)?;
    }
    Ok(())
}

/// Read the state → animation-names map (§2.6).
pub(crate) fn read_states(c: &mut Cursor<'_>, form: StringForm) -> Result<Vec<State>> {
    let count = c.u16()? as usize;
    let mut states = Vec::with_capacity(count);
    for _ in 0..count {
        let name = c.text(form)?;
        let anim_count = c.u16()? as usize;
        let mut animations = Vec::with_capacity(anim_count);
        for _ in 0..anim_count {
            animations.push(c.text(form)?);
        }
        states.push(State { name, animations });
    }
    Ok(states)
}

/// Read the per-locale name/description table (§2.7), which ends the character info block.
pub(crate) fn read_names(c: &mut Cursor<'_>, form: StringForm) -> Result<Vec<Name>> {
    let count = c.u16()? as usize;
    let mut names = Vec::with_capacity(count);
    for _ in 0..count {
        let language = c.u16()?;
        let name = capitalized(&c.text(form)?);
        let desc1 = c.text(form)?;
        let desc2 = c.text(form)?;
        names.push(Name {
            language,
            name,
            desc1,
            desc2,
        });
    }
    Ok(names)
}

/// Character names are displayed as-authored except for their first letter, which the
/// engine presents capitalized ("genie" → "Genie").
pub(crate) fn capitalized(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Read the animation directory (§1.2) and each animation block it points at, returning
/// the directory names paired with the parsed animations.
fn read_animations(data: &[u8], block: Extent) -> Result<Vec<(String, Animation)>> {
    let mut dir = Cursor::at(data, block.offset);
    let count = dir.u32()? as usize;
    let mut animations = Vec::with_capacity(count.min(block.size / 12));
    for _ in 0..count {
        let name = dir.text(FLAT_STRINGS)?;
        let extent = Extent::read(&mut dir, data.len(), "animation block")?;
        animations.push((name, read_animation(data, extent)?));
    }
    Ok(animations)
}

/// Read one animation block (§3.1): its own name, how it returns to rest, and its frames.
fn read_animation(data: &[u8], block: Extent) -> Result<Animation> {
    let mut c = Cursor::at(data, block.offset);
    let name = c.text(FLAT_STRINGS)?;
    let return_kind = ReturnKind::from_u8(c.u8()?);
    let return_name = c.text(FLAT_STRINGS)?;
    let frame_count = c.u16()? as usize;
    let mut frames = Vec::with_capacity(frame_count);
    for _ in 0..frame_count {
        frames.push(read_frame(&mut c)?);
    }
    if c.pos() > block.end() {
        return Err(Error::InvalidData(format!(
            "animation {name:?} overran its {} bytes",
            block.size
        )));
    }
    Ok(Animation {
        name,
        return_kind,
        return_name,
        frames,
    })
}

/// Read one frame record (§3.2): its image layers, sound, hold time, wind-down target,
/// branch table, and lip-sync mouth overlays.
fn read_frame(c: &mut Cursor<'_>) -> Result<Frame> {
    let image_count = c.u16()? as usize;
    let mut images = Vec::with_capacity(image_count);
    for _ in 0..image_count {
        images.push(FrameImage {
            image_ndx: c.u32()?,
            offset: (c.i16()?, c.i16()?),
        });
    }
    let sound_ndx = c.i16()?;
    let duration = c.u16()?;
    let exit_frame = c.i16()?;

    let branch_count = c.u8()? as usize;
    let mut branching = Vec::with_capacity(branch_count);
    for _ in 0..branch_count {
        branching.push(Branch {
            frame_ndx: c.i16()?,
            probability: c.u16()?,
        });
    }

    let overlay_count = c.u8()? as usize;
    let mut overlays = Vec::with_capacity(overlay_count);
    for _ in 0..overlay_count {
        let overlay_type = MouthOverlay::from_u8(c.u8()?);
        let replace = c.u8()? != 0;
        let image_ndx = c.u32()? as u16;
        let offset = (c.i16()?, c.i16()?);
        c.skip(4)?; // per-overlay region origin; 0 in every sampled character
        overlays.push(FrameOverlay {
            overlay_type,
            image_ndx,
            replace,
            offset,
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

/// Read an image or sound table (§1.3/§1.4): a count followed by `{offset, size, checksum}`
/// entries. The checksums are a caching aid the reader does not need.
fn read_media_table(data: &[u8], block: Extent, what: &'static str) -> Result<Vec<Extent>> {
    let mut c = Cursor::at(data, block.offset);
    let count = c.u32()? as usize;
    let mut entries = Vec::with_capacity(count.min(block.size / 12));
    for _ in 0..count {
        entries.push(Extent::read(&mut c, data.len(), what)?);
        c.skip(4)?;
    }
    Ok(entries)
}
