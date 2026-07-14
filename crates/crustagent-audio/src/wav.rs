//! A small WAV decoder for the formats Microsoft Agent characters actually use: 8-/16-bit
//! **PCM** and **MS-ADPCM** (`WAVE_FORMAT_ADPCM`, 0x0002) — the latter is what most Agent
//! sound effects are stored as, and stock audio decoders don't handle it. Returns
//! interleaved `i16` samples so a player can treat everything as PCM.

/// MS-ADPCM delta adaptation table.
const ADAPT: [i32; 16] = [
    230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230,
];
/// Default MS-ADPCM coefficient pairs (used if the file omits them).
const DEFAULT_COEF: [(i32, i32); 7] = [
    (256, 0),
    (512, -256),
    (0, 0),
    (192, 64),
    (240, 0),
    (460, -208),
    (392, -232),
];

/// A decoded clip: channel count, sample rate (Hz), and interleaved `i16` samples.
pub struct Pcm {
    pub channels: u16,
    pub sample_rate: u32,
    pub samples: Vec<i16>,
}

struct Fmt {
    tag: u16,
    channels: u16,
    sample_rate: u32,
    block_align: u16,
    bits: u16,
    coef: Vec<(i32, i32)>,
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn i16le(b: &[u8], o: usize) -> i32 {
    i16::from_le_bytes([b[o], b[o + 1]]) as i32
}

/// Decode a RIFF/WAVE clip. Returns `None` if it isn't a WAV we understand.
pub fn decode(bytes: &[u8]) -> Option<Pcm> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    // Walk chunks for "fmt " and "data".
    let mut fmt: Option<Fmt> = None;
    let mut data: Option<&[u8]> = None;
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32le(bytes, pos + 4) as usize;
        let body_start = pos + 8;
        let body_end = (body_start + size).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => fmt = parse_fmt(body),
            b"data" => data = Some(body),
            _ => {}
        }
        pos = body_end + (size & 1); // chunks are word-aligned
    }

    let fmt = fmt?;
    let data = data?;
    if fmt.channels == 0 || fmt.sample_rate == 0 {
        return None;
    }
    let samples = match fmt.tag {
        1 => decode_pcm(data, fmt.bits)?,
        2 => decode_ms_adpcm(data, &fmt),
        _ => return None, // unsupported (e.g. a/mu-law, GSM) — silent rather than noise
    };
    Some(Pcm {
        channels: fmt.channels,
        sample_rate: fmt.sample_rate,
        samples,
    })
}

fn parse_fmt(b: &[u8]) -> Option<Fmt> {
    if b.len() < 16 {
        return None;
    }
    let tag = u16le(b, 0);
    let mut coef = Vec::new();
    if tag == 2 && b.len() >= 20 {
        // cbSize(16), samplesPerBlock(18), numCoef(20), then coef pairs.
        let num_coef = u16le(b, 20) as usize;
        let mut o = 22;
        for _ in 0..num_coef {
            if o + 4 > b.len() {
                break;
            }
            coef.push((i16le(b, o), i16le(b, o + 2)));
            o += 4;
        }
    }
    if coef.is_empty() {
        coef = DEFAULT_COEF.to_vec();
    }
    Some(Fmt {
        tag,
        channels: u16le(b, 2),
        sample_rate: u32le(b, 4),
        block_align: u16le(b, 12),
        bits: u16le(b, 14),
        coef,
    })
}

fn decode_pcm(data: &[u8], bits: u16) -> Option<Vec<i16>> {
    match bits {
        16 => Some(
            data.chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect(),
        ),
        8 => Some(data.iter().map(|&b| ((b as i16) - 128) << 8).collect()),
        _ => None,
    }
}

/// One 4-bit ADPCM nibble → next sample, updating the per-channel predictor state.
#[inline]
fn adpcm_nibble(nib: u8, c1: i32, c2: i32, delta: &mut i32, s1: &mut i32, s2: &mut i32) -> i16 {
    let signed = if nib >= 8 { nib as i32 - 16 } else { nib as i32 };
    let pred = (*s1 * c1 + *s2 * c2) >> 8;
    let val = (pred + signed * *delta).clamp(-32768, 32767);
    *delta = (ADAPT[nib as usize] * *delta) >> 8;
    if *delta < 16 {
        *delta = 16;
    }
    *s2 = *s1;
    *s1 = val;
    val as i16
}

fn decode_ms_adpcm(data: &[u8], fmt: &Fmt) -> Vec<i16> {
    let ch = fmt.channels.min(2) as usize;
    let block_align = (fmt.block_align as usize).max(7 * ch);
    let mut out = Vec::new();

    for block in data.chunks(block_align) {
        if block.len() < 7 * ch {
            break;
        }
        let mut p = 0;
        let (mut c1, mut c2, mut delta, mut s1, mut s2) =
            ([0i32; 2], [0i32; 2], [0i32; 2], [0i32; 2], [0i32; 2]);
        for c in c1.iter_mut().zip(c2.iter_mut()).take(ch) {
            let bp = block[p] as usize;
            let (a, b) = fmt.coef.get(bp).copied().unwrap_or((256, 0));
            *c.0 = a;
            *c.1 = b;
            p += 1;
        }
        for d in delta.iter_mut().take(ch) {
            *d = i16le(block, p);
            p += 2;
        }
        for s in s1.iter_mut().take(ch) {
            *s = i16le(block, p);
            p += 2;
        }
        for s in s2.iter_mut().take(ch) {
            *s = i16le(block, p);
            p += 2;
        }
        // Priming samples, output oldest-first, interleaved by channel.
        for &s in s2.iter().take(ch) {
            out.push(s as i16);
        }
        for &s in s1.iter().take(ch) {
            out.push(s as i16);
        }
        // Remaining bytes: two nibbles each. Mono → two samples of ch0; stereo → L,R.
        for &byte in &block[p..] {
            let hi = byte >> 4;
            let lo = byte & 0x0F;
            if ch == 1 {
                out.push(adpcm_nibble(hi, c1[0], c2[0], &mut delta[0], &mut s1[0], &mut s2[0]));
                out.push(adpcm_nibble(lo, c1[0], c2[0], &mut delta[0], &mut s1[0], &mut s2[0]));
            } else {
                out.push(adpcm_nibble(hi, c1[0], c2[0], &mut delta[0], &mut s1[0], &mut s2[0]));
                out.push(adpcm_nibble(lo, c1[1], c2[1], &mut delta[1], &mut s1[1], &mut s2[1]));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(fmt_tag: u16, fmt_body: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&0u32.to_le_bytes()); // riff size (unused by decoder)
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&(fmt_body.len() as u32).to_le_bytes());
        v.extend_from_slice(fmt_body);
        let _ = fmt_tag;
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn decodes_pcm16() {
        // fmt: tag1, 1ch, 8000Hz, avg, blockalign2, 16bit
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&8000u32.to_le_bytes());
        fmt.extend_from_slice(&16000u32.to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        let data: Vec<u8> = [1i16, -1, 1000, -1000]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let pcm = decode(&wav(1, &fmt, &data)).expect("decode");
        assert_eq!(pcm.channels, 1);
        assert_eq!(pcm.sample_rate, 8000);
        assert_eq!(pcm.samples, vec![1, -1, 1000, -1000]);
    }

    #[test]
    fn decodes_ms_adpcm_priming() {
        // Minimal MS-ADPCM: fmt with default coef table + a single 7-byte block (header
        // only), so output is just the two priming samples [samp2, samp1].
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&2u16.to_le_bytes()); // tag ADPCM
        fmt.extend_from_slice(&1u16.to_le_bytes()); // channels
        fmt.extend_from_slice(&8000u32.to_le_bytes());
        fmt.extend_from_slice(&8000u32.to_le_bytes());
        fmt.extend_from_slice(&7u16.to_le_bytes()); // block align = 7 (header only)
        fmt.extend_from_slice(&4u16.to_le_bytes()); // bits
        fmt.extend_from_slice(&4u16.to_le_bytes()); // cbSize
        fmt.extend_from_slice(&2u16.to_le_bytes()); // samplesPerBlock
        fmt.extend_from_slice(&0u16.to_le_bytes()); // numCoef 0 -> defaults
        let mut data = Vec::new();
        data.push(0u8); // predictor index 0
        data.extend_from_slice(&16i16.to_le_bytes()); // delta
        data.extend_from_slice(&100i16.to_le_bytes()); // samp1
        data.extend_from_slice(&200i16.to_le_bytes()); // samp2
        let pcm = decode(&wav(2, &fmt, &data)).expect("decode");
        assert_eq!(pcm.samples, vec![200, 100]); // samp2 then samp1
    }
}
