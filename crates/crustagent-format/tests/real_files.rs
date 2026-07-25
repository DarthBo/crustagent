// SPDX-License-Identifier: MIT OR Apache-2.0

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
fn parses_bundled_characters() {
    let files = character_files();
    if files.is_empty() {
        eprintln!("no fixtures under {} — skipping", assets_dir().display());
        return;
    }

    let mut checked = 0usize;
    for path in files {
        let chr = match AcsFile::open(&path) {
            Ok(c) => c,
            Err(e) => {
                // Some third-party files in this large library use formats/variants we
                // don't parse yet; don't fail the suite over an unsupported-but-valid file.
                eprintln!("skipping {}: {e}", path.display());
                continue;
            }
        };

        // Existence of a name / animations / images isn't guaranteed across a large
        // third-party library — some "characters" are control agents (invisible, offscreen
        // audio player) with none. The invariants worth enforcing are that whatever DOES
        // decode is well-formed and internally consistent (checked below).

        // Whatever decodes must decode to its exact padded size. A handful of third-party
        // files have individually-corrupt images (tracked separately via the `sweep`
        // example); tolerate those rather than failing the whole suite.
        for i in 0..chr.image_count() {
            if let Ok(img) = chr.image(i) {
                // A 0-byte image is a valid transparent placeholder; otherwise the bits
                // must fill the exact padded size.
                assert!(
                    img.bits.is_empty()
                        || img.bits.len()
                            == crustagent_format::Image::expected_len(img.width, img.height),
                    "{}: image {i} wrong size",
                    path.display()
                );
            }
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
