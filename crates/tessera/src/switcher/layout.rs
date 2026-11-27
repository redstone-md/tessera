// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How the switcher lays out the windows it offers: the styles of
//! `[switcher]` and the measures of their cells, in logical pixels.

use serde::Deserialize;

use crate::platform::ChoiceKey;

use super::choice::stepped;

/// How the switcher shows the windows, as `style` of `[switcher]` says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    /// Tiles of icon and title, eight in a row; with `previews`, larger
    /// tiles with a picture of the window above them, six in a row.
    #[default]
    Tiles,
    /// Large pictures of the windows, as many in a row and as large as
    /// the display gives room for.
    Thumbnails,
    /// One line for each window: its icon, its title and its program.
    List,
    /// Icons alone, in a row, with the title of the chosen window under
    /// them.
    Icons,
}

/// A tile of icon and title, the side of its icon, and how many stand in
/// a row at most.
const TILE: (f32, f32) = (136., 112.);
const ICON: f32 = 48.;
const COLUMNS: usize = 8;

/// A tile with a picture of its window, the room the picture has in it,
/// and how many stand in a row at most.
const PICTURE_TILE: (f32, f32) = (208., 172.);
const PICTURE: (f32, f32) = (192., 120.);
const PICTURE_COLUMNS: usize = 6;

/// The height of the line with the name under a picture, which is also
/// the height of the line of what is typed and of the title under the
/// icons, and the side of the icon in it.
pub const NAME_HEIGHT: f32 = 28.;
pub const SMALL_ICON: f32 = 20.;

/// The room of a picture of the `thumbnails` style at its largest, what
/// a cell has beside it (the name under it and the margins), and how far
/// it is made smaller at most to fit the display before the rows scroll.
const THUMBNAIL: (f32, f32) = (320., 200.);
const AROUND_THUMBNAIL: (f32, f32) = (PICTURE_TILE.0 - PICTURE.0, PICTURE_TILE.1 - PICTURE.1);
const SMALLEST_THUMBNAIL: f32 = 0.5;

/// A line of the `list` style, the side of its icon, and the distance
/// between lines.
const LINE: (f32, f32) = (480., 40.);
const LINE_ICON: f32 = 24.;
const LINE_GAP: f32 = 2.;

/// A cell of the `icons` style and the side of its icon.
const ICON_CELL: f32 = 80.;
const LARGE_ICON: f32 = 56.;

/// The measures of a switcher.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub style: Style,
    /// The width and the height of a cell.
    pub cell: (f32, f32),
    /// The room a picture of the window has in a cell, where the cells
    /// show pictures.
    pub picture: Option<(f32, f32)>,
    /// The side of the icon of a cell; under a picture, of the one beside
    /// the name.
    pub icon: f32,
    /// How many cells stand in a row.
    pub columns: usize,
    /// How many rows there are, and how many are seen at once; the
    /// others are scrolled to.
    pub rows: usize,
    pub seen_rows: usize,
    /// The distance between cells.
    pub gap: f32,
}

impl Layout {
    /// The measures of `count` cells of `style`, with pictures where
    /// `previews` asks for them in tiles, in `room`.
    pub fn new(style: Style, previews: bool, count: usize, room: (f32, f32), gap: f32) -> Self {
        let count = count.max(1);
        let (cell, picture, icon, most, gap) = match (style, previews) {
            (Style::Tiles, false) => (TILE, None, ICON, COLUMNS, gap),
            (Style::Tiles, true) => (
                PICTURE_TILE,
                Some(PICTURE),
                SMALL_ICON,
                PICTURE_COLUMNS,
                gap,
            ),
            (Style::Thumbnails, _) => {
                let (columns, scale) = thumbnail_columns(count, room, gap);
                let picture = (THUMBNAIL.0 * scale, THUMBNAIL.1 * scale);
                let cell = (
                    picture.0 + AROUND_THUMBNAIL.0,
                    picture.1 + AROUND_THUMBNAIL.1,
                );
                (cell, Some(picture), SMALL_ICON, columns, gap)
            }
            (Style::List, _) => (LINE, None, LINE_ICON, 1, LINE_GAP),
            (Style::Icons, _) => ((ICON_CELL, ICON_CELL), None, LARGE_ICON, usize::MAX, gap),
        };
        let columns = count.min(most).min(fitting(room.0, cell.0, gap)).max(1);
        let rows = count.div_ceil(columns);
        Self {
            style,
            cell,
            picture,
            icon,
            columns,
            rows,
            seen_rows: rows.min(fitting(room.1, cell.1, gap)),
            gap,
        }
    }

    /// The width and the height of the cells that are seen at once.
    pub fn grid(&self) -> (f32, f32) {
        let along = |cells: usize, length: f32| {
            cells as f32 * length + cells.saturating_sub(1) as f32 * self.gap
        };
        (
            along(self.columns, self.cell.0),
            along(self.seen_rows, self.cell.1),
        )
    }

    /// Where the cell in `column` and `row` begins, from the first cell
    /// that is seen; a fraction stands between two.
    pub fn place(&self, column: f32, row: f32) -> (f32, f32) {
        (
            column * (self.cell.0 + self.gap),
            row * (self.cell.1 + self.gap),
        )
    }

    /// The column and the row of the cell at `at`.
    pub fn cell_of(&self, at: usize) -> (usize, usize) {
        (at % self.columns, at / self.columns)
    }

    /// Where the choice goes from `at` of `count` cells with an arrow
    /// key: to the cell beside it, around the ends of the list, or to the
    /// one above or below it, where there is one. In a list, up and down
    /// go around the ends as well.
    pub fn moved(&self, at: usize, count: usize, key: ChoiceKey) -> usize {
        let columns = self.columns;
        match key {
            ChoiceKey::Left => stepped(at, count, true),
            ChoiceKey::Right => stepped(at, count, false),
            ChoiceKey::Up if columns == 1 => stepped(at, count, true),
            ChoiceKey::Down if columns == 1 => stepped(at, count, false),
            ChoiceKey::Up if at >= columns => at - columns,
            ChoiceKey::Down if at + columns < count => at + columns,
            _ => at,
        }
    }

    /// The first row seen once the row of `at` is brought into sight,
    /// the first being `first` before: it moves as little as it must.
    pub fn first_row(&self, first: usize, at: usize) -> usize {
        let (_, row) = self.cell_of(at);
        let seen = self.seen_rows.max(1);
        let first = match row {
            row if row < first => row,
            row if row >= first + seen => row + 1 - seen,
            _ => first,
        };
        first.min(self.rows.saturating_sub(seen))
    }

    /// The logical size of the picture of a window of `source` physical
    /// pixels on a display of `scale` in a cell: as large as its room
    /// allows, never enlarged.
    pub fn fitted(&self, (width, height): (i32, i32), scale: f32) -> Option<(f32, f32)> {
        let room = self.picture?;
        let (width, height) = (width as f32 / scale, height as f32 / scale);
        let factor = (room.0 / width).min(room.1 / height).min(1.);
        Some((width * factor, height * factor))
    }
}

/// How many cells of `length` stand in `room`, `gap` apart; one at
/// least. A cell that misses its room by a rounding error fits.
fn fitting(room: f32, length: f32, gap: f32) -> usize {
    (((room + gap) / (length + gap) + 1e-3).floor() as usize).max(1)
}

/// In how many columns `count` pictures of the `thumbnails` style are
/// largest in `room`, and how large they are then, as a share of their
/// largest; among columns that give the same size, the most. They are
/// never made smaller than [`SMALLEST_THUMBNAIL`]; the rows scroll then.
fn thumbnail_columns(count: usize, room: (f32, f32), gap: f32) -> (usize, f32) {
    let share = |cells: usize, room: f32, around: f32, largest: f32| {
        let cell = (room - cells.saturating_sub(1) as f32 * gap) / cells as f32;
        (cell - around) / largest
    };
    let mut best = (1, f32::MIN);
    for columns in 1..=count.max(1) {
        let rows = count.div_ceil(columns);
        let scale = share(columns, room.0, AROUND_THUMBNAIL.0, THUMBNAIL.0)
            .min(share(rows, room.1, AROUND_THUMBNAIL.1, THUMBNAIL.1))
            .min(1.);
        if scale >= best.1 {
            best = (columns, scale);
        }
    }
    (best.0, best.1.max(SMALLEST_THUMBNAIL))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: (f32, f32) = (1800., 1000.);

    #[test]
    fn tiles_fill_a_row_before_another_one_begins() {
        let tiles = |count| Layout::new(Style::Tiles, false, count, ROOM, 8.);
        assert_eq!((tiles(1).columns, tiles(1).rows), (1, 1));
        assert_eq!((tiles(8).columns, tiles(8).rows), (8, 1));
        assert_eq!((tiles(9).columns, tiles(9).rows), (8, 2));
        assert_eq!((tiles(40).columns, tiles(40).rows), (8, 5));
        assert_eq!(tiles(0).columns, 1);
        let pictured = Layout::new(Style::Tiles, true, 9, ROOM, 8.);
        assert_eq!((pictured.columns, pictured.rows), (6, 2));
        assert_eq!(tiles(2).grid(), (2. * 136. + 8., 112.));
        assert_eq!(pictured.grid(), (6. * 208. + 5. * 8., 2. * 172. + 8.));
    }

    #[test]
    fn rows_that_do_not_fit_are_scrolled_to() {
        // Seven rows of pictured tiles are higher than the room.
        let pictured = Layout::new(Style::Tiles, true, 40, ROOM, 8.);
        assert_eq!((pictured.rows, pictured.seen_rows), (7, 5));
        assert_eq!(pictured.first_row(0, 5), 0);
        assert_eq!(pictured.first_row(0, 30), 1);
        assert_eq!(pictured.first_row(2, 39), 2);
        assert_eq!(pictured.first_row(2, 0), 0);
        assert_eq!(pictured.first_row(9, 0), 0);
        let narrow = Layout::new(Style::Tiles, false, 10, (300., 1000.), 8.);
        assert_eq!((narrow.columns, narrow.rows), (2, 5));
    }

    #[test]
    fn thumbnails_are_as_large_as_the_room_gives() {
        let few = Layout::new(Style::Thumbnails, false, 3, ROOM, 8.);
        assert_eq!((few.columns, few.rows), (3, 1));
        assert_eq!(few.picture, Some(THUMBNAIL));
        let many = Layout::new(Style::Thumbnails, false, 20, ROOM, 8.);
        let (width, height) = many.picture.unwrap();
        assert!(width < THUMBNAIL.0 && width >= THUMBNAIL.0 * SMALLEST_THUMBNAIL);
        assert!((width / height - THUMBNAIL.0 / THUMBNAIL.1).abs() < 1e-4);
        let (grid_width, grid_height) = many.grid();
        assert!(grid_width <= ROOM.0 + 0.01 && grid_height <= ROOM.1 + 0.01);
        assert_eq!(many.seen_rows, many.rows);
        for count in 1..=40 {
            let layout = Layout::new(Style::Thumbnails, false, count, ROOM, 8.);
            assert!(layout.columns * layout.rows >= count);
            assert!(layout.grid().0 <= ROOM.0 + 0.01);
        }
        // On a small display they stop shrinking and scroll.
        let crowded = Layout::new(Style::Thumbnails, false, 40, (800., 500.), 8.);
        assert_eq!(crowded.picture.unwrap().0, THUMBNAIL.0 * SMALLEST_THUMBNAIL);
        assert!(crowded.seen_rows < crowded.rows);
    }

    #[test]
    fn a_list_has_a_line_for_each_window() {
        let list = Layout::new(Style::List, true, 12, ROOM, 8.);
        assert_eq!((list.columns, list.rows), (1, 12));
        assert_eq!(list.picture, None);
        assert_eq!(list.moved(0, 12, ChoiceKey::Up), 11);
        assert_eq!(list.moved(11, 12, ChoiceKey::Down), 0);
        assert_eq!(list.moved(4, 12, ChoiceKey::Down), 5);
        let long = Layout::new(Style::List, false, 40, (480., 400.), 8.);
        assert_eq!(long.seen_rows, 9);
    }

    #[test]
    fn icons_stand_in_one_row_while_the_display_is_wide_enough() {
        let icons = Layout::new(Style::Icons, false, 15, ROOM, 8.);
        assert_eq!((icons.columns, icons.rows), (15, 1));
        let more = Layout::new(Style::Icons, false, 30, ROOM, 8.);
        assert_eq!((more.columns, more.rows), (20, 2));
    }

    #[test]
    fn arrows_go_beside_and_to_the_row_above_or_below() {
        let tiles = Layout::new(Style::Tiles, false, 11, ROOM, 8.);
        assert_eq!(tiles.moved(0, 11, ChoiceKey::Left), 10);
        assert_eq!(tiles.moved(10, 11, ChoiceKey::Right), 0);
        assert_eq!(tiles.moved(1, 11, ChoiceKey::Down), 9);
        assert_eq!(tiles.moved(5, 11, ChoiceKey::Down), 5);
        assert_eq!(tiles.moved(9, 11, ChoiceKey::Up), 1);
        assert_eq!(tiles.moved(1, 11, ChoiceKey::Up), 1);
    }

    #[test]
    fn a_picture_fills_its_room_and_is_never_enlarged() {
        let tiles = Layout::new(Style::Tiles, true, 1, ROOM, 8.);
        assert_eq!(tiles.fitted((1920, 1080), 1.), Some((192., 108.)));
        assert_eq!(tiles.fitted((600, 1200), 1.), Some((60., 120.)));
        assert_eq!(tiles.fitted((100, 50), 1.), Some((100., 50.)));
        assert_eq!(tiles.fitted((200, 100), 2.), Some((100., 50.)));
        let plain = Layout::new(Style::Tiles, false, 1, ROOM, 8.);
        assert_eq!(plain.fitted((1920, 1080), 1.), None);
    }
}
