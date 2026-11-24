// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the configuration file holds, changed without editing it by hand:
//! a rail of parts on the left, and the cards of the part chosen on the
//! right. Widget: `settings`, made for a popup in the middle of the display.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AnyView, AppContext, Context, CursorStyle, Div, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, Stateful, StatefulInteractiveElement,
    Styled, Window, div, img, px, transparent_black,
};

use crate::config::edit::{CardLength, ConfigFile, DEEDS, LAYOUTS, Place, Slot};
use crate::config::{Config, Displays, Hiding, KeyConfig, SurfaceConfig, WidgetSpec, config_dir};
use crate::desk::{CardConfig, Corner};
use crate::i18n::Language;
use crate::icons::AppIcons;
use crate::keys::{HELD, KEYS, Press, key_label};
use crate::module::{Module, WidgetRegistry};
use crate::modules::marketplace::{Changed, Marketplace};
use crate::motion::{Interaction, Spring, mix, staggered};
use crate::panel::Arranging;
use crate::platform::{AppWindow, Modifiers, ScreenEdge};
use crate::popup::Popups;
use crate::shell::Shell;
use crate::theme::{ACCENT_THEME, Theme};
use crate::tiling::TileLayout;
use crate::ui::{icon, state_layer, switch};

/// The directory of the configuration directory that holds theme files.
const THEMES_DIRECTORY: &str = "themes";

/// The height of a chip and the side of the round buttons in it.
const CHIP_HEIGHT: Pixels = px(28.);
const SMALL_BUTTON: Pixels = px(20.);
/// The size of the name of a group of widgets, as a share of the label size.
const CAPTION_SHARE: f32 = 0.86;

/// How much of the content color a chip that holds buttons has, to stand
/// out from its card.
const CHIP_TINT: f32 = 0.07;

/// The most a list of widgets to add takes while it opens.
const ADD_LIST_HEIGHT: Pixels = px(240.);
/// How tall the list of widgets for the bars gets before it scrolls.
const PALETTE_HEIGHT: Pixels = px(220.);
/// How much of the color of what is dragged a slot takes while that is
/// over it.
const ZONE_TINT: f32 = 0.14;
/// How much is seen of a widget while it is dragged away.
const LIFTED: f32 = 0.35;

/// The width of the rail of parts, and the height of a part in it.
const RAIL_WIDTH: Pixels = px(196.);
const RAIL_ITEM: Pixels = px(40.);
const RAIL_GAP: Pixels = px(4.);

/// The size of the title over the rail.
const TITLE_SIZE: Pixels = px(20.);

/// How far below its place a card starts when its part is chosen.
const CARD_RISE: f32 = 18.;

/// The width kept for the number between the buttons that change it.
const COUNT_WIDTH: Pixels = px(64.);

const ICON_BACK: &str = "\u{e76b}";
const ICON_FORWARD: &str = "\u{e76c}";
const ICON_REMOVE: &str = "\u{e711}";
const ICON_LESS: &str = "\u{e738}";
const ICON_MORE: &str = "\u{e710}";
const ICON_QUIT: &str = "\u{e7e8}";
const ICON_RESTART: &str = "\u{e72c}";

/// The widget that offers what the registries hold, and its option that
/// says what of it.
const MARKETPLACE_WIDGET: &str = "marketplace";
const MARKETPLACE_SHOWS: &str = "show";
const MARKETPLACE_COLUMNS: &str = "columns";

/// In how many columns the marketplace of the settings stands.
const MARKET_COLUMNS: i64 = 2;

/// The parts the settings are divided into, in the order of the rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Section {
    General,
    Bars,
    Desktop,
    Widgets,
    Dock,
    Tray,
    Windows,
    Keys,
    Themes,
    Extensions,
}

impl Section {
    const ALL: [Self; 10] = [
        Self::General,
        Self::Bars,
        Self::Desktop,
        Self::Widgets,
        Self::Dock,
        Self::Tray,
        Self::Windows,
        Self::Keys,
        Self::Themes,
        Self::Extensions,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Bars => "Bars",
            Self::Desktop => "Desktop",
            Self::Widgets => "Widgets",
            Self::Dock => "Dock",
            Self::Tray => "Tray",
            Self::Windows => "Windows",
            Self::Keys => "Keys",
            Self::Themes => "Themes",
            Self::Extensions => "Extensions",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::General => "\u{e713}",
            Self::Bars => "\u{e700}",
            Self::Desktop => "\u{e7f4}",
            Self::Widgets => "\u{e71d}",
            Self::Dock => "\u{e7c4}",
            Self::Tray => "\u{e712}",
            Self::Windows => "\u{e8a7}",
            Self::Keys => "\u{e765}",
            Self::Themes => "\u{e790}",
            Self::Extensions => "\u{e719}",
        }
    }

    /// What the marketplace shows in this part, if it has one.
    fn market(self) -> Option<&'static str> {
        match self {
            Self::Themes => Some("themes"),
            Self::Extensions => Some("extensions"),
            _ => None,
        }
    }
}

/// Where a setting is written in the file.
#[derive(Clone, Copy)]
enum Target {
    /// An option of every widget of this name.
    Widget(&'static str, &'static str),
    /// A key at the top of the file.
    Key(&'static str),
    /// A key of a table.
    Table(&'static str, &'static str),
}

/// What a setting can be set to.
#[derive(Clone, Copy, PartialEq)]
enum Answer {
    /// The key is not in the file.
    Unset,
    Text(&'static str),
    Number(i64),
    Flag(bool),
}

impl Answer {
    /// Whether the file holding `current` holds this answer.
    fn is(self, current: Option<&toml::Value>) -> bool {
        match (self, current) {
            (Self::Unset, None) => true,
            (Self::Text(text), Some(value)) => value.as_str() == Some(text),
            (Self::Number(number), Some(value)) => value.as_integer() == Some(number),
            (Self::Flag(flag), Some(value)) => value.as_bool() == Some(flag),
            _ => false,
        }
    }

    fn value(self) -> Option<toml_edit::Value> {
        match self {
            Self::Unset => None,
            Self::Text(text) => Some(text.into()),
            Self::Number(number) => Some(number.into()),
            Self::Flag(flag) => Some(flag.into()),
        }
    }
}

/// How a setting is chosen.
#[derive(Clone, Copy)]
enum Kind {
    /// One of a few answers, each with what its chip says.
    Choice(&'static [(&'static str, Answer)]),
    /// A number from `min` to `max` that buttons change by `step`. One
    /// step under `min` the key leaves the file, which `unset` describes;
    /// a step up from there gives `start`.
    Count {
        min: i64,
        max: i64,
        step: i64,
        start: i64,
        unset: &'static str,
    },
    /// On or off.
    Switch { on: Answer, off: Answer },
}

impl Kind {
    /// Whether a switch between `on` and `off` is on in a file that holds
    /// `current`.
    fn is_on(on: Answer, off: Answer, current: Option<&toml::Value>) -> bool {
        if off == Answer::Unset {
            on.is(current)
        } else {
            !off.is(current)
        }
    }

    /// The answer one step from `current`, up or down.
    fn stepped(self, current: Option<i64>, up: bool) -> Option<Answer> {
        let Self::Count {
            min,
            max,
            step,
            start,
            ..
        } = self
        else {
            return None;
        };
        Some(match (current, up) {
            (None, true) => Answer::Number(start),
            (None, false) => Answer::Unset,
            (Some(number), true) => Answer::Number((number + step).clamp(min, max)),
            (Some(number), false) if number - step < min => Answer::Unset,
            (Some(number), false) => Answer::Number((number - step).min(max)),
        })
    }
}

/// One line of a card: something of the file, and how it is chosen.
struct Setting {
    section: Section,
    /// The card it stands on; settings of one card follow each other.
    card: &'static str,
    title: &'static str,
    target: Target,
    kind: Kind,
}

const STYLES: Kind = Kind::Choice(&[("Wave", Answer::Unset), ("Text", Answer::Text("text"))]);

const SETTINGS: &[Setting] = &[
    Setting {
        section: Section::General,
        card: "Windows key",
        title: "Opens the launcher",
        target: Target::Key("windows_key"),
        kind: Kind::Switch {
            on: Answer::Text("launcher"),
            off: Answer::Unset,
        },
    },
    Setting {
        section: Section::General,
        card: "Contrast",
        title: "Keep text readable (WCAG 2.1 AA)",
        target: Target::Key("contrast"),
        kind: Kind::Switch {
            on: Answer::Unset,
            off: Answer::Flag(false),
        },
    },
    Setting {
        section: Section::General,
        card: "Notifications",
        title: "Banners",
        target: Target::Table("notifications", "banners"),
        kind: Kind::Switch {
            on: Answer::Flag(true),
            off: Answer::Unset,
        },
    },
    Setting {
        section: Section::General,
        card: "Notifications",
        title: "Do not disturb",
        target: Target::Table("notifications", "quiet"),
        kind: Kind::Switch {
            on: Answer::Flag(true),
            off: Answer::Unset,
        },
    },
    Setting {
        section: Section::General,
        card: "Notifications",
        title: "Seconds shown",
        target: Target::Table("notifications", "seconds"),
        kind: Kind::Count {
            min: 2,
            max: 30,
            step: 2,
            start: 6,
            unset: "Default",
        },
    },
    Setting {
        section: Section::General,
        card: "Notifications",
        title: "Volume flyout",
        target: Target::Table("flyout", "volume"),
        kind: Kind::Switch {
            on: Answer::Flag(true),
            off: Answer::Unset,
        },
    },
    Setting {
        section: Section::Widgets,
        card: "Clock",
        title: "Format",
        target: Target::Widget("clock", "format"),
        kind: Kind::Choice(&[
            ("Default", Answer::Unset),
            ("14:05", Answer::Text("%H:%M")),
            ("2:05 PM", Answer::Text("%-I:%M %p")),
            ("14:05  21.11.2026", Answer::Text("%H:%M  %d.%m.%Y")),
            ("Sat, 21 Nov  14:05", Answer::Text("%a, %-d %b  %H:%M")),
        ]),
    },
    Setting {
        section: Section::Widgets,
        card: "Calendar",
        title: "Week starts on",
        target: Target::Widget("calendar", "week_start"),
        kind: Kind::Choice(&[
            ("Monday", Answer::Unset),
            ("Sunday", Answer::Text("sunday")),
        ]),
    },
    Setting {
        section: Section::Widgets,
        card: "Processor",
        title: "Style",
        target: Target::Widget("cpu", "style"),
        kind: STYLES,
    },
    Setting {
        section: Section::Widgets,
        card: "Memory",
        title: "Style",
        target: Target::Widget("memory", "style"),
        kind: STYLES,
    },
    Setting {
        section: Section::Widgets,
        card: "Memory",
        title: "Shows",
        target: Target::Widget("memory", "show"),
        kind: Kind::Choice(&[
            ("Percent", Answer::Unset),
            ("Gigabytes", Answer::Text("used")),
        ]),
    },
    Setting {
        section: Section::Widgets,
        card: "Battery",
        title: "Style",
        target: Target::Widget("battery", "style"),
        kind: STYLES,
    },
    Setting {
        section: Section::Widgets,
        card: "Network",
        title: "Shows",
        target: Target::Widget("network", "show"),
        kind: Kind::Choice(&[("Name", Answer::Unset), ("Icon", Answer::Text("icon"))]),
    },
    Setting {
        section: Section::Dock,
        card: "Start menu",
        title: "Columns",
        target: Target::Widget("apps", "columns"),
        kind: Kind::Count {
            min: 3,
            max: 8,
            step: 1,
            start: 6,
            unset: "Default",
        },
    },
    Setting {
        section: Section::Tray,
        card: "Tray",
        title: "Icons shown",
        target: Target::Widget("tray", "limit"),
        kind: Kind::Count {
            min: 1,
            max: 30,
            step: 1,
            start: 6,
            unset: "All",
        },
    },
    Setting {
        section: Section::Tray,
        card: "Tray",
        title: "Descriptions under the pointer",
        target: Target::Widget("tray", "tips"),
        kind: Kind::Switch {
            on: Answer::Unset,
            off: Answer::Flag(false),
        },
    },
    Setting {
        section: Section::Windows,
        card: "Tiling",
        title: "Tile windows",
        target: Target::Table("tiling", "enabled"),
        kind: Kind::Switch {
            on: Answer::Flag(true),
            off: Answer::Unset,
        },
    },
    Setting {
        section: Section::Windows,
        card: "Tiling",
        title: "Layout",
        target: Target::Table("tiling", "layout"),
        kind: Kind::Choice(&[
            ("Master", Answer::Unset),
            ("Columns", Answer::Text("columns")),
            ("Rows", Answer::Text("rows")),
            ("Grid", Answer::Text("grid")),
            ("Monocle", Answer::Text("monocle")),
        ]),
    },
    Setting {
        section: Section::Windows,
        card: "Tiling",
        title: "Gap",
        target: Target::Table("tiling", "gap"),
        kind: Kind::Count {
            min: 0,
            max: 48,
            step: 4,
            start: 0,
            unset: "Default",
        },
    },
    Setting {
        section: Section::Windows,
        card: "Tiling",
        title: "Border of the window in front",
        target: Target::Table("tiling", "border"),
        kind: Kind::Switch {
            on: Answer::Flag(true),
            off: Answer::Unset,
        },
    },
    Setting {
        section: Section::Windows,
        card: "Window switcher",
        title: "Window previews",
        target: Target::Table("switcher", "previews"),
        kind: Kind::Switch {
            on: Answer::Flag(true),
            off: Answer::Unset,
        },
    },
    Setting {
        section: Section::Windows,
        card: "Workspaces",
        title: "Own workspaces",
        target: Target::Table("workspaces", "own"),
        kind: Kind::Count {
            min: 2,
            max: 9,
            step: 1,
            start: 2,
            unset: "Off",
        },
    },
    Setting {
        section: Section::Windows,
        card: "Workspaces",
        title: "Keys with a digit",
        target: Target::Table("workspaces", "keys"),
        kind: Kind::Choice(&[
            ("Off", Answer::Unset),
            ("Alt", Answer::Text("alt")),
            ("Ctrl+Alt", Answer::Text("ctrl+alt")),
            ("Win+Alt", Answer::Text("win+alt")),
        ]),
    },
];

pub struct SettingsModule;

impl Module for SettingsModule {
    fn name(&self) -> &'static str {
        "settings"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("settings", |_, cx| cx.new(Settings::new).into())
    }
}

/// A card of a part: how much room it is taken to need, in lines, and
/// what it shows.
type Card = (usize, AnyElement);

/// The cards in the two columns they are laid out in: each goes, in
/// order, where less stands so far, and is given with its place in that
/// order.
fn masonry<T>(cards: Vec<(usize, T)>) -> [Vec<(usize, T)>; 2] {
    let mut columns = [Vec::new(), Vec::new()];
    let mut heights = [0; 2];
    for (index, (weight, card)) in cards.into_iter().enumerate() {
        let column = usize::from(heights[1] < heights[0]);
        heights[column] += weight;
        columns[column].push((index, card));
    }
    columns
}

/// Something to click, which also names its pointer feedback.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Control {
    /// Shows a part.
    Section(Section),
    Quit,
    /// Ends Tessera and starts it again.
    Restart,
    /// Counts the arrangements Tessera comes with.
    Layout(usize),
    /// Starts arranging the widgets on the bars themselves.
    Arrange,
    Language(Language),
    /// Gives the setting at this place this one of its answers.
    Set(usize, usize),
    /// Takes the number of the setting at this place a step up, or down.
    Step(usize, bool),
    /// Switches the setting at this place.
    Toggle(usize),
    /// Stands for the widget at this place of a bar, which is dragged.
    Grab(Place),
    Remove(Place),
    /// Chooses this slot of a surface for the widgets that are pressed.
    Aim(usize, Slot),
    /// Adds the widget at this place of the list to the chosen slot.
    Offer(usize),
    /// Says when a surface hides.
    SurfaceHide(usize, Hiding),
    /// Opens or closes the list of widgets to add to a card.
    Adding(Holder),
    /// Adds the widget at this place of the list to what is being added to.
    Add(usize),
    /// Lays another card on the desktop.
    NewCard,
    /// Counts the cards of the desktop.
    DropCard(usize),
    /// Puts a card at this one of the corners.
    CardCorner(usize, usize),
    /// Shows a card on the primary display only, or on all of them.
    CardDisplays(usize, bool),
    /// Takes a length of a card a step up, or down.
    CardStep(usize, CardLength, bool),
    /// Moves the widget at this place of a card down, or up.
    CardShift(usize, usize, bool),
    /// Takes the widget at this place out of a card.
    CardTake(usize, usize),
    /// Counts the pinned applications.
    Unpin(usize),
    /// Counts the running applications that are offered for pinning.
    Pin(usize),
    /// Counts the theme files; `None` is the built-in theme.
    Theme(Option<usize>),
    /// Adds a key combination.
    NewKey,
    /// Counts the key combinations.
    DropKey(usize),
    /// Holds this one of the keys to hold for a combination, or lets it go.
    KeyHeld(usize, usize),
    /// Opens or closes the list of keys a combination can end with.
    KeyList(usize),
    /// Ends a combination with the key at this place of the list.
    KeyPick(usize, usize),
    /// Has a combination do this one of the deeds.
    KeyDeed(usize, usize),
    /// Gives the deed of a combination this one of its answers.
    KeyValue(usize, usize),
}

/// The deeds of a key combination as they are shown, in the order of
/// [`DEEDS`]. The last one is not chosen here: it takes a text.
const DEED_TITLES: [&str; 7] = [
    "Popup", "Tiling", "Focus", "Move", "Windows", "Dock", "Program",
];
const DEED_RUN: usize = 6;
const SIDES: [&str; 4] = ["left", "right", "up", "down"];

/// Which of [`DEEDS`] a key combination does, and with what.
fn deed_of(key: &KeyConfig) -> Option<(usize, String)> {
    let run = key.run.as_ref().map(|run| run.join(" "));
    let dock = key.dock.map(|number| number.to_string());
    let deeds = [
        &key.popup,
        &key.tiling,
        &key.focus,
        &key.carry,
        &key.switch,
        &dock,
        &run,
    ];
    let mut deeds = deeds.into_iter().enumerate();
    deeds.find_map(|(kind, deed)| Some((kind, deed.clone()?)))
}

/// What the answer `answer` of a deed is called.
fn answer_title(answer: &str, language: Language) -> SharedString {
    let title = match answer {
        "next" => "Next",
        "off" => "Off",
        "previous" => "Previous",
        "left" => "Left",
        "right" => "Right",
        "up" => "Up",
        "down" => "Down",
        _ => {
            let named = |layout: &&TileLayout| layout.name().eq_ignore_ascii_case(answer);
            match TileLayout::ALL.iter().find(named) {
                Some(layout) => layout.name(),
                None => return SharedString::new(answer),
            }
        }
    };
    language.say(title).into()
}

/// A line of a card: what is chosen, and the chooser.
fn line(theme: &Theme, title: &'static str, chooser: AnyElement) -> gpui::Div {
    div()
        .min_h(CHIP_HEIGHT)
        .flex()
        .items_center()
        .justify_between()
        .gap(theme.spacing.small)
        .child(
            div()
                .flex_none()
                .text_color(theme.colors.on_surface_variant)
                .child(title),
        )
        .child(chooser)
}

/// What widgets are added to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Holder {
    /// The card at this place.
    Card(usize),
}

/// How a control looks.
enum Face {
    /// A small round button with a character of the icon font.
    Glyph(&'static str),
    /// A chip with a text, tinted while `selected`.
    Chip { text: SharedString, selected: bool },
}

/// What the widgets of Tessera are called, by the names a layout has for
/// them.
const WIDGET_LABELS: &[(&str, &str)] = &[
    ("appearance", "Appearance"),
    ("battery", "Battery"),
    ("bluetooth-button", "Bluetooth"),
    ("brightness", "Brightness"),
    ("calendar", "Calendar"),
    ("clock", "Clock"),
    ("command", "Command"),
    ("cpu", "Processor"),
    ("desktop", "Show desktop"),
    ("focused", "Window title"),
    ("hdr", "HDR"),
    ("keyboard", "Keyboard layout"),
    ("label", "Label"),
    ("media", "Now playing"),
    ("memory", "Memory"),
    ("network", "Network"),
    ("notifications", "Notifications"),
    ("plan", "Power plan"),
    ("problems", "Problems"),
    ("quit", "Quit"),
    ("settings", "Settings"),
    ("tiling", "Tiling"),
    ("trash", "Recycle Bin"),
    ("tray", "Tray"),
    ("update", "Updates"),
    ("user", "Account"),
    ("volume", "Volume"),
    ("windows", "Windows"),
    ("workspaces", "Workspaces"),
];

/// What the widget a layout names `name` is called: one of Tessera by
/// what it shows, one of an extension by the extension and its own name,
/// any other by `name` itself.
fn widget_name(name: &str, language: Language) -> String {
    if let Some((owner, title)) = name.split_once('.') {
        return format!("{owner} \u{b7} {title}");
    }
    let label = WIDGET_LABELS.iter().find(|(widget, _)| *widget == name);
    label.map_or_else(
        || name.to_owned(),
        |(_, label)| language.say(label).to_owned(),
    )
}

/// What a widget of the layout is called: its name, and its text where it
/// has one, which tells several of a kind apart.
fn widget_title(spec: &WidgetSpec, language: Language) -> String {
    let name = widget_name(&spec.widget, language);
    match spec.options.get("text").and_then(toml::Value::as_str) {
        Some(text) => format!("{name}: {text}"),
        None => name,
    }
}

/// The widgets of a slot of a surface.
fn held(surface: &SurfaceConfig, slot: Slot) -> &[WidgetSpec] {
    match slot {
        Slot::Start => &surface.start,
        Slot::Center => &surface.center,
        Slot::End => &surface.end,
    }
}

/// A widget on its way to a slot of a bar.
struct Carried {
    /// Where it stood; `None` for one taken from the list of widgets.
    from: Option<Place>,
    /// The name a layout has for it.
    widget: String,
    title: SharedString,
}

/// What follows the pointer while a widget is dragged.
struct Lifted {
    title: SharedString,
}

impl Render for Lifted {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        div()
            .h(CHIP_HEIGHT)
            .px(theme.spacing.small + theme.spacing.tiny)
            .flex()
            .items_center()
            .rounded(theme.shape.full)
            .bg(theme.colors.primary)
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.surface_container)
            .whitespace_nowrap()
            .child(self.title.clone())
    }
}

/// What a surface is called.
fn surface_title(surface: &SurfaceConfig, language: Language) -> String {
    let edge = match surface.edge {
        ScreenEdge::Top => "Top bar",
        ScreenEdge::Bottom => "Bottom bar",
    };
    let displays = match surface.displays {
        Displays::All => "every display",
        Displays::Primary => "the primary display",
    };
    format!("{} \u{b7} {}", language.say(edge), language.say(displays))
}

/// What tells the arrangements apart: the edges of the bars and how thick
/// they are.
fn outline(config: &Config) -> Vec<(ScreenEdge, f32)> {
    let surfaces = config.surfaces.iter();
    surfaces
        .map(|surface| (surface.edge, surface.thickness))
        .collect()
}

/// A path the way it is compared and written to the file.
fn plain(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// What a pinned application is called: the file name without its
/// extension for a path, the name itself otherwise.
fn pin_title(pin: &str) -> String {
    let path = Path::new(pin);
    match path.file_stem() {
        Some(stem) if path.is_absolute() => stem.to_string_lossy().into_owned(),
        _ => pin.to_owned(),
    }
}

/// The names of the theme files, in order.
fn themes() -> Vec<String> {
    let entries = config_dir()
        .and_then(|directory| fs::read_dir(directory.join(THEMES_DIRECTORY)).ok())
        .into_iter()
        .flatten();
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .filter_map(|path| Some(path.file_stem()?.to_str()?.to_owned()))
        .collect();
    names.retain(|name| name != ACCENT_THEME);
    names.sort();
    // The colors of Windows are a choice like a file.
    names.insert(0, ACCENT_THEME.to_owned());
    names
}

/// What the chip of the theme `name` says.
fn theme_title(name: &str, language: Language) -> SharedString {
    if name == ACCENT_THEME {
        language.say("Windows accent").into()
    } else {
        SharedString::new(name)
    }
}

/// Shows the configuration and changes its file.
struct Settings {
    /// What the file held when it was last read; `None` if it is unusable.
    config: Option<Config>,
    /// The same as a table, for the keys looked up one by one.
    table: Option<toml::Table>,
    /// The part shown.
    section: Section,
    /// The place of that part in the rail, which the mark behind it
    /// travels to.
    mark: Spring,
    /// 0 when a part is chosen, 1 once its cards have arrived.
    arrival: Spring,
    /// 1 for each switch that is on, by the place of its setting.
    switches: HashMap<usize, Spring>,
    /// The outlines of the arrangements Tessera comes with.
    layouts: Vec<Vec<(ScreenEdge, f32)>>,
    /// The marketplaces of the parts that have one, made when first shown.
    markets: HashMap<Section, AnyView>,
    /// Why the last change was not made, or the file not read.
    failure: Option<SharedString>,
    /// The widgets a layout can name.
    available: Vec<String>,
    /// Which of them are made to fill a popup or a card, not for a bar.
    panels: HashSet<String>,
    /// The programs with an open window that are not pinned.
    running: Vec<PathBuf>,
    themes: Vec<String>,
    /// The languages there are to choose.
    languages: Vec<Language>,
    /// The slot of a surface that takes the widgets pressed in the list.
    aimed: (usize, Slot),
    /// 1 for the chosen slot, 0 for the others.
    aims: HashMap<(usize, Slot), Spring>,
    /// The widget of a bar that is being dragged.
    lifted: Option<Place>,
    /// The card whose list of widgets to add is open, kept while it closes.
    adding: Option<Holder>,
    /// 1 while that list is open, 0 while it is closed.
    add_list: Spring,
    /// The popups a key combination can open.
    popups: Vec<String>,
    /// The key combination whose list of keys is open, kept while it
    /// closes.
    picking: Option<usize>,
    /// 1 while that list is open, 0 while it is closed.
    key_list: Spring,
    interactions: HashMap<Control, Interaction>,
}

impl Settings {
    fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let layouts = LAYOUTS.iter().map(|(_, layout)| {
            let layout = toml::from_str::<Config>(layout);
            layout.map(|layout| outline(&layout)).unwrap_or_default()
        });
        let mut arrival = Spring::new(motion.spatial, 0.);
        arrival.animate_to(1.);
        let mut this = Self {
            config: None,
            table: None,
            section: Section::General,
            mark: Spring::new(motion.spatial_fast, 0.),
            arrival,
            switches: HashMap::new(),
            layouts: layouts.collect(),
            markets: HashMap::new(),
            failure: None,
            available: Vec::new(),
            panels: HashSet::new(),
            running: Vec::new(),
            themes: Vec::new(),
            languages: Vec::new(),
            aimed: (0, Slot::End),
            aims: HashMap::new(),
            lifted: None,
            adding: None,
            add_list: Spring::new(motion.spatial_fast, 0.),
            popups: Vec::new(),
            picking: None,
            key_list: Spring::new(motion.spatial_fast, 0.),
            interactions: HashMap::new(),
        };
        match ConfigFile::open().and_then(|file| Ok((file.config()?, file.table()))) {
            Ok((config, table)) => (this.config, this.table) = (Some(config), table),
            Err(error) => this.failure = Some(error.into()),
        }
        this.look_around(cx);
        this
    }

    /// Reads what the choices depend on besides the file.
    fn look_around(&mut self, cx: &mut Context<Self>) {
        let registry = cx.global::<WidgetRegistry>();
        self.available = registry.names();
        let panels = self.available.iter();
        self.panels = panels.filter(|id| registry.is_panel(id)).cloned().collect();
        self.themes = themes();
        self.languages = Language::installed();
        let mut popups = cx.global::<WidgetRegistry>().popup_names();
        let own = self.config.iter().flat_map(|config| &config.popups);
        popups.extend(own.map(|popup| popup.name.clone()));
        popups.sort();
        popups.dedup();
        self.popups = popups;
        let pins = self
            .config
            .as_ref()
            .and_then(Config::pinned)
            .unwrap_or_default();
        let pins: Vec<String> = pins.iter().map(|pin| pin.to_lowercase()).collect();
        let mut running: Vec<PathBuf> = Vec::new();
        for window in AppWindow::enumerate() {
            let Some(program) = window.executable else {
                continue;
            };
            let name = plain(&program).to_lowercase();
            let known = |other: &PathBuf| plain(other).to_lowercase() == name;
            if !pins.contains(&name) && !running.iter().any(known) {
                running.push(program);
            }
        }
        self.running = running;
    }

    /// Reads the file, applies `change` to it, writes it and has the shell
    /// take it over. A change that no longer fits the file is not made.
    fn change(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut ConfigFile) -> bool) {
        let changed = ConfigFile::open().and_then(|mut file| {
            if !change(&mut file) {
                return Err("the configuration has changed in the meantime".to_owned());
            }
            file.save()?;
            Ok((file.config()?, file.table()))
        });
        match changed {
            Ok((config, table)) => {
                self.config = Some(config);
                self.table = table;
                self.failure = None;
                // The controls now stand for other things.
                self.interactions.clear();
                cx.defer(Shell::config_changed);
            }
            Err(error) => {
                report!("{error}");
                self.failure = Some(error.into());
            }
        }
        self.look_around(cx);
        cx.notify();
    }

    /// What the file holds at `target`.
    fn current(&self, target: Target) -> Option<&toml::Value> {
        match target {
            Target::Widget(widget, key) => self.config.as_ref()?.option(widget, key),
            Target::Key(key) => self.table.as_ref()?.get(key),
            Target::Table(table, key) => self.table.as_ref()?.get(table)?.get(key),
        }
    }

    /// Writes `answer` at `target`.
    fn put(&mut self, target: Target, answer: Answer, cx: &mut Context<Self>) {
        let value = answer.value();
        self.change(cx, |file| match target {
            Target::Widget(widget, key) => file.set_option(widget, key, value),
            Target::Table(table, key) => file.set_in(table, key, value),
            Target::Key(key) => {
                file.set_key(key, value);
                true
            }
        });
    }

    /// Shows `section`: its cards arrive, and its marketplace is made.
    fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.section == section {
            return;
        }
        self.section = section;
        let place = Section::ALL.iter().position(|other| *other == section);
        self.mark.animate_to(place.unwrap_or_default() as f32);
        self.arrival.jump_to(0.);
        self.arrival.animate_to(1.);
        if let Some(shown) = section.market()
            && !self.markets.contains_key(&section)
        {
            let mut options = toml::Table::new();
            options.insert(MARKETPLACE_SHOWS.to_owned(), shown.into());
            options.insert(MARKETPLACE_COLUMNS.to_owned(), MARKET_COLUMNS.into());
            let spec = WidgetSpec {
                widget: MARKETPLACE_WIDGET.to_owned(),
                popup: None,
                options,
            };
            if let Some(market) = WidgetRegistry::build(&spec, None, cx) {
                // A theme that was installed or removed is one to choose
                // or one no more.
                if let Ok(market) = market.clone().downcast::<Marketplace>() {
                    let changed = |this: &mut Self, _, _: &Changed, cx: &mut Context<Self>| {
                        this.themes = themes();
                        cx.notify();
                    };
                    cx.subscribe(&market, changed).detach();
                }
                self.markets.insert(section, market);
            }
        }
        cx.notify();
    }

    fn act(&mut self, control: &Control, cx: &mut Context<Self>) {
        match *control {
            Control::Section(section) => self.show(section, cx),
            Control::Quit => cx.quit(),
            Control::Restart => crate::shell::restart(cx),
            Control::Layout(index) => {
                if let Some((_, layout)) = LAYOUTS.get(index) {
                    self.change(cx, |file| file.set_layout(layout));
                }
            }
            Control::Arrange => {
                Arranging::set(true, cx);
                // The settings give way to the bars; this window cannot
                // be told to close while it handles the click.
                cx.defer(Popups::close);
            }
            Control::Language(language) => {
                let answer = if language == Language::default() {
                    Answer::Unset
                } else {
                    Answer::Text(language.code())
                };
                self.put(Target::Key("language"), answer, cx);
            }
            Control::Set(index, choice) => {
                let Some(setting) = SETTINGS.get(index) else {
                    return;
                };
                if let Kind::Choice(answers) = setting.kind
                    && let Some(&(_, answer)) = answers.get(choice)
                {
                    self.put(setting.target, answer, cx);
                }
            }
            Control::Step(index, up) => {
                let Some(setting) = SETTINGS.get(index) else {
                    return;
                };
                let current = self.current(setting.target);
                let current = current.and_then(toml::Value::as_integer);
                if let Some(answer) = setting.kind.stepped(current, up) {
                    self.put(setting.target, answer, cx);
                }
            }
            Control::Toggle(index) => {
                let Some(setting) = SETTINGS.get(index) else {
                    return;
                };
                if let Kind::Switch { on, off } = setting.kind {
                    let is_on = Kind::is_on(on, off, self.current(setting.target));
                    self.put(setting.target, if is_on { off } else { on }, cx);
                }
            }
            Control::Grab(_) => {}
            Control::Remove(place) => self.change(cx, |file| file.remove(place)),
            Control::Aim(surface, slot) => {
                self.aimed = (surface, slot);
                cx.notify();
            }
            Control::Offer(index) => {
                let Some(widget) = self.available.get(index).cloned() else {
                    return;
                };
                if self.panels.contains(&widget) {
                    return;
                }
                let (surface, slot) = self.aimed;
                self.change(cx, |file| file.add(surface, slot, &widget));
            }
            Control::Adding(holder) => {
                let open = self.add_list.target() > 0.;
                if open && self.adding == Some(holder) {
                    self.add_list.animate_to(0.);
                } else {
                    if self.adding != Some(holder) {
                        self.add_list.jump_to(0.);
                    }
                    self.adding = Some(holder);
                    self.add_list.animate_to(1.);
                }
                cx.notify();
            }
            Control::Add(index) => {
                let (Some(holder), Some(widget)) =
                    (self.adding, self.available.get(index).cloned())
                else {
                    return;
                };
                let Holder::Card(card) = holder;
                self.change(cx, |file| file.add_to_card(card, &widget));
            }
            Control::NewCard => self.change(cx, ConfigFile::add_card),
            Control::DropCard(card) => {
                // The cards after it are counted anew.
                if matches!(self.adding, Some(Holder::Card(_))) {
                    self.adding = None;
                }
                self.change(cx, |file| file.remove_card(card));
            }
            Control::CardCorner(card, corner) => {
                if let Some(corner) = Corner::ALL.get(corner) {
                    let name = corner.name();
                    self.change(cx, |file| file.set_card(card, "corner", Some(name.into())));
                }
            }
            Control::SurfaceHide(surface, hide) => {
                let hide = match hide {
                    Hiding::Never => None,
                    Hiding::Pointer => Some(true.into()),
                    Hiding::Overlap => Some("overlap".into()),
                };
                self.change(cx, |file| file.set_surface(surface, "hide", hide));
            }
            Control::CardDisplays(card, primary) => {
                let displays = primary.then(|| "primary".into());
                self.change(cx, |file| file.set_card(card, "displays", displays));
            }
            Control::CardStep(card, length, up) => {
                self.change(cx, |file| file.step_card(card, length, up));
            }
            Control::CardShift(card, index, forward) => {
                self.change(cx, |file| file.shift_in_card(card, index, forward));
            }
            Control::CardTake(card, index) => {
                self.change(cx, |file| file.remove_from_card(card, index));
            }
            Control::Unpin(index) => {
                let Some(mut pins) = self.config.as_ref().and_then(Config::pinned) else {
                    return;
                };
                if index < pins.len() {
                    pins.remove(index);
                    self.change(cx, |file| file.set_pinned(&pins));
                }
            }
            Control::Pin(index) => {
                let (Some(mut pins), Some(program)) = (
                    self.config.as_ref().and_then(Config::pinned),
                    self.running.get(index),
                ) else {
                    return;
                };
                pins.push(plain(program));
                self.change(cx, |file| file.set_pinned(&pins));
            }
            Control::NewKey => {
                let keys = self.config.iter().flat_map(|config| &config.keys);
                let taken: Vec<Press> = keys.filter_map(|key| Press::parse(&key.press)).collect();
                let held = Modifiers {
                    control: true,
                    alt: true,
                    ..Modifiers::default()
                };
                let letters = KEYS.iter().map(|key| Press {
                    held,
                    key: (*key).to_owned(),
                });
                let free = letters.into_iter().find(|press| !taken.contains(press));
                if let Some(press) = free.and_then(|press| press.written()) {
                    self.change(cx, |file| file.add_hotkey(&press));
                }
            }
            Control::DropKey(key) => {
                // The combinations after it are counted anew.
                self.picking = None;
                self.change(cx, |file| file.remove_hotkey(key));
            }
            Control::KeyHeld(key, which) => {
                let mut press = self.press(key);
                let Some(held) = press.held_mut(which) else {
                    return;
                };
                *held = !*held;
                // The last key held stays: without one it is no combination.
                if let Some(press) = press.written() {
                    self.change(cx, |file| file.set_hotkey_press(key, &press));
                }
            }
            Control::KeyList(key) => {
                let open = self.key_list.target() > 0.;
                if open && self.picking == Some(key) {
                    self.key_list.animate_to(0.);
                } else {
                    if self.picking != Some(key) {
                        self.key_list.jump_to(0.);
                    }
                    self.picking = Some(key);
                    self.key_list.animate_to(1.);
                }
                cx.notify();
            }
            Control::KeyPick(key, at) => {
                let mut press = self.press(key);
                let Some(name) = KEYS.get(at) else {
                    return;
                };
                press.key = (*name).to_owned();
                self.key_list.animate_to(0.);
                if let Some(press) = press.written() {
                    self.change(cx, |file| file.set_hotkey_press(key, &press));
                }
            }
            Control::KeyDeed(key, kind) => {
                let first = self.answers(kind).into_iter().next();
                if let (Some(deed), Some(answer)) = (DEEDS.get(kind), first) {
                    self.change(cx, |file| file.set_hotkey_deed(key, deed, &answer));
                }
            }
            Control::KeyValue(key, at) => {
                let keys = self.config.as_ref().map(|config| &config.keys);
                let deed = keys.and_then(|keys| deed_of(keys.get(key)?));
                let Some((kind, _)) = deed else {
                    return;
                };
                if let Some(answer) = self.answers(kind).into_iter().nth(at) {
                    self.change(cx, |file| file.set_hotkey_deed(key, DEEDS[kind], &answer));
                }
            }
            Control::Theme(choice) => {
                let theme = choice.and_then(|index| self.themes.get(index).cloned());
                if choice.is_some() && theme.is_none() {
                    return;
                }
                self.change(cx, |file| {
                    file.set_theme(theme.as_deref());
                    true
                });
            }
        }
    }

    /// A control that looks like `face` and follows the pointer.
    fn control(
        &mut self,
        control: Control,
        face: Face,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.button(control, face, window, cx).into_any_element()
    }

    /// The same, for more to be said of it.
    fn button(
        &mut self,
        control: Control,
        face: Face,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let state = self
            .interactions
            .entry(control.clone())
            .or_insert_with(|| Interaction::new(&theme.motion))
            .state(window);
        let button = div()
            .id(SharedString::from(format!("{control:?}")))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full);
        let button = match face {
            Face::Glyph(glyph) => button
                .size(SMALL_BUTTON)
                .bg(state_layer(&theme, theme.colors.surface_container, state))
                .text_color(theme.colors.on_surface_variant)
                .child(icon(&theme, glyph).text_size(theme.typography.label_size)),
            Face::Chip { text, selected } => {
                let (base, color) = if selected {
                    (theme.colors.secondary_container, theme.colors.primary)
                } else {
                    (theme.colors.surface_container, theme.colors.on_surface)
                };
                button
                    .h(CHIP_HEIGHT)
                    .px(theme.spacing.small + theme.spacing.tiny)
                    .bg(state_layer(&theme, base, state))
                    .text_color(color)
                    .child(text)
            }
        };
        let action = control.clone();
        let button = button.on_click(cx.listener(move |this, _, _, cx| this.act(&action, cx)));
        Interaction::track(button, cx, move |this: &mut Self| {
            this.interactions.get_mut(&control)
        })
    }

    /// A widget of a card: its name, and buttons that move it back,
    /// move it forward and remove it, which is what `controls` do.
    fn widget_chip(
        &mut self,
        controls: [Control; 3],
        spec: &WidgetSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let known = self.available.contains(&spec.widget);
        let [back, forward, remove] = controls;
        let buttons = [
            (back, ICON_BACK),
            (forward, ICON_FORWARD),
            (remove, ICON_REMOVE),
        ]
        .map(|(control, glyph)| self.control(control, Face::Glyph(glyph), window, cx));
        div()
            .flex_none()
            .h(CHIP_HEIGHT)
            .pl(theme.spacing.small + theme.spacing.tiny)
            .pr(theme.spacing.tiny)
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .rounded(theme.shape.full)
            .bg(mix(
                theme.colors.surface_container_highest,
                theme.colors.on_surface,
                CHIP_TINT,
            ))
            // A widget no module provides is shown, but dimmed.
            .text_color(if known {
                theme.colors.on_surface
            } else {
                theme.colors.outline
            })
            .child(
                div()
                    .pr(theme.spacing.tiny)
                    .child(widget_title(spec, language)),
            )
            .children(buttons)
            .into_any_element()
    }

    /// A widget of a bar: what it is called and a button that takes it
    /// out. It is dragged to another place, and what is let go over it
    /// goes before it.
    fn pill(
        &mut self,
        place: Place,
        spec: &WidgetSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let known = self.available.contains(&spec.widget);
        let title: SharedString = widget_title(spec, language).into();
        let grab = Control::Grab(place);
        let state = self
            .interactions
            .entry(grab.clone())
            .or_insert_with(|| Interaction::new(&theme.motion))
            .state(window);
        let remove = self.control(Control::Remove(place), Face::Glyph(ICON_REMOVE), window, cx);
        let carried = Carried {
            from: Some(place),
            widget: spec.widget.clone(),
            title: title.clone(),
        };
        let view = cx.entity().downgrade();
        let base = mix(
            theme.colors.surface_container_highest,
            theme.colors.on_surface,
            CHIP_TINT,
        );
        let body = div()
            .id(SharedString::from(format!("{grab:?}")))
            .flex_none()
            .h(CHIP_HEIGHT)
            .pl(theme.spacing.small + theme.spacing.tiny)
            .pr(theme.spacing.tiny)
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .rounded(theme.shape.full)
            .bg(state_layer(&theme, base, state))
            // A widget no module provides is shown, but dimmed.
            .text_color(if known {
                theme.colors.on_surface
            } else {
                theme.colors.outline
            })
            .cursor(CursorStyle::OpenHand)
            .child(div().pr(theme.spacing.tiny).child(title))
            .child(remove)
            .on_drag(carried, move |carried: &Carried, _, _, cx| {
                let from = carried.from;
                let lifted = view.update(cx, |this, cx| {
                    this.lifted = from;
                    cx.notify();
                });
                lifted.ok();
                let title = carried.title.clone();
                cx.new(|_| Lifted { title })
            });
        let body = Interaction::track(body, cx, move |this: &mut Self| {
            this.interactions.get_mut(&grab)
        });
        let mark = theme.colors.primary;
        div()
            .flex_none()
            .pl(theme.spacing.tiny)
            .border_l_2()
            .border_color(transparent_black())
            .opacity(if self.lifted == Some(place) {
                LIFTED
            } else {
                1.
            })
            .drag_over::<Carried>(move |style, _, _, _| style.border_color(mark))
            .on_drop(cx.listener(move |this, carried: &Carried, _, cx| {
                this.land(carried, place, cx);
            }))
            .child(body)
            .into_any_element()
    }

    /// A slot of a bar, standing where it stands in the bar: its widgets,
    /// and room to let one go after them. A press chooses it for what is
    /// pressed in the list of widgets.
    fn zone(
        &mut self,
        surface: usize,
        slot: Slot,
        specs: &[WidgetSpec],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let aim = Control::Aim(surface, slot);
        let state = self
            .interactions
            .entry(aim.clone())
            .or_insert_with(|| Interaction::new(&theme.motion))
            .state(window);
        let chosen = if self.aimed == (surface, slot) {
            1.
        } else {
            0.
        };
        let spring = self
            .aims
            .entry((surface, slot))
            .or_insert_with(|| Spring::new(theme.motion.spatial_fast, chosen));
        if spring.target() != chosen {
            spring.animate_to(chosen);
        }
        let chosen = spring.value(window).clamp(0., 1.);
        let mut pills = Vec::new();
        for (index, spec) in specs.iter().enumerate() {
            let place = Place {
                surface,
                slot,
                index,
            };
            pills.push(self.pill(place, spec, window, cx));
        }
        let last = Place {
            surface,
            slot,
            index: specs.len(),
        };
        let held = wrap(&theme).min_h(CHIP_HEIGHT);
        let caption = div()
            .flex()
            .text_size(theme.typography.label_size * CAPTION_SHARE)
            .text_color(mix(
                theme.colors.on_surface_variant,
                theme.colors.primary,
                chosen,
            ))
            .child(language.say(slot.label()));
        let (held, caption) = match slot {
            Slot::Start => (held.justify_start(), caption.justify_start()),
            Slot::Center => (held.justify_center(), caption.justify_center()),
            Slot::End => (held.justify_end(), caption.justify_end()),
        };
        let held = if pills.is_empty() {
            held.child(note(&theme, language.say("Empty")))
        } else {
            held.children(pills)
        };
        let (rest, lit) = (theme.colors.surface_container, theme.colors.primary);
        let over = mix(rest, lit, ZONE_TINT);
        let action = aim.clone();
        let zone = div()
            .id(SharedString::from(format!("{aim:?}")))
            .flex_1()
            .min_w(px(0.))
            .p(theme.spacing.small)
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .rounded(theme.shape.medium)
            .border_1()
            .border_color(mix(theme.colors.surface_container, lit, chosen))
            .bg(state_layer(&theme, rest, state))
            .drag_over::<Carried>(move |style, _, _, _| style.bg(over).border_color(lit))
            .on_drop(cx.listener(move |this, carried: &Carried, _, cx| {
                this.land(carried, last, cx);
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.act(&action, cx)))
            .child(caption)
            .child(held);
        Interaction::track(zone, cx, move |this: &mut Self| {
            this.interactions.get_mut(&aim)
        })
        .into_any_element()
    }

    /// A bar as it is laid out: its three slots side by side, and when it
    /// hides.
    fn surface(
        &mut self,
        index: usize,
        surface: &SurfaceConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut zones = Vec::new();
        for slot in Slot::ALL {
            zones.push(self.zone(index, slot, held(surface, slot), window, cx));
        }
        let mut hiding = Vec::new();
        for (label, hide) in [
            ("Always shown", Hiding::Never),
            ("Hides by itself", Hiding::Pointer),
            ("Hides under windows", Hiding::Overlap),
        ] {
            let face = Face::Chip {
                text: language.say(label).into(),
                selected: surface.hide == hide,
            };
            hiding.push(self.control(Control::SurfaceHide(index, hide), face, window, cx));
        }
        card(&theme, surface_title(surface, language))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(theme.spacing.small)
                    .children(zones),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(theme.spacing.small)
                    .child(
                        div()
                            .flex_none()
                            .h(CHIP_HEIGHT)
                            .flex()
                            .items_center()
                            .text_color(theme.colors.on_surface_variant)
                            .child(language.say("Hiding")),
                    )
                    .child(wrap(&theme).flex_1().children(hiding)),
            )
            .into_any_element()
    }

    /// The widgets there are for a bar, those of Tessera first and then
    /// those of each extension under its name. One is dragged onto a bar,
    /// or pressed to go to the chosen slot; a widget of a bar let go here
    /// is taken out.
    fn palette(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        // A drag that ended anywhere is over.
        if self.lifted.is_some() && !cx.has_active_drag() {
            self.lifted = None;
        }
        let mut groups: BTreeMap<Option<String>, Vec<AnyElement>> = BTreeMap::new();
        for at in 0..self.available.len() {
            let name = self.available[at].clone();
            if self.panels.contains(&name) {
                continue;
            }
            let (owner, title) = match name.split_once('.') {
                Some((owner, title)) => (Some(owner.to_owned()), title.to_owned()),
                None => (None, widget_name(&name, language)),
            };
            let title: SharedString = title.into();
            let face = Face::Chip {
                text: title.clone(),
                selected: false,
            };
            let carried = Carried {
                from: None,
                widget: name,
                title: widget_name(&self.available[at], language).into(),
            };
            let tile = self
                .button(Control::Offer(at), face, window, cx)
                .cursor(CursorStyle::OpenHand)
                .on_drag(carried, |carried: &Carried, _, _, cx| {
                    let title = carried.title.clone();
                    cx.new(|_| Lifted { title })
                });
            groups
                .entry(owner)
                .or_default()
                .push(tile.into_any_element());
        }
        // `None`, which is Tessera, sorts first.
        let groups = groups.into_iter().map(|(owner, tiles)| {
            let title = match owner {
                Some(owner) => format!("{} {owner}", language.say("Extension")),
                None => "Tessera".to_owned(),
            };
            div()
                .flex()
                .flex_col()
                .gap(theme.spacing.tiny)
                .child(note(&theme, title).text_size(theme.typography.label_size * CAPTION_SHARE))
                .child(wrap(&theme).children(tiles))
        });
        let (rest, away) = (theme.colors.surface_container, theme.colors.outline);
        let over = mix(rest, away, ZONE_TINT);
        let list = div()
            .id("palette")
            .max_h(PALETTE_HEIGHT)
            .overflow_y_scroll()
            .p(theme.spacing.small)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .rounded(theme.shape.medium)
            .bg(rest)
            .drag_over::<Carried>(move |style, _, _, _| style.bg(over))
            .on_drop(cx.listener(|this, carried: &Carried, _, cx| {
                if let Some(from) = carried.from {
                    this.change(cx, |file| file.remove(from));
                }
            }))
            .children(groups);
        let remark = "Drag a widget onto a bar, or press it to add it to the chosen part. Drag one between the parts to move it, and back here to take it out.";
        card(&theme, language.say("Widgets"))
            .child(note(&theme, language.say(remark)))
            .child(list)
            .into_any_element()
    }

    /// Puts what was dragged before what stands at `to`, or last in that
    /// slot: a widget of a bar is moved there, one of the list is added.
    fn land(&mut self, carried: &Carried, to: Place, cx: &mut Context<Self>) {
        self.aimed = (to.surface, to.slot);
        if let Some(from) = carried.from {
            self.change(cx, |file| file.transfer(from, to));
            return;
        }
        if self.panels.contains(&carried.widget) {
            return;
        }
        let surfaces = self.config.as_ref().map(|config| &config.surfaces);
        let Some(count) = surfaces.and_then(|all| Some(held(all.get(to.surface)?, to.slot).len()))
        else {
            return;
        };
        let widget = carried.widget.clone();
        self.change(cx, |file| {
            // It is added last, and goes from there to its place.
            let added = Place { index: count, ..to };
            file.add(to.surface, to.slot, &widget)
                && (to.index >= count || file.transfer(added, to))
        });
    }

    /// The chip that opens the list of widgets to add to `holder`.
    fn add_chip(
        &mut self,
        holder: Holder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let face = Face::Chip {
            text: Language::of(cx).say("Add").into(),
            selected: self.adding == Some(holder) && self.add_list.target() > 0.,
        };
        self.control(Control::Adding(holder), face, window, cx)
    }

    /// The list of widgets to add to `holder`, while it is open there.
    fn offers(
        &mut self,
        holder: Holder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let theme = *cx.global::<Theme>();
        let add_list = self.add_list.value(window);
        if self.adding != Some(holder) || add_list <= 0. {
            return None;
        }
        let language = Language::of(cx);
        // Those of Tessera first, then those of each extension under its
        // name.
        let mut groups: BTreeMap<Option<String>, Vec<AnyElement>> = BTreeMap::new();
        for at in 0..self.available.len() {
            let name = self.available[at].clone();
            let (owner, title) = match name.split_once('.') {
                Some((owner, title)) => (Some(owner.to_owned()), title.to_owned()),
                None => (None, widget_name(&name, language)),
            };
            let face = Face::Chip {
                text: title.into(),
                selected: true,
            };
            let chip = self.control(Control::Add(at), face, window, cx);
            groups.entry(owner).or_default().push(chip);
        }
        // `None`, which is Tessera, sorts first.
        let groups = groups.into_iter().map(|(owner, chips)| {
            let title = match owner {
                Some(owner) => format!("{} {owner}", language.say("Extension")),
                None => "Tessera".to_owned(),
            };
            div()
                .flex()
                .flex_col()
                .gap(theme.spacing.tiny)
                .child(note(&theme, title).text_size(theme.typography.label_size * CAPTION_SHARE))
                .child(wrap(&theme).children(chips))
        });
        let list = div()
            .id(SharedString::from(format!("offers-{holder:?}")))
            .max_h(ADD_LIST_HEIGHT * add_list)
            .opacity(add_list.clamp(0., 1.))
            .overflow_y_scroll()
            .p(theme.spacing.small)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container)
            .children(groups);
        Some(div().child(list))
    }

    fn desk(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let about = "Panels of widgets that lie on the desktop, below every window.";
        let face = Face::Chip {
            text: language.say("Add a card").into(),
            selected: true,
        };
        let add = self.control(Control::NewCard, face, window, cx);
        card(&theme, language.say("Cards"))
            .child(note(&theme, language.say(about)))
            .child(div().flex().child(add))
            .into_any_element()
    }

    /// A card of the desktop: where it is, how large, and its widgets.
    fn desk_card(
        &mut self,
        index: usize,
        config: &CardConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let line =
            |title: &'static str, chooser: AnyElement| line(&theme, language.say(title), chooser);
        let title = format!("{} {}", language.say("Card"), index + 1);
        let mut section = card(&theme, title);

        let mut corners = Vec::new();
        for (at, corner) in Corner::ALL.into_iter().enumerate() {
            let face = Face::Chip {
                text: language.say(corner.label()).into(),
                selected: config.corner == corner,
            };
            corners.push(self.control(Control::CardCorner(index, at), face, window, cx));
        }
        let corners = wrap(&theme).justify_end().children(corners);
        section = section.child(line("Place", corners.into_any_element()));

        let mut displays = Vec::new();
        for (label, primary) in [("All displays", false), ("Primary display", true)] {
            let face = Face::Chip {
                text: language.say(label).into(),
                selected: (config.displays == Displays::Primary) == primary,
            };
            displays.push(self.control(Control::CardDisplays(index, primary), face, window, cx));
        }
        let displays = wrap(&theme).justify_end().children(displays);
        section = section.child(line("Displays", displays.into_any_element()));

        for length in CardLength::ALL {
            let now = match length {
                CardLength::Width => config.width,
                CardLength::Height => config.height,
                CardLength::X => config.x,
                CardLength::Y => config.y,
            };
            let [less, more] = [(false, ICON_LESS), (true, ICON_MORE)].map(|(up, glyph)| {
                let control = Control::CardStep(index, length, up);
                self.control(control, Face::Glyph(glyph), window, cx)
            });
            let stepper = div()
                .flex_none()
                .flex()
                .items_center()
                .gap(theme.spacing.tiny)
                .child(less)
                .child(
                    div()
                        .w(COUNT_WIDTH)
                        .flex()
                        .justify_center()
                        .child(format!("{}", now.round() as i64)),
                )
                .child(more);
            section = section.child(line(length.label(), stepper.into_any_element()));
        }

        let holder = Holder::Card(index);
        let mut chips = Vec::new();
        for (at, spec) in config.content.iter().enumerate() {
            let controls = [
                Control::CardShift(index, at, false),
                Control::CardShift(index, at, true),
                Control::CardTake(index, at),
            ];
            chips.push(self.widget_chip(controls, spec, window, cx));
        }
        chips.push(self.add_chip(holder, window, cx));
        section = section
            .child(
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(language.say("Widgets of the card")),
            )
            .child(wrap(&theme).children(chips))
            .children(self.offers(holder, window, cx));

        let face = Face::Chip {
            text: language.say("Remove the card").into(),
            selected: false,
        };
        let remove = self.control(Control::DropCard(index), face, window, cx);
        section
            .child(div().flex().justify_end().child(remove))
            .into_any_element()
    }

    /// What the key combination at `key` presses; Ctrl and the first key
    /// where the file has none that can be held.
    fn press(&self, key: usize) -> Press {
        let keys = self.config.as_ref().map(|config| &config.keys);
        let press = keys.and_then(|keys| Press::parse(&keys.get(key)?.press));
        press.unwrap_or_else(|| Press {
            held: Modifiers::default(),
            key: KEYS[0].to_owned(),
        })
    }

    /// What the deed at `kind` of [`DEEDS`] can be done with, as the file
    /// writes it. A program is not among it: it is a text.
    fn answers(&self, kind: usize) -> Vec<String> {
        let layouts = TileLayout::ALL.iter();
        let layouts = layouts.map(|layout| layout.name().to_ascii_lowercase());
        match DEEDS.get(kind) {
            Some(&"popup") => self.popups.clone(),
            Some(&"tiling") => { ["next", "off"].map(str::to_owned).into_iter() }
                .chain(layouts)
                .collect(),
            Some(&"focus" | &"move") => SIDES.map(str::to_owned).to_vec(),
            Some(&"switch") => ["next", "previous"].map(str::to_owned).to_vec(),
            Some(&"dock") => (1..=9).map(|number| number.to_string()).collect(),
            _ => Vec::new(),
        }
    }

    /// What the key combinations are, and the button that adds one.
    fn keys(&mut self, config: &Config, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let about = "Key combinations Tessera answers wherever they are pressed. One that Windows or another program holds does nothing and is reported as a problem.";
        let face = Face::Chip {
            text: language.say("Add a key").into(),
            selected: true,
        };
        let add = self.control(Control::NewKey, face, window, cx);
        let none = config.keys.is_empty();
        let none = none.then(|| note(&theme, language.say("No key combination is set.")));
        card(&theme, language.say("Keys"))
            .child(note(&theme, language.say(about)))
            .children(none)
            .child(div().flex().child(add))
            .into_any_element()
    }

    /// The list of keys the combination at `index` can end with, while it
    /// is open there.
    fn key_offers(
        &mut self,
        index: usize,
        now: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let theme = *cx.global::<Theme>();
        let key_list = self.key_list.value(window);
        if self.picking != Some(index) || key_list <= 0. {
            return None;
        }
        let mut offers = Vec::new();
        for (at, key) in KEYS.into_iter().enumerate() {
            let face = Face::Chip {
                text: key_label(key).into(),
                selected: now == Some(key),
            };
            offers.push(self.control(Control::KeyPick(index, at), face, window, cx));
        }
        Some(
            div()
                .max_h(ADD_LIST_HEIGHT * key_list)
                .opacity(key_list.clamp(0., 1.))
                .overflow_hidden()
                .child(wrap(&theme).children(offers)),
        )
    }

    /// A key combination: the keys held, the key pressed, and what it does.
    fn key_card(
        &mut self,
        index: usize,
        key: &KeyConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let press = Press::parse(&key.press);
        // A combination that cannot be held is shown as the file has it.
        let title = { press.as_ref() }.map_or_else(|| key.press.clone(), Press::shown);
        let mut section = card(&theme, title);

        let mut held = Vec::new();
        for (which, name) in HELD.into_iter().enumerate() {
            let mut pressed = press.clone();
            let is_held = pressed.as_mut().and_then(|press| press.held_mut(which));
            let face = Face::Chip {
                text: name.into(),
                selected: is_held.is_some_and(|held| *held),
            };
            held.push(self.control(Control::KeyHeld(index, which), face, window, cx));
        }
        let held = wrap(&theme).justify_end().children(held);
        section = section.child(line(&theme, language.say("Held"), held.into_any_element()));

        let now = press.as_ref().map(|press| press.key.as_str());
        let face = Face::Chip {
            text: now.map_or_else(|| "?".to_owned(), key_label).into(),
            selected: self.picking == Some(index) && self.key_list.target() > 0.,
        };
        let pick = self.control(Control::KeyList(index), face, window, cx);
        section = section
            .child(line(&theme, language.say("Key"), pick))
            .children(self.key_offers(index, now, window, cx));

        let deed = deed_of(key);
        let kind = deed.as_ref().map(|(kind, _)| *kind);
        let mut kinds = Vec::new();
        for (at, title) in DEED_TITLES.into_iter().enumerate() {
            // A program is shown where the file has one and not offered.
            if at == DEED_RUN && kind != Some(DEED_RUN) {
                continue;
            }
            let face = Face::Chip {
                text: language.say(title).into(),
                selected: kind == Some(at),
            };
            kinds.push(self.control(Control::KeyDeed(index, at), face, window, cx));
        }
        let kinds = wrap(&theme).justify_end().children(kinds);
        section = section.child(line(&theme, language.say("Does"), kinds.into_any_element()));

        if let Some((kind, value)) = deed {
            if kind == DEED_RUN {
                let by_hand = "The program is written in the file by hand.";
                section = section
                    .child(div().child(SharedString::new(value)))
                    .child(note(&theme, language.say(by_hand)));
            } else {
                let mut answers = Vec::new();
                for (at, answer) in self.answers(kind).into_iter().enumerate() {
                    let face = Face::Chip {
                        text: answer_title(&answer, language),
                        selected: answer == value,
                    };
                    answers.push(self.control(Control::KeyValue(index, at), face, window, cx));
                }
                section = section.child(wrap(&theme).justify_end().children(answers));
            }
        }

        let face = Face::Chip {
            text: language.say("Remove the key").into(),
            selected: false,
        };
        let remove = self.control(Control::DropKey(index), face, window, cx);
        section
            .child(div().flex().justify_end().child(remove))
            .into_any_element()
    }

    fn pins(&mut self, pins: &[String], window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut pinned = Vec::new();
        for (index, pin) in pins.iter().enumerate() {
            let remove = self.control(Control::Unpin(index), Face::Glyph(ICON_REMOVE), window, cx);
            let image = cx.global::<AppIcons>().of_executable(Path::new(pin));
            pinned.push(
                div()
                    .flex_none()
                    .h(CHIP_HEIGHT)
                    .pl(theme.spacing.small)
                    .pr(theme.spacing.tiny)
                    .flex()
                    .items_center()
                    .gap(theme.spacing.tiny + theme.spacing.tiny)
                    .rounded(theme.shape.full)
                    .bg(mix(
                        theme.colors.surface_container_highest,
                        theme.colors.on_surface,
                        CHIP_TINT,
                    ))
                    .children(image.map(|image| img(image).size(SMALL_BUTTON).flex_none()))
                    .child(pin_title(pin))
                    .child(remove),
            );
        }
        let mut offers = Vec::new();
        for index in 0..self.running.len() {
            let name = self.running[index].file_stem().unwrap_or_default();
            let face = Face::Chip {
                text: name.to_string_lossy().into_owned().into(),
                selected: true,
            };
            offers.push(self.control(Control::Pin(index), face, window, cx));
        }
        card(&theme, language.say("Pinned applications"))
            .child(if pinned.is_empty() {
                note(&theme, language.say("Nothing is pinned.")).into_any_element()
            } else {
                wrap(&theme).children(pinned).into_any_element()
            })
            .children(
                (!offers.is_empty())
                    .then(|| note(&theme, language.say("Running now, click to pin:"))),
            )
            .children((!offers.is_empty()).then(|| wrap(&theme).children(offers)))
            .into_any_element()
    }

    fn themes(
        &mut self,
        current: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut choices = vec![self.control(
            Control::Theme(None),
            Face::Chip {
                text: language.say("Built-in").into(),
                selected: current.is_none(),
            },
            window,
            cx,
        )];
        for index in 0..self.themes.len() {
            let face = Face::Chip {
                text: theme_title(&self.themes[index], language),
                selected: current == Some(self.themes[index].as_str()),
            };
            choices.push(self.control(Control::Theme(Some(index)), face, window, cx));
        }
        card(&theme, language.say("Theme"))
            .child(wrap(&theme).children(choices))
            .children((self.themes.len() == 1).then(|| {
                note(
                    &theme,
                    language.say("Theme files are looked for in the themes directory of the configuration directory."),
                )
            }))
            .into_any_element()
    }

    /// The way to arrange the widgets on the bars themselves.
    fn arranging(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let face = Face::Chip {
            text: language.say("Arrange on the bars").into(),
            selected: true,
        };
        let start = self.control(Control::Arrange, face, window, cx);
        let remark = "Drag a widget to another place of its bar, or off the bar to take it out; the plus adds one.";
        card(&theme, language.say("By hand"))
            .child(wrap(&theme).child(start))
            .child(note(&theme, language.say(remark)))
            .into_any_element()
    }

    /// The arrangements Tessera comes with, the one in use marked.
    fn arrangements(
        &mut self,
        config: &Config,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Card {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let now = outline(config);
        let mut choices = Vec::new();
        for (index, (name, _)) in LAYOUTS.iter().enumerate() {
            let face = Face::Chip {
                text: (*name).into(),
                selected: self.layouts.get(index) == Some(&now),
            };
            choices.push(self.control(Control::Layout(index), face, window, cx));
        }
        let remark = "Replaces the bars and popups of the layout; pinned applications stay.";
        let card = card(&theme, language.say("Arrangement"))
            .child(wrap(&theme).children(choices))
            .child(note(&theme, language.say(remark)));
        (4, card.into_any_element())
    }

    /// The languages the settings speak.
    fn languages(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Card {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut choices = Vec::new();
        for other in self.languages.clone() {
            let face = Face::Chip {
                text: other.name().into(),
                selected: other == language,
            };
            choices.push(self.control(Control::Language(other), face, window, cx));
        }
        let card = card(&theme, language.say("Language"))
            .child(wrap(&theme).children(choices))
            .child(note(
                &theme,
                language
                    .say("Of the settings, the marketplace, the calendar, the clock and the notifications."),
            ));
        (3, card.into_any_element())
    }

    /// What chooses the setting at `index`.
    fn chooser(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let setting = &SETTINGS[index];
        let current = self.current(setting.target).cloned();
        match setting.kind {
            Kind::Choice(answers) => {
                let mut chips = Vec::new();
                for (choice, &(label, answer)) in answers.iter().enumerate() {
                    let face = Face::Chip {
                        text: language.say(label).into(),
                        selected: answer.is(current.as_ref()),
                    };
                    chips.push(self.control(Control::Set(index, choice), face, window, cx));
                }
                wrap(&theme)
                    .justify_end()
                    .children(chips)
                    .into_any_element()
            }
            Kind::Count { unset, .. } => {
                let number = current.as_ref().and_then(toml::Value::as_integer);
                let shown: SharedString = match number {
                    Some(number) => number.to_string().into(),
                    None => language.say(unset).into(),
                };
                let less = self.control(
                    Control::Step(index, false),
                    Face::Glyph(ICON_LESS),
                    window,
                    cx,
                );
                let more = self.control(
                    Control::Step(index, true),
                    Face::Glyph(ICON_MORE),
                    window,
                    cx,
                );
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.tiny)
                    .child(less)
                    .child(div().w(COUNT_WIDTH).flex().justify_center().child(shown))
                    .child(more)
                    .into_any_element()
            }
            Kind::Switch { on, off } => {
                let is_on = f32::from(u8::from(Kind::is_on(on, off, current.as_ref())));
                let spring = self
                    .switches
                    .entry(index)
                    .or_insert_with(|| Spring::new(theme.motion.spatial_fast, is_on));
                if spring.target() != is_on {
                    spring.animate_to(is_on);
                }
                let travel = spring.value(window);
                let control = Control::Toggle(index);
                let state = self
                    .interactions
                    .entry(control.clone())
                    .or_insert_with(|| Interaction::new(&theme.motion))
                    .state(window);
                let action = control.clone();
                let id = SharedString::from(format!("{control:?}"));
                let switch = switch(&theme, id, travel, state)
                    .on_click(cx.listener(move |this, _, _, cx| this.act(&action, cx)));
                Interaction::track(switch, cx, move |this: &mut Self| {
                    this.interactions.get_mut(&control)
                })
                .into_any_element()
            }
        }
    }

    /// The cards of the settings of `section`: those that follow each
    /// other with the same card stand on one.
    fn setting_cards(
        &mut self,
        section: Section,
        config: &Config,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Card> {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut cards: Vec<(&'static str, usize, gpui::Div)> = Vec::new();
        for (index, setting) in SETTINGS.iter().enumerate() {
            if setting.section != section {
                continue;
            }
            if cards.last().is_none_or(|(name, ..)| *name != setting.card) {
                cards.push((setting.card, 1, card(&theme, language.say(setting.card))));
            }
            let missing = match setting.target {
                Target::Widget(widget, _) => config.widget(widget).is_none(),
                _ => false,
            };
            let line = if missing {
                // Said once for the card, however many settings it has.
                let said = cards.last().is_some_and(|(_, weight, _)| *weight > 1);
                (!said).then(|| note(&theme, language.say("Not in the layout")))
            } else {
                let chooser = self.chooser(index, window, cx);
                Some(
                    div()
                        .min_h(CHIP_HEIGHT)
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(theme.spacing.small)
                        .child(
                            div()
                                .flex_none()
                                .text_color(theme.colors.on_surface_variant)
                                .child(language.say(setting.title)),
                        )
                        .child(chooser),
                )
            };
            let Some((_, weight, card)) = cards.pop() else {
                continue;
            };
            let wide = matches!(setting.kind, Kind::Choice(answers) if answers.len() > 3);
            let weight = weight + usize::from(line.is_some()) + usize::from(wide);
            cards.push((setting.card, weight, card.children(line)));
        }
        let cards = cards.into_iter();
        cards
            .map(|(_, weight, card)| (weight, card.into_any_element()))
            .collect()
    }

    /// The rail: the title, the parts with a mark behind the one shown,
    /// and at its end the buttons that start Tessera again and close it.
    fn rail(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mark = self.mark.value(window);
        let mut parts = Vec::new();
        for section in Section::ALL {
            parts.push(self.rail_item(
                Control::Section(section),
                section.glyph(),
                language.say(section.title()),
                section == self.section,
                window,
                cx,
            ));
        }
        let restart = self.rail_item(
            Control::Restart,
            ICON_RESTART,
            language.say("Restart Tessera"),
            false,
            window,
            cx,
        );
        let quit = self.rail_item(
            Control::Quit,
            ICON_QUIT,
            language.say("Quit"),
            false,
            window,
            cx,
        );
        div()
            .flex_none()
            .w(RAIL_WIDTH)
            .flex()
            .flex_col()
            .gap(theme.spacing.medium)
            .child(
                div()
                    .px(theme.spacing.small)
                    .text_size(TITLE_SIZE)
                    .font_weight(FontWeight::MEDIUM)
                    .child(language.say("Settings")),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .gap(RAIL_GAP)
                    .child(
                        div()
                            .absolute()
                            .left(px(0.))
                            .right(px(0.))
                            .top((RAIL_ITEM + RAIL_GAP) * mark)
                            .h(RAIL_ITEM)
                            .rounded(theme.shape.full)
                            .bg(theme.colors.secondary_container),
                    )
                    .children(parts),
            )
            .child(div().flex_1())
            .child(restart)
            .child(quit)
            .into_any_element()
    }

    /// A line of the rail: an icon and a text.
    fn rail_item(
        &mut self,
        control: Control,
        glyph: &'static str,
        text: &'static str,
        chosen: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let state = self
            .interactions
            .entry(control.clone())
            .or_insert_with(|| Interaction::new(&theme.motion))
            .state(window);
        let color = if chosen {
            theme.colors.primary
        } else {
            theme.colors.on_surface_variant
        };
        let layer = crate::ui::HOVER_LAYER * state.hover + crate::ui::PRESS_LAYER * state.press;
        let action = control.clone();
        let item = div()
            .id(SharedString::from(format!("{control:?}")))
            .relative()
            .h(RAIL_ITEM)
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.small + theme.spacing.tiny)
            .rounded(theme.shape.full)
            // The mark behind the part shown is seen through it.
            .bg(theme.colors.on_surface.opacity(layer.clamp(0., 1.)))
            .text_color(color)
            .child(icon(&theme, glyph))
            .child(text)
            .on_click(cx.listener(move |this, _, _, cx| this.act(&action, cx)));
        Interaction::track(item, cx, move |this: &mut Self| {
            this.interactions.get_mut(&control)
        })
        .into_any_element()
    }

    /// The cards of the part shown: those as wide as the page, and those
    /// that share it.
    fn cards(
        &mut self,
        config: &Config,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Vec<AnyElement>, Vec<Card>) {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let section = self.section;
        let mut wide = Vec::new();
        let mut shared = Vec::new();
        match section {
            Section::General => {
                shared.push(self.arrangements(config, window, cx));
                shared.push(self.languages(window, cx));
            }
            Section::Bars => {
                wide.push(self.arranging(window, cx));
                wide.push(self.palette(window, cx));
                for (index, surface) in config.surfaces.iter().enumerate() {
                    wide.push(self.surface(index, surface, window, cx));
                }
            }
            Section::Desktop => {
                wide.push(self.desk(window, cx));
                for (index, card) in config.cards.iter().enumerate() {
                    wide.push(self.desk_card(index, card, window, cx));
                }
            }
            Section::Dock => wide.push(match config.pinned() {
                Some(pins) => self.pins(&pins, window, cx),
                None => {
                    let lacking =
                        "Applications are pinned to the `windows` widget, which the layout lacks.";
                    card(&theme, language.say("Pinned applications"))
                        .child(note(&theme, language.say(lacking)))
                        .into_any_element()
                }
            }),
            Section::Keys => {
                wide.push(self.keys(config, window, cx));
                for (index, key) in config.keys.iter().enumerate() {
                    wide.push(self.key_card(index, key, window, cx));
                }
            }
            Section::Themes => wide.push(self.themes(config.theme.as_deref(), window, cx)),
            Section::Widgets | Section::Tray | Section::Windows | Section::Extensions => {}
        }
        shared.extend(self.setting_cards(section, config, window, cx));
        (wide, shared)
    }
}

/// A card: a rounded field with the title of what it holds.
fn card(theme: &Theme, title: impl Into<SharedString>) -> gpui::Div {
    div()
        .p(theme.spacing.medium)
        .flex()
        .flex_col()
        .gap(theme.spacing.small + theme.spacing.tiny)
        .rounded(theme.shape.large)
        .bg(theme.colors.surface_container_highest)
        .child(heading(theme, title))
}

/// The title of a part of the settings.
fn heading(theme: &Theme, text: impl Into<SharedString>) -> gpui::Div {
    div()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.primary)
        .child(text.into())
}

/// A remark in the quieter color.
fn note(theme: &Theme, text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_color(theme.colors.on_surface_variant)
        .child(text.into())
}

/// A row of chips that continues on the next line.
fn wrap(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(theme.spacing.tiny + theme.spacing.tiny)
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let rail = self.rail(window, cx);
        let arrival = self.arrival.value(window);
        // A card comes up into its place, each a little after the one before.
        let arriving = move |index: usize, count: usize, card: AnyElement| {
            let along = staggered(arrival, index, count);
            div()
                .relative()
                .top(px(CARD_RISE * (1. - along)))
                .opacity(along.clamp(0., 1.))
                .child(card)
        };
        let mut page = div()
            .id(SharedString::from(format!("{:?}", self.section)))
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(theme.spacing.medium)
            .children(self.failure.clone().map(|failure| note(&theme, failure)));
        if let Some(config) = self.config.clone() {
            let (wide, shared) = self.cards(&config, window, cx);
            let count = wide.len() + shared.len() + 1;
            let before = wide.len();
            let wide = wide.into_iter().enumerate();
            page = page.children(wide.map(|(index, card)| arriving(index, count, card)));
            if !shared.is_empty() {
                let columns = masonry(shared).map(|column| {
                    let cards = column.into_iter();
                    let cards = cards.map(|(index, card)| arriving(before + index, count, card));
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(theme.spacing.medium)
                        .children(cards)
                });
                page = page.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(theme.spacing.medium)
                        .children(columns),
                );
            }
            // The marketplace scrolls by itself in the room that is left.
            if let Some(market) = self.markets.get(&self.section).cloned() {
                page = page.child(
                    arriving(count - 1, count, market.into_any_element())
                        .flex_1()
                        .min_h(px(0.))
                        .flex()
                        .flex_col(),
                );
            }
        }
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .gap(theme.spacing.medium)
            .child(rail)
            .child(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_widget_is_called_by_what_it_shows_or_by_its_extension() {
        assert_eq!(widget_name("cpu", Language::English), "Processor");
        assert_eq!(widget_name("cpu", Language::Russian), "Процессор");
        assert_eq!(
            widget_name("home.air", Language::Russian),
            "home \u{b7} air"
        );
        assert_eq!(widget_name("unheard-of", Language::English), "unheard-of");
        for (_, label) in WIDGET_LABELS {
            let russian = Language::Russian.say(label);
            assert!(russian != *label || label.is_ascii() && label.len() <= 9);
        }
    }

    #[test]
    fn a_pinned_application_is_called_by_its_file_or_its_name() {
        assert_eq!(pin_title("C:/Windows/System32/charmap.exe"), "charmap");
        assert_eq!(pin_title("Command Prompt"), "Command Prompt");
        assert_eq!(pin_title("notes.v2"), "notes.v2");
    }

    #[test]
    fn cards_go_where_less_stands() {
        let [left, right] = masonry(vec![(4, 'a'), (2, 'b'), (2, 'c'), (1, 'd'), (3, 'e')]);
        assert_eq!(left, [(0, 'a'), (3, 'd')]);
        assert_eq!(right, [(1, 'b'), (2, 'c'), (4, 'e')]);
    }

    #[test]
    fn a_number_leaves_the_file_a_step_under_its_least() {
        let count = Kind::Count {
            min: 2,
            max: 4,
            step: 1,
            start: 3,
            unset: "Off",
        };
        assert!(count.stepped(None, true) == Some(Answer::Number(3)));
        assert!(count.stepped(Some(4), true) == Some(Answer::Number(4)));
        assert!(count.stepped(Some(3), false) == Some(Answer::Number(2)));
        assert!(count.stepped(Some(2), false) == Some(Answer::Unset));
        assert!(count.stepped(None, false) == Some(Answer::Unset));
    }

    #[test]
    fn a_switch_is_on_by_what_the_file_holds() {
        let (shown, hidden) = (Answer::Unset, Answer::Flag(false));
        assert!(Kind::is_on(shown, hidden, None));
        assert!(Kind::is_on(shown, hidden, Some(&"left".into())));
        assert!(!Kind::is_on(shown, hidden, Some(&false.into())));
        let (tiled, left_alone) = (Answer::Flag(true), Answer::Unset);
        assert!(!Kind::is_on(tiled, left_alone, None));
        assert!(!Kind::is_on(tiled, left_alone, Some(&false.into())));
        assert!(Kind::is_on(tiled, left_alone, Some(&true.into())));
    }

    #[test]
    fn every_setting_stands_on_one_card() {
        for (index, setting) in SETTINGS.iter().enumerate() {
            let card = (setting.section, setting.card);
            let before = &SETTINGS[..index];
            let last = before.last().map(|last| (last.section, last.card));
            let earlier = before
                .iter()
                .any(|other| (other.section, other.card) == card);
            assert!(
                !earlier || last == Some(card),
                "`{}` is split",
                setting.card
            );
        }
    }

    #[test]
    fn paths_are_written_with_forward_slashes() {
        assert_eq!(plain(Path::new(r"C:\Apps\app.exe")), "C:/Apps/app.exe");
    }
}
