// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! User configuration: which surfaces exist and what they hold.

use std::collections::HashMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

use crate::banners::NotificationsConfig;
use crate::clipboard::ClipboardConfig;
use crate::corners::CornerConfig;
use crate::desk::CardConfig;
use crate::extension::registry::OFFICIAL_REGISTRY;
use crate::files::FilesConfig;
use crate::flyout::FlyoutConfig;
use crate::i18n::Language;
use crate::modules::night::NightLightConfig;
use crate::notifications_store::NotificationRule;
use crate::platform::ScreenEdge;
use crate::rules::RuleConfig;
use crate::snap::SnapConfig;
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

/// When the files a configuration is made of were last written: the file
/// itself, the theme it names and each theme stacked over that. It differs
/// after any of them was written, created or deleted, or when the themes
/// named change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp(u64);

impl Stamp {
    fn of(times: &[Option<SystemTime>]) -> Self {
        let mut hasher = DefaultHasher::new();
        times.hash(&mut hasher);
        Self(hasher.finish())
    }
}

/// What a display is called and where it stands, as surfaces choose
/// displays by it.
type DisplayName = crate::platform::DisplayName;

/// Which displays a surface appears on: `"all"`, `"primary"`, `"others"`
/// (every display but the primary one), or the displays a selector or a
/// list of selectors names, as [`DisplaySelector`] reads them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(try_from = "DisplaysText")]
pub enum Displays {
    #[default]
    All,
    Primary,
    /// Every display but the primary one.
    Others,
    /// The displays any of these selectors matches.
    Chosen(Vec<DisplaySelector>),
}

impl Displays {
    /// Whether the surface appears on `display`, which is the primary
    /// display where `primary` says so. A display whose name is unknown
    /// is matched by no selector.
    pub fn includes(&self, display: Option<&DisplayName>, primary: bool) -> bool {
        match self {
            Self::All => true,
            Self::Primary => primary,
            Self::Others => !primary,
            Self::Chosen(selectors) => display
                .is_some_and(|display| selectors.iter().any(|selector| selector.matches(display))),
        }
    }
}

/// What the configuration says of displays: a name or a selector, or a
/// list of selectors.
#[derive(Deserialize)]
#[serde(untagged)]
enum DisplaysText {
    Name(String),
    List(Vec<SelectorText>),
}

/// A selector as the configuration writes it: a position or a text.
#[derive(Deserialize)]
#[serde(untagged)]
enum SelectorText {
    Position(i64),
    Text(String),
}

impl TryFrom<SelectorText> for DisplaySelector {
    type Error = String;

    fn try_from(text: SelectorText) -> Result<Self, String> {
        match text {
            SelectorText::Position(position) if position >= 1 => {
                Ok(Self::Position(position as usize))
            }
            SelectorText::Position(position) => Err(format!(
                "displays are counted from 1 from the left, `{position}` is none"
            )),
            SelectorText::Text(text) => Self::parse(&text),
        }
    }
}

impl TryFrom<DisplaysText> for Displays {
    type Error = String;

    fn try_from(text: DisplaysText) -> Result<Self, String> {
        match text {
            DisplaysText::Name(name) => match name.as_str() {
                "all" => Ok(Self::All),
                "primary" => Ok(Self::Primary),
                "others" => Ok(Self::Others),
                _ => Ok(Self::Chosen(vec![DisplaySelector::parse(&name)?])),
            },
            DisplaysText::List(list) => {
                let selectors = list.into_iter().map(DisplaySelector::try_from);
                Ok(Self::Chosen(selectors.collect::<Result<_, _>>()?))
            }
        }
    }
}

/// Names one display or several: by where it stands from left to right,
/// the first being 1; by the name GDI gives its output, as `\\.\DISPLAY2`;
/// or by the name the monitor gives itself, as the display settings of
/// Windows show it, where `*` stands for any text and `?` for any one
/// character. Names are compared without regard to case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplaySelector {
    Position(usize),
    Device(String),
    Name(String),
}

/// How the name GDI gives an output begins.
const DEVICE_PREFIX: &str = r"\\.\";

impl DisplaySelector {
    /// Reads a selector written as text: digits are a position, a text
    /// that begins as `\\.\` an output, and anything else a name.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err(
                "a display is named by its position, output or name, not by nothing".into(),
            );
        }
        if text.bytes().all(|byte| byte.is_ascii_digit()) {
            return match text.parse::<usize>() {
                Ok(position) if position >= 1 => Ok(Self::Position(position)),
                _ => Err(format!(
                    "displays are counted from 1 from the left, `{text}` is none"
                )),
            };
        }
        if text.starts_with(DEVICE_PREFIX) {
            return Ok(Self::Device(text.to_owned()));
        }
        Ok(Self::Name(text.to_owned()))
    }

    /// Whether the selector names `display`.
    pub fn matches(&self, display: &DisplayName) -> bool {
        match self {
            Self::Position(position) => display.position == *position,
            Self::Device(device) => device.eq_ignore_ascii_case(&display.device),
            Self::Name(pattern) => wildcard_matches(pattern, &display.friendly),
        }
    }

    /// The selector as the configuration writes it.
    pub fn text(&self) -> String {
        match self {
            Self::Position(position) => position.to_string(),
            Self::Device(text) | Self::Name(text) => text.clone(),
        }
    }
}

/// Whether `text` is what `pattern` describes, without regard to case:
/// `*` stands for any text, also none, and `?` for any one character.
fn wildcard_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let (mut at, mut from) = (0, 0);
    // Where the last `*` was, and where in the text it began to stand.
    let mut star: Option<(usize, usize)> = None;
    while from < text.len() {
        match pattern.get(at) {
            Some('*') => {
                star = Some((at, from));
                at += 1;
            }
            Some(&wanted) if wanted == '?' || wanted == text[from] => {
                at += 1;
                from += 1;
            }
            _ => match star {
                // The `*` stands for one character more.
                Some((star_at, star_from)) => {
                    at = star_at + 1;
                    from = star_from + 1;
                    star = Some((star_at, star_from + 1));
                }
                None => return false,
            },
        }
    }
    pattern[at..].iter().all(|&left| left == '*')
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
    /// Near the top of the display, in the middle of its width, without a
    /// veil, as KRunner of KDE Plasma stands.
    Top,
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
    /// Every way, in the order the menus offer them.
    pub const ALL: [Self; 3] = [Self::Never, Self::Pointer, Self::Overlap];

    /// Whether the surface ever goes away.
    pub fn hides(self) -> bool {
        self != Self::Never
    }

    /// What the settings and the menus call it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Never => "Always shown",
            Self::Pointer => "Hides by itself",
            Self::Overlap => "Hides under windows",
        }
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

/// Where along its edge a surface shorter than the edge stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpanAlign {
    /// At the top or the left end.
    Start,
    #[default]
    Center,
    /// At the bottom or the right end.
    End,
}

impl SpanAlign {
    /// Every place, in the order the menus offer them.
    pub const ALL: [Self; 3] = [Self::Start, Self::Center, Self::End];

    /// What the configuration file calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Center => "center",
            Self::End => "end",
        }
    }
}

/// When the background of a surface turns opaque.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SolidWhen {
    /// While a maximized window touches the surface on its display.
    Maximized,
}

/// How long the pointer is elsewhere before a surface that hides goes, in
/// milliseconds, where the configuration does not say; it comes back as
/// soon as the pointer reaches its edge unless `show_delay` says.
const HIDE_DELAY: u64 = 700;

/// The longest either may be.
const LONGEST_DELAY: u64 = 10_000;

/// The least and the most of the background a surface shows.
pub const OPACITY_RANGE: (f32, f32) = (0.3, 1.);

/// The least and the most of the length of its edge a surface takes.
pub const LENGTH_RANGE: (f32, f32) = (0.2, 1.);

fn hide_delay() -> u64 {
    HIDE_DELAY
}

fn whole_length() -> f32 {
    LENGTH_RANGE.1
}

/// What a surface has instead on the displays a selector names.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceOverride {
    pub thickness: Option<f32>,
    pub hide: Option<Hiding>,
    pub start: Option<Vec<WidgetSpec>>,
    pub center: Option<Vec<WidgetSpec>>,
    pub end: Option<Vec<WidgetSpec>>,
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
    /// How long, in milliseconds, the pointer rests at the edge before a
    /// strip that hides comes back.
    #[serde(default)]
    pub show_delay: u64,
    /// How long, in milliseconds, the pointer is elsewhere before a strip
    /// that hides goes.
    #[serde(default = "hide_delay")]
    pub hide_delay: u64,
    /// How much of the background of the strip is drawn, from 0.3 to 1;
    /// none where it is not given, and the widgets stand on what is under
    /// the strip.
    pub opacity: Option<f32>,
    /// The share of the length of its edge the strip takes, from 0.2 to 1.
    #[serde(default = "whole_length")]
    pub length: f32,
    /// Where a strip shorter than its edge stands along it.
    #[serde(default)]
    pub align: SpanAlign,
    /// Whether the widgets have fields of their own behind them, as the
    /// theme says where this does not.
    pub floating: Option<bool>,
    /// When the background turns opaque.
    pub solid_when: Option<SolidWhen>,
    /// What the strip has instead on the displays each selector names, by
    /// selector. Where several name a display, each is applied in turn, in
    /// the order of their selectors as text, so the last says what it
    /// gives. A selector that cannot be read makes the file rejected.
    #[serde(default, deserialize_with = "per_display")]
    pub per_display: std::collections::BTreeMap<String, SurfaceOverride>,
}

/// Reads the overrides of a surface by display, and rejects the file where
/// a selector cannot be read, as the other blocks are rejected.
fn per_display<'de, D>(
    deserializer: D,
) -> Result<std::collections::BTreeMap<String, SurfaceOverride>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let overrides =
        std::collections::BTreeMap::<String, SurfaceOverride>::deserialize(deserializer)?;
    for selector in overrides.keys() {
        DisplaySelector::parse(selector).map_err(|error| {
            serde::de::Error::custom(format!("`per_display.\"{selector}\"`: {error}"))
        })?;
    }
    Ok(overrides)
}

impl SurfaceConfig {
    /// Every widget of the surface, in the order of the file.
    fn widgets(&self) -> impl Iterator<Item = &WidgetSpec> {
        let slots = [&self.start, &self.center, &self.end];
        slots.into_iter().flatten()
    }

    /// The surface as it is on `display`, with what the overrides that
    /// name the display give, and for each slot whether an override
    /// replaced it. The overrides are taken out of what is returned. Their
    /// selectors were read when the file was.
    pub fn on_display(&self, display: Option<&DisplayName>) -> (Self, [bool; 3]) {
        let mut placed = Self {
            per_display: std::collections::BTreeMap::new(),
            ..self.clone()
        };
        let mut replaced = [false; 3];
        for (selector, change) in &self.per_display {
            let Ok(selector) = DisplaySelector::parse(selector) else {
                continue;
            };
            if !display.is_some_and(|display| selector.matches(display)) {
                continue;
            }
            if let Some(thickness) = change.thickness {
                placed.thickness = thickness;
            }
            if let Some(hide) = change.hide {
                placed.hide = hide;
            }
            let slots = [&mut placed.start, &mut placed.center, &mut placed.end];
            let given = [&change.start, &change.center, &change.end];
            for (index, (slot, given)) in slots.into_iter().zip(given).enumerate() {
                if let Some(widgets) = given {
                    *slot = widgets.clone();
                    replaced[index] = true;
                }
            }
        }
        (placed, replaced)
    }

    /// The part of its edge the strip takes.
    pub fn span(&self) -> crate::platform::EdgeSpan {
        let (least, most) = LENGTH_RANGE;
        if !self.length.is_finite() || self.length >= most {
            return crate::platform::EdgeSpan::WHOLE;
        }
        let length = self.length.max(least);
        let from = match self.align {
            SpanAlign::Start => 0.,
            SpanAlign::Center => (1. - length) / 2.,
            SpanAlign::End => 1. - length,
        };
        crate::platform::EdgeSpan {
            from,
            to: from + length,
        }
    }

    /// How much of the background is drawn, within its range; `None` for
    /// none.
    pub fn opacity(&self) -> Option<f32> {
        let (least, most) = OPACITY_RANGE;
        let opacity = self.opacity.filter(|opacity| opacity.is_finite())?;
        Some(opacity.clamp(least, most))
    }

    /// How long the pointer rests at the edge before the strip comes back.
    pub fn show_after(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.show_delay.min(LONGEST_DELAY))
    }

    /// How long the pointer is elsewhere before the strip goes.
    pub fn hide_after(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.hide_delay.min(LONGEST_DELAY))
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
    /// What is kept and shown of the notifications of which application.
    #[serde(default, rename = "notification_rule")]
    pub notification_rules: Vec<NotificationRule>,
    /// Whether a change of the volume is shown for a moment.
    #[serde(default)]
    pub flyout: FlyoutConfig,
    /// The notices Tessera gives in place of Explorer: drives that are
    /// plugged in, batteries that run low.
    #[serde(default)]
    pub notices: crate::notices::NoticesConfig,
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
    /// Whether Tessera keeps a history of the clipboard, and how long it
    /// is.
    #[serde(default)]
    pub clipboard: ClipboardConfig,
    /// Which night light the quick settings switch, and when Tessera's
    /// own comes on by itself.
    #[serde(default)]
    pub night_light: NightLightConfig,
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
    /// What Tessera does as the shell of the session.
    #[serde(default)]
    pub shell: crate::session::ShellConfig,
    /// How windows snap to the edges, zones and layouts of the displays.
    #[serde(default)]
    pub snap: SnapConfig,
    /// What is done to the windows each rule names.
    #[serde(default, rename = "rule")]
    pub rules: Vec<RuleConfig>,
    /// The corners and edges of the displays that do something when the
    /// pointer rests there.
    #[serde(default, rename = "corner")]
    pub corners: Vec<CornerConfig>,
    // Wave 2, declared-settings: theme files stacked over `theme`, in order.
    #[serde(default)]
    pub theme_layers: Vec<String>,
    // Wave 2, declared-settings: the values of declared theme settings, per theme name.
    #[serde(default)]
    pub theme_options: HashMap<String, toml::Table>,
    // Wave 2, resources-icon-packs: the installed icon packs, the first that has an icon wins.
    #[serde(default)]
    pub icon_packs: Vec<String>,
    // Wave 2, files-previews: thumbnails, preview and details panes of the file manager.
    // #[serde(default)]
    // pub file_view: crate::files::FileViewConfig,
    // Wave 2, desktop-icons: the icons of the desktop.
    // #[serde(default)]
    // pub desktop: crate::desktop::DesktopConfig,
    // Wave 2, system-settings: performance mode by power source and energy saver.
    // #[serde(default)]
    // pub performance: crate::performance::PerformanceConfig,
    // Wave 2, system-settings: how often the system meters are read.
    // #[serde(default)]
    // pub system: crate::modules::system::SystemConfig,
    // Wave 2, window-behaviour-rules: modifier+drag, title bar clicks, focus follows mouse.
    // #[serde(default)]
    // pub window_actions: crate::window_actions::WindowActionsConfig,
    // Wave 2, window-behaviour-rules: bundled rule sets that are switched on.
    // #[serde(default)]
    // pub rule_templates: Vec<String>,
    // Wave 2, tray-bluetooth: which notification icons are shown, hidden and in what order.
    // #[serde(default)]
    // pub tray: crate::tray_settings::TrayConfig,
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

    /// The state of the files this configuration is read from: the layout,
    /// the theme it names and the themes stacked over it.
    pub fn stamp(&self) -> Stamp {
        let themes = self.theme.iter().chain(&self.theme_layers);
        let themes = themes.map(|name| modified(&theme_file(name)));
        let times: Vec<Option<SystemTime>> = std::iter::once(modified(CONFIG_FILE))
            .chain(themes)
            .collect();
        Stamp::of(&times)
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
    fn themes_are_stacked_and_given_settings() {
        let text = concat!(
            "theme = \"ocean\"\ntheme_layers = [\"dense\", \"round\"]\n",
            "[theme_options.ocean]\nvariant = \"teal\"\n",
        );
        let config: Config = toml::from_str(text).unwrap();
        assert_eq!(config.theme_layers, ["dense", "round"]);
        assert_eq!(
            config.theme_options["ocean"]["variant"].as_str(),
            Some("teal")
        );
        let mut other = config.clone();
        other.theme_layers.pop();
        // A theme taken off the stack counts, whether its file exists or not.
        assert_ne!(config.stamp(), other.stamp());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let text = "[[surface]]\nedge = \"top\"\nthickness = 40\ncentre = [\"clock\"]\n";
        assert!(toml::from_str::<Config>(text).is_err());
    }

    // surfaces-edges: displays, overrides and looks of surfaces.

    fn display(device: &str, friendly: &str, position: usize, primary: bool) -> DisplayName {
        DisplayName {
            device: device.to_owned(),
            friendly: friendly.to_owned(),
            position,
            primary,
        }
    }

    fn surface(text: &str) -> SurfaceConfig {
        #[derive(Deserialize)]
        struct File {
            surface: Vec<SurfaceConfig>,
        }
        let file: File = toml::from_str(text).expect("a valid surface");
        file.surface.into_iter().next().expect("one surface")
    }

    #[test]
    fn a_selector_names_a_display_by_position_output_or_name() {
        let dell = display(r"\\.\DISPLAY2", "DELL U2720Q", 2, false);
        let parse = |text: &str| DisplaySelector::parse(text).unwrap();
        assert_eq!(parse("2"), DisplaySelector::Position(2));
        assert!(parse("2").matches(&dell));
        assert!(!parse("1").matches(&dell));
        assert!(parse(r"\\.\display2").matches(&dell));
        assert!(!parse(r"\\.\DISPLAY1").matches(&dell));
        assert!(parse("dell*").matches(&dell));
        assert!(parse("DELL U27?0Q").matches(&dell));
        assert!(parse("*2720*").matches(&dell));
        assert!(!parse("DELL").matches(&dell));
        assert!(parse("DELL U2720Q").matches(&dell));
        assert!(DisplaySelector::parse("0").is_err());
        assert!(DisplaySelector::parse(" ").is_err());
        assert_eq!(parse(r"\\.\DISPLAY3").text(), r"\\.\DISPLAY3");
        assert_eq!(parse("4").text(), "4");
    }

    #[test]
    fn wildcards_stand_for_any_text_or_one_character() {
        assert!(wildcard_matches("*", ""));
        assert!(wildcard_matches("a*b*c", "aXXbYYc"));
        assert!(wildcard_matches("a*c", "abcbc"));
        assert!(!wildcard_matches("a*c", "abcb"));
        assert!(wildcard_matches("??", "ab"));
        assert!(!wildcard_matches("??", "abc"));
        assert!(wildcard_matches("ÄB*", "äbc"));
    }

    #[test]
    fn displays_are_all_the_primary_the_others_or_those_named() {
        let read = |text: &str| {
            surface(&format!(
                "[[surface]]\nedge = \"top\"\nthickness = 40\ndisplays = {text}\n"
            ))
            .displays
        };
        assert_eq!(read("\"all\""), Displays::All);
        assert_eq!(read("\"primary\""), Displays::Primary);
        assert_eq!(read("\"others\""), Displays::Others);
        assert_eq!(
            read("[1, \"LG*\", '\\\\.\\DISPLAY3']"),
            Displays::Chosen(vec![
                DisplaySelector::Position(1),
                DisplaySelector::Name("LG*".into()),
                DisplaySelector::Device(r"\\.\DISPLAY3".into()),
            ])
        );
        assert_eq!(
            read("\"DELL*\""),
            Displays::Chosen(vec![DisplaySelector::Name("DELL*".into())])
        );
        let text = "[[surface]]\nedge = \"top\"\nthickness = 40\ndisplays = [0]\n";
        assert!(toml::from_str::<Config>(text).is_err());

        let main = display(r"\\.\DISPLAY1", "Built-in", 1, true);
        let side = display(r"\\.\DISPLAY2", "LG HDR", 2, false);
        let others = Displays::Others;
        assert!(!others.includes(Some(&main), true));
        assert!(others.includes(Some(&side), false));
        assert!(Displays::Primary.includes(None, true));
        assert!(Displays::All.includes(None, false));
        let chosen = read("[\"LG*\"]");
        assert!(chosen.includes(Some(&side), false));
        assert!(!chosen.includes(Some(&main), true));
        assert!(!chosen.includes(None, false));
    }

    #[test]
    fn an_override_replaces_what_it_gives_on_the_displays_it_names() {
        let bar = surface(
            "[[surface]]\nedge = \"left\"\nthickness = 40\nstart = [\"clock\"]\nend = [\"tray\"]\n\
             [surface.per_display.\"2\"]\nthickness = 56\nend = []\n\
             [surface.per_display.\"LG*\"]\nhide = \"overlap\"\nthickness = 60\n\
             [surface.per_display.\"Other\"]\nhide = true\n",
        );
        assert_eq!(bar.edge, ScreenEdge::Left);
        let main = display(r"\\.\DISPLAY1", "Built-in", 1, true);
        let (placed, replaced) = bar.on_display(Some(&main));
        assert_eq!((placed.thickness, placed.hide), (40., Hiding::Never));
        assert_eq!(replaced, [false; 3]);
        assert!(placed.per_display.is_empty());

        let side = display(r"\\.\DISPLAY2", "LG HDR", 2, false);
        let (placed, replaced) = bar.on_display(Some(&side));
        // "2" comes before "LG*" as text, so the second gives the thickness.
        assert_eq!((placed.thickness, placed.hide), (60., Hiding::Overlap));
        assert_eq!(replaced, [false, false, true]);
        assert!(placed.end.is_empty());
        assert_eq!(placed.start.len(), 1);
        let (placed, _) = bar.on_display(None);
        assert_eq!(placed.thickness, 40.);
    }

    #[test]
    fn an_override_for_no_display_rejects_the_file() {
        for selector in ["\"0\"", "\" \""] {
            let text = format!(
                "[[surface]]\nedge = \"top\"\n[surface.per_display.{selector}]\nthickness = 40\n"
            );
            assert!(toml::from_str::<Config>(&text).is_err(), "{selector}");
        }
    }

    #[test]
    fn a_surface_takes_a_part_of_its_edge_and_its_looks_are_kept_in_range() {
        use std::time::Duration;
        let bar = surface("[[surface]]\nedge = \"right\"\nthickness = 40\n");
        assert!(bar.span().is_whole());
        assert_eq!(bar.opacity(), None);
        assert_eq!(bar.show_after(), Duration::ZERO);
        assert_eq!(bar.hide_after(), Duration::from_millis(HIDE_DELAY));
        let bar = surface(
            "[[surface]]\nedge = \"bottom\"\nthickness = 40\nlength = 0.5\nalign = \"end\"\nopacity = 0.1\nfloating = false\nsolid_when = \"maximized\"\nshow_delay = 250\nhide_delay = 99999\n",
        );
        let span = bar.span();
        assert_eq!((span.from, span.to), (0.5, 1.));
        assert_eq!(bar.opacity(), Some(OPACITY_RANGE.0));
        assert_eq!(bar.floating, Some(false));
        assert_eq!(bar.solid_when, Some(SolidWhen::Maximized));
        assert_eq!(bar.show_after(), Duration::from_millis(250));
        assert_eq!(bar.hide_after(), Duration::from_millis(LONGEST_DELAY));
        let centered = surface("[[surface]]\nedge = \"top\"\nthickness = 40\nlength = 0.05\n");
        let span = centered.span();
        assert!((span.from - 0.4).abs() < 1e-6 && (span.to - 0.6).abs() < 1e-6);
        let text = "[[surface]]\nedge = \"top\"\nthickness = 40\nalign = \"middle\"\n";
        assert!(toml::from_str::<Config>(text).is_err());
    }
}
