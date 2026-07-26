// SPDX-License-Identifier: MIT OR Apache-2.0

//! Show which system voice each character would speak in.
//!
//! Usage: `cargo run -p crustagent-tts --example voices -- path/to/characters/`

use crustagent_format::AcsFile;
use crustagent_tts::{SystemTts, TtsEngine, VoiceRequest};

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: voices <dir-of-acs-files>");
        std::process::exit(2);
    });
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("read the character directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("acs")))
        .collect();
    paths.sort();

    // One engine, reused: the platform backends generally allow a single synthesizer per
    // process (macOS `AVSpeechSynthesizer` refuses a second one).
    let mut engine = SystemTts::default();

    for path in paths {
        let Ok(chr) = AcsFile::open(&path) else {
            continue;
        };
        let name = chr
            .default_name()
            .map(|n| n.name.as_str())
            .unwrap_or("<unnamed>")
            .to_string();
        let want = chr
            .tts
            .as_ref()
            .map(VoiceRequest::from_tts)
            .unwrap_or_default();

        engine.set_voice(want);
        // A character that states no gender leaves the engine on whatever voice it already
        // had — the system default on a fresh engine, the previous pick on a shared one.
        println!(
            "{name:<24} {:<12} {:<4} -> {}",
            format!("{:?}", want.gender),
            want.language.unwrap_or("--"),
            engine.voice_name().unwrap_or("(system default)")
        );
    }
}
