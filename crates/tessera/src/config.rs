// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! User configuration: which surfaces exist and what they hold.

use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use serde::Deserialize;

use crate::platform::ScreenEdge;

const DEFAULT: &str = include_str!("default_config.toml");

/// The environment variable that names another configuration directory.
const DIRECTORY_VARIABLE: &str = "TESSERA_CONFIG_DIR";

/// The directory user configuration is read from, when the system has one:
/// the one `TESSERA_CONFIG_DIR` names, or else the `tessera` directory in `%APPDATA%`.
pub fn config_dir() -> Option<PathBuf> {
    match std::env::var_os(DIRECTORY_VARIABLE) {
        Some(directory) if !directory.is_empty() => Some(PathBuf::from(directory)),
        _ => Some(PathBuf::from(std::env::var_os("APPDATA")?).join("tessera")),
    }
}

/// Reads `file` from the configuration directory. A missing file is `None`;
/// an unreadable or invalid one is reported and also `None`.
pub fn read_user_file<T: for<'de> Deserialize<'de>>(file: &str) -> Option<T> {
    let path = config_dir()?.join(file);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!("tessera: {} is ignored: {error}", path.display());
            return None;
        }
    };
    toml::from_str(&text)
        .inspect_err(|error| eprintln!("tessera: {} is ignored: {error}", path.display()))
        .ok()
}

/// Which displays a surface appears on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Displays {
    #[default]
    All,
    Primary,
}

/// A widget to place: its name, the popup a click on it opens, and the
/// options its module understands. Written either as the bare name or as a
/// table with a `widget` key.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(from = "WidgetSpecText")]
pub struct WidgetSpec {
    pub widget: String,
    pub popup: Option<String>,
    pub options: toml::Table,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WidgetSpecText {
    Name(String),
    Detailed {
        widget: String,
        popup: Option<String>,
        #[serde(flatten)]
        options: toml::Table,
    },
}

impl From<WidgetSpecText> for WidgetSpec {
    fn from(text: WidgetSpecText) -> Self {
        match text {
            WidgetSpecText::Name(widget) => Self {
                widget,
                popup: None,
                options: toml::Table::new(),
            },
            WidgetSpecText::Detailed {
                widget,
                popup,
                options,
            } => Self {
                widget,
                popup,
                options,
            },
        }
    }
}

/// A floating panel of widgets that a click on a widget opens.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PopupConfig {
    pub name: String,
    /// Size of the panel, in logical pixels.
    pub width: f32,
    pub height: f32,
    /// Widgets, stacked from the top.
    #[serde(default)]
    pub content: Vec<WidgetSpec>,
}

/// One strip docked to a screen edge, and the widgets in its slots.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceConfig {
    pub edge: ScreenEdge,
    /// Reserved screen space, in logical pixels.
    pub thickness: f32,
    #[serde(default)]
    pub displays: Displays,
    #[serde(default)]
    pub start: Vec<WidgetSpec>,
    #[serde(default)]
    pub center: Vec<WidgetSpec>,
    #[serde(default)]
    pub end: Vec<WidgetSpec>,
}

/// Everything the user can configure.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Name of the theme file to apply over the built-in theme.
    pub theme: Option<String>,
    #[serde(default, rename = "surface")]
    pub surfaces: Vec<SurfaceConfig>,
    #[serde(default, rename = "popup")]
    pub popups: Vec<PopupConfig>,
    /// Image files to show instead of the icons of applications, by the
    /// file name of the executable.
    #[serde(default)]
    pub icons: HashMap<String, PathBuf>,
    /// Settings of extensions, by extension identifier.
    #[serde(default, rename = "extension")]
    pub extensions: HashMap<String, toml::Table>,
}

impl Config {
    /// The user's configuration, or the built-in one when there is none or
    /// it cannot be used.
    pub fn load() -> Self {
        read_user_file("config.toml").unwrap_or_else(Self::builtin)
    }

    fn builtin() -> Self {
        toml::from_str(DEFAULT).expect("the built-in configuration is valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_configuration_describes_a_bar_and_a_dock() {
        let config = Config::builtin();
        assert_eq!(config.theme, None);
        let [bar, dock] = config.surfaces.as_slice() else {
            panic!("expected two surfaces, got {}", config.surfaces.len());
        };
        assert_eq!(bar.edge, ScreenEdge::Top);
        let names = |specs: &[WidgetSpec]| -> Vec<String> {
            specs.iter().map(|spec| spec.widget.clone()).collect()
        };
        assert_eq!(names(&bar.center), ["clock"]);
        assert_eq!(names(&bar.start), ["label", "cpu", "memory", "battery"]);
        assert_eq!(names(&bar.end), ["tray", "volume", "quit"]);
        assert_eq!(dock.edge, ScreenEdge::Bottom);
        assert_eq!(dock.displays, Displays::All);
        assert_eq!(names(&dock.center), ["windows"]);

        let popup = bar.center[0]
            .popup
            .as_deref()
            .expect("the clock opens a popup");
        assert!(config.popups.iter().any(|known| known.name == popup));
    }

    #[test]
    fn a_widget_is_a_name_or_a_table_with_options() {
        #[derive(Deserialize)]
        struct Slot {
            slot: Vec<WidgetSpec>,
        }
        let text = "slot = [\"quit\", { widget = \"clock\", popup = \"agenda\", format = \"%H\" }]";
        let slot = toml::from_str::<Slot>(text).unwrap().slot;
        assert_eq!(slot[0].widget, "quit");
        assert_eq!(slot[0].popup, None);
        assert_eq!(slot[1].popup.as_deref(), Some("agenda"));
        assert_eq!(slot[1].options["format"].as_str(), Some("%H"));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let text = "[[surface]]\nedge = \"top\"\nthickness = 40\ncentre = [\"clock\"]\n";
        assert!(toml::from_str::<Config>(text).is_err());
    }
}
