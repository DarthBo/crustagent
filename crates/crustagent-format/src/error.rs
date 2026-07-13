//! Error types for the format layer.

use std::fmt;

/// Result alias for the format crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong reading a character file.
#[derive(Debug)]
pub enum Error {
    /// Underlying I/O failure (opening / reading the file).
    Io(std::io::Error),
    /// A read ran past the end of the buffer.
    UnexpectedEof {
        /// What the reader was trying to read.
        context: &'static str,
        /// Byte offset at which the read was attempted.
        offset: usize,
        /// Number of bytes needed.
        needed: usize,
        /// Number of bytes actually available.
        available: usize,
    },
    /// The file's leading signature did not match a known format.
    BadSignature {
        /// The signature DWORD found at offset 0.
        found: u32,
    },
    /// An image record was structurally invalid (bad header / dimensions).
    BadImage {
        /// Zero-based image index.
        index: usize,
    },
    /// Image decompression produced the wrong number of bytes.
    DecodeFailed {
        /// Bytes actually produced.
        got: usize,
        /// Bytes expected (the frame's padded 8-bpp size).
        expected: usize,
    },
    /// A value in the file was out of the expected range.
    InvalidData(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "i/o error: {e}"),
            Error::UnexpectedEof {
                context,
                offset,
                needed,
                available,
            } => write!(
                f,
                "unexpected end of data while reading {context}: need {needed} byte(s) at offset {offset}, but only {available} available"
            ),
            Error::BadSignature { found } => {
                write!(f, "unrecognized file signature 0x{found:08X}")
            }
            Error::BadImage { index } => write!(f, "invalid image record at index {index}"),
            Error::DecodeFailed { got, expected } => {
                write!(f, "image decode produced {got} bytes, expected {expected}")
            }
            Error::InvalidData(msg) => write!(f, "invalid data: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
