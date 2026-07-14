//! A windowed viewer that plays a Microsoft Agent character on screen.
//!
//! Usage: `cargo run -p crustagent-render -- <file.acs> [Animation] [--float] [--dry-run]`
//!
//! With no animation named, the character **idles**: it plays escalating `IDLINGLEVEL`
//! animations back-to-back (via `crustagent_core::IdleDirector`), like the desktop
//! assistant standing around. Name an animation to loop that gesture (its full
//! start/continued/return) instead.
//!
//! Presentation:
//! - default: an opaque window drawing the character on a transparency checkerboard
//!   (`softbuffer`) — reliable everywhere.
//! - `--float`: a borderless, always-on-top, **transparent** window rendered with `wgpu`
//!   (premultiplied-alpha surface) — the desktop-buddy look. Drag it anywhere.
//!
//! Drag the character with the left mouse button. Esc/Q quits.

mod present;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crustagent_core::{sequence_animation, Character, IdleDirector, SplitMix64};
use crustagent_format::AcsFile;
use present::{Presenter, SoftPresenter, WgpuPresenter};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId, WindowLevel};

const SCALE: u32 = 3;
const CHECKER: u32 = 16;

struct CompiledFrame {
    /// Top-down RGBA8, character-sized.
    pixels: Vec<u8>,
    delay_ms: u32,
}

enum Mode {
    /// Loop escalating idle animations.
    Idle,
    /// Loop one named gesture (base + continued + return).
    Gesture(String),
}

struct App {
    chr: AcsFile,
    mode: Mode,
    float: bool,
    char_w: u32,
    char_h: u32,
    rng: SplitMix64,
    director: IdleDirector,

    // current clip
    frames: Vec<CompiledFrame>,
    clip_total_ms: u32,
    clip_loop: bool,
    clip_start: Instant,

    scratch: Vec<u8>,
    window: Option<Arc<Window>>,
    presenter: Option<Box<dyn Presenter>>,
}

impl App {
    /// Build the next clip to play. In gesture mode this is the full gesture (looping);
    /// in idle mode it's the next single idle animation (play-once, then advance).
    fn build_clip(&mut self) {
        let (names, do_loop): (Vec<String>, bool) = match &self.mode {
            Mode::Gesture(name) => {
                let name = name.clone();
                let ch = Character::new(&self.chr);
                let parts = ch
                    .full_gesture(&name)
                    .iter()
                    .map(|a| a.name.clone())
                    .collect();
                (parts, true)
            }
            Mode::Idle => {
                let ch = Character::new(&self.chr);
                match self.director.next_idle(&ch, &mut self.rng) {
                    Some(n) => (vec![n], false),
                    None => (Vec::new(), true),
                }
            }
        };

        let ch = Character::new(&self.chr);
        let mut frames = Vec::new();
        for name in &names {
            if let Some(anim) = ch.animation(name) {
                let seq = sequence_animation(anim, &mut self.rng);
                for e in &seq.frames {
                    if let Ok(img) = self.chr.composite_frame(&anim.frames[e.frame], None) {
                        frames.push(CompiledFrame {
                            pixels: img.pixels,
                            delay_ms: (e.duration_cs as u32 * 10).max(1),
                        });
                    }
                }
            }
        }

        // Fall back to a single static rest frame if we produced nothing.
        if frames.is_empty() {
            if let Ok(img) = self
                .chr
                .animation("RestPose")
                .and_then(|a| a.frames.first())
                .map(|f| self.chr.composite_frame(f, None))
                .unwrap_or_else(|| self.chr.composite_frame(&empty_frame(), None))
            {
                frames.push(CompiledFrame {
                    pixels: img.pixels,
                    delay_ms: 1000,
                });
            }
        }

        self.clip_total_ms = frames.iter().map(|f| f.delay_ms).sum::<u32>().max(1);
        self.clip_loop = do_loop;
        self.frames = frames;
        self.clip_start = Instant::now();
    }

    fn current_index(&self) -> usize {
        if self.frames.is_empty() {
            return 0;
        }
        let elapsed = self.clip_start.elapsed().as_millis() as u32;
        let t = if self.clip_loop {
            elapsed % self.clip_total_ms
        } else {
            elapsed.min(self.clip_total_ms.saturating_sub(1))
        };
        let mut acc = 0;
        for (i, f) in self.frames.iter().enumerate() {
            acc += f.delay_ms;
            if t < acc {
                return i;
            }
        }
        self.frames.len() - 1
    }

    /// True when a play-once (idle) clip has finished and we should pick the next one.
    fn clip_finished(&self) -> bool {
        !self.clip_loop && self.clip_start.elapsed().as_millis() as u32 >= self.clip_total_ms
    }

    fn compose(&mut self, win_w: u32, win_h: u32) {
        self.scratch.resize((win_w * win_h * 4) as usize, 0);
        if self.frames.is_empty() {
            return;
        }
        let idx = self.current_index();
        let frame = &self.frames[idx];
        let float = self.float;
        let (cw, ch) = (self.char_w, self.char_h);
        let scale = (win_w / cw).min(win_h / ch).max(1);
        let (dw, dh) = (cw * scale, ch * scale);
        let ox = win_w.saturating_sub(dw) / 2;
        let oy = win_h.saturating_sub(dh) / 2;

        for y in 0..win_h {
            for x in 0..win_w {
                let o = ((y * win_w + x) * 4) as usize;
                // background
                let (mut r, mut g, mut b, mut a) = if float {
                    (0, 0, 0, 0)
                } else if ((x / CHECKER) + (y / CHECKER)).is_multiple_of(2) {
                    (0xC8, 0xC8, 0xC8, 0xFF)
                } else {
                    (0x90, 0x90, 0x90, 0xFF)
                };
                if x >= ox && x < ox + dw && y >= oy && y < oy + dh {
                    let cx = (x - ox) / scale;
                    let cy = (y - oy) / scale;
                    let p = ((cy * cw + cx) * 4) as usize;
                    if frame.pixels[p + 3] != 0 {
                        r = frame.pixels[p];
                        g = frame.pixels[p + 1];
                        b = frame.pixels[p + 2];
                        a = 0xFF;
                    }
                }
                self.scratch[o] = r;
                self.scratch[o + 1] = g;
                self.scratch[o + 2] = b;
                self.scratch[o + 3] = a;
            }
        }
    }
}

fn empty_frame() -> crustagent_format::Frame {
    crustagent_format::Frame {
        duration: 100,
        sound_ndx: -1,
        exit_frame: -1,
        branching: vec![],
        images: vec![],
        overlays: vec![],
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let name = self.chr.default_name().map(|n| n.name.clone()).unwrap_or_default();
        let mut attrs = Window::default_attributes()
            .with_title(format!("crustagent — {name}"))
            .with_inner_size(LogicalSize::new(self.char_w * SCALE, self.char_h * SCALE));
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
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(window) = &self.window {
                    let _ = window.drag_window(); // system move; borderless-friendly
                }
            }
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
        if self.clip_finished() {
            self.build_clip(); // advance to the next idle animation
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(16),
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
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();

    let Some(path) = positional.first().map(|s| (*s).clone()) else {
        eprintln!("usage: crustagent-render <file.acs> [Animation] [--float] [--dry-run]");
        std::process::exit(2);
    };
    let chr = AcsFile::open(&path).unwrap_or_else(|e| {
        eprintln!("parse {path}: {e}");
        std::process::exit(1);
    });

    let mode = match positional.get(1) {
        Some(name) => {
            if Character::new(&chr).full_gesture(name).is_empty() {
                eprintln!("no animation {name:?}. Available:");
                for n in &chr.gesture_names {
                    eprintln!("  {n}");
                }
                std::process::exit(1);
            }
            Mode::Gesture((*name).clone())
        }
        None => Mode::Idle,
    };

    let director = IdleDirector::new(&Character::new(&chr));
    let (char_w, char_h) = (chr.header.image_size.0 as u32, chr.header.image_size.1 as u32);
    let name = chr.default_name().map(|n| n.name.clone()).unwrap_or_default();

    let mut app = App {
        chr,
        mode,
        float,
        char_w,
        char_h,
        rng: SplitMix64::new(0),
        director,
        frames: Vec::new(),
        clip_total_ms: 1,
        clip_loop: true,
        clip_start: Instant::now(),
        scratch: Vec::new(),
        window: None,
        presenter: None,
    };
    app.build_clip();

    match &app.mode {
        Mode::Idle => println!("{name} is idling (level {})", app.director.level()),
        Mode::Gesture(g) => println!("{name} playing {g}"),
    }
    println!(
        "{} frames ready{}",
        app.frames.len(),
        if float { "  [float]" } else { "" }
    );

    if dry_run {
        println!("dry run OK ({}x{})", char_w, char_h);
        return;
    }
    println!("drag to move · Esc/Q to quit");

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app).expect("run");
}
