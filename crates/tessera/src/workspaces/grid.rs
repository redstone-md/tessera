// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The workspaces as a grid, as the pager of KWin lays out its desktops:
//! `rows` rows filled from the left, the first row first, and the step
//! from one workspace to its neighbour in a direction.

/// A direction to step in from one workspace to another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toward {
    /// The one after, in reading order.
    Next,
    /// The one before.
    Previous,
    /// The one on the left in the same row.
    Left,
    /// The one on the right in the same row.
    Right,
    /// The one above in the same column.
    Up,
    /// The one below in the same column.
    Down,
}

impl Toward {
    /// Every direction, with the word that names it in the configuration.
    pub const ALL: [(&'static str, Self); 6] = [
        ("next", Self::Next),
        ("previous", Self::Previous),
        ("left", Self::Left),
        ("right", Self::Right),
        ("up", Self::Up),
        ("down", Self::Down),
    ];

    /// The direction `word` names.
    pub fn parse(word: &str) -> Option<Self> {
        let word = word.trim();
        { Self::ALL.into_iter() }
            .find(|(name, _)| name.eq_ignore_ascii_case(word))
            .map(|(_, toward)| toward)
    }
}

/// How many workspaces a row of `count` workspaces in `rows` rows holds:
/// the rows are as full as they can be, the last one taking the rest.
pub fn columns(count: usize, rows: usize) -> usize {
    let rows = rows.clamp(1, count.max(1));
    count.div_ceil(rows).max(1)
}

/// The workspace next to the one at `index` in the direction `toward`,
/// among `count` workspaces in `rows` rows; from the end of a row, a
/// column or the list to its other end where `wrap`. `None` where there is
/// nothing in that direction, which is so for `Up` and `Down` with one
/// row.
pub fn neighbour(
    index: usize,
    count: usize,
    rows: usize,
    toward: Toward,
    wrap: bool,
) -> Option<usize> {
    if index >= count {
        return None;
    }
    let found = match toward {
        Toward::Next | Toward::Right if rows <= 1 => along(index, count, true, wrap),
        Toward::Previous | Toward::Left if rows <= 1 => along(index, count, false, wrap),
        Toward::Next => along(index, count, true, wrap),
        Toward::Previous => along(index, count, false, wrap),
        Toward::Left | Toward::Right => {
            let width = columns(count, rows);
            let start = index / width * width;
            let length = width.min(count - start);
            let at = index - start;
            let at = match toward {
                Toward::Right => along(at, length, true, wrap),
                _ => along(at, length, false, wrap),
            };
            at.map(|at| start + at)
        }
        Toward::Up | Toward::Down if rows <= 1 => None,
        Toward::Up | Toward::Down => {
            let width = columns(count, rows);
            let (row, column) = (index / width, index % width);
            // The rows that hold something in this column.
            let height = (count - column).div_ceil(width);
            let row = match toward {
                Toward::Down => along(row, height, true, wrap),
                _ => along(row, height, false, wrap),
            };
            row.map(|row| row * width + column)
        }
    };
    found.filter(|&found| found != index)
}

/// The place after `at` among `length` places, or before it unless
/// `forward`; from one end to the other where `wrap`.
fn along(at: usize, length: usize, forward: bool, wrap: bool) -> Option<usize> {
    match forward {
        true if at + 1 < length => Some(at + 1),
        true => wrap.then_some(0),
        false if at > 0 => Some(at - 1),
        false => wrap.then(|| length.saturating_sub(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_direction_is_named_by_its_word() {
        assert_eq!(Toward::parse("next"), Some(Toward::Next));
        assert_eq!(Toward::parse(" Up "), Some(Toward::Up));
        assert_eq!(Toward::parse("sideways"), None);
    }

    #[test]
    fn the_rows_are_as_full_as_they_can_be() {
        assert_eq!(columns(4, 1), 4);
        assert_eq!(columns(4, 2), 2);
        assert_eq!(columns(5, 2), 3);
        assert_eq!(columns(3, 5), 1);
        assert_eq!(columns(0, 2), 1);
    }

    #[test]
    fn in_one_row_the_steps_go_along_it() {
        assert_eq!(neighbour(0, 4, 1, Toward::Next, false), Some(1));
        assert_eq!(neighbour(3, 4, 1, Toward::Next, false), None);
        assert_eq!(neighbour(3, 4, 1, Toward::Next, true), Some(0));
        assert_eq!(neighbour(0, 4, 1, Toward::Previous, true), Some(3));
        assert_eq!(neighbour(2, 4, 1, Toward::Left, false), Some(1));
        assert_eq!(neighbour(2, 4, 1, Toward::Right, false), Some(3));
        assert_eq!(neighbour(2, 4, 1, Toward::Up, true), None);
        assert_eq!(neighbour(2, 4, 1, Toward::Down, true), None);
    }

    #[test]
    fn one_workspace_has_no_neighbour() {
        for (_, toward) in Toward::ALL {
            assert_eq!(neighbour(0, 1, 1, toward, true), None);
            assert_eq!(neighbour(0, 1, 2, toward, true), None);
        }
        assert_eq!(neighbour(4, 4, 1, Toward::Previous, true), None);
    }

    #[test]
    fn in_a_grid_left_and_right_stay_in_the_row() {
        // 0 1 2
        // 3 4
        assert_eq!(neighbour(2, 5, 2, Toward::Right, false), None);
        assert_eq!(neighbour(2, 5, 2, Toward::Right, true), Some(0));
        assert_eq!(neighbour(3, 5, 2, Toward::Left, true), Some(4));
        assert_eq!(neighbour(4, 5, 2, Toward::Right, true), Some(3));
        assert_eq!(neighbour(1, 5, 2, Toward::Left, false), Some(0));
        // Next and Previous go on into the next row.
        assert_eq!(neighbour(2, 5, 2, Toward::Next, false), Some(3));
        assert_eq!(neighbour(3, 5, 2, Toward::Previous, false), Some(2));
    }

    #[test]
    fn in_a_grid_up_and_down_stay_in_the_column() {
        // 0 1 2
        // 3 4
        assert_eq!(neighbour(0, 5, 2, Toward::Down, false), Some(3));
        assert_eq!(neighbour(4, 5, 2, Toward::Up, false), Some(1));
        assert_eq!(neighbour(3, 5, 2, Toward::Down, false), None);
        assert_eq!(neighbour(3, 5, 2, Toward::Down, true), Some(0));
        assert_eq!(neighbour(0, 5, 2, Toward::Up, true), Some(3));
        // The last column has nothing in the last row.
        assert_eq!(neighbour(2, 5, 2, Toward::Down, false), None);
        assert_eq!(neighbour(2, 5, 2, Toward::Down, true), None);
        assert_eq!(neighbour(2, 5, 2, Toward::Up, true), None);
    }

    #[test]
    fn three_rows_wrap_from_the_bottom_to_the_top() {
        // 0 1
        // 2 3
        // 4 5
        assert_eq!(neighbour(1, 6, 3, Toward::Down, false), Some(3));
        assert_eq!(neighbour(5, 6, 3, Toward::Down, true), Some(1));
        assert_eq!(neighbour(0, 6, 3, Toward::Up, true), Some(4));
        assert_eq!(neighbour(5, 6, 3, Toward::Right, true), Some(4));
    }
}
