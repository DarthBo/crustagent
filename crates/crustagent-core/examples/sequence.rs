//! Flatten a named animation into its timed playback sequence and print the timeline.
//!
//! Usage: `cargo run -p crustagent-core --example sequence -- <file.acs> <Animation> [seed]`

use crustagent_core::{sequence_animation, sequence_exit, SplitMix64};
use crustagent_format::AcsFile;

fn main() {
    let mut args = std::env::args().skip(1);
    let (path, anim_name) = match (args.next(), args.next()) {
        (Some(p), Some(a)) => (p, a),
        _ => {
            eprintln!("usage: sequence <file.acs> <Animation> [seed]");
            std::process::exit(2);
        }
    };
    let seed: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);

    let chr = AcsFile::open(&path).unwrap_or_else(|e| {
        eprintln!("parse {path}: {e}");
        std::process::exit(1);
    });
    let anim = chr.animation(&anim_name).unwrap_or_else(|| {
        eprintln!("no animation {anim_name:?}");
        std::process::exit(1);
    });

    let mut rng = SplitMix64::new(seed);
    let seq = sequence_animation(anim, &mut rng);

    println!(
        "return: {:?}{}",
        anim.return_kind,
        if anim.return_name.is_empty() {
            String::new()
        } else {
            format!(" -> {:?}", anim.return_name)
        }
    );
    println!(
        "{anim_name}: {} source frame(s) -> {} timeline entr(y/ies), {} ms{}{}",
        anim.frames.len(),
        seq.len(),
        seq.total_ms(),
        match seq.loop_start_cs {
            Some(cs) => format!(", loops from {} ms", cs * 10),
            None => ", play-once".to_string(),
        },
        if seq.truncated { " (TRUNCATED)" } else { "" }
    );
    for e in &seq.frames {
        println!(
            "  t={:>5}ms  frame {:>3}  {:>4}ms",
            e.start_cs * 10,
            e.frame,
            e.duration_cs as u32 * 10
        );
    }

    // The return (exit) walk the Agent appends for idle: from the last forward frame's
    // exit target, following exit frames back to rest.
    if let Some(last) = seq.frames.last() {
        let exit_from = anim.frames[last.frame].exit_frame;
        if exit_from >= 0 && (exit_from as usize) < anim.frames.len() {
            let ex = sequence_exit(anim, exit_from as usize);
            println!(
                "--- return walk from frame {} ({} entries, {} ms) ---",
                exit_from,
                ex.len(),
                ex.total_ms()
            );
            for e in &ex.frames {
                println!("  frame {:>3}  {:>4}ms", e.frame, e.duration_cs as u32 * 10);
            }
        }
    }

    println!("--- raw source frames ---");
    for (i, f) in anim.frames.iter().enumerate() {
        let branches: Vec<String> = f
            .branching
            .iter()
            .map(|b| format!("{}@{}%", b.frame_ndx, b.probability))
            .collect();
        println!(
            "  frame {:>3}  dur={:>4}cs  exit={:<4}  branch=[{}]",
            i,
            f.duration,
            f.exit_frame,
            branches.join(", ")
        );
    }
}
