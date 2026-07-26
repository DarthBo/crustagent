// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pluggable text-to-speech for crustagent.
//!
//! Speech is modeled as an engine that, once told to [`speak`](TtsEngine::speak), emits a
//! stream of [`VoiceEvent`]s as it plays — word boundaries (to reveal balloon words),
//! visemes (to move the mouth), bookmarks, and start/end. The host pumps it with
//! [`poll`](TtsEngine::poll) each tick, matching crustagent's `update(dt)` loop (no
//! threads or callbacks needed), which keeps everything deterministic and testable.
//!
//! [`TimedTts`] is the portable default: **no audio**, it just paces the events on a
//! timer (the classic silent-balloon behavior). [`SystemTts`] adds real audio on
//! Windows/macOS/Linux via the [`tts`] crate (WinRT/SAPI, AVSpeech, speech-dispatcher),
//! reusing the timed event stream for word/mouth pacing since those engines don't expose
//! visemes uniformly.

use crustagent_format::MouthOverlay;

pub use crustagent_format::Gender;

/// The voice a character asks for, taken from its file's TTS block.
///
/// The original SAPI 4 voices are long gone, so the engine can't honor the exact mode id;
/// what still carries over is *which kind* of voice the author picked. [`SystemTts`] uses
/// this to choose among the OS voices instead of leaving everyone on the system default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoiceRequest {
    pub gender: Gender,
    /// ISO 639-1 code of the character's language, mapped from its Windows LANGID.
    pub language: Option<&'static str>,
}

impl VoiceRequest {
    /// The voice described by a parsed character's TTS block.
    pub fn from_tts(tts: &crustagent_format::Tts) -> VoiceRequest {
        VoiceRequest {
            gender: tts.resolved_gender(),
            language: tts.language.and_then(iso_639_1),
        }
    }
}

/// Map a Windows `LANGID`'s primary language to its ISO 639-1 code.
///
/// Only the primary id matters here — matching `en` to any English system voice is the
/// right behavior; insisting on the exact `en-GB`/`en-US` sublanguage would more often
/// leave us with no voice at all. Unlisted languages return `None` (no language filter).
fn iso_639_1(langid: u16) -> Option<&'static str> {
    Some(match langid & 0x3FF {
        0x01 => "ar",
        0x02 => "bg",
        0x03 => "ca",
        0x04 => "zh",
        0x05 => "cs",
        0x06 => "da",
        0x07 => "de",
        0x08 => "el",
        0x09 => "en",
        0x0A => "es",
        0x0B => "fi",
        0x0C => "fr",
        0x0D => "he",
        0x0E => "hu",
        0x0F => "is",
        0x10 => "it",
        0x11 => "ja",
        0x12 => "ko",
        0x13 => "nl",
        0x14 => "no",
        0x15 => "pl",
        0x16 => "pt",
        0x18 => "ro",
        0x19 => "ru",
        0x1A => "hr",
        0x1B => "sk",
        0x1D => "sv",
        0x1E => "th",
        0x1F => "tr",
        0x22 => "uk",
        0x24 => "sl",
        _ => return None,
    })
}

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
    /// Adopt the voice a character asks for. Called when the engine is attached to an
    /// agent; engines without real audio ignore it, hence the default no-op.
    fn set_voice(&mut self, _voice: VoiceRequest) {}
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

/// A real-audio backend using the cross-platform [`tts`] crate (WinRT/SAPI on Windows,
/// `AVSpeechSynthesizer` on macOS, speech-dispatcher on Linux). Audio plays through the
/// OS engine while the timed engine supplies the word/mouth events — those engines don't
/// expose visemes uniformly, so word reveal and the mouth are paced on the timer (not
/// tightly synced to the actual speaking rate; a viseme-capable backend would fix that).
///
/// The character's own voice can't be reproduced (its SAPI 4 mode id names an engine that
/// no longer exists), but [`set_voice`](TtsEngine::set_voice) matches its gender and
/// language against the installed voices — otherwise every character speaks in whatever
/// single voice the OS defaults to.
///
/// If no system engine is available (e.g. speech-dispatcher not installed on Linux), it
/// degrades gracefully to silent timed playback.
pub struct SystemTts {
    engine: Option<tts::Tts>,
    timed: TimedTts,
    voice_name: Option<String>,
}

impl Default for SystemTts {
    fn default() -> Self {
        SystemTts {
            engine: tts::Tts::default().ok(),
            timed: TimedTts::new(),
            voice_name: None,
        }
    }
}

impl SystemTts {
    /// The system voice picked for the character, if one was — `None` means the OS default
    /// is still in use. Handy for reporting what a character will actually sound like.
    pub fn voice_name(&self) -> Option<&str> {
        self.voice_name.as_deref()
    }

    /// Pick the system voice that best matches `want`: right gender first, then the
    /// character's language if any voice of that gender speaks it. Leaves the engine on
    /// its current voice when nothing matches, when the character declares no gender, or
    /// when the backend doesn't enumerate voices (AppKit, speech-dispatcher).
    fn choose_voice(&mut self, want: VoiceRequest) -> Option<()> {
        let gender = match want.gender {
            Gender::Female => tts::Gender::Female,
            Gender::Male => tts::Gender::Male,
            Gender::Unspecified => return None,
        };
        let engine = self.engine.as_mut()?;
        let voices = engine.voices().ok()?;
        let matching: Vec<&tts::Voice> = voices
            .iter()
            .filter(|v| v.gender() == Some(gender))
            .collect();
        let speaks = |v: &tts::Voice, lang: &str| {
            let tag = v.language().to_string();
            tag.split(['-', '_'])
                .next()
                .unwrap_or("")
                .eq_ignore_ascii_case(lang)
        };
        let pick = want
            .language
            .and_then(|lang| matching.iter().find(|v| speaks(v, lang)).copied())
            .or_else(|| matching.first().copied())?;
        engine.set_voice(pick).ok()?;
        self.voice_name = Some(pick.name());
        Some(())
    }
}

impl TtsEngine for SystemTts {
    fn set_voice(&mut self, voice: VoiceRequest) {
        self.choose_voice(voice);
    }

    fn speak(&mut self, text: &str, word_count: usize) {
        self.stop();
        if let Some(engine) = &mut self.engine {
            let _ = engine.speak(text, true); // interrupt = replace anything in progress
        }
        self.timed.speak(text, word_count);
    }
    fn stop(&mut self) {
        if let Some(engine) = &mut self.engine {
            let _ = engine.stop();
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

/// The default engine: real system audio via [`SystemTts`] (silent fallback if none).
pub fn default_engine() -> Box<dyn TtsEngine> {
    Box::new(SystemTts::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_langid_to_iso_639_1() {
        assert_eq!(iso_639_1(0x0409), Some("en")); // en-US, every Microsoft character
        assert_eq!(iso_639_1(0x0809), Some("en")); // en-GB — sublanguage ignored
        assert_eq!(iso_639_1(0x080C), Some("fr")); // fr-BE
        assert_eq!(iso_639_1(0x0413), Some("nl"));
        assert_eq!(iso_639_1(0x045E), None); // not in the table: no language filter
    }

    #[test]
    fn voice_request_reads_the_character_voice_block() {
        let mut mode = [0u8; 16];
        // {CA141FD0-AC7F-11D1-97A3-006008273008} — TruVoice adult female #1.
        mode.copy_from_slice(&[
            0xD0, 0x1F, 0x14, 0xCA, 0x7F, 0xAC, 0xD1, 0x11, 0x97, 0xA3, 0x00, 0x60, 0x08, 0x27,
            0x30, 0x08,
        ]);
        let mut tts = crustagent_format::Tts {
            engine: crustagent_format::Guid::NIL,
            mode: crustagent_format::Guid(mode),
            speed: -1,
            pitch: -1,
            language: Some(0x0409),
            gender: 0, // no extended block: gender comes from the voice id
            age: 0,
            style: String::new(),
        };
        assert_eq!(
            VoiceRequest::from_tts(&tts),
            VoiceRequest {
                gender: Gender::Female,
                language: Some("en"),
            }
        );

        tts.gender = 2;
        assert_eq!(VoiceRequest::from_tts(&tts).gender, Gender::Male);
    }

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
