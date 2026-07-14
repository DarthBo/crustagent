//! Parser for the Microsoft Agent ".acf" format — the *uncompiled*, web-distributable
//! character: a small binary header file that references external ".aca" animation files
//! by relative path. Reverse-engineered from the original ACF header format.
//!
//! This currently parses the **header** — identity, palette, TTS/balloon metadata, states,
//! and the animation reference table (name → `.aca` file + checksum). Loading the frame /
//! image / sound data out of the external `.aca` files is not yet implemented (and there
//! are no `.acf`/`.aca` fixtures on hand to validate against — the header layout is a
//! faithful port but unverified against a real file).

use crate::decode::decode_data;
use crate::error::{Error, Result};
use crate::model::{char_style, Balloon, FileHeader, Name, State, Tts};
use crate::reader::Cursor;

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
    pub fn parse(data: Vec<u8>) -> Result<AcfFile> {
        let mut head = Cursor::new(&data);
        let sig = head.u32()?;
        if sig != ACF_SIGNATURE {
            return Err(Error::BadSignature { found: sig });
        }
        let uncompressed = head.u32()? as usize;
        let compressed = head.u32()? as usize;

        // Header payload: raw when compressedSize == 0, else DecodeData-compressed.
        let payload: Vec<u8> = if compressed == 0 {
            data.get(12..12 + uncompressed)
                .ok_or(Error::UnexpectedEof {
                    context: "acf payload",
                    offset: 12,
                    needed: uncompressed,
                    available: data.len().saturating_sub(12),
                })?
                .to_vec()
        } else {
            let src = data.get(12..12 + compressed).ok_or(Error::UnexpectedEof {
                context: "acf compressed payload",
                offset: 12,
                needed: compressed,
                available: data.len().saturating_sub(12),
            })?;
            decode_data(src, uncompressed)?
        };

        let mut c = Cursor::new(&payload);
        let version_minor = c.u16()?;
        let version_major = c.u16()?;

        let anim_count = c.u16()?;
        let mut animations = Vec::with_capacity(anim_count as usize);
        for _ in 0..anim_count {
            // ACF strings are NOT null-terminated.
            let name = c.string(false)?;
            let file_name = c.string(false)?;
            let return_name = c.string(false)?;
            let checksum = c.u32()?;
            animations.push(AcfAnimationRef {
                name,
                file_name,
                return_name,
                checksum,
            });
        }

        let guid = c.guid()?;
        let names = crate::blocks::names(&mut c, false)?;
        let width = c.u16()?;
        let height = c.u16()?;
        let transparency = c.u8()?;
        let style = c.u32()?;
        let _unknown = c.u32()?; // always 2

        let tts = if style & char_style::TTS != 0 {
            Some(crate::blocks::tts(&mut c, false)?)
        } else {
            None
        };
        let balloon = if style & char_style::BALLOON != 0 {
            Some(crate::blocks::balloon(&mut c, false)?)
        } else {
            None
        };
        let palette = crate::blocks::palette(&mut c)?;
        crate::blocks::skip_icon(&mut c)?;
        let states = crate::blocks::states(&mut c, false)?;

        Ok(AcfFile {
            header: FileHeader {
                version_major,
                version_minor,
                guid,
                image_size: (width, height),
                transparency,
                style,
                palette,
            },
            tts,
            balloon,
            names,
            states,
            animations,
        })
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
        payload.extend_from_slice(&2u32.to_le_bytes()); // unknown
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
