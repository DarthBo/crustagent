//! # crustagent-balloon
//!
//! Software rendering for Microsoft Agent **word balloons** — the pixels behind
//! [`crustagent_core::BalloonLayout`] / `crustagent::BalloonView`. Given the already-wrapped
//! lines, colors, and a speech-vs-think flag, it paints a rounded balloon (pointed speech
//! tail or a trail of thought bubbles) with anti-aliased TrueType text (via `fontdue`, face
//! discovered by `fontdb`) into a top-down RGBA8 buffer. No windowing, no GPU — the caller
//! blits/uploads the buffer.
//!
//! Two entry points:
//! - [`paint_balloon`] sizes a fresh buffer to the text and returns a [`BalloonImage`].
//! - [`paint_into`] paints into a caller-provided buffer of a known size.
//!
//! [`balloon_size`] computes the pixel size for a given line set (to size a window up front).
//!
//! ```no_run
//! use crustagent_balloon::{paint_balloon, BalloonPaint, Font};
//! let font = Font::system("Arial", 30.0, false, false);
//! let img = paint_balloon(
//!     &["Hello there!".to_string()],
//!     0, 1, false,
//!     &BalloonPaint { bg: [255, 255, 225], border: [0, 0, 0], text: [0, 0, 0], think: false },
//!     font.as_ref(),
//!     2.0,
//! );
//! // img.rgba is img.width * img.height * 4 bytes, top-down, [r,g,b,a].
//! ```

use font8x8::legacy::BASIC_LEGACY;

/// Bitmap-font scale for the no-TrueType fallback path.
const BSCALE: i32 = 2;
const PAD: i32 = 6;
const TAIL_LEN: i32 = 9;
/// Thought-balloon bubble radii (at scale 1.0), largest nearest the body.
const THINK_BUBBLES: [f32; 3] = [4.5, 3.0, 2.0];

/// A real, anti-aliased text font: a system TrueType face rasterized at a pixel size.
pub struct Font {
    face: fontdue::Font,
    px: f32,
    ascent: f32,
    line_h: f32,
    avg_advance: i32,
}

impl Font {
    /// Find a system font for `family` (falling back through common cross-platform sans
    /// families, then any installed face) and load it at `px` pixels. `None` if the system
    /// has no usable fonts.
    pub fn system(family: &str, px: f32, bold: bool, italic: bool) -> Option<Font> {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let weight = if bold {
            fontdb::Weight::BOLD
        } else {
            fontdb::Weight::NORMAL
        };
        let style = if italic {
            fontdb::Style::Italic
        } else {
            fontdb::Style::Normal
        };

        let mut names: Vec<String> = Vec::new();
        if !family.is_empty() {
            names.push(family.to_string());
        }
        names.extend(
            [
                "Arial",
                "Helvetica",
                "Helvetica Neue",
                "Segoe UI",
                "DejaVu Sans",
                "Liberation Sans",
                "Noto Sans",
                "Verdana",
            ]
            .iter()
            .map(|s| s.to_string()),
        );

        let id = names
            .iter()
            .find_map(|n| {
                db.query(&fontdb::Query {
                    families: &[fontdb::Family::Name(n)],
                    weight,
                    stretch: fontdb::Stretch::Normal,
                    style,
                })
            })
            .or_else(|| db.faces().next().map(|f| f.id))?;

        let (data, index) = db.with_face_data(id, |data, index| (data.to_vec(), index))?;
        Font::from_bytes(&data, index, px)
    }

    /// Parse `data` (TTF/OTF, `index` selects a face in a collection) at `px` pixels.
    pub fn from_bytes(data: &[u8], index: u32, px: f32) -> Option<Font> {
        let px = px.max(6.0);
        let face = fontdue::Font::from_bytes(
            data,
            fontdue::FontSettings {
                collection_index: index,
                scale: px,
                ..Default::default()
            },
        )
        .ok()?;
        let lm = face.horizontal_line_metrics(px);
        let ascent = lm.map(|m| m.ascent).unwrap_or(px * 0.8);
        let line_h = lm.map(|m| m.new_line_size).unwrap_or(px * 1.25);
        let avg_advance = face.metrics('x', px).advance_width.round().max(1.0) as i32;
        Some(Font {
            face,
            px,
            ascent,
            line_h,
            avg_advance,
        })
    }

    /// Line-to-line spacing in pixels.
    pub fn line_height(&self) -> i32 {
        self.line_h.ceil() as i32
    }

    /// Typical advance width (of `x`), for sizing a fixed character-count box.
    pub fn avg_advance(&self) -> i32 {
        self.avg_advance
    }

    /// Pixel advance width of `s`.
    pub fn measure(&self, s: &str) -> i32 {
        s.chars()
            .map(|c| self.face.metrics(c, self.px).advance_width)
            .sum::<f32>()
            .ceil() as i32
    }
}

/// Padding around the text, scaled for the display.
fn pad_px(scale: f32) -> i32 {
    (PAD as f32 * scale).round().max(PAD as f32) as i32
}

/// Vertical space reserved for the tail: a short spike for speech, a longer trail of
/// (scaled) bubbles for thought.
fn tail_px(scale: f32, think: bool) -> i32 {
    if think {
        let gap = (2.0 * scale).round() as i32;
        THINK_BUBBLES
            .iter()
            .map(|&r| gap + 2 * (r * scale).round() as i32)
            .sum::<i32>()
            + gap
    } else {
        (TAIL_LEN as f32 * scale).round().max(TAIL_LEN as f32) as i32
    }
}

/// The pixel size needed to hold a balloon, including padding and the tail. Sized to the
/// widest measured `lines`, but at least `min_cols` characters wide (so a fixed-size box
/// with blank placeholder lines still reserves its full width). `scale` is the display
/// scale factor (matches the DPI-sized font); `think` reserves the taller thought-bubble
/// tail. With no `font`, falls back to the 8x8 bitmap metrics.
pub fn balloon_size(
    font: Option<&Font>,
    lines: &[String],
    min_cols: usize,
    rows: usize,
    scale: f32,
    think: bool,
) -> (u32, u32) {
    let (char_w, line_h) = match font {
        Some(f) => (f.avg_advance(), f.line_height()),
        None => (8 * BSCALE, 8 * BSCALE),
    };
    let measured = lines
        .iter()
        .map(|l| match font {
            Some(f) => f.measure(l),
            None => l.chars().count() as i32 * 8 * BSCALE,
        })
        .max()
        .unwrap_or(0);
    let pad = pad_px(scale);
    let text_w = measured.max(min_cols as i32 * char_w);
    let text_h = rows.max(1) as i32 * line_h;
    let bw = text_w + pad * 2 + 2;
    let bh = text_h + pad * 2 + tail_px(scale, think) + 2;
    (bw.max(16) as u32, bh.max(16) as u32)
}

/// Colors + shape for painting a balloon.
pub struct BalloonPaint {
    pub bg: [u8; 3],
    pub border: [u8; 3],
    pub text: [u8; 3],
    /// A thought balloon (bubble-trail tail) vs. a speech balloon (pointed tail).
    pub think: bool,
}

/// A painted balloon: top-down, non-premultiplied RGBA8, `width`×`height`.
pub struct BalloonImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Size a fresh buffer to `lines` (via [`balloon_size`]) and paint the balloon into it.
/// `min_cols`/`rows` reserve a minimum box; `below` points the tail up (balloon under the
/// character) vs down. See [`paint_into`] to paint into your own buffer.
#[allow(clippy::too_many_arguments)]
pub fn paint_balloon(
    lines: &[String],
    min_cols: usize,
    rows: usize,
    below: bool,
    paint: &BalloonPaint,
    font: Option<&Font>,
    scale: f32,
) -> BalloonImage {
    let (w, h) = balloon_size(font, lines, min_cols, rows.max(lines.len()), scale, paint.think);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    paint_into(&mut rgba, w, h, lines, below, paint, font, scale);
    BalloonImage { rgba, width: w, height: h }
}

/// Paint a balloon that fills a caller-provided top-down RGBA8 buffer of size `w`×`h`
/// (must already be sized, e.g. via [`balloon_size`]). Untouched pixels stay as-is, so pass
/// a transparent (zeroed) buffer for a clean balloon.
#[allow(clippy::too_many_arguments)]
pub fn paint_into(
    buf: &mut [u8],
    w: u32,
    h: u32,
    lines: &[String],
    below: bool,
    paint: &BalloonPaint,
    font: Option<&Font>,
    scale: f32,
) {
    Canvas { buf, w: w as i32, h: h as i32 }.balloon(lines, below, paint, font, scale);
}

/// A borrowed RGBA8 drawing target (top-down, non-premultiplied).
struct Canvas<'a> {
    buf: &'a mut [u8],
    w: i32,
    h: i32,
}

impl Canvas<'_> {
    #[inline]
    fn put(&mut self, x: i32, y: i32, rgb: [u8; 3]) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let o = ((y * self.w + x) * 4) as usize;
        self.buf[o] = rgb[0];
        self.buf[o + 1] = rgb[1];
        self.buf[o + 2] = rgb[2];
        self.buf[o + 3] = 0xFF;
    }

    /// Alpha-blend `rgb` over the pixel at `(x, y)` with coverage `a` (0..=255).
    #[inline]
    fn blend(&mut self, x: i32, y: i32, rgb: [u8; 3], a: u8) {
        if a == 0 || x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let o = ((y * self.w + x) * 4) as usize;
        let inv = 255 - a as u16;
        for (k, &c) in rgb.iter().enumerate() {
            self.buf[o + k] = ((c as u16 * a as u16 + self.buf[o + k] as u16 * inv) / 255) as u8;
        }
        self.buf[o + 3] = self.buf[o + 3].max(a);
    }

    /// Draw `s` with a real font, its top edge at `top`, left edge at `x`.
    fn text_font(&mut self, font: &Font, x: i32, top: i32, s: &str, rgb: [u8; 3]) {
        let baseline = top + font.ascent.round() as i32;
        let mut pen = x as f32;
        for c in s.chars() {
            let (m, bitmap) = font.face.rasterize(c, font.px);
            let gx = pen.round() as i32 + m.xmin;
            let gy = baseline - m.height as i32 - m.ymin;
            for row in 0..m.height {
                for col in 0..m.width {
                    self.blend(
                        gx + col as i32,
                        gy + row as i32,
                        rgb,
                        bitmap[row * m.width + col],
                    );
                }
            }
            pen += m.advance_width;
        }
    }

    fn fill_rect(&mut self, x: i32, y: i32, rw: i32, rh: i32, rgb: [u8; 3]) {
        for yy in y..y + rh {
            for xx in x..x + rw {
                self.put(xx, yy, rgb);
            }
        }
    }

    /// One 8x8 bitmap glyph (the no-TrueType fallback), scaled `scale`×.
    fn glyph(&mut self, x: i32, y: i32, scale: i32, ch: char, rgb: [u8; 3]) {
        let code = ch as usize;
        if code >= 128 {
            return;
        }
        for (row, bits) in BASIC_LEGACY[code].iter().enumerate() {
            for col in 0..8 {
                if bits & (1 << col) != 0 {
                    self.fill_rect(x + col * scale, y + row as i32 * scale, scale, scale, rgb);
                }
            }
        }
    }

    fn text_bitmap(&mut self, x: i32, y: i32, scale: i32, s: &str, rgb: [u8; 3]) {
        let mut cx = x;
        for ch in s.chars() {
            self.glyph(cx, y, scale, ch, rgb);
            cx += 8 * scale;
        }
    }

    /// Fill a rectangle with rounded corners of radius `r`.
    fn fill_round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, rgb: [u8; 3]) {
        if w <= 0 || h <= 0 {
            return;
        }
        let r = r.clamp(0, w.min(h) / 2);
        for row in 0..h {
            let inset = if r > 0 && row < r {
                let dy = (r - row) as f32;
                (r as f32 - ((r * r) as f32 - dy * dy).max(0.0).sqrt()).round() as i32
            } else if r > 0 && row >= h - r {
                let dy = (row - (h - 1 - r)) as f32;
                (r as f32 - ((r * r) as f32 - dy * dy).max(0.0).sqrt()).round() as i32
            } else {
                0
            };
            self.fill_rect(x + inset, y + row, (w - 2 * inset).max(0), 1, rgb);
        }
    }

    /// A filled disc of radius `r` at `(cx, cy)` with a one-pixel border ring.
    fn disc(&mut self, cx: i32, cy: i32, r: i32, fill: [u8; 3], border: [u8; 3]) {
        for dy in -r..=r {
            for dx in -r..=r {
                let d2 = dx * dx + dy * dy;
                if d2 <= r * r {
                    let c = if d2 >= (r - 1) * (r - 1) { border } else { fill };
                    self.put(cx + dx, cy + dy, c);
                }
            }
        }
    }

    /// Draw a word balloon filling this (already correctly-sized) canvas, its tail pointing
    /// down to the character's head (`below == false`) or up to its chin (`below == true`).
    /// A **speech** balloon gets a pointed tail merged into the body; a **think** balloon
    /// gets a trail of shrinking bubbles. Text is drawn with `font` (real TrueType) when
    /// present, else the 8x8 bitmap fallback.
    fn balloon(
        &mut self,
        lines: &[String],
        below: bool,
        style: &BalloonPaint,
        font: Option<&Font>,
        scale: f32,
    ) {
        let (bg, border, text) = (style.bg, style.border, style.text);
        let pad = pad_px(scale);
        let tail_len = tail_px(scale, style.think);
        let tail_half = (6.0 * scale).round().max(3.0) as i32;

        // The body fills the canvas minus the tail strip.
        let bx = 0;
        let bw = self.w;
        let by = if below { tail_len } else { 0 };
        let bh = (self.h - tail_len).max(1);
        let tip_x = self.w / 2;
        let attach_y = if below { by } else { by + bh - 1 };
        // Direction from the body edge toward the character (down if the balloon is above).
        let dir = if below { -1 } else { 1 };

        // Rounded body: a border-colored rounded rect with a 1px-smaller bg rect inside,
        // leaving a clean 1px rounded outline.
        let r = (6.0 * scale).round() as i32;
        self.fill_round_rect(bx, by, bw, bh, r, border);
        self.fill_round_rect(bx + 1, by + 1, bw - 2, bh - 2, (r - 1).max(0), bg);

        if style.think {
            // A descending trail of shrinking, separated bubbles.
            let gap = (2.0 * scale).round() as i32;
            let tcx = tip_x.clamp(bx + tail_len, bx + bw - tail_len);
            let mut edge = attach_y;
            for &base in &THINK_BUBBLES {
                let rr = (base * scale).round().max(1.0) as i32;
                edge += dir * (gap + rr);
                self.disc(tcx, edge, rr, bg, border);
                edge += dir * rr;
            }
        } else {
            // Pointed tail, filled from the attach edge so it opens into the rounded body;
            // its two slanted sides are outlined.
            let tcx = tip_x.clamp(bx + tail_half + 3, bx + bw - tail_half - 3);
            for row in 0..=tail_len {
                let half = tail_half - row * tail_half / tail_len;
                let y = attach_y + dir * row;
                self.fill_rect(tcx - half, y, half * 2 + 1, 1, bg);
                self.put(tcx - half, y, border);
                self.put(tcx + half, y, border);
            }
        }

        let line_h = font.map(|f| f.line_height()).unwrap_or(8 * BSCALE);
        for (i, line) in lines.iter().enumerate() {
            let ty = by + pad + i as i32 * line_h;
            match font {
                Some(f) => self.text_font(f, bx + pad, ty, line, text),
                None => self.text_bitmap(bx + pad, ty, BSCALE, line, text),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paints_a_sized_opaque_balloon() {
        // No system font in some CI sandboxes — the 8x8 fallback still paints.
        let img = paint_balloon(
            &["Hi".to_string()],
            0,
            1,
            false,
            &BalloonPaint { bg: [255, 255, 225], border: [0, 0, 0], text: [0, 0, 0], think: false },
            None,
            2.0,
        );
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
        // The body is opaque somewhere (not an all-transparent buffer).
        assert!(img.rgba.iter().skip(3).step_by(4).any(|&a| a == 0xFF));
    }

    #[test]
    fn think_reserves_more_height_than_speak() {
        let lines = ["Hmm".to_string()];
        let speak = balloon_size(None, &lines, 0, 1, 2.0, false);
        let think = balloon_size(None, &lines, 0, 1, 2.0, true);
        assert!(think.1 > speak.1, "thought tail is taller: {think:?} vs {speak:?}");
    }
}
