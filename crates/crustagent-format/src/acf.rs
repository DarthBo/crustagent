//! Parser for the Microsoft Agent ".acf" format — the *uncompiled*, web-distributable
//! character: a small binary header file that references external ".aca" animation files
//! by relative path. Reverse-engineered from the original ACF header format.
//!
//! This currently parses the **header** — identity, palette, TTS/balloon metadata, states,
//! and the animation reference table (name → `.aca` file + checksum). Loading the frame /
//! image / sound data out of the external `.aca` files is not yet implemented (and there
//! are no `.acf`/`.aca` fixtures on hand to validate against — the header layout is a
//! faithful port but unverified against a real file).

use crate::error::{Error, Result};
use crate::model::{Balloon, FileHeader, Name, State, Tts};

/// First DWORD of an ACF file.
pub const ACF_SIGNATURE: u32 = 0xABCD_ABC4;

/// One animation reference: the animation's name and the external `.aca` file (relative
/// path) that holds its frames/images/sounds.
#[derive(Clone, Debug)]
pub struct AcfAnimationRef {
    pub name: String,
    /// Relative path to the external `.aca` file.
    pub file_name: String,
    pub return_name: String,
    /// Checksum that must match the one stored inside the `.aca`.
    pub checksum: u32,
}

/// A parsed ACF header.
pub struct AcfFile {
    pub header: FileHeader,
    pub tts: Option<Tts>,
    pub balloon: Option<Balloon>,
    pub names: Vec<Name>,
    pub states: Vec<State>,
    /// Animation references (to external `.aca` files).
    pub animations: Vec<AcfAnimationRef>,
}

impl AcfFile {
    /// Open and parse an `.acf` file from disk.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<AcfFile> {
        AcfFile::parse(std::fs::read(path)?)
    }

    /// Parse an in-memory `.acf` byte buffer.
    ///
    /// **Temporarily stubbed.** The ACF header parser shared its byte-level readers with the
    /// ACS parser, which is being reimplemented clean-room (see [`crate::acs`]); it currently
    /// returns [`Error::Unsupported`] after validating the signature.
    pub fn parse(data: Vec<u8>) -> Result<AcfFile> {
        match crate::acs::signature(&data) {
            Some(ACF_SIGNATURE) => Err(Error::Unsupported(
                "the .acf header parser is being reimplemented clean-room and is not yet available",
            )),
            Some(found) => Err(Error::BadSignature { found }),
            None => Err(Error::UnexpectedEof {
                context: "signature",
                offset: 0,
                needed: 4,
                available: data.len(),
            }),
        }
    }

    /// The default (US-English preferred, else first) character name.
    pub fn default_name(&self) -> Option<&Name> {
        self.names
            .iter()
            .find(|n| n.language == 0x0409)
            .or_else(|| self.names.first())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wstr(s: &str) -> Vec<u8> {
        // ACF strings: u32 char length + UTF-16LE, no NUL terminator.
        let mut v = Vec::new();
        let units: Vec<u16> = s.encode_utf16().collect();
        v.extend_from_slice(&(units.len() as u32).to_le_bytes());
        for u in &units {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }

    #[test]
    #[ignore = "ACF header parser stubbed during clean-room rewrite (see crate::acs)"]
    fn parses_synthetic_acf_header() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0u16.to_le_bytes()); // version minor
        payload.extend_from_slice(&2u16.to_le_bytes()); // version major
        payload.extend_from_slice(&1u16.to_le_bytes()); // animation count
        payload.extend_from_slice(&wstr("Wave"));
        payload.extend_from_slice(&wstr("wave.aca"));
        payload.extend_from_slice(&wstr("")); // return name
        payload.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes()); // checksum
        payload.extend_from_slice(&[0u8; 16]); // guid
                                               // names: 1 entry
        payload.extend_from_slice(&1u16.to_le_bytes());
        payload.extend_from_slice(&0x0409u16.to_le_bytes());
        payload.extend_from_slice(&wstr("genie")); // -> "Genie"
        payload.extend_from_slice(&wstr(""));
        payload.extend_from_slice(&wstr(""));
        // header tail
        payload.extend_from_slice(&128u16.to_le_bytes()); // width
        payload.extend_from_slice(&96u16.to_le_bytes()); // height
        payload.push(5); // transparency
        payload.extend_from_slice(&0x0010_0000u32.to_le_bytes()); // style = Standard
        payload.extend_from_slice(&2u32.to_le_bytes()); // reserved (0x00000002 in samples)
                                                        // palette: 2 entries
        payload.extend_from_slice(&2u32.to_le_bytes());
        payload.extend_from_slice(&[0, 0, 0, 0]);
        payload.extend_from_slice(&[255, 0, 0, 0]);
        payload.push(0); // no icon
                         // states: 1
        payload.extend_from_slice(&1u16.to_le_bytes());
        payload.extend_from_slice(&wstr("SHOWING"));
        payload.extend_from_slice(&1u16.to_le_bytes());
        payload.extend_from_slice(&wstr("SHOW"));

        let mut file = Vec::new();
        file.extend_from_slice(&ACF_SIGNATURE.to_le_bytes());
        file.extend_from_slice(&(payload.len() as u32).to_le_bytes()); // uncompressed
        file.extend_from_slice(&0u32.to_le_bytes()); // compressed = 0 (raw)
        file.extend_from_slice(&payload);

        let acf = AcfFile::parse(file).expect("parse acf");
        assert_eq!(acf.header.version_major, 2);
        assert_eq!(acf.header.image_size, (128, 96));
        assert_eq!(acf.header.transparency, 5);
        assert_eq!(acf.header.palette.len(), 2);
        assert!(acf.tts.is_none());
        assert_eq!(acf.default_name().unwrap().name, "Genie");
        assert_eq!(acf.animations.len(), 1);
        assert_eq!(acf.animations[0].name, "Wave");
        assert_eq!(acf.animations[0].file_name, "wave.aca");
        assert_eq!(acf.animations[0].checksum, 0xDEAD_BEEF);
        assert_eq!(acf.states.len(), 1);
        assert_eq!(acf.states[0].name, "SHOWING");
        assert_eq!(acf.states[0].animations, vec!["SHOW".to_string()]);
    }
}
