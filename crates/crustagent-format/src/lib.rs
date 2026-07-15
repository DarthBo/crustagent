//! # crustagent-format
//!
//! Parsers for Microsoft Agent character files (`.acs`, and later `.acf`/`.acd`). This
//! crate is the pure, dependency-free format layer: it turns bytes into a [`model`] and
//! decodes images/sounds. Runtime concerns (animation playback, rendering, speech) live
//! in higher crates.
//!
//! The byte-level format was reverse-engineered from the original character files.
//!
//! Currently implemented:
//! - **ACS 2.0** ([`AcsFile`]) — the compiled binary format (full), incl. LZ77 image
//!   decompression ([`decode::decode_data`]).
//! - **ACF** ([`AcfFile`]) — the uncompiled format's *header* (metadata + animation
//!   references to external `.aca` files); `.aca` frame/image loading is TODO.
//!
//! Planned: `.aca` bodies, ACS 1.5 (OLE2 compound document),
//! and ACD (text script).
//!
//! ```no_run
//! use crustagent_format::AcsFile;
//! let chr = AcsFile::open("Merlin.acs")?;
//! println!("{} — {} animations, {} images",
//!     chr.default_name().map(|n| n.name.as_str()).unwrap_or("?"),
//!     chr.animations.len(), chr.image_count());
//! # Ok::<(), crustagent_format::Error>(())
//! ```

pub mod acf;
pub mod acs;
pub mod acs_v15;
mod blocks;
pub mod decode;
pub mod error;
pub mod model;
pub mod reader;

pub use acf::{AcfAnimationRef, AcfFile, ACF_SIGNATURE};
pub use acs::{signature, AcsFile, ACS_SIGNATURE};
pub use error::{Error, Result};
pub use model::{
    char_style, Animation, Balloon, Branch, Color, FileHeader, Frame, FrameImage, FrameOverlay,
    Guid, Image, Indexed, MouthOverlay, Name, ReturnKind, Rgba, State, Tts,
};
