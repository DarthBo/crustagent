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
    parse_speech, sequence_animation, sequence_exit, wrap_last_rows, wrap_words, BalloonLayout,
    Character, Direction, IdleDirector, MoveTo, SplitMix64,
};
use crustagent_format::{char_style, AcsFile, ReturnKind};

/// How long an auto-hiding balloon lingers, fully revealed, before disappearing (ms).
const AUTO_HIDE_MS: u32 = 3000;
/// Per-word reveal pacing for a silent `Think` balloon (ms).
const THINK_PACE_MS: u32 = 300;

pub use crustagent_format::{self as format, AcsFile as CharacterFile, MouthOverlay, Rgba};
pub use crustagent_tts::{self, default_engine, TimedTts, TtsEngine, VoiceEvent};

/// A high-level request enqueued on the [`Agent`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Make the character visible. `fast` skips the SHOWING animation (appear instantly) —
    /// use it when the entrance is handled by a following animation that starts empty.
    Show { fast: bool },
    /// Hide the character. `fast` skips the HIDING animation (vanish instantly) — use it
    /// when the exit was handled by a preceding animation that ends empty.
    Hide { fast: bool },
    /// Play a named gesture (its full base + `…Continued` + `…Return`).
    Play(String),
    /// Play a named gesture on a loop until [`Agent::stop`] or the next queued request —
    /// for holding a pose or sustaining a gesture rather than playing it once.
    PlayLoop(String),
    /// Speak: show a balloon, pace the words, and drive the TTS engine + mouth.
    Speak(String),
    /// Think: show a thought balloon (no audio), pacing the words silently.
    Think(String),
    /// Walk to a screen point at `speed` pixels/second.
    MoveTo { x: i32, y: i32, speed: u32 },
    /// Point toward a screen point.
    GestureAt { x: i32, y: i32 },
    /// Hold for a number of milliseconds.
    Wait(u32),
}

/// A handle to an enqueued request, returned by the request methods. Pair it with
/// [`Event::RequestStarted`]/[`Event::RequestCompleted`] to track a specific action.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct ReqId(pub u64);

/// A pointer button, for the input events a host reports back to the agent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// A speak vs. think balloon (the tail shape differs).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BalloonKind {
    /// Spoken — pointed triangular tail.
    Speak,
    /// Thought — trail of shrinking bubbles.
    Think,
}

/// Something the agent reports to its host. Drain with [`Agent::drain_events`] each tick.
///
/// The lifecycle variants are raised by the agent itself; the input variants
/// ([`Clicked`](Event::Clicked), [`DragStarted`](Event::DragStarted), …) are ones the host
/// feeds back in (via [`Agent::report_click`] etc.) so an app can consume a single event
/// stream.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// A queued request began.
    RequestStarted(ReqId),
    /// A queued request finished (completed or was cut short by [`Agent::stop`]).
    RequestCompleted(ReqId),
    /// The character became visible.
    Shown,
    /// The character became hidden.
    Hidden,
    /// Auto-idling began (queue drained, character visible).
    IdleStarted,
    /// Auto-idling ended (a request preempted it).
    IdleEnded,
    /// A balloon appeared.
    BalloonShown,
    /// A balloon disappeared.
    BalloonHidden,
    /// Speech (audio + reveal) began.
    SpeechStarted,
    /// Speech finished.
    SpeechEnded,
    /// A `\Mrk=N` bookmark was reached during speech.
    Bookmark(i64),
    /// The character moved (during a `MoveTo` or a host-reported drag).
    Moved { x: i32, y: i32 },
    /// The host reported a click on the character.
    Clicked { button: MouseButton, x: i32, y: i32 },
    /// The host reported a double-click.
    DoubleClicked { button: MouseButton, x: i32, y: i32 },
    /// The host reported the start of a drag.
    DragStarted,
    /// The host reported the end of a drag.
    DragCompleted,
}

/// The resolved word-balloon styling for a character (from its file's balloon block and
/// style flags, with sensible fallbacks).
#[derive(Clone, Copy, Debug)]
pub struct BalloonStyle {
    /// Text color (RGB).
    pub fg: (u8, u8, u8),
    /// Background color (RGB).
    pub bg: (u8, u8, u8),
    /// Border color (RGB).
    pub border: (u8, u8, u8),
    /// Max lines in a fixed-size balloon (the box scrolls past this).
    pub lines: usize,
    /// Characters per line (wrap width).
    pub per_line: usize,
    /// Grow the balloon to fit the whole phrase instead of scrolling a fixed box.
    pub size_to_text: bool,
    /// Auto-hide the balloon a few seconds after speech ends.
    pub auto_hide: bool,
    /// Reveal words progressively (vs. all at once).
    pub auto_pace: bool,
}

/// What the balloon currently shows (already wrapped to the visible words).
#[derive(Clone, Debug)]
pub struct BalloonView {
    /// Layout of the words revealed so far (what to draw now).
    pub layout: BalloonLayout,
    /// Layout of the *entire* phrase — constant during a phrase, so a host can size a
    /// balloon window once instead of resizing as words appear.
    pub full: BalloonLayout,
    /// Total words in the phrase (for progress).
    pub total_words: usize,
    /// Words revealed so far.
    pub shown_words: usize,
    /// Speak or think (chooses the tail shape).
    pub kind: BalloonKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Activity {
    Hidden,
    /// Holding the rest pose between idle animations (standing still).
    IdleRest,
    /// Playing an idle animation.
    Idle,
    Gesture,
    Hiding,
    Move,
    Speak,
    Think,
    Wait,
}

struct TrackFrame {
    anim: usize,
    frame: usize,
    dur_ms: u32,
}

/// Plays a character's embedded sound effects (raw WAV bytes from the file). Implemented
/// by a host audio backend (e.g. `crustagent-audio`'s rodio sink); the default is silent.
pub trait AudioSink {
    /// Play a standalone WAV clip (fire-and-forget; clips may overlap).
    fn play(&mut self, wav: &[u8]);
    /// Stop any playing clips.
    fn stop(&mut self) {}
}

/// A no-op [`AudioSink`] — sound effects are silently dropped.
pub struct NullSink;
impl AudioSink for NullSink {
    fn play(&mut self, _wav: &[u8]) {}
}

/// Resolve the character's balloon styling from its file (balloon block + style flags),
/// falling back to Microsoft Agent's defaults (2×32, info colors, auto-pace + auto-hide).
fn resolve_balloon_style(file: &AcsFile) -> BalloonStyle {
    let flags = file.header.style;
    let b = file.balloon.as_ref();
    let rgb = |c: &crustagent_format::Color| (c.r, c.g, c.b);
    BalloonStyle {
        fg: b.map(|b| rgb(&b.fg_color)).unwrap_or((0x00, 0x00, 0x00)),
        bg: b.map(|b| rgb(&b.bg_color)).unwrap_or((0xFF, 0xFF, 0xE1)),
        border: b.map(|b| rgb(&b.border_color)).unwrap_or((0x40, 0x40, 0x40)),
        lines: b.map(|b| b.lines as usize).filter(|&n| n > 0).unwrap_or(2),
        per_line: b.map(|b| b.per_line as usize).filter(|&n| n > 0).unwrap_or(32),
        size_to_text: flags & char_style::SIZE_TO_TEXT != 0,
        auto_hide: flags & char_style::NO_AUTO_HIDE == 0,
        auto_pace: flags & char_style::NO_AUTO_PACE == 0,
    }
}

/// An embedded, drivable character.
pub struct Agent {
    file: AcsFile,
    rng: SplitMix64,
    idle: IdleDirector,

    visible: bool,
    position: (i32, i32),
    style: BalloonStyle,

    queue: VecDeque<(ReqId, Request)>,
    activity: Activity,

    // events / request tracking
    events: Vec<Event>,
    next_id: u64,
    current_req: Option<ReqId>,
    idle_active: bool,
    paused: bool,
    paused_word: usize,

    // current animation "track"
    track: Vec<TrackFrame>,
    track_total_ms: u32,
    track_loops: bool,
    /// When looping, wrap back to this ms offset (the animation's loop start) rather than
    /// to 0 — so a one-shot intro (e.g. a walk's takeoff) plays once and only the hold
    /// repeats.
    track_loop_start_ms: u32,
    track_elapsed_ms: u32,

    // move / speak state
    movement: MoveTo,
    /// The moving animation currently playing, so its exit (the landing) can play on
    /// arrival. `None` unless a move is in flight.
    move_anim: Option<usize>,
    /// Whether the position tween is running yet. A **walk** (looping animation) glides
    /// from the start, animating the whole way. A **flight** (finite animation, e.g. Merlin
    /// putting on his glasses) plays that animation in place first — he can't prepare while
    /// flying — and only then zips to the destination.
    move_gliding: bool,
    /// This move is a **teleport**: the character has no `MOVING*` animation, so instead of
    /// walking/flying it vanishes with `HIDING`, jumps to the destination, and reappears with
    /// `SHOWING`. `move_teleport_appearing` is the phase (false = vanishing, true = appearing).
    move_teleport: bool,
    move_teleport_appearing: bool,
    tts: Box<dyn TtsEngine>,
    think_timer: TimedTts,
    speak_words: Vec<String>,
    speak_shown: usize,
    speak_mouth: Option<MouthOverlay>,
    pending_bookmarks: Vec<(i64, usize)>,

    // balloon presentation (decoupled from the queue so it can linger/auto-hide)
    balloon_kind: Option<BalloonKind>,
    balloon_done: bool,
    balloon_hold_ms: u32,
    /// A `say_over`/`think_over` balloon revealing *in parallel* with the current animation
    /// (rather than occupying the queue as a `Speak`/`Think` activity would).
    speaking_overlay: bool,
    /// Whether the active overlay balloon is a silent think (vs a spoken say).
    overlay_think: bool,

    // sound effects
    audio: Box<dyn AudioSink>,
    last_track_index: Option<usize>,
}

impl Agent {
    /// Load a character from an `.acs` file.
    pub fn load(path: impl AsRef<std::path::Path>) -> crustagent_format::Result<Agent> {
        Ok(Agent::from_file(AcsFile::open(path)?))
    }

    /// Build an agent from an already-parsed character.
    pub fn from_file(file: AcsFile) -> Agent {
        let idle = IdleDirector::new(&Character::new(&file));
        let style = resolve_balloon_style(&file);
        Agent {
            file,
            rng: SplitMix64::new(0),
            idle,
            visible: false,
            position: (0, 0),
            style,
            queue: VecDeque::new(),
            activity: Activity::Hidden,
            events: Vec::new(),
            next_id: 1,
            current_req: None,
            idle_active: false,
            paused: false,
            paused_word: 0,
            track: Vec::new(),
            track_total_ms: 1,
            track_loops: false,
            track_loop_start_ms: 0,
            track_elapsed_ms: 0,
            movement: MoveTo::new((0, 0), (0, 0), 0),
            move_anim: None,
            move_gliding: false,
            move_teleport: false,
            move_teleport_appearing: false,
            tts: Box::new(TimedTts::new()),
            think_timer: TimedTts::new().with_pace(THINK_PACE_MS),
            speak_words: Vec::new(),
            speak_shown: 0,
            speak_mouth: None,
            pending_bookmarks: Vec::new(),
            balloon_kind: None,
            balloon_done: false,
            balloon_hold_ms: 0,
            speaking_overlay: false,
            overlay_think: false,
            audio: Box::new(NullSink),
            last_track_index: None,
        }
    }

    /// Swap the text-to-speech engine (default is the silent [`TimedTts`]). Use
    /// [`default_engine`] for real audio where a backend exists.
    pub fn set_tts(&mut self, engine: Box<dyn TtsEngine>) {
        self.tts = engine;
    }

    /// Set the sound-effect audio sink (default is silent [`NullSink`]).
    pub fn set_audio_sink(&mut self, sink: Box<dyn AudioSink>) {
        self.audio = sink;
    }

    /// The parsed character file.
    pub fn file(&self) -> &AcsFile {
        &self.file
    }

    /// Character frame size in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.file.header.image_size.0 as u32, self.file.header.image_size.1 as u32)
    }

    /// Whether the character is currently auto-idling (an idle animation or the rest
    /// pause between them).
    pub fn is_idle(&self) -> bool {
        matches!(self.activity, Activity::Idle | Activity::IdleRest)
    }

    /// Whether a `Play`ed gesture is currently running.
    pub fn is_gesturing(&self) -> bool {
        matches!(self.activity, Activity::Gesture)
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
        if self.position != (x, y) {
            self.position = (x, y);
            self.emit(Event::Moved { x, y });
        }
    }

    /// The resolved word-balloon styling (colors, size, flags) for this character.
    pub fn balloon_style(&self) -> BalloonStyle {
        self.style
    }

    // -- events --------------------------------------------------------------

    /// Take all events accumulated since the last drain.
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Pop a single event, oldest first.
    pub fn poll_event(&mut self) -> Option<Event> {
        if self.events.is_empty() {
            None
        } else {
            Some(self.events.remove(0))
        }
    }

    /// Report a click on the character (raised back as [`Event::Clicked`]).
    pub fn report_click(&mut self, button: MouseButton, x: i32, y: i32) {
        self.emit(Event::Clicked { button, x, y });
    }
    /// Report a double-click on the character.
    pub fn report_double_click(&mut self, button: MouseButton, x: i32, y: i32) {
        self.emit(Event::DoubleClicked { button, x, y });
    }
    /// Report the start of a user drag.
    pub fn report_drag_start(&mut self) {
        self.emit(Event::DragStarted);
    }
    /// Report the end of a user drag.
    pub fn report_drag_complete(&mut self) {
        self.emit(Event::DragCompleted);
    }

    fn emit(&mut self, e: Event) {
        self.events.push(e);
    }

    fn set_visible(&mut self, v: bool) {
        if v != self.visible {
            self.visible = v;
            self.emit(if v { Event::Shown } else { Event::Hidden });
            if !v {
                self.clear_balloon();
            }
        }
    }

    // -- enqueue -------------------------------------------------------------

    /// Enqueue a request, returning a handle you can match against request events.
    pub fn request(&mut self, req: Request) -> ReqId {
        let id = ReqId(self.next_id);
        self.next_id += 1;
        self.queue.push_back((id, req));
        id
    }
    /// Show the character, playing its SHOWING animation.
    pub fn show(&mut self) -> ReqId {
        self.request(Request::Show { fast: false })
    }
    /// Show the character instantly (no SHOWING animation).
    pub fn show_fast(&mut self) -> ReqId {
        self.request(Request::Show { fast: true })
    }
    /// Hide the character, playing its HIDING animation.
    pub fn hide(&mut self) -> ReqId {
        self.request(Request::Hide { fast: false })
    }
    /// Hide the character instantly (no HIDING animation).
    pub fn hide_fast(&mut self) -> ReqId {
        self.request(Request::Hide { fast: true })
    }
    pub fn play(&mut self, animation: impl Into<String>) -> ReqId {
        self.request(Request::Play(animation.into()))
    }
    /// Queue a gesture that **loops** until [`stop`](Agent::stop) or the next queued request
    /// preempts it — for holding a pose or sustaining a gesture, rather than the single
    /// one-shot of [`play`](Agent::play).
    pub fn play_looping(&mut self, animation: impl Into<String>) -> ReqId {
        self.request(Request::PlayLoop(animation.into()))
    }
    pub fn speak(&mut self, text: impl Into<String>) -> ReqId {
        self.request(Request::Speak(text.into()))
    }
    /// Show a speech balloon that reveals **over the current animation**, without taking a
    /// queue slot — so the character keeps whatever gesture is playing (e.g. a looping
    /// [`play_looping`](Agent::play_looping) pose) while it talks. Fire-and-forget: it
    /// replaces any current balloon and auto-hides per the character's style. Use plain
    /// [`speak`](Agent::speak) when you want the serial, mouth-driven `SPEAKING` behaviour.
    pub fn say_over(&mut self, text: impl Into<String>) {
        self.begin_overlay_speech(text.into(), false);
    }
    /// Like [`say_over`](Agent::say_over) but a silent thought balloon.
    pub fn think_over(&mut self, text: impl Into<String>) {
        self.begin_overlay_speech(text.into(), true);
    }

    /// Start a parallel balloon (see [`say_over`]/[`think_over`]): set up the balloon + a
    /// speech/think timer without queuing or changing the activity.
    fn begin_overlay_speech(&mut self, text: String, think: bool) {
        let parsed = parse_speech(&text);
        let spoken = parsed.spoken_text();
        self.speak_mouth = None;
        let kind = if think { BalloonKind::Think } else { BalloonKind::Speak };
        self.begin_balloon(kind, parsed.display_words, parsed.bookmark_at);
        let words = self.speak_words.len().max(1);
        if think {
            self.think_timer.speak("", words);
        } else {
            self.tts.speak(&spoken, words);
            self.emit(Event::SpeechStarted);
        }
        self.overlay_think = think;
        self.speaking_overlay = true;
    }
    /// Show a thought balloon (no audio), pacing the words silently.
    pub fn think(&mut self, text: impl Into<String>) -> ReqId {
        self.request(Request::Think(text.into()))
    }
    pub fn move_to(&mut self, x: i32, y: i32, speed: u32) -> ReqId {
        self.request(Request::MoveTo { x, y, speed })
    }
    pub fn gesture_at(&mut self, x: i32, y: i32) -> ReqId {
        self.request(Request::GestureAt { x, y })
    }
    pub fn wait(&mut self, ms: u32) -> ReqId {
        self.request(Request::Wait(ms))
    }
    /// Clear the queue (does not interrupt the current activity's frame). Any not-yet-run
    /// requests are reported as completed.
    pub fn stop(&mut self) {
        let cancelled: Vec<ReqId> = self.queue.drain(..).map(|(id, _)| id).collect();
        for id in cancelled {
            self.emit(Event::RequestCompleted(id));
        }
        // End a looping gesture too — otherwise it never finishes and would block the queue
        // forever. Mark it done so the next update advances to the queue (or to idle).
        if self.track_loops && self.activity == Activity::Gesture {
            self.track_loops = false;
            self.track_elapsed_ms = self.track_total_ms;
        }
    }

    // -- speech control ------------------------------------------------------

    /// Pause speech/animation, remembering the revealed-word position.
    pub fn pause(&mut self) {
        if !self.paused {
            self.paused = true;
            self.paused_word = self.speak_shown;
        }
    }
    /// Resume after [`pause`](Agent::pause).
    pub fn resume(&mut self) {
        self.paused = false;
    }
    /// Whether the agent is paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }
    /// Dismiss the balloon now (if any), raising [`Event::BalloonHidden`].
    pub fn hide_balloon(&mut self) {
        self.clear_balloon();
    }

    fn clear_balloon(&mut self) {
        self.speaking_overlay = false; // stop any parallel reveal
        if self.balloon_kind.take().is_some() {
            self.balloon_done = false;
            self.balloon_hold_ms = 0;
            self.emit(Event::BalloonHidden);
        }
    }

    // -- tick ----------------------------------------------------------------

    /// Advance by `dt_ms` milliseconds.
    pub fn update(&mut self, dt_ms: u32) {
        if self.paused {
            return; // frozen: animation, speech reveal, and the balloon all hold
        }

        // Idle/hidden — and a looping gesture, which otherwise never ends — yield
        // immediately to any queued request.
        if !self.queue.is_empty()
            && (matches!(
                self.activity,
                Activity::Idle | Activity::IdleRest | Activity::Hidden
            ) || (self.activity == Activity::Gesture && self.track_loops))
        {
            self.next();
        }

        self.track_elapsed_ms = self.track_elapsed_ms.saturating_add(dt_ms);

        match self.activity {
            Activity::Hidden => {}
            Activity::Move if self.move_teleport => self.advance_teleport(),
            Activity::Move => {
                // A flight plays its in-place preparation (the finite animation) first, then
                // zips; a walk glides from the start. Once gliding, the exit (landing) plays
                // on arrival.
                if !self.move_gliding && self.track_elapsed_ms >= self.track_total_ms {
                    self.move_gliding = true;
                }
                if self.move_gliding {
                    let before = self.position;
                    self.movement.advance(dt_ms);
                    self.position = self.movement.position();
                    if self.position != before {
                        let (x, y) = self.position;
                        self.emit(Event::Moved { x, y });
                    }
                    if self.movement.is_done() {
                        self.begin_landing();
                    }
                }
            }
            Activity::Speak => {
                if self.poll_speech(dt_ms, false) {
                    self.next();
                }
            }
            Activity::Think => {
                if self.poll_speech(dt_ms, true) {
                    self.next();
                }
            }
            Activity::Wait | Activity::IdleRest => {
                if self.track_elapsed_ms >= self.track_total_ms {
                    self.next();
                }
            }
            Activity::Idle | Activity::Gesture | Activity::Hiding => {
                if self.track_finished() {
                    if self.activity == Activity::Hiding {
                        self.set_visible(false);
                    }
                    self.next();
                }
            }
        }

        // A parallel `say_over`/`think_over` balloon reveals alongside the animation above
        // (never while a serial Speak/Think activity is already driving the reveal).
        if self.speaking_overlay
            && !matches!(self.activity, Activity::Speak | Activity::Think)
            && self.poll_speech(dt_ms, self.overlay_think)
        {
            self.speaking_overlay = false;
        }

        // Auto-hide the balloon a few seconds after its content finished revealing.
        self.tick_balloon(dt_ms);

        // Play the current frame's embedded sound effect on entry.
        if self.visible {
            self.fire_frame_sound();
        }
    }

    /// Poll the active speech/think engine, reveal words, and fire bookmarks. Returns `true`
    /// the tick speech finishes (the caller decides what to do next: a serial Speak/Think
    /// activity advances the queue; a parallel overlay just stops revealing).
    fn poll_speech(&mut self, dt_ms: u32, think: bool) -> bool {
        let events = if think {
            self.think_timer.poll(dt_ms)
        } else {
            self.tts.poll(dt_ms)
        };
        for event in events {
            match event {
                VoiceEvent::WordStarted(i) => {
                    if self.style.auto_pace {
                        self.speak_shown = i + 1;
                        self.fire_bookmarks();
                    }
                }
                VoiceEvent::Mouth(m) if !think => self.speak_mouth = Some(m),
                _ => {}
            }
        }
        let done = if think {
            !self.think_timer.is_speaking()
        } else {
            !self.tts.is_speaking()
        };
        if done {
            self.speak_mouth = None;
            self.speak_shown = self.speak_words.len();
            self.fire_bookmarks();
            self.balloon_done = true;
            self.balloon_hold_ms = if self.style.auto_hide { AUTO_HIDE_MS } else { u32::MAX };
            if !think {
                self.emit(Event::SpeechEnded);
            }
        }
        done
    }

    /// Raise any pending bookmarks whose word position the reveal has now passed.
    fn fire_bookmarks(&mut self) {
        while let Some(&(id, threshold)) = self.pending_bookmarks.first() {
            if threshold <= self.speak_shown {
                self.pending_bookmarks.remove(0);
                self.emit(Event::Bookmark(id));
            } else {
                break;
            }
        }
    }

    /// Count down an auto-hiding balloon and clear it when the linger expires.
    fn tick_balloon(&mut self, dt_ms: u32) {
        if self.balloon_kind.is_some() && self.balloon_done && self.balloon_hold_ms != u32::MAX {
            self.balloon_hold_ms = self.balloon_hold_ms.saturating_sub(dt_ms);
            if self.balloon_hold_ms == 0 {
                self.clear_balloon();
            }
        }
    }

    /// Set up the balloon for a new speak/think phrase.
    fn begin_balloon(&mut self, kind: BalloonKind, words: Vec<String>, bookmarks: Vec<(i64, usize)>) {
        self.clear_balloon(); // replace any lingering balloon (emits BalloonHidden)
        self.speak_words = words;
        self.speak_shown = if self.style.auto_pace {
            0
        } else {
            self.speak_words.len()
        };
        self.pending_bookmarks = bookmarks;
        self.pending_bookmarks.sort_by_key(|&(_, threshold)| threshold);
        self.balloon_kind = Some(kind);
        self.balloon_done = false;
        self.balloon_hold_ms = 0;
        self.emit(Event::BalloonShown);
    }

    fn track_finished(&self) -> bool {
        !self.track_loops && self.track_elapsed_ms >= self.track_total_ms
    }

    fn next(&mut self) {
        if let Some(id) = self.current_req.take() {
            self.emit(Event::RequestCompleted(id));
        }
        if let Some((id, req)) = self.queue.pop_front() {
            if self.idle_active {
                self.idle_active = false;
                self.emit(Event::IdleEnded);
            }
            self.current_req = Some(id);
            self.emit(Event::RequestStarted(id));
            self.start(req);
        } else if self.visible {
            if !self.idle_active {
                self.idle_active = true;
                self.emit(Event::IdleStarted);
            }
            // Alternate: rest a beat, then one idle animation, then rest again.
            if self.activity == Activity::IdleRest {
                self.start_idle_animation();
            } else {
                self.start_idle_rest();
            }
        } else {
            self.activity = Activity::Hidden;
        }
    }

    fn start(&mut self, req: Request) {
        match req {
            Request::Show { fast } => {
                self.set_visible(true);
                if fast {
                    // Appear instantly; proceed straight to the next request (e.g. a
                    // greeting that starts empty) or to idling — no in-between frame.
                    self.next();
                } else {
                    self.start_state("SHOWING", false, Activity::Gesture);
                }
            }
            Request::Hide { fast } => {
                if fast {
                    self.set_visible(false);
                    self.activity = Activity::Hidden;
                    self.next();
                } else {
                    self.start_state("HIDING", false, Activity::Hiding);
                }
            }
            Request::Play(name) => {
                let frames = self.build_gesture_frames(&name, false);
                self.install_track(frames, false);
                self.activity = Activity::Gesture;
            }
            Request::PlayLoop(name) => {
                // Forward walk only (no exit branch) so the cycle repeats cleanly; loops
                // until stop() or the next queued request preempts it.
                let indices: Vec<usize> = {
                    let ch = Character::new(&self.file);
                    ch.full_gesture(&name)
                        .iter()
                        .filter_map(|a| self.anim_index(&a.name))
                        .collect()
                };
                if let [idx] = indices[..] {
                    // Single animation: honor its own loop point (e.g. Merlin's Processing
                    // has a ~720ms intro, then loops) — play the intro once, then repeat only
                    // the looping body, instead of restarting the whole clip each cycle.
                    let (track, loop_start_ms) = {
                        let anim = &self.file.animations[idx];
                        let seq = sequence_animation(anim, &mut self.rng);
                        let track: Vec<TrackFrame> = seq
                            .frames
                            .iter()
                            .map(|e| TrackFrame { anim: idx, frame: e.frame, dur_ms: (e.duration_cs as u32 * 10).max(1) })
                            .collect();
                        (track, seq.loop_start_cs.map(|cs| cs * 10))
                    };
                    self.install_track(track, true);
                    self.track_loop_start_ms = loop_start_ms.unwrap_or(0);
                } else {
                    // Multi-part gesture: loop the whole concatenated forward track.
                    self.build_track(&indices, true);
                }
                self.activity = Activity::Gesture;
            }
            Request::GestureAt { x, y } => {
                let dir = self.direction_to(x, y);
                self.start_state(dir.gesture_state(), false, Activity::Gesture);
            }
            Request::MoveTo { x, y, speed } => {
                self.movement = MoveTo::new(self.position, (x, y), speed);
                let dir = self.movement.direction();
                self.start_move(dir.move_state());
            }
            Request::Speak(text) => {
                let parsed = parse_speech(&text);
                let spoken = parsed.spoken_text();
                self.speak_mouth = None;
                self.begin_balloon(BalloonKind::Speak, parsed.display_words, parsed.bookmark_at);
                self.tts.speak(&spoken, self.speak_words.len().max(1));
                self.emit(Event::SpeechStarted);
                self.start_state("SPEAKING", true, Activity::Speak);
            }
            Request::Think(text) => {
                let parsed = parse_speech(&text);
                self.speak_mouth = None;
                self.begin_balloon(BalloonKind::Think, parsed.display_words, parsed.bookmark_at);
                // Silent pacing; keep the current pose (a Think has no dedicated state).
                self.think_timer.speak("", self.speak_words.len().max(1));
                self.build_rest_track();
                self.track_loops = true;
                self.activity = Activity::Think;
            }
            Request::Wait(ms) => {
                // Freeze the current frame for `ms`. Reduce the track to just that frame —
                // otherwise the old (longer) track's timeline gets replayed from the start
                // over `ms`, so a wait after a gesture looks like the gesture playing again.
                let held = self
                    .track_frame()
                    .map(|f| TrackFrame { anim: f.anim, frame: f.frame, dur_ms: ms.max(1) });
                self.track = held.into_iter().collect();
                self.track_loops = false;
                self.track_loop_start_ms = 0;
                self.track_total_ms = ms.max(1);
                self.track_elapsed_ms = 0;
                self.last_track_index = None;
                self.activity = Activity::Wait;
            }
        }
    }

    /// Stand at rest for a randomized pause before the next idle animation.
    fn start_idle_rest(&mut self) {
        self.build_rest_track();
        self.track_total_ms = 1500 + (self.rng.next_u64() % 3000) as u32; // 1.5–4.5s
        self.track_elapsed_ms = 0;
        self.activity = Activity::IdleRest;
    }

    /// Play one idle animation (with its exit-return so it ends back at rest).
    fn start_idle_animation(&mut self) {
        let name = {
            let ch = Character::new(&self.file);
            self.idle.next_idle(&ch, &mut self.rng)
        };
        match name {
            Some(n) => {
                let frames = self.build_gesture_frames(&n, true);
                self.install_track(frames, false);
            }
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

    /// Start a `MOVING*` state for travel. Two shapes, distinguished by whether the move
    /// animation loops:
    ///
    /// - a **walk** (looping animation, e.g. the gnome's 2-frame cycle): glide from the
    ///   start, cycling the walk the whole way at the requested speed.
    /// - a **flight** (finite animation, e.g. Merlin putting on his glasses): play that
    ///   preparation *in place* first (he can't prepare mid-air), then **zip** to the
    ///   destination fast (capped duration), holding the final frame.
    ///
    /// The landing (the exit walk) plays on arrival in [`begin_landing`].
    fn start_move(&mut self, state: &str) {
        /// A flight's zip caps at this long, so a far trip just crosses faster rather than
        /// dragging out (the preparation animation, played in place, is separate).
        const ZIP_MAX_MS: u32 = 500;

        self.move_teleport = false;
        match self.state_anim_index(state) {
            Some(i) => {
                let (track, loop_start_ms) = {
                    let anim = &self.file.animations[i];
                    let seq = sequence_animation(anim, &mut self.rng);
                    let track: Vec<TrackFrame> = seq
                        .frames
                        .iter()
                        .map(|e| TrackFrame { anim: i, frame: e.frame, dur_ms: (e.duration_cs as u32 * 10).max(1) })
                        .collect();
                    (track, seq.loop_start_cs.map(|cs| cs * 10))
                };
                let walk = loop_start_ms.is_some();
                self.install_track(track, walk);
                self.track_loop_start_ms = loop_start_ms.unwrap_or(0);
                self.move_anim = Some(i);
                if walk {
                    self.move_gliding = true; // walk animates the whole way
                } else {
                    // Flight: prepare in place, then zip fast on arrival of the tween.
                    self.move_gliding = false;
                    let zip = self.movement.duration_ms().min(ZIP_MAX_MS);
                    self.movement.retime(zip);
                }
            }
            None => {
                // No walk/fly animation → teleport: vanish (HIDING), jump, reappear (SHOWING).
                self.move_teleport = true;
                self.move_teleport_appearing = false;
                self.move_gliding = false;
                self.install_phase_track("HIDING");
            }
        }
        self.activity = Activity::Move;
    }

    /// Resolve the first existing animation of a `state`, if any.
    fn state_anim_index(&self, state: &str) -> Option<usize> {
        let ch = Character::new(&self.file);
        ch.state_animations(state)
            .into_iter()
            .flatten()
            .find_map(|n| self.anim_index(n))
    }

    /// Install a state's animation as a one-shot track (for a teleport phase). If the state
    /// has no animation, install a zero-length rest track so the phase advances immediately.
    fn install_phase_track(&mut self, state: &str) {
        match self.state_anim_index(state) {
            Some(i) => self.build_track(&[i], false),
            None => {
                self.build_rest_track();
                self.track_total_ms = 1; // finish next tick — no animation for this phase
            }
        }
    }

    /// Drive a teleport (character with no `MOVING*` animation): play `HIDING` in place, then
    /// on completion jump to the destination and play `SHOWING`, then finish the move.
    fn advance_teleport(&mut self) {
        if self.track_elapsed_ms < self.track_total_ms {
            return; // the current phase's animation is still playing
        }
        if !self.move_teleport_appearing {
            // Vanished — jump to the destination and reappear there.
            self.move_teleport_appearing = true;
            self.position = self.movement.dest();
            let (x, y) = self.position;
            self.emit(Event::Moved { x, y });
            self.install_phase_track("SHOWING");
            self.move_teleport = true; // install_* cleared move_gliding but not this
        } else {
            // Reappeared — the move is complete.
            self.move_teleport = false;
            self.next();
        }
    }

    /// On arrival, play the moving animation's exit (the landing) once, then continue the
    /// queue. Falls back to advancing immediately if there's no exit branch.
    fn begin_landing(&mut self) {
        let Some(i) = self.move_anim.take() else {
            self.next();
            return;
        };
        let Some(cur) = self.track_frame().map(|f| f.frame) else {
            self.next();
            return;
        };
        let track: Vec<TrackFrame> = {
            let anim = &self.file.animations[i];
            let exit_from = anim.frames.get(cur).map(|f| f.exit_frame).unwrap_or(-1);
            if exit_from < 0 || exit_from as usize >= anim.frames.len() {
                Vec::new()
            } else {
                sequence_exit(anim, exit_from as usize)
                    .frames
                    .iter()
                    .map(|e| TrackFrame { anim: i, frame: e.frame, dur_ms: (e.duration_cs as u32 * 10).max(1) })
                    .collect()
            }
        };
        if track.is_empty() {
            self.next();
            return;
        }
        self.install_track(track, false);
        self.activity = Activity::Gesture; // a non-looping gesture: finishes → next()
    }

    fn anim_index(&self, name: &str) -> Option<usize> {
        self.file
            .gesture_names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
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
            self.append_forward(&mut track, idx);
        }
        self.install_track(track, loops);
    }

    /// Append an animation's forward (RNG) walk to `track`.
    fn append_forward(&mut self, track: &mut Vec<TrackFrame>, idx: usize) {
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

    /// Build the full track for a gesture: each part's forward walk, plus — when the part
    /// declares `ExitBranching` (returnType 1, e.g. Merlin's `Pleased`) or `force_return`
    /// is set — its *exit* walk, so it winds back to rest instead of freezing mid-motion
    /// and snapping away. `force_return` is used for idle animations: their forward walk
    /// often stops mid-loop, and following the exit frames returns them cleanly to rest.
    fn build_gesture_frames(&mut self, name: &str, force_return: bool) -> Vec<TrackFrame> {
        let indices: Vec<usize> = {
            let ch = Character::new(&self.file);
            ch.full_gesture(name)
                .iter()
                .filter_map(|a| self.anim_index(&a.name))
                .collect()
        };
        let mut track = Vec::new();
        for idx in indices {
            let before = track.len();
            self.append_forward(&mut track, idx);

            let anim = &self.file.animations[idx];
            if force_return || anim.return_kind == ReturnKind::ExitBranching {
                // Continue the exit branch from where the forward walk ended.
                if let Some(last) = track.get(before..).and_then(|s| s.last()) {
                    let exit_from = anim.frames[last.frame].exit_frame;
                    if exit_from >= 0 && (exit_from as usize) < anim.frames.len() {
                        let ex = sequence_exit(anim, exit_from as usize);
                        for e in &ex.frames {
                            track.push(TrackFrame {
                                anim: idx,
                                frame: e.frame,
                                dur_ms: (e.duration_cs as u32 * 10).max(1),
                            });
                        }
                    }
                }
            }
        }
        track
    }

    fn install_track(&mut self, track: Vec<TrackFrame>, loops: bool) {
        if track.is_empty() {
            self.build_rest_track();
            self.track_loops = loops;
            return;
        }
        self.track_total_ms = track.iter().map(|f| f.dur_ms).sum::<u32>().max(1);
        self.track = track;
        self.track_loops = loops;
        self.track_loop_start_ms = 0; // a move overrides this after installing
        self.move_anim = None;
        self.move_gliding = false;
        self.track_elapsed_ms = 0;
        self.last_track_index = None;
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
        self.last_track_index = None;
    }

    // -- render read-back ----------------------------------------------------

    /// Index into `track` of the entry active at the current time.
    fn current_track_index(&self) -> Option<usize> {
        if self.track.is_empty() {
            return None;
        }
        let t = if self.track_loops {
            if self.track_elapsed_ms < self.track_total_ms {
                self.track_elapsed_ms // first pass: play the intro (takeoff) once
            } else {
                // then repeat only [loop_start, total) — the hold/fly, not the intro.
                let ls = self.track_loop_start_ms.min(self.track_total_ms.saturating_sub(1));
                let loop_len = (self.track_total_ms - ls).max(1);
                ls + (self.track_elapsed_ms - ls) % loop_len
            }
        } else {
            self.track_elapsed_ms.min(self.track_total_ms.saturating_sub(1))
        };
        let mut acc = 0;
        for (i, f) in self.track.iter().enumerate() {
            acc += f.dur_ms;
            if t < acc {
                return Some(i);
            }
        }
        Some(self.track.len() - 1)
    }

    fn track_frame(&self) -> Option<&TrackFrame> {
        self.current_track_index().map(|i| &self.track[i])
    }

    /// Play the sound effect of the frame we just entered (once per frame entry).
    fn fire_frame_sound(&mut self) {
        let cur = self.current_track_index();
        if cur == self.last_track_index {
            return;
        }
        self.last_track_index = cur;
        let Some(i) = cur else { return };
        let (anim, frame) = {
            let tf = &self.track[i];
            (tf.anim, tf.frame)
        };
        let snd = self
            .file
            .animations
            .get(anim)
            .and_then(|a| a.frames.get(frame))
            .map(|f| f.sound_ndx);
        if let Some(snd) = snd {
            if snd >= 0 {
                if let Some(wav) = self.file.sound(snd as usize) {
                    self.audio.play(wav);
                }
            }
        }
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

    /// A cheap identity for the frame [`composite_current`](Agent::composite_current) would
    /// return right now: the `(animation index, frame index, mouth overlay)` triple, or
    /// `None` when hidden. Two ticks with equal tokens composite to byte-identical pixels,
    /// so a host can skip re-compositing / re-encoding / re-uploading while the token is
    /// unchanged — and use it as a content-address key when caching frames across a process
    /// boundary (e.g. shipping PNGs to a separate UI process).
    pub fn current_frame_token(&self) -> Option<(usize, usize, Option<MouthOverlay>)> {
        if !self.visible {
            return None;
        }
        let tf = self.track_frame()?;
        Some((tf.anim, tf.frame, self.current_mouth()))
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

    /// The balloon to draw now, or `None` when none is showing. Independent of the current
    /// activity: a balloon can linger (auto-hiding) while the character resumes idling.
    pub fn balloon(&self) -> Option<BalloonView> {
        let kind = self.balloon_kind?;
        if self.speak_words.is_empty() {
            return None;
        }
        let total = self.speak_words.len();
        let shown = if self.balloon_done {
            total
        } else {
            self.speak_shown.clamp(1, total)
        };
        let per_line = self.style.per_line;
        let (layout, full) = if self.style.size_to_text {
            // Grow to fit: draw the revealed words; size the window to the whole phrase.
            (
                wrap_words(&self.speak_words[..shown], per_line),
                wrap_words(&self.speak_words, per_line),
            )
        } else {
            // Fixed box: scroll the revealed words within `lines` rows.
            let rows = self.style.lines;
            (
                wrap_last_rows(&self.speak_words[..shown], per_line, rows),
                fixed_box_layout(per_line, rows),
            )
        };
        Some(BalloonView {
            layout,
            full,
            total_words: total,
            shown_words: shown,
            kind,
        })
    }
}

/// A blank layout sized to a fixed `per_line`×`rows` box, for sizing a scrolling balloon.
fn fixed_box_layout(per_line: usize, rows: usize) -> BalloonLayout {
    BalloonLayout {
        lines: vec![String::new(); rows.max(1)],
        cols: per_line,
        rows: rows.max(1),
    }
}
