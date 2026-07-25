//! Image bitstream decompression — the LZ77 codec of
//! [`docs/acs-format.md`](../../../docs/acs-format.md) §4.
//!
//! Every compressed payload in the Agent formats (image rasters, per-image regions, the ACS
//! 1.5 streams, `.acf`/`.aca` blocks and the Actor `MNAK` blocks) is the same stream: a
//! leading `0x00` flag byte, then an LSB-first bitstream of 8-bit literals and back-references
//! over four distance tiers, closed by an explicit end-of-stream token. The stream's own
//! terminator — not the byte count — defines its end, so the trailing padding a file may keep
//! after a stream is never touched.

use crate::error::{Error, Result};

/// Marks a compressed stream; anything else in `src[0]` is not one (§4.1).
const COMPRESSED_FLAG: u8 = 0x00;

/// A tier-4 back-reference whose 20-bit distance field is all ones ends the stream (§4.1).
const END_OF_STREAM: u32 = 0x000F_FFFF;

/// Ceiling on the unary match-length code. Real streams stay far below this; the cap keeps a
/// corrupt run of set bits from overflowing the shift that turns `k` into a length.
const MAX_LENGTH_CODE: u32 = 24;

/// How far past the caller's expected size a stream may run before it is written off as
/// corrupt. A handful of third-party characters encode a few bytes — usually one padded row —
/// more than their image record's `width`/`height` calls for; the original decoder simply filled
/// its fixed-size raster and dropped the rest, so a small overrun is clipped, not rejected. The
/// slack is what keeps a runaway stream from inflating without bound.
const OVERRUN_SLACK: usize = 4096;

/// Cap on the up-front output reservation, so a corrupt record claiming a gigabyte-sized raster
/// does not allocate one before its first token is even read.
const MAX_RESERVE: usize = 1 << 20;

/// Decode a compressed bitstream into exactly `expected` output bytes.
///
/// Strict wrapper over [`decode_run`]: errors unless the decode produced exactly `expected`
/// bytes (used for header/animation streams, where a short decode means corruption).
pub fn decode_data(src: &[u8], expected: usize) -> Result<Vec<u8>> {
    let out = decode_run(src, expected);
    if out.len() != expected {
        return Err(Error::DecodeFailed {
            got: out.len(),
            expected,
        });
    }
    Ok(out)
}

/// Decode a compressed bitstream, returning as many bytes as it yields (0..=`expected`).
///
/// A stream that ends on its terminator before filling `expected` bytes yields what it
/// produced — a few third-party characters ship individually truncated art, and the callers
/// pad such an image with the color key rather than failing the whole character. One that runs
/// slightly long is clipped to `expected`, as the original's fixed-size raster was. One that is
/// not decodable at all (too short, not flagged compressed, out of bits before the terminator,
/// a back-reference reaching behind the output, or output running away past `expected`) yields
/// nothing, which callers read as a blank image.
pub fn decode_run(src: &[u8], expected: usize) -> Vec<u8> {
    lz77(src, expected).unwrap_or_default()
}

/// An LSB-first bit cursor: bit `t` of the stream is bit `t & 7` of byte `t >> 3` (§4.1).
struct Bits<'a> {
    body: &'a [u8],
    cursor: usize,
}

impl Bits<'_> {
    /// The next bit, or `None` once the body is exhausted.
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.body.get(self.cursor >> 3)?;
        let bit = u32::from(byte >> (self.cursor & 7)) & 1;
        self.cursor += 1;
        Some(bit)
    }

    /// The next `n` bits assembled LSB-first (`n <= 20` in this grammar, `<= 24` for a
    /// length code).
    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut value = 0;
        for i in 0..n {
            value |= self.bit()? << i;
        }
        Some(value)
    }
}

/// The LZ77 decoder of §4.1. `None` for any stream that cannot be decoded (see
/// [`decode_run`]); `Some(out)` with `out.len() <= expected` otherwise.
fn lz77(src: &[u8], expected: usize) -> Option<Vec<u8>> {
    // A compressed stream is longer than its 8-byte minimum and flagged by a zero first byte;
    // the flag is consumed and the bitstream proper is the rest.
    if src.len() <= 7 || src[0] != COMPRESSED_FLAG {
        return None;
    }
    let mut bits = Bits {
        body: &src[1..],
        cursor: 0,
    };
    let ceiling = expected.saturating_add(OVERRUN_SLACK);
    let mut out: Vec<u8> = Vec::with_capacity(expected.min(MAX_RESERVE));

    loop {
        // One flag bit per token: 0 = literal byte, 1 = back-reference.
        if bits.bit()? == 0 {
            if out.len() >= ceiling {
                return None; // running away past the raster it claims to fill
            }
            out.push(bits.bits(8)? as u8);
            continue;
        }

        // A back-reference opens with a unary distance tier, then that tier's distance field
        // biased past the tiers below it. Tier 4 doubles as the end-of-stream marker.
        let (distance, min_length) = if bits.bit()? == 0 {
            (bits.bits(6)? as usize + 0x1, 2)
        } else if bits.bit()? == 0 {
            (bits.bits(9)? as usize + 0x41, 2)
        } else if bits.bit()? == 0 {
            (bits.bits(12)? as usize + 0x241, 2)
        } else {
            let field = bits.bits(20)?;
            if field == END_OF_STREAM {
                out.truncate(expected);
                return Some(out);
            }
            (field as usize + 0x1241, 3)
        };

        // Match length: the tier's minimum, extended by a unary-prefixed variable code.
        let mut k = 0;
        while bits.bit()? == 1 {
            k += 1;
            if k > MAX_LENGTH_CODE {
                return None;
            }
        }
        let length = if k == 0 {
            min_length
        } else {
            min_length + ((1usize << k) - 1) + bits.bits(k)? as usize
        };

        // Copy byte-by-byte so overlapping references act as run-length fills.
        let from = out.len().checked_sub(distance)?;
        if out.len() + length > ceiling {
            return None;
        }
        for i in 0..length {
            let byte = out[from + i];
            out.push(byte);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_input() {
        assert!(decode_data(&[0, 0, 0], 10).is_err());
    }

    #[test]
    fn rejects_nonzero_first_byte() {
        let mut src = vec![0xFFu8; 16];
        src[0] = 1; // must be 0
        assert!(decode_data(&src, 10).is_err());
    }

    #[test]
    fn rejects_missing_trailing_ff() {
        // Valid leading byte but no 0xFF terminator run.
        let src = vec![0u8; 16];
        assert!(decode_data(&src, 10).is_err());
    }

    #[test]
    fn decode_run_is_lenient_where_decode_data_is_strict() {
        // A bad frame yields empty (not a panic/error) from the lenient path, while the
        // strict wrapper still errors — read_image relies on this to pad short/blank images.
        let mut bad = vec![0xFFu8; 16];
        bad[0] = 1;
        assert!(decode_run(&bad, 10).is_empty());
        assert!(decode_data(&bad, 10).is_err());
        // Never yields more than `expected`.
        assert!(decode_run(&[0u8; 3], 100).len() <= 100);
    }

    /// Bit-writer mirror of [`Bits`], so the round-trip tests below encode with the same
    /// LSB-first convention the decoder reads.
    struct Writer {
        bytes: Vec<u8>,
        bits: usize,
    }

    impl Writer {
        fn new() -> Writer {
            Writer {
                bytes: vec![COMPRESSED_FLAG],
                bits: 0,
            }
        }

        fn bit(&mut self, bit: u32) {
            if self.bits.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if bit != 0 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 1 << (self.bits % 8);
            }
            self.bits += 1;
        }

        fn write(&mut self, value: u32, n: u32) {
            for i in 0..n {
                self.bit((value >> i) & 1);
            }
        }

        fn literal(&mut self, byte: u8) {
            self.bit(0);
            self.write(u32::from(byte), 8);
        }

        /// A tier-1 back-reference (`distance` 1..=64) with the given match length.
        fn back_ref(&mut self, distance: u32, length: u32) {
            self.bit(1);
            self.bit(0);
            self.write(distance - 1, 6);
            let mut k = 0;
            while (1 << (k + 1)) - 1 + 2 <= length {
                k += 1;
            }
            for _ in 0..k {
                self.bit(1);
            }
            self.bit(0);
            if k > 0 {
                self.write(length - 2 - ((1 << k) - 1), k);
            }
        }

        fn finish(mut self) -> Vec<u8> {
            self.bit(1); // back-reference token
            self.bit(1); // tier-4 prefix `111`
            self.bit(1);
            self.bit(1);
            self.write(END_OF_STREAM, 20);
            self.bytes.extend_from_slice(&[0xFF; 8]); // on-disk padding past the terminator
            self.bytes
        }
    }

    #[test]
    fn decodes_literals() {
        let mut w = Writer::new();
        for b in b"crab" {
            w.literal(*b);
        }
        assert_eq!(decode_data(&w.finish(), 4).unwrap(), b"crab");
    }

    #[test]
    fn decodes_back_references_including_overlapping_runs() {
        let mut w = Writer::new();
        w.literal(b'a');
        w.literal(b'b');
        w.back_ref(2, 4); // "abab" — copies across its own output
        w.literal(b'!');
        w.back_ref(1, 9); // a run of '!' from the byte just written
        let out = decode_data(&w.finish(), 16).unwrap();
        assert_eq!(out, b"ababab!!!!!!!!!!".to_vec());
    }

    #[test]
    fn stops_on_the_terminator_and_ignores_padding() {
        let mut w = Writer::new();
        w.literal(b'x');
        let mut stream = w.finish();
        stream.extend_from_slice(b"trailing junk that is not part of the stream");
        assert_eq!(decode_data(&stream, 1).unwrap(), b"x");
    }

    #[test]
    fn short_decode_is_returned_by_the_lenient_path() {
        let mut w = Writer::new();
        w.literal(b'x');
        let stream = w.finish();
        // Terminated cleanly, but the caller wanted a bigger raster.
        assert_eq!(decode_run(&stream, 32), b"x");
        assert!(decode_data(&stream, 32).is_err());
    }

    #[test]
    fn rejects_a_back_reference_before_the_output_start() {
        let mut w = Writer::new();
        w.literal(b'x');
        w.back_ref(8, 2); // 8 bytes back from a 1-byte output
        assert!(decode_run(&w.finish(), 16).is_empty());
    }

    #[test]
    fn clips_a_slightly_overlong_stream_to_the_expected_size() {
        // Some third-party art encodes a few bytes more than its record's dimensions call
        // for; the surplus is dropped rather than failing the image.
        let mut w = Writer::new();
        for b in b"overlong" {
            w.literal(*b);
        }
        assert_eq!(decode_data(&w.finish(), 4).unwrap(), b"over");
    }

    #[test]
    fn rejects_output_running_far_past_expected() {
        let mut w = Writer::new();
        w.literal(b'x');
        w.back_ref(1, 9000); // one token, far past a 4-byte raster plus its slack
        assert!(decode_run(&w.finish(), 4).is_empty());
    }
}
