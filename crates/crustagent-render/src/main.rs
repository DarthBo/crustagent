//! A viewer that plays a Microsoft Agent character on the desktop, driven by the
//! `crustagent` embedding API.
//!
//! Usage: `cargo run -p crustagent-render -- <file.acs> [Animation] [--say]`
//!
//! Two borderless, transparent, always-on-top windows (via `wgpu`), MS-Agent-style: a
//! tight, non-resizable **character** window, and a separate **balloon** window that
//! appears above (or below, near the screen top) the character while it speaks. The
//! character idles by default.
//!
//! Interaction: **left-drag** moves the character, **right-click** opens a command menu
//! (left-click an item to run it), **Esc/Q** quits. `--say` uses a real audio TTS backend
//! (macOS).

mod paint;
mod png;
mod present;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crustagent::{Agent, Request};
use present::WgpuPresenter;

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId, WindowLevel};

const SCALE: i32 = 3;
const MENU_SCALE: i32 = 2;
const GAP: i32 = 4; // px between balloon and character

#[derive(Clone)]
struct Menu {
    x: i32,
    y: i32,
    items: Vec<(String, Request)>,
}

impl Menu {
    fn row_h() -> i32 {
        8 * MENU_SCALE + 6
    }
    fn width(&self) -> i32 {
        let cols = self
            .items
            .iter()
            .map(|(l, _)| l.chars().count())
            .max()
            .unwrap_or(4) as i32;
        cols * 8 * MENU_SCALE + 14
    }
    fn height(&self) -> i32 {
        self.items.len() as i32 * Self::row_h() + 4
    }
    fn hit(&self, cx: i32, cy: i32) -> Option<usize> {
        if cx < self.x || cx >= self.x + self.width() || cy < self.y || cy >= self.y + self.height()
        {
            return None;
        }
        let i = ((cy - self.y - 2) / Self::row_h()) as usize;
        (i < self.items.len()).then_some(i)
    }
}

fn build_menu(agent: &Agent, x: i32, y: i32) -> Menu {
    let mut items = Vec::new();
    for g in ["Greet", "Wave", "Congratulate", "Pleased", "Surprised", "Read"] {
        if agent.file().animation(g).is_some() {
            items.push((g.to_string(), Request::Play(g.to_string())));
        }
    }
    items.push((
        "Speak".to_string(),
        Request::Speak("Hello from crustagent!".to_string()),
    ));
    items.push(("Hide".to_string(), Request::Hide));
    Menu { x, y, items }
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

    cursor: (i32, i32),
    menu: Option<Menu>,
    last: Instant,
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
        let menu = self.menu.clone();
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
        if let Some(m) = &menu {
            canvas.menu(m.x, m.y, m.width(), Menu::row_h(), &m.items);
        }
    }

    fn compose_balloon(&mut self, w: u32, h: u32) {
        let balloon = self.agent.balloon();
        let below = self.balloon_below;
        self.balloon_scratch.clear();
        self.balloon_scratch.resize((w * h * 4) as usize, 0); // transparent

        if let Some(bv) = &balloon {
            let mut canvas = paint::Canvas::new(&mut self.balloon_scratch, w, h);
            // Tail tip at the window edge nearest the character.
            let (tip_x, tip_y) = if below {
                (w as i32 / 2, 0)
            } else {
                (w as i32 / 2, h as i32 - 1)
            };
            canvas.balloon(&bv.layout.lines, tip_x, tip_y, below);
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

        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    if let PhysicalKey::Code(KeyCode::Escape | KeyCode::KeyQ) = event.physical_key {
                        el.exit();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } if is_char => {
                self.cursor = (position.x as i32, position.y as i32);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } if is_char => match button {
                MouseButton::Right => {
                    self.menu = Some(build_menu(&self.agent, self.cursor.0, self.cursor.1));
                }
                MouseButton::Left => {
                    if let Some(menu) = self.menu.take() {
                        if let Some(i) = menu.hit(self.cursor.0, self.cursor.1) {
                            self.agent.request(menu.items[i].1.clone());
                        }
                    } else if let Some(window) = &self.char_window {
                        let _ = window.drag_window();
                    }
                }
                _ => {}
            },
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
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_millis() as u32;
        self.last = now;
        self.agent.update(dt);

        // Balloon window: size once per phrase, keep it, show/hide as speech starts/stops.
        if let Some(bv) = self.agent.balloon() {
            let (bw, bh) = paint::balloon_size(bv.full.cols, bv.full.rows);
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
        let (w, h) = (420u32, 120u32);
        let mut buf = vec![0x50u8; (w * h * 4) as usize];
        for px in buf.chunks_exact_mut(4) {
            px[3] = 0xFF;
        }
        let mut canvas = paint::Canvas::new(&mut buf, w, h);
        canvas.balloon(&["Hello from crustagent!".to_string()], (w / 2) as i32, 108, false);
        std::fs::write(&out, png::encode_rgba(&buf, w, h)).expect("write png");
        println!("wrote {out}");
        return;
    }

    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let Some(path) = positional.first().map(|s| (*s).clone()) else {
        eprintln!("usage: crustagent-render <file.acs> [Animation] [--say]");
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
    if args.iter().any(|a| a == "--say") {
        agent.set_tts(crustagent::default_engine());
    }

    agent.show();
    if let Some(name) = positional.get(1) {
        if agent.file().animation(name).is_none() {
            eprintln!("no animation {name:?}. Available:");
            for n in &agent.file().gesture_names {
                eprintln!("  {n}");
            }
            std::process::exit(1);
        }
        agent.play((*name).clone());
    }

    let name = agent.file().default_name().map(|n| n.name.clone()).unwrap_or_default();
    println!("{name}: right-click for a menu · left-drag to move · Esc/Q to quit");
    println!("(pass --say for audible speech)");

    if dry_run {
        agent.speak("Hello from crustagent!");
        agent.play("Greet");
        for _ in 0..200 {
            agent.update(16);
        }
        println!("dry run OK ({}x{})", agent.size().0, agent.size().1);
        return;
    }

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
        cursor: (0, 0),
        menu: None,
        last: Instant::now(),
    };

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app).expect("run");
}
