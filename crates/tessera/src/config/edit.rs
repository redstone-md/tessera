// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Changes to the user's configuration file that leave the rest of it, its
//! comments included, as the user wrote it.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use toml_edit::{Array, DocumentMut, InlineTable, Item, Value};

use super::{CONFIG_FILE, Config, DEFAULT, config_dir};

/// The widget whose `pinned` option lists the applications pinned to the
/// dock, and the one whose `pinned` option lists those of the Start menu.
const WINDOWS_WIDGET: &str = "windows";
const APPS_WIDGET: &str = "apps";

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

    fn neighbor(self, forward: bool) -> Option<Self> {
        match (self, forward) {
            (Self::Start, true) | (Self::End, false) => Some(Self::Center),
            (Self::Center, true) => Some(Self::End),
            (Self::Center, false) => Some(Self::Start),
            (Self::Start, false) | (Self::End, true) => None,
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

/// The configuration file, open for changing.
pub struct ConfigFile {
    document: DocumentMut,
}

impl ConfigFile {
    /// The user's file, or the built-in configuration, comments and all,
    /// where there is none yet.
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
    pub fn save(&self) -> Result<(), String> {
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

    /// Moves the widget at `place` one step toward the end of its surface,
    /// or toward the start: past its neighbor, or into the next slot when
    /// it is the last of its own.
    pub fn shift(&mut self, place: Place, forward: bool) -> bool {
        let Some(count) = self.slot(place.surface, place.slot).map(|slot| slot.len()) else {
            return false;
        };
        let inside = if forward {
            place.index + 1 < count
        } else {
            place.index > 0 && place.index < count
        };
        let (slot, index) = if inside {
            let index = if forward {
                place.index + 1
            } else {
                place.index - 1
            };
            (place.slot, Some(index))
        } else {
            let Some(neighbor) = place.slot.neighbor(forward) else {
                return false;
            };
            // It enters the neighbor at the near end.
            (neighbor, (forward).then_some(0))
        };
        let Some(widget) = self.take(place) else {
            return false;
        };
        let Some(widgets) = self.slot(place.surface, slot) else {
            return false;
        };
        let index = index.unwrap_or(widgets.len()).min(widgets.len());
        widgets.insert(index, widget);
        widgets.fmt();
        true
    }

    /// Makes `pinned` the applications that keep a button in every dock:
    /// the `pinned` option of each `windows` widget. Whether the layout
    /// has such a widget is returned.
    pub fn set_pinned(&mut self, pinned: &[String]) -> bool {
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
                    found |= Self::pin(widget, WINDOWS_WIDGET, pinned);
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
                found |= Self::pin(widget, APPS_WIDGET, pinned);
            }
            widgets.fmt();
        }
        found
    }

    /// Sets the option on `widget` if it is the widget `name`.
    fn pin(widget: &mut Value, name: &str, pinned: &[String]) -> bool {
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
        if pinned.is_empty() {
            table.remove("pinned");
        } else {
            let list: Array = pinned.iter().map(String::as_str).collect();
            table.insert("pinned", Value::Array(list));
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

    fn place(surface: usize, slot: Slot, index: usize) -> Place {
        Place {
            surface,
            slot,
            index,
        }
    }

    #[test]
    fn a_widget_moves_past_its_neighbor_and_into_the_next_slot() {
        let mut file = ConfigFile::parse(LAYOUT).unwrap();
        assert!(file.shift(place(0, Slot::Start, 0), true));
        assert_eq!(names(&file, 0, Slot::Start), ["label", "clock"]);
        assert!(file.shift(place(0, Slot::Start, 1), true));
        assert_eq!(names(&file, 0, Slot::Start), ["label"]);
        assert_eq!(names(&file, 0, Slot::Center), ["clock"]);
        assert!(file.shift(place(0, Slot::Center, 0), true));
        assert_eq!(names(&file, 0, Slot::End), ["clock", "quit"]);
        assert!(file.shift(place(0, Slot::End, 0), false));
        assert!(file.shift(place(0, Slot::Center, 0), false));
        assert_eq!(names(&file, 0, Slot::Start), ["label", "clock"]);
        assert!(!file.shift(place(0, Slot::Start, 0), false));
        assert!(!file.shift(place(0, Slot::End, 0), true));
        assert!(!file.shift(place(0, Slot::End, 7), true));
        assert!(!file.shift(place(5, Slot::End, 0), true));
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
    fn the_built_in_configuration_can_be_changed() {
        let mut file = ConfigFile::parse(DEFAULT).unwrap();
        assert!(file.set_pinned(&["Paint".to_owned()]));
        assert!(file.shift(place(0, Slot::Start, 0), true));
        file.config().expect("it stays usable");
    }
}
