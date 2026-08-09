# Interactive Balloons and Command Menus

A reference for the *interactive* parts of the two classic character technologies crustagent
reimplements — the Office Assistant's clickable balloon and Microsoft Agent's pop-up command menu —
followed by a proposed mapping onto crustagent's API. This document is written so a later session
can implement the feature from it alone.

> Microsoft Agent is deprecated as of Windows 7; the Office Assistant was removed in Office 2007.
> The documentation cited below remains published on Microsoft Learn (the Office pages in the
> archived Office 2003 reference) and is the authoritative source.

## Provenance & method

**Everything below is derived from Microsoft's published API documentation.** No binary was
disassembled for this document, and no third-party reimplementation was consulted.

That is not a stylistic choice — it is forced by where the feature lives. **Neither the interactive
balloon nor the command menu is stored in the character file.** Both are host-side surfaces:

- The Office Assistant balloon is drawn and driven by `mso.dll`, not by the `.act` character. Our
  own Actor spec already records this — [`act-format.md` §11](act-format.md): *"The speech balloon
  — drawn by the host (Office), not stored in the `.act`."*
- Microsoft Agent's pop-up menu is built by the Agent server from commands the *client application*
  registers at runtime. The `.acs` file contributes nothing to it.

So there is no amount of further `.acs`/`.act` reverse engineering that would reveal this
behavior. The `.acs` format's only balloon contribution is *appearance* — the word-balloon block in
[`acs-format.md` §2.3](acs-format.md) (sizing bytes, fg/bg/border colors, `LOGFONT`), which
`read_balloon` already parses into `format::Balloon`.

A useful side effect: because this traces only to public API documentation, the resulting
implementation carries none of the license-provenance concerns that attach to code derived from
third-party Agent reimplementations.

Sources, all under `learn.microsoft.com`:

- *Balloon Object* — `/previous-versions/office/developer/office-2003/aa171343(v=office.11)`
- *Creating and Modifying Balloons* — `…/aa170979(v=office.11)`
- *BalloonType / Button / Mode / Labels Property*, *Show Method*, *BalloonCheckBoxes Collection* —
  `…/aa210149`, `…/aa210156`, `…/aa210775`, `…/aa210695`, `…/aa171327`, `…/aa171345` `(v=office.11)`
- *The Commands Collection Object*, *Commands Object Properties*, *Command Object Properties*,
  *Command Event* — `/windows/win32/lwef/the-commands-collection-object`,
  `…/commands-object-properties`, `…/command-object-properties`, `…/command-event`

Conventions: **(INFERRED)** marks anything not stated in Microsoft's documentation, with its basis.

---

## 1. Two lineages, two mechanisms

| | Office Assistant (Actor, `.act`) | Microsoft Agent (`.acs`) |
| --- | --- | --- |
| Balloon content | Heading, body text, **labels**, **check boxes**, **buttons** | Text only |
| Balloon interactivity | **Yes** — clickable | **None** |
| Other interactivity | — | Right-click **pop-up command menu**, Voice Commands Window |
| Where defined | Office object model (`Assistant.NewBalloon`) | Agent server + client's `Commands` collection |
| Stored in character file | No | No (only balloon *appearance*) |

The interactive balloon people remember — *"What would you like to do?"* with a list of clickable
options — is the **Office Assistant** one. Microsoft Agent characters never had buttons in the
balloon; their equivalent affordance was the pop-up menu.

---

## 2. The Office Assistant `Balloon` object

Obtained from `Assistant.NewBalloon`. There is no `Balloons` collection — only one balloon is
visible at a time, though several may be defined and shown in turn.

### 2.1 Members

**Properties:** `Animation`, `Application`, `BalloonType`, `Button`, `Callback`, `Checkboxes`,
`Creator`, `Heading`, `Icon`, `Labels`, `Mode`, `Name`, `Parent`, `Private`, `Text`

**Methods:** `Close`, `SetAvoidRectangle`, `Show`

Content order within the balloon, per *Creating and Modifying Balloons*: `Heading` (bold, top) →
`Text` (body) → check boxes / labels → buttons (bottom).

### 2.2 `BalloonType` — `MsoBalloonType`

| Constant | Rendering |
| --- | --- |
| `msoBalloonTypeBullets` | Labels as a bulleted list |
| `msoBalloonTypeButtons` | Labels as clickable buttons (the default for a new balloon) |
| `msoBalloonTypeNumbers` | Labels as a numbered list |

### 2.3 `Labels` → `BalloonLabels` collection

`Labels(index)`, `index` 1–5. A label appears once a value is assigned to its `Text`. **The user's
choice registers as soon as a label is clicked, and the balloon is dismissed immediately.**

`Show` returns the 1-based index of the clicked label.

### 2.4 `Checkboxes` → `BalloonCheckBoxes` collection

`CheckBoxes(index)`, `index` 1–5; more than five raises a run-time error. Each check box appears
once its `Text` is set. Check boxes are **not** self-dismissing: the user toggles them, then clicks
a bottom button to commit. Read results afterward from `CheckBoxes(i).Checked`.

Check boxes cannot be added or removed after the balloon has been displayed.

### 2.5 `Button` — `MsoButtonSetType`

The button row at the bottom. One of:

`msoButtonSetAbortRetryIgnore`, `msoButtonSetBackClose`, `msoButtonSetBackNextClose`,
`msoButtonSetBackNextSnooze`, `msoButtonSetCancel`, `msoButtonSetNextClose`, `msoButtonSetNone`,
`msoButtonSetOK`, `msoButtonSetOkCancel`, `msoButtonSetRetryCancel`, `msoButtonSetSearchClose`,
`msoButtonSetTipsOptionsClose`, `msoButtonSetYesAllNoCancel`, `msoButtonSetYesNo`,
`msoButtonSetYesNoCancel`

### 2.6 `Show` — return value, `MsoBalloonButtonType`

`Show` displays (or refreshes) the balloon; changes made to a balloon only take effect on the next
`Show`. Its return value is **overloaded**:

- Label clicked → the 1-based **label index** (1–5).
- Bottom button clicked → an `MsoBalloonButtonType` constant: `msoBalloonButtonAbort`, `…Back`,
  `…Cancel`, `…Close`, `…Ignore`, `…Next`, `…No`, `…Null`, `…OK`, `…Options`, `…Retry`, `…Search`,
  `…Snooze`, `…Tips`, `…Yes`, `…YesToAll`.

The documented constant values are negative, which is what keeps them disjoint from the 1–5 label
indices. **(INFERRED — the doc pages list the constant names without numeric values; the disjointness
is evident from the `Select Case` examples that switch on both in one statement.)**

### 2.7 `Mode` — `MsoModeType`

| Constant | Behavior |
| --- | --- |
| `msoModeModal` | User must dismiss the balloon before continuing to work |
| `msoModeModeless` | Balloon stays visible while the user works in the application |
| `msoModeAutoDown` | Dismissed when the user clicks anywhere on screen |

Rules: a modeless balloon **must** supply `Callback` (a procedure name) or an error occurs; `Close`
is only valid on modeless balloons, and only from within the `Callback` procedure.

`Show` on a modal balloon blocks and returns the selection; a modeless balloon delivers the
selection to `Callback` instead.

---

## 3. Microsoft Agent's `Commands` collection and pop-up menu

The Agent server maintains one live command list: its own global commands (Hide, Open The Voice
Commands Window), the list of available clients, and the commands of the currently *input-active*
client. Each client app owns a `Commands` collection, populated with `Add` / `Insert`.

### 3.1 Members

**`Commands` collection properties:** `Caption`, `Count`, `DefaultCommand`, `FontName`, `FontSize`,
`GlobalVoiceCommandsEnabled`, `HelpContextID`, `Visible`, `Voice`, `VoiceCaption`

**`Command` object properties:** `Caption`, `Confidence`, `ConfidenceText`, `Enabled`,
`HelpContextID`, `Visible`, `Voice`, `VoiceCaption`

### 3.2 Where a command shows up

Two independent surfaces, selected by which properties are set:

- **Pop-up menu** (right-click the character) — requires `Caption` **and** `Visible = True`.
- **Voice Commands Window** — requires `Voice`, and uses `VoiceCaption` for display (falling back to
  `Caption` when `VoiceCaption` is null, for backward compatibility).

The same rules apply to the `Commands` collection object itself, whose `Caption` places a submenu
entry for the whole client in the character's pop-up menu.

Menu contents are snapshotted at display time: changes to a `Commands` collection while its pop-up
menu is showing do not appear until the user redisplays the menu. (The Voice Commands Window updates
live.)

### 3.3 The `Command` event

`Sub agent_Command(ByVal UserInput)` — fires when the user chooses a client command, by pop-up menu
or by speech. `UserInput` exposes: `CharacterID`, `Name`, `Confidence`, `Voice`, `Count`, plus
`Alt1Name` / `Alt1Confidence` / `Alt1Voice` and `Alt2Name` / `Alt2Confidence` / `Alt2Voice` for the
second- and third-best speech matches.

For a **pop-up menu selection** (our case), the shape is fixed and simple:

- `Name` = the command's ID, `Confidence` = 100, `Voice` = `""`, `Count` = 1
- `Alt1*` / `Alt2*` are empty strings and zeros

For speech input, `Confidence` scores range −100…100, an empty `Name` means the input matched no
client command, and `Count = 0` means speech was detected but matched nothing.

### 3.4 What does not carry over

The input-activation model (`Activate`, `ActivateInput` / `DeActivateInput` /
`ActiveClientChange`) exists to arbitrate *multiple client applications sharing one character*.
crustagent embeds a character in a single host, so this whole layer is out of scope, along with the
speech-recognition half of the `Command` event.

---

## 4. The mapping onto crustagent — as implemented

The two mechanisms unify cleanly: both are *"present a set of choices, get one back."* The Office
balloon is the richer superset, so the implementation models it and treats the Agent pop-up menu as
the same data rendered in a different surface.

### 4.1 Model (`crustagent-core/src/ask.rs`)

The vocabulary maps one-to-one onto §2, and is re-exported from `crustagent` so hosts need not
depend on `crustagent-core`:

```rust
pub enum ChoiceStyle { Buttons, Bullets, Numbers }              // MsoBalloonType (§2.2)
pub enum Button      { Ok, Cancel, Yes, No, Next, Back, Close } // MsoBalloonButtonType (§2.6)
pub enum ButtonSet   { None, Ok, Cancel, OkCancel, YesNo,       // MsoButtonSetType (§2.5)
                       YesNoCancel, NextClose, BackNextClose }
pub enum BalloonMode { Modal, Modeless, AutoDown }              // MsoModeType (§2.7)

pub struct BalloonUi {
    pub heading: Option<String>,
    pub text: String,
    pub choices: Vec<String>,      // ≤ MAX_ITEMS, Office's `Labels`
    pub checkboxes: Vec<String>,   // ≤ MAX_ITEMS, Office's `Checkboxes`
    pub buttons: ButtonSet,
    pub style: ChoiceStyle,
    pub mode: BalloonMode,
}
```

with a builder so a question reads like one: `BalloonUi::new(text).heading(…).choice(…).checkbox(…)
.buttons(ButtonSet::Cancel)`.

**Layout stops at rows, not rectangles.** `layout_ask(&BalloonUi, checked, per_line) -> AskLayout`
wraps the content into `AskRow { text, role, marker, indent }`, where `AskRole` is `Heading` /
`Text` / `Choice(n)` / `CheckBox(n)` / `Buttons`. This keeps `crustagent-core` what it says it is —
pure text math, no fonts, no pixels — and a wrapped choice simply spans several rows carrying the
same role. Row order matches Office's: heading, text, choices, check boxes, button row.

`text` is the **label alone**; the control that belongs in the row's left margin is data, in
`marker: RowMarker` — `Choice` / `Bullet` / `Number(n)` / `CheckBox(checked)` / `None`. A renderer
paints real chrome from it; `AskLayout::lines()` renders the ASCII stand-ins (`* `, `1. `, `[x] `)
for hosts that draw none, which is also what the no-TrueType bitmap path falls back to. Only the
first row of a wrapped control carries a marker — continuations carry `None` and the same `indent`,
so they align under the text rather than under the marker.

Pixel geometry therefore lives one layer up, in `crustagent-balloon`, which already owns font
metrics and padding: `ask_rects` turns an `AskLayout` into clickable `AskRect`s (merging a wrapped
choice's rows into one region, giving each commit button a region hugging its drawn box),
`ask_hit_test` resolves a point against them, and `paint_ask_into` draws from the same metrics — so
the hit map cannot drift from the pixels. `ask_size` sizes the buffer.

This is the one place the implementation departs from the original sketch, which had hit rectangles
hanging off `BalloonLayout` in core. Core would have had to know line heights and padding to produce
them, which is exactly the layering the module set out not to cross.

The existing word-wrap path is untouched: a question is fully present the moment it appears, never
word-paced.

### 4.1.1 Appearance

Following the reference screenshots rather than a literal reading of `MsoBalloonType`: Office's
"buttons" were never framed push-buttons but **marked links** — a small blue radio-style disc and
blue label — with real push-buttons reserved for the commit row. So:

| Element | Drawn as |
| --- | --- |
| `Heading` | Bold — hence `AskFonts { text, bold }`; without a bold face it falls back to the body weight |
| `ChoiceStyle::Buttons` | Accent-coloured radio disc + accent label text |
| `ChoiceStyle::Bullets` / `Numbers` | A dot / the right-aligned number, label still accent-coloured |
| Check box | A rounded, bordered box with a white fill and a two-stroke accent tick when ticked |
| Commit buttons | Rounded, bordered faces with centred labels |

`BalloonPaint` gained `accent` (the link blue) and `face` (button fill) for this, both with
defaults, so existing construction sites only need `..BalloonPaint::default()`. Everything is
antialiased and scale-aware: this is deliberately *not* a pixel clone of the 1997 rendering.

### 4.1.2 Hover, press, and when a click commits

A control commits on **release**, not on press, and only when the release lands on the control the
press armed — so a press can be cancelled by dragging off, the way every other button behaves.

`AskState { hover, pressed }` drives the feedback, and it is **host state, not agent state**: the
agent has no pointer, and hover changes must not churn the event stream. The host tracks it from
`ask_hit_test` on pointer moves and presses, hands it to `paint_ask_into`, and calls
`Agent::report_ask_hit` once, on the committing release. `AskState::phase(hit)` resolves the two
fields into `Phase::Idle` / `Hover` / `Pressed`, including the drag-off rule: a held control drops
back to `Idle` while the pointer is away from it, so it visibly un-presses before the cancel.

Feedback is paint-only and must never move anything — a control that shifted under the pointer
could slide out from under the click committing it.

| Phase | Choice / check box | Commit button |
| --- | --- | --- |
| Hover | Accent-tinted band behind the row; the choice label underlines | Face lightened, border accented |
| Pressed | Stronger tint | Face darkened, border accented, label nudged a pixel down-right |

### 4.2 Request and event (`crustagent`)

`Request::Ask(BalloonUi)` — enqueued via `Agent::ask(question)` — and the answer:

```rust
Event::Answered {
    choice: Option<u8>,     // 1-based choice index, as Office's `Show` returns
    button: Option<Button>, // the commit button, if one was clicked
    checked: u8,            // bitmask of ticked check boxes
}
```

Modeling the answer as indices and a bitmask rather than strings keeps `Event: Copy`, which the enum
already was and which the `drain_events` API depends on. It also mirrors Office's own
`Show`-returns-an-index design, and sidesteps its one wart — the overloaded return (§2.6) — by
splitting choice and button into separate fields instead of relying on disjoint numeric ranges.

The host hit-tests and reports the result: `agent.report_ask_hit(hit)`. The agent deliberately does
*not* hit-test raw coordinates, because it has no font metrics — see §4.1. `Agent::balloon()`
carries the laid-out question in `BalloonView::ask`, and `BalloonView::layout.lines` holds the same
rows as plain text, so a host that draws no chrome at all still shows a readable question.

Supporting surface: `pending_ask()`, `ask_checked()`, and `dismiss_ask()` (takes the question down
*without* raising `Answered`).

### 4.3 Behavioral rules preserved

- A choice click commits **immediately** and dismisses the balloon; a check box toggles and leaves it
  up, its state riding out on the next commit button (§2.3, §2.4).
- `choices` and `checkboxes` are each capped at `MAX_ITEMS` (5). The builder ignores extras and
  `layout_ask` drops them — Office raised a run-time error; we saturate rather than panic.
- A question **never auto-hides**, whatever the character's `auto_hide` style flag says. It waits.
- `Modal` (the default) holds the agent's action queue until the question is answered — Office's
  "the user must dismiss the balloon before continuing to work." `Modeless` and `AutoDown` leave the
  balloon up without holding the queue, so the character carries on with whatever is enqueued.
  `AutoDown` additionally dismisses on the next `report_click`, unanswered.
- `Modeless` needs no `Callback` analogue — the event stream already *is* the callback — which
  removes Office's "modeless without `Callback` is an error" rule.
- `stop()` releases an unanswered question, so a modal one can never wedge the queue.

### 4.4 The command menu — **not implemented**

`crustagent-render`'s right-click menu (`main.rs:42`) remains what it was: a hand-rolled version of
§3, hardcoded to Hide/Speak/Think/Ask plus every gesture, owned entirely by the renderer.

Promoting it to an agent-level API would mean a `Commands`-equivalent the *host* populates —
`Caption` + `Enabled` + `Visible` per entry, plus §3.1's `DefaultCommand` for double-click — with
selections arriving as their own event. It cannot reuse `Event::Answered`: a command is identified
by name, and `Event` is `Copy`, so it would need an interned `CommandId(u64)` handle in the style of
`ReqId`. That is a separate feature about the *pop-up menu*, not the balloon, and is left undone
deliberately.

The `Voice` / `VoiceCaption` half of §3.2 has no counterpart until there is speech input, and should
be left out rather than stubbed.

### 4.5 Deliberately out of scope

Speech recognition and the entire `Alt1*`/`Alt2*` confidence machinery (§3.3); multi-client input
activation (§3.4); Office's `Icon`, `Animation`, `Private`, `SetAvoidRectangle`, and the
Office-specific button sets (`Search`, `Tips`/`Options`, `Snooze`) that exist only to serve Office's
own help UI.

---

## 5. The text field — a crustagent extension

Office's own screenshots show a balloon with a text box and a **Search** button: *"What would you
like to do?"* with a typed question. That balloon was **never reachable from the API**. The
`Balloon` object's full member list (§2.1) has no text-input member at all — `Labels` and
`CheckBoxes` are its only controls. The search box belongs to MSO's built-in help balloon, driven by
`Assistant.Help` and the separate `AnswerWizard` object, which `Assistant.NewBalloon` could not
reproduce.

There is a revealing half-exception: the *buttons* from that balloon were exposed —
`msoBalloonButtonSearch` / `Options` / `Tips` / `Snooze` are all in `MsoBalloonButtonType`, and
`msoButtonSetSearchClose` in `MsoButtonSetType`. A developer could put a Search button on a balloon
and have nothing to search from.

crustagent adds the field, so the pairing finally means something. It is marked as an extension
everywhere it appears (`TextInput`'s doc comment, `Button::Search`) so it is never mistaken for
recovered behaviour.

### 5.1 Model

`BalloonUi.input: Option<TextInput { placeholder, initial }>`, built with `.input(placeholder)` or
`.input_with(placeholder, initial)`. It lays out as one `AskRole::Input` row between the check boxes
and the button row — where the Assistant's own box sat.

The *live* state moved into `AskAnswer { checked, text, caret }`, which the agent owns per question
and `layout_ask` reads; the bare `checked: u8` parameter it replaces was never going to carry a
string. `AskLayout.input: Option<InputView>` carries what to draw: the value (or the placeholder),
whether it *is* the placeholder, the caret, and the prompt — the last so a renderer sizes the box
from the placeholder rather than the value.

**The caret is a char offset, not a byte offset.** `caret_char()` clamps it and `caret_byte()`
converts for slicing, so no edit can split a multi-byte character.

### 5.2 Editing

The agent owns the buffer, as it already owned `checked`, and the host reports intent:

| Host call | Effect |
| --- | --- |
| `report_ask_text(&str)` | Insert at the caret, replacing any selection. Control characters are stripped — Enter and Tab are the host's to interpret and must never land in the buffer. This is also the paste path |
| `report_ask_edit(AskEdit)` | Movement (`Left`/`Right`/`WordLeft`/`WordRight`/`Home`/`End`), deletion (`Backspace`/`Delete`/`DeleteWordBack`/`Clear`), selection (`Select*` + `SelectAll`), and `Undo`/`Redo` |
| `report_ask_caret(usize)` | Place the caret and collapse the selection, e.g. from a click (`ask_caret_at` maps a pixel x to an offset) |
| `report_ask_select_to(usize)` | Extend the selection, keeping its anchor — a drag, or a shifted click |
| `report_ask_select_word(usize)` | Select the run around an offset — a double-click |
| `report_ask_submit()` | What Enter does: answers with the field's contents and the set's **first** button, mirroring the search balloon submitting as *Search* |

### 4.1.3 Button order

Office's constant *names* imply an order — `msoButtonSetOkCancel`, `msoButtonSetSearchClose` —
and the first cut took them literally, putting Search on the left. That is a Windows-era reading,
and it contradicts both the reference screenshots (where *Search* sits right of *Options*) and the
macOS and GNOME HIGs, which put the primary action rightmost.

Platforms genuinely disagree here, so it is a **policy**, not a fact: `BalloonUi.button_order:
ButtonOrder` is `PrimaryFirst` or `PrimaryLast`, defaulting to the host platform's convention
(`PrimaryFirst` on Windows, `PrimaryLast` elsewhere) and overridable per question by the client.

Two things follow:

- `ButtonSet::buttons()` keeps the **semantic** order, primary first, whatever the layout. That is
  what identifies the affirmative action, so `report_ask_submit` still submits `SearchClose` as
  *Search* however it is drawn. `ButtonSet::ordered(order)` gives the drawing order.
- The reordering is **spelled out per set rather than reversed**, because reversing is wrong for
  navigation: `Back  Next  Close` must not become `Close  Next  Back`. Back belongs before Next in
  either layout; it is the auxiliary button that moves, giving `Close  Back  Next`.

The **group itself is right-aligned**, flush with the content's right edge. That part is *not* a
policy — dialog buttons sit bottom-right on every platform; only the order within the group differs.
Because the group's position depends on the final row width, the metrics store each button's offset
from the *start of the group* and the paint and hit-test align it, so the two cannot disagree.

Any test that asserts a drawn button order must pin `button_order` explicitly, or it asserts the
build platform instead.

### 5.2.1 Selection and the clipboard

`AskAnswer.anchor: Option<usize>` is the fixed end of the selection; `caret` is the moving end.
It is an `Option` rather than "equal to `caret` when nothing is selected" because the latter makes
`AskAnswer { text, caret: 9, ..Default::default() }` silently select 0..9 — a trap that fired
immediately in the demo sheet. `AskAnswer::at(text, caret)` and `::selecting(text, anchor, caret)`
are the constructors; `selection()` normalises whichever way the range was drawn.

Selection semantics follow what every text field does: a bare arrow collapses a selection to the
edge it points at rather than stepping off the caret; typing or deleting replaces the selection;
word runs are classified as word-characters / whitespace / punctuation, so a double-click in
`"hello,  big"` takes `hello`, the comma, both spaces, or `big` depending on where it lands.

**The clipboard is the host's, not the agent's.** `crustagent` never touches a clipboard — copy is
`ask_selected_text()` for the host to hand to the system, and cut is that plus
`report_ask_delete_selection()`; paste is just `report_ask_text`. That keeps the whole clipboard
dependency (`arboard`) inside `crustagent-render`, where the platform integration already lives, and
leaves an embedder free to route copy/paste wherever it likes.

Rendering: a selected run is drawn as an accent band with the text inverted to white, in three
measured segments. The caret is **hidden** while a selection is up — the band already says where you
are — which is asserted by a test that renders with `caret_on` both ways and requires the results to
be identical.

`Event::Answered` gained `text: Option<String>` — `Some` exactly when the question had a field.
**This costs `Event` its `Copy`**, which §4.2 previously leaned on; it stays `Clone`, which is all
`drain_events` needs. Indices and the bitmask are kept as they were: only the typed text genuinely
needs an allocation.

Clicking the field is *not* an answer — `AskHit::Input` places the caret and nothing else.

### 5.2.2 Undo and redo

Snapshot-based: `FieldState { text, caret, anchor }` captured *before* each change that actually
alters the text. Deliberately just the field — undoing a typed word should not also un-tick a check
box — and per question, so a fresh question starts with a clean history. Capped at 100 steps.

The interesting part is **what counts as one step**, since a step per keystroke is unusable:

- Consecutive **typing** folds into one step, as do consecutive **deletions**.
- A change of kind starts a new step — typing after deleting, or vice versa.
- A **paste**, a **clear**, and **typing over a selection** each stand alone, because they destroy
  more than a character.
- **Moving the caret ends the run.** Type, click elsewhere, type again is two steps, not one. This
  is why `report_ask_caret` / `report_ask_select_to` / `report_ask_select_word` call `break_run`
  even though they record nothing themselves.
- An **undo followed by an edit** starts fresh rather than folding into the step just restored, and
  abandons the redo branch — the usual rule.

Because a snapshot carries the selection, undoing a replacement restores both the old text *and*
the selection it was typed over, so the next keystroke can replace it again.

`ask_can_undo()` / `ask_can_redo()` are there for a host that wants to grey out a menu item.
`crustagent-render` binds Cmd/Ctrl+Z, with Shift (or Ctrl+Y) for redo.

### 5.3 Rendering

The field draws as a white, bordered, rounded box. **Focus is real state**, on
`AskState.focused` alongside hover and pressed — host state, for the same reason: the agent has no
pointer. A question opens unfocused; clicking the field (or typing into it) focuses it.

| Focus | Empty | With a value |
| --- | --- | --- |
| Unfocused | Dimmed placeholder, quiet border | Value, quiet border |
| Focused | **No placeholder**, caret at the start, accent border | Value + caret, accent border |

The placeholder gives way the moment the field is focused: a hint that survives your click is a
hint that has outstayed its welcome, and it leaves the caret nowhere to sit. `InputView` therefore
keeps `value` and `prompt` as separate fields, with `shows_prompt(focused)` / `display(focused)`
resolving them — an earlier design collapsed them into one `text` plus a `placeholder` flag, which
could not express "focused and empty".

The caret is drawn whenever the field is focused, including when it is empty, and its blink is
driven by the **host** through `AskState.caret_on` — the host owns the frame clock. Focusing or
typing restarts the blink, so the caret is solid the instant it matters rather than half-way
through an off phase.

A value wider than the box **scrolls rather than wraps**, so the balloon never resizes as you type:
`input_scroll` offsets the text to keep the caret in view, and the text is drawn inside a clip rect
(new on `Canvas`) so the overflow is cut off at the box edge instead of spilling across the balloon.

Sizing therefore comes from the placeholder and a `INPUT_MIN_CHARS` floor — never from the value.

### 5.4 Not done

Multi-line fields, and more than one field per balloon (which would need real focus *traversal* —
tab order — rather than the single `AskState.focused` flag). Vertical scrolling, since there is only
ever one line. IME composition (dead keys work, but a pre-edit buffer is not shown inline).

The reference screenshots show a pre-filled value *selected* on open; crustagent places the caret at
the end instead. `AskAnswer::selecting` makes the other choice available to an embedder that wants
it — it just isn't the default.

---

## 6. Not specified here

- **Pixel metrics.** Office's balloon padding, button sizing, label indent, and check-box glyph are
  not documented and were not measured. The renderer derives them from the character's own balloon
  block ([`acs-format.md` §2.3](acs-format.md)) and font metrics, with its own constants for the
  chrome — see §4.1.1. Reviewing them is what `crustagent-render --ask-png` is for; `--hits`
  overlays the click map on the same render.
- **Visited-choice colouring.** The Office Assistant tinted an already-chosen link purple, following
  the web's visited-link convention. Not reproduced: it needs history the agent doesn't keep, and it
  reads oddly in a balloon that is dismissed the moment you answer.
- **Exact numeric values** of the `Mso*` constants (§2.6) — irrelevant to a clean-room
  reimplementation, which needs the semantics, not the ABI.
- **Actor-side balloon appearance.** Unlike `.acs`, the `.act` format carries no balloon block at
  all; an Actor character rendered by crustagent has no file-provided balloon styling to inherit.
