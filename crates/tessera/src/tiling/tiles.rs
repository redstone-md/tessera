// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The tiles of one workspace of one display: which windows are tiled
//! there, in which order, in what layout and with what sizes.
//!
//! What is said here only reckons; it neither reads nor moves windows.

use super::Toward;
use super::command::Reserve;
use super::declarative::Declared;
use super::layouts::{Generated, Growth, LayoutChoice, TileLayout};
use super::state::Kept;
use super::tree::{Axis, Node, STILL, Side};
use crate::platform::{AppWindowId, ScreenRect};

/// A layout as it is used: one of Tessera's own, or one of a file.
#[derive(Clone, Copy, Debug)]
pub enum Layout<'a> {
    Builtin(TileLayout),
    Declared(&'a Declared),
}

impl Layout<'_> {
    /// What the layout is chosen as.
    pub fn choice(&self) -> LayoutChoice {
        match self {
            Self::Builtin(layout) => LayoutChoice::Builtin(*layout),
            Self::Declared(layout) => LayoutChoice::File(layout.name.clone()),
        }
    }

    fn growth(&self) -> Option<Growth> {
        match self {
            Self::Builtin(layout) => layout.growth(),
            Self::Declared(_) => None,
        }
    }

    fn generate(&self, count: usize, area: ScreenRect) -> Generated {
        match self {
            Self::Builtin(layout) => layout.generate(count, area),
            Self::Declared(layout) => layout.generate(count),
        }
    }
}

impl Toward {
    /// The direction a window beside another on this side divides its
    /// tile in, and whether it comes before the other.
    fn division(self) -> (Axis, bool) {
        match self {
            Self::Left => (Axis::Horizontal, true),
            Self::Right => (Axis::Horizontal, false),
            Self::Up => (Axis::Vertical, true),
            Self::Down => (Axis::Vertical, false),
        }
    }
}

/// The tiles of one place.
#[derive(Debug, Default)]
pub struct Tiles {
    /// The layout chosen for the place while Tessera ran; `None` for the
    /// one the configuration gives.
    pub chosen: Option<LayoutChoice>,
    /// The layout the tree is of.
    laid: Option<LayoutChoice>,
    tree: Option<Node>,
    /// The window of each leaf, by its place in `windows`.
    at: Vec<usize>,
    /// The tiled windows, in their order.
    windows: Vec<AppWindowId>,
    /// The last tree there was of a layout, kept for when windows come
    /// again: from an earlier run, or from before the windows went.
    kept: Option<(LayoutChoice, Node)>,
    /// Where the next window that comes goes.
    pub reserve: Option<Reserve>,
}

impl Tiles {
    /// Tiles that start from what was kept of them.
    pub fn from_kept(kept: Kept) -> Self {
        Self {
            chosen: kept.chosen,
            kept: kept.tree,
            ..Self::default()
        }
    }

    /// What is to be kept of the tiles.
    pub fn kept(&self) -> Kept {
        let now = { self.laid.clone() }.zip(self.tree.clone());
        Kept {
            chosen: self.chosen.clone(),
            tree: now.or_else(|| self.kept.clone()),
        }
    }

    pub fn windows(&self) -> &[AppWindowId] {
        &self.windows
    }

    pub fn holds(&self, id: AppWindowId) -> bool {
        self.windows.contains(&id)
    }

    fn index_of(&self, id: AppWindowId) -> Option<usize> {
        self.windows.iter().position(|&listed| listed == id)
    }

    /// The leaf of the window `id`.
    fn leaf_of(&self, id: AppWindowId) -> Option<usize> {
        let index = self.index_of(id)?;
        self.at.iter().position(|&window| window == index)
    }

    /// Keeps the tree there is for when windows come again.
    fn remember(&mut self) {
        if let (Some(laid), Some(tree)) = (self.laid.clone(), self.tree.take()) {
            self.kept = Some((laid, tree));
        }
        self.at.clear();
    }

    /// The tree kept of `layout`.
    fn kept_of(&self, layout: &LayoutChoice) -> Option<&Node> {
        { self.kept.as_ref() }
            .filter(|(laid, _)| laid == layout)
            .map(|(_, tree)| tree)
    }

    /// Lets go of every window, keeping the tree for when they come back.
    pub fn clear(&mut self) {
        self.remember();
        self.windows.clear();
        self.reserve = None;
    }

    /// Takes in that the windows `present`, in the order they were found,
    /// are the ones to tile here in `layout`, in `area` with tiles `gap`
    /// apart: those that went leave their tiles and those that came get
    /// one, the window in front being `front`. The windows the place had
    /// reserved to float are returned; they are not tiled.
    pub fn fit(
        &mut self,
        present: &[AppWindowId],
        layout: Layout,
        front: Option<AppWindowId>,
        area: ScreenRect,
        gap: i32,
    ) -> Vec<AppWindowId> {
        let choice = layout.choice();
        // With another layout the windows come again, in their order.
        let mut fresh = Vec::new();
        if self.laid.as_ref() != Some(&choice) {
            self.remember();
            self.laid = Some(choice.clone());
            fresh = std::mem::take(&mut self.windows);
            fresh.retain(|id| present.contains(id));
        }
        let growth = layout.growth();
        let mut index = self.windows.len();
        while index > 0 {
            index -= 1;
            if !present.contains(&self.windows[index]) {
                self.remove_at(index, growth.is_some());
            }
        }
        // The windows new to the place, apart from those that only come
        // again for another layout: a reserve is for a window that opens.
        let mut came: Vec<AppWindowId> = { present.iter().copied() }
            .filter(|id| !self.windows.contains(id) && !fresh.contains(id))
            .collect();
        let mut floated = Vec::new();
        if self.reserve == Some(Reserve::Float) && !came.is_empty() {
            self.reserve = None;
            floated.push(came.remove(0));
        }
        let first_came = came.first().copied();
        fresh.extend(came);
        // Windows that come to an empty place of a lasting tree take the
        // tree that was kept, as far as they fill it.
        if growth.is_some()
            && self.windows.is_empty()
            && !fresh.is_empty()
            && let Some(mut tree) = self.kept_of(&choice).cloned()
        {
            while tree.count() > fresh.len() {
                match tree.clone().without(tree.count() - 1) {
                    Some(less) => tree = less,
                    None => break,
                }
            }
            let count = tree.count().min(fresh.len());
            self.windows = fresh.drain(..count).collect();
            self.at = (0..count).collect();
            self.tree = Some(tree);
        }
        for id in fresh {
            let reserve = { Some(id) == first_came }
                .then(|| self.reserve.take())
                .flatten();
            self.add(id, reserve, front, layout, area, gap);
        }
        if growth.is_none() {
            self.generate(layout, area, true);
        }
        floated
    }

    /// Takes the window at `index` out, and its leaf where the tree lasts.
    fn remove_at(&mut self, index: usize, lasting: bool) {
        self.windows.remove(index);
        if lasting {
            self.tree = self.tree.take().and_then(|tree| tree.without(index));
            self.at = (0..self.windows.len()).collect();
        }
    }

    /// Makes the tree of the windows again, taking over the weights of the
    /// tree there was where `adopt` says so.
    fn generate(&mut self, layout: Layout, area: ScreenRect, adopt: bool) {
        let made = layout.generate(self.windows.len(), area);
        let mut tree = made.tree;
        let choice = layout.choice();
        let old = self.tree.as_ref().or_else(|| self.kept_of(&choice));
        if let (Some(tree), Some(old), true) = (&mut tree, old, adopt) {
            tree.adopt(old);
        }
        if tree.is_none() {
            self.remember();
        }
        self.tree = tree;
        self.at = made.at;
    }

    /// Gives the window `id` a tile, where `reserve` says or else where the
    /// layout puts a new window.
    fn add(
        &mut self,
        id: AppWindowId,
        reserve: Option<Reserve>,
        front: Option<AppWindowId>,
        layout: Layout,
        area: ScreenRect,
        gap: i32,
    ) {
        let front = front.and_then(|front| self.index_of(front));
        let Some(growth) = layout.growth() else {
            let index = match (reserve, front) {
                (Some(Reserve::Side(side)), Some(front)) => match side.division().1 {
                    true => front,
                    false => front + 1,
                },
                (Some(Reserve::Split(_)), Some(front)) => front + 1,
                _ => self.windows.len(),
            };
            self.windows.insert(index.min(self.windows.len()), id);
            return;
        };
        let Some(tree) = self.tree.take() else {
            self.tree = Some(Node::Leaf);
            self.windows = vec![id];
            self.at = vec![0];
            return;
        };
        let last = self.windows.len().saturating_sub(1);
        let (leaf, axis, before) = match reserve {
            Some(Reserve::Side(side)) => {
                let (axis, before) = side.division();
                (front.unwrap_or(last), axis, before)
            }
            Some(Reserve::Split(axis)) => (front.unwrap_or(last), axis, false),
            _ => {
                let leaf = match growth {
                    Growth::Bsp => front.unwrap_or(last),
                    Growth::Dwindle => last,
                };
                (leaf, growth.axis(&tree, leaf, area, gap), false)
            }
        };
        let (tree, leaf) = tree.split(leaf, axis, before);
        self.windows.insert(leaf.min(self.windows.len()), id);
        self.at = (0..self.windows.len()).collect();
        self.tree = Some(tree);
    }

    /// The tile of each window.
    pub fn places(&self, area: ScreenRect, gap: i32) -> Vec<(AppWindowId, ScreenRect)> {
        let Some(tree) = &self.tree else {
            return Vec::new();
        };
        let places = tree.place(area, gap);
        { self.at.iter().zip(places) }
            .filter_map(|(&window, place)| Some((*self.windows.get(window)?, place)))
            .collect()
    }

    /// The tile of the window `id`.
    pub fn tile_of(&self, id: AppWindowId, area: ScreenRect, gap: i32) -> Option<ScreenRect> {
        let leaf = self.leaf_of(id)?;
        self.tree.as_ref()?.place(area, gap).get(leaf).copied()
    }

    /// Takes the window `id` out of the tiles, and says where it was in
    /// the order.
    pub fn take(&mut self, id: AppWindowId, layout: Layout) -> Option<usize> {
        let index = self.index_of(id)?;
        let lasting = layout.growth().is_some();
        self.remove_at(index, lasting);
        Some(index)
    }

    /// Gives the window `id` the place `index` in the order; in a lasting
    /// tree it divides the tile of the window that had that place, or of
    /// the last one.
    pub fn put(
        &mut self,
        id: AppWindowId,
        index: usize,
        layout: Layout,
        area: ScreenRect,
        gap: i32,
    ) {
        if self.holds(id) {
            return;
        }
        if layout.growth().is_none() {
            self.windows.insert(index.min(self.windows.len()), id);
            return;
        }
        let Some(tree) = self.tree.take() else {
            self.tree = Some(Node::Leaf);
            self.windows = vec![id];
            self.at = vec![0];
            return;
        };
        let (leaf, before) = match index < self.windows.len() {
            true => (index, true),
            false => (self.windows.len() - 1, false),
        };
        let place = tree.place(area, gap).get(leaf).copied().unwrap_or(area);
        let (tree, leaf) = tree.split(leaf, Axis::longer(place), before);
        self.windows.insert(leaf.min(self.windows.len()), id);
        self.at = (0..self.windows.len()).collect();
        self.tree = Some(tree);
    }

    /// Has the windows `first` and `second` trade tiles.
    pub fn swap(&mut self, first: AppWindowId, second: AppWindowId) -> bool {
        match (self.index_of(first), self.index_of(second)) {
            (Some(first), Some(second)) if first != second => {
                self.windows.swap(first, second);
                true
            }
            _ => false,
        }
    }

    /// Moves the window `id` to the place of `onto` in the order, those
    /// from there on moving one place on.
    pub fn sort(&mut self, id: AppWindowId, onto: AppWindowId) -> bool {
        let (Some(from), Some(to)) = (self.index_of(id), self.index_of(onto)) else {
            return false;
        };
        if from == to {
            return false;
        }
        let moved = self.windows.remove(from);
        self.windows.insert(to, moved);
        true
    }

    /// Gives the window `id` `by` more of the room along `axis` of the
    /// nearest room around it that is divided that way.
    pub fn grow(&mut self, id: AppWindowId, axis: Axis, by: f32) -> bool {
        let Some(leaf) = self.leaf_of(id) else {
            return false;
        };
        self.tree
            .as_mut()
            .is_some_and(|tree| tree.grow(leaf, axis, by))
    }

    /// What `wider` and `narrower` do: in `master` the first window gets
    /// `step` more of the width, or less, whichever window is in front; in
    /// `rows` the window `front` gets more of the height; in the others
    /// more of the width. Without a window in front here it is the first.
    pub fn wider(&mut self, layout: Layout, front: Option<AppWindowId>, step: f32) -> bool {
        let first = self.windows.first().copied();
        let Some(front) = front.filter(|&id| self.holds(id)).or(first) else {
            return false;
        };
        match layout {
            Layout::Builtin(TileLayout::Master) => match &mut self.tree {
                Some(tree @ Node::Split { .. }) => tree.grow_child(&[], 0, step),
                _ => false,
            },
            Layout::Builtin(TileLayout::Rows) => self.grow(front, Axis::Vertical, step),
            _ => self.grow(front, Axis::Horizontal, step),
        }
    }

    /// Makes every tile weigh alike again: a lasting tree keeps its shape,
    /// the others are made again as for the first time.
    pub fn reset(&mut self, layout: Layout, area: ScreenRect) {
        self.kept = None;
        match layout.growth() {
            Some(_) => {
                if let Some(tree) = &mut self.tree {
                    tree.equalize();
                }
            }
            None => self.generate(layout, area, false),
        }
    }

    /// Takes in that the window `id` was resized by hand to `frame`: the
    /// sides of its tile that moved move the borders they lie on. Whether
    /// any did is returned; a window whose size did not change was not
    /// resized.
    pub fn resized(
        &mut self,
        id: AppWindowId,
        frame: ScreenRect,
        area: ScreenRect,
        gap: i32,
    ) -> bool {
        let (Some(leaf), Some(tile)) = (self.leaf_of(id), self.tile_of(id, area, gap)) else {
            return false;
        };
        if same_size(tile, frame) {
            return false;
        }
        let Some(tree) = &mut self.tree else {
            return false;
        };
        let mut changed = false;
        for (side, to) in Side::moved(tile, frame) {
            changed |= tree.drag(leaf, area, gap, side, to);
        }
        changed
    }
}

/// Whether `first` and `second` are as wide and as high as each other,
/// near enough.
pub fn same_size(first: ScreenRect, second: ScreenRect) -> bool {
    let width = |rect: ScreenRect| rect.right - rect.left;
    let height = |rect: ScreenRect| rect.bottom - rect.top;
    (width(first) - width(second)).abs() <= STILL && (height(first) - height(second)).abs() <= STILL
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1000,
        bottom: 600,
    };

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    fn ids(raw: &[isize]) -> Vec<AppWindowId> {
        raw.iter().map(|&raw| AppWindowId::from_raw(raw)).collect()
    }

    fn id(raw: isize) -> AppWindowId {
        AppWindowId::from_raw(raw)
    }

    fn tile(tiles: &Tiles, raw: isize) -> ScreenRect {
        tiles.tile_of(id(raw), AREA, 0).unwrap()
    }

    const MASTER: Layout = Layout::Builtin(TileLayout::Master);
    const BSP: Layout = Layout::Builtin(TileLayout::Bsp);

    #[test]
    fn windows_that_come_get_tiles_and_those_that_go_leave_them() {
        let mut tiles = Tiles::default();
        tiles.fit(&ids(&[1, 2, 3]), MASTER, None, AREA, 0);
        assert_eq!(tiles.windows(), ids(&[1, 2, 3]));
        assert_eq!(tile(&tiles, 1), rect(0, 0, 500, 600));
        tiles.fit(&ids(&[3, 1]), MASTER, None, AREA, 0);
        assert_eq!(tiles.windows(), ids(&[1, 3]));
        assert_eq!(tile(&tiles, 3), rect(500, 0, 1000, 600));
        tiles.fit(&[], MASTER, None, AREA, 0);
        assert!(tiles.places(AREA, 0).is_empty());
    }

    #[test]
    fn the_width_given_to_master_stays_when_windows_come_and_go() {
        let mut tiles = Tiles::default();
        tiles.fit(&ids(&[1, 2]), MASTER, Some(id(2)), AREA, 0);
        // With the second window in front, the first still grows.
        assert!(tiles.wider(MASTER, Some(id(2)), 0.1));
        assert_eq!(tile(&tiles, 1), rect(0, 0, 600, 600));
        tiles.fit(&ids(&[1, 2, 3]), MASTER, None, AREA, 0);
        assert_eq!(tile(&tiles, 1), rect(0, 0, 600, 600));
        // When every window went, the width is kept for those to come.
        tiles.fit(&[], MASTER, None, AREA, 0);
        tiles.fit(&ids(&[4, 5]), MASTER, None, AREA, 0);
        assert_eq!(tile(&tiles, 4), rect(0, 0, 600, 600));
        tiles.reset(MASTER, AREA);
        assert_eq!(tile(&tiles, 4), rect(0, 0, 500, 600));
    }

    #[test]
    fn every_layout_resizes_including_grid_and_monocle() {
        let mut grid = Tiles::default();
        let grid_layout = Layout::Builtin(TileLayout::Grid);
        grid.fit(&ids(&[1, 2, 3, 4]), grid_layout, None, AREA, 0);
        assert!(grid.grow(id(1), Axis::Horizontal, 0.1));
        assert_eq!(tile(&grid, 1).right, 600);
        assert!(grid.grow(id(3), Axis::Vertical, 0.1));
        assert_eq!(tile(&grid, 3).top, 240);
        let mut monocle = Tiles::default();
        let monocle_layout = Layout::Builtin(TileLayout::Monocle);
        monocle.fit(&ids(&[1, 2]), monocle_layout, None, AREA, 0);
        assert_eq!(tile(&monocle, 2), AREA);
        assert!(monocle.grow(id(1), Axis::Horizontal, -0.2));
        assert_eq!(tile(&monocle, 2), rect(100, 0, 900, 600));
        assert!(monocle.grow(id(2), Axis::Horizontal, 0.2));
        assert_eq!(tile(&monocle, 1), AREA);
        assert!(!monocle.grow(id(2), Axis::Horizontal, 0.2));
    }

    #[test]
    fn bsp_divides_the_tile_in_front_and_keeps_its_shape() {
        let mut tiles = Tiles::default();
        tiles.fit(&ids(&[1, 2]), BSP, None, AREA, 0);
        assert_eq!(tile(&tiles, 2), rect(500, 0, 1000, 600));
        // The first window is in front: the third divides its tile.
        tiles.fit(&ids(&[1, 2, 3]), BSP, Some(id(1)), AREA, 0);
        assert_eq!(tile(&tiles, 1), rect(0, 0, 500, 300));
        assert_eq!(tile(&tiles, 3), rect(0, 300, 500, 600));
        assert_eq!(tile(&tiles, 2), rect(500, 0, 1000, 600));
        // The window that goes gives its room back to its neighbour.
        tiles.fit(&ids(&[2, 3]), BSP, None, AREA, 0);
        assert_eq!(tile(&tiles, 3), rect(0, 0, 500, 600));
    }

    #[test]
    fn a_reserved_side_takes_the_next_window_only() {
        let mut tiles = Tiles::default();
        tiles.fit(&ids(&[1]), BSP, None, AREA, 0);
        tiles.reserve = Some(Reserve::Side(Toward::Up));
        tiles.fit(&ids(&[1, 2]), BSP, Some(id(1)), AREA, 0);
        assert_eq!(tile(&tiles, 2), rect(0, 0, 1000, 300));
        assert_eq!(tiles.reserve, None);
        tiles.reserve = Some(Reserve::Split(Axis::Horizontal));
        tiles.fit(&ids(&[1, 2, 3]), BSP, Some(id(2)), AREA, 0);
        assert_eq!(tile(&tiles, 3), rect(500, 0, 1000, 300));
        // In a layout that is made again, the side is a place in the order.
        let mut columns = Tiles::default();
        let layout = Layout::Builtin(TileLayout::Columns);
        columns.fit(&ids(&[1, 2]), layout, None, AREA, 0);
        columns.reserve = Some(Reserve::Side(Toward::Left));
        columns.fit(&ids(&[1, 2, 3]), layout, Some(id(2)), AREA, 0);
        assert_eq!(columns.windows(), ids(&[1, 3, 2]));
        columns.reserve = Some(Reserve::Float);
        let floated = columns.fit(&ids(&[1, 2, 3, 4]), layout, None, AREA, 0);
        assert_eq!(floated, ids(&[4]));
        assert_eq!(columns.windows(), ids(&[1, 3, 2]));
    }

    #[test]
    fn a_reserve_waits_for_a_new_window_across_a_change_of_layout() {
        let mut tiles = Tiles::default();
        tiles.fit(&ids(&[1, 2]), MASTER, None, AREA, 0);
        tiles.reserve = Some(Reserve::Float);
        let grid = Layout::Builtin(TileLayout::Grid);
        assert!(tiles.fit(&ids(&[1, 2]), grid, None, AREA, 0).is_empty());
        assert_eq!(tiles.windows(), ids(&[1, 2]));
        assert_eq!(tiles.reserve, Some(Reserve::Float));
        let floated = tiles.fit(&ids(&[1, 2, 3]), MASTER, None, AREA, 0);
        assert_eq!(floated, ids(&[3]));
        assert_eq!(tiles.windows(), ids(&[1, 2]));
        assert_eq!(tiles.reserve, None);
    }

    #[test]
    fn a_dragged_border_resizes_and_a_drop_trades_or_sorts() {
        let mut tiles = Tiles::default();
        let layout = Layout::Builtin(TileLayout::Columns);
        tiles.fit(&ids(&[1, 2, 3]), layout, None, AREA, 0);
        assert!(tiles.resized(id(1), rect(0, 0, 500, 600), AREA, 0));
        assert_eq!(tile(&tiles, 2).left, 500);
        // The same size somewhere else is a move, not a resize.
        assert!(!tiles.resized(id(1), rect(100, 0, 600, 600), AREA, 0));
        assert!(tiles.swap(id(1), id(3)));
        assert_eq!(tiles.windows(), ids(&[3, 2, 1]));
        assert!(tiles.sort(id(1), id(3)));
        assert_eq!(tiles.windows(), ids(&[1, 3, 2]));
        assert!(!tiles.swap(id(1), id(9)));
    }

    #[test]
    fn a_lasting_tree_comes_back_for_the_windows_of_a_new_start() {
        let mut before = Tiles::default();
        before.fit(&ids(&[1, 2, 3]), BSP, Some(id(1)), AREA, 0);
        before.grow(id(2), Axis::Horizontal, 0.1);
        let kept = before.kept();
        let mut after = Tiles::from_kept(kept.clone());
        after.fit(&ids(&[7, 8, 9]), BSP, None, AREA, 0);
        assert_eq!(after.kept(), kept);
        // Fewer windows take what they fill of it.
        let mut fewer = Tiles::from_kept(kept);
        fewer.fit(&ids(&[7, 8]), BSP, None, AREA, 0);
        assert_eq!(fewer.windows().len(), 2);
        assert_eq!(fewer.places(AREA, 0).len(), 2);
    }

    #[test]
    fn a_window_is_taken_out_and_put_in_elsewhere() {
        let mut tiles = Tiles::default();
        tiles.fit(&ids(&[1, 2]), BSP, None, AREA, 0);
        assert_eq!(tiles.take(id(1), BSP), Some(0));
        assert_eq!(tile(&tiles, 2), AREA);
        tiles.put(id(5), 0, BSP, AREA, 0);
        assert_eq!(tiles.windows(), ids(&[5, 2]));
        assert_eq!(tile(&tiles, 5), rect(0, 0, 500, 600));
        let mut rows = Tiles::default();
        let layout = Layout::Builtin(TileLayout::Rows);
        rows.fit(&ids(&[1]), layout, None, AREA, 0);
        rows.put(id(2), 9, layout, AREA, 0);
        rows.fit(&ids(&[1, 2]), layout, None, AREA, 0);
        assert_eq!(tile(&rows, 2), rect(0, 300, 1000, 600));
    }
}
