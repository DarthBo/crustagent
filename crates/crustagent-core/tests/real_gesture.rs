//! Exercise the multi-part gesture helpers against real characters, when present under
//! `assets/agents/` at the workspace root. Skips (passes) when no fixtures are found.

use crustagent_core::Character;
use crustagent_format::AcsFile;
use std::path::PathBuf;

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/agents")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("assets/agents"))
}

#[test]
fn full_gesture_chains_continued_and_return() {
    let dir = assets_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => {
            eprintln!("no fixtures at {} — skipping", dir.display());
            return;
        }
    };

    let mut checked = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("acs"))
        {
            continue;
        }
        let Ok(chr) = AcsFile::open(&path) else {
            continue;
        };
        let ch = Character::new(&chr);

        for name in chr.gesture_names.clone() {
            // Skip the parts themselves so we test only "base" names.
            if name.ends_with("Continued") || name.ends_with("Return") {
                continue;
            }
            let has_continued = ch.continued_animation(&name).is_some();
            let has_return = ch.return_animation(&name).is_some();

            let parts: Vec<&str> = ch
                .full_gesture(&name)
                .iter()
                .map(|a| a.name.as_str())
                .collect();

            // Base is always first. (The gesture-index name and the animation record's
            // own name can differ in case, so compare case-insensitively.)
            assert!(
                parts.first().is_some_and(|p| p.eq_ignore_ascii_case(&name)),
                "{}: base mismatch for {name}: {parts:?}",
                path.display()
            );
            // Expected length = base + continued? + return?
            let expected = 1 + has_continued as usize + has_return as usize;
            assert_eq!(
                parts.len(),
                expected,
                "{}: {name} parts {parts:?}",
                path.display()
            );

            if has_continued {
                assert!(
                    parts.contains(&format!("{name}Continued").as_str())
                        || parts
                            .iter()
                            .any(|p| p.eq_ignore_ascii_case(&format!("{name}Continued")))
                );
            }
        }

        // GetAttention is a good multi-part example. Its return can be either the
        // conventional `GetAttentionReturn` or a *named* return (e.g. `RestPose`), so
        // check against whatever `return_animation` actually resolves to.
        if ch.animation("GetAttention").is_some() {
            let parts: Vec<&str> = ch
                .full_gesture("GetAttention")
                .iter()
                .map(|a| a.name.as_str())
                .collect();
            if ch.continued_animation("GetAttention").is_some() {
                assert!(parts
                    .iter()
                    .any(|p| p.eq_ignore_ascii_case("GetAttentionContinued")));
            }
            if let Some(ret) = ch.return_animation("GetAttention") {
                assert!(
                    parts.iter().any(|p| p.eq_ignore_ascii_case(&ret.name)),
                    "{}: GetAttention return {:?} not chained: {parts:?}",
                    path.display(),
                    ret.name
                );
            }
        }
        checked += 1;
    }
    eprintln!("checked {checked} character file(s)");
}
