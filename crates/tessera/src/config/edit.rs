// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Changes to the user's configuration file that leave the rest of it, its
//! comments included, as the user wrote it.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value, value};

use super::{CONFIG_FILE, Config, DEFAULT, config_dir};

/// The widget whose `pinned` option lists the applications pinned to the
/// dock, and the one whose `pinned` option lists those of the Start menu.
pub(super) const WINDOWS_WIDGET: &str = "windows";
const APPS_WIDGET: &str = "apps";

/// The option of both that lists the pinned applications.
const PINNED: &str = "pinned";

/// The arrangements that come with Tessera, by name: the surfaces and
/// popups of a layout. The first is the built-in one.
pub const LAYOUTS: [(&str, &str); 3] = [
    ("macOS", include_str!("../layouts/macos.toml")),
    ("Windows", include_str!("../layouts/windows.toml")),
    ("Void", include_str!("../layouts/void.toml")),
];

/// A length of a card on the desktop that the settings change by steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CardLength {
    Width,
    Height,
    X,
    Y,
}

impl CardLength {
    pub const ALL: [Self; 4] = [Self::Width, Self::Height, Self::X, Self::Y];

    fn key(self) -> &'static str {
        match self {
            Self::Width => "width",
            Self::Height => "height",
            Self::X => "x",
            Self::Y => "y",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Width => "Width",
            Self::Height => "Height",
            Self::X => "Horizontal distance",
            Self::Y => "Vertical distance",
        }
    }

    /// The least, the most and the step of the length.
    fn range(self) -> (i64, i64, i64) {
        match self {
            Self::Width => (120, 1200, 20),
            Self::Height => (60, 1200, 20),
            Self::X | Self::Y => (0, 2000, 12),
        }
    }

    /// What the length is taken to be where the file does not say.
    fn unwritten(self) -> f64 {
        match self {
            Self::Width => 260.,
            Self::Height => 150.,
            Self::X | Self::Y => 24.,
        }
    }

    /// The length a step above `now`, or below, within its range.
    fn stepped(self, now: f64, up: bool) -> i64 {
        let (least, most, step) = self.range();
        let now = now.round() as i64;
        let next = if up { now + step } else { now - step };
        next.clamp(least, most)
    }
}

/// The names as a list of the file.
fn list(names: &[String]) -> Option<Value> {
    let list: Array = names.iter().map(String::as_str).collect();
    (!names.is_empty()).then_some(Value::Array(list))
}

/// The parts a surface is divided into along its length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Start,
    Center,
    End,
}

impl Slot {
    /// In the order they are laid out.
    pub const ALL: [Self; 3] = [Self::Start, Self::Center, Self::End];

    fn key(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Center => "center",
            Self::End => "end",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Center => "Center",
            Self::End => "End",
        }
    }
}

/// Where a widget is in the layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Place {
    /// Counts the surfaces in the order of the file.
    pub surface: usize,
    pub slot: Slot,
    pub index: usize,
}

/// What a `[[key]]` has one of.
pub const DEEDS: [&str; 7] = ["popup", "tiling", "focus", "move", "switch", "dock", "run"];

/// The configuration file, open for changing.
pub struct ConfigFile {
    document: DocumentMut,
}

/// Held from the reading of the file to its writing by [`ConfigFile::change`],
/// so that changes from different threads (the settings, the tray, a
/// program that sets the wallpaper) are made one after the other and none
/// writes over another it did not read.
static WRITING: Mutex<()> = Mutex::new(());

impl ConfigFile {
    /// Reads the file, has `change` change it and writes it, unless
    /// `change` fails or the shell could not use what the file would hold;
    /// what `change` answers is answered. No other change of the file is
    /// made meanwhile, so `change` must not change the file itself.
    /// Before the change, config.toml is copied to the backups, once a
    /// minute at most; a backup that fails is reported and the change
    /// made all the same.
    pub fn change<T>(change: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
        let _alone = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(error) = back_up(false) {
            report!("config.toml is not backed up: {error}");
        }
        let mut file = Self::open()?;
        let answer = change(&mut file)?;
        file.save()?;
        Ok(answer)
    }

    /// The user's file, or the built-in configuration, comments and all,
    /// where there is none yet. It is changed through [`Self::change`].
    pub fn open() -> Result<Self, String> {
        let path = Self::path()?;
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == ErrorKind::NotFound => Self::parse(DEFAULT),
            Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
        }
    }

    fn path() -> Result<PathBuf, String> {
        let directory = config_dir().ok_or("there is no place for a configuration")?;
        Ok(directory.join(CONFIG_FILE))
    }

    fn parse(text: &str) -> Result<Self, String> {
        let document = text
            .parse()
            .map_err(|error| format!("the configuration cannot be changed: {error}"))?;
        Ok(Self { document })
    }

    /// What the file holds now, as the shell reads it.
    pub fn config(&self) -> Result<Config, String> {
        toml::from_str(&self.document.to_string())
            .map_err(|error| format!("the configuration would not be usable: {error}"))
    }

    /// Writes the file, unless the shell could not use what it would hold.
    /// It is written beside itself first and then put in place, so that a
    /// half-written file is never read.
    fn save(&self) -> Result<(), String> {
        self.config()?;
        let path = Self::path()?;
        let failed = |error| format!("{} cannot be written: {error}", path.display());
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory).map_err(failed)?;
        }
        let draft = path.with_extension("toml.new");
        fs::write(&draft, self.document.to_string()).map_err(failed)?;
        fs::rename(&draft, &path).map_err(failed)
    }

    /// Sets a key of a surface, or takes it out with `None`.
    pub fn set_surface(&mut self, surface: usize, key: &str, to: Option<Value>) -> bool {
        let surfaces = self.document.get_mut("surface");
        let Some(surface) = surfaces
            .and_then(Item::as_array_of_tables_mut)
            .and_then(|surfaces| surfaces.get_mut(surface))
        else {
            return false;
        };
        match to {
            Some(to) => {
                surface.insert(key, Item::Value(to));
            }
            None => {
                surface.remove(key);
            }
        }
        true
    }

    /// The widgets of one slot, which is created if the surface lacks it.
    fn slot(&mut self, surface: usize, slot: Slot) -> Option<&mut Array> {
        let surfaces = self.document.get_mut("surface")?.as_array_of_tables_mut()?;
        let item = surfaces
            .get_mut(surface)?
            .entry(slot.key())
            .or_insert_with(|| Item::Value(Value::Array(Array::new())));
        item.as_array_mut()
    }

    /// Appends the widget `widget` to a slot.
    pub fn add(&mut self, surface: usize, slot: Slot, widget: &str) -> bool {
        let Some(widgets) = self.slot(surface, slot) else {
            return false;
        };
        widgets.push(widget);
        widgets.fmt();
        true
    }

    /// Takes the widget at `place` out of the layout.
    pub fn remove(&mut self, place: Place) -> bool {
        self.take(place).is_some()
    }

    fn take(&mut self, place: Place) -> Option<Value> {
        let widgets = self.slot(place.surface, place.slot)?;
        if place.index >= widgets.len() {
            return None;
        }
        let widget = widgets.remove(place.index);
        widgets.fmt();
        Some(widget)
    }

    /// Moves the widget at `from` to stand before what is at `to` now,
    /// which may be on another surface, or last in that slot when the
    /// index of `to` is past its end.
    pub fn transfer(&mut self, from: Place, to: Place) -> bool {
        let surfaces = self
            .document
            .get("surface")
            .and_then(Item::as_array_of_tables)
            .map_or(0, |surfaces| surfaces.len());
        if to.surface >= surfaces {
            return false;
        }
        let Some(widget) = self.take(from) else {
            return false;
        };
        // What stood after it in its own slot has moved up by one.
        let same_slot = to.surface == from.surface && to.slot == from.slot;
        let index = if same_slot && to.index > from.index {
            to.index - 1
        } else {
            to.index
        };
        let Some(widgets) = self.slot(to.surface, to.slot) else {
            return false;
        };
        let index = index.min(widgets.len());
        widgets.insert(index, widget);
        widgets.fmt();
        true
    }

    /// A card of the desktop.
    fn card(&mut self, card: usize) -> Option<&mut Table> {
        let cards = self.document.get_mut("card")?.as_array_of_tables_mut()?;
        cards.get_mut(card)
    }

    /// The widgets of a card, a list that is created if the card lacks it.
    fn card_content(&mut self, card: usize) -> Option<&mut Array> {
        let item = self
            .card(card)?
            .entry("content")
            .or_insert_with(|| Item::Value(Value::Array(Array::new())));
        item.as_array_mut()
    }

    /// A `[[key]]` of the file.
    fn hotkey(&mut self, key: usize) -> Option<&mut Table> {
        let keys = self.document.get_mut("key")?.as_array_of_tables_mut()?;
        keys.get_mut(key)
    }

    /// Appends a `[[key]]` of the combination `press`, which takes the
    /// tiling mode after the one in use.
    pub fn add_hotkey(&mut self, press: &str) -> bool {
        let item = self
            .document
            .entry("key")
            .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
        let Some(keys) = item.as_array_of_tables_mut() else {
            return false;
        };
        let mut key = Table::new();
        key.insert("press", value(press));
        key.insert("tiling", value("next"));
        keys.push(key);
        true
    }

    /// Takes a `[[key]]` out.
    pub fn remove_hotkey(&mut self, key: usize) -> bool {
        let keys = self.document.get_mut("key");
        let Some(keys) = keys.and_then(Item::as_array_of_tables_mut) else {
            return false;
        };
        if key >= keys.len() {
            return false;
        }
        keys.remove(key);
        if keys.is_empty() {
            self.document.remove("key");
        }
        true
    }

    /// Gives a `[[key]]` another combination.
    pub fn set_hotkey_press(&mut self, key: usize, press: &str) -> bool {
        let Some(key) = self.hotkey(key) else {
            return false;
        };
        key.insert("press", value(press));
        true
    }

    /// Has a `[[key]]` do `deed`, one of [`DEEDS`], with `to`, instead of
    /// what it did.
    pub fn set_hotkey_deed(&mut self, key: usize, deed: &str, to: &str) -> bool {
        let Some(key) = self.hotkey(key) else {
            return false;
        };
        if !DEEDS.contains(&deed) {
            return false;
        }
        // What a key does is one of these, or a folder or an action.
        for other in DEEDS.into_iter().chain(["files", "action", "arg"]) {
            key.remove(other);
        }
        // The number of an application in the dock is written as one.
        let written = match (deed, to.parse::<i64>()) {
            ("dock", Ok(number)) => value(number),
            _ => value(to),
        };
        key.insert(deed, written);
        true
    }

    /// Appends `rule` as a `[[rule]]`, with `note` as a comment above it.
    pub fn add_rule(&mut self, mut rule: Table, note: &str) -> bool {
        let item = self
            .document
            .entry("rule")
            .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
        let Some(rules) = item.as_array_of_tables_mut() else {
            return false;
        };
        let note: String = note.lines().map(|line| format!("# {line}\n")).collect();
        rule.decor_mut().set_prefix(format!("\n{note}"));
        rules.push(rule);
        true
    }

    /// Where the user's file is written.
    pub fn location() -> Result<PathBuf, String> {
        Self::path()
    }

    /// Appends a card with the clock to the desktop.
    pub fn add_card(&mut self) -> bool {
        let item = self
            .document
            .entry("card")
            .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
        let Some(cards) = item.as_array_of_tables_mut() else {
            return false;
        };
        let mut content = Array::new();
        content.push("clock");
        let mut card = Table::new();
        card.insert("corner", value("top-right"));
        card.insert("width", value(CardLength::Width.unwritten() as i64));
        card.insert("height", value(CardLength::Height.unwritten() as i64));
        card.insert("content", value(content));
        cards.push(card);
        true
    }

    /// Takes a card off the desktop.
    pub fn remove_card(&mut self, card: usize) -> bool {
        let cards = self.document.get_mut("card");
        let Some(cards) = cards.and_then(Item::as_array_of_tables_mut) else {
            return false;
        };
        if card >= cards.len() {
            return false;
        }
        cards.remove(card);
        if cards.is_empty() {
            self.document.remove("card");
        }
        true
    }

    /// Sets a key of a card, or takes it out with `None`.
    pub fn set_card(&mut self, card: usize, key: &str, to: Option<Value>) -> bool {
        let Some(card) = self.card(card) else {
            return false;
        };
        match to {
            Some(to) => {
                card.insert(key, Item::Value(to));
            }
            None => {
                card.remove(key);
            }
        }
        true
    }

    /// Makes a length of a card a step longer, or shorter. Whether it
    /// changed is returned.
    pub fn step_card(&mut self, card: usize, length: CardLength, up: bool) -> bool {
        let Some(card) = self.card(card) else {
            return false;
        };
        let written = card.get(length.key()).and_then(|item| {
            let whole = || item.as_integer().map(|number| number as f64);
            item.as_float().or_else(whole)
        });
        let next = length.stepped(written.unwrap_or(length.unwritten()), up);
        if written == Some(next as f64) {
            return false;
        }
        card.insert(length.key(), value(next));
        true
    }

    /// Appends the widget `widget` to a card.
    pub fn add_to_card(&mut self, card: usize, widget: &str) -> bool {
        let Some(widgets) = self.card_content(card) else {
            return false;
        };
        widgets.push(widget);
        widgets.fmt();
        true
    }

    /// Takes the widget at `index` out of a card.
    pub fn remove_from_card(&mut self, card: usize, index: usize) -> bool {
        let Some(widgets) = self.card_content(card) else {
            return false;
        };
        if index >= widgets.len() {
            return false;
        }
        widgets.remove(index);
        widgets.fmt();
        true
    }

    /// Moves the widget at `index` of a card past the one below it, or
    /// past the one above.
    pub fn shift_in_card(&mut self, card: usize, index: usize, forward: bool) -> bool {
        let Some(widgets) = self.card_content(card) else {
            return false;
        };
        let to = if forward {
            index + 1
        } else {
            index.wrapping_sub(1)
        };
        if index >= widgets.len() || to >= widgets.len() {
            return false;
        }
        let widget = widgets.remove(index);
        widgets.insert(to, widget);
        widgets.fmt();
        true
    }

    /// Makes `pinned` the applications that keep a button in every dock:
    /// the `pinned` option of each `windows` widget. Whether the layout
    /// has such a widget is returned.
    pub fn set_pinned(&mut self, pinned: &[String]) -> bool {
        self.on_surfaces(WINDOWS_WIDGET, PINNED, list(pinned).as_ref())
    }

    /// Sets the option `key` of every widget `name` on a surface; `None`
    /// takes it away. Whether there is such a widget is returned.
    fn on_surfaces(&mut self, name: &str, key: &str, value: Option<&Value>) -> bool {
        let surfaces = self
            .document
            .get("surface")
            .and_then(Item::as_array_of_tables)
            .map_or(0, |surfaces| surfaces.len());
        let mut found = false;
        for surface in 0..surfaces {
            for slot in Slot::ALL {
                // A slot the file does not mention stays unmentioned.
                let written = self.document["surface"][surface].get(slot.key());
                if written.is_none() {
                    continue;
                }
                let Some(widgets) = self.slot(surface, slot) else {
                    continue;
                };
                for widget in widgets.iter_mut() {
                    found |= Self::option(widget, name, key, value);
                }
                widgets.fmt();
            }
        }
        found
    }

    /// Makes `pinned` the applications of the Start menu: the `pinned`
    /// option of each `apps` widget of a popup. Whether a popup has such a
    /// widget is returned.
    pub fn set_start_pinned(&mut self, pinned: &[String]) -> bool {
        self.in_popups(APPS_WIDGET, PINNED, list(pinned).as_ref())
    }

    /// Sets the option `key` of every widget `name` in a popup; `None`
    /// takes it away. Whether there is such a widget is returned.
    fn in_popups(&mut self, name: &str, key: &str, value: Option<&Value>) -> bool {
        let popups = self.document.get_mut("popup");
        let Some(popups) = popups.and_then(Item::as_array_of_tables_mut) else {
            return false;
        };
        let mut found = false;
        for popup in popups.iter_mut() {
            let content = popup.get_mut("content").and_then(Item::as_array_mut);
            let Some(widgets) = content else {
                continue;
            };
            for widget in widgets.iter_mut() {
                found |= Self::option(widget, name, key, value);
            }
            widgets.fmt();
        }
        found
    }

    /// Sets the option `key` of every widget `name`, wherever it is placed:
    /// on a surface, in a popup (both its columns) or on a card of the
    /// desktop; `None` takes it away. Whether there is such a widget is
    /// returned.
    pub fn set_option(&mut self, name: &str, key: &str, value: Option<Value>) -> bool {
        let placed = self.on_surfaces(name, key, value.as_ref());
        let shown = self.in_popups(name, key, value.as_ref());
        let named = (name, key);
        let beside = set_in_entries(&mut self.document, ("popup", "side"), named, value.as_ref());
        let lying = set_in_entries(
            &mut self.document,
            ("card", "content"),
            named,
            value.as_ref(),
        );
        placed || shown || beside || lying
    }

    /// Sets `key` at the top of the file; `None` takes it away.
    pub fn set_key(&mut self, key: &str, value: Option<Value>) {
        match value {
            Some(value) => self.document[key] = Item::Value(value),
            None => {
                self.document.remove(key);
            }
        }
    }

    /// Sets `key` of the table `table`, which is created for it and taken
    /// away with its last key; `None` takes the key away. Whether `table`
    /// is something keys can be set in is returned.
    pub fn set_in(&mut self, table: &str, key: &str, value: Option<Value>) -> bool {
        if value.is_none() && self.document.get(table).is_none() {
            return true;
        }
        let item = self.document.entry(table);
        let item = item.or_insert_with(|| Item::Table(Table::new()));
        let Some(entries) = item.as_table_like_mut() else {
            return false;
        };
        match value {
            Some(value) => {
                entries.insert(key, Item::Value(value));
            }
            None => {
                entries.remove(key);
            }
        }
        if entries.is_empty() {
            self.document.remove(table);
        }
        true
    }

    /// Replaces the surfaces and popups by those `layout` holds. The
    /// applications pinned to the dock and the Start menu stay pinned, and
    /// what is written above the first surface stays written.
    pub fn set_layout(&mut self, layout: &str) -> bool {
        let Ok(layout) = layout.parse::<DocumentMut>() else {
            return false;
        };
        let config = self.config().ok();
        let kept = [WINDOWS_WIDGET, APPS_WIDGET].map(|name| {
            let pinned = config.as_ref()?.option(name, PINNED)?;
            pinned.to_string().parse::<Value>().ok()
        });
        let first = |document: &DocumentMut| {
            let surfaces = document.get("surface")?.as_array_of_tables()?;
            Some(surfaces.get(0)?.decor().prefix()?.clone())
        };
        let above = first(&self.document);
        for key in ["surface", "popup"] {
            match layout.get(key) {
                Some(item) => self.document[key] = item.clone(),
                None => {
                    self.document.remove(key);
                }
            }
        }
        let surfaces = self.document.get_mut("surface");
        let surfaces = surfaces.and_then(Item::as_array_of_tables_mut);
        if let (Some(above), Some(surface)) = (above, surfaces.and_then(|all| all.get_mut(0))) {
            surface.decor_mut().set_prefix(above);
        }
        let [dock, start] = kept;
        if dock.is_some() {
            self.on_surfaces(WINDOWS_WIDGET, PINNED, dock.as_ref());
        }
        if start.is_some() {
            self.in_popups(APPS_WIDGET, PINNED, start.as_ref());
        }
        true
    }

    /// The file as a table, to look single keys up in.
    pub fn table(&self) -> Option<toml::Table> {
        toml::from_str(&self.document.to_string()).ok()
    }

    /// Sets the option `key` on `widget` if it is the widget `name`.
    fn option(widget: &mut Value, name: &str, key: &str, value: Option<&Value>) -> bool {
        if widget.as_str() == Some(name) {
            let mut table = InlineTable::new();
            table.insert("widget", name.into());
            *widget = Value::InlineTable(table);
        }
        let Some(table) = widget.as_inline_table_mut() else {
            return false;
        };
        if table.get("widget").and_then(Value::as_str) != Some(name) {
            return false;
        }
        match value {
            Some(value) => {
                table.insert(key, value.clone());
            }
            None => {
                table.remove(key);
            }
        }
        // Otherwise the new option keeps the spacing of the brace it
        // displaced.
        table.fmt();
        if table.len() == 1 {
            // Nothing but the name is left, which reads better bare.
            *widget = name.into();
        }
        true
    }

    /// Names the theme to apply; `None` is the built-in one.
    pub fn set_theme(&mut self, theme: Option<&str>) {
        match theme {
            Some(theme) => self.document["theme"] = toml_edit::value(theme),
            None => {
                self.document.remove("theme");
            }
        }
    }
}

// What the marketplace writes: the icon packs in use, a wallpaper it
// applies and a widget it places.
impl ConfigFile {
    /// Names the icon packs in use, the first one first; none takes the
    /// key away.
    pub fn set_icon_packs(&mut self, packs: &[String]) {
        self.set_key("icon_packs", list(packs));
    }

    /// Shows the picture at `picture` as the wallpaper: it becomes the
    /// only one listed, and the source is the listed files. Videos and
    /// the pictures for light and dark Windows, which would be shown in
    /// its place, are taken out.
    pub fn apply_wallpaper(&mut self, picture: &std::path::Path) -> bool {
        let Some(picture) = picture.to_str() else {
            return false;
        };
        let images = Value::Array(Array::from_iter([picture]));
        let mut done = self.set_in("wallpaper", "source", Some("files".into()));
        done &= self.set_in("wallpaper", "images", Some(images));
        for key in ["video", "videos", "images_light", "images_dark"] {
            done &= self.set_in("wallpaper", key, None);
        }
        done
    }

    /// Adds `widget` to `slot` of the first surface along the top of a
    /// display, or along the bottom with `in_dock`; of the first surface
    /// where there is none such. Whether there was a surface is returned.
    pub fn place_widget(&mut self, in_dock: bool, slot: Slot, widget: &str) -> bool {
        let Ok(config) = self.config() else {
            return false;
        };
        let wanted = if in_dock {
            crate::platform::ScreenEdge::Bottom
        } else {
            crate::platform::ScreenEdge::Top
        };
        let surfaces = &config.surfaces;
        let surface = { surfaces.iter() }
            .position(|surface| surface.edge == wanted)
            .or((!surfaces.is_empty()).then_some(0));
        surface.is_some_and(|surface| self.add(surface, slot, widget))
    }
}

// launcher-kickoff: the pins of the dock, changed from the menu of an
// application as the dock changes them.
impl ConfigFile {
    /// Makes `change` of the applications pinned to the dock in the
    /// configuration file and saves it. Fails where the layout has no
    /// `windows` widget or the file cannot be read or written; it reads
    /// and writes the disk, so it is called away from the thread of GPUI.
    pub fn change_pinned(change: impl FnOnce(&[String]) -> Vec<String>) -> Result<(), String> {
        const NO_DOCK: &str = "the layout has no `windows` widget to pin to";
        Self::change(|file| {
            let pins = file.config()?.pinned().ok_or(NO_DOCK)?;
            if !file.set_pinned(&change(&pins)) {
                return Err(NO_DOCK.to_owned());
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAYOUT: &str = r#"# Mine.
[[surface]]
edge = "top"
thickness = 48
# The clock goes first.
start = ["clock", { widget = "label", text = "Hi" }]
end = ["quit"]

[[surface]]
edge = "bottom"
thickness = 64
center = ["windows"]
"#;

    fn names(file: &ConfigFile, surface: usize, slot: Slot) -> Vec<String> {
        let config = file.config().expect("the layout stays usable");
        let surface = &config.surfaces[surface];
        let widgets = match slot {
            Slot::Start => &surface.start,
            Slot::Center => &surface.center,
            Slot::End => &surface.end,
        };
        widgets.iter().map(|spec| spec.widget.clone()).collect()
    }

    /// Moves a widget within its surface.
    fn relocate(file: &mut ConfigFile, from: Place, slot: Slot, index: usize) -> bool {
        file.transfer(from, place(from.surface, slot, index))
    }

    fn place(surface: usize, slot: Slot, index: usize) -> Place {
        Place {
            surface,
            slot,
            index,
        }
    }

    #[test]
    fn cards_are_added_changed_and_taken_away() {
        use crate::desk::Corner;

        let mut file = ConfigFile::parse(LAYOUT).unwrap();
        assert!(!file.remove_card(0));
        assert!(!file.step_card(0, CardLength::Width, true));
        assert!(file.add_card());
        assert!(file.add_card());
        assert!(file.set_card(1, "corner", Some("bottom-left".into())));
        assert!(file.set_card(1, "displays", Some("primary".into())));
        assert!(file.step_card(1, CardLength::Width, true));
        assert!(file.step_card(1, CardLength::Y, false));
        assert!(file.add_to_card(1, "cpu"));
        assert!(file.add_to_card(1, "memory"));
        assert!(file.shift_in_card(1, 0, true));
        assert!(!file.shift_in_card(1, 2, true));
        assert!(!file.shift_in_card(1, 0, false));
        assert!(file.remove_from_card(1, 2));
        assert!(!file.remove_from_card(1, 2));
        let config = file.config().expect("the cards are usable");
        assert_eq!(config.surfaces.len(), 2);
        assert_eq!(config.cards.len(), 2);
        assert_eq!(config.cards[0].corner, Corner::TopRight);
        let card = &config.cards[1];
        assert_eq!(card.corner, Corner::BottomLeft);
        assert_eq!(card.displays, crate::config::Displays::Primary);
        assert_eq!(
            (card.width, card.height, card.x, card.y),
            (280., 150., 24., 12.)
        );
        let names: Vec<_> = card.content.iter().map(|spec| &spec.widget).collect();
        assert_eq!(names, ["cpu", "clock"]);

        assert!(file.set_card(1, "displays", None));
        assert!(file.remove_card(0));
        assert!(file.remove_card(0));
        let config = file.config().expect("the layout stays usable");
        assert!(config.cards.is_empty());
        assert!(file.document.to_string().starts_with("# Mine."));
        assert!(!file.document.to_string().contains("card"));
    }

    #[test]
    fn a_length_of_a_card_stays_within_its_range() {
        assert_eq!(CardLength::Width.stepped(259.6, true), 280);
        assert_eq!(CardLength::Width.stepped(120., false), 120);
        assert_eq!(CardLength::Height.stepped(1195., true), 1200);
        assert_eq!(CardLength::X.stepped(6., false), 0);
    }

    #[test]
    fn a_widget_is_put_before_another_one_anywhere_in_its_surface() {
        let mut file = ConfigFile::parse(LAYOUT).unwrap();
        assert!(file.add(0, Slot::End, "volume"));
        assert!(file.add(0, Slot::End, "cpu"));
        // Toward the end of its own slot, and to the very end of it.
        assert!(relocate(&mut file, place(0, Slot::End, 0), Slot::End, 2));
        assert_eq!(names(&file, 0, Slot::End), ["volume", "quit", "cpu"]);
        assert!(relocate(&mut file, place(0, Slot::End, 0), Slot::End, 9));
        assert_eq!(names(&file, 0, Slot::End), ["quit", "cpu", "volume"]);
        // Toward the start of it.
        assert!(relocate(&mut file, place(0, Slot::End, 2), Slot::End, 0));
        assert_eq!(names(&file, 0, Slot::End), ["volume", "quit", "cpu"]);
        // Into another slot, also one the file does not have yet.
        assert!(relocate(&mut file, place(0, Slot::End, 1), Slot::Start, 1));
        assert_eq!(names(&file, 0, Slot::Start), ["clock", "quit", "label"]);
        assert!(relocate(
            &mut file,
            place(0, Slot::Start, 2),
            Slot::Center,
            0
        ));
        assert_eq!(names(&file, 0, Slot::Center), ["label"]);
        assert_eq!(names(&file, 0, Slot::End), ["volume", "cpu"]);
        assert!(
            file.document
                .to_string()
                .contains(r#"{ widget = "label", text = "Hi" }"#)
        );
        assert!(!relocate(&mut file, place(0, Slot::End, 5), Slot::Start, 0));
        assert!(!relocate(&mut file, place(4, Slot::End, 0), Slot::Start, 0));
    }

    #[test]
    fn a_widget_goes_to_another_surface_with_its_options() {
        let mut file = ConfigFile::parse(LAYOUT).unwrap();
        assert!(file.transfer(place(0, Slot::Start, 1), place(1, Slot::Center, 0)));
        assert_eq!(names(&file, 0, Slot::Start), ["clock"]);
        assert_eq!(names(&file, 1, Slot::Center), ["label", "windows"]);
        assert!(
            file.document
                .to_string()
                .contains(r#"{ widget = "label", text = "Hi" }"#)
        );
        assert!(file.transfer(place(1, Slot::Center, 1), place(0, Slot::End, 9)));
        assert_eq!(names(&file, 0, Slot::End), ["quit", "windows"]);
        // A surface the file does not have takes nothing.
        assert!(!file.transfer(place(0, Slot::End, 0), place(2, Slot::Start, 0)));
        assert_eq!(names(&file, 0, Slot::End), ["quit", "windows"]);
    }

    #[test]
    fn changes_keep_comments_and_options() {
        let mut file = ConfigFile::parse(LAYOUT).unwrap();
        assert!(file.add(0, Slot::End, "volume"));
        assert!(file.remove(place(0, Slot::Start, 0)));
        assert!(!file.remove(place(0, Slot::Start, 4)));
        let text = file.document.to_string();
        assert!(text.contains("# Mine."));
        assert!(text.contains("# The clock goes first."));
        assert!(text.contains(r#"start = [{ widget = "label", text = "Hi" }]"#));
        assert!(text.contains(r#"end = ["quit", "volume"]"#));
    }

    #[test]
    fn pins_are_an_option_of_the_windows_widget() {
        let mut file = ConfigFile::parse(LAYOUT).unwrap();
        let pinned = ["C:/Windows/notepad.exe".to_owned(), "Paint".to_owned()];
        assert!(file.set_pinned(&pinned));
        let text = file.document.to_string();
        assert!(text.contains(
            r#"center = [{ widget = "windows", pinned = ["C:/Windows/notepad.exe", "Paint"] }]"#
        ));
        let config = file.config().unwrap();
        let listed = config.surfaces[1].center[0].options["pinned"].as_array();
        assert_eq!(listed.map(Vec::len), Some(2));

        assert!(file.set_pinned(&[]));
        assert_eq!(file.document.to_string(), LAYOUT);

        let mut bare = ConfigFile::parse("[[surface]]\nedge = \"top\"\nthickness = 4\n").unwrap();
        assert!(!bare.set_pinned(&pinned));
    }

    #[test]
    fn start_pins_are_an_option_of_the_apps_widget() {
        const POPUPS: &str = r#"[[surface]]
edge = "top"
thickness = 48

# Start.
[[popup]]
name = "start"
width = 400
height = 500
content = [{ widget = "apps", columns = 4 }, "launcher"]

[[popup]]
name = "more"
width = 400
height = 500
content = ["apps"]
"#;
        let mut file = ConfigFile::parse(POPUPS).unwrap();
        assert!(file.set_start_pinned(&["Paint".to_owned()]));
        let text = file.document.to_string();
        assert!(text.contains(
            r#"content = [{ widget = "apps", columns = 4, pinned = ["Paint"] }, "launcher"]"#
        ));
        assert!(text.contains(r#"content = [{ widget = "apps", pinned = ["Paint"] }]"#));
        file.config().expect("it stays usable");
        assert!(file.set_start_pinned(&[]));
        assert_eq!(file.document.to_string(), POPUPS);

        let mut bare = ConfigFile::parse(LAYOUT).unwrap();
        assert!(!bare.set_start_pinned(&["Paint".to_owned()]));
    }

    #[test]
    fn the_theme_is_named_and_forgotten() {
        let mut file = ConfigFile::parse(LAYOUT).unwrap();
        file.set_theme(Some("night"));
        assert_eq!(file.config().unwrap().theme.as_deref(), Some("night"));
        file.set_theme(None);
        assert_eq!(file.config().unwrap().theme, None);
    }

    #[test]
    fn an_option_is_set_wherever_the_widget_is_and_taken_away_again() {
        let mut file = ConfigFile::parse(LAYOUT).unwrap();
        assert!(file.set_option("clock", "format", Some("%H".into())));
        assert!(file.set_option("windows", "limit", Some(3.into())));
        assert!(!file.set_option("tray", "limit", Some(3.into())));
        let config = file.config().unwrap();
        assert_eq!(
            config.option("clock", "format").unwrap().as_str(),
            Some("%H")
        );
        assert_eq!(
            config.option("windows", "limit").unwrap().as_integer(),
            Some(3)
        );
        assert!(file.set_option("clock", "format", None));
        assert!(file.set_option("windows", "limit", None));
        assert_eq!(file.document.to_string(), LAYOUT);
    }

    #[test]
    fn keys_are_set_at_the_top_and_in_tables_of_their_own() {
        let mut file = ConfigFile::parse(LAYOUT).unwrap();
        file.set_key("language", Some("ru".into()));
        assert!(file.set_in("tiling", "enabled", Some(true.into())));
        assert!(file.set_in("tiling", "gap", Some(12.into())));
        let config = file.config().expect("it stays usable");
        assert_eq!(config.language, crate::i18n::Language::Russian);
        assert!(config.tiling.enabled);
        let table = file.table().unwrap();
        assert_eq!(table["tiling"]["gap"].as_integer(), Some(12));

        file.set_key("language", None);
        assert!(file.set_in("tiling", "enabled", None));
        assert!(file.set_in("tiling", "gap", None));
        assert!(file.set_in("workspaces", "own", None));
        assert_eq!(file.document.to_string(), LAYOUT);
    }

    #[test]
    fn an_arrangement_replaces_the_bars_and_keeps_the_pins() {
        let mut file = ConfigFile::parse(DEFAULT).unwrap();
        assert!(file.set_pinned(&["Paint".to_owned()]));
        assert!(file.set_start_pinned(&["Notepad".to_owned()]));
        file.set_key("theme", Some("night".into()));
        let (_, windows) = LAYOUTS[1];
        assert!(file.set_layout(windows));
        let config = file.config().expect("it stays usable");
        assert_eq!(config.surfaces.len(), 1);
        assert_eq!(config.theme.as_deref(), Some("night"));
        let pinned = |name| config.option(name, "pinned").unwrap().as_array().unwrap()[0].clone();
        assert_eq!(pinned("windows").as_str(), Some("Paint"));
        assert_eq!(pinned("apps").as_str(), Some("Notepad"));
        let text = file.document.to_string();
        assert!(text.contains("# Built-in widgets:"));
        assert!(text.contains("# [tiling] puts"));

        let (_, first) = LAYOUTS[0];
        assert!(file.set_layout(first));
        assert_eq!(file.config().unwrap().surfaces.len(), 2);
        assert!(!file.set_layout("[[surface"));
    }

    #[test]
    fn the_built_in_configuration_is_the_first_arrangement() {
        let (_, first) = LAYOUTS[0];
        let layout: Config = toml::from_str(first).unwrap();
        let builtin: Config = toml::from_str(DEFAULT).unwrap();
        assert_eq!(layout.surfaces, builtin.surfaces);
        assert_eq!(layout.popups, builtin.popups);
        for (name, layout) in LAYOUTS {
            let layout: Config = toml::from_str(layout).unwrap();
            let settings = layout.popups.iter().any(|popup| popup.name == "settings");
            assert!(settings, "{name} cannot be left again");
        }
    }

    #[test]
    fn keys_are_added_changed_and_taken_out() {
        let mut file = ConfigFile::parse(LAYOUT).unwrap();
        assert!(!file.remove_hotkey(0));
        assert!(file.add_hotkey("ctrl+alt+a"));
        assert!(file.add_hotkey("ctrl+alt+b"));
        assert!(file.set_hotkey_press(1, "alt+shift+left"));
        assert!(file.set_hotkey_deed(1, "dock", "3"));
        assert!(file.document.to_string().contains("dock = 3"));
        assert!(file.set_hotkey_deed(1, "move", "left"));
        assert!(!file.set_hotkey_deed(1, "press", "left"));
        assert!(!file.set_hotkey_deed(2, "focus", "left"));
        let keys = file.config().unwrap().keys;
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].tiling.as_deref(), Some("next"));
        assert_eq!(keys[1].press, "alt+shift+left");
        assert_eq!(
            (keys[1].tiling.as_deref(), keys[1].carry.as_deref()),
            (None, Some("left"))
        );
        assert!(file.remove_hotkey(0));
        assert!(file.remove_hotkey(0));
        assert!(file.config().unwrap().keys.is_empty());
        assert!(!file.document.contains_key("key"));
    }

    #[test]
    fn the_built_in_configuration_can_be_changed() {
        let mut file = ConfigFile::parse(DEFAULT).unwrap();
        assert!(file.set_pinned(&["Paint".to_owned()]));
        assert!(file.transfer(place(0, Slot::Start, 0), place(0, Slot::End, 0)));
        file.config().expect("it stays usable");
    }
}

// The values of the settings extensions and themes declare, and the themes
// stacked over the base theme.
impl ConfigFile {
    /// Sets the setting `key` of the extension `id`, under
    /// `[extension.<id>]`; `None` takes it away, and the table with its
    /// last key.
    pub fn set_extension_setting(&mut self, id: &str, key: &str, value: Option<Value>) -> bool {
        self.set_nested("extension", id, key, value)
    }

    /// Sets the setting `key` of the theme `theme`, under
    /// `[theme_options.<theme>]`; `None` takes it away, and the table with
    /// its last key.
    pub fn set_theme_option(&mut self, theme: &str, key: &str, value: Option<Value>) -> bool {
        self.set_nested("theme_options", theme, key, value)
    }

    /// Names the themes stacked over the base theme, in order; none takes
    /// the key away.
    pub fn set_theme_layers(&mut self, layers: &[String]) {
        self.set_key("theme_layers", list(layers));
    }

    /// Sets `key` of the table `name` within the table `outer`, both made
    /// for it where they are missing and taken away with their last key.
    /// Whether they are tables keys can be set in is returned.
    fn set_nested(&mut self, outer: &str, name: &str, key: &str, value: Option<Value>) -> bool {
        if value.is_none() && self.document.get(outer).is_none() {
            return true;
        }
        let mut made = Table::new();
        // Only `[outer.name]` is written, not `[outer]` above it.
        made.set_implicit(true);
        let tables = self.document.entry(outer).or_insert(Item::Table(made));
        let Some(tables) = tables.as_table_like_mut() else {
            return false;
        };
        if value.is_none() && tables.get(name).is_none() {
            return true;
        }
        let table = tables.entry(name).or_insert(Item::Table(Table::new()));
        let Some(entries) = table.as_table_like_mut() else {
            return false;
        };
        match value {
            Some(value) => {
                entries.insert(key, Item::Value(value));
            }
            None => {
                entries.remove(key);
            }
        }
        if entries.is_empty() {
            tables.remove(name);
        }
        if tables.is_empty() {
            self.document.remove(outer);
        }
        true
    }
}

#[cfg(test)]
mod declared_settings_tests {
    use super::*;

    const MINE: &str =
        "# Mine.\ntheme = \"ocean\"\n\n[extension.weather]\n# Berlin.\nlatitude = 52.52\n";

    #[test]
    fn settings_of_extensions_are_set_in_their_tables_and_taken_away_again() {
        let mut file = ConfigFile::parse(MINE).unwrap();
        assert!(file.set_extension_setting("weather", "unit", Some("fahrenheit".into())));
        assert!(file.set_extension_setting("crypto", "coins", Some("bitcoin, monero".into())));
        let config = file.config().unwrap();
        assert_eq!(
            config.extensions["weather"]["unit"].as_str(),
            Some("fahrenheit")
        );
        assert_eq!(
            config.extensions["crypto"]["coins"].as_str(),
            Some("bitcoin, monero")
        );
        let text = file.document.to_string();
        assert!(text.contains("# Berlin."));
        assert!(!text.contains("[extension]\n"));
        assert!(file.set_extension_setting("weather", "unit", None));
        assert!(file.set_extension_setting("crypto", "coins", None));
        assert!(file.set_extension_setting("nothing", "coins", None));
        assert_eq!(file.document.to_string(), MINE);
        assert!(file.set_extension_setting("weather", "latitude", None));
        assert!(!file.document.to_string().contains("extension"));
    }

    #[test]
    fn settings_of_themes_and_layers_are_written_and_forgotten() {
        let mut file = ConfigFile::parse(MINE).unwrap();
        assert!(file.set_theme_option("ocean", "corners", Some(1.5.into())));
        assert!(file.set_theme_option("builtin", "accent", Some("#3d7bd9".into())));
        file.set_theme_layers(&["dense".to_owned(), "round".to_owned()]);
        let config = file.config().unwrap();
        assert_eq!(config.theme_layers, ["dense", "round"]);
        assert_eq!(
            config.theme_options["ocean"]["corners"].as_float(),
            Some(1.5)
        );
        assert_eq!(
            config.theme_options["builtin"]["accent"].as_str(),
            Some("#3d7bd9")
        );
        assert!(file.set_theme_option("ocean", "corners", None));
        assert!(file.set_theme_option("builtin", "accent", None));
        file.set_theme_layers(&[]);
        assert_eq!(file.document.to_string(), MINE);
        let mut odd = ConfigFile::parse("theme_options = 3\n").unwrap();
        assert!(!odd.set_theme_option("ocean", "corners", Some(1.into())));
    }
}

#[cfg(test)]
mod icon_pack_tests {
    use super::*;

    const LAYOUT: &str = r#"
[[surface]]
edge = "bottom"
thickness = 64
center = ["windows"]

[[surface]]
edge = "top"
thickness = 48
end = ["clock"]

[wallpaper]
video = "C:/a.mp4"
fit = "fill"
"#;

    #[test]
    fn the_marketplace_writes_packs_wallpapers_and_widgets() {
        let mut file = ConfigFile::parse(LAYOUT).unwrap();
        file.set_icon_packs(&["line".to_owned(), "other".to_owned()]);
        assert!(file.apply_wallpaper(std::path::Path::new("C:/w/hills.jpg")));
        assert!(file.place_widget(false, Slot::End, "lights.button"));
        assert!(file.place_widget(true, Slot::Start, "lights.button"));
        let config = file.config().unwrap();
        assert_eq!(config.icon_packs, ["line", "other"]);
        assert_eq!(config.surfaces[1].end.len(), 2);
        assert_eq!(config.surfaces[1].end[1].widget, "lights.button");
        assert_eq!(config.surfaces[0].start[0].widget, "lights.button");
        let table = file.table().unwrap();
        let wallpaper = table["wallpaper"].as_table().unwrap();
        assert_eq!(wallpaper["source"].as_str(), Some("files"));
        assert_eq!(wallpaper["images"][0].as_str(), Some("C:/w/hills.jpg"));
        assert_eq!(wallpaper["fit"].as_str(), Some("fill"));
        assert!(!wallpaper.contains_key("video"));
        file.set_icon_packs(&[]);
        assert!(file.config().unwrap().icon_packs.is_empty());
    }
}

// The changes the menu of a bar makes to its surface.
impl ConfigFile {
    /// How many surfaces the file has.
    fn surface_count(&self) -> usize {
        let surfaces = self.document.get("surface");
        surfaces
            .and_then(Item::as_array_of_tables)
            .map_or(0, ArrayOfTables::len)
    }

    /// Moves a surface to another edge of its displays.
    pub fn set_surface_edge(&mut self, surface: usize, edge: crate::platform::ScreenEdge) -> bool {
        self.set_surface(surface, "edge", Some(Value::from(edge.name())))
    }

    /// Makes a surface `thickness` logical pixels thick.
    pub fn set_surface_thickness(&mut self, surface: usize, thickness: f32) -> bool {
        self.set_surface(surface, "thickness", Some(length_value(thickness)))
    }

    /// Has a surface go away as `hide` says, or never.
    pub fn set_surface_hide(&mut self, surface: usize, hide: super::Hiding) -> bool {
        let written = match hide {
            super::Hiding::Never => None,
            super::Hiding::Pointer => Some(Value::from(true)),
            super::Hiding::Overlap => Some(Value::from("overlap")),
        };
        self.set_surface(surface, "hide", written)
    }

    /// Has a surface take `share` of the length of its edge; the whole
    /// length takes `length` and `align` out of the file.
    pub fn set_surface_length(&mut self, surface: usize, share: f32) -> bool {
        if share >= 1. {
            return self.set_surface(surface, "length", None)
                && self.set_surface(surface, "align", None);
        }
        let share = (f64::from(share) * 100.).round() / 100.;
        self.set_surface(surface, "length", Some(Value::from(share)))
    }

    /// Has a surface shorter than its edge stand at `align` of it.
    pub fn set_surface_align(&mut self, surface: usize, align: super::SpanAlign) -> bool {
        let written = (align != super::SpanAlign::default()).then(|| Value::from(align.name()));
        self.set_surface(surface, "align", written)
    }

    /// Has a surface appear on `displays`.
    pub fn set_surface_displays(&mut self, surface: usize, displays: &super::Displays) -> bool {
        self.set_surface(surface, "displays", displays_value(displays))
    }

    /// Appends a surface at `edge` of `displays`, `thickness` logical
    /// pixels thick, with a background, so that it is seen, and a clock in
    /// its middle where it runs across the display.
    pub fn add_surface(
        &mut self,
        edge: crate::platform::ScreenEdge,
        thickness: f32,
        displays: &super::Displays,
    ) -> bool {
        let item = self
            .document
            .entry("surface")
            .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
        let Some(surfaces) = item.as_array_of_tables_mut() else {
            return false;
        };
        let mut surface = Table::new();
        surface.insert("edge", value(edge.name()));
        surface.insert("thickness", Item::Value(length_value(thickness)));
        if let Some(displays) = displays_value(displays) {
            surface.insert("displays", Item::Value(displays));
        }
        surface.insert("opacity", value(0.8));
        if !edge.is_vertical() {
            let mut center = Array::new();
            center.push("clock");
            surface.insert("center", value(center));
        }
        surfaces.push(surface);
        true
    }

    /// Takes a surface out of the file, unless it is the last one: the
    /// shell needs one.
    pub fn remove_surface(&mut self, surface: usize) -> bool {
        if surface >= self.surface_count() || self.surface_count() < 2 {
            return false;
        }
        let surfaces = self.document.get_mut("surface");
        let Some(surfaces) = surfaces.and_then(Item::as_array_of_tables_mut) else {
            return false;
        };
        surfaces.remove(surface);
        true
    }
}

/// A length as the file writes it: whole where it is whole.
fn length_value(length: f32) -> Value {
    if length.fract() == 0. && length.abs() < 1e9 {
        Value::from(length as i64)
    } else {
        Value::from(f64::from(length))
    }
}

/// `displays` as the file writes it; `None` for every display, which is
/// what a surface that says nothing appears on.
fn displays_value(displays: &super::Displays) -> Option<Value> {
    use super::{DisplaySelector, Displays};
    Some(match displays {
        Displays::All => return None,
        Displays::Primary => Value::from("primary"),
        Displays::Others => Value::from("others"),
        Displays::Chosen(selectors) => {
            let mut list = Array::new();
            for selector in selectors {
                match selector {
                    DisplaySelector::Position(position) => list.push(*position as i64),
                    other => list.push(other.text()),
                }
            }
            Value::Array(list)
        }
    })
}

#[cfg(test)]
mod surfaces_edges_tests {
    use super::*;
    use crate::config::{DisplaySelector, Displays, Hiding, SpanAlign};
    use crate::platform::ScreenEdge;

    #[test]
    fn the_menu_of_a_bar_changes_its_surface() {
        let mut file = ConfigFile::parse(DEFAULT).unwrap();
        assert!(file.set_surface_edge(0, ScreenEdge::Left));
        assert!(file.set_surface_thickness(0, 52.));
        assert!(file.set_surface_hide(0, Hiding::Overlap));
        assert!(file.set_surface_length(0, 0.5));
        assert!(file.set_surface_align(0, SpanAlign::End));
        let chosen = Displays::Chosen(vec![
            DisplaySelector::Position(2),
            DisplaySelector::Name("LG*".into()),
        ]);
        assert!(file.set_surface_displays(0, &chosen));
        let config = file.config().expect("it stays usable");
        let bar = &config.surfaces[0];
        assert_eq!(
            (bar.edge, bar.thickness, bar.hide),
            (ScreenEdge::Left, 52., Hiding::Overlap)
        );
        assert_eq!((bar.length, bar.align), (0.5, SpanAlign::End));
        assert_eq!(bar.displays, chosen);
        assert!(file.document.to_string().contains("thickness = 52\n"));

        assert!(file.set_surface_length(0, 1.));
        assert!(file.set_surface_hide(0, Hiding::Never));
        assert!(file.set_surface_displays(0, &Displays::All));
        let config = file.config().unwrap();
        let bar = &config.surfaces[0];
        assert_eq!(
            (bar.length, bar.hide, &bar.displays),
            (1., Hiding::Never, &Displays::All)
        );
        assert!(file.document["surface"][0].get("length").is_none());
        assert!(file.document["surface"][0].get("align").is_none());
        assert!(!file.set_surface_edge(9, ScreenEdge::Top));
    }

    #[test]
    fn a_bar_is_added_and_removed_but_the_last_stays() {
        let mut file = ConfigFile::parse(DEFAULT).unwrap();
        let before = file.config().unwrap().surfaces.len();
        assert!(file.add_surface(ScreenEdge::Right, 40., &Displays::Primary));
        assert!(file.add_surface(ScreenEdge::Top, 40., &Displays::All));
        let config = file.config().expect("it stays usable");
        assert_eq!(config.surfaces.len(), before + 2);
        let right = &config.surfaces[before];
        assert_eq!(
            (right.edge, &right.displays),
            (ScreenEdge::Right, &Displays::Primary)
        );
        assert!(right.center.is_empty());
        assert_eq!(right.opacity(), Some(0.8));
        assert_eq!(config.surfaces[before + 1].center.len(), 1);
        for _ in 0..before + 1 {
            assert!(file.remove_surface(0));
        }
        assert!(!file.remove_surface(0));
        assert_eq!(file.config().unwrap().surfaces.len(), 1);
    }
}

// wallpaper-looks: the wallpaper, its displays, workspaces and collections.
impl ConfigFile {
    /// The table `[wallpaper]`, made where there is none; `None` where
    /// the file has something else under that name.
    fn wallpaper_table(&mut self) -> Option<&mut Table> {
        let item = self.document.entry("wallpaper");
        let item = item.or_insert_with(|| Item::Table(Table::new()));
        item.as_table_mut()
    }

    /// The entries `[[wallpaper.<kind>]]`, made where there are none.
    fn wallpaper_tables(&mut self, kind: &str) -> Option<&mut ArrayOfTables> {
        let item = self.wallpaper_table()?.entry(kind);
        let item = item.or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
        item.as_array_of_tables_mut()
    }

    /// The entries `[[wallpaper.<kind>]]` as they are.
    fn wallpaper_entries(&self, kind: &str) -> impl Iterator<Item = &Table> {
        let entries = { self.document.get("wallpaper") }
            .and_then(Item::as_table)
            .and_then(|wallpaper| wallpaper.get(kind))
            .and_then(Item::as_array_of_tables);
        entries.into_iter().flat_map(ArrayOfTables::iter)
    }

    /// Takes away `[wallpaper]` and its entries of `kind` where nothing is
    /// left in them.
    fn tidy_wallpaper(&mut self, kind: &str) {
        let wallpaper = self.document.get_mut("wallpaper");
        let Some(wallpaper) = wallpaper.and_then(Item::as_table_mut) else {
            return;
        };
        let empty = { wallpaper.get(kind) }
            .and_then(Item::as_array_of_tables)
            .is_some_and(ArrayOfTables::is_empty);
        if empty {
            wallpaper.remove(kind);
        }
        if wallpaper.is_empty() {
            self.document.remove("wallpaper");
        }
    }

    /// Sets `key` of `[wallpaper]`; `None` takes it away. Whether the
    /// file has a `[wallpaper]` keys can be set in is returned.
    pub fn set_wallpaper(&mut self, key: &str, value: Option<Value>) -> bool {
        self.set_in("wallpaper", key, value)
    }

    /// Sets `key` of the entry `[[wallpaper.<kind>]]` whose `number` is
    /// `number`, `kind` being `display` or `workspace`; the entry is made
    /// where there is none, and taken away where only its number is left.
    pub fn set_wallpaper_place(
        &mut self,
        kind: &str,
        number: usize,
        key: &str,
        value: Option<Value>,
    ) -> bool {
        let Some(entries) = self.wallpaper_tables(kind) else {
            return false;
        };
        let numbered = |entry: &Table| {
            let of = entry.get("number").and_then(Item::as_integer);
            of == i64::try_from(number).ok()
        };
        let found = entries.iter().position(numbered);
        let at = match (found, &value) {
            (Some(at), _) => at,
            (None, None) => {
                self.tidy_wallpaper(kind);
                return true;
            }
            (None, Some(_)) => {
                let mut entry = Table::new();
                entry.insert("number", value_of_number(number));
                entries.push(entry);
                entries.len() - 1
            }
        };
        let Some(entry) = entries.get_mut(at) else {
            return false;
        };
        match value {
            Some(value) => {
                entry.insert(key, Item::Value(value));
            }
            None => {
                entry.remove(key);
            }
        }
        if entry.len() == 1 && entry.contains_key("number") {
            entries.remove(at);
        }
        self.tidy_wallpaper(kind);
        true
    }

    /// Takes `key` away from every entry `[[wallpaper.<kind>]]`, and the
    /// entries where only their number is left.
    pub fn clear_wallpaper_places(&mut self, kind: &str, key: &str) {
        for number in self.wallpaper_numbers(kind) {
            self.set_wallpaper_place(kind, number, key, None);
        }
    }

    /// The numbers of the entries `[[wallpaper.<kind>]]`.
    fn wallpaper_numbers(&self, kind: &str) -> Vec<usize> {
        { self.wallpaper_entries(kind) }
            .filter_map(|entry| entry.get("number")?.as_integer())
            .filter_map(|number| usize::try_from(number).ok())
            .collect()
    }

    /// Adds a collection `[[wallpaper.collections]]` named `name` with the
    /// pictures and folders `images`, after the others.
    pub fn add_wallpaper_collection(&mut self, name: &str, images: &[PathBuf]) -> bool {
        let Some(collections) = self.wallpaper_tables("collections") else {
            return false;
        };
        let mut entry = Table::new();
        entry.insert("name", value(name));
        let images: Array = { images.iter() }
            .map(|image| image.to_string_lossy().into_owned())
            .collect();
        entry.insert("images", Item::Value(Value::Array(images)));
        collections.push(entry);
        true
    }

    /// Takes away the collection at `index`, and every `collection` that
    /// names it.
    pub fn remove_wallpaper_collection(&mut self, index: usize) -> bool {
        let Some(collections) = self.wallpaper_tables("collections") else {
            return false;
        };
        let name = { collections.get(index) }
            .and_then(|entry| entry.get("name")?.as_str())
            .map(str::to_owned);
        let Some(name) = name else {
            self.tidy_wallpaper("collections");
            return false;
        };
        collections.remove(index);
        self.tidy_wallpaper("collections");
        let chosen = { self.document.get("wallpaper") }
            .and_then(Item::as_table)
            .and_then(|wallpaper| wallpaper.get("collection")?.as_str())
            == Some(name.as_str());
        if chosen {
            self.set_wallpaper("collection", None);
        }
        for kind in ["display", "workspace"] {
            let showing: Vec<usize> = { self.wallpaper_entries(kind) }
                .filter(|entry| entry.get("collection").and_then(Item::as_str) == Some(&name))
                .filter_map(|entry| entry.get("number")?.as_integer())
                .filter_map(|number| usize::try_from(number).ok())
                .collect();
            for number in showing {
                self.set_wallpaper_place(kind, number, "collection", None);
            }
        }
        true
    }

    /// Moves the collection at `index` one place on, or back, among the
    /// collections. Whether there was a place to move it to is returned.
    pub fn move_wallpaper_collection(&mut self, index: usize, forward: bool) -> bool {
        let Some(collections) = self.wallpaper_tables("collections") else {
            return false;
        };
        let other = if forward {
            index + 1
        } else {
            index.wrapping_sub(1)
        };
        if index >= collections.len() || other >= collections.len() {
            self.tidy_wallpaper("collections");
            return false;
        }
        let mut entries: Vec<Table> = collections.iter().cloned().collect();
        // The file is written in the order of the places tables were read
        // at, so the two take each other's place there too.
        let places: Vec<Option<isize>> = entries.iter().map(Table::position).collect();
        entries.swap(index, other);
        collections.clear();
        for (mut entry, place) in entries.into_iter().zip(places) {
            entry.set_position(place);
            collections.push(entry);
        }
        true
    }

    /// Where among the collections the one named `name` is.
    pub fn wallpaper_collection_index(&self, name: &str) -> Option<usize> {
        { self.wallpaper_entries("collections") }
            .position(|entry| entry.get("name").and_then(Item::as_str) == Some(name))
    }

    /// Sets `key` of the collection named `name`; `None` takes it away.
    /// Whether there is such a collection is returned.
    pub fn set_wallpaper_collection(
        &mut self,
        name: &str,
        key: &str,
        value: Option<Value>,
    ) -> bool {
        let Some(index) = self.wallpaper_collection_index(name) else {
            return false;
        };
        let entry = { self.wallpaper_tables("collections") }
            .and_then(|collections| collections.get_mut(index));
        let Some(entry) = entry else {
            return false;
        };
        match value {
            Some(value) => {
                entry.insert(key, Item::Value(value));
            }
            None => {
                entry.remove(key);
            }
        }
        true
    }
}

/// `number` as a value of the file.
fn value_of_number(number: usize) -> Item {
    value(i64::try_from(number).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod wallpaper_tests {
    use super::*;

    fn file(text: &str) -> ConfigFile {
        ConfigFile::parse(text).unwrap()
    }

    #[test]
    fn keys_of_a_display_are_set_in_an_entry_of_its_own() {
        let mut edited = file("# Mine.\n[wallpaper]\nfit = \"cover\" # kept\n");
        assert!(edited.set_wallpaper("blur", Some(8.into())));
        assert!(edited.set_wallpaper_place("display", 2, "collection", Some("sea".into())));
        assert!(edited.set_wallpaper_place("display", 2, "brightness", Some(0.5.into())));
        let text = edited.document.to_string();
        assert!(text.contains("# kept"));
        assert!(text.contains("[[wallpaper.display]]\nnumber = 2\ncollection = \"sea\""));
        assert!(text.contains("blur = 8"));
        // Without its keys the entry goes, and so does the table.
        assert!(edited.set_wallpaper_place("display", 2, "collection", None));
        assert!(edited.set_wallpaper_place("display", 2, "brightness", None));
        assert!(
            !edited
                .document
                .to_string()
                .contains("[[wallpaper.display]]")
        );
        let mut empty = file("");
        assert!(empty.set_wallpaper_place("workspace", 1, "blur", None));
        assert_eq!(empty.document.to_string(), "");
    }

    #[test]
    fn collections_are_added_moved_and_taken_away_with_their_names() {
        let mut edited = file("");
        assert!(edited.add_wallpaper_collection("sea", &[PathBuf::from(r"C:\Sea")]));
        assert!(edited.add_wallpaper_collection("hills", &[]));
        assert!(edited.set_wallpaper("collection", Some("sea".into())));
        assert!(edited.set_wallpaper_place("display", 1, "collection", Some("sea".into())));
        edited.config().expect("it is usable");
        let names = |edited: &ConfigFile| -> Vec<String> {
            let table = edited.table().unwrap();
            let collections = table["wallpaper"]["collections"]
                .as_array()
                .unwrap()
                .clone();
            { collections.iter() }
                .map(|entry| entry["name"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(names(&edited), ["sea", "hills"]);
        assert_eq!(edited.wallpaper_collection_index("hills"), Some(1));
        assert_eq!(edited.wallpaper_collection_index("lakes"), None);
        assert!(edited.set_wallpaper_collection("hills", "interval", Some(60.into())));
        assert!(!edited.set_wallpaper_collection("lakes", "interval", None));
        assert!(edited.document.to_string().contains("interval = 60"));
        assert!(edited.set_wallpaper_collection("hills", "interval", None));
        assert!(!edited.document.to_string().contains("interval"));
        assert!(edited.move_wallpaper_collection(0, true));
        assert_eq!(names(&edited), ["hills", "sea"]);
        assert!(!edited.move_wallpaper_collection(0, false));
        assert!(edited.remove_wallpaper_collection(1));
        assert_eq!(names(&edited), ["hills"]);
        let text = edited.document.to_string();
        assert!(!text.contains("collection = \"sea\""), "{text}");
        assert!(!text.contains("[[wallpaper.display]]"));
        edited.config().expect("it stays usable");
    }

    #[test]
    fn a_color_set_alone_keeps_where_the_wallpaper_comes_from() {
        use crate::wallpaper::{Edit, Source};
        let colored = |text: &str| {
            let mut edited = file(text);
            assert!(Edit::KeepSource.apply(&mut edited));
            assert!(Edit::Key("color", Some("#102030".into())).apply(&mut edited));
            edited.config().unwrap().wallpaper.source()
        };
        assert_eq!(colored(""), Source::Windows);
        assert_eq!(
            colored("[wallpaper]\nimages = [\"a.jpg\"]\n"),
            Source::Files
        );
        assert_eq!(colored("[wallpaper]\nsource = \"color\"\n"), Source::Color);
    }
}

// settings-core: keys at any depth of a table, the options of one placed
// widget, and of widgets named so on popups' second column and on cards.

/// One placed widget, as the Widgets section of the settings and its
/// `open_at` item name it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WidgetPlace {
    /// The widget at `index` of a slot of the surface at `surface`.
    Surface {
        surface: usize,
        slot: Slot,
        index: usize,
    },
    /// The widget at `index` of the popup `name`, counting its `content`
    /// and then its `side`.
    Popup { name: String, index: usize },
    /// The widget at `widget` of the card of the desktop at `index`.
    Card { index: usize, widget: usize },
}

impl WidgetPlace {
    /// The place an item of `Settings::open_at` names:
    /// `surface/<n>/<start|center|end>/<i>`, `popup/<name>/<i>`, or
    /// `card/<i>` for the first widget of a card and `card/<i>/<j>`.
    pub fn parse(item: &str) -> Option<Self> {
        let parts: Vec<&str> = item.split('/').collect();
        let number = |text: &str| text.parse::<usize>().ok();
        match parts.as_slice() {
            ["surface", surface, slot, index] => Some(Self::Surface {
                surface: number(surface)?,
                slot: Slot::ALL.into_iter().find(|known| known.key() == *slot)?,
                index: number(index)?,
            }),
            ["popup", name, index] if !name.is_empty() => Some(Self::Popup {
                name: (*name).to_owned(),
                index: number(index)?,
            }),
            ["card", index] => Some(Self::Card {
                index: number(index)?,
                widget: 0,
            }),
            ["card", index, widget] => Some(Self::Card {
                index: number(index)?,
                widget: number(widget)?,
            }),
            _ => None,
        }
    }

    /// The item that names it, as [`Self::parse`] reads it.
    pub fn item(&self) -> String {
        match self {
            Self::Surface {
                surface,
                slot,
                index,
            } => format!("surface/{surface}/{}/{index}", slot.key()),
            Self::Popup { name, index } => format!("popup/{name}/{index}"),
            Self::Card { index, widget: 0 } => format!("card/{index}"),
            Self::Card { index, widget } => format!("card/{index}/{widget}"),
        }
    }

    /// The widget at this place of `config`.
    pub fn spec<'a>(&self, config: &'a Config) -> Option<&'a super::WidgetSpec> {
        match self {
            Self::Surface {
                surface,
                slot,
                index,
            } => {
                let surface = config.surfaces.get(*surface)?;
                let widgets = match slot {
                    Slot::Start => &surface.start,
                    Slot::Center => &surface.center,
                    Slot::End => &surface.end,
                };
                widgets.get(*index)
            }
            Self::Popup { name, index } => {
                let popup = config.popups.iter().find(|popup| popup.name == *name)?;
                popup.widgets().nth(*index)
            }
            Self::Card { index, widget } => config.cards.get(*index)?.content.get(*widget),
        }
    }
}

impl ConfigFile {
    /// Sets the key at `path`, each part below the one before, as
    /// `["tiling", "border", "width"]`; the tables on the way are made
    /// where they are missing, and those left empty are taken away with
    /// the key. `None` takes the key away. Comments stay. Whether every
    /// part on the way is a table keys can be set in is returned.
    pub fn set_at(&mut self, path: &[&str], value: Option<Value>) -> bool {
        fn set(table: &mut dyn toml_edit::TableLike, path: &[&str], value: Option<Value>) -> bool {
            let Some((first, rest)) = path.split_first() else {
                return false;
            };
            if rest.is_empty() {
                match value {
                    Some(value) => {
                        table.insert(first, Item::Value(value));
                    }
                    None => {
                        table.remove(first);
                    }
                }
                return true;
            }
            if value.is_none() && table.get(first).is_none() {
                return true;
            }
            let item = table.entry(first).or_insert_with(|| {
                let mut made = Table::new();
                // `[outer.inner]` is written without an empty `[outer]`.
                made.set_implicit(true);
                Item::Table(made)
            });
            let Some(inner) = item.as_table_like_mut() else {
                return false;
            };
            let done = set(inner, rest, value);
            if inner.is_empty() {
                table.remove(first);
            }
            done
        }
        set(self.document.as_table_mut(), path, value)
    }

    /// Sets the option `key` of the one widget at `place`, which becomes
    /// an inline table where it is written by its name alone and its name
    /// again once it has no other option; `None` takes the option away.
    /// Whether there is a widget at `place` is returned.
    pub fn set_widget_option(
        &mut self,
        place: &WidgetPlace,
        key: &str,
        value: Option<Value>,
    ) -> bool {
        let Some(widget) = widget_at_place(&mut self.document, place) else {
            return false;
        };
        let name = match widget {
            Value::String(name) => name.value().clone(),
            Value::InlineTable(table) => match table.get("widget").and_then(Value::as_str) {
                Some(name) => name.to_owned(),
                None => return false,
            },
            _ => return false,
        };
        Self::option(widget, &name, key, value.as_ref())
    }
}

/// The entry of the widget at `place` in `document`.
fn widget_at_place<'a>(
    document: &'a mut DocumentMut,
    place: &WidgetPlace,
) -> Option<&'a mut Value> {
    match place {
        WidgetPlace::Surface {
            surface,
            slot,
            index,
        } => {
            let surfaces = document.get_mut("surface")?.as_array_of_tables_mut()?;
            let widgets = surfaces.get_mut(*surface)?.get_mut(slot.key())?;
            widgets.as_array_mut()?.get_mut(*index)
        }
        WidgetPlace::Popup { name, index } => {
            let popups = document.get_mut("popup")?.as_array_of_tables_mut()?;
            let popup = { popups.iter_mut() }
                .find(|popup| popup.get("name").and_then(Item::as_str) == Some(name))?;
            let content = popup
                .get("content")
                .and_then(Item::as_array)
                .map_or(0, Array::len);
            let (column, at) = match index.checked_sub(content) {
                Some(at) => ("side", at),
                None => ("content", *index),
            };
            popup.get_mut(column)?.as_array_mut()?.get_mut(at)
        }
        WidgetPlace::Card { index, widget } => {
            let cards = document.get_mut("card")?.as_array_of_tables_mut()?;
            let content = cards.get_mut(*index)?.get_mut("content")?;
            content.as_array_mut()?.get_mut(*widget)
// widget-options: the popups of the file and the widgets in them, as the
// popup page of the settings changes them.
impl ConfigFile {
    /// The `[[popup]]` of the file named `name`.
    fn popup_table(&mut self, name: &str) -> Option<&mut Table> {
        let popups = self.document.get_mut("popup")?.as_array_of_tables_mut()?;
        { popups.iter_mut() }.find(|popup| popup.get("name").and_then(Item::as_str) == Some(name))
    }

    /// Appends a `[[popup]]` that holds what `popup` holds. False where
    /// its name is empty or the file has a popup of that name already.
    pub fn add_popup(&mut self, popup: &super::PopupConfig) -> bool {
        let name = popup.name.trim();
        if name.is_empty() || self.popup_table(name).is_some() {
            return false;
        }
        let mut table = Table::new();
        table.insert("name", value(name));
        if let Some(place) = popup_place_name(popup.place) {
            table.insert("place", value(place));
        }
        table.insert("width", Item::Value(length_value(popup.width)));
        table.insert("height", Item::Value(length_value(popup.height)));
        let column = |specs: &[super::WidgetSpec]| {
            let mut widgets: Array = specs.iter().map(spec_value).collect();
            widgets.fmt();
            Item::Value(Value::Array(widgets))
        };
        table.insert("content", column(&popup.content));
        if !popup.side.is_empty() {
            table.insert("side", column(&popup.side));
        }
        let item = { self.document.entry("popup") }
            .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
        let Some(popups) = item.as_array_of_tables_mut() else {
            return false;
        };
        popups.push(table);
        true
    }

    /// Takes the `[[popup]]` named `name` out of the file; whatever opens
    /// it then opens the popup of that name a module brings, if one does.
    pub fn remove_popup(&mut self, name: &str) -> bool {
        let popups = { self.document.get_mut("popup") }.and_then(Item::as_array_of_tables_mut);
        let Some(popups) = popups else {
            return false;
        };
        let found = { popups.iter() }
            .position(|popup| popup.get("name").and_then(Item::as_str) == Some(name));
        let Some(index) = found else {
            return false;
        };
        popups.remove(index);
        if popups.is_empty() {
            self.document.remove("popup");
        }
        true
    }

    /// Names the `[[popup]]` named `from` `to`, and so does whatever opens
    /// it: the `popup` of a widget, the `start` of a dock, and the `popup`
    /// and the `popup` action of a `[[key]]`. False where `to` is empty or
    /// taken, or there is no popup `from`.
    pub fn rename_popup(&mut self, from: &str, to: &str) -> bool {
        let to = to.trim();
        if to.is_empty() || from == to || self.popup_table(to).is_some() {
            return false;
        }
        let Some(popup) = self.popup_table(from) else {
            return false;
        };
        popup.insert("name", value(to));
        self.each_widget(|widget| rename_opened(widget, from, to));
        let keys = { self.document.get_mut("key") }.and_then(Item::as_array_of_tables_mut);
        for key in keys.into_iter().flat_map(|keys| keys.iter_mut()) {
            if key.get("popup").and_then(Item::as_str) == Some(from) {
                key.insert("popup", value(to));
            }
            let opens = key.get("action").and_then(Item::as_str) == Some("popup");
            if opens && key.get("arg").and_then(Item::as_str) == Some(from) {
                key.insert("arg", value(to));
            }
        }
        true
    }

    /// Calls `change` with every widget of the surfaces, the popups and
    /// the cards of the desktop.
    fn each_widget(&mut self, mut change: impl FnMut(&mut Value)) {
        const LISTS: [(&str, &[&str]); 3] = [
            ("surface", &["start", "center", "end"]),
            ("popup", &["content", "side"]),
            ("card", &["content"]),
        ];
        for (tables, keys) in LISTS {
            let entries = { self.document.get_mut(tables) }.and_then(Item::as_array_of_tables_mut);
            for entry in entries.into_iter().flat_map(|entries| entries.iter_mut()) {
                for (key, item) in entry.iter_mut() {
                    if keys.contains(&key.get())
                        && let Some(list) = item.as_array_mut()
                    {
                        list.iter_mut().for_each(&mut change);
                    }
                }
            }
        }
    }

    /// Sets `place`, `width` or `height` of the `[[popup]]` named `name`;
    /// `None` takes it out. False for another key or where there is no
    /// such popup.
    pub fn set_popup_field(&mut self, name: &str, key: &str, value: Option<Value>) -> bool {
        if !POPUP_FIELDS.contains(&key) {
            return false;
        }
        let Some(popup) = self.popup_table(name) else {
            return false;
        };
        match value {
            Some(value) => {
                popup.insert(key, Item::Value(value));
            }
            None => {
                popup.remove(key);
            }
        }
        true
    }

    /// Makes `widgets` the widgets of `column` of the `[[popup]]` named
    /// `name`: those it holds, by their place, with their options and
    /// what is written beside them, and new ones by name. An empty second
    /// column leaves the file. False where there is no such popup or a
    /// place is past its widgets.
    pub fn set_popup_widgets(
        &mut self,
        name: &str,
        column: PopupColumn,
        widgets: &[PopupWidget],
    ) -> bool {
        let Some(popup) = self.popup_table(name) else {
            return false;
        };
        let held: Vec<Value> = { popup.get(column.key()) }
            .and_then(Item::as_array)
            .map(|held| held.iter().cloned().collect())
            .unwrap_or_default();
        let mut list = Array::new();
        for widget in widgets {
            match widget {
                PopupWidget::Kept(index) => match held.get(*index) {
                    Some(kept) => list.push_formatted(kept.clone()),
                    None => return false,
                },
                PopupWidget::New(name) => list.push(name.as_str()),
            }
        }
        list.fmt();
        if list.is_empty() && column == PopupColumn::Side {
            popup.remove(column.key());
        } else {
            popup.insert(column.key(), Item::Value(Value::Array(list)));
        }
// shortcuts-editor: whole `[[key]]` entries, as the Shortcuts page writes
// them. An entry that stays keeps what the user wrote around it, comments
// included.
impl ConfigFile {
    /// Appends `key` as a `[[key]]`.
    pub fn add_key_entry(&mut self, key: &super::KeyConfig) -> bool {
        let item = self
            .document
            .entry("key")
            .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
        let Some(keys) = item.as_array_of_tables_mut() else {
            return false;
        };
        keys.push(key_table(key));
        true
    }

    /// Puts `key` in place of the `[[key]]` at `index`, with the comments
    /// above it.
    pub fn replace_key_entry(&mut self, index: usize, key: &super::KeyConfig) -> bool {
        let Some(entry) = self.hotkey(index) else {
            return false;
        };
        let mut table = key_table(key);
        *table.decor_mut() = entry.decor().clone();
        *entry = table;
        true
    }

    /// Takes the `[[key]]` at `index` out.
    pub fn remove_key_entry(&mut self, index: usize) -> bool {
        self.remove_hotkey(index)
    }

    /// Makes `keys` the `[[key]]` entries of the file, in their order. An
    /// entry the file has already is kept as it is written there.
    pub fn set_key_entries(&mut self, keys: &[super::KeyConfig]) -> bool {
        let mut written: Vec<(super::KeyConfig, Table)> = Vec::new();
        let old = self.document.remove("key");
        if let Some(old) = old.as_ref().and_then(Item::as_array_of_tables) {
            for table in old.iter() {
                if let Some(key) = key_of_table(table) {
                    written.push((key, table.clone()));
                }
            }
        }
        if keys.is_empty() {
            return true;
        }
        let mut entries = ArrayOfTables::new();
        for key in keys {
            let kept = written.iter().position(|(known, _)| known == key);
            let table = match kept {
                Some(at) => written.remove(at).1,
                None => key_table(key),
            };
            entries.push(table);
        }
        self.document.insert("key", Item::ArrayOfTables(entries));
// window-rules-editor: the `[[rule]]` entries changed one at a time by the
// editor of the rules, each keeping the comment above it, and the others
// as they are written.
impl ConfigFile {
    /// The `[[rule]]` entries of the file, made where there are none yet
    /// if `make`.
    fn window_rules(&mut self, make: bool) -> Option<&mut ArrayOfTables> {
        if make {
            let item = { self.document.entry("rule") }
                .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
            return item.as_array_of_tables_mut();
        }
        self.document.get_mut("rule")?.as_array_of_tables_mut()
    }

    /// Takes the `[[rule]]` list away once it is empty.
    fn tidy_window_rules(&mut self) {
        let empty = { self.document.get("rule") }
            .and_then(Item::as_array_of_tables)
            .is_some_and(ArrayOfTables::is_empty);
        if empty {
            self.document.remove("rule");
        }
    }

    /// Puts `rule` in place of the `[[rule]]` at `index`, keeping the
    /// comment above it and its place in the file. Whether there is one
    /// there is returned.
    pub fn set_window_rule(&mut self, index: usize, rule: &toml::Table) -> bool {
        let Some(old) = self
            .window_rules(false)
            .and_then(|rules| rules.get_mut(index))
        else {
            return false;
        };
        let mut new = rule_table(rule);
        *new.decor_mut() = old.decor().clone();
        new.set_position(old.position());
        *old = new;
        true
    }

    /// Puts `rule` as a `[[rule]]` before the one at `index`, or after the
    /// last one where `index` is how many there are. Whether there is such
    /// a place is returned.
    pub fn insert_window_rule(&mut self, index: usize, rule: &toml::Table) -> bool {
        let Some(rules) = self.window_rules(true) else {
            return false;
        };
        if index > rules.len() {
            self.tidy_window_rules();
            return false;
        }
        if index == rules.len() {
            return self.add_rule(rule_table(rule), "");
        }
        let mut new = rule_table(rule);
        new.decor_mut().set_prefix("\n");
        // The file is written in the order of the places tables were read
        // at; the new one takes the place of the one it goes before, and
        // comes first of the two.
        let place = rules.get(index).and_then(Table::position);
        new.set_position(place);
        rules.insert(index, new);
        true
    }

    /// Takes the `[[rule]]` at `index` out, with the comment above it.
    /// Whether there was one is returned.
    pub fn remove_window_rule(&mut self, index: usize) -> bool {
        let Some(rules) = self.window_rules(false) else {
            return false;
        };
        if index >= rules.len() {
            return false;
        }
        rules.remove(index);
        self.tidy_window_rules();
        true
    }

    /// Moves the `[[rule]]` at `from` to `to` among the rules, with the
    /// comment above it; the others keep their order. Whether both are
    /// places of rules is returned.
    pub fn move_window_rule(&mut self, from: usize, to: usize) -> bool {
        let Some(rules) = self.window_rules(false) else {
            return false;
        };
        if from >= rules.len() || to >= rules.len() {
            return false;
        }
        if from == to {
            return true;
        }
        let mut tables: Vec<Table> = rules.iter().cloned().collect();
        // The places tables were read at stay where they are, in order, and
        // the tables take them in their new order.
        let places: Vec<Option<isize>> = tables.iter().map(Table::position).collect();
        let moved = tables.remove(from);
        tables.insert(to, moved);
        rules.clear();
        for (mut table, place) in tables.into_iter().zip(places) {
            table.set_position(place);
            rules.push(table);
        }
        true
    }
}

/// The keys of a `[[popup]]` that `set_popup_field` sets.
const POPUP_FIELDS: [&str; 3] = ["place", "width", "height"];

/// One of the two columns of widgets of a popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PopupColumn {
    /// `content`, stacked from the top.
    Content,
    /// `side`, the second column at the right.
    Side,
}

impl PopupColumn {
    pub const ALL: [Self; 2] = [Self::Content, Self::Side];

    fn key(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Side => "side",
        }
    }
}

/// A widget of a column as `set_popup_widgets` lays them out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PopupWidget {
    /// The one at this place of the column now, as it is written.
    Kept(usize),
    /// A new one, of this name, without options.
    New(String),
}

/// What the file writes for where a popup opens; nothing for the place
/// next to the bar, which it takes without the key.
pub fn popup_place_name(place: super::PopupPlace) -> Option<&'static str> {
    match place {
        super::PopupPlace::Surface => None,
        super::PopupPlace::Center => Some("center"),
        super::PopupPlace::Top => Some("top"),
    }
}

/// A widget as the file writes it: its name alone where it has nothing
/// more, a table otherwise.
fn spec_value(spec: &super::WidgetSpec) -> Value {
    if spec.popup.is_none() && spec.options.is_empty() {
        return spec.widget.as_str().into();
    }
    let mut table = InlineTable::new();
    table.insert("widget", spec.widget.as_str().into());
    if let Some(popup) = &spec.popup {
        table.insert("popup", popup.as_str().into());
    }
    for (key, option) in &spec.options {
        table.insert(key, edit_value(option));
    }
    table.fmt();
    Value::InlineTable(table)
}

/// A value read from the file, as it is written again.
fn edit_value(read: &toml::Value) -> Value {
    match read {
/// `rule` as a table of the file: the keys that name windows first, in
/// the order the rules document them, then the others; tables in it are
/// written inline.
fn rule_table(rule: &toml::Table) -> Table {
    let naming = crate::rules::matching::Naming::KEYS;
    let mut table = Table::new();
    for key in naming {
        if let Some(found) = rule.get(key) {
            table.insert(key, Item::Value(edit_value_of(found)));
        }
    }
    for (key, found) in rule
        .iter()
        .filter(|(key, _)| !naming.contains(&key.as_str()))
    {
        table.insert(key, Item::Value(edit_value_of(found)));
    }
    table
}

/// A value read with `toml` as a value of the file, a table in it written
/// inline.
fn edit_value_of(found: &toml::Value) -> Value {
    match found {
        toml::Value::String(text) => text.as_str().into(),
        toml::Value::Integer(number) => (*number).into(),
        toml::Value::Float(number) => (*number).into(),
        toml::Value::Boolean(flag) => (*flag).into(),
        toml::Value::Datetime(moment) => {
            { moment.to_string().parse::<Value>() }.unwrap_or_else(|_| moment.to_string().into())
        }
        toml::Value::Array(items) => {
            let mut list: Array = items.iter().map(edit_value).collect();
            list.fmt();
            Value::Array(list)
        }
        toml::Value::Table(entries) => {
            let mut table = InlineTable::new();
            for (key, entry) in entries {
                table.insert(key, edit_value(entry));
            }
            table.fmt();
            Value::InlineTable(table)
        toml::Value::Array(items) => Value::Array(items.iter().map(edit_value_of).collect()),
        toml::Value::Table(entries) => {
            let mut inline = InlineTable::new();
            for (key, entry) in entries {
                inline.insert(key, edit_value_of(entry));
            }
            Value::InlineTable(inline)
        }
        toml::Value::Datetime(datetime) => {
            { datetime.to_string().parse() }.unwrap_or_else(|_| datetime.to_string().into())
        }
    }
}

/// Sets the option `key` of every widget `name` in the list `column` of
/// every entry of the tables `tables` of `document`, as the second column
/// (`side`) of the popups or the widgets (`content`) of the cards. Whether
/// there is such a widget is returned.
fn set_in_entries(
    document: &mut DocumentMut,
    (tables, column): (&str, &str),
    (name, key): (&str, &str),
    value: Option<&Value>,
) -> bool {
    let entries = document.get_mut(tables);
    let Some(entries) = entries.and_then(Item::as_array_of_tables_mut) else {
        return false;
    };
    let mut found = false;
    for entry in entries.iter_mut() {
        let Some(widgets) = entry.get_mut(column).and_then(Item::as_array_mut) else {
            continue;
        };
        for widget in widgets.iter_mut() {
            found |= ConfigFile::option(widget, name, key, value);
        }
        widgets.fmt();
    }
    found
}

/// The directory of the configuration directory that holds the backups
/// of config.toml.
const BACKUPS_DIRECTORY: &str = "backups";

/// How many backups are kept: the newest.
pub const BACKUPS_KEPT: usize = 20;

/// How long after one backup the next is made by a change, in seconds.
const BACKUP_EVERY: i64 = 60;

/// How the time of a backup is written in its name.
const BACKUP_TIME: &str = "%Y%m%d-%H%M%S";

/// Where the backups of config.toml are kept.
pub fn backups_dir() -> Option<PathBuf> {
    config_dir().map(|directory| directory.join(BACKUPS_DIRECTORY))
}

/// The name of the backup made at `at`: `config-<yyyymmdd-hhmmss>.toml`,
/// the time in UTC.
pub fn backup_name(at: chrono::DateTime<chrono::Utc>) -> String {
    format!("config-{}.toml", at.format(BACKUP_TIME))
}

/// When the backup named `name` was made, in UTC; none for another file.
pub fn backup_time(name: &str) -> Option<chrono::NaiveDateTime> {
    let stamp = name.strip_prefix("config-")?.strip_suffix(".toml")?;
    chrono::NaiveDateTime::parse_from_str(stamp, BACKUP_TIME).ok()
}

/// Whether a change made at `now` makes a backup, the newest being made
/// at `newest`: once a minute at most.
fn backup_due(newest: Option<chrono::NaiveDateTime>, now: chrono::NaiveDateTime) -> bool {
    newest.is_none_or(|newest| (now - newest).num_seconds() >= BACKUP_EVERY)
}

/// The names among `names` that are not backups, or older than the
/// [`BACKUPS_KEPT`] newest, which are taken away.
fn surplus_backups(names: &[String]) -> Vec<String> {
    let mut dated: Vec<(chrono::NaiveDateTime, &String)> = { names.iter() }
        .filter_map(|name| Some((backup_time(name)?, name)))
        .collect();
    dated.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
    { dated.into_iter().skip(BACKUPS_KEPT) }
        .map(|(_, name)| name.clone())
        .collect()
}

/// The backups there are, the newest first, each with when it was made.
pub fn list_backups() -> Vec<(PathBuf, chrono::NaiveDateTime)> {
    let Some(directory) = backups_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut backups: Vec<(PathBuf, chrono::NaiveDateTime)> = { entries.flatten() }
        .filter_map(|entry| {
            let time = backup_time(entry.file_name().to_str()?)?;
            Some((entry.path(), time))
        })
        .collect();
    backups.sort_by_key(|(_, time)| std::cmp::Reverse(*time));
    backups
}

/// Copies config.toml to the backups, unless there is none yet, or,
/// without `always`, a backup was made less than a minute ago; takes away
/// the backups past the newest [`BACKUPS_KEPT`]. The backup made, if any,
/// is returned. Called with the file held by [`ConfigFile::change`] or
/// [`replace_config`].
fn back_up(always: bool) -> Result<Option<PathBuf>, String> {
    let (Some(directory), Ok(path)) = (backups_dir(), ConfigFile::location()) else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    let now = chrono::Utc::now();
    let newest = list_backups().first().map(|(_, time)| *time);
    if !always && !backup_due(newest, now.naive_utc()) {
        return Ok(None);
    }
    let failed =
        |error: std::io::Error| format!("{} cannot be written: {error}", directory.display());
    fs::create_dir_all(&directory).map_err(failed)?;
    let backup = directory.join(backup_name(now));
    fs::copy(&path, &backup).map_err(failed)?;
    let names: Vec<String> = { list_backups().into_iter() }
        .filter_map(|(path, _)| Some(path.file_name()?.to_str()?.to_owned()))
        .collect();
    for name in surplus_backups(&names) {
        fs::remove_file(directory.join(name)).ok();
    }
    Ok(Some(backup))
}

/// Puts `text` in place of config.toml, after backing up what it holds
/// now: an imported file, a backup restored, or the built-in
/// configuration. Text the shell could not use is not written. It reads
/// and writes the disk, so it is called away from the thread of GPUI.
pub fn replace_config(text: &str) -> Result<(), String> {
    let _alone = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let file = ConfigFile::parse(text)?;
    file.config()?;
    back_up(true)?;
    file.save()
}

/// Puts the built-in configuration, comments and all, in place of
/// config.toml, after backing up what it holds now.
pub fn reset_config() -> Result<(), String> {
    replace_config(DEFAULT)
}

#[cfg(test)]
mod settings_core_tests {
#[cfg(test)]
mod window_rules_editor_tests {
    use super::*;

    fn file(text: &str) -> ConfigFile {
        ConfigFile::parse(text).unwrap()
    }

    #[test]
    fn a_key_at_a_path_makes_its_tables_and_takes_them_away_again() {
        let mut edited = file("# Mine.\n[tiling]\ngap = 8 # kept\n");
        assert!(edited.set_at(&["tiling", "border", "width"], Some(4.into())));
        assert!(edited.set_at(&["night_light", "from"], Some("21:30".into())));
        assert!(edited.set_at(&["contrast"], Some(false.into())));
        let text = edited.document.to_string();
        assert!(text.contains("# kept"), "{text}");
        assert!(text.contains("[tiling.border]\nwidth = 4"), "{text}");
        assert!(text.contains("[night_light]\nfrom = \"21:30\""), "{text}");
        assert!(text.contains("contrast = false"), "{text}");
        let config = edited.config().expect("it stays usable");
        assert!(!config.contrast);
        assert!(edited.set_at(&["tiling", "border", "width"], None));
        assert!(edited.set_at(&["night_light", "from"], None));
        let text = edited.document.to_string();
        assert!(!text.contains("border"), "{text}");
        assert!(!text.contains("night_light"), "{text}");
        assert!(text.contains("gap = 8 # kept"), "{text}");
        // Taking away what is not there changes nothing.
        let mut empty = file("");
        assert!(empty.set_at(&["snap", "zones", "gap"], None));
        assert_eq!(empty.document.to_string(), "");
        // A value is no table to set a key in.
        let mut flag = file("[tiling]\nborder = true\n");
        assert!(!flag.set_at(&["tiling", "border", "width"], Some(2.into())));
    }

    #[test]
    fn a_placed_widget_is_named_by_its_item() {
        for item in ["surface/1/center/0", "popup/sound/3", "card/2", "card/2/1"] {
            let place = WidgetPlace::parse(item).expect(item);
            assert_eq!(place.item(), item);
        }
        assert_eq!(WidgetPlace::parse("card/0/0"), WidgetPlace::parse("card/0"));
        for wrong in ["surface/1/middle/0", "popup//1", "card/x", "dock/1", ""] {
            assert_eq!(WidgetPlace::parse(wrong), None, "{wrong}");
        }
    }

    const PLACED: &str = r#"[[surface]]
edge = "top"
thickness = 40
end = ["clock", { widget = "clock", format = "%H:%M" }]

[[popup]]
name = "info"
width = 300
height = 200
content = ["cpu"]
side = ["clock"]

[[card]]
corner = "top-left"
width = 200
height = 100
content = ["memory", "clock"]
"#;

    #[test]
    fn one_widget_gets_an_option_of_its_own() {
        let mut edited = file(PLACED);
        let second = WidgetPlace::parse("surface/0/end/1").unwrap();
        assert!(edited.set_widget_option(&second, "format", Some("%H".into())));
        let first = WidgetPlace::parse("surface/0/end/0").unwrap();
        assert!(edited.set_widget_option(&first, "seconds", Some(true.into())));
        let side = WidgetPlace::parse("popup/info/1").unwrap();
        assert!(edited.set_widget_option(&side, "format", Some("%M".into())));
        let card = WidgetPlace::parse("card/0/1").unwrap();
        assert!(edited.set_widget_option(&card, "format", Some("%S".into())));
        let config = edited.config().expect("it stays usable");
        let option = |place: &WidgetPlace, key: &str| {
            let spec = place.spec(&config)?;
            spec.options.get(key).cloned()
        };
        assert_eq!(option(&second, "format"), Some("%H".into()));
        assert_eq!(option(&first, "seconds"), Some(true.into()));
        assert_eq!(option(&first, "format"), None);
        assert_eq!(option(&side, "format"), Some("%M".into()));
        assert_eq!(option(&card, "format"), Some("%S".into()));
        // Without its last option a widget is written by its name again.
        assert!(edited.set_widget_option(&first, "seconds", None));
        assert!(edited.document.to_string().contains("end = [\"clock\","));
        let nowhere = WidgetPlace::parse("popup/info/5").unwrap();
        assert!(!edited.set_widget_option(&nowhere, "format", None));
    }

    #[test]
    fn a_backup_is_named_by_its_time_and_made_once_a_minute() {
        use chrono::TimeZone;
        let at = chrono::Utc.with_ymd_and_hms(2026, 11, 27, 9, 5, 3).unwrap();
        let name = backup_name(at);
        assert_eq!(name, "config-20261127-090503.toml");
        assert_eq!(backup_time(&name), Some(at.naive_utc()));
        assert_eq!(backup_time("config.toml"), None);
        assert_eq!(backup_time("config-yesterday.toml"), None);
        let later = |seconds| at.naive_utc() + chrono::Duration::seconds(seconds);
        assert!(backup_due(None, later(0)));
        assert!(!backup_due(Some(at.naive_utc()), later(59)));
        assert!(backup_due(Some(at.naive_utc()), later(60)));
    }

    #[test]
    fn only_the_newest_backups_are_kept() {
        use chrono::TimeZone;
        let start = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let mut names: Vec<String> = (0..BACKUPS_KEPT as i64 + 3)
            .map(|minute| backup_name(start + chrono::Duration::minutes(minute)))
            .collect();
        names.reverse();
        names.push("notes.txt".to_owned());
        let surplus = surplus_backups(&names);
        assert_eq!(surplus.len(), 3);
        for minute in 0..3 {
            let oldest = backup_name(start + chrono::Duration::minutes(minute));
            assert!(surplus.contains(&oldest), "{oldest} is kept");
        }
        assert!(!surplus.contains(&"notes.txt".to_owned()));
    }

    #[test]
    fn an_option_of_every_widget_of_a_name_reaches_cards_and_second_columns() {
        let mut edited = file(PLACED);
        assert!(edited.set_option("clock", "format", Some("%T".into())));
        let config = edited.config().expect("it stays usable");
        let clocks = { config.surfaces[0].end.iter() }
            .chain(&config.popups[0].side)
            .chain(&config.cards[0].content[1..]);
        for clock in clocks {
            assert_eq!(clock.options.get("format"), Some(&"%T".into()));
        }
        assert!(!edited.set_option("weather", "unit", None));
/// Has `widget`, and the widgets written inside it (the tiles of `tiles`),
/// open the popup `to` where they opened `from`: their `popup`, and the
/// `start` of a dock.
fn rename_opened(widget: &mut Value, from: &str, to: &str) {
    match widget {
        Value::InlineTable(table) => {
            let dock = table.get("widget").and_then(Value::as_str) == Some(WINDOWS_WIDGET);
            for (key, item) in table.iter_mut() {
                let names = key.get() == "popup" || (dock && key.get() == "start");
                if names && item.as_str() == Some(from) {
                    let decor = item.decor().clone();
                    *item = to.into();
                    *item.decor_mut() = decor;
                } else {
                    rename_opened(item, from, to);
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                rename_opened(item, from, to);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod popup_tests {
    use super::super::{PopupConfig, PopupPlace};
    use super::*;

    const POPUPS: &str = r#"# Mine.
[[surface]]
edge = "top"
thickness = 40
start = [{ widget = "clock", popup = "calendar" }, { widget = "windows", start = "calendar" }]

# The calendar.
[[popup]]
name = "calendar"
width = 620
height = 400
content = ["calendar", { widget = "clock", format = "%H" }] # two
side = ["notifications"]

[[popup]]
name = "settings"
place = "center"
width = 1000
height = 700
content = ["settings"]

[[key]]
press = "win+alt+d"
action = "popup"
arg = "calendar"
"#;

    fn file() -> ConfigFile {
        ConfigFile::parse(POPUPS).unwrap()
    }

    fn popup<'a>(config: &'a Config, name: &str) -> Option<&'a PopupConfig> {
        config.popups.iter().find(|popup| popup.name == name)
    }

    #[test]
    fn a_popup_is_added_as_it_is_and_only_once() {
        let mut edited = file();
        let config = edited.config().unwrap();
        let mut copy = popup(&config, "calendar").unwrap().clone();
        assert!(!edited.add_popup(&copy));
        copy.name = "agenda".to_owned();
        copy.place = PopupPlace::Top;
        assert!(edited.add_popup(&copy));
        let config = edited.config().expect("it stays usable");
        let added = popup(&config, "agenda").unwrap();
        assert_eq!(added.content, copy.content);
        assert_eq!(added.side, copy.side);
        assert_eq!(added.place, PopupPlace::Top);
        assert_eq!((added.width, added.height), (620., 400.));
        copy.name = " ".to_owned();
        assert!(!edited.add_popup(&copy));
    }

    #[test]
    fn a_popup_is_renamed_with_what_opens_it() {
        let mut edited = file();
        assert!(!edited.rename_popup("calendar", "settings"));
        assert!(!edited.rename_popup("calendar", " "));
        assert!(!edited.rename_popup("nothing", "else"));
        assert!(edited.rename_popup("calendar", "agenda"));
        let config = edited.config().expect("it stays usable");
        assert!(popup(&config, "calendar").is_none());
        assert!(popup(&config, "agenda").is_some());
        let start = &config.surfaces[0].start;
        assert_eq!(start[0].popup.as_deref(), Some("agenda"));
        assert_eq!(start[1].options["start"].as_str(), Some("agenda"));
        assert_eq!(config.keys[0].arg.as_deref(), Some("agenda"));
        let text = edited.document.to_string();
        assert!(text.contains("# The calendar."));
        assert!(text.contains("# two"));
    }

    #[test]
    fn the_fields_of_a_popup_are_set_and_taken_out() {
        let mut edited = file();
        assert!(edited.set_popup_field("calendar", "place", Some("center".into())));
        assert!(edited.set_popup_field("calendar", "width", Some(500.into())));
        assert!(!edited.set_popup_field("calendar", "name", Some("x".into())));
        assert!(!edited.set_popup_field("nothing", "width", Some(500.into())));
        let config = edited.config().unwrap();
        let calendar = popup(&config, "calendar").unwrap();
        assert_eq!(calendar.place, PopupPlace::Center);
        assert_eq!(calendar.width, 500.);
        assert!(edited.set_popup_field("calendar", "place", None));
        let config = edited.config().unwrap();
        assert_eq!(
            popup(&config, "calendar").unwrap().place,
            PopupPlace::Surface
        );
    }

    #[test]
    fn the_widgets_of_a_popup_are_added_moved_and_taken_out() {
        let mut edited = file();
        let order = [
            PopupWidget::Kept(1),
            PopupWidget::New("media".to_owned()),
            PopupWidget::Kept(0),
        ];
        assert!(edited.set_popup_widgets("calendar", PopupColumn::Content, &order));
        let past = [PopupWidget::Kept(7)];
        assert!(!edited.set_popup_widgets("calendar", PopupColumn::Content, &past));
        assert!(edited.set_popup_widgets("calendar", PopupColumn::Side, &[]));
        let config = edited.config().expect("it stays usable");
        let calendar = popup(&config, "calendar").unwrap();
        let names: Vec<&str> = { calendar.content.iter() }
            .map(|spec| spec.widget.as_str())
            .collect();
        assert_eq!(names, ["clock", "media", "calendar"]);
        assert_eq!(calendar.content[0].options["format"].as_str(), Some("%H"));
        assert!(calendar.side.is_empty());
        assert!(!edited.document.to_string().contains("side"));
    }

    #[test]
    fn a_popup_is_taken_out_of_the_file() {
        let mut edited = file();
        assert!(edited.remove_popup("calendar"));
        assert!(!edited.remove_popup("calendar"));
        assert!(edited.remove_popup("settings"));
        let config = edited.config().expect("it stays usable");
        assert!(config.popups.is_empty());
        assert!(!edited.document.to_string().contains("[[popup]]"));
/// `key` as a `[[key]]` table of the file, its fields in the order the
/// configuration documents them.
pub fn key_table(key: &super::KeyConfig) -> Table {
    let mut table = Table::new();
    table.insert("press", value(key.press.as_str()));
    let texts = [
        ("popup", &key.popup),
        ("tiling", &key.tiling),
        ("focus", &key.focus),
        ("move", &key.carry),
        ("switch", &key.switch),
        ("files", &key.files),
        ("action", &key.action),
        ("arg", &key.arg),
    ];
    for (name, text) in texts {
        if let Some(text) = text {
            table.insert(name, value(text.as_str()));
        }
    }
    if let Some(run) = &key.run {
        let run: Array = run.iter().map(String::as_str).collect();
        table.insert("run", value(run));
    }
    if let Some(dock) = key.dock {
        table.insert("dock", value(i64::from(dock)));
    }
    table
}

/// What the `[[key]]` table `table` asks for, where it can be read.
fn key_of_table(table: &Table) -> Option<super::KeyConfig> {
    let mut whole = DocumentMut::new();
    whole.extend(table.iter().map(|(key, item)| (key, item.clone())));
    toml::from_str(&whole.to_string()).ok()
}

#[cfg(test)]
mod key_entry_tests {
    use super::*;
    use crate::config::KeyConfig;

    fn opening(press: &str, action: &str, arg: Option<&str>) -> KeyConfig {
        KeyConfig {
            press: press.to_owned(),
            action: Some(action.to_owned()),
            arg: arg.map(str::to_owned),
            ..KeyConfig::default()
        }
    }

    #[test]
    fn whole_entries_are_added_replaced_and_taken_out() {
        let text = "# My keys.\n\n# The launcher.\n[[key]]\npress = \"ctrl+alt+l\"\npopup = \"launcher\"\n";
        let mut file = ConfigFile::parse(text).unwrap();
        let notepad = KeyConfig {
            press: "ctrl+alt+n".to_owned(),
            run: Some(vec!["notepad.exe".to_owned(), "a b.txt".to_owned()]),
            ..KeyConfig::default()
        };
        assert!(file.add_key_entry(&notepad));
        assert!(file.add_key_entry(&opening("win+d", "none", None)));
        let keys = file.config().unwrap().keys;
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[1], notepad);
        assert_eq!(keys[2].action.as_deref(), Some("none"));
        let window = opening("ctrl+alt+e", "activate-window", Some("exe:code.exe"));
        assert!(file.replace_key_entry(0, &window));
        assert!(!file.replace_key_entry(3, &window));
        let text = file.document.to_string();
        assert!(
            text.contains("# The launcher.\n[[key]]\npress = \"ctrl+alt+e\""),
            "{text}"
        );
        assert_eq!(file.config().unwrap().keys[0], window);
        assert!(file.remove_key_entry(2));
        assert!(!file.remove_key_entry(2));
        assert_eq!(file.config().unwrap().keys.len(), 2);
    }

    #[test]
    fn a_scheme_keeps_the_entries_it_shares_with_the_file() {
        let text = "[[key]]\n# Kept as it is.\npress = \"ctrl+alt+l\"\npopup = \"launcher\"\n\n[[key]]\npress = \"ctrl+alt+m\"\npopup = \"media\"\n";
        let mut file = ConfigFile::parse(text).unwrap();
        let kept = KeyConfig {
            press: "ctrl+alt+l".to_owned(),
            popup: Some("launcher".to_owned()),
            ..KeyConfig::default()
        };
        let new = opening("win+alt+p", "pause-keys", None);
        assert!(file.set_key_entries(&[new.clone(), kept.clone()]));
        assert_eq!(file.config().unwrap().keys, [new, kept]);
        assert!(file.document.to_string().contains("# Kept as it is."));
        assert!(file.set_key_entries(&[]));
        assert!(!file.document.contains_key("key"));
        file.config().expect("it stays usable");
    }

    #[test]
    fn every_field_of_an_entry_is_written() {
        let key = KeyConfig {
            press: "ctrl+alt+k, t".to_owned(),
            focus: Some("left".to_owned()),
            carry: Some("right".to_owned()),
            dock: Some(3),
            files: Some("~".to_owned()),
            switch: Some("next".to_owned()),
            tiling: Some("grid".to_owned()),
            ..opening("x", "none", Some("y"))
        };
        assert_eq!(key_of_table(&key_table(&key)), Some(key));
    fn rule(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    /// The `exe` of each rule, in the order of the file as it is written.
    fn exes(edited: &ConfigFile) -> Vec<String> {
        let table: toml::Table = toml::from_str(&edited.document.to_string()).unwrap();
        let rules = table.get("rule").and_then(toml::Value::as_array);
        { rules.into_iter().flatten() }
            .map(|rule| rule["exe"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    const RULES: &str = "# Mine.\n[[rule]]\n# The editor.\nexe = \"a.exe\"\nfloat = true\n\n# For b.\n[[rule]]\nexe = \"b.exe\"\n\n[tiling]\nenabled = true\n\n[[rule]]\nexe = \"c.exe\"\n";

    #[test]
    fn a_rule_is_replaced_in_its_place_with_its_comment() {
        let mut edited = file(RULES);
        let new = rule(
            "exe = \"b2.exe\"\nnot = { title = \"x\" }\nany = [{ class = \"A\" }]\nopacity = 0.5",
        );
        assert!(edited.set_window_rule(1, &new));
        let text = edited.document.to_string();
        assert!(
            text.contains("# For b.\n[[rule]]\nexe = \"b2.exe\""),
            "{text}"
        );
        assert!(text.contains("not = { title = \"x\" }"), "{text}");
        assert!(text.contains("# The editor."));
        assert_eq!(exes(&edited), ["a.exe", "b2.exe", "c.exe"]);
        assert!(!edited.set_window_rule(3, &new));
        edited.config().expect("it is usable");
    }

    #[test]
    fn rules_are_put_in_taken_out_and_moved() {
        let mut edited = file(RULES);
        assert!(edited.insert_window_rule(0, &rule("exe = \"first.exe\"")));
        assert!(edited.insert_window_rule(4, &rule("exe = \"last.exe\"")));
        assert!(!edited.insert_window_rule(9, &rule("exe = \"x.exe\"")));
        assert_eq!(
            exes(&edited),
            ["first.exe", "a.exe", "b.exe", "c.exe", "last.exe"]
        );
        assert!(edited.move_window_rule(0, 3));
        assert_eq!(
            exes(&edited),
            ["a.exe", "b.exe", "c.exe", "first.exe", "last.exe"]
        );
        assert!(edited.move_window_rule(4, 0));
        assert_eq!(
            exes(&edited),
            ["last.exe", "a.exe", "b.exe", "c.exe", "first.exe"]
        );
        assert!(!edited.move_window_rule(5, 0));
        // The comments go with their rules.
        let text = edited.document.to_string();
        assert!(
            text.contains("# For b.\n[[rule]]\nexe = \"b.exe\""),
            "{text}"
        );
        assert!(text.contains("[tiling]\nenabled = true"));
        assert!(edited.remove_window_rule(1));
        assert_eq!(exes(&edited), ["last.exe", "b.exe", "c.exe", "first.exe"]);
        assert!(!edited.document.to_string().contains("# The editor."));
        edited.config().expect("it is usable");
        for _ in 0..4 {
            assert!(edited.remove_window_rule(0));
        }
        assert!(!edited.remove_window_rule(0));
        assert!(!edited.document.to_string().contains("[[rule]]"));
    }

    #[test]
    fn a_rule_is_put_in_a_file_without_rules() {
        let mut edited = file("[tiling]\nenabled = true\n");
        assert!(edited.insert_window_rule(0, &rule("exe = \"a.exe\"\nenabled = false")));
        assert_eq!(exes(&edited), ["a.exe"]);
        let config = edited.config().expect("it is usable");
        assert_eq!(config.rules.len(), 1);
        assert!(!config.rules[0].is_enabled());
        let mut empty = file("");
        assert!(!empty.insert_window_rule(1, &rule("exe = \"a.exe\"")));
        assert_eq!(empty.document.to_string(), "");
    }

    #[test]
    fn the_names_come_first() {
        let table = rule_table(&rule(
            "above = true\nexe = \"a.exe\"\napply = \"always\"\ntitle = \"T\"",
        ));
        let keys: Vec<&str> = table.iter().map(|(key, _)| key).collect();
        assert_eq!(keys, ["exe", "title", "above", "apply"]);
    }
}
