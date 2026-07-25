// SPDX-License-Identifier: MIT OR Apache-2.0

//! Exercise the idle director against real characters (skips if none present).

use crustagent_core::{Character, IdleDirector, SplitMix64};
use crustagent_format::AcsFile;
use std::path::PathBuf;

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/agents")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("assets/agents"))
}

/// Every `.acs` under `assets/agents`, including in sub-directories (character libraries
/// are often filed by format). Empty when the directory is absent.
fn character_files() -> Vec<PathBuf> {
    fn collect(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for path in entries.flatten().map(|e| e.path()) {
            if path.is_dir() {
                collect(&path, out);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("acs"))
            {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    collect(&assets_dir(), &mut files);
    files.sort();
    files
}

#[test]
fn idle_picks_valid_animations_and_escalates() {
    for path in character_files() {
        let Ok(chr) = AcsFile::open(&path) else {
            continue;
        };
        let ch = Character::new(&chr);

        // Only meaningful for characters that actually define idle states.
        if ch.state_animations("IDLINGLEVEL1").is_none() {
            continue;
        }

        let mut director = IdleDirector::new(&ch).escalate_after(3);
        let mut rng = SplitMix64::new(7);

        let mut names = Vec::new();
        for _ in 0..12 {
            let name = director
                .next_idle(&ch, &mut rng)
                .expect("idle animation for a character with idle states");
            // Every chosen name must resolve to a real animation.
            assert!(
                ch.animation(&name).is_some(),
                "{}: idle picked unknown animation {name:?}",
                path.display()
            );
            names.push(name);
        }
        // After 12 turns at escalate_after=3, we should have climbed to the max level.
        assert_eq!(director.level(), director.max_level(), "{}", path.display());
        eprintln!(
            "{}: idle level reached {}/{}, samples: {:?}",
            path.display(),
            director.level(),
            director.max_level(),
            &names[..names.len().min(5)]
        );
    }
}
