//! End-to-end test: hand-build a minimal but structurally complete ACS 2.0 file,
//! then parse it and assert every section round-trips. This exercises the block
//! directory, header, palette, states, names, gesture index, animation/frame parsing,
//! and the uncompressed image path without needing an external fixture.

use crustagent_format::{AcsFile, MouthOverlay, ReturnKind, ACS_SIGNATURE};

/// Encode a DWORD-length-prefixed, null-terminated UTF-16LE string.
fn wstr(s: &str) -> Vec<u8> {
    let mut v = Vec::new();
    let units: Vec<u16> = s.encode_utf16().collect();
    v.extend_from_slice(&(units.len() as u32).to_le_bytes());
    for u in &units {
        v.extend_from_slice(&u.to_le_bytes());
    }
    if !units.is_empty() {
        v.extend_from_slice(&[0, 0]); // NUL terminator
    }
    v
}

fn put_u32(buf: &mut [u8], pos: usize, val: u32) {
    buf[pos..pos + 4].copy_from_slice(&val.to_le_bytes());
}

fn build_acs() -> Vec<u8> {
    const HEADER_OFF: usize = 36; // 4 (sig) + 32 (four u64 block descriptors)

    // ---- header block ----
    let mut hb: Vec<u8> = Vec::new();
    hb.extend_from_slice(&0u16.to_le_bytes()); // version minor
    hb.extend_from_slice(&2u16.to_le_bytes()); // version major
    let names_off_pos = hb.len();
    hb.extend_from_slice(&0u32.to_le_bytes()); // names offset (patched)
    let names_size_pos = hb.len();
    hb.extend_from_slice(&0u32.to_le_bytes()); // names size (patched)
    hb.extend_from_slice(&[0u8; 16]); // guid
    hb.extend_from_slice(&128u16.to_le_bytes()); // width
    hb.extend_from_slice(&96u16.to_le_bytes()); // height
    hb.push(0); // transparency index
    hb.extend_from_slice(&0x0010_0000u32.to_le_bytes()); // style = Standard (no TTS/Balloon)
    hb.extend_from_slice(&2u32.to_le_bytes()); // unknown
                                               // palette: 2 entries (B,G,R,pad)
    hb.extend_from_slice(&2u32.to_le_bytes());
    hb.extend_from_slice(&[0, 0, 0, 0]); // index 0 -> black
    hb.extend_from_slice(&[255, 0, 0, 0]); // index 1 -> blue (b=255)
                                           // icon: none
    hb.push(0);
    // states: 1 state "Showing" -> ["Show"]
    hb.extend_from_slice(&1u16.to_le_bytes());
    hb.extend_from_slice(&wstr("Showing"));
    hb.extend_from_slice(&1u16.to_le_bytes());
    hb.extend_from_slice(&wstr("Show"));
    // names sub-block starts here
    let names_rel = hb.len();
    let mut names = Vec::new();
    names.extend_from_slice(&1u16.to_le_bytes()); // name count
    names.extend_from_slice(&0x0409u16.to_le_bytes()); // en-US
    names.extend_from_slice(&wstr("test")); // lowercase -> should become "Test"
    names.extend_from_slice(&wstr("")); // desc1
    names.extend_from_slice(&wstr("")); // desc2
    let names_size = names.len();
    hb.extend_from_slice(&names);
    put_u32(&mut hb, names_off_pos, (HEADER_OFF + names_rel) as u32);
    put_u32(&mut hb, names_size_pos, names_size as u32);

    // ---- gesture block (animOffset patched once known) ----
    let gestures_off = HEADER_OFF + hb.len();
    let mut gb = Vec::new();
    gb.extend_from_slice(&1u32.to_le_bytes()); // gesture count
    gb.extend_from_slice(&wstr("Show"));
    let anim_off_pos = gb.len();
    gb.extend_from_slice(&0u32.to_le_bytes()); // anim offset (patched)
    let anim_size_pos = gb.len();
    gb.extend_from_slice(&0u32.to_le_bytes()); // anim size (patched)

    // ---- animation record ----
    let anim_off = gestures_off + gb.len();
    let mut ar = Vec::new();
    ar.extend_from_slice(&wstr("Show")); // name
    ar.push(2); // returnType = None
    ar.extend_from_slice(&wstr("")); // returnName
    ar.extend_from_slice(&1u16.to_le_bytes()); // frame count
                                               // frame 0
    ar.extend_from_slice(&1u16.to_le_bytes()); // image count
    ar.extend_from_slice(&0u32.to_le_bytes()); // image ndx 0
    ar.extend_from_slice(&0i16.to_le_bytes()); // offset x
    ar.extend_from_slice(&0i16.to_le_bytes()); // offset y
    ar.extend_from_slice(&(-1i16).to_le_bytes()); // sound ndx = none
    ar.extend_from_slice(&10u16.to_le_bytes()); // duration (cs)
    ar.extend_from_slice(&(-1i16).to_le_bytes()); // exit frame
    ar.push(0); // branch count
    ar.push(1); // overlay count
                // overlay (14 bytes): mouth Narrow, no replace, image 0
    ar.push(MouthOverlay::Narrow as u8);
    ar.push(0); // replace
    ar.extend_from_slice(&0u16.to_le_bytes()); // image ndx
    ar.push(0); // unknown
    ar.push(0); // rgn flag
    ar.extend_from_slice(&3i16.to_le_bytes()); // offset x
    ar.extend_from_slice(&4i16.to_le_bytes()); // offset y
    ar.extend_from_slice(&0i16.to_le_bytes()); // something x
    ar.extend_from_slice(&0i16.to_le_bytes()); // something y
    let anim_size = ar.len();
    put_u32(&mut gb, anim_off_pos, anim_off as u32);
    put_u32(&mut gb, anim_size_pos, anim_size as u32);

    // ---- image index (offset patched once image record placed) ----
    let images_off = anim_off + ar.len();
    let mut ii = Vec::new();
    ii.extend_from_slice(&1u32.to_le_bytes()); // image count
    let img_off_pos = ii.len();
    ii.extend_from_slice(&0u32.to_le_bytes()); // offset (patched)
    let img_size_pos = ii.len();
    ii.extend_from_slice(&0u32.to_le_bytes()); // size (patched)
    ii.extend_from_slice(&0u32.to_le_bytes()); // checksum

    // ---- sound index (empty) ----
    let sounds_off = images_off + ii.len();
    let mut si = Vec::new();
    si.extend_from_slice(&0u32.to_le_bytes()); // sound count

    // ---- image record (2x2, uncompressed; stride 4 -> 8 bytes) ----
    let image_rec_off = sounds_off + si.len();
    let mut ir = Vec::new();
    ir.push(1); // first byte > 0
    ir.extend_from_slice(&2u16.to_le_bytes()); // width
    ir.extend_from_slice(&2u16.to_le_bytes()); // height
    ir.push(0); // uncompressed
    ir.extend_from_slice(&8u32.to_le_bytes()); // byte count
    ir.extend_from_slice(&[1, 0, 0, 0, 0, 1, 0, 0]); // 8bpp indices (stride 4, 2 rows)
    let image_rec_size = ir.len();
    put_u32(&mut ii, img_off_pos, image_rec_off as u32);
    put_u32(&mut ii, img_size_pos, image_rec_size as u32);

    // ---- assemble file ----
    let mut file = Vec::new();
    file.extend_from_slice(&ACS_SIGNATURE.to_le_bytes());
    // block directory: header, gestures, images, sounds — {offset(lo), length(hi)}
    let dir = |off: usize, len: usize, f: &mut Vec<u8>| {
        f.extend_from_slice(&(off as u32).to_le_bytes());
        f.extend_from_slice(&(len as u32).to_le_bytes());
    };
    dir(HEADER_OFF, hb.len(), &mut file);
    dir(gestures_off, gb.len(), &mut file);
    dir(images_off, ii.len(), &mut file);
    dir(sounds_off, si.len(), &mut file);
    assert_eq!(file.len(), HEADER_OFF);
    file.extend_from_slice(&hb);
    file.extend_from_slice(&gb);
    file.extend_from_slice(&ar);
    file.extend_from_slice(&ii);
    file.extend_from_slice(&si);
    file.extend_from_slice(&ir);
    file
}

#[test]
fn parses_synthetic_acs() {
    let bytes = build_acs();
    let chr = AcsFile::parse(bytes).expect("parse");

    assert_eq!(chr.header.version_major, 2);
    assert_eq!(chr.header.version_minor, 0);
    assert_eq!(chr.header.image_size, (128, 96));
    assert_eq!(chr.header.transparency, 0);
    assert_eq!(chr.header.palette.len(), 2);
    assert_eq!(chr.header.palette[1].b, 255);
    assert!(chr.tts.is_none());
    assert!(chr.balloon.is_none());

    // first-letter caps applied
    assert_eq!(chr.default_name().unwrap().name, "Test");
    assert_eq!(chr.default_name().unwrap().language, 0x0409);

    // states
    assert_eq!(chr.states.len(), 1);
    assert_eq!(chr.states[0].name, "Showing");
    assert_eq!(chr.states[0].animations, vec!["Show".to_string()]);

    // animation + frame
    assert_eq!(chr.animations.len(), 1);
    let show = chr.animation("Show").expect("Show animation");
    assert_eq!(show.return_kind, ReturnKind::None);
    assert_eq!(show.frames.len(), 1);
    let f = &show.frames[0];
    assert_eq!(f.duration, 10);
    assert_eq!(f.sound_ndx, -1);
    assert_eq!(f.exit_frame, -1);
    assert_eq!(f.images.len(), 1);
    assert_eq!(f.images[0].image_ndx, 0);
    assert_eq!(f.overlays.len(), 1);
    assert_eq!(f.overlays[0].overlay_type, MouthOverlay::Narrow);
    assert_eq!(f.overlays[0].offset, (3, 4));

    // sounds
    assert_eq!(chr.sound_count(), 0);

    // image decode (uncompressed)
    assert_eq!(chr.image_count(), 1);
    let img = chr.image(0).expect("image 0");
    assert_eq!((img.width, img.height), (2, 2));
    assert_eq!(img.stride(), 4);
    assert_eq!(img.bits.len(), 8);
    assert_eq!(&img.bits, &[1, 0, 0, 0, 0, 1, 0, 0]);

    // rgba conversion: bottom-up source row order flips vertically.
    // source row0 = [1,0,..] (blue, black), source row1 = [0,1,..] (black, blue).
    // output row0 = source row1, output row1 = source row0.
    let rgba = img.to_rgba(&chr.header.palette, chr.header.transparency);
    assert_eq!(rgba.len(), 2 * 2 * 4);
    // output pixel (0,0) = source (0,1) = index 0 (black, opaque? no — index 0 == transparency -> transparent)
    assert_eq!(&rgba[0..4], &[0, 0, 0, 0]);
    // output pixel (1,0) = source (1,1) = index 1 -> blue opaque
    assert_eq!(&rgba[4..8], &[0, 0, 255, 255]);
}
