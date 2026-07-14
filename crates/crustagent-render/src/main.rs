//! A windowed viewer that plays a Microsoft Agent character, driven by the `crustagent`
//! embedding API.
//!
//! Usage: `cargo run -p crustagent-render -- <file.acs> [Animation] [--float] [--dry-run]`
//!
//! The character idles by default; a balloon appears when it speaks. Interaction:
//! - **left-drag** moves it (system window drag),
//! - **right-click** opens a small command menu; **left-click** an item to run it,
//! - **Esc/Q** quits.
//!
//! `--float` renders through the `wgpu` backend (transparent, borderless, always-on-top);
//! otherwise a `softbuffer` window draws on a transparency checkerboard.

mod paint;
mod png;
mod present;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crustagent::{Agent, Request};
use present::{Presenter, SoftPresenter, WgpuPresenter};

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId, WindowLevel};

const SCALE: i32 = 3;
const STRIP: i32 = 72; // modest area reserved above the character for the balloon
const MENU_SCALE: i32 = 2;

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

struct App {
    agent: Agent,
    float: bool,
    scratch: Vec<u8>,
    cursor: (i32, i32),
    menu: Option<Menu>,
    last: Instant,
    window: Option<Arc<Window>>,
    presenter: Option<Box<dyn Presenter>>,
}

impl App {
    fn compose(&mut self, win_w: u32, win_h: u32) {
        // Gather owned data first (no borrow held while writing scratch).
        let img = self.agent.composite_current();
        let balloon = self.agent.balloon();
        let menu = self.menu.clone();
        let float = self.float;

        let w = win_w as i32;
        self.scratch.resize((win_w * win_h * 4) as usize, 0);
        let buf = &mut self.scratch;

        // background
        for y in 0..win_h {
            for x in 0..win_w {
                let o = ((y * win_w + x) * 4) as usize;
                let (r, g, b, a) = if float {
                    (0, 0, 0, 0)
                } else if ((x / 16) + (y / 16)).is_multiple_of(2) {
                    (0xC8, 0xC8, 0xC8, 0xFF)
                } else {
                    (0x90, 0x90, 0x90, 0xFF)
                };
                buf[o] = r;
                buf[o + 1] = g;
                buf[o + 2] = b;
                buf[o + 3] = a;
            }
        }

        let mut canvas = paint::Canvas::new(buf, win_w, win_h);

        // character (centered horizontally, below the balloon strip)
        if let Some(img) = &img {
            let cw = img.width as i32;
            let ox = (w - cw * SCALE) / 2;
            canvas.blit_scaled(&img.pixels, cw, img.height as i32, ox, STRIP, SCALE);
        }
        // balloon: sits above the character, tail pointing down at its head
        if let Some(b) = &balloon {
            canvas.balloon(&b.layout.lines, w / 2, STRIP, false);
        }
        // menu
        if let Some(m) = &menu {
            canvas.menu(m.x, m.y, m.width(), Menu::row_h(), &m.items);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let (cw, ch) = self.agent.size();
        let win_w = (cw as i32 * SCALE).max(320);
        // Character sits at the bottom; a modest strip on top holds the balloon.
        let win_h = ch as i32 * SCALE + STRIP;
        let name = self
            .agent
            .file()
            .default_name()
            .map(|n| n.name.clone())
            .unwrap_or_default();
        // Size in *physical* pixels: we composite in physical pixels (the surface's real
        // size), so a logical size would double the window on HiDPI and leave dead space.
        let mut attrs = Window::default_attributes()
            .with_title(format!("crustagent — {name}"))
            .with_inner_size(PhysicalSize::new(win_w as u32, win_h as u32));
        if self.float {
            attrs = attrs
                .with_decorations(false)
                .with_transparent(true)
                .with_window_level(WindowLevel::AlwaysOnTop);
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let presenter: Box<dyn Presenter> = if self.float {
            Box::new(WgpuPresenter::new(window.clone()))
        } else {
            Box::new(SoftPresenter::new(window.clone()))
        };
        window.request_redraw();
        self.window = Some(window);
        self.presenter = Some(presenter);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    if let PhysicalKey::Code(KeyCode::Escape | KeyCode::KeyQ) = event.physical_key {
                        event_loop.exit();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as i32, position.y as i32);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => match button {
                MouseButton::Right => {
                    self.menu = Some(build_menu(&self.agent, self.cursor.0, self.cursor.1));
                }
                MouseButton::Left => {
                    if let Some(menu) = self.menu.take() {
                        if let Some(i) = menu.hit(self.cursor.0, self.cursor.1) {
                            self.agent.request(menu.items[i].1.clone());
                        }
                    } else if let Some(window) = &self.window {
                        let _ = window.drag_window();
                    }
                }
                _ => {}
            },
            WindowEvent::RedrawRequested => {
                let Some(window) = self.window.clone() else {
                    return;
                };
                let size = window.inner_size();
                if size.width == 0 || size.height == 0 {
                    return;
                }
                self.compose(size.width, size.height);
                if let Some(p) = self.presenter.as_mut() {
                    p.present(&self.scratch, size.width, size.height);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_millis() as u32;
        self.last = now;
        self.agent.update(dt);

        event_loop.set_control_flow(ControlFlow::WaitUntil(
            now + Duration::from_millis(16),
        ));
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let float = args.iter().any(|a| a == "--float");
    let dry_run = args.iter().any(|a| a == "--dry-run");

    // Debug: render a sample balloon to a PNG and exit (for headless visual checks).
    if let Some(i) = args.iter().position(|a| a == "--balloon-png") {
        let out = args.get(i + 1).cloned().unwrap_or_else(|| "balloon.png".into());
        let (w, h) = (420u32, 120u32);
        let mut buf = vec![0x50u8; (w * h * 4) as usize]; // opaque dark-gray bg
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
        eprintln!("usage: crustagent-render <file.acs> [Animation] [--float] [--dry-run]");
        std::process::exit(2);
    };
    let mut agent = Agent::load(&path).unwrap_or_else(|e| {
        eprintln!("parse {path}: {e}");
        std::process::exit(1);
    });

    // --say: use a real audio TTS backend (macOS `say`) so speech is audible.
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
        // Exercise a scripted session headlessly.
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
        float,
        scratch: Vec::new(),
        cursor: (0, 0),
        menu: None,
        last: Instant::now(),
        window: None,
        presenter: None,
    };

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app).expect("run");
}
