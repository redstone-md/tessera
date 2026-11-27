// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Where the icons of the desktop stand: the grid of cells of each
//! display, how icons are arranged in it (column after column from the
//! top left, as Explorer arranges them), where a new icon goes, and the
//! places kept for every arrangement of displays.
//!
//! A place is a [`Spot`]: a column and a row of the grid of a display,
//! whole numbers where the icons are aligned to the grid, any where they
//! are placed freely. Counting in cells, not in pixels, keeps the icons
//! where they were when they are drawn larger or smaller.
//!
//! The places are kept per arrangement of displays, as Explorer keeps
//! them: under a key that names every display by its size and where it
//! is. When a display goes, the other arrangement is used, and the
//! places of the one before are kept for when the display comes back.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::platform::ScreenRect;

/// How the icons are arranged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arrangement {
    /// In the order they are sorted in, column after column; they cannot
    /// be put elsewhere.
    Auto,
    /// Where they were put, in the cells of the grid.
    #[default]
    Grid,
    /// Where they were put, anywhere.
    Free,
}

impl Arrangement {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Grid => "grid",
            Self::Free => "free",
        }
    }
}

/// A place of an icon: a column and a row of the grid of the display
/// numbered `display` among the displays in their order (see
/// [`ordered_displays`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "(f32, f32, usize)", into = "(f32, f32, usize)")]
pub struct Spot {
    pub column: f32,
    pub row: f32,
    pub display: usize,
}

impl From<(f32, f32, usize)> for Spot {
    fn from((column, row, display): (f32, f32, usize)) -> Self {
        Self {
            column,
            row,
            display,
        }
    }
}

impl From<Spot> for (f32, f32, usize) {
    fn from(spot: Spot) -> Self {
        (spot.column, spot.row, spot.display)
    }
}

impl Spot {
    /// The cell at `column` and `row` of `display`.
    pub fn cell(column: usize, row: usize, display: usize) -> Self {
        Self {
            column: column as f32,
            row: row as f32,
            display,
        }
    }

    /// The cell the spot lies most in.
    pub fn nearest_cell(self) -> (usize, usize) {
        (
            self.column.round().max(0.) as usize,
            self.row.round().max(0.) as usize,
        )
    }
}

/// The cells of the room of a display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub columns: usize,
    pub rows: usize,
}

impl Grid {
    /// The grid of cells `cell_width` by `cell_height` that fits a room
    /// `width` by `height`, all in logical pixels; at least one cell.
    pub fn fitting(width: f32, height: f32, cell_width: f32, cell_height: f32) -> Self {
        let count = |length: f32, cell: f32| (length / cell.max(1.)).floor().max(1.) as usize;
        Self {
            columns: count(width, cell_width),
            rows: count(height, cell_height),
        }
    }

    pub fn cells(self) -> usize {
        self.columns * self.rows
    }

    /// The cell numbered `index`, counted down the columns from the top
    /// left; beyond the last cell, the last cell, as Explorer piles up
    /// what does not fit.
    pub fn nth(self, index: usize) -> (usize, usize) {
        let index = index.min(self.cells() - 1);
        (index / self.rows, index % self.rows)
    }

    /// The number of the cell `cell`, counted as [`Self::nth`] counts.
    fn index_of(self, (column, row): (usize, usize)) -> usize {
        column.min(self.columns - 1) * self.rows + row.min(self.rows - 1)
    }

    /// `spot` moved into the grid where it lies beyond it, as after the
    /// display became smaller.
    pub fn clamp(self, spot: Spot) -> Spot {
        Spot {
            column: spot.column.clamp(0., (self.columns - 1) as f32),
            row: spot.row.clamp(0., (self.rows - 1) as f32),
            display: spot.display,
        }
    }

    /// The cell nearest to `spot`, within the grid.
    pub fn snap(self, spot: Spot) -> (usize, usize) {
        let (column, row) = spot.nearest_cell();
        (column.min(self.columns - 1), row.min(self.rows - 1))
    }

    /// The first cell not in `taken`, counted down the columns from
    /// `from` and around to the top left; the last cell where all are
    /// taken.
    pub fn first_free(
        self,
        taken: &HashSet<(usize, usize)>,
        from: Option<(usize, usize)>,
    ) -> (usize, usize) {
        let start = from.map_or(0, |from| self.index_of(from));
        let cells = self.cells();
        (0..cells)
            .map(|offset| self.nth((start + offset) % cells))
            .find(|cell| !taken.contains(cell))
            .unwrap_or_else(|| self.nth(cells - 1))
    }
}

/// Where each icon stands, by its key, given the keys in the order they
/// are sorted in, the places kept for them, how they are arranged, the
/// grid of each display and which display is the primary one. An icon
/// without a place, or with one on a display that is not there, goes
/// into the first free cell of the primary display, from `new_from` on
/// where it is given (where something was dropped or made).
pub fn lay_out(
    order: &[String],
    kept: &HashMap<String, Spot>,
    arrangement: Arrangement,
    grids: &[Grid],
    primary: usize,
    new_from: Option<Spot>,
) -> HashMap<String, Spot> {
    let mut placed = HashMap::new();
    if grids.is_empty() {
        return placed;
    }
    let primary = primary.min(grids.len() - 1);
    if arrangement == Arrangement::Auto {
        // The primary display first, then the others where it is full.
        let mut displays =
            std::iter::once(primary).chain((0..grids.len()).filter(|&d| d != primary));
        let mut display = displays.next().unwrap_or(0);
        let mut index = 0;
        for key in order {
            while index >= grids[display].cells() {
                match displays.next() {
                    Some(next) => (display, index) = (next, 0),
                    None => break,
                }
            }
            let (column, row) = grids[display].nth(index);
            placed.insert(key.clone(), Spot::cell(column, row, display));
            index += 1;
        }
        return placed;
    }
    let mut taken: Vec<HashSet<(usize, usize)>> = vec![HashSet::new(); grids.len()];
    let mut homeless = Vec::new();
    for key in order {
        let Some(spot) = kept.get(key).filter(|spot| spot.display < grids.len()) else {
            homeless.push(key);
            continue;
        };
        let grid = grids[spot.display];
        let spot = match arrangement {
            Arrangement::Free => grid.clamp(*spot),
            _ => {
                let wanted = grid.snap(*spot);
                let taken = &taken[spot.display];
                let cell = match taken.contains(&wanted) {
                    true => grid.first_free(taken, Some(wanted)),
                    false => wanted,
                };
                Spot::cell(cell.0, cell.1, spot.display)
            }
        };
        taken[spot.display].insert(grid.snap(spot));
        placed.insert(key.clone(), spot);
    }
    let start = new_from.filter(|spot| spot.display < grids.len());
    let display = start.map_or(primary, |spot| spot.display);
    let grid = grids[display];
    let mut from = start.map(|spot| grid.snap(spot));
    for key in homeless {
        let cell = grid.first_free(&taken[display], from);
        taken[display].insert(cell);
        from = Some(cell);
        placed.insert(key.clone(), Spot::cell(cell.0, cell.1, display));
    }
    placed
}

/// The direction an arrow key moves the choice in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toward {
    Left,
    Right,
    Up,
    Down,
}

/// The icon an arrow key moves to from `from`: among `spots`, the one
/// nearest in the direction `toward` on the same display, a step along
/// counting less than one aside. `None` where there is none that way.
pub fn neighbour<'a>(
    spots: impl IntoIterator<Item = (&'a String, &'a Spot)>,
    from: Spot,
    toward: Toward,
) -> Option<&'a String> {
    let mut best: Option<(f32, &String)> = None;
    for (key, spot) in spots {
        if spot.display != from.display {
            continue;
        }
        let (dx, dy) = (spot.column - from.column, spot.row - from.row);
        let (along, aside) = match toward {
            Toward::Right => (dx, dy),
            Toward::Left => (-dx, dy),
            Toward::Down => (dy, dx),
            Toward::Up => (-dy, dx),
        };
        if along < 0.25 {
            continue;
        }
        let cost = along * along + 4. * aside * aside;
        if best.is_none_or(|(least, _)| cost < least) {
            best = Some((cost, key));
        }
    }
    best.map(|(_, key)| key)
}

/// The displays in the order spots count them: from the left, then from
/// the top.
pub fn ordered_displays(mut frames: Vec<ScreenRect>) -> Vec<ScreenRect> {
    frames.sort_by_key(|frame| (frame.left, frame.top));
    frames
}

/// The key the places of an arrangement of displays are kept under: each
/// display by its size and where it is, in their order.
pub fn display_set_key(frames: &[ScreenRect]) -> String {
    let named: Vec<String> = ordered_displays(frames.to_vec())
        .iter()
        .map(|frame| {
            format!(
                "{}x{}@{},{}",
                frame.right - frame.left,
                frame.bottom - frame.top,
                frame.left,
                frame.top
            )
        })
        .collect();
    named.join(";")
}

/// The places of the icons for every arrangement of displays, as written
/// in `desktop-icons.toml`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Positions {
    sets: BTreeMap<String, BTreeMap<String, Spot>>,
}

impl Positions {
    /// The places written in the file at `path`; none where there is no
    /// file, and none, reported, where it cannot be read.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|error| {
                report!(
                    "the places of the icons in {} are not read: {error}",
                    path.display()
                );
                Self::default()
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                report!("the places of the icons are not read: {error}");
                Self::default()
            }
        }
    }

    /// Writes the places to the file at `path`, through a file beside it,
    /// so that a file half written is never read.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string(self).map_err(io::Error::other)?;
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let written = path.with_extension("toml.new");
        std::fs::write(&written, text)?;
        std::fs::rename(&written, path)
    }

    /// The places kept for the arrangement `set`.
    pub fn of_set(&self, set: &str) -> HashMap<String, Spot> {
        { self.sets.get(set).into_iter().flatten() }
            .map(|(key, spot)| (key.clone(), *spot))
            .collect()
    }

    /// Keeps `spots` as the places of the arrangement `set`, in place of
    /// those kept before; says whether anything changed.
    pub fn keep(&mut self, set: &str, spots: &HashMap<String, Spot>) -> bool {
        let spots: BTreeMap<String, Spot> = { spots.iter() }
            .map(|(key, spot)| (key.clone(), *spot))
            .collect();
        if self.sets.get(set) == Some(&spots) {
            return false;
        }
        self.sets.insert(set.to_owned(), spots);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn rect(left: i32, top: i32, width: i32, height: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }

    const SMALL: Grid = Grid {
        columns: 2,
        rows: 3,
    };

    #[test]
    fn a_grid_holds_as_many_whole_cells_as_fit_and_at_least_one() {
        assert_eq!(
            Grid::fitting(1000., 700., 100., 120.),
            Grid {
                columns: 10,
                rows: 5
            }
        );
        assert_eq!(
            Grid::fitting(10., 10., 100., 120.),
            Grid {
                columns: 1,
                rows: 1
            }
        );
    }

    #[test]
    fn icons_are_counted_down_the_columns_and_pile_up_in_the_last_cell() {
        assert_eq!(SMALL.nth(0), (0, 0));
        assert_eq!(SMALL.nth(2), (0, 2));
        assert_eq!(SMALL.nth(3), (1, 0));
        assert_eq!(SMALL.nth(5), (1, 2));
        assert_eq!(SMALL.nth(9), (1, 2));
    }

    #[test]
    fn auto_arrangement_fills_the_primary_display_first() {
        let order = keys(&["a", "b", "c", "d"]);
        let grids = [
            Grid {
                columns: 1,
                rows: 1,
            },
            Grid {
                columns: 1,
                rows: 2,
            },
        ];
        let placed = lay_out(&order, &HashMap::new(), Arrangement::Auto, &grids, 1, None);
        assert_eq!(placed["a"], Spot::cell(0, 0, 1));
        assert_eq!(placed["b"], Spot::cell(0, 1, 1));
        assert_eq!(placed["c"], Spot::cell(0, 0, 0));
        // Nothing is left out where nothing has room: the last cell.
        assert_eq!(placed["d"], Spot::cell(0, 0, 0));
    }

    #[test]
    fn auto_arrangement_ignores_the_places_kept() {
        let order = keys(&["a", "b"]);
        let kept = HashMap::from([("a".to_owned(), Spot::cell(1, 2, 0))]);
        let placed = lay_out(&order, &kept, Arrangement::Auto, &[SMALL], 0, None);
        assert_eq!(placed["a"], Spot::cell(0, 0, 0));
        assert_eq!(placed["b"], Spot::cell(0, 1, 0));
    }

    #[test]
    fn a_new_icon_takes_the_first_free_cell() {
        let order = keys(&["kept", "new"]);
        let kept = HashMap::from([("kept".to_owned(), Spot::cell(0, 0, 0))]);
        let placed = lay_out(&order, &kept, Arrangement::Grid, &[SMALL], 0, None);
        assert_eq!(placed["kept"], Spot::cell(0, 0, 0));
        assert_eq!(placed["new"], Spot::cell(0, 1, 0));
    }

    #[test]
    fn a_new_icon_goes_where_it_was_dropped_when_that_is_free() {
        let order = keys(&["kept", "one", "two"]);
        let kept = HashMap::from([("kept".to_owned(), Spot::cell(1, 1, 0))]);
        let dropped = Some(Spot::cell(1, 1, 0));
        let placed = lay_out(&order, &kept, Arrangement::Grid, &[SMALL], 0, dropped);
        assert_eq!(placed["one"], Spot::cell(1, 2, 0));
        // Around to the top left once the last cell is passed.
        assert_eq!(placed["two"], Spot::cell(0, 0, 0));
    }

    #[test]
    fn two_icons_in_one_cell_are_set_apart_on_the_grid() {
        let order = keys(&["a", "b"]);
        let kept = HashMap::from([
            ("a".to_owned(), Spot::cell(0, 1, 0)),
            ("b".to_owned(), Spot::cell(0, 1, 0)),
        ]);
        let placed = lay_out(&order, &kept, Arrangement::Grid, &[SMALL], 0, None);
        assert_eq!(placed["a"], Spot::cell(0, 1, 0));
        assert_eq!(placed["b"], Spot::cell(0, 2, 0));
    }

    #[test]
    fn free_places_are_kept_but_stay_on_the_display() {
        let order = keys(&["a", "b"]);
        let free = Spot {
            column: 0.4,
            row: 1.6,
            display: 0,
        };
        let beyond = Spot {
            column: 7.5,
            row: -1.,
            display: 0,
        };
        let kept = HashMap::from([("a".to_owned(), free), ("b".to_owned(), beyond)]);
        let placed = lay_out(&order, &kept, Arrangement::Free, &[SMALL], 0, None);
        assert_eq!(placed["a"], free);
        assert_eq!(
            placed["b"],
            Spot {
                column: 1.,
                row: 0.,
                display: 0
            }
        );
        // Aligned to the grid, the free places snap to cells.
        let placed = lay_out(&order, &kept, Arrangement::Grid, &[SMALL], 0, None);
        assert_eq!(placed["a"], Spot::cell(0, 2, 0));
    }

    #[test]
    fn an_icon_of_a_display_that_is_gone_moves_to_the_primary_one() {
        let order = keys(&["far"]);
        let kept = HashMap::from([("far".to_owned(), Spot::cell(0, 0, 3))]);
        let placed = lay_out(&order, &kept, Arrangement::Grid, &[SMALL, SMALL], 1, None);
        assert_eq!(placed["far"], Spot::cell(0, 0, 1));
    }

    #[test]
    fn the_first_free_cell_is_the_last_one_when_all_are_taken() {
        let taken: HashSet<_> = (0..SMALL.cells()).map(|index| SMALL.nth(index)).collect();
        assert_eq!(SMALL.first_free(&taken, None), (1, 2));
    }

    #[test]
    fn an_arrow_moves_to_the_nearest_icon_that_way() {
        let spots = HashMap::from([
            ("right".to_owned(), Spot::cell(1, 0, 0)),
            ("far right".to_owned(), Spot::cell(3, 0, 0)),
            ("below".to_owned(), Spot::cell(0, 1, 0)),
            ("right below".to_owned(), Spot::cell(1, 1, 0)),
            ("elsewhere".to_owned(), Spot::cell(0, 2, 1)),
        ]);
        let from = Spot::cell(0, 0, 0);
        let moved = |toward| neighbour(&spots, from, toward).map(String::as_str);
        assert_eq!(moved(Toward::Right), Some("right"));
        assert_eq!(moved(Toward::Down), Some("below"));
        assert_eq!(moved(Toward::Left), None);
        assert_eq!(moved(Toward::Up), None);
    }

    #[test]
    fn displays_are_named_in_their_order_whatever_order_they_come_in() {
        let left = rect(-1920, 0, 1920, 1080);
        let right = rect(0, 0, 2560, 1440);
        assert_eq!(
            display_set_key(&[right, left]),
            "1920x1080@-1920,0;2560x1440@0,0"
        );
        assert_eq!(
            display_set_key(&[left, right]),
            display_set_key(&[right, left])
        );
        assert_eq!(ordered_displays(vec![right, left]), vec![left, right]);
    }

    #[test]
    fn the_places_of_one_arrangement_stay_when_another_is_kept() {
        let mut positions = Positions::default();
        let one = HashMap::from([("a".to_owned(), Spot::cell(1, 2, 0))]);
        let two = HashMap::from([("a".to_owned(), Spot::cell(0, 0, 1))]);
        assert!(positions.keep("one", &one));
        assert!(!positions.keep("one", &one));
        assert!(positions.keep("two", &two));
        assert_eq!(positions.of_set("one"), one);
        assert_eq!(positions.of_set("two"), two);
        assert!(positions.of_set("three").is_empty());
    }

    #[test]
    fn the_places_are_written_and_read_back() {
        let mut positions = Positions::default();
        let spots = HashMap::from([
            ("Report.docx".to_owned(), Spot::cell(0, 1, 0)),
            (
                "::{645FF040-5081-101B-9F08-00AA002F954E}".to_owned(),
                Spot {
                    column: 2.5,
                    row: 0.25,
                    display: 1,
                },
            ),
        ]);
        positions.keep("1920x1080@0,0;2560x1440@1920,0", &spots);
        let text = toml::to_string(&positions).unwrap();
        let read: Positions = toml::from_str(&text).unwrap();
        assert_eq!(read, positions);
        let file = std::env::temp_dir().join(format!("tessera-icons-{}.toml", std::process::id()));
        positions.save(&file).unwrap();
        assert_eq!(Positions::load(&file), positions);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(Positions::load(&file), Positions::default());
    }
}
