//! Build a character in memory from an RGBA image pool via [`AcsFile::from_parts_rgba`],
//! then composite its frames and assert the RGBA path blits (and source-over-blends) the
//! pool directly — no palette, no 1-bit transparency key.

use crustagent_format::{
    AcsFile, Animation, FileHeader, Frame, FrameImage, Guid, ReturnKind, Rgba, State,
};

/// A `w`×`h` image filled with one straight-alpha color.
fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Rgba {
    Rgba {
        width: w,
        height: h,
        pixels: rgba.iter().copied().cycle().take((w * h * 4) as usize).collect(),
    }
}

fn header(w: u16, h: u16) -> FileHeader {
    FileHeader {
        version_major: 2,
        version_minor: 0,
        guid: Guid([0; 16]),
        image_size: (w, h),
        transparency: 0,
        style: 0,
        palette: Vec::new(),
    }
}

fn frame(image_ndx: u32) -> Frame {
    Frame {
        duration: 10,
        sound_ndx: -1,
        exit_frame: -1,
        branching: Vec::new(),
        images: vec![FrameImage { image_ndx, offset: (0, 0) }],
        overlays: Vec::new(),
    }
}

fn build() -> AcsFile {
    // Two 2×2 frames: opaque red, opaque green.
    let images = vec![
        solid(2, 2, [255, 0, 0, 255]),
        solid(2, 2, [0, 255, 0, 255]),
    ];
    let anim = Animation {
        name: "Wave".into(),
        return_kind: ReturnKind::None,
        return_name: String::new(),
        frames: vec![frame(0), frame(1)],
    };
    AcsFile::from_parts_rgba(
        header(2, 2),
        None,
        None,
        Vec::new(),
        vec![State { name: "IDLINGLEVEL1".into(), animations: vec!["Wave".into()] }],
        vec!["Wave".into()],
        vec![anim],
        images,
        Vec::new(),
    )
}

#[test]
fn composites_full_frame_rgba_directly() {
    let file = build();
    assert_eq!(file.image_count(), 2);
    let anim = file.animation("wave").expect("case-insensitive lookup");

    let f0 = file.composite_frame(&anim.frames[0], None).unwrap();
    assert_eq!((f0.width, f0.height), (2, 2));
    assert_eq!(&f0.pixels[0..4], &[255, 0, 0, 255], "frame 0 is opaque red");

    let f1 = file.composite_frame(&anim.frames[1], None).unwrap();
    assert_eq!(&f1.pixels[0..4], &[0, 255, 0, 255], "frame 1 is opaque green");
}

#[test]
fn transparent_source_leaves_canvas_clear() {
    // A fully-transparent image over the (transparent) canvas stays transparent —
    // exercises the sa == 0 skip.
    let file = AcsFile::from_parts_rgba(
        header(2, 2),
        None,
        None,
        Vec::new(),
        vec![State { name: "IDLINGLEVEL1".into(), animations: vec!["Idle".into()] }],
        vec!["Idle".into()],
        vec![Animation {
            name: "Idle".into(),
            return_kind: ReturnKind::None,
            return_name: String::new(),
            frames: vec![frame(0)],
        }],
        vec![solid(2, 2, [123, 45, 67, 0])],
        Vec::new(),
    );
    let f = file.composite_frame(&file.animation("Idle").unwrap().frames[0], None).unwrap();
    assert!(f.is_fully_transparent(), "transparent source keeps the frame clear");
}

#[test]
fn semi_transparent_over_blends() {
    // 50%-alpha white over an opaque-red base layer: base image is index 1 (bottom),
    // overlay image index 0 (top) — matches the highest-index-is-bottom convention.
    let base = solid(1, 1, [255, 0, 0, 255]);
    let top = solid(1, 1, [255, 255, 255, 128]);
    let mut frame_two = frame(0);
    frame_two.images = vec![
        FrameImage { image_ndx: 0, offset: (0, 0) }, // top layer (index 0)
        FrameImage { image_ndx: 1, offset: (0, 0) }, // bottom layer (highest index)
    ];
    let file = AcsFile::from_parts_rgba(
        header(1, 1),
        None,
        None,
        Vec::new(),
        vec![State { name: "IDLINGLEVEL1".into(), animations: vec!["Blend".into()] }],
        vec!["Blend".into()],
        vec![Animation {
            name: "Blend".into(),
            return_kind: ReturnKind::None,
            return_name: String::new(),
            frames: vec![frame_two],
        }],
        vec![top, base],
        Vec::new(),
    );
    let f = file.composite_frame(&file.animation("Blend").unwrap().frames[0], None).unwrap();
    // out_a = 128 + 255*(127)/255 = 255. R ≈ (255*128 + 255*255*127/255)/255 = 255.
    // G,B ≈ (255*128 + 0)/255 = 128.
    assert_eq!(f.pixels[3], 255, "opaque after blend");
    assert_eq!(f.pixels[0], 255, "red channel stays maxed");
    assert!((120..=136).contains(&f.pixels[1]), "green blends toward ~128, got {}", f.pixels[1]);
}
