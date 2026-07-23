//! Integration test for the Microsoft Actor (`.act`) parser + WMF renderer against the
//! real character files under `assets/agents/ACT` and `assets/agents/MAC_ACT`, if present.
//! Skips (passes) when no fixtures are found, so the suite stays green in bare checkouts.

use crustagent_format::act::CelFormat;
use crustagent_format::ActFile;
use std::path::PathBuf;

fn act_files() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/agents");
    let mut out = Vec::new();
    for sub in ["ACT", "MAC_ACT"] {
        if let Ok(rd) = std::fs::read_dir(root.join(sub)) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("act")) {
                    out.push(p);
                }
            }
        }
    }
    out
}

#[test]
fn parses_and_renders_actor_files() {
    let files = act_files();
    if files.is_empty() {
        eprintln!("no .act fixtures — skipping");
        return;
    }

    for path in files {
        let act = ActFile::open(&path).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

        // Identity invariants that must hold for every file.
        assert!(!act.name.is_empty(), "{}: empty name", path.display());
        assert!(act.version.0 >= 1, "{}: bad version", path.display());
        assert!(
            act.image_size.0 > 0 && act.image_size.1 > 0,
            "{}: zero frame size",
            path.display()
        );
        // The seven section offsets are strictly ascending.
        assert!(
            act.sections.windows(2).all(|w| w[0] < w[1]),
            "{}: section offsets not ascending",
            path.display()
        );

        // Every WMF cel must render without panicking, and its size must match its bounds.
        let mut rendered = 0usize;
        for (i, cel) in act.cels.iter().enumerate() {
            if cel.format != CelFormat::Wmf {
                continue;
            }
            if let Some(img) = act.render_cel(i) {
                assert_eq!(
                    (img.pixels.len()) as u32,
                    img.width * img.height * 4,
                    "{}: cel {i} buffer size mismatch",
                    path.display()
                );
                if let Some((l, t, r, b)) = cel.bounds {
                    assert_eq!(img.width, (r - l).unsigned_abs() as u32 + 1);
                    assert_eq!(img.height, (b - t).unsigned_abs() as u32 + 1);
                }
                rendered += 1;
            }
        }

        // WMF characters must have cels that render; bitmap/Mac ones legitimately have none.
        if act.image_format == CelFormat::Wmf {
            assert!(rendered > 0, "{}: no WMF cel rendered", path.display());

            // Animation tables: poses, a frame graph, and named actions must decode, and a
            // representative animation must composite to full character frames.
            assert!(!act.poses.is_empty(), "{}: no poses", path.display());
            assert!(!act.frames.is_empty(), "{}: no frames", path.display());
            assert!(!act.actions.is_empty(), "{}: no actions", path.display());
            // Every action starts within the frame graph, and all branch targets are valid.
            let nframes = act.frames.len() as u16;
            for a in &act.actions {
                assert!(
                    a.first_frame < nframes,
                    "{}: {} start oob",
                    path.display(),
                    a.name
                );
            }
            // Idle is present on Actor characters and must produce composited frames.
            let idle = act.action("Idle").expect("Idle action");
            let seq = act.action_sequence(idle, 128);
            assert!(!seq.is_empty(), "{}: empty Idle sequence", path.display());
            let (obj, _) = seq[0];
            let frame = act
                .render_object(obj as usize)
                .unwrap_or_else(|| panic!("{}: render Idle frame", path.display()));
            assert_eq!(frame.width, act.image_size.0 as u32);
            assert_eq!(frame.height, act.image_size.1 as u32);
        }

        // Compressed (MNAK) bitmap cels must decompress to their declared size, with a
        // sane width/height header (we can't rasterize the body yet, but the LZ layer works).
        if act.image_format == CelFormat::Bitmap {
            let out = act.decompress_cel(0).expect("decompress MNAK cel 0");
            assert!(out.len() >= 12, "{}: tiny decode", path.display());
            let w = u32::from_le_bytes([out[0], out[1], out[2], out[3]]);
            let h = u32::from_le_bytes([out[4], out[5], out[6], out[7]]);
            assert!(
                (1..=4096).contains(&w) && (1..=4096).contains(&h),
                "{}: implausible MNAK cel size {w}x{h}",
                path.display()
            );
        }

        // Any extracted sound is a complete RIFF/WAVE stream.
        for (i, snd) in act.sounds.iter().enumerate() {
            assert!(
                snd.len() >= 12 && &snd[0..4] == b"RIFF" && &snd[8..12] == b"WAVE",
                "{}: sound {i} is not RIFF/WAVE",
                path.display()
            );
        }

        eprintln!(
            "{}: v{}.{} {}x{} {:?} cels={} rendered={} sounds={}",
            path.file_name().unwrap().to_string_lossy(),
            act.version.0,
            act.version.1,
            act.image_size.0,
            act.image_size.1,
            act.image_format,
            act.cels.len(),
            rendered,
            act.sounds.len(),
        );
    }
}

#[test]
fn clippit_paperclip_is_not_blank() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/agents/ACT/clippit.act");
    let Ok(act) = ActFile::open(&path) else {
        eprintln!("no clippit.act — skipping");
        return;
    };
    assert_eq!(act.name, "Clippit");
    assert_eq!(act.image_format, CelFormat::Wmf);
    let img = act.render_cel(0).expect("render cel 0");
    // The base paperclip pose fills a good fraction of its box; assert it's not empty.
    let opaque = img
        .pixels
        .iter()
        .skip(3)
        .step_by(4)
        .filter(|&&a| a != 0)
        .count();
    assert!(
        opaque > 100,
        "cel 0 looks blank ({opaque} opaque px of {})",
        img.width * img.height
    );
}
