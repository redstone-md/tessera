// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shared edges of snapped windows, as the custom tiling of KWin and the
//! snap groups of Windows have them: the windows Tessera snapped on one
//! display are a group, and when one of them is resized along an edge it
//! shares with others, their touching edges follow it. An edge is shared
//! with a window whose opposite edge lies across the gap between snapped
//! windows from it, within a few pixels, and along a line that runs on
//! through windows whose same edge lies on it too, as the middle line of
//! four quarters runs through all four. Each window keeps the least size
//! it can have.
//!
//! What is here only works out where the windows go; the snapping moves
//! them, live while the edge is dragged and exactly once it is let go.

use crate::platform::ScreenRect;

/// How far apart, in logical pixels, two edges may be and still touch.
const SLACK: f32 = 2.;

/// A window of a group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Member {
    /// What is seen of it on the screen.
    pub rect: ScreenRect,
    /// The least width and height it can have, as it answers
    /// `WM_GETMINMAXINFO`.
    pub least: (i32, i32),
}

/// A side of a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Top,
    Right,
    Bottom,
}

impl Side {
    const ALL: [Self; 4] = [Self::Left, Self::Top, Self::Right, Self::Bottom];

    fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }

    /// 1 where the side faces toward larger coordinates, -1 otherwise.
    fn outward(self) -> i32 {
        match self {
            Self::Right | Self::Bottom => 1,
            Self::Left | Self::Top => -1,
        }
    }

    /// Where the side of `rect` lies.
    fn of(self, rect: ScreenRect) -> i32 {
        match self {
            Self::Left => rect.left,
            Self::Top => rect.top,
            Self::Right => rect.right,
            Self::Bottom => rect.bottom,
        }
    }

    /// `rect` with this side at `at`, keeping `least` of its length.
    fn set(self, rect: &mut ScreenRect, at: i32, (width, height): (i32, i32)) {
        let (width, height) = (width.max(1), height.max(1));
        match self {
            Self::Left => rect.left = at.min(rect.right - width),
            Self::Right => rect.right = at.max(rect.left + width),
            Self::Top => rect.top = at.min(rect.bottom - height),
            Self::Bottom => rect.bottom = at.max(rect.top + height),
        }
    }

    /// Where `rect` begins and ends along the side.
    fn span(self, rect: ScreenRect) -> (i32, i32) {
        match self {
            Self::Left | Self::Right => (rect.top, rect.bottom),
            Self::Top | Self::Bottom => (rect.left, rect.right),
        }
    }
}

/// How far apart two edges of windows on a display of `scale` may be,
/// in physical pixels, and still touch.
pub fn slack(scale: f32) -> i32 {
    ((SLACK * scale).round() as i32).max(1)
}

/// Where the members of a group go once the one at `moved` was resized to
/// `frame`: those whose edges it shared follow it, those beside it moving
/// the touching edge and those in line with it the same edge, and keep
/// `gap` between them; every one keeps its least size. `slack` is how far
/// apart edges may be and still touch. The rects of all the members are
/// returned in their order, `frame` for the one that was moved.
pub fn follow(
    members: &[Member],
    moved: usize,
    frame: ScreenRect,
    gap: i32,
    slack: i32,
) -> Vec<ScreenRect> {
    let mut placed: Vec<ScreenRect> = members.iter().map(|member| member.rect).collect();
    let Some(before) = members.get(moved).map(|member| member.rect) else {
        return placed;
    };
    placed[moved] = frame;
    for side in Side::ALL {
        let (from, to) = (side.of(before), side.of(frame));
        if from == to {
            continue;
        }
        for (index, same) in seam(members, moved, side, gap, slack) {
            if index == moved {
                continue;
            }
            let least = members[index].least;
            match same {
                true => side.set(&mut placed[index], to, least),
                false => {
                    let across = to + side.outward() * gap;
                    side.opposite().set(&mut placed[index], across, least);
                }
            }
        }
    }
    placed
}

/// The members along the line of the `side` of the one at `moved`, each
/// with whether its own `side` lies on the line (or else its opposite
/// side, across `gap`): those reached from `moved` along the line, a
/// window in line touching the next across the gap, and a window across
/// the line overlapping one on this side of it by more than `slack`.
fn seam(members: &[Member], moved: usize, side: Side, gap: i32, slack: i32) -> Vec<(usize, bool)> {
    let line = side.of(members[moved].rect);
    let across = line + side.outward() * gap;
    let on_line: Vec<(usize, bool)> = { members.iter().enumerate() }
        .filter_map(|(index, member)| {
            if (side.of(member.rect) - line).abs() <= slack {
                Some((index, true))
            } else if (side.opposite().of(member.rect) - across).abs() <= slack {
                Some((index, false))
            } else {
                None
            }
        })
        .collect();
    let joined = |(one, one_same): (usize, bool), (other, other_same): (usize, bool)| {
        let (start, end) = side.span(members[one].rect);
        let (other_start, other_end) = side.span(members[other].rect);
        match one_same == other_same {
            // In line: touching across the gap is enough.
            true => start <= other_end + gap + slack && other_start <= end + gap + slack,
            // Across the line: they are to share some of the edge.
            false => end.min(other_end) - start.max(other_start) > slack,
        }
    };
    let mut reached = vec![(moved, true)];
    let mut next = 0;
    while let Some(&from) = reached.get(next) {
        next += 1;
        for &candidate in &on_line {
            if !reached.iter().any(|&(index, _)| index == candidate.0) && joined(from, candidate) {
                reached.push(candidate);
            }
        }
    }
    reached
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

    fn members(rects: &[ScreenRect]) -> Vec<Member> {
        { rects.iter() }
            .map(|&rect| Member {
                rect,
                least: (100, 100),
            })
            .collect()
    }

    #[test]
    fn the_window_beside_follows_the_edge_it_shares() {
        // Two halves 10 pixels apart.
        let group = members(&[rect(0, 0, 955, 1080), rect(965, 0, 1920, 1080)]);
        let placed = follow(&group, 0, rect(0, 0, 1155, 1080), 10, 2);
        assert_eq!(placed, [rect(0, 0, 1155, 1080), rect(1165, 0, 1920, 1080)]);
        // Two pixels off still touch; three do not.
        let near = members(&[rect(0, 0, 955, 1080), rect(967, 0, 1920, 1080)]);
        assert_eq!(follow(&near, 0, rect(0, 0, 900, 1080), 10, 2)[1].left, 910);
        let apart = members(&[rect(0, 0, 955, 1080), rect(968, 0, 1920, 1080)]);
        assert_eq!(follow(&apart, 0, rect(0, 0, 900, 1080), 10, 2)[1].left, 968);
        // An edge that does not move moves nothing.
        let taller = follow(&group, 0, rect(0, 0, 955, 1000), 10, 2);
        assert_eq!(taller[1], rect(965, 0, 1920, 1080));
    }

    #[test]
    fn the_line_through_four_quarters_moves_all_of_them() {
        let group = members(&[
            rect(0, 0, 960, 540),
            rect(960, 0, 1920, 540),
            rect(0, 540, 960, 1080),
            rect(960, 540, 1920, 1080),
        ]);
        let placed = follow(&group, 0, rect(0, 0, 800, 600), 0, 2);
        assert_eq!(
            placed,
            [
                rect(0, 0, 800, 600),
                rect(800, 0, 1920, 600),
                rect(0, 600, 800, 1080),
                rect(800, 600, 1920, 1080),
            ]
        );
    }

    #[test]
    fn a_chain_of_three_columns_moves_only_the_shared_edge() {
        let group = members(&[
            rect(0, 0, 640, 1080),
            rect(640, 0, 1280, 1080),
            rect(1280, 0, 1920, 1080),
        ]);
        let placed = follow(&group, 1, rect(640, 0, 1400, 1080), 0, 2);
        assert_eq!(
            placed,
            [
                rect(0, 0, 640, 1080),
                rect(640, 0, 1400, 1080),
                rect(1400, 0, 1920, 1080),
            ]
        );
        // A half and two quarters on the right: the left edge of one
        // quarter moves the half and the other quarter with it.
        let group = members(&[
            rect(0, 0, 960, 1080),
            rect(960, 0, 1920, 540),
            rect(960, 540, 1920, 1080),
        ]);
        let placed = follow(&group, 1, rect(1000, 0, 1920, 540), 0, 2);
        assert_eq!(placed[0], rect(0, 0, 1000, 1080));
        assert_eq!(placed[2], rect(1000, 540, 1920, 1080));
        // Its bottom edge moves only the quarter below.
        let placed = follow(&group, 1, rect(960, 0, 1920, 700), 0, 2);
        assert_eq!(placed[0], rect(0, 0, 960, 1080));
        assert_eq!(placed[2], rect(960, 700, 1920, 1080));
    }

    #[test]
    fn windows_that_only_meet_at_a_corner_do_not_follow() {
        let group = members(&[rect(0, 0, 960, 540), rect(960, 540, 1920, 1080)]);
        let placed = follow(&group, 0, rect(0, 0, 1100, 540), 0, 2);
        assert_eq!(placed[1], rect(960, 540, 1920, 1080));
    }

    #[test]
    fn a_window_keeps_its_least_size() {
        let mut group = members(&[rect(0, 0, 960, 1080), rect(960, 0, 1920, 1080)]);
        group[1].least = (500, 200);
        let placed = follow(&group, 0, rect(0, 0, 1700, 1080), 0, 2);
        assert_eq!(placed[1], rect(1420, 0, 1920, 1080));
        group[0].least = (0, 0);
        let placed = follow(&group, 1, rect(-100, 0, 1920, 1080), 0, 2);
        assert_eq!(placed[0], rect(0, 0, 1, 1080));
    }

    #[test]
    fn edges_touch_within_more_pixels_on_a_display_of_a_larger_scale() {
        assert_eq!(slack(1.), 2);
        assert_eq!(slack(1.5), 3);
        assert_eq!(slack(2.), 4);
        assert_eq!(slack(0.1), 1);
        let group = members(&[rect(0, 0, 1440, 1620), rect(1443, 0, 2880, 1620)]);
        let moved = rect(0, 0, 1500, 1620);
        assert_eq!(follow(&group, 0, moved, 0, slack(1.))[1].left, 1443);
        assert_eq!(follow(&group, 0, moved, 0, slack(1.5))[1].left, 1500);
    }
}
