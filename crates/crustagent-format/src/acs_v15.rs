//! ACS 1.5 reader — the older Microsoft Agent format, an **OLE2 compound document**
//! (Structured Storage) rather than the flat blob of ACS 2.0
//! ([`docs/acs-format.md`](../../../docs/acs-format.md) §6).
//!
//! The root storage holds a compressed character definition in `char.acf` plus one stream
//! per animation. The definition carries the same logical blocks as the 2.0 header — with
//! ANSI name/description strings, no on-disk string terminators, and no tray icon — while
//! each animation stream carries its own frames *and* their artwork, since 1.5 has no
//! shared image table.
//!
//! Everything is normalized into the same [`AcsFile`] the 2.0 path produces, so the rest of
//! the crate stays oblivious to which generation a character came from: each frame's inline
//! bitmap becomes an entry in a synthesized image pool.

use std::io::Read;

use crate::acs::{check_header_version, read_balloon, read_palette, read_states, AcsFile};
use crate::decode::{decode_data, decode_run};
use crate::error::{Error, Result};
use crate::model::*;
use crate::reader::{Cursor, StringForm};

/// First 4 bytes of an OLE2 compound document (`D0 CF 11 E0` little-endian).
pub const OLE2_SIGNATURE: u32 = 0xE011_CFD0;
/// Signature DWORD at the head of the decompressed `char.acf` header stream.
pub const ACS_V15_HEADER_SIGNATURE: u32 = 0xABCD_ABC1;

/// The 1.5 header spells its animation/state names in UTF-16 with no terminator on disk.
const V15_STRINGS: StringForm = StringForm::Utf16;

/// Feature boundary (Appendix A): from here up, an animation entry carries an id and its
/// stream leads with a version + that id.
const VERSION_WITH_ANIM_IDS: u32 = 0x0001_001D;
/// Above this, the balloon block carries a second style byte.
const VERSION_WITH_BALLOON_STYLE: u32 = 0x0001_001E;

/// One animation as named by the character definition.
struct AnimationRef {
    name: String,
    stream: String,
    return_name: String,
}

/// Parse an ACS 1.5 (`OLE2`) character.
pub fn parse_v15(bytes: Vec<u8>) -> Result<AcsFile> {
    let container = std::io::Cursor::new(bytes);
    let mut ole = cfb::CompoundFile::open(container)
        .map_err(|e| Error::InvalidData(format!("not a readable compound document: {e}")))?;

    let definition = inflate_stream(&stream_bytes(&mut ole, "char.acf")?)?;
    let header = read_definition(&definition)?;

    let mut pool = MediaPool {
        images: Vec::new(),
        sounds: Vec::new(),
        frame_size: header.header.image_size,
        transparency: header.header.transparency,
    };
    let mut animations = Vec::with_capacity(header.animations.len());
    let mut gesture_names = Vec::with_capacity(header.animations.len());
    for entry in &header.animations {
        let raw = stream_bytes(&mut ole, &entry.stream)?;
        animations.push(read_animation_stream(
            &raw,
            entry,
            header.version,
            &mut pool,
        )?);
        gesture_names.push(entry.name.clone());
    }

    Ok(AcsFile::from_v15(
        header.header,
        header.tts,
        header.balloon,
        header.names,
        header.states,
        gesture_names,
        animations,
        pool.images,
        pool.sounds,
    ))
}

/// Read a whole stream out of a compound document by name.
pub(crate) fn read_stream(container: &[u8], name: &str) -> Result<Vec<u8>> {
    let mut ole = cfb::CompoundFile::open(std::io::Cursor::new(container))
        .map_err(|e| Error::InvalidData(format!("not a readable compound document: {e}")))?;
    stream_bytes(&mut ole, name)
}

fn stream_bytes<F: Read + std::io::Seek>(
    ole: &mut cfb::CompoundFile<F>,
    name: &str,
) -> Result<Vec<u8>> {
    let mut stream = ole
        .open_stream(name)
        .map_err(|e| Error::InvalidData(format!("no {name:?} stream: {e}")))?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Unwrap the `char.acf` envelope: signature, the two sizes, then the LZ payload (§6.2).
fn inflate_stream(stream: &[u8]) -> Result<Vec<u8>> {
    let mut c = Cursor::new(stream);
    let signature = c.u32()?;
    if signature != ACS_V15_HEADER_SIGNATURE {
        return Err(Error::BadSignature { found: signature });
    }
    let inflated_len = c.u32()? as usize;
    let packed_len = c.u32()? as usize;
    decode_data(c.bytes(packed_len)?, inflated_len)
}

/// The decompressed `char.acf` definition.
struct Definition {
    version: u32,
    /// How much of the definition the walk consumed; a correct reading lands on its end.
    consumed: usize,
    header: FileHeader,
    tts: Option<Tts>,
    balloon: Option<Balloon>,
    names: Vec<Name>,
    states: Vec<State>,
    animations: Vec<AnimationRef>,
}

/// Read the decompressed character definition, trying both spellings of its identity
/// strings.
///
/// The character's name and description are ANSI in Microsoft's own 1.5 characters and
/// UTF-16 in some third-party ones, with nothing in the header to say which. Both are read;
/// a correct reading consumes the definition exactly, which settles it.
fn read_definition(payload: &[u8]) -> Result<Definition> {
    let mut fallback = None;
    for form in [StringForm::Ansi, StringForm::Utf16] {
        match read_definition_as(payload, form) {
            Ok(def) if def.consumed == payload.len() => return Ok(def),
            Ok(def) => fallback = fallback.or(Some(Ok(def))),
            Err(e) => fallback = fallback.or(Some(Err(e))),
        }
    }
    fallback.unwrap_or_else(|| {
        Err(Error::InvalidData(
            "unreadable 1.5 character definition".into(),
        ))
    })
}

/// Read the definition (§6.2) with `identity` as the spelling of its name/description
/// strings. Its blocks are the familiar ones, but the animation list comes first and the
/// name/description are single strings rather than the 2.0 per-locale table.
fn read_definition_as(payload: &[u8], identity: StringForm) -> Result<Definition> {
    let mut c = Cursor::new(payload);
    let version = c.u32()?;
    check_header_version(version)?;
    let version_major = (version >> 16) as u16;
    let version_minor = version as u16;

    let anim_count = c.u16()? as usize;
    let mut animations = Vec::with_capacity(anim_count);
    for _ in 0..anim_count {
        let name = c.text(V15_STRINGS)?;
        let stream = c.text(V15_STRINGS)?;
        let return_name = c.text(V15_STRINGS)?;
        if version > VERSION_WITH_ANIM_IDS {
            c.skip(4)?; // the id the animation stream echoes back
        }
        animations.push(AnimationRef {
            name,
            stream,
            return_name,
        });
    }

    let guid = c.guid()?;
    let name = crate::acs::capitalized(&c.text(identity)?);
    let desc1 = c.text(identity)?;
    let desc2 = c.text(identity)?;
    let width = c.u16()?;
    let height = c.u16()?;
    let transparency = c.u8()?;
    let style = c.u32()?;

    let tts = (style & char_style::TTS != 0)
        .then(|| read_v15_tts(&mut c))
        .transpose()?;
    let balloon = (style & char_style::BALLOON != 0)
        .then(|| read_balloon(&mut c, identity, version > VERSION_WITH_BALLOON_STYLE))
        .transpose()?;
    let palette = read_palette(&mut c)?;
    let states = read_states(&mut c, V15_STRINGS)?;

    Ok(Definition {
        version,
        consumed: c.pos(),
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
        // 1.5 stores one name/description, so it becomes the single locale-neutral entry.
        names: vec![Name {
            language: 0,
            name,
            desc1,
            desc2,
        }],
        states,
        animations,
    })
}

/// The 1.5 voice block: the two SAPI ids and the numeric settings, without the language and
/// display-name tail the 2.0 header added (§6.2 field 11).
fn read_v15_tts(c: &mut Cursor<'_>) -> Result<Tts> {
    Ok(Tts {
        engine: c.guid()?,
        mode: c.guid()?,
        speed: c.i32()?,
        pitch: c.u16()? as i16,
        language: None,
        gender: 0,
        age: 0,
        style: String::new(),
    })
}

/// The character-wide image and sound pools the 2.0 model expects, filled in as the
/// per-animation streams are read (1.5 stores its art and audio inside each animation).
struct MediaPool {
    images: Vec<Image>,
    sounds: Vec<Vec<u8>>,
    frame_size: (u16, u16),
    /// The character's color key, used to pad a raster that does not fill its rows.
    transparency: u8,
}

impl MediaPool {
    /// Add a full-frame raster and return its index. The stream states only the byte count,
    /// so the height is however many rows those bytes cover at the character's frame width,
    /// rounded up — a raster that stops mid-row keeps the partial one.
    fn push_frame_image(&mut self, bits: Vec<u8>) -> u32 {
        let width = self.frame_size.0;
        let stride = (width as usize).div_ceil(4) * 4;
        let height = if stride == 0 {
            0
        } else {
            bits.len().div_ceil(stride)
        };
        self.push(width, height as u16, bits)
    }

    /// Add an image, squaring its bits up with its dimensions: the rest of the crate takes
    /// `bits.len() == Image::expected_len(width, height)` for granted, and a damaged stream
    /// should not be able to violate that.
    fn push(&mut self, width: u16, height: u16, mut bits: Vec<u8>) -> u32 {
        bits.resize(Image::expected_len(width, height), self.transparency);
        let index = self.images.len();
        self.images.push(Image {
            index,
            width,
            height,
            bits,
        });
        index as u32
    }
}

/// Read one animation stream (§6.3) and fold its art and audio into the shared pools.
///
/// The stream leads with its own sound and image tables, then a frame table whose entries
/// index them. Frames may carry a branch list and inline lip-sync mouth images, which the
/// pool absorbs as ordinary images so playback works exactly as it does for 2.0.
fn read_animation_stream(
    raw: &[u8],
    entry: &AnimationRef,
    char_version: u32,
    pool: &mut MediaPool,
) -> Result<Animation> {
    let body = inflate_animation(raw, char_version)?;
    let mut frames = Vec::new();
    // A stream that stops making sense part-way keeps the frames it did yield: one sample
    // character ships an animation whose compressed body is a few bytes short, and losing
    // its tail beats losing the character.
    let _ = read_stream_body(&body, pool, &mut frames);

    Ok(Animation {
        name: entry.name.clone(),
        return_kind: if entry.return_name.is_empty() {
            ReturnKind::None
        } else {
            ReturnKind::Named
        },
        return_name: entry.return_name.clone(),
        frames,
    })
}

/// Walk a decompressed animation body: its sound table, its image table, then its frames.
fn read_stream_body(body: &[u8], pool: &mut MediaPool, frames: &mut Vec<Frame>) -> Result<()> {
    let mut c = Cursor::new(body);

    let sound_base = pool.sounds.len();
    let sound_count = c.u16()? as usize;
    for _ in 0..sound_count {
        let len = c.u32()? as usize;
        pool.sounds.push(c.bytes(len)?.to_vec());
    }

    let image_count = c.u16()? as usize;
    let mut image_ndx = Vec::with_capacity(image_count);
    for _ in 0..image_count {
        let len = c.u32()? as usize;
        if len == 0 {
            // A placeholder: nothing is drawn for this slot.
            image_ndx.push(pool.push(0, 0, Vec::new()));
            continue;
        }
        c.skip(1)?; // storage flag; raw 8-bpp in every sampled stream
        let bits = c.bytes(len)?.to_vec();
        // Each raster is followed by its click/shape region, which nothing here draws.
        let region_len = c.u32()? as usize;
        c.skip(region_len)?;
        image_ndx.push(pool.push_frame_image(bits));
    }

    let frame_count = c.u16()? as usize;
    frames.reserve(frame_count);
    for _ in 0..frame_count {
        let image = c.u16()? as usize;
        let sound = c.i16()?;
        let duration = c.u16()?;
        let offset = (c.i16()?, c.i16()?);

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
            let len = c.u32()? as usize;
            if len == 0 {
                // An empty mouth slot: like the image table, a zero size ends the record.
                continue;
            }
            c.skip(1)?; // storage flag, as above
            let position = (c.i16()?, c.i16()?);
            let width = c.u16()?;
            let height = c.u16()?;
            let bits = c.bytes(len)?.to_vec();
            let image_ndx = pool.push(width, height, bits);
            // The overlay slot is a u16; a character with more art than that can't address
            // it, so the mouth image is simply not offered for lip-sync.
            if let Ok(image_ndx) = u16::try_from(image_ndx) {
                overlays.push(FrameOverlay {
                    overlay_type,
                    image_ndx,
                    replace: false,
                    offset: position,
                });
            }
        }

        frames.push(Frame {
            duration,
            // Per-animation sound indices are rebased onto the character-wide pool.
            sound_ndx: match usize::try_from(sound) {
                Ok(local) => i16::try_from(sound_base + local).unwrap_or(-1),
                Err(_) => -1,
            },
            exit_frame: -1,
            branching,
            images: image_ndx
                .get(image)
                .map(|&image_ndx| FrameImage { image_ndx, offset })
                .into_iter()
                .collect(),
            overlays,
        });
    }

    Ok(())
}

/// Unwrap an animation stream's envelope (§6.3) into its decompressed body.
///
/// A short decode is padded rather than rejected: one stream in the sample set stops a
/// handful of bytes early, and losing the tail of one frame beats losing the animation.
fn inflate_animation(raw: &[u8], char_version: u32) -> Result<Vec<u8>> {
    let mut c = Cursor::new(raw);
    if char_version >= VERSION_WITH_ANIM_IDS {
        c.skip(8)?; // stream version + the id from the animation list
    }
    let compressed = c.u8()? != 0;
    let inflated_len = c.u32()? as usize;
    let packed_len = c.u32()? as usize;
    if !compressed {
        return Ok(c.bytes(inflated_len)?.to_vec());
    }
    let mut packed = c.bytes(packed_len.min(c.remaining()))?.to_vec();
    packed.extend_from_slice(&[0xFF; 8]);
    let mut body = decode_run(&packed, inflated_len);
    if body.is_empty() {
        return Err(Error::DecodeFailed {
            got: 0,
            expected: inflated_len,
        });
    }
    body.resize(inflated_len, 0);
    Ok(body)
}
