//! Recognises a side-to-side "shake" of the cursor during a drag, which opens
//! a new shelf right where the cursor is.
//!
//! The detector only sees x positions and timestamps, so it's testable without
//! a mouse. A shake is several direction changes, each after travelling at
//! least `min_travel` pixels, all within `window_ms`.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    /// Small, quick wiggles count.
    Gentle,
    #[default]
    Normal,
    /// Needs a deliberate shake; fewest accidental shelves.
    Firm,
}

impl Sensitivity {
    /// (direction changes needed, minimum travel in px, time window in ms)
    fn params(self) -> (usize, i32, u32) {
        match self {
            Sensitivity::Gentle => (3, 18, 750),
            Sensitivity::Normal => (3, 30, 600),
            Sensitivity::Firm => (4, 40, 600),
        }
    }
}

pub struct ShakeDetector {
    needed: usize,
    min_travel: i32,
    window_ms: u32,
    /// Where the current stroke started (or last turned).
    anchor_x: i32,
    /// -1 left, 1 right, 0 not moving yet.
    dir: i32,
    /// Timestamps of recent direction changes.
    turns: VecDeque<u32>,
}

impl ShakeDetector {
    pub fn new(sensitivity: Sensitivity) -> Self {
        let (needed, min_travel, window_ms) = sensitivity.params();
        Self { needed, min_travel, window_ms, anchor_x: 0, dir: 0, turns: VecDeque::new() }
    }

    /// Starts tracking a new drag at `x`.
    pub fn reset(&mut self, x: i32) {
        self.anchor_x = x;
        self.dir = 0;
        self.turns.clear();
    }

    /// Feeds a cursor position; returns true when this move completes a shake.
    pub fn feed(&mut self, x: i32, t_ms: u32) -> bool {
        let dx = x - self.anchor_x;
        if dx.abs() < self.min_travel {
            return false;
        }
        let dir = dx.signum();
        if self.dir != 0 && dir != self.dir {
            self.turns.push_back(t_ms);
        }
        self.dir = dir;
        self.anchor_x = x;
        while self.turns.front().is_some_and(|&t| t_ms.wrapping_sub(t) > self.window_ms) {
            self.turns.pop_front();
        }
        if self.turns.len() >= self.needed {
            self.turns.clear();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Moves the cursor between `points`, sampling every 10 px / 8 ms.
    fn run(d: &mut ShakeDetector, points: &[i32], ms_per_step: u32) -> bool {
        let mut t = 1_000;
        let mut x = points[0];
        d.reset(x);
        for &target in &points[1..] {
            while x != target {
                x += (target - x).signum() * 10.min((target - x).abs());
                t += ms_per_step;
                if d.feed(x, t) {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn quick_wiggle_is_a_shake() {
        let mut d = ShakeDetector::new(Sensitivity::Normal);
        assert!(run(&mut d, &[500, 560, 500, 560, 500], 4));
    }

    #[test]
    fn straight_drag_is_not() {
        let mut d = ShakeDetector::new(Sensitivity::Normal);
        assert!(!run(&mut d, &[100, 900], 4));
        // An L-shaped drag turns once at most.
        assert!(!run(&mut d, &[100, 600, 400], 4));
    }

    #[test]
    fn slow_back_and_forth_is_not() {
        let mut d = ShakeDetector::new(Sensitivity::Normal);
        // Same path, but each 10 px step takes 60 ms: far outside the window.
        assert!(!run(&mut d, &[500, 560, 500, 560, 500], 60));
    }

    #[test]
    fn tiny_jitter_is_not() {
        let mut d = ShakeDetector::new(Sensitivity::Normal);
        assert!(!run(&mut d, &[500, 520, 500, 520, 500, 520, 500], 4));
        // ...but counts with the gentle setting.
        let mut gentle = ShakeDetector::new(Sensitivity::Gentle);
        assert!(run(&mut gentle, &[500, 520, 500, 520, 500, 520, 500], 4));
    }

    #[test]
    fn firm_needs_more_turns() {
        let mut d = ShakeDetector::new(Sensitivity::Firm);
        assert!(!run(&mut d, &[500, 560, 500, 560, 500], 4));
        assert!(run(&mut d, &[500, 560, 500, 560, 500, 560], 4));
    }
}
