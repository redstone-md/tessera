// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the configuration file holds, changed without editing it by hand:
//! a rail of parts on the left, and the cards of the part chosen on the
//! right. Widget: `settings`, made for a popup in the middle of the display.
//! The settings that extensions and themes declare are drawn by the
//! controls of [`controls`].

mod builtin;
mod controls;
mod dynamic;
mod placed;
pub(super) mod rows;

#[allow(unused_imports, reason = "for the Shortcuts page of shortcuts-editor")]
pub(crate) use rows::{ActionTitle, action_titles};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AnyView, AppContext, Context, CursorStyle, Div, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, Stateful, StatefulInteractiveElement,
    Styled, Window, div, img, px, transparent_black,
};

use crate::config::edit::{CardLength, ConfigFile, DEEDS, LAYOUTS, Place, Slot, WidgetPlace};
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
use crate::theme::WALLPAPER_THEME;
use crate::theme::{ACCENT_THEME, Theme};
use crate::tiling::TileLayout;
use crate::ui::{icon, state_layer};
use controls::{CHANGED, DeclaredState, Deed, RowKey};

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
/// How tall the list of widgets to add is taken to be while it opens.
const OFFERS_HEIGHT: Pixels = px(420.);
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
const ICON_CHECKED: &str = "\u{e73a}";
const ICON_UNCHECKED: &str = "\u{e739}";

/// The widget that offers what the registries hold, and its option that
/// says what of it.
const MARKETPLACE_WIDGET: &str = "marketplace";
const MARKETPLACE_SHOWS: &str = "show";
const MARKETPLACE_COLUMNS: &str = "columns";

/// In how many columns the marketplace of the settings stands.
const MARKET_COLUMNS: i64 = 2;

/// The page of the Keys section that takes the place of its key cards.
const SHORTCUTS: &str = "shortcuts";

/// The parts the settings are divided into, in the order of the rail,
/// which is the order of [`rows::SECTIONS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Section {
    General,
    Bars,
    Desktop,
    Wallpaper,
    Widgets,
    Dock,
    Tray,
    Notifications,
    Windows,
    WindowRules,
    ScreenEdges,
    Workspaces,
    Keys,
    Search,
    Files,
    Clipboard,
    Displays,
    Session,
    Themes,
    Extensions,
    About,
}

impl Section {
    const ALL: [Self; 21] = [
        Self::General,
        Self::Bars,
        Self::Desktop,
        Self::Wallpaper,
        Self::Widgets,
        Self::Dock,
        Self::Tray,
        Self::Notifications,
        Self::Windows,
        Self::WindowRules,
        Self::ScreenEdges,
        Self::Workspaces,
        Self::Keys,
        Self::Search,
        Self::Files,
        Self::Clipboard,
        Self::Displays,
        Self::Session,
        Self::Themes,
        Self::Extensions,
        Self::About,
    ];

    /// Its English title, as `open_at` and the rows files name it.
    fn title(self) -> &'static str {
        let at = Self::ALL.iter().position(|known| *known == self);
        at.and_then(|at| rows::SECTIONS.get(at))
            .copied()
            .unwrap_or_default()
    }

    /// The section of the English title `title`, in any case.
    fn named(title: &str) -> Option<Self> {
        { Self::ALL.into_iter() }.find(|known| known.title().eq_ignore_ascii_case(title.trim()))
    }

    /// The character of Segoe Fluent Icons it is shown with.
    fn glyph(self) -> &'static str {
        match self {
            Self::General => "\u{e713}",
            Self::Bars => "\u{e700}",
            Self::Desktop => "\u{e7f4}",
            Self::Wallpaper => "\u{e8b9}",
            Self::Widgets => "\u{e71d}",
            Self::Dock => "\u{e7c4}",
            Self::Tray => "\u{e712}",
            Self::Notifications => "\u{ea8f}",
            Self::Windows => "\u{e8a7}",
            Self::WindowRules => "\u{e71c}",
            Self::ScreenEdges => "\u{e740}",
            Self::Workspaces => "\u{e8a9}",
            Self::Keys => "\u{e765}",
            Self::Search => "\u{e721}",
            Self::Files => "\u{e8b7}",
            Self::Clipboard => "\u{e77f}",
            Self::Displays => "\u{e7f8}",
            Self::Session => "\u{e77b}",
            Self::Themes => "\u{e790}",
            Self::Extensions => "\u{e719}",
            Self::About => "\u{e946}",
        }
    }

    /// The kinds the marketplace shows in this part, if it has one: what
    /// changes the looks under Themes, what adds to the shell under
    /// Extensions.
    fn market(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Themes => Some(&["themes", "icon-packs", "wallpapers"]),
            Self::Extensions => Some(&["extensions", "tiling-layouts"]),
            _ => None,
        }
    }
}

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
    /// Stands for the widget at this place of a bar, which is dragged.
    Grab(Place),
    Remove(Place),
    /// Says when a surface hides.
    SurfaceHide(usize, Hiding),
    /// Opens or closes the list of widgets to add to a slot or a card.
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
    /// Does this to a setting an extension or a theme declares.
    Declared(RowKey, Deed),
    /// Takes these declared settings out of the file: those of a card
    /// that the file sets.
    ResetAll(Vec<RowKey>),
    /// Runs the action of a link row, with its argument.
    Link(String, Option<String>),
    /// Opens the card of the options of the widget at this place, or
    /// closes it.
    Expand(WidgetPlace),
    /// Puts what is at this place of what can be stacked at the end of
    /// the stack: a theme over the others, or an icon pack after those in
    /// use.
    AddLayer(Stack, usize),
    /// Takes what is at this place of the stack off it.
    DropLayer(Stack, usize),
    /// Moves what is at this place of the stack up it, or down.
    ShiftLayer(Stack, usize, bool),
}

/// What the settings keep in an order of its own, changed by the same
/// buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Stack {
    /// The themes stacked over the base theme, `theme_layers`.
    Layers,
    /// The icon packs in use, the first one first, `icon_packs`.
    IconPacks,
}

/// What the card of a [`Stack`] says.
struct StackTexts {
    title: &'static str,
    about: &'static str,
    /// Said when the stack holds nothing.
    empty: &'static str,
    /// Said when it holds nothing and nothing can be put on it.
    nothing: &'static str,
    /// Over what can be put on it.
    add: &'static str,
}

impl Stack {
    /// What `config` has on the stack, in order.
    fn of(self, config: &Config) -> &[String] {
        match self {
            Self::Layers => &config.theme_layers,
            Self::IconPacks => &config.icon_packs,
        }
    }

    fn texts(self) -> StackTexts {
        match self {
            Self::Layers => StackTexts {
                title: "Layers",
                about: "Themes stacked over the theme, in order: each changes only what it sets, and the last one wins.",
                empty: "Nothing is stacked over the theme.",
                nothing: "Nothing is stacked over the theme.",
                add: "Add a layer",
            },
            Self::IconPacks => StackTexts {
                title: "Icon packs",
                about: "The icon packs in use, in order: the first that has an icon for a program or a kind of file gives it.",
                empty: "No icon pack is in use.",
                nothing: "No icon pack is installed; the marketplace offers them.",
                add: "Use an icon pack",
            },
        }
    }
}

/// The deeds of a key combination as they are shown, in the order of
/// [`DEEDS`], and then an action, which `DEEDS` has not. The last two are
/// not chosen here: they are texts written by hand.
const DEED_TITLES: [&str; 8] = [
    "Popup", "Tiling", "Focus", "Move", "Windows", "Dock", "Program", "Action",
];
const DEED_RUN: usize = 6;
const DEED_ACTION: usize = 7;
const SIDES: [&str; 4] = ["left", "right", "up", "down"];

/// Which of [`DEED_TITLES`] a key combination does, and with what. An
/// action is shown as its name and `arg`; a folder of the file manager as
/// the action `files` it is the same as.
fn deed_of(key: &KeyConfig) -> Option<(usize, String)> {
    let run = key.run.as_ref().map(|run| run.join(" "));
    let dock = key.dock.map(|number| number.to_string());
    let action = match (&key.action, &key.files) {
        (Some(name), _) => Some(name.clone()),
        (None, Some(_)) => Some("files".to_owned()),
        (None, None) => None,
    };
    let arg = key
        .arg
        .as_ref()
        .or(key.files.as_ref().filter(|_| key.action.is_none()));
    let action = action.map(|name| match arg {
        Some(arg) => format!("{name} {arg}"),
        None => name,
    });
    let deeds = [
        &key.popup,
        &key.tiling,
        &key.focus,
        &key.carry,
        &key.switch,
        &dock,
        &run,
        &action,
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
    /// A slot of the surface at this place.
    Slot(usize, Slot),
    /// The card at this place.
    Card(usize),
}

/// How a control looks.
enum Face {
    /// A small round button with a character of the icon font.
    Glyph(&'static str),
    /// A chip with a text, tinted while `selected`.
    Chip { text: SharedString, selected: bool },
    /// A box, ticked while `checked`, with a text.
    Check { text: SharedString, checked: bool },
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
    ("devices", "Devices"),
    ("files", "File manager"),
    ("focused", "Window title"),
    ("gpu", "Graphics"),
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
    ("startup", "Startup apps"),
    ("tiling", "Tiling"),
    ("trash", "Recycle Bin"),
    ("tray", "Tray"),
    ("update", "Updates"),
    ("user", "Account"),
    ("volume", "Volume"),
    ("wallpaper", "Wallpaper"),
    ("windows", "Windows"),
    ("winx", "Win+X menu"),
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

/// A widget of a bar on its way to another place.
struct Carried {
    /// Where it stood.
    from: Place,
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
        ScreenEdge::Left => "Left bar",
        ScreenEdge::Right => "Right bar",
    };
    let displays = match surface.displays {
        Displays::All => "every display",
        Displays::Primary => "the primary display",
        Displays::Others => "the other displays",
        Displays::Chosen(_) => "the chosen displays",
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
    names.retain(|name| name != WALLPAPER_THEME);
    names.sort();
    // The colors of Windows are a choice like a file.
    names.insert(0, ACCENT_THEME.to_owned());
    // So are those of the wallpaper.
    names.insert(1, WALLPAPER_THEME.to_owned());
    names
}

/// What the chip of the theme `name` says.
fn theme_title(name: &str, language: Language) -> SharedString {
    if name == WALLPAPER_THEME {
        return language.say("Wallpaper colors").into();
    }
    if name == ACCENT_THEME {
        language.say("Windows accent").into()
    } else {
        SharedString::new(name)
    }
}

/// Shows the configuration and changes its file.
pub struct Settings {
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
    /// The widget of a bar that is being dragged.
    lifted: Option<Place>,
    /// What has its list of widgets to add open, kept while it closes.
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
    /// The programs that start with the session, read when first shown.
    startup: Option<AnyView>,
    /// What extensions and themes declare can be set, and their controls.
    declared: DeclaredState,
    /// The placed widget whose options are shown in the Widgets section.
    expanded: Option<WidgetPlace>,
}

impl Settings {
    /// The English titles of the sections, in the order of the rail, as
    /// `open_at` takes them.
    #[allow(dead_code, reason = "for windows-selftest, which opens each section")]
    pub fn section_titles() -> &'static [&'static str] {
        rows::SECTIONS
    }

    /// Whether `section` has cards drawn by hand, besides those of the
    /// rows files.
    fn hand_drawn(&self, section: Section) -> bool {
        match section {
            Section::General
            | Section::Bars
            | Section::Desktop
            | Section::Widgets
            | Section::Dock
            | Section::Themes
            | Section::Extensions => true,
            Section::Keys => !self.available.iter().any(|name| name == SHORTCUTS),
            _ => false,
        }
    }

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
            layouts: layouts.collect(),
            markets: HashMap::new(),
            failure: None,
            available: Vec::new(),
            panels: HashSet::new(),
            running: Vec::new(),
            themes: Vec::new(),
            languages: Vec::new(),
            lifted: None,
            adding: None,
            add_list: Spring::new(motion.spatial_fast, 0.),
            popups: Vec::new(),
            picking: None,
            key_list: Spring::new(motion.spatial_fast, 0.),
            interactions: HashMap::new(),
            startup: None,
            declared: DeclaredState::new(),
            expanded: None,
        };
        match ConfigFile::open().and_then(|file| Ok((file.config()?, file.table()))) {
            Ok((config, table)) => (this.config, this.table) = (Some(config), table),
            Err(error) => this.failure = Some(error.into()),
        }
        this.look_around(cx);
        this.read_declared(cx);
        this.arrive(cx);
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
        self.change_row(None, cx, change);
    }

    /// The same, for the setting `row`: what kept the change from being
    /// made is said under it. The file as `change` leaves it is read as
    /// the shell reads it before anything is written, so that a value the
    /// shell cannot use never makes the whole file unusable.
    fn change_row(
        &mut self,
        row: Option<&RowKey>,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut ConfigFile) -> bool,
    ) {
        let changed = ConfigFile::change(|file| {
            if !change(file) {
                return Err(CHANGED.to_owned());
            }
            Ok((file.config()?, file.table()))
        });
        match changed {
            Ok((config, table)) => {
                self.config = Some(config);
                self.table = table;
                self.failure = None;
                if let Some(row) = row {
                    self.clear_refusal(row, cx);
                }
                // The controls now stand for other things.
                self.interactions.clear();
                cx.defer(Shell::config_changed);
            }
            Err(error) => {
                report!("{error}");
                match row {
                    Some(row) => self.refuse(row, &error, cx),
                    None => self.failure = Some(error.into()),
                }
            }
        }
        self.look_around(cx);
        cx.notify();
    }

    /// Shows `section`: its cards arrive, and its marketplace is made.
    fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.section == section {
            return;
        }
        self.section = section;
        // One handle scrolls the page of every section, which opens at
        // its top.
        self.declared.scroll.set_offset(gpui::Point::default());
        // The pages of a section are built each time it is shown, and
        // dropped once another is.
        self.declared.pages.clear();
        self.declared.page_item = None;
        self.arrival.jump_to(0.);
        self.arrival.animate_to(1.);
        if let Some(shown) = section.market()
            && !self.markets.contains_key(&section)
        {
            let mut options = toml::Table::new();
            let shown = shown.iter().map(|&kind| toml::Value::from(kind));
            options.insert(
                MARKETPLACE_SHOWS.to_owned(),
                toml::Value::Array(shown.collect()),
            );
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
                        this.read_declared(cx);
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
        if self.act_declared(control, cx) {
            return;
        }
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
                let code = (language != Language::default()).then(|| language.code().into());
                self.change(cx, |file| {
                    file.set_key("language", code);
                    true
                });
            }
            Control::Grab(_) => {}
            Control::Remove(place) => self.change(cx, |file| file.remove(place)),
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
                self.change(cx, |file| match holder {
                    Holder::Slot(surface, slot) => file.add(surface, slot, &widget),
                    Holder::Card(card) => file.add_to_card(card, &widget),
                });
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
            Control::Expand(ref place) => {
                let open = self.expanded.as_ref() == Some(place);
                self.expanded = (!open).then(|| place.clone());
                cx.notify();
            }
            // Done above.
            Control::Declared(..)
            | Control::ResetAll(_)
            | Control::Link(..)
            | Control::AddLayer(..)
            | Control::DropLayer(..)
            | Control::ShiftLayer(..) => {}
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
            Face::Check { text, checked } => {
                let (glyph, color) = if checked {
                    (ICON_CHECKED, theme.colors.primary)
                } else {
                    (ICON_UNCHECKED, theme.colors.on_surface_variant)
                };
                button
                    .h(CHIP_HEIGHT)
                    .px(theme.spacing.small)
                    .gap(theme.spacing.small)
                    .bg(state_layer(
                        &theme,
                        theme.colors.surface_container_highest,
                        state,
                    ))
                    .child(icon(&theme, glyph).text_color(color))
                    .child(div().text_color(theme.colors.on_surface).child(text))
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
            from: place,
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
                    this.lifted = Some(from);
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
    /// the plus that opens the list of widgets to add to it, and room to
    /// let a dragged widget go after the others.
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
        let holder = Holder::Slot(surface, slot);
        let open = if self.adding == Some(holder) {
            self.add_list.value(window).clamp(0., 1.)
        } else {
            0.
        };
        let mut pills = Vec::new();
        for (index, spec) in specs.iter().enumerate() {
            let place = Place {
                surface,
                slot,
                index,
            };
            pills.push(self.pill(place, spec, window, cx));
        }
        pills.push(self.control(Control::Adding(holder), Face::Glyph(ICON_MORE), window, cx));
        let last = Place {
            surface,
            slot,
            index: specs.len(),
        };
        let held = wrap(&theme).min_h(CHIP_HEIGHT).children(pills);
        let caption = div()
            .flex()
            .text_size(theme.typography.label_size * CAPTION_SHARE)
            .text_color(mix(
                theme.colors.on_surface_variant,
                theme.colors.primary,
                open,
            ))
            .child(language.say(slot.label()));
        let (held, caption) = match slot {
            Slot::Start => (held.justify_start(), caption.justify_start()),
            Slot::Center => (held.justify_center(), caption.justify_center()),
            Slot::End => (held.justify_end(), caption.justify_end()),
        };
        let (rest, lit) = (theme.colors.surface_container, theme.colors.primary);
        let over = mix(rest, lit, ZONE_TINT);
        div()
            .flex_1()
            .min_w(px(0.))
            .p(theme.spacing.small)
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .rounded(theme.shape.medium)
            .border_1()
            .border_color(mix(rest, lit, open))
            .bg(rest)
            .drag_over::<Carried>(move |style, _, _, _| style.bg(over).border_color(lit))
            .on_drop(cx.listener(move |this, carried: &Carried, _, cx| {
                this.land(carried, last, cx);
            }))
            .child(caption)
            .child(held)
            .into_any_element()
    }

    /// A bar as it is laid out: its three slots side by side, the list of
    /// widgets to add while one of them has it open, and when it hides.
    fn surface(
        &mut self,
        index: usize,
        surface: &SurfaceConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        // A drag that ended anywhere is over.
        if self.lifted.is_some() && !cx.has_active_drag() {
            self.lifted = None;
        }
        let mut zones = Vec::new();
        let mut offers = None;
        for slot in Slot::ALL {
            zones.push(self.zone(index, slot, held(surface, slot), window, cx));
            let list = self.offers(Holder::Slot(index, slot), window, cx);
            offers = offers.or(list);
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
            .children(offers)
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

    /// Moves the dragged widget to stand before what is at `to`, or last
    /// in that slot.
    fn land(&mut self, carried: &Carried, to: Place, cx: &mut Context<Self>) {
        let from = carried.from;
        self.change(cx, |file| file.transfer(from, to));
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
            // A bar is offered no widget made to fill a popup or a card.
            if matches!(holder, Holder::Slot(..)) && self.panels.contains(&name) {
                continue;
            }
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
        // It is as tall as what it holds, so that the wheel turns the page
        // and nothing inside it; it is cut only while it opens or closes.
        let list = div().opacity(add_list.clamp(0., 1.));
        let list = if add_list < 1. {
            list.max_h(OFFERS_HEIGHT * add_list).overflow_hidden()
        } else {
            list
        };
        let list = list
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
                // Others and chosen displays are neither of these.
                selected: config.displays
                    == if primary {
                        Displays::Primary
                    } else {
                        Displays::All
                    },
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
        for (at, key) in KEYS.iter().copied().enumerate() {
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
            // A program and an action are shown where the file has one and
            // not offered.
            if (at == DEED_RUN || at == DEED_ACTION) && kind != Some(at) {
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
            if kind == DEED_RUN || kind == DEED_ACTION {
                let by_hand = match kind {
                    DEED_RUN => "The program is written in the file by hand.",
                    _ => "The action is written in the file by hand.",
                };
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

    /// The rail: the title, the parts with a mark behind the one shown,
    /// and at its end the buttons that start Tessera again and close it.
    fn rail(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        // A section with nothing to draw is left out of the rail.
        let shown: Vec<Section> = { Section::ALL.into_iter() }
            .filter(|section| *section == self.section || self.section_drawn(*section, cx))
            .collect();
        let place = shown.iter().position(|section| *section == self.section);
        let place = place.unwrap_or_default() as f32;
        if self.mark.target() != place {
            self.mark.animate_to(place);
        }
        let mark = self.mark.value(window);
        let mut parts = Vec::new();
        for section in shown {
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
                    .id("rail")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
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
                // A note, a line of packs and one of those to use.
                shared.push((4, self.icon_packs_card(config, window, cx)));
                shared.push(self.startup(&theme, language, cx));
            }
            Section::Bars => {
                wide.push(self.arranging(window, cx));
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
            // The key cards stand until the Shortcuts page is there.
            Section::Keys if !self.available.iter().any(|name| name == SHORTCUTS) => {
                wide.push(self.keys(config, window, cx));
                for (index, key) in config.keys.iter().enumerate() {
                    wide.push(self.key_card(index, key, window, cx));
                }
            }
            Section::Widgets => {
                let first = usize::from(self.failure.is_some());
                wide.extend(self.placed_widgets(config, first, window, cx));
            }
            Section::Themes => {
                wide.push(self.themes(config.theme.as_deref(), window, cx));
                let first = usize::from(self.failure.is_some()) + wide.len();
                wide.extend(self.theme_cards(config, first, window, cx));
            }
            Section::Extensions => {
                let first = usize::from(self.failure.is_some()) + wide.len();
                wide.extend(self.extension_cards(first, window, cx));
            }
            _ => {}
        }
        let (pages, rows) = self.builtin_cards(config, window, cx);
        wide.extend(pages);
        shared.extend(rows);
        (wide, shared)
    }
}

impl Settings {
    /// The card of the programs that start with the session; the list
    /// is made, and read, when the card is first shown. It is taken to
    /// be as long as a dozen lines, as a list of ten programs is.
    fn startup(&mut self, theme: &Theme, language: Language, cx: &mut Context<Self>) -> Card {
        let list = { self.startup.clone() }.unwrap_or_else(|| super::startup::list(cx));
        self.startup = Some(list.clone());
        let card = card(theme, language.say("Startup apps")).child(list);
        (STARTUP_LINES, card.into_any_element())
    }
}

/// How many lines the card of the programs that start with the session
/// is taken to need.
const STARTUP_LINES: usize = 12;

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
            .track_scroll(&self.declared.scroll)
            .flex()
            .flex_col()
            .gap(theme.spacing.medium)
            .children(self.failure.clone().map(|failure| note(&theme, failure)));
        if let Some(config) = self.config.clone() {
            let (wide, shared) = self.cards(&config, window, cx);
            let count = wide.len() + shared.len() + 1;
            let before = wide.len();
            // A card asked for among those that share the width is
            // brought into sight with them.
            if std::mem::take(&mut self.declared.sought_shared) {
                let place = usize::from(self.failure.is_some()) + before;
                self.declared.scroll.scroll_to_item(place);
                self.declared.seek = None;
            }
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
    fn a_key_that_names_an_action_is_shown_as_doing_it() {
        let mut key = KeyConfig {
            action: Some("desktop".to_owned()),
            arg: Some("minimize".to_owned()),
            ..KeyConfig::default()
        };
        assert_eq!(
            deed_of(&key),
            Some((DEED_ACTION, "desktop minimize".to_owned()))
        );
        key.arg = None;
        assert_eq!(deed_of(&key), Some((DEED_ACTION, "desktop".to_owned())));
        let folder = KeyConfig {
            files: Some("~".to_owned()),
            ..KeyConfig::default()
        };
        assert_eq!(deed_of(&folder), Some((DEED_ACTION, "files ~".to_owned())));
        let popup = KeyConfig {
            popup: Some("launcher".to_owned()),
            ..KeyConfig::default()
        };
        assert_eq!(deed_of(&popup), Some((0, "launcher".to_owned())));
        assert_eq!(DEED_TITLES[DEED_ACTION], "Action");
    }

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
    fn the_sections_stand_in_the_order_of_their_titles() {
        let titles: Vec<&str> = Section::ALL.iter().map(|section| section.title()).collect();
        assert_eq!(titles, rows::SECTIONS);
        assert_eq!(Section::named("window rules"), Some(Section::WindowRules));
        assert_eq!(
            Section::named(" Displays and power "),
            Some(Section::Displays)
        );
        assert_eq!(Section::named("nowhere"), None);
        for section in Section::ALL {
            let russian = Language::Russian.say(section.title());
            assert_ne!(
                russian,
                section.title(),
                "`{}` is not translated",
                section.title()
            );
        }
    }

    #[test]
    fn paths_are_written_with_forward_slashes() {
        assert_eq!(plain(Path::new(r"C:\Apps\app.exe")), "C:/Apps/app.exe");
    }
}
