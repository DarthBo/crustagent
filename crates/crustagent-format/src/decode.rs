//! Image bitstream decompression — a faithful port of the original decoder.
//!
//! It is a bit-oriented LZ77 variant: a control bit selects a back-reference copy
//! (with a tiered distance field) or a 9-bit literal palette index. The output is a
//! raw 8-bpp index buffer that exactly fills the frame's padded DIB size.

use crate::error::{Error, Result};

/// Read a little-endian `u32` window of 4 bytes starting at `i`, padding any bytes
/// at/after the end of `src` with `0xFF`.
///
/// The original reads `*(DWORD*)(srcPtr - 4)` from a memory-mapped file whose data is
/// followed by `0xFF` padding; near the tail it can read a couple of bytes past the
/// logical end. Padding with `0xFF` reproduces that safely without unsafe reads.
#[inline]
fn window(src: &[u8], i: usize) -> u32 {
    let mut v = 0u32;
    for k in 0..4 {
        let b = src.get(i + k).copied().unwrap_or(0xFF);
        v |= (b as u32) << (k * 8);
    }
    v
}

/// Decode a compressed image bitstream into exactly `expected` output bytes.
///
/// Framing requirements (else the stream is rejected): `src.len() > 7`, `src[0] == 0`,
/// and at least 6 trailing `0xFF` bytes.
pub fn decode_data(src: &[u8], expected: usize) -> Result<Vec<u8>> {
    if src.len() <= 7 || src[0] != 0 {
        return Err(Error::DecodeFailed {
            got: 0,
            expected,
        });
    }

    // Require >= 6 trailing 0xFF bytes (the terminator/padding marker). This mirrors
    // the original loop exactly: bc counts consecutive 0xFF from the end (capped at 7).
    {
        let mut bc = 1usize;
        loop {
            if src[src.len() - bc] != 0xFF {
                break;
            }
            if bc > 6 {
                break;
            }
            bc += 1;
        }
        if bc < 6 {
            return Err(Error::DecodeFailed { got: 0, expected });
        }
    }

    let mut out: Vec<u8> = Vec::with_capacity(expected);
    let mut sp: usize = 5; // source position (== lSrcPtr - src)
    let mut bit: u32 = 0; // bit offset within the current window

    while sp < src.len() && out.len() < expected {
        let mut quad = window(src, sp - 4);

        if quad & (1u32 << bit) != 0 {
            // ---- back-reference copy ----
            let mut off_extra = 1usize;
            let dist: usize;

            if quad & (1u32 << (bit + 1)) != 0 {
                if quad & (1u32 << (bit + 2)) != 0 {
                    if quad & (1u32 << (bit + 3)) != 0 {
                        // 20-bit distance field
                        quad >>= bit + 4;
                        quad &= 0x000F_FFFF;
                        if quad == 0x000F_FFFF {
                            break; // end-of-stream marker
                        }
                        dist = (quad + 4673) as usize;
                        bit += 24;
                        off_extra = 2;
                    } else {
                        // 12-bit distance field
                        quad >>= bit + 4;
                        quad &= 0x0000_0FFF;
                        dist = (quad + 577) as usize;
                        bit += 16;
                    }
                } else {
                    // 9-bit distance field
                    quad >>= bit + 3;
                    quad &= 0x0000_01FF;
                    dist = (quad + 65) as usize;
                    bit += 12;
                }
            } else {
                // 6-bit distance field
                quad >>= bit + 2;
                quad &= 0x0000_003F;
                dist = (quad + 1) as usize;
                bit += 8;
            }

            // Advance the byte pointer for the distance field, then read the run-length.
            sp += (bit / 8) as usize;
            bit &= 7;
            let runq = window(src, sp - 4);

            // Unary run of set bits (capped at 11) gives the length magnitude class.
            let mut rc: u32 = 0;
            while runq & (1u32 << (bit + rc)) != 0 {
                rc += 1;
                if rc > 11 {
                    break;
                }
            }
            let mut run_len = ((runq >> (bit + rc + 1)) & ((1u32 << rc) - 1)) as usize;
            run_len += 1usize << rc;
            run_len += off_extra;
            bit += rc * 2 + 1;

            if out.len() + run_len > expected {
                break;
            }
            if dist > out.len() {
                break;
            }
            let start = out.len() - dist;
            for k in 0..run_len {
                let byte = out[start + k]; // overlap is intentional and valid
                out.push(byte);
            }
        } else {
            // ---- literal: one 8-bit palette index ----
            quad >>= bit + 1;
            bit += 9;
            out.push((quad & 0xFF) as u8);
        }

        sp += (bit / 8) as usize;
        bit &= 7;
    }

    if out.len() != expected {
        return Err(Error::DecodeFailed {
            got: out.len(),
            expected,
        });
    }
    Ok(out)
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
}
