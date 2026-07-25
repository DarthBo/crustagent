//! # crustagent-format
//!
//! Parsers for Microsoft Agent character files (`.acs`, and later `.acf`/`.acd`). This
//! crate is the format layer: it turns bytes into a [`model`] and decodes images/sounds.
//! Runtime concerns (animation playback, rendering, speech) live in higher crates.
//!
//! The byte-level layouts are specified in [`docs/acs-format.md`](../../../docs/acs-format.md),
//! reverse-engineered from Microsoft's own binaries.
//!
//! Currently implemented:
//! - **ACS 2.0** ([`AcsFile`]) — the compiled binary format (full), incl. LZ77 image
//!   decompression ([`decode::decode_data`]).
//! - **ACS 1.5** — the OLE2 compound-document form of `.acs` (a compressed `char.acf`
//!   definition plus one stream per animation), normalized into the same [`AcsFile`].
//! - **ACF** ([`AcfFile`]) — the uncompiled format's *header* (metadata + animation
//!   references to external `.aca` files); `.aca` frame/image loading is TODO.
//! - **ACT** ([`ActFile`]) — the *Microsoft Actor* character table that preceded Agent
//!   (the Office 97/98 Assistants and Microsoft Bob), little- and big-endian. For the
//!   vector-metafile characters it parses the full model — cels, poses (layered parts),
//!   the frame graph, and named animations — and composites any frame to RGBA
//!   ([`act::ActFile::render_object`]). The bitmap characters decode too: the newer `MNAK`
//!   cels through the same LZ77 as ACS, and the classic-Mac cels through their own codec.
//!
//! Planned: `.aca` bodies and ACD (text script).
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
pub mod act;
pub mod decode;
pub mod error;
pub mod model;
pub mod reader;

pub use acf::{AcfAnimationRef, AcfFile, ACF_SIGNATURE};
pub use acs::{signature, AcsFile, ACS_SIGNATURE};
pub use act::{ActFile, Cel, CelFormat};
pub use error::{Error, Result};
pub use model::{
    char_style, Animation, Balloon, Branch, Color, FileHeader, Frame, FrameImage, FrameOverlay,
    Guid, Image, Indexed, MouthOverlay, Name, ReturnKind, Rgba, State, Tts,
};
