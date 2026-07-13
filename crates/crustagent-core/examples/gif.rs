//! Export a named animation to an animated GIF (sequencer + index-space compositor).
//!
//! Usage: `cargo run -p crustagent-core --example gif -- <file.acs> <Animation> [seed] [out.gif]`
//!
//! The GIF encoder lives in the `crustagent-gif` crate (dependency-free, round-trip tested).

use crustagent_core::{sequence_animation, Character, SplitMix64};
use crustagent_format::AcsFile;
use crustagent_gif::GifBuilder;

fn main() {
    let mut args = std::env::args().skip(1);
    let (path, anim_name) = match (args.next(), args.next()) {
        (Some(p), Some(a)) => (p, a),
        _ => {
            eprintln!("usage: gif <file.acs> <Animation> [seed] [out.gif]");
            std::process::exit(2);
        }
    };
    let seed: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let out_path = args.next().unwrap_or_else(|| format!("{anim_name}.gif"));

    let chr = AcsFile::open(&path).unwrap_or_else(|e| {
        eprintln!("parse {path}: {e}");
        std::process::exit(1);
    });

    // Play the full gesture: base + "…Continued" + "…Return" when those parts exist
    // (convention; the engine doesn't chain them). Falls back to just the base animation.
    let character = Character::new(&chr);
    let segments = character.full_gesture(&anim_name);
    if segments.is_empty() {
        eprintln!("no animation {anim_name:?}");
        std::process::exit(1);
    }

    let (w, h) = chr.header.image_size;
    let palette: Vec<[u8; 3]> = (0..256)
        .map(|i| {
            chr.header
                .palette
                .get(i)
                .map(|c| [c.r, c.g, c.b])
                .unwrap_or([0, 0, 0])
        })
        .collect();

    let mut rng = SplitMix64::new(seed);
    let mut gif = GifBuilder::new(w, h, &palette, chr.header.transparency);
    let mut total_frames = 0usize;
    let mut total_ms = 0u32;

    for anim in &segments {
        let seq = sequence_animation(anim, &mut rng);
        for e in &seq.frames {
            let frame = &anim.frames[e.frame];
            let img = chr.composite_frame_indexed(frame, None).unwrap_or_else(|err| {
                eprintln!("composite {} frame {}: {err}", anim.name, e.frame);
                std::process::exit(1);
            });
            gif.add_frame(&img.indices, e.duration_cs);
        }
        total_frames += seq.len();
        total_ms += seq.total_ms();
    }

    if total_frames == 0 {
        eprintln!("{anim_name} produced no visible frames");
        std::process::exit(1);
    }

    std::fs::write(&out_path, gif.finish()).unwrap_or_else(|e| {
        eprintln!("write {out_path}: {e}");
        std::process::exit(1);
    });

    let parts: Vec<&str> = segments.iter().map(|a| a.name.as_str()).collect();
    println!(
        "wrote {out_path} ({w}x{h}, {total_frames} frame(s), {total_ms} ms) parts: {}",
        parts.join(" + ")
    );
}
