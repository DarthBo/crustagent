//! Dump a summary of a Microsoft Actor (`.act`) character and optionally render a cel.
//!
//! Usage:
//!   `cargo run -p crustagent-format --example act_dump -- <file.act>`
//!   `cargo run -p crustagent-format --example act_dump -- <file.act> <celIndex> [out.png]`
//!
//! With a cel index, the placeable-WMF cel is rasterized and written as a PNG. The PNG
//! encoder here is the same tiny dependency-free one used by the `render` example.

use crustagent_format::act::CelFormat;
use crustagent_format::{ActFile, Rgba};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("usage: act_dump <file.act> [celIndex] [out.png]");
            std::process::exit(2);
        }
    };
    let cel_index: Option<usize> = args.next().and_then(|s| s.parse().ok());

    let act = ActFile::open(&path).unwrap_or_else(|e| {
        eprintln!("parse {path}: {e}");
        std::process::exit(1);
    });

    println!("Name       : {}", act.name);
    println!(
        "Version    : {}.{} ({}, {}-endian)",
        act.version.0,
        act.version.1,
        if act.version.0 >= 2 {
            "Actor 2.0"
        } else {
            "Actor 1.0"
        },
        if act.big_endian { "big" } else { "little" }
    );
    println!("Frame size : {}x{}", act.image_size.0, act.image_size.1);
    println!("Palette    : {} colors", act.palette.len());
    println!("Artwork    : {:?}", act.image_format);
    println!("Cels       : {}", act.cels.len());
    println!("Sounds     : {} embedded WAVE stream(s)", act.sounds.len());

    let Some(idx) = cel_index else { return };
    let out_path = std::env::args()
        .nth(3)
        .unwrap_or_else(|| format!("cel_{idx}.png"));
    match act.cels.get(idx).map(|c| c.format) {
        Some(CelFormat::Wmf) => match act.render_cel(idx) {
            Some(img) => {
                std::fs::write(&out_path, encode_png(&img)).unwrap();
                println!("wrote {out_path} ({}x{})", img.width, img.height);
            }
            None => eprintln!("cel {idx} failed to render"),
        },
        Some(other) => eprintln!("cel {idx} is {other:?}, not renderable yet"),
        None => eprintln!("no cel {idx} (have {})", act.cels.len()),
    }
}

// ---------------------------------------------------------------------------
// Minimal PNG encoder (RGBA8, stored deflate) — mirrors examples/render.rs.
// ---------------------------------------------------------------------------

fn encode_png(img: &Rgba) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&img.width.to_be_bytes());
    ihdr.extend_from_slice(&img.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    write_chunk(&mut out, b"IHDR", &ihdr);
    let row_bytes = img.width as usize * 4;
    let mut raw = Vec::with_capacity((row_bytes + 1) * img.height as usize);
    for y in 0..img.height as usize {
        raw.push(0);
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

fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
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
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
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
