//! Speech-text markup parser — a port of the original tag grammar.
//!
//! A `Speak` string mixes balloon text with inline backslash tags. This splits it into
//! two aligned views: the **display words** shown in the balloon, and an ordered
//! **speech stream** of words and directives for a TTS backend. Unlike the original — which
//! emits SAPI4 control codes or SAPI5 XML — we produce a neutral [`Tag`] enum so any
//! backend (or none) can consume it.
//!
//! Key behaviors reproduced:
//! - `\\` → literal `\`, `\"` → literal `"`.
//! - Tags are recognized **case-insensitively**; only the 23 known names are tags, any
//!   other `\x` is literal text.
//! - `\Map="displayed"="spoken"\` shows one thing and speaks another (the spoken half is
//!   parsed recursively).
//! - `\Mrk=N\` is a bookmark: a speech-stream directive with no display word.
//! - All other tags are speech-stream directives and never produce display text.

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
pub fn parse_speech(input: &str) -> ParsedSpeech {
    let chars: Vec<char> = input.chars().collect();
    let mut out = ParsedSpeech::default();
    let mut run = String::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c != '\\' {
            run.push(c);
            i += 1;
            continue;
        }

        // Escapes.
        if matches!(chars.get(i + 1), Some('\\')) {
            run.push('\\');
            i += 2;
            continue;
        }
        if matches!(chars.get(i + 1), Some('"')) {
            run.push('"');
            i += 2;
            continue;
        }

        // Read the tag name (letters/digits after the backslash).
        let name_start = i + 1;
        let mut k = name_start;
        while k < chars.len() && chars[k].is_ascii_alphanumeric() {
            k += 1;
        }
        let name: String = chars[name_start..k].iter().collect();
        let lname = name.to_ascii_lowercase();

        if !is_known_tag(&lname) {
            // Not a tag: the backslash is literal text.
            run.push('\\');
            i += 1;
            continue;
        }

        // A recognized tag begins — flush pending literal text first.
        flush_run(&mut run, &mut out);

        if lname == "map" {
            i = parse_map(&chars, k, &mut out);
            continue;
        }

        // Optional `=value` or `;value` up to the closing backslash.
        let mut value: Option<String> = None;
        if matches!(chars.get(k), Some('=') | Some(';')) {
            let vstart = k + 1;
            let mut v = vstart;
            while v < chars.len() && chars[v] != '\\' {
                v += 1;
            }
            value = Some(chars[vstart..v].iter().collect());
            k = v;
        }
        // Consume the closing backslash if present.
        if matches!(chars.get(k), Some('\\')) {
            k += 1;
        }

        push_tag(&mut out, make_tag(&lname, &name, value));
        i = k;
    }

    flush_run(&mut run, &mut out);
    out
}

/// Push a directive onto the speech stream, recording a bookmark's display position (the
/// number of display words emitted so far) so the runtime can fire it during reveal.
fn push_tag(out: &mut ParsedSpeech, tag: Tag) {
    if let Tag::Bookmark(id) = tag {
        out.bookmark_at.push((id, out.display_words.len()));
    }
    out.speech.push(SpeechItem::Tag(tag));
}

fn flush_run(run: &mut String, out: &mut ParsedSpeech) {
    for w in run.split_whitespace() {
        out.display_words.push(w.to_string());
        out.speech.push(SpeechItem::Word(w.to_string()));
    }
    run.clear();
}

/// Parse `\Map="disp"="spk"\` starting just after the tag name (`chars[at]` should be
/// `=`). Returns the index past the tag.
fn parse_map(chars: &[char], at: usize, out: &mut ParsedSpeech) -> usize {
    let mut k = at;
    // Expect `="disp"="spk"` ; be lenient if malformed (treat body as display text).
    let start = k;
    if !matches!(chars.get(k), Some('=')) {
        return fallback_map(chars, start, out);
    }
    k += 1;
    let (disp, nk) = match read_quoted(chars, k) {
        Some(v) => v,
        None => return fallback_map(chars, start, out),
    };
    k = nk;
    if !matches!(chars.get(k), Some('=')) {
        return fallback_map(chars, start, out);
    }
    k += 1;
    let (spk, nk) = match read_quoted(chars, k) {
        Some(v) => v,
        None => return fallback_map(chars, start, out),
    };
    k = nk;
    if matches!(chars.get(k), Some('\\')) {
        k += 1;
    }

    // Display half: words to the balloon only.
    for w in disp.split_whitespace() {
        out.display_words.push(w.to_string());
    }
    // Spoken half: parse recursively; its speech stream feeds ours (its display ignored).
    // Bookmarks inside the spoken half are re-anchored to the current display position.
    let inner = parse_speech(&spk);
    for item in inner.speech {
        match item {
            SpeechItem::Tag(t) => push_tag(out, t),
            w => out.speech.push(w),
        }
    }
    k
}

/// If `\Map` is malformed, treat its raw body (to the next backslash) as display text —
/// matching the original's fallback.
fn fallback_map(chars: &[char], from: usize, out: &mut ParsedSpeech) -> usize {
    let mut k = from;
    let mut body = String::new();
    while k < chars.len() && chars[k] != '\\' {
        body.push(chars[k]);
        k += 1;
    }
    if matches!(chars.get(k), Some('\\')) {
        k += 1;
    }
    for w in body.split_whitespace() {
        out.display_words.push(w.to_string());
    }
    k
}

/// Read a double-quoted string starting at `chars[k] == '"'`, treating `""` as an escaped
/// quote. Returns the unescaped content and the index past the closing quote.
fn read_quoted(chars: &[char], k: usize) -> Option<(String, usize)> {
    if !matches!(chars.get(k), Some('"')) {
        return None;
    }
    let mut i = k + 1;
    let mut s = String::new();
    while i < chars.len() {
        if chars[i] == '"' {
            if matches!(chars.get(i + 1), Some('"')) {
                s.push('"');
                i += 2;
            } else {
                return Some((s, i + 1));
            }
        } else {
            s.push(chars[i]);
            i += 1;
        }
    }
    None // unterminated
}

fn is_known_tag(lname: &str) -> bool {
    matches!(
        lname,
        "chr" | "com" | "ctx" | "dem" | "emp" | "eng" | "lst" | "map" | "mrk" | "pau" | "pit"
            | "pra" | "prn" | "pro" | "prt" | "rst" | "rms" | "rmw" | "rpit" | "rprn" | "rspd"
            | "spd" | "vol"
    )
}

fn make_tag(lname: &str, name: &str, value: Option<String>) -> Tag {
    let v = value.clone().unwrap_or_default();
    match lname {
        "mrk" => Tag::Bookmark(v.trim().parse().unwrap_or(0)),
        "pau" => Tag::Pause(v.trim().parse().unwrap_or(0)),
        "emp" => Tag::Emphasize,
        "dem" => Tag::Deemphasize,
        "vol" => Tag::Volume(v.trim().parse().unwrap_or(0)),
        "spd" => Tag::Speed(v.trim().parse().unwrap_or(0)),
        "pit" => Tag::Pitch(v.trim().parse().unwrap_or(0)),
        "rst" => Tag::Reset,
        "lst" => Tag::RepeatLast,
        "ctx" => Tag::Context(v),
        "chr" => Tag::Voice(v),
        "com" => Tag::Command(v),
        "eng" => Tag::Engine(v),
        "prn" | "pra" | "pro" | "prt" => Tag::Pronounce {
            kind: lname.to_string(),
            value: v,
        },
        _ => Tag::Other {
            name: name.to_string(),
            value,
        },
    }
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
        let p = parse_speech(r#"\Map="Dr. Smith"="Doctor Smith"\ here"#);
        assert_eq!(p.display_words, ["Dr.", "Smith", "here"]);
        assert_eq!(p.spoken_text(), "Doctor Smith here");
    }

    #[test]
    fn map_spoken_half_is_parsed_recursively() {
        let p = parse_speech(r#"\Map="!"="\Emp\wow"\"#);
        assert_eq!(p.display_words, ["!"]);
        assert_eq!(
            p.speech,
            vec![SpeechItem::Tag(Tag::Emphasize), SpeechItem::Word("wow".into())]
        );
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
