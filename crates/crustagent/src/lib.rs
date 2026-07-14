//! # crustagent
//!
//! Embed a classic Microsoft Agent character in a Rust app. [`Agent`] owns a parsed
//! character and a small **serial action queue**: you enqueue high-level requests
//! ([`Agent::show`], [`play`](Agent::play), [`speak`](Agent::speak),
//! [`move_to`](Agent::move_to), [`gesture_at`](Agent::gesture_at), [`hide`](Agent::hide)),
//! call [`update`](Agent::update) with elapsed time each tick, and read back what to draw
//! ([`composite_current`](Agent::composite_current), [`balloon`](Agent::balloon),
//! [`position`](Agent::position)). When the queue drains and the character is visible it
//! auto-idles.
//!
//! It is windowing/audio-agnostic — a host (e.g. `crustagent-render`) supplies time and a
//! surface. This is the layer that makes the goal — *using `.acs` characters in modern
//! apps* — a few lines:
//!
//! ```no_run
//! use crustagent::Agent;
//! let mut agent = Agent::load("Merlin.acs")?;
//! agent.show();
//! agent.speak("Hello there!");
//! agent.play("Wave");
//! loop {
//!     agent.update(16); // ms since last tick
//!     if let Some(_frame) = agent.composite_current() { /* blit it */ }
//!     # break;
//! }
//! # Ok::<(), crustagent_format::Error>(())
//! ```

use std::collections::VecDeque;

use crustagent_core::{
    sequence_animation, wrap_words, BalloonLayout, Character, Direction, IdleDirector, MoveTo,
    SplitMix64,
};
use crustagent_format::{AcsFile, MouthOverlay, Rgba};

pub use crustagent_format::{self as format, AcsFile as CharacterFile};
pub use crustagent_tts::{self, default_engine, TimedTts, TtsEngine, VoiceEvent};

/// A high-level request enqueued on the [`Agent`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Show,
    Hide,
    /// Play a named gesture (its full base + `…Continued` + `…Return`).
    Play(String),
    /// Show a balloon and pace the words (no audio yet).
    Speak(String),
    /// Walk to a screen point at `speed` pixels/second.
    MoveTo { x: i32, y: i32, speed: u32 },
    /// Point toward a screen point.
    GestureAt { x: i32, y: i32 },
    /// Hold for a number of milliseconds.
    Wait(u32),
}

/// What the balloon currently shows (already wrapped to the visible words).
#[derive(Clone, Debug)]
pub struct BalloonView {
    pub layout: BalloonLayout,
    /// Total words in the phrase (for progress).
    pub total_words: usize,
    /// Words revealed so far.
    pub shown_words: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Activity {
    Hidden,
    Idle,
    Gesture,
    Hiding,
    Move,
    Speak,
    Wait,
}

struct TrackFrame {
    anim: usize,
    frame: usize,
    dur_ms: u32,
}

/// An embedded, drivable character.
pub struct Agent {
    file: AcsFile,
    rng: SplitMix64,
    idle: IdleDirector,

    visible: bool,
    position: (i32, i32),
    per_line: usize,

    queue: VecDeque<Request>,
    activity: Activity,

    // current animation "track"
    track: Vec<TrackFrame>,
    track_total_ms: u32,
    track_loops: bool,
    track_elapsed_ms: u32,

    // move / speak state
    movement: MoveTo,
    tts: Box<dyn TtsEngine>,
    speak_words: Vec<String>,
    speak_shown: usize,
    speak_mouth: Option<MouthOverlay>,
}

impl Agent {
    /// Load a character from an `.acs` file.
    pub fn load(path: impl AsRef<std::path::Path>) -> crustagent_format::Result<Agent> {
        Ok(Agent::from_file(AcsFile::open(path)?))
    }

    /// Build an agent from an already-parsed character.
    pub fn from_file(file: AcsFile) -> Agent {
        let idle = IdleDirector::new(&Character::new(&file));
        let per_line = file
            .balloon
            .as_ref()
            .map(|b| b.per_line as usize)
            .filter(|&n| n > 0)
            .unwrap_or(32);
        Agent {
            file,
            rng: SplitMix64::new(0),
            idle,
            visible: false,
            position: (0, 0),
            per_line,
            queue: VecDeque::new(),
            activity: Activity::Hidden,
            track: Vec::new(),
            track_total_ms: 1,
            track_loops: false,
            track_elapsed_ms: 0,
            movement: MoveTo::new((0, 0), (0, 0), 0),
            tts: Box::new(TimedTts::new()),
            speak_words: Vec::new(),
            speak_shown: 0,
            speak_mouth: None,
        }
    }

    /// Swap the text-to-speech engine (default is the silent [`TimedTts`]). Use
    /// [`default_engine`] for real audio where a backend exists.
    pub fn set_tts(&mut self, engine: Box<dyn TtsEngine>) {
        self.tts = engine;
    }

    /// The parsed character file.
    pub fn file(&self) -> &AcsFile {
        &self.file
    }

    /// Character frame size in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.file.header.image_size.0 as u32, self.file.header.image_size.1 as u32)
    }

    /// Whether the character is currently shown.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Top-left screen position.
    pub fn position(&self) -> (i32, i32) {
        self.position
    }

    /// Set the top-left screen position directly (e.g. after a user drag).
    pub fn set_position(&mut self, x: i32, y: i32) {
        self.position = (x, y);
    }

    // -- enqueue -------------------------------------------------------------

    /// Enqueue a request.
    pub fn request(&mut self, req: Request) {
        self.queue.push_back(req);
    }
    pub fn show(&mut self) {
        self.request(Request::Show);
    }
    pub fn hide(&mut self) {
        self.request(Request::Hide);
    }
    pub fn play(&mut self, animation: impl Into<String>) {
        self.request(Request::Play(animation.into()));
    }
    pub fn speak(&mut self, text: impl Into<String>) {
        self.request(Request::Speak(text.into()));
    }
    pub fn move_to(&mut self, x: i32, y: i32, speed: u32) {
        self.request(Request::MoveTo { x, y, speed });
    }
    pub fn gesture_at(&mut self, x: i32, y: i32) {
        self.request(Request::GestureAt { x, y });
    }
    pub fn wait(&mut self, ms: u32) {
        self.request(Request::Wait(ms));
    }
    /// Clear the queue (does not interrupt the current activity's frame).
    pub fn stop(&mut self) {
        self.queue.clear();
    }

    // -- tick ----------------------------------------------------------------

    /// Advance by `dt_ms` milliseconds.
    pub fn update(&mut self, dt_ms: u32) {
        // Idle/hidden yields immediately to any queued request.
        if !self.queue.is_empty() && matches!(self.activity, Activity::Idle | Activity::Hidden) {
            self.next();
        }

        self.track_elapsed_ms = self.track_elapsed_ms.saturating_add(dt_ms);

        match self.activity {
            Activity::Hidden => {}
            Activity::Move => {
                self.movement.advance(dt_ms);
                self.position = self.movement.position();
                if self.movement.is_done() {
                    self.next();
                }
            }
            Activity::Speak => {
                for event in self.tts.poll(dt_ms) {
                    match event {
                        VoiceEvent::WordStarted(i) => self.speak_shown = i + 1,
                        VoiceEvent::Mouth(m) => self.speak_mouth = Some(m),
                        _ => {}
                    }
                }
                if !self.tts.is_speaking() {
                    self.speak_mouth = None;
                    self.next();
                }
            }
            Activity::Wait => {
                if self.track_elapsed_ms >= self.track_total_ms {
                    self.next();
                }
            }
            Activity::Idle | Activity::Gesture | Activity::Hiding => {
                if self.track_finished() {
                    if self.activity == Activity::Hiding {
                        self.visible = false;
                    }
                    self.next();
                }
            }
        }
    }

    fn track_finished(&self) -> bool {
        !self.track_loops && self.track_elapsed_ms >= self.track_total_ms
    }

    fn next(&mut self) {
        if let Some(req) = self.queue.pop_front() {
            self.start(req);
        } else if self.visible {
            self.start_idle();
        } else {
            self.activity = Activity::Hidden;
        }
    }

    fn start(&mut self, req: Request) {
        match req {
            Request::Show => {
                self.visible = true;
                self.start_state("SHOWING", false, Activity::Gesture);
            }
            Request::Hide => {
                self.start_state("HIDING", false, Activity::Hiding);
            }
            Request::Play(name) => {
                let indices = self.gesture_indices(&name);
                self.build_track(&indices, false);
                self.activity = Activity::Gesture;
            }
            Request::GestureAt { x, y } => {
                let dir = self.direction_to(x, y);
                self.start_state(dir.gesture_state(), false, Activity::Gesture);
            }
            Request::MoveTo { x, y, speed } => {
                self.movement = MoveTo::new(self.position, (x, y), speed);
                let dir = self.movement.direction();
                self.start_state(dir.move_state(), true, Activity::Move);
            }
            Request::Speak(text) => {
                let parsed = crustagent_core::parse_speech(&text);
                let spoken = parsed.spoken_text();
                self.speak_words = parsed.display_words;
                self.speak_shown = 0;
                self.speak_mouth = None;
                self.tts.speak(&spoken, self.speak_words.len());
                self.start_state("SPEAKING", true, Activity::Speak);
            }
            Request::Wait(ms) => {
                // freeze the current frame for `ms`
                self.track_loops = false;
                self.track_total_ms = ms.max(1);
                self.track_elapsed_ms = 0;
                self.activity = Activity::Wait;
            }
        }
    }

    fn start_idle(&mut self) {
        let name = {
            let ch = Character::new(&self.file);
            self.idle.next_idle(&ch, &mut self.rng)
        };
        match name.and_then(|n| self.anim_index(&n)) {
            Some(idx) => self.build_track(&[idx], false),
            None => self.build_rest_track(),
        }
        self.activity = Activity::Idle;
    }

    /// Play the first existing animation of a state; fall back to rest.
    fn start_state(&mut self, state: &str, loops: bool, activity: Activity) {
        let idx = {
            let ch = Character::new(&self.file);
            ch.state_animations(state)
                .into_iter()
                .flatten()
                .find_map(|n| self.anim_index(n))
        };
        match idx {
            Some(i) => self.build_track(&[i], loops),
            None => {
                self.build_rest_track();
                self.track_loops = loops;
            }
        }
        self.activity = activity;
    }

    fn anim_index(&self, name: &str) -> Option<usize> {
        self.file
            .gesture_names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
    }

    fn gesture_indices(&self, name: &str) -> Vec<usize> {
        let ch = Character::new(&self.file);
        ch.full_gesture(name)
            .iter()
            .filter_map(|a| self.anim_index(&a.name))
            .collect()
    }

    fn direction_to(&self, x: i32, y: i32) -> Direction {
        let (w, h) = self.size();
        let cx = self.position.0 + w as i32 / 2;
        let cy = self.position.1 + h as i32 / 2;
        Direction::toward(x - cx, y - cy)
    }

    fn build_track(&mut self, indices: &[usize], loops: bool) {
        let mut track = Vec::new();
        for &idx in indices {
            if let Some(anim) = self.file.animations.get(idx) {
                let seq = sequence_animation(anim, &mut self.rng);
                for e in &seq.frames {
                    track.push(TrackFrame {
                        anim: idx,
                        frame: e.frame,
                        dur_ms: (e.duration_cs as u32 * 10).max(1),
                    });
                }
            }
        }
        if track.is_empty() {
            self.build_rest_track();
            return;
        }
        self.track_total_ms = track.iter().map(|f| f.dur_ms).sum::<u32>().max(1);
        self.track = track;
        self.track_loops = loops;
        self.track_elapsed_ms = 0;
    }

    fn build_rest_track(&mut self) {
        // A single static frame from RestPose (or animation 0), held briefly.
        let idx = self
            .anim_index("RestPose")
            .or(if self.file.animations.is_empty() { None } else { Some(0) });
        self.track = idx
            .map(|anim| {
                vec![TrackFrame {
                    anim,
                    frame: 0,
                    dur_ms: 1000,
                }]
            })
            .unwrap_or_default();
        self.track_total_ms = 1000;
        self.track_loops = false;
        self.track_elapsed_ms = 0;
    }

    // -- render read-back ----------------------------------------------------

    fn track_frame(&self) -> Option<&TrackFrame> {
        if self.track.is_empty() {
            return None;
        }
        let t = if self.track_loops {
            self.track_elapsed_ms % self.track_total_ms
        } else {
            self.track_elapsed_ms.min(self.track_total_ms.saturating_sub(1))
        };
        let mut acc = 0;
        for f in &self.track {
            acc += f.dur_ms;
            if t < acc {
                return Some(f);
            }
        }
        self.track.last()
    }

    /// The mouth overlay to composite now — driven by the TTS engine's viseme/mouth
    /// events while speaking (see [`TtsEngine`]).
    fn current_mouth(&self) -> Option<MouthOverlay> {
        if self.activity == Activity::Speak {
            self.speak_mouth
        } else {
            None
        }
    }

    /// Composite the frame that should be on screen now, or `None` when hidden.
    pub fn composite_current(&self) -> Option<Rgba> {
        if !self.visible {
            return None;
        }
        let tf = self.track_frame()?;
        let anim = self.file.animations.get(tf.anim)?;
        let frame = anim.frames.get(tf.frame)?;
        self.file.composite_frame(frame, self.current_mouth()).ok()
    }

    /// The balloon to draw now, or `None` when not speaking.
    pub fn balloon(&self) -> Option<BalloonView> {
        if self.activity != Activity::Speak || self.speak_words.is_empty() {
            return None;
        }
        let total = self.speak_words.len();
        let shown = self.speak_shown.clamp(1, total);
        let layout = wrap_words(&self.speak_words[..shown], self.per_line);
        Some(BalloonView {
            layout,
            total_words: total,
            shown_words: shown,
        })
    }
}
