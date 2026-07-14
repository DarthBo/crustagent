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
//! use crustagent_core::{sequence_animation, Player, SplitMix64};
//! # use crustagent_format::{Animation, Frame, ReturnKind};
//! # let anim = Animation { name: "x".into(), return_kind: ReturnKind::None,
//! #   return_name: String::new(),
//! #   frames: vec![Frame { duration: 10, sound_ndx: -1, exit_frame: -1,
//! #     branching: vec![], images: vec![], overlays: vec![] }] };
//! let mut rng = SplitMix64::new(0);
//! let seq = sequence_animation(&anim, &mut rng);
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

pub use balloon::{wrap_words, BalloonLayout};
pub use character::Character;
pub use idle::IdleDirector;
pub use motion::{Direction, MoveTo};
pub use player::Player;
pub use rng::{BranchRng, SplitMix64};
pub use sequence::{
    sequence_animation, sequence_exit, AnimationSequence, SeqFrame, MAX_LOOP_FRAMES, MAX_LOOP_TIME,
};
pub use text::{parse_speech, ParsedSpeech, SpeechItem, Tag};
