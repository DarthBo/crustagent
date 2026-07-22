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
//! **Fidelity note.** The branch-selection and exit-walk rules here faithfully follow the
//! original (cumulative-probability roll of `1..=100`; exit
//! via `ExitFrame` or a 100%-probability forward branch). We *deliberately deviate* on
//! one point: the original unrolls a looping animation up to a 1000-frame / `MAX_LOOP_TIME`
//! cap and truncates to whole iterations. Instead we emit the intro plus exactly one loop
//! iteration and expose the loop via [`AnimationSequence::loop_start_cs`] /
//! [`AnimationSequence::loop_duration_cs`], which is behaviorally identical for a looping
//! player and far cheaper. The runaway guards are retained as a safety net.

use crate::rng::BranchRng;
use crustagent_format::{Animation, Frame};

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
/// Frames with zero duration are traversed (their branches still count) but not emitted
/// as timeline entries, matching the original. Walk terminates when it steps past the
/// last frame, revisits a frame (loop), or trips a runaway guard.
pub fn sequence_animation(anim: &Animation, rng: &mut impl BranchRng) -> AnimationSequence {
    let count = anim.frames.len();
    let mut seq = AnimationSequence::default();
    if count == 0 {
        return seq;
    }

    // first_seen[frame] = the sequence time at which we first entered that frame.
    let mut first_seen: Vec<Option<u32>> = vec![None; count];
    let mut frame_ndx: usize = 0;
    let mut steps = 0usize;

    loop {
        if frame_ndx >= count {
            break; // fell off the end -> finite animation
        }
        if let Some(start) = first_seen[frame_ndx] {
            seq.loop_start_cs = Some(start); // revisit -> loop back to here
            break;
        }
        steps += 1;
        if steps > MAX_LOOP_FRAMES || seq.total_cs > MAX_LOOP_TIME {
            seq.truncated = true;
            break;
        }

        first_seen[frame_ndx] = Some(seq.total_cs);
        let frame = &anim.frames[frame_ndx];
        if frame.duration > 0 {
            seq.frames.push(SeqFrame {
                frame: frame_ndx,
                start_cs: seq.total_cs,
                duration_cs: frame.duration,
            });
            seq.total_cs += frame.duration as u32;
        }

        frame_ndx = next_frame(frame, frame_ndx, count, rng);
    }

    seq
}

/// Build the deterministic *exit* sequence starting at `from_frame`, used for
/// return-to-neutral when an animation ends or is interrupted.
///
/// Ports the `pExit` path of `SequenceAnimationFrames`: no RNG; each frame advances via
/// its `exit_frame` (`>= 0` jumps there, `-1` ends), or, when `exit_frame < -1`, follows
/// a 100%-probability *forward* branch; otherwise it falls through sequentially. A
/// revisited frame or a runaway guard terminates the walk.
pub fn sequence_exit(anim: &Animation, from_frame: usize) -> AnimationSequence {
    let count = anim.frames.len();
    let mut seq = AnimationSequence::default();
    if from_frame >= count {
        return seq;
    }

    let mut seen = vec![false; count];
    let mut frame_ndx: usize = from_frame;
    let mut steps = 0usize;

    loop {
        if frame_ndx >= count || seen[frame_ndx] {
            break;
        }
        steps += 1;
        if steps > MAX_LOOP_FRAMES || seq.total_cs > MAX_LOOP_TIME {
            seq.truncated = true;
            break;
        }
        seen[frame_ndx] = true;

        let frame = &anim.frames[frame_ndx];
        if frame.duration > 0 {
            seq.frames.push(SeqFrame {
                frame: frame_ndx,
                start_cs: seq.total_cs,
                duration_cs: frame.duration,
            });
            seq.total_cs += frame.duration as u32;
        }

        // Advance (deterministic).
        let next: i64 = if frame.exit_frame >= -1 {
            frame.exit_frame as i64 // -1 ends the walk
        } else if let Some(b) = frame.branching.first() {
            if b.probability == 100
                && (b.frame_ndx as i64) > frame_ndx as i64
                && (b.frame_ndx as usize) < count
            {
                b.frame_ndx as i64
            } else {
                frame_ndx as i64 + 1
            }
        } else {
            frame_ndx as i64 + 1
        };
        if next < 0 {
            break;
        }
        frame_ndx = next as usize;
    }

    seq
}

/// Pick the next frame index after `current`, following the branch table if present.
///
/// Mirrors the original: if branch slot 0 has a non-zero probability, roll `1..=100`
/// and walk the (up to 3) branch entries subtracting cumulative probabilities; the first
/// entry that drives the roll `<= 0` and targets an in-range frame wins. Otherwise (no
/// branching, or nothing selected) advance sequentially.
fn next_frame(frame: &Frame, current: usize, count: usize, rng: &mut impl BranchRng) -> usize {
    let has_branch = frame.branching.first().is_some_and(|b| b.probability != 0);
    if has_branch {
        let mut r = rng.roll_1_100() as i64;
        for b in frame.branching.iter().take(3) {
            let target = b.frame_ndx;
            if b.probability != 0 && target >= 0 && (target as usize) < count {
                r -= b.probability as i64;
                if r <= 0 {
                    return target as usize;
                }
            }
        }
    }
    current + 1
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
