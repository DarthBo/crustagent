//! Composite a frame of an animation and write it to a PNG.
//!
//! Usage: `cargo run -p crustagent-format --example render -- <file.acs> <Animation> [frame] [out.png]`
//!
//! The PNG encoder here is a tiny, dependency-free implementation (stored/uncompressed
//! zlib blocks) so `crustagent-format` itself stays dependency-free.

use crustagent_format::{AcsFile, Rgba};

fn main() {
    let mut args = std::env::args().skip(1);
    let (path, anim_name) = match (args.next(), args.next()) {
        (Some(p), Some(a)) => (p, a),
        _ => {
            eprintln!("usage: render <file.acs> <Animation> [frame] [out.png]");
            std::process::exit(2);
        }
    };
    let frame_index: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let out_path = args
        .next()
        .unwrap_or_else(|| format!("{anim_name}_{frame_index}.png"));

    let chr = AcsFile::open(&path).unwrap_or_else(|e| {
        eprintln!("parse {path}: {e}");
        std::process::exit(1);
    });

    let anim = chr.animation(&anim_name).unwrap_or_else(|| {
        eprintln!("no animation named {anim_name:?}. Available:");
        for n in &chr.gesture_names {
            eprintln!("  {n}");
        }
        std::process::exit(1);
    });

    let frame = anim.frames.get(frame_index).unwrap_or_else(|| {
        eprintln!(
            "frame {frame_index} out of range (animation has {})",
            anim.frames.len()
        );
        std::process::exit(1);
    });

    let img = chr.composite_frame(frame, None).unwrap_or_else(|e| {
        eprintln!("composite: {e}");
        std::process::exit(1);
    });

    let png = encode_png(&img);
    std::fs::write(&out_path, png).unwrap_or_else(|e| {
        eprintln!("write {out_path}: {e}");
        std::process::exit(1);
    });

    println!(
        "wrote {out_path} ({}x{}, {} frame(s) in {anim_name})",
        img.width,
        img.height,
        anim.frames.len()
    );
}

// ---------------------------------------------------------------------------
// Minimal PNG encoder (RGBA8, stored deflate). Not fast, but correct and tiny.
// ---------------------------------------------------------------------------

fn encode_png(img: &Rgba) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    // IHDR
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&img.width.to_be_bytes());
    ihdr.extend_from_slice(&img.height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // color type: RGBA
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    write_chunk(&mut out, b"IHDR", &ihdr);

    // IDAT: raw scanlines (filter byte 0 per row) wrapped in a stored-block zlib stream.
    let row_bytes = img.width as usize * 4;
    let mut raw = Vec::with_capacity((row_bytes + 1) * img.height as usize);
    for y in 0..img.height as usize {
        raw.push(0); // filter: None
        raw.extend_from_slice(&img.pixels[y * row_bytes..(y + 1) * row_bytes]);
    }
    write_chunk(&mut out, b"IDAT", &zlib_store(&raw));

    write_chunk(&mut out, b"IEND", &[]);
    out
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc32::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// Wrap `data` in a zlib stream using only uncompressed ("stored") deflate blocks.
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x78); // CMF: deflate, 32K window
    out.push(0x01); // FLG: no dict, fastest
    let mut i = 0;
    while i < data.len() || (data.is_empty() && i == 0) {
        let chunk = (data.len() - i).min(0xFFFF);
        let is_last = i + chunk >= data.len();
        out.push(if is_last { 1 } else { 0 });
        out.extend_from_slice(&(chunk as u16).to_le_bytes());
        out.extend_from_slice(&(!(chunk as u16)).to_le_bytes());
        out.extend_from_slice(&data[i..i + chunk]);
        i += chunk;
        if data.is_empty() {
            break;
        }
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

struct Crc32 {
    crc: u32,
    table: [u32; 256],
}

impl Crc32 {
    fn new() -> Crc32 {
        let mut table = [0u32; 256];
        for (n, entry) in table.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *entry = c;
        }
        Crc32 {
            crc: 0xFFFF_FFFF,
            table,
        }
    }
    fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.crc = self.table[((self.crc ^ b as u32) & 0xFF) as usize] ^ (self.crc >> 8);
        }
    }
    fn finish(self) -> u32 {
        self.crc ^ 0xFFFF_FFFF
    }
}
