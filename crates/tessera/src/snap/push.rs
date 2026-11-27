// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Switching the workspace by pushing a dragged window against an edge of
//! the displays, as Plasma does with "Switch desktop on edge: only when
//! moving windows": the pointer held against the outer left or right edge
//! of a display, or its top or bottom where the workspaces stand in rows,
//! for a moment goes to the workspace on that side, and the window with
//! it. An edge another display lies beyond never switches, and once it
//! switched the pointer is to come some way back before it switches
//! again.

use std::time::{Duration, Instant};

use crate::platform::ScreenRect;
use crate::workspaces::Toward;

/// How near the side of a display the pointer is to be to push against
/// it, in physical pixels.
const AGAINST: i32 = 2;

/// How far the pointer has to come back from where it switched before it
/// can switch again, in physical pixels.
const PUSHBACK: i32 = 40;

/// The outer edge of the displays `frames` that the pointer at `(x, y)`
/// pushes against, as the direction it leads to: the left or right side
/// of the display it is on, or with `rows` its top or bottom, where no
/// other display lies beyond.
pub fn pushed((x, y): (i32, i32), frames: &[ScreenRect], rows: bool) -> Option<Toward> {
    let holds = |frame: &ScreenRect, (x, y): (i32, i32)| {
        (frame.left..frame.right).contains(&x) && (frame.top..frame.bottom).contains(&y)
    };
    let frame = frames.iter().find(|frame| holds(frame, (x, y)))?;
    let beyond = |point: (i32, i32)| !frames.iter().any(|other| holds(other, point));
    let sides = [
        (x < frame.left + AGAINST, (frame.left - 1, y), Toward::Left),
        (x >= frame.right - AGAINST, (frame.right, y), Toward::Right),
        (
            rows && y < frame.top + AGAINST,
            (x, frame.top - 1),
            Toward::Up,
        ),
        (
            rows && y >= frame.bottom - AGAINST,
            (x, frame.bottom),
            Toward::Down,
        ),
    ];
    { sides.into_iter() }
        .find(|&(against, past, _)| against && beyond(past))
        .map(|(_, _, toward)| toward)
}

/// Waits for the pointer to be pushed against an edge long enough, and
/// for it to come back once it switched.
#[derive(Debug, Default)]
pub struct EdgePush {
    /// The edge pushed against and since when.
    pushing: Option<(Toward, Instant)>,
    /// Where the pointer was when it switched last, while it has not come
    /// back far enough.
    switched_at: Option<(i32, i32)>,
}

impl EdgePush {
    /// Takes in that the pointer is at `point`, pushing toward `toward`
    /// if it is against an edge, at `now`. The direction to switch to now
    /// is returned, once it has pushed there for `delay`.
    pub fn see(
        &mut self,
        point: (i32, i32),
        toward: Option<Toward>,
        delay: Duration,
        now: Instant,
    ) -> Option<Toward> {
        if let Some((x, y)) = self.switched_at {
            if (point.0 - x).abs() < PUSHBACK && (point.1 - y).abs() < PUSHBACK {
                self.pushing = None;
                return None;
            }
            self.switched_at = None;
        }
        let Some(toward) = toward else {
            self.pushing = None;
            return None;
        };
        let since = match self.pushing {
            Some((pushing, since)) if pushing == toward => since,
            _ => {
                self.pushing = Some((toward, now));
                now
            }
        };
        if now.duration_since(since) < delay {
            return None;
        }
        self.pushing = None;
        self.switched_at = Some(point);
        Some(toward)
    }

    /// Whether it switched and the pointer has not come back yet: the
    /// window is not to snap at that edge then.
    pub fn just_switched(&self) -> bool {
        self.switched_at.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    const RIGHT: ScreenRect = ScreenRect {
        left: 1920,
        top: 0,
        right: 3840,
        bottom: 1080,
    };

    #[test]
    fn only_the_outer_edges_are_pushed_against() {
        let both = [LEFT, RIGHT];
        assert_eq!(pushed((0, 500), &both, false), Some(Toward::Left));
        assert_eq!(pushed((3839, 500), &both, false), Some(Toward::Right));
        // Where the two displays meet, the pointer goes on to the other.
        assert_eq!(pushed((1919, 500), &both, false), None);
        assert_eq!(pushed((1920, 500), &both, false), None);
        assert_eq!(pushed((900, 500), &both, false), None);
        // The top and the bottom only where the workspaces are in rows.
        assert_eq!(pushed((900, 0), &both, false), None);
        assert_eq!(pushed((900, 0), &both, true), Some(Toward::Up));
        assert_eq!(pushed((900, 1079), &both, true), Some(Toward::Down));
        // A display above the right one shares its top edge.
        let above = ScreenRect {
            left: 1920,
            top: -1080,
            right: 3840,
            bottom: 0,
        };
        assert_eq!(pushed((2500, 0), &[LEFT, RIGHT, above], true), None);
        assert_eq!(
            pushed((2500, -1080), &[LEFT, RIGHT, above], true),
            Some(Toward::Up)
        );
        // A display beyond only part of the edge shares only that part.
        let lower = ScreenRect {
            left: 1920,
            top: 540,
            right: 3840,
            bottom: 1620,
        };
        assert_eq!(
            pushed((1919, 100), &[LEFT, lower], false),
            Some(Toward::Right)
        );
        assert_eq!(pushed((1919, 700), &[LEFT, lower], false), None);
    }

    #[test]
    fn it_switches_after_the_delay_and_again_only_after_the_pushback() {
        let mut push = EdgePush::default();
        let start = Instant::now();
        let at = |millis| start + Duration::from_millis(millis);
        let delay = Duration::from_millis(600);
        let left = Some(Toward::Left);
        assert_eq!(push.see((0, 500), left, delay, at(0)), None);
        assert_eq!(push.see((0, 510), left, delay, at(500)), None);
        assert_eq!(push.see((0, 520), left, delay, at(610)), left);
        assert!(push.just_switched());
        // Held there, it does not switch again.
        assert_eq!(push.see((0, 520), left, delay, at(2000)), None);
        assert_eq!(push.see((30, 520), None, delay, at(2100)), None);
        // Back far enough, it can.
        assert_eq!(push.see((60, 520), None, delay, at(2200)), None);
        assert!(!push.just_switched());
        assert_eq!(push.see((0, 520), left, delay, at(2300)), None);
        assert_eq!(push.see((0, 520), left, delay, at(2900)), left);
        // Leaving the edge before the delay switches nothing.
        let mut push = EdgePush::default();
        assert_eq!(push.see((0, 500), left, delay, at(0)), None);
        assert_eq!(push.see((5, 500), None, delay, at(300)), None);
        assert_eq!(push.see((0, 500), left, delay, at(700)), None);
    }
}
