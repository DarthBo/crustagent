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
            // RIFF: "fmt " chunk at offset 12; wFormatTag is 2 bytes at offset 20.
            let tag = if wav.len() >= 22 && &wav[0..4] == b"RIFF" && &wav[8..12] == b"WAVE" {
                u16::from_le_bytes([wav[20], wav[21]])
            } else {
                0xFFFF
            };
            let name = match tag {
                1 => "PCM",
                2 => "MS-ADPCM",
                6 => "A-law",
                7 => "mu-law",
                17 => "IMA-ADPCM",
                49 => "GSM 6.10",
                0xFFFF => "not-RIFF",
                _ => "other",
            };
            println!("  sound {i:2}: {:6} bytes  fmt=0x{tag:04X} ({name})", wav.len());
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
