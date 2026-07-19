//! Simple software drawing into a top-down RGBA8 buffer: scaled sprite blit and an 8x8
//! bitmap font (via `font8x8`) for the menu. Word-balloon painting (rounded body, tails,
//! anti-aliased TrueType text) lives in the `crustagent-balloon` crate.

use font8x8::legacy::BASIC_LEGACY;

pub const MENU_SCALE: i32 = 2;
/// Height of one menu row, in px.
pub const MENU_ROW_H: i32 = 8 * MENU_SCALE + 6;

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
