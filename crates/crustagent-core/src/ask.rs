// SPDX-License-Identifier: MIT OR Apache-2.0

//! Interactive balloon content: a question with clickable choices, check boxes and a
//! commit-button row — modeled on the Office Assistant's `Balloon` object, which is where
//! classic characters got their clickable balloons (Microsoft Agent's own balloon was text
//! only). See `docs/balloon-ui.md` for the reference this is derived from.
//!
//! Pure text math, like [`balloon`](crate::balloon): this lays content out into *rows*
//! tagged with what each one is, and the renderer turns rows into pixels and hit-tests
//! them. Nothing here knows about fonts or pixels.

use crate::balloon::wrap_words;

/// Office caps `Labels` and `CheckBoxes` at five each; content past that is dropped rather
/// than raising the run-time error Office does.
pub const MAX_ITEMS: usize = 5;

/// How a choice list renders — Office's `MsoBalloonType`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ChoiceStyle {
    /// Clickable buttons (Office's default for a new balloon).
    #[default]
    Buttons,
    /// A bulleted list.
    Bullets,
    /// A numbered list.
    Numbers,
}

/// A commit button in the row at the bottom of the balloon — the useful subset of Office's
/// `MsoBalloonButtonType` (its `Search` / `Tips` / `Options` / `Snooze` buttons existed only
/// to serve Office's own help UI).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Ok,
    Cancel,
    Yes,
    No,
    Next,
    Back,
    Close,
    /// Submits the balloon's text field. A crustagent extension: Office had the constant
    /// (`msoBalloonButtonSearch`) but never let a developer put a field beside it.
    Search,
}

impl Button {
    /// The button's display text.
    pub fn label(self) -> &'static str {
        match self {
            Button::Ok => "OK",
            Button::Cancel => "Cancel",
            Button::Yes => "Yes",
            Button::No => "No",
            Button::Next => "Next",
            Button::Back => "Back",
            Button::Close => "Close",
            Button::Search => "Search",
        }
    }
}

/// Which commit buttons the balloon shows — Office's `MsoButtonSetType`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ButtonSet {
    /// No button row. Choices alone commit the answer.
    #[default]
    None,
    Ok,
    Cancel,
    OkCancel,
    YesNo,
    YesNoCancel,
    NextClose,
    BackNextClose,
    /// Search + Close, for a balloon with a text field (see [`TextInput`]).
    SearchClose,
}

impl ButtonSet {
    /// The buttons in this set, left to right.
    pub fn buttons(self) -> &'static [Button] {
        match self {
            ButtonSet::None => &[],
            ButtonSet::Ok => &[Button::Ok],
            ButtonSet::Cancel => &[Button::Cancel],
            ButtonSet::OkCancel => &[Button::Ok, Button::Cancel],
            ButtonSet::YesNo => &[Button::Yes, Button::No],
            ButtonSet::YesNoCancel => &[Button::Yes, Button::No, Button::Cancel],
            ButtonSet::NextClose => &[Button::Next, Button::Close],
            ButtonSet::BackNextClose => &[Button::Back, Button::Next, Button::Close],
            ButtonSet::SearchClose => &[Button::Search, Button::Close],
        }
    }
}

/// When the balloon goes away — Office's `MsoModeType`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BalloonMode {
    /// Holds the agent's queue until the user answers (Office: the user must dismiss the
    /// balloon before continuing).
    #[default]
    Modal,
    /// Shown without holding the queue — the character carries on with whatever is queued
    /// while the question lingers.
    Modeless,
    /// Like [`Modeless`](BalloonMode::Modeless), but any reported click dismisses it
    /// unanswered.
    AutoDown,
}

/// A single-line text field in the balloon, for a question whose answer is typed rather
/// than picked — the Assistant's "What would you like to do?" search box.
///
/// **This is a crustagent extension, not an Office feature.** The `Balloon` object had no
/// text-input member at all; the search box in Office's screenshots belongs to MSO's own
/// built-in help balloon, driven by `Assistant.Help` and the `AnswerWizard`, which
/// `Assistant.NewBalloon` could never reproduce. See `docs/balloon-ui.md` §5.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextInput {
    /// Dimmed prompt shown while the field is empty.
    pub placeholder: String,
    /// Text the field starts out holding.
    pub initial: String,
}

/// A question to put in the balloon: an optional heading, body text, up to
/// [`MAX_ITEMS`] clickable choices, up to [`MAX_ITEMS`] check boxes, and a commit-button row.
///
/// Clicking a **choice** answers immediately and dismisses the balloon; a **check box**
/// toggles and waits for a commit button — the split Office drew between `Labels` and
/// `CheckBoxes`.
///
/// ```
/// use crustagent_core::ask::{layout_ask, AskAnswer, BalloonUi, ButtonSet};
/// let ui = BalloonUi::new("What would you like to do?")
///     .heading("Getting started")
///     .choice("Write a letter")
///     .choice("Make a chart")
///     .buttons(ButtonSet::OkCancel);
/// let laid_out = layout_ask(&ui, &AskAnswer::default(), 32);
/// assert_eq!(laid_out.buttons.len(), 2);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BalloonUi {
    /// Bold text at the top of the balloon (Office `Heading`).
    pub heading: Option<String>,
    /// Body text, above the choices (Office `Text`).
    pub text: String,
    /// Clickable choices; each commits immediately (Office `Labels`). Capped at [`MAX_ITEMS`].
    pub choices: Vec<String>,
    /// Toggles that wait for a commit button (Office `CheckBoxes`). Capped at [`MAX_ITEMS`].
    pub checkboxes: Vec<String>,
    /// An optional text field, below the choices and above the buttons.
    pub input: Option<TextInput>,
    /// The commit-button row.
    pub buttons: ButtonSet,
    /// How the choices render.
    pub style: ChoiceStyle,
    /// When the balloon goes away.
    pub mode: BalloonMode,
}

impl BalloonUi {
    /// A question with body `text` and no controls yet.
    pub fn new(text: impl Into<String>) -> BalloonUi {
        BalloonUi {
            text: text.into(),
            ..Default::default()
        }
    }
    /// Set the bold heading.
    pub fn heading(mut self, heading: impl Into<String>) -> BalloonUi {
        self.heading = Some(heading.into());
        self
    }
    /// Add a clickable choice. Ignored past [`MAX_ITEMS`].
    pub fn choice(mut self, label: impl Into<String>) -> BalloonUi {
        if self.choices.len() < MAX_ITEMS {
            self.choices.push(label.into());
        }
        self
    }
    /// Add a check box. Ignored past [`MAX_ITEMS`].
    pub fn checkbox(mut self, label: impl Into<String>) -> BalloonUi {
        if self.checkboxes.len() < MAX_ITEMS {
            self.checkboxes.push(label.into());
        }
        self
    }
    /// Add a text field with `placeholder` shown while it is empty.
    pub fn input(mut self, placeholder: impl Into<String>) -> BalloonUi {
        self.input = Some(TextInput {
            placeholder: placeholder.into(),
            initial: String::new(),
        });
        self
    }
    /// Add a text field pre-filled with `initial`.
    pub fn input_with(
        mut self,
        placeholder: impl Into<String>,
        initial: impl Into<String>,
    ) -> BalloonUi {
        self.input = Some(TextInput {
            placeholder: placeholder.into(),
            initial: initial.into(),
        });
        self
    }
    /// Set the commit-button row.
    pub fn buttons(mut self, buttons: ButtonSet) -> BalloonUi {
        self.buttons = buttons;
        self
    }
    /// Set how the choices render.
    pub fn style(mut self, style: ChoiceStyle) -> BalloonUi {
        self.style = style;
        self
    }
    /// Set when the balloon goes away.
    pub fn mode(mut self, mode: BalloonMode) -> BalloonUi {
        self.mode = mode;
        self
    }
}

/// What a laid-out row *is* — so the renderer can draw the right chrome and turn a click on
/// the row into an answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AskRole {
    /// The heading.
    Heading,
    /// Body text.
    Text,
    /// Part of choice `n` (0-based). A wrapped choice spans several rows, all with this role.
    Choice(usize),
    /// Part of check box `n` (0-based).
    CheckBox(usize),
    /// The text field.
    Input,
    /// The commit-button row — one row holding every button in the set.
    Buttons,
}

impl AskRole {
    /// Whether a click on this row means something.
    pub fn is_interactive(self) -> bool {
        matches!(
            self,
            AskRole::Choice(_) | AskRole::CheckBox(_) | AskRole::Input | AskRole::Buttons
        )
    }
}

/// The control drawn in a row's left margin. A renderer paints real chrome for these — a
/// radio-style disc, a tickable box — while a chrome-free host falls back to the ASCII
/// stand-ins [`AskLayout::lines`] produces.
///
/// Only the *first* row of a wrapped control carries a marker; its continuation rows carry
/// [`RowMarker::None`] and align under the text, not the marker.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowMarker {
    /// No marker (a heading, body text, or a continuation row).
    None,
    /// A clickable choice, in [`ChoiceStyle::Buttons`] — drawn as a radio-style disc.
    Choice,
    /// A bulleted-list choice.
    Bullet,
    /// A numbered-list choice; the 1-based number.
    Number(usize),
    /// A check box and whether it is ticked.
    CheckBox(bool),
}

impl RowMarker {
    /// The ASCII stand-in a chrome-free renderer prints in the left margin.
    pub fn ascii(self, indent: usize) -> String {
        match self {
            RowMarker::None => " ".repeat(indent),
            RowMarker::Choice => "* ".to_string(),
            RowMarker::Bullet => "- ".to_string(),
            RowMarker::Number(n) => format!("{n}. "),
            RowMarker::CheckBox(true) => "[x] ".to_string(),
            RowMarker::CheckBox(false) => "[ ] ".to_string(),
        }
    }
}

/// One laid-out row: the label text **without** any marker, what the row is, the marker to
/// draw in its left margin, and how many character cells that margin reserves (so the ASCII
/// fallback and the wrap width agree).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskRow {
    pub text: String,
    pub role: AskRole,
    pub marker: RowMarker,
    pub indent: usize,
}

impl AskRow {
    /// The row as plain text: marker stand-in (or matching indent) followed by the label.
    pub fn line(&self) -> String {
        format!("{}{}", self.marker.ascii(self.indent), self.text)
    }
}

/// The live state of a question being answered: which boxes are ticked, and what has been
/// typed into its text field. The agent owns one of these per question; a renderer reads it
/// through [`layout_ask`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskAnswer {
    /// Bitmask of ticked check boxes (bit *n* = box *n*).
    pub checked: u8,
    /// The text field's contents.
    pub text: String,
    /// Caret position in `text`, as a **char** offset (clamped on use, so it can never split
    /// a multi-byte character). This is the *moving* end of a selection.
    pub caret: usize,
    /// The fixed end of the selection — where it was started — or `None` when there is no
    /// selection, which is the common case. Optional rather than "equal to `caret`" so that
    /// building an answer with just a caret cannot accidentally select from 0.
    pub anchor: Option<usize>,
}

impl AskAnswer {
    /// A fresh answer, with the field pre-filled from `ui` and the caret at the end.
    pub fn for_question(ui: &BalloonUi) -> AskAnswer {
        let text = ui
            .input
            .as_ref()
            .map(|i| i.initial.clone())
            .unwrap_or_default();
        AskAnswer {
            checked: 0,
            caret: text.chars().count(),
            anchor: None,
            text,
        }
    }
    /// An answer whose field holds `text` with the caret at `caret` and nothing selected.
    pub fn at(text: impl Into<String>, caret: usize) -> AskAnswer {
        AskAnswer {
            text: text.into(),
            caret,
            ..Default::default()
        }
    }
    /// An answer whose field holds `text` with `anchor..caret` selected.
    pub fn selecting(text: impl Into<String>, anchor: usize, caret: usize) -> AskAnswer {
        AskAnswer {
            text: text.into(),
            caret,
            anchor: Some(anchor),
            ..Default::default()
        }
    }
    /// An answer with only check boxes ticked — the common case in tests.
    pub fn checked(mask: u8) -> AskAnswer {
        AskAnswer {
            checked: mask,
            ..Default::default()
        }
    }
    /// The caret, clamped into `text`.
    pub fn caret_char(&self) -> usize {
        self.caret.min(self.text.chars().count())
    }
    /// Byte index of the caret, for slicing `text`.
    pub fn caret_byte(&self) -> usize {
        self.byte_of(self.caret_char())
    }
    /// Byte index of char offset `n`, clamped to the end of `text`.
    pub fn byte_of(&self, n: usize) -> usize {
        self.text
            .char_indices()
            .nth(n)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }
    /// The number of chars in `text`.
    pub fn len_chars(&self) -> usize {
        self.text.chars().count()
    }
    /// The selected range as **char** offsets, low end first, or `None` when the caret is
    /// just a caret.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?.min(self.len_chars());
        let b = self.caret_char();
        let (lo, hi) = (a.min(b), a.max(b));
        (lo != hi).then_some((lo, hi))
    }
    /// The selected text, or `""` when nothing is selected.
    pub fn selected_text(&self) -> &str {
        match self.selection() {
            Some((lo, hi)) => &self.text[self.byte_of(lo)..self.byte_of(hi)],
            None => "",
        }
    }
    /// Collapse any selection, leaving the caret at `n` (clamped).
    pub fn set_caret(&mut self, n: usize) {
        self.caret = n.min(self.len_chars());
        self.anchor = None;
    }
    /// Move the caret to `n`, dragging the selection with it (the anchor stays put).
    pub fn select_to(&mut self, n: usize) {
        let from = self.caret_char();
        self.anchor.get_or_insert(from);
        self.caret = n.min(self.len_chars());
    }
    /// Delete the selection, if any, leaving the caret where it was. Returns whether
    /// anything was removed.
    pub fn delete_selection(&mut self) -> bool {
        let Some((lo, hi)) = self.selection() else {
            return false;
        };
        let (a, b) = (self.byte_of(lo), self.byte_of(hi));
        self.text.replace_range(a..b, "");
        self.set_caret(lo);
        true
    }
}

/// Whether `c` is part of a word, for word-wise movement and double-click selection.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word-ish run containing char offset `n` — a run of word characters, of whitespace, or
/// of punctuation, whichever `n` sits in. Used for double-click-to-select.
pub fn word_at(text: &str, n: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return (0, 0);
    }
    // A click past the last char selects the run ending there.
    let at = n.min(chars.len() - 1);
    let class = |c: char| {
        if is_word_char(c) {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let want = class(chars[at]);
    let mut lo = at;
    while lo > 0 && class(chars[lo - 1]) == want {
        lo -= 1;
    }
    let mut hi = at + 1;
    while hi < chars.len() && class(chars[hi]) == want {
        hi += 1;
    }
    (lo, hi)
}

/// The offset a word-wise move left from `n` lands on: skip any whitespace, then the word.
pub fn word_left(text: &str, n: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut i = n.min(chars.len());
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !chars[i - 1].is_whitespace() {
        i -= 1;
    }
    i
}

/// The offset a word-wise move right from `n` lands on: skip the word, then any whitespace.
pub fn word_right(text: &str, n: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut i = n.min(chars.len());
    while i < chars.len() && !chars[i].is_whitespace() {
        i += 1;
    }
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

/// How the text field should be drawn, when the question has one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputView {
    /// The typed value — empty until something is entered.
    pub value: String,
    /// The placeholder, shown *only* while the field is empty **and** unfocused. A renderer
    /// also sizes the field from this rather than from `value`, so the box doesn't resize as
    /// the value is typed — the field scrolls instead.
    pub prompt: String,
    /// Caret position as a char offset into `value` — the moving end of the selection.
    pub caret: usize,
    /// The selected range as char offsets, low end first, when there is one.
    pub selection: Option<(usize, usize)>,
    /// Index into [`AskLayout::rows`] of the [`AskRole::Input`] row.
    pub row: usize,
}

impl InputView {
    /// Whether the placeholder is what should be drawn. Focusing the field trades the hint
    /// for a caret: once the pointer has put you in the field, the prompt has done its job.
    pub fn shows_prompt(&self, focused: bool) -> bool {
        self.value.is_empty() && !focused
    }
    /// The text to draw for the given focus state.
    pub fn display(&self, focused: bool) -> &str {
        if self.shows_prompt(focused) {
            &self.prompt
        } else {
            &self.value
        }
    }
}

/// A question laid out into rows, ready for a renderer to draw and hit-test.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskLayout {
    /// Rows top to bottom.
    pub rows: Vec<AskRow>,
    /// Widest row, in characters.
    pub cols: usize,
    /// The buttons in the [`AskRole::Buttons`] row, left to right (empty if there is none).
    pub buttons: Vec<Button>,
    /// The text field's contents and caret, when the question has one.
    pub input: Option<InputView>,
    /// How the choices render, so the renderer knows whether to frame them.
    pub style: ChoiceStyle,
}

impl AskLayout {
    /// The role of row `i`, if it exists.
    pub fn role_at(&self, i: usize) -> Option<AskRole> {
        self.rows.get(i).map(|r| r.role)
    }
    /// The rows as plain text, markers rendered as ASCII stand-ins — what a host that draws
    /// no chrome (or the no-TrueType bitmap fallback) can render as ordinary lines.
    pub fn lines(&self) -> Vec<String> {
        self.rows.iter().map(AskRow::line).collect()
    }
}

/// What a click landed on, as reported back to the agent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AskHit {
    /// Choice `n` (0-based) — commits the answer.
    Choice(usize),
    /// Check box `n` (0-based) — toggles.
    CheckBox(usize),
    /// The text field — clicking it places the caret rather than answering.
    Input,
    /// A commit button.
    Button(Button),
}

/// The marker for choice `i`, per the choice style.
fn choice_marker(style: ChoiceStyle, i: usize) -> RowMarker {
    match style {
        ChoiceStyle::Buttons => RowMarker::Choice,
        ChoiceStyle::Bullets => RowMarker::Bullet,
        ChoiceStyle::Numbers => RowMarker::Number(i + 1),
    }
}

/// Wrap `label` to `per_line` characters and push it as rows tagged `role`: the first row
/// carries `marker`, continuation rows carry none and align under the text.
fn push_wrapped(
    rows: &mut Vec<AskRow>,
    label: &str,
    marker: RowMarker,
    role: AskRole,
    per_line: usize,
) {
    let indent = marker.ascii(0).chars().count();
    let words: Vec<String> = label.split_whitespace().map(String::from).collect();
    let wrapped = wrap_words(&words, per_line.saturating_sub(indent).max(1));
    if wrapped.lines.is_empty() {
        // An empty label still occupies its row, so its marker (a check box, a number)
        // doesn't silently vanish.
        rows.push(AskRow {
            text: String::new(),
            role,
            marker,
            indent,
        });
        return;
    }
    for (n, line) in wrapped.lines.iter().enumerate() {
        rows.push(AskRow {
            text: line.clone(),
            role,
            marker: if n == 0 { marker } else { RowMarker::None },
            indent,
        });
    }
}

/// Lay a question out into rows at `per_line` characters wide, drawing check-box state and
/// the text field's contents from `answer`.
///
/// Row order follows Office's balloon — heading, text, choices, check boxes, button row —
/// with the text field (a crustagent addition) sitting between the check boxes and the
/// buttons, where the Assistant's own search box sat.
pub fn layout_ask(ui: &BalloonUi, answer: &AskAnswer, per_line: usize) -> AskLayout {
    // Below ~8 columns a prefixed row has no room for text at all.
    let per_line = per_line.max(8);
    let mut rows: Vec<AskRow> = Vec::new();

    if let Some(heading) = &ui.heading {
        push_wrapped(
            &mut rows,
            heading,
            RowMarker::None,
            AskRole::Heading,
            per_line,
        );
    }
    if !ui.text.trim().is_empty() {
        push_wrapped(
            &mut rows,
            &ui.text,
            RowMarker::None,
            AskRole::Text,
            per_line,
        );
    }
    for (i, label) in ui.choices.iter().take(MAX_ITEMS).enumerate() {
        let marker = choice_marker(ui.style, i);
        push_wrapped(&mut rows, label, marker, AskRole::Choice(i), per_line);
    }
    for (i, label) in ui.checkboxes.iter().take(MAX_ITEMS).enumerate() {
        let marker = RowMarker::CheckBox(answer.checked & (1 << i) != 0);
        push_wrapped(&mut rows, label, marker, AskRole::CheckBox(i), per_line);
    }

    // The text field is one row whatever its contents: it scrolls horizontally rather than
    // wrapping, so the balloon doesn't resize itself out from under the typist.
    let input = ui.input.as_ref().map(|field| {
        let view = InputView {
            value: answer.text.clone(),
            prompt: field.placeholder.clone(),
            caret: answer.caret_char(),
            selection: answer.selection(),
            row: rows.len(),
        };
        rows.push(AskRow {
            // The ASCII fallback has no focus to speak of, so it shows the unfocused text.
            text: view.display(false).to_string(),
            role: AskRole::Input,
            marker: RowMarker::None,
            indent: 0,
        });
        view
    });

    let buttons = ui.buttons.buttons().to_vec();
    if !buttons.is_empty() {
        // The button row's text is the ASCII fallback rendering; a real renderer draws
        // buttons from `AskLayout::buttons` instead and ignores this.
        rows.push(AskRow {
            text: buttons
                .iter()
                .map(|b| format!("[ {} ]", b.label()))
                .collect::<Vec<_>>()
                .join("  "),
            role: AskRole::Buttons,
            marker: RowMarker::None,
            indent: 0,
        });
    }

    let cols = rows
        .iter()
        .map(|r| r.line().chars().count())
        .max()
        .unwrap_or(0);
    AskLayout {
        rows,
        cols,
        buttons,
        input,
        style: ui.style,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lays_out_in_office_order() {
        let ui = BalloonUi::new("Pick one")
            .heading("Orientation")
            .choice("Portrait")
            .choice("Landscape")
            .checkbox("Remember this")
            .buttons(ButtonSet::OkCancel);
        let l = layout_ask(&ui, &AskAnswer::default(), 40);
        let roles: Vec<AskRole> = l.rows.iter().map(|r| r.role).collect();
        assert_eq!(
            roles,
            vec![
                AskRole::Heading,
                AskRole::Text,
                AskRole::Choice(0),
                AskRole::Choice(1),
                AskRole::CheckBox(0),
                AskRole::Buttons,
            ]
        );
        assert_eq!(l.buttons, vec![Button::Ok, Button::Cancel]);
    }

    #[test]
    fn the_marker_is_data_and_the_text_is_just_the_label() {
        let ui = BalloonUi::new("")
            .choice("First")
            .choice("Second")
            .style(ChoiceStyle::Numbers);
        let l = layout_ask(&ui, &AskAnswer::default(), 40);
        // The label carries no marker — a renderer draws one from `marker`...
        assert_eq!(l.rows[0].text, "First");
        assert_eq!(l.rows[0].marker, RowMarker::Number(1));
        assert_eq!(l.rows[1].marker, RowMarker::Number(2));
        // ...and a chrome-free host gets the ASCII stand-in.
        assert_eq!(l.lines(), vec!["1. First", "2. Second"]);
    }

    #[test]
    fn checkbox_state_comes_from_the_mask() {
        let ui = BalloonUi::new("").checkbox("A").checkbox("B");
        let l = layout_ask(&ui, &AskAnswer::checked(0b10), 40);
        assert_eq!(l.rows[0].marker, RowMarker::CheckBox(false));
        assert_eq!(l.rows[1].marker, RowMarker::CheckBox(true));
        assert_eq!(l.lines(), vec!["[ ] A", "[x] B"]);
    }

    #[test]
    fn wrapped_choice_keeps_its_role_and_indents() {
        let l = layout_ask(
            &BalloonUi::new("")
                .choice("one two three four five six")
                .style(ChoiceStyle::Numbers),
            &AskAnswer::default(),
            12,
        );
        assert!(l.rows.len() > 1, "should wrap: {:?}", l.rows);
        assert!(l.rows.iter().all(|r| r.role == AskRole::Choice(0)));
        // Only the first row is marked; continuation rows align under the text, not the
        // marker — in the data (`RowMarker::None` + the same indent) and in the fallback.
        assert_eq!(l.rows[0].marker, RowMarker::Number(1));
        assert_eq!(l.rows[1].marker, RowMarker::None);
        assert_eq!(l.rows[1].indent, l.rows[0].indent);
        let lines = l.lines();
        assert!(lines[0].starts_with("1. "));
        assert!(lines[1].starts_with("   ") && !lines[1].starts_with("1."));
    }

    #[test]
    fn caps_at_five_items() {
        let mut ui = BalloonUi::new("");
        for i in 0..8 {
            ui = ui.choice(format!("c{i}")).checkbox(format!("b{i}"));
        }
        assert_eq!(ui.choices.len(), MAX_ITEMS);
        assert_eq!(ui.checkboxes.len(), MAX_ITEMS);
        // A struct built by hand can still exceed the cap; the layout drops the excess.
        let over = BalloonUi {
            choices: (0..9).map(|i| format!("c{i}")).collect(),
            ..Default::default()
        };
        let l = layout_ask(&over, &AskAnswer::default(), 40);
        assert_eq!(l.rows.len(), MAX_ITEMS);
    }

    #[test]
    fn the_text_field_sits_between_the_check_boxes_and_the_buttons() {
        let ui = BalloonUi::new("What would you like to do?")
            .choice("Print")
            .checkbox("Search help too")
            .input("Type your question here")
            .buttons(ButtonSet::SearchClose);
        let l = layout_ask(&ui, &AskAnswer::default(), 40);
        let roles: Vec<AskRole> = l.rows.iter().map(|r| r.role).collect();
        assert_eq!(
            roles,
            vec![
                AskRole::Text,
                AskRole::Choice(0),
                AskRole::CheckBox(0),
                AskRole::Input,
                AskRole::Buttons,
            ]
        );
        assert_eq!(l.buttons, vec![Button::Search, Button::Close]);
    }

    #[test]
    fn the_placeholder_shows_until_the_field_is_focused_or_filled() {
        let ui = BalloonUi::new("").input("Type your question here");
        let view = layout_ask(&ui, &AskAnswer::default(), 40).input.unwrap();

        // Empty and unfocused: the hint is doing its job.
        assert!(view.shows_prompt(false));
        assert_eq!(view.display(false), "Type your question here");
        // Focused: the hint gives way, so a caret has somewhere to sit.
        assert!(!view.shows_prompt(true));
        assert_eq!(view.display(true), "");

        let typed = AskAnswer::at("mail merge", 4);
        let view = layout_ask(&ui, &typed, 40).input.unwrap();
        // With a value there is no hint either way.
        assert!(!view.shows_prompt(false) && !view.shows_prompt(true));
        assert_eq!(view.display(false), "mail merge");
        assert_eq!(view.value, "mail merge");
        assert_eq!(view.caret, 4);
    }

    #[test]
    fn a_prefilled_field_starts_with_the_caret_at_the_end() {
        let ui = BalloonUi::new("").input_with("Search", "Resume");
        let answer = AskAnswer::for_question(&ui);
        assert_eq!(answer.text, "Resume");
        assert_eq!(answer.caret, 6);
        assert_eq!(answer.caret_byte(), 6);
    }

    #[test]
    fn the_caret_never_splits_a_multibyte_char() {
        // Four chars, seven bytes — a byte-indexed caret would slice mid-character.
        let answer = AskAnswer::at("héllo", 2);
        assert_eq!(answer.caret_byte(), 3);
        assert!(answer.text.is_char_boundary(answer.caret_byte()));

        // Past the end, it clamps rather than panicking.
        let past = AskAnswer::at("hé", 99);
        assert_eq!(past.caret_char(), 2);
        assert_eq!(past.caret_byte(), past.text.len());
    }

    #[test]
    fn word_runs_are_classified_by_what_you_clicked_in() {
        let t = "hello, big  world";
        assert_eq!(word_at(t, 0), (0, 5), "a word");
        assert_eq!(word_at(t, 5), (5, 6), "the comma, alone");
        assert_eq!(word_at(t, 6), (6, 7), "the space between");
        assert_eq!(word_at(t, 8), (7, 10), "'big'");
        assert_eq!(word_at(t, 10), (10, 12), "the double space, as one run");
        // Clicking past the end selects the run that ends there rather than panicking.
        assert_eq!(word_at(t, 99), (12, 17));
        assert_eq!(word_at("", 3), (0, 0));
    }

    #[test]
    fn word_movement_skips_whitespace_with_the_word() {
        let t = "hello big  world";
        assert_eq!(word_right(t, 0), 6, "past 'hello' and its space");
        assert_eq!(word_right(t, 6), 11, "past 'big' and both spaces");
        assert_eq!(word_right(t, 11), 16);
        assert_eq!(word_right(t, 16), 16, "already at the end");

        assert_eq!(word_left(t, 16), 11);
        assert_eq!(word_left(t, 11), 6);
        assert_eq!(word_left(t, 6), 0);
        assert_eq!(word_left(t, 0), 0);
    }

    #[test]
    fn a_selection_normalises_whichever_way_it_was_drawn() {
        let mut a = AskAnswer::at("hello world", 0);
        assert_eq!(a.selection(), None, "a bare caret selects nothing");

        a.set_caret(5);
        a.select_to(2);
        assert_eq!(a.selection(), Some((2, 5)), "drawn backwards");
        assert_eq!(a.selected_text(), "llo");
        assert_eq!(a.caret, 2, "the caret is the moving end");

        a.set_caret(0);
        assert_eq!(a.selection(), None, "placing the caret collapses it");
    }

    #[test]
    fn deleting_a_selection_leaves_the_caret_at_its_start() {
        let mut a = AskAnswer::at("hello world", 0);
        a.set_caret(5);
        a.select_to(11);
        assert!(a.delete_selection());
        assert_eq!(a.text, "hello");
        assert_eq!(a.caret, 5);
        assert_eq!(a.selection(), None);
        assert!(!a.delete_selection(), "nothing left to delete");
    }

    #[test]
    fn a_long_value_stays_on_one_row() {
        // The field scrolls rather than wrapping, so the balloon doesn't grow as you type.
        let ui = BalloonUi::new("").input("Search");
        let answer = AskAnswer::at(
            "a very long question that would wrap over several lines if it could",
            0,
        );
        let l = layout_ask(&ui, &answer, 16);
        assert_eq!(
            l.rows.iter().filter(|r| r.role == AskRole::Input).count(),
            1
        );
    }

    #[test]
    fn no_buttons_means_no_button_row() {
        let ui = BalloonUi::new("Hi").choice("Yes");
        let l = layout_ask(&ui, &AskAnswer::default(), 40);
        assert!(l.buttons.is_empty());
        assert!(!l.rows.iter().any(|r| r.role == AskRole::Buttons));
    }

    #[test]
    fn empty_question_lays_out_to_nothing() {
        let l = layout_ask(&BalloonUi::default(), &AskAnswer::default(), 40);
        assert!(l.rows.is_empty());
        assert_eq!(l.cols, 0);
    }
}
