//! Deterministic randomness for frame-branch selection.
//!
//! The original engine calls `rand() % 100 + 1`. We abstract that behind [`BranchRng`]
//! so playback is reproducible and unit-testable (inject a scripted RNG in tests), and
//! provide a small seedable default so nothing depends on a global RNG.

/// Source of `1..=100` branch rolls.
pub trait BranchRng {
    /// Return a value in `1..=100`, matching `rand() % 100 + 1`.
    fn roll_1_100(&mut self) -> u32;
}

/// A tiny, fast, seedable PRNG (SplitMix64). Deterministic for a given seed.
#[derive(Clone, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Create a generator from `seed`.
    pub fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }

    /// Next raw 64-bit value.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl BranchRng for SplitMix64 {
    fn roll_1_100(&mut self) -> u32 {
        (self.next_u64() % 100) as u32 + 1
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::BranchRng;

    /// An RNG that replays a fixed script of rolls (cycling if exhausted). Handy for
    /// pinning branch decisions in tests.
    pub struct ScriptedRng {
        rolls: Vec<u32>,
        pos: usize,
    }

    impl ScriptedRng {
        pub fn new(rolls: Vec<u32>) -> Self {
            ScriptedRng { rolls, pos: 0 }
        }
    }

    impl BranchRng for ScriptedRng {
        fn roll_1_100(&mut self) -> u32 {
            let v = self.rolls[self.pos % self.rolls.len()];
            self.pos += 1;
            v
        }
    }
}
