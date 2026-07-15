//! Auto-idle behavior: when a character has nothing queued, it plays idle animations
//! that escalate through `IDLINGLEVEL1` → `2` → `3` the longer it stays idle.
//!
//! This is the portable decision logic — *which* idle animation to play next — not a
//! timer. The host/renderer decides *when* to ask (typically when the current clip ends),
//! and this picks the next animation name, escalating the level over successive turns.
//! (Like MS Agent, idle animations have no separate "exit"; the character just plays the
//! next one.)

use crate::rng::BranchRng;
use crate::Character;

/// Picks escalating idle animations for a character.
#[derive(Clone, Debug)]
pub struct IdleDirector {
    level: u8,
    max_level: u8,
    turns_at_level: u32,
    escalate_after: u32,
}

impl IdleDirector {
    /// Create a director for `character`, detecting the highest populated `IDLINGLEVEL`
    /// state (1..=3). Escalates a level every 3 idle turns by default.
    pub fn new(character: &Character) -> IdleDirector {
        let mut max_level = 1;
        for lvl in 1..=3u8 {
            if character
                .state_animations(&format!("IDLINGLEVEL{lvl}"))
                .is_some_and(|a| !a.is_empty())
            {
                max_level = lvl;
            }
        }
        IdleDirector {
            level: 1,
            max_level,
            turns_at_level: 0,
            escalate_after: 3,
        }
    }

    /// Create a director with an explicit maximum level (for tests / custom setups).
    pub fn with_levels(max_level: u8) -> IdleDirector {
        IdleDirector {
            level: 1,
            max_level: max_level.max(1),
            turns_at_level: 0,
            escalate_after: 3,
        }
    }

    /// Set how many idle turns to play before escalating a level (min 1).
    pub fn escalate_after(mut self, turns: u32) -> Self {
        self.escalate_after = turns.max(1);
        self
    }

    /// The current idle level (1-based).
    pub fn level(&self) -> u8 {
        self.level
    }

    /// The highest idle level this character defines.
    pub fn max_level(&self) -> u8 {
        self.max_level
    }

    /// Choose the next idle animation name, then advance the escalation counter.
    ///
    /// Uses the current level's `IDLINGLEVEL{level}` state, falling back to lower levels
    /// if that state is empty/missing. Returns `None` if the character has no idle states.
    pub fn next_idle(&mut self, character: &Character, rng: &mut impl BranchRng) -> Option<String> {
        let mut lvl = self.level;
        let chosen = loop {
            if let Some(anims) = character.state_animations(&format!("IDLINGLEVEL{lvl}")) {
                // Skip dangling references: some characters list idle animations that
                // aren't actually defined, so only pick names that resolve.
                let valid: Vec<&String> = anims
                    .iter()
                    .filter(|n| character.animation(n).is_some())
                    .collect();
                if !valid.is_empty() {
                    let i = (rng.roll_1_100() as usize - 1) % valid.len();
                    break Some(valid[i].clone());
                }
            }
            if lvl <= 1 {
                break None;
            }
            lvl -= 1;
        };
        self.escalate();
        chosen
    }

    fn escalate(&mut self) {
        self.turns_at_level += 1;
        if self.turns_at_level >= self.escalate_after && self.level < self.max_level {
            self.level += 1;
            self.turns_at_level = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escalates_up_to_max_then_holds() {
        let mut d = IdleDirector::with_levels(3).escalate_after(2);
        assert_eq!(d.level(), 1);
        d.escalate();
        assert_eq!(d.level(), 1); // 1 turn
        d.escalate();
        assert_eq!(d.level(), 2); // 2 turns -> level 2
        d.escalate();
        d.escalate();
        assert_eq!(d.level(), 3); // 2 more -> level 3
        for _ in 0..10 {
            d.escalate();
        }
        assert_eq!(d.level(), 3); // capped at max
    }

    #[test]
    fn single_level_never_escalates() {
        let mut d = IdleDirector::with_levels(1).escalate_after(1);
        for _ in 0..20 {
            d.escalate();
        }
        assert_eq!(d.level(), 1);
    }
}
