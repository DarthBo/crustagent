//! A thin runtime view over a parsed character file: name→animation and state→animation
//! resolution plus convenience sequence builders.
//!
//! States are engine categories (`"SHOWING"`, `"SPEAKING"`, `"IDLINGLEVEL1"`, …) that map
//! to an *ordered* list of animation names; the engine plays the first and queues the
//! rest. Lookups are case-insensitive, matching `FindState`/`FindGesture` in the original
//! (state definitions frequently reference animations in a different case than authored).

use crate::rng::BranchRng;
use crate::sequence::{sequence_animation, sequence_exit, AnimationSequence};
use crustagent_format::{AcsFile, Animation, ReturnKind};

/// Runtime accessor over a parsed [`AcsFile`].
pub struct Character<'a> {
    file: &'a AcsFile,
}

impl<'a> Character<'a> {
    /// Wrap a parsed character file.
    pub fn new(file: &'a AcsFile) -> Character<'a> {
        Character { file }
    }

    /// The underlying parsed file.
    pub fn file(&self) -> &AcsFile {
        self.file
    }

    /// The ordered animation names for a state (e.g. `"SPEAKING"`), case-insensitive.
    pub fn state_animations(&self, state: &str) -> Option<&'a [String]> {
        self.file
            .states
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(state))
            .map(|s| s.animations.as_slice())
    }

    /// Look up an animation by name (case-insensitive).
    pub fn animation(&self, name: &str) -> Option<&'a Animation> {
        self.file.animation(name)
    }

    /// Build the playback sequence for a named animation.
    pub fn gesture_sequence(
        &self,
        name: &str,
        rng: &mut impl BranchRng,
    ) -> Option<AnimationSequence> {
        self.animation(name).map(|a| sequence_animation(a, rng))
    }

    /// Build the sequence for the animation the engine would play *first* for a state
    /// (the first entry in the state's animation list that actually exists).
    pub fn state_sequence(
        &self,
        state: &str,
        rng: &mut impl BranchRng,
    ) -> Option<AnimationSequence> {
        let names = self.state_animations(state)?;
        names
            .iter()
            .find_map(|n| self.animation(n))
            .map(|a| sequence_animation(a, rng))
    }

    /// Build the return-to-neutral (exit) sequence for a named animation, starting from
    /// its first frame. Returns `None` if the animation is unknown or has no frames.
    pub fn gesture_exit_sequence(&self, name: &str) -> Option<AnimationSequence> {
        let a = self.animation(name)?;
        if a.frames.is_empty() {
            None
        } else {
            Some(sequence_exit(a, 0))
        }
    }

    // -- Multi-part gesture conventions (start / continued / return) -----------------
    //
    // Some gestures ship as three separate animations — e.g. `GetAttention`,
    // `GetAttentionContinued`, `GetAttentionReturn`. The engine does NOT chain
    // them: `Continued` is never referenced in the runtime, and a `Return` is only
    // auto-played when the base animation's `ReturnKind` is `Named`/`ExitBranching`
    // (Merlin's `GetAttention` is `None`). These helpers resolve the convention so a host
    // can play the parts in order.

    /// The "continued"/middle animation for a gesture, by the `<name>Continued`
    /// convention, if present.
    pub fn continued_animation(&self, name: &str) -> Option<&'a Animation> {
        self.animation(&format!("{name}Continued"))
    }

    /// The return/end animation for a gesture: an explicit `ReturnName` link when the base
    /// animation declares one, otherwise the `<name>Return` naming convention, if present.
    pub fn return_animation(&self, name: &str) -> Option<&'a Animation> {
        if let Some(base) = self.animation(name) {
            if base.return_kind == ReturnKind::Named && !base.return_name.is_empty() {
                if let Some(a) = self.animation(&base.return_name) {
                    return Some(a);
                }
            }
        }
        self.animation(&format!("{name}Return"))
    }

    /// The ordered set of animations a host typically plays for a gesture: the base, then
    /// the "continued" middle (if any), then the return (if any). Empty if `name` is
    /// unknown. This is a *convention* helper — the engine itself does not chain parts.
    pub fn full_gesture(&self, name: &str) -> Vec<&'a Animation> {
        let mut out = Vec::new();
        match self.animation(name) {
            Some(base) => out.push(base),
            None => return out,
        }
        if let Some(a) = self.continued_animation(name) {
            out.push(a);
        }
        if let Some(a) = self.return_animation(name) {
            out.push(a);
        }
        out
    }
}
