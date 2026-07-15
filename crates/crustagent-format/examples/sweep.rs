//! Coverage sweep: open every `.acs` in a directory and report what we can't handle —
//! parse errors/panics, image-decode failures, and unknown embedded-sound formats.
//!
//! Usage: `cargo run --release -p crustagent-format --example sweep -- [dir]`

use crustagent_format::AcsFile;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

fn wav_tag(b: &[u8]) -> Option<u16> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    while pos + 8 <= b.len() {
        let size = u32::from_le_bytes([b[pos + 4], b[pos + 5], b[pos + 6], b[pos + 7]]) as usize;
        if &b[pos..pos + 4] == b"fmt " && pos + 10 <= b.len() {
            return Some(u16::from_le_bytes([b[pos + 8], b[pos + 9]]));
        }
        pos += 8 + size + (size & 1);
    }
    None
}

fn main() {
    std::panic::set_hook(Box::new(|_| {})); // silence per-file panic spew
    let dir = std::env::args().nth(1).unwrap_or_else(|| "assets/agents".into());
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .map(|e| e.eq_ignore_ascii_case("acs"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();

    let total = files.len();
    let mut parse_err: Vec<(String, String)> = Vec::new();
    let mut decode_fail: Vec<(String, usize, usize)> = Vec::new();
    let mut sound_unsup: Vec<(String, Vec<u16>)> = Vec::new();
    let mut ok = 0usize;

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let parsed = catch_unwind(AssertUnwindSafe(|| AcsFile::open(path)));
        let file = match parsed {
            Err(_) => {
                parse_err.push((name, "PANIC".into()));
                continue;
            }
            Ok(Err(e)) => {
                parse_err.push((name, format!("{e}")));
                continue;
            }
            Ok(Ok(f)) => f,
        };
        ok += 1;

        let mut failed = 0usize;
        for i in 0..file.image_count() {
            let r = catch_unwind(AssertUnwindSafe(|| file.image(i)));
            if !matches!(r, Ok(Ok(_))) {
                failed += 1;
            }
        }
        if failed > 0 {
            decode_fail.push((name.clone(), failed, file.image_count()));
        }

        let mut tags: Vec<u16> = Vec::new();
        for i in 0..file.sound_count() {
            if let Some(w) = file.sound(i) {
                if let Some(t) = wav_tag(w) {
                    if !matches!(t, 1 | 2) && !tags.contains(&t) {
                        tags.push(t);
                    }
                }
            }
        }
        if !tags.is_empty() {
            sound_unsup.push((name.clone(), tags));
        }
    }

    println!("swept {total} files: {ok} parsed ok, {} failed to parse\n", parse_err.len());

    if !parse_err.is_empty() {
        println!("== PARSE FAILURES ({}) ==", parse_err.len());
        for (n, e) in &parse_err {
            println!("  {n}: {e}");
        }
        println!();
    }
    if !decode_fail.is_empty() {
        println!("== IMAGE DECODE FAILURES ({}) ==", decode_fail.len());
        for (n, f, t) in &decode_fail {
            println!("  {n}: {f}/{t} images failed");
        }
        println!();
    }
    if !sound_unsup.is_empty() {
        println!("== NON-PCM/ADPCM SOUND FORMATS ({}) ==", sound_unsup.len());
        for (n, tags) in &sound_unsup {
            let t: Vec<String> = tags.iter().map(|t| format!("0x{t:04X}")).collect();
            println!("  {n}: {}", t.join(", "));
        }
        println!();
    }
}
