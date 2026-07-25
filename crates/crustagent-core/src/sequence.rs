//! Flatten an [`Animation`]'s frame graph into a linear, timed playback sequence.
//!
//! This is the frame sequencer — the piece with the highest value to get right and
//! unit-test. It walks frames, following probabilistic branches
//! (using an injectable [`BranchRng`]), accumulates each frame's start time, and detects
//! a loop so the player knows where to repeat from.
//!
//! Timing is kept in the file's native base — **centiseconds** (1/100 s) — with
//! [`AnimationSequence::total_ms`] for conversion.
//!
//! **Status: the branch/exit walk is temporarily stubbed.** The frame-graph traversal is
//! being reimplemented clean-room from the Microsoft-binary-derived spec
//! ([`docs/acs-format.md`](../../../docs/acs-format.md), §5) so the crate can be relicensed.
//! Until then [`sequence_animation`] and [`sequence_exit`] return an empty
//! [`AnimationSequence`]; the timeline types and helpers below are unaffected.

use crate::rng::BranchRng;
use crustagent_format::Animation;

/// Runaway-loop guards.
pub const MAX_LOOP_FRAMES: usize = 1000;
/// In centiseconds (the duration base).
pub const MAX_LOOP_TIME: u32 = 300_000;

/// One entry in a flattened sequence: which animation frame plays, when, for how long.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SeqFrame {
    /// Index into the source animation's `frames`.
    pub frame: usize,
    /// Start time within the sequence, in centiseconds.
    pub start_cs: u32,
    /// On-screen duration, in centiseconds.
    pub duration_cs: u16,
}

/// A linear, timed sequence produced from an animation.
#[derive(Clone, Debug, Default)]
pub struct AnimationSequence {
    /// Timeline entries, in playback order (only frames with a non-zero duration).
    pub frames: Vec<SeqFrame>,
    /// Total sequence length, in centiseconds.
    pub total_cs: u32,
    /// If the frame graph loops back on itself, the start time (cs) of the frame the
    /// loop returns to — i.e. where a looping player should seek on repeat. `None` for a
    /// finite (play-once) animation.
    pub loop_start_cs: Option<u32>,
    /// True if the walk hit a runaway-loop guard ([`MAX_LOOP_FRAMES`]/[`MAX_LOOP_TIME`]).
    pub truncated: bool,
}

impl AnimationSequence {
    /// Total duration in milliseconds.
    pub fn total_ms(&self) -> u32 {
        self.total_cs * 10
    }

    /// Length of the looping tail in centiseconds, or `None` for a play-once sequence.
    pub fn loop_duration_cs(&self) -> Option<u32> {
        self.loop_start_cs.map(|start| self.total_cs - start)
    }

    /// True if this sequence loops.
    pub fn is_looping(&self) -> bool {
        self.loop_start_cs.is_some()
    }

    /// Number of timeline entries.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// True if the sequence has no visible frames.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The timeline entry active at `time_cs` (centiseconds), if any. For looping
    /// sequences the caller should reduce `time_cs` into the loop range first.
    pub fn frame_at_cs(&self, time_cs: u32) -> Option<&SeqFrame> {
        self.frames
            .iter()
            .rev()
            .find(|f| f.start_cs <= time_cs && time_cs < f.start_cs + f.duration_cs as u32)
    }
}

/// Build the playback sequence for `anim`, resolving branches with `rng`.
///
/// **Temporarily stubbed** — returns an empty [`AnimationSequence`] while the frame-graph
/// walk is reimplemented clean-room (see the module docs). Callers keep compiling; a stubbed
/// character simply produces no timeline.
pub fn sequence_animation(anim: &Animation, rng: &mut impl BranchRng) -> AnimationSequence {
    let _ = (anim, rng);
    AnimationSequence::default()
}

/// Build the deterministic *exit* sequence starting at `from_frame`, used for
/// return-to-neutral when an animation ends or is interrupted.
///
/// **Temporarily stubbed** — returns an empty [`AnimationSequence`] while the exit walk is
/// reimplemented clean-room (see the module docs).
pub fn sequence_exit(anim: &Animation, from_frame: usize) -> AnimationSequence {
    let _ = (anim, from_frame);
    AnimationSequence::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::test_util::ScriptedRng;
    use crate::rng::SplitMix64;
    use crustagent_format::{Branch, Frame};

    fn frame(duration: u16, branches: &[(i16, u16)]) -> Frame {
        Frame {
            duration,
            sound_ndx: -1,
            exit_frame: -1,
            branching: branches
                .iter()
                .map(|&(frame_ndx, probability)| Branch {
                    frame_ndx,
                    probability,
                })
                .collect(),
            images: Vec::new(),
            overlays: Vec::new(),
        }
    }

    fn anim(frames: Vec<Frame>) -> Animation {
        Animation {
            name: "test".into(),
            return_kind: crustagent_format::ReturnKind::None,
            return_name: String::new(),
            frames,
        }
    }

    #[test]
    #[ignore = "sequencer walk stubbed during clean-room rewrite (see module docs)"]
    fn linear_animation_accumulates_time() {
        let a = anim(vec![frame(10, &[]), frame(20, &[]), frame(5, &[])]);
        let mut rng = SplitMix64::new(1);
        let seq = sequence_animation(&a, &mut rng);

        assert_eq!(seq.len(), 3);
        assert_eq!(
            seq.frames[0],
            SeqFrame {
                frame: 0,
                start_cs: 0,
                duration_cs: 10
            }
        );
        assert_eq!(
            seq.frames[1],
            SeqFrame {
                frame: 1,
                start_cs: 10,
                duration_cs: 20
            }
        );
        assert_eq!(
            seq.frames[2],
            SeqFrame {
                frame: 2,
                start_cs: 30,
                duration_cs: 5
            }
        );
        assert_eq!(seq.total_cs, 35);
        assert_eq!(seq.total_ms(), 350);
        assert_eq!(seq.loop_start_cs, None);
        assert!(!seq.truncated);
    }

    #[test]
    #[ignore = "sequencer walk stubbed during clean-room rewrite (see module docs)"]
    fn zero_duration_frames_are_traversed_not_emitted() {
        // frame 0 (dur 0) branches 100% to frame 1 (dur 10) which ends.
        let a = anim(vec![frame(0, &[(1, 100)]), frame(10, &[])]);
        let mut rng = SplitMix64::new(1);
        let seq = sequence_animation(&a, &mut rng);
        assert_eq!(seq.len(), 1);
        assert_eq!(seq.frames[0].frame, 1);
        assert_eq!(seq.total_cs, 10);
    }

    #[test]
    #[ignore = "sequencer walk stubbed during clean-room rewrite (see module docs)"]
    fn deterministic_branch_selection() {
        // frame 0: 30% -> frame 2, 70% -> frame 1. Cumulative: roll<=30 => frame2.
        let a = anim(vec![
            frame(10, &[(2, 30), (1, 70)]),
            frame(10, &[]),
            frame(10, &[]),
        ]);

        // roll 25 (<=30) picks the first branch -> frame 2.
        let mut low = ScriptedRng::new(vec![25]);
        let seq = sequence_animation(&a, &mut low);
        assert_eq!(
            seq.frames.iter().map(|f| f.frame).collect::<Vec<_>>(),
            vec![0, 2]
        );

        // roll 80 (>30, then 80-30=50<=70) picks second branch -> frame 1, then frame 2.
        let mut high = ScriptedRng::new(vec![80]);
        let seq = sequence_animation(&a, &mut high);
        assert_eq!(
            seq.frames.iter().map(|f| f.frame).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    #[ignore = "sequencer walk stubbed during clean-room rewrite (see module docs)"]
    fn detects_loop_and_reports_start() {
        // 0 -> 1 -> 2 -> back to 1 (100%). Loop starts at frame 1 (start_cs = 10).
        let a = anim(vec![frame(10, &[]), frame(20, &[]), frame(5, &[(1, 100)])]);
        let mut rng = SplitMix64::new(1);
        let seq = sequence_animation(&a, &mut rng);
        assert_eq!(
            seq.frames.iter().map(|f| f.frame).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(seq.loop_start_cs, Some(10));
        assert!(!seq.truncated);
    }

    #[test]
    #[ignore = "sequencer walk stubbed during clean-room rewrite (see module docs)"]
    fn loop_duration_derives_from_start() {
        let a = anim(vec![frame(10, &[]), frame(20, &[]), frame(5, &[(1, 100)])]);
        let mut rng = SplitMix64::new(1);
        let seq = sequence_animation(&a, &mut rng);
        assert!(seq.is_looping());
        assert_eq!(seq.loop_start_cs, Some(10));
        // total 35, loop starts at 10 -> loop tail is 25cs (frames 1 and 2).
        assert_eq!(seq.loop_duration_cs(), Some(25));
    }

    #[test]
    #[ignore = "sequencer walk stubbed during clean-room rewrite (see module docs)"]
    fn exit_walk_follows_exit_frames() {
        // frame 0 exits to frame 2; frame 1 is skipped; frame 2 ends (exit -1).
        let mut f0 = frame(10, &[]);
        f0.exit_frame = 2;
        let f1 = frame(99, &[]);
        let mut f2 = frame(5, &[]);
        f2.exit_frame = -1;
        let a = anim(vec![f0, f1, f2]);

        let seq = sequence_exit(&a, 0);
        assert_eq!(
            seq.frames.iter().map(|f| f.frame).collect::<Vec<_>>(),
            vec![0, 2]
        );
        assert_eq!(seq.total_cs, 15);
        assert!(!seq.truncated);
    }

    #[test]
    #[ignore = "sequencer walk stubbed during clean-room rewrite (see module docs)"]
    fn exit_walk_from_middle() {
        let mut f0 = frame(10, &[]);
        f0.exit_frame = -1;
        let mut f1 = frame(20, &[]);
        f1.exit_frame = -1;
        let a = anim(vec![f0, f1]);
        // Starting the exit at frame 1 plays only frame 1.
        let seq = sequence_exit(&a, 1);
        assert_eq!(
            seq.frames.iter().map(|f| f.frame).collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    #[ignore = "sequencer walk stubbed during clean-room rewrite (see module docs)"]
    fn frame_at_cs_lookup() {
        let a = anim(vec![frame(10, &[]), frame(20, &[])]);
        let mut rng = SplitMix64::new(1);
        let seq = sequence_animation(&a, &mut rng);
        assert_eq!(seq.frame_at_cs(0).unwrap().frame, 0);
        assert_eq!(seq.frame_at_cs(9).unwrap().frame, 0);
        assert_eq!(seq.frame_at_cs(10).unwrap().frame, 1);
        assert_eq!(seq.frame_at_cs(29).unwrap().frame, 1);
        assert!(seq.frame_at_cs(30).is_none());
    }
}
