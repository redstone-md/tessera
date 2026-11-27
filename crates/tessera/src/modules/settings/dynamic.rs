// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The choices of rows that a rows file cannot list, because only the
//! running shell knows them: the tiling layouts there are, the places of
//! flyouts, the displays, the installed icon packs, the installed themes
//! and the interface languages. What
//! needs the disk is read away from the thread that draws, into
//! [`Scanned`], and the lists are made from it.

use std::fs;

use gpui::{App, Global};

use super::themes;
use crate::config::{DisplaySelector, config_dir};
use crate::flyout::FlyoutPlace;
use crate::i18n::Language;
use crate::options::{Choice, Label, OptionDecl};
use crate::platform::{DisplayName, display_names};
use crate::tiling::TileLayout;

/// The directory of the configuration directory that holds the layout
/// files of the tiling.
const TILING_LAYOUTS: &str = "tiling-layouts";

/// A table of the rows files that stands for every table: an entry for it
/// gives its choices to the rows of that key in any card.
const ANY_TABLE: &str = "*";

/// What the lists of choices are made from that needs the disk or
/// Windows, read away from the thread that draws.
#[derive(Clone, Debug, Default)]
pub(super) struct Scanned {
    /// The layout files of the user, by name, in lowercase.
    pub layouts: Vec<String>,
    /// The installed icon packs, by identifier.
    pub icon_packs: Vec<String>,
    /// The displays there are, in the order Windows lists them.
    pub displays: Vec<DisplayName>,
    /// The theme files there are.
    pub themes: Vec<String>,
    /// The languages there are to choose.
    pub languages: Vec<Language>,
}

impl Global for Scanned {}

impl Scanned {
    /// Reads it all; it waits for the disk and for Windows.
    pub(super) fn read() -> Self {
        Self {
            layouts: layout_files(),
            icon_packs: crate::icon_packs::installed_ids(),
            displays: display_names(),
            themes: themes(),
            languages: Language::installed(),
        }
    }
}

/// The names of the layout files of the user, in lowercase, sorted.
fn layout_files() -> Vec<String> {
    let Some(directory) = config_dir().map(|directory| directory.join(TILING_LAYOUTS)) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = { entries.flatten() }
        .map(|entry| entry.path())
        .filter(|path| {
            { path.extension() }.is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
        })
        .filter_map(|path| Some(path.file_stem()?.to_str()?.to_lowercase()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// What makes the choices of a row: from the lists that were read, and
/// from what the shell knows.
type Make = fn(&Scanned, &App) -> Vec<Choice>;

/// The rows whose choices are made here, by the table of their card (or
/// the widget its rows set an option of, or `""` for keys at the top of
/// the file, or `*` for any table) and their key. Lines are only added.
pub(super) const DYNAMIC: &[(&str, &str, Make)] = &[
    ("tiling", "layout", tile_layouts),
    ("flyout", "place", flyout_places),
    (ANY_TABLE, "displays", displays),
    ("", "icon_packs", icon_packs),
    ("", "theme", installed_themes),
    ("", "theme_layers", installed_themes),
    ("", "language", languages),
];

/// The choices of the row `key` of a card of `table`, made here; none
/// where nothing here makes them.
pub(super) fn choices_for(table: &str, key: &str, cx: &App) -> Option<Vec<Choice>> {
    let scanned = cx.try_global::<Scanned>().cloned().unwrap_or_default();
    { DYNAMIC.iter() }
        .find(|(known, named, _)| (*known == table || *known == ANY_TABLE) && *named == key)
        .map(|(_, _, make)| make(&scanned, cx))
}

/// Adds the choices of `more` that `decl` does not offer yet after its
/// own, in their order.
pub(super) fn merge(decl: &mut OptionDecl, more: Vec<Choice>) {
    for choice in more {
        let offered =
            { decl.choices.iter() }.any(|known| crate::options::same(&known.value, &choice.value));
        if !offered {
            decl.choices.push(choice);
        }
    }
}

/// A choice of `value`, called `label` or by the value.
fn choice(value: &str, label: Option<String>) -> Choice {
    Choice {
        value: value.into(),
        label: label.map(Label::Plain),
        overlay: None,
    }
}

/// Tessera's own layouts by their names, and the layout files of the
/// user; the first of Tessera's own is the one without the key.
fn tile_layouts(scanned: &Scanned, _: &App) -> Vec<Choice> {
    let own = { TileLayout::ALL.iter().skip(1) }.map(|layout| {
        choice(
            &layout.name().to_ascii_lowercase(),
            Some(layout.name().into()),
        )
    });
    let files = { scanned.layouts.iter() }
        .filter(|name| TileLayout::named(name).is_none())
        .map(|name| choice(name, None));
    own.chain(files).collect()
}

/// The places a flyout may hang; the first is the one without the key.
fn flyout_places(_: &Scanned, _: &App) -> Vec<Choice> {
    { FlyoutPlace::ALL.iter().skip(1) }
        .map(|place| choice(place.name(), Some(place.title().into())))
        .collect()
}

/// Every display, the primary one, the others, and each display there is
/// by the name the monitor gives itself.
fn displays(scanned: &Scanned, _: &App) -> Vec<Choice> {
    let kinds = [
        ("all", "All displays"),
        ("primary", "Primary display"),
        ("others", "The other displays"),
    ];
    let kinds = kinds.map(|(value, label)| choice(value, Some(label.to_owned())));
    let each = { scanned.displays.iter() }
        .filter(|display| DisplaySelector::parse(&display.friendly).is_ok())
        .map(|display| {
            let label = format!("{}. {}", display.position, display.friendly);
            choice(&display.friendly, Some(label))
        });
    kinds.into_iter().chain(each).collect()
}

/// The installed icon packs, by identifier.
fn icon_packs(scanned: &Scanned, _: &App) -> Vec<Choice> {
    { scanned.icon_packs.iter() }
        .map(|pack| choice(pack, None))
        .collect()
}

/// The theme files of the themes directory, and the colors of Windows
/// and of the wallpaper.
fn installed_themes(scanned: &Scanned, _: &App) -> Vec<Choice> {
    { scanned.themes.iter() }
        .map(|name| choice(name, None))
        .collect()
}

/// The languages there are, each by what it calls itself; English is the
/// one without the key.
fn languages(scanned: &Scanned, _: &App) -> Vec<Choice> {
    { scanned.languages.iter() }
        .filter(|language| **language != Language::default())
        .map(|language| choice(language.code(), Some(language.name().to_owned())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::Kind;

    fn declared(kind: Kind, choices: &[&str]) -> OptionDecl {
        let mut decl = OptionDecl::counter(0., 1., 1., 0.);
        decl.kind = kind;
        decl.default = None;
        decl.choices = choices.iter().map(|value| choice(value, None)).collect();
        decl
    }

    #[test]
    fn choices_are_added_after_those_of_the_file_once_each() {
        let mut decl = declared(Kind::Choice, &["unset", "columns"]);
        merge(
            &mut decl,
            vec![choice("columns", None), choice("wide", None)],
        );
        let values: Vec<String> = { decl.choices.iter() }
            .map(|choice| crate::options::text_of(&choice.value))
            .collect();
        assert_eq!(values, ["unset", "columns", "wide"]);
        let mut list = declared(Kind::List, &[]);
        merge(&mut list, Vec::new());
        assert!(list.choices.is_empty(), "a list without choices stays free");
    }

    #[test]
    fn every_table_named_here_has_one_maker_for_a_key() {
        for (index, (table, key, _)) in DYNAMIC.iter().enumerate() {
            let again = DYNAMIC[..index]
                .iter()
                .any(|(other, named, _)| other == table && named == key);
            assert!(!again, "`{table}` `{key}` is made twice");
        }
    }
}
