// SPDX-License-Identifier: MIT OR Apache-2.0

//! # crustagent-balloon
//!
//! Software rendering for Microsoft Agent **word balloons** — the pixels behind
//! `crustagent_core::BalloonLayout` / `crustagent::BalloonView`. Given the already-wrapped
//! lines, colors, and a speech-vs-think flag, it paints a rounded balloon (pointed speech
//! tail or a trail of thought bubbles) — with antialiased edges and anti-aliased TrueType
//! text (via `fontdue`, face discovered by `fontdb`) — into a top-down RGBA8 buffer. Colour
//! emoji render from the system emoji face for any codepoint the text face lacks (via
//! `swash`, so COLR / CBDT / sbix all work). No windowing, no GPU — the caller blits/uploads
//! the buffer. Paint at the display's scale for crisp results: the shape is antialiased at
//! whatever resolution you render.
//!
//! Two entry points:
//! - [`paint_balloon`] sizes a fresh buffer to the text and returns a [`BalloonImage`].
//! - [`paint_into`] paints into a caller-provided buffer of a known size.
//!
//! [`balloon_size`] computes the pixel size for a given line set (to size a window up front).
//!
//! **Interactive balloons** — a question with clickable choices, check boxes and commit
//! buttons (`crustagent_core::ask`, and `docs/balloon-ui.md` for where the design comes
//! from) — get their own trio: [`ask_size`], [`paint_ask_into`], and [`ask_hit_test`], which
//! resolves a click back to the control under it. Give all three the same [`AskFonts`] /
//! `scale` / `below`, or the hit map won't match the pixels.
//!
//! These draw real chrome rather than text stand-ins: a bold heading, choices as
//! radio-marked links in [`BalloonPaint::accent`], tickable check boxes, and bordered commit
//! buttons. `AskFonts` carries the bold face for the heading; without one the heading falls
//! back to the body weight. The no-TrueType path still renders `AskLayout::lines`' ASCII
//! stand-ins through [`paint_into`].
//!
//! [`AskState`] adds hover and pressed feedback. It is host state — see its docs for the
//! press-arms / release-commits rule that goes with it.
//!
//! ```no_run
//! use crustagent_balloon::{paint_balloon, BalloonPaint, Font};
//! let font = Font::system("Arial", 30.0, false, false);
//! let img = paint_balloon(
//!     &["Hello there!".to_string()],
//!     0, 1, false,
//!     &BalloonPaint { bg: [255, 255, 225], ..BalloonPaint::default() },
//!     font.as_ref(),
//!     2.0,
//! );
//! // img.rgba is img.width * img.height * 4 bytes, top-down, [r,g,b,a].
//! ```

use crustagent_core::ask::{AskHit, AskLayout, AskRole, InputView, RowMarker};
use font8x8::legacy::BASIC_LEGACY;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::Format;
use swash::FontRef;

/// Bitmap-font scale for the no-TrueType fallback path.
const BSCALE: i32 = 2;
const PAD: i32 = 6;
const TAIL_LEN: i32 = 9;
/// Thought-balloon bubble radii (at scale 1.0), largest nearest the body.
const THINK_BUBBLES: [f32; 3] = [4.5, 3.0, 2.0];

/// A real, anti-aliased text font: a system TrueType face rasterized at a pixel size, with
/// an optional colour-emoji face for codepoints the text face lacks (rendered via swash, so
/// COLR / CBDT / sbix all work). The `fontdb` database is kept alive so the (possibly large,
/// e.g. Apple Color Emoji) emoji face can be memory-mapped on demand rather than copied.
pub struct Font {
    face: fontdue::Font,
    px: f32,
    ascent: f32,
    line_h: f32,
    avg_advance: i32,
    db: fontdb::Database,
    emoji: Option<fontdb::ID>,
}

/// A rasterized colour-emoji glyph: straight-alpha RGBA plus its placement (offsets from the
/// pen position / baseline) and pen advance, all in pixels.
struct EmojiGlyph {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    advance: f32,
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
        let mut font = Font::from_bytes(&data, index, px)?;
        // Colour-emoji fallback: the first installed system emoji family (names differ per
        // platform — Apple Color Emoji / Segoe UI Emoji / Noto Color Emoji). swash reads
        // whichever colour-glyph format the face uses (sbix / COLR / CBDT).
        font.emoji = [
            "Apple Color Emoji",
            "Segoe UI Emoji",
            "Noto Color Emoji",
            "Twemoji Mozilla",
        ]
        .iter()
        .find_map(|n| {
            db.query(&fontdb::Query {
                families: &[fontdb::Family::Name(n)],
                ..Default::default()
            })
        });
        font.db = db;
        Some(font)
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
            db: fontdb::Database::new(),
            emoji: None,
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

    /// Pixel advance width of `s`, counting emoji at their colour-face advance.
    pub fn measure(&self, s: &str) -> i32 {
        s.chars().map(|c| self.advance_of(c)).sum::<f32>().ceil() as i32
    }

    /// Whether the text face has a real glyph for `c` (index 0 = missing → try emoji).
    fn has_text_glyph(&self, c: char) -> bool {
        self.face.lookup_glyph_index(c) != 0
    }

    /// Advance of one char: the text face's, or — for codepoints it lacks — the emoji
    /// face's, falling back to the text face's notdef advance if neither has the glyph.
    fn advance_of(&self, c: char) -> f32 {
        if self.has_text_glyph(c) {
            return self.face.metrics(c, self.px).advance_width;
        }
        if let Some(id) = self.emoji {
            let adv = self
                .db
                .with_face_data(id, |data, index| {
                    let font = FontRef::from_index(data, index as usize)?;
                    let gid = font.charmap().map(c);
                    (gid != 0).then(|| font.glyph_metrics(&[]).scale(self.px).advance_width(gid))
                })
                .flatten();
            if let Some(adv) = adv {
                return adv;
            }
        }
        self.face.metrics(c, self.px).advance_width
    }

    /// Rasterize a colour-emoji glyph for `c`, or `None` if there's no emoji face / glyph.
    /// The emoji face is memory-mapped on demand (never copied — Apple Color Emoji is huge).
    fn render_emoji(&self, c: char) -> Option<EmojiGlyph> {
        let id = self.emoji?;
        self.db
            .with_face_data(id, |data, index| {
                let font = FontRef::from_index(data, index as usize)?;
                let gid = font.charmap().map(c);
                if gid == 0 {
                    return None;
                }
                let mut cx = ScaleContext::new();
                let mut scaler = cx.builder(font).size(self.px).hint(false).build();
                let img = Render::new(&[
                    Source::ColorBitmap(StrikeWith::BestFit),
                    Source::ColorOutline(0),
                ])
                .format(Format::Alpha)
                .render(&mut scaler, gid)?;
                if !matches!(img.content, Content::Color) {
                    return None;
                }
                let advance = font.glyph_metrics(&[]).scale(self.px).advance_width(gid);
                Some(EmojiGlyph {
                    left: img.placement.left,
                    top: img.placement.top,
                    width: img.placement.width as usize,
                    height: img.placement.height as usize,
                    rgba: img.data,
                    advance,
                })
            })
            .flatten()
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
///
/// `accent` and `face` only matter for interactive balloons ([`paint_ask_into`]); build one
/// with `..BalloonPaint::default()` to take their defaults.
pub struct BalloonPaint {
    pub bg: [u8; 3],
    pub border: [u8; 3],
    pub text: [u8; 3],
    /// Choice markers and choice text — the Assistant's link blue.
    pub accent: [u8; 3],
    /// Commit-button fill.
    pub face: [u8; 3],
    /// A thought balloon (bubble-trail tail) vs. a speech balloon (pointed tail).
    pub think: bool,
}

impl Default for BalloonPaint {
    fn default() -> BalloonPaint {
        BalloonPaint {
            bg: [0xFF, 0xFF, 0xE1],
            border: [0x40, 0x40, 0x40],
            text: [0x10, 0x10, 0x10],
            accent: [0x1A, 0x5F, 0xB4],
            face: [0xF4, 0xF2, 0xE4],
            think: false,
        }
    }
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
    let (w, h) = balloon_size(
        font,
        lines,
        min_cols,
        rows.max(lines.len()),
        scale,
        paint.think,
    );
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    paint_into(&mut rgba, w, h, lines, below, paint, font, scale);
    BalloonImage {
        rgba,
        width: w,
        height: h,
    }
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
    Canvas {
        buf,
        w: w as i32,
        h: h as i32,
        clip: None,
    }
    .balloon(lines, below, paint, font, scale);
}

// -- interactive balloons ----------------------------------------------------------------

/// Button chrome, at scale 1.0.
const BTN_HPAD: f32 = 10.0;
const BTN_VPAD: f32 = 4.0;
const BTN_GAP_X: f32 = 8.0;
/// Space above the commit-button row, and above the first choice / check box — the grouping
/// that makes a question read as heading, prose, list, actions.
const GROUP_GAP: f32 = 6.0;
/// Space below the heading, setting it off from the question that follows.
const HEADING_GAP: f32 = 7.0;
/// Breathing room between a row's marker column and its label.
const MARKER_GAP: f32 = 5.0;
/// Text-field chrome, at scale 1.0.
const INPUT_HPAD: f32 = 6.0;
const INPUT_VPAD: f32 = 4.0;
/// Narrowest a text field is allowed to get, in `x`-widths — it must look typeable even
/// beside a one-word question.
const INPUT_MIN_CHARS: i32 = 18;

/// The faces an interactive balloon draws with: the body face, and an optional **bold** one
/// for the heading (Office drew the balloon's `Heading` in bold). With no bold face the
/// heading falls back to the body face.
pub struct AskFonts<'a> {
    pub text: Option<&'a Font>,
    pub bold: Option<&'a Font>,
}

impl<'a> AskFonts<'a> {
    /// Body face only — the heading renders at the same weight.
    pub fn new(text: Option<&'a Font>) -> AskFonts<'a> {
        AskFonts { text, bold: None }
    }
    /// Add the bold face used for the heading.
    pub fn with_bold(mut self, bold: Option<&'a Font>) -> AskFonts<'a> {
        self.bold = bold;
        self
    }
    /// The face a row of this role draws with.
    fn for_role(&self, role: AskRole) -> Option<&Font> {
        match role {
            AskRole::Heading => self.bold.or(self.text),
            _ => self.text,
        }
    }
    fn line_h(&self) -> i32 {
        self.text.map(|f| f.line_height()).unwrap_or(8 * BSCALE)
    }
}

/// Pixel advance of `s`, matching whichever text path will draw it.
fn measure_text(font: Option<&Font>, s: &str) -> i32 {
    match font {
        Some(f) => f.measure(s),
        None => s.chars().count() as i32 * 8 * BSCALE,
    }
}

/// One row's box, relative to the content origin.
struct RowMetric {
    dy: i32,
    h: i32,
    /// Where the label starts — past the marker column for a marked row, 0 otherwise.
    text_dx: i32,
}

/// Everything the paint and the hit-test must agree on: per-row boxes, the commit buttons'
/// boxes, the marker column width, and the content size. Computed once, used by both, so a
/// click can never land somewhere the pixels don't.
struct AskMetrics {
    rows: Vec<RowMetric>,
    /// `(dx, width)` per commit button, left to right.
    buttons: Vec<(i32, i32)>,
    marker_w: i32,
    w: i32,
    h: i32,
}

fn ask_metrics(layout: &AskLayout, fonts: &AskFonts, scale: f32) -> AskMetrics {
    let line_h = fonts.line_h();
    let px = |v: f32| (v * scale).round().max(1.0) as i32;
    let (btn_hpad, btn_vpad, btn_gap, group_gap) =
        (px(BTN_HPAD), px(BTN_VPAD), px(BTN_GAP_X), px(GROUP_GAP));

    // The marker column is square-ish, but must also fit the widest list number.
    let mut marker_w = 0;
    for row in layout.rows.iter().filter(|r| r.indent > 0) {
        let need = match row.marker {
            RowMarker::Number(n) => measure_text(fonts.text, &format!("{n}.")),
            _ => (line_h as f32 * 0.8) as i32,
        };
        marker_w = marker_w.max(need);
    }
    // Labels clear the column by a fixed gap, so every row's text starts on one edge.
    let text_indent = if marker_w > 0 {
        marker_w + px(MARKER_GAP)
    } else {
        0
    };

    let mut rows = Vec::with_capacity(layout.rows.len());
    let mut buttons = Vec::new();
    let (mut y, mut w) = (0, 0);
    let (mut seen_choice, mut seen_check) = (false, false);
    let (mut in_heading, mut heading_closed) = (false, false);

    for row in &layout.rows {
        let mut gap = match row.role {
            AskRole::Choice(_) if !seen_choice => {
                seen_choice = true;
                group_gap
            }
            AskRole::CheckBox(_) if !seen_check => {
                seen_check = true;
                group_gap
            }
            AskRole::Input | AskRole::Buttons => group_gap,
            _ => 0,
        };
        // The heading gets its own breathing room below it — max'd with any group gap, so a
        // heading followed straight by the choices doesn't get both.
        match row.role {
            AskRole::Heading => in_heading = true,
            _ if in_heading && !heading_closed => {
                heading_closed = true;
                gap = gap.max(px(HEADING_GAP));
            }
            _ => {}
        }
        y += gap;

        if row.role == AskRole::Input {
            let hpad = px(INPUT_HPAD);
            let char_w = fonts.text.map(|f| f.avg_advance()).unwrap_or(8 * BSCALE);
            // Sized from the *placeholder*, never the typed value: a field that grew with
            // its contents would resize the balloon out from under the typist.
            let prompt = layout
                .input
                .as_ref()
                .map(|v| v.prompt.as_str())
                .unwrap_or("");
            let need = measure_text(fonts.text, prompt).max(INPUT_MIN_CHARS * char_w) + 2 * hpad;
            w = w.max(need);
            let h = line_h + 2 * px(INPUT_VPAD);
            rows.push(RowMetric {
                dy: y,
                h,
                text_dx: hpad,
            });
            y += h;
            continue;
        }

        if row.role == AskRole::Buttons {
            let h = line_h + 2 * btn_vpad;
            let mut dx = 0;
            for button in &layout.buttons {
                let bw = measure_text(fonts.text, button.label()) + 2 * btn_hpad;
                buttons.push((dx, bw));
                dx += bw + btn_gap;
            }
            w = w.max((dx - btn_gap).max(0));
            rows.push(RowMetric {
                dy: y,
                h,
                text_dx: 0,
            });
            y += h;
        } else {
            let text_dx = if row.indent > 0 { text_indent } else { 0 };
            w = w.max(text_dx + measure_text(fonts.for_role(row.role), &row.text));
            rows.push(RowMetric {
                dy: y,
                h: line_h,
                text_dx,
            });
            y += line_h;
        }
    }

    AskMetrics {
        rows,
        buttons,
        marker_w,
        w: w.max(1),
        h: y.max(1),
    }
}

/// The content origin inside a painted buffer: left edge, and top edge past the tail strip
/// when the balloon sits below the character.
fn ask_origin(scale: f32, below: bool) -> (i32, i32) {
    let pad = pad_px(scale);
    (pad, if below { tail_px(scale, false) } else { 0 } + pad)
}

/// How a control is currently being touched by the pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    /// The pointer is over it.
    Hover,
    /// The pointer went down on it and has not been released.
    Pressed,
}

/// Which control the pointer is over and which is being held down.
///
/// This is pure host-side interaction state — the agent has no business knowing about it, so
/// the host tracks it (from [`ask_hit_test`] on pointer moves and presses) and hands it to
/// [`paint_ask_into`]. Note that a control is only *committed* on release, and only if the
/// release lands on the same control the press did: that's what makes a press cancellable by
/// dragging off, the way every other button behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AskState {
    pub hover: Option<AskHit>,
    pub pressed: Option<AskHit>,
    /// Whether the text field has the keyboard. Focusing it drops the placeholder for a
    /// caret; the host sets this when the field is clicked (or typed into).
    pub focused: bool,
    /// Whether the text field's caret is currently drawn. The host owns the blink — it knows
    /// its own frame clock — and toggles this; leave it `true` for a steady caret.
    pub caret_on: bool,
}

impl Default for AskState {
    fn default() -> AskState {
        AskState {
            hover: None,
            pressed: None,
            focused: false,
            caret_on: true,
        }
    }
}

impl AskState {
    /// How `hit` should be drawn. A held control stays [`Phase::Pressed`] only while the
    /// pointer is still on it; drag off and it falls back to idle, ready to be cancelled.
    pub fn phase(&self, hit: AskHit) -> Phase {
        if self.pressed == Some(hit) {
            return if self.hover == Some(hit) {
                Phase::Pressed
            } else {
                Phase::Idle
            };
        }
        // Another control is held: nothing else lights up until it is released.
        if self.pressed.is_some() {
            return Phase::Idle;
        }
        if self.hover == Some(hit) {
            Phase::Hover
        } else {
            Phase::Idle
        }
    }
}

/// Blend `a` toward `b` by `t` (0..=1).
fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [lerp(a[0], b[0]), lerp(a[1], b[1]), lerp(a[2], b[2])]
}

/// One clickable region of an interactive balloon: what a click there means, and where it is
/// in the painted buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AskRect {
    /// What to report to the agent when this region is clicked.
    pub hit: AskHit,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl AskRect {
    /// Whether `(px, py)` — in the painted buffer's pixel space — is inside this region.
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
}

/// The clickable regions of an interactive balloon painted at width `w` with `scale`/`fonts`
/// (the same arguments given to [`paint_ask_into`], or the region map won't match the
/// pixels). A choice that wrapped over several rows yields one region spanning them all;
/// the commit-button row yields one region per button, each hugging its drawn box.
pub fn ask_rects(
    layout: &AskLayout,
    fonts: &AskFonts,
    w: u32,
    below: bool,
    scale: f32,
) -> Vec<AskRect> {
    let m = ask_metrics(layout, fonts, scale);
    let (x0, y0) = ask_origin(scale, below);
    let row_w = (w as i32 - 2 * x0).max(1);
    let mut out = Vec::new();
    let mut i = 0;
    while i < layout.rows.len() {
        let role = layout.rows[i].role;
        // Consecutive rows sharing a role are one control (a wrapped choice, say).
        let mut j = i + 1;
        while j < layout.rows.len() && layout.rows[j].role == role {
            j += 1;
        }
        let y = y0 + m.rows[i].dy;
        let h = m.rows[j - 1].dy + m.rows[j - 1].h - m.rows[i].dy;
        match role {
            AskRole::Choice(n) => out.push(AskRect {
                hit: AskHit::Choice(n),
                x: x0,
                y,
                w: row_w,
                h,
            }),
            AskRole::CheckBox(n) => out.push(AskRect {
                hit: AskHit::CheckBox(n),
                x: x0,
                y,
                w: row_w,
                h,
            }),
            AskRole::Input => out.push(AskRect {
                hit: AskHit::Input,
                x: x0,
                y,
                w: row_w,
                h,
            }),
            AskRole::Buttons => {
                for (button, &(dx, bw)) in layout.buttons.iter().zip(&m.buttons) {
                    out.push(AskRect {
                        hit: AskHit::Button(*button),
                        x: x0 + dx,
                        y,
                        w: bw,
                        h,
                    });
                }
            }
            AskRole::Heading | AskRole::Text => {}
        }
        i = j;
    }
    out
}

/// How far the field's text is scrolled left so the caret stays visible, in pixels. A field
/// scrolls rather than wrapping, so this is what keeps a long value usable.
fn input_scroll(view: &InputView, fonts: &AskFonts, inner_w: i32) -> i32 {
    let full = measure_text(fonts.text, &view.value);
    if full <= inner_w {
        return 0;
    }
    let upto: String = view.value.chars().take(view.caret).collect();
    let caret_x = measure_text(fonts.text, &upto);
    // Keep the caret inside the box, and never scroll past the end of the text.
    caret_x.saturating_sub(inner_w).max(0).min(full - inner_w)
}

/// Where the field's text and caret sit inside a field box of width `w` at `x`: the text
/// origin (already scrolled) and the inner width available to it.
fn input_text_origin(view: &InputView, fonts: &AskFonts, x: i32, w: i32, scale: f32) -> (i32, i32) {
    let hpad = (INPUT_HPAD * scale).round().max(1.0) as i32;
    let inner_w = (w - 2 * hpad).max(1);
    (x + hpad - input_scroll(view, fonts, inner_w), inner_w)
}

/// The caret position — as a **char** offset — for a click at `px` inside the text field, or
/// `None` when the balloon has no field or `px` is outside it. Feed it to
/// `Agent::report_ask_caret`.
pub fn ask_caret_at(
    layout: &AskLayout,
    fonts: &AskFonts,
    w: u32,
    below: bool,
    scale: f32,
    px: i32,
) -> Option<usize> {
    let view = layout.input.as_ref()?;
    let field = ask_rects(layout, fonts, w, below, scale)
        .into_iter()
        .find(|r| r.hit == AskHit::Input)?;
    if view.value.is_empty() {
        return Some(0);
    }
    let (text_x, _) = input_text_origin(view, fonts, field.x, field.w, scale);
    // Walk the chars, taking the boundary nearest the click.
    let mut best = (0usize, (px - text_x).abs());
    let mut run = String::new();
    for (n, c) in view.value.chars().enumerate() {
        run.push(c);
        let dist = (px - (text_x + measure_text(fonts.text, &run))).abs();
        if dist < best.1 {
            best = (n + 1, dist);
        }
    }
    Some(best.0)
}

/// Which control — if any — a click at `(px, py)` in the painted buffer landed on. Feed the
/// result to `Agent::report_ask_hit`.
#[allow(clippy::too_many_arguments)]
pub fn ask_hit_test(
    layout: &AskLayout,
    fonts: &AskFonts,
    w: u32,
    below: bool,
    scale: f32,
    px: i32,
    py: i32,
) -> Option<AskHit> {
    ask_rects(layout, fonts, w, below, scale)
        .into_iter()
        .find(|r| r.contains(px, py))
        .map(|r| r.hit)
}

/// The pixel size needed to hold an interactive balloon (a question never uses the thought
/// tail, so this always reserves the speech one).
pub fn ask_size(fonts: &AskFonts, layout: &AskLayout, scale: f32) -> (u32, u32) {
    let m = ask_metrics(layout, fonts, scale);
    let pad = pad_px(scale);
    let w = m.w + pad * 2 + 2;
    let h = m.h + pad * 2 + tail_px(scale, false) + 2;
    (w.max(16) as u32, h.max(16) as u32)
}

/// Paint an interactive balloon into a caller-provided buffer of size `w`×`h` (size it with
/// [`ask_size`]). Pass the same `fonts`, `scale` and `below` to [`ask_hit_test`] so clicks
/// resolve against the pixels actually drawn. `state` carries hover / pressed feedback —
/// pass `&AskState::default()` for none.
#[allow(clippy::too_many_arguments)]
pub fn paint_ask_into(
    buf: &mut [u8],
    w: u32,
    h: u32,
    layout: &AskLayout,
    below: bool,
    paint: &BalloonPaint,
    fonts: &AskFonts,
    state: &AskState,
    scale: f32,
) {
    Canvas {
        buf,
        w: w as i32,
        h: h as i32,
        clip: None,
    }
    .ask(layout, below, paint, fonts, state, scale);
}

/// A borrowed RGBA8 drawing target (top-down, non-premultiplied).
struct Canvas<'a> {
    buf: &'a mut [u8],
    w: i32,
    h: i32,
    /// When set, drawing is confined to this `(x, y, w, h)` rect — the text field uses it so
    /// a value scrolled past the box edge is cut off rather than spilling into the balloon.
    clip: Option<(i32, i32, i32, i32)>,
}

impl Canvas<'_> {
    /// Whether `(x, y)` is on-canvas and inside the clip rect, if any.
    #[inline]
    fn writable(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return false;
        }
        match self.clip {
            Some((cx, cy, cw, ch)) => x >= cx && y >= cy && x < cx + cw && y < cy + ch,
            None => true,
        }
    }

    /// Run `draw` with drawing confined to `rect`, restoring the previous clip after.
    fn clipped(&mut self, rect: (i32, i32, i32, i32), draw: impl FnOnce(&mut Self)) {
        let prev = self.clip.replace(rect);
        draw(self);
        self.clip = prev;
    }
}

/// Coverage (0..=1) of the pixel centered at (`px`, `py`) inside the rounded rectangle
/// (`x`, `y`, `w`, `h`) with corner radius `r` — a signed-distance field sampled with a
/// 1px antialiased edge (coverage 0.5 exactly on the boundary).
fn round_rect_cov(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let (hw, hh) = (w / 2.0, h / 2.0);
    let qx = (px - (x + hw)).abs() - (hw - r);
    let qy = (py - (y + hh)).abs() - (hh - r);
    let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
    (0.5 - d).clamp(0.0, 1.0)
}

impl Canvas<'_> {
    #[inline]
    fn put(&mut self, x: i32, y: i32, rgb: [u8; 3]) {
        if !self.writable(x, y) {
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
        if a == 0 || !self.writable(x, y) {
            return;
        }
        let o = ((y * self.w + x) * 4) as usize;
        let inv = 255 - a as u16;
        for (k, &c) in rgb.iter().enumerate() {
            self.buf[o + k] = ((c as u16 * a as u16 + self.buf[o + k] as u16 * inv) / 255) as u8;
        }
        self.buf[o + 3] = self.buf[o + 3].max(a);
    }

    /// Composite `rgb` at `(x, y)` with straight-alpha coverage `cov` (0..=1) using an
    /// "over" blend. Shape edges meet *transparent* pixels, so — unlike [`Self::blend`],
    /// which assumes an opaque backdrop — they need real straight-alpha compositing or the
    /// antialiased edge picks up a dark fringe.
    fn cover(&mut self, x: i32, y: i32, rgb: [u8; 3], cov: f32) {
        if !self.writable(x, y) {
            return;
        }
        let sa = (cov.clamp(0.0, 1.0) * 255.0).round() as u32;
        if sa == 0 {
            return;
        }
        let o = ((y * self.w + x) * 4) as usize;
        let da = self.buf[o + 3] as u32;
        let out_a = sa + da * (255 - sa) / 255;
        if out_a == 0 {
            return;
        }
        for (k, &channel) in rgb.iter().enumerate() {
            let s = channel as u32;
            let d = self.buf[o + k] as u32;
            self.buf[o + k] = ((s * sa + d * da * (255 - sa) / 255) / out_a).min(255) as u8;
        }
        self.buf[o + 3] = out_a as u8;
    }

    /// Composite a straight-alpha RGBA image (a colour-emoji glyph) with its top-left at
    /// (`x`, `y`), row-major `w`×`h`.
    fn blit_rgba(&mut self, x: i32, y: i32, w: usize, h: usize, rgba: &[u8]) {
        for row in 0..h {
            for col in 0..w {
                let i = (row * w + col) * 4;
                let a = rgba[i + 3];
                if a == 0 {
                    continue;
                }
                // `cover` does straight-alpha "over" — reuse it per pixel with the glyph's
                // own colour and coverage.
                let rgb = [rgba[i], rgba[i + 1], rgba[i + 2]];
                self.cover(x + col as i32, y + row as i32, rgb, a as f32 / 255.0);
            }
        }
    }

    /// Fill the horizontal span [`left`, `right`) on row `y` with `rgb`, antialiasing the
    /// fractional ends.
    fn hspan(&mut self, y: i32, left: f32, right: f32, rgb: [u8; 3]) {
        if right <= left {
            return;
        }
        for x in left.floor() as i32..right.ceil() as i32 {
            let cov = ((x as f32 + 1.0).min(right) - (x as f32).max(left)).clamp(0.0, 1.0);
            self.cover(x, y, rgb, cov);
        }
    }

    /// Draw `s` with a real font, its top edge at `top`, left edge at `x`.
    fn text_font(&mut self, font: &Font, x: i32, top: i32, s: &str, rgb: [u8; 3]) {
        let baseline = top + font.ascent.round() as i32;
        let mut pen = x as f32;
        for c in s.chars() {
            // Colour emoji (and any codepoint the text face lacks) come from the emoji face,
            // rasterized as RGBA and composited straight.
            if !font.has_text_glyph(c) {
                if let Some(g) = font.render_emoji(c) {
                    self.blit_rgba(
                        pen.round() as i32 + g.left,
                        baseline - g.top,
                        g.width,
                        g.height,
                        &g.rgba,
                    );
                    pen += g.advance;
                    continue;
                }
            }
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

    /// Fill a rectangle with antialiased rounded corners of radius `r`.
    fn fill_round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, rgb: [u8; 3]) {
        if w <= 0 || h <= 0 {
            return;
        }
        let r = r.clamp(0, w.min(h) / 2) as f32;
        // Scan the box grown by 1px so the outer half of the AA edge is covered too.
        for yy in y - 1..y + h + 1 {
            for xx in x - 1..x + w + 1 {
                let cov = round_rect_cov(
                    xx as f32 + 0.5,
                    yy as f32 + 0.5,
                    x as f32,
                    y as f32,
                    w as f32,
                    h as f32,
                    r,
                );
                if cov > 0.0 {
                    self.cover(xx, yy, rgb, cov);
                }
            }
        }
    }

    /// An antialiased filled disc of radius `r` at `(cx, cy)` with a `bord`px border ring.
    fn disc(&mut self, cx: i32, cy: i32, r: i32, bord: i32, fill: [u8; 3], border: [u8; 3]) {
        let rf = r as f32;
        for dy in -r - 1..=r + 1 {
            for dx in -r - 1..=r + 1 {
                let dist = ((dx * dx + dy * dy) as f32).sqrt();
                let outer = (0.5 - (dist - rf)).clamp(0.0, 1.0);
                if outer <= 0.0 {
                    continue;
                }
                self.cover(cx + dx, cy + dy, border, outer);
                let inner = (0.5 - (dist - (rf - bord as f32).max(0.0))).clamp(0.0, 1.0);
                if inner > 0.0 {
                    self.cover(cx + dx, cy + dy, fill, inner);
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
        self.body(below, style, scale);
        let pad = pad_px(scale);
        let y0 = if below {
            tail_px(scale, style.think)
        } else {
            0
        } + pad;
        let line_h = font.map(|f| f.line_height()).unwrap_or(8 * BSCALE);
        for (i, line) in lines.iter().enumerate() {
            let ty = y0 + i as i32 * line_h;
            match font {
                Some(f) => self.text_font(f, pad, ty, line, style.text),
                None => self.text_bitmap(pad, ty, BSCALE, line, style.text),
            }
        }
    }

    /// Draw an interactive balloon: the body, then each row — a bold heading, prose, choices
    /// drawn as marked links, tickable check boxes, and real commit buttons.
    #[allow(clippy::too_many_arguments)]
    fn ask(
        &mut self,
        layout: &AskLayout,
        below: bool,
        style: &BalloonPaint,
        fonts: &AskFonts,
        state: &AskState,
        scale: f32,
    ) {
        self.body(below, style, scale);
        let m = ask_metrics(layout, fonts, scale);
        let (x0, y0) = ask_origin(scale, below);
        let line_h = fonts.line_h();
        let rects = ask_rects(layout, fonts, self.w as u32, below, scale);

        // Row highlights go down first, under everything else. Buttons are skipped: they
        // carry their state in their own face rather than a band behind them.
        for rect in &rects {
            if matches!(rect.hit, AskHit::Button(_)) {
                continue;
            }
            let tint = match state.phase(rect.hit) {
                Phase::Pressed => 0.22,
                Phase::Hover => 0.11,
                Phase::Idle => continue,
            };
            let r = (scale * 3.0).round() as i32;
            self.fill_round_rect(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                r,
                mix(style.bg, style.accent, tint),
            );
        }

        for (i, row) in layout.rows.iter().enumerate() {
            let metric = &m.rows[i];
            let y = y0 + metric.dy;

            if row.role == AskRole::Input {
                if let Some(view) = &layout.input {
                    let row_w = (self.w - 2 * x0).max(1);
                    self.input_field(x0, y, row_w, metric.h, view, style, fonts, state, scale);
                }
                continue;
            }

            if row.role == AskRole::Buttons {
                for (button, &(dx, bw)) in layout.buttons.iter().zip(&m.buttons) {
                    let phase = state.phase(AskHit::Button(*button));
                    self.button(
                        x0 + dx,
                        y,
                        bw,
                        metric.h,
                        button.label(),
                        style,
                        fonts,
                        phase,
                        scale,
                    );
                }
                continue;
            }

            // The marker sits in its own column, centred on the row's text line.
            self.marker(x0, y, m.marker_w, line_h, row.marker, style, fonts, scale);

            let color = match row.role {
                // Choices read as links, the way the Assistant's did.
                AskRole::Choice(_) => style.accent,
                _ => style.text,
            };
            let tx = x0 + metric.text_dx;
            match fonts.for_role(row.role) {
                Some(f) => self.text_font(f, tx, y, &row.text, color),
                None => self.text_bitmap(tx, y, BSCALE, &row.text, color),
            }

            // Under a pointer, a choice underlines like the link it reads as.
            if let AskRole::Choice(n) = row.role {
                if state.phase(AskHit::Choice(n)) != Phase::Idle && !row.text.is_empty() {
                    let tw = measure_text(fonts.for_role(row.role), &row.text);
                    let uy = y + (line_h as f32 * 0.86) as i32;
                    let half = (scale * 0.5).max(0.5);
                    self.thick_line(
                        tx as f32,
                        uy as f32,
                        (tx + tw) as f32,
                        uy as f32,
                        half,
                        color,
                    );
                }
            }
        }
    }

    /// Draw a row's marker centred in a `w`×`h` column at (`x`, `y`): a radio-style disc for
    /// a clickable choice, a dot for a bulleted one, the number for a numbered one, or a
    /// tickable box for a check box.
    #[allow(clippy::too_many_arguments)]
    fn marker(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        marker: RowMarker,
        paint: &BalloonPaint,
        fonts: &AskFonts,
        scale: f32,
    ) {
        if marker == RowMarker::None || w <= 0 {
            return;
        }
        let (cx, cy) = (x + w / 2, y + h / 2);
        let bord = scale.round().max(1.0) as i32;
        // A darker rim of the accent, so the disc reads as a control rather than a blob.
        let rim = [
            (paint.accent[0] as u32 * 7 / 10) as u8,
            (paint.accent[1] as u32 * 7 / 10) as u8,
            (paint.accent[2] as u32 * 7 / 10) as u8,
        ];

        match marker {
            RowMarker::None => {}
            RowMarker::Choice => {
                let r = ((h as f32) * 0.26).round().max(3.0) as i32;
                self.disc(cx, cy, r, bord, paint.accent, rim);
                // The bright centre is what makes it read as a radio dot.
                let inner = ((r as f32) * 0.32).round().max(1.0) as i32;
                self.disc(cx, cy, inner, 0, [0xFF, 0xFF, 0xFF], [0xFF, 0xFF, 0xFF]);
            }
            RowMarker::Bullet => {
                let r = ((h as f32) * 0.13).round().max(2.0) as i32;
                self.disc(cx, cy, r, 0, paint.text, paint.text);
            }
            RowMarker::Number(n) => {
                let label = format!("{n}.");
                // Right-aligned in the column, so the labels line up whatever the digits.
                let tw = measure_text(fonts.text, &label);
                let tx = x + (w - tw).max(0);
                match fonts.text {
                    Some(f) => self.text_font(f, tx, y, &label, paint.text),
                    None => self.text_bitmap(tx, y, BSCALE, &label, paint.text),
                }
            }
            RowMarker::CheckBox(checked) => {
                let s = ((h as f32) * 0.58).round().max(7.0) as i32;
                let (bx, by) = (cx - s / 2, cy - s / 2);
                let r = (scale * 2.0).round() as i32;
                self.fill_round_rect(bx, by, s, s, r, paint.border);
                self.fill_round_rect(
                    bx + bord,
                    by + bord,
                    s - 2 * bord,
                    s - 2 * bord,
                    (r - bord).max(0),
                    [0xFF, 0xFF, 0xFF],
                );
                if checked {
                    // A two-stroke tick, inset so it sits inside the box's border.
                    let f =
                        |dx: f32, dy: f32| (bx as f32 + s as f32 * dx, by as f32 + s as f32 * dy);
                    let half = (scale * 1.1).max(1.0);
                    let (ax, ay) = f(0.24, 0.52);
                    let (mx, my) = f(0.43, 0.71);
                    let (zx, zy) = f(0.77, 0.29);
                    self.thick_line(ax, ay, mx, my, half, paint.accent);
                    self.thick_line(mx, my, zx, zy, half, paint.accent);
                }
            }
        }
    }

    /// Draw the text field: a white, bordered box holding the value (or a dimmed
    /// placeholder) and the caret. The value is clipped to the box and scrolled to keep the
    /// caret in view, so a long answer neither wraps nor spills.
    #[allow(clippy::too_many_arguments)]
    fn input_field(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        view: &InputView,
        paint: &BalloonPaint,
        fonts: &AskFonts,
        state: &AskState,
        scale: f32,
    ) {
        let bord = scale.round().max(1.0) as i32;
        let r = (scale * 3.0).round().max(2.0) as i32;
        // Focused or hovered, the field wears the accent border; otherwise it sits quiet.
        let border = if state.focused || state.hover == Some(AskHit::Input) {
            paint.accent
        } else {
            mix(paint.border, paint.accent, 0.35)
        };
        self.fill_round_rect(x, y, w, h, r, border);
        self.fill_round_rect(
            x + bord,
            y + bord,
            w - 2 * bord,
            h - 2 * bord,
            (r - bord).max(0),
            [0xFF, 0xFF, 0xFF],
        );

        let line_h = fonts.line_h();
        let ty = y + (h - line_h) / 2;
        let (text_x, inner_w) = input_text_origin(view, fonts, x, w, scale);
        let prompting = view.shows_prompt(state.focused);
        let color = if prompting {
            // A placeholder is a hint, not content.
            mix(paint.text, [0xFF, 0xFF, 0xFF], 0.55)
        } else {
            paint.text
        };
        let text = view.display(state.focused);

        let hpad = (INPUT_HPAD * scale).round().max(1.0) as i32;
        let inner = (x + hpad, y + bord, inner_w, h - 2 * bord);
        // Everything inside the box is clipped: a scrolled value must be cut at the edge
        // rather than spill across the balloon.
        let run_x = |n: usize| {
            let upto: String = view.value.chars().take(n).collect();
            text_x + measure_text(fonts.text, &upto)
        };

        match view.selection.filter(|_| !prompting) {
            // Selected text is drawn in three runs so the middle can be inverted.
            Some((lo, hi)) => {
                let (sel_x, sel_end) = (run_x(lo), run_x(hi));
                let take = |a: usize, b: usize| -> String {
                    view.value.chars().skip(a).take(b - a).collect()
                };
                let (before, selected, after) = (
                    take(0, lo),
                    take(lo, hi),
                    take(hi, view.value.chars().count()),
                );
                self.clipped(inner, |c| {
                    c.fill_rect(sel_x, ty, (sel_end - sel_x).max(1), line_h, paint.accent);
                    let draw = |c: &mut Self, x: i32, s: &str, rgb: [u8; 3]| match fonts.text {
                        Some(f) => c.text_font(f, x, ty, s, rgb),
                        None => c.text_bitmap(x, ty, BSCALE, s, rgb),
                    };
                    draw(c, text_x, &before, color);
                    draw(c, sel_x, &selected, [0xFF, 0xFF, 0xFF]);
                    draw(c, sel_end, &after, color);
                });
            }
            None => {
                self.clipped(inner, |c| match fonts.text {
                    Some(f) => c.text_font(f, text_x, ty, text, color),
                    None => c.text_bitmap(text_x, ty, BSCALE, text, color),
                });
            }
        }

        // The caret belongs to focus, not to content: a focused empty field shows one. It is
        // hidden while a selection is up — the highlight already says where you are.
        if state.caret_on && state.focused && view.selection.is_none() {
            let caret_x = run_x(view.caret);
            let (top, bottom) = (ty as f32, (ty + line_h) as f32);
            self.clipped(inner, |c| {
                c.thick_line(
                    caret_x as f32,
                    top,
                    caret_x as f32,
                    bottom,
                    (scale * 0.6).max(0.5),
                    paint.text,
                )
            });
        }
    }

    /// Draw a commit button: a rounded, bordered face with its label centred. Hovering
    /// lightens the face and accents the border; holding it down darkens the face and nudges
    /// the label a pixel down-right, so the button visibly takes the press.
    #[allow(clippy::too_many_arguments)]
    fn button(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        label: &str,
        paint: &BalloonPaint,
        fonts: &AskFonts,
        phase: Phase,
        scale: f32,
    ) {
        let bord = scale.round().max(1.0) as i32;
        let r = (scale * 4.0).round().max(2.0) as i32;
        let (face, border, nudge) = match phase {
            Phase::Idle => (paint.face, paint.border, 0),
            Phase::Hover => (mix(paint.face, [0xFF, 0xFF, 0xFF], 0.55), paint.accent, 0),
            Phase::Pressed => (mix(paint.face, paint.border, 0.20), paint.accent, bord),
        };
        self.fill_round_rect(x, y, w, h, r, border);
        self.fill_round_rect(
            x + bord,
            y + bord,
            w - 2 * bord,
            h - 2 * bord,
            (r - bord).max(0),
            face,
        );
        let tw = measure_text(fonts.text, label);
        let line_h = fonts.line_h();
        let (tx, ty) = (x + (w - tw) / 2 + nudge, y + (h - line_h) / 2 + nudge);
        match fonts.text {
            Some(f) => self.text_font(f, tx, ty, label, paint.text),
            None => self.text_bitmap(tx, ty, BSCALE, label, paint.text),
        }
    }

    /// An antialiased line of half-width `half` from (`x0`, `y0`) to (`x1`, `y1`), drawn as
    /// the coverage of a distance field so the ends and slopes stay clean at any scale.
    fn thick_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, half: f32, rgb: [u8; 3]) {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len_sq = dx * dx + dy * dy;
        let pad = half.ceil() as i32 + 1;
        let (lo_x, hi_x) = (x0.min(x1) as i32 - pad, x0.max(x1) as i32 + pad);
        let (lo_y, hi_y) = (y0.min(y1) as i32 - pad, y0.max(y1) as i32 + pad);
        for py in lo_y..=hi_y {
            for px in lo_x..=hi_x {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                // Distance to the segment: project onto it, clamped to the endpoints.
                let t = if len_sq > 0.0 {
                    (((fx - x0) * dx + (fy - y0) * dy) / len_sq).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let dist = (fx - (x0 + t * dx)).hypot(fy - (y0 + t * dy));
                let cov = (0.5 - (dist - half)).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.cover(px, py, rgb, cov);
                }
            }
        }
    }

    /// Draw the balloon shape itself — rounded body plus the speech tail or thought-bubble
    /// trail — filling this (already correctly-sized) canvas. Text is the caller's job.
    fn body(&mut self, below: bool, style: &BalloonPaint, scale: f32) {
        let (bg, border) = (style.bg, style.border);
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

        // Rounded body: a border-colored rounded rect with a smaller bg rect inside,
        // leaving a rounded outline. The outline is `bord` px — scaled to ~1 logical px so
        // it survives the compositor's fractional downscale instead of thinning to a faint
        // sub-pixel line.
        let r = (6.0 * scale).round() as i32;
        let bord = scale.round().max(1.0) as i32;
        self.fill_round_rect(bx, by, bw, bh, r, border);
        self.fill_round_rect(
            bx + bord,
            by + bord,
            bw - 2 * bord,
            bh - 2 * bord,
            (r - bord).max(0),
            bg,
        );

        if style.think {
            // A descending trail of shrinking, separated bubbles.
            let gap = (2.0 * scale).round() as i32;
            let tcx = tip_x.clamp(bx + tail_len, bx + bw - tail_len);
            let mut edge = attach_y;
            for &base in &THINK_BUBBLES {
                let rr = (base * scale).round().max(1.0) as i32;
                edge += dir * (gap + rr);
                self.disc(tcx, edge, rr, bord, bg, border);
                edge += dir * rr;
            }
        } else {
            // Pointed tail: a border-colored triangle with a bg triangle inset `bord` px on
            // each slanted side — so the outline matches the body's and it opens into the
            // body (the inset is horizontal only, no cap across the top). Antialiased ends
            // via fractional row widths.
            let tcx = tip_x.clamp(bx + tail_half + 3, bx + bw - tail_half - 3);
            let cxf = tcx as f32 + 0.5;
            let len = tail_len.max(1) as f32;
            // Start `bord` rows inside the body so the tail's bg reaches up through the
            // body's bottom border band and the two interiors merge — otherwise a thick
            // outline leaves a border line across the junction. Rows inside the body draw
            // bg only (the body already painted the outline there).
            for row in -bord..=tail_len {
                let half = tail_half as f32 * (1.0 - row.max(0) as f32 / len);
                let y = attach_y + dir * row;
                if row >= 0 {
                    self.hspan(y, cxf - half, cxf + half, border);
                }
                let inner = half - bord as f32;
                if inner > 0.0 {
                    self.hspan(y, cxf - inner, cxf + inner, bg);
                }
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
            &BalloonPaint::default(),
            None,
            2.0,
        );
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
        // The body is opaque somewhere (not an all-transparent buffer).
        assert!(img.rgba.iter().skip(3).step_by(4).any(|&a| a == 0xFF));
    }

    #[test]
    fn shape_edges_are_antialiased() {
        // No text (empty line, no font), so any partial-alpha pixel must come from the
        // antialiased shape edges — the rounded corners and the tapered tail.
        let img = paint_balloon(
            &[String::new()],
            4,
            1,
            false,
            &BalloonPaint::default(),
            None,
            2.0,
        );
        let partial = img
            .rgba
            .iter()
            .skip(3)
            .step_by(4)
            .filter(|&&a| a > 0 && a < 255)
            .count();
        assert!(
            partial > 0,
            "shape edges should be antialiased (have partial-alpha pixels)"
        );
    }

    /// The demo question: a heading, body text, two choices, a check box and a button row.
    fn ask_layout() -> AskLayout {
        use crustagent_core::ask::{layout_ask, AskAnswer, BalloonUi, ButtonSet};
        layout_ask(
            &BalloonUi::new("Select one of these things:")
                .heading("What would you like to do?")
                .choice("Write a letter")
                .choice("Make a chart")
                .checkbox("Don't ask again")
                .buttons(ButtonSet::OkCancel),
            &AskAnswer::default(),
            32,
        )
    }

    #[test]
    fn every_control_is_hittable_at_its_own_row() {
        let layout = ask_layout();
        let (w, _h) = ask_size(&AskFonts::new(None), &layout, 2.0);
        let rects = ask_rects(&layout, &AskFonts::new(None), w, false, 2.0);
        // Two choices, one check box, two buttons.
        assert_eq!(rects.len(), 5, "{rects:?}");
        for rect in &rects {
            let (cx, cy) = (rect.x + rect.w / 2, rect.y + rect.h / 2);
            assert_eq!(
                ask_hit_test(&layout, &AskFonts::new(None), w, false, 2.0, cx, cy),
                Some(rect.hit),
                "center of {rect:?} should hit itself"
            );
        }
    }

    #[test]
    fn heading_and_body_rows_are_not_clickable() {
        let layout = ask_layout();
        let fonts = AskFonts::new(None);
        let (w, _h) = ask_size(&fonts, &layout, 2.0);
        let m = ask_metrics(&layout, &fonts, 2.0);
        let (x0, y0) = ask_origin(2.0, false);
        // Rows 0 and 1 are the heading and the body text.
        for row in 0..2 {
            let y = y0 + m.rows[row].dy + m.rows[row].h / 2;
            assert_eq!(
                ask_hit_test(&layout, &fonts, w, false, 2.0, x0 + 4, y),
                None
            );
        }
    }

    #[test]
    fn buttons_get_side_by_side_regions_in_order() {
        use crustagent_core::ask::Button;
        let layout = ask_layout();
        let (w, _h) = ask_size(&AskFonts::new(None), &layout, 2.0);
        let buttons: Vec<AskRect> = ask_rects(&layout, &AskFonts::new(None), w, false, 2.0)
            .into_iter()
            .filter(|r| matches!(r.hit, AskHit::Button(_)))
            .collect();
        assert_eq!(buttons[0].hit, AskHit::Button(Button::Ok));
        assert_eq!(buttons[1].hit, AskHit::Button(Button::Cancel));
        assert_eq!(buttons[0].y, buttons[1].y, "same row");
        assert!(
            buttons[1].x >= buttons[0].x + buttons[0].w,
            "Cancel sits right of OK without overlapping: {buttons:?}"
        );

        // Same again through the real-font measuring path, which is what actually ships
        // (skipped where the sandbox has no installed fonts).
        let Some(font) = Font::system("", 24.0, false, false) else {
            return;
        };
        let (fw, _) = ask_size(&AskFonts::new(Some(&font)), &layout, 2.0);
        let real: Vec<AskRect> = ask_rects(&layout, &AskFonts::new(Some(&font)), fw, false, 2.0)
            .into_iter()
            .filter(|r| matches!(r.hit, AskHit::Button(_)))
            .collect();
        assert!(
            real[1].x >= real[0].x + real[0].w,
            "buttons must not overlap with a real font either: {real:?}"
        );
        for rect in &real {
            let (cx, cy) = (rect.x + rect.w / 2, rect.y + rect.h / 2);
            assert_eq!(
                ask_hit_test(&layout, &AskFonts::new(Some(&font)), fw, false, 2.0, cx, cy),
                Some(rect.hit)
            );
        }
    }

    #[test]
    fn a_wrapped_choice_is_one_region_spanning_its_rows() {
        use crustagent_core::ask::{layout_ask, AskAnswer, BalloonUi};
        // Narrow enough that the choice wraps over several rows.
        let layout = layout_ask(
            &BalloonUi::new("").choice("one two three four five six"),
            &AskAnswer::default(),
            10,
        );
        assert!(layout.rows.len() > 1);
        let fonts = AskFonts::new(None);
        let rects = ask_rects(&layout, &fonts, 200, false, 2.0);
        assert_eq!(rects.len(), 1, "one control, not one per row: {rects:?}");
        let m = ask_metrics(&layout, &fonts, 2.0);
        let last = m.rows.last().unwrap();
        assert_eq!(rects[0].h, last.dy + last.h - m.rows[0].dy);
    }

    #[test]
    fn a_below_balloon_shifts_its_regions_past_the_tail() {
        let layout = ask_layout();
        let (w, _h) = ask_size(&AskFonts::new(None), &layout, 2.0);
        let above = ask_rects(&layout, &AskFonts::new(None), w, false, 2.0);
        let below = ask_rects(&layout, &AskFonts::new(None), w, true, 2.0);
        assert!(
            below[0].y > above[0].y,
            "the tail strip is on top when the balloon sits below the character"
        );
    }

    #[test]
    fn marked_rows_reserve_a_marker_column_and_plain_rows_do_not() {
        let layout = ask_layout();
        let m = ask_metrics(&layout, &AskFonts::new(None), 2.0);
        assert!(m.marker_w > 0);
        for (row, metric) in layout.rows.iter().zip(&m.rows) {
            match row.role {
                AskRole::Choice(_) | AskRole::CheckBox(_) => assert!(
                    metric.text_dx > m.marker_w,
                    "{:?} must clear the marker column",
                    row.role
                ),
                // The field indents by its own padding, not the marker column.
                AskRole::Input => assert!(metric.text_dx > 0 && metric.text_dx < m.marker_w),
                AskRole::Heading | AskRole::Text | AskRole::Buttons => {
                    assert_eq!(metric.text_dx, 0, "{:?} is not marked", row.role)
                }
            }
        }
    }

    #[test]
    fn a_bold_heading_is_measured_at_its_own_weight() {
        // The heading is the widest row here, so the bold face has to widen the balloon —
        // proof the heading is measured (and so drawn) with it. Skipped without fonts.
        let (Some(text), Some(bold)) = (
            Font::system("", 24.0, false, false),
            Font::system("", 24.0, true, false),
        ) else {
            return;
        };
        let layout = ask_layout();
        let plain = ask_size(&AskFonts::new(Some(&text)), &layout, 2.0);
        let heavy = ask_size(
            &AskFonts::new(Some(&text)).with_bold(Some(&bold)),
            &layout,
            2.0,
        );
        assert!(
            heavy.0 >= plain.0,
            "bold heading should not shrink the balloon: {heavy:?} vs {plain:?}"
        );
        assert_eq!(heavy.1, plain.1, "weight must not change row heights");
    }

    #[test]
    fn a_held_control_reads_pressed_only_while_the_pointer_stays_on_it() {
        let a = AskHit::Choice(0);
        let b = AskHit::Choice(1);

        let idle = AskState::default();
        assert_eq!(idle.phase(a), Phase::Idle);

        let hovering = AskState {
            hover: Some(a),
            ..AskState::default()
        };
        assert_eq!(hovering.phase(a), Phase::Hover);
        assert_eq!(hovering.phase(b), Phase::Idle);

        let holding = AskState {
            hover: Some(a),
            pressed: Some(a),
            ..AskState::default()
        };
        assert_eq!(holding.phase(a), Phase::Pressed);

        // Dragged off while still held: the control releases visually, so letting go there
        // reads as the cancel it is. Nothing else lights up while a press is in flight.
        let dragged_off = AskState {
            hover: Some(b),
            pressed: Some(a),
            ..AskState::default()
        };
        assert_eq!(dragged_off.phase(a), Phase::Idle);
        assert_eq!(dragged_off.phase(b), Phase::Idle);
    }

    #[test]
    fn interaction_state_does_not_move_anything() {
        // Feedback is paint-only: hover and press must not shift the layout under the
        // pointer, or a control could slide out from under the click committing it.
        let layout = ask_layout();
        let fonts = AskFonts::new(None);
        let (w, h) = ask_size(&fonts, &layout, 2.0);
        let rects = ask_rects(&layout, &fonts, w, false, 2.0);

        let render = |state: &AskState| {
            let mut buf = vec![0u8; (w * h * 4) as usize];
            paint_ask_into(
                &mut buf,
                w,
                h,
                &layout,
                false,
                &BalloonPaint::default(),
                &fonts,
                state,
                2.0,
            );
            buf
        };
        let idle = render(&AskState::default());
        let hovered = render(&AskState {
            hover: Some(rects[0].hit),
            ..AskState::default()
        });
        assert_ne!(idle, hovered, "hover should be visible at all");
        // ...and the region map is identical either way.
        assert_eq!(rects, ask_rects(&layout, &fonts, w, false, 2.0));
    }

    /// A search-style question with a text field, at `text` with the caret at `caret`.
    fn search_layout(text: &str, caret: usize) -> AskLayout {
        selected_layout(text, caret, caret)
    }

    /// The same, with `anchor`..`caret` selected.
    fn selected_layout(text: &str, anchor: usize, caret: usize) -> AskLayout {
        use crustagent_core::ask::{layout_ask, AskAnswer, BalloonUi, ButtonSet};
        layout_ask(
            &BalloonUi::new("What would you like to do?")
                .input("Type your question here")
                .buttons(ButtonSet::SearchClose),
            &if anchor == caret {
                AskAnswer::at(text, caret)
            } else {
                AskAnswer::selecting(text, anchor, caret)
            },
            32,
        )
    }

    #[test]
    fn a_selection_paints_a_highlight_and_hides_the_caret() {
        let fonts = AskFonts::new(None);
        let focused = AskState {
            focused: true,
            ..AskState::default()
        };
        let render = |layout: &AskLayout| {
            let (w, h) = ask_size(&fonts, layout, 2.0);
            let mut buf = vec![0u8; (w * h * 4) as usize];
            paint_ask_into(
                &mut buf,
                w,
                h,
                layout,
                false,
                &BalloonPaint::default(),
                &fonts,
                &focused,
                2.0,
            );
            (buf, w, h)
        };

        let plain = render(&search_layout("hello world", 0));
        let selected = render(&selected_layout("hello world", 0, 5));
        assert_ne!(plain.0, selected.0);

        // The highlight is the accent colour, and only a selection puts it in the field.
        let accent = BalloonPaint::default().accent;
        let count = |buf: &[u8]| {
            buf.chunks_exact(4)
                .filter(|p| p[0] == accent[0] && p[1] == accent[1] && p[2] == accent[2])
                .count()
        };
        assert!(
            count(&selected.0) > count(&plain.0) + 100,
            "the selected run should be filled with the accent"
        );

        // With the caret blinked on but a selection up, no caret is drawn: the two renders
        // of a selection differ only if the caret leaked through.
        let blinked_off = {
            let layout = selected_layout("hello world", 0, 5);
            let (w, h) = ask_size(&fonts, &layout, 2.0);
            let mut buf = vec![0u8; (w * h * 4) as usize];
            paint_ask_into(
                &mut buf,
                w,
                h,
                &layout,
                false,
                &BalloonPaint::default(),
                &fonts,
                &AskState {
                    focused: true,
                    caret_on: false,
                    ..AskState::default()
                },
                2.0,
            );
            buf
        };
        assert_eq!(
            selected.0, blinked_off,
            "a selection hides the caret, so the blink must make no difference"
        );
    }

    #[test]
    fn the_field_is_hittable_and_sized_from_its_placeholder() {
        let fonts = AskFonts::new(None);
        let empty = search_layout("", 0);
        let (w, _) = ask_size(&fonts, &empty, 2.0);
        let field = ask_rects(&empty, &fonts, w, false, 2.0)
            .into_iter()
            .find(|r| r.hit == AskHit::Input)
            .expect("the field is clickable");
        assert!(field.w > 0 && field.h > 0);

        // Typing a value far longer than the box must not widen the balloon: the field
        // scrolls instead, so it stays put under the typist.
        let long = search_layout("a question far too long to fit inside the field at once", 0);
        assert_eq!(
            ask_size(&fonts, &long, 2.0),
            (w, ask_size(&fonts, &empty, 2.0).1)
        );
    }

    #[test]
    fn clicking_the_field_maps_x_to_a_caret_offset() {
        let fonts = AskFonts::new(None);
        let layout = search_layout("hello", 5);
        let (w, _) = ask_size(&fonts, &layout, 2.0);
        let field = ask_rects(&layout, &fonts, w, false, 2.0)
            .into_iter()
            .find(|r| r.hit == AskHit::Input)
            .unwrap();

        let at = |x| ask_caret_at(&layout, &fonts, w, false, 2.0, x).unwrap();
        // Left of the text lands before the first char; far right lands after the last.
        assert_eq!(at(field.x), 0);
        assert_eq!(at(field.x + field.w), 5);
        // ...and the offsets in between are monotonic.
        let mut last = 0;
        for step in 0..12 {
            let here = at(field.x + step * field.w / 12);
            assert!(here >= last, "caret offsets must not go backwards");
            last = here;
        }
    }

    #[test]
    fn focusing_the_field_trades_the_placeholder_for_a_caret() {
        let fonts = AskFonts::new(None);
        let layout = search_layout("", 0);
        let (w, h) = ask_size(&fonts, &layout, 2.0);
        let render = |state: &AskState| {
            let mut buf = vec![0u8; (w * h * 4) as usize];
            paint_ask_into(
                &mut buf,
                w,
                h,
                &layout,
                false,
                &BalloonPaint::default(),
                &fonts,
                state,
                2.0,
            );
            buf
        };
        let unfocused = render(&AskState::default());
        let focused = render(&AskState {
            focused: true,
            ..AskState::default()
        });
        assert_ne!(
            unfocused, focused,
            "an empty field must look different once focused"
        );

        // With the caret blinked off, a focused empty field is genuinely blank — proof the
        // difference above is the placeholder leaving, not just the caret arriving.
        let blank = render(&AskState {
            focused: true,
            caret_on: false,
            ..AskState::default()
        });
        let ink = |buf: &[u8]| {
            let field = ask_rects(&layout, &fonts, w, false, 2.0)
                .into_iter()
                .find(|r| r.hit == AskHit::Input)
                .unwrap();
            let mut n = 0;
            for y in field.y..field.y + field.h {
                for x in field.x..field.x + field.w {
                    let o = ((y as u32 * w + x as u32) * 4) as usize;
                    if buf[o..o + 3] != [0xFF, 0xFF, 0xFF] {
                        n += 1;
                    }
                }
            }
            n
        };
        assert!(
            ink(&blank) < ink(&unfocused),
            "the placeholder should be gone once focused"
        );
    }

    #[test]
    fn an_empty_field_puts_the_caret_at_the_start_wherever_you_click() {
        // There is nothing to place a caret *within* — the placeholder is not content.
        let fonts = AskFonts::new(None);
        let layout = search_layout("", 0);
        let (w, _) = ask_size(&fonts, &layout, 2.0);
        assert_eq!(ask_caret_at(&layout, &fonts, w, false, 2.0, 9999), Some(0));
    }

    #[test]
    fn a_balloon_without_a_field_has_no_caret_to_place() {
        let fonts = AskFonts::new(None);
        let layout = ask_layout();
        let (w, _) = ask_size(&fonts, &layout, 2.0);
        assert_eq!(ask_caret_at(&layout, &fonts, w, false, 2.0, 10), None);
    }

    #[test]
    fn a_long_value_scrolls_to_keep_the_caret_in_view() {
        let fonts = AskFonts::new(None);
        let text = "a question far too long to fit inside the field at once";
        let inner = 100;

        let view = |caret| search_layout(text, caret).input.take().unwrap();
        // With the caret at the start there is nothing to scroll past...
        assert_eq!(input_scroll(&view(0), &fonts, inner), 0);
        // ...and at the end the text is pushed left so the caret stays inside.
        let end = input_scroll(&view(text.chars().count()), &fonts, inner);
        assert!(end > 0, "should scroll: {end}");
        assert!(
            end <= measure_text(fonts.text, text) - inner,
            "never scrolls past the end of the text"
        );
    }

    #[test]
    fn the_field_clips_a_value_wider_than_its_box() {
        // The value is drawn scrolled; without clipping it would spill across the balloon.
        let fonts = AskFonts::new(None);
        let layout = search_layout(
            "a question far too long to fit inside the field at once",
            54,
        );
        let (w, h) = ask_size(&fonts, &layout, 2.0);
        let field = ask_rects(&layout, &fonts, w, false, 2.0)
            .into_iter()
            .find(|r| r.hit == AskHit::Input)
            .unwrap();

        let mut buf = vec![0u8; (w * h * 4) as usize];
        paint_ask_into(
            &mut buf,
            w,
            h,
            &layout,
            false,
            &BalloonPaint::default(),
            &fonts,
            &AskState::default(),
            2.0,
        );
        // Every row above the field must be free of the field's white fill — i.e. nothing
        // leaked out of the box vertically — and the rows are otherwise painted.
        let px = |x: i32, y: i32| {
            let o = ((y as u32 * w + x as u32) * 4) as usize;
            [buf[o], buf[o + 1], buf[o + 2]]
        };
        let above = field.y - 2;
        assert!(above > 0);
        assert_ne!(px(field.x + 2, above), [0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn paints_an_interactive_balloon() {
        let layout = ask_layout();
        let (w, h) = ask_size(&AskFonts::new(None), &layout, 2.0);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        paint_ask_into(
            &mut buf,
            w,
            h,
            &layout,
            false,
            &BalloonPaint::default(),
            &AskFonts::new(None),
            &AskState::default(),
            2.0,
        );
        assert!(buf.iter().skip(3).step_by(4).any(|&a| a == 0xFF));
    }

    #[test]
    fn think_reserves_more_height_than_speak() {
        let lines = ["Hmm".to_string()];
        let speak = balloon_size(None, &lines, 0, 1, 2.0, false);
        let think = balloon_size(None, &lines, 0, 1, 2.0, true);
        assert!(
            think.1 > speak.1,
            "thought tail is taller: {think:?} vs {speak:?}"
        );
    }
}
