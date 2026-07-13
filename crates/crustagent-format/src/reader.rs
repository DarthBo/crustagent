//! A little-endian byte cursor with the primitive readers the Agent formats use.
//!
//! Everything is little-endian (the formats were authored on x86). Strings are
//! DWORD-length-prefixed UTF-16LE; see [`Cursor::string`].

use crate::error::{Error, Result};
use crate::model::{Color, Guid};

/// A forward/seekable cursor over a byte slice.
pub struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// Create a cursor at the start of `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Cursor { buf, pos: 0 }
    }

    /// Create a cursor positioned at `pos`.
    pub fn at(buf: &'a [u8], pos: usize) -> Self {
        Cursor { buf, pos }
    }

    /// Current absolute byte offset.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Seek to an absolute byte offset.
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// Bytes remaining after the cursor.
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize, context: &'static str) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(Error::UnexpectedEof {
            context,
            offset: self.pos,
            needed: n,
            available: self.remaining(),
        })?;
        if end > self.buf.len() {
            return Err(Error::UnexpectedEof {
                context,
                offset: self.pos,
                needed: n,
                available: self.remaining(),
            });
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// Read a `u8`.
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1, "u8")?[0])
    }

    /// Read a little-endian `u16`.
    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2, "u16")?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    /// Read a little-endian `i16`.
    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    /// Read a little-endian `u32`.
    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4, "u32")?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read a little-endian `i32`.
    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    /// Read a little-endian `u64` (used for the `{offset, length}` block descriptors).
    pub fn u64(&mut self) -> Result<u64> {
        let lo = self.u32()? as u64;
        let hi = self.u32()? as u64;
        Ok(lo | (hi << 32))
    }

    /// Read a Windows `GUID` (16 bytes, mixed-endian components as laid out on disk).
    pub fn guid(&mut self) -> Result<Guid> {
        let b = self.take(16, "guid")?;
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(b);
        Ok(Guid(bytes))
    }

    /// Read a `COLORREF`/`RGBQUAD`: 4 bytes stored B, G, R, pad.
    pub fn color(&mut self) -> Result<Color> {
        let b = self.take(4, "color")?;
        Ok(Color {
            b: b[0],
            g: b[1],
            r: b[2],
        })
    }

    /// Advance past `n` bytes.
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n, "skip")?;
        Ok(())
    }

    /// Borrow `n` bytes and advance.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        self.take(n, "bytes")
    }

    /// Read a DWORD-length-prefixed UTF-16LE string.
    ///
    /// Layout: `u32 charLength` (count of UTF-16 code units, *not* bytes), then
    /// `charLength * 2` bytes of UTF-16LE. If `null_terminated` and `charLength > 0`,
    /// a trailing `0x0000` code unit follows and is consumed. An empty string
    /// (`charLength == 0`) has no terminator. This mirrors the
    /// original's length-prefixed string reader.
    pub fn string(&mut self, null_terminated: bool) -> Result<String> {
        let char_len = self.u32()? as usize;
        if char_len == 0 {
            return Ok(String::new());
        }
        let bytes = self.take(char_len * 2, "string")?;
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let s = String::from_utf16_lossy(&units);
        if null_terminated {
            self.skip(2)?;
        }
        Ok(s)
    }
}
