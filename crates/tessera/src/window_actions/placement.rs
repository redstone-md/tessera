// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Where new windows are put, as KWin's placement of windows does
//! (`[window_actions] placement`): where Windows puts them, in the middle
//! of the work area, under the pointer, in its top left corner, where they
//! cover the least of the other windows, maximized, or each a step below
//! and to the right of the one placed before.
//!
//! A window is placed once, when it is first seen, if it has a title bar,
//! is owned by no other window, is not tiled and no rule says where it
//! goes. Everything here but [`place`] is worked out on rectangles alone.

use gpui::App;
use serde::Deserialize;

use crate::platform::{AppWindow, Display, DisplayKey, ScreenRect, pointer_at};
use crate::tiling::Tiler;

/// How far each window of a cascade is from the one before, in logical
/// pixels across and down.
const CASCADE_STEP: f32 = 32.;

/// How many places across and down are tried for the least overlap,
/// besides those beside the other windows.
const SMART_STEPS: i32 = 8;

/// Where a new window goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Placement {
    /// Where Windows and the program put it.
    #[default]
    Windows,
    /// In the middle of the work area of its display.
    Centered,
    /// Its middle at the pointer, kept within the work area.
    UnderPointer,
    /// In the top left corner of the work area.
    ZeroCorner,
    /// Where it covers the least of the other windows on its display.
    Smart,
    Maximized,
    /// A step below and to the right of the window placed before, back in
    /// the corner once that would leave the work area.
    Cascade,
}

/// The width and the height of `rect`.
fn extent(rect: ScreenRect) -> (i32, i32) {
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// A rectangle `width` by `height` with its top left corner at
/// `(left, top)`, moved into `work` as far as it fits.
fn kept_in(left: i32, top: i32, (width, height): (i32, i32), work: ScreenRect) -> ScreenRect {
    let left = left.clamp(work.left, (work.right - width).max(work.left));
    let top = top.clamp(work.top, (work.bottom - height).max(work.top));
    ScreenRect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// `frame` in the middle of `work`.
pub fn centered(frame: ScreenRect, work: ScreenRect) -> ScreenRect {
    let (width, height) = extent(frame);
    let (room_width, room_height) = extent(work);
    let left = work.left + (room_width - width) / 2;
    let top = work.top + (room_height - height) / 2;
    kept_in(left, top, (width, height), work)
}

/// `frame` with its middle at `pointer`, kept within `work`.
pub fn under_pointer(frame: ScreenRect, pointer: (i32, i32), work: ScreenRect) -> ScreenRect {
    let (width, height) = extent(frame);
    kept_in(
        pointer.0 - width / 2,
        pointer.1 - height / 2,
        (width, height),
        work,
    )
}

/// `frame` in the top left corner of `work`.
pub fn zero_corner(frame: ScreenRect, work: ScreenRect) -> ScreenRect {
    kept_in(work.left, work.top, extent(frame), work)
}

/// How much of `a` lies on `b`, in square pixels.
fn overlap(a: ScreenRect, b: ScreenRect) -> i64 {
    let width = (a.right.min(b.right) - a.left.max(b.left)).max(0);
    let height = (a.bottom.min(b.bottom) - a.top.max(b.top)).max(0);
    i64::from(width) * i64::from(height)
}

/// Where in `work` a window as large as `frame` covers the least of
/// `others`: of the places along the edges of the work area and of the
/// other windows, and of a grid over the work area, the one of least
/// overlap, the highest and then the leftmost of those alike.
pub fn smart(frame: ScreenRect, work: ScreenRect, others: &[ScreenRect]) -> ScreenRect {
    let (width, height) = extent(frame);
    let along = |start: i32, end: i32, size: i32, edges: &mut dyn Iterator<Item = i32>| {
        let mut places: Vec<i32> = (0..=SMART_STEPS)
            .map(|step| start + (end - size - start).max(0) * step / SMART_STEPS)
            .collect();
        places.extend(edges);
        places.retain(|&place| place >= start && place + size <= end.max(start + size));
        places.sort_unstable();
        places.dedup();
        places
    };
    let across = along(
        work.left,
        work.right,
        width,
        &mut others
            .iter()
            .flat_map(|other| [other.right, other.left - width]),
    );
    let down = along(
        work.top,
        work.bottom,
        height,
        &mut others
            .iter()
            .flat_map(|other| [other.bottom, other.top - height]),
    );
    let mut best: Option<(i64, ScreenRect)> = None;
    for &top in &down {
        for &left in &across {
            let candidate = kept_in(left, top, (width, height), work);
            let covered: i64 = others.iter().map(|&other| overlap(candidate, other)).sum();
            if best.is_none_or(|(least, _)| covered < least) {
                best = Some((covered, candidate));
            }
        }
    }
    best.map_or_else(|| zero_corner(frame, work), |(_, place)| place)
}

/// Where the next window of a cascade goes: a `step` below and to the
/// right of `last`, the window placed before on the same display, or in
/// the top left corner of `work` where there was none or the step would
/// take it out of the work area.
pub fn cascade(
    frame: ScreenRect,
    work: ScreenRect,
    last: Option<ScreenRect>,
    step: i32,
) -> ScreenRect {
    let (width, height) = extent(frame);
    let Some(last) = last else {
        return zero_corner(frame, work);
    };
    let (left, top) = (last.left + step, last.top + step);
    if left + width > work.right || top + height > work.bottom {
        return zero_corner(frame, work);
    }
    kept_in(left, top, (width, height), work)
}

/// The window placed last by a cascade, and its display.
#[derive(Default)]
pub struct Cascade(Option<(DisplayKey, ScreenRect)>);

/// Puts the new window `window` where `placement` says, unless the tiling
/// has it, it has no title bar, it is minimized or maximized already. The
/// windows of a cascade are counted in `cascade`.
pub fn place(placement: Placement, window: &AppWindow, cascade_state: &mut Cascade, cx: &App) {
    let id = window.id;
    if placement == Placement::Windows || Tiler::tiles(window, cx) {
        return;
    }
    if !id.has_title_bar() || id.is_minimized() || id.is_maximized() || id.fills_display() {
        return;
    }
    let (Some(frame), Some(display)) = (id.frame(), Display::of_window(id)) else {
        return;
    };
    let work = display.work;
    let placed = match placement {
        Placement::Windows => return,
        Placement::Maximized => {
            id.maximize();
            return;
        }
        Placement::Centered => centered(frame, work),
        Placement::ZeroCorner => zero_corner(frame, work),
        Placement::UnderPointer => match pointer_at() {
            Some(pointer) => under_pointer(frame, pointer, work),
            None => return,
        },
        Placement::Smart => {
            let others: Vec<ScreenRect> = { AppWindow::enumerate().into_iter() }
                .map(|other| other.id)
                .filter(|&other| other != id && !other.is_minimized())
                .filter(|&other| Display::of_window(other).is_some_and(|on| on.key == display.key))
                .filter_map(|other| other.frame())
                .collect();
            smart(frame, work, &others)
        }
        Placement::Cascade => {
            let last = { cascade_state.0 }
                .filter(|(key, _)| *key == display.key)
                .map(|(_, last)| last);
            let step = (CASCADE_STEP * display.scale.max(0.1)).round() as i32;
            let placed = cascade(frame, work, last, step);
            cascade_state.0 = Some((display.key, placed));
            placed
        }
    };
    if placed != frame {
        id.place(placed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    const WORK: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    #[test]
    fn a_window_is_put_in_the_middle_in_the_corner_or_under_the_pointer() {
        let frame = rect(300, 300, 1100, 900);
        assert_eq!(centered(frame, WORK), rect(560, 220, 1360, 820));
        assert_eq!(zero_corner(frame, WORK), rect(0, 0, 800, 600));
        assert_eq!(
            under_pointer(frame, (1000, 500), WORK),
            rect(600, 200, 1400, 800)
        );
        // Near an edge it is kept within the work area.
        assert_eq!(
            under_pointer(frame, (1900, 1030), WORK),
            rect(1120, 440, 1920, 1040)
        );
        // A window larger than the work area keeps its corner in it.
        let large = rect(0, 0, 2400, 1200);
        assert_eq!(centered(large, WORK).left, 0);
        assert_eq!(centered(large, WORK).top, 0);
    }

    #[test]
    fn smart_placement_covers_the_least_of_the_others() {
        let frame = rect(0, 0, 800, 600);
        // Nothing else: the top left corner.
        assert_eq!(smart(frame, WORK, &[]), rect(0, 0, 800, 600));
        // One window in the top left: beside it.
        let left = rect(0, 0, 900, 700);
        assert_eq!(smart(frame, WORK, &[left]), rect(900, 0, 1700, 600));
        // The left and the right are taken: below the lower one, where
        // the least is covered.
        let right = rect(1000, 0, 1920, 400);
        let placed = smart(frame, WORK, &[left, right]);
        let covered = overlap(placed, left) + overlap(placed, right);
        let corner = overlap(rect(0, 0, 800, 600), left);
        assert!(covered < corner, "{placed:?} covers {covered}");
        assert!(placed.left >= WORK.left && placed.right <= WORK.right);
        assert!(placed.top >= WORK.top && placed.bottom <= WORK.bottom);
    }

    #[test]
    fn a_cascade_steps_on_and_begins_again_in_the_corner() {
        let frame = rect(500, 500, 1300, 1100);
        let first = cascade(frame, WORK, None, 32);
        assert_eq!(first, rect(0, 0, 800, 600));
        let second = cascade(frame, WORK, Some(first), 32);
        assert_eq!(second, rect(32, 32, 832, 632));
        // Out of room below: back in the corner.
        let low = rect(100, 420, 900, 1020);
        assert_eq!(cascade(frame, WORK, Some(low), 32), rect(0, 0, 800, 600));
    }

    #[test]
    fn the_placements_are_read_by_their_words() {
        let read = |word: &str| toml::Value::String(word.to_owned()).try_into::<Placement>();
        assert_eq!(read("under-pointer").unwrap(), Placement::UnderPointer);
        assert_eq!(read("zero-corner").unwrap(), Placement::ZeroCorner);
        assert_eq!(read("windows").unwrap(), Placement::default());
        assert!(read("random").is_err());
    }
}
