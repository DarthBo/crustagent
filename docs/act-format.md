# Microsoft Actor (`.act`) Character Format

A specification of the **Microsoft Actor** character-table format, reverse-engineered for
`crustagent`. Actor is the mid-1990s interactive-character technology that powered the
**Office 97/98 Assistant** (Clippit, Rover, The Genius, Mother Nature, Will, …) and
**Microsoft Bob**; it is the direct predecessor of Microsoft Agent (`.acs`).

## Provenance

This spec was derived independently, from:

- **Real character files** — validated against both the little-endian Windows Office 97
  Assistants and their big-endian classic-Mac (Office 98) counterparts. (The copyrighted
  character files themselves are not redistributed with this project.)
- **Microsoft's own binaries**, decompiled/inspected:
  - **`Char11.dll`** (Microsoft Agent 1.1 — the Actor/Agent runtime): the container header,
    the LZ77 decompressor, the `MNAK`/`DICK` cel container, the object/pose/frame/action
    tables, and the palette handling. Cited functions use Ghidra's `FUN_<addr>` labels.
  - **`mso97.dll`** (Office object model): the official `MsoAnimationType` enumeration, which
    is the authoritative naming for the action ids.
  - **The Office 98 shared library** (PowerPC): shows the classic-Mac path handing cel bytes
    to **QuickTime** (`DecompressSequenceBegin/Frame` are QuickTime imports) with an
    `ImageDescription` naming the `'smc '` codec.
- **Public standards**: the Aldus Placeable Metafile (WMF) layout, and Apple's QuickTime
  **SMC** codec (as also implemented in FFmpeg's `libavcodec/smc.c`).

All numeric constants and layouts below are facts about the on-disk format.

## 1. Byte-order dialects

Every value's endianness is fixed by the 2-byte signature:

| Signature | Bytes | Dialect | Integers |
|-----------|-------|---------|----------|
| `LP` | `4C 50` | PC (Windows) | little-endian |
| `PL` | `50 4C` | classic Mac (Office 98/2001) | big-endian |

The two dialects are otherwise the same structure, byte-swapped. A handful of fields use
QuickDraw conventions on the Mac (noted inline). "u16"/"u32"/"i16" below mean the
dialect's byte order.

Two format versions exist: **Actor 1.0** (e.g. Rover, Microsoft Bob) reports version `1`,
**Actor 2.0** (e.g. Clippit, The Genius) reports version `2`. They differ only slightly in
the header; both are handled identically here.

## 2. Outer container

A `.act` file is a thin wrapper around one *named section* (the character).

```
offset  size  field
0x00    u16   signature   ("LP" = 0x504C LE, or "PL" = 0x504C BE)
0x02    u16   version     (1 or 2)
0x04    u16   field@4     (a sub-type/flags word: 2 for the PC Assistants, 0 for Rover)
0x06    u16   nsec        (number of named sections; 1 for a character file)
0x08    u16   namelen     (size in bytes of the names block)
0x0A    u32[nsec+1]       section offset table (absolute file offsets; the last entry = EOF)
...     namelen bytes     NUL-separated section names (one per section)
```

For a character file `nsec == 1`: the offset table is `[bodyBase, EOF]`, the single name is
the character name (e.g. `"The Genius"`), and the character body begins at
**`bodyBase = offsetTable[0]`** (typically 24–29).

*Engine ref:* `Char11.dll` `FUN_67e498d2` reads this header; `FUN_67e45f2e` parses the body.

## 3. Character body — the 70-byte char-info header

At `bodyBase` sits a fixed **0x46 (70)-byte** header. Its `u32` fields at `0x26…0x42` are
byte offsets **relative to `bodyBase`** and are the master directory for everything else.

```
rel     type   field
0x00    u16    header size (= 70)
0x02    u16    type/class (= 7 for the shipped characters)
0x04    u16    cel count hint (250 for Clippit; small/unused for MNAK & Mac)
0x08    u16    frame width  in twips (1/1440")   [Mac: height first — see below]
0x0A    u16    frame height in twips
0x0C    u16    frame width  in pixels            [Mac stores (height, width)]
0x0E    u16    frame height in pixels
0x10    u16    constant 2083   ┐ a fixed marker word pair; used to locate the
0x12    u16    constant 2083   ┘ pixel frame size when scanning the header
0x14…0x25      misc (dpi/rects; 0xFFFF fill at 0x20/0x22/0x24)
0x26    u32    → artwork pool start
0x2A    u32    → object directory
0x2E    u32    → sounds region (Windows: embedded RIFF/WAVE pool; Mac: see §8)
0x32    u32    → small table
0x36    u32    → frame-program pool
0x3A    u32    → action table
0x3E    u32    → (state/overlay table)
0x42    u32    → (final table)      ; region N spans [field(N), field(N+1))
```

**Frame size** = the two pixel words before the `2083, 2083` marker. Windows stores them
`(width, height)`; the classic-Mac dialect uses QuickDraw rect order `(height, width)`, so
they are swapped on read.

The artwork pool (`0x26`) begins at the first cel; regions are contiguous and the pool ends
where the object directory begins. The `0x3E`/`0x42` regions feed a *secondary* state/overlay
sequencer the engine builds alongside the main one; they are not needed to render or animate
a character and are not decoded here (§11).

*Engine ref:* `Char11.dll` `FUN_67e45f2e` reads this 70-byte header and builds the tables.

## 4. Artwork encodings

A character uses exactly one of three cel encodings. All three rasterize to 8-bit-per-pixel
or RGBA.

### 4.1 WMF — Aldus Placeable Metafile (vector; Clippit, Rover, Will, …)

Each cel is a standalone **Aldus Placeable Windows Metafile** beginning with the placeable
magic `0x9AC6CDD7`:

```
0x00  u32   0x9AC6CDD7          placeable key
0x04  u16   handle (0)
0x06  i16   bounding box left, top, right, bottom (logical units)
0x0E  u16   inch
0x10  u32   reserved
0x14  u16   checksum
0x16  ...   standard WMF header, then WMF records
```

Cels are located by scanning the artwork pool for the placeable key; each cel's bounding box
is `(right-left+1) × (bottom-top+1)` pixels. Records used by the characters:

| Record | Func | Effect |
|--------|------|--------|
| SetWindowOrg | 0x020B | window origin (y, x) |
| SetWindowExt | 0x020C | window extent (y, x) |
| SetPolyFillMode | 0x0106 | 2 = winding, else even-odd |
| CreatePenIndirect | 0x02FA | occupies a handle slot (outlines not drawn) |
| CreateBrushIndirect | 0x02FC | style + COLORREF fill color |
| SelectObject | 0x012D | select pen/brush by handle |
| DeleteObject | 0x01F0 | free a handle slot |
| Polygon / PolyPolygon | 0x0324 / 0x0538 | filled polygon(s) |
| Polyline | 0x0325 | ignored (fills carry the shapes) |

Points are mapped from the metafile's logical window to the output bitmap as an
**MM_ANISOTROPIC** window→viewport transform. Gotcha: Actor 1.0 cels (Rover, Bob) omit
`SetWindowOrg/Ext` entirely and draw in placeable-bbox space, so the window must **default to
the placeable bounding box** (not `(0,0)`); an explicit `SetWindowOrg/Ext` overrides it (e.g.
Clippit's eye cels draw in a 360×200 window scaled into a ~46×26 box).

### 4.2 MNAK — compressed bitmaps (Windows; The Genius, Mother Nature, Earl, Rocky)

The artwork pool is a run of blocks tagged `MNAK` (the engine also handles `DICK` the same
way). Each block packs several sub-images:

```
0x00  u32   "MNAK"
0x04  u32   uncompressed size
0x08  u32   sub-image count N
0x0C  u32[N-1]  body offsets of sub-images 1..N-1 (into the decompressed buffer)
...   LZ payload  (starts at 0x0C + (N-1)*4)
```

The LZ payload is **the ACS LZ77 bitstream**, decoded to `uncompressed size` bytes
(*engine ref:* `Char11.dll` `FUN_67e472f8`, verified byte-exact against `crustagent`'s
`decode.rs`): a bit-driven copy/backreference scheme — `src[0] == 0` framing, back-reference
distance tiers at `+0x41` / `+0x241` / a 20-bit offset, and a `0xFFFFF` end marker.

The decompressed buffer is `N` concatenated sub-images. Sub-image `s` spans
`[off[s-1], off[s])` (with `off[-1]=0`, `off[N-1]=end`), and is:

```
0x00  u32   width
0x04  u32   height
0x08  u32   flags   (bit 0 selects the RLE decoder; = 1 for the shipped MNAK cels)
0x0C  ...   RLE bytes
```

**RLE ("scheme A")**: control byte `c` — if `c < 0x80`, output the next byte `c` times; if
`c >= 0x80`, copy the next `c & 0x7F` bytes literally. The result is an **8bpp bottom-up DIB**;
rows are padded to a **4-byte stride** (`(w+3) & ~3`) — Genius (w=124) needs none but e.g.
TUTOR (w=143 → stride 144) does. Flip bottom-up→top-down and drop the row padding.

**Palette / transparency:** MNAK cels carry no palette. Windows characters use the standard
Windows 256-color halftone palette (an OS palette; system colors 0–9, a gray ramp, magenta at
253). The transparent color key is **index 10**.

*Engine ref:* `Char11.dll` `FUN_67e43ded` (MNAK/DICK container), `FUN_67e4b8f1`/`FUN_67e4ceaf`
(RLE → 8bpp DIB), and the halftone default at `FUN_67e4aaa0`.

### 4.3 SMC — Apple QuickTime (classic-Mac; Genius, Earl, Rocky, Bosgrove, Max)

Classic-Mac characters store each cel as an **Apple QuickTime SMC (`'smc '`)** chunk. The Mac
engine hands the bytes to QuickTime's Image Compression Manager (the exported
`DecompressSequenceBegin/Frame` are QuickTime imports, not Microsoft code).

Cel chunk framing:

```
0x00  u8    flags   (QuickTime sample flags: 0x40/0x60/0x80/0xA0; not needed to decode)
0x01  u24   chunk length (big-endian), INCLUDING these 4 header bytes
0x04  ...   SMC opcode stream
```

The pixel size of every cel comes from a per-character **QuickTime `ImageDescription`** (an
86-byte structure located by its `'smc '` codec tag): `width`, `height` at offsets `+32`/`+34`,
`depth = 8`, `clutID = 8`. (In the Mac dialect, the `ImageDescription` is prepended to the
action region; consumers must step over it — see §7.)

**SMC decoding** works on 4×4-pixel blocks in raster-of-tiles order, keeping three round-robin
color caches (2-, 4-, and 8-entry). Opcodes (high nibble = class):

| Class | Meaning |
|-------|---------|
| 0x00 / 0x10 | **skip** N blocks (leave existing pixels) |
| 0x20 / 0x30 | repeat the previous block N times |
| 0x40 / 0x50 | repeat the previous *two* blocks N times |
| 0x60 / 0x70 | one color, N blocks |
| 0x80 / 0x90 | two colors — `0x80` loads a new pair, `0x90` reuses a cached pair; a 16-bit mask picks per pixel |
| 0xA0 / 0xB0 | four colors (quad cache); a 32-bit field, 2 bits/pixel |
| 0xC0 / 0xD0 | eight colors (octet cache); 6 bytes → two 24-bit fields (rows 0–1, 2–3), 3 bits/pixel |
| 0xE0 / 0xF0 | raw — 16 literal indices per block |

For classes 0x00–0x70 the block count is `(op & 0x10) ? next_byte+1 : (op & 0x0F)+1`; the
color/raw classes use `(op & 0x0F)+1`. (Reference implementation: FFmpeg `libavcodec/smc.c`.)

**Inter-frame:** SMC is a *video* codec. Cels form a stream of a **keyframe** (full opaque
scene: a backdrop plus the character) followed by **delta frames** whose *skip* opcodes mean
"keep the previous frame." A cel therefore only makes full sense composited over the running
frame in playback order (decode into one persistent buffer). Mac cels are **opaque** (index 0
is white, not a transparency key) — the backdrop is part of the art; the Agent/`.acs` versions
of the same characters omit that backdrop.

**Palette:** the standard **Macintosh 256-color system color table** (QuickTime `clutID = 8` →
`GetCTable(8)`). Note the ordering: the `clut` resource lists entries in value order
(0 = black … 255 = white), but the SMC cel indices are the **reverse** (index 0 = white,
255 = black) — store the table reversed.

## 5. Object table

Region `0x2A` is the **object directory**: a flat array of `u32`, one entry per object index.

```
bits [29:0]   byte offset into the artwork pool (relative to the first cel)
bits [31:30]  sub-image selector (0 for WMF; 0..N-1 selects a sub-image within an MNAK block)
```

An object resolves to either:

- a **leaf image cel** — the `(offset, sub)` addresses a WMF or MNAK sub-image; or
- a **pose** — the bytes at that offset begin with the composite type word `0x0014` (§6).

Animation `Show` steps and pose parts reference objects **by this index**.

## 6. Poses

A pose composites several image parts into a full character frame:

```
0x00  u16   type (0x0014)
0x02  u16   part count
0x04  part[]   — 10 bytes each:
        u16   object index (the source image; §5)
        i16   dest left, top, right, bottom  (twips; ÷15 → pixels)
```

Parts are drawn in order; each source object is placed at its destination rectangle on the
`image_size` canvas.

## 7. Animations

### 7.1 Action table (region `0x3A`)

```
0x00  u16   count
0x02  u16   (pad)
0x04  action-header[count]  — 6 bytes each, sorted by id:
        u16   action id            (see §7.3)
        u16   variant count        (>= 1)
        u16   first pointer index
then  frame-pointer[]  — 6 bytes each (a shared array):
        u32   frame offset (into the frame-program pool, region 0x36)
        u16   blob length
```

Variant *v* of an action uses frame-pointer record index `count + firstPointerIndex + v`.
Lookup is a binary search over the id-sorted headers.

**Mac note:** the classic-Mac dialect prepends the 86-byte QuickTime `ImageDescription`
(`u32 size`, then the `'smc '` tag) to the front of this region; skip `size` bytes to reach
the `count`.

*Engine ref:* `Char11.dll` `FUN_67e462b8` builds the sequencer, `FUN_67e46349` does the
id lookup.

### 7.2 Frame program (a blob in region `0x36`)

Each animation variant is one contiguous op program:

```
0x00  u16   0x0100   (marker)
0x02  u16   op count
0x04  op[]  — 6 bytes each:
        u16   opcode
        u16   a
        u16   b
```

| Opcode | Name | Fields |
|--------|------|--------|
| 0 | **Show** | `a` = object index (§5); `b` = duration ms. `b == 0` is an instantaneous routing step (previous image stays). |
| 1 | **Branch** | with probability `b / 65536`, jump to op index `a`; else fall through. `b == 0` never branches (a fall-through / terminal marker). |
| 2 | **Sound** | `a` = index of an embedded sound (the `a`-th `RIFF`/`WAVE` stream, §8); `b & 0xFF` = volume, `b >> 8` = pan/flags. |
| 3 | **LoopBranch** | jump to op `a` once the animation has repeated `b` times. |
| 4 | **StateBranch** | jump to op `a` when the host mood / time-of-day state matches `b`. |

Playback runs from op 0; it ends when the op index runs past `op count`. There is no explicit
exit opcode. For Mac (SMC) characters, each shown cel composites over the previous frame
(§4.3).

*Engine ref:* `Char11.dll` frame walk `FUN_67e41da7`, branch predicate `FUN_67e41cc9`.

This is the **same animation model for every character** — WMF, MNAK, and Mac SMC all share
it; only the artwork encoding and pool sizes differ.

### 7.3 Action ids → names

Action ids are the values of Microsoft's official **`MsoAnimationType`** enumeration (the
Office object model; the `msoAnimation*` symbols in `mso97.dll`, and the same values the
Assistant/Actor engine uses). Ids outside the enum — a few internal Actor actions (7–10,
14–17, 20, 21) and character-specific ids — have no Microsoft-published name.

| id | name | id | name | id | name |
|----|------|----|------|----|------|
| 1 | Idle | 22 | WritingNotingSomething | 106 | LookDownRight |
| 2 | Greeting | 23 | WorkingAtSomething | 107 | LookLeft |
| 3 | Goodbye | 24 | Thinking | 108 | LookRight |
| 4 | BeginSpeaking | 25 | SendingMail | 109 | LookUp |
| 5 | RestPose | 26 | ListensToComputer | 110 | LookUpLeft |
| 6 | CharacterSuccessMajor | 31 | Disappear | 111 | LookUpRight |
| 11 | GetAttentionMajor | 32 | Appear | 112 | Saving |
| 12 | GetAttentionMinor | 100 | GetArtsy | 113 | GestureDown |
| 13 | Searching | 101 | GetTechy | 114 | GestureLeft |
| 18 | Printing | 102 | GetWizardy | 115 | GestureUp |
| 19 | GestureRight | 103 | CheckingSomething | 116 | EmptyTrash |
|    |      | 104 | LookDown | 105 | LookDownLeft |

### 7.4 Playback — how a character actually animates

A `.act` file is a **library of named actions**, not a timeline. The host application drives
it: it plays an action by id in response to events — `Idle` (1) autonomously whenever nothing
else is happening, `Greeting` (2) on appear, `Searching` (13) while searching, `Printing` (18)
on print, and so on. Nothing in the file *schedules* actions; that is host behaviour. (The
Agent 1.1 runtime layers its own show/hide/idle/speaking state machine on top when a host
uses the Agent API, but the file itself supplies only the action library.)

Playing one action:

1. **Look it up** by id (binary search over the id-sorted headers, §7.1).
2. **Pick a variant at random.** A character often has several takes on the same action
   (Genius's `Idle` has 10); repeated plays therefore differ.
3. **Run that variant's op program** (§7.2) as a tiny bytecode, with a program counter
   starting at op 0:
   - **Show** — display `object` for `duration_ms`, then advance to the next op.
     `duration_ms == 0` is an instantaneous *routing* step (the previously shown image stays).
   - **Branch** — a probabilistic `goto`: with probability `weight / 65536` jump to op
     `target`, else advance. A *decision point* is usually a run of consecutive Branch ops —
     an N-way weighted choice whose "none taken" leftover falls through to the next op;
     `weight == 0` never fires (a marker / unconditional fall-through).
   - **Sound / LoopBranch / StateBranch** — play sound `a`, bound a repeat, or gate on the
     host's mood / time-of-day state.
   - The program ends when the counter runs **past** `op count` — there is no exit opcode; a
     terminal is typically a `Show …, dur = 0` reached after a weight-0 branch.

Because Branch `target`s can point backward, the op list is really a small **directed graph**:
loops are back-branches and an idle animation wanders it probabilistically for as long as the
host lets it run. A finite render (e.g. a GIF preview) walks the graph with a fixed RNG seed
and a step cap. Each shown `object` is resolved through the object table (§5) to a cel or pose
and drawn to a full character frame; the `Show` durations drive the on-screen timing.

**Classic-Mac (SMC) playback** adds one step: keep a single running framebuffer and apply
each shown cel's SMC opcodes *over it* (skip = keep previous, §4.3), so the keyframe's opaque
backdrop persists while the delta frames animate the figure.

Worked example — Genius `Idle` (id 1), variant 0 (24 ops; first 11 shown; `a`/`b` are the two
operand words):

```
op  kind    a    b        note
 0  Show    4    0        establish the rest pose (instant)
 1  Show    4    100
 2  Branch  23   27852    ~42% jump to the tail (op 23)
 3  Branch  23   0        marker (never taken)
 4  Show    5    200
 5  Branch  23   0        marker
 6  Show    262  117
 7  Branch  13   16384    ~25% jump to op 13
 8  Branch  21   0        marker
 9  Show    263  333
10  Branch  9    29491    ~45% loop back to op 9
...                       ends when the PC runs past op 23
```

*Engine ref:* the runtime sequencer is `Char11.dll` `FUN_67e462b8` (setup), `FUN_67e41da7`
(frame walk), `FUN_67e41cc9` (branch predicate); `crustagent` exposes it as
`ActFile::action_sequence` (the linearised step list) and `ActFile::animate` (composited,
timed frames — including the Mac inter-frame path).

## 8. Sounds

Windows characters embed their sound effects as complete **`RIFF`/`WAVE`** streams inside the
sounds region (`0x2E`); extract each `RIFF….WAVE` chunk whole. `crustagent` does this.

**Classic-Mac audio is not yet reverse-engineered.** The available Mac files contain no
standard audio container anywhere in the data fork — no `RIFF`/`WAVE`, no `AIFF`/`FORM`, no
`'snd '` — and the region that holds the WAVE pool on Windows holds a different, undecoded
structure on the Mac (it begins `00 01 00 01 00 05 …`). Classic-Mac programs typically kept
sounds as `'snd '` resources in a file's **resource fork**, and these extracted Mac files are
**data-fork only** (their resource forks are absent — the same reason the per-character color
`clut` had to be sourced elsewhere, §4.3), so the Mac sound data may simply not be present in
what we have. `crustagent` does not extract Mac audio.

## 9. Palette summary

| Artwork | Palette source | Transparent |
|---------|----------------|-------------|
| WMF | small palette in the file header (RGBQUAD array after the section table) | — (opaque fills) |
| MNAK (Windows) | standard Windows 256-color halftone (not in the file) | index 10 |
| SMC (Mac) | standard Macintosh 256-color system `clut` id 8, reverse-indexed (not in the file) | opaque (index 0 = white) |

## 10. Engine function reference (`Char11.dll`, Agent 1.1)

For anyone re-verifying against Microsoft's binary:

| Function | Role |
|----------|------|
| `FUN_67e498d2` | outer container header (magic/version/section table/names) |
| `FUN_67e45f2e` | character loader: reads the 70-byte header and builds the tables |
| `FUN_67e49f12` | object-directory / artwork region reader |
| `FUN_67e462b8` | frame/action sequencer setup |
| `FUN_67e46349` | action id binary search |
| `FUN_67e41da7` | frame-program walk |
| `FUN_67e41cc9` | branch predicate (random / loop-count / state) |
| `FUN_67e43ded` | `MNAK`/`DICK` cel container |
| `FUN_67e472f8` | LZ77 decompressor |
| `FUN_67e4b8f1`, `FUN_67e4ceaf` | RLE → 8bpp DIB |
| `FUN_67e4aaa0` | default Windows halftone palette |

## 11. Not fully decoded

Honest gaps — none block rendering or animating the characters we have:

- **The `0x3E`/`0x42` regions** — a secondary state/overlay sequencer the engine builds in
  parallel (via `FUN_67e462b8` with a different selector). Identified but not decoded; not
  needed to render or play a character.
- **Classic-Mac audio** — not reverse-engineered; likely lived in the (absent) resource fork
  (§8).
- **Lip-sync / mouth overlays** — none were found in the `.act` files. Actor speech is carried
  by the host-drawn Office balloon plus the generic `BeginSpeaking` (4) gesture; there is no
  separate mouth-overlay / viseme table like the one Microsoft Agent's `.acs` format has.
- **`LoopBranch` / `StateBranch` runtime** — the opcodes and operands are known, but their
  exact counters (the loop-count source; the mood / time-of-day state bits) are host-runtime
  state and only lightly validated.
- **The speech balloon** — drawn by the host (Office), not stored in the `.act`.
