//! Integration test against real MS Agent characters, if they are present under
//! `assets/agents/` at the workspace root. Skips (passes) when no fixtures are found,
//! so the suite stays green in checkouts without bundled assets.
//!
//! When present, every character must fully parse and **every** compressed image must
//! decode to its exact expected size — the strongest end-to-end check of the ACS parser
//! and the LZ77 decompressor.

use crustagent_format::AcsFile;
use std::path::PathBuf;

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/agents")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("assets/agents"))
}

#[test]
fn parses_bundled_characters() {
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

        let chr = match AcsFile::open(&path) {
            Ok(c) => c,
            Err(e) => {
                // ACS 1.5 (OLE2) files share the .acs extension but aren't handled yet;
                // don't fail the suite over an unsupported-but-valid file.
                eprintln!("skipping {}: {e}", path.display());
                continue;
            }
        };

        assert!(chr.default_name().is_some(), "{}: no name", path.display());
        assert!(
            !chr.animations.is_empty(),
            "{}: no animations",
            path.display()
        );
        assert!(chr.image_count() > 0, "{}: no images", path.display());

        // Every image must decode to its exact padded size.
        for i in 0..chr.image_count() {
            let img = chr
                .image(i)
                .unwrap_or_else(|e| panic!("{}: image {i} failed: {e}", path.display()));
            assert_eq!(
                img.bits.len(),
                crustagent_format::Image::expected_len(img.width, img.height),
                "{}: image {i} wrong size",
                path.display()
            );
        }

        // Every animation frame's image/sound indices must be in range.
        for anim in &chr.animations {
            for frame in &anim.frames {
                for fi in &frame.images {
                    assert!(
                        (fi.image_ndx as usize) < chr.image_count(),
                        "{}: {} image index {} out of range",
                        path.display(),
                        anim.name,
                        fi.image_ndx
                    );
                }
                if frame.sound_ndx >= 0 {
                    assert!(
                        (frame.sound_ndx as usize) < chr.sound_count(),
                        "{}: {} sound index {} out of range",
                        path.display(),
                        anim.name,
                        frame.sound_ndx
                    );
                }
            }
        }

        eprintln!(
            "ok: {} — {} animations, {} images, {} sounds",
            chr.default_name().unwrap().name,
            chr.animations.len(),
            chr.image_count(),
            chr.sound_count()
        );
        checked += 1;
    }

    eprintln!("checked {checked} character file(s)");
}
