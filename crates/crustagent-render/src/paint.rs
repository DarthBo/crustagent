//! Simple software drawing into a top-down RGBA8 buffer: scaled sprite blit, filled
//! rectangles, an 8x8 bitmap font (via `font8x8`) for the menu, and real anti-aliased
//! TrueType text (via `fontdue`, with the face discovered by `fontdb`) for the balloon.

use font8x8::legacy::BASIC_LEGACY;

const BSCALE: i32 = 2;

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
pub const MENU_SCALE: i32 = 2;
/// Height of one menu row, in px.
pub const MENU_ROW_H: i32 = 8 * MENU_SCALE + 6;
const PAD: i32 = 6;
const TAIL_LEN: i32 = 9;
/// Thought-balloon bubble radii (at scale 1.0), largest nearest the body.
const THINK_BUBBLES: [f32; 3] = [4.5, 3.0, 2.0];

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

/// The window size (physical px) needed to hold a balloon, including padding and the tail.
/// Sized to the widest measured `lines`, but at least `min_cols` characters wide (so a
/// fixed-size box with blank placeholder lines still reserves its full width). `scale` is
/// the display scale factor (matches the DPI-sized font); `think` reserves the taller
/// thought-bubble tail. With no `font`, falls back to the 8x8 bitmap metrics.
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

/// A borrowed RGBA8 drawing target.
pub struct Canvas<'a> {
    buf: &'a mut [u8],
    w: i32,
    h: i32,
}

impl<'a> Canvas<'a> {
    pub fn new(buf: &'a mut [u8], w: u32, h: u32) -> Canvas<'a> {
        Canvas {
            buf,
            w: w as i32,
            h: h as i32,
        }
    }

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

    fn stroke_rect(&mut self, x: i32, y: i32, rw: i32, rh: i32, rgb: [u8; 3]) {
        for xx in x..x + rw {
            self.put(xx, y, rgb);
            self.put(xx, y + rh - 1, rgb);
        }
        for yy in y..y + rh {
            self.put(x, yy, rgb);
            self.put(x + rw - 1, yy, rgb);
        }
    }

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

    fn text(&mut self, x: i32, y: i32, scale: i32, s: &str, rgb: [u8; 3]) {
        let mut cx = x;
        for ch in s.chars() {
            self.glyph(cx, y, scale, ch, rgb);
            cx += 8 * scale;
        }
    }

    /// Blit `img` (top-down RGBA, `cw`×`ch`) at `(ox, oy)` scaled by `scale`, skipping
    /// transparent source pixels.
    pub fn blit_scaled(&mut self, img: &[u8], cw: i32, ch: i32, ox: i32, oy: i32, scale: i32) {
        for sy in 0..ch {
            for sx in 0..cw {
                let p = ((sy * cw + sx) * 4) as usize;
                if img[p + 3] == 0 {
                    continue;
                }
                let rgb = [img[p], img[p + 1], img[p + 2]];
                self.fill_rect(ox + sx * scale, oy + sy * scale, scale, scale, rgb);
            }
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

    /// Draw a word balloon filling this (already correctly-sized) window, its tail pointing
    /// down to the character's head (`below == false`) or up to its chin (`below == true`).
    /// A **speech** balloon gets a pointed tail merged into the body; a **think** balloon
    /// gets a trail of shrinking bubbles. Text is drawn with `font` (real TrueType) when
    /// present, else the 8x8 bitmap fallback.
    pub fn balloon(
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

        // The body fills the window minus the tail strip.
        let bx = 0;
        let bw = self.w;
        let by = if below { tail_len } else { 0 };
        let bh = (self.h - tail_len).max(1);
        let tip_x = self.w / 2;
        let attach_y = if below { by } else { by + bh - 1 };
        // Direction from the body edge toward the character (down if the balloon is above).
        let dir = if below { -1 } else { 1 };

        self.fill_rect(bx, by, bw, bh, bg);

        if style.think {
            // Full border, then a descending trail of shrinking, separated bubbles.
            self.stroke_rect(bx, by, bw, bh, border);
            let gap = (2.0 * scale).round() as i32;
            let tcx = tip_x.clamp(bx + tail_len, bx + bw - tail_len);
            let mut edge = attach_y;
            for &base in &THINK_BUBBLES {
                let r = (base * scale).round().max(1.0) as i32;
                edge += dir * (gap + r);
                self.disc(tcx, edge, r, bg, border);
                edge += dir * r;
            }
        } else {
            // Pointed tail, merged into the body with a border gap on the attach edge.
            let far_y = if below { by + bh - 1 } else { by };
            let tcx = tip_x.clamp(bx + tail_half + 3, bx + bw - tail_half - 3);
            for row in 0..=tail_len {
                let half = tail_half - row * tail_half / tail_len;
                let y = attach_y + dir * row;
                self.fill_rect(tcx - half, y, half * 2 + 1, 1, bg);
                self.put(tcx - half, y, border);
                self.put(tcx + half, y, border);
            }
            for y in by..by + bh {
                self.put(bx, y, border);
                self.put(bx + bw - 1, y, border);
            }
            for x in bx..bx + bw {
                self.put(x, far_y, border);
                if x < tcx - tail_half || x > tcx + tail_half {
                    self.put(x, attach_y, border);
                }
            }
        }

        let line_h = font.map(|f| f.line_height()).unwrap_or(8 * BSCALE);
        for (i, line) in lines.iter().enumerate() {
            let ty = by + pad + i as i32 * line_h;
            match font {
                Some(f) => self.text_font(f, bx + pad, ty, line, text),
                None => self.text(bx + pad, ty, BSCALE, line, text),
            }
        }
    }

    /// Fill the whole canvas with a scrollable menu: a list of `labels` offset by
    /// `scroll` px, with the `hover`ed row highlighted. `MENU_ROW_H` px per row.
    pub fn menu_list(&mut self, labels: &[String], scroll: i32, hover: Option<usize>) {
        let bg = [0xF2, 0xF2, 0xF2];
        let border = [0x30, 0x30, 0x30];
        let text = [0x10, 0x10, 0x10];
        let hi = [0xC8, 0xD8, 0xF0];

        self.fill_rect(0, 0, self.w, self.h, bg);
        for (i, label) in labels.iter().enumerate() {
            let y = 2 + i as i32 * MENU_ROW_H - scroll;
            if y + MENU_ROW_H <= 0 || y >= self.h {
                continue; // off-screen row
            }
            if hover == Some(i) {
                self.fill_rect(0, y, self.w, MENU_ROW_H, hi);
            }
            self.text(6, y + 3, MENU_SCALE, label, text);
        }
        self.stroke_rect(0, 0, self.w, self.h, border);
    }
}
