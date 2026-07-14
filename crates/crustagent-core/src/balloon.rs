//! Word-balloon text layout: wrap display words into lines, break only at word
//! boundaries. Pure text math — pixel rendering (rounded rect, tail, font)
//! lives in the renderer.

/// Wrapped balloon text plus its size in character cells.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BalloonLayout {
    /// Lines of text, already wrapped.
    pub lines: Vec<String>,
    /// Widest line, in characters (≤ `per_line` unless a single word is longer).
    pub cols: usize,
    /// Number of lines.
    pub rows: usize,
}

/// Greedily wrap `words` into lines of at most `per_line` characters, never splitting a
/// word (a word longer than `per_line` gets its own overflowing line). Words are joined
/// with single spaces. `per_line` is clamped to ≥1.
pub fn wrap_words(words: &[String], per_line: usize) -> BalloonLayout {
    let per_line = per_line.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    for word in words {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= per_line {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }

    let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let rows = lines.len();
    BalloonLayout { lines, cols, rows }
}

/// Wrap `words` like [`wrap_words`] but keep only the last `max_rows` lines. Because words
/// reveal front-to-back, the most recent word is always on the last line, so keeping the
/// tail lines makes a fixed-height balloon scroll to follow the speech. `max_rows` ≥ 1.
pub fn wrap_last_rows(words: &[String], per_line: usize, max_rows: usize) -> BalloonLayout {
    let mut l = wrap_words(words, per_line);
    let max_rows = max_rows.max(1);
    if l.lines.len() > max_rows {
        l.lines.drain(0..l.lines.len() - max_rows);
        l.cols = l.lines.iter().map(|s| s.chars().count()).max().unwrap_or(0);
        l.rows = l.lines.len();
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn last_rows_scrolls_to_the_tail() {
        // 6 words wrap to 3 lines at width 10; a 2-row box shows the last two.
        let full = wrap_words(&w("aaa bbb ccc ddd eee fff"), 7);
        assert!(full.rows >= 3);
        let paged = wrap_last_rows(&w("aaa bbb ccc ddd eee fff"), 7, 2);
        assert_eq!(paged.rows, 2);
        assert_eq!(paged.lines, full.lines[full.rows - 2..]);
    }

    #[test]
    fn last_rows_noop_when_fits() {
        let l = wrap_last_rows(&w("a b c"), 40, 2);
        assert_eq!(l.lines, ["a b c"]);
    }

    #[test]
    fn wraps_at_word_boundaries() {
        let l = wrap_words(&w("the quick brown fox"), 10);
        assert_eq!(l.lines, ["the quick", "brown fox"]);
        assert_eq!(l.rows, 2);
        assert_eq!(l.cols, 9);
    }

    #[test]
    fn long_word_gets_its_own_line() {
        let l = wrap_words(&w("hi supercalifragilistic ok"), 8);
        assert_eq!(l.lines, ["hi", "supercalifragilistic", "ok"]);
        assert_eq!(l.cols, "supercalifragilistic".len());
    }

    #[test]
    fn empty_input() {
        let l = wrap_words(&[], 20);
        assert!(l.lines.is_empty());
        assert_eq!((l.cols, l.rows), (0, 0));
    }

    #[test]
    fn single_line_fits() {
        let l = wrap_words(&w("all on one line"), 40);
        assert_eq!(l.lines, ["all on one line"]);
        assert_eq!(l.rows, 1);
    }
}
