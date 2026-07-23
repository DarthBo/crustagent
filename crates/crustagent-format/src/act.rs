//! Parser and renderer for the **Actor Character Table** (`.act`) format — the character
//! files used by *Microsoft Actor*, the mid-'90s predecessor to Microsoft Agent that
//! powered the Office 97/98 Assistant (Clippit, Rover, The Genius, …) and Microsoft Bob.
//!
//! The byte layout was reverse-engineered from real files. An ACT file is a small header
//! (identity, palette, default frame size) followed by a table of seven end-of-file
//! *sections* (artwork directory, sounds, animation/frame tables, strings) and a pool of
//! artwork *cels*.
//!
//! # Two dialects, two byte orders
//!
//! The leading signature is `"LP"` for the little-endian PC files and `"PL"` for the
//! big-endian classic-Mac files (Office 98/2001) — the whole structure is simply
//! byte-swapped. Actor 1.0 (e.g. Rover, Microsoft Bob) and Actor 2.0 (e.g. Clippit)
//! differ slightly in the header; both are handled.
//!
//! # Artwork
//!
//! Cels are stored in one of a few forms. This module fully supports the common PC form:
//! each cel is an **Aldus Placeable Windows Metafile** (a vector drawing — filled polygons
//! with pen/brush colors) which [`ActFile::render_cel`] rasterizes to [`Rgba`].
//!
//! Newer PC characters (e.g. The Genius) instead store their artwork as LZ-compressed
//! blocks tagged `MNAK`. Each block holds several sub-images; each is an 8bpp raster under
//! a simple run-length encoding. [`ActFile::render_cel`] decodes these too, coloring them
//! with the standard Windows 256-color palette (index 10 is the transparent color key).
//! The classic-Mac artwork codec is a different, still-undecoded form. In all cases the
//! container still parses (identity, palette, sounds) and reports each cel's encoding via
//! [`CelFormat`].
//!
//! ```no_run
//! use crustagent_format::act::ActFile;
//! let act = ActFile::open("Clippit.act")?;
//! println!("{} — {} cels, {} sounds", act.name, act.cels.len(), act.sounds.len());
//! if let Some(img) = act.render_cel(0) {
//!     println!("cel 0 is {}x{}", img.width, img.height);
//! }
//! # Ok::<(), crustagent_format::Error>(())
//! ```

use crate::error::{Error, Result};
use crate::model::{Color, Indexed, Rgba};

/// Signature bytes of a little-endian (PC) actor file.
pub const ACT_SIGNATURE_LE: [u8; 2] = *b"LP";
/// Signature bytes of a big-endian (classic-Mac) actor file.
pub const ACT_SIGNATURE_BE: [u8; 2] = *b"PL";

/// The Aldus Placeable Metafile magic key (`0x9AC6CDD7`) that begins each WMF cel.
const PLACEABLE_KEY: u32 = 0x9AC6_CDD7;

/// The palette index that Microsoft Actor treats as the transparent color key.
pub const ACTOR_TRANSPARENT_INDEX: u8 = 0x0A;

/// The standard Windows 256-color ("halftone") palette, as RGB triples. `MNAK` bitmap cels
/// carry no palette of their own — the engine colors them from this system palette (index 10
/// is the transparent color key).
const HALFTONE_RGB: [u8; 768] = [
    0, 0, 0, 128, 0, 0, 0, 128, 0, 128, 128, 0, 0, 0, 128, 128, 0, 128, 0, 128, 128, 192, 192, 192,
    192, 220, 192, 166, 202, 240, 4, 4, 4, 8, 8, 8, 12, 12, 12, 17, 17, 17, 22, 22, 22, 28, 28, 28,
    34, 34, 34, 41, 41, 41, 85, 85, 85, 77, 77, 77, 66, 66, 66, 57, 57, 57, 255, 124, 128, 255, 80,
    80, 214, 0, 147, 204, 236, 255, 239, 214, 198, 231, 231, 214, 173, 169, 144, 51, 0, 0, 102, 0,
    0, 153, 0, 0, 204, 0, 0, 0, 51, 0, 51, 51, 0, 102, 51, 0, 153, 51, 0, 204, 51, 0, 255, 51, 0,
    0, 102, 0, 51, 102, 0, 102, 102, 0, 153, 102, 0, 204, 102, 0, 255, 102, 0, 0, 153, 0, 51, 153,
    0, 102, 153, 0, 153, 153, 0, 204, 153, 0, 255, 153, 0, 0, 204, 0, 51, 204, 0, 102, 204, 0, 153,
    204, 0, 204, 204, 0, 255, 204, 0, 102, 255, 0, 153, 255, 0, 204, 255, 0, 0, 0, 51, 51, 0, 51,
    102, 0, 51, 153, 0, 51, 204, 0, 51, 255, 0, 51, 0, 51, 51, 51, 51, 51, 102, 51, 51, 153, 51,
    51, 204, 51, 51, 255, 51, 51, 0, 102, 51, 51, 102, 51, 102, 102, 51, 153, 102, 51, 204, 102,
    51, 255, 102, 51, 0, 153, 51, 51, 153, 51, 102, 153, 51, 153, 153, 51, 204, 153, 51, 255, 153,
    51, 0, 204, 51, 51, 204, 51, 102, 204, 51, 153, 204, 51, 204, 204, 51, 255, 204, 51, 51, 255,
    51, 102, 255, 51, 153, 255, 51, 204, 255, 51, 255, 255, 51, 0, 0, 102, 51, 0, 102, 102, 0, 102,
    153, 0, 102, 204, 0, 102, 255, 0, 102, 0, 51, 102, 51, 51, 102, 102, 51, 102, 153, 51, 102,
    204, 51, 102, 255, 51, 102, 0, 102, 102, 51, 102, 102, 102, 102, 102, 153, 102, 102, 204, 102,
    102, 0, 153, 102, 51, 153, 102, 102, 153, 102, 153, 153, 102, 204, 153, 102, 255, 153, 102, 0,
    204, 102, 51, 204, 102, 153, 204, 102, 204, 204, 102, 255, 204, 102, 0, 255, 102, 51, 255, 102,
    153, 255, 102, 204, 255, 102, 255, 0, 204, 204, 0, 255, 0, 153, 153, 153, 51, 153, 153, 0, 153,
    204, 0, 153, 0, 0, 153, 51, 51, 153, 102, 0, 153, 204, 51, 153, 255, 0, 153, 0, 102, 153, 51,
    102, 153, 102, 51, 153, 153, 102, 153, 204, 102, 153, 255, 51, 153, 51, 153, 153, 102, 153,
    153, 153, 153, 153, 204, 153, 153, 255, 153, 153, 0, 204, 153, 51, 204, 153, 102, 204, 102,
    153, 204, 153, 204, 204, 153, 255, 204, 153, 0, 255, 153, 51, 255, 153, 102, 204, 153, 153,
    255, 153, 204, 255, 153, 255, 255, 153, 0, 0, 204, 51, 0, 153, 102, 0, 204, 153, 0, 204, 204,
    0, 204, 0, 51, 153, 51, 51, 204, 102, 51, 204, 153, 51, 204, 204, 51, 204, 255, 51, 204, 0,
    102, 204, 51, 102, 204, 102, 102, 153, 153, 102, 204, 204, 102, 204, 255, 102, 153, 0, 153,
    204, 51, 153, 204, 102, 153, 204, 153, 153, 204, 204, 153, 204, 255, 153, 204, 0, 204, 204, 51,
    204, 204, 102, 204, 204, 153, 204, 204, 204, 204, 204, 255, 204, 204, 0, 255, 204, 51, 255,
    204, 102, 255, 153, 153, 255, 204, 204, 255, 204, 255, 255, 204, 51, 0, 204, 102, 0, 255, 153,
    0, 255, 0, 51, 204, 51, 51, 255, 102, 51, 255, 153, 51, 255, 204, 51, 255, 255, 51, 255, 0,
    102, 255, 51, 102, 255, 102, 102, 204, 153, 102, 255, 204, 102, 255, 255, 102, 204, 0, 153,
    255, 51, 153, 255, 102, 153, 255, 153, 153, 255, 204, 153, 255, 255, 153, 255, 0, 204, 255, 51,
    204, 255, 102, 204, 255, 153, 204, 255, 204, 204, 255, 255, 204, 255, 51, 255, 255, 102, 255,
    204, 153, 255, 255, 204, 255, 255, 255, 102, 102, 102, 255, 102, 255, 255, 102, 102, 102, 255,
    255, 102, 255, 102, 255, 255, 165, 0, 33, 95, 95, 95, 119, 119, 119, 134, 134, 134, 150, 150,
    150, 203, 203, 203, 178, 178, 178, 215, 215, 215, 221, 221, 221, 227, 227, 227, 234, 234, 234,
    241, 241, 241, 248, 248, 248, 255, 251, 240, 160, 160, 164, 128, 128, 128, 255, 0, 0, 0, 255,
    0, 255, 255, 0, 0, 0, 255, 255, 0, 255, 0, 255, 255, 255, 255, 255,
];

/// The standard Windows 256-color palette used for `MNAK` bitmap cels.
fn bitmap_palette() -> Vec<Color> {
    (0..256)
        .map(|i| Color {
            r: HALFTONE_RGB[i * 3],
            g: HALFTONE_RGB[i * 3 + 1],
            b: HALFTONE_RGB[i * 3 + 2],
        })
        .collect()
}

/// Decode Actor's 8bpp run-length scheme to `expected` bytes: control byte `c` — when
/// `c < 0x80`, repeat the following byte `c` times; otherwise copy the next `c & 0x7f` bytes
/// literally. Output is clamped to `expected` (short/garbled input just stops early).
fn decode_rle8(src: &[u8], expected: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(expected);
    let mut i = 0;
    while i < src.len() && out.len() < expected {
        let c = src[i];
        i += 1;
        if c < 0x80 {
            let Some(&v) = src.get(i) else { break };
            i += 1;
            for _ in 0..c {
                if out.len() >= expected {
                    break;
                }
                out.push(v);
            }
        } else {
            for _ in 0..(c & 0x7f) {
                let Some(&v) = src.get(i) else { break };
                i += 1;
                if out.len() >= expected {
                    break;
                }
                out.push(v);
            }
        }
    }
    out.resize(expected, ACTOR_TRANSPARENT_INDEX);
    out
}

/// The palette index left "unchanged" by an SMC skip opcode — the transparent/background key
/// for the classic-Mac bitmaps (their real color table is an external QuickTime `clut`).
const MAC_TRANSPARENT_INDEX: u8 = 0x00;

/// Decode an Apple QuickTime **SMC** (`'smc '`) opcode stream to a top-down 8bpp raster of
/// `w`×`h`. SMC works on 4×4-pixel blocks in raster-of-tiles order, keeping three round-robin
/// color caches (pairs / quads / octets). `buf` is the chunk payload *after* the 4-byte
/// flags+length header. Malformed input stops early. (Port of the reference SMC decoder.)
fn decode_smc(buf: &[u8], w: usize, h: usize) -> Vec<u8> {
    let stride = w;
    let hp = h.div_ceil(4) * 4;
    let mut pix = vec![MAC_TRANSPARENT_INDEX; stride * hp];
    let mut pair = [0u8; 512];
    let mut quad = [0u8; 1024];
    let mut octet = [0u8; 2048];
    let (mut cpi, mut cqi, mut coi) = (0usize, 0usize, 0usize);
    let mut total = (w.div_ceil(4) * h.div_ceil(4)) as i64;
    let row_inc = (stride - 4) as i64;
    let (mut row_ptr, mut pixel_ptr, mut gb) = (0i64, 0i64, 0usize);
    let (sp, wi) = (stride as i64, w as i64);
    // A byte from the stream (0 past the end), advancing the cursor.
    macro_rules! next {
        () => {{
            let v = *buf.get(gb).unwrap_or(&0);
            gb += 1;
            v
        }};
    }
    // Block count for the skip/repeat/1-color classes; move to the next 4×4 block.
    macro_rules! blocks {
        ($op:expr) => {
            if $op & 0x10 != 0 {
                next!() as i64 + 1
            } else {
                1 + ($op & 0x0F) as i64
            }
        };
    }
    macro_rules! advance {
        () => {{
            pixel_ptr += 4;
            if pixel_ptr >= wi {
                pixel_ptr = 0;
                row_ptr += sp * 4;
            }
        }};
    }
    // Write a 4×4 block starting at pixel offset `dst`, sourcing each pixel via `$val`
    // (an expression evaluated per pixel with the block-local index `i` in 0..16).
    macro_rules! block {
        ($dst:expr, |$i:ident| $val:expr) => {{
            let mut b = $dst;
            let mut $i = 0usize;
            for _ in 0..4 {
                for _ in 0..4 {
                    let v = $val;
                    put(&mut pix, b, v);
                    b += 1;
                    $i += 1;
                }
                b += row_inc;
            }
        }};
    }
    while total > 0 && gb < buf.len() {
        let op = next!();
        match op & 0xF0 {
            0x00 | 0x10 => {
                let mut n = blocks!(op);
                while n > 0 {
                    advance!();
                    total -= 1;
                    n -= 1;
                }
            }
            0x20 | 0x30 => {
                let mut n = blocks!(op);
                while n > 0 {
                    let prev = if pixel_ptr == 0 {
                        row_ptr - wi * 4 + wi - 4
                    } else {
                        row_ptr + pixel_ptr - 4
                    };
                    let dst = row_ptr + pixel_ptr;
                    block!(dst, |i| get(&pix, prev + (i % 4 + (i / 4) * stride) as i64));
                    advance!();
                    total -= 1;
                    n -= 1;
                }
            }
            0x40 | 0x50 => {
                let mut n = blocks!(op) * 2;
                let pbp1 = if pixel_ptr == 0 {
                    row_ptr - wi * 4 + wi - 8
                } else if pixel_ptr == 4 {
                    row_ptr - wi * 4 + row_inc
                } else {
                    row_ptr + pixel_ptr - 8
                };
                let pbp2 = if pixel_ptr == 0 {
                    row_ptr - wi * 4 + row_inc
                } else {
                    row_ptr + pixel_ptr - 4
                };
                let mut flag = false;
                while n > 0 {
                    let prev = if flag { pbp2 } else { pbp1 };
                    flag = !flag;
                    let dst = row_ptr + pixel_ptr;
                    block!(dst, |i| get(&pix, prev + (i % 4 + (i / 4) * stride) as i64));
                    advance!();
                    total -= 1;
                    n -= 1;
                }
            }
            0x60 | 0x70 => {
                let mut n = blocks!(op);
                let val = next!();
                let dst0 = row_ptr + pixel_ptr;
                block!(dst0, |_i| val);
                advance!();
                total -= 1;
                n -= 1;
                while n > 0 {
                    let dst = row_ptr + pixel_ptr;
                    block!(dst, |_i| val);
                    advance!();
                    total -= 1;
                    n -= 1;
                }
            }
            0x80 | 0x90 => {
                let mut n = (op & 0x0F) as i64 + 1;
                let cti = if op & 0x10 == 0 {
                    let t = 2 * cpi;
                    pair[t] = next!();
                    pair[t + 1] = next!();
                    cpi = (cpi + 1) & 0xFF;
                    t
                } else {
                    2 * next!() as usize
                };
                while n > 0 {
                    let cf = read_u16(buf, gb);
                    gb += 2;
                    let dst = row_ptr + pixel_ptr;
                    block!(dst, |i| pair[cti + ((cf >> (15 - i)) & 1) as usize]);
                    advance!();
                    total -= 1;
                    n -= 1;
                }
            }
            0xA0 | 0xB0 => {
                let mut n = (op & 0x0F) as i64 + 1;
                let cti = if op & 0x10 == 0 {
                    let t = 4 * cqi;
                    for k in 0..4 {
                        quad[t + k] = next!();
                    }
                    cqi = (cqi + 1) & 0xFF;
                    t
                } else {
                    4 * next!() as usize
                };
                while n > 0 {
                    let cf = read_u32(buf, gb);
                    gb += 4;
                    let dst = row_ptr + pixel_ptr;
                    block!(dst, |i| quad[cti + ((cf >> (30 - i * 2)) & 3) as usize]);
                    advance!();
                    total -= 1;
                    n -= 1;
                }
            }
            0xC0 | 0xD0 => {
                let mut n = (op & 0x0F) as i64 + 1;
                let cti = if op & 0x10 == 0 {
                    let t = 8 * coi;
                    for k in 0..8 {
                        octet[t + k] = next!();
                    }
                    coi = (coi + 1) & 0xFF;
                    t
                } else {
                    8 * next!() as usize
                };
                while n > 0 {
                    let v1 = read_u16(buf, gb) as u32;
                    let v2 = read_u16(buf, gb + 2) as u32;
                    let v3 = read_u16(buf, gb + 4) as u32;
                    gb += 6;
                    // Two 24-bit flag words: rows 0-1 use `cfa`, rows 2-3 use `cfb`; 3 bits/px.
                    let cfa = ((v1 & 0xFFF0) << 8) | (v2 >> 4);
                    let cfb = ((v3 & 0xFFF0) << 8)
                        | ((v1 & 0x0F) << 8)
                        | ((v2 & 0x0F) << 4)
                        | (v3 & 0x0F);
                    let dst = row_ptr + pixel_ptr;
                    block!(dst, |i| {
                        let (cf, sh) = if i < 8 {
                            (cfa, 21 - i * 3)
                        } else {
                            (cfb, 21 - (i - 8) * 3)
                        };
                        octet[cti + ((cf >> sh) & 7) as usize]
                    });
                    advance!();
                    total -= 1;
                    n -= 1;
                }
            }
            _ => {
                // 0xE0/0xF0: raw — 16 literal indices per block.
                let mut n = (op & 0x0F) as i64 + 1;
                while n > 0 {
                    let dst = row_ptr + pixel_ptr;
                    block!(dst, |_i| next!());
                    advance!();
                    total -= 1;
                    n -= 1;
                }
            }
        }
    }
    // Crop the block-padded buffer to w×h (already top-down).
    let mut out = vec![MAC_TRANSPARENT_INDEX; w * h];
    for y in 0..h {
        out[y * w..(y + 1) * w].copy_from_slice(&pix[y * stride..y * stride + w]);
    }
    out
}

#[inline]
fn get(pix: &[u8], i: i64) -> u8 {
    if i >= 0 && (i as usize) < pix.len() {
        pix[i as usize]
    } else {
        MAC_TRANSPARENT_INDEX
    }
}

#[inline]
fn read_u16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([*b.get(o).unwrap_or(&0), *b.get(o + 1).unwrap_or(&0)])
}
#[inline]
fn read_u32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([
        *b.get(o).unwrap_or(&0),
        *b.get(o + 1).unwrap_or(&0),
        *b.get(o + 2).unwrap_or(&0),
        *b.get(o + 3).unwrap_or(&0),
    ])
}
#[inline]
fn put(pix: &mut [u8], i: i64, v: u8) {
    if i >= 0 && (i as usize) < pix.len() {
        pix[i as usize] = v;
    }
}

/// How a cel's artwork is encoded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CelFormat {
    /// Aldus Placeable Windows Metafile (vector). Rasterized by [`ActFile::render_cel`].
    Wmf,
    /// Compressed 8bpp raster (an `MNAK` sub-image). Rasterized by [`ActFile::render_cel`].
    Bitmap,
    /// The classic-Mac artwork codec (not yet decoded).
    MacBitmap,
}

/// One artwork cel: a slice of the file plus, for WMF cels, its placeable bounding box.
#[derive(Clone, Debug)]
pub struct Cel {
    pub format: CelFormat,
    /// Byte offset of the cel within the file.
    pub offset: usize,
    /// Byte length of the cel.
    pub len: usize,
    /// Placeable bounding box `(left, top, right, bottom)` in the cel's logical units.
    /// `None` for non-WMF cels. Cels share one logical space, so these positions are how
    /// parts (eyes, mouths) line up over a body when composited.
    pub bounds: Option<(i16, i16, i16, i16)>,
    /// For [`CelFormat::Bitmap`] cels, which sub-image within the `MNAK` block at `offset`
    /// this cel is (one block packs several). Always `0` for other formats.
    pub sub: u16,
}

/// One layered part of a pose: an image cel placed at a destination rectangle.
#[derive(Clone, Copy, Debug)]
pub struct Part {
    /// Index into [`ActFile::cels`].
    pub image: u16,
    /// Destination rectangle `(left, top, right, bottom)` in twips (1/1440"), signed —
    /// parts can sit partly off the frame edge.
    pub rect: (i16, i16, i16, i16),
}

/// A pose: a complete character image built by layering image parts (body, eyes, mouth…).
#[derive(Clone, Debug, Default)]
pub struct Pose {
    pub parts: Vec<Part>,
}

/// What an object-table entry resolves to.
#[derive(Clone, Copy, Debug)]
enum ObjRef {
    /// A leaf image at [`ActFile::cels`]`[_]`.
    Cel(u32),
    /// A composited pose at [`ActFile::poses`]`[_]`.
    Pose(u32),
    /// Undecodable / empty (drawn as a transparent frame).
    Empty,
}

/// One step of an animation. Branch `target`s are indices into the same [`Animation::ops`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Display `object` (an index into the object table — render with
    /// [`ActFile::render_object`]) for `duration_ms`. Duration `0` is an instantaneous
    /// routing step (the previous image stays on screen).
    Show { object: u16, duration_ms: u16 },
    /// With probability `weight / 65536`, jump to op `target`; otherwise fall through to the
    /// next op. `weight == 0` never branches (a fall-through / terminal marker).
    Branch { target: u16, weight: u16 },
    /// Play embedded sound `id` (an index into [`ActFile::sounds`]) at `volume`; `pan` is a
    /// balance/mode byte. Advances to the next op.
    Sound { id: u16, volume: u8, pan: u8 },
    /// Jump to op `target` once the animation has repeated `count` times (a bounded loop).
    LoopBranch { target: u16, count: u16 },
    /// Jump to op `target` when the host's mood / time-of-day state matches `state`.
    StateBranch { target: u16, state: u16 },
}

/// One animation variant: a self-contained op list. Branch/loop/state targets index into
/// `ops`; playback ends when the index runs past the end.
#[derive(Clone, Debug, Default)]
pub struct Animation {
    pub ops: Vec<Op>,
}

/// A named animation (e.g. `"Greeting"`, `"Thinking"`), referenced by the classic Microsoft
/// Actor action id. An action holds one or more interchangeable `variants` (the engine picks
/// one at random each time it plays); walk one with [`ActFile::action_sequence`].
#[derive(Clone, Debug)]
pub struct Action {
    /// Microsoft Actor action id (1 = Idle, 2 = Greeting, 24 = Thinking, …).
    pub id: u16,
    /// Human-readable name for `id`, or `"Action{id}"` if unknown.
    pub name: String,
    /// Interchangeable animation variants (at least one).
    pub variants: Vec<Animation>,
}

/// Names for the Office Assistant action ids, from Microsoft's official `MsoAnimationType`
/// enumeration (the Office object model — the same values the Assistant/Actor engine uses).
/// Ids outside that enum — a few internal Actor actions (7–10, 14–17, 20, 21) and
/// character-specific ids — have no Microsoft-published name; callers fall back to
/// `Action{id}` rather than invent one.
fn action_name(id: u16) -> Option<&'static str> {
    Some(match id {
        1 => "Idle",
        2 => "Greeting",
        3 => "Goodbye",
        4 => "BeginSpeaking",
        5 => "RestPose",
        6 => "CharacterSuccessMajor",
        11 => "GetAttentionMajor",
        12 => "GetAttentionMinor",
        13 => "Searching",
        18 => "Printing",
        19 => "GestureRight",
        22 => "WritingNotingSomething",
        23 => "WorkingAtSomething",
        24 => "Thinking",
        25 => "SendingMail",
        26 => "ListensToComputer",
        31 => "Disappear",
        32 => "Appear",
        100 => "GetArtsy",
        101 => "GetTechy",
        102 => "GetWizardy",
        103 => "CheckingSomething",
        104 => "LookDown",
        105 => "LookDownLeft",
        106 => "LookDownRight",
        107 => "LookLeft",
        108 => "LookRight",
        109 => "LookUp",
        110 => "LookUpLeft",
        111 => "LookUpRight",
        112 => "Saving",
        113 => "GestureDown",
        114 => "GestureLeft",
        115 => "GestureUp",
        116 => "EmptyTrash",
        _ => return None,
    })
}

/// A parsed Actor Character Table.
pub struct ActFile {
    /// `true` for the big-endian classic-Mac dialect (`"PL"` signature).
    pub big_endian: bool,
    /// `(major, minor)` — Actor 1.0 files report `(1, 0)`, Actor 2.0 `(2, x)`.
    pub version: (u16, u16),
    /// Character name (e.g. `"Clippit"`).
    pub name: String,
    /// Default frame size in pixels `(width, height)`.
    pub image_size: (u16, u16),
    /// Optional color palette (used by the bitmap artwork forms; often tiny for WMF files).
    pub palette: Vec<Color>,
    /// The artwork encoding this file uses. Only [`CelFormat::Wmf`] can be rendered.
    pub image_format: CelFormat,
    /// Artwork cels, in file order.
    pub cels: Vec<Cel>,
    /// Poses (layered image parts) referenced by the object table. May be empty.
    pub poses: Vec<Pose>,
    /// Named animations, decoded from the character's action/frame tables.
    pub actions: Vec<Action>,
    /// Object table: index → renderable (a cel or a pose). Animation `Show` ops and pose
    /// parts reference objects by their index here. Private; use [`ActFile::render_object`].
    objects: Vec<ObjRef>,
    /// Embedded sound effects, each a complete `RIFF`/`WAVE` byte stream.
    pub sounds: Vec<Vec<u8>>,
    /// The seven section offsets from the header (artwork, sounds, tables, strings).
    pub sections: [u32; 7],
    /// Pixel size of every classic-Mac SMC cel (from the QuickTime image description); `(0,0)`
    /// for non-Mac files.
    smc_size: (u16, u16),
    data: Vec<u8>,
}

impl ActFile {
    /// Open and parse an `.act` file from disk.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<ActFile> {
        ActFile::parse(std::fs::read(path)?)
    }

    /// Parse an in-memory `.act` byte buffer.
    pub fn parse(data: Vec<u8>) -> Result<ActFile> {
        if data.len() < 24 {
            return Err(Error::UnexpectedEof {
                context: "act header",
                offset: 0,
                needed: 24,
                available: data.len(),
            });
        }
        let big_endian = match &data[0..2] {
            s if s == ACT_SIGNATURE_LE => false,
            s if s == ACT_SIGNATURE_BE => true,
            other => {
                return Err(Error::BadSignature {
                    found: u32::from_le_bytes([other[0], other[1], 0, 0]),
                })
            }
        };
        let r = Rdr {
            b: &data,
            be: big_endian,
        };

        let version = (r.u16(2), r.u16(4));

        // Null-terminated ASCII name at offset 0x12.
        let name = {
            let mut p = 0x12;
            while p < data.len() && data[p] != 0 {
                p += 1;
            }
            String::from_utf8_lossy(&data[0x12..p.min(data.len())]).into_owned()
        };

        // The default frame size (pixels) sits immediately before the constant `2083, 2083`
        // marker word that both dialects embed in the header. Locate it endian-aware.
        let marker = {
            let mut m = [0u8; 4];
            m[..2].copy_from_slice(&r.pack16(2083));
            m[2..].copy_from_slice(&r.pack16(2083));
            m
        };
        // The two words before the marker are the pixel frame size. Windows stores them
        // (width, height); the classic-Mac dialect uses QuickDraw order (height, width), so
        // swap for big-endian to always yield (width, height).
        let image_size = find_subslice(&data[..data.len().min(0x200)], &marker)
            .filter(|&pos| pos >= 4)
            .map(|pos| {
                let (a, b) = (r.u16(pos - 4), r.u16(pos - 2));
                if big_endian {
                    (b, a)
                } else {
                    (a, b)
                }
            })
            .unwrap_or((0, 0));

        // Section table: seven ascending u32 offsets that end near EOF, preceded by a small
        // count. Scan the header region for the first window that fits (this skips the
        // count word, which is < 0x1000 while the real offsets are large).
        let fsz = data.len();
        let mut table = None;
        let scan_end = data.len().saturating_sub(28);
        for q in 0x12..scan_end.min(0x200) {
            let offs = [
                r.u32(q),
                r.u32(q + 4),
                r.u32(q + 8),
                r.u32(q + 12),
                r.u32(q + 16),
                r.u32(q + 20),
                r.u32(q + 24),
            ];
            let ascending = offs.windows(2).all(|w| w[0] < w[1]);
            let in_range = offs.iter().all(|&o| (o as usize) < fsz);
            if offs[0] > 0x1000 && ascending && in_range && offs[6] as usize > fsz * 85 / 100 {
                table = Some((q, offs));
                break;
            }
        }
        let (table_pos, sections) = table
            .ok_or_else(|| Error::InvalidData("could not locate the actor section table".into()))?;

        // Palette: a u32 count immediately after the table, then that many RGBQUADs.
        let palette = {
            let pc = r.u32(table_pos + 28) as usize;
            let mut pal = Vec::new();
            if pc <= 256 {
                let base = table_pos + 32;
                for i in 0..pc {
                    let o = base + i * 4;
                    if o + 4 > data.len() {
                        break;
                    }
                    // RGBQUAD: blue, green, red, reserved.
                    pal.push(Color {
                        b: data[o],
                        g: data[o + 1],
                        r: data[o + 2],
                    });
                }
            }
            pal
        };

        // Cels: walk the artwork pool (from the first placeable key up to the first section)
        // by the placeable magic. Boundaries taken from magic-to-magic; more robust than
        // trusting each metafile's self-reported size.
        let key = r.pack32(PLACEABLE_KEY);
        let art_end = sections[0] as usize;
        let mut positions = Vec::new();
        if let Some(first) = find_subslice(&data[..art_end.min(data.len())], &key) {
            let mut p = first;
            while p + 4 <= art_end {
                if r.u32(p) == PLACEABLE_KEY {
                    positions.push(p);
                    // Skip ahead by at least the placeable header before searching again.
                    match find_subslice(&data[p + 4..art_end], &key) {
                        Some(rel) => p = p + 4 + rel,
                        None => break,
                    }
                } else {
                    break;
                }
            }
        }
        let mut cels = Vec::with_capacity(positions.len());
        for (i, &start) in positions.iter().enumerate() {
            let end = positions.get(i + 1).copied().unwrap_or(art_end);
            // Placeable header: key(4), handle(2), left, top, right, bottom (i16 each).
            let bounds = Some((
                r.i16(start + 6),
                r.i16(start + 8),
                r.i16(start + 10),
                r.i16(start + 12),
            ));
            cels.push(Cel {
                format: CelFormat::Wmf,
                offset: start,
                len: end - start,
                bounds,
                sub: 0,
            });
        }

        // Newer PC characters (e.g. The Genius) store no WMF cels; instead the artwork is a
        // run of LZ-compressed blocks tagged "MNAK", each packing several sub-images (the
        // header's `count`). The compression is the very same bitstream as ACS. We enumerate
        // every sub-image as its own cel so each animation image is individually rasterizable.
        if cels.is_empty() && !big_endian {
            let mut from = 0usize;
            let mut starts = Vec::new();
            while let Some(rel) = find_subslice(&data[from..art_end.min(data.len())], b"MNAK") {
                starts.push(from + rel);
                from += rel + 4;
            }
            for (i, &start) in starts.iter().enumerate() {
                let end = starts.get(i + 1).copied().unwrap_or(art_end);
                // MNAK header: tag(4), u32 uncompressed size, u32 sub-image count.
                let count = data
                    .get(start + 8..start + 12)
                    .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .unwrap_or(1)
                    .max(1);
                for sub in 0..count as u16 {
                    cels.push(Cel {
                        format: CelFormat::Bitmap,
                        offset: start,
                        len: end - start,
                        bounds: None,
                        sub,
                    });
                }
            }
        }

        // Classic-Mac characters: the artwork pool is a run of QuickTime SMC (`'smc '`)
        // compressed cels, one per object-directory entry. Enumerate them from the object
        // directory (big-endian pool offsets); pose entries (type word 0x14) are skipped and
        // picked up as poses by `parse_animation`. Every cel's pixel size comes from the
        // QuickTime image description, located by its `'smc '` codec tag.
        let mut smc_size = (0u16, 0u16);
        if cels.is_empty() && big_endian {
            if let Some(id) = find_subslice(&data, b"smc ").map(|p| p.saturating_sub(4)) {
                if id + 36 <= data.len() {
                    smc_size = (r.u16(id + 32), r.u16(id + 34));
                }
            }
            let body_base = r.u32(10) as usize;
            let region = |f: usize| body_base + r.u32(body_base + f) as usize;
            let art_base = region(0x26);
            let (objdir_s, objdir_e) = (region(0x2a), region(0x2e));
            if art_base <= data.len()
                && objdir_e <= data.len()
                && objdir_s < objdir_e
                && art_base <= objdir_s
            {
                let mut offs: Vec<usize> = (objdir_s..objdir_e)
                    .step_by(4)
                    .map(|q| (r.u32(q) & 0x3FFF_FFFF) as usize)
                    .collect();
                offs.push(objdir_s - art_base); // pool end sentinel
                offs.sort_unstable();
                offs.dedup();
                for w in offs.windows(2) {
                    let (off, end) = (w[0], w[1]);
                    let abs = art_base + off;
                    if abs + 4 > data.len() || r.u16(abs) == 0x0014 {
                        continue; // out of range, or a pose object (not a cel)
                    }
                    cels.push(Cel {
                        format: CelFormat::MacBitmap,
                        offset: abs,
                        len: end - off,
                        bounds: None,
                        sub: 0,
                    });
                }
            }
        }

        // The artwork encoding, from what was actually found.
        let image_format = match cels.first().map(|c| c.format) {
            Some(f) => f,
            None if big_endian => CelFormat::MacBitmap,
            None => CelFormat::Bitmap,
        };

        // Animation tables — the object table (index → cel/pose), poses, and named actions
        // with their frame programs. All PC and Mac characters share this format; only files
        // whose cels we couldn't enumerate (unknown artwork) are skipped.
        let (objects, poses, actions) = if !cels.is_empty() {
            parse_animation(&r, &cels, &sections)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };

        // Sounds: extract every complete RIFF/WAVE stream. (Big-endian Mac audio is stored
        // differently and is not extracted here.)
        let sounds = if big_endian {
            Vec::new()
        } else {
            extract_wave_streams(&data)
        };

        Ok(ActFile {
            big_endian,
            version,
            name,
            image_size,
            palette,
            image_format,
            cels,
            poses,
            actions,
            objects,
            sounds,
            sections,
            smc_size,
            data,
        })
    }

    /// Raw bytes of cel `index` (the placeable-WMF stream for WMF cels, or the `MNAK`
    /// block for compressed bitmap cels).
    pub fn cel_bytes(&self, index: usize) -> Option<&[u8]> {
        let cel = self.cels.get(index)?;
        self.data.get(cel.offset..cel.offset + cel.len)
    }

    /// Decompress the whole `MNAK` block backing a [`CelFormat::Bitmap`] cel to its raw bytes.
    ///
    /// The compression is the same LZ77 bitstream as ACS. The decoded buffer is the block's
    /// concatenated sub-images, each `u32 width`, `u32 height`, `u32 flags`, then a
    /// run-length-encoded 8bpp raster. All cels sharing one block return the same buffer;
    /// use [`decode_bitmap_cel`](Self::decode_bitmap_cel) for a single sub-image's pixels, or
    /// [`render_cel`](Self::render_cel) to rasterize it. Returns `None` for non-`MNAK` cels or
    /// if decompression fails.
    pub fn decompress_cel(&self, index: usize) -> Option<Vec<u8>> {
        let cel = self.cels.get(index)?;
        if cel.format != CelFormat::Bitmap {
            return None;
        }
        let block = self.data.get(cel.offset..cel.offset + cel.len)?;
        if block.len() < 12 || &block[0..4] != b"MNAK" {
            return None;
        }
        // MNAK header: tag(4), u32 uncompressed size, u32 sub-image count, then (count-1)
        // u32 body offsets, then the LZ payload.
        let size = u32::from_le_bytes([block[4], block[5], block[6], block[7]]) as usize;
        let count = u32::from_le_bytes([block[8], block[9], block[10], block[11]]).max(1) as usize;
        let payload = 12 + (count - 1) * 4;
        crate::decode::decode_data(block.get(payload..)?, size).ok()
    }

    /// Decode a single [`CelFormat::Bitmap`] cel to `(width, height, indices)`: an 8bpp raster
    /// in top-down row order. Index [`ACTOR_TRANSPARENT_INDEX`] is the transparent color key.
    /// Returns `None` for non-bitmap cels or on any decode failure.
    pub fn decode_bitmap_cel(&self, index: usize) -> Option<(u32, u32, Vec<u8>)> {
        let cel = self.cels.get(index)?;
        if cel.format != CelFormat::Bitmap {
            return None;
        }
        let block = self.data.get(cel.offset..cel.offset + cel.len)?;
        if block.len() < 12 || &block[0..4] != b"MNAK" {
            return None;
        }
        let count = u32::from_le_bytes([block[8], block[9], block[10], block[11]]).max(1) as usize;
        let body = self.decompress_cel(index)?;
        // Sub-image `s` body range: [0, off[0], off[1], …, off[count-2], body.len()].
        let sub = cel.sub as usize;
        let sub_start = if sub == 0 {
            0
        } else {
            let o = 12 + (sub - 1) * 4;
            u32::from_le_bytes([block[o], block[o + 1], block[o + 2], block[o + 3]]) as usize
        };
        let sub_end = if sub + 1 < count {
            let o = 12 + sub * 4;
            u32::from_le_bytes([block[o], block[o + 1], block[o + 2], block[o + 3]]) as usize
        } else {
            body.len()
        };
        let seg = body.get(sub_start..sub_end)?;
        if seg.len() < 12 {
            return None;
        }
        let w = u32::from_le_bytes([seg[0], seg[1], seg[2], seg[3]]) as usize;
        let h = u32::from_le_bytes([seg[4], seg[5], seg[6], seg[7]]) as usize;
        let n = w.checked_mul(h)?;
        if n == 0 || n > (1 << 24) {
            return None;
        }
        // The raster is an 8bpp bottom-up DIB, so each row is padded to a 4-byte boundary.
        // Genius (width 124) needs no padding, but e.g. TUTOR (width 143) does — decode the
        // full padded stride, then copy just the `w` real pixels per row while flipping to
        // top-down. RLE control byte `c`: `c < 0x80` repeats the next byte `c` times;
        // `c >= 0x80` copies the next `c & 0x7f` bytes literally.
        let stride = (w + 3) & !3;
        let rows = decode_rle8(&seg[12..], stride * h);
        let mut top = vec![ACTOR_TRANSPARENT_INDEX; n];
        for y in 0..h {
            let s = (h - 1 - y) * stride;
            top[y * w..(y + 1) * w].copy_from_slice(&rows[s..s + w]);
        }
        Some((w as u32, h as u32, top))
    }

    /// Decode a [`CelFormat::MacBitmap`] cel (a QuickTime SMC chunk) to `(width, height,
    /// indices)`: a top-down 8bpp raster. Index [`MAC_TRANSPARENT_INDEX`] is the transparent
    /// key. Returns `None` for non-Mac cels or if the pixel size is unknown.
    pub fn decode_smc_cel(&self, index: usize) -> Option<(u32, u32, Vec<u8>)> {
        let cel = self.cels.get(index)?;
        if cel.format != CelFormat::MacBitmap {
            return None;
        }
        let (w, h) = (self.smc_size.0 as usize, self.smc_size.1 as usize);
        if w == 0 || h == 0 {
            return None;
        }
        // Each cel is `[u8 flags][u24-BE length]` then the SMC opcode stream.
        let chunk = self.data.get(cel.offset..cel.offset + cel.len)?;
        let payload = chunk.get(4..)?;
        Some((w as u32, h as u32, decode_smc(payload, w, h)))
    }

    /// Rasterize cel `index` to top-down RGBA. WMF cels are rendered from their metafile
    /// (sized to the cel's bounding box); [`CelFormat::Bitmap`] cels are RLE-decoded and
    /// [`CelFormat::MacBitmap`] cels are SMC-decoded, both colored with the standard Windows
    /// palette (the characters' true color tables are external). Returns `None` if undecodable.
    pub fn render_cel(&self, index: usize) -> Option<Rgba> {
        let cel = self.cels.get(index)?;
        match cel.format {
            CelFormat::Bitmap => {
                let (w, h, indices) = self.decode_bitmap_cel(index)?;
                let img = Indexed {
                    width: w,
                    height: h,
                    indices,
                    transparent: ACTOR_TRANSPARENT_INDEX,
                };
                Some(img.to_rgba(&bitmap_palette()))
            }
            CelFormat::MacBitmap => {
                let (w, h, indices) = self.decode_smc_cel(index)?;
                let img = Indexed {
                    width: w,
                    height: h,
                    indices,
                    transparent: MAC_TRANSPARENT_INDEX,
                };
                Some(img.to_rgba(&bitmap_palette()))
            }
            CelFormat::Wmf => {
                let bytes = self.data.get(cel.offset..cel.offset + cel.len)?;
                wmf::render(bytes, self.big_endian)
            }
        }
    }

    /// Composite pose `index` (from [`ActFile::poses`]) into a full character frame, sized
    /// [`ActFile::image_size`]. Parts are drawn in order, each part's source object placed at
    /// its destination (twips ÷ 15 → pixels). Returns `None` if the pose can't be rendered.
    pub fn render_pose(&self, index: usize) -> Option<Rgba> {
        self.render_pose_depth(index, 0)
    }

    fn render_pose_depth(&self, index: usize, depth: u8) -> Option<Rgba> {
        let pose = self.poses.get(index)?;
        let (cw, ch) = (self.image_size.0 as u32, self.image_size.1 as u32);
        if cw == 0 || ch == 0 {
            return None;
        }
        let mut canvas = Rgba::transparent(cw, ch);
        for part in &pose.parts {
            // A part places another object (a leaf cel, or occasionally a nested pose) at its
            // own destination rect. Render it at natural size; guard pose→pose cycles.
            let Some(img) = self.render_leaf(part.image as usize, depth + 1) else {
                continue;
            };
            let ox = (part.rect.0 as i32) / 15;
            let oy = (part.rect.1 as i32) / 15;
            blit_over(&mut canvas, &img, ox, oy);
        }
        Some(canvas)
    }

    /// An object at its natural size: a leaf cel as rendered, or a pose composited onto the
    /// character canvas. Used for pose parts (which then place it at their own rect).
    fn render_leaf(&self, object: usize, depth: u8) -> Option<Rgba> {
        if depth > 8 {
            return None;
        }
        match self.objects.get(object) {
            Some(&ObjRef::Cel(ci)) => self.render_cel(ci as usize),
            Some(&ObjRef::Pose(pi)) => self.render_pose_depth(pi as usize, depth),
            _ => None,
        }
    }

    /// Render an object (by its index in the character's object table) to a full character
    /// frame ([`ActFile::image_size`]). An object is either a composited pose or a bare image
    /// cel (centered on the canvas); unknown/empty objects give a transparent frame. Animation
    /// `Show` ops reference objects by this index. Returns `None` only if the frame size is 0.
    pub fn render_object(&self, object: usize) -> Option<Rgba> {
        let (w, h) = (self.image_size.0 as u32, self.image_size.1 as u32);
        if w == 0 || h == 0 {
            return None;
        }
        match self.objects.get(object) {
            Some(&ObjRef::Pose(pi)) => self.render_pose_depth(pi as usize, 0),
            Some(&ObjRef::Cel(ci)) => {
                let mut canvas = Rgba::transparent(w, h);
                if let Some(cel) = self.render_cel(ci as usize) {
                    let ox = (w as i32 - cel.width as i32) / 2;
                    let oy = (h as i32 - cel.height as i32) / 2;
                    blit_over(&mut canvas, &cel, ox, oy);
                }
                Some(canvas)
            }
            _ => Some(Rgba::transparent(w, h)),
        }
    }

    /// Find an action by (case-insensitive) name — e.g. `"Greeting"`, `"Thinking"`.
    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// Resolve an action to a linear list of `(object, duration_ms)` display steps by running
    /// its first variant's op program: `Show` emits a step, `Branch` takes its weighted jump
    /// (probability `weight / 65536`), and sound/loop/state ops advance. `seed` makes the
    /// probabilistic choices reproducible; `max_steps` bounds looping animations for a finite
    /// preview. Playback ends when the op index runs past the program.
    pub fn action_sequence_seeded(
        &self,
        action: &Action,
        max_steps: usize,
        seed: u64,
    ) -> Vec<(u16, u16)> {
        let mut out = Vec::new();
        let Some(anim) = action.variants.first() else {
            return out;
        };
        let mut rng = seed ^ 0x9E37_79B9_7F4A_7C15;
        let mut roll = || {
            // SplitMix64 step → a 16-bit roll.
            rng = rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = rng;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            ((z ^ (z >> 31)) & 0xFFFF) as u32
        };
        let mut i = 0usize;
        let mut steps = 0usize;
        while out.len() < max_steps && steps < max_steps * 8 {
            steps += 1;
            let Some(&op) = anim.ops.get(i) else {
                break;
            };
            match op {
                Op::Show {
                    object,
                    duration_ms,
                } => {
                    // Zero-duration shows are instantaneous routing steps — don't emit.
                    if duration_ms > 0 {
                        out.push((object, duration_ms));
                    }
                    i += 1;
                }
                Op::Branch { target, weight } => {
                    if weight > 0 && roll() < weight as u32 {
                        i = target as usize;
                    } else {
                        i += 1;
                    }
                }
                // Sound and the loop/state branches don't affect a linear preview; advance.
                _ => i += 1,
            }
        }
        out
    }

    /// [`action_sequence_seeded`](Self::action_sequence_seeded) with a fixed seed.
    pub fn action_sequence(&self, action: &Action, max_steps: usize) -> Vec<(u16, u16)> {
        self.action_sequence_seeded(action, max_steps, 0x1234_5678)
    }
}

/// Alpha-over composite `src` onto `dst` at pixel offset `(ox, oy)` (opaque src pixels win).
fn blit_over(dst: &mut Rgba, src: &Rgba, ox: i32, oy: i32) {
    for sy in 0..src.height as i32 {
        let dy = oy + sy;
        if dy < 0 || dy >= dst.height as i32 {
            continue;
        }
        for sx in 0..src.width as i32 {
            let dx = ox + sx;
            if dx < 0 || dx >= dst.width as i32 {
                continue;
            }
            let si = ((sy as u32 * src.width + sx as u32) * 4) as usize;
            if src.pixels[si + 3] == 0 {
                continue;
            }
            let di = ((dy as u32 * dst.width + dx as u32) * 4) as usize;
            dst.pixels[di..di + 4].copy_from_slice(&src.pixels[si..si + 4]);
        }
    }
}

/// A tiny endian-aware view over the file bytes.
struct Rdr<'a> {
    b: &'a [u8],
    be: bool,
}

impl Rdr<'_> {
    fn u16(&self, p: usize) -> u16 {
        let b = self.b.get(p..p + 2).unwrap_or(&[0, 0]);
        if self.be {
            u16::from_be_bytes([b[0], b[1]])
        } else {
            u16::from_le_bytes([b[0], b[1]])
        }
    }
    fn i16(&self, p: usize) -> i16 {
        self.u16(p) as i16
    }
    fn u32(&self, p: usize) -> u32 {
        let b = self.b.get(p..p + 4).unwrap_or(&[0, 0, 0, 0]);
        if self.be {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        }
    }
    fn pack16(&self, v: u16) -> [u8; 2] {
        if self.be {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    }
    fn pack32(&self, v: u32) -> [u8; 4] {
        if self.be {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    }
}

/// Find the first occurrence of `needle` in `hay`.
fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Scan a buffer for complete `RIFF....WAVE` streams and return each one's bytes.
fn extract_wave_streams(data: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 12 <= data.len() {
        if &data[i..i + 4] == b"RIFF" && &data[i + 8..i + 12] == b"WAVE" {
            let size =
                u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
            let total = size + 8;
            if total >= 12 && i + total <= data.len() {
                out.push(data[i..i + total].to_vec());
                i += total;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Decode the object directory (poses), the frame graph, and the named actions for a WMF
/// character. Tolerant: returns whatever parses cleanly, or empties on any mismatch.
/// Decode the character's animation tables — the object table (index → cel/pose), the poses,
/// and the named actions with their frame programs. This is the format the Actor engine
/// actually uses; the WMF (Clippit, Rover) and MNAK-bitmap (The Genius) PC characters share
/// it. Region offsets come from the 70-byte char-info header at the body base (mirrored into
/// `sections`, relative to that base); the artwork pool begins at the first cel.
fn parse_animation(
    r: &Rdr,
    cels: &[Cel],
    sections: &[u32; 7],
) -> (Vec<ObjRef>, Vec<Pose>, Vec<Action>) {
    let empty = || (Vec::new(), Vec::new(), Vec::new());
    // Header region fields are relative to the body base (the first outer section offset).
    let body_base = r.u32(10) as usize;
    let art_base = body_base + r.u32(body_base + 0x26) as usize;
    let region = |i: usize| body_base + sections[i] as usize;
    let (objdir_s, objdir_e) = (region(0), region(1));
    let (frames_s, frames_e) = (region(3), region(4));
    let (mut act_s, act_e) = (region(4), region(5));
    let n = r.b.len();
    if objdir_e > n || frames_e > n || act_e > n || objdir_s >= objdir_e || act_s >= act_e {
        return empty();
    }
    // Classic-Mac files prepend a QuickTime image description (`u32 size`, then the `'smc '`
    // codec tag) to the action region; step over it to reach the action table.
    if r.b.get(act_s + 4..act_s + 8) == Some(b"smc ") {
        act_s = (act_s + r.u32(act_s) as usize).min(act_e);
    }

    // Object directory: one u32 per object. Low 30 bits = byte offset into the artwork pool
    // (relative to `art_base`); top 2 bits = sub-image selector (for multi-image MNAK blocks).
    let nobj = (objdir_e - objdir_s) / 4;
    let mut cel_at: std::collections::HashMap<(usize, u16), usize> =
        std::collections::HashMap::new();
    for (i, c) in cels.iter().enumerate() {
        if let Some(off) = c.offset.checked_sub(art_base) {
            cel_at.entry((off, c.sub)).or_insert(i);
        }
    }
    // Distinct artwork offsets, ascending: a pose object spans [offset, next-distinct-offset).
    // The pool ends where the object directory begins.
    let mut distinct: Vec<usize> = (0..nobj)
        .map(|k| (r.u32(objdir_s + k * 4) & 0x3FFF_FFFF) as usize)
        .collect();
    distinct.sort_unstable();
    distinct.dedup();
    let pool_end = objdir_s.saturating_sub(art_base);
    let next_distinct = |off: usize| -> usize {
        match distinct.binary_search(&off) {
            Ok(i) => distinct.get(i + 1).copied().unwrap_or(pool_end),
            Err(i) => distinct.get(i).copied().unwrap_or(pool_end),
        }
    };

    let mut objects = Vec::with_capacity(nobj);
    let mut poses: Vec<Pose> = Vec::new();
    let mut pose_at: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
    for k in 0..nobj {
        let entry = r.u32(objdir_s + k * 4);
        let off = (entry & 0x3FFF_FFFF) as usize;
        let sub = (entry >> 30) as u16;
        if let Some(&ci) = cel_at.get(&(off, sub)) {
            objects.push(ObjRef::Cel(ci as u32));
            continue;
        }
        let s = art_base + off;
        // A pose object begins with the composite type word (0x0014).
        if s + 4 <= n && r.u16(s) == 0x0014 {
            let idx = *pose_at.entry(off).or_insert_with(|| {
                let e = (art_base + next_distinct(off)).min(n);
                poses.push(parse_pose(r, s, e));
                (poses.len() - 1) as u32
            });
            objects.push(ObjRef::Pose(idx));
        } else {
            objects.push(ObjRef::Empty);
        }
    }

    // Action table: u16 count, u16 pad, then `count` 6-byte (id, nVariants, firstPtrIndex)
    // headers sorted by id, then a shared array of 6-byte (u32 frameOffset, u16 length)
    // frame-pointer records. Variant v of an action is pointer index `count + firstPtrIndex + v`.
    let count = r.u16(act_s) as usize;
    let hdr = act_s + 4;
    let ptr0 = hdr + count * 6;
    let mut actions = Vec::new();
    if count > 0 && count < 4096 && ptr0 <= act_e {
        for i in 0..count {
            let o = hdr + i * 6;
            let id = r.u16(o);
            let nvar = r.u16(o + 2) as usize;
            let first = r.u16(o + 4) as usize;
            let mut variants = Vec::new();
            for v in 0..nvar.min(64) {
                let pr = ptr0 + (first + v) * 6;
                if pr + 6 > act_e {
                    break;
                }
                let off = r.u32(pr) as usize;
                let len = r.u16(pr + 4) as usize;
                let blob = frames_s + off;
                if blob + 4 > frames_e || blob + len > frames_e {
                    continue;
                }
                variants.push(parse_animation_blob(r, blob, len));
            }
            if variants.is_empty() {
                continue;
            }
            let name = action_name(id)
                .map(String::from)
                .unwrap_or_else(|| format!("Action{id}"));
            actions.push(Action { id, name, variants });
        }
    }

    (objects, poses, actions)
}

/// Parse a pose (composite) object: `u16 type(0x14)`, `u16 partCount`, then 10-byte parts
/// `(u16 object, i16 left, i16 top, i16 right, i16 bottom)` — each a source object placed at a
/// destination rectangle in twips.
fn parse_pose(r: &Rdr, s: usize, e: usize) -> Pose {
    let count = r.u16(s + 2) as usize;
    let mut parts = Vec::with_capacity(count);
    for p in 0..count {
        let po = s + 4 + p * 10;
        if po + 10 > e {
            break;
        }
        parts.push(Part {
            image: r.u16(po),
            rect: (r.i16(po + 2), r.i16(po + 4), r.i16(po + 6), r.i16(po + 8)),
        });
    }
    Pose { parts }
}

/// Decode one animation variant: `u16 0x0100` marker, `u16 opCount`, then `opCount` 6-byte
/// records `(u16 opcode, u16, u16)`. Opcodes: 0 show, 1 random branch, 2 sound, 3 loop
/// branch, 4 state branch.
fn parse_animation_blob(r: &Rdr, blob: usize, len: usize) -> Animation {
    // Clamp the declared op count to what the blob length can hold (4-byte header + 6/op).
    let count = (r.u16(blob + 2) as usize).min(len.saturating_sub(4) / 6);
    let mut ops = Vec::with_capacity(count);
    for k in 0..count {
        let o = blob + 4 + k * 6;
        let (op, a, b) = (r.u16(o), r.u16(o + 2), r.u16(o + 4));
        ops.push(match op {
            0 => Op::Show {
                object: a,
                duration_ms: b,
            },
            1 => Op::Branch {
                target: a,
                weight: b,
            },
            2 => Op::Sound {
                id: a,
                volume: (b & 0xff) as u8,
                pan: (b >> 8) as u8,
            },
            3 => Op::LoopBranch {
                target: a,
                count: b,
            },
            4 => Op::StateBranch {
                target: a,
                state: b,
            },
            // Unknown opcode: fall through (weight-0 branch keeps op indices aligned).
            _ => Op::Branch {
                target: 0,
                weight: 0,
            },
        });
    }
    Animation { ops }
}

/// Minimal interpreter for the Windows Metafile subset used by actor cels: window
/// mapping, indirect pen/brush objects, object selection, polygon fill mode, and
/// filled polygons / polylines. Enough to draw the characters; not a general WMF engine.
mod wmf {
    use crate::model::Rgba;

    // WMF record function numbers.
    const META_SETWINDOWORG: u16 = 0x020B;
    const META_SETWINDOWEXT: u16 = 0x020C;
    const META_SETPOLYFILLMODE: u16 = 0x0106;
    const META_CREATEPENINDIRECT: u16 = 0x02FA;
    const META_CREATEBRUSHINDIRECT: u16 = 0x02FC;
    const META_SELECTOBJECT: u16 = 0x012D;
    const META_DELETEOBJECT: u16 = 0x01F0;
    const META_POLYGON: u16 = 0x0324;
    const META_POLYLINE: u16 = 0x0325;
    const META_POLYPOLYGON: u16 = 0x0538;
    const META_EOF: u16 = 0x0000;

    const BS_NULL: u16 = 1;

    #[derive(Clone, Copy)]
    enum Obj {
        /// A pen. We don't stroke outlines (fills carry the shapes), but pens still occupy
        /// a handle-table slot — pens and brushes share one table, so slot indices, and
        /// therefore `SELECTOBJECT`, depend on counting them.
        Pen,
        Brush {
            style: u16,
            color: [u8; 3],
        },
    }

    struct R<'a> {
        b: &'a [u8],
        be: bool,
    }
    impl R<'_> {
        fn u16(&self, p: usize) -> u16 {
            let b = self.b.get(p..p + 2).unwrap_or(&[0, 0]);
            if self.be {
                u16::from_be_bytes([b[0], b[1]])
            } else {
                u16::from_le_bytes([b[0], b[1]])
            }
        }
        fn i16(&self, p: usize) -> i16 {
            self.u16(p) as i16
        }
        fn u32(&self, p: usize) -> u32 {
            let b = self.b.get(p..p + 4).unwrap_or(&[0, 0, 0, 0]);
            if self.be {
                u32::from_be_bytes([b[0], b[1], b[2], b[3]])
            } else {
                u32::from_le_bytes([b[0], b[1], b[2], b[3]])
            }
        }
    }

    /// Rasterize a placeable-WMF cel to top-down RGBA sized to its bounding box.
    pub fn render(bytes: &[u8], be: bool) -> Option<Rgba> {
        let r = R { b: bytes, be };
        if r.u32(0) != 0x9AC6_CDD7 {
            return None;
        }
        // Placeable header: key(4) handle(2) L T R B (i16) inch(2) reserved(4) checksum(2).
        let (left, top, right, bottom) = (r.i16(6), r.i16(8), r.i16(10), r.i16(12));
        let w = (right - left).unsigned_abs() as usize + 1;
        let h = (bottom - top).unsigned_abs() as usize + 1;
        if w == 0 || h == 0 || w > 4096 || h > 4096 {
            return None;
        }
        // Standard metafile header follows the 22-byte placeable header; its size field is
        // in words (always 9 words = 18 bytes here).
        let mut p = 22 + r.u16(22 + 2) as usize * 2;

        let mut px = vec![0u8; w * h * 4];
        let mut objects: Vec<Option<Obj>> = Vec::new();
        let mut brush: Option<[u8; 3]> = Some([0, 0, 0]);
        let mut winding = false;
        // MM_ANISOTROPIC window→viewport mapping. Cels map their logical window onto the
        // output bitmap. Default the window to the placeable bounding box, so cels that
        // omit SETWINDOWORG/EXT (e.g. all the Actor 1.0 / Rover cels) still map their
        // bbox-space polygon coordinates onto the frame. An explicit SETWINDOWORG/EXT
        // overrides this (e.g. Clippit's eye cels draw in a 360×200 window scaled into a
        // ~46×26 frame). Window params are stored (y, x).
        let mut win_org = (left.min(right) as i32, top.min(bottom) as i32);
        let mut win_ext = (w as i32, h as i32);

        while p + 6 <= bytes.len() {
            let size = r.u32(p) as usize; // record size in 16-bit words
            let func = r.u16(p + 4);
            if func == META_EOF || size < 3 {
                break;
            }
            let params = p + 6; // first parameter word
            match func {
                META_SETWINDOWORG => win_org = (r.i16(params + 2) as i32, r.i16(params) as i32),
                META_SETWINDOWEXT => win_ext = (r.i16(params + 2) as i32, r.i16(params) as i32),
                META_SETPOLYFILLMODE => winding = r.u16(params) == 2,
                META_CREATEPENINDIRECT => insert_object(&mut objects, Obj::Pen),
                META_CREATEBRUSHINDIRECT => {
                    // LOGBRUSH: style(u16), color COLORREF(R,G,B,0), hatch(u16).
                    let style = r.u16(params);
                    let color = [
                        bytes.get(params + 2).copied().unwrap_or(0),
                        bytes.get(params + 3).copied().unwrap_or(0),
                        bytes.get(params + 4).copied().unwrap_or(0),
                    ];
                    insert_object(&mut objects, Obj::Brush { style, color });
                }
                META_SELECTOBJECT => {
                    let idx = r.u16(params) as usize;
                    if let Some(Some(obj)) = objects.get(idx) {
                        match *obj {
                            Obj::Brush { style, color } => {
                                brush = if style == BS_NULL { None } else { Some(color) };
                            }
                            Obj::Pen => {}
                        }
                    }
                }
                META_DELETEOBJECT => {
                    let idx = r.u16(params) as usize;
                    if let Some(slot) = objects.get_mut(idx) {
                        *slot = None;
                    }
                }
                META_POLYGON => {
                    if let Some(color) = brush {
                        let n = r.u16(params) as usize;
                        let pts = read_points(&r, params + 2, n, win_org, win_ext, w, h);
                        fill_polygon(&mut px, w, h, &pts, color, winding);
                    }
                }
                META_POLYPOLYGON => {
                    if let Some(color) = brush {
                        let count = r.u16(params) as usize;
                        let mut counts = Vec::with_capacity(count);
                        for k in 0..count {
                            counts.push(r.u16(params + 2 + k * 2) as usize);
                        }
                        let mut pp = params + 2 + count * 2;
                        for c in counts {
                            let pts = read_points(&r, pp, c, win_org, win_ext, w, h);
                            fill_polygon(&mut px, w, h, &pts, color, winding);
                            pp += c * 4;
                        }
                    }
                }
                META_POLYLINE => {} // outline-only; ignored (fills carry the shape)
                _ => {}
            }
            p += size * 2;
        }

        Some(Rgba {
            width: w as u32,
            height: h as u32,
            pixels: px,
        })
    }

    fn insert_object(objects: &mut Vec<Option<Obj>>, obj: Obj) {
        // WMF places a new object in the first free handle slot.
        if let Some(slot) = objects.iter_mut().find(|s| s.is_none()) {
            *slot = Some(obj);
        } else {
            objects.push(Some(obj));
        }
    }

    /// Read `n` polygon points, mapping each from the metafile's logical window
    /// (`win_org`/`win_ext`) onto the `w`×`h` output bitmap (MM_ANISOTROPIC viewport).
    fn read_points(
        r: &R,
        base: usize,
        n: usize,
        win_org: (i32, i32),
        win_ext: (i32, i32),
        w: usize,
        h: usize,
    ) -> Vec<(i32, i32)> {
        let (ex, ey) = (win_ext.0, win_ext.1);
        let mut pts = Vec::with_capacity(n);
        for k in 0..n {
            let lx = r.i16(base + k * 4) as i32 - win_org.0;
            let ly = r.i16(base + k * 4 + 2) as i32 - win_org.1;
            let x = if ex != 0 { lx * w as i32 / ex } else { lx };
            let y = if ey != 0 { ly * h as i32 / ey } else { ly };
            pts.push((x, y));
        }
        pts
    }

    /// Scanline polygon fill (even-odd or nonzero winding) into a top-down RGBA buffer.
    fn fill_polygon(
        px: &mut [u8],
        w: usize,
        h: usize,
        pts: &[(i32, i32)],
        color: [u8; 3],
        winding: bool,
    ) {
        if pts.len() < 3 {
            return;
        }
        let (mut ymin, mut ymax) = (i32::MAX, i32::MIN);
        for &(_, y) in pts {
            ymin = ymin.min(y);
            ymax = ymax.max(y);
        }
        let ymin = ymin.max(0);
        let ymax = ymax.min(h as i32 - 1);
        for y in ymin..=ymax {
            // Collect edge crossings at pixel-center scanline `y`.
            let mut xs: Vec<(f32, i32)> = Vec::new();
            for i in 0..pts.len() {
                let (x1, y1) = pts[i];
                let (x2, y2) = pts[(i + 1) % pts.len()];
                if (y1 <= y && y < y2) || (y2 <= y && y < y1) {
                    let t = (y - y1) as f32 / (y2 - y1) as f32;
                    let x = x1 as f32 + t * (x2 - x1) as f32;
                    let dir = if y2 > y1 { 1 } else { -1 };
                    xs.push((x, dir));
                }
            }
            if xs.len() < 2 {
                continue;
            }
            xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            if winding {
                let mut wind = 0;
                let mut i = 0;
                while i < xs.len() {
                    let start_wind = wind;
                    wind += xs[i].1;
                    if start_wind == 0 && wind != 0 {
                        // span opens here; find where it closes
                        let xa = xs[i].0;
                        let mut j = i + 1;
                        let mut ww = wind;
                        while j < xs.len() && ww != 0 {
                            ww += xs[j].1;
                            j += 1;
                        }
                        let xb = xs.get(j - 1).map(|e| e.0).unwrap_or(xa);
                        span(px, w, y, xa, xb, color);
                        wind = ww;
                        i = j;
                    } else {
                        i += 1;
                    }
                }
            } else {
                let mut i = 0;
                while i + 1 < xs.len() {
                    span(px, w, y, xs[i].0, xs[i + 1].0, color);
                    i += 2;
                }
            }
        }
    }

    fn span(px: &mut [u8], w: usize, y: i32, xa: f32, xb: f32, color: [u8; 3]) {
        let xa = xa.round().max(0.0) as i32;
        let xb = (xb.round() as i32).min(w as i32);
        for x in xa..xb {
            let o = (y as usize * w + x as usize) * 4;
            px[o] = color[0];
            px[o + 1] = color[1];
            px[o + 2] = color[2];
            px[o + 3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_subslice_basic() {
        assert_eq!(find_subslice(b"abcdef", b"cd"), Some(2));
        assert_eq!(find_subslice(b"abcdef", b"xy"), None);
    }

    #[test]
    fn extracts_wave_streams() {
        let mut buf = vec![0u8; 4];
        // one 16-byte RIFF/WAVE (size = 8 covering "WAVE" + 4 bytes)
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&8u32.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(&[1, 2, 3, 4]);
        buf.extend_from_slice(&[0u8; 3]);
        let waves = extract_wave_streams(&buf);
        assert_eq!(waves.len(), 1);
        assert_eq!(&waves[0][0..4], b"RIFF");
        assert_eq!(waves[0].len(), 16);
    }
}
