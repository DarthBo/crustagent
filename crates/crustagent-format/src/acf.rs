// SPDX-License-Identifier: MIT OR Apache-2.0

//! Parser for the Microsoft Agent ".acf" format — the *uncompiled*, web-distributable
//! character: a small binary header file that references external ".aca" animation files
//! by name ([`docs/acs-format.md`](../../../docs/acs-format.md) §7).
//!
//! The payload is the same family of sub-blocks as the flat `.acs` (identity, palette,
//! voice, balloon, states, localized names), in a slightly different order, wrapped in a
//! length-prefixed — usually LZ-compressed — envelope. It carries no art of its own: each
//! animation entry names the `.aca` that holds its frames.
//!
//! This parses the **header**. Loading the frame/image/sound data out of the referenced
//! `.aca` files is not implemented, and there are no `.acf`/`.aca` fixtures on hand, so
//! unlike the `.acs` paths this layout is unconfirmed against a real file.

use crate::acs::{
    check_header_version, read_balloon, read_names, read_palette, read_states, read_tray_icon,
    read_tts,
};
use crate::decode::decode_data;
use crate::error::{Error, Result};
use crate::model::{char_style, Balloon, FileHeader, Name, State, Tts};
use crate::reader::{Cursor, StringForm};

/// First DWORD of an ACF file.
pub const ACF_SIGNATURE: u32 = 0xABCD_ABC4;
/// A flat `.acf` may also be stamped with this older variant of the signature.
pub const ACF_SIGNATURE_ALT: u32 = 0xABCD_ABC2;
/// Signature of the `char.acf` definition stream inside an OLE2-packaged `.acf`.
pub const ACF_STREAM_SIGNATURE: u32 = 0xABCD_ABC1;

/// `.acf` strings omit the UTF-16 terminator the flat `.acs` writes to disk (§7.2).
const ACF_STRINGS: StringForm = StringForm::Utf16;

/// Feature boundaries (Appendix A): above the first, an animation entry carries the id its
/// `.aca` must echo back; above the second, the balloon block carries a second style byte.
const VERSION_WITH_ANIM_IDS: u32 = 0x0001_001D;
const VERSION_WITH_BALLOON_STYLE: u32 = 0x0001_001E;

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
    /// Accepts a flat `.acf` and an OLE2-packaged one, whose definition lives in a
    /// `char.acf` stream framed the same way.
    pub fn parse(data: Vec<u8>) -> Result<AcfFile> {
        match crate::acs::signature(&data) {
            Some(ACF_SIGNATURE | ACF_SIGNATURE_ALT) => AcfFile::read(inflate(&data, 4)?),
            Some(crate::acs_v15::OLE2_SIGNATURE) => {
                let stream = crate::acs_v15::read_stream(&data, "char.acf")?;
                let sig = crate::acs::signature(&stream);
                if sig != Some(ACF_STREAM_SIGNATURE) {
                    return Err(Error::BadSignature {
                        found: sig.unwrap_or(0),
                    });
                }
                AcfFile::read(inflate(&stream, 4)?)
            }
            Some(found) => Err(Error::BadSignature { found }),
            None => Err(Error::UnexpectedEof {
                context: "signature",
                offset: 0,
                needed: 4,
                available: data.len(),
            }),
        }
    }

    /// Read the inflated character definition (§7.2). Its sub-blocks are the `.acs` ones in
    /// a different order — the animation table comes first, and the localized name table
    /// sits before the geometry rather than at the very end.
    fn read(payload: Vec<u8>) -> Result<AcfFile> {
        let mut c = Cursor::new(&payload);
        let version_minor = c.u16()?;
        let version_major = c.u16()?;
        let version = ((version_major as u32) << 16) | version_minor as u32;
        check_header_version(version)?;

        let anim_count = c.u16()? as usize;
        let mut animations = Vec::with_capacity(anim_count);
        for _ in 0..anim_count {
            animations.push(AcfAnimationRef {
                name: c.text(ACF_STRINGS)?,
                file_name: c.text(ACF_STRINGS)?,
                return_name: c.text(ACF_STRINGS)?,
                checksum: if version > VERSION_WITH_ANIM_IDS {
                    c.u32()?
                } else {
                    0
                },
            });
        }

        let guid = c.guid()?;
        let names = read_names(&mut c, ACF_STRINGS)?;
        let width = c.u16()?;
        let height = c.u16()?;
        let transparency = c.u8()?;
        let style = c.u32()?;
        c.skip(4)?; // reserved; 0x00000002 in the flat .acs equivalent

        let tts = (style & char_style::TTS != 0)
            .then(|| read_tts(&mut c, ACF_STRINGS))
            .transpose()?;
        let balloon = (style & char_style::BALLOON != 0)
            .then(|| read_balloon(&mut c, ACF_STRINGS, version > VERSION_WITH_BALLOON_STYLE))
            .transpose()?;
        let palette = read_palette(&mut c)?;
        read_tray_icon(&mut c)?;
        let states = read_states(&mut c, ACF_STRINGS)?;

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

    /// Look up an animation reference by name (case-insensitive, as the engine does).
    pub fn animation(&self, name: &str) -> Option<&AcfAnimationRef> {
        self.animations
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// The default (US-English preferred, else first) character name.
    pub fn default_name(&self) -> Option<&Name> {
        self.names
            .iter()
            .find(|n| n.language == 0x0409)
            .or_else(|| self.names.first())
    }
}

/// Unwrap the `{u32 uncompressedSize, u32 compressedSize, u8 data[]}` envelope that follows
/// the signature of a character definition (§6.2/§7.1). A zero compressed size means the
/// payload is stored as-is.
fn inflate(data: &[u8], at: usize) -> Result<Vec<u8>> {
    let mut c = Cursor::at(data, at);
    let inflated_len = c.u32()? as usize;
    let stored_len = c.u32()? as usize;
    if stored_len == 0 {
        return Ok(c.bytes(inflated_len)?.to_vec());
    }
    decode_data(c.bytes(stored_len)?, inflated_len)
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
