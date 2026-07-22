//! Drive an [`AnimationSequence`] against a monotonic clock.
//!
//! This is the piece that replaces the original's DirectShow playback: advance the player
//! by elapsed wall-clock time and ask which source animation frame should be on screen.
//! It handles looping (wrapping time into the loop tail) and completion for play-once
//! sequences. It is clock-agnostic — the caller supplies `dt` — so it is fully testable.

use crate::sequence::AnimationSequence;

/// A time-driven cursor over an [`AnimationSequence`].
#[derive(Clone, Debug)]
pub struct Player {
    seq: AnimationSequence,
    elapsed_ms: u64,
}

impl Player {
    /// Start a player at time zero for `seq`.
    pub fn new(seq: AnimationSequence) -> Player {
        Player { seq, elapsed_ms: 0 }
    }

    /// The sequence being played.
    pub fn sequence(&self) -> &AnimationSequence {
        &self.seq
    }

    /// Elapsed playback time in milliseconds.
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms
    }

    /// Restart playback from the beginning.
    pub fn reset(&mut self) {
        self.elapsed_ms = 0;
    }

    /// Advance the clock by `dt_ms`.
    pub fn advance(&mut self, dt_ms: u64) {
        self.elapsed_ms = self.elapsed_ms.saturating_add(dt_ms);
    }

    /// The effective sequence time (centiseconds) after resolving looping, or `None` if a
    /// play-once sequence has finished.
    fn time_cs(&self) -> Option<u32> {
        let total = self.seq.total_cs;
        if total == 0 {
            return None;
        }
        let t = (self.elapsed_ms / 10) as u32;
        if t < total {
            return Some(t);
        }
        match self.seq.loop_start_cs {
            Some(start) => {
                let span = total - start;
                if span == 0 {
                    None
                } else {
                    Some(start + (t - start) % span)
                }
            }
            None => None,
        }
    }

    /// True once a non-looping sequence has played past its end.
    pub fn is_finished(&self) -> bool {
        self.time_cs().is_none()
    }

    /// The index (into the source animation's `frames`) that should be displayed now, or
    /// `None` when a play-once sequence has finished.
    pub fn current_frame(&self) -> Option<usize> {
        let t = self.time_cs()?;
        self.seq.frame_at_cs(t).map(|f| f.frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequence::{AnimationSequence, SeqFrame};

    // frame 0: [0,10) ; frame 1: [10,30)
    fn play_once() -> AnimationSequence {
        AnimationSequence {
            frames: vec![
                SeqFrame {
                    frame: 0,
                    start_cs: 0,
                    duration_cs: 10,
                },
                SeqFrame {
                    frame: 1,
                    start_cs: 10,
                    duration_cs: 20,
                },
            ],
            total_cs: 30,
            loop_start_cs: None,
            truncated: false,
        }
    }

    #[test]
    fn play_once_progresses_then_finishes() {
        let mut p = Player::new(play_once());
        assert_eq!(p.current_frame(), Some(0)); // t=0
        p.advance(90); // 90ms = 9cs -> still frame 0
        assert_eq!(p.current_frame(), Some(0));
        p.advance(20); // 110ms = 11cs -> frame 1
        assert_eq!(p.current_frame(), Some(1));
        p.advance(200); // 310ms = 31cs -> past end (30cs)
        assert!(p.is_finished());
        assert_eq!(p.current_frame(), None);
    }

    #[test]
    fn looping_wraps_into_loop_tail() {
        // Same frames, but loop back to frame 1 (loop_start = 10cs, span = 20cs).
        let mut seq = play_once();
        seq.loop_start_cs = Some(10);
        let mut p = Player::new(seq);

        p.advance(300); // 30cs == total -> wraps to loop start (10cs) -> frame 1
        assert!(!p.is_finished());
        assert_eq!(p.current_frame(), Some(1));

        p.advance(200); // +20cs -> 50cs; (50-10)%20 = 0 -> 10cs -> frame 1
        assert_eq!(p.current_frame(), Some(1));

        // Intro frame 0 is never revisited once looping.
        p.reset();
        assert_eq!(p.current_frame(), Some(0));
    }

    #[test]
    fn empty_sequence_is_immediately_finished() {
        let p = Player::new(AnimationSequence::default());
        assert!(p.is_finished());
        assert_eq!(p.current_frame(), None);
    }
}
