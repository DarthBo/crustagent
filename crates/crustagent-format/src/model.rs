//! Portable, engine-agnostic data model for an Agent character.
//!
//! These types are the parsed, in-memory representation shared by all file formats
//! (ACS 2.0/1.5, ACF, ACD). They intentionally carry no rendering/runtime state —
//! that belongs to `crustagent-core`.

use std::fmt;

/// A Windows `GUID`, stored as its on-disk 16 bytes.
///
/// On disk: `u32 Data1` (LE), `u16 Data2` (LE), `u16 Data3` (LE), `[u8; 8] Data4`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    /// The all-zero GUID.
    pub const NIL: Guid = Guid([0; 16]);

    /// True if this is the nil GUID.
    pub fn is_nil(&self) -> bool {
        self.0 == [0; 16]
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        let d1 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let d2 = u16::from_le_bytes([b[4], b[5]]);
        let d3 = u16::from_le_bytes([b[6], b[7]]);
        write!(
            f,
            "{{{d1:08X}-{d2:04X}-{d3:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
        )
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

/// An opaque 24-bit RGB color (the alpha of the character comes from the transparency
/// palette index, handled at composite time, not here).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Character style flags (`FileHeader::style`).
pub mod char_style {
    pub const TTS: u32 = 0x0000_0020;
    pub const BALLOON: u32 = 0x0000_0200;
    pub const SIZE_TO_TEXT: u32 = 0x0001_0000;
    pub const NO_AUTO_HIDE: u32 = 0x0002_0000;
    pub const NO_AUTO_PACE: u32 = 0x0004_0000;
    pub const STANDARD: u32 = 0x0010_0000;
}

/// Which mouth image an overlay supplies, for lip-sync.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum MouthOverlay {
    Closed = 0,
    Wide1 = 1,
    Wide2 = 2,
    Wide3 = 3,
    Wide4 = 4,
    Medium = 5,
    Narrow = 6,
}

impl MouthOverlay {
    /// Map a raw on-disk overlay-type byte to a [`MouthOverlay`]. Unknown values
    /// clamp to `Closed` (matches the tolerant behavior of the original loader).
    pub fn from_u8(v: u8) -> MouthOverlay {
        match v {
            1 => MouthOverlay::Wide1,
            2 => MouthOverlay::Wide2,
            3 => MouthOverlay::Wide3,
            4 => MouthOverlay::Wide4,
            5 => MouthOverlay::Medium,
            6 => MouthOverlay::Narrow,
            _ => MouthOverlay::Closed,
        }
    }
}

/// General character information (the fixed header + palette + transparency).
#[derive(Clone, Debug)]
pub struct FileHeader {
    pub version_major: u16,
    pub version_minor: u16,
    pub guid: Guid,
    /// Default character frame size, in pixels (width, height).
    pub image_size: (u16, u16),
    /// Palette index treated as transparent when compositing.
    pub transparency: u8,
    pub style: u32,
    /// Up to 256 palette entries.
    pub palette: Vec<Color>,
}

/// The character's default text-to-speech voice.
#[derive(Clone, Debug)]
pub struct Tts {
    pub engine: Guid,
    pub mode: Guid,
    pub speed: i32,
    pub pitch: i16,
    /// Present only when the file carries the extended TTS block.
    pub language: Option<u16>,
    pub gender: u16,
    pub age: u16,
    pub style: String,
}

/// Default word-balloon appearance.
#[derive(Clone, Debug)]
pub struct Balloon {
    pub lines: u8,
    pub per_line: u8,
    pub fg_color: Color,
    pub bg_color: Color,
    pub border_color: Color,
    pub font_name: String,
    /// `LOGFONT.lfHeight` (signed; negative = character height in device units).
    pub font_height: i32,
    pub bold: bool,
    pub strikeout: bool,
    pub italic: bool,
}

/// A localized character name + optional descriptions.
#[derive(Clone, Debug)]
pub struct Name {
    /// Windows `LANGID`.
    pub language: u16,
    pub name: String,
    pub desc1: String,
    pub desc2: String,
}

/// A named state → the ordered list of animation names the engine plays for it
/// (e.g. `"SPEAKING"` → `["Speak", "Explain"]`).
#[derive(Clone, Debug)]
pub struct State {
    pub name: String,
    pub animations: Vec<String>,
}

/// One branch target within a frame.
#[derive(Clone, Copy, Debug)]
pub struct Branch {
    pub frame_ndx: i16,
    /// Cumulative-percentage weight (1..=100).
    pub probability: u16,
}

/// One base-image layer within a frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameImage {
    /// Index into the file's image table.
    pub image_ndx: u32,
    pub offset: (i16, i16),
}

/// One mouth-overlay layer within a frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameOverlay {
    pub overlay_type: MouthOverlay,
    pub image_ndx: u16,
    /// When set, the overlay replaces the frame's base image (index 0).
    pub replace: bool,
    pub offset: (i16, i16),
}

/// A single animation frame.
#[derive(Clone, Debug)]
pub struct Frame {
    /// On-screen time, in centiseconds (1/100 s).
    pub duration: u16,
    /// Index into the sound table, or `-1` for none.
    pub sound_ndx: i16,
    pub exit_frame: i16,
    /// Up to 3 probabilistic branch targets.
    pub branching: Vec<Branch>,
    pub images: Vec<FrameImage>,
    pub overlays: Vec<FrameOverlay>,
}

/// How an animation returns to the neutral pose when it ends or is interrupted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReturnKind {
    /// Follow the frames' exit branching.
    ExitBranching,
    /// No return.
    None,
    /// Play the named return animation.
    Named,
}

impl ReturnKind {
    /// Interpret the raw `returnType` byte.
    pub fn from_u8(v: u8) -> ReturnKind {
        match v {
            1 => ReturnKind::ExitBranching,
            2 => ReturnKind::None,
            _ => ReturnKind::Named,
        }
    }
}

/// A named animation clip (a "gesture").
#[derive(Clone, Debug)]
pub struct Animation {
    pub name: String,
    pub return_kind: ReturnKind,
    /// Return animation name (meaningful when `return_kind == Named`).
    pub return_name: String,
    pub frames: Vec<Frame>,
}

/// A top-down, non-premultiplied RGBA8 image (row 0 is the top). This is the output of
/// frame compositing and the natural input to a PNG encoder or GPU texture upload.
#[derive(Clone, Debug)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, row-major, top-down, `[r, g, b, a]` per pixel.
    pub pixels: Vec<u8>,
}

impl Rgba {
    /// Allocate a fully-transparent image.
    pub fn transparent(width: u32, height: u32) -> Rgba {
        Rgba {
            width,
            height,
            pixels: vec![0u8; (width as usize) * (height as usize) * 4],
        }
    }

    /// True if every pixel is fully transparent.
    pub fn is_fully_transparent(&self) -> bool {
        self.pixels.iter().skip(3).step_by(4).all(|&a| a == 0)
    }
}

/// A top-down, palette-indexed composited frame (row 0 is the top). Pixels equal to
/// `transparent` are the transparent color key. This is the natural form for GIF export
/// (palette + single transparent index) and the intermediate for [`Rgba`] conversion.
#[derive(Clone, Debug)]
pub struct Indexed {
    pub width: u32,
    pub height: u32,
    /// `width * height` palette indices, row-major, top-down.
    pub indices: Vec<u8>,
    /// The palette index that means "transparent".
    pub transparent: u8,
}

impl Indexed {
    /// Allocate a fully-transparent (filled with the transparent index) canvas.
    pub fn filled(width: u32, height: u32, transparent: u8) -> Indexed {
        Indexed {
            width,
            height,
            indices: vec![transparent; (width as usize) * (height as usize)],
            transparent,
        }
    }

    /// Map to top-down RGBA8 using `palette` (transparent index → transparent pixel).
    pub fn to_rgba(&self, palette: &[Color]) -> Rgba {
        let mut px = vec![0u8; self.indices.len() * 4];
        for (i, &idx) in self.indices.iter().enumerate() {
            let o = i * 4;
            if idx == self.transparent {
                continue; // already 0,0,0,0
            }
            let c = palette.get(idx as usize).copied().unwrap_or_default();
            px[o] = c.r;
            px[o + 1] = c.g;
            px[o + 2] = c.b;
            px[o + 3] = 255;
        }
        Rgba {
            width: self.width,
            height: self.height,
            pixels: px,
        }
    }
}

/// A decoded image: 8-bpp palette indices in a bottom-up DIB with 4-byte-aligned rows.
#[derive(Clone, Debug)]
pub struct Image {
    /// Zero-based index in the file's image table.
    pub index: usize,
    pub width: u16,
    pub height: u16,
    /// Palette-index bytes. Row stride is `((width + 3) / 4) * 4`; rows are bottom-up.
    pub bits: Vec<u8>,
}

impl Image {
    /// Row stride in bytes (4-byte aligned).
    pub fn stride(&self) -> usize {
        (self.width as usize).div_ceil(4) * 4
    }

    /// Expected size of `bits` for the given dimensions.
    pub fn expected_len(width: u16, height: u16) -> usize {
        (width as usize).div_ceil(4) * 4 * (height as usize)
    }

    /// Convert this single image to top-down RGBA8 using `palette`, mapping the
    /// `transparency` palette index to a fully transparent pixel.
    ///
    /// This is a convenience for previewing one image; full frame compositing
    /// (layering + overlays + offsets) lives in `crustagent-core`.
    pub fn to_rgba(&self, palette: &[Color], transparency: u8) -> Vec<u8> {
        let w = self.width as usize;
        let h = self.height as usize;
        let stride = self.stride();
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            // Source is bottom-up: source row (h-1-y) maps to output row y.
            let src_row = (h - 1 - y) * stride;
            for x in 0..w {
                let idx = self.bits.get(src_row + x).copied().unwrap_or(transparency);
                let o = (y * w + x) * 4;
                if idx == transparency {
                    out[o] = 0;
                    out[o + 1] = 0;
                    out[o + 2] = 0;
                    out[o + 3] = 0;
                } else {
                    let c = palette.get(idx as usize).copied().unwrap_or_default();
                    out[o] = c.r;
                    out[o + 1] = c.g;
                    out[o + 2] = c.b;
                    out[o + 3] = 255;
                }
            }
        }
        out
    }
}
