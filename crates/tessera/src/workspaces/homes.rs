// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One row of workspaces of Tessera's own: which window lives on which
//! workspace, which are hidden, the one shown, and what is hidden and
//! shown when another one is gone to, a window is sent elsewhere, or a
//! workspace is made or removed.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::platform::AppWindowId;

/// How long a window may take to hide. One that is still, or again, seen
/// on another workspace after that showed itself, and stays.
const HIDING_TIME: Duration = Duration::from_millis(1500);

/// Which window belongs to which workspace, and which are hidden.
#[derive(Default)]
pub(super) struct Homes {
    /// The number that stays of every workspace, the first first; none
    /// when the workspaces are not kept here.
    pub(super) ids: Vec<u64>,
    pub(super) current: usize,
    /// The workspace of every window that was seen.
    pub(super) home: HashMap<AppWindowId, usize>,
    /// The windows that were asked to hide, and when.
    pub(super) hidden: HashMap<AppWindowId, Instant>,
    /// The window that was last in front, workspace by workspace.
    pub(super) fronts: Vec<Option<AppWindowId>>,
    /// Whether which window lives where, or the workspace shown, changed
    /// since this was last cleared.
    pub(super) moved: bool,
}

/// What is to be done to the windows after a change.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Change {
    pub(super) hide: Vec<AppWindowId>,
    pub(super) show: Vec<AppWindowId>,
    /// The window to bring to the front.
    pub(super) front: Option<AppWindowId>,
    /// Whether the workspace the change leads to comes after the one
    /// before, so that what leaves does so to the left.
    pub(super) onward: bool,
}

impl Homes {
    /// Workspaces numbered `ids`, the first one shown.
    pub(super) fn with_ids(ids: Vec<u64>) -> Self {
        Self {
            fronts: vec![None; ids.len()],
            ids,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(super) fn new(count: usize) -> Self {
        Self::with_ids((1..=count as u64).collect())
    }

    /// How many workspaces there are.
    pub(super) fn count(&self) -> usize {
        self.ids.len()
    }

    /// Takes in the windows that are seen right now and the one in front.
    /// Whether the hidden windows changed is returned.
    pub(super) fn see(
        &mut self,
        seen: &[AppWindowId],
        front: Option<AppWindowId>,
        now: Instant,
    ) -> bool {
        let mut changed = false;
        for &id in seen {
            let home = match self.home.get(&id) {
                Some(&home) => home,
                None => {
                    self.home.insert(id, self.current);
                    self.moved = true;
                    self.current
                }
            };
            if home == self.current {
                continue;
            }
            let hiding = self
                .hidden
                .get(&id)
                .is_some_and(|&since| now.duration_since(since) < HIDING_TIME);
            if !hiding {
                // The window showed itself, or was never hidden.
                self.home.insert(id, self.current);
                self.moved = true;
                changed |= self.hidden.remove(&id).is_some();
            }
        }
        if let Some(front) = front
            && self.home.get(&front) == Some(&self.current)
        {
            self.fronts[self.current] = Some(front);
        }
        changed
    }

    /// Whether `id` was asked to hide so lately that it may still be on
    /// its way.
    pub(super) fn hiding(&self, id: AppWindowId, now: Instant) -> bool {
        let hidden = self.hidden.get(&id);
        hidden.is_some_and(|&since| now.duration_since(since) < HIDING_TIME)
    }

    /// Forgets the window `id`, which went elsewhere. Whether the hidden
    /// windows changed is returned.
    pub(super) fn evict(&mut self, id: AppWindowId) -> bool {
        self.moved |= self.home.remove(&id).is_some();
        for front in &mut self.fronts {
            *front = front.filter(|&front| front != id);
        }
        self.hidden.remove(&id).is_some()
    }

    /// Forgets the windows of which `exists` says that they are gone.
    /// Whether the hidden windows changed is returned.
    pub(super) fn forget(&mut self, exists: impl Fn(AppWindowId) -> bool) -> bool {
        let (homes, hidden) = (self.home.len(), self.hidden.len());
        self.home.retain(|&id, _| exists(id));
        self.hidden.retain(|&id, _| exists(id));
        for front in &mut self.fronts {
            *front = front.filter(|&id| exists(id));
        }
        self.moved |= homes != self.home.len();
        hidden != self.hidden.len()
    }

    /// The windows of every workspace, each list in an order that stays.
    pub(super) fn residents(&self) -> Vec<Vec<AppWindowId>> {
        let mut residents = vec![Vec::new(); self.count()];
        for (&id, &home) in &self.home {
            if let Some(windows) = residents.get_mut(home) {
                windows.push(id);
            }
        }
        for windows in &mut residents {
            windows.sort_by_key(|id| id.raw());
        }
        residents
    }

    /// Makes `id` the window in front on its workspace, and says which
    /// workspace that is.
    pub(super) fn bring(&mut self, id: AppWindowId) -> Option<usize> {
        let home = *self.home.get(&id)?;
        *self.fronts.get_mut(home)? = Some(id);
        Some(home)
    }

    /// Has the window `id` live on the workspace at `index` from now on:
    /// it is hidden when it leaves the workspace shown and shown when it
    /// comes there.
    pub(super) fn settle(&mut self, id: AppWindowId, index: usize, now: Instant) -> Change {
        let Some(&home) = self.home.get(&id) else {
            return Change::default();
        };
        if index >= self.count() || home == index {
            return Change::default();
        }
        self.home.insert(id, index);
        self.moved = true;
        if self.fronts[home] == Some(id) {
            self.fronts[home] = None;
        }
        if self.fronts[index].is_none() {
            self.fronts[index] = Some(id);
        }
        let mut change = Change::default();
        if index == self.current {
            self.hidden.remove(&id);
            change.show.push(id);
        } else if home == self.current {
            self.hidden.insert(id, now);
            change.hide.push(id);
            change.onward = index < self.current;
        }
        change
    }

    /// Goes to the workspace at `index`.
    pub(super) fn go(&mut self, index: usize, now: Instant) -> Change {
        if index >= self.count() || index == self.current {
            return Change::default();
        }
        let mut change = Change::default();
        for (&id, &home) in &self.home {
            if home == self.current && !self.hidden.contains_key(&id) {
                change.hide.push(id);
            } else if home == index {
                change.show.push(id);
            }
        }
        for &id in &change.hide {
            self.hidden.insert(id, now);
        }
        for id in &change.show {
            self.hidden.remove(id);
        }
        change.onward = index > self.current;
        self.current = index;
        self.moved = true;
        change.front = self.fronts[index];
        change
    }

    /// Sends the window that was last in front on the workspace shown to
    /// the workspace at `index`.
    pub(super) fn send(&mut self, index: usize, now: Instant) -> Change {
        if index >= self.count() || index == self.current {
            return Change::default();
        }
        let Some(id) = self.fronts[self.current].take() else {
            return Change::default();
        };
        self.home.insert(id, index);
        self.hidden.insert(id, now);
        self.fronts[index] = Some(id);
        self.moved = true;
        Change {
            hide: vec![id],
            // A window that is sent leaves towards where it goes.
            onward: index < self.current,
            ..Change::default()
        }
    }

    /// Adds a workspace numbered `id` at the end; its place is returned.
    pub(super) fn add(&mut self, id: u64) -> usize {
        self.ids.push(id);
        self.fronts.push(None);
        self.moved = true;
        self.count() - 1
    }

    /// Removes the workspace at `index`, which is not the only one. Its
    /// windows go to the workspace before it, or to the one after the
    /// first; where that is the workspace shown they are shown, and where
    /// the workspace removed was shown, the windows of the one they go to
    /// come with them. No window stays hidden on a workspace that is gone.
    pub(super) fn remove(&mut self, index: usize) -> Change {
        if index >= self.count() || self.count() < 2 {
            return Change::default();
        }
        // The places after the change.
        let target = index.saturating_sub(1);
        let place = |home: usize| match home.cmp(&index) {
            std::cmp::Ordering::Less => home,
            std::cmp::Ordering::Equal => target,
            std::cmp::Ordering::Greater => home - 1,
        };
        let current = place(self.current);
        let mut change = Change {
            // Those that come arrive from where their workspace was.
            onward: index == 0,
            ..Change::default()
        };
        for (&id, home) in &mut self.home {
            *home = place(*home);
            if *home == current && self.hidden.remove(&id).is_some() {
                change.show.push(id);
            }
        }
        let front = self.fronts.remove(index);
        let kept = &mut self.fronts[target];
        *kept = match self.current == index {
            true => front.or(*kept),
            false => kept.or(front),
        };
        self.ids.remove(index);
        self.current = current;
        self.moved = true;
        change
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: isize) -> AppWindowId {
        AppWindowId::from_raw(raw)
    }

    fn sorted(mut ids: Vec<AppWindowId>) -> Vec<isize> {
        ids.sort_by_key(|id| id.raw());
        ids.into_iter().map(AppWindowId::raw).collect()
    }

    fn listed(homes: &Homes) -> Vec<Vec<isize>> {
        homes.residents().into_iter().map(sorted).collect()
    }

    #[test]
    fn the_windows_are_listed_workspace_by_workspace() {
        let start = Instant::now();
        let mut homes = Homes::new(3);
        homes.see(&[id(7), id(3)], Some(id(3)), start);
        homes.go(2, start);
        homes.see(&[id(5)], Some(id(5)), start);
        assert_eq!(listed(&homes), [vec![3, 7], vec![], vec![5]]);
    }

    #[test]
    fn a_window_brought_forward_is_in_front_where_it_lives() {
        let start = Instant::now();
        let mut homes = Homes::new(2);
        homes.see(&[id(1), id(2)], Some(id(1)), start);
        homes.go(1, start);
        assert_eq!(homes.bring(id(2)), Some(0));
        assert_eq!(homes.go(0, start).front, Some(id(2)));
        assert_eq!(homes.bring(id(9)), None);
    }

    #[test]
    fn a_window_that_left_for_another_display_is_forgotten() {
        let start = Instant::now();
        let mut homes = Homes::new(2);
        homes.see(&[id(1), id(2)], Some(id(1)), start);
        homes.go(1, start);
        // Both were asked to hide a moment ago and are still on their way.
        assert!(homes.hiding(id(1), start));
        assert!(!homes.hiding(id(1), start + HIDING_TIME));
        assert!(!homes.hiding(id(7), start));
        // The one that is gone leaves nothing behind.
        assert!(homes.evict(id(1)));
        assert_eq!(homes.fronts, [None, None]);
        assert!(!homes.home.contains_key(&id(1)));
        assert!(!homes.hidden.contains_key(&id(1)));
        assert!(homes.hidden.contains_key(&id(2)));
        // A window that was not hidden changes nothing about the hidden.
        homes.go(0, start);
        assert!(!homes.evict(id(2)));
        assert!(!homes.evict(id(7)));
    }

    #[test]
    fn a_window_settles_on_another_workspace() {
        let start = Instant::now();
        let mut homes = Homes::new(3);
        homes.see(&[id(1), id(2)], Some(id(1)), start);
        // Away from the workspace shown it is hidden.
        let away = homes.settle(id(1), 2, start);
        assert_eq!(sorted(away.hide), [1]);
        assert!(away.show.is_empty());
        assert_eq!(homes.fronts, [None, None, Some(id(1))]);
        // Between two workspaces that are not shown nothing is seen.
        let aside = homes.settle(id(1), 1, start);
        assert!(aside.hide.is_empty() && aside.show.is_empty());
        // Back to the workspace shown it is shown.
        let back = homes.settle(id(1), 0, start);
        assert_eq!(sorted(back.show), [1]);
        assert!(!homes.hidden.contains_key(&id(1)));
        // Where it is, and where there is nothing, nothing changes.
        assert!(homes.settle(id(1), 0, start).show.is_empty());
        assert!(homes.settle(id(1), 7, start).hide.is_empty());
        assert!(homes.settle(id(9), 1, start).hide.is_empty());
        assert_eq!(listed(&homes), [vec![1, 2], vec![], vec![]]);
    }

    #[test]
    fn going_to_a_workspace_hides_the_windows_of_the_one_left() {
        let start = Instant::now();
        let mut homes = Homes::new(3);
        homes.see(&[id(1), id(2)], Some(id(2)), start);
        let change = homes.go(1, start);
        assert_eq!(sorted(change.hide), [1, 2]);
        assert!(change.show.is_empty());
        assert_eq!(change.front, None);

        homes.see(&[id(3)], Some(id(3)), start);
        let back = homes.go(0, start);
        assert_eq!(sorted(back.hide), [3]);
        assert_eq!(sorted(back.show), [1, 2]);
        assert_eq!(back.front, Some(id(2)));
        assert_eq!(sorted(homes.hidden.keys().copied().collect()), [3]);
    }

    #[test]
    fn a_window_still_hiding_keeps_its_workspace() {
        let start = Instant::now();
        let mut homes = Homes::new(2);
        homes.see(&[id(1)], None, start);
        homes.go(1, start);
        assert!(!homes.see(&[id(1)], None, start + HIDING_TIME / 2));
        assert_eq!(homes.home[&id(1)], 0);
    }

    #[test]
    fn a_window_that_shows_itself_joins_the_workspace_shown() {
        let start = Instant::now();
        let mut homes = Homes::new(2);
        homes.see(&[id(1)], None, start);
        homes.go(1, start);
        assert!(homes.see(&[id(1)], None, start + HIDING_TIME));
        assert_eq!(homes.home[&id(1)], 1);
        assert!(homes.hidden.is_empty());
    }

    #[test]
    fn the_window_in_front_is_sent_to_another_workspace() {
        let start = Instant::now();
        let mut homes = Homes::new(2);
        homes.see(&[id(1), id(2)], Some(id(1)), start);
        let change = homes.send(1, start);
        assert_eq!(sorted(change.hide), [1]);
        assert_eq!(homes.send(1, start), Change::default());
        let there = homes.go(1, start);
        assert_eq!(sorted(there.hide), [2]);
        assert_eq!(sorted(there.show), [1]);
        assert_eq!(there.front, Some(id(1)));
    }

    #[test]
    fn windows_that_are_gone_are_forgotten() {
        let start = Instant::now();
        let mut homes = Homes::new(2);
        homes.see(&[id(1), id(2)], Some(id(1)), start);
        homes.go(1, start);
        assert!(homes.forget(|window| window == id(2)));
        assert_eq!(sorted(homes.hidden.keys().copied().collect()), [2]);
        assert_eq!(homes.fronts[0], None);
    }

    #[test]
    fn workspaces_out_of_reach_change_nothing() {
        let mut homes = Homes::new(2);
        homes.see(&[id(1)], Some(id(1)), Instant::now());
        assert_eq!(homes.go(2, Instant::now()), Change::default());
        assert_eq!(homes.go(0, Instant::now()), Change::default());
        assert_eq!(homes.send(0, Instant::now()), Change::default());
        assert_eq!(homes.remove(2), Change::default());
    }

    #[test]
    fn a_workspace_made_comes_at_the_end_with_its_own_number() {
        let mut homes = Homes::new(2);
        assert_eq!(homes.add(9), 2);
        assert_eq!(homes.ids, [1, 2, 9]);
        assert_eq!(homes.fronts.len(), 3);
        assert!(homes.moved);
        assert!(homes.go(2, Instant::now()).onward);
    }

    #[test]
    fn the_windows_of_a_workspace_removed_go_to_the_one_before() {
        let start = Instant::now();
        // 1 lives on the first workspace, 2 on the second, 3 on the third;
        // the first is shown.
        let mut homes = Homes::new(3);
        homes.see(&[id(1)], Some(id(1)), start);
        homes.go(1, start);
        homes.see(&[id(2)], Some(id(2)), start);
        homes.go(2, start);
        homes.see(&[id(3)], Some(id(3)), start);
        homes.go(0, start);
        let change = homes.remove(1);
        // The second went to the first, which is shown: its window comes.
        assert_eq!(sorted(change.show), [2]);
        assert!(change.hide.is_empty());
        assert_eq!(homes.ids, [1, 3]);
        assert_eq!(homes.current, 0);
        assert_eq!(listed(&homes), [vec![1, 2], vec![3]]);
        assert_eq!(sorted(homes.hidden.keys().copied().collect()), [3]);
        // The first keeps the window in front it had.
        assert_eq!(homes.fronts, [Some(id(1)), Some(id(3))]);
    }

    #[test]
    fn removing_a_workspace_not_shown_keeps_its_windows_hidden() {
        let start = Instant::now();
        let mut homes = Homes::new(4);
        homes.see(&[id(1)], Some(id(1)), start);
        homes.go(2, start);
        homes.see(&[id(3)], Some(id(3)), start);
        homes.go(3, start);
        homes.see(&[id(4)], Some(id(4)), start);
        // The third is removed while the fourth is shown.
        let change = homes.remove(2);
        assert_eq!(change, Change::default());
        assert_eq!(homes.ids, [1, 2, 4]);
        assert_eq!(homes.current, 2);
        assert_eq!(listed(&homes), [vec![1], vec![3], vec![4]]);
        assert_eq!(sorted(homes.hidden.keys().copied().collect()), [1, 3]);
    }

    #[test]
    fn removing_the_workspace_shown_shows_where_its_windows_go() {
        let start = Instant::now();
        let mut homes = Homes::new(3);
        homes.see(&[id(1)], Some(id(1)), start);
        homes.go(1, start);
        homes.see(&[id(2)], Some(id(2)), start);
        let change = homes.remove(1);
        assert_eq!(sorted(change.show), [1]);
        assert_eq!(homes.current, 0);
        assert_eq!(listed(&homes), [vec![1, 2], vec![]]);
        assert!(homes.hidden.is_empty());
        // The window that was in front stays so.
        assert_eq!(homes.fronts[0], Some(id(2)));
    }

    #[test]
    fn removing_the_first_workspace_sends_its_windows_to_the_next() {
        let start = Instant::now();
        let mut homes = Homes::new(2);
        homes.see(&[id(1)], Some(id(1)), start);
        homes.go(1, start);
        homes.see(&[id(2)], Some(id(2)), start);
        let change = homes.remove(0);
        // What was on the first comes from the left of the one shown.
        assert_eq!(sorted(change.show), [1]);
        assert!(change.onward);
        assert_eq!(homes.ids, [2]);
        assert_eq!(homes.current, 0);
        assert_eq!(listed(&homes), [vec![1, 2]]);
        // The only workspace is not removed.
        assert_eq!(homes.remove(0), Change::default());
        assert_eq!(homes.count(), 1);
    }
}
