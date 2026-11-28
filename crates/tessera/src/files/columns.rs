// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The columns of the details of a folder as data: which are shown, in
//! which order and how wide, as the user chooses them from the menu of
//! the headings, drags their edges and drags them among each other. Each
//! folder keeps its own, in the store of the file manager.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use super::model::{Column, Location};
use crate::platform::{FileProperty, known_folder_path};

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

    /// The columns a location shows before the user chose others: those
    /// Explorer gives the type of a folder of music, pictures or videos.
    pub fn first_of(location: &Location) -> Self {
        match location {
            Location::Search { .. } => Self::of(&Column::FOUND),
            Location::RecycleBin => Self::of(&Column::BIN),
            Location::Folder(path) => match FolderType::of(path) {
                Some(kind) => Self::of(&kind.columns()),
                None => Self::of(&Column::ALL),
            },
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

    /// The properties of the Shell the columns show; the state of cloud
    /// files reads those its sync provider gives, beside the attributes.
    pub fn properties(&self) -> Vec<FileProperty> {
        let mut properties: Vec<FileProperty> = { self.shown.iter() }
            .filter_map(|shown| shown.column.property())
            .collect();
        if self.has(Column::Status) {
            properties.extend([FileProperty::SyncState, FileProperty::Sharing]);
        }
        properties
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

/// The further columns "More…" of the menu of the headings offers.
pub fn more_choosable() -> Vec<Column> {
    FileProperty::MORE.map(Column::Property).to_vec()
}

/// The types of folders Explorer gives columns of their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderType {
    Music,
    Pictures,
    Videos,
}

/// The known folders of the user, by type, once they were looked up.
static KNOWN_FOLDERS: OnceLock<Vec<(PathBuf, FolderType)>> = OnceLock::new();
static LOOKING_UP: AtomicBool = AtomicBool::new(false);

impl FolderType {
    /// The columns a folder of the type starts with, as Explorer's: for
    /// music the number, title, artists and album; for pictures the day
    /// taken, the tags, the size and the rating; for videos the date and
    /// the length.
    pub fn columns(self) -> Vec<Column> {
        let property = Column::Property;
        match self {
            Self::Music => vec![
                Column::Name,
                property(FileProperty::Track),
                property(FileProperty::Title),
                property(FileProperty::Artists),
                property(FileProperty::Album),
            ],
            Self::Pictures => vec![
                Column::Name,
                property(FileProperty::DateTaken),
                property(FileProperty::Tags),
                Column::Size,
                property(FileProperty::Rating),
            ],
            Self::Videos => vec![
                Column::Name,
                Column::Modified,
                property(FileProperty::Length),
            ],
        }
    }

    /// The type of the folder at `path`: that of the known folder of the
    /// user (Music, Pictures, Videos) it is or lies in. The known folders
    /// are looked up once, away from the user interface; until they are,
    /// no folder has a type.
    pub fn of(path: &Path) -> Option<Self> {
        match KNOWN_FOLDERS.get() {
            Some(known) => type_below(known, path),
            None => {
                if !LOOKING_UP.swap(true, Ordering::Relaxed) {
                    let looked = std::thread::Builder::new()
                        .name("tessera-folder-types".to_owned())
                        .spawn(|| {
                            let known: Vec<(PathBuf, FolderType)> = [
                                ("My Music", Self::Music),
                                ("My Pictures", Self::Pictures),
                                ("My Video", Self::Videos),
                            ]
                            .into_iter()
                            .filter_map(|(name, kind)| Some((known_folder_path(name)?, kind)))
                            .collect();
                            KNOWN_FOLDERS.set(known).ok();
                        });
                    if let Err(error) = looked {
                        report!("the types of folders are not looked up: {error}");
                    }
                }
                None
            }
        }
    }
}

/// The type of the known folder of `known` that `path` is or lies in, the
/// nearest first.
fn type_below(known: &[(PathBuf, FolderType)], path: &Path) -> Option<FolderType> {
    let key = |path: &Path| path.to_string_lossy().trim_end_matches('\\').to_lowercase();
    let path = key(path);
    { known.iter() }
        .filter(|(folder, _)| {
            let folder = key(folder);
            path == folder || path.starts_with(&format!("{folder}\\"))
        })
        .max_by_key(|(folder, _)| folder.as_os_str().len())
        .map(|(_, kind)| *kind)
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
    fn folders_of_music_pictures_and_videos_have_their_columns() {
        let known = [
            (PathBuf::from(r"C:\Users\Ann\Music"), FolderType::Music),
            (
                PathBuf::from(r"C:\Users\Ann\Pictures"),
                FolderType::Pictures,
            ),
            (
                PathBuf::from(r"C:\Users\Ann\Pictures\Videos"),
                FolderType::Videos,
            ),
        ];
        let at = |path: &str| type_below(&known, Path::new(path));
        assert_eq!(at(r"C:\Users\Ann\Music"), Some(FolderType::Music));
        assert_eq!(at(r"c:\users\ann\music\Album\"), Some(FolderType::Music));
        assert_eq!(at(r"C:\Users\Ann\Musical"), None);
        assert_eq!(
            at(r"C:\Users\Ann\Pictures\Videos\2026"),
            Some(FolderType::Videos)
        );
        assert_eq!(
            at(r"C:\Users\Ann\Pictures\Trip"),
            Some(FolderType::Pictures)
        );
        let music = Columns::of(&FolderType::Music.columns());
        assert_eq!(names(&music)[1], Column::Property(FileProperty::Track));
        // The state of cloud files reads the states its provider gives.
        assert!(music.properties().contains(&FileProperty::Album));
        let status = Columns::of(&Column::ALL).with_status();
        assert!(status.properties().contains(&FileProperty::SyncState));
        assert!(
            more_choosable()
                .iter()
                .all(|column| !choosable().contains(column))
        );
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
