use crustagent_format::{AcsFile, ActFile, Color};
fn cube_fallback() -> Vec<(u8, u8, u8)> {
    let c = [255u8, 204, 153, 102, 51, 0];
    let r = [238u8, 221, 187, 170, 136, 119, 85, 68, 34, 17];
    let mut p = vec![];
    for &a in &c {
        for &b in &c {
            for &d in &c {
                p.push((a, b, d));
            }
        }
    }
    for &v in &r {
        p.push((v, 0, 0));
    }
    for &v in &r {
        p.push((0, v, 0));
    }
    for &v in &r {
        p.push((0, 0, v));
    }
    for &v in &r {
        p.push((v, v, v));
    }
    p
}
fn png(name: &str, idx: &[u8], w: usize, h: usize, pal: &[(u8, u8, u8)]) {
    let mut rgba = vec![0u8; w * h * 4];
    for i in 0..w * h {
        let c = pal[idx[i] as usize];
        let o = i * 4;
        rgba[o] = c.0;
        rgba[o + 1] = c.1;
        rgba[o + 2] = c.2;
        rgba[o + 3] = 255;
    }
    let mut out = vec![0x89, 80, 78, 71, 13, 10, 26, 10];
    let mut ih = vec![];
    ih.extend_from_slice(&(w as u32).to_be_bytes());
    ih.extend_from_slice(&(h as u32).to_be_bytes());
    ih.extend_from_slice(&[8, 6, 0, 0, 0]);
    let c = |o: &mut Vec<u8>, k: &[u8; 4], d: &[u8]| {
        o.extend_from_slice(&(d.len() as u32).to_be_bytes());
        o.extend_from_slice(k);
        o.extend_from_slice(d);
        let mut cc = 0xFFFFFFFFu32;
        for b in k.iter().chain(d) {
            cc ^= *b as u32;
            for _ in 0..8 {
                cc = if cc & 1 != 0 {
                    0xEDB88320 ^ (cc >> 1)
                } else {
                    cc >> 1
                };
            }
        }
        o.extend_from_slice(&(cc ^ 0xFFFFFFFF).to_be_bytes());
    };
    c(&mut out, b"IHDR", &ih);
    let mut raw = vec![];
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    let mut z = vec![0x78, 0x01];
    let mut i = 0;
    while i < raw.len() {
        let n = (raw.len() - i).min(0xffff);
        let l = i + n >= raw.len();
        z.push(l as u8);
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&raw[i..i + n]);
        i += n;
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    c(&mut out, b"IDAT", &z);
    c(&mut out, b"IEND", &[]);
    std::fs::write(name, out).unwrap();
}
fn main() {
    let act = ActFile::open("assets/agents/MAC_ACT/Genius").unwrap();
    let acs = AcsFile::open("assets/agents/ACS/GENIUS.ACS").unwrap();
    let pal = &acs.header.palette;
    let tr = acs.header.transparency as usize;
    let mut imgs = vec![];
    for j in 0..6000 {
        match acs.image(j) {
            Ok(i) => imgs.push(i),
            Err(_) => break,
        }
    }
    let mut hist: Vec<std::collections::HashMap<u32, u32>> =
        vec![std::collections::HashMap::new(); 256];
    for ci in 0..act.cels.len() {
        let Some((w, h, idx)) = act.decode_smc_cel(ci) else {
            continue;
        };
        let (w, h) = (w as usize, h as usize);
        let stride = ((w + 3) / 4) * 4;
        let mut best = (f64::MAX, 0usize);
        for (j, img) in imgs.iter().enumerate() {
            if img.width as usize != w || img.height as usize != h {
                continue;
            }
            let mut m = std::collections::HashMap::new();
            let mut bad = 0u32;
            let mut tot = 0u32;
            for y in (0..h).step_by(2) {
                for x in (0..w).step_by(2) {
                    let a = idx[y * w + x];
                    let ai = img.bits[(h - 1 - y) * stride + x] as usize;
                    let c = pal.get(ai).copied().unwrap_or(Color { r: 0, g: 0, b: 0 });
                    let t = (c.r, c.g, c.b);
                    tot += 1;
                    match m.get(&a) {
                        Some(&e) if e != t => bad += 1,
                        None => {
                            m.insert(a, t);
                        }
                        _ => {}
                    }
                }
            }
            let sc = bad as f64 / tot.max(1) as f64;
            if sc < best.0 {
                best = (sc, j);
            }
        }
        if best.0 < 0.005 {
            let img = &imgs[best.1];
            for y in 0..h {
                for x in 0..w {
                    let a = idx[y * w + x] as usize;
                    let ai = img.bits[(h - 1 - y) * stride + x] as usize;
                    let c = pal.get(ai).copied().unwrap_or(Color { r: 0, g: 0, b: 0 });
                    *hist[a]
                        .entry(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32)
                        .or_insert(0) += 1;
                }
            }
        }
    }
    let fb = cube_fallback();
    let mut mac = vec![(0u8, 0u8, 0u8); 256];
    let mut filled = 0;
    for i in 0..256 {
        if let Some((&k, &cnt)) = hist[i].iter().max_by_key(|(_, &c)| c) {
            if cnt >= 3 {
                mac[i] = ((k >> 16) as u8, (k >> 8) as u8, k as u8);
                filled += 1;
                continue;
            }
        }
        mac[i] = fb[i];
    }
    println!("derived {filled}/256 (rest = cube fallback)");
    let raw: Vec<u8> = mac.iter().flat_map(|&(r, g, b)| [r, g, b]).collect();
    std::fs::write("/tmp/mac_true.pal", &raw).unwrap();
    // render genius idle cel (obj via idle) and bosgrove cel5-equivalent
    let (w, h, idx) = act.decode_smc_cel(0).unwrap();
    png("/tmp/d_gen0.png", &idx, w as usize, h as usize, &mac);
    let idle = act.action("Idle").unwrap();
    let seq = act.action_sequence(idle, 8);
    // render idle's first shown cel by decoding — need obj->cel; just decode a mid cel
    let (w, h, idx) = act.decode_smc_cel(20).unwrap();
    png("/tmp/d_gen20.png", &idx, w as usize, h as usize, &mac);
    let bos = ActFile::open("assets/agents/MAC_ACT/Bosgrove").unwrap();
    let (w, h, idx) = bos.decode_smc_cel(5).unwrap();
    png("/tmp/d_bos5.png", &idx, w as usize, h as usize, &mac);
    println!(
        "wrote /tmp/d_gen0.png d_gen20.png d_bos5.png; seq0={:?}",
        seq.first()
    );
}
