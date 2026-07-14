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

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
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
