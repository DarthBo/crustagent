//! Dump a summary of an ACS 2.0 character file.
//!
//! Usage: `cargo run -p crustagent-format --example dump -- path/to/Character.acs`
//!
//! Real MS Agent characters (Merlin, Genie, Peedy, Robby, …) are freely downloadable
//! and make good end-to-end fixtures.

use crustagent_format::AcsFile;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: dump <path-to.acs>");
            std::process::exit(2);
        }
    };

    let chr = match AcsFile::open(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to parse {path}: {e}");
            std::process::exit(1);
        }
    };

    let name = chr
        .default_name()
        .map(|n| n.name.as_str())
        .unwrap_or("<unnamed>");
    println!("Character : {name}");
    println!(
        "Version   : {}.{}",
        chr.header.version_major, chr.header.version_minor
    );
    println!("GUID      : {}", chr.header.guid);
    println!(
        "Frame size: {}x{}",
        chr.header.image_size.0, chr.header.image_size.1
    );
    println!(
        "Style     : 0x{:08X}   transparency index: {}",
        chr.header.style, chr.header.transparency
    );
    println!("Palette   : {} entries", chr.header.palette.len());
    if let Some(tts) = &chr.tts {
        println!("TTS       : engine {}  speed {}  pitch {}", tts.engine, tts.speed, tts.pitch);
    }
    if let Some(b) = &chr.balloon {
        println!(
            "Balloon   : {} lines x {} chars, font \"{}\"",
            b.lines, b.per_line, b.font_name
        );
    }
    println!("Names     : {}", chr.names.len());
    println!("States    : {}", chr.states.len());
    for s in &chr.states {
        println!("            {} -> {:?}", s.name, s.animations);
    }
    println!("Animations: {}", chr.animations.len());
    let total_frames: usize = chr.animations.iter().map(|a| a.frames.len()).sum();
    println!("            {total_frames} frames total");
    println!("Images    : {}", chr.image_count());
    println!("Sounds    : {}", chr.sound_count());

    // Frames referencing sounds, and the WAV format of each sound.
    let sound_frames: usize = chr
        .animations
        .iter()
        .flat_map(|a| &a.frames)
        .filter(|f| f.sound_ndx >= 0)
        .count();
    println!("            {sound_frames} frames reference a sound");
    for i in 0..chr.sound_count() {
        if let Some(wav) = chr.sound(i) {
            let tag = wav_format_tag(wav).unwrap_or(0xFFFF);
            let name = match tag {
                1 => "PCM",
                2 => "MS-ADPCM",
                6 => "A-law",
                7 => "mu-law",
                17 => "IMA-ADPCM",
                49 => "GSM 6.10",
                0xFFFF => "no fmt chunk",
                _ => "other",
            };
            // Animations whose frames reference this sound.
            let users: Vec<&str> = chr
                .animations
                .iter()
                .enumerate()
                .filter(|(_, a)| a.frames.iter().any(|f| f.sound_ndx == i as i16))
                .map(|(idx, _)| chr.gesture_names[idx].as_str())
                .collect();
            println!(
                "  sound {i:2}: {:6} bytes  fmt=0x{tag:04X} ({name})  used by: {users:?}",
                wav.len()
            );
        }
    }

    // Try decoding every image to validate the decompressor end-to-end.
    let mut ok = 0usize;
    let mut failed = 0usize;
    for i in 0..chr.image_count() {
        match chr.image(i) {
            Ok(_) => ok += 1,
            Err(_) => failed += 1,
        }
    }
    println!("Decoded   : {ok} images ok, {failed} failed");
}

/// Read a WAV's `wFormatTag` by walking RIFF chunks (fmt isn't always at offset 12).
fn wav_format_tag(b: &[u8]) -> Option<u16> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let size = u32::from_le_bytes([b[pos + 4], b[pos + 5], b[pos + 6], b[pos + 7]]) as usize;
        if id == b"fmt " && pos + 10 <= b.len() {
            return Some(u16::from_le_bytes([b[pos + 8], b[pos + 9]]));
        }
        pos += 8 + size + (size & 1);
    }
    None
}
