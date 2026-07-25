//! Image bitstream decompression (LZ77 + RLE).
//!
//! **Status: temporarily stubbed.** The decoder was carried over from the pre-relicense
//! GPL-derived code and is being reimplemented clean-room from the codec described
//! in [`docs/acs-format.md`](../../../docs/acs-format.md) §4. Until then [`decode_run`] yields
//! no output and [`decode_data`] reports [`Error::DecodeFailed`]. The bit layout it targets is
//! Microsoft's on-disk format (facts), so the reimplementation will reproduce the same tiered
//! back-reference encoding and 9-bit literals — from the spec, not this code.

use crate::error::{Error, Result};

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
/// **Temporarily stubbed** during the clean-room rewrite of the codec (see the module docs):
/// yields an empty buffer, which callers already treat as a blank/placeholder image.
pub fn decode_run(src: &[u8], expected: usize) -> Vec<u8> {
    let _ = (src, expected);
    Vec::new()
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
}
