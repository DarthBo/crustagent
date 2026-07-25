# Microsoft Agent `Speak()` Output-Tag (Markup) Grammar

A reference for the speech-output markup that Microsoft Agent's `Speak` method accepts in its
`Text` parameter — the `\Tag\` / `\Tag=value\` sequences that control text-to-speech (TTS) output
and split the spoken audio from the word-balloon display. This document is written so a later
clean-room session can implement crustagent's speech-markup parser from it alone.

> Microsoft Agent is deprecated as of Windows 7. The documentation cited below remains published on
> Microsoft Learn and is the authoritative source for this grammar.

## Provenance & method

Every rule below is derived from **Microsoft's own published documentation** and cross-checked
against **Microsoft's own binary**:

- **Microsoft Agent SDK / MSDN reference** (public API documentation), primarily:
  - *Microsoft Agent Speech Output Tags* — the syntax rules and the list of supported tags.
    <https://learn.microsoft.com/en-us/windows/win32/lwef/microsoft-agent-speech-output-tags>
  - The per-tag pages: *Chr, Ctx, Emp, Lst, Map, Mrk, Pau, Pit, Rst, Spd, Vol Tag*
    (`.../lwef/chr-tag` … `.../lwef/vol-tag`).
  - *Speak Method* — text/URL parameters, balloon vs audio, alternative strings, bookmarks.
    <https://learn.microsoft.com/en-us/windows/win32/lwef/speak-method>
  - Raw markdown source of the above: `github.com/MicrosoftDocs/win32` under
    `desktop-src/lwef/*-tag.md`.
- **`AgentSvr.exe`** (Microsoft Agent 2.0 server, build 3422), decompiled with Ghidra, used only to
  *confirm* behavior the docs describe loosely — the exact tag set, the separator character, the
  escape handling, the `Map` parse, and how tags are forwarded to the TTS engine. Cited by their
  Ghidra `FUN_<address>` labels (image base `0x01000000`).

**Clean-room boundary.** This document traces only to Microsoft. It was written **without** reading
`crates/crustagent-core/src/text.rs` (the code being replaced) or any third-party Microsoft Agent
reimplementation. Where a behavior is *not* stated in Microsoft's documentation but was established
from the binary or is otherwise inferred, it is tagged **(INFERRED)** with its basis.

Conventions: a literal backslash is written `\`. "Spoken stream" = the text handed to the TTS
engine (or the audio); "balloon" = the character's on-screen word balloon.

---

## 1. General tag syntax

Source: *Microsoft Agent Speech Output Tags* (syntax-rules section), confirmed in
`AgentSvr.exe!FUN_0100d323` (tag dispatch) and `FUN_0100d62b` (generic `\Tag=value\` parser).

### 1.1 Delimiters and forms

Every tag is delimited by a backslash at **both** ends. A tag takes one of two forms:

```
\Tag\            ; parameterless   (e.g. \Emp\, \Rst\, \Lst\)
\Tag=value\      ; parameterized   (e.g. \Pit=100\, \Chr=Whisper\, \Mrk=5\)
```

- The **separator is the equals sign `=`** and nothing else. Microsoft Agent's parser tests only
  for `=` (`U+003D`) after the tag name; it does **not** accept a semicolon. A `\Tag;value\` form is
  **not** part of Microsoft Agent — if a reimplementation supports `;`, that is a non-Microsoft
  extension, not documented or implemented by the Agent server. (Confirmed: `FUN_0100d62b` and the
  `Map` parser `FUN_0100d698` compare against `0x3d` `=` only; there is no `0x3b` `;` test in the
  tag path.)
- The **closing backslash is required.** A parameterless tag is exactly `\Tag\`; a parameterized
  tag is `\Tag=value\`, the value running up to the closing `\`.
- The tag name is a fixed 3-letter code (§4). Use **single-byte characters** for the tag name, the
  separator, and the delimiters even when the surrounding text is double-byte (DBCS). Double-byte
  characters may appear only inside quoted string *values* (per the SDK's DBCS note).

### 1.2 Case-insensitivity and whitespace

- **Tag names are case-insensitive**: `\pit\` is identical to `\PIT\` (SDK). The server lowercases
  before matching — its internal tag table stores the names lowercased (`\chr`, `\ctx`, `\emp`,
  `\lst`, `\map`, `\mrk`, `\pau`, `\pit`, `\rst`, `\spd`, `\vol`; observed in `AgentSvr.exe`).
- **Tags are whitespace-dependent**: `\Rst\` is **not** the same as `\ Rst \` (SDK). No spaces are
  permitted between the delimiters and the tag name/separator/value.

### 1.3 Text escapes

- **`\\` → `\`** — because a single backslash always begins a tag, a *literal* backslash in ordinary
  text (or in a tag's text parameter) must be written as a double backslash. This is the only
  general-text escape defined by Microsoft ("The single backslash character is not enabled within a
  tag. To include a backslash character in a text parameter of a tag, use a double backslash").
  Confirmed in `AgentSvr.exe!FUN_01006b29`, which collapses each `\\` to one `\` (and in the
  balloon-text pass `FUN_0100d0d8`).
- **There is no `\"` text escape.** The `\"` seen in Microsoft's C/C++ code samples is *C
  source-string* escaping for the compiler, not Agent markup. At the markup level a double quote is
  literal except inside a `Map` quoted string, where it is escaped by **doubling** it — see §2.
- A literal double quote inside a `Map` parameter is written `""` (SDK; §2). (INFERRED that `""`
  is the *only* in-markup way to represent a quote, from the `Map` parser's terminator logic in
  `FUN_0100d698`, §2.)

### 1.4 Unrecognized tags

Microsoft's documentation does not define what happens to a backslash sequence that is not one of
the known tags. **(INFERRED)** From the parser structure (`FUN_0100d323` dispatches only the fixed
set of §4; the tokenizer matches those specific `\tag` prefixes), a `\x` that matches no known tag
is not consumed as a tag and remains part of the text stream — i.e. an unrecognized `\x` is treated
as literal text. Treat this as observed parser behavior, not a documented guarantee.

### 1.5 Scope and reset

Unless changed by another tag, a setting established by a tag persists for the remainder of the text
in a **single** `Speak` call. Speech output is **automatically reset to the character's defaults
after each `Speak` completes** (SDK). `\Rst\` resets all tags to their defaults mid-string (§4);
in the server this re-emits the default control tags to the engine — e.g. `\vol=4294967295\`
(`0xFFFFFFFF` = engine default), `\ctx="unknown"\`, `\chr="normal"\` (`AgentSvr.exe!FUN_0100d794`).

---

## 2. The `Map` construct — display one thing, speak another

Source: *Map Tag* (`.../lwef/map-tag`), confirmed in `AgentSvr.exe!FUN_0100d698`.

```
\Map="spokentext"="balloontext"\
```

- **Description (verbatim):** "Maps spoken text to text displayed in the word balloon."
- **Parameter order:** the **first** quoted string is the **spoken** text; the **second** quoted
  string is the **balloon** (displayed) text. (Microsoft's own example is
  `\map="Spoken text"="Balloon text"\`.) This lets the balloon show something different from what is
  spoken — e.g. `\Map="Doctor"="Dr."\` speaks "Doctor" while the balloon shows "Dr.".
- **Quoting.** Each parameter is wrapped in double quotes. The parser (`FUN_0100d698`) reads:
  1. `\map`, then `=`, then the opening `"`.
  2. the **spoken** text up to the 2-character sequence `"=` (quote-then-equals), which closes the
     first parameter and introduces the second.
  3. the opening `"` of the second parameter, then the **balloon** text up to the 2-character
     sequence `"\` (quote-then-backslash), which closes the tag.
- **In-quote escape `""`.** Because a parameter ends only at `"=` (spoken) or `"\` (balloon), a bare
  `"` that is *not* followed by `=` or `\` does not terminate the parameter. Doubling a quote
  (`""`) therefore embeds a literal quote inside either parameter. (SDK notes the `""` doubling for
  quoting-sensitive host languages; the terminator logic in `FUN_0100d698` is what makes it work at
  the markup level — **INFERRED** that this is the intended escape.)
- **The spoken half feeds the speech stream.** The `balloontext` is what the balloon displays
  verbatim; the `spokentext` is what is sent onward as the spoken output, so engine-level speech
  content there is honored. Whether *further* `\...\` tags nested inside the `spokentext` parameter
  are re-parsed is **not specified** by Microsoft's docs and the server does not visibly recurse in
  `FUN_0100d698` — treat nested-tag processing inside `Map` as unspecified **(INFERRED)**.
- **Works with recorded audio.** Along with `Mrk`, `Map` is one of only two tags usable with
  sound-file (`.WAV`/`.LWV`) output, not just TTS (SDK: "Only the Mrk and Map tags can be used with
  sound file-based spoken output").

Host-language escaping (informative; not part of the markup itself). In VBScript the quotes are
doubled at the language level; in C/C++/Java the backslashes and quotes are escaped for the
compiler. Microsoft's examples:

```
' VBScript  ->  markup:  This is \map="Spoken text"="Balloon text"\.
Agent1.Characters("Genie").Speak "This is \map=" + chr(34) + "Spoken text" _
  + chr(34) + "=" + chr(34) + "Balloon text" + chr(34) + "\."

// C/C++  ->  markup:  This is \map="Spoken text"="Balloon text"\
BSTR bszSpeak = SysAllocString(L"This is \\map=\"Spoken text\"=\"Balloon text\"\\");
```

---

## 3. `Mrk` bookmarks — a speech-stream event with no displayed text

Source: *Mrk Tag* (`.../lwef/mrk-tag`).

```
\Mrk=number\
```

- **Description (verbatim):** "Defines a bookmark in the spoken text." The `number` is a **Long
  integer** that identifies the bookmark.
- **Event, no text.** "When the server processes a bookmark, it generates a bookmark event." The
  bookmark produces **no displayed balloon text and no audio** — it is purely a synchronization
  event fired as the spoken stream reaches that point. Applications handle it via the character's
  `Bookmark` event (`BookmarkID`), e.g. `Genie.Speak("And here \mrk=100\it is.")` fires
  `Agent1_Bookmark(100)` (Speak Method page).
- **Value constraints (verbatim):** "You must specify a number greater than zero (0) and not equal
  to 2147483647 or 2147483646." (I.e. `1 … 0x7FFFFFFD`; `0x7FFFFFFF` and `0x7FFFFFFE` are reserved.)
- **Works with recorded audio** (one of the two sound-file-compatible tags; see §2).
- `\Lst\` repeats a prior statement **except** its bookmarks (§4).

---

## 4. Tag reference (complete Microsoft Agent set)

These eleven tags are the complete supported set per *Microsoft Agent Speech Output Tags*. Numeric
value ranges for the TTS parameters "may vary depending on the installed TTS engine" (SDK) — Agent
forwards them to the engine (Appendix B). "Affects" = whether the tag changes the **balloon** text,
the **spoken** stream, or is a **stream event**.

| Tag | Syntax | Parameter (type / values) | Meaning | Affects | Works w/o TTS? |
|-----|--------|---------------------------|---------|---------|----------------|
| **Chr** | `\Chr=string\` | `"Normal"` (default), `"Monotone"`, `"Whisper"` | Sets the *character* (tone) of the voice. | spoken | TTS only |
| **Ctx** | `\Ctx=string\` | `"Address"`, `"E-mail"`, `"Unknown"` (default) | Sets *context* used to normalize symbols/abbreviations (e.g. how addresses vs e-mail are read). | spoken | TTS only |
| **Emp** | `\Emp\` | *(none)* | Emphasizes the **next** word. **Must immediately precede** that word. | spoken | TTS only |
| **Lst** | `\Lst\` | *(none)* | Repeats the character's **last** spoken statement. Must appear **by itself** in the `Speak` call (no other text/params). Repeats the original tags and any `.WAV`/`.LWV`, **except bookmarks**. | spoken | see note¹ |
| **Map** | `\Map="spoken"="balloon"\` | two quoted strings | Speaks one string, displays another (§2). | spoken **and** balloon | **yes** |
| **Mrk** | `\Mrk=number\` | Long, `1 … 0x7FFFFFFD` (not `0x7FFFFFFE`/`0x7FFFFFFF`) | Bookmark; fires a `Bookmark` event (§3). No text. | stream event | **yes** |
| **Pau** | `\Pau=number\` | milliseconds | Pauses speech for `number` ms. | spoken | TTS only |
| **Pit** | `\Pit=number\` | hertz | Sets the baseline **pitch**. | spoken | TTS only |
| **Rst** | `\Rst\` | *(none)* | **Resets** all tags to default settings (§1.5). | spoken | TTS only |
| **Spd** | `\Spd=number\` | words per minute | Sets the baseline average talking **speed**. | spoken | TTS only |
| **Vol** | `\Vol=number\` | `0`–`65535` (0 = silence, 65535 = max; both channels) | Sets the baseline **volume**. Cannot set channels separately. | spoken | TTS only |

Per-tag sources: `chr-tag`, `ctx-tag`, `emp-tag`, `lst-tag`, `map-tag`, `mrk-tag`, `pau-tag`,
`pit-tag`, `rst-tag`, `spd-tag`, `vol-tag` on Microsoft Learn (`.../windows/win32/lwef/`).

¹ `Lst` and `Map`/`Mrk` interact with recorded audio: `Map` and `Mrk` are explicitly usable with
sound-file output; `Lst` replays whatever the last statement was (TTS and/or `.WAV`/`.LWV`).
All other tags are documented as **"supported only for TTS-generated output."**

**SAPI generation.** All eleven map to **SAPI 4** text-control tags — the TTS generation Microsoft
Agent 2.0 targets — and Agent forwards the voice-shaping ones to the engine in the same backslash
form (`\Pit=N\`, `\Vol=N\`, `\Chr="…"\`, …). See **Appendix B** for the per-tag SAPI 4 form and the
conceptual SAPI 5 XML equivalent (`<pitch>`, `<volume>`, `<bookmark>`, …).

Notes on individual parameters:
- **Chr / Ctx values.** The docs give these string sets; a specific TTS engine may accept more or
  fewer. The server forwards them to the engine as `\chr="…"\` / `\ctx="…"\` (Appendix B), so the
  value is a string (case per engine).
- **Pit / Spd / Pau / Vol.** The docs give the *unit* (hertz / words-per-minute / milliseconds /
  0–65535) but state the numeric range is engine-dependent.
- **Emp** applies to exactly one following word and takes no value.

---

## 5. Tags that are **not** part of Microsoft Agent

The task brief asked specifically about pronunciation tags — `Prn` and possible `Pra`/`Pro`/`Prt`
variants. **None of these are Microsoft Agent speech-output tags.**

- **`Prn` (and `Pra`, `Pro`, `Prt`) — NOT Agent tags (INFERRED / confirmed absent).** They do not
  appear in *Microsoft Agent Speech Output Tags* (whose complete list is the eleven of §4), and
  **none of them exist in `AgentSvr.exe`'s tag table or dispatch** (`FUN_0100d323`); the server's
  tag set is exactly `{chr, ctx, emp, lst, map, mrk, pau, pit, rst, spd, vol}`. `Prn` ("Pronounce")
  is a tag of the **Microsoft Speech SDK (SAPI)**, not of Microsoft Agent. The Agent docs state
  this explicitly: *"Microsoft Agent does not support all the tags documented in the Microsoft
  Speech SDK. Parameters may also vary depending on the TTS engine selected."* `Pra`/`Pro`/`Prt`
  were not found in the Agent documentation or the Agent binary — **do not implement them as Agent
  tags** (mark "observed only" if a real sample ever exhibits one).
- **Engine passthrough.** Because Agent forwards its recognized control tags to the SAPI 4 engine as
  backslash control tags (Appendix B), a SAPI-only tag such as `\Prn=…\` embedded in text is not
  recognized by Agent and would, at most, reach the engine only if the engine itself parses it —
  behavior that is engine-specific and outside Microsoft Agent's grammar. Treat any `Pr*` tag as
  **out of scope** for the Agent markup parser unless a Microsoft-authored sample forces otherwise.

---

## 6. Related `Speak` `Text` features (not output tags)

A parser for the `Text` argument will meet these Microsoft-documented features that are **not**
`\...\` output tags but share the same string (Speak Method page). A reimplementation should decide
where to handle them:

- **Alternative strings — `|`.** Vertical-bar characters partition the `Text` into alternatives;
  the server **randomly chooses one** each time it processes the `Speak` method. (SDK: "include
  vertical bar characters (|) … so that the server randomly chooses a different string each time.")
- **Word breaks.** The balloon breaks lines on whitespace (space/tab). For scripts without spaces
  (Japanese, Chinese, Thai), insert a Unicode **zero-width space `U+200B`** to mark logical word
  breaks.
- **Recorded audio via `Url`.** The separate `Url` argument names a `.WAV` or `.LWV` file spoken
  instead of TTS; with `.LWV`, if `Text` is omitted the balloon uses text stored in the file. Only
  `Map` and `Mrk` tags apply to sound-file output.
- **Balloon gating.** Text displays only if the word balloon's `Enabled` property is `True`; set the
  character `LanguageID` before `Speak` for correct balloon text.

---

## Appendix A — Binary cross-check (`AgentSvr.exe`, build 3422)

Confirmations used above (Ghidra labels; image base `0x01000000`):

| Function | Role | What it confirms |
|----------|------|------------------|
| `FUN_0100d323` | Tag dispatcher (switch over tag ids 2–12) | The complete tag set is exactly the eleven of §4; ids map to `\emp\`, `\mrk`, `\pau`, `\pit`, `\ctx`, `\chr`, `\rst\`, `\spd`, `\vol`, `Map`, `\lst\`. |
| `FUN_0100d62b` | Generic `\Tag=value\` parser | Separator is `=` (`0x3d`) only; value runs to the closing `\`. No `;`. |
| `FUN_0100d698` | `Map` parser | `\map="spoken"="balloon"\`; first param = spoken (terminated by `"=`), second = balloon (terminated by `"\`); `""` survives as a literal quote. |
| `FUN_01006b29` | Text unescape | `\\` → `\`; no `\"` handling. |
| `FUN_0100d0d8` | Balloon-text extraction | Scans on `\`; preserves `\\` for the later unescape. |
| `FUN_0100d794` | `\Rst\` handler | Re-emits defaults to the engine: `\vol=4294967295\`, `\ctx="unknown"\`, `\chr="normal"\`, etc. |

The tag-name table in `AgentSvr.exe` stores the names **lowercased** with the leading delimiter
(`\chr`, `\ctx`, …) for parameterized tags and both delimiters (`\emp\`, `\rst\`, `\lst\`) for the
parameterless ones — matching §1.1/§1.2.

## Appendix B — Relationship to SAPI

Microsoft Agent 2.0 drives **SAPI 4** text-to-speech engines (e.g. Microsoft TruVoice,
`mslwvtts.dll`). Its speech-output tags are, in effect, the SAPI 4 **text control tags**: the Agent
server parses the `Text`, peels off the balloon/`Map`/`Mrk`/`Lst`/`Emp` handling it owns, and
**forwards the voice-shaping tags to the engine as backslash control tags** — the reset path
literally builds strings like `\pit=…\`, `\vol=4294967295\`, `\ctx="unknown"\`, `\chr="normal"\`
(`FUN_0100d794`). This is why the numeric ranges are documented as engine-dependent.

Mapping to SAPI generations (informative):

| Agent tag | SAPI 4 control tag (what Agent 2.0 emits) | SAPI 5 XML equivalent (conceptual, not emitted by Agent 2.0) |
|-----------|-------------------------------------------|---------------------------------------------------------------|
| `Pit`     | `\Pit=N\`                                 | `<pitch absmiddle="…">` / `<pitch middle="…">` |
| `Spd`     | `\Spd=N\`                                 | `<rate absspeed="…">` |
| `Vol`     | `\Vol=N\`                                 | `<volume level="…">` |
| `Emp`     | `\Emp\`                                   | `<emph>` |
| `Pau`     | `\Pau=N\`                                 | `<silence msec="N"/>` |
| `Mrk`     | `\Mrk=N\`                                 | `<bookmark mark="N"/>` |
| `Chr`     | `\Chr="…"\`                               | (voice-style; engine-specific) |
| `Ctx`     | `\Ctx="…"\`                               | `<context id="…">` |
| `Rst`     | resets/re-emits defaults                  | (reset by re-issuing defaults) |
| `Map`,`Lst` | handled by the Agent server (not a voice tag) | n/a |

The SAPI 5 column is provided only to orient an implementer; Microsoft Agent 2.0 targets SAPI 4 and
uses the backslash control-tag form throughout. **(INFERRED for the SAPI 5 column — general SAPI
knowledge, not established from the Agent binary.)**

---

## Sources

- Microsoft Learn — *Microsoft Agent Speech Output Tags*:
  <https://learn.microsoft.com/en-us/windows/win32/lwef/microsoft-agent-speech-output-tags>
- Microsoft Learn — *Speak Method*:
  <https://learn.microsoft.com/en-us/windows/win32/lwef/speak-method>
- Microsoft Learn — per-tag pages *Chr/Ctx/Emp/Lst/Map/Mrk/Pau/Pit/Rst/Spd/Vol Tag*:
  `https://learn.microsoft.com/en-us/windows/win32/lwef/{chr,ctx,emp,lst,map,mrk,pau,pit,rst,spd,vol}-tag`
- Raw doc source: `https://github.com/MicrosoftDocs/win32` (`desktop-src/lwef/*-tag.md`).
- Binary cross-check: `AgentSvr.exe` (Microsoft Agent 2.0, build 3422) — functions in Appendix A.
