// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! User configuration: which surfaces exist and what they hold.

use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

use crate::banners::NotificationsConfig;
use crate::desk::CardConfig;
use crate::extension::registry::OFFICIAL_REGISTRY;
use crate::files::FilesConfig;
use crate::flyout::FlyoutConfig;
use crate::i18n::Language;
use crate::platform::ScreenEdge;
use crate::switcher::SwitcherConfig;
use crate::tiling::TilingConfig;
use crate::wallpaper::WallpaperConfig;
use crate::workspaces::WorkspacesConfig;

pub mod edit;

const DEFAULT: &str = include_str!("default_config.toml");

const CONFIG_FILE: &str = "config.toml";

/// Where the theme `name` is kept in the configuration directory.
pub fn theme_file(name: &str) -> String {
    format!("themes/{name}.toml")
}

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

/// What reading a file of the configuration directory came to.
enum UserFile<T> {
    Missing,
    /// It is unreadable or invalid, which was reported.
    Unusable,
    Read(T),
}

fn read<T: for<'de> Deserialize<'de>>(file: &str) -> UserFile<T> {
    let Some(path) = config_dir().map(|directory| directory.join(file)) else {
        return UserFile::Missing;
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return UserFile::Missing,
        Err(error) => {
            report!("{} is ignored: {error}", path.display());
            return UserFile::Unusable;
        }
    };
    match toml::from_str(&text) {
        Ok(value) => UserFile::Read(value),
        Err(error) => {
            report!("{} is ignored: {error}", path.display());
            UserFile::Unusable
        }
    }
}

/// Reads `file` from the configuration directory. A missing file is `None`;
/// an unreadable or invalid one is reported and also `None`.
pub fn read_user_file<T: for<'de> Deserialize<'de>>(file: &str) -> Option<T> {
    match read(file) {
        UserFile::Read(value) => Some(value),
        UserFile::Missing | UserFile::Unusable => None,
    }
}

/// When a file of the configuration directory was last written; `None` if
/// it is missing.
fn modified(file: &str) -> Option<SystemTime> {
    fs::metadata(config_dir()?.join(file)).ok()?.modified().ok()
}

/// When the files a configuration is made of were last written. It differs
/// after any of them was written, created or deleted.
pub type Stamp = [Option<SystemTime>; 2];

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

/// Where a popup opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PopupPlace {
    /// Next to the surface, at the widget that opened it.
    #[default]
    Surface,
    /// In the middle of the display, over a veil that dims the rest.
    Center,
}

/// A floating panel of widgets that a click on a widget opens.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PopupConfig {
    pub name: String,
    #[serde(default)]
    pub place: PopupPlace,
    /// Size of the panel, in logical pixels.
    pub width: f32,
    pub height: f32,
    /// Widgets, stacked from the top.
    #[serde(default)]
    pub content: Vec<WidgetSpec>,
    /// Widgets stacked in a second column, to the right of `content`;
    /// the two columns share the width evenly.
    #[serde(default)]
    pub side: Vec<WidgetSpec>,
}

impl PopupConfig {
    /// Every widget of the popup, in the order of the file.
    pub fn widgets(&self) -> impl Iterator<Item = &WidgetSpec> {
        self.content.iter().chain(&self.side)
    }
}

/// A key combination Tessera answers wherever it is pressed, and the one
/// thing it does.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyConfig {
    /// The combination, as `ctrl+alt+space`.
    pub press: String,
    /// The popup to open, or to close when it is open.
    pub popup: Option<String>,
    /// How to tile: `next` for the mode after the one in use, `off`, or
    /// the name of a layout.
    pub tiling: Option<String>,
    /// A program to start, and its arguments.
    pub run: Option<Vec<String>>,
    /// The side, `left`, `right`, `up` or `down`, on which the window to
    /// bring in front lies.
    pub focus: Option<String>,
    /// The side toward which the window in front trades tiles.
    #[serde(rename = "move")]
    pub carry: Option<String>,
    /// `next` or `previous`: the window switcher goes that way through
    /// the windows while the keys to hold stay down.
    pub switch: Option<String>,
    /// 1 to 9: the application with that number in the dock is started
    /// or its windows are gone through, as a click on its button does.
    pub dock: Option<u32>,
    /// A folder to open in the file manager: a path, `~` for the folder
    /// of the user, or `computer` for the drives.
    pub files: Option<String>,
    /// The name of an action, one of those the modules of Tessera
    /// register (see crate::actions), such as `desktop`.
    pub action: Option<String>,
    /// What the action of `action` is done with, as `tessera do <action>
    /// <arg>` would give it.
    pub arg: Option<String>,
}

/// When a surface goes away.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "HidingName")]
pub enum Hiding {
    /// Never: the surface keeps its screen edge.
    #[default]
    Never,
    /// While the pointer is elsewhere; `true` in the configuration.
    Pointer,
    /// While the pointer is elsewhere and a window lies where the surface
    /// would be; `"overlap"` in the configuration.
    Overlap,
}

impl Hiding {
    /// Whether the surface ever goes away.
    pub fn hides(self) -> bool {
        self != Self::Never
    }
}

/// What the configuration says of hiding: a yes or no, or a name.
#[derive(Deserialize)]
#[serde(untagged)]
enum HidingName {
    Flag(bool),
    Name(String),
}

impl TryFrom<HidingName> for Hiding {
    type Error = String;

    fn try_from(name: HidingName) -> Result<Self, String> {
        match name {
            HidingName::Flag(false) => Ok(Self::Never),
            HidingName::Flag(true) => Ok(Self::Pointer),
            HidingName::Name(name) if name == "overlap" => Ok(Self::Overlap),
            HidingName::Name(name) => {
                Err(format!("`hide` is true, false or `overlap`, not `{name}`"))
            }
        }
    }
}

/// One strip docked to a screen edge, and the widgets in its slots.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceConfig {
    pub edge: ScreenEdge,
    /// Reserved screen space, in logical pixels.
    pub thickness: f32,
    #[serde(default)]
    pub displays: Displays,
    /// When the strip goes away; one that does reserves no screen space.
    #[serde(default)]
    pub hide: Hiding,
    #[serde(default)]
    pub start: Vec<WidgetSpec>,
    #[serde(default)]
    pub center: Vec<WidgetSpec>,
    #[serde(default)]
    pub end: Vec<WidgetSpec>,
}

impl SurfaceConfig {
    /// Every widget of the surface, in the order of the file.
    fn widgets(&self) -> impl Iterator<Item = &WidgetSpec> {
        let slots = [&self.start, &self.center, &self.end];
        slots.into_iter().flatten()
    }
}

fn yes() -> bool {
    true
}

/// Everything the user can configure.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Name of the theme file to apply over the built-in theme.
    pub theme: Option<String>,
    /// Whether colors that are read are moved away from what they lie on
    /// until they have the contrast WCAG 2.1 asks for.
    #[serde(default = "yes")]
    pub contrast: bool,
    /// The language of the settings, the marketplace, the calendar, the
    /// lists of notifications and problems and the names in the clock.
    #[serde(default)]
    pub language: Language,
    #[serde(default, rename = "surface")]
    pub surfaces: Vec<SurfaceConfig>,
    #[serde(default, rename = "popup")]
    pub popups: Vec<PopupConfig>,
    /// Panels of widgets that lie on the desktop.
    #[serde(default, rename = "card")]
    pub cards: Vec<CardConfig>,
    /// Image files to show instead of the icons of applications, by the
    /// file name of the executable.
    #[serde(default)]
    pub icons: HashMap<String, PathBuf>,
    /// Settings of extensions, by extension identifier.
    #[serde(default, rename = "extension")]
    pub extensions: HashMap<String, toml::Table>,
    /// Where the registries are that offer extensions to install; the
    /// official one when the key is missing.
    registries: Option<Vec<String>>,
    /// Whether and how windows are arranged side by side.
    #[serde(default)]
    pub tiling: TilingConfig,
    /// Whether the workspaces are Tessera's own, and whose windows they
    /// hold.
    #[serde(default)]
    pub workspaces: WorkspacesConfig,
    /// The pictures Tessera draws behind the icons of the desktop.
    #[serde(default)]
    pub wallpaper: WallpaperConfig,
    /// Whether notifications that arrive are shown as banners.
    #[serde(default)]
    pub notifications: NotificationsConfig,
    /// Whether a change of the volume is shown for a moment.
    #[serde(default)]
    pub flyout: FlyoutConfig,
    /// How the window switcher looks.
    #[serde(default)]
    pub switcher: SwitcherConfig,
    /// How the file manager shows folders, and whether it opens those the
    /// surfaces of Tessera open.
    #[serde(default)]
    pub files: FilesConfig,
    /// Whether and when Tessera, as the shell, starts the programs that
    /// start with the session.
    #[serde(default)]
    pub startup: crate::startup::StartupConfig,
    /// The popup that the Windows key, pressed by itself, opens and closes
    /// instead of the Start menu of Windows.
    pub windows_key: Option<String>,
    /// Whether the keys of Windows that features of Tessera bring, such
    /// as Win+D for `desktop`, are bound to them where no `[[key]]` uses
    /// the same keys; `None` binds them only when Tessera is the shell.
    pub windows_keys: Option<bool>,
    /// The key combinations that open a popup, change the tiling or start
    /// a program.
    #[serde(default, rename = "key")]
    pub keys: Vec<KeyConfig>,
}

impl Config {
    /// Where the registries are that offer extensions to install.
    pub fn registries(&self) -> Vec<String> {
        let official = || vec![OFFICIAL_REGISTRY.to_owned()];
        self.registries.clone().unwrap_or_else(official)
    }

    /// The user's configuration, or the built-in one when there is none or
    /// it cannot be used.
    pub fn load() -> Self {
        Self::reload().unwrap_or_else(Self::builtin)
    }

    /// The user's configuration, or the built-in one when there is none;
    /// `None` when the user's cannot be used.
    pub fn reload() -> Option<Self> {
        match read(CONFIG_FILE) {
            UserFile::Read(config) => Some(config),
            UserFile::Missing => Some(Self::builtin()),
            UserFile::Unusable => None,
        }
    }

    /// The state of the files this configuration is read from: the layout
    /// and the theme it names.
    pub fn stamp(&self) -> Stamp {
        let theme = self
            .theme
            .as_deref()
            .and_then(|name| modified(&theme_file(name)));
        [modified(CONFIG_FILE), theme]
    }

    /// The applications the first `windows` widget pins; `None` if the
    /// layout has no such widget.
    pub fn pinned(&self) -> Option<Vec<String>> {
        let widgets = self.surfaces.iter().flat_map(SurfaceConfig::widgets);
        let dock = { widgets }.find(|spec| spec.widget == edit::WINDOWS_WIDGET)?;
        let listed = dock.options.get("pinned").and_then(toml::Value::as_array);
        let names = |list: &Vec<toml::Value>| {
            let names = list.iter().filter_map(toml::Value::as_str);
            names.map(str::to_owned).collect()
        };
        Some(listed.map(names).unwrap_or_default())
    }

    /// The option `key` of the first widget `name`, on a surface or in a
    /// popup.
    pub fn option(&self, name: &str, key: &str) -> Option<&toml::Value> {
        self.widget(name)?.options.get(key)
    }

    /// The first widget `name`, on a surface or in a popup.
    pub fn widget(&self, name: &str) -> Option<&WidgetSpec> {
        let placed = self.surfaces.iter().flat_map(SurfaceConfig::widgets);
        let shown = self.popups.iter().flat_map(PopupConfig::widgets);
        placed.chain(shown).find(|spec| spec.widget == name)
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
        assert!(config.contrast);
        let [bar, dock] = config.surfaces.as_slice() else {
            panic!("expected two surfaces, got {}", config.surfaces.len());
        };
        assert_eq!(bar.edge, ScreenEdge::Top);
        let names = |specs: &[WidgetSpec]| -> Vec<String> {
            specs.iter().map(|spec| spec.widget.clone()).collect()
        };
        assert_eq!(names(&bar.center), ["clock"]);
        assert_eq!(
            names(&bar.start),
            ["workspaces", "cpu", "memory", "battery"]
        );
        assert_eq!(config.registries(), [OFFICIAL_REGISTRY]);
        assert_eq!(
            names(&bar.end),
            [
                "problems",
                "media",
                "tray",
                "bluetooth-button",
                "network",
                "volume",
                "label"
            ]
        );
        assert_eq!(dock.edge, ScreenEdge::Bottom);
        assert_eq!(dock.displays, Displays::All);
        assert_eq!(names(&dock.center), ["windows"]);
        assert_eq!(config.language, Language::English);
        let settings = config.popups.iter().find(|popup| popup.name == "settings");
        assert_eq!(settings.map(|popup| popup.place), Some(PopupPlace::Center));
        assert!(config.option("windows", "pinned").is_none());

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
