//! Speech-text markup parser for Microsoft Agent `Speak`/`Think` strings.
//!
//! A `Speak` string mixes balloon text with inline backslash tags; parsing splits it into
//! the **display words** shown in the balloon and an ordered **speech stream** of words and
//! directives for a TTS backend (a neutral [`Tag`] enum, so any backend — or none — can
//! consume it).
//!
//! The grammar is Microsoft's `Speak()` output-tag markup, specified in
//! [`docs/speak-markup.md`](../../../docs/speak-markup.md): `\Tag\` and `\Tag=value\`, an
//! `=` separator, case-insensitive three-letter tag names, no whitespace inside the
//! delimiters, `\\` for a literal backslash, and the eleven tags of that document's §4.

/// A parsed inline directive from the speech stream.
///
/// The variants [`Deemphasize`](Tag::Deemphasize), [`Command`](Tag::Command),
/// [`Engine`](Tag::Engine), [`Pronounce`](Tag::Pronounce) and [`Other`](Tag::Other) are not
/// part of Microsoft Agent's tag set (`docs/speak-markup.md` §5) and so are never produced by
/// [`parse_speech`]; they remain for callers that synthesize directives of their own.
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
/// Plain text is split on whitespace into words that both show in the balloon and are spoken.
/// A tag is lifted out of the text: most become a [`Tag`] in the speech stream at the point
/// they appeared (so a backend applies them between words), `\Mrk=N\` additionally records its
/// position in the balloon in [`ParsedSpeech::bookmark_at`], and `\Map="…"="…"\` splits the two
/// halves apart — the first is parsed as speech, the second goes to the balloon. A tag always
/// ends the word it interrupts. `\\` and `\"` stand for a literal backslash and quote; a
/// backslash sequence that is not one of the documented tags is literal text
/// (`docs/speak-markup.md` §1.3–§1.4).
pub fn parse_speech(input: &str) -> ParsedSpeech {
    let mut parsed = ParsedSpeech::default();
    parse_stream(input, &mut parsed, Sink::Both);
    parsed
}

/// Which stream(s) a run of text contributes its words to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sink {
    /// Ordinary text: shown in the balloon and spoken.
    Both,
    /// The spoken half of a `\Map\` — spoken, never displayed.
    SpeechOnly,
    /// The balloon half of a `\Map\` — displayed, never spoken.
    DisplayOnly,
}

/// One piece of markup lifted out of the text.
enum Markup {
    Tag(Tag),
    /// `\Map="spoken"="balloon"\` (§2): speak one thing, display another. The spoken text is
    /// the first parameter, as in Microsoft's own example `\map="Spoken text"="Balloon text"\`.
    Map { spoken: String, balloon: String },
}

/// Walk `input`, accumulating words into `out` and interpreting tags as they appear.
fn parse_stream(input: &str, out: &mut ParsedSpeech, sink: Sink) {
    let chars: Vec<char> = input.chars().collect();
    let mut word = String::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            // An escape stays inside the current word; a tag breaks it.
            if let Some(&escaped @ ('\\' | '"')) = chars.get(i + 1) {
                word.push(escaped);
                i += 2;
                continue;
            }
            if let Some((markup, next)) = parse_tag(&chars, i) {
                flush_word(&mut word, out, sink);
                match markup {
                    Markup::Tag(tag) => push_tag(tag, out),
                    Markup::Map { spoken, balloon } => {
                        push_words(&balloon, out, Sink::DisplayOnly);
                        parse_stream(&spoken, out, Sink::SpeechOnly);
                    }
                }
                i = next;
                continue;
            }
            // Not a known tag: the backslash is ordinary text (§1.4).
        }
        if c.is_whitespace() {
            flush_word(&mut word, out, sink);
        } else {
            word.push(c);
        }
        i += 1;
    }
    flush_word(&mut word, out, sink);
}

/// Emit the word built so far, if any, and start a new one.
fn flush_word(word: &mut String, out: &mut ParsedSpeech, sink: Sink) {
    if word.is_empty() {
        return;
    }
    if sink != Sink::SpeechOnly {
        out.display_words.push(word.clone());
    }
    if sink != Sink::DisplayOnly {
        out.speech.push(SpeechItem::Word(std::mem::take(word)));
    }
    word.clear();
}

/// Split `text` on whitespace and emit its words (no tag interpretation).
fn push_words(text: &str, out: &mut ParsedSpeech, sink: Sink) {
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        flush_word(&mut word, out, sink);
    }
}

/// Append a tag to the speech stream, noting where a bookmark falls in the balloon.
fn push_tag(tag: Tag, out: &mut ParsedSpeech) {
    if let Tag::Bookmark(id) = tag {
        out.bookmark_at.push((id, out.display_words.len()));
    }
    out.speech.push(SpeechItem::Tag(tag));
}

/// Parse the tag starting at `chars[start]` (a backslash), returning it and the index just
/// past its closing backslash. `None` if this is not one of the documented tags — including a
/// well-formed-looking but unknown three-letter name, or a missing closing delimiter.
fn parse_tag(chars: &[char], start: usize) -> Option<(Markup, usize)> {
    // A tag name is exactly three single-byte letters, matched case-insensitively (§1.1–§1.2).
    let name = chars.get(start + 1..start + 4)?;
    if !name.iter().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let name: String = name.iter().map(|c| c.to_ascii_lowercase()).collect();
    let after_name = start + 4;

    match *chars.get(after_name)? {
        // Parameterless: \Emp\, \Rst\, \Lst\.
        '\\' => {
            let tag = match name.as_str() {
                "emp" => Tag::Emphasize,
                "rst" => Tag::Reset,
                "lst" => Tag::RepeatLast,
                _ => return None,
            };
            Some((Markup::Tag(tag), after_name + 1))
        }
        // Parameterized: \Tag=value\ — the separator is `=` and nothing else (§1.1).
        '=' => {
            let value_start = after_name + 1;
            if name == "map" {
                return parse_map(chars, value_start);
            }
            // The value runs to the closing delimiter. A literal backslash inside a tag's text
            // parameter is doubled (§1.3), so `\\` is consumed as one character rather than
            // read as the end of the tag.
            let mut end = value_start;
            let mut raw = String::new();
            loop {
                match *chars.get(end)? {
                    '\\' if chars.get(end + 1) == Some(&'\\') => {
                        raw.push('\\');
                        end += 2;
                    }
                    '\\' => break,
                    c => {
                        raw.push(c);
                        end += 1;
                    }
                }
            }
            let value = unquote(&raw);
            let tag = match name.as_str() {
                "mrk" => Tag::Bookmark(value.parse().ok()?),
                "pau" => Tag::Pause(number(value)?),
                "vol" => Tag::Volume(number(value)?),
                "spd" => Tag::Speed(number(value)?),
                "pit" => Tag::Pitch(number(value)?),
                "ctx" => Tag::Context(value.to_string()),
                "chr" => Tag::Voice(value.to_string()),
                _ => return None,
            };
            Some((Markup::Tag(tag), end + 1))
        }
        _ => None,
    }
}

/// Parse the two quoted halves of a `\Map=` from `start` (just past the `=`), per §2: the
/// spoken text runs to `"=`, the balloon text to `"\`, and a doubled `""` is a literal quote
/// inside either.
fn parse_map(chars: &[char], start: usize) -> Option<(Markup, usize)> {
    let (spoken, after_spoken) = map_parameter(chars, start, '=')?;
    let (balloon, after_balloon) = map_parameter(chars, after_spoken, '\\')?;
    Some((Markup::Map { spoken, balloon }, after_balloon))
}

/// Read one `"…"`-quoted `Map` parameter starting at `chars[start]`, terminated by a quote
/// followed by `terminator`. Returns the unescaped text and the index past the terminator.
fn map_parameter(chars: &[char], start: usize, terminator: char) -> Option<(String, usize)> {
    if *chars.get(start)? != '"' {
        return None;
    }
    let mut i = start + 1;
    let mut text = String::new();
    loop {
        let c = *chars.get(i)?;
        // Only a quote *followed by the terminator* closes the parameter, which is what makes
        // the doubled-quote escape work.
        if c == '"' && chars.get(i + 1) == Some(&terminator) {
            return Some((text.replace("\"\"", "\""), i + 2));
        }
        text.push(c);
        i += 1;
    }
}

/// Strip one layer of surrounding double quotes, as the server writes on string values
/// (`\Chr="Whisper"\`).
fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

/// A numeric tag parameter. Documented ranges are engine-dependent (§4), so the value is only
/// clamped to what the [`Tag`] carries rather than range-checked.
fn number(value: &str) -> Option<u32> {
    let n: i64 = value.trim().parse().ok()?;
    Some(n.clamp(0, u32::MAX.into()) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text() {
        let p = parse_speech("Hello there world");
        assert_eq!(p.display_words, ["Hello", "there", "world"]);
        assert_eq!(p.spoken_text(), "Hello there world");
        assert_eq!(p.display_text(), "Hello there world");
    }

    #[test]
    fn escapes() {
        let p = parse_speech(r#"a\\b \"q\""#);
        // \\ -> \, \" -> " ; words split on whitespace
        assert_eq!(p.display_words, [r"a\b", "\"q\""]);
    }

    #[test]
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
    fn map_splits_display_and_speech() {
        // \Map="spoken"="balloon"\ — speak "Doctor", show "Dr.".
        let p = parse_speech(r#"\Map="Doctor Smith"="Dr. Smith"\ here"#);
        assert_eq!(p.display_words, ["Dr.", "Smith", "here"]);
        assert_eq!(p.spoken_text(), "Doctor Smith here");
    }

    #[test]
    fn map_spoken_half_is_parsed_recursively() {
        let p = parse_speech(r#"\Map="\Emp\wow"="!"\"#);
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
    fn a_doubled_backslash_inside_a_value_is_not_the_end_of_the_tag() {
        // §1.3: a literal backslash in a tag's text parameter is written `\\`.
        let p = parse_speech(r#"\Ctx="a\\b"\x"#);
        assert_eq!(
            p.speech[0],
            SpeechItem::Tag(Tag::Context(r"a\b".into())),
            "the value runs past the doubled backslash to the real delimiter"
        );
        assert_eq!(p.display_words, ["x"]);
    }

    #[test]
    fn case_insensitive_and_unknown_is_literal() {
        // \MRK recognized regardless of case; \Foo is not a tag -> literal backslash text.
        let p = parse_speech(r"\mRk=1\ x \Foo\ y");
        assert_eq!(p.bookmarks().collect::<Vec<_>>(), vec![1]);
        // "\Foo\" stays literal in the display words.
        assert!(p.display_words.iter().any(|w| w.contains(r"\Foo\")));
    }
}
