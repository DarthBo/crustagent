//! Export a Microsoft Actor (`.act`) animation to an animated GIF.
//!
//! Usage: `cargo run -p crustagent-core --example act_gif -- <file.act> [Action] [out.gif]`
//!
//! With an action name (e.g. `Greeting`, `Thinking`, `Searching`) the frame graph is
//! walked and each frame's pose is composited to the character's full frame. With no name
//! (or `cels`) it flips through every artwork cel instead. Bitmap characters (The Genius)
//! have no decoded frame graph yet, so they always use the cel gallery. The classic-Mac
//! artwork codec isn't rasterized yet.

use crustagent_format::act::CelFormat;
use crustagent_format::{ActFile, Rgba};
use crustagent_gif::GifBuilder;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("usage: act_gif <file.act> [Action|cels] [out.gif]");
            std::process::exit(2);
        }
    };
    let which = args.next();
    let act = ActFile::open(&path).unwrap_or_else(|e| {
        eprintln!("parse {path}: {e}");
        std::process::exit(1);
    });
    if !matches!(
        act.image_format,
        CelFormat::Wmf | CelFormat::Bitmap | CelFormat::MacBitmap
    ) {
        eprintln!(
            "{}: artwork is {:?}, which isn't rasterized yet",
            act.name, act.image_format
        );
        std::process::exit(1);
    }

    // Collect (frame, delay_cs) pairs: an action's composited poses, or the cel gallery.
    let (w, h, frames): (usize, usize, Vec<(Rgba, u16)>) =
        if which.as_deref() == Some("cels") || act.actions.is_empty() {
            cel_gallery(&act)
        } else {
            // Named action, or default to Greeting/first when none given.
            let action = which
                .as_deref()
                .and_then(|n| act.action(n))
                .or_else(|| act.action("Greeting"))
                .or_else(|| act.actions.first())
                .expect("actions non-empty");
            // `animate` composites the frames (Mac SMC characters are inter-frame video, so
            // each delta frame is drawn over the previous one).
            let frames: Vec<(Rgba, u16)> = act
                .animate(action, 200, 0x1234_5678)
                .into_iter()
                .map(|(img, dur)| (img, (dur / 10).max(2)))
                .collect();
            let (cw, ch) = frames
                .first()
                .map(|(f, _)| (f.width as usize, f.height as usize))
                .unwrap_or((act.image_size.0 as usize, act.image_size.1 as usize));
            println!("action {:?}: {} frame(s)", action.name, frames.len());
            (cw, ch, frames)
        };

    if frames.is_empty() {
        eprintln!("{}: nothing to render", act.name);
        std::process::exit(1);
    }
    let out_path = args.next().unwrap_or_else(|| "actor.gif".to_string());

    // Palette from the distinct opaque colors (index 0 = transparent key).
    let transparent = 0u8;
    let mut palette: Vec<[u8; 3]> = vec![[0, 0, 0]];
    for (img, _) in &frames {
        for px in img.pixels.chunks_exact(4) {
            if px[3] != 0 && palette.len() < 256 {
                let rgb = [px[0], px[1], px[2]];
                if !palette[1..].contains(&rgb) {
                    palette.push(rgb);
                }
            }
        }
    }

    let mut gif = GifBuilder::new(w as u16, h as u16, &palette, transparent);
    for (img, delay) in &frames {
        let mut idx = vec![transparent; w * h];
        for (i, px) in img.pixels.chunks_exact(4).enumerate() {
            if px[3] != 0 {
                idx[i] = nearest(&palette, [px[0], px[1], px[2]], transparent);
            }
        }
        gif.add_frame(&idx, *delay);
    }
    std::fs::write(&out_path, gif.finish()).unwrap_or_else(|e| {
        eprintln!("write {out_path}: {e}");
        std::process::exit(1);
    });
    println!(
        "wrote {out_path} ({w}x{h}, {} frame(s), {} colors)",
        frames.len(),
        palette.len()
    );
}

/// Fallback: one frame per artwork cel, each centered on a canvas sized to the largest cel.
fn cel_gallery(act: &ActFile) -> (usize, usize, Vec<(Rgba, u16)>) {
    let cels: Vec<Rgba> = (0..act.cels.len())
        .filter_map(|i| act.render_cel(i))
        .collect();
    let w = cels.iter().map(|c| c.width).max().unwrap_or(1) as usize;
    let h = cels.iter().map(|c| c.height).max().unwrap_or(1) as usize;
    let framed = cels
        .into_iter()
        .map(|cel| {
            let mut canvas = Rgba::transparent(w as u32, h as u32);
            let ox = (w - cel.width as usize) / 2;
            let oy = (h - cel.height as usize) / 2;
            for y in 0..cel.height as usize {
                for x in 0..cel.width as usize {
                    let s = (y * cel.width as usize + x) * 4;
                    if cel.pixels[s + 3] != 0 {
                        let d = ((oy + y) * w + (ox + x)) * 4;
                        canvas.pixels[d..d + 4].copy_from_slice(&cel.pixels[s..s + 4]);
                    }
                }
            }
            (canvas, 12u16)
        })
        .collect();
    (w, h, framed)
}

/// Nearest palette index to `rgb` (skipping the transparent slot).
fn nearest(palette: &[[u8; 3]], rgb: [u8; 3], transparent: u8) -> u8 {
    let mut best = 1u8;
    let mut best_d = u32::MAX;
    for (i, c) in palette.iter().enumerate() {
        if i as u8 == transparent {
            continue;
        }
        let d: u32 = c
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
