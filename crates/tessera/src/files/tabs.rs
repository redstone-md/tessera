// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The tabs of a window of the file manager, each one pane or two side by
//! side, and the tabs closed lately, to open again. What a pane is, is up
//! to the window; here it is only kept and ordered.

use super::model::Location;

/// How many closed tabs are kept to open again.
const CLOSED_KEPT: usize = 10;

/// The share of the width the first pane of a split tab takes at first.
pub const EVEN_SPLIT: f32 = 0.5;

/// The least share of the width either pane of a split tab takes.
const LEAST_SPLIT: f32 = 0.2;

/// A tab: one pane, or two side by side, one of them in front.
#[derive(Clone, Debug, PartialEq)]
pub struct Tab<P> {
    panes: Vec<P>,
    active: usize,
    /// The share of the width the first pane takes when there are two.
    split: f32,
}

impl<P> Tab<P> {
    pub fn single(pane: P) -> Self {
        Self {
            panes: vec![pane],
            active: 0,
            split: EVEN_SPLIT,
        }
    }

    pub fn panes(&self) -> &[P] {
        &self.panes
    }

    /// The pane in front.
    pub fn active(&self) -> &P {
        &self.panes[self.active]
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    /// The pane beside the one in front, where the tab is split.
    pub fn other(&self) -> Option<&P> {
        match self.panes.len() {
            2 => Some(&self.panes[1 - self.active]),
            _ => None,
        }
    }

    pub fn is_split(&self) -> bool {
        self.panes.len() == 2
    }

    pub fn split(&self) -> f32 {
        self.split
    }

    /// Sets the share of the width of the first pane, within bounds that
    /// leave either pane usable.
    pub fn set_split(&mut self, share: f32) {
        self.split = share.clamp(LEAST_SPLIT, 1. - LEAST_SPLIT);
    }

    /// Puts `pane` beside the one there is, and in front, unless the tab
    /// is split already, when `pane` is handed back.
    pub fn split_with(&mut self, pane: P) -> Result<(), P> {
        if self.is_split() {
            return Err(pane);
        }
        self.panes.push(pane);
        self.active = 1;
        Ok(())
    }

    /// Takes away the pane that is not in front, where the tab is split.
    pub fn unsplit(&mut self) -> Option<P> {
        if !self.is_split() {
            return None;
        }
        let other = self.panes.remove(1 - self.active);
        self.active = 0;
        Some(other)
    }

    /// Puts the other pane in front; says whether there was one.
    pub fn switch(&mut self) -> bool {
        if !self.is_split() {
            return false;
        }
        self.active = 1 - self.active;
        true
    }

    /// Puts the pane `index` in front, where the tab has it.
    pub fn focus(&mut self, index: usize) {
        if index < self.panes.len() {
            self.active = index;
        }
    }

    /// Swaps the panes of a split tab, the one in front staying in front.
    pub fn swap(&mut self) {
        if self.is_split() {
            self.panes.swap(0, 1);
            self.active = 1 - self.active;
            self.split = 1. - self.split;
        }
    }
}

/// The tabs of a window, one of them in front.
#[derive(Debug)]
pub struct Tabs<P> {
    tabs: Vec<Tab<P>>,
    active: usize,
    /// What the tabs closed lately showed, the last closed last.
    closed: Vec<Vec<Location>>,
}

impl<P> Tabs<P> {
    pub fn new(first: Tab<P>) -> Self {
        Self {
            tabs: vec![first],
            active: 0,
            closed: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn all(&self) -> &[Tab<P>] {
        &self.tabs
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active(&self) -> &Tab<P> {
        &self.tabs[self.active]
    }

    pub fn active_mut(&mut self) -> &mut Tab<P> {
        &mut self.tabs[self.active]
    }

    /// Adds `tab` after the one in front and puts it in front, or with
    /// `behind` leaves the one in front where it is. Says where it went.
    pub fn open(&mut self, tab: Tab<P>, behind: bool) -> usize {
        let at = self.active + 1;
        self.tabs.insert(at, tab);
        if !behind {
            self.active = at;
        }
        at
    }

    /// Takes away the tab `index` and gives it, unless it is the last,
    /// which a window cannot lose: closing it closes the window. The tab
    /// to its right comes in front where it was in front, else the one
    /// in front stays.
    pub fn close(&mut self, index: usize) -> Option<Tab<P>> {
        if self.tabs.len() < 2 || index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        if index < self.active || self.active >= self.tabs.len() {
            self.active -= 1;
        }
        Some(tab)
    }

    /// Keeps what a closed tab showed, to open it again.
    pub fn remember(&mut self, locations: Vec<Location>) {
        if locations.is_empty() {
            return;
        }
        self.closed.push(locations);
        if self.closed.len() > CLOSED_KEPT {
            self.closed.remove(0);
        }
    }

    /// What the tab closed last showed, which is forgotten then.
    pub fn reopen(&mut self) -> Option<Vec<Location>> {
        self.closed.pop()
    }

    /// Puts the tab `index` in front, where there is one.
    pub fn select(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    /// Puts the tab `step` places on in front, going round at the ends.
    pub fn step(&mut self, step: isize) {
        let count = self.tabs.len() as isize;
        self.active = (self.active as isize + step).rem_euclid(count) as usize;
    }

    /// Moves the tab `from` to the place `to`, the one in front staying in
    /// front.
    pub fn reorder(&mut self, from: usize, to: usize) {
        let count = self.tabs.len();
        if from >= count || to >= count || from == to {
            return;
        }
        let front = self.active;
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = match front {
            front if front == from => to,
            front if from < front && front <= to => front - 1,
            front if to <= front && front < from => front + 1,
            front => front,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(names: &[&'static str]) -> Tabs<&'static str> {
        let mut tabs = Tabs::new(Tab::single(names[0]));
        for &name in &names[1..] {
            tabs.open(Tab::single(name), false);
        }
        tabs
    }

    fn order(tabs: &Tabs<&'static str>) -> Vec<&'static str> {
        tabs.all().iter().map(|tab| *tab.active()).collect()
    }

    #[test]
    fn a_tab_opens_after_the_one_in_front() {
        let mut tabs = tabs(&["a", "b"]);
        tabs.select(0);
        assert_eq!(tabs.open(Tab::single("c"), false), 1);
        assert_eq!(order(&tabs), ["a", "c", "b"]);
        assert_eq!(tabs.active_index(), 1);
        tabs.open(Tab::single("d"), true);
        assert_eq!(order(&tabs), ["a", "c", "d", "b"]);
        assert_eq!(tabs.active_index(), 1);
    }

    #[test]
    fn closing_keeps_a_tab_in_front_and_never_the_last() {
        let mut tabs = tabs(&["a", "b", "c"]);
        tabs.select(1);
        assert_eq!(tabs.close(1).map(|tab| *tab.active()), Some("b"));
        assert_eq!(*tabs.active().active(), "c");
        tabs.close(1);
        assert_eq!(*tabs.active().active(), "a");
        assert!(tabs.close(0).is_none());
        let mut tabs = super::tests::tabs(&["a", "b", "c"]);
        tabs.close(0);
        assert_eq!(*tabs.active().active(), "c");
    }

    #[test]
    fn closed_tabs_open_again_the_last_first() {
        let mut tabs = tabs(&["a"]);
        let folder = |name: &str| vec![Location::Folder(name.into())];
        for number in 0..12 {
            tabs.remember(folder(&number.to_string()));
        }
        tabs.remember(Vec::new());
        assert_eq!(tabs.reopen(), Some(folder("11")));
        assert_eq!(tabs.reopen(), Some(folder("10")));
        let mut left = 0;
        while tabs.reopen().is_some() {
            left += 1;
        }
        assert_eq!(left, CLOSED_KEPT - 2);
    }

    #[test]
    fn stepping_goes_round() {
        let mut tabs = tabs(&["a", "b", "c"]);
        tabs.step(1);
        assert_eq!(tabs.active_index(), 0);
        tabs.step(-1);
        assert_eq!(tabs.active_index(), 2);
    }

    #[test]
    fn reordering_keeps_the_tab_in_front() {
        let mut tabs = tabs(&["a", "b", "c", "d"]);
        tabs.select(1);
        tabs.reorder(0, 3);
        assert_eq!(order(&tabs), ["b", "c", "d", "a"]);
        assert_eq!(*tabs.active().active(), "b");
        tabs.reorder(0, 2);
        assert_eq!(order(&tabs), ["c", "d", "b", "a"]);
        assert_eq!(*tabs.active().active(), "b");
        tabs.reorder(3, 0);
        assert_eq!(order(&tabs), ["a", "c", "d", "b"]);
        assert_eq!(*tabs.active().active(), "b");
    }

    #[test]
    fn a_split_tab_has_two_panes_and_one_in_front() {
        let mut tab = Tab::single("left");
        assert_eq!(tab.other(), None);
        assert!(tab.split_with("right").is_ok());
        assert_eq!(tab.split_with("third"), Err("third"));
        assert_eq!((*tab.active(), tab.other()), ("right", Some(&"left")));
        assert!(tab.switch());
        assert_eq!(*tab.active(), "left");
        tab.set_split(0.95);
        assert_eq!(tab.split(), 1. - LEAST_SPLIT);
        tab.swap();
        assert_eq!(tab.panes(), ["right", "left"]);
        assert_eq!(*tab.active(), "left");
        assert_eq!(tab.unsplit(), Some("right"));
        assert_eq!(tab.panes(), ["left"]);
        assert!(!tab.switch());
    }
}
