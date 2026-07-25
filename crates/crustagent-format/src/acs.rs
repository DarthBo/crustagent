//! Parser for the Microsoft Agent 2.0 compiled binary format (`.acs`).
//!
//! **Status:** the byte-level parser is being reimplemented from scratch (clean-room, from
//! [`docs/acs-format.md`](../../../docs/acs-format.md)) so the crate can be relicensed. The
//! public type ([`AcsFile`]), the in-memory constructors ([`AcsFile::from_parts`] /
//! [`AcsFile::from_parts_rgba`]) and the frame compositor are the original, unaffected code;
//! only [`AcsFile::parse`] (and the lazy `.acs`-blob image reader) are stubbed out until the
//! rewrite lands, and return [`Error::Unsupported`].

use crate::error::{Error, Result};
use crate::model::*;

/// Shared message for the temporarily-stubbed `.acs` reader.
const PARSER_STUBBED: &str =
    "the .acs byte-level parser is being reimplemented clean-room and is not yet available; \
     build characters via AcsFile::from_parts / from_parts_rgba in the meantime";

/// First DWORD of an ACS 2.0 file.
pub const ACS_SIGNATURE: u32 = 0xABCD_ABC3;

/// A parsed ACS 2.0 character file.
///
/// While the byte-level parser is being reimplemented, an `AcsFile` is built in memory from
/// already-decoded parts ([`AcsFile::from_parts`] / [`AcsFile::from_parts_rgba`]); its images
/// and sounds live in the pre-decoded pools below.
pub struct AcsFile {
    pub header: FileHeader,
    pub tts: Option<Tts>,
    pub balloon: Option<Balloon>,
    pub names: Vec<Name>,
    pub states: Vec<State>,
    /// Animation names in file order (parallel to `animations`).
    pub gesture_names: Vec<String>,
    pub animations: Vec<Animation>,
    /// Pre-decoded 8-bpp image / WAV sound pools. (When the ACS 2.0 blob parser returns, it
    /// will populate these — or reintroduce a lazy index — behind this same public surface.)
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
    ///
    /// **Temporarily stubbed.** The byte-level ACS/ACF parser is being rewritten clean-room
    /// (see the module docs) and currently returns [`Error::Unsupported`]. The signature is
    /// validated first so callers still get [`Error::BadSignature`] for a non-ACS buffer.
    pub fn parse(data: Vec<u8>) -> Result<AcsFile> {
        let sig = signature(&data).ok_or(Error::UnexpectedEof {
            context: "signature",
            offset: 0,
            needed: 4,
            available: data.len(),
        })?;
        if sig != ACS_SIGNATURE && sig != crate::acs_v15::OLE2_SIGNATURE {
            return Err(Error::BadSignature { found: sig });
        }
        Err(Error::Unsupported(PARSER_STUBBED))
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
        self.images.as_ref().map_or(0, Vec::len)
    }

    /// Fetch image `index` from the pre-decoded 8-bpp pool.
    ///
    /// (The lazy decode-from-`.acs`-blob path was part of the byte-level parser and is
    /// stubbed during the clean-room rewrite; in-memory characters carry a decoded pool.)
    pub fn image(&self, index: usize) -> Result<Image> {
        match &self.images {
            Some(imgs) => imgs.get(index).cloned().ok_or(Error::BadImage { index }),
            None => Err(Error::Unsupported(PARSER_STUBBED)),
        }
    }

    /// Number of sounds in the sound table.
    pub fn sound_count(&self) -> usize {
        self.sounds.as_ref().map_or(0, Vec::len)
    }

    /// Borrow the raw bytes of sound `index` (a complete standalone WAV file).
    pub fn sound(&self, index: usize) -> Option<&[u8]> {
        self.sounds.as_ref()?.get(index).map(|v| v.as_slice())
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
