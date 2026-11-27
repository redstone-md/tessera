// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The layouts that come with Tessera, each a way to make a tree of tiles
//! for a number of windows, and the choice between them and the layouts
//! of files.

use serde::Deserialize;

use super::tree::{Axis, Node};
use crate::platform::ScreenRect;

/// How the windows of a display share it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TileLayout {
    /// The first window takes the left half, the others share the right
    /// one, stacked.
    #[default]
    Master,
    /// Side by side.
    Columns,
    /// One above the other.
    Rows,
    /// Rows of equally many windows; the last row may hold fewer, wider
    /// ones.
    Grid,
    /// Every window takes the whole space, one over the other.
    Monocle,
    /// Binary space partition: each new window divides the tile of the
    /// window in front in two, along its longer side.
    Bsp,
    /// A spiral: each new window divides the last tile in two, across and
    /// down in turn.
    Dwindle,
    /// The first window in the middle, the others stacked to its right and
    /// to its left, for very wide displays.
    Ultrawide,
    /// One column in the middle, with room beside it on displays much
    /// wider than they are high.
    Centered,
}

/// How a layout that keeps its tree adds a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Growth {
    /// The tile of the window in front is divided along its longer side.
    Bsp,
    /// The last tile is divided across the direction its room is divided
    /// in.
    Dwindle,
}

/// A tree of tiles made for a number of windows, and the window that goes
/// to each leaf: `at[leaf]` is the place of the window in the order of
/// the windows.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    pub tree: Option<Node>,
    pub at: Vec<usize>,
}

impl Generated {
    /// `tree`, whose leaves take the windows in their order.
    pub fn in_order(tree: Node) -> Self {
        let at = (0..tree.count()).collect();
        Self {
            tree: Some(tree),
            at,
        }
    }

    fn empty() -> Self {
        Self {
            tree: None,
            at: Vec::new(),
        }
    }
}

/// How wide a column of `centered` is against the height of the display:
/// as a display of 16 by 9.
const COLUMN_ASPECT: f32 = 16. / 9.;

/// The share of the width from which the column of `centered` takes all
/// of it, so that only displays much wider than that have room beside it.
const WHOLE_COLUMN: f32 = 0.9;

impl TileLayout {
    pub const ALL: [Self; 9] = [
        Self::Master,
        Self::Columns,
        Self::Rows,
        Self::Grid,
        Self::Monocle,
        Self::Bsp,
        Self::Dwindle,
        Self::Ultrawide,
        Self::Centered,
    ];

    /// What the layout is called where it is shown; in lowercase, what the
    /// configuration calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Master => "Master",
            Self::Columns => "Columns",
            Self::Rows => "Rows",
            Self::Grid => "Grid",
            Self::Monocle => "Monocle",
            Self::Bsp => "BSP",
            Self::Dwindle => "Dwindle",
            Self::Ultrawide => "Ultrawide",
            Self::Centered => "Centered",
        }
    }

    /// The layout the configuration calls `name`, whatever the case of
    /// its letters.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|layout| layout.name().eq_ignore_ascii_case(name))
    }

    /// How the layout adds windows, if it keeps its tree instead of
    /// making it again for each number of windows.
    pub fn growth(self) -> Option<Growth> {
        match self {
            Self::Bsp => Some(Growth::Bsp),
            Self::Dwindle => Some(Growth::Dwindle),
            _ => None,
        }
    }

    /// The tree of `count` windows in `area`, for a layout that makes it
    /// again for each number of windows. One that keeps its tree gets the
    /// tree it would have if the windows came one after the other.
    pub fn generate(self, count: usize, area: ScreenRect) -> Generated {
        if count == 0 {
            return Generated::empty();
        }
        let tree = match self {
            Self::Columns => Node::even(Axis::Horizontal, count),
            Self::Rows => Node::even(Axis::Vertical, count),
            Self::Master if count == 1 => Node::Leaf,
            Self::Master => Node::Split {
                axis: Axis::Horizontal,
                children: vec![
                    (Node::Leaf, 1.),
                    (Node::even(Axis::Vertical, count - 1), 1.),
                ],
            },
            Self::Grid => grid(count),
            // The space beside the windows has room only once a window
            // was made narrower.
            Self::Monocle => {
                let middle = Node::Split {
                    axis: Axis::Vertical,
                    children: vec![
                        (Node::Space, 0.),
                        (Node::Layered(count), 1.),
                        (Node::Space, 0.),
                    ],
                };
                Node::Split {
                    axis: Axis::Horizontal,
                    children: vec![(Node::Space, 0.), (middle, 1.), (Node::Space, 0.)],
                }
            }
            Self::Centered => {
                let (width, height) = (area.right - area.left, area.bottom - area.top);
                let column = match width > 0 {
                    true => height as f32 * COLUMN_ASPECT / width as f32,
                    false => 1.,
                };
                let column = if column >= WHOLE_COLUMN { 1. } else { column };
                let beside = (1. - column) / 2.;
                Node::Split {
                    axis: Axis::Horizontal,
                    children: vec![
                        (Node::Space, beside),
                        (Node::even(Axis::Vertical, count), column),
                        (Node::Space, beside),
                    ],
                }
            }
            Self::Ultrawide => return ultrawide(count),
            Self::Bsp | Self::Dwindle => {
                let growth = self.growth().unwrap_or(Growth::Bsp);
                let mut tree = Node::Leaf;
                for _ in 1..count {
                    let leaf = tree.count() - 1;
                    let axis = growth.axis(&tree, leaf, area, 0);
                    tree = tree.split(leaf, axis, false).0;
                }
                tree
            }
        };
        Generated::in_order(tree)
    }
}

impl Growth {
    /// The direction the leaf at `leaf` of `tree` is divided in when a
    /// window comes and nothing was asked of where it goes.
    pub fn axis(self, tree: &Node, leaf: usize, area: ScreenRect, gap: i32) -> Axis {
        match self {
            Self::Bsp => {
                let places = tree.place(area, gap);
                places
                    .get(leaf)
                    .map_or(Axis::Horizontal, |&place| Axis::longer(place))
            }
            Self::Dwindle => parent_axis(tree, leaf).map_or(Axis::Horizontal, Axis::other),
        }
    }
}

/// The direction the room right around the leaf at `leaf` is divided in.
fn parent_axis(tree: &Node, leaf: usize) -> Option<Axis> {
    let path = tree.leaves().into_iter().nth(leaf)?;
    let (_, room) = path.split_last()?;
    let mut node = tree;
    for &step in room {
        match node {
            Node::Split { children, .. } => node = &children.get(step)?.0,
            _ => return None,
        }
    }
    match node {
        Node::Split { axis, .. } => Some(*axis),
        _ => None,
    }
}

/// Rows of equally many windows, as many columns as the square root of
/// `count` rounded up; the last row may hold fewer.
fn grid(count: usize) -> Node {
    let columns = (1..=count)
        .find(|columns| columns * columns >= count)
        .unwrap_or(count);
    let rows = count.div_ceil(columns);
    let children = (0..rows)
        .map(|row| {
            let held = columns.min(count - row * columns);
            (Node::even(Axis::Horizontal, held), 1.)
        })
        .collect();
    Node::Split {
        axis: Axis::Vertical,
        children,
    }
}

/// The first window in the middle, twice as wide as each side; the next
/// ones stacked on the right, and once that side holds half of them,
/// on the left. With two windows the left side is space that takes no
/// room, so that the parts keep their places when a third one comes.
fn ultrawide(count: usize) -> Generated {
    if count == 1 {
        return Generated::in_order(Node::Leaf);
    }
    let others = count - 1;
    let right = others.div_ceil(2);
    let left = others - right;
    let side = |held: usize| match held {
        0 => (Node::Space, 0.),
        held => (Node::even(Axis::Vertical, held), 1.),
    };
    let tree = Node::Split {
        axis: Axis::Horizontal,
        children: vec![side(left), (Node::Leaf, 2.), side(right)],
    };
    // The leaves go from the left: the left stack, the middle, the right.
    let mut at: Vec<usize> = (1 + right..count).collect();
    at.push(0);
    at.extend(1..=right);
    Generated {
        tree: Some(tree),
        at,
    }
}

/// A layout to use: one of Tessera's own, or one of a layout file, by the
/// name of the file without its extension, in lowercase.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LayoutChoice {
    Builtin(TileLayout),
    File(String),
}

impl Default for LayoutChoice {
    fn default() -> Self {
        Self::Builtin(TileLayout::default())
    }
}

impl LayoutChoice {
    /// The layout `name` names: one of Tessera's own where it is the name
    /// of one, whatever the case of its letters, or else a layout file.
    pub fn of(name: &str) -> Self {
        match TileLayout::named(name) {
            Some(layout) => Self::Builtin(layout),
            None => Self::File(name.trim().to_lowercase()),
        }
    }

    /// The name the configuration and the commands give the layout.
    pub fn name(&self) -> String {
        match self {
            Self::Builtin(layout) => layout.name().to_ascii_lowercase(),
            Self::File(name) => name.clone(),
        }
    }
}

impl<'de> Deserialize<'de> for LayoutChoice {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        match name.trim().is_empty() {
            true => Err(serde::de::Error::custom("a layout has a name")),
            false => Ok(Self::of(&name)),
        }
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

    const AREA: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1000,
        bottom: 600,
    };

    fn tiles(layout: TileLayout, count: usize) -> Vec<ScreenRect> {
        let made = layout.generate(count, AREA);
        let places = made
            .tree
            .map(|tree| tree.place(AREA, 0))
            .unwrap_or_default();
        let mut by_window = vec![AREA; count];
        for (leaf, &window) in made.at.iter().enumerate() {
            by_window[window] = places[leaf];
        }
        by_window
    }

    #[test]
    fn a_layout_is_named_in_any_case_and_a_file_otherwise() {
        assert_eq!(TileLayout::named("bsp"), Some(TileLayout::Bsp));
        assert_eq!(TileLayout::named("Ultrawide"), Some(TileLayout::Ultrawide));
        assert_eq!(
            LayoutChoice::of("GRID"),
            LayoutChoice::Builtin(TileLayout::Grid)
        );
        assert_eq!(
            LayoutChoice::of("Tall"),
            LayoutChoice::File("tall".to_owned())
        );
        assert_eq!(LayoutChoice::of("Tall").name(), "tall");
        assert_eq!(LayoutChoice::Builtin(TileLayout::Bsp).name(), "bsp");
        for layout in TileLayout::ALL {
            assert_eq!(TileLayout::named(layout.name()), Some(layout));
        }
    }

    #[test]
    fn every_layout_gives_each_window_a_tile() {
        for layout in TileLayout::ALL {
            assert_eq!(layout.generate(0, AREA).tree, None);
            for count in 1..=9 {
                let made = layout.generate(count, AREA);
                let tree = made.tree.unwrap();
                assert_eq!(tree.count(), count, "{layout:?}");
                let mut at = made.at.clone();
                at.sort_unstable();
                assert_eq!(at, (0..count).collect::<Vec<_>>(), "{layout:?}");
            }
        }
    }

    #[test]
    fn master_columns_rows_and_grid_place_as_before() {
        assert_eq!(
            tiles(TileLayout::Master, 3),
            [
                rect(0, 0, 500, 600),
                rect(500, 0, 1000, 300),
                rect(500, 300, 1000, 600)
            ]
        );
        assert_eq!(
            tiles(TileLayout::Rows, 2),
            [rect(0, 0, 1000, 300), rect(0, 300, 1000, 600)]
        );
        // Three in a grid: two above, a wide one below.
        assert_eq!(
            tiles(TileLayout::Grid, 3),
            [
                rect(0, 0, 500, 300),
                rect(500, 0, 1000, 300),
                rect(0, 300, 1000, 600)
            ]
        );
        assert_eq!(tiles(TileLayout::Monocle, 2), [AREA, AREA]);
    }

    #[test]
    fn ultrawide_puts_the_first_window_in_the_middle() {
        let two = tiles(TileLayout::Ultrawide, 2);
        assert_eq!(two, [rect(0, 0, 666, 600), rect(666, 0, 1000, 600)]);
        let four = tiles(TileLayout::Ultrawide, 4);
        assert_eq!(four[0], rect(250, 0, 750, 600));
        assert_eq!(four[1], rect(750, 0, 1000, 300));
        assert_eq!(four[2], rect(750, 300, 1000, 600));
        assert_eq!(four[3], rect(0, 0, 250, 600));
    }

    #[test]
    fn centered_leaves_room_beside_its_column_only_where_the_display_is_wide() {
        let wide = rect(0, 0, 3440, 1440);
        let made = TileLayout::Centered.generate(1, wide);
        let place = made.tree.unwrap().place(wide, 0)[0];
        assert!((place.right - place.left).abs_diff(2560) <= 1, "{place:?}");
        assert!(place.left.abs_diff((3440 - 2560) / 2) <= 1, "{place:?}");
        // A display of 16 by 10 with a bar has none.
        assert_eq!(tiles(TileLayout::Centered, 1), [AREA]);
        let bar = rect(0, 40, 2560, 1440);
        let made = TileLayout::Centered.generate(2, bar);
        assert_eq!(made.tree.unwrap().place(bar, 0)[0].left, 0);
    }

    #[test]
    fn bsp_and_dwindle_divide_the_newest_tile() {
        // On a wide area, the second window goes beside the first and the
        // third under the second.
        assert_eq!(
            tiles(TileLayout::Dwindle, 3),
            [
                rect(0, 0, 500, 600),
                rect(500, 0, 1000, 300),
                rect(500, 300, 1000, 600)
            ]
        );
        let four = tiles(TileLayout::Dwindle, 4);
        assert_eq!(four[2], rect(500, 300, 750, 600));
        assert_eq!(four[3], rect(750, 300, 1000, 600));
        // BSP divides the longer side: the tile of 500 by 300 across.
        let four = tiles(TileLayout::Bsp, 4);
        assert_eq!(four[2], rect(500, 300, 750, 600));
        assert_eq!(
            tiles(TileLayout::Bsp, 2),
            [rect(0, 0, 500, 600), rect(500, 0, 1000, 600)]
        );
    }

    #[test]
    fn a_layout_is_read_by_its_name() {
        #[derive(Deserialize)]
        struct Holder {
            layout: LayoutChoice,
        }
        let read = |text: &str| toml::from_str::<Holder>(text).map(|holder| holder.layout);
        assert_eq!(
            read("layout = \"dwindle\"").unwrap(),
            LayoutChoice::Builtin(TileLayout::Dwindle)
        );
        assert_eq!(
            read("layout = \"three-columns\"").unwrap(),
            LayoutChoice::File("three-columns".to_owned())
        );
        assert!(read("layout = \" \"").is_err());
        assert!(read("layout = 3").is_err());
    }
}
