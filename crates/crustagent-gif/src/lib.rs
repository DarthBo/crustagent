//! A minimal, dependency-free **animated GIF89a** encoder for palette-indexed frames.
//!
//! Microsoft Agent characters are a natural fit: they are 8-bpp palettized with a single
//! transparent color key (→ GIF's global color table + transparent index), and their
//! frame durations are in centiseconds (→ GIF's 1/100 s delay unit).
//!
//! ```
//! use crustagent_gif::GifBuilder;
//! let palette = vec![[0u8, 0, 0]; 256];
//! let mut gif = GifBuilder::new(2, 2, &palette, 0);
//! gif.add_frame(&[0, 1, 1, 0], 10); // 2x2 indices, 100ms
//! let bytes = gif.finish();
//! assert_eq!(&bytes[0..6], b"GIF89a");
//! ```

/// Builds an animated GIF from full-frame palette-indexed images sharing one palette.
pub struct GifBuilder {
    out: Vec<u8>,
    width: u16,
    height: u16,
    transparent: u8,
}

impl GifBuilder {
    /// Start a GIF `width`×`height` with a global `palette` (up to 256 `[r,g,b]` entries)
    /// and a `transparent` palette index. Loops forever.
    pub fn new(width: u16, height: u16, palette: &[[u8; 3]], transparent: u8) -> GifBuilder {
        let mut out = Vec::new();
        out.extend_from_slice(b"GIF89a");
        // Logical Screen Descriptor
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.push(0xF7); // GCT present, color resolution 8, 256-entry table
        out.push(transparent); // background color index
        out.push(0); // pixel aspect ratio
                     // Global Color Table: exactly 256 RGB triples.
        for i in 0..256 {
            let c = palette.get(i).copied().unwrap_or([0, 0, 0]);
            out.extend_from_slice(&c);
        }
        // NETSCAPE2.0 application extension: loop forever.
        out.extend_from_slice(&[0x21, 0xFF, 0x0B]);
        out.extend_from_slice(b"NETSCAPE2.0");
        out.extend_from_slice(&[0x03, 0x01, 0x00, 0x00, 0x00]);

        GifBuilder {
            out,
            width,
            height,
            transparent,
        }
    }

    /// Append one full-frame image (`width*height` palette indices, top-down) shown for
    /// `delay_cs` centiseconds. Frames use restore-to-background disposal so the
    /// transparent key shows between frames.
    pub fn add_frame(&mut self, indices: &[u8], delay_cs: u16) {
        debug_assert_eq!(indices.len(), self.width as usize * self.height as usize);

        // Graphic Control Extension: disposal=2 (restore to bg), transparent flag set.
        self.out.extend_from_slice(&[0x21, 0xF9, 0x04, 0x09]);
        self.out.extend_from_slice(&delay_cs.to_le_bytes());
        self.out.push(self.transparent);
        self.out.push(0x00);

        // Image Descriptor (full-frame, no local color table).
        self.out.push(0x2C);
        self.out.extend_from_slice(&0u16.to_le_bytes()); // left
        self.out.extend_from_slice(&0u16.to_le_bytes()); // top
        self.out.extend_from_slice(&self.width.to_le_bytes());
        self.out.extend_from_slice(&self.height.to_le_bytes());
        self.out.push(0x00);

        // LZW image data as sub-blocks of <= 255 bytes, terminated by a zero block.
        let min_code_size = 8u8;
        self.out.push(min_code_size);
        let lzw = lzw_encode(indices, min_code_size);
        for chunk in lzw.chunks(255) {
            self.out.push(chunk.len() as u8);
            self.out.extend_from_slice(chunk);
        }
        self.out.push(0x00);
    }

    /// Finish and return the GIF bytes.
    pub fn finish(mut self) -> Vec<u8> {
        self.out.push(0x3B); // trailer
        self.out
    }
}

struct BitWriter {
    acc: u32,
    nbits: u32,
    bytes: Vec<u8>,
}

impl BitWriter {
    fn new() -> BitWriter {
        BitWriter {
            acc: 0,
            nbits: 0,
            bytes: Vec::new(),
        }
    }
    fn write(&mut self, code: u32, size: u32) {
        self.acc |= code << self.nbits;
        self.nbits += size;
        while self.nbits >= 8 {
            self.bytes.push((self.acc & 0xFF) as u8);
            self.acc >>= 8;
            self.nbits -= 8;
        }
    }
    fn finish(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.bytes.push((self.acc & 0xFF) as u8);
        }
        self.bytes
    }
}

/// Standard GIF variable-width LZW.
///
/// The code-width bump uses `next_code == (1 << code_size) + 1`: the encoder's table runs
/// one entry ahead of the decoder's, so bumping at the naive `== 1 << code_size` widens
/// codes one step too early and desyncs every conforming decoder (symptom: the image
/// decodes correctly down to some row, then the rest is dropped). This `+ 1` is the fix,
/// pinned by the round-trip test below.
pub fn lzw_encode(data: &[u8], min_code_size: u8) -> Vec<u8> {
    use std::collections::HashMap;

    let clear = 1u32 << min_code_size;
    let end = clear + 1;
    let mut bits = BitWriter::new();
    let mut code_size = min_code_size as u32 + 1;
    let mut dict: HashMap<Vec<u8>, u32> = HashMap::new();
    let mut next_code = end + 1;

    let reset = |dict: &mut HashMap<Vec<u8>, u32>| {
        dict.clear();
        for i in 0..clear {
            dict.insert(vec![i as u8], i);
        }
    };
    reset(&mut dict);

    bits.write(clear, code_size);
    if data.is_empty() {
        bits.write(end, code_size);
        return bits.finish();
    }

    let mut current = vec![data[0]];
    for &k in &data[1..] {
        let mut cand = current.clone();
        cand.push(k);
        if dict.contains_key(&cand) {
            current = cand;
        } else {
            bits.write(dict[&current], code_size);
            if next_code < 4096 {
                dict.insert(cand, next_code);
                next_code += 1;
                if next_code == (1 << code_size) + 1 && code_size < 12 {
                    code_size += 1;
                }
            } else {
                bits.write(clear, code_size);
                reset(&mut dict);
                code_size = min_code_size as u32 + 1;
                next_code = end + 1;
            }
            current = vec![k];
        }
    }
    bits.write(dict[&current], code_size);
    bits.write(end, code_size);
    bits.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A conforming GIF LZW decoder, used only to verify the encoder.
    fn lzw_decode(bytes: &[u8], min_code_size: u8) -> Result<Vec<u8>, String> {
        let clear = 1u32 << min_code_size;
        let end = clear + 1;
        let mut code_size = min_code_size as u32 + 1;
        let mut table: Vec<Vec<u8>> = Vec::new();
        let reset = |t: &mut Vec<Vec<u8>>| {
            t.clear();
            for i in 0..clear {
                t.push(vec![i as u8]);
            }
            t.push(vec![]); // clear
            t.push(vec![]); // end
        };
        reset(&mut table);

        let mut out = Vec::new();
        let mut bitpos = 0usize;
        let mut read = |size: u32| -> Option<u32> {
            let mut v = 0u32;
            for i in 0..size {
                let byte = bitpos / 8;
                let bit = bitpos % 8;
                if byte >= bytes.len() {
                    return None;
                }
                v |= (((bytes[byte] >> bit) & 1) as u32) << i;
                bitpos += 1;
            }
            Some(v)
        };

        let mut old: Option<u32> = None;
        while let Some(code) = read(code_size) {
            if code == clear {
                reset(&mut table);
                code_size = min_code_size as u32 + 1;
                old = None;
                continue;
            }
            if code == end {
                break;
            }
            let entry = if (code as usize) < table.len() {
                table[code as usize].clone()
            } else if code as usize == table.len() {
                let o = old.ok_or("first code out of range")? as usize;
                let mut e = table[o].clone();
                e.push(table[o][0]);
                e
            } else {
                return Err(format!("invalid code {code} (table {})", table.len()));
            };
            out.extend_from_slice(&entry);
            if let Some(o) = old {
                let mut ne = table[o as usize].clone();
                ne.push(entry[0]);
                table.push(ne);
                if table.len() == (1 << code_size) as usize && code_size < 12 {
                    code_size += 1;
                }
            }
            old = Some(code);
        }
        Ok(out)
    }

    fn roundtrip(data: &[u8]) {
        let enc = lzw_encode(data, 8);
        let dec = lzw_decode(&enc, 8).expect("decode");
        assert_eq!(dec, data, "round-trip mismatch (len {})", data.len());
    }

    #[test]
    fn empty_and_tiny() {
        roundtrip(&[]);
        roundtrip(&[0]);
        roundtrip(&[5, 5, 5, 5]);
    }

    #[test]
    fn crosses_code_width_boundaries() {
        // Enough distinct sequences to push code width from 9 up through 12 and force a
        // table clear. This is the case the off-by-one used to corrupt.
        let mut data = Vec::new();
        let mut x = 12345u32;
        for _ in 0..20_000 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let v = ((x >> 16) % 256) as u8;
            let run = ((x >> 8) % 30) as usize;
            data.extend(std::iter::repeat_n(v, run));
        }
        roundtrip(&data);
    }

    #[test]
    fn builds_valid_container() {
        let palette = vec![[10u8, 20, 30]; 256];
        let mut gif = GifBuilder::new(4, 2, &palette, 0);
        gif.add_frame(&[1, 2, 3, 4, 5, 6, 7, 8], 10);
        let bytes = gif.finish();
        assert_eq!(&bytes[0..6], b"GIF89a");
        assert_eq!(*bytes.last().unwrap(), 0x3B);
    }
}
