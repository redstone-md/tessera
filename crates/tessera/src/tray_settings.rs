// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Which icons of the notification area stand in the bar, which wait in
//! its overflow, and in what order, under `[tray]`.
//!
//! An icon is named by its global identifier where its application gave
//! one, as `{7820AE78-23E3-4229-82C1-E41CB67D5B9C}`, and otherwise by its
//! program and the number the program gave it, as
//! `C:/Program Files/App/app.exe#1`. An entry may also name every icon of
//! a program, by its path or only its file name (`app.exe`), or one of
//! them by file name and number (`app.exe#2`). Where several entries name
//! an icon, the one that names it most closely decides.
//!
//! An icon no entry places stands where the user put it in the settings of
//! Windows (Taskbar > Other system tray icons), and an icon Windows has no
//! choice for either stands where `new` says. Moving an icon in the strip
//! or its overflow writes its own name into `shown` or `hidden`, and
//! dragging writes `order`.

use std::path::Path;

use gpui::{App, AsyncApp, Global};
use serde::Deserialize;
use toml_edit::{Array, Value};

use crate::config::edit::ConfigFile;
use crate::shell::Shell;

/// The table of the configuration this is kept in.
const TABLE: &str = "tray";

/// What the configuration says about the icons, under `[tray]`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrayConfig {
    /// The icons that stand in the bar.
    #[serde(default)]
    pub shown: Vec<String>,
    /// The icons that wait in the overflow.
    #[serde(default)]
    pub hidden: Vec<String>,
    /// The order of the icons, in the bar and in the overflow alike; those
    /// it does not name follow in the order they came.
    #[serde(default)]
    pub order: Vec<String>,
    /// Where an icon goes that neither these lists nor the settings of
    /// Windows place.
    #[serde(default)]
    pub new: NewIcons,
}

/// Where new icons go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NewIcons {
    #[default]
    Shown,
    Hidden,
}

/// Where an icon stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// In the strip of the bar.
    Bar,
    /// In the overflow, which the button at the end of the strip opens.
    Overflow,
}

/// How closely an entry names an icon: more is closer.
type Closeness = u8;

/// The names an icon answers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconKey {
    /// The name written for it, as the user may read it.
    written: String,
    /// The written name as it is compared: lower case, forward slashes.
    exact: String,
    /// The program, compared as above.
    path: Option<String>,
    /// The file name of the program, compared as above.
    file: Option<String>,
    uid: u32,
}

impl IconKey {
    /// The names of an icon with the identifier `guid`, of the program
    /// `executable`, added for a window of the class `class` with the
    /// number `uid`. Without a program the class stands in its place.
    pub fn new(guid: Option<[u8; 16]>, executable: Option<&Path>, class: &str, uid: u32) -> Self {
        let path = executable.map(|path| path.to_string_lossy().replace('\\', "/"));
        let written = match (&guid, &path) {
            (Some(guid), _) => guid_text(guid),
            (None, Some(path)) => format!("{path}#{uid}"),
            (None, None) => format!("class:{class}#{uid}"),
        };
        let file = path
            .as_deref()
            .and_then(|path| path.rsplit('/').next())
            .map(str::to_lowercase);
        Self {
            exact: comparable(&written),
            written,
            path: path.as_deref().map(comparable),
            file,
            uid,
        }
    }

    /// The name written into the configuration for this icon.
    pub fn as_str(&self) -> &str {
        &self.written
    }

    /// How closely `entry` names this icon; `None` if it does not.
    fn closeness(&self, entry: &str) -> Option<Closeness> {
        let entry = comparable(entry);
        if entry == self.exact {
            return Some(3);
        }
        match entry.rsplit_once('#') {
            Some((program, uid)) => {
                let same = uid.parse() == Ok(self.uid);
                let named =
                    Some(program) == self.file.as_deref() || Some(program) == self.path.as_deref();
                (same && named).then_some(2)
            }
            None if Some(entry.as_str()) == self.path.as_deref() => Some(2),
            None if Some(entry.as_str()) == self.file.as_deref() => Some(1),
            None => None,
        }
    }

    /// Whether `entry` names this icon exactly, as a move writes it.
    fn is_named_by(&self, entry: &str) -> bool {
        self.closeness(entry) == Some(3)
    }

    /// The closest any of `entries` names this icon, and the first entry
    /// that does so.
    fn closest(&self, entries: &[String]) -> Option<(Closeness, usize)> {
        let found = entries.iter().enumerate();
        let found = found.filter_map(|(index, entry)| Some((self.closeness(entry)?, index)));
        found.fold(None, |best, (closeness, index)| match best {
            Some(kept) if kept.0 >= closeness => Some(kept),
            _ => Some((closeness, index)),
        })
    }
}

/// `text` as names are compared: lower case, with forward slashes.
fn comparable(text: &str) -> String {
    text.trim().replace('\\', "/").to_lowercase()
}

/// The identifier `guid`, laid out as Windows lays out a `GUID`, written
/// as `{01234567-89AB-CDEF-0123-456789ABCDEF}`.
pub fn guid_text(guid: &[u8; 16]) -> String {
    let data1 = u32::from_le_bytes([guid[0], guid[1], guid[2], guid[3]]);
    let data2 = u16::from_le_bytes([guid[4], guid[5]]);
    let data3 = u16::from_le_bytes([guid[6], guid[7]]);
    let tail: String = guid[10..]
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect();
    format!(
        "{{{data1:08X}-{data2:04X}-{data3:04X}-{:02X}{:02X}-{tail}}}",
        guid[8], guid[9]
    )
}

/// What Windows says of an icon, as [`windows_choice`] reads it.
pub struct WindowsChoice<'a> {
    pub guid: Option<[u8; 16]>,
    pub executable: Option<&'a Path>,
    pub uid: Option<u32>,
    /// Whether the user put the icon in the taskbar, if the user chose.
    pub promoted: Option<bool>,
}

/// Whether the settings of Windows put the icon with `guid`, of the
/// program `executable` with the number `uid`, in the taskbar; `None`
/// where `choices` have nothing to say of it. An icon with an identifier
/// is found by it; others by their program and, where the choice names
/// one, their number.
pub fn windows_choice(
    choices: &[WindowsChoice],
    guid: Option<[u8; 16]>,
    executable: Option<&Path>,
    uid: u32,
) -> Option<bool> {
    let same_program = |other: Option<&Path>| match (executable, other) {
        (Some(one), Some(other)) => {
            comparable(&one.to_string_lossy()) == comparable(&other.to_string_lossy())
        }
        _ => false,
    };
    let by_guid = guid.and_then(|guid| choices.iter().find(|choice| choice.guid == Some(guid)));
    let found = by_guid.or_else(|| {
        choices.iter().find(|choice| {
            choice.guid.is_none()
                && same_program(choice.executable)
                && choice.uid.is_none_or(|of| of == uid)
        })
    });
    found?.promoted
}

impl TrayConfig {
    /// Where the lists put the icon `key`, if they name it. Of entries in
    /// both lists that name it equally closely, the one in `shown` wins.
    fn chosen(&self, key: &IconKey) -> Option<Place> {
        let shown = key.closest(&self.shown).map(|(closeness, _)| closeness);
        let hidden = key.closest(&self.hidden).map(|(closeness, _)| closeness);
        match (shown, hidden) {
            (Some(shown), Some(hidden)) if hidden > shown => Some(Place::Overflow),
            (Some(_), _) => Some(Place::Bar),
            (None, Some(_)) => Some(Place::Overflow),
            (None, None) => None,
        }
    }

    /// Where the icon `key` stands: where the lists put it, or else where
    /// the settings of Windows do (`windows` says whether they put it in
    /// the taskbar), or else where new icons go.
    pub fn place(&self, key: &IconKey, windows: Option<bool>) -> Place {
        let windows = windows.map(|promoted| match promoted {
            true => Place::Bar,
            false => Place::Overflow,
        });
        let new = match self.new {
            NewIcons::Shown => Place::Bar,
            NewIcons::Hidden => Place::Overflow,
        };
        self.chosen(key).or(windows).unwrap_or(new)
    }

    /// Where `order` puts the icon `key`: the place of the closest entry
    /// that names it.
    fn rank(&self, key: &IconKey) -> Option<usize> {
        key.closest(&self.order).map(|(_, index)| index)
    }

    /// The icons `icons` (each with what the settings of Windows say of
    /// it), in the order shown and split between the bar and the overflow.
    /// The bar holds at most `limit` of them; those over it go to the
    /// overflow, ahead of the hidden ones. Each icon is given by its index
    /// in `icons`.
    pub fn arrange(&self, icons: &[(&IconKey, Option<bool>)], limit: Option<usize>) -> Arranged {
        let mut ranked: Vec<(usize, usize)> = icons
            .iter()
            .enumerate()
            .map(|(index, (key, _))| (self.rank(key).unwrap_or(usize::MAX), index))
            .collect();
        ranked.sort_unstable();
        let (mut bar, mut overflow) = (Vec::new(), Vec::new());
        for (_, index) in ranked {
            let (key, windows) = icons[index];
            match self.place(key, windows) {
                Place::Bar => bar.push(index),
                Place::Overflow => overflow.push(index),
            }
        }
        let limit = limit.unwrap_or(usize::MAX);
        if bar.len() > limit {
            let over = bar.split_off(limit);
            overflow.splice(0..0, over);
        }
        Arranged { bar, overflow }
    }

    /// These settings with the icon `key` moved to `to`: its own name is
    /// written into the list of that place and taken out of the other.
    /// Entries that name it less closely stay, as they may name others.
    pub fn moved(&self, key: &IconKey, to: Place) -> Self {
        let mut moved = self.clone();
        moved.shown.retain(|entry| !key.is_named_by(entry));
        moved.hidden.retain(|entry| !key.is_named_by(entry));
        let list = match to {
            Place::Bar => &mut moved.shown,
            Place::Overflow => &mut moved.hidden,
        };
        list.push(key.as_str().to_owned());
        moved
    }

    /// These settings with `present`, the icons there are now, in the
    /// order they are to be shown. Entries of `order` that name none of
    /// them stay, each after the icon it followed, so that an icon that
    /// comes back finds its place again.
    pub fn reordered(&self, present: &[&IconKey]) -> Self {
        let mut absent: Vec<(Option<usize>, String)> = Vec::new();
        let mut anchor = None;
        for entry in &self.order {
            let named = present
                .iter()
                .enumerate()
                .filter_map(|(index, key)| Some((key.closeness(entry)?, index)))
                .max_by_key(|&(closeness, index)| (closeness, std::cmp::Reverse(index)));
            match named {
                Some((_, index)) => anchor = Some(index),
                None => absent.push((anchor, entry.clone())),
            }
        }
        let after = |anchor: Option<usize>| {
            let absent = absent.iter().filter(move |(of, _)| *of == anchor);
            absent.map(|(_, entry)| entry.clone())
        };
        let mut order: Vec<String> = after(None).collect();
        for (index, key) in present.iter().enumerate() {
            order.push(key.as_str().to_owned());
            order.extend(after(Some(index)));
        }
        Self {
            order,
            ..self.clone()
        }
    }
}

/// The icons of the notification area as a strip shows them: indices into
/// the icons it was arranged from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arranged {
    pub bar: Vec<usize>,
    pub overflow: Vec<usize>,
}

/// The settings of the icons in use, and the writing of changes to them.
#[derive(Default)]
pub struct TraySettings {
    config: TrayConfig,
    /// Whether a change is being written; the file read meanwhile is
    /// older than what is shown.
    writing: bool,
    /// The change that waits for the one being written.
    waiting: Option<TrayConfig>,
}

impl Global for TraySettings {}

impl TraySettings {
    /// Takes over what the configuration says, unless a change made here
    /// is still being written.
    pub fn configure(config: &TrayConfig, cx: &mut App) {
        let this = cx.default_global::<Self>();
        if !this.writing && this.config != *config {
            this.config = config.clone();
        }
    }

    /// The settings in use.
    pub fn config(cx: &App) -> TrayConfig {
        cx.try_global::<Self>()
            .map(|this| this.config.clone())
            .unwrap_or_default()
    }

    /// Uses `config` at once and writes it into the configuration file.
    pub fn change(config: TrayConfig, cx: &mut App) {
        let this = cx.default_global::<Self>();
        if this.config == config {
            return;
        }
        this.config = config.clone();
        if this.writing {
            this.waiting = Some(config);
            return;
        }
        this.writing = true;
        cx.spawn(async move |cx: &mut AsyncApp| {
            let mut next = Some(config);
            while let Some(config) = next.take() {
                let written = cx
                    .background_executor()
                    .spawn(async move { write(&config) })
                    .await;
                if let Err(error) = written {
                    report!("the places of the notification icons stay unsaved: {error}");
                }
                let Ok(waiting) = cx.update(|cx| cx.global_mut::<Self>().waiting.take()) else {
                    return;
                };
                next = waiting;
            }
            let _ = cx.update(|cx| {
                cx.global_mut::<Self>().writing = false;
                Shell::config_changed(cx);
            });
        })
        .detach();
    }
}

/// Writes the lists of `config` into the configuration file. Empty lists
/// are taken out.
fn write(config: &TrayConfig) -> Result<(), String> {
    let lists = [
        ("shown", &config.shown),
        ("hidden", &config.hidden),
        ("order", &config.order),
    ];
    ConfigFile::change(|file| {
        for (key, entries) in lists {
            let value =
                (!entries.is_empty()).then(|| Value::Array(entries.iter().collect::<Array>()));
            if !file.set_in(TABLE, key, value) {
                return Err("`tray` in the configuration is not a table".to_owned());
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const GUID: [u8; 16] = [
        0x78, 0xae, 0x20, 0x78, 0xe3, 0x23, 0x29, 0x42, 0x82, 0xc1, 0xe4, 0x1c, 0xb6, 0x7d, 0x5b,
        0x9c,
    ];

    fn app(uid: u32) -> IconKey {
        IconKey::new(
            None,
            Some(Path::new("C:\\Program Files\\App\\App.exe")),
            "Window",
            uid,
        )
    }

    fn config(shown: &[&str], hidden: &[&str], order: &[&str]) -> TrayConfig {
        let owned = |list: &[&str]| list.iter().map(|entry| (*entry).to_owned()).collect();
        TrayConfig {
            shown: owned(shown),
            hidden: owned(hidden),
            order: owned(order),
            new: NewIcons::Shown,
        }
    }

    #[test]
    fn an_icon_is_named_by_its_identifier_or_its_program_and_number() {
        let guided = IconKey::new(Some(GUID), Some(Path::new("C:\\x.exe")), "Window", 1);
        assert_eq!(guided.as_str(), "{7820AE78-23E3-4229-82C1-E41CB67D5B9C}");
        assert_eq!(
            guided.closeness("{7820ae78-23e3-4229-82c1-e41cb67d5b9c}"),
            Some(3)
        );
        assert_eq!(guided.closeness("x.exe"), Some(1));
        assert_eq!(app(3).as_str(), "C:/Program Files/App/App.exe#3");
        assert_eq!(IconKey::new(None, None, "Shy", 2).as_str(), "class:Shy#2");
    }

    #[test]
    fn an_entry_names_an_icon_more_or_less_closely() {
        let key = app(3);
        assert_eq!(key.closeness("c:\\program files\\app\\app.exe#3"), Some(3));
        assert_eq!(key.closeness("C:/Program Files/App/App.exe"), Some(2));
        assert_eq!(key.closeness("app.exe#3"), Some(2));
        assert_eq!(key.closeness("App.exe"), Some(1));
        assert_eq!(key.closeness("app.exe#4"), None);
        assert_eq!(key.closeness("other.exe"), None);
        assert_eq!(key.closeness("App"), None);
    }

    #[test]
    fn the_closest_entry_decides_and_shown_wins_a_tie() {
        let key = app(3);
        assert_eq!(
            config(&["app.exe"], &[], &[]).place(&key, Some(false)),
            Place::Bar
        );
        assert_eq!(
            config(&["app.exe"], &["app.exe#3"], &[]).place(&key, None),
            Place::Overflow
        );
        assert_eq!(
            config(&["app.exe#3"], &["app.exe"], &[]).place(&key, None),
            Place::Bar
        );
        assert_eq!(
            config(&["app.exe"], &["app.exe"], &[]).place(&key, None),
            Place::Bar
        );
    }

    #[test]
    fn without_an_entry_windows_decides_and_then_new() {
        let key = app(1);
        let none = config(&[], &[], &[]);
        assert_eq!(none.place(&key, Some(false)), Place::Overflow);
        assert_eq!(none.place(&key, Some(true)), Place::Bar);
        assert_eq!(none.place(&key, None), Place::Bar);
        let hiding = TrayConfig {
            new: NewIcons::Hidden,
            ..none
        };
        assert_eq!(hiding.place(&key, None), Place::Overflow);
        assert_eq!(hiding.place(&key, Some(true)), Place::Bar);
    }

    #[test]
    fn icons_are_ordered_and_split_with_a_limit() {
        let (one, two, three, four) = (app(1), app(2), app(3), app(4));
        let settings = config(&[], &["app.exe#4"], &["app.exe#3", "app.exe#1"]);
        let icons = [(&one, None), (&two, None), (&three, None), (&four, None)];
        assert_eq!(
            settings.arrange(&icons, None),
            Arranged {
                bar: vec![2, 0, 1],
                overflow: vec![3]
            }
        );
        assert_eq!(
            settings.arrange(&icons, Some(1)),
            Arranged {
                bar: vec![2],
                overflow: vec![0, 1, 3]
            }
        );
    }

    #[test]
    fn a_move_writes_the_name_of_the_icon() {
        let key = app(2);
        let settings = config(&["app.exe", "C:/Program Files/App/App.exe#2"], &[], &[]);
        let moved = settings.moved(&key, Place::Overflow);
        assert_eq!(moved.shown, ["app.exe"]);
        assert_eq!(moved.hidden, ["C:/Program Files/App/App.exe#2"]);
        assert_eq!(moved.place(&key, None), Place::Overflow);
        let back = moved.moved(&key, Place::Bar);
        assert_eq!(back.shown, ["app.exe", "C:/Program Files/App/App.exe#2"]);
        assert!(back.hidden.is_empty());
    }

    #[test]
    fn a_new_order_keeps_the_places_of_absent_icons() {
        let (one, two) = (app(1), app(2));
        let settings = config(
            &[],
            &[],
            &["gone.exe", "app.exe#1", "away.exe", "app.exe#2"],
        );
        let order = settings.reordered(&[&two, &one]).order;
        assert_eq!(
            order,
            [
                "gone.exe",
                "C:/Program Files/App/App.exe#2",
                "C:/Program Files/App/App.exe#1",
                "away.exe"
            ]
        );
        assert_eq!(
            config(&[], &[], &[]).reordered(&[&one]).order,
            [one.as_str()]
        );
    }

    #[test]
    fn the_choice_of_windows_is_found_by_identifier_or_program() {
        let program = PathBuf::from("C:\\App\\app.exe");
        let choices = [
            WindowsChoice {
                guid: Some(GUID),
                executable: None,
                uid: None,
                promoted: Some(true),
            },
            WindowsChoice {
                guid: None,
                executable: Some(Path::new("c:\\app\\APP.exe")),
                uid: Some(2),
                promoted: Some(false),
            },
        ];
        assert_eq!(windows_choice(&choices, Some(GUID), None, 0), Some(true));
        assert_eq!(
            windows_choice(&choices, None, Some(&program), 2),
            Some(false)
        );
        assert_eq!(windows_choice(&choices, None, Some(&program), 3), None);
        assert_eq!(windows_choice(&choices, None, None, 2), None);
    }

    #[test]
    fn the_table_is_read_with_its_defaults() {
        let read: TrayConfig =
            toml::from_str("shown = [\"app.exe\"]\nnew = \"hidden\"\n").expect("a valid table");
        assert_eq!(read.shown, ["app.exe"]);
        assert_eq!(read.new, NewIcons::Hidden);
        assert!(toml::from_str::<TrayConfig>("pinned = []\n").is_err());
        assert!(toml::from_str::<TrayConfig>("new = \"maybe\"\n").is_err());
    }
}
