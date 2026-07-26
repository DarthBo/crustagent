# Microsoft Agent Character File Formats (`.acs`, `.acf`, `.aca`)

A byte-level specification of the **Microsoft Agent** character-file formats, reverse-engineered
clean-room for reimplementation. This document lets a competent developer implement both a parser
and the animation sequencer from scratch, without reference to any other implementation.

## Provenance & method

Every structural fact below is derived **only** from:

- **Microsoft's own binaries**, decompiled with Ghidra (headless) and cited by their
  `FUN_<address>` labels and, where useful, the decompiled line number:
  - **`Char11.dll`** (Microsoft Agent 1.x runtime, 1996) — the shared **LZ77** decompressor, the
    byte **RLE**, and the **animation sequencer**.
  - **`AgentDp2.dll`** (Microsoft Agent 2.0 *single-file* data provider) — the flat `.acs`
    container and character header. Registers `.acs` = `Agent.Character2.2`.
  - **`AgentDpv.dll`** (Microsoft Agent 2.0 *definition* data provider) — `.acf` + `.aca`.
    Registers `.acf`/`.aca` = `Agent.Character.2`.
  - **`AgentDPv.dll`** (Microsoft Agent **1.5**, the "DocFile Provider 1.5") — the OLE2
    structured-storage `.acs`.
  - **`AgentAnm.dll`** (Microsoft Agent 2.0 animation/render component).
  - Primary 2.0 build cited: `build.3422` (Nov 1999); constants cross-checked against `build.2202`.
- **Microsoft public documentation** — the Microsoft Agent SDK / "Microsoft Agent Programming
  Interface" (MSDN), the Agent Character Editor documentation, and the `MsoAnimationType`
  animation vocabulary. Where a fact is merely a documented Microsoft API convention (the state
  names, the animation-type ids), that is stated explicitly.
- **Sample files**, used only to *confirm* the reading of bytes whose structure was first taken
  from the binary. Confirmation rests on Microsoft's own shipping characters: **`CLIPPIT.ACS`,
  `Robby.acs`, `Genie.acs`, `GENIUS.ACS`** (and `Merlin.acs`, `F1.ACS`) for the 2.0 flat form, and
  **`genie.acs`, `robby.acs`** for the 1.5 OLE2 form. The layout was additionally stress-tested
  against a large corpus of third-party community characters (see Appendix F) — used only to
  exercise robustness, never as a source of structural claims.

Facts that could not be tied to a specific binary instruction and are inferred from sample bytes
alone are tagged **(INFERRED)**.

### Conventions

- All integers are **little-endian** unless stated. `u8`/`u16`/`u32` = unsigned; `i16`/`i32` =
  signed. Offsets written `+0xNN` are relative to the start of the structure under discussion.
- **`LPSTR` (length-prefixed wide string)** — the format's ubiquitous string type. In the flat
  `.acs`:

  ```
  u32  charCount            ; number of UTF-16 code units (NOT bytes)
  u16  chars[charCount]     ; UTF-16LE, no BOM
  u16  0x0000               ; NUL terminator (present on disk)
  ```

  Cited: `AgentDp2.dll!FUN_74c55adc`/`FUN_74c55c3c` consume strings in place, advancing
  `charCount*2 + 2` bytes past the count field (the `+2` is the on-disk terminator). **In `.acf`
  the terminator is *absent* on disk** — see §7. **An empty string is the 4-byte
  `charCount == 0` and nothing else** — no terminator follows (byte-verified: reading it any
  other way desynchronizes the Character Info walk, which otherwise lands exactly on
  `charInfoOffset + charInfoSize` for every sample).
- Error codes shown as `0x8004xxxx`/`0x8007xxxx` are the `HRESULT`s the loader returns; they are
  useful landmarks and are cited from the binary.

---

## 1. Flat `.acs` container (Microsoft Agent 2.0)

The 2.0 single-file `.acs` is a **flat, memory-mapped** file. Source: `AgentDp2.dll!FUN_74c55979`
(the open routine). It `CreateFileW`s the path, `CreateFileMappingW` + `MapViewOfFile` maps it
read-only, records the file size, then reads and validates a fixed **36-byte header**.

### 1.1 File header (36 bytes / 0x24)

`FUN_74c55979` reads exactly `0x24` bytes and requires the read to return `0x24` bytes **and**
`header[0] == 0xABCDABC3`; otherwise it returns `0x80042204` ("not a character file").

| Off  | Type | Field            | Notes |
|------|------|------------------|-------|
| 0x00 | u32  | `magic`          | **`0xABCDABC3`** (on-disk bytes `C3 AB CD AB`) |
| 0x04 | u32  | `charInfoOffset` | file offset of the Character Info block |
| 0x08 | u32  | `charInfoSize`   | byte length of the Character Info block |
| 0x0C | u32  | `animDirOffset`  | file offset of the Animation directory |
| 0x10 | u32  | `animDirSize`    | byte length |
| 0x14 | u32  | `imageDirOffset` | file offset of the Image directory |
| 0x18 | u32  | `imageDirSize`   | byte length |
| 0x1C | u32  | `soundDirOffset` | file offset of the Sound (audio) directory |
| 0x20 | u32  | `soundDirSize`   | byte length |

The four (offset,size) pairs are four **directories**. Each directory is resolved by
`FUN_74c5630d`, which bounds-checks `offset+size <= fileSize` (returning `0x80070570` on failure)
and yields a raw pointer `mapBase + offset`. In shipping files these four directories sit
contiguously at the **end** of the file, with the bulk animation/image/sound data occupying the
region from byte `0x24` up to the first directory; but a parser must use the header offsets, not
assume adjacency.

### 1.2 Animation directory

Source: `AgentDp2.dll!FUN_74c55adc` (builds the runtime table from this block). Confirmed
byte-for-byte on all samples.

```
u32  animationCount
animationCount × AnimEntry:
    LPSTR  name            ; animation name (case-preserving, e.g. "RestPose", "GestureLeft")
    u32    dataOffset      ; absolute file offset of this animation's data block
    u32    dataSize        ; byte length of that block
```

The `dataOffset`/`dataSize` point into the animation-data region (per-animation frame blocks; see
§3). Example (`Robby.acs`): `RestPose` → offset `0x24`, size `0x91`; blocks are contiguous, each
`dataOffset+dataSize == next dataOffset`, the first starting at `0x24` (immediately after the file
header).

### 1.3 Image directory

Source: image blocks referenced from here are decoded by the codec (§4). Confirmed on all samples.

```
u32  imageCount
imageCount × ImageEntry:
    u32  dataOffset        ; absolute file offset of the image block
    u32  dataSize          ; byte length of the image block
    u32  checksum          ; per-image checksum (not needed to parse; used for caching/dedup)
```

Each `dataOffset` points at an on-disk image block (§4.3). Image blocks are contiguous in the data
region, immediately following the animation blocks.

### 1.4 Sound directory

Identical entry layout to the image directory. `soundCount` may be `0` (a character with no audio).

```
u32  soundCount
soundCount × SoundEntry:
    u32  dataOffset        ; absolute file offset of the sound block
    u32  dataSize          ; byte length
    u32  checksum
```

Each block is a standard **RIFF/WAV** payload (the audio the sequencer triggers per frame).
(INFERRED that the block is raw WAV; the directory framing is confirmed.)

---

## 2. Character Info block & header sub-blocks

Source: `AgentDp2.dll!FUN_74c55c3c`. The block at `charInfoOffset` is copied to a heap buffer and
parsed **sequentially**; all sub-blocks are variable-length and packed with no padding, so a
parser must walk them in order. Optional sub-blocks are gated by flag bits in the fixed header.

### 2.1 Fixed header (offsets from block start)

| Off  | Type  | Field | Notes |
|------|-------|-------|-------|
| 0x00 | u32   | `version` | see §Appendix A. Must be one of `0x1001C`, `0x1001E`, `0x1001F`, `0x20001`; `> 0x20001` → `0x80042203`, any other value → `0x80042202`. All 2.0 flat samples = `0x20001`. |
| 0x04 | u32   | `localeTableOffset` | **absolute file offset of the localized character-info table (§2.7)** — a direct pointer to the same table the sequential walk ends on. Byte-verified: on all 16 sampled flat characters this equals, exactly, the offset the walk reaches. |
| 0x08 | u32   | `localeTableSize` | its byte length; `localeTableOffset + localeTableSize == charInfoOffset + charInfoSize`. |
| 0x0C | GUID  | `characterGuid` | 16 bytes; the character's unique class id (standard little-endian GUID field order). |
| 0x1C | u16   | `width` | default frame width, px |
| 0x1E | u16   | `height` | default frame height, px |
| 0x20 | u8    | `transparencyIndex` | **palette index used as the transparency color key** (confirmed: every image's background fill and border pixels equal this value). |
| 0x21 | u32   | `styleFlags` | the same style word the 1.5 header stores as one dword (§6.2 field 10): bit `0x20` ⇒ **Voice/TTS block present** (§2.2), bit `0x200` ⇒ **Word-balloon block present** (§2.3). Observed: `0x110`, `0x120`, `0x210`, `0x220`, `0x10210`, `0x100220`, `0x110220`. |
| 0x25 | u32   | reserved | `0x00000002` in every sample. |

The variable area begins at block **+0x29**.

*(Byte-level correction, verified against 16 flat characters:* the earlier reading of this
header put the GUID at +0x04 and eight undetermined reserved bytes at +0x14. Both readings
occupy the same 24 bytes, but the first dword pair is the locale-table pointer above — it
matches the sequential walk's landing offset exactly in every sample — which places the GUID
at **+0x0C**. Likewise `flags1`/`flags2` at +0x21/+0x22 are the low half of one style dword.
On the balloon gate the two candidate rules — "`0x100` clear" and "`0x200` set" — agree on
every sampled character; `0x200` is used here because it is the balloon bit the 1.5 header
sets, and `0x100` its complement.*)

`FUN_74c55c3c` also precomputes the frame raster size as `roundup4(width) * height` where
`roundup4(x) = (x + 3) & ~3` (stored at runtime `this+0xE4`); this is the 8-bpp stride×height used
throughout the codec (§4).

### 2.2 Voice / TTS block (present iff `flags1 & 0x20`)

Begins at block **+0x29**. Microsoft Agent selects a SAPI 4 text-to-speech voice by an engine
**mode GUID**; speed and pitch are stored numerically.

| Off (from +0x29) | Type | Field | Notes |
|------|------|-------|-------|
| +0x00 | GUID | `ttsEngineId` | 16-byte TTS engine/mode class id (SAPI). Samples: `{CA141FD0-AC7F-11D1-97A3-0060082730xx}` (TruVoice, `mslwvtts.dll`). |
| +0x10 | GUID | `ttsModeId` | 16-byte secondary/mode id; differs from `ttsEngineId` only in the trailing byte (the per-character voice selector). |
| +0x20 | u32  | `speed` | words/min; `0xFFFFFFFF` = engine default. |
| +0x24 | u16  | `pitch` | Hz; `0xFFFF` = engine default. |
| +0x26 | u8   | `extraFlag` | non-zero ⇒ the trailing language/name fields below are present. |

If `extraFlag != 0`, the following fields continue (offsets from +0x29):

| Off  | Type  | Field | Notes |
|------|-------|-------|-------|
| +0x27 | u16   | `langId` | Windows LANGID (`0x0409` en-US in every sample). |
| +0x29 | LPSTR | `languageName` | the language's display name, e.g. `"US English"`, `"American English"`, `"Standard"`. **Empty in every Microsoft character**, which is what made the tail look like a fixed 10-byte prefix. |
| after | u16   | `gender` | SAPI 4 `GENDER_FEMALE` = `0x0001`, `GENDER_MALE` = `0x0002` (`0x0002` in every Microsoft character); see the voice-selector note below. |
| +2    | u16   | `age` | INFERRED (`0x001E` in every sample, Microsoft and third-party alike). |
| +4    | LPSTR | `voiceName` | speaker/voice display name (e.g. `"Business"`, `"Normal"`). |

**Correction (byte-verified over ~340 characters):** this tail was previously read as a fixed
`u32 langId` + three u16 before `voiceName`. That happens to consume the same 10 bytes when
`languageName` is empty — true of every Microsoft character — but desynchronizes the whole
Character Info walk on the ~30 third-party characters that fill it in. Reading `languageName` as
a string is what makes those files parse to their exact block end.

**The mode id's trailing byte is the voice selector** (verified over the same ~340 characters).
Both engine families below differ only in their last byte, and it maps to a gender: `0x00…0x07`
are the adult male voices, `0x08`/`0x09` the adult female ones.

| Mode id | Engine |
|---------|--------|
| `{CA141FD0-AC7F-11D1-97A3-0060082730xx}` | Microsoft TruVoice (`mslwvtts.dll`) |
| `{1B6BF831-9299-101B-8A19-265D428C60xx}` | the older Agent 1.5-era voices |

Of the 251 library characters that declare a `gender`, **all 251 agree with their selector byte**,
which makes the byte a sound fallback for the ~29 that carry a voice block with no extended tail
(`extraFlag == 0`, so no `gender` at all). `Tts::resolved_gender()` implements exactly that, and
`crustagent-tts` uses it to pick a same-gender system voice instead of leaving every character on
the OS default. There is no way to recover the *specific* voice: those SAPI 4 engines are gone.

In the `.acf` variant the same fields appear at object offsets +0x24…, see §7. *Third-party
authoring note:* some non-Microsoft characters use a different SAPI engine GUID, and a few store
their strings ANSI rather than as the UTF-16 `LPSTR` above.

### 2.3 Word-balloon block (present iff `styleFlags & 0x200`)

Source: the balloon walk in `FUN_74c55c3c` (and `.acf` `FUN_74c492a9`).

| Off  | Type  | Field | Notes |
|------|-------|-------|-------|
| +0x00 | u8   | `perLineA` | balloon sizing byte A (chars-per-line / lines; order INFERRED). Samples = 2. |
| +0x01 | u8   | `perLineB` | balloon sizing byte B. Samples = 28. |
| +0x02 | u32  | `fgColor` | text color, `0x00BBGGRR`. |
| +0x06 | u32  | `bgColor` | balloon fill, `0x00BBGGRR` (samples `0x00E1FFFF`, pale yellow). |
| +0x0A | u32  | `borderColor` | `0x00BBGGRR`. |
| +0x0E | LPSTR| `fontName` | e.g. `"MS Sans Serif"`. |
| after | i32  | `fontHeight` | Win32 `LOGFONT.lfHeight` (samples `-13`). |
| +4    | u32  | `fontWeight` | `LOGFONT.lfWeight` (samples `400` = FW_NORMAL). |
| +8    | u8   | `italic` | `LOGFONT.lfItalic` (INFERRED; `0` in samples). |
| +9    | u8   | `strikeOut` | `LOGFONT.lfStrikeOut` (INFERRED; `0` in samples). In the split/1.5 header this second byte is the one gated on version > `0x1001E`. |

(The trailing metrics block is 10 bytes total: `i32 fontHeight, u32 fontWeight, u8, u8`.)

### 2.4 Palette

Immediately follows the balloon block (or the voice block, or the fixed header, depending on which
are present).

```
u32     paletteCount            ; explicit; all observed characters store 256 (0x100)
RGBQUAD entries[paletteCount]   ; 4 bytes each: { u8 blue, u8 green, u8 red, u8 reserved }
```

`FUN_74c55c3c` reads the palette table only when `paletteCount != 0`. **There is no
count-`0`→`256` sentinel** in the 2.0 flat provider or in the 1.5 provider (contrary to the older
Actor `.act` format); the count is authoritative. Pixel bytes in every image (§4) are indices into
this single global palette, and `transparencyIndex` (§2.1) selects the transparent entry. When the
system runs at 8-bpp the runtime builds an `HPALETTE` from these entries, swapping R/B into DIB
order and reserving indices 0–9 and 246–255 for the system palette (`AgentAnm.dll` palette builder,
`GetPaletteEntries`/`CreatePalette`).

### 2.5 Tray icon (optional)

```
u8   present                    ; 0 => no tray icon (stop here)
; if present != 0:
u32  colorSize                  ; bytes of the color-image chunk
u8   colorData[colorSize]       ; INFERRED: DIB (color bitmap)
u32  maskSize                   ; bytes of the AND-mask chunk
u8   maskData[maskSize]         ; INFERRED: 1-bpp AND mask
```

The two length-prefixed chunks are the icon's color DIB and its AND mask (INFERRED from the
two-chunk shape; the framing is byte-confirmed). The samples set `present = 0`. (The 1.5 format
carries no tray icon at all — §6.)

### 2.6 State → animation map

The engine maps a fixed vocabulary of **state names** to the character's own animation names.

```
u16  stateCount
stateCount × StateEntry:
    LPSTR  stateName            ; e.g. "SHOWING", "IDLINGLEVEL1"
    u16    animCount
    animCount × LPSTR animName  ; animation names to play for that state
```

The `stateName` values are documented Microsoft Agent API states (not invented here) — see
Appendix B. Example (`CLIPPIT.ACS`): `IDLINGLEVEL1 → [IDLESIDETOSIDE, IDLEEYEBROWRAISE,
IDLEHEADSCRATCH]`, `SHOWING → [SHOW]`, `HIDING → [HIDE]`. When a state lists several animations the
engine picks among them (see §5.5).

### 2.7 Localized character-info table

Ends the Character Info block. Source: lookup routine `FUN_74c560ad` (selects by `LCID & 0x3FF`,
with a fallback record). This table carries the character's **name, description, and "extra data"
string per language** — it is *not* an animation list.

```
u16  localeCount
localeCount × LocaleEntry:
    u16    lcid                 ; Windows LCID (9 = en, 0x0401 = ar, 0x0404 = zh-TW, 0x0407 = de, …)
    LPSTR  name                 ; localized character name
    LPSTR  description          ; localized description
    LPSTR  extraData            ; localized extra data / vendor string
```

Example (`Robby.acs`, LCID 9): name `"Robby"`, description `"I am an ICA 2.0, interactive
cybernetic assistant…"`, extraData `"Greetings. I am online and ready to assist you.^^You asked me
to remind you."`. Parsing this table lands **exactly** on `charInfoOffset + charInfoSize` (verified
on CLIPPIT and Robby), which confirms the whole Character Info layout end-to-end.

---

## 3. Animations, frames, and images (on-disk records)

Source: `AgentDp2.dll!FUN_74c54351` (animation/frame parser), `FUN_74c5499f` (image parser),
`FUN_74c550e5` (sound table). Every record below was validated byte-for-byte on `CLIPPIT.ACS`
(43 anims / 902 images / 15 sounds) and `Robby.acs` (68 / 593 / 32) — and across the wider corpus of
Appendix F — each consumed exactly to its declared block end.

### 3.1 Animation block

Located at the `dataOffset` from an Animation-directory entry (§1.2).

```
LPSTR  name                     ; repeats the directory name, upper-cased
u8     flagByte                 ; type/flag; ==1 gates a special exit path (FUN_74c5485b).
                                ; observed 0x02 in all samples (semantics INFERRED)
LPSTR  returnAnimation          ; name of the "return to rest" animation; empty in all samples
u16    frameCount               ; 0 => error 0x8004200f
Frame  frames[frameCount]       ; §3.2
```

(The 5 bytes following the name are the constant `02 00 00 00 00` in every sampled animation — a
`flagByte = 0x02` plus an **empty `returnAnimation` encoded as `u32 count = 0` with no chars and no
terminator** (4 bytes). Note this differs from the animation *name* just above it, which is a full
`LPSTR` including its `u16 0x0000` terminator. When `returnAnimation` is non-empty its `count`
UTF-16 chars follow the count; whether a terminator is then present is INFERRED (all samples are
empty). When present it names the animation whose frames become the return-to-rest path.)

### 3.2 Frame record

Variable length; fields packed in this exact order (`FUN_74c54351:3013–3088`):

```
u16          imageCount
Overlay      overlays[imageCount]     ; 8 bytes each — §3.2.1
u16          soundIndex               ; index into sound table; 0xFFFF = no sound
u16          duration                 ; hold time in CENTISECONDS (1/100 s)
u16          exitFrame                ; §3.2.3
u8           branchCount
Branch       branches[branchCount]    ; 4 bytes each — §3.2.2
u8           mouthOverlayCount         ; 0..7
MouthOverlay mouths[mouthOverlayCount] ; 14 bytes each — §3.2.4
```

#### 3.2.1 Image overlay (8 bytes)

```
u32  imageIndex        ; index into the Image table (§3.3)
u16  x                 ; placement, pixels from top-left of the character frame
u16  y
```

Overlays are drawn **back-to-front** (`FUN_74c54cd0`) with color-key transparency using
`transparencyIndex` (§2.1). **There is no per-overlay region flag or replace/composite flag in
2.0** — "replace vs composite" is derived at runtime (a lone overlay at (0,0) is a direct blit),
and the clipping **region belongs to the image** (§3.3.1), not the overlay.

#### 3.2.2 Branch (4 bytes)

```
u16  targetFrame       ; must be < frameCount
u16  probability       ; PERCENT, evaluated cumulatively (§5.2)
```

`branchCount == 0` means "no branch". The field is a `u8` (so up to 255 branches are representable),
but across a ~300-character corpus the largest branch list observed is **3** (Appendix F.2). See
§5.2 for the exact selection algorithm.

#### 3.2.3 Exit frame

`exitFrame` is consulted only when a stop/interrupt is pending (`FUN_74c553f8:3813`):

- `0xFFFF` → stop / end of animation.
- `0xFFFE` → no exit branch; fall through to normal branching.
- otherwise → jump to that frame index (the graceful-exit target).

#### 3.2.4 Mouth overlay (14 bytes) — lip-sync

Per-frame set of up to 7 mouth images, keyed by mouth-state 0..6 (chosen at play time from the TTS
engine, `FUN_74c54c19`):

```
u8   mouthState        ; 0..6 (slot index)
u8   flag              ; INFERRED composite/"requires base image" flag (0 in samples)
u32  imageIndex        ; index into the Image table
u16  x
u16  y
u16  regionX           ; secondary/region offset (0 in samples) — INFERRED
u16  regionY           ; (0 in samples) — INFERRED
```

### 3.3 Image table and image record

**Image table** = the Image directory of §1.3 (`u32 count` + 12-byte `{offset,size,checksum}`
entries).

**Image record** at `mapBase + offset` (`FUN_74c5499f:3269`):

| Off  | Type | Field | Notes |
|------|------|-------|-------|
| 0x00 | u8   | `present` | `0` → empty image (record ends here); `1` → present |
| 0x01 | u16  | `width` | |
| 0x03 | u16  | `height` | |
| 0x05 | u8   | `compressed` | `0` = raw 8-bpp, `1` = LZ (§4) |
| 0x06 | u32  | `compressedSize` | length of the LZ stream (used only when `compressed==1`) |
| 0x0A | u8[] | `pixels` | 8-bpp palette indices; stride `roundup4(width)`; `height` rows |

- Raster (uncompressed) size = `roundup4(width) * height`. When `compressed==0`, that many raw
  bytes follow at +0x0A. When `compressed==1`, `compressedSize` LZ bytes follow and decode to
  exactly that raster size (§4).
- Pixels are indices into the character's single global palette (§2.4); the image carries no
  palette of its own. The transparent pixel value is `transparencyIndex` (§2.1).

#### 3.3.1 Per-image region (follows the pixels)

Immediately after the pixel data, each image carries a Win32 **region** (used for click hit-testing
and the character's window shape):

```
u32  regionCompressedSize      ; 0 => region data is raw; else the LZ byte count
u32  regionUncompressedSize    ; size of the decompressed RGNDATA blob
u8[] regionData                ; raw RGNDATA, or an LZ stream (§4) that decodes to RGNDATA
```

`regionData` is (decompressed to) a Win32 `RGNDATA` structure passed to `ExtCreateRegion`.
Validated: `Genie.acs` image 0 is 128×128, pixels 3984 bytes (compressed), region 598 bytes →
1440-byte RGNDATA, the record ending exactly at `offset + size`.

### 3.4 Sound table

Same shape as the image table: `u32 count` then 12-byte `{offset, size, checksum}` entries
(`FUN_74c550e5`). The data at each `offset` is a complete `RIFF`/WAVE file (verified: all 15
`CLIPPIT.ACS` sounds begin with `RIFF`). Frames reference sounds by `soundIndex` (§3.2).

---

## 4. Image codec (LZ + RLE)

Source: `Char11.dll!FUN_67e472f8` (LZ77), `Char11.dll!FUN_67e4ceaf` (byte RLE); the 2.0 provider's
copy is `AgentDp2.dll!FUN_74c53705` (identical algorithm). A reference reimplementation
decompresses **every** image in `CLIPPIT.ACS` and `Robby.acs` (and the rest of the Appendix F
corpus) to exactly `roundup4(width)*height` bytes.

For on-disk `.acs`/`.acf`/`.aca` images the pixel raster is a **single LZ77 stream** (no RLE
layer). The byte-RLE codec and the `DCIK`/`MNAK` sub-container exist in the runtime for the COM/DIB
path but are **not** used by the on-disk image records above; they are documented in §4.3 for
completeness.

### 4.1 LZ77 bitstream — `FUN_67e472f8`

Signature `(const u8* src, int srcLen, u8* dst, int dstCap, int* outLen)`.

- Precondition: `srcLen > 7`; and **`src[0]` must be `0x00`** to mark a compressed stream (a
  nonzero first byte means "stored/uncompressed", handled by the caller). The `0x00` flag byte is
  consumed; the bitstream proper is `src[1..]`.
- The bitstream is read **LSB-first**: maintain a bit cursor `T` over `src[1..]`; bit `t` is
  `(src[1 + t/8] >> (t & 7)) & 1`; multi-bit values are assembled LSB-first.

Each token begins with **one flag bit**:

- **flag `0` → literal**: read 8 bits, emit that byte.
- **flag `1` → back-reference**: read a distance *tier* (a unary run of up to three `1` bits),
  then a distance field, then a length code.

| Tier | Unary prefix | Distance bits | Distance = field + | Min length |
|------|--------------|---------------|--------------------|------------|
| 1 | `0`   | 6  | `+ 0x1`    (1..64)          | 2 |
| 2 | `10`  | 9  | `+ 0x41`   (65..576)        | 2 |
| 3 | `110` | 12 | `+ 0x241`  (577..4672)      | 2 |
| 4 | `111` | 20 | `+ 0x1241` (4673..1052671)  | 3 |

**End-of-stream**: tier 4 whose 20-bit distance field is exactly **`0xFFFFF`** — decoding stops and
returns success. (On disk the stream is followed by padding bytes; the terminator, not the byte
count, defines the end.)

**Match length** = base (the tier's min length) plus a variable code:

```
k = number of leading 1-bits (terminated by a 0)
length = base                              if k == 0
length = base + (2^k - 1) + read_bits(k)   if k >= 1
```

Then copy `length` bytes from `dst[outPos - distance]` forward, one byte at a time (overlap is
allowed, i.e. run-length copies work).

Reference decoder (validated):

```python
def lz_decompress(buf):                      # buf[0] must be 0x00
    t = 0
    def bit():
        nonlocal t
        b = (buf[1 + (t >> 3)] >> (t & 7)) & 1; t += 1; return b
    def bits(n):
        v = 0
        for i in range(n): v |= bit() << i
        return v
    out = bytearray()
    while True:
        if bit() == 0:                        # literal
            out.append(bits(8)); continue
        if   bit() == 0: dist = bits(6)  + 0x1;    minlen = 2
        elif bit() == 0: dist = bits(9)  + 0x41;   minlen = 2
        elif bit() == 0: dist = bits(12) + 0x241;  minlen = 2
        else:
            v = bits(20)
            if v == 0xFFFFF: break             # end of stream
            dist = v + 0x1241; minlen = 3
        k = 0
        while bit() == 1: k += 1               # match length
        length = minlen if k == 0 else minlen + ((1 << k) - 1) + bits(k)
        s = len(out) - dist
        for i in range(length): out.append(out[s + i])
    return bytes(out)
```

### 4.2 Byte RLE — `FUN_67e4ceaf`

A separate primitive (used only in the COM/DIB path, §4.3). Opcodes, one byte each; a `0x00` opcode
terminates:

```
op = next byte
if op == 0x00:            end
elif op & 0x80 == 0:      run:     v = next byte; emit op copies of v          (op = 1..127)
else:                     literal: n = op & 0x7F; emit next n bytes verbatim   (n = 0..127)
```

### 4.3 `DCIK`/`MNAK` sub-container (runtime COM path only — not on disk in `.acs`)

`Char11.dll!FUN_67e43ded` handles an in-memory image blob whose 8-byte header is `{u32 tag, u32
decompressedSize}`. `tag == 'DCIK' (0x4B494344)` or `'MNAK' (0x4B414E4D)` ⇒ the body is an LZ77
stream (§4.1) that decompresses to a 12-byte sub-header `{u32 width, u32 height, u32 flags}`
followed by the raster; `flags & 1` ⇒ that raster is additionally **RLE-compressed** (§4.2, LZ77
outer / RLE inner), else it is raw. Any other tag ⇒ raw. On disk, `.acs`/`.acf`/`.aca` images do
**not** use this wrapper (they are bare LZ77 per §3.3 / §4.1); it is documented so an implementer
recognises the runtime DIB path.

### 4.4 Raster geometry and rendering

- Row **stride = `roundup4(width) = (width + 3) & ~3`**; the raster is `stride * height` bytes; the
  `stride - width` trailing bytes of each row are padding.
- Each byte is an index into the 256-entry global palette (§2.4). When the runtime builds a DIB it
  uses `biBitCount = 8`, a positive `biHeight` (a **bottom-up** DIB — raster row 0 is the bottom
  scanline), and a 256-entry `RGBQUAD` color table copied from the palette with R/B swapped.
- Transparency: pixels equal to `transparencyIndex` (§2.1) are the transparent background, masked
  at blit time. `AgentAnm.dll` builds companion mask surfaces from that key index.

---

## 5. Animation sequencer (runtime playback)

Two engine generations exist. The **2.0** sequencer that plays the on-disk records of §3 is split
between `AgentAnm.dll` (the frame pump / timer) and the data provider `AgentDp2.dll`
(`FUN_74c553f8`, the "get next frame" that performs branch selection). The **1.x** engine in
`Char11.dll` (`FUN_67e41da7`) is described in §5.6 for completeness and for its timing constants
(which the 2.0 pump reuses). This section specifies behavior precisely enough to reproduce it.

### 5.1 Frame pump and timing (2.0)

Source: `AgentAnm.dll!FUN_74c939ea`/`FUN_74c9389c`.

1. Show the current frame's overlays (§3.2.1) and, if `soundIndex != 0xFFFF`, start the referenced
   WAV (§3.4).
2. Compute the hold: on disk `duration` is in **centiseconds**; the pump multiplies by 10 to get
   **milliseconds** (`duration * 10`). It schedules the next tick at `scheduledTick += durationMs`
   and arms `SetTimer(hwnd, id, elapse, …)` where `elapse = scheduledTick - GetTickCount()` (reset
   to `now + durationMs` if negative). Timer id `1000` = ordinary frame, `2000` = terminal frame.
3. On the timer, ask the provider for the next frame index (§5.2). If the provider reports "last"
   (terminal), fire the completion callback instead of rescheduling.

Minimum interval / runaway guards: see §5.4.

### 5.2 Branch selection (2.0) — `AgentDp2.dll!FUN_74c553f8`

After a frame's hold expires, the next frame is chosen as follows (order matters):

```
1. If a stop/interrupt is pending, consult exitFrame (§3.2.3):
       exitFrame == 0xFFFF  -> STOP (animation ends)
       exitFrame == 0xFFFE  -> ignore; continue to branch evaluation
       else                 -> next = exitFrame ; done
2. Evaluate the branch table (§3.2.2) with a single RNG draw, cumulatively:
       r = rand() % 100 + 1                 ; r in 1..100
       for each branch in order:
           if r <= branch.probability:  next = branch.targetFrame ; goto done
           else:                        r  -= branch.probability   ; continue
3. If no branch fired: next = currentFrame + 1 (sequential advance).
4. If next >= frameCount: STOP (returns 0xFFFFFFFF).
```

Notes:
- `probability` is a **percentage**; the branch list therefore encodes a cumulative distribution.
  A single branch with `probability = 100` is an unconditional jump (a loop-back). If the
  probabilities in a list sum to less than 100, the remaining mass falls through to the sequential
  next frame (step 3).
- Exactly one RNG draw is made per frame regardless of branch count; branches are tested in stored
  order and the **first** satisfied one wins.
- `rand()` is the C runtime PRNG (MSVC, period-bounded `0..0x7FFF`); `rand() % 100 + 1` yields the
  1..100 selector. To reproduce Microsoft's sequence exactly an implementation must use the same
  `rand()`/`srand()` stream; for functional equivalence any uniform draw in `1..100` matches the
  probability semantics.

### 5.3 Frame advance, exit, and termination (2.0)

- **Advance**: normal flow is step 3 above (frame → frame+1) unless a branch or `exitFrame`
  redirects.
- **Looping** is expressed *in data*: an animation loops by ending its branch list with a
  `probability = 100` branch back to an earlier frame; it terminates by running off the end
  (`next >= frameCount`, step 4) or hitting an `exitFrame == 0xFFFF`.
- **Interrupt / graceful stop**: when the host requests a stop, `exitFrame` steers the animation to
  a designated wind-down frame (or ends immediately if `0xFFFF`), so a character stops on a natural
  pose rather than snapping. `returnAnimation` (§3.1), if present, provides the return-to-rest path.
- **Completion**: reaching a terminal frame fires the provider/host completion notification (the
  `2000` timer id in §5.1); the runtime then advances the character's state queue.

### 5.4 Runaway guards / timing constants (from `Char11.dll`, reused by the pump)

| Guard | Value | Source |
|-------|-------|--------|
| Minimum timer interval | **14 ms** (`0x0E`; tested `< 0x0F`) | `Char11.dll!FUN_67e436ac` (`if ((int)elapse < 0xF) elapse = 0xE;`) |
| Per-callback batching budget | **14 ms** | `Char11.dll!FUN_67e41da7` (`frameStart + 0x0E`) — zero/short frames are batched within one tick |
| Graceful-exit duration cap | **100 ms** (`0x64`) | `Char11.dll!FUN_67e41da7` (during exit mode, each frame's hold is clamped to ≤ 100 ms) |
| Global enable gate | `DAT_67e55124` | if 0, the pump prepares but never schedules |

There is **no** explicit maximum-frame-count or maximum-total-time counter; the engine relies on
well-formed data (branch cycles must eventually hit a terminal frame). A malformed all-branch,
zero-duration cycle can spin within one timer callback — an implementation may add its own cap, but
Microsoft's does not (INFERRED from the absence of any iteration counter in `FUN_67e41da7`).

### 5.5 Selecting among multiple animations for a state

When a state maps to several animation names (§2.6) or a name resolves to several interchangeable
variations, the runtime picks one at random: `Char11.dll!FUN_67e46349` computes
`index = rand() % variationCount` (a caller may force a specific index to replay). This is a
separate RNG draw from the per-frame branch draw.

### 5.6 `Char11.dll` 1.x sequencer (historical)

The 1.x runtime plays a compiled **6-byte-entry** frame stream (`block[+2] = u16 frameCount`; each
entry `{i16 type, i16 arg1, i16 arg2}` at `block + 4 + i*6`): `type 0` = image (arg1 = image
index, arg2 = duration ms), `1` = probabilistic branch (arg1 = target, arg2 = probability), `2` =
sound, `3` = mouse-pointer-state branch, `4` = time-of-day branch. Its branch test
(`FUN_67e41cc9`) is `(uint16)rand() < arg2` where `arg2` is scaled into `0..0x8000`
(`0x8000` = always). Fall-through (no branch) advances to the next entry. This differs from the 2.0
model (§5.2), which uses `rand()%100+1` over an explicit percentage list; the two on-disk frame
formats are not interchangeable. The timing constants in §5.4 originate here.

---

## 6. ACS 1.5 — OLE2 structured storage

Microsoft Agent **1.5** stores a character as an **OLE2 compound file** (magic
`D0 CF 11 E0 A1 B1 1A E1`). Loader: `AgentDPv.dll (1.5)` ("Microsoft Agent DocFile Provider 1.5"),
`FUN_67f92034` → `StgOpenStorage(file, 0, STGM_READ|STGM_SHARE_DENY_WRITE, …)`. All facts below were
byte-verified on `1.5/genie.acs` and `1.5/robby.acs`.

### 6.1 Compound-file tree

The root storage is flat (no sub-storages). It contains:

```
Root Entry
├── "char.acf"                  ; the compressed character header (in the OLE mini-stream)
└── "anim1.aaf" … "animN.aaf"   ; one stream per animation (one full-sector stream each)
```

`char.acf` is opened by the constant wide name `"char.acf"` (`FUN_67f92034` → `OpenStream`). Each
animation stream's name is **stored verbatim** in the `char.acf` animation list (§6.2 field 2); the
loader opens it by that stored name (`FUN_67f94307`, which formats `"%s%s"` = path-prefix +
name; the prefix is empty for compound files). There is no `anim%d` name template in the binary.
(genie: `char.acf` + `anim1..103`; robby: `char.acf` + `anim1..94`.)

### 6.2 `char.acf` stream

Framing (before decompression):

```
u32  magic = 0xABCDABC1        ; on-disk C1 AB CD AB (checked; mismatch -> 0x80042204)
u32  uncompressedSize
u32  compressedSize
u8   lzData[compressedSize]    ; LZ77 (§4.1); first byte 0x00
```

(`FUN_67f9b532` reads the sizes; `FUN_67f9a495` is the 1.5 LZ77 — identical tiers/`0xFFFFF` end
marker to §4.1, with a 6-byte `0xFF` trailer.) A loose/on-disk sibling path uses a stream named
`"CONTENTS"` with magic `0xABCDABC2`; ACS files use `char.acf`/`0xABCDABC1`.

**Strings in 1.5 have no on-disk terminator**: `{ u32 charCount; u16 chars[charCount] }`
(UTF-16LE), or an ANSI variant `{ u32 charCount; char chars[charCount] }` for name/description.
(`FUN_67f9a278`/`FUN_67f9a2ed`.) Which of the two the name/description/extraData fields use is
**not** recorded in the header, and it varies: Microsoft's own `genie`/`robby` write them ANSI,
while both third-party 1.5 samples here write them UTF-16. A reader can settle it by trying
both — only the correct one consumes the definition exactly.

Decompressed header (`FUN_67f9693c`), in order — all byte-verified:

| # | Field | Type | Notes |
|---|-------|------|-------|
| 1 | `version` | u32 | `0x1001C`/`0x1001E`/`0x1001F` (>… → `0x80042203`; else → `0x80042202`; `0x1001D` used only as a feature boundary). Samples `0x1001F`. |
| 2 | animation list | `u16 count`, then per entry: `wLPSTR animName`, `wLPSTR streamName`, `wLPSTR returnAnimName`, and if `version > 0x1001D` a `u32 animGID` | `streamName` = the OLE stream to open; `animGID` equals the GID stored inside that stream; the 3rd string is the return-to-rest animation. |
| 3 | `characterGuid` | 16 bytes | |
| 4 | `name` | ANSI LPSTR | e.g. "Genie", "Robby" |
| 5 | `description` | ANSI LPSTR | |
| 6 | `extraData` | ANSI LPSTR | vendor/copyright string |
| 7 | `width` | u16 | |
| 8 | `height` | u16 | |
| 9 | `transparencyIndex` | u8 | matches the image background fill (genie = 10) |
| 10 | `styleFlags` | u32 | samples `0x00000220` |
| 11 | Voice/TTS block | if `styleFlags & 0x20` | `{ 16-byte engine/mode GUID, 16-byte secondary GUID, u32 speed, u16 pitch }` (INFERRED field split; speed `0xFFFFFFFF` = default). |
| 12 | Word-balloon block | if `(styleFlags & 0x100) == 0` | `FUN_67f96d05`: `u8, u8, u32 fg, u32 bg, u32 border, ANSI-LPSTR font, i32 fontHeight, u32 fontWeight, u8`, plus a trailing `u8` if `version > 0x1001E`. Samples: `2, 28, 0x000000, 0x00E1FFFF, 0x000000, "MS Sans Serif", -13, 400, 0, 0`. |
| 13 | `paletteCount` | u32 | **explicit**; samples 256 |
| 14 | palette | `RGBQUAD[paletteCount]`, only if count ≠ 0 | |
| 15 | State→animation map | `FUN_67f96dba` | `u16 stateCount`, per state `{ wLPSTR name; u16 count; wLPSTR animName[count] }` |

Differences from the 2.0 flat header:
- The balloon gate is the same style word in both (§2.1): the block is present when
  `styleFlags & 0x200` is set / `styleFlags & 0x100` is clear — the two are complementary in
  every sample. The voice bit is `styleFlags & 0x20` in both.
- `name`/`description`/`extraData` are **top-level single strings** here (ANSI or UTF-16, see
  above), versus the per-LCID UTF-16 localized table of the 2.0 header (§2.7).
- **No tray-icon block** (the header consumes the whole decompressed buffer with none).
- **No palette `0`→`256` sentinel** and **no pitch→gender heuristic** in the 1.5 loader (gender is
  resolved by the TTS engine from the mode GUID). *(These two behaviors, hypothesized for 1.5, are
  confirmed absent.)*

### 6.3 Animation streams (`.aaf`)

Each animation is one root-level stream, opened by its stored `streamName` (§6.2 field 2).

Framing (`FUN_67f93290`, for `version >= 0x1001D`):

```
u32  animVersion               ; e.g. 0x1001F  (omitted if char version < 0x1001D)
u32  animGID                   ; equals the animation-list GID (omitted with the version)
u8   compressFlag              ; 1 = LZ (§4.1), 0 = raw
u32  uncompressedSize
u32  compressedSize
u8   data[compressedSize]
```

Decompressed animation record (`FUN_67f95822`). Unlike 2.0, an animation is self-contained:
it carries its own audio and artwork, and its frames index those local tables.

```
u16  soundCount
{ u32 size; u8 riff[size] } sounds[soundCount]         ; complete RIFF/WAVE files

u16  imageCount
imageCount × Image:
    u32  size                    ; 0 => empty slot, the record ends here
    u8   storage                 ; 0 = raw 8-bpp in every sample
    u8   pixels[size]            ; full character frame; stride roundup4(width), bottom-up
    u32  regionSize
    u8   region[regionSize]      ; RGNDATA, as in §3.3.1

u16  frameCount
frameCount × Frame:
    u16  imageIndex              ; into this stream's image table
    i16  soundIndex              ; into this stream's sound table; -1 = none
    u16  duration                ; CENTISECONDS
    i16  x                       ; image placement (0 in every sample)
    i16  y
    u8   branchCount
    { i16 targetFrame; u16 probability } branches[branchCount]     ; percent, as §5.2
    u8   mouthOverlayCount
    mouthOverlayCount × MouthOverlay:                              ; inline lip-sync art
        u8   mouthState          ; 0..6, as §3.2.4
        u32  size                ; 0 => empty slot, the record ends here (as in the image table)
        u8   storage             ; 0 = raw, as above
        i16  x; i16 y            ; placement within the frame
        u16  width; u16 height   ; size == roundup4(width) * height
        u8   pixels[size]
```

Byte-verified end-to-end: this grammar consumes **795 of the 802 animation streams** across the
30 sampled 1.5 characters exactly to their declared decompressed length, and their rasters
render correctly against the `char.acf` palette. There is no per-stream palette, and no
`exitFrame` field — 1.5 frames wind down only through their branch lists.

The seven exceptions are all damage rather than structure: six streams' LZ payloads decode a
handful of bytes short of the declared size, so a robust reader should keep the frames it did
parse rather than reject the animation.

---

## 7. ACF (character definition) + ACA (external animation data)

Microsoft Agent **2.0** also supports a **split** representation: an `.acf` character-definition
file that references external `.aca` animation-data files. Loader: `AgentDpv.dll` (registers
`.acf`/`.aca` = `Agent.Character.2`). No `.acf`/`.aca` samples were available, so field *meanings*
are sometimes INFERRED; the *structure* is taken from the binary (and the equivalent 1.5 `char.acf`
in §6, which shares most of it).

### 7.1 `.acf` container

An `.acf` is normally an **OLE2 compound file** whose definition lives in a stream named
`"char.acf"` (`AgentDpv.dll!FUN_74c44604`: `StgOpenStorage` → `OpenStream("char.acf")`). That
stream is framed and LZ-compressed exactly like the 1.5 `char.acf` (§6.2) but with magic
**`0xABCDABC1`**:

```
u32  magic = 0xABCDABC1
u32  uncompressedSize
u32  compressedSize
u8   lzData[compressedSize]
```

A **flat** (non-compound) `.acf` variant is also accepted (`FUN_74c45855`), distinguished by magic
`0xABCDABC2` or `0xABCDABC4`; it is inflated and parsed by the same routine. Magic family (all
compared in the `0x5432543x` two's-complement space the decompiler shows):

| Magic | Meaning |
|-------|---------|
| `0xABCDABC1` | `char.acf` stream inside an OLE2 `.acf` (or 1.5 ACS) |
| `0xABCDABC2` / `0xABCDABC4` | flat `.acf` file |
| `0xABCDABC3` | flat `.acs` (§1) |

### 7.2 `.acf` character header — `AgentDpv.dll!FUN_74c48cb8`

The header carries the **same logical sub-blocks** as the flat `.acs` (§2): version (same accepted
set), an animation list, character GUID, a per-locale name/description/extraData table, geometry +
flags, voice/TTS, word-balloon (`FUN_74c492a9`), palette, tray icon (only if version > `0x20000`),
and the state→animation map (`FUN_74c49374`). The on-disk read order differs slightly from `.acs`
(the animation list precedes the GUID here) but the block contents match. Two `.acf`-specific
points:

- **String encoding differs from `.acs`.** `.acf` strings are `{ u32 charCount; u16
  chars[charCount] }` with **no on-disk NUL terminator** (`FUN_74c43c76`:
  `Read(count,4); Read(buf, count*2); buf[count*2] = 0` — the loader appends the terminator in
  memory). The flat `.acs` (§Conventions) *does* store the `u16 0x0000` terminator on disk. A
  parser must not consume a trailing terminator in `.acf`.
- **Animation-list entry** (0x10-byte runtime record) = `{ LPSTR animName; LPSTR acaReference;
  LPSTR thirdName; u32 animId (only if version > 0x1001D) }`. `acaReference` (the record's second
  string) is how an `.acf` names its animation data (§7.3); `thirdName` is INFERRED as the
  return/alt animation.

### 7.3 How an `.acf` references its `.aca` files

The reference is the per-animation **name string** `acaReference` (§7.2), resolved as a **relative
filename** against the `.acf`'s own directory, and validated by a matching id:

1. On open, the loader stores the `.acf`'s directory (`wcsrchr(path, '\\')` truncated;
   `this+0x18`).
2. To load an animation, `FUN_74c463c7` looks it up by `animName` (`FUN_74c494e6` returns the
   record's `acaReference` string), builds `"<dir>\<acaReference>"` (`swprintf`, format at
   `0x74c41bec`, INFERRED `L"%s\\%s"`), and opens that file (`CreateFileW`, or a URL moniker when
   the character was loaded from a URL).
3. Validation (`FUN_74c46bb3`): for char-definition version ≥ `0x1001D` the `.aca` begins with
   `u32 version` + `u32 animationId`, and `animationId` must equal the `.acf` record's `animId`
   (`FUN_74c495f8` returns record `+0x0C`). So the `.acf`'s per-animation id is echoed in the
   `.aca` header.

**Compound variant**: when the `.acf` is an OLE2 file, the same `acaReference` string is instead an
**internal stream name** opened on the `.acf`'s own storage (`FUN_74c44e17` →
`IStorage::OpenStream(acaReference)`). So `acaReference` is a *name* — an external sibling filename
(flat `.acf`) or an internal OLE stream (compound `.acf`).

### 7.4 `.aca` animation-data layout

Per-animation `.aca` stream/file (`FUN_74c46bb3` flat / `FUN_74c44e17` compound):

```
u32  version                   ; only if char-def version >= 0x1001D; ∈ {0x1001C,0x1001E,0x1001F,0x20001}
u32  animationId               ; only if version present; must equal the .acf record's animId
u8   compressFlag              ; 1 => LZ (§4.1), 0 => raw
;  animation body (FUN_74c47825):
BSTR name                      ; (SysAllocString of the animation name)
u16  imageCount
{ u32 size; u8 data[size] } images[imageCount]      ; frame image bitmaps (8-bpp; §4)
u16  count2
{ u32 len0; u8 flag; u8 d0[len0]; u32 len1; u8 d1[len1] } regionOrMask[count2]  ; INFERRED region/mask blocks
;  if version >= 0x10020: u8 ?, u16 frameCount
Frame frames[frameCount]       ; via FUN_74c47c5a, 0x40-byte runtime records
```

Each frame (`FUN_74c47c5a`): five u16 header fields; `u16 duration` (`0xFFFF` if version < 0x10020);
`u8 imageRefCount` then `{u16,u16}` image-composite refs; `u8 branchCount` then branch records
(INFERRED `{u16 targetFrame, u16 probability}` plus padding, in a 0x1C-byte runtime slot); and, for
version > 0x1001F with a flag set, an extra block of two length-prefixed buffers (INFERRED
audio/lip-sync). The overall shape — image table + region/mask table + frame table with per-frame
image refs, duration, and a branch list — matches the `.acs`/1.5 frame model (§3.2 / §5). Field
meanings marked INFERRED lack a byte-level sample to confirm.

### 7.5 Dispatch (`AgentAnm.dll`)

`AgentAnm.dll!FUN_74c92fbf` decides the provider: extension `.acs` ⇒ read the 4-byte magic, require
`0xABCDABC3`, and use the fixed single-file provider CLSID; otherwise `GetClassFile()` reads the
provider CLSID from the OLE2 file (which is why an OLE2 `.acf` resolves to `AgentDpv`), then
`CoCreateInstance` + `IPersistFile::Load`. `AgentAnm` itself parses no `.acf`/`.aca` bytes — it
performs window/palette creation and blitting only.

---

## Appendix A — Version stamps

The `version` dword at the start of the character header (and the `.aca`/`.aaf` streams) encodes
the format revision. Accepted values (`AgentDp2.dll!FUN_74c55c3c`, `AgentDpv.dll!FUN_74c48cb8`,
`AgentDPv.dll(1.5)!FUN_67f9693c`):

| Value | Interpretation | Seen in |
|-------|----------------|---------|
| `0x0001001C` | 1.x (28) | 1.5-era |
| `0x0001001E` | 1.x (30) | 1.5-era |
| `0x0001001F` | 1.x (31) | 1.5 samples (`genie`, `robby`) |
| `0x00020001` | 2.0 (2.1) | all 2.0 flat samples |
| `0x0001001D` | — | never accepted as a header version; used only as a **feature boundary** (≥ it ⇒ animation-list entries carry a `u32` id, `.aca`/`.aaf` streams carry `version`+`id`) |
| `0x00010020` | — | feature boundary in `.aca` (≥ it ⇒ per-frame `duration` present; else `0xFFFF`) |

Validation: `version > 0x20001` → `HRESULT 0x80042203`; a value that is neither `> 0x20001` nor in
the accepted set → `0x80042202`.

## Appendix B — Documented state names and animation vocabulary

These are **Microsoft Agent API facts** (from the Microsoft Agent SDK / "Programming Interface"),
not invented by this spec. The state names are the keys of the state→animation map (§2.6 / §6.2).
The standard states observed across the samples:

```
SHOWING            HIDING
SPEAKING           LISTENING          HEARING
IDLINGLEVEL1       IDLINGLEVEL2       IDLINGLEVEL3
GESTURINGLEFT      GESTURINGRIGHT     GESTURINGUP     GESTURINGDOWN
MOVINGLEFT         MOVINGRIGHT        MOVINGUP        MOVINGDOWN
```

(Microsoft's full documented set also includes `GESTURINGUP`/etc., `RESTPOSE`, and search/attention
states; a character need only define the states it uses.) Individual **animation names** (the values
in the map, and the keys of the animation directory) are author-chosen strings — e.g. `RestPose`,
`Show`, `Hide`, `IdleSideToSide`, `GestureLeft`. The Office-Assistant lineage additionally uses the
`MsoAnimationType` enumeration (a documented Microsoft enum) to name assistant actions; that
enumeration is the authoritative source for those action ids and is not reproduced here.

## Appendix C — HRESULT landmarks (from the loaders)

| HRESULT | Meaning | Cited from |
|---------|---------|-----------|
| `0x80042204` | not a character file / bad container magic | `FUN_74c55979`, `FUN_74c44604` |
| `0x80042203` | unsupported (too-new) header version | `FUN_74c55c3c`, `FUN_74c48cb8` |
| `0x80042202` | invalid header version | `FUN_74c55c3c`, `FUN_74c48cb8` |
| `0x8004200F` | animation has zero frames | `FUN_74c54351` |
| `0x80070570` | `ERROR_FILE_CORRUPT` — offset+size exceeds file | `FUN_74c5630d`, `FUN_74c55c3c` |
| `0x8007000E` | `E_OUTOFMEMORY` | allocation sites |
| `0x8007007A` | `ERROR_INSUFFICIENT_BUFFER` — string truncated to caller buffer | `FUN_74c560ad` |
| `0x80030020`/`0x80030021` | STG sharing violation (retried up to 10×) | `FUN_74c44604` |

## Appendix D — Checksums, and open/inferred items

- **Directory checksums.** Image- and sound-directory entries carry a third `u32` (`checksum`). It
  is a per-blob 32-bit hash used for caching/deduplication; the loaders do not require it to parse,
  and its algorithm is not needed to read the format (INFERRED — not computed by the parse path).
- **INFERRED (structure confirmed, meaning not byte-proven):** the animation-header `flagByte`
  (always `0x02` in samples) and non-empty `returnAnimation` sub-encoding; the mouth-overlay `flag`
  byte and its trailing two u16 offsets (all `0`); the tray-icon two chunks being color-DIB +
  AND-mask; the exact split of the voice block's language/gender/age u16 sub-fields; the `.acf`
  `swprintf` path format; the field *meanings* inside `.aca` `FUN_74c47c5a` frames.
- **Revisions from implementation.** Writing a reader against this document and running it over
  ~340 characters corrected five byte-level readings, each noted inline where it applies: the
  Character Info pointer/GUID split (§2.1), the empty-string encoding (§Conventions), the voice
  block's `languageName` string (§2.2), the balloon block's trailing `LOGFONT` flags (§2.3), and
  the 1.5 animation-stream grammar and identity-string encoding (§6.2/§6.3).
- **Clean-room note.** No third-party reimplementation of these formats was consulted for any part
  of this document; every structural claim traces to a cited Microsoft binary function or a
  Microsoft public API fact, and byte-level details were confirmed by decompressing/parsing the
  sample files listed in *Provenance*.

## Appendix E — Reference parse order (flat `.acs`)

```
read 36-byte file header; check magic 0xABCDABC3
resolve 4 directories (CharInfo, AnimDir, ImageDir, SoundDir) by (offset,size)
parse CharInfo:
    version, localeTable(offset,size), characterGuid, width, height,
        transparencyIndex, styleFlags, reserved
    if styleFlags & 0x20:    parse Voice/TTS block
    if styleFlags & 0x200:   parse Word-balloon block
    parse Palette (u32 count + RGBQUAD[count])
    parse Tray icon (u8 present + optional two chunks)
    parse State->animation map (u16 count + entries)
    parse Localized info table (u16 count + entries)   ; must end at CharInfo end
parse AnimDir  -> list of {name, offset, size}
parse ImageDir -> list of {offset, size, checksum}
parse SoundDir -> list of {offset, size, checksum}
for each animation block (at its offset):
    name, flagByte, returnAnimation, frameCount, frames[]
    for each frame: overlays[], soundIndex, duration, exitFrame, branches[], mouths[]
for each image block:  present, w, h, compressed, [compressedSize], pixels(LZ or raw), region
for each sound block:  RIFF/WAVE
playback: pump frames on a timer (duration ×10 ms), select next frame via §5.2
```

## Appendix F — Validation & corpus survey

A reference parser written **only from this document** was run against Microsoft's shipping
characters and a large third-party corpus. For each file it consumes every structure — the 36-byte
header, the full Character Info block (fixed header, voice, balloon, palette, tray, state map,
localized table), the animation directory, every animation block and every frame (overlays, sound,
duration, exit, branches, mouth overlays), every image block (LZ-decoded to exactly
`roundup4(width)*height`) with its trailing region, and the sound table — and checks that each
**ends exactly on its declared size**.

### F.1 Microsoft characters (byte-exact, end-to-end)

| Character | ver | frame | anims | frames | images (LZ OK) | sounds | states | locales | result |
|-----------|-----|-------|-------|--------|----------------|--------|--------|---------|--------|
| `CLIPPIT.ACS` | 0x20001 | 124×93  | 43 | 1233 | 902 / 902 | 15 (all RIFF) | 11 | 30 | **exact** |
| `Robby.acs`   | 0x20001 | 128×128 | 68 |  927 | 593 / 593 | 32 (all RIFF) | 16 | 30 | **exact** |
| `Genie.acs`   | 0x20001 | 128×128 | 76 |  967 | 591 / 591 | 14 (all RIFF) | 16 | 30 | **exact** |
| `GENIUS.ACS`  | 0x20001 | 124×93  | 47 | 1365 | 692 / 692 | 23 (all RIFF) | 11 | 30 | **exact** |
| `Merlin.acs`  | 0x20001 | 128×128 | 73 |  945 | 614 / 614 | 32 (all RIFF) | 16 | 30 | **exact** |
| `F1.ACS`      | 0x20001 | 124×93  | 48 | 1560 | 897 / 897 | 31 (all RIFF) | 11 | 30 | **exact** |

Every image in every character LZ-decodes to precisely `roundup4(width)*height` bytes, and every
structure lands exactly on its declared boundary — confirming the container, the full Character
Info grammar, the frame/overlay/branch/mouth records, and the codec.

### F.2 Third-party corpus (~340 community characters)

A robustness sweep over a mixed library of 359 `.acs` files — Microsoft's shipping characters plus
~340 community ones, in both container generations:

| Outcome | Count | Meaning |
|---------|-------|---------|
| **Flat `.acs` (§1–§5), parsed** | **309 / 325** | consumes exactly to each structure's declared size; every image LZ-decodes to `roundup4(width)*height` |
| **OLE2 1.5 (§6), parsed** | **34 / 34** | the structured-storage form, 795 of its 802 animation streams consumed exactly (the other 7 are damaged payloads) |
| Corrupt / non-conformant | 16 | garbage version dword + garbage directory counts; rejected (see F.3) |

Feature aggregates over the valid flat files:

- **`version` is `0x20001` in all of them** — the flat single-file form is uniformly 2.0.
- **Maximum `branchCount` observed across the whole corpus is 3** (the field is a `u8` allowing up
  to 255; real characters use very short branch lists). §3.2.2.
- Roughly a third use exit-frame branches; most use per-frame mouth overlays (lip-sync).
- No structural deviations from §1–§4 in any of them — the only variability is inside the voice
  block's tail (§2.2) and in outright damage (F.3).

### F.3 Quirks & robustness notes

Real-world files include tool- and damage-related deviations that a **robust** reader should
tolerate without abandoning the canonical layout above:

- **Corrupted files (16 in the corpus).** These carry the `0xABCDABC3` magic and in-bounds header
  offsets, but the bytes at those offsets are garbage — the `version` dword is not one of the
  accepted values and the directory counts are nonsensical (billions). Example: two copies of
  `Alfred.acs` share an identical header and first ~12.9 KB, then one copy diverges into noise.
  Microsoft's own loader rejects these at the `version` check (`0x80042202`/`0x80042203`, §2.1); a
  conformant parser should do the same. **Validate the `version` dword before trusting the rest.**
- **Missing final terminator.** Some third-party authoring tools omit the `u16 0x0000` terminator
  on the *last* (empty) string of the Character Info block, so a strict parser finishes 2 bytes
  past end-of-block. Microsoft's memory-mapped loader tolerates this because it reads the absent
  terminator from the zero-filled tail of the last mapped page. Robust readers should not fail hard
  on a 2-byte shortfall at the very end of the Character Info block.
- **Non-Microsoft voice blocks.** A minority of third-party characters use a different SAPI engine
  GUID and fill in the `languageName` string that Microsoft's characters leave empty (§2.2) — the
  single most common reason a reader written against Microsoft's samples alone falls over on
  community characters. The block is still gated by the same style bit; only its tail varies.
- **General guidance.** Always walk the container by the header's `(offset,size)` fields rather than
  assuming block adjacency; treat directory counts and the `version` dword as trust gates; and read
  images by their per-record `compressedSize`/raster size rather than scanning.
