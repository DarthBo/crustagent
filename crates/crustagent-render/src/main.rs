//! A windowed viewer that plays a Microsoft Agent character on screen.
//!
//! Usage: `cargo run -p crustagent-render -- <file.acs> [Animation] [--float]`
//!
//! It parses the character (`crustagent-format`), builds the full gesture sequence
//! (`crustagent-core`), composites each frame to RGBA, and presents it in a window via
//! `softbuffer`, looping in real time.
//!
//! By default the character is drawn on a transparency checkerboard (works everywhere).
//! `--float` requests a borderless, always-on-top, transparent window — the "desktop
//! buddy" effect. That relies on the platform honoring per-pixel window alpha; where it
//! doesn't (notably macOS with the software `softbuffer` backend), the transparent areas
//! fall back to black, so the checkerboard mode is the reliable default for now. A true
//! cross-platform transparent overlay is a planned enhancement (GPU/compositor backend).

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crustagent_core::{sequence_animation, Character, SplitMix64};
use crustagent_format::AcsFile;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId, WindowLevel};

const SCALE: u32 = 3;
const CHECKER: u32 = 16; // checkerboard tile size, in window pixels

struct CompiledFrame {
    /// Top-down RGBA8, `w * h * 4` bytes.
    pixels: Vec<u8>,
    delay_ms: u32,
}

struct App {
    title: String,
    float: bool,
    char_w: u32,
    char_h: u32,
    frames: Vec<CompiledFrame>,
    total_ms: u32,
    start: Instant,

    window: Option<Rc<Window>>,
    context: Option<softbuffer::Context<Rc<Window>>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
}

impl App {
    /// Index into `frames` of the frame that should be showing now (looped).
    fn current_index(&self) -> usize {
        if self.total_ms == 0 {
            return 0;
        }
        let elapsed = (self.start.elapsed().as_millis() as u32) % self.total_ms;
        let mut acc = 0;
        for (i, f) in self.frames.iter().enumerate() {
            acc += f.delay_ms;
            if elapsed < acc {
                return i;
            }
        }
        self.frames.len() - 1
    }
}

/// Draw `frame` centered and integer-scaled into a window-sized `softbuffer` buffer
/// (`0x00RRGGBB` per pixel), over a transparency checkerboard (or transparent in float
/// mode). Free function so it can borrow the frame and the surface buffer independently.
fn draw_into(
    buf: &mut [u32],
    win_w: u32,
    win_h: u32,
    frame: &CompiledFrame,
    char_w: u32,
    char_h: u32,
    float: bool,
) {
    let scale = (win_w / char_w).min(win_h / char_h).max(1);
    let dw = char_w * scale;
    let dh = char_h * scale;
    let ox = win_w.saturating_sub(dw) / 2;
    let oy = win_h.saturating_sub(dh) / 2;

    for y in 0..win_h {
        for x in 0..win_w {
            let mut pixel = if float {
                0x0000_0000 // transparent where the platform supports it
            } else if ((x / CHECKER) + (y / CHECKER)).is_multiple_of(2) {
                0x00C8_C8C8
            } else {
                0x0090_9090
            };

            if x >= ox && x < ox + dw && y >= oy && y < oy + dh {
                let cx = (x - ox) / scale;
                let cy = (y - oy) / scale;
                let p = ((cy * char_w + cx) * 4) as usize;
                if frame.pixels[p + 3] != 0 {
                    let (r, g, b) = (frame.pixels[p], frame.pixels[p + 1], frame.pixels[p + 2]);
                    pixel = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
                }
            }
            buf[(y * win_w + x) as usize] = pixel;
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(LogicalSize::new(self.char_w * SCALE, self.char_h * SCALE));
        if self.float {
            attrs = attrs
                .with_decorations(false)
                .with_transparent(true)
                .with_window_level(WindowLevel::AlwaysOnTop);
        }

        let window = Rc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");

        window.request_redraw();
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
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
            WindowEvent::RedrawRequested => {
                let Some(window) = self.window.clone() else {
                    return;
                };
                let size = window.inner_size();
                let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                else {
                    return;
                };
                let idx = self.current_index();
                let Some(surface) = self.surface.as_mut() else {
                    return;
                };
                surface.resize(w, h).expect("resize");
                let mut buffer = surface.buffer_mut().expect("buffer");
                draw_into(
                    &mut buffer,
                    w.get(),
                    h.get(),
                    &self.frames[idx],
                    self.char_w,
                    self.char_h,
                    self.float,
                );
                buffer.present().expect("present");
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // ~60 fps; the frame shown is chosen by real elapsed time, so this only controls
        // redraw cadence, not animation speed.
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
    // --dry-run: load, sequence and composite everything, then exit without a window
    // (useful for headless checks).
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();

    let path = match positional.first() {
        Some(p) => (*p).clone(),
        None => {
            eprintln!("usage: crustagent-render <file.acs> [Animation] [--float]");
            std::process::exit(2);
        }
    };
    let chr = AcsFile::open(&path).unwrap_or_else(|e| {
        eprintln!("parse {path}: {e}");
        std::process::exit(1);
    });

    // Pick the animation: the given name, else "Greet", else the first gesture.
    let character = Character::new(&chr);
    let anim_name = positional
        .get(1)
        .map(|s| s.to_string())
        .or_else(|| character.animation("Greet").map(|_| "Greet".to_string()))
        .or_else(|| chr.gesture_names.first().cloned())
        .unwrap_or_default();

    let segments = character.full_gesture(&anim_name);
    if segments.is_empty() {
        eprintln!("no animation {anim_name:?}. Available:");
        for n in &chr.gesture_names {
            eprintln!("  {n}");
        }
        std::process::exit(1);
    }

    // Composite every timeline frame once.
    let mut rng = SplitMix64::new(0);
    let mut frames = Vec::new();
    for anim in &segments {
        let seq = sequence_animation(anim, &mut rng);
        for e in &seq.frames {
            let frame = &anim.frames[e.frame];
            let img = chr.composite_frame(frame, None).unwrap_or_else(|err| {
                eprintln!("composite: {err}");
                std::process::exit(1);
            });
            frames.push(CompiledFrame {
                pixels: img.pixels,
                delay_ms: (e.duration_cs as u32 * 10).max(1),
            });
        }
    }
    if frames.is_empty() {
        eprintln!("{anim_name} produced no visible frames");
        std::process::exit(1);
    }
    let total_ms = frames.iter().map(|f| f.delay_ms).sum();

    let name = chr.default_name().map(|n| n.name.clone()).unwrap_or_default();
    let parts: Vec<&str> = segments.iter().map(|a| a.name.as_str()).collect();
    println!(
        "playing {name} / {} ({} frames, {} ms){}",
        parts.join(" + "),
        frames.len(),
        total_ms,
        if float { "  [float]" } else { "" }
    );
    if dry_run {
        println!("dry run: {}x{} character, composited OK", chr.header.image_size.0, chr.header.image_size.1);
        return;
    }
    println!("press Esc or Q to quit");

    let mut app = App {
        title: format!("crustagent — {name}"),
        float,
        char_w: chr.header.image_size.0 as u32,
        char_h: chr.header.image_size.1 as u32,
        frames,
        total_ms,
        start: Instant::now(),
        window: None,
        context: None,
        surface: None,
    };

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app).expect("run");
}
