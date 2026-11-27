// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The places the user keeps at the side of the file manager, apart from
//! Quick access, whose pins stay Explorer's own: any folder or place of
//! the Shell, in groups (Places, Remote, Devices, Searches), renamed,
//! ordered by dragging, removed, and the groups shown or hidden.
//!
//! They are kept in `files-places.toml` of the configuration directory
//! (`%APPDATA%\tessera`), which the file manager alone writes. Every
//! window shares them: a change made in one is drawn in all, and written
//! away from the user interface.

use std::path::PathBuf;

use gpui::{App, Global};
use serde::{Deserialize, Serialize};

use super::model::KnownShellPlace;

/// The file the places are kept in.
const PLACES_FILE: &str = "files-places.toml";

/// A group of places at the side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaceGroup {
    /// Places of this computer: folders, Control Panel, the libraries.
    Places,
    /// Places of other computers: Network, shares, FTP and WebDAV sites.
    Remote,
    /// Phones, cameras and other devices.
    Devices,
    /// Saved searches.
    Searches,
}

impl PlaceGroup {
    pub const ALL: [Self; 4] = [Self::Places, Self::Remote, Self::Devices, Self::Searches];

    /// The group a place is kept in, by how the name it is known by is
    /// written: the places of other computers are remote, saved searches
    /// are searches, the rest are places.
    pub fn of_location(name: &str) -> Self {
        let lower = name.trim().to_ascii_lowercase();
        let network = KnownShellPlace::of_word("network")
            .is_some_and(|network| network.parsing.eq_ignore_ascii_case(&lower));
        if lower.starts_with("\\\\") || lower.contains("://") || lower == "network" || network {
            return Self::Remote;
        }
        if lower.ends_with(".search-ms") || lower.starts_with("search-ms:") {
            return Self::Searches;
        }
        Self::Places
    }

    /// Its heading, a text of the RUSSIAN table.
    pub fn title(self) -> &'static str {
        match self {
            Self::Places => "Places",
            Self::Remote => "Remote",
            Self::Devices => "Devices",
            Self::Searches => "Searches",
        }
    }
}

/// A place kept at the side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeptPlace {
    /// What it is called at the side.
    pub name: String,
    /// Where it leads, as `Files::named` reads it: a path, a word such as
    /// `control-panel`, or a parsing name of the Shell.
    pub location: String,
    pub group: PlaceGroup,
}

/// The places kept, and the groups hidden.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Places {
    #[serde(rename = "place")]
    pub places: Vec<KeptPlace>,
    pub hidden: Vec<PlaceGroup>,
}

impl Default for Places {
    /// The places Explorer has at its side beyond Quick access and This
    /// PC: Control Panel, the libraries and the Gallery, and Network.
    fn default() -> Self {
        let known = |word: &str, group: PlaceGroup| {
            let place = KnownShellPlace::of_word(word)?;
            Some(KeptPlace {
                name: place.title.to_owned(),
                location: word.to_owned(),
                group,
            })
        };
        let places = [
            known("gallery", PlaceGroup::Places),
            known("libraries", PlaceGroup::Places),
            known("control-panel", PlaceGroup::Places),
            known("network", PlaceGroup::Remote),
        ];
        Self {
            places: places.into_iter().flatten().collect(),
            hidden: Vec::new(),
        }
    }
}

impl Places {
    /// The places of `group`, with their places in the whole list.
    pub fn of(&self, group: PlaceGroup) -> Vec<(usize, &KeptPlace)> {
        { self.places.iter().enumerate() }
            .filter(|(_, place)| place.group == group)
            .collect()
    }

    /// Adds `place` at the end of its group, unless a place leads there
    /// already; says whether it was added.
    pub fn add(&mut self, place: KeptPlace) -> bool {
        let there =
            { self.places.iter() }.any(|kept| kept.location.eq_ignore_ascii_case(&place.location));
        if there {
            return false;
        }
        let after = { self.places.iter() }.rposition(|kept| kept.group == place.group);
        match after {
            Some(index) => self.places.insert(index + 1, place),
            None => self.places.push(place),
        }
        true
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.places.len() {
            self.places.remove(index);
        }
    }

    /// Calls the place at `index` `name`; an empty name changes nothing.
    pub fn rename(&mut self, index: usize, name: &str) {
        let name = name.trim();
        if let (Some(place), false) = (self.places.get_mut(index), name.is_empty()) {
            place.name = name.to_owned();
        }
    }

    /// Puts the place at `from` where the place at `to` is, in the group
    /// of that one, as a drop onto it does.
    pub fn move_to(&mut self, from: usize, to: usize) {
        if from >= self.places.len() || to >= self.places.len() || from == to {
            return;
        }
        let group = self.places[to].group;
        let mut place = self.places.remove(from);
        place.group = group;
        self.places.insert(to, place);
    }

    /// Whether `group` is shown at the side.
    pub fn shows(&self, group: PlaceGroup) -> bool {
        !self.hidden.contains(&group)
    }

    /// Shows `group`, or hides it.
    pub fn show(&mut self, group: PlaceGroup, shown: bool) {
        self.hidden.retain(|hidden| *hidden != group);
        if !shown {
            self.hidden.push(group);
        }
    }

    /// The places as their file has them, or the first ones where there is
    /// no file or it cannot be read. Blocks on the disk.
    fn read() -> Self {
        let Some(path) = places_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|error| {
                report!("{} cannot be read: {error}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Writes the places to their file, through a file beside it. Blocks
    /// on the disk.
    fn write(&self) {
        let Some(path) = places_path() else {
            return;
        };
        let text = match toml::to_string(self) {
            Ok(text) => text,
            Err(error) => return report!("the places are not kept: {error}"),
        };
        let fresh = path.with_extension("toml.new");
        let written = { path.parent().map(std::fs::create_dir_all) }
            .unwrap_or(Ok(()))
            .and_then(|()| std::fs::write(&fresh, text))
            .and_then(|()| std::fs::rename(&fresh, &path));
        if let Err(error) = written {
            std::fs::remove_file(&fresh).ok();
            report!("the places are not kept in {}: {error}", path.display());
        }
    }
}

/// Where the places are kept.
fn places_path() -> Option<PathBuf> {
    Some(crate::config::config_dir()?.join(PLACES_FILE))
}

/// The places every window of the file manager shares, once read; the
/// sides observe it and draw anew when it changes.
#[derive(Default)]
pub struct KeptPlaces {
    places: Places,
    asked: bool,
    loaded: bool,
}

impl Global for KeptPlaces {}

impl KeptPlaces {
    /// Reads the places once, away from the user interface.
    pub fn load(cx: &mut App) {
        let kept = cx.default_global::<Self>();
        if kept.asked {
            return;
        }
        kept.asked = true;
        cx.spawn(async move |cx| {
            let places = cx
                .background_executor()
                .spawn(async { Places::read() })
                .await;
            cx.update(|cx| {
                let kept = cx.default_global::<Self>();
                kept.places = places;
                kept.loaded = true;
            })
            .ok();
        })
        .detach();
    }

    /// The places, where they were read.
    pub fn get(cx: &App) -> Option<&Places> {
        { cx.try_global::<Self>() }
            .filter(|kept| kept.loaded)
            .map(|kept| &kept.places)
    }

    /// Changes the places with `change` and keeps them, once they were
    /// read.
    pub fn change(cx: &mut App, change: impl FnOnce(&mut Places)) {
        let kept = cx.default_global::<Self>();
        if !kept.loaded {
            return;
        }
        change(&mut kept.places);
        let places = kept.places.clone();
        cx.background_executor()
            .spawn(async move { places.write() })
            .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(name: &str, group: PlaceGroup) -> KeptPlace {
        KeptPlace {
            name: name.to_owned(),
            location: format!(r"C:\{name}"),
            group,
        }
    }

    #[test]
    fn the_first_places_are_those_explorer_has() {
        let places = Places::default();
        let words: Vec<&str> = places
            .places
            .iter()
            .map(|place| place.location.as_str())
            .collect();
        assert_eq!(words, ["gallery", "libraries", "control-panel", "network"]);
        assert_eq!(places.of(PlaceGroup::Remote).len(), 1);
        assert!(PlaceGroup::ALL.iter().all(|group| places.shows(*group)));
    }

    #[test]
    fn a_place_is_added_once_at_the_end_of_its_group() {
        let mut places = Places {
            places: vec![
                place("a", PlaceGroup::Places),
                place("r", PlaceGroup::Remote),
            ],
            hidden: Vec::new(),
        };
        assert!(places.add(place("b", PlaceGroup::Places)));
        assert!(!places.add(place("B", PlaceGroup::Places)));
        let names: Vec<&str> = places
            .places
            .iter()
            .map(|place| place.name.as_str())
            .collect();
        assert_eq!(names, ["a", "b", "r"]);
    }

    #[test]
    fn places_are_renamed_moved_removed_and_their_groups_hidden() {
        let mut places = Places {
            places: vec![
                place("a", PlaceGroup::Places),
                place("b", PlaceGroup::Places),
                place("r", PlaceGroup::Remote),
            ],
            hidden: Vec::new(),
        };
        places.rename(0, " Work ");
        places.rename(1, "  ");
        assert_eq!(places.places[0].name, "Work");
        assert_eq!(places.places[1].name, "b");
        places.move_to(0, 2);
        assert_eq!(places.places[2].name, "Work");
        assert_eq!(places.places[2].group, PlaceGroup::Remote);
        places.remove(9);
        places.remove(0);
        assert_eq!(places.places.len(), 2);
        places.show(PlaceGroup::Devices, false);
        assert!(!places.shows(PlaceGroup::Devices));
        places.show(PlaceGroup::Devices, true);
        assert!(places.shows(PlaceGroup::Devices));
    }

    #[test]
    fn a_place_goes_into_the_group_its_name_says() {
        assert_eq!(PlaceGroup::of_location(r"\\nas\music"), PlaceGroup::Remote);
        assert_eq!(
            PlaceGroup::of_location("ftp://example.org"),
            PlaceGroup::Remote
        );
        assert_eq!(PlaceGroup::of_location("network"), PlaceGroup::Remote);
        assert_eq!(
            PlaceGroup::of_location(r"C:\Users\me\Searches\Notes.search-ms"),
            PlaceGroup::Searches
        );
        assert_eq!(PlaceGroup::of_location(r"C:\Work"), PlaceGroup::Places);
        assert_eq!(PlaceGroup::of_location("control-panel"), PlaceGroup::Places);
    }

    #[test]
    fn the_places_file_is_read_back_as_written() {
        let mut places = Places::default();
        places.show(PlaceGroup::Searches, false);
        let text = toml::to_string(&places).expect("the places are written");
        assert_eq!(toml::from_str::<Places>(&text).expect("and read"), places);
        assert!(toml::from_str::<Places>("colour = 1").is_err());
    }
}
