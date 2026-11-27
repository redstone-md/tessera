// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Changes to the wallpaper in the configuration file, as the wallpaper
//! settings and the programs that set the wallpaper make them. The rest of
//! the file, its comments included, stays as it is written.
//!
//! [`write`] and [`read`] read and write the file: they are for a thread
//! other than that of the interface.

use std::path::PathBuf;

use toml_edit::{Array, Value};

use super::WallpaperConfig;
use crate::config::edit::ConfigFile;

/// A change to the wallpaper in the configuration.
#[derive(Clone, Debug)]
pub enum Edit {
    /// A key of `[wallpaper]`, set or taken away.
    Key(&'static str, Option<Value>),
    /// A key of the entry `[[wallpaper.display]]` with a number.
    Display(usize, &'static str, Option<Value>),
    /// A key taken away from every `[[wallpaper.display]]`.
    EveryDisplay(&'static str),
    /// A collection with a name and its pictures and folders.
    AddCollection(String, Vec<PathBuf>),
    /// The collection at a place taken away, and every choice of it.
    RemoveCollection(usize),
    /// The collection at a place moved one on, or with `false` one back.
    MoveCollection(usize, bool),
}

impl Edit {
    /// Makes the change in `file`; whether it could be made.
    pub fn apply(self, file: &mut ConfigFile) -> bool {
        match self {
            Self::Key(key, value) => file.set_wallpaper(key, value),
            Self::Display(number, key, value) => {
                file.set_wallpaper_place("display", number, key, value)
            }
            Self::EveryDisplay(key) => {
                file.clear_wallpaper_places("display", key);
                true
            }
            Self::AddCollection(name, images) => file.add_wallpaper_collection(&name, &images),
            Self::RemoveCollection(index) => file.remove_wallpaper_collection(index),
            Self::MoveCollection(index, forward) => file.move_wallpaper_collection(index, forward),
        }
    }

    /// Whether this and `other` set the same key, so that the later one
    /// replaces the earlier.
    pub fn same_key(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Key(one, _), Self::Key(two, _)) => one == two,
            (Self::Display(one, key, _), Self::Display(two, other_key, _)) => {
                one == two && key == other_key
            }
            _ => false,
        }
    }
}

/// `paths` as a list of the file.
pub fn paths_value(paths: &[PathBuf]) -> Value {
    let list: Array = { paths.iter() }
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    Value::Array(list)
}

/// Makes `edits` in the configuration file, writes it and gives the
/// configuration of the wallpaper it holds then. A change that does not
/// fit the file is left out; a file that would not be usable is not
/// written.
pub fn write(edits: Vec<Edit>) -> Result<WallpaperConfig, String> {
    let mut file = ConfigFile::open()?;
    for edit in edits {
        edit.apply(&mut file);
    }
    file.save()?;
    Ok(file.config()?.wallpaper)
}

/// The configuration of the wallpaper the file holds.
pub fn read() -> Result<WallpaperConfig, String> {
    Ok(ConfigFile::open()?.config()?.wallpaper)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_later_change_of_a_key_replaces_the_earlier() {
        let blur = Edit::Key("blur", Some(4.into()));
        assert!(blur.same_key(&Edit::Key("blur", None)));
        assert!(!blur.same_key(&Edit::Key("contrast", None)));
        let display = Edit::Display(2, "blur", None);
        assert!(display.same_key(&Edit::Display(2, "blur", Some(1.into()))));
        assert!(!display.same_key(&Edit::Display(1, "blur", None)));
        assert!(!display.same_key(&blur));
        assert!(!Edit::RemoveCollection(0).same_key(&Edit::RemoveCollection(0)));
    }
}
