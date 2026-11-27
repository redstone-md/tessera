// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The shape the tiles of one workspace of one display take: a tree of
//! rooms, each divided along one axis among its parts by their weights,
//! whose leaves hold one window each.
//!
//! Every layout is such a tree. The built-in ones and those of layout
//! files are made again whenever the number of windows changes, taking
//! over the weights the parts had; `bsp` and `dwindle` keep their tree
//! and only split a leaf or take one away. What is said here is pure: it
//! places rectangles and changes weights, and knows nothing of windows.
//!
//! Window stacks, tabbed containers whose windows take turns in one
//! tile, are to come as another kind of node beside these.

use crate::platform::ScreenRect;

/// The share of its room a part keeps at least when weights are changed.
pub const LEAST_SHARE: f32 = 0.05;

/// How far, in pixels, a side of a window may be from the side of its
/// tile and still count as unmoved.
pub const STILL: i32 = 2;

/// The most leaves and the deepest nesting a tree written as text may
/// have.
const MOST_LEAVES: usize = 512;
const DEEPEST: usize = 64;

/// The direction a room is divided in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    /// The parts lie side by side, from the left to the right.
    Horizontal,
    /// The parts lie one above the other, from the top down.
    Vertical,
}

impl Axis {
    /// The other direction.
    pub fn other(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }

    /// Where `rect` begins and ends along this direction.
    fn span(self, rect: ScreenRect) -> (i32, i32) {
        match self {
            Self::Horizontal => (rect.left, rect.right),
            Self::Vertical => (rect.top, rect.bottom),
        }
    }

    /// `rect` with its span along this direction replaced.
    fn cut(self, rect: ScreenRect, (start, end): (i32, i32)) -> ScreenRect {
        match self {
            Self::Horizontal => ScreenRect {
                left: start,
                right: end,
                ..rect
            },
            Self::Vertical => ScreenRect {
                top: start,
                bottom: end,
                ..rect
            },
        }
    }

    /// The direction along which `rect` is longer, across when it is as
    /// high as wide.
    pub fn longer(rect: ScreenRect) -> Self {
        match rect.right - rect.left >= rect.bottom - rect.top {
            true => Self::Horizontal,
            false => Self::Vertical,
        }
    }
}

/// A side of a tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Horizontal,
            Self::Top | Self::Bottom => Axis::Vertical,
        }
    }

    /// Whether the side is the one where its axis begins.
    fn leading(self) -> bool {
        matches!(self, Self::Left | Self::Top)
    }

    /// The sides of `tile` that `frame` moved, each with where it is now,
    /// unless the whole rectangle moved without changing its size.
    pub fn moved(tile: ScreenRect, frame: ScreenRect) -> Vec<(Self, i32)> {
        let off = |from: i32, to: i32| (from - to).abs() > STILL;
        let sides = [
            (Self::Left, tile.left, frame.left),
            (Self::Right, tile.right, frame.right),
            (Self::Top, tile.top, frame.top),
            (Self::Bottom, tile.bottom, frame.bottom),
        ];
        sides
            .into_iter()
            .filter(|&(_, from, to)| off(from, to))
            .map(|(side, _, to)| (side, to))
            .collect()
    }
}

/// A part of a tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// One window.
    Leaf,
    /// Room that holds no window, as beside a centred column. One that
    /// weighs nothing takes no room at all.
    Space,
    /// As many windows as it says, each taking all of the room, one over
    /// the other.
    Layered(usize),
    /// Parts along `axis`, each with its weight: a part takes as much of
    /// the room as its weight is of the weights of all of them.
    Split {
        axis: Axis,
        children: Vec<(Node, f32)>,
    },
}

impl Node {
    /// A room divided along `axis` into `count` leaves that weigh alike.
    pub fn even(axis: Axis, count: usize) -> Self {
        Self::Split {
            axis,
            children: vec![(Self::Leaf, 1.); count],
        }
    }

    /// How many windows the tree holds.
    pub fn count(&self) -> usize {
        match self {
            Self::Leaf => 1,
            Self::Space => 0,
            Self::Layered(count) => *count,
            Self::Split { children, .. } => children.iter().map(|(child, _)| child.count()).sum(),
        }
    }

    /// Whether `other` is a part of the same kind, whose weights mean the
    /// same here.
    fn alike(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Leaf, Self::Leaf) | (Self::Space, Self::Space) => true,
            (Self::Layered(_), Self::Layered(_)) => true,
            (Self::Split { axis, .. }, Self::Split { axis: other, .. }) => axis == other,
            _ => false,
        }
    }

    /// The place of each window in `area`, in the order of the leaves,
    /// parts being `gap` apart.
    pub fn place(&self, area: ScreenRect, gap: i32) -> Vec<ScreenRect> {
        let mut places = Vec::with_capacity(self.count());
        self.walk(
            area,
            gap,
            &mut Vec::new(),
            &mut |_, node, room| match node {
                Self::Leaf => places.push(room),
                Self::Layered(count) => places.extend(std::iter::repeat_n(room, *count)),
                Self::Space | Self::Split { .. } => {}
            },
        );
        places
    }

    /// The room of every part that takes some, with the way to it from the
    /// root: the place of the part among its siblings at each level.
    fn rooms(&self, area: ScreenRect, gap: i32) -> Vec<(Vec<usize>, ScreenRect)> {
        let mut rooms = Vec::new();
        self.walk(area, gap, &mut Vec::new(), &mut |path, _, room| {
            rooms.push((path.to_vec(), room));
        });
        rooms
    }

    /// Calls `visit` with every part that takes room, before its parts.
    fn walk(
        &self,
        area: ScreenRect,
        gap: i32,
        path: &mut Vec<usize>,
        visit: &mut impl FnMut(&[usize], &Self, ScreenRect),
    ) {
        visit(path, self, area);
        if let Self::Split { axis, children } = self {
            let spans = divide(axis.span(area), children, gap);
            for (index, ((child, _), span)) in children.iter().zip(spans).enumerate() {
                if let Some(span) = span {
                    path.push(index);
                    child.walk(axis.cut(area, span), gap, path, visit);
                    path.pop();
                }
            }
        }
    }

    /// The way from the root to each leaf, in the order of the leaves;
    /// the windows of a layered part share its way.
    pub fn leaves(&self) -> Vec<Vec<usize>> {
        let mut leaves = Vec::with_capacity(self.count());
        self.gather(&mut Vec::new(), &mut leaves);
        leaves
    }

    fn gather(&self, path: &mut Vec<usize>, leaves: &mut Vec<Vec<usize>>) {
        match self {
            Self::Leaf => leaves.push(path.clone()),
            Self::Layered(count) => leaves.extend(std::iter::repeat_n(path.clone(), *count)),
            Self::Space => {}
            Self::Split { children, .. } => {
                for (index, (child, _)) in children.iter().enumerate() {
                    path.push(index);
                    child.gather(path, leaves);
                    path.pop();
                }
            }
        }
    }

    fn at(&self, path: &[usize]) -> Option<&Self> {
        match path.split_first() {
            None => Some(self),
            Some((&first, rest)) => match self {
                Self::Split { children, .. } => children.get(first)?.0.at(rest),
                _ => None,
            },
        }
    }

    fn at_mut(&mut self, path: &[usize]) -> Option<&mut Self> {
        match path.split_first() {
            None => Some(self),
            Some((&first, rest)) => match self {
                Self::Split { children, .. } => children.get_mut(first)?.0.at_mut(rest),
                _ => None,
            },
        }
    }

    /// Gives the part at `child` of the room at `room` a share of that
    /// room `by` larger, or smaller when it is negative. The other parts
    /// of the room keep what they weigh against each other. Whether
    /// anything changed is returned.
    pub fn grow_child(&mut self, room: &[usize], child: usize, by: f32) -> bool {
        let Some(Self::Split { children, .. }) = self.at_mut(room) else {
            return false;
        };
        let share = share_of(children, child);
        set_share(children, child, share + by)
    }

    /// Gives the window of the leaf at `leaf` a share `by` larger, or
    /// smaller, of the nearest room around it that is divided along
    /// `axis` among more than one part. Whether anything changed is
    /// returned; a window that has no such room around it stays.
    pub fn grow(&mut self, leaf: usize, axis: Axis, by: f32) -> bool {
        let Some(path) = self.leaves().into_iter().nth(leaf) else {
            return false;
        };
        for depth in (1..=path.len()).rev() {
            let (room, child) = (&path[..depth - 1], path[depth - 1]);
            if let Some(Self::Split {
                axis: along,
                children,
            }) = self.at(room)
                && *along == axis
                && children.len() > 1
            {
                return self.grow_child(room, child, by);
            }
        }
        false
    }

    /// Has the side `side` of the window of the leaf at `leaf` lie at
    /// `to`, by changing the weights of the part around the window that
    /// has the side and of the part on the other side of it, in the room
    /// both share. `area` and `gap` are what the tree was placed with.
    /// Whether anything changed is returned.
    pub fn drag(&mut self, leaf: usize, area: ScreenRect, gap: i32, side: Side, to: i32) -> bool {
        let Some(path) = self.leaves().into_iter().nth(leaf) else {
            return false;
        };
        let rooms = self.rooms(area, gap);
        let room_of = |path: &[usize]| rooms.iter().find(|(at, _)| at == path).map(|(_, r)| *r);
        let axis = side.axis();
        for depth in (1..=path.len()).rev() {
            let (room, child) = (&path[..depth - 1], path[depth - 1]);
            let Some(Self::Split {
                axis: along,
                children,
            }) = self.at(room)
            else {
                continue;
            };
            let other = match side.leading() {
                true => child.checked_sub(1),
                false => Some(child + 1).filter(|&next| next < children.len()),
            };
            let Some(other) = other.filter(|_| *along == axis) else {
                continue;
            };
            let Some(own) = room_of(&path[..depth]) else {
                return false;
            };
            let (start, end) = axis.span(own);
            let (length, wanted) = (
                end - start,
                if side.leading() { end - to } else { to - start },
            );
            if length <= 0 {
                return false;
            }
            let Some(Self::Split { children, .. }) = self.at_mut(room) else {
                return false;
            };
            return shift(children, child, other, wanted as f32 / length as f32);
        }
        false
    }

    /// Makes every part of every room weigh alike.
    pub fn equalize(&mut self) {
        if let Self::Split { children, .. } = self {
            for (child, weight) in children {
                *weight = 1.;
                child.equalize();
            }
        }
    }

    /// Takes over the weights of `old` where it has a part of the same
    /// kind in the same place, as when a layout is made again for
    /// another number of windows.
    pub fn adopt(&mut self, old: &Self) {
        if let (
            Self::Split { axis, children },
            Self::Split {
                axis: old_axis,
                children: old_children,
            },
        ) = (self, old)
            && axis == old_axis
        {
            for ((child, weight), (old_child, old_weight)) in children.iter_mut().zip(old_children)
            {
                if child.alike(old_child) {
                    *weight = *old_weight;
                    child.adopt(old_child);
                }
            }
        }
    }

    /// The tree with the leaf at `leaf` divided along `axis` into two
    /// leaves that weigh alike, the new one first with `before`, and the
    /// place of the new leaf among the leaves.
    pub fn split(mut self, leaf: usize, axis: Axis, before: bool) -> (Self, usize) {
        let Some(path) = self.leaves().into_iter().nth(leaf) else {
            let count = self.count();
            return (self.with_leaf_at_end(axis), count);
        };
        if let Some(node @ Self::Leaf) = self.at_mut(&path) {
            *node = Self::even(axis, 2);
            return (self, if before { leaf } else { leaf + 1 });
        }
        let count = self.count();
        (self.with_leaf_at_end(axis), count)
    }

    /// The tree with one more leaf after all the others, the whole of it
    /// and the leaf dividing the room along `axis`.
    fn with_leaf_at_end(self, axis: Axis) -> Self {
        Self::Split {
            axis,
            children: vec![(self, 1.), (Self::Leaf, 1.)],
        }
    }

    /// The tree without the leaf at `leaf`; a room left with one part
    /// gives its place to that part, and none is left of a tree without
    /// leaves.
    pub fn without(self, leaf: usize) -> Option<Self> {
        let Some(path) = self.leaves().into_iter().nth(leaf) else {
            return Some(self);
        };
        self.cut(&path)
    }

    fn cut(self, path: &[usize]) -> Option<Self> {
        let Some((&first, rest)) = path.split_first() else {
            return match self {
                Self::Layered(count) if count > 1 => Some(Self::Layered(count - 1)),
                _ => None,
            };
        };
        let Self::Split { axis, mut children } = self else {
            return Some(self);
        };
        if first >= children.len() {
            return Some(Self::Split { axis, children });
        }
        let (child, weight) = children.remove(first);
        if let Some(child) = child.cut(rest) {
            children.insert(first, (child, weight));
        }
        let holding = children
            .iter()
            .filter(|(child, _)| child.count() > 0)
            .count();
        match holding {
            0 => None,
            1 if children.len() == 1 => children.pop().map(|(child, _)| child),
            _ => Some(Self::Split { axis, children }),
        }
    }

    /// The tree written as text, as `tiling-state.toml` keeps it: `l` for
    /// a leaf, `s` for space, `m3` for three layered windows and
    /// `h(1:l,2:v(1:l,1:l))` for rooms divided across (`h`) or down (`v`),
    /// each part after its weight.
    pub fn to_text(&self) -> String {
        match self {
            Self::Leaf => "l".to_owned(),
            Self::Space => "s".to_owned(),
            Self::Layered(count) => format!("m{count}"),
            Self::Split { axis, children } => {
                let letter = match axis {
                    Axis::Horizontal => 'h',
                    Axis::Vertical => 'v',
                };
                let parts: Vec<String> = { children.iter() }
                    .map(|(child, weight)| format!("{}:{}", number(*weight), child.to_text()))
                    .collect();
                format!("{letter}({})", parts.join(","))
            }
        }
    }

    /// The tree `text` writes, as [`Node::to_text`] writes it, or why it
    /// is none.
    pub fn from_text(text: &str) -> Result<Self, String> {
        let mut reader = Reader {
            text: text.trim().as_bytes(),
            at: 0,
            leaves: 0,
        };
        let node = reader.node(0)?;
        if reader.at != reader.text.len() {
            return Err(format!("`{text}` goes on after its tree"));
        }
        Ok(node)
    }
}

/// `weight` written briefly: at most four decimals, without trailing
/// zeros.
fn number(weight: f32) -> String {
    let text = format!("{weight:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    match text {
        "" | "-" => "0".to_owned(),
        text => text.to_owned(),
    }
}

/// Reads a tree written by [`Node::to_text`].
struct Reader<'a> {
    text: &'a [u8],
    at: usize,
    leaves: usize,
}

impl Reader<'_> {
    fn node(&mut self, depth: usize) -> Result<Node, String> {
        if depth > DEEPEST {
            return Err("the tree is nested too deeply".to_owned());
        }
        let letter = self.text.get(self.at).copied();
        self.at += 1;
        let node = match letter {
            Some(b'l') => Node::Leaf,
            Some(b's') => Node::Space,
            Some(b'm') => {
                let count = self.digits()?;
                if count == 0 {
                    return Err("layered windows are at least one".to_owned());
                }
                if count > MOST_LEAVES {
                    return Err("the tree has too many leaves".to_owned());
                }
                self.leaves = self.leaves.saturating_add(count);
                Node::Layered(count)
            }
            Some(letter @ (b'h' | b'v')) => {
                let axis = match letter {
                    b'h' => Axis::Horizontal,
                    _ => Axis::Vertical,
                };
                self.expect(b'(')?;
                let mut children = Vec::new();
                loop {
                    let weight = self.weight()?;
                    self.expect(b':')?;
                    children.push((self.node(depth + 1)?, weight));
                    match self.text.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b')') => {
                            self.at += 1;
                            break;
                        }
                        _ => return Err("a room ends with `)`".to_owned()),
                    }
                }
                Node::Split { axis, children }
            }
            _ => return Err(format!("no part begins at {}", self.at - 1)),
        };
        if matches!(node, Node::Leaf) {
            self.leaves += 1;
        }
        if self.leaves > MOST_LEAVES {
            return Err("the tree has too many leaves".to_owned());
        }
        Ok(node)
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        match self.text.get(self.at) {
            Some(&found) if found == byte => {
                self.at += 1;
                Ok(())
            }
            _ => Err(format!("`{}` is missing at {}", byte as char, self.at)),
        }
    }

    fn taken(&mut self, wanted: impl Fn(u8) -> bool) -> &str {
        let from = self.at;
        while self.text.get(self.at).is_some_and(|&byte| wanted(byte)) {
            self.at += 1;
        }
        std::str::from_utf8(&self.text[from..self.at]).unwrap_or_default()
    }

    fn digits(&mut self) -> Result<usize, String> {
        let digits = self.taken(|byte| byte.is_ascii_digit());
        digits
            .parse()
            .map_err(|_| format!("`{digits}` is no count"))
    }

    fn weight(&mut self) -> Result<f32, String> {
        let digits = self.taken(|byte| byte.is_ascii_digit() || byte == b'.');
        match digits.parse::<f32>() {
            Ok(weight) if weight.is_finite() && weight >= 0. => Ok(weight),
            _ => Err(format!("`{digits}` is no weight")),
        }
    }
}

/// Whether a part takes room: space that weighs nothing takes none.
fn takes_room((child, weight): &(Node, f32)) -> bool {
    !(matches!(child, Node::Space) && *weight <= 0.)
}

/// What a part weighs when room is divided: a part that holds windows
/// weighs something, whatever it was given.
fn heft((child, weight): &(Node, f32)) -> f64 {
    match child {
        Node::Space => f64::from(weight.max(0.)),
        _ => f64::from(weight.max(1e-3)),
    }
}

/// Divides the stretch from `start` to `end` among `children` by their
/// weights, `gap` apart; a part that takes no room gets none and no gap.
fn divide((start, end): (i32, i32), children: &[(Node, f32)], gap: i32) -> Vec<Option<(i32, i32)>> {
    let taking = children.iter().filter(|child| takes_room(child)).count() as i32;
    if taking == 0 {
        return vec![None; children.len()];
    }
    let room = (end - start - gap * (taking - 1)).max(taking);
    let total: f64 = { children.iter() }
        .filter(|child| takes_room(child))
        .map(heft)
        .sum();
    let (mut taken, mut part, mut from) = (0., 0, start);
    let mut spans = Vec::with_capacity(children.len());
    for child in children {
        if !takes_room(child) {
            spans.push(None);
            continue;
        }
        taken += heft(child);
        part += 1;
        let to = match part == taking {
            true => end,
            false => {
                // Weights are kept as `f32`, so a share that should end on
                // a whole pixel may end a hair short of it.
                let edge = start + (f64::from(room) * taken / total + 1e-3).floor() as i32;
                (edge + gap * (part - 1)).max(from + 1)
            }
        };
        spans.push(Some((from, to.max(from + 1))));
        from = to + gap;
    }
    spans
}

/// The share of their room the part at `child` of `children` takes.
fn share_of(children: &[(Node, f32)], child: usize) -> f32 {
    let total: f32 = children.iter().map(|part| heft(part) as f32).sum();
    match (children.get(child), total > 0.) {
        (Some(part), true) => heft(part) as f32 / total,
        _ => 0.,
    }
}

/// The least share a part may be left with: space may go, a part that
/// holds windows keeps [`LEAST_SHARE`].
fn least((child, _): &(Node, f32)) -> f32 {
    match child {
        Node::Space => 0.,
        _ => LEAST_SHARE,
    }
}

/// Has the part at `child` of `children` take `share` of their room, as
/// far as the others can give it or take it; the others keep what they
/// weigh against each other. Whether anything changed is returned.
fn set_share(children: &mut [(Node, f32)], child: usize, share: f32) -> bool {
    if children.len() < 2 || child >= children.len() {
        return false;
    }
    let others_least: f32 = { children.iter().enumerate() }
        .filter(|&(index, _)| index != child)
        .map(|(_, part)| least(part))
        .sum();
    let (lowest, highest) = (least(&children[child]).max(LEAST_SHARE), 1. - others_least);
    if highest < lowest {
        return false;
    }
    let before = share_of(children, child);
    let share = share.clamp(lowest, highest);
    if (share - before).abs() < 1e-4 {
        return false;
    }
    let hefts: Vec<f32> = children.iter().map(|part| heft(part) as f32).collect();
    let total: f32 = hefts.iter().sum();
    let total = if total > 0. { total } else { 1. };
    let others = total - hefts[child];
    let rest = (1. - share) * total;
    let count = children.len() as f32 - 1.;
    for (index, (_, weight)) in children.iter_mut().enumerate() {
        *weight = match index == child {
            true => share * total,
            false if others > 1e-6 => hefts[index] * rest / others,
            false => rest / count,
        };
    }
    true
}

/// Has the part at `child` of `children` take `ratio` of the length it
/// has now, the part at `other` beside it giving or taking the
/// difference; both keep at least what they may be left with. Whether
/// anything changed is returned.
fn shift(children: &mut [(Node, f32)], child: usize, other: usize, ratio: f32) -> bool {
    if child == other || child >= children.len() || other >= children.len() || ratio <= 0. {
        return false;
    }
    let total: f32 = children.iter().map(|part| heft(part) as f32).sum();
    let own = heft(&children[child]) as f32;
    let beside = children[other].1.max(0.);
    let pair = own + beside;
    let least_own = LEAST_SHARE * total;
    let least_beside = least(&children[other]) * total;
    let wanted = (own * ratio).clamp(least_own.min(pair), (pair - least_beside).max(least_own));
    if (wanted - own).abs() < 1e-4 * total.max(1.) {
        return false;
    }
    children[child].1 = wanted;
    children[other].1 = (pair - wanted).max(0.);
    true
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

    const AREA: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1000,
        bottom: 600,
    };

    #[test]
    fn a_room_is_divided_by_the_weights_of_its_parts() {
        let tree = Node::Split {
            axis: Axis::Horizontal,
            children: vec![(Node::Leaf, 3.), (Node::Leaf, 1.)],
        };
        assert_eq!(
            tree.place(AREA, 0),
            [rect(0, 0, 750, 600), rect(750, 0, 1000, 600)]
        );
        assert_eq!(
            Node::even(Axis::Vertical, 2).place(AREA, 20),
            [rect(0, 0, 1000, 290), rect(0, 310, 1000, 600)]
        );
        assert_eq!(Node::Layered(2).place(AREA, 8), [AREA, AREA]);
        assert_eq!(Node::Leaf.place(AREA, 8), [AREA]);
    }

    #[test]
    fn space_that_weighs_nothing_takes_no_room() {
        let tree = Node::Split {
            axis: Axis::Horizontal,
            children: vec![(Node::Space, 0.), (Node::Leaf, 1.), (Node::Space, 0.)],
        };
        assert_eq!(tree.place(AREA, 10), [AREA]);
        let centred = Node::Split {
            axis: Axis::Horizontal,
            children: vec![(Node::Space, 1.), (Node::Leaf, 2.), (Node::Space, 1.)],
        };
        assert_eq!(centred.place(AREA, 0), [rect(250, 0, 750, 600)]);
    }

    #[test]
    fn a_leaf_grows_within_the_nearest_room_along_the_axis() {
        // A window on the left, two stacked on the right.
        let mut tree = Node::Split {
            axis: Axis::Horizontal,
            children: vec![(Node::Leaf, 1.), (Node::even(Axis::Vertical, 2), 1.)],
        };
        // The lower window on the right grows across: its column grows.
        assert!(tree.grow(2, Axis::Horizontal, 0.1));
        let places = tree.place(AREA, 0);
        assert_eq!(places[0], rect(0, 0, 400, 600));
        assert_eq!(places[2], rect(400, 300, 1000, 600));
        // Down it grows against the window above it.
        assert!(tree.grow(2, Axis::Vertical, 0.25));
        assert_eq!(tree.place(AREA, 0)[1], rect(400, 0, 1000, 150));
        // The window on the left has no room divided down around it.
        assert!(!tree.grow(0, Axis::Vertical, 0.1));
        // A part keeps a twentieth of its room.
        assert!(tree.grow(0, Axis::Horizontal, -0.9));
        assert_eq!(tree.place(AREA, 0)[0], rect(0, 0, 50, 600));
        assert!(!tree.grow(0, Axis::Horizontal, -0.1));
    }

    #[test]
    fn equal_weights_come_back() {
        let mut tree = Node::Split {
            axis: Axis::Horizontal,
            children: vec![(Node::Leaf, 3.), (Node::even(Axis::Vertical, 2), 0.5)],
        };
        tree.grow(1, Axis::Vertical, 0.2);
        tree.equalize();
        let even = Node::Split {
            axis: Axis::Horizontal,
            children: vec![(Node::Leaf, 1.), (Node::even(Axis::Vertical, 2), 1.)],
        };
        assert_eq!(tree, even);
    }

    #[test]
    fn a_dragged_side_moves_the_border_with_the_part_beside_it() {
        let mut tree = Node::even(Axis::Horizontal, 3);
        let tiles = tree.place(AREA, 0);
        assert_eq!(tiles[1], rect(333, 0, 666, 600));
        // The right side of the middle window is dragged to 800.
        assert!(tree.drag(1, AREA, 0, Side::Right, 800));
        let tiles = tree.place(AREA, 0);
        assert_eq!(tiles[0], rect(0, 0, 333, 600));
        assert_eq!(tiles[1].right, 800);
        assert_eq!(tiles[2], rect(800, 0, 1000, 600));
        // The left side of the first window has nothing beside it.
        assert!(!tree.drag(0, AREA, 0, Side::Left, 100));
        // A window keeps a twentieth of the room.
        assert!(tree.drag(2, AREA, 0, Side::Left, 999));
        assert!(tree.place(AREA, 0)[2].right - tree.place(AREA, 0)[2].left >= 49);
    }

    #[test]
    fn a_dragged_side_in_a_nested_room_finds_the_room_that_has_it() {
        let mut tree = Node::Split {
            axis: Axis::Horizontal,
            children: vec![(Node::Leaf, 1.), (Node::even(Axis::Vertical, 2), 1.)],
        };
        // The left side of the upper window on the right is the border of
        // the two columns.
        assert!(tree.drag(1, AREA, 0, Side::Left, 300));
        let tiles = tree.place(AREA, 0);
        assert_eq!(tiles[0], rect(0, 0, 300, 600));
        assert_eq!(tiles[1].left, 300);
        assert_eq!(tiles[2].left, 300);
        // Its bottom is the border of the two windows on the right.
        assert!(tree.drag(1, AREA, 0, Side::Bottom, 400));
        assert_eq!(tree.place(AREA, 0)[2].top, 400);
    }

    #[test]
    fn the_sides_that_moved_are_found_and_a_move_is_no_resize() {
        let tile = rect(0, 0, 500, 600);
        assert_eq!(
            Side::moved(tile, rect(0, 0, 650, 600)),
            [(Side::Right, 650)]
        );
        assert_eq!(Side::moved(tile, rect(1, 0, 501, 601)), []);
        let both = Side::moved(tile, rect(0, 100, 600, 600));
        assert_eq!(both, [(Side::Right, 600), (Side::Top, 100)]);
    }

    #[test]
    fn a_leaf_is_split_and_taken_away() {
        let (tree, at) = Node::Leaf.split(0, Axis::Horizontal, false);
        assert_eq!((tree.count(), at), (2, 1));
        let (tree, at) = tree.split(1, Axis::Vertical, true);
        assert_eq!(at, 1);
        let shaped = Node::Split {
            axis: Axis::Horizontal,
            children: vec![(Node::Leaf, 1.), (Node::even(Axis::Vertical, 2), 1.)],
        };
        assert_eq!(tree, shaped);
        // The room left with one part gives its place to it.
        let tree = tree.without(2).unwrap();
        assert_eq!(tree, Node::even(Axis::Horizontal, 2));
        let tree = tree.without(0).unwrap();
        assert_eq!(tree, Node::Leaf);
        assert_eq!(tree.without(0), None);
        assert_eq!(Node::Layered(3).without(1), Some(Node::Layered(2)));
    }

    #[test]
    fn weights_carry_over_to_parts_of_the_same_kind() {
        let old = Node::Split {
            axis: Axis::Horizontal,
            children: vec![(Node::Leaf, 3.), (Node::even(Axis::Vertical, 2), 1.)],
        };
        let mut grown = Node::Split {
            axis: Axis::Horizontal,
            children: vec![(Node::Leaf, 1.), (Node::even(Axis::Vertical, 3), 1.)],
        };
        grown.adopt(&old);
        assert_eq!(grown.place(AREA, 0)[0], rect(0, 0, 750, 600));
        let mut other = Node::even(Axis::Vertical, 2);
        other.adopt(&old);
        assert_eq!(other, Node::even(Axis::Vertical, 2));
    }

    #[test]
    fn a_tree_is_written_and_read_back() {
        let tree = Node::Split {
            axis: Axis::Horizontal,
            children: vec![
                (Node::Space, 0.25),
                (Node::even(Axis::Vertical, 2), 1.5),
                (Node::Layered(3), 1.),
            ],
        };
        let text = tree.to_text();
        assert_eq!(text, "h(0.25:s,1.5:v(1:l,1:l),1:m3)");
        assert_eq!(Node::from_text(&text), Ok(tree));
        assert_eq!(Node::from_text(" l "), Ok(Node::Leaf));
        for wrong in ["", "x", "h(", "h(1:l", "h(l)", "m0", "l l", "h(-1:l)"] {
            assert!(Node::from_text(wrong).is_err(), "{wrong}");
        }
        let deep = format!("{}l{}", "h(1:".repeat(80), ")".repeat(80));
        assert!(Node::from_text(&deep).is_err());
        for huge in [
            "m513",
            "h(1:l,1:l,1:m18446744073709551614)",
            "h(1:m300,1:m300)",
        ] {
            assert!(Node::from_text(huge).is_err(), "{huge}");
        }
        assert_eq!(Node::from_text("m512"), Ok(Node::Layered(512)));
    }

    #[test]
    fn a_room_is_divided_without_overlaps_for_any_weights() {
        for weights in [[1., 1., 1.], [0.01, 5., 0.3], [7., 0.2, 0.2]] {
            let tree = Node::Split {
                axis: Axis::Horizontal,
                children: weights.iter().map(|&weight| (Node::Leaf, weight)).collect(),
            };
            let tiles = tree.place(rect(10, 10, 1001, 700), 7);
            assert_eq!(tiles[0].left, 10);
            assert_eq!(tiles[2].right, 1001);
            for pair in tiles.windows(2) {
                assert!(pair[0].right + 7 <= pair[1].left + 1, "{weights:?}");
                assert!(pair[0].right > pair[0].left);
            }
        }
    }
}
