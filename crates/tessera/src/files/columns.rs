// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The columns of the details of a folder as data: which are shown, in
//! which order and how wide, as the user chooses them from the menu of
//! the headings, drags their edges and drags them among each other. Each
//! folder keeps its own, in the store of the file manager.

use serde::{Deserialize, Serialize};

use super::model::{Column, Location};
use crate::platform::FileProperty;

/// The narrowest and the widest a column is made by dragging its edge,
/// in logical pixels.
pub const NARROWEST: f32 = 48.;
pub const WIDEST: f32 = 900.;

/// A column shown, with how wide it is; the name takes what the others
/// leave and has no width of its own.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShownColumn {
    pub column: Column,
    pub width: f32,
}

impl ShownColumn {
    pub fn new(column: Column) -> Self {
        Self {
            column,
            width: column.first_width(),
        }
    }
}

/// The columns of the details of a location, in order; the name is
/// always there, first.
#[derive(Clone, Debug, PartialEq)]
pub struct Columns {
    shown: Vec<ShownColumn>,
}

impl Columns {
    /// `columns` at the widths they start at, the name first.
    pub fn of(columns: &[Column]) -> Self {
        let mut shown = vec![ShownColumn::new(Column::Name)];
        shown.extend(
            { columns.iter() }
                .filter(|column| **column != Column::Name)
                .map(|column| ShownColumn::new(*column)),
        );
        Self { shown }
    }

    /// The columns a location shows before the user chose others.
    pub fn first_of(location: &Location) -> Self {
        match location {
            Location::Search { .. } => Self::of(&Column::FOUND),
            Location::RecycleBin => Self::of(&Column::BIN),
            _ => Self::of(&Column::ALL),
        }
    }

    /// The columns kept, as they were kept: unknown ones are already left
    /// out, the name is put first and nothing is shown twice.
    pub fn kept(kept: Vec<ShownColumn>) -> Self {
        let mut columns = Self::of(&[]);
        for shown in kept {
            if !columns.has(shown.column) {
                columns.shown.push(ShownColumn {
                    width: shown.width.clamp(NARROWEST, WIDEST),
                    ..shown
                });
            }
        }
        columns
    }

    pub fn shown(&self) -> &[ShownColumn] {
        &self.shown
    }

    pub fn has(&self, column: Column) -> bool {
        self.shown.iter().any(|shown| shown.column == column)
    }

    /// Shows `column` at the end, or takes it away where it is shown;
    /// the name stays.
    pub fn toggle(&mut self, column: Column) {
        if column == Column::Name {
            return;
        }
        match self.shown.iter().position(|shown| shown.column == column) {
            Some(at) => {
                self.shown.remove(at);
            }
            None => self.shown.push(ShownColumn::new(column)),
        }
    }

    /// Makes the column at `index` `width` wide, within the widths a
    /// column may have; the name has none to make.
    pub fn resize(&mut self, index: usize, width: f32) {
        if let Some(shown) = self.shown.get_mut(index)
            && shown.column != Column::Name
        {
            shown.width = width.clamp(NARROWEST, WIDEST);
        }
    }

    /// Moves the column at `from` to the place of the one at `to`; the
    /// name stays first.
    pub fn reorder(&mut self, from: usize, to: usize) {
        let last = self.shown.len().saturating_sub(1);
        if from == 0 || from > last || from == to {
            return;
        }
        let moved = self.shown.remove(from);
        self.shown.insert(to.clamp(1, last), moved);
    }

    /// The properties of the Shell the columns show.
    pub fn properties(&self) -> Vec<FileProperty> {
        { self.shown.iter() }
            .filter_map(|shown| shown.column.property())
            .collect()
    }

    /// The columns with the state of cloud files after the name, where
    /// they lack it: what a folder of the cloud shows.
    pub fn with_status(&self) -> Self {
        let mut columns = self.clone();
        if !columns.has(Column::Status) {
            columns.shown.insert(1, ShownColumn::new(Column::Status));
        }
        columns
    }
}

/// The columns the menu of the headings offers to show or hide, in its
/// order: those of the file system, then the properties of the Shell.
pub fn choosable() -> Vec<Column> {
    let mut columns = vec![Column::Modified, Column::Kind, Column::Size, Column::Status];
    columns.extend(FileProperty::COLUMNS.map(Column::Property));
    columns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(columns: &Columns) -> Vec<Column> {
        columns.shown().iter().map(|shown| shown.column).collect()
    }

    #[test]
    fn the_name_stays_first_whatever_is_done() {
        let mut columns = Columns::first_of(&Location::Computer);
        columns.toggle(Column::Name);
        columns.reorder(0, 2);
        columns.reorder(3, 0);
        assert_eq!(names(&columns)[0], Column::Name);
        assert_eq!(
            names(&columns),
            [Column::Name, Column::Size, Column::Modified, Column::Kind]
        );
    }

    #[test]
    fn a_column_is_shown_and_hidden_and_kept_within_its_widths() {
        let taken = Column::Property(FileProperty::DateTaken);
        let mut columns = Columns::of(&Column::ALL);
        columns.toggle(taken);
        assert!(columns.has(taken));
        assert_eq!(columns.properties(), [FileProperty::DateTaken]);
        columns.resize(4, 5.);
        assert_eq!(columns.shown()[4].width, NARROWEST);
        columns.resize(0, 500.);
        assert_eq!(columns.shown()[0].width, 0.);
        columns.toggle(taken);
        assert!(!columns.has(taken));
    }

    #[test]
    fn kept_columns_are_mended() {
        let kept = vec![
            ShownColumn {
                column: Column::Size,
                width: 10_000.,
            },
            ShownColumn::new(Column::Size),
            ShownColumn::new(Column::Name),
        ];
        let columns = Columns::kept(kept);
        assert_eq!(names(&columns), [Column::Name, Column::Size]);
        assert_eq!(columns.shown()[1].width, WIDEST);
        let status = Columns::of(&Column::ALL).with_status();
        assert_eq!(names(&status)[1], Column::Status);
    }

    #[test]
    fn a_shown_column_is_written_with_its_name() {
        let shown = ShownColumn::new(Column::Property(FileProperty::Length));
        let text = toml::to_string(&shown).expect("a column can be written");
        assert!(text.contains("column = \"length\""));
        let read: ShownColumn = toml::from_str(&text).expect("and read");
        assert_eq!(read, shown);
    }
}
