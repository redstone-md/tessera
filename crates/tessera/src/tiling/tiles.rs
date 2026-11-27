// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The tiles of one workspace of one display: which windows are tiled
//! there, in which order, in what layout and with what sizes, and which
//! of them take turns in one tile as a stack.
//!
//! A tile holds one window, or a stack of windows the user made: the
//! layout sees one window for it, the one of the stack listed among the
//! windows, and every window of the stack is placed in the tile, below
//! its bar of tabs. A layout file may also make stacks of tiles that
//! share their room. What is said here only reckons; it neither reads nor
//! moves windows.

use super::Toward;
use super::command::Reserve;
use super::config::Bars;
use super::declarative::Declared;
use super::layouts::{Generated, Growth, LayoutChoice, TileLayout};
use super::state::{Kept, WindowKey};
use super::tree::{Axis, Node, STILL, Side, Stack};
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

/// Windows that share one room: a tile of one window, a stack the user
/// made, or a stack of a layout file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// The whole room, with the bar of tabs at its top.
    pub room: ScreenRect,
    /// How much of the top of the room the bar of tabs takes; nothing
    /// where there is no bar.
    pub bar: i32,
    /// The windows, in the order of their tabs.
    pub windows: Vec<AppWindowId>,
    /// The one in front of the others.
    pub active: AppWindowId,
}

impl Group {
    /// Where the windows go: the room below the bar.
    pub fn tile(&self) -> ScreenRect {
        let top = (self.room.top + self.bar).min(self.room.bottom - 1);
        ScreenRect { top, ..self.room }
    }

    /// Where the bar is, if there is one.
    pub fn bar_rect(&self) -> Option<ScreenRect> {
        (self.bar > 0).then(|| ScreenRect {
            bottom: self.room.top + self.bar,
            ..self.room
        })
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
    /// The window of each tile, in their order: one of a stack stands for
    /// all of it.
    windows: Vec<AppWindowId>,
    /// The stacks the user made, each with exactly one of its windows in
    /// `windows`.
    stacks: Vec<Stack<AppWindowId>>,
    /// The windows of the stacks of a layout file that were brought in
    /// front, the latest first.
    raised: Vec<AppWindowId>,
    /// The last tree there was of a layout, kept for when windows come
    /// again: from an earlier run, or from before the windows went.
    kept: Option<(LayoutChoice, Node)>,
    /// The stacks kept from an earlier run that were not made again yet.
    pub kept_stacks: Vec<Vec<WindowKey>>,
    /// Where the next window that comes goes.
    pub reserve: Option<Reserve>,
    /// The bars of tabs over the stacks.
    pub bars: Bars,
    /// Windows that, when they come, join the stack of the window after
    /// them instead of taking a tile of their own.
    pub joins: Vec<(AppWindowId, AppWindowId)>,
}

/// The most windows of the stacks of layout files remembered as brought
/// in front.
const RAISED_KEPT: usize = 64;

impl Tiles {
    /// Tiles that start from what was kept of them.
    pub fn from_kept(kept: Kept) -> Self {
        Self {
            chosen: kept.chosen,
            kept: kept.tree,
            kept_stacks: kept.stacks,
            ..Self::default()
        }
    }

    /// What is to be kept of the tiles; `identify` tells each stacked
    /// window by what it is known again after a restart.
    pub fn kept(&self, identify: impl Fn(AppWindowId) -> Option<WindowKey>) -> Kept {
        let now = { self.laid.clone() }.zip(self.tree.clone());
        let mut stacks: Vec<Vec<WindowKey>> = { self.stacks.iter() }
            .map(|stack| {
                stack
                    .windows()
                    .iter()
                    .filter_map(|&id| identify(id))
                    .collect()
            })
            .filter(|keys: &Vec<WindowKey>| keys.len() >= 2)
            .collect();
        stacks.extend(self.kept_stacks.iter().cloned());
        Kept {
            chosen: self.chosen.clone(),
            tree: now.or_else(|| self.kept.clone()),
            stacks,
        }
    }

    /// The window of each tile, in their order.
    pub fn windows(&self) -> &[AppWindowId] {
        &self.windows
    }

    /// Every window tiled here, those of stacks included, in the order of
    /// the tiles and of their tabs.
    pub fn all(&self) -> Vec<AppWindowId> {
        { self.windows.iter() }
            .flat_map(|&id| self.members(id))
            .collect()
    }

    pub fn holds(&self, id: AppWindowId) -> bool {
        self.holder_of(id).is_some()
    }

    /// The stacks the user made.
    pub fn stacks(&self) -> &[Stack<AppWindowId>] {
        &self.stacks
    }

    /// Whether `id` is a window of a stack the user made.
    fn stacked(&self, id: AppWindowId) -> bool {
        self.stacks.iter().any(|stack| stack.contains(id))
    }

    /// The window that holds the tile of `id`: itself, or the window of
    /// its stack listed among the windows.
    fn holder_of(&self, id: AppWindowId) -> Option<AppWindowId> {
        if self.windows.contains(&id) {
            return Some(id);
        }
        let stack = self.stacks.iter().find(|stack| stack.contains(id))?;
        { stack.windows().iter().copied() }.find(|window| self.windows.contains(window))
    }

    /// The windows of the tile of `holder`.
    fn members(&self, holder: AppWindowId) -> Vec<AppWindowId> {
        match self.stacks.iter().find(|stack| stack.contains(holder)) {
            Some(stack) => stack.windows().to_vec(),
            None => vec![holder],
        }
    }

    /// The place of the tile of `id` in the order of the tiles.
    pub fn slot_of(&self, id: AppWindowId) -> Option<usize> {
        let holder = self.holder_of(id)?;
        self.windows.iter().position(|&listed| listed == holder)
    }

    fn index_of(&self, id: AppWindowId) -> Option<usize> {
        self.slot_of(id)
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
    /// The stacks are let go of with them.
    pub fn clear(&mut self) {
        self.remember();
        self.windows.clear();
        self.stacks.clear();
        self.raised.clear();
        self.joins.clear();
        self.reserve = None;
    }

    /// Takes `id` out of the stack it is in, if it is in one: the tile
    /// stays, held by the window then in front of the stack, and a stack
    /// left with one window is a tile of that window. Whether it was in a
    /// stack is returned.
    fn leave_stack(&mut self, id: AppWindowId) -> bool {
        let Some(at) = self.stacks.iter().position(|stack| stack.contains(id)) else {
            return false;
        };
        let stack = &mut self.stacks[at];
        stack.remove(id);
        let next = (!stack.is_empty()).then(|| stack.active());
        let lone = stack.lone().is_some() || stack.is_empty();
        if let Some(place) = self.windows.iter().position(|&listed| listed == id) {
            match next {
                Some(next) => self.windows[place] = next,
                None => {
                    self.windows.remove(place);
                }
            }
        }
        if lone {
            self.stacks.remove(at);
        }
        true
    }

    /// Has `id`, which holds no tile here, join the stack of the tile of
    /// `target` in front, or make a stack with the window of that tile.
    /// Whether there was such a tile is returned.
    fn stack_in(&mut self, id: AppWindowId, target: AppWindowId) -> bool {
        let Some(holder) = self.holder_of(target) else {
            return false;
        };
        if self.holder_of(id).is_some() {
            return false;
        }
        match self.stacks.iter_mut().find(|stack| stack.contains(holder)) {
            Some(stack) => stack.join(id, true),
            None => self.stacks.extend(Stack::new(vec![holder, id], 1)),
        }
        true
    }

    /// Takes in that the windows `present`, in the order they were found,
    /// are the ones to tile here in `layout`, in `area` with tiles `gap`
    /// apart: those that went leave their tiles and those that came get
    /// one, the window in front being `front`; a window that came and is
    /// named in `joins`, or that a reserved stack waits for, joins a
    /// stack instead. The windows the place had reserved to float are
    /// returned; they are not tiled.
    pub fn fit(
        &mut self,
        present: &[AppWindowId],
        layout: Layout,
        front: Option<AppWindowId>,
        area: ScreenRect,
        gap: i32,
    ) -> Vec<AppWindowId> {
        let choice = layout.choice();
        // The windows of stacks that went leave them, and a tile whose
        // window went is held by another window of its stack.
        let gone: Vec<AppWindowId> = { self.stacks.iter() }
            .flat_map(|stack| stack.windows().iter().copied())
            .filter(|id| !present.contains(id))
            .collect();
        for id in gone {
            self.leave_stack(id);
        }
        self.raised.retain(|id| present.contains(id));
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
            .filter(|id| !self.windows.contains(id) && !fresh.contains(id) && !self.stacked(*id))
            .collect();
        let mut floated = Vec::new();
        if self.reserve == Some(Reserve::Float) && !came.is_empty() {
            self.reserve = None;
            floated.push(came.remove(0));
        }
        // The windows that join a stack: one a reserved stack waits for,
        // the first to come, and those `joins` names.
        let mut joining = Vec::new();
        if self.reserve == Some(Reserve::Stack) && !came.is_empty() {
            self.reserve = None;
            let target = front
                .filter(|&front| front != came[0])
                .or_else(|| self.windows.last().copied())
                .or_else(|| fresh.last().copied());
            if let Some(target) = target {
                joining.push((came.remove(0), target));
            }
        }
        let joins = std::mem::take(&mut self.joins);
        came.retain(|&id| match joins.iter().find(|(new, _)| *new == id) {
            Some(&(_, target)) if present.contains(&target) && target != id => {
                joining.push((id, target));
                false
            }
            _ => true,
        });
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
        for (id, target) in joining {
            // A window whose stack is not here after all takes a tile.
            if !self.stack_in(id, target) {
                self.add(id, None, front, layout, area, gap);
            }
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

    /// The windows that share a room, each tile or stack once, in the
    /// order of the leaves.
    pub fn groups(&self, area: ScreenRect, gap: i32) -> Vec<Group> {
        let Some(tree) = &self.tree else {
            return Vec::new();
        };
        let places = tree.place(area, gap);
        let stacks = tree.stacks();
        let mut groups: Vec<Group> = Vec::new();
        // The groups of the stacks of a layout, by their number.
        let mut declared: Vec<(usize, usize)> = Vec::new();
        for (leaf, (&window, room)) in self.at.iter().zip(places).enumerate() {
            let Some(&holder) = self.windows.get(window) else {
                continue;
            };
            let members = self.members(holder);
            let stack = stacks.get(leaf).copied().flatten();
            if let Some(number) = stack
                && let Some(&(_, at)) = declared.iter().find(|(known, _)| *known == number)
            {
                groups[at].windows.extend(members);
                continue;
            }
            if let Some(number) = stack {
                declared.push((number, groups.len()));
            }
            let active = { self.stacks.iter() }
                .find(|stack| stack.contains(holder))
                .map_or(holder, Stack::active);
            groups.push(Group {
                room,
                bar: 0,
                windows: members,
                active,
            });
        }
        for &(_, at) in &declared {
            let group = &mut groups[at];
            let raised = { self.raised.iter() }.find(|id| group.windows.contains(id));
            group.active = raised.copied().unwrap_or(group.windows[0]);
        }
        for group in &mut groups {
            let height = group.room.bottom - group.room.top;
            group.bar = self.bars.over(group.windows.len()).min(height / 2);
        }
        groups
    }

    /// The group `id` is a window of.
    pub fn group_of(&self, id: AppWindowId, area: ScreenRect, gap: i32) -> Option<Group> {
        { self.groups(area, gap).into_iter() }.find(|group| group.windows.contains(&id))
    }

    /// The tile of each window, those of stacks included.
    pub fn places(&self, area: ScreenRect, gap: i32) -> Vec<(AppWindowId, ScreenRect)> {
        { self.groups(area, gap).into_iter() }
            .flat_map(|group| {
                let tile = group.tile();
                group.windows.into_iter().map(move |id| (id, tile))
            })
            .collect()
    }

    /// The tile of the window `id`.
    pub fn tile_of(&self, id: AppWindowId, area: ScreenRect, gap: i32) -> Option<ScreenRect> {
        Some(self.group_of(id, area, gap)?.tile())
    }

    /// Takes the window `id` out of the tiles, and says where it was in
    /// the order. A window of a stack leaves the stack, which keeps its
    /// tile.
    pub fn take(&mut self, id: AppWindowId, layout: Layout) -> Option<usize> {
        let index = self.index_of(id)?;
        if self.leave_stack(id) {
            return Some(index);
        }
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

    /// Moves the window `id` out of its tile into the stack of the tile of
    /// `target`, in front of it. Whether it moved is returned; a window
    /// already in that tile stays.
    pub fn stack_with(&mut self, id: AppWindowId, target: AppWindowId, layout: Layout) -> bool {
        match (self.holder_of(id), self.holder_of(target)) {
            (Some(own), Some(other)) if own != other => {}
            _ => return false,
        }
        self.take(id, layout);
        self.stack_in(id, target)
    }

    /// Has `id`, a window that is tiled nowhere yet, join the stack of the
    /// tile of `target` in front. Whether there is such a tile is
    /// returned.
    pub fn join(&mut self, id: AppWindowId, target: AppWindowId) -> bool {
        self.stack_in(id, target)
    }

    /// Takes the window `id` out of the stack the user made it a window
    /// of, into a tile of its own after the stack's. Whether it was in
    /// such a stack is returned.
    pub fn unstack(&mut self, id: AppWindowId, layout: Layout, area: ScreenRect, gap: i32) -> bool {
        let Some(index) = self.index_of(id).filter(|_| self.stacked(id)) else {
            return false;
        };
        self.leave_stack(id);
        if layout.growth().is_none() {
            self.windows.insert((index + 1).min(self.windows.len()), id);
            return true;
        }
        let leaf = self.at.iter().position(|&window| window == index);
        let (Some(tree), Some(leaf)) = (self.tree.take(), leaf) else {
            self.windows.push(id);
            self.at = (0..self.windows.len()).collect();
            return true;
        };
        let place = tree.place(area, gap).get(leaf).copied().unwrap_or(area);
        let (tree, leaf) = tree.split(leaf, Axis::longer(place), false);
        self.windows.insert(leaf.min(self.windows.len()), id);
        self.at = (0..self.windows.len()).collect();
        self.tree = Some(tree);
        true
    }

    /// Brings the window after the one in front of the stack of `id` in
    /// front, or the one before it with `back`; the window in front then
    /// is returned. A window of no stack has no other.
    pub fn cycle(
        &mut self,
        id: AppWindowId,
        back: bool,
        area: ScreenRect,
        gap: i32,
    ) -> Option<AppWindowId> {
        if let Some(stack) = self.stacks.iter_mut().find(|stack| stack.contains(id)) {
            return Some(stack.cycle(back));
        }
        let group = self.group_of(id, area, gap)?;
        let mut stack = Stack::new(group.windows.clone(), 0)?;
        stack.activate(group.active);
        let next = stack.cycle(back);
        self.raise(next);
        Some(next)
    }

    /// Takes in that `id` came in front, as the user brought it there:
    /// it is the window in front of its stack now. Whether that changed
    /// which window of a stack is in front is returned.
    pub fn activate(&mut self, id: AppWindowId, area: ScreenRect, gap: i32) -> bool {
        if let Some(stack) = self.stacks.iter_mut().find(|stack| stack.contains(id)) {
            return stack.activate(id);
        }
        match self.group_of(id, area, gap) {
            Some(group) if group.windows.len() > 1 && group.active != id => {
                self.raise(id);
                true
            }
            _ => false,
        }
    }

    /// Remembers `id` as the window brought in front last of its stack of
    /// a layout file.
    fn raise(&mut self, id: AppWindowId) {
        self.raised.retain(|&other| other != id);
        self.raised.insert(0, id);
        self.raised.truncate(RAISED_KEPT);
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
        let (Some(leaf), Some(group)) = (self.leaf_of(id), self.group_of(id, area, gap)) else {
            return false;
        };
        let tile = group.tile();
        if same_size(tile, frame) {
            return false;
        }
        let Some(tree) = &mut self.tree else {
            return false;
        };
        let mut changed = false;
        for (side, to) in Side::moved(tile, frame) {
            // The top of the room lies above the bar of tabs.
            let to = if side == Side::Top {
                to - group.bar
            } else {
                to
            };
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
    use crate::tiling::config::StackBarShown;

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
        let kept = before.kept(|_| None);
        let mut after = Tiles::from_kept(kept.clone());
        after.fit(&ids(&[7, 8, 9]), BSP, None, AREA, 0);
        assert_eq!(after.kept(|_| None), kept);
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

    #[test]
    fn a_window_joins_the_stack_of_another_and_leaves_it() {
        let mut tiles = Tiles::default();
        tiles.fit(&ids(&[1, 2, 3]), MASTER, None, AREA, 0);
        assert!(tiles.stack_with(id(3), id(1), MASTER));
        // A window already in that tile stays.
        assert!(!tiles.stack_with(id(3), id(1), MASTER));
        tiles.fit(&ids(&[1, 2, 3]), MASTER, None, AREA, 0);
        assert_eq!(tiles.windows(), ids(&[1, 2]));
        assert_eq!(tiles.all(), ids(&[1, 3, 2]));
        assert_eq!(tile(&tiles, 1), rect(0, 0, 500, 600));
        assert_eq!(tile(&tiles, 3), tile(&tiles, 1));
        let group = tiles.group_of(id(1), AREA, 0).unwrap();
        assert_eq!((group.active, group.bar), (id(3), 0));
        assert_eq!(tiles.cycle(id(1), false, AREA, 0), Some(id(1)));
        assert!(tiles.activate(id(3), AREA, 0));
        assert!(!tiles.activate(id(3), AREA, 0));
        assert_eq!(tiles.cycle(id(2), false, AREA, 0), None);
        // The window that held the tile closes: the other keeps it.
        tiles.fit(&ids(&[2, 3]), MASTER, None, AREA, 0);
        assert_eq!(tiles.windows(), ids(&[3, 2]));
        assert!(tiles.stacks().is_empty());
        assert_eq!(tile(&tiles, 3), rect(0, 0, 500, 600));
    }

    #[test]
    fn the_bar_of_a_stack_takes_the_top_of_its_tile() {
        let mut tiles = Tiles {
            bars: Bars {
                shown: StackBarShown::AsNeeded,
                height: 30,
            },
            ..Tiles::default()
        };
        tiles.fit(&ids(&[1, 2]), MASTER, None, AREA, 0);
        assert_eq!(tile(&tiles, 2), rect(500, 0, 1000, 600));
        // A window that comes named among the joins joins a stack.
        tiles.joins = vec![(id(3), id(2))];
        tiles.fit(&ids(&[1, 2, 3]), MASTER, None, AREA, 0);
        assert_eq!(tiles.windows(), ids(&[1, 2]));
        assert_eq!(tile(&tiles, 3), rect(500, 30, 1000, 600));
        let group = tiles.group_of(id(2), AREA, 0).unwrap();
        assert_eq!(group.bar_rect(), Some(rect(500, 0, 1000, 30)));
        assert_eq!(group.active, id(3));
        assert_eq!(tiles.places(AREA, 0).len(), 3);
        // Out of the stack, into a tile of its own after it.
        assert!(tiles.unstack(id(3), MASTER, AREA, 0));
        tiles.fit(&ids(&[1, 2, 3]), MASTER, None, AREA, 0);
        assert_eq!(tiles.windows(), ids(&[1, 2, 3]));
        assert_eq!(tile(&tiles, 2).top, 0);
        assert!(!tiles.unstack(id(3), MASTER, AREA, 0));
        // Over every tile, the bar is there for one window too.
        tiles.bars.shown = StackBarShown::Always;
        assert_eq!(tile(&tiles, 1).top, 30);
    }

    #[test]
    fn a_reserved_stack_takes_the_next_window_into_the_tile_in_front() {
        let mut tiles = Tiles::default();
        tiles.fit(&ids(&[1, 2]), BSP, None, AREA, 0);
        tiles.reserve = Some(Reserve::Stack);
        tiles.fit(&ids(&[1, 2, 3]), BSP, Some(id(1)), AREA, 0);
        assert_eq!(tiles.reserve, None);
        assert_eq!(tiles.windows(), ids(&[1, 2]));
        assert_eq!(tile(&tiles, 3), rect(0, 0, 500, 600));
        // In a lasting tree the window unstacked divides the stack's tile.
        assert!(tiles.unstack(id(3), BSP, AREA, 0));
        assert_eq!(tile(&tiles, 1), rect(0, 0, 500, 300));
        assert_eq!(tile(&tiles, 3), rect(0, 300, 500, 600));
        // A window dragged into the stack of another leaves its tile.
        assert!(tiles.stack_with(id(3), id(2), BSP));
        assert_eq!(tile(&tiles, 1), rect(0, 0, 500, 600));
        assert_eq!(tile(&tiles, 3), rect(500, 0, 1000, 600));
        // Taking the window of a stack out leaves the stack its tile.
        assert_eq!(tiles.take(id(2), BSP), Some(1));
        assert_eq!(tiles.windows(), ids(&[1, 3]));
        assert_eq!(tile(&tiles, 3), rect(500, 0, 1000, 600));
    }

    #[test]
    fn stacks_are_kept_by_what_their_windows_are() {
        let mut tiles = Tiles::default();
        tiles.fit(&ids(&[1, 2]), MASTER, None, AREA, 0);
        tiles.stack_with(id(2), id(1), MASTER);
        let key = |raw: isize| WindowKey {
            exe: format!("{raw}.exe"),
            ..WindowKey::default()
        };
        let kept = tiles.kept(|id| Some(key(id.raw())));
        assert_eq!(kept.stacks, [vec![key(1), key(2)]]);
        let again = Tiles::from_kept(kept.clone());
        assert_eq!(again.kept_stacks, kept.stacks);
        assert_eq!(again.kept(|_| None).stacks, kept.stacks);
    }
}
