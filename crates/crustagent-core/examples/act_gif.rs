//! Export a Microsoft Actor (`.act`) character's artwork to an animated GIF that flips
//! through its cels — a quick way to *see* the reverse-engineered vector artwork.
//!
//! Usage: `cargo run -p crustagent-core --example act_gif -- <file.act> [out.gif] [delay_cs]`
//!
//! Actor files don't carry a decoded animation timeline yet (see `crustagent_format::act`),
//! so this cycles the cels one per frame rather than playing a real gesture. Non-WMF
//! artwork (The Genius, the classic-Mac files) can't be rendered yet and yields no frames.

use crustagent_format::act::CelFormat;
use crustagent_format::{ActFile, Rgba};
use crustagent_gif::GifBuilder;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("usage: act_gif <file.act> [out.gif] [delay_cs]");
            std::process::exit(2);
        }
    };
    let out_path = args.next().unwrap_or_else(|| "actor.gif".to_string());
    let delay_cs: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(12);

    let act = ActFile::open(&path).unwrap_or_else(|e| {
        eprintln!("parse {path}: {e}");
        std::process::exit(1);
    });

    if act.image_format != CelFormat::Wmf {
        eprintln!(
            "{}: artwork is {:?}, which isn't decoded yet — nothing to render",
            act.name, act.image_format
        );
        std::process::exit(1);
    }

    // Render every WMF cel to RGBA.
    let cels: Vec<Rgba> = (0..act.cels.len())
        .filter_map(|i| act.render_cel(i))
        .collect();
    if cels.is_empty() {
        eprintln!("{}: no renderable cels", act.name);
        std::process::exit(1);
    }

    // Canvas is the largest cel; each cel is centered on it (the cels share a logical space
    // but frame-accurate placement needs the not-yet-decoded frame table).
    let cw = cels.iter().map(|c| c.width).max().unwrap() as usize;
    let ch = cels.iter().map(|c| c.height).max().unwrap() as usize;

    // Build a palette from the distinct opaque colors across all cels. Index 0 is the
    // transparent key. Actor vector art uses few colors, so an exact table almost always
    // fits; any overflow maps to the nearest existing entry.
    let mut palette: Vec<[u8; 3]> = vec![[0, 0, 0]]; // 0 = transparent key
    let transparent = 0u8;
    for cel in &cels {
        for px in cel.pixels.chunks_exact(4) {
            if px[3] == 0 {
                continue;
            }
            let rgb = [px[0], px[1], px[2]];
            if palette.len() < 256 && !palette[1..].contains(&rgb) {
                palette.push(rgb);
            }
        }
    }

    let mut gif = GifBuilder::new(cw as u16, ch as u16, &palette, transparent);
    for cel in &cels {
        let mut frame = vec![transparent; cw * ch];
        let ox = (cw - cel.width as usize) / 2;
        let oy = (ch - cel.height as usize) / 2;
        for y in 0..cel.height as usize {
            for x in 0..cel.width as usize {
                let s = (y * cel.width as usize + x) * 4;
                if cel.pixels[s + 3] == 0 {
                    continue;
                }
                let rgb = [cel.pixels[s], cel.pixels[s + 1], cel.pixels[s + 2]];
                frame[(oy + y) * cw + (ox + x)] = nearest(&palette, rgb, transparent);
            }
        }
        gif.add_frame(&frame, delay_cs);
    }

    std::fs::write(&out_path, gif.finish()).unwrap_or_else(|e| {
        eprintln!("write {out_path}: {e}");
        std::process::exit(1);
    });
    println!(
        "wrote {out_path} ({cw}x{ch}, {} cel frame(s), {} colors)",
        cels.len(),
        palette.len()
    );
}

/// Index of the palette entry closest to `rgb` (skipping the transparent slot).
fn nearest(palette: &[[u8; 3]], rgb: [u8; 3], transparent: u8) -> u8 {
    let mut best = 1u8;
    let mut best_d = u32::MAX;
    for (i, c) in palette.iter().enumerate() {
        if i as u8 == transparent {
            continue;
        }
        let d = c
            .iter()
            .zip(rgb.iter())
            .map(|(&a, &b)| {
                let e = a as i32 - b as i32;
                (e * e) as u32
            })
            .sum();
        if d < best_d {
            best_d = d;
            best = i as u8;
            if d == 0 {
                break;
            }
        }
    }
    best
}
