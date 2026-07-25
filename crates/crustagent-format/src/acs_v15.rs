//! ACS 1.5 reader — the older Microsoft Agent format, an **OLE2 compound document**
//! (Structured Storage) rather than the flat blob of ACS 2.0.
//!
//! **Status: temporarily stubbed.** The byte-level ACS/ACF readers this shared are being
//! reimplemented clean-room (from [`docs/acs-format.md`](../../../docs/acs-format.md), §6)
//! so the crate can be relicensed. [`parse_v15`] validates the OLE2 signature and then
//! returns [`Error::Unsupported`]; the format's public signature constants remain available.
//!
//! When it returns, the reader will decode the `char.acf` header stream plus the per-animation
//! streams and normalize them into the same [`AcsFile`] the 2.0 path produces, so the rest of
//! the crate (compositor, runtime) stays oblivious to which format a character came from.

use crate::acs::AcsFile;
use crate::error::{Error, Result};

/// First 4 bytes of an OLE2 compound document (`D0 CF 11 E0` little-endian).
pub const OLE2_SIGNATURE: u32 = 0xE011_CFD0;
/// Signature DWORD at the head of the decompressed `char.acf` header stream.
pub const ACS_V15_HEADER_SIGNATURE: u32 = 0xABCD_ABC1;

/// Parse an ACS 1.5 (`OLE2`) character.
///
/// **Temporarily stubbed** during the clean-room rewrite: returns [`Error::Unsupported`]
/// after validating the OLE2 container signature.
pub fn parse_v15(bytes: Vec<u8>) -> Result<AcsFile> {
    match crate::acs::signature(&bytes) {
        Some(OLE2_SIGNATURE) => Err(Error::Unsupported(
            "the ACS 1.5 (OLE2) reader is being reimplemented clean-room and is not yet available",
        )),
        Some(found) => Err(Error::BadSignature { found }),
        None => Err(Error::UnexpectedEof {
            context: "signature",
            offset: 0,
            needed: 4,
            available: bytes.len(),
        }),
    }
}
