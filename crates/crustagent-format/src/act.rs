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
//! Newer PC characters (e.g. The Genius) instead store each cel as an LZ-compressed block
//! tagged `MNAK` — the compression is the same bitstream as ACS, so [`ActFile::decompress_cel`]
//! recovers the raw bytes, but the decompressed cel *body* is a further encoding that isn't
//! decoded yet (so those cels can't be rasterized). The classic-Mac artwork codec is a
//! different, still-undecoded form. In all cases the container still parses (identity,
//! palette, sounds) and reports each cel's encoding via [`CelFormat`].
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
use crate::model::{Color, Rgba};

/// Signature bytes of a little-endian (PC) actor file.
pub const ACT_SIGNATURE_LE: [u8; 2] = *b"LP";
/// Signature bytes of a big-endian (classic-Mac) actor file.
pub const ACT_SIGNATURE_BE: [u8; 2] = *b"PL";

/// The Aldus Placeable Metafile magic key (`0x9AC6CDD7`) that begins each WMF cel.
const PLACEABLE_KEY: u32 = 0x9AC6_CDD7;

/// How a cel's artwork is encoded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CelFormat {
    /// Aldus Placeable Windows Metafile (vector). Rasterized by [`ActFile::render_cel`].
    Wmf,
    /// Compressed bitmap form used by some newer PC characters (not yet decoded).
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

/// How an animation frame proceeds after its pose is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Branch {
    /// Advance to the next frame in order.
    Next,
    /// Unconditional jump to another frame index.
    Jump(u16),
    /// Branch to a frame with a probability weight (`0..=100`-ish).
    Probable { target: u16, weight: u16 },
}

/// One animation frame: show an object for a duration, then follow its [`Branch`].
#[derive(Clone, Copy, Debug)]
pub struct AnimFrame {
    /// Object index: `< cels.len()` is a bare image cel; otherwise a pose at
    /// `poses[object - cels.len()]`. Render with [`ActFile::render_object`].
    pub object: u16,
    /// On-screen time in milliseconds.
    pub duration_ms: u16,
    pub branch: Branch,
}

/// A named animation (e.g. `"Greeting"`, `"Thinking"`), referenced by the classic
/// Microsoft Actor action id. `first_frame` indexes [`ActFile::frames`]; playback follows
/// each frame's [`Branch`] from there.
#[derive(Clone, Debug)]
pub struct Action {
    /// Microsoft Actor action id (1 = Idle, 2 = Greeting, 24 = Thinking, …).
    pub id: u16,
    /// Human-readable name for `id`, or `"Action{id}"` if unknown.
    pub name: String,
    /// Number of consecutive frame-list entries (usually 1).
    pub count: u16,
    /// Starting index into [`ActFile::frames`].
    pub first_frame: u16,
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
    /// Poses (layered image parts). Empty unless the artwork is renderable WMF.
    pub poses: Vec<Pose>,
    /// Animation frames (a global graph; actions index into this). Empty for non-WMF.
    pub frames: Vec<AnimFrame>,
    /// Named animations. Empty for non-WMF artwork.
    pub actions: Vec<Action>,
    /// Embedded sound effects, each a complete `RIFF`/`WAVE` byte stream.
    pub sounds: Vec<Vec<u8>>,
    /// The seven section offsets from the header (artwork, sounds, tables, strings).
    pub sections: [u32; 7],
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
        let image_size = find_subslice(&data[..data.len().min(0x200)], &marker)
            .filter(|&pos| pos >= 4)
            .map(|pos| (r.u16(pos - 4), r.u16(pos - 2)))
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
            });
        }

        // Newer PC characters (e.g. The Genius) store no WMF cels; instead each cel is an
        // LZ-compressed block tagged "MNAK". The compression is the very same bitstream as
        // ACS, so we enumerate the blocks and can decompress them (see `decompress_cel`) —
        // though the decompressed cel *body* is a further encoding we don't rasterize yet.
        if cels.is_empty() && !big_endian {
            let mut from = 0usize;
            let mut starts = Vec::new();
            while let Some(rel) = find_subslice(&data[from..art_end.min(data.len())], b"MNAK") {
                starts.push(from + rel);
                from += rel + 4;
            }
            for (i, &start) in starts.iter().enumerate() {
                let end = starts.get(i + 1).copied().unwrap_or(art_end);
                cels.push(Cel {
                    format: CelFormat::Bitmap,
                    offset: start,
                    len: end - start,
                    bounds: None,
                });
            }
        }

        // The artwork encoding, from what was actually found. WMF is the only form we can
        // rasterize; Bitmap (MNAK) can be decompressed but not yet rasterized; the
        // classic-Mac codec isn't decoded at all.
        let image_format = match cels.first().map(|c| c.format) {
            Some(f) => f,
            None if big_endian => CelFormat::MacBitmap,
            None => CelFormat::Bitmap,
        };

        // Animation tables — poses (layered parts), the frame graph, and named actions.
        // Only decoded for the WMF vector characters, whose layout is validated.
        let (poses, frames, actions) = if image_format == CelFormat::Wmf {
            parse_anim_tables(&r, &positions, art_end, &sections)
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
            frames,
            actions,
            sounds,
            sections,
            data,
        })
    }

    /// Raw bytes of cel `index` (the placeable-WMF stream for WMF cels, or the `MNAK`
    /// block for compressed bitmap cels).
    pub fn cel_bytes(&self, index: usize) -> Option<&[u8]> {
        let cel = self.cels.get(index)?;
        self.data.get(cel.offset..cel.offset + cel.len)
    }

    /// Decompress a [`CelFormat::Bitmap`] (`MNAK`) cel to its raw bytes.
    ///
    /// The compression is the same LZ77 bitstream as ACS. The decoded buffer starts with a
    /// small header — `u32 width`, `u32 height`, `u32` (flag) — followed by a cel body whose
    /// pixel encoding is **not yet decoded**, so these cels can't be rasterized
    /// ([`render_cel`](Self::render_cel) returns `None` for them). Exposed so the raw
    /// decoded bytes are available for further reverse-engineering. Returns `None` for
    /// non-`MNAK` cels or if decompression fails.
    pub fn decompress_cel(&self, index: usize) -> Option<Vec<u8>> {
        let cel = self.cels.get(index)?;
        if cel.format != CelFormat::Bitmap {
            return None;
        }
        let block = self.data.get(cel.offset..cel.offset + cel.len)?;
        if block.len() < 24 || &block[0..4] != b"MNAK" {
            return None;
        }
        // MNAK header: tag(4), u32 uncompressed size, u32 count, 3× u32 segment offsets.
        let size = u32::from_le_bytes([block[4], block[5], block[6], block[7]]) as usize;
        crate::decode::decode_data(&block[24..], size).ok()
    }

    /// Rasterize cel `index` to top-down RGBA. Returns `None` for non-WMF cels or if the
    /// metafile cannot be interpreted. The image is sized to the cel's bounding box; use
    /// [`Cel::bounds`] to position it within a composited frame.
    pub fn render_cel(&self, index: usize) -> Option<Rgba> {
        let cel = self.cels.get(index)?;
        if cel.format != CelFormat::Wmf {
            return None;
        }
        let bytes = self.data.get(cel.offset..cel.offset + cel.len)?;
        wmf::render(bytes, self.big_endian)
    }

    /// Composite pose `index` (from [`ActFile::poses`]) into a full character frame, sized
    /// [`ActFile::image_size`]. Parts are drawn in order, each cel placed at its
    /// destination (twips ÷ 15 → pixels). Returns `None` if the pose can't be rendered.
    pub fn render_pose(&self, index: usize) -> Option<Rgba> {
        let pose = self.poses.get(index)?;
        let (cw, ch) = (self.image_size.0 as u32, self.image_size.1 as u32);
        if cw == 0 || ch == 0 {
            return None;
        }
        let mut canvas = Rgba::transparent(cw, ch);
        for part in &pose.parts {
            let Some(cel) = self.render_cel(part.image as usize) else {
                continue;
            };
            let ox = (part.rect.0 as i32) / 15;
            let oy = (part.rect.1 as i32) / 15;
            blit_over(&mut canvas, &cel, ox, oy);
        }
        Some(canvas)
    }

    /// Render an animation frame's object to a full character frame ([`ActFile::image_size`]).
    /// `object >= cels.len()` is a pose (composited). A sentinel (`0xFFFF`) or any other
    /// out-of-range value is a blank/hidden frame → a fully transparent frame. Returns
    /// `None` only if the frame size is unknown.
    pub fn render_object(&self, object: usize) -> Option<Rgba> {
        let pose = object.checked_sub(self.cels.len());
        match pose {
            Some(p) if p < self.poses.len() => self.render_pose(p),
            _ => {
                let (w, h) = (self.image_size.0 as u32, self.image_size.1 as u32);
                (w != 0 && h != 0).then(|| Rgba::transparent(w, h))
            }
        }
    }

    /// Find an action by (case-insensitive) name — e.g. `"Greeting"`, `"Thinking"`.
    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// Resolve an action to a linear list of `(object, duration_ms)` steps by walking the
    /// frame graph from its start frame — advancing, jumping, or taking branches — with a
    /// visit cap so loops terminate. This is the ACT analogue of the ACS sequencer.
    pub fn action_sequence(&self, action: &Action, max_steps: usize) -> Vec<(u16, u16)> {
        let mut out = Vec::new();
        let mut visits = std::collections::HashMap::new();
        let mut i = action.first_frame as usize;
        while out.len() < max_steps {
            let Some(frame) = self.frames.get(i) else {
                break;
            };
            *visits.entry(i).or_insert(0u32) += 1;
            if visits[&i] > 3 {
                break; // looped enough for a preview
            }
            // Zero-duration frames are instantaneous routing nodes (e.g. a branch hub) — walk
            // through them without displaying, so they don't flash on screen.
            if frame.duration_ms > 0 {
                out.push((frame.object, frame.duration_ms));
            }
            i = match frame.branch {
                Branch::Next => i + 1,
                Branch::Jump(t) => t as usize,
                Branch::Probable { target, .. } => target as usize,
            };
        }
        out
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
fn parse_anim_tables(
    r: &Rdr,
    cel_positions: &[usize],
    art_end: usize,
    sections: &[u32; 7],
) -> (Vec<Pose>, Vec<AnimFrame>, Vec<Action>) {
    let empty = || (Vec::new(), Vec::new(), Vec::new());
    let ncel = cel_positions.len();
    if ncel < 3 {
        return empty();
    }
    let base = cel_positions[0];
    let sec0 = sections[0] as usize;
    let lim = (art_end - base) as u32;

    // Locate the object directory inside section[0] by matching the known cel end-offsets
    // (object i's end == the next cel's start). No reliance on a fixed header size.
    let want0 = (cel_positions[1] - base) as u32;
    let want1 = (cel_positions[2] - base) as u32;
    let mut dir_pos = None;
    for q in sec0..(sec0 + 64).min(r.b.len().saturating_sub(8)) {
        if r.u32(q) == want0 && r.u32(q + 4) == want1 {
            dir_pos = Some(q);
            break;
        }
    }
    let Some(mut q) = dir_pos else {
        return empty();
    };
    // Read all object end-offsets (increasing, within the artwork region).
    let mut ends = Vec::new();
    while q + 4 <= r.b.len() {
        let v = r.u32(q);
        if v >= lim || (ends.last().is_some_and(|&p| v <= p)) {
            break;
        }
        ends.push(v);
        q += 4;
    }
    let nobj = ends.len();
    if nobj <= ncel {
        return empty();
    }

    // Poses: objects [ncel, nobj). Object o spans [base + ends[o-1], base + ends[o]).
    let mut poses = Vec::with_capacity(nobj - ncel);
    for o in ncel..nobj {
        let s = base + ends[o - 1] as usize;
        let e = base + ends[o] as usize;
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
        poses.push(Pose { parts });
    }

    // Frame graph: section[3]. Find the frame start by trying offsets until the walk lands
    // exactly on section[4]. Each frame is (object u16, durationMs u16, branchType u16) plus
    // a 6-byte branch when branchType is 1 (jump) or 2 (probable); branchType 0x0100 ends a
    // list and is followed by a u32 to skip. Frames are flattened to one global index space.
    let s3 = sections[3] as usize;
    let s3_end = sections[4] as usize;
    // The frame stream is preceded by a header (a leading word + several cel-pool offsets)
    // terminated by the first end-of-list marker (u16 0x0100) and a u32. Candidate frame
    // starts are just past each such marker; take the first whose walk consumes the section
    // (so the header's cel offsets aren't mis-parsed as frames, which would shift indices).
    let ncel_u16 = ncel as u16;
    let marker_starts: Vec<usize> = (s3..s3_end.saturating_sub(6))
        .step_by(2)
        .filter(|&p| r.u16(p) == 0x0100)
        .map(|p| p + 6)
        // The first frame shows the first pose (object == ncel); this pins the true start
        // so the header's cel offsets aren't mis-parsed as frames (which shifts all indices).
        .filter(|&start| r.u16(start) == ncel_u16)
        .collect();
    let frames = marker_starts.into_iter().find_map(|start| {
        let mut fr = Vec::new();
        let mut p = start;
        while p + 6 <= s3_end {
            let object = r.u16(p);
            let duration_ms = r.u16(p + 2);
            let bt = r.u16(p + 4);
            p += 6;
            if bt == 0x0100 {
                p += 4; // end-of-list marker + count
                continue;
            }
            if bt > 2 {
                return None; // not the frame stream (or wrong start)
            }
            let branch = match bt {
                1 => Branch::Jump(r.u16(p)),
                2 => Branch::Probable {
                    target: r.u16(p),
                    weight: r.u16(p + 2),
                },
                _ => Branch::Next,
            };
            if bt != 0 {
                p += 6;
            }
            fr.push(AnimFrame {
                object,
                duration_ms,
                branch,
            });
        }
        // Accept when the walk consumed the section (the last marker leaves a few bytes).
        (p >= s3_end.saturating_sub(6) && !fr.is_empty()).then_some(fr)
    });
    let Some(frames) = frames else {
        return (poses, Vec::new(), Vec::new());
    };

    // Actions: section[4] u16 stream of (actionId, count, firstFrame) triples. Find the run
    // by scanning for the first triple sequence whose ids/frames all validate.
    let s4 = sections[4] as usize;
    let s4_end = sections[5] as usize;
    let nframes = frames.len() as u16;
    let valid = |aid: u16| action_name(aid).is_some();
    let mut actions = Vec::new();
    let mut best: Vec<Action> = Vec::new();
    let mut start = s4;
    while start + 6 <= s4_end {
        let mut acts = Vec::new();
        let mut p = start;
        while p + 6 <= s4_end {
            let id = r.u16(p);
            let count = r.u16(p + 2);
            let first = r.u16(p + 4);
            if !valid(id) || count == 0 || count > 64 || first >= nframes {
                break;
            }
            acts.push(Action {
                id,
                name: action_name(id).unwrap().to_string(),
                count,
                first_frame: first,
            });
            p += 6;
        }
        if acts.len() > best.len() {
            best = acts;
        }
        start += 2;
    }
    actions.append(&mut best);
    (poses, frames, actions)
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
        // MM_ANISOTROPIC window→viewport mapping. Cels map their logical window (SETWINDOW*)
        // onto the output bitmap; the window need not match the placeable bbox (e.g. eye
        // cels draw in a 360×200 window scaled into a ~46×26 frame). Params are stored
        // (y, x). Default is identity (logical == device).
        let mut win_org = (0i32, 0i32);
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
