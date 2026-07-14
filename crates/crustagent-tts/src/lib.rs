//! Pluggable text-to-speech for crustagent.
//!
//! Speech is modeled as an engine that, once told to [`speak`](TtsEngine::speak), emits a
//! stream of [`VoiceEvent`]s as it plays — word boundaries (to reveal balloon words),
//! visemes (to move the mouth), bookmarks, and start/end. The host pumps it with
//! [`poll`](TtsEngine::poll) each tick, matching crustagent's `update(dt)` loop (no
//! threads or callbacks needed), which keeps everything deterministic and testable.
//!
//! [`TimedTts`] is the portable default: **no audio**, it just paces the events on a
//! timer (the classic silent-balloon behavior). Real audio backends implement the same
//! trait; [`SayTts`] (macOS) plays actual speech via the `say` command while reusing the
//! timed event stream for lip/word sync.

use crustagent_format::MouthOverlay;

/// Something that happened during speech, consumed by the runtime.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VoiceEvent {
    /// Speech started.
    Started,
    /// The word at this index (into the display words) began.
    WordStarted(usize),
    /// The mouth should take this shape now.
    Mouth(MouthOverlay),
    /// A `\Mrk=N` bookmark was reached.
    Bookmark(i64),
    /// Speech finished.
    Ended,
}

/// A text-to-speech engine driven by polling.
pub trait TtsEngine {
    /// Begin speaking `text`; `word_count` is how many balloon words to pace events over.
    fn speak(&mut self, text: &str, word_count: usize);
    /// Stop immediately.
    fn stop(&mut self);
    /// Advance by `dt_ms` and return any events that occurred.
    fn poll(&mut self, dt_ms: u32) -> Vec<VoiceEvent>;
    /// Whether speech is in progress.
    fn is_speaking(&self) -> bool;
}

/// Default pacing: one word every 300 ms, mouth toggles every 150 ms.
const PACE_MS: u32 = 300;
const MOUTH_MS: u32 = 150;

/// A silent engine that paces voice events on a timer. Deterministic and dependency-free.
#[derive(Clone, Debug)]
pub struct TimedTts {
    pace_ms: u32,
    words: usize,
    elapsed: u32,
    next_word: usize,
    speaking: bool,
    started: bool,
    mouth_phase: i32,
}

impl Default for TimedTts {
    fn default() -> Self {
        TimedTts::new()
    }
}

impl TimedTts {
    pub fn new() -> TimedTts {
        TimedTts {
            pace_ms: PACE_MS,
            words: 0,
            elapsed: 0,
            next_word: 0,
            speaking: false,
            started: false,
            mouth_phase: -1,
        }
    }

    /// Set the per-word pacing interval.
    pub fn with_pace(mut self, ms: u32) -> Self {
        self.pace_ms = ms.max(1);
        self
    }

    fn total_ms(&self) -> u32 {
        self.words as u32 * self.pace_ms
    }
}

impl TtsEngine for TimedTts {
    fn speak(&mut self, _text: &str, word_count: usize) {
        self.words = word_count.max(1);
        self.elapsed = 0;
        self.next_word = 0;
        self.speaking = true;
        self.started = false;
        self.mouth_phase = -1;
    }

    fn stop(&mut self) {
        self.speaking = false;
    }

    fn poll(&mut self, dt_ms: u32) -> Vec<VoiceEvent> {
        if !self.speaking {
            return Vec::new();
        }
        let mut events = Vec::new();
        if !self.started {
            self.started = true;
            events.push(VoiceEvent::Started);
        }
        self.elapsed = self.elapsed.saturating_add(dt_ms);

        while self.next_word < self.words && self.elapsed >= self.next_word as u32 * self.pace_ms {
            events.push(VoiceEvent::WordStarted(self.next_word));
            self.next_word += 1;
        }

        let phase = ((self.elapsed / MOUTH_MS) % 2) as i32;
        if phase != self.mouth_phase {
            self.mouth_phase = phase;
            let mouth = if phase == 0 {
                MouthOverlay::Wide2
            } else {
                MouthOverlay::Closed
            };
            events.push(VoiceEvent::Mouth(mouth));
        }

        if self.elapsed >= self.total_ms() {
            events.push(VoiceEvent::Mouth(MouthOverlay::Closed));
            events.push(VoiceEvent::Ended);
            self.speaking = false;
        }
        events
    }

    fn is_speaking(&self) -> bool {
        self.speaking
    }
}

/// A real-audio backend for macOS: plays speech with the `say` command while the timed
/// engine supplies the word/mouth events (they aren't perfectly synced to `say`'s actual
/// rate — see the crate docs — but you hear the character talk).
#[cfg(target_os = "macos")]
#[derive(Default)]
pub struct SayTts {
    timed: TimedTts,
    child: Option<std::process::Child>,
}

#[cfg(target_os = "macos")]
impl TtsEngine for SayTts {
    fn speak(&mut self, text: &str, word_count: usize) {
        self.stop();
        self.child = std::process::Command::new("say").arg(text).spawn().ok();
        self.timed.speak(text, word_count);
    }
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
        self.timed.stop();
    }
    fn poll(&mut self, dt_ms: u32) -> Vec<VoiceEvent> {
        self.timed.poll(dt_ms)
    }
    fn is_speaking(&self) -> bool {
        self.timed.is_speaking()
    }
}

/// The best available default engine: real audio where we have a backend, else silent.
pub fn default_engine() -> Box<dyn TtsEngine> {
    #[cfg(target_os = "macos")]
    {
        Box::new(SayTts::default())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(TimedTts::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(engine: &mut TimedTts, ms: u32, step: u32) -> Vec<VoiceEvent> {
        let mut all = Vec::new();
        let mut left = ms;
        while left > 0 {
            let dt = left.min(step);
            all.extend(engine.poll(dt));
            left -= dt;
        }
        all
    }

    #[test]
    fn paces_words_then_ends() {
        let mut t = TimedTts::new(); // 300ms/word
        t.speak("one two three", 3);
        let events = drain(&mut t, 1000, 16);

        let words: Vec<usize> = events
            .iter()
            .filter_map(|e| match e {
                VoiceEvent::WordStarted(i) => Some(*i),
                _ => None,
            })
            .collect();
        assert_eq!(words, vec![0, 1, 2]);
        assert!(events.first() == Some(&VoiceEvent::Started));
        assert!(events.contains(&VoiceEvent::Ended));
        assert!(!t.is_speaking());
    }

    #[test]
    fn emits_mouth_movement() {
        let mut t = TimedTts::new();
        t.speak("hello", 1);
        let events = drain(&mut t, 400, 16);
        let mouths = events
            .iter()
            .filter(|e| matches!(e, VoiceEvent::Mouth(_)))
            .count();
        assert!(mouths >= 2, "mouth should move at least twice");
    }

    #[test]
    fn stop_halts_events() {
        let mut t = TimedTts::new();
        t.speak("a b c d", 4);
        let _ = t.poll(16);
        t.stop();
        assert!(t.poll(1000).is_empty());
        assert!(!t.is_speaking());
    }
}
