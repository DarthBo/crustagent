//! Speech-text markup parser for Microsoft Agent `Speak`/`Think` strings.
//!
//! A `Speak` string mixes balloon text with inline backslash tags; parsing splits it into
//! the **display words** shown in the balloon and an ordered **speech stream** of words and
//! directives for a TTS backend (a neutral [`Tag`] enum, so any backend — or none — can
//! consume it).
//!
//! **Status: temporarily stubbed.** This parser was carried over from the pre-relicense
//! GPL-derived code and is being reimplemented clean-room from Microsoft's
//! *documented* `Speak()` markup grammar (the backslash tags — `\Mrk`, `\Pau`, `\Map`, …).
//! [`parse_speech`] currently returns an empty [`ParsedSpeech`]; the [`Tag`] / [`SpeechItem`]
//! / [`ParsedSpeech`] types below are unaffected.

/// A parsed inline directive from the speech stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    /// `\Mrk=N` — bookmark; the engine raises a callback when speech reaches it.
    Bookmark(i64),
    /// `\Pau=N` — pause N milliseconds.
    Pause(u32),
    /// `\Emp` — emphasize the next word.
    Emphasize,
    /// `\Dem` — de-emphasize.
    Deemphasize,
    /// `\Vol=N` — volume, 0..=65535.
    Volume(u32),
    /// `\Spd=N` — speaking speed (words/min).
    Speed(u32),
    /// `\Pit=N` — pitch (Hz).
    Pitch(u32),
    /// `\Rst` — reset voice parameters to default.
    Reset,
    /// `\Lst` — repeat the last spoken string.
    RepeatLast,
    /// `\Ctx=…` — speaking context (numbers/dates normalization).
    Context(String),
    /// `\Chr=…` — voice character (e.g. `Normal`, `Whisper`).
    Voice(String),
    /// `\Com=…` — speaking command/context hint.
    Command(String),
    /// `\Eng=…` / `\Eng;…` — direct engine control string.
    Engine(String),
    /// Pronunciation family: `\Prn` / `\Pra` / `\Pro` / `\Prt`.
    Pronounce { kind: String, value: String },
    /// A recognized-but-unmodeled tag (`\RmS`, `\RmW`, `\RPit`, `\RPrn`, `\RSpd`).
    Other { name: String, value: Option<String> },
}

/// One element of the ordered speech stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechItem {
    /// A spoken word.
    Word(String),
    /// An inline directive.
    Tag(Tag),
}

/// The result of parsing a `Speak` string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedSpeech {
    /// Words shown in the balloon, in order.
    pub display_words: Vec<String>,
    /// Ordered stream of spoken words and directives.
    pub speech: Vec<SpeechItem>,
    /// Each `\Mrk=N` bookmark paired with the number of display words that precede it, so a
    /// runtime can raise the bookmark as the balloon reveals past that word.
    pub bookmark_at: Vec<(i64, usize)>,
}

impl ParsedSpeech {
    /// The balloon text (display words joined by single spaces).
    pub fn display_text(&self) -> String {
        self.display_words.join(" ")
    }

    /// The spoken text with directives removed (words joined by single spaces).
    pub fn spoken_text(&self) -> String {
        self.speech
            .iter()
            .filter_map(|it| match it {
                SpeechItem::Word(w) => Some(w.as_str()),
                SpeechItem::Tag(_) => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// All bookmark ids in order.
    pub fn bookmarks(&self) -> impl Iterator<Item = i64> + '_ {
        self.speech.iter().filter_map(|it| match it {
            SpeechItem::Tag(Tag::Bookmark(n)) => Some(*n),
            _ => None,
        })
    }
}

/// Parse a `Speak`/`Think` string into display words and a speech stream.
///
/// **Temporarily stubbed.** The markup parser was carried over from the pre-relicense
/// GPL-derived code and is being reimplemented clean-room from Microsoft's
/// documented `Speak()` markup grammar; it currently returns an empty [`ParsedSpeech`].
pub fn parse_speech(input: &str) -> ParsedSpeech {
    let _ = input;
    ParsedSpeech::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "speech-markup parser stubbed during clean-room rewrite (see module docs)"]
    fn plain_text() {
        let p = parse_speech("Hello there world");
        assert_eq!(p.display_words, ["Hello", "there", "world"]);
        assert_eq!(p.spoken_text(), "Hello there world");
        assert_eq!(p.display_text(), "Hello there world");
    }

    #[test]
    #[ignore = "speech-markup parser stubbed during clean-room rewrite (see module docs)"]
    fn escapes() {
        let p = parse_speech(r#"a\\b \"q\""#);
        // \\ -> \, \" -> " ; words split on whitespace
        assert_eq!(p.display_words, [r"a\b", "\"q\""]);
    }

    #[test]
    #[ignore = "speech-markup parser stubbed during clean-room rewrite (see module docs)"]
    fn bookmark_is_speech_only() {
        let p = parse_speech(r"Hi \Mrk=5\ there");
        assert_eq!(p.display_words, ["Hi", "there"]);
        assert_eq!(
            p.speech,
            vec![
                SpeechItem::Word("Hi".into()),
                SpeechItem::Tag(Tag::Bookmark(5)),
                SpeechItem::Word("there".into()),
            ]
        );
        assert_eq!(p.bookmarks().collect::<Vec<_>>(), vec![5]);
        // The bookmark sits after "Hi" (1 display word) and before "there".
        assert_eq!(p.bookmark_at, vec![(5, 1)]);
    }

    #[test]
    #[ignore = "speech-markup parser stubbed during clean-room rewrite (see module docs)"]
    fn value_tags() {
        let p = parse_speech(r"\Vol=32768\loud \Pau=250\ \Spd=140\fast");
        let tags: Vec<&Tag> = p
            .speech
            .iter()
            .filter_map(|i| match i {
                SpeechItem::Tag(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(tags[0], &Tag::Volume(32768));
        assert_eq!(tags[1], &Tag::Pause(250));
        assert_eq!(tags[2], &Tag::Speed(140));
        assert_eq!(p.display_words, ["loud", "fast"]);
    }

    #[test]
    #[ignore = "speech-markup parser stubbed during clean-room rewrite (see module docs)"]
    fn toggle_tags() {
        let p = parse_speech(r"\Emp\Now \Rst\done");
        assert!(matches!(p.speech[0], SpeechItem::Tag(Tag::Emphasize)));
        assert!(p
            .speech
            .iter()
            .any(|i| matches!(i, SpeechItem::Tag(Tag::Reset))));
        assert_eq!(p.display_words, ["Now", "done"]);
    }

    #[test]
    #[ignore = "speech-markup parser stubbed during clean-room rewrite (see module docs)"]
    fn map_splits_display_and_speech() {
        let p = parse_speech(r#"\Map="Dr. Smith"="Doctor Smith"\ here"#);
        assert_eq!(p.display_words, ["Dr.", "Smith", "here"]);
        assert_eq!(p.spoken_text(), "Doctor Smith here");
    }

    #[test]
    #[ignore = "speech-markup parser stubbed during clean-room rewrite (see module docs)"]
    fn map_spoken_half_is_parsed_recursively() {
        let p = parse_speech(r#"\Map="!"="\Emp\wow"\"#);
        assert_eq!(p.display_words, ["!"]);
        assert_eq!(
            p.speech,
            vec![
                SpeechItem::Tag(Tag::Emphasize),
                SpeechItem::Word("wow".into())
            ]
        );
    }

    #[test]
    #[ignore = "speech-markup parser stubbed during clean-room rewrite (see module docs)"]
    fn case_insensitive_and_unknown_is_literal() {
        // \MRK recognized regardless of case; \Foo is not a tag -> literal backslash text.
        let p = parse_speech(r"\mRk=1\ x \Foo\ y");
        assert_eq!(p.bookmarks().collect::<Vec<_>>(), vec![1]);
        // "\Foo\" stays literal in the display words.
        assert!(p.display_words.iter().any(|w| w.contains(r"\Foo\")));
    }
}
