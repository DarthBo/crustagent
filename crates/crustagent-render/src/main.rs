//! A viewer that plays a Microsoft Agent character on the desktop, driven by the
//! `crustagent` embedding API.
//!
//! Usage: `cargo run -p crustagent-render -- <file.acs> [Animation] [--tts]`
//!
//! Two borderless, transparent, always-on-top windows (via `wgpu`), MS-Agent-style: a
//! tight, non-resizable **character** window, and a separate **balloon** window that
//! appears above (or below, near the screen top) the character while it speaks. The
//! character idles by default.
//!
//! Interaction: **left-drag** moves the character, **right-click** opens a command menu
//! (left-click an item to run it), **Esc/Q** quits. `--tts` enables real audio speech via
//! the cross-platform system TTS backend.

mod paint;
mod png;
mod present;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crustagent::{Agent, BalloonKind, Request};
use present::WgpuPresenter;

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId, WindowLevel};

const SCALE: i32 = 3;
const GAP: i32 = 4; // px between balloon and character

const MENU_MAX_H: i32 = 640; // tall menus scroll instead of growing past this

/// All actions: play any of the character's animations (sorted), plus Speak and Hide.
fn build_menu_items(agent: &Agent) -> Vec<(String, Request)> {
    let mut items = vec![
        ("Hide".to_string(), Request::Hide { fast: false }),
        (
            "Speak".to_string(),
            Request::Speak("Hello from crustagent! This is a longer line so you can watch the words appear one at a time.".to_string()),
        ),
        (
            "Think".to_string(),
            Request::Think("Hmm, let me think about that for a moment...".to_string()),
        ),
    ];
    let mut anims = agent.file().gesture_names.clone();
    anims.sort_by_key(|n| n.to_lowercase());
    for name in anims {
        items.push((name.clone(), Request::Play(name)));
    }
    items
}

/// Does `name`'s first (`first == true`) or last frame composite to nothing? Such a frame
/// means the animation appears-from / vanishes-to empty, so it can *be* the entrance/exit —
/// letting us skip the separate Show/Hide animation that would otherwise flash the
/// character in and back out.
fn frame_empty(agent: &Agent, name: &str, first: bool) -> bool {
    let file = agent.file();
    let Some(anim) = file.animation(name) else {
        return false;
    };
    let frame = if first {
        anim.frames.first()
    } else {
        anim.frames.last()
    };
    let Some(frame) = frame else { return false };
    frame.images.is_empty()
        || matches!(file.composite_frame(frame, None), Ok(img) if img.is_fully_transparent())
}

/// Load a real balloon font, matching the character's requested family/size/weight where
/// possible (falling back to a system sans). `None` if the system has no usable fonts, in
/// which case the balloon degrades to the 8x8 bitmap font.
fn load_balloon_font(agent: &Agent) -> Option<paint::Font> {
    let (family, px, bold, italic) = match agent.file().balloon.as_ref() {
        Some(b) => (
            b.font_name.clone(),
            font_px(b.font_height),
            b.bold,
            b.italic,
        ),
        None => (String::new(), 18.0, false, false),
    };
    paint::Font::system(&family, px, bold, italic)
}

/// Turn a LOGFONT `lfHeight` (device units, often negative) into a readable pixel size.
fn font_px(lf_height: i32) -> f32 {
    let p = lf_height.unsigned_abs() as f32;
    if p < 1.0 {
        18.0
    } else {
        p.clamp(15.0, 40.0)
    }
}

/// Build the balloon paint colors from the character's own balloon style, falling back to
/// readable defaults if the file's colors are degenerate (e.g. text == background).
fn balloon_paint(agent: &Agent, kind: BalloonKind) -> paint::BalloonPaint {
    let s = agent.balloon_style();
    let (mut bg, mut text) = (s.bg, s.fg);
    if bg == text {
        bg = (0xFF, 0xFF, 0xE1);
        text = (0x10, 0x10, 0x10);
    }
    paint::BalloonPaint {
        bg: [bg.0, bg.1, bg.2],
        border: [s.border.0, s.border.1, s.border.2],
        text: [text.0, text.1, text.2],
        think: matches!(kind, BalloonKind::Think),
    }
}

fn make_window(el: &ActiveEventLoop, w: u32, h: u32, title: &str) -> Arc<Window> {
    let attrs = Window::default_attributes()
        .with_title(title)
        .with_resizable(false)
        .with_decorations(false)
        .with_transparent(true)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_inner_size(PhysicalSize::new(w, h));
    Arc::new(el.create_window(attrs).expect("create window"))
}

struct App {
    agent: Agent,

    char_window: Option<Arc<Window>>,
    char_presenter: Option<WgpuPresenter>,
    char_scratch: Vec<u8>,

    balloon_window: Option<Arc<Window>>,
    balloon_presenter: Option<WgpuPresenter>,
    balloon_scratch: Vec<u8>,
    balloon_dim: (u32, u32),
    balloon_below: bool,

    // command menu (its own scrollable window)
    menu_window: Option<Arc<Window>>,
    menu_presenter: Option<WgpuPresenter>,
    menu_scratch: Vec<u8>,
    menu_items: Vec<(String, Request)>,
    menu_open: bool,
    menu_scroll: i32,
    menu_cursor: (i32, i32),
    menu_dim: (u32, u32),

    cursor: (i32, i32),
    last: Instant,

    // real balloon font (system TrueType), chosen from the character's balloon config
    font: Option<paint::Font>,

    // graceful shutdown: play Goodbye + Hide before exiting
    quitting: bool,
    quit_deadline: Option<Instant>,

    // print the agent's event stream to stdout (--events)
    log_events: bool,
}

impl App {
    /// Create the balloon window on first use, then keep it (shown/hidden per phrase).
    fn ensure_balloon_window(&mut self, el: &ActiveEventLoop, w: u32, h: u32) {
        if self.balloon_window.is_none() {
            let win = make_window(el, w, h, "crustagent balloon");
            win.set_visible(false);
            self.balloon_presenter = Some(WgpuPresenter::new(win.clone()));
            self.balloon_window = Some(win);
            self.balloon_dim = (w, h);
        } else if self.balloon_dim != (w, h) {
            if let Some(win) = &self.balloon_window {
                let _ = win.request_inner_size(PhysicalSize::new(w, h));
            }
            self.balloon_dim = (w, h);
        }
    }

    /// Position the balloon window above the character (or below, near the screen top).
    fn reposition_balloon(&mut self) {
        let (Some(cw), Some(bw)) = (&self.char_window, &self.balloon_window) else {
            return;
        };
        let Ok(cpos) = cw.outer_position() else {
            return;
        };
        let csize = cw.outer_size();
        let (bwidth, bheight) = self.balloon_dim;
        let bx = cpos.x + (csize.width as i32 - bwidth as i32) / 2;
        let mut by = cpos.y - bheight as i32 - GAP;
        self.balloon_below = by < 0;
        if self.balloon_below {
            by = cpos.y + csize.height as i32 + GAP;
        }
        bw.set_outer_position(PhysicalPosition::new(bx, by));
    }

    fn compose_char(&mut self, w: u32, h: u32) {
        let img = self.agent.composite_current();
        self.char_scratch.clear();
        self.char_scratch.resize((w * h * 4) as usize, 0); // transparent

        let mut canvas = paint::Canvas::new(&mut self.char_scratch, w, h);
        if let Some(img) = &img {
            let cw = img.width as i32;
            let ch = img.height as i32;
            let ox = (w as i32 - cw * SCALE) / 2;
            let oy = (h as i32 - ch * SCALE) / 2;
            canvas.blit_scaled(&img.pixels, cw, ch, ox, oy, SCALE);
        }
    }

    // -- command menu (its own window) ---------------------------------------

    fn menu_size(&self) -> (u32, u32) {
        let cols = self
            .menu_items
            .iter()
            .map(|(l, _)| l.chars().count())
            .max()
            .unwrap_or(6) as i32;
        let w = (cols * 8 * paint::MENU_SCALE + 12).clamp(80, 480);
        let content = self.menu_items.len() as i32 * paint::MENU_ROW_H + 4;
        let h = content.clamp(paint::MENU_ROW_H + 4, MENU_MAX_H);
        (w as u32, h as u32)
    }

    fn menu_max_scroll(&self) -> i32 {
        let content = self.menu_items.len() as i32 * paint::MENU_ROW_H + 4;
        (content - self.menu_dim.1 as i32).max(0)
    }

    /// Open the menu at a screen position (top-left), creating its window on first use.
    fn open_menu(&mut self, el: &ActiveEventLoop, screen: PhysicalPosition<i32>) {
        let (w, h) = self.menu_size();
        if self.menu_window.is_none() {
            let win = make_window(el, w, h, "crustagent menu");
            win.set_visible(false);
            self.menu_presenter = Some(WgpuPresenter::new(win.clone()));
            self.menu_window = Some(win);
            self.menu_dim = (w, h);
        }
        if let Some(win) = &self.menu_window {
            win.set_outer_position(screen);
            win.set_visible(true);
            win.focus_window();
            win.request_redraw();
        }
        self.menu_open = true;
        self.menu_scroll = 0;
        self.menu_cursor = (-1, -1);
    }

    fn close_menu(&mut self) {
        self.menu_open = false;
        if let Some(win) = &self.menu_window {
            win.set_visible(false);
        }
    }

    /// Begin a graceful shutdown: wave Goodbye, then Hide; the event loop exits once the
    /// character is hidden (or after a safety timeout).
    fn begin_quit(&mut self) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        self.close_menu();
        self.agent.stop();
        // Characters like Clippit/Genius have a dedicated Goodbye animation that IS the
        // exit (it ends on an empty frame). Play it, then hide instantly, otherwise the
        // quick Hide pop would make them reappear and blink out again. Characters without
        // a Goodbye (e.g. Merlin) use their native Hide animation as the exit — Wave is a
        // mid-scene gesture, not the goodbye.
        if self.agent.file().animation("Goodbye").is_some() {
            self.agent.play("Goodbye");
            if frame_empty(&self.agent, "Goodbye", false) {
                self.agent.hide_fast();
            } else {
                self.agent.hide();
            }
        } else {
            self.agent.hide();
        }
        self.quit_deadline = Some(Instant::now() + Duration::from_secs(6));
    }

    fn menu_hover(&self) -> Option<usize> {
        let (cx, cy) = self.menu_cursor;
        if cx < 0 || cy < 0 || cx >= self.menu_dim.0 as i32 || cy >= self.menu_dim.1 as i32 {
            return None;
        }
        let i = ((cy + self.menu_scroll - 2) / paint::MENU_ROW_H) as usize;
        (i < self.menu_items.len()).then_some(i)
    }

    fn compose_menu(&mut self, w: u32, h: u32) {
        let labels: Vec<String> = self.menu_items.iter().map(|(l, _)| l.clone()).collect();
        let scroll = self.menu_scroll;
        let hover = self.menu_hover();
        self.menu_scratch.clear();
        self.menu_scratch.resize((w * h * 4) as usize, 0);
        let mut canvas = paint::Canvas::new(&mut self.menu_scratch, w, h);
        canvas.menu_list(&labels, scroll, hover);
    }

    fn compose_balloon(&mut self, w: u32, h: u32) {
        let balloon = self.agent.balloon();
        let below = self.balloon_below;
        self.balloon_scratch.clear();
        self.balloon_scratch.resize((w * h * 4) as usize, 0); // transparent

        if let Some(bv) = &balloon {
            let mut canvas = paint::Canvas::new(&mut self.balloon_scratch, w, h);
            let style = balloon_paint(&self.agent, bv.kind);
            // The tail is centered on the window, which is sized to fit the balloon.
            canvas.balloon(&bv.layout.lines, below, &style, self.font.as_ref());
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.char_window.is_some() {
            return;
        }
        let (cw, ch) = self.agent.size();
        let name = self
            .agent
            .file()
            .default_name()
            .map(|n| n.name.clone())
            .unwrap_or_default();
        let win = make_window(
            el,
            cw * SCALE as u32,
            ch * SCALE as u32,
            &format!("crustagent — {name}"),
        );
        win.request_redraw();
        self.char_presenter = Some(WgpuPresenter::new(win.clone()));
        self.char_window = Some(win);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let is_char = self.char_window.as_ref().is_some_and(|w| w.id() == id);
        let is_balloon = self.balloon_window.as_ref().is_some_and(|w| w.id() == id);
        let is_menu = self.menu_window.as_ref().is_some_and(|w| w.id() == id);

        match event {
            WindowEvent::CloseRequested => {
                if self.quitting {
                    el.exit();
                } else {
                    self.begin_quit();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Escape) if is_menu => self.close_menu(),
                    PhysicalKey::Code(KeyCode::Escape | KeyCode::KeyQ) => {
                        // First press: play Goodbye and hide; press again to force-quit.
                        if self.quitting {
                            el.exit();
                        } else {
                            self.begin_quit();
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if is_char {
                    self.cursor = (position.x as i32, position.y as i32);
                } else if is_menu {
                    self.menu_cursor = (position.x as i32, position.y as i32);
                    if let Some(w) = &self.menu_window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } if is_menu => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y * paint::MENU_ROW_H as f32) as i32,
                    MouseScrollDelta::PixelDelta(p) => p.y as i32,
                };
                self.menu_scroll = (self.menu_scroll - dy).clamp(0, self.menu_max_scroll());
                if let Some(w) = &self.menu_window {
                    w.request_redraw();
                }
            }
            WindowEvent::Focused(false) if is_menu => self.close_menu(),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                if is_char {
                    let (cx, cy) = self.cursor;
                    match button {
                        MouseButton::Right => {
                            self.agent.report_click(crustagent::MouseButton::Right, cx, cy);
                            let screen = self
                                .char_window
                                .as_ref()
                                .and_then(|w| w.outer_position().ok())
                                .map(|p| {
                                    PhysicalPosition::new(p.x + self.cursor.0, p.y + self.cursor.1)
                                });
                            if let Some(screen) = screen {
                                self.open_menu(el, screen);
                            }
                        }
                        MouseButton::Left => {
                            self.agent.report_click(crustagent::MouseButton::Left, cx, cy);
                            if self.menu_open {
                                self.close_menu();
                            } else if let Some(window) = &self.char_window {
                                // Whole-body drag; report it so a host can react.
                                self.agent.report_drag_start();
                                let _ = window.drag_window();
                                self.agent.report_drag_complete();
                            }
                        }
                        _ => {}
                    }
                } else if is_menu && button == MouseButton::Left {
                    if let Some(i) = self.menu_hover() {
                        self.agent.request(self.menu_items[i].1.clone());
                    }
                    self.close_menu();
                }
            }
            WindowEvent::RedrawRequested if is_char => {
                if let Some(window) = self.char_window.clone() {
                    let s = window.inner_size();
                    if s.width > 0 && s.height > 0 {
                        self.compose_char(s.width, s.height);
                        if let Some(p) = self.char_presenter.as_mut() {
                            p.present(&self.char_scratch, s.width, s.height);
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested if is_balloon => {
                if let Some(window) = self.balloon_window.clone() {
                    let s = window.inner_size();
                    if s.width > 0 && s.height > 0 {
                        self.compose_balloon(s.width, s.height);
                        if let Some(p) = self.balloon_presenter.as_mut() {
                            p.present(&self.balloon_scratch, s.width, s.height);
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested if is_menu => {
                if let Some(window) = self.menu_window.clone() {
                    let s = window.inner_size();
                    if s.width > 0 && s.height > 0 {
                        self.compose_menu(s.width, s.height);
                        if let Some(p) = self.menu_presenter.as_mut() {
                            p.present(&self.menu_scratch, s.width, s.height);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_millis() as u32;
        self.last = now;
        self.agent.update(dt);

        // Surface the agent's event stream (what an integrating app would consume).
        let events = self.agent.drain_events();
        if self.log_events {
            for e in &events {
                println!("[event] {e:?}");
            }
        }

        // Once Goodbye + Hide finish (character no longer visible), or on timeout, exit.
        if self.quitting {
            let hidden = !self.agent.is_visible();
            let timed_out = self.quit_deadline.is_some_and(|d| now >= d);
            if hidden || timed_out {
                el.exit();
                return;
            }
        }

        // Balloon window: size once per phrase, keep it, show/hide as speech starts/stops.
        if let Some(bv) = self.agent.balloon() {
            let (bw, bh) =
                paint::balloon_size(self.font.as_ref(), &bv.full.lines, bv.full.cols, bv.full.rows);
            self.ensure_balloon_window(el, bw, bh);
            self.reposition_balloon();
            if let Some(win) = &self.balloon_window {
                win.set_visible(true);
                win.request_redraw();
            }
        } else if let Some(win) = &self.balloon_window {
            win.set_visible(false);
        }

        el.set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(16)));
        if let Some(window) = &self.char_window {
            window.request_redraw();
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");

    if let Some(i) = args.iter().position(|a| a == "--balloon-png") {
        let out = args.get(i + 1).cloned().unwrap_or_else(|| "balloon.png".into());
        let font = paint::Font::system("", 20.0, false, false);
        let style = |think| paint::BalloonPaint {
            bg: [0xFF, 0xFF, 0xE1],
            border: [0x40, 0x40, 0x40],
            text: [0x10, 0x10, 0x10],
            think,
        };
        let lines = vec!["Real TrueType balloon text!".to_string()];
        let (w, h) = paint::balloon_size(font.as_ref(), &lines, lines[0].len(), 1);
        let mut buf = vec![0x50u8; (w * h * 4) as usize];
        for px in buf.chunks_exact_mut(4) {
            px[3] = 0xFF;
        }
        let mut canvas = paint::Canvas::new(&mut buf, w, h);
        canvas.balloon(&lines, false, &style(false), font.as_ref());
        std::fs::write(&out, png::encode_rgba(&buf, w, h)).expect("write png");
        println!("wrote {out} ({w}x{h}, font: {})", if font.is_some() { "system" } else { "8x8 fallback" });
        return;
    }

    if let Some(i) = args.iter().position(|a| a == "--menu-png") {
        let out = args.get(i + 1).cloned().unwrap_or_else(|| "menu.png".into());
        let labels: Vec<String> = ["Hide", "Speak", "Acknowledge", "Blink", "Congratulate", "Greet"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (w, h) = (220u32, (labels.len() as i32 * paint::MENU_ROW_H + 4) as u32);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let mut canvas = paint::Canvas::new(&mut buf, w, h);
        canvas.menu_list(&labels, 0, Some(2)); // hover the 3rd row
        std::fs::write(&out, png::encode_rgba(&buf, w, h)).expect("write png");
        println!("wrote {out}");
        return;
    }

    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let Some(path) = positional.first().map(|s| (*s).clone()) else {
        eprintln!("usage: crustagent-render <file.acs> [Animation] [--tts]");
        std::process::exit(2);
    };
    let mut agent = Agent::load(&path).unwrap_or_else(|e| {
        eprintln!("parse {path}: {e}");
        std::process::exit(1);
    });

    // Play the character's embedded sound effects (if an audio device is available).
    if let Some(sink) = crustagent_audio::RodioSink::new() {
        agent.set_audio_sink(Box::new(sink));
    }
    if args.iter().any(|a| a == "--tts") {
        agent.set_tts(crustagent::default_engine());
    }

    if let Some(name) = positional.get(1) {
        if agent.file().animation(name).is_none() {
            eprintln!("no animation {name:?}. Available:");
            for n in &agent.file().gesture_names {
                eprintln!("  {n}");
            }
            std::process::exit(1);
        }
        agent.show();
        agent.play((*name).clone());
    } else if agent.file().animation("Greeting").is_some() {
        // Characters like Clippit/Genius have a dedicated Greeting animation that IS the
        // entrance (it appears from an empty frame). Show instantly and let Greeting play,
        // otherwise the quick Show pop would flash the character in before it reappears.
        // (A Greeting that starts from rest instead just plays after a normal Show.)
        if frame_empty(&agent, "Greeting", true) {
            agent.show_fast();
        } else {
            agent.show();
        }
        agent.play("Greeting");
    } else {
        // No dedicated greeting (e.g. Merlin): the native Show animation *is* the entrance
        // (the magician's puff-of-smoke appear). Greet/Wave are mid-scene gestures, not
        // the entrance, so we don't play them here.
        agent.show();
    }

    let name = agent.file().default_name().map(|n| n.name.clone()).unwrap_or_default();
    println!("{name}: right-click for a menu · left-drag to move · Esc/Q to quit");
    println!("(pass --tts for audible speech)");

    if dry_run {
        agent.speak("Hello from crustagent!");
        agent.play("Greet");
        for _ in 0..200 {
            agent.update(16);
        }
        println!("dry run OK ({}x{})", agent.size().0, agent.size().1);
        return;
    }

    let menu_items = build_menu_items(&agent);
    let font = load_balloon_font(&agent);
    let mut app = App {
        agent,
        char_window: None,
        char_presenter: None,
        char_scratch: Vec::new(),
        balloon_window: None,
        balloon_presenter: None,
        balloon_scratch: Vec::new(),
        balloon_dim: (0, 0),
        balloon_below: false,
        menu_window: None,
        menu_presenter: None,
        menu_scratch: Vec::new(),
        menu_items,
        menu_open: false,
        menu_scroll: 0,
        menu_cursor: (-1, -1),
        menu_dim: (0, 0),
        cursor: (0, 0),
        last: Instant::now(),
        font,
        quitting: false,
        quit_deadline: None,
        log_events: args.iter().any(|a| a == "--events"),
    };

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app).expect("run");
}
