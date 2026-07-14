//! Simple software drawing into a top-down RGBA8 buffer: scaled sprite blit, filled
//! rectangles, and 8x8 bitmap text (via `font8x8`) for the balloon and menu.

use crustagent::Request;
use font8x8::legacy::BASIC_LEGACY;

const BSCALE: i32 = 2;
const MENU_SCALE: i32 = 2;
const PAD: i32 = 6;
const TAIL_LEN: i32 = 9;

/// The window size (physical px) needed to hold a balloon with `cols`×`rows` characters,
/// including padding and the tail.
pub fn balloon_size(cols: usize, rows: usize) -> (u32, u32) {
    let bw = cols as i32 * 8 * BSCALE + PAD * 2 + 2;
    let bh = rows as i32 * 8 * BSCALE + PAD * 2 + TAIL_LEN + 2;
    (bw.max(16) as u32, bh.max(16) as u32)
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

    /// Draw a speech balloon whose tail points at `(tip_x, tip_y)` — the character's head
    /// (`below == false`, balloon sits above, tail down) or chin (`below == true`, balloon
    /// sits below, tail up). The tail merges into the body, the balloon is kept on-window,
    /// and the tail leans toward `tip_x` so it stays aimed at the character.
    pub fn balloon(&mut self, lines: &[String], tip_x: i32, tip_y: i32, below: bool) {
        let bg = [0xFF, 0xFF, 0xE8];
        let border = [0x40, 0x40, 0x40];
        let text = [0x10, 0x10, 0x10];

        let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as i32;
        let rows = lines.len() as i32;
        if rows == 0 {
            return;
        }
        let pad = PAD;
        let bw = cols * 8 * BSCALE + pad * 2;
        let bh = rows * 8 * BSCALE + pad * 2;
        let tail_half = 6;
        let tail_len = TAIL_LEN;

        // Body position: centered on the tip, clamped to stay on the window.
        let bx = (tip_x - bw / 2).clamp(2, (self.w - bw - 2).max(2));
        let by = if below {
            tip_y + tail_len
        } else {
            tip_y - tail_len - bh
        };
        // Tail attaches at the body edge nearest the tip; its base leans toward tip_x.
        let attach_y = if below { by } else { by + bh - 1 };
        let far_y = if below { by + bh - 1 } else { by };
        let tcx = tip_x.clamp(bx + tail_half + 3, bx + bw - tail_half - 3);

        // body + tail fill
        self.fill_rect(bx, by, bw, bh, bg);
        for row in 0..=tail_len {
            let half = tail_half - row * tail_half / tail_len;
            let y = if below { attach_y - row } else { attach_y + row };
            self.fill_rect(tcx - half, y, half * 2 + 1, 1, bg);
            self.put(tcx - half, y, border);
            self.put(tcx + half, y, border);
        }

        // outline: sides, the far edge in full, and the attach edge with a tail gap
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

        for (i, line) in lines.iter().enumerate() {
            self.text(bx + pad, by + pad + i as i32 * 8 * BSCALE, BSCALE, line, text);
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
