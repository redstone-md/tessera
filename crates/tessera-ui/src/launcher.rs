// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

mod model;
mod placement;
mod preferences;

pub(crate) use model::{LauncherInventory, LauncherRows};
pub(crate) use placement::launcher_rect;
pub use preferences::{LauncherDisplayMode, LauncherPreferences};

/// Direction within the caller's row-major launcher grid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Navigation {
    Up,
    Down,
    Left,
    Right,
}

/// Stores identity, never an index: callers supply the current ordered, unique
/// displayed keys on every operation. Returned indices belong to that list only.
#[derive(Default)]
pub(crate) struct LauncherSelection {
    key: Option<String>,
}

impl LauncherSelection {
    pub(crate) fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    pub(crate) fn reset(&mut self) {
        self.key = None;
    }

    /// A new query discards prior selection; only an active query preselects.
    pub(crate) fn search_changed(&mut self, keys: &[String], active_query: bool) -> Option<usize> {
        self.reset();
        self.reconcile(keys, active_query)
    }

    /// Keep a displayed identity across reorder. If absent, fall back to the
    /// first result only for an active query; blank-query lists stay unselected.
    pub(crate) fn reconcile(&mut self, keys: &[String], active_query: bool) -> Option<usize> {
        if let Some(index) = self
            .key()
            .and_then(|selected| keys.iter().position(|key| key == selected))
        {
            return Some(index);
        }

        self.key = if active_query {
            keys.first().cloned()
        } else {
            None
        };
        self.key.as_ref().map(|_| 0)
    }

    /// Reject keys outside the displayed list without changing selection.
    pub(crate) fn select(&mut self, keys: &[String], key: &str) -> Option<usize> {
        let index = keys.iter().position(|candidate| candidate == key)?;
        self.key = Some(keys[index].clone());
        Some(index)
    }

    /// Any direction establishes the first selection before moving. Movement
    /// never wraps or fills a missing cell in the final row. Zero columns leaves
    /// an existing selection in place; an empty list clears it.
    pub(crate) fn navigate(
        &mut self,
        keys: &[String],
        direction: Navigation,
        columns: usize,
    ) -> Option<usize> {
        let Some(current) = self.reconcile(keys, false) else {
            return self.reconcile(keys, true);
        };
        if columns == 0 {
            return Some(current);
        }

        let target = match direction {
            Navigation::Up => current.checked_sub(columns),
            Navigation::Down => current.checked_add(columns),
            Navigation::Left if current % columns != 0 => current.checked_sub(1),
            Navigation::Right if current % columns < columns - 1 => current.checked_add(1),
            Navigation::Left | Navigation::Right => None,
        }
        .filter(|&index| index < keys.len())
        .unwrap_or(current);

        if target != current {
            self.key = Some(keys[target].clone());
        }
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::{LauncherSelection, Navigation};

    fn keys(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("opaque:{index}")).collect()
    }

    #[test]
    fn query_changes_preselect_only_nonempty_query_results() {
        let keys = keys(3);
        let mut selection = LauncherSelection::default();
        assert_eq!(selection.key(), None);
        assert_eq!(selection.search_changed(&keys, false), None);
        assert_eq!(selection.key(), None);
        assert_eq!(selection.search_changed(&keys, true), Some(0));
        assert_eq!(selection.key(), Some(keys[0].as_str()));

        selection.select(&keys, &keys[2]);
        assert_eq!(selection.search_changed(&keys, true), Some(0));
        assert_eq!(selection.key(), Some(keys[0].as_str()));
        assert_eq!(selection.search_changed(&keys, false), None);
        assert_eq!(selection.key(), None);
        assert_eq!(selection.search_changed(&[], true), None);
        assert_eq!(selection.key(), None);
    }

    #[test]
    fn reconciliation_preserves_identity_across_reorder_for_either_query_mode() {
        let keys = keys(3);
        let reordered = vec![keys[2].clone(), keys[0].clone(), keys[1].clone()];
        let mut selection = LauncherSelection::default();
        selection.select(&keys, &keys[2]);

        assert_eq!(selection.reconcile(&reordered, false), Some(0));
        assert_eq!(selection.key(), Some(keys[2].as_str()));
        assert_eq!(selection.reconcile(&keys, true), Some(2));
        assert_eq!(selection.key(), Some(keys[2].as_str()));
    }

    #[test]
    fn removed_selection_falls_back_only_for_active_query() {
        let keys = keys(3);
        let remaining = &keys[..2];
        let mut selection = LauncherSelection::default();
        selection.select(&keys, &keys[2]);
        assert_eq!(selection.reconcile(remaining, false), None);
        assert_eq!(selection.key(), None);
        assert_eq!(selection.reconcile(remaining, false), None);

        selection.select(&keys, &keys[2]);
        assert_eq!(selection.reconcile(remaining, true), Some(0));
        assert_eq!(selection.key(), Some(keys[0].as_str()));
        assert_eq!(selection.reconcile(&[], true), None);
        assert_eq!(selection.key(), None);
    }

    #[test]
    fn fabricated_focus_key_does_not_replace_selection() {
        let keys = keys(3);
        let mut selection = LauncherSelection::default();
        assert_eq!(selection.select(&keys, "fabricated"), None);
        assert_eq!(selection.key(), None);
        assert_eq!(selection.select(&keys, &keys[1]), Some(1));
        assert_eq!(selection.select(&keys, "fabricated"), None);
        assert_eq!(selection.key(), Some(keys[1].as_str()));
        assert_eq!(selection.select(&[], &keys[1]), None);
        assert_eq!(selection.key(), Some(keys[1].as_str()));
    }

    #[test]
    fn any_direction_starts_at_first_and_reset_clears_identity() {
        let keys = keys(10);
        let mut selection = LauncherSelection::default();
        for direction in [
            Navigation::Up,
            Navigation::Down,
            Navigation::Left,
            Navigation::Right,
        ] {
            assert_eq!(selection.navigate(&keys, direction, 7), Some(0));
            assert_eq!(selection.key(), Some(keys[0].as_str()));
            selection.reset();
            assert_eq!(selection.key(), None);
        }

        selection.select(&keys, &keys[9]);
        assert_eq!(selection.navigate(&keys[..2], Navigation::Down, 7), Some(0));
        assert_eq!(selection.key(), Some(keys[0].as_str()));
    }

    #[test]
    fn seven_column_navigation_respects_edges_and_incomplete_last_row() {
        let keys = keys(10);
        let mut selection = LauncherSelection::default();
        for (start, direction, expected) in [
            (0, Navigation::Up, 0),
            (0, Navigation::Left, 0),
            (0, Navigation::Down, 7),
            (0, Navigation::Right, 1),
            (6, Navigation::Right, 6),
            (6, Navigation::Down, 6),
            (7, Navigation::Left, 7),
            (7, Navigation::Up, 0),
            (7, Navigation::Right, 8),
            (2, Navigation::Down, 9),
            (3, Navigation::Down, 3),
            (9, Navigation::Right, 9),
            (9, Navigation::Down, 9),
            (9, Navigation::Up, 2),
            (9, Navigation::Left, 8),
        ] {
            selection.select(&keys, &keys[start]);
            assert_eq!(selection.navigate(&keys, direction, 7), Some(expected));
            assert_eq!(selection.key(), Some(keys[expected].as_str()));
        }
    }

    #[test]
    fn navigation_uses_caller_columns_including_single_and_oversized_rows() {
        let keys = keys(5);
        let mut selection = LauncherSelection::default();
        for (start, direction, columns, expected) in [
            (0, Navigation::Down, 3, 3),
            (2, Navigation::Right, 3, 2),
            (2, Navigation::Down, 3, 2),
            (3, Navigation::Left, 3, 3),
            (4, Navigation::Up, 3, 1),
            (4, Navigation::Right, 3, 4),
            (2, Navigation::Left, 1, 2),
            (2, Navigation::Right, 1, 2),
            (2, Navigation::Up, 1, 1),
            (2, Navigation::Down, 1, 3),
            (2, Navigation::Right, usize::MAX, 3),
            (2, Navigation::Down, usize::MAX, 2),
            (2, Navigation::Up, usize::MAX, 2),
        ] {
            selection.select(&keys, &keys[start]);
            assert_eq!(
                selection.navigate(&keys, direction, columns),
                Some(expected)
            );
            assert_eq!(selection.key(), Some(keys[expected].as_str()));
        }
    }

    #[test]
    fn empty_lists_clear_selection_and_zero_columns_do_not_move() {
        let keys = keys(3);
        let mut selection = LauncherSelection::default();
        for direction in [
            Navigation::Up,
            Navigation::Down,
            Navigation::Left,
            Navigation::Right,
        ] {
            assert_eq!(selection.navigate(&keys, direction, 0), Some(0));
            selection.select(&keys, &keys[2]);
            assert_eq!(selection.navigate(&keys, direction, 0), Some(2));
            assert_eq!(selection.key(), Some(keys[2].as_str()));
            assert_eq!(selection.navigate(&[], direction, 0), None);
            assert_eq!(selection.key(), None);
            assert_eq!(selection.navigate(&[], direction, 7), None);
            assert_eq!(selection.key(), None);
        }
    }
}
