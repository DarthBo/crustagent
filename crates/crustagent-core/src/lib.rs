// SPDX-License-Identifier: MIT OR Apache-2.0

//! # crustagent-core
//!
//! The portable animation runtime for Microsoft Agent characters — the OS-independent
//! "business logic", separate from rendering, audio and speech.
//!
//! Implemented so far:
//! - [`sequence`] — flatten an animation's branching frame graph into a linear, timed
//!   [`AnimationSequence`], and build return-to-neutral exit sequences.
//! - [`player`] — drive a sequence against a monotonic clock (looping, completion).
//! - [`character`] — name/state → animation resolution over a parsed character file.
//! - [`idle`] — escalating auto-idle animation selection.
//! - [`motion`] — directional-state selection + position interpolation for moves.
//! - [`balloon`] — word-balloon text layout (wrapping).
//! - [`text`] — parse `Speak`/`Think` markup into display words + a speech directive stream.
//! - [`rng`] — deterministic, injectable branch randomness.
//!
//! Planned: the serial action queue.
//!
//! ```
//! use crustagent_core::{AnimationSequence, Player, SeqFrame};
//! // A one-frame timeline, built directly. (For a real character, [`sequence_animation`]
//! // flattens an [`Animation`](crustagent_format::Animation)'s frame graph into one.)
//! let seq = AnimationSequence {
//!     frames: vec![SeqFrame { frame: 0, start_cs: 0, duration_cs: 10 }],
//!     total_cs: 10,
//!     loop_start_cs: None,
//!     truncated: false,
//! };
//! let mut player = Player::new(seq);
//! assert_eq!(player.current_frame(), Some(0));
//! ```

pub mod balloon;
pub mod character;
pub mod idle;
pub mod motion;
pub mod player;
pub mod rng;
pub mod sequence;
pub mod text;

pub use balloon::{wrap_last_rows, wrap_words, BalloonLayout};
pub use character::Character;
pub use idle::IdleDirector;
pub use motion::{Direction, MoveTo};
pub use player::Player;
pub use rng::{BranchRng, SplitMix64};
pub use sequence::{sequence_animation, sequence_exit, AnimationSequence, SeqFrame};
pub use text::{parse_speech, pick_alternative, ParsedSpeech, SpeechItem, Tag};
