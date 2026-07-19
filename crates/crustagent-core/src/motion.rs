//! Movement and pointing: choosing directional animation states and interpolating a
//! character's screen position over time.
//!
//! Pure geometry + timing, no windowing. `MoveTo` picks a `MOVING{UP,DOWN,LEFT,RIGHT}`
//! state and walks the position linearly; `GestureAt` picks a `GESTURING{…}` state toward
//! a point. Direction is the dominant axis of the offset (the 4-way scheme the classic
//! characters animate).

/// A cardinal direction for `MOVING*` / `GESTURING*` states.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    /// Pick the direction toward offset `(dx, dy)` (screen coords: +y is down). The
    /// dominant axis wins; ties favor horizontal, matching the original's comparisons.
    pub fn toward(dx: i32, dy: i32) -> Direction {
        if dx.abs() >= dy.abs() {
            if dx >= 0 {
                Direction::Right
            } else {
                Direction::Left
            }
        } else if dy >= 0 {
            Direction::Down
        } else {
            Direction::Up
        }
    }

    /// The `MOVING*` state name for travel in this (screen) direction.
    ///
    /// **Horizontal is mirrored on purpose.** Microsoft Agent's `MOVINGLEFT`/`MOVINGRIGHT`
    /// animations are authored from the *character's* own perspective, not the screen's: a
    /// character walking toward the **right of the screen** plays `MOVINGLEFT` (it is moving
    /// to *its* left as it faces the viewer), and vice-versa. So travel `Direction::Right`
    /// maps to `"MOVINGLEFT"`. Vertical is not mirrored. This matches clippy.js
    /// (`_getDirection` swaps L/R for the same reason) and was verified against Merlin on
    /// real GNOME — the screen-literal mapping played his left/right flights reversed.
    ///
    /// Pointing/gesturing is *not* mirrored (it is described from the viewer's side); see
    /// [`gesture_state`](Direction::gesture_state).
    pub fn move_state(self) -> &'static str {
        match self {
            Direction::Up => "MOVINGUP",
            Direction::Down => "MOVINGDOWN",
            Direction::Left => "MOVINGRIGHT",
            Direction::Right => "MOVINGLEFT",
        }
    }

    /// The `GESTURING*` state name for this direction.
    pub fn gesture_state(self) -> &'static str {
        match self {
            Direction::Up => "GESTURINGUP",
            Direction::Down => "GESTURINGDOWN",
            Direction::Left => "GESTURINGLEFT",
            Direction::Right => "GESTURINGRIGHT",
        }
    }
}

/// A linear move from a start to a destination over a duration derived from distance.
#[derive(Clone, Copy, Debug)]
pub struct MoveTo {
    start: (i32, i32),
    dest: (i32, i32),
    duration_ms: u32,
    elapsed_ms: u32,
}

impl MoveTo {
    /// Create a move from `start` to `dest` at `pixels_per_sec` (clamped to ≥1). A zero
    /// speed means "teleport" (duration 0).
    pub fn new(start: (i32, i32), dest: (i32, i32), pixels_per_sec: u32) -> MoveTo {
        let dist = (((dest.0 - start.0).pow(2) + (dest.1 - start.1).pow(2)) as f64).sqrt();
        let duration_ms = if pixels_per_sec == 0 {
            0
        } else {
            ((dist / pixels_per_sec as f64) * 1000.0).round() as u32
        };
        MoveTo {
            start,
            dest,
            duration_ms,
            elapsed_ms: 0,
        }
    }

    /// Direction of travel, for choosing the `MOVING*` state.
    pub fn direction(&self) -> Direction {
        Direction::toward(self.dest.0 - self.start.0, self.dest.1 - self.start.1)
    }

    /// The travel duration (ms).
    pub fn duration_ms(&self) -> u32 {
        self.duration_ms
    }

    /// Override the travel duration (ms), keeping start/dest/elapsed. Used to fit a finite
    /// move animation exactly across the trip so it doesn't freeze on its last frame when
    /// the distance would otherwise outlast it.
    pub fn retime(&mut self, duration_ms: u32) {
        self.duration_ms = duration_ms;
        self.elapsed_ms = self.elapsed_ms.min(duration_ms);
    }

    /// Advance the move clock.
    pub fn advance(&mut self, dt_ms: u32) {
        self.elapsed_ms = self.elapsed_ms.saturating_add(dt_ms).min(self.duration_ms);
    }

    /// True once the destination is reached.
    pub fn is_done(&self) -> bool {
        self.elapsed_ms >= self.duration_ms
    }

    /// The interpolated position at the current elapsed time.
    pub fn position(&self) -> (i32, i32) {
        if self.duration_ms == 0 {
            return self.dest;
        }
        let t = self.elapsed_ms as f64 / self.duration_ms as f64;
        let x = self.start.0 as f64 + (self.dest.0 - self.start.0) as f64 * t;
        let y = self.start.1 as f64 + (self.dest.1 - self.start.1) as f64 * t;
        (x.round() as i32, y.round() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_dominant_axis() {
        assert_eq!(Direction::toward(100, 10), Direction::Right);
        assert_eq!(Direction::toward(-100, 10), Direction::Left);
        assert_eq!(Direction::toward(10, 100), Direction::Down);
        assert_eq!(Direction::toward(10, -100), Direction::Up);
        // tie favors horizontal
        assert_eq!(Direction::toward(50, 50), Direction::Right);
        assert_eq!(Direction::toward(-50, -50), Direction::Left);
    }

    #[test]
    fn state_names() {
        assert_eq!(Direction::Up.move_state(), "MOVINGUP");
        assert_eq!(Direction::Down.move_state(), "MOVINGDOWN");
        // Horizontal move states are mirrored (character's perspective): travelling
        // screen-right plays MOVINGLEFT and vice-versa.
        assert_eq!(Direction::Right.move_state(), "MOVINGLEFT");
        assert_eq!(Direction::Left.move_state(), "MOVINGRIGHT");
        // Gestures are NOT mirrored (viewer's perspective).
        assert_eq!(Direction::Left.gesture_state(), "GESTURINGLEFT");
        assert_eq!(Direction::Right.gesture_state(), "GESTURINGRIGHT");
    }

    #[test]
    fn move_interpolates_and_finishes() {
        // 100px at 100px/s -> 1000ms.
        let mut m = MoveTo::new((0, 0), (100, 0), 100);
        assert_eq!(m.direction(), Direction::Right);
        assert_eq!(m.position(), (0, 0));
        m.advance(500);
        assert_eq!(m.position(), (50, 0));
        assert!(!m.is_done());
        m.advance(600); // clamps at 1000ms
        assert!(m.is_done());
        assert_eq!(m.position(), (100, 0));
    }

    #[test]
    fn zero_speed_teleports() {
        let m = MoveTo::new((0, 0), (40, 60), 0);
        assert!(m.is_done());
        assert_eq!(m.position(), (40, 60));
    }
}
