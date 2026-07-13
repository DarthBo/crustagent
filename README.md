# crustagent

Use classic **Microsoft Agent** characters — *Clippy, Merlin, Genie, Peedy, Robby* — in
modern, cross-platform apps, from safe **Rust**.

crustagent reads the original `.acs` character files (and, over time, `.acf`/`.acd`) and
gives you their palettes, animations, frames, sounds and speech markup as clean Rust
types — plus a portable runtime to sequence and play them. No Windows, no COM, no SAPI, no
DirectShow. The aim isn't to re-clone the old desktop assistant wholesale; it's to make
these lovingly-made characters usable again wherever Rust runs.

## Lineage

- **Microsoft Agent** (late-'90s/early-2000s) is the OG — the technology and the `.acs`
  format we target.
- **[DoubleAgent](https://sourceforge.net/projects/doubleagent/)** (Cinnamon Software) is a
  faithful open-source Windows/C++ reimplementation. We used its source as the reference
  for reverse-engineering the byte formats and playback behavior — huge thanks to it.
- **crustagent** is a from-scratch, platform-independent Rust take: a reimplementation of a
  reimplementation, aimed at modern apps rather than at reproducing every Windows detail.

## Workspace layout

```
crates/
  crustagent-format/   # pure, dependency-free parsers for the character file formats
  crustagent-core/     # portable animation runtime (sequencing, branching, timing, text)
  crustagent-gif/      # dependency-free animated GIF89a encoder (round-trip tested)
```

Planned crates: the rest of `crustagent-core` (action queue + state machine + idle/move),
`crustagent-render` (per-pixel-alpha surface), and audio/TTS/host layers as needed.

## `crustagent-format` — status

Implemented:
- **ACS 2.0** (`AcsFile`) — the compiled binary format: header, palette, TTS/balloon
  metadata, names (with language preference), states, gestures→animations→frames
  (images, overlays, branching), the LZ77 image bitstream **decompressor**, raw WAV
  sound extraction, and a **frame compositor** to RGBA/indexed.

Not yet (nice-to-have): ACS 1.5 (OLE2 compound document), ACF (+ external `.aca`), ACD
(text script).

## `crustagent-core` — status

Implemented:
- **Sequence builder** (`sequence_animation`) — flattens an animation's branching frame
  graph into a linear, timed `AnimationSequence`, with deterministic (injectable) branch
  RNG, loop detection, and runaway-loop guards; plus `sequence_exit` for return-to-neutral.
- **Player** — drives a sequence against a monotonic clock, handling looping and
  completion; ask it which frame is on screen at time *t*.
- **Character** — name/state → animation resolution (case-insensitive) over a parsed
  file, incl. the multi-part gesture convention (`full_gesture` chains a gesture's base +
  `…Continued` + `…Return` parts).
- **Speech-text parser** (`parse_speech`) — splits a `Speak` string into balloon display
  words and a neutral speech-directive stream (all 23 tags, `\Map` dual text, `\Mrk`
  bookmarks, `\\`/`\"` escaping).

Not yet: action queue, idle escalation, move interpolation.

## Try it

Character files are third-party; drop your own into `assets/agents/` (see
`assets/README.md`). Then:

```sh
cargo test
cargo run -p crustagent-format --example dump     -- assets/agents/Merlin.acs
cargo run -p crustagent-format --example render   -- assets/agents/Merlin.acs Greet 0   # one frame -> PNG
cargo run -p crustagent-core   --example sequence -- assets/agents/Merlin.acs Greet     # print the timeline
cargo run -p crustagent-core   --example gif      -- assets/agents/Merlin.acs GetAttention  # gesture -> GIF
```

## Provenance & license

The `.acs` format and the character artwork belong to Microsoft and the original character
authors; **no character assets are included in this repository**. crustagent's format and
behavior were reverse-engineered from — and in places ported from — the
[DoubleAgent](https://sourceforge.net/projects/doubleagent/) source (Cinnamon Software
Inc.), which is GPL/LGPL. As a derivative work, crustagent is licensed **GPL-3.0-or-later**
(see [`LICENSE`](LICENSE)); attribution and third-party notices are in [`NOTICE`](NOTICE).
