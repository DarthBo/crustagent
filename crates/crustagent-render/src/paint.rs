//! Simple software drawing into a top-down RGBA8 buffer: scaled sprite blit, filled
//! rectangles, and 8x8 bitmap text (via `font8x8`) for the balloon and menu.

use crustagent::Request;
use font8x8::legacy::BASIC_LEGACY;

const BSCALE: i32 = 2;
const MENU_SCALE: i32 = 2;

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

    /// Draw a speech balloon centered horizontally near the top, with a downward tail.
    pub fn balloon(&mut self, lines: &[String]) {
        let bg = [0xFF, 0xFF, 0xE8];
        let border = [0x40, 0x40, 0x40];
        let text = [0x10, 0x10, 0x10];

        let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as i32;
        let rows = lines.len() as i32;
        if rows == 0 {
            return;
        }
        let pad = 6;
        let bw = cols * 8 * BSCALE + pad * 2;
        let bh = rows * 8 * BSCALE + pad * 2;
        let bx = (self.w - bw) / 2;
        let by = 6;

        self.fill_rect(bx, by, bw, bh, bg);
        self.stroke_rect(bx, by, bw, bh, border);
        for (i, line) in lines.iter().enumerate() {
            self.text(bx + pad, by + pad + i as i32 * 8 * BSCALE, BSCALE, line, text);
        }

        let tip_x = self.w / 2;
        for row in 0..10 {
            let half = 6 - row.min(6);
            self.fill_rect(tip_x - half, by + bh + row, half * 2 + 1, 1, bg);
        }
    }

    /// Draw a command menu (a bordered list of labels) at `(x, y)`.
    pub fn menu(&mut self, x: i32, y: i32, width: i32, row_h: i32, items: &[(String, Request)]) {
        let bg = [0xF2, 0xF2, 0xF2];
        let border = [0x30, 0x30, 0x30];
        let text = [0x10, 0x10, 0x10];
        let height = items.len() as i32 * row_h + 4;

        self.fill_rect(x, y, width, height, bg);
        self.stroke_rect(x, y, width, height, border);
        for (i, (label, _)) in items.iter().enumerate() {
            self.text(x + 6, y + 4 + i as i32 * row_h, MENU_SCALE, label, text);
        }
    }
}
