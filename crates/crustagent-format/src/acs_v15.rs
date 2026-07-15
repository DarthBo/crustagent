//! ACS 1.5 reader — the older Microsoft Agent format, an **OLE2 compound document**
//! (Structured Storage) rather than the flat blob of ACS 2.0.
//!
//! Layout (reverse-engineered from the original ACS 1.5 container):
//! - The container holds a `char.acf` header stream plus one stream per animation.
//! - Each stream is independently compressed: a small prefix (sizes) then a `DecodeData`
//!   (same LZ77 as 2.0) payload.
//! - The decoded `char.acf` gives version, the animation table (with each animation's
//!   **stream name**), identity, header (image size / style / TTS / balloon), palette and
//!   states. Each animation stream decodes to sounds + images + frames, whose local image
//!   and sound indices are rebased into shared pools.
//!
//! We normalize all of this into the same [`AcsFile`] the 2.0 path produces, so the rest
//! of the crate (compositor, runtime) is oblivious to which format a character came from.

use std::io::{Cursor as IoCursor, Read};

use crate::acs::AcsFile;
use crate::decode::decode_data;
use crate::error::{Error, Result};
use crate::model::{
    Animation, Balloon, Branch, Color, FileHeader, Frame, FrameImage, FrameOverlay, Image,
    MouthOverlay, Name, ReturnKind, State, Tts,
};
use crate::reader::Cursor;

/// First 4 bytes of an OLE2 compound document (`D0 CF 11 E0` little-endian).
pub const OLE2_SIGNATURE: u32 = 0xE011_CFD0;
/// Signature DWORD at the head of the decompressed `char.acf` header stream.
pub const ACS_V15_HEADER_SIGNATURE: u32 = 0xABCD_ABC1;

// -- char_style bits (16-bit in 1.5, same low bits as 2.0) ---------------------
const STYLE_TTS: u16 = 0x0020;
const STYLE_BALLOON: u16 = 0x0200;

/// A parsed entry from the header's animation table.
struct AnimRef {
    name: String,
    stream: String,
    return_kind: ReturnKind,
    return_name: String,
    checksum: u32,
}

/// Parse an ACS 1.5 (`OLE2`) character.
pub fn parse_v15(bytes: Vec<u8>) -> Result<AcsFile> {
    let mut comp = cfb::CompoundFile::open(IoCursor::new(bytes))
        .map_err(|e| Error::InvalidData(format!("not a valid OLE2 compound file: {e}")))?;

    // -- header stream (char.acf) --
    let raw = read_stream(&mut comp, "char.acf")?;
    let decoded = decode_stream(&raw, 4)?; // sig(4) then DecodedSize/EncodedSize at 4/8
    let mut c = Cursor::new(&decoded);

    let version = c.u32()?;
    let (version_major, version_minor) = ((version >> 16) as u16, (version & 0xFFFF) as u16);
    let anim_refs = read_animations(&mut c)?;
    let (guid, name, desc1, desc2) = read_identity(&mut c)?;
    let (image_size, transparency, style, tts, balloon) = read_header(&mut c)?;
    let palette = read_palette(&mut c)?;
    let states = read_states(&mut c)?;

    // -- per-animation streams --
    let mut images: Vec<Image> = Vec::new();
    let mut sounds: Vec<Vec<u8>> = Vec::new();
    let mut gesture_names: Vec<String> = Vec::new();
    let mut animations: Vec<Animation> = Vec::new();

    for a in &anim_refs {
        let frames = read_animation_stream(&mut comp, a, image_size, &mut images, &mut sounds)
            .unwrap_or_default(); // a bad/missing stream yields an empty (still-listed) gesture
        gesture_names.push(a.name.clone());
        animations.push(Animation {
            name: a.name.clone(),
            return_kind: a.return_kind,
            return_name: a.return_name.clone(),
            frames,
        });
    }

    let header = FileHeader {
        version_major,
        version_minor,
        guid,
        image_size,
        transparency,
        style: style as u32,
        palette,
    };
    let names = vec![Name {
        language: 0x0409, // 1.5 stores a single English name
        name,
        desc1,
        desc2,
    }];

    Ok(AcsFile::from_v15(
        header,
        tts,
        balloon,
        names,
        states,
        gesture_names,
        animations,
        images,
        sounds,
    ))
}

/// Read a whole named stream out of the compound file.
fn read_stream<F: Read + std::io::Seek>(
    comp: &mut cfb::CompoundFile<F>,
    name: &str,
) -> Result<Vec<u8>> {
    let mut s = comp
        .open_stream(name)
        .map_err(|e| Error::InvalidData(format!("missing OLE2 stream {name:?}: {e}")))?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Decompress a stream whose DecodedSize/EncodedSize DWORDs sit at `size_off`, with the
/// compressed payload immediately after them.
fn decode_stream(raw: &[u8], size_off: usize) -> Result<Vec<u8>> {
    let mut c = Cursor::at(raw, size_off);
    let decoded_size = c.u32()? as usize;
    let encoded_size = c.u32()? as usize;
    let payload = c.bytes(encoded_size)?;
    decode_data(payload, decoded_size)
}

fn read_animations(c: &mut Cursor) -> Result<Vec<AnimRef>> {
    let count = c.u16()?;
    let mut refs = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = c.string(false)?;
        let stream = c.string(false)?;
        let return_name = c.string(false)?;
        let checksum = c.u32()?;
        let return_kind = if return_name.is_empty() {
            ReturnKind::None
        } else {
            ReturnKind::Named
        };
        refs.push(AnimRef {
            name,
            stream,
            return_kind,
            return_name,
            checksum,
        });
    }
    Ok(refs)
}

type Identity = (crate::model::Guid, String, String, String);
fn read_identity(c: &mut Cursor) -> Result<Identity> {
    let guid = c.guid()?;
    let name = first_caps(c.string(false)?);
    let desc1 = c.string(false)?;
    let desc2 = c.string(false)?;
    Ok((guid, name, desc1, desc2))
}

type HeaderFields = ((u16, u16), u8, u16, Option<Tts>, Option<Balloon>);
fn read_header(c: &mut Cursor) -> Result<HeaderFields> {
    let cx = c.u16()?;
    let cy = c.u16()?;
    let transparency = c.u8()?;
    let style = c.u16()?; // 16-bit in 1.5
    c.u8()?; // skip 1

    let tts = if style & STYLE_TTS != 0 {
        Some(read_tts(c)?)
    } else {
        None
    };
    let balloon = if style & STYLE_BALLOON != 0 {
        Some(read_balloon(c)?)
    } else {
        None
    };
    c.u16()?; // trailing unknown, always 0
    Ok(((cx, cy), transparency, style, tts, balloon))
}

fn read_tts(c: &mut Cursor) -> Result<Tts> {
    let engine = c.guid()?;
    let mode = c.guid()?;
    c.u8()?; // skip 1
    let speed = c.i32()?;
    let pitch = c.i16()?;
    let gender = if pitch >= 200 { 1 } else { 2 };
    Ok(Tts {
        engine,
        mode,
        speed,
        pitch,
        language: None,
        gender,
        age: 0,
        style: String::new(),
    })
}

fn read_balloon(c: &mut Cursor) -> Result<Balloon> {
    let lines = c.u8()?;
    let per_line = c.u8()?;
    let fg_color = c.colorref()?;
    let bg_color = c.colorref()?;
    let border_color = c.colorref()?;
    let font_name = c.string(false)?;
    let font_height = c.i32()?;
    let weight = c.u16()?;
    let strikeout = c.u8()?;
    let italic = c.u8()?;
    Ok(Balloon {
        lines,
        per_line,
        fg_color,
        bg_color,
        border_color,
        font_name,
        font_height,
        bold: weight >= 700,
        strikeout: strikeout != 0,
        italic: italic != 0,
    })
}

/// 1.5 palette: a WORD count but the pointer advances a DWORD; a count of 1 is a sentinel
/// for 256 (and rewinds a byte). Entries are `COLORREF`s stored B,G,R,pad.
fn read_palette(c: &mut Cursor) -> Result<Vec<Color>> {
    let raw = c.u16()?;
    c.u16()?; // advance to a full DWORD
    let count = if raw == 1 {
        c.seek(c.pos() - 1);
        256
    } else {
        raw as usize
    };
    let mut palette = Vec::with_capacity(count.min(256));
    for i in 0..count {
        let color = c.color()?;
        if i < 256 {
            palette.push(color);
        }
    }
    Ok(palette)
}

fn read_states(c: &mut Cursor) -> Result<Vec<State>> {
    let count = c.u16()?;
    let mut states = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = c.string(false)?;
        let gesture_count = c.u16()?;
        let mut animations = Vec::with_capacity(gesture_count as usize);
        for _ in 0..gesture_count {
            animations.push(c.string(false)?);
        }
        states.push(State { name, animations });
    }
    Ok(states)
}

/// Open, decompress and parse one animation stream, appending its images/sounds to the
/// shared pools and returning the frames (with indices rebased into those pools).
fn read_animation_stream<F: Read + std::io::Seek>(
    comp: &mut cfb::CompoundFile<F>,
    a: &AnimRef,
    image_size: (u16, u16),
    images: &mut Vec<Image>,
    sounds: &mut Vec<Vec<u8>>,
) -> Result<Vec<Frame>> {
    let _ = a.checksum; // validated by the original; we trust the stream
    let raw = read_stream(comp, &a.stream)?;
    // prefix: version(4) checksum(4) skip(1) DecodedSize(4)@9 EncodedSize(4)@13 payload@17
    let decoded = decode_stream(&raw, 9)?;
    let mut c = Cursor::new(&decoded);

    let (cx, cy) = image_size;
    let first_image = images.len();
    let first_sound = sounds.len();

    // sounds
    let snd_count = c.u16()?;
    for _ in 0..snd_count {
        let bc = c.u32()? as usize;
        sounds.push(c.bytes(bc)?.to_vec());
    }
    // images (8-bit bits, full character size; a trailing region blob we skip)
    let img_count = c.u16()?;
    for _ in 0..img_count {
        let bc = c.u32()? as usize;
        c.u8()?; // unknown
        let bits = c.bytes(bc)?.to_vec();
        images.push(Image {
            index: images.len(),
            width: cx,
            height: cy,
            bits,
        });
        let region = c.u32()? as usize;
        c.skip(region)?;
    }
    // frames
    let frame_count = c.u16()?;
    let mut frames = Vec::with_capacity(frame_count as usize);
    for _ in 0..frame_count {
        let image_ndx = c.i16()?;
        let sound_ndx = c.i16()?;
        let duration = c.u16()?;
        c.u16()?; // unknown
        c.u16()?; // unknown

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
        let mut overlays = Vec::new();
        for _ in 0..overlay_count {
            let overlay_type = MouthOverlay::from_u8(c.u8()?);
            let overlay_size = c.u32()? as usize;
            if overlay_size == 0 {
                continue; // 1.5 skips empty overlays
            }
            c.u8()?; // flag
            let off_x = c.i16()?;
            let off_y = c.i16()?;
            let w = c.u16()?;
            let h = c.u16()?;
            let bits = c.bytes(overlay_size)?.to_vec();
            let idx = images.len();
            images.push(Image {
                index: idx,
                width: w,
                height: h,
                bits,
            });
            overlays.push(FrameOverlay {
                overlay_type,
                image_ndx: idx as u16,
                replace: false, // 1.5 has no per-overlay replace flag
                offset: (off_x, off_y),
            });
        }

        let images_vec = if image_ndx >= 0 {
            vec![FrameImage {
                image_ndx: image_ndx as u32 + first_image as u32,
                offset: (0, 0),
            }]
        } else {
            Vec::new()
        };
        let sound = if sound_ndx >= 0 {
            sound_ndx + first_sound as i16
        } else {
            -1
        };
        frames.push(Frame {
            duration,
            sound_ndx: sound,
            exit_frame: -1, // 1.5 frames carry no explicit exit frame
            branching,
            images: images_vec,
            overlays,
        });
    }
    Ok(frames)
}

/// Upper-case the first character (matching `firstLetterCaps`).
fn first_caps(s: String) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
        None => s,
    }
}
