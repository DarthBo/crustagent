//! Shared sub-block parsers used by both the ACS and ACF headers. They differ only in
//! whether strings carry a trailing NUL (`null_terminated`): ACS 2.0 uses `true`, ACF
//! uses `false`. Everything else is identical.

use crate::error::Result;
use crate::model::{Balloon, Color, Name, State, Tts};
use crate::reader::Cursor;

/// TTS voice block (`ReadBufferTts`).
pub fn tts(c: &mut Cursor, nt: bool) -> Result<Tts> {
    let engine = c.guid()?;
    let mode = c.guid()?;
    let speed = c.i32()?;
    let pitch = c.i16()?;
    let has_extra = c.u8()? != 0;
    if has_extra {
        let language = c.u16()?;
        let _unknown = c.string(nt)?;
        let gender = c.u16()?;
        let age = c.u16()?;
        let style = c.string(nt)?;
        Ok(Tts {
            engine,
            mode,
            speed,
            pitch,
            language: Some(language),
            gender,
            age,
            style,
        })
    } else {
        Ok(Tts {
            engine,
            mode,
            speed,
            pitch,
            language: None,
            gender: 0,
            age: 0,
            style: String::new(),
        })
    }
}

/// Word-balloon block (`ReadBufferBalloon`).
pub fn balloon(c: &mut Cursor, nt: bool) -> Result<Balloon> {
    let lines = c.u8()?;
    let per_line = c.u8()?;
    let fg_color = c.color()?;
    let bg_color = c.color()?;
    let border_color = c.color()?;
    let font_name = c.string(nt)?;
    let font_height = c.i32()?;
    let weight = c.u16()?;
    let strikeout = c.u16()?;
    let italic = c.u16()?;
    Ok(Balloon {
        lines,
        per_line,
        fg_color,
        bg_color,
        border_color,
        font_name,
        font_height,
        bold: weight >= 700, // FW_BOLD
        strikeout: strikeout != 0,
        italic: italic != 0,
    })
}

/// Palette block (`ReadBufferPalette`): `u32 count` then `count` BGRA entries; keep ≤256.
pub fn palette(c: &mut Cursor) -> Result<Vec<Color>> {
    let count = c.u32()? as usize;
    let mut palette = Vec::with_capacity(count.min(256));
    for i in 0..count {
        let color = c.color()?;
        if i < 256 {
            palette.push(color);
        }
    }
    Ok(palette)
}

/// Icon block (`ReadBufferIcon`): a flag, then (if set) two length-prefixed DIB blobs.
/// We skip the icon bitmaps for now.
pub fn skip_icon(c: &mut Cursor) -> Result<()> {
    if c.u8()? != 0 {
        let mask = c.u32()? as usize;
        c.skip(mask)?;
        let color = c.u32()? as usize;
        c.skip(color)?;
    }
    Ok(())
}

/// States block (`ReadBufferStates`): `u16 count` × `{name, u16 gestureCount, names…}`.
pub fn states(c: &mut Cursor, nt: bool) -> Result<Vec<State>> {
    let count = c.u16()?;
    let mut states = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = c.string(nt)?;
        let gesture_count = c.u16()?;
        let mut animations = Vec::with_capacity(gesture_count as usize);
        for _ in 0..gesture_count {
            animations.push(c.string(nt)?);
        }
        states.push(State { name, animations });
    }
    Ok(states)
}

/// Names block (`ReadBufferNames`, `firstLetterCaps=true`): `u16 count` × `{langId,
/// name, desc1, desc2}`.
pub fn names(c: &mut Cursor, nt: bool) -> Result<Vec<Name>> {
    let count = c.u16()?;
    let mut names = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let language = c.u16()?;
        let name = first_letter_caps(c.string(nt)?);
        let desc1 = c.string(nt)?;
        let desc2 = c.string(nt)?;
        names.push(Name {
            language,
            name,
            desc1,
            desc2,
        });
    }
    Ok(names)
}

pub fn first_letter_caps(s: String) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) if first.is_lowercase() => {
            first.to_uppercase().collect::<String>() + chars.as_str()
        }
        _ => s,
    }
}
