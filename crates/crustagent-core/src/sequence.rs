// SPDX-License-Identifier: MIT OR Apache-2.0

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
//! The walk follows the playback rules in [`docs/acs-format.md`](../../../docs/acs-format.md)
//! §5: a frame holds for its duration, then the next frame is the first branch whose
//! cumulative percentage covers a single `1..=100` roll, or the next frame in order when no
//! branch fires. Running off the end stops the animation. The engine expresses looping in
//! data (a `100%` branch back to an earlier frame), so revisiting a frame is how this walk
//! finds — and reports — the loop point instead of unrolling forever.

use crate::rng::BranchRng;
use crustagent_format::{Animation, Frame};

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
    /// True if the walk stopped on its defensive frame-count guard instead of reaching a
    /// natural end (a loop point or the end of the graph). Well-formed data never trips it.
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
/// Starts at frame 0 and follows branches until the walk runs off the end of the animation
/// or returns to a frame it has already played — the latter is the loop point, reported as
/// [`AnimationSequence::loop_start_cs`].
pub fn sequence_animation(anim: &Animation, rng: &mut impl BranchRng) -> AnimationSequence {
    let mut walk = Walk::new(anim.frames.len());
    let mut index = 0usize;
    while let Some(frame) = anim.frames.get(index) {
        if !walk.play(index, frame.duration) {
            break;
        }
        index = branch_target(frame, rng).unwrap_or(index + 1);
    }
    walk.finish()
}

/// Build the deterministic *exit* sequence starting at `from_frame`, used for
/// return-to-neutral when an animation ends or is interrupted.
///
/// Where the forward walk rolls the dice, this one follows each frame's `exit_frame` — the
/// wind-down pose the author designated for an interrupted animation — and stops as soon as
/// a frame declines to name a successor.
pub fn sequence_exit(anim: &Animation, from_frame: usize) -> AnimationSequence {
    let mut walk = Walk::new(anim.frames.len());
    let mut index = from_frame;
    while let Some(frame) = anim.frames.get(index) {
        if !walk.play(index, frame.duration) {
            break;
        }
        match exit_target(frame, anim.frames.len()) {
            Some(next) => index = next,
            None => break,
        }
    }
    walk.finish()
}

/// Resolve a frame's branch table against a single `1..=100` roll, cumulatively: the first
/// branch whose probability covers the roll wins, and each miss consumes its share of the
/// roll. `None` means no branch fired (the probabilities did not add up to 100, or the
/// target is not a usable frame index) and the caller should advance in order.
fn branch_target(frame: &Frame, rng: &mut impl BranchRng) -> Option<usize> {
    if frame.branching.is_empty() {
        return None;
    }
    let mut roll = rng.roll_1_100();
    for branch in &frame.branching {
        let share = branch.probability as u32;
        if roll <= share {
            return usize::try_from(branch.frame_ndx).ok();
        }
        roll -= share;
    }
    None
}

/// The frame an interrupted animation winds down to from `frame`, if it names one that is
/// in range. The sentinels (`-1` "stop here", `-2` "nothing special") both end the walk:
/// with no interrupt still pending there is nothing left to wind down.
fn exit_target(frame: &Frame, frame_count: usize) -> Option<usize> {
    usize::try_from(frame.exit_frame)
        .ok()
        .filter(|&next| next < frame_count)
}

/// Shared bookkeeping for both walks: remembers when each source frame was reached (so a
/// revisit is recognised as the loop point), emits the frames that are actually on screen,
/// and trips the runaway guards.
struct Walk {
    seq: AnimationSequence,
    /// Time at which each source frame was first reached, or `None` if not yet played.
    reached_cs: Vec<Option<u32>>,
    steps: usize,
}

impl Walk {
    fn new(frame_count: usize) -> Walk {
        Walk {
            seq: AnimationSequence::default(),
            reached_cs: vec![None; frame_count],
            steps: 0,
        }
    }

    /// Play source frame `index`. Returns `false` when the walk must end here: either the
    /// frame has been played before (its start time becomes the loop point) or a guard
    /// tripped. Zero-duration frames are recorded but never emitted — they exist only to
    /// route the graph.
    fn play(&mut self, index: usize, duration_cs: u16) -> bool {
        if let Some(start_cs) = self.reached_cs[index] {
            self.seq.loop_start_cs = Some(start_cs);
            return false;
        }
        // Defensive net only. The revisit check above already bounds the walk: each frame is
        // played at most once (a repeat is the loop point, §5), so `steps` can never exceed the
        // frame count regardless of which branches the RNG takes. Microsoft's engine keeps no
        // runaway counter at all (acs-format.md §5.4); this cap is ours, derived from the data.
        if self.steps >= self.reached_cs.len() {
            self.seq.truncated = true;
            return false;
        }
        self.steps += 1;
        self.reached_cs[index] = Some(self.seq.total_cs);
        if duration_cs > 0 {
            self.seq.frames.push(SeqFrame {
                frame: index,
                start_cs: self.seq.total_cs,
                duration_cs,
            });
            self.seq.total_cs += duration_cs as u32;
        }
        true
    }

    fn finish(self) -> AnimationSequence {
        self.seq
    }
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
