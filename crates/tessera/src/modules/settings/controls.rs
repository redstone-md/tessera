// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The settings that extensions and themes declare, each shown with the
//! control its kind asks for: a switch, a field, a stepper, a slider,
//! buttons or a list of choices, a color, a path with Browse, a font, or
//! rows of texts. What is changed is written to the configuration file,
//! a field once typing pauses, Enter is pressed or it is left, a slider
//! once it is let go, so that a drag is not dozens of rewrites.

use std::collections::HashMap;
use std::ops::Range;
use std::time::Duration;

use gpui::{
    AnyElement, AnyView, App, AppContext, AsyncApp, Context, Entity, EventEmitter, FocusHandle,
    Global, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement,
    PathPromptOptions, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    Task, UniformListScrollHandle, WeakEntity, Window, div, px, rgb, transparent_black,
    uniform_list,
};

use super::builtin::{BuiltinRows, RowTarget};
use super::dynamic::{self, Scanned};
use super::{
    CAPTION_SHARE, CHIP_HEIGHT, CHIP_TINT, Control, Face, ICON_LESS, ICON_MORE, ICON_REMOVE,
    LIFTED, Lifted, SMALL_BUTTON, Section, Settings, Stack, card, note, wrap,
};
use crate::actions::Actions;
use crate::config::Config;
use crate::config::edit::{ConfigFile, WidgetPlace};
use crate::extension::{InstalledExtension, installed_extensions};
use crate::field::TextField;
use crate::i18n::Language;
use crate::motion::{Spring, SpringSpec, mix};
use crate::options::{Kind, OptionDecl, OptionSet, PathPick, SEGMENTS, is_color, same, text_of};
use crate::platform::font_families;
use crate::popup::Popups;
use crate::shell::Shell;
use crate::theme::{
    ACCENT_THEME, BUILTIN_THEME, Theme, WALLPAPER_THEME, affected_groups, declared_settings,
};
use crate::ui::{Slider, icon, switch};

/// How long typing has to pause before what was typed is written.
const TYPING_PAUSE: Duration = Duration::from_millis(300);

/// How long after the settings give way to the dialog to choose a path
/// the dialog opens, so that it is not closed with them.
const BROWSE_AFTER: Duration = Duration::from_millis(350);

/// How a card that was asked for goes dark again: slowly, without
/// coming back.
const FADE: SpringSpec = SpringSpec {
    stiffness: 12.,
    damping_ratio: 1.,
};

/// Colors a color setting offers with a click.
const PALETTE: [u32; 8] = [
    0xd0bcff, 0x3d7bd9, 0x2fb3a8, 0x4fa37a, 0xf2b84b, 0xff7a2f, 0xe8404f, 0xd6457a,
];

/// How wide a field, a slider and the list of fonts are.
const FIELD_WIDTH: f32 = 220.;
const NUMBER_WIDTH: f32 = 96.;
const SLIDER_WIDTH: f32 = 200.;
const FONT_LIST_HEIGHT: f32 = 220.;
const FONT_ROW_HEIGHT: f32 = 32.;

const ICON_RESET: &str = "\u{e7a7}";
const ICON_UP: &str = "\u{e70e}";
const ICON_DOWN: &str = "\u{e70d}";
const ICON_OPEN: &str = "\u{e70d}";
const ICON_DRAG: &str = "\u{e76f}";
const ICON_FOLDER: &str = "\u{e8b7}";
const ICON_WARNING: &str = "\u{e7ba}";

/// How wide the field of a time is.
const TIME_WIDTH: f32 = 64.;

/// The widget the settings are, which a popup holds.
const WIDGET: &str = "settings";

/// Whose a declared setting is, and so where its value is written.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Owner {
    /// Under `[extension.<id>]`.
    Extension(String),
    /// Under `[theme_options.<name>]`.
    Theme(String),
    /// A key of Tessera's own, as a rows file declares it.
    Builtin(RowTarget),
    /// An option of the one widget at this place.
    Widget(WidgetPlace),
}

impl Owner {
    /// What the file holds for an extension or a theme.
    fn values<'a>(&self, config: &'a Config) -> Option<&'a toml::Table> {
        match self {
            Self::Extension(id) => config.extensions.get(id),
            Self::Theme(name) => config.theme_options.get(name),
            Self::Builtin(_) | Self::Widget(_) => None,
        }
    }

    /// What [`Settings::open_at`] names it by: an extension by its
    /// identifier, a theme by its name, a key of Tessera's own by its path
    /// and a placed widget by its place.
    pub(super) fn item(&self) -> String {
        match self {
            Self::Extension(id) | Self::Theme(id) => id.clone(),
            Self::Builtin(target) => target.item(),
            Self::Widget(place) => place.item(),
        }
    }

    /// Whether it is a setting of Tessera's own, whose key leaves the file
    /// when it is set to its default.
    fn is_tessera(&self) -> bool {
        matches!(self, Self::Builtin(_) | Self::Widget(_))
    }

    /// Writes `value` at `key`, as the file holds the values of this
    /// owner: a list of an extension as the text its kit reads.
    fn write(&self, file: &mut ConfigFile, key: &str, value: Option<&toml::Value>) -> bool {
        match self {
            Self::Extension(id) => {
                let value = value.map(|value| match value {
                    toml::Value::Array(_) => toml_edit::Value::from(text_of(value)),
                    other => edit_value(other),
                });
                file.set_extension_setting(id, key, value)
            }
            Self::Theme(name) => file.set_theme_option(name, key, value.map(edit_value)),
            Self::Builtin(target) => target.write(file, value.map(edit_value)),
            Self::Widget(place) => file.set_widget_option(place, key, value.map(edit_value)),
        }
    }
}

/// A value of the file as the writer of the file takes it.
pub(super) fn edit_value(value: &toml::Value) -> toml_edit::Value {
    match value {
        toml::Value::String(text) => text.as_str().into(),
        toml::Value::Integer(number) => (*number).into(),
        toml::Value::Float(number) => (*number).into(),
        toml::Value::Boolean(flag) => (*flag).into(),
        toml::Value::Array(items) => {
            let items: toml_edit::Array = items.iter().map(edit_value).collect();
            toml_edit::Value::Array(items)
        }
        toml::Value::Table(entries) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, value) in entries {
                table.insert(key, edit_value(value));
            }
            table.fmt();
            toml_edit::Value::InlineTable(table)
        }
        other => { other.to_string().parse() }.unwrap_or_else(|_| other.to_string().into()),
    }
}

/// A declared setting: whose it is, and its key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct RowKey {
    pub(super) owner: Owner,
    pub(super) key: String,
}

impl RowKey {
    pub(super) fn new(owner: Owner, key: &str) -> Self {
        Self {
            owner,
            key: key.to_owned(),
        }
    }
}

/// The rows of the settings `set` declares for `owner`, in order.
pub(super) fn owned_rows(owner: &Owner, set: &OptionSet) -> Vec<(RowKey, OptionDecl)> {
    { set.decls.iter() }
        .map(|decl| (RowKey::new(owner.clone(), &decl.key), decl.clone()))
        .collect()
}

/// Whether a number counts from no value, which its `unset_label` names,
/// a step under its least: it shows that while the file holds none.
fn counts_from_unset(decl: &OptionDecl) -> bool {
    decl.optional && decl.unset_label.is_some()
}

/// What a card of declared settings is made of.
pub(super) struct CardParts<'a> {
    /// What [`Settings::open_at`] names the card by, to light it.
    pub item: String,
    pub title: SharedString,
    /// Its settings, each with whose it is, in order.
    pub rows: Vec<(RowKey, OptionDecl)>,
    /// The declarations left out, each said as a sentence.
    pub problems: &'a [String],
    /// Said under the settings, such as that there is nothing to set.
    pub remark: Option<&'static str>,
    /// Drawn under the remark.
    pub footer: Vec<AnyElement>,
    /// What each setting affects, as it is said under it.
    pub affects: &'a dyn Fn(&OptionDecl) -> Vec<String>,
}

/// Pads a time typed with a one-digit hour, `7:30`, to the `HH:MM` the
/// file holds; anything else is given back as it is.
pub(super) fn padded_time(text: &str) -> String {
    let text = text.trim();
    match text.split_once(':') {
        Some((hours, minutes))
            if hours.len() == 1 && hours.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            format!("0{hours}:{minutes}")
        }
        _ => text.to_owned(),
    }
}

/// The time `text` holds, `HH:MM`, moved by an hour, or to the next or
/// the last full [`TIME_STEP`] minutes, around the clock; midnight where
/// it holds none.
pub(super) fn stepped_time(text: Option<&str>, hours: bool, up: bool) -> String {
    let minutes_of = |text: &str| -> Option<i32> {
        let (hours, minutes) = text.split_once(':')?;
        Some(hours.parse::<i32>().ok()? * 60 + minutes.parse::<i32>().ok()?)
    };
    let now = text.and_then(minutes_of).unwrap_or(0);
    let step = TIME_STEP;
    let moved = match (hours, up) {
        (true, true) => now + 60,
        (true, false) => now - 60,
        (false, true) => (now / step + 1) * step,
        (false, false) => (now + step - 1) / step * step - step,
    };
    let moved = moved.rem_euclid(24 * 60);
    format!("{:02}:{:02}", moved / 60, moved % 60)
}

/// How many minutes the steppers of a time move it.
const TIME_STEP: i32 = 5;

/// The items `value` holds in order of `choices` with the one at `index`
/// added or taken out, as a multi-choice writes them.
pub(super) fn toggled_items(
    choices: &[crate::options::Choice],
    value: Option<&toml::Value>,
    index: usize,
) -> Vec<String> {
    let held: Vec<String> = match value {
        Some(value) => crate::options::parse_list(&text_of(value)),
        None => Vec::new(),
    };
    let values: Vec<String> = { choices.iter() }
        .filter(|choice| !choice.is_unset())
        .map(|choice| text_of(&choice.value))
        .collect();
    let toggled = values.get(index);
    { values.iter() }
        .filter(|one| held.contains(one) != (Some(*one) == toggled))
        .cloned()
        .collect()
}

/// What a button of a declared setting does.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Deed {
    /// Switches it.
    Toggle,
    /// Gives it the answer at this place: of its choices, or of unset, on
    /// and off for a switch that can be unset.
    Choose(usize),
    /// Takes it out of the file.
    Unset,
    /// Opens or closes its list of choices or fonts.
    Open,
    /// Takes its number a step up, or down.
    Step(bool),
    /// Gives it the color at this place of [`PALETTE`].
    Swatch(usize),
    /// Asks for a file or a folder.
    Browse,
    /// Puts the choice at this place in the list, or takes it out.
    Check(usize),
    /// Moves a time an hour, or some minutes, up or down.
    StepTime { hours: bool, up: bool },
}

/// What extensions and the themes in use declare, as read away from the
/// thread that draws.
#[derive(Default)]
struct Declared {
    /// `None` until they have been read.
    extensions: Option<Vec<InstalledExtension>>,
    /// The themes in use, the base first.
    themes: Vec<(String, ThemeDeclared)>,
    /// The themes the last reading was for.
    read_for: Vec<String>,
    /// The installed icon packs, by identifier.
    icon_packs: Vec<String>,
    reading: Option<Task<()>>,
}

/// What a theme declares, and what is wrong with the settings left out;
/// `None` for a theme that is not there.
pub(super) type ThemeDeclared = Option<(OptionSet, Vec<String>)>;

/// A control of a declared setting that keeps something between frames.
enum Held {
    Switch(Spring),
    Field(Entity<DeclaredField>),
    Slider(Entity<DeclaredSlider>),
    List(Entity<DeclaredList>),
    Fonts(Entity<FontPicker>),
    Sides(Entity<DeclaredSides>),
}

/// What the settings keep of the declared settings.
pub(super) struct DeclaredState {
    read: Declared,
    /// The controls that keep something, each with the declaration it was
    /// made for.
    held: HashMap<RowKey, (OptionDecl, Held)>,
    /// The setting whose list of choices or fonts is open.
    opened: Option<RowKey>,
    /// The card that was asked for, lit while the light fades.
    lit: Option<(String, Spring)>,
    /// The card to bring into sight once it is drawn.
    pub(super) seek: Option<String>,
    /// Whether that card stands among the cards that share the width,
    /// which are brought into sight together.
    pub(super) sought_shared: bool,
    /// The page of cards, scrolled to that card.
    pub(super) scroll: ScrollHandle,
    /// The rows of Tessera's own settings, as the rows files declare them.
    pub(super) builtin: BuiltinRows,
    /// The options each widget declares, by widget name, as last read.
    widget_decls: HashMap<String, Vec<OptionDecl>>,
    /// The setting whose last change was not made, and why, said under it.
    row_error: Option<(RowKey, SharedString)>,
    /// The pages of the section shown, by widget name, built when the
    /// section is shown.
    pub(super) pages: HashMap<String, AnyView>,
    /// What the pages of the section shown are given as their `item`.
    pub(super) page_item: Option<String>,
}

impl DeclaredState {
    pub(super) fn new() -> Self {
        Self {
            read: Declared::default(),
            held: HashMap::new(),
            opened: None,
            lit: None,
            seek: None,
            sought_shared: false,
            scroll: ScrollHandle::new(),
            builtin: BuiltinRows::default(),
            widget_decls: HashMap::new(),
            row_error: None,
            pages: HashMap::new(),
            page_item: None,
        }
    }

    /// The installed extensions, as last read.
    pub(super) fn extensions(&self) -> &[InstalledExtension] {
        self.read.extensions.as_deref().unwrap_or_default()
    }

    /// The themes in use and what they declare, as last read.
    pub(super) fn themes(&self) -> &[(String, ThemeDeclared)] {
        &self.read.themes
    }

    /// Whether `item` was asked for and is lit.
    pub(super) fn is_lit(&self, item: &str) -> bool {
        self.lit.as_ref().is_some_and(|(lit, _)| lit == item)
    }
}

/// Where [`Settings::open_at`] asked the settings to go, for those made
/// next; and the settings shown, which go there at once.
#[derive(Default)]
struct Destination {
    asked: Option<(Section, Option<String>)>,
    shown: Option<WeakEntity<Settings>>,
}

impl Global for Destination {}

/// What a control wants written: a value, or `None` to take the key out.
/// It is checked against the declaration before it is written. An error
/// says why what the control holds cannot be written.
pub(super) struct Commit(Result<Option<toml::Value>, String>);

/// The shortcuts `extension` declares, each with what it does, the keys it
/// has unless others are bound and the action to bind others to; `None`
/// where it declares none.
fn shortcuts_of(
    extension: &InstalledExtension,
    theme: &Theme,
    language: Language,
) -> Option<AnyElement> {
    if extension.shortcuts.is_empty() {
        return None;
    }
    let mut list = div()
        .flex()
        .flex_col()
        .gap(theme.spacing.tiny)
        .child(caption(theme, language.say("Shortcuts")));
    for shortcut in &extension.shortcuts {
        let keys = shortcut.keys.as_deref().unwrap_or(language.say("no keys"));
        let line = format!("{}: {keys} \u{b7} {}", shortcut.label, shortcut.action);
        list = list.child(note(theme, line));
    }
    let bind = "Other keys are bound to these actions with a [[key]] in config.toml.";
    Some(
        list.child(note(theme, language.say(bind)))
            .into_any_element(),
    )
}

/// The names of the themes in use: the base, or the built-in one, and
/// those stacked over it.
fn themes_in_use(config: &Config) -> Vec<String> {
    let base = config
        .theme
        .clone()
        .unwrap_or_else(|| BUILTIN_THEME.to_owned());
    std::iter::once(base)
        .chain(config.theme_layers.iter().cloned())
        .collect()
}

/// Writes `value` at the declared setting `row` straight to the file, for
/// a change made while the settings are not shown. What the shell could
/// not read is not written.
fn write_now(row: &RowKey, value: Option<&toml::Value>, cx: &mut App) {
    let written = ConfigFile::change(|file| {
        if !row.owner.write(file, &row.key, value) {
            return Err(CHANGED.to_owned());
        }
        file.config().map(drop)
    });
    match written {
        Ok(()) => Shell::config_changed(cx),
        Err(error) => report!("{error}"),
    }
}

/// Why a change that no longer fits the file is not made.
pub(super) const CHANGED: &str = "the configuration has changed in the meantime";

/// What the `arg` of a link row stands for when it is run: `{config}` is
/// the path of the configuration file.
fn link_argument(arg: &str) -> String {
    match arg {
        CONFIG_ARG => ConfigFile::location()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        other => other.to_owned(),
    }
}

/// The `arg` of a link row that stands for the configuration file.
const CONFIG_ARG: &str = "{config}";

impl Settings {
    /// Opens the settings at `section`, named by its English title in any
    /// case, spaces included, such as `window rules`, and brings the card
    /// of `item` into sight, lit for a moment: an extension by its
    /// identifier, a theme by its name, a card of Tessera's own by its
    /// English title, a key of Tessera's own by its path (`tiling.gap`), a
    /// placed widget in Widgets by `surface/<n>/<start|center|end>/<i>`,
    /// `popup/<name>/<i>` or `card/<i>`. The pages of the section are given
    /// `item` as their `item` option. Settings that are shown go there;
    /// otherwise the popup that holds them opens.
    pub fn open_at(section: &str, item: Option<&str>, cx: &mut App) {
        let found = Section::ALL
            .into_iter()
            .find(|known| known.title().eq_ignore_ascii_case(section.trim()));
        let Some(section) = found else {
            report!("the settings have no part `{section}`");
            return;
        };
        let asked = (section, item.map(str::to_owned));
        let shown = { cx.try_global::<Destination>() }
            .and_then(|destination| destination.shown.as_ref()?.upgrade());
        match shown {
            Some(settings) if Popups::is_open(cx) => {
                settings.update(cx, |this, cx| this.go_to(asked, cx));
            }
            _ => {
                cx.set_global(Destination {
                    asked: Some(asked),
                    shown: None,
                });
                let popup = Popups::holding(WIDGET, cx).unwrap_or_else(|| WIDGET.to_owned());
                Shell::toggle_popup(&popup, cx);
            }
        }
    }

    /// Takes over where [`Self::open_at`] asked the settings to go, and
    /// becomes the settings shown.
    pub(super) fn arrive(&mut self, cx: &mut Context<Self>) {
        let asked =
            { cx.try_global::<Destination>() }.and_then(|destination| destination.asked.clone());
        let shown = Some(cx.weak_entity());
        cx.set_global(Destination { asked: None, shown });
        if let Some(asked) = asked {
            self.go_to(asked, cx);
        }
    }

    /// Shows `section`, and the card of `item` lit; a placed widget of the
    /// Widgets section opened, and the pages of the section given `item`.
    pub(super) fn go_to(
        &mut self,
        (section, item): (Section, Option<String>),
        cx: &mut Context<Self>,
    ) {
        self.show(section, cx);
        if self.declared.page_item != item {
            // The pages are built again for what they are to show.
            self.declared.page_item.clone_from(&item);
            self.declared.pages.clear();
        }
        if let Some(item) = item {
            if section == Section::Widgets
                && let Some(place) = WidgetPlace::parse(&item)
            {
                self.expanded = Some(place);
            }
            let mut light = Spring::new(FADE, 1.);
            light.animate_to(0.);
            self.declared.lit = Some((item.clone(), light));
            self.declared.seek = Some(item);
        }
        cx.notify();
    }

    /// Reads again, away from the thread that draws, what the extensions
    /// and the themes in use declare, and the lists the rows of Tessera's
    /// own settings choose from.
    pub(super) fn read_declared(&mut self, cx: &mut Context<Self>) {
        let names = self.config.as_ref().map(themes_in_use).unwrap_or_default();
        self.declared.read.read_for = names.clone();
        let reading = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx.background_executor().spawn(async move {
                let themes = { names.into_iter() }
                    .map(|name| {
                        let declared = declared_settings(&name);
                        (name, declared)
                    })
                    .collect();
                (installed_extensions(), themes, Scanned::read())
            });
            let (extensions, themes, scanned) = read.await;
            this.update(cx, |this, cx| {
                this.declared.read.extensions = Some(extensions);
                this.declared.read.themes = themes;
                this.declared.read.icon_packs = scanned.icon_packs.clone();
                cx.set_global(scanned);
                this.declared.builtin = BuiltinRows::read(cx);
                this.declared.widget_decls.clear();
                cx.notify();
            })
            .ok();
        });
        self.declared.read.reading = Some(reading);
    }

    /// The options the widget `name` declares, with the choices only the
    /// running shell knows, read once and kept.
    pub(super) fn widget_declarations(&mut self, name: &str, cx: &App) -> Vec<OptionDecl> {
        let made = self.declared.widget_decls.entry(name.to_owned());
        made.or_insert_with(|| {
            let decls = crate::widget_options::of(name);
            { decls.into_iter() }
                .map(|mut decl| {
                    if let Some(choices) = crate::widget_options::dynamic(name, &decl.key, cx) {
                        dynamic::merge(&mut decl, choices);
                    }
                    decl
                })
                .collect()
        })
        .clone()
    }

    /// The declaration of the setting `row`, as last read.
    fn declaration(&self, row: &RowKey) -> Option<OptionDecl> {
        let read = &self.declared.read;
        let set = match &row.owner {
            Owner::Extension(id) => {
                let extensions = read.extensions.as_ref()?;
                &extensions
                    .iter()
                    .find(|extension| extension.id == *id)?
                    .settings
            }
            Owner::Theme(name) => {
                let (_, declared) = read.themes.iter().find(|(known, _)| known == name)?;
                &declared.as_ref()?.0
            }
            Owner::Builtin(target) => return self.declared.builtin.declaration(target).cloned(),
            Owner::Widget(place) => {
                let spec = place.spec(self.config.as_ref()?)?;
                let decls = self.declared.widget_decls.get(&spec.widget)?;
                return decls.iter().find(|decl| decl.key == row.key).cloned();
            }
        };
        set.get(&row.key).cloned()
    }

    /// What the file holds at `row`, as written.
    pub(super) fn written(&self, row: &RowKey) -> Option<toml::Value> {
        let config = self.config.as_ref()?;
        match &row.owner {
            Owner::Extension(_) | Owner::Theme(_) => {
                row.owner.values(config)?.get(&row.key).cloned()
            }
            Owner::Builtin(target) => target.written(self.table.as_ref(), config),
            Owner::Widget(place) => place.spec(config)?.options.get(&row.key).cloned(),
        }
    }

    /// Checks `value` against the declaration of `row` and writes it; one
    /// that does not fit is said under the row and not written. A setting
    /// of Tessera's own set to its default leaves the file.
    fn write_declared(&mut self, row: &RowKey, value: Option<toml::Value>, cx: &mut Context<Self>) {
        let Some(decl) = self.declaration(row) else {
            return;
        };
        let value = match value {
            Some(toml::Value::String(text)) if decl.kind == Kind::Time => {
                Some(padded_time(&text).into())
            }
            other => other,
        };
        let mut value = match value.map(|value| decl.validate(&value)) {
            Some(Ok(value)) => Some(value),
            Some(Err(error)) => {
                self.refuse(row, &error, cx);
                return;
            }
            None => None,
        };
        if row.owner.is_tessera() {
            let default = decl.default.as_ref();
            let is_default =
                { value.as_ref().zip(default) }.is_some_and(|(one, other)| same(one, other));
            if !decl.optional && is_default {
                value = None;
            }
            // Names, each with a value, are a table of the file.
            if decl.kind == Kind::Pairs
                && let Some(toml::Value::String(text)) = &value
            {
                let pairs = crate::options::parse_pairs(text).unwrap_or_default();
                let table = { pairs.into_iter() }.map(|(name, value)| (name, value.into()));
                value = Some(toml::Value::Table(table.collect()));
            }
        }
        if value.as_ref() == self.written(row).as_ref() {
            self.clear_refusal(row, cx);
            return;
        }
        let written = row.clone();
        self.change_row(Some(row), cx, move |file| {
            written.owner.write(file, &written.key, value.as_ref())
        });
    }

    /// Says under `row` that what its control holds is not written, and
    /// why.
    pub(super) fn refuse(&mut self, row: &RowKey, error: &str, cx: &mut Context<Self>) {
        self.declared.row_error = Some((row.clone(), SharedString::from(error.to_owned())));
        cx.notify();
    }

    /// Takes away what was said under `row`.
    pub(super) fn clear_refusal(&mut self, row: &RowKey, cx: &mut Context<Self>) {
        if self
            .declared
            .row_error
            .as_ref()
            .is_some_and(|(known, _)| known == row)
        {
            self.declared.row_error = None;
            cx.notify();
        }
    }

    /// Does what the controls of declared settings and of the layers of
    /// themes do; whether `control` is one of them.
    pub(super) fn act_declared(&mut self, control: &Control, cx: &mut Context<Self>) -> bool {
        match control {
            Control::Declared(row, deed) => self.deed(row, deed, cx),
            Control::ResetAll(rows) => {
                let rows = rows.clone();
                self.change(cx, move |file| {
                    rows.iter().all(|row| row.owner.write(file, &row.key, None))
                });
            }
            Control::AddLayer(stack, index) => {
                let Some(name) = self.stackable(*stack).get(*index).cloned() else {
                    return true;
                };
                let mut stacked = self.stacked(*stack);
                stacked.push(name);
                self.restack(*stack, stacked, cx);
            }
            Control::DropLayer(stack, index) => {
                let mut stacked = self.stacked(*stack);
                if *index < stacked.len() {
                    stacked.remove(*index);
                    self.restack(*stack, stacked, cx);
                }
            }
            Control::ShiftLayer(stack, index, up) => {
                let mut stacked = self.stacked(*stack);
                let other = if *up {
                    index.checked_sub(1)
                } else {
                    Some(index + 1)
                };
                if let Some(other) = other.filter(|other| *other < stacked.len()) {
                    stacked.swap(*index, other);
                    self.restack(*stack, stacked, cx);
                }
            }
            Control::Link(action, arg) => {
                let (action, arg) = (action.clone(), arg.clone());
                // The action may open a popup in place of the settings,
                // which cannot be done while they handle the click.
                cx.defer(move |cx| {
                    let arg = arg.map(|arg| link_argument(&arg));
                    Actions::run(&action, arg.as_deref(), cx);
                });
            }
            _ => return false,
        }
        true
    }

    /// Brings the card at `place` of the page into sight if it is the card
    /// of `item` that was asked for.
    pub(super) fn seek(&mut self, item: &str, place: usize) {
        if self.declared.seek.as_deref() == Some(item) {
            self.declared.scroll.scroll_to_item(place);
            self.declared.seek = None;
        }
    }

    /// What `stack` holds, in its order, as the file says.
    fn stacked(&self, stack: Stack) -> Vec<String> {
        let config = self.config.as_ref();
        config
            .map(|config| stack.of(config).to_vec())
            .unwrap_or_default()
    }

    /// What can be put on `stack`: the theme files, or the installed icon
    /// packs.
    fn stackable(&self, stack: Stack) -> &[String] {
        match stack {
            Stack::Layers => &self.themes,
            Stack::IconPacks => &self.declared.read.icon_packs,
        }
    }

    /// Writes `stacked` as what `stack` holds.
    fn restack(&mut self, stack: Stack, stacked: Vec<String>, cx: &mut Context<Self>) {
        self.change(cx, move |file| {
            match stack {
                Stack::Layers => file.set_theme_layers(&stacked),
                Stack::IconPacks => file.set_icon_packs(&stacked),
            }
            true
        });
    }

    fn deed(&mut self, row: &RowKey, deed: &Deed, cx: &mut Context<Self>) {
        let Some(decl) = self.declaration(row) else {
            return;
        };
        let written = self.written(row);
        let shown = { written.clone() }.or_else(|| decl.default.clone());
        match deed {
            Deed::Toggle => {
                let on = shown.and_then(|value| value.as_bool()).unwrap_or(false);
                // A switch that may be unset and has no default is off
                // without the key, and leaves the file when it is switched
                // off.
                let value = match (on, decl.optional && decl.default.is_none()) {
                    (true, true) => None,
                    _ => Some((!on).into()),
                };
                self.write_declared(row, value, cx);
            }
            Deed::Choose(index) => {
                let value = match decl.kind {
                    Kind::Toggle => [None, Some(true.into()), Some(false.into())]
                        .get(*index)
                        .cloned()
                        .flatten(),
                    _ => { decl.choices.get(*index) }
                        .filter(|choice| !choice.is_unset())
                        .map(|choice| choice.value.clone()),
                };
                if decl.kind == Kind::Choice {
                    self.declared.opened = None;
                }
                self.write_declared(row, value, cx);
            }
            Deed::Unset => {
                self.declared.opened = None;
                self.write_declared(row, None, cx);
            }
            Deed::Open => {
                let open = self.declared.opened.as_ref() == Some(row);
                self.declared.opened = (!open).then(|| row.clone());
                cx.notify();
            }
            Deed::Step(up) => {
                let now = match counts_from_unset(&decl) {
                    true => written.as_ref().and_then(OptionDecl::number),
                    false => shown.as_ref().and_then(OptionDecl::number),
                };
                let next = decl
                    .stepped(now, *up)
                    .map(|number| decl.number_value(number));
                self.write_declared(row, next, cx);
            }
            Deed::Swatch(index) => {
                if let Some(color) = PALETTE.get(*index) {
                    self.write_declared(row, Some(format!("#{color:06x}").into()), cx);
                }
            }
            Deed::Browse => self.browse(row, &decl, cx),
            Deed::Check(index) => {
                let items = toggled_items(&decl.choices, shown.as_ref(), *index);
                let empty = items.is_empty();
                let items = toml::Value::Array(items.into_iter().map(Into::into).collect());
                let value = (!empty || !decl.optional).then_some(items);
                self.write_declared(row, value, cx);
            }
            Deed::StepTime { hours, up } => {
                let now = shown.as_ref().and_then(toml::Value::as_str);
                let next = stepped_time(now, *hours, *up);
                self.write_declared(row, Some(next.into()), cx);
            }
        }
    }

    /// Asks for a file or a folder for `row` in the dialog of Windows. The
    /// settings give way to it, and open again where they were once a
    /// path is chosen or the dialog is left.
    fn browse(&mut self, row: &RowKey, decl: &OptionDecl, cx: &mut Context<Self>) {
        let folder = decl.pick == Some(PathPick::Folder);
        let prompt: SharedString = decl.label.text(Language::of(cx)).into();
        let (row, decl, section) = (row.clone(), decl.clone(), self.section);
        cx.defer(Popups::close);
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            cx.background_executor().timer(BROWSE_AFTER).await;
            let options = PathPromptOptions {
                files: !folder,
                directories: folder,
                multiple: false,
                prompt: Some(prompt),
            };
            let Ok(chosen) = cx.update(|cx| cx.prompt_for_paths(options)) else {
                return;
            };
            let path = match chosen.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(error)) => {
                    report!("no path can be chosen: {error:#}");
                    None
                }
                _ => None,
            };
            cx.update(|cx| {
                let path = path.map(|path| path.to_string_lossy().replace('\\', "/"));
                match path.map(|path| decl.validate(&path.into())) {
                    Some(Ok(value)) => write_now(&row, Some(&value), cx),
                    Some(Err(error)) => report!("{}: {error}", decl.label.english()),
                    None => {}
                }
                Self::open_at(section.title(), Some(&row.owner.item()), cx);
            })
            .ok();
        })
        .detach();
    }

    /// The control of `row` that keeps something of its own, made where
    /// it is not there yet or was made for another declaration, as after
    /// a theme or an extension was changed; it tells the settings what it
    /// wants written.
    fn held(&mut self, row: &RowKey, decl: &OptionDecl, cx: &mut Context<Self>) -> &mut Held {
        let made = self.declared.held.get(row).map(|(made, _)| made);
        if made != Some(decl) {
            let language = Language::of(cx);
            let theme = *cx.global::<Theme>();
            let placeholder = { decl.placeholder.as_ref().or(decl.unset_label.as_ref()) }
                .map(|label| label.text(language))
                .unwrap_or_default();
            let held = match decl.kind {
                Kind::Toggle => Held::Switch(Spring::new(theme.motion.spatial_fast, 0.)),
                Kind::Slider => {
                    let decl = decl.clone();
                    let slider = cx.new(|_| DeclaredSlider::new(decl, &theme));
                    self.listen(row, &slider, cx);
                    Held::Slider(slider)
                }
                Kind::List | Kind::Pairs => {
                    let pairs = decl.kind == Kind::Pairs;
                    let bounds = (
                        decl.min.map_or(0, |least| least as usize),
                        decl.max.map(|most| most as usize),
                    );
                    let list = cx.new(|cx| DeclaredList::new(pairs, bounds, cx));
                    self.listen(row, &list, cx);
                    Held::List(list)
                }
                Kind::Font => {
                    let fonts = cx.new(FontPicker::new);
                    self.listen(row, &fonts, cx);
                    Held::Fonts(fonts)
                }
                Kind::Sides => {
                    let decl = decl.clone();
                    let sides = cx.new(|cx| DeclaredSides::new(decl, cx));
                    self.listen(row, &sides, cx);
                    Held::Sides(sides)
                }
                _ => {
                    // A time is written once it is whole: when Enter is
                    // pressed or the field is left, not while typing pauses.
                    let on_pause = decl.kind != Kind::Time;
                    let field = cx.new(|cx| DeclaredField::new(placeholder.into(), on_pause, cx));
                    self.listen(row, &field, cx);
                    Held::Field(field)
                }
            };
            self.declared.held.insert(row.clone(), (decl.clone(), held));
        }
        let (_, held) = self.declared.held.get_mut(row).expect("it was just made");
        held
    }

    /// Writes what `control` asks for at `row`.
    fn listen<C: EventEmitter<Commit>>(
        &mut self,
        row: &RowKey,
        control: &Entity<C>,
        cx: &mut Context<Self>,
    ) {
        let row = row.clone();
        let commit =
            move |this: &mut Self, _: Entity<C>, commit: &Commit, cx: &mut Context<Self>| {
                match &commit.0 {
                    Ok(value) => this.write_declared(&row, value.clone(), cx),
                    Err(error) => this.refuse(&row, error, cx),
                }
            };
        cx.subscribe(control, commit).detach();
    }

    /// The cards of the Extensions part: one for each installed extension
    /// with what it declares.
    /// `first` is the place of the first of them among the cards of the
    /// page, for the one asked for to be brought into sight.
    pub(super) fn extension_cards(
        &mut self,
        first: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let Some(extensions) = self.declared.read.extensions.take() else {
            return Vec::new();
        };
        let mut cards = Vec::new();
        for extension in &extensions {
            let owner = Owner::Extension(extension.id.clone());
            let affects = |decl: &OptionDecl| {
                let names = extension.affected(&decl.key);
                let names = names
                    .into_iter()
                    .map(|name| format!("{}.{name}", extension.id));
                names.collect::<Vec<_>>()
            };
            self.seek(&extension.id, first + cards.len());
            // A changed setting reaches the running extension at once.
            let remark = extension.settings.is_empty().then_some(
                "This extension declares no settings; what it reads is written in config.toml.",
            );
            let parts = CardParts {
                item: extension.id.clone(),
                title: extension.id.clone().into(),
                rows: owned_rows(&owner, &extension.settings),
                problems: &[],
                remark,
                footer: shortcuts_of(extension, &theme, language)
                    .into_iter()
                    .collect(),
                affects: &affects,
            };
            cards.push(self.declared_card(parts, window, cx));
        }
        if extensions.is_empty() {
            let none = card(&theme, language.say("Installed extensions"))
                .child(note(&theme, language.say("No extension is installed.")));
            cards.push(none.into_any_element());
        }
        self.declared.read.extensions = Some(extensions);
        cards
    }

    /// The cards of the Themes part after the choice of the base theme:
    /// the layers over it, and one card for each theme in use with what it
    /// declares.
    /// `first` is the place of the first of them among the cards of the
    /// page, for the one asked for to be brought into sight.
    pub(super) fn theme_cards(
        &mut self,
        config: &Config,
        first: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if themes_in_use(config) != self.declared.read.read_for {
            self.read_declared(cx);
        }
        let mut cards = vec![self.layers_card(config, window, cx)];
        let themes = std::mem::take(&mut self.declared.read.themes);
        let language = Language::of(cx);
        for (name, declared) in &themes {
            let Some((set, problems)) = declared else {
                continue;
            };
            self.seek(name, first + cards.len());
            let title = match name.as_str() {
                BUILTIN_THEME => language.say("Built-in").to_owned(),
                ACCENT_THEME => language.say("Windows accent").to_owned(),
                WALLPAPER_THEME => language.say("Wallpaper colors").to_owned(),
                other => other.to_owned(),
            };
            let affects = |decl: &OptionDecl| {
                let groups = affected_groups(decl).into_iter();
                groups
                    .map(|group| language.say(group).to_owned())
                    .collect::<Vec<_>>()
            };
            let lacking = set.is_empty().then_some("This theme declares no settings.");
            let owner = Owner::Theme(name.clone());
            let parts = CardParts {
                item: name.clone(),
                title: title.into(),
                rows: owned_rows(&owner, set),
                problems,
                remark: lacking,
                footer: Vec::new(),
                affects: &affects,
            };
            cards.push(self.declared_card(parts, window, cx));
        }
        self.declared.read.themes = themes;
        cards
    }

    /// The themes stacked over the base theme, with buttons that move and
    /// remove them, and those that can be stacked.
    fn layers_card(
        &mut self,
        config: &Config,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = Language::of(cx);
        let base = config.theme.as_deref();
        let titles: Vec<SharedString> = { self.themes.iter() }
            .map(|name| super::theme_title(name, language))
            .collect();
        // The base theme is not stacked over itself.
        let offered = |name: &String| Some(name.as_str()) != base;
        self.stack_card(Stack::Layers, config, &titles, &offered, window, cx)
    }

    /// The icon packs in use, in order, with buttons that move and remove
    /// them, and the installed packs that can be used besides.
    pub(super) fn icon_packs_card(
        &mut self,
        config: &Config,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let packs = &self.declared.read.icon_packs;
        let titles: Vec<SharedString> = packs.iter().cloned().map(SharedString::from).collect();
        self.stack_card(Stack::IconPacks, config, &titles, &|_| true, window, cx)
    }

    /// A card of what `stack` holds, each with buttons that move it and
    /// take it off, and chips, named by `titles`, for what can be put on it
    /// that `offered` lets through and it does not hold yet.
    fn stack_card(
        &mut self,
        stack: Stack,
        config: &Config,
        titles: &[SharedString],
        offered: &dyn Fn(&String) -> bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let held = stack.of(config);
        let mut stacked = Vec::new();
        for (index, name) in held.iter().enumerate() {
            let mut buttons = Vec::new();
            if index > 0 {
                let up = Control::ShiftLayer(stack, index, true);
                buttons.push(self.control(up, Face::Glyph(ICON_UP), window, cx));
            }
            if index + 1 < held.len() {
                let down = Control::ShiftLayer(stack, index, false);
                buttons.push(self.control(down, Face::Glyph(ICON_DOWN), window, cx));
            }
            let remove = Control::DropLayer(stack, index);
            buttons.push(self.control(remove, Face::Glyph(ICON_REMOVE), window, cx));
            stacked.push(chip(&theme, format!("{}. {name}", index + 1), buttons));
        }
        let mut offers = Vec::new();
        for index in 0..self.stackable(stack).len() {
            let name = &self.stackable(stack)[index];
            if !offered(name) || held.contains(name) {
                continue;
            }
            let face = Face::Chip {
                text: titles
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| name.clone().into()),
                selected: true,
            };
            offers.push(self.control(Control::AddLayer(stack, index), face, window, cx));
        }
        let texts = stack.texts();
        let mut section =
            card(&theme, language.say(texts.title)).child(note(&theme, language.say(texts.about)));
        section = match (stacked.is_empty(), offers.is_empty()) {
            (true, true) => section.child(note(&theme, language.say(texts.nothing))),
            (true, false) => section.child(note(&theme, language.say(texts.empty))),
            (false, _) => section.child(wrap(&theme).children(stacked)),
        };
        if !offers.is_empty() {
            section = section
                .child(caption(&theme, language.say(texts.add)))
                .child(wrap(&theme).children(offers));
        }
        section.into_any_element()
    }

    /// A card of declared settings: each row with its control, what it
    /// affects as `affects` names it, Reset where the file sets it and
    /// what kept a change from being made under it; Reset all over them.
    /// The remark is said under the settings, such as that there is
    /// nothing to set, then the footer, then the declarations left out.
    pub(super) fn declared_card(
        &mut self,
        parts: CardParts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        // What the file holds at each row, and what holds there: what the
        // file holds where it fits the row, its default otherwise.
        let values: Vec<(Option<toml::Value>, Option<toml::Value>)> = { parts.rows.iter() }
            .map(|(row, decl)| {
                let written = self.written(row);
                let fitting = written.as_ref().and_then(|value| decl.validate(value).ok());
                let resolved = fitting.or_else(|| decl.default.clone());
                (written, resolved)
            })
            .collect();
        // What the conditions of the rows look at, by key.
        let resolved: toml::Table = { parts.rows.iter().zip(&values) }
            .filter_map(|((_, decl), (_, resolved))| Some((decl.key.clone(), resolved.clone()?)))
            .collect();
        let set_rows: Vec<RowKey> = { parts.rows.iter().zip(&values) }
            .filter(|(_, (written, _))| written.is_some())
            .map(|((row, _), _)| row.clone())
            .collect();
        let reset_all = (!set_rows.is_empty()).then(|| {
            let face = Face::Chip {
                text: language.say("Reset all").into(),
                selected: false,
            };
            self.control(Control::ResetAll(set_rows), face, window, cx)
        });
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(theme.spacing.small)
            .child(super::heading(&theme, parts.title))
            .children(reset_all);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.small + theme.spacing.tiny)
            .child(header);
        let mut group: Option<String> = None;
        let mut lit = self.declared.is_lit(&parts.item);
        for ((row, decl), (written, value)) in parts.rows.iter().zip(&values) {
            if !OptionSet::visible(decl, &resolved) {
                continue;
            }
            let section = decl.section.as_ref().map(|section| section.text(language));
            if section.is_some() && section != group {
                group.clone_from(&section);
                body = body.children(section.map(|section| caption(&theme, section)));
            }
            lit |= self.declared.is_lit(&row.owner.item());
            let line = self.declared_row(
                row,
                decl,
                written.as_ref(),
                value.as_ref(),
                (parts.affects)(decl),
                window,
                cx,
            );
            body = body.child(line);
        }
        body = body.children(
            parts
                .remark
                .map(|remark| note(&theme, language.say(remark))),
        );
        body = body.children(parts.footer);
        if !parts.problems.is_empty() {
            let left_out = language.say("Some settings are declared wrongly and are left out:");
            body = body.child(note(&theme, left_out));
            for problem in parts.problems {
                body = body.child(note(&theme, problem.clone()).text_size(small_text(&theme)));
            }
        }
        let light = match (&mut self.declared.lit, lit) {
            (Some((_, light)), true) => light.value(window).clamp(0., 1.),
            _ => 0.,
        };
        div()
            .p(theme.spacing.medium)
            .rounded(theme.shape.large)
            .bg(theme.colors.surface_container_highest)
            .border_2()
            .border_color(mix(transparent_black(), theme.colors.primary, light))
            .child(body)
            .into_any_element()
    }

    /// One declared setting: its label, Reset where the file sets it, its
    /// control, what it does and what it affects, and why its last change
    /// was not made. It is lit while it is the one asked for.
    #[allow(clippy::too_many_arguments, reason = "the parts of one row")]
    fn declared_row(
        &mut self,
        row: &RowKey,
        decl: &OptionDecl,
        written: Option<&toml::Value>,
        resolved: Option<&toml::Value>,
        affected: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let (inline, below) = self.declared_control(row, decl, written, resolved, window, cx);
        let reset = written.is_some().then(|| {
            let reset = Control::Declared(row.clone(), Deed::Unset);
            self.control(reset, Face::Glyph(ICON_RESET), window, cx)
        });
        let line = div()
            .min_h(CHIP_HEIGHT)
            .flex()
            .items_center()
            .justify_between()
            .gap(theme.spacing.small)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(theme.colors.on_surface)
                    .child(decl.label.text(language)),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.tiny)
                    .children(reset)
                    .children(inline),
            );
        let small = small_text(&theme);
        let description = { decl.description.as_ref() }
            .map(|description| note(&theme, description.text(language)).text_size(small));
        let affects = (!affected.is_empty()).then(|| {
            let said = format!("{} {}", language.say("Affects:"), affected.join(", "));
            note(&theme, said).text_size(small)
        });
        let refused = { self.declared.row_error.as_ref() }
            .filter(|(known, _)| known == row)
            .map(|(_, error)| {
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.tiny)
                    .text_size(small)
                    .text_color(theme.colors.primary)
                    .child(icon(&theme, ICON_WARNING).text_size(small))
                    .child(format!("{} {error}", language.say("Not written:")))
            });
        let light = match &mut self.declared.lit {
            Some((item, light)) if *item == row.owner.item() => light.value(window).clamp(0., 1.),
            _ => 0.,
        };
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .rounded(theme.shape.medium)
            .bg(mix(
                transparent_black(),
                theme.colors.secondary_container,
                light,
            ))
            .child(line)
            .children(description)
            .children(affects)
            .children(below)
            .children(refused)
            .into_any_element()
    }

    /// The control of a declared setting: what stands beside its label,
    /// and what takes the width of the card under it.
    fn declared_control(
        &mut self,
        row: &RowKey,
        decl: &OptionDecl,
        written: Option<&toml::Value>,
        resolved: Option<&toml::Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, Option<AnyElement>) {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let unset_label = || {
            decl.unset_label.as_ref().map_or_else(
                || language.say("Default").to_owned(),
                |label| label.text(language),
            )
        };
        let deed = |deed: Deed| Control::Declared(row.clone(), deed);
        let beside = |element: gpui::Div| (Some(element.into_any_element()), None);
        match decl.kind {
            Kind::Toggle if decl.optional && decl.unset_label.is_some() => {
                let now = written.and_then(toml::Value::as_bool);
                let answers = [
                    (unset_label(), now.is_none()),
                    (language.say("On").to_owned(), now == Some(true)),
                    (language.say("Off").to_owned(), now == Some(false)),
                ];
                let mut chips = Vec::new();
                for (index, (text, selected)) in answers.into_iter().enumerate() {
                    let face = Face::Chip {
                        text: text.into(),
                        selected,
                    };
                    chips.push(self.control(deed(Deed::Choose(index)), face, window, cx));
                }
                beside(wrap(&theme).justify_end().children(chips))
            }
            Kind::Toggle => {
                let on = resolved.and_then(toml::Value::as_bool).unwrap_or(false);
                let on = f32::from(u8::from(on));
                let Held::Switch(spring) = self.held(row, decl, cx) else {
                    return (None, None);
                };
                if spring.target() != on {
                    spring.animate_to(on);
                }
                let travel = spring.value(window);
                let control = deed(Deed::Toggle);
                let state = self
                    .interactions
                    .entry(control.clone())
                    .or_insert_with(|| crate::motion::Interaction::new(&theme.motion))
                    .state(window);
                let action = control.clone();
                let id = SharedString::from(format!("{control:?}"));
                let switch = switch(&theme, id, travel, state)
                    .on_click(cx.listener(move |this, _, _, cx| this.act(&action, cx)));
                let switch =
                    crate::motion::Interaction::track(switch, cx, move |this: &mut Self| {
                        this.interactions.get_mut(&control)
                    });
                (Some(switch.into_any_element()), None)
            }
            Kind::Choice => {
                let mut answers: Vec<(String, bool)> = { decl.choices.iter().enumerate() }
                    .map(|(index, choice)| {
                        (choice.label(language), decl.chosen(written) == Some(index))
                    })
                    .collect();
                // A choice that may be unset, but has no answer for it,
                // gets one.
                let extra = decl.optional && decl.chosen(None).is_none();
                if extra {
                    answers.push((unset_label(), written.is_none()));
                }
                // Where nothing is written, the default is what holds.
                if written.is_none()
                    && !answers.iter().any(|(_, selected)| *selected)
                    && let Some(index) = decl.chosen(resolved)
                {
                    answers[index].1 = true;
                }
                let count = answers.len();
                let mut chips = Vec::new();
                for (index, (text, selected)) in answers.iter().cloned().enumerate() {
                    let deed = if extra && index + 1 == count {
                        deed(Deed::Unset)
                    } else {
                        deed(Deed::Choose(index))
                    };
                    let face = Face::Chip {
                        text: text.into(),
                        selected,
                    };
                    chips.push(self.control(deed, face, window, cx));
                }
                if count <= SEGMENTS {
                    return beside(wrap(&theme).justify_end().children(chips));
                }
                let current = { answers.iter() }
                    .find(|(_, selected)| *selected)
                    .map_or_else(|| "?".to_owned(), |(text, _)| text.clone());
                let open = self.declared.opened.as_ref() == Some(row);
                let opener = self.opener(deed(Deed::Open), current, open, window, cx);
                let list = open.then(|| wrap(&theme).children(chips).into_any_element());
                (Some(opener), list)
            }
            Kind::Number => {
                // A number that a step under its least takes out of the
                // file shows what that means until it has one.
                let value = if counts_from_unset(decl) {
                    written
                } else {
                    resolved
                };
                let shown = value.and_then(OptionDecl::number).map(|number| {
                    let mut plain = decl.clone();
                    plain.unit = None;
                    plain.shown_number(number)
                });
                let field = self.field(row, decl, shown.unwrap_or_default(), cx);
                let stepper = self.stepper(row, Deed::Step, field, window, cx);
                let unit = decl.unit.clone().filter(|unit| !unit.is_empty());
                beside(stepper.children(unit.map(|unit| {
                    div()
                        .text_color(theme.colors.on_surface_variant)
                        .child(unit)
                })))
            }
            Kind::Slider => {
                let share = resolved.and_then(OptionDecl::number);
                let Held::Slider(slider) = self.held(row, decl, cx) else {
                    return (None, None);
                };
                let slider = slider.clone();
                if let Some(number) = share {
                    slider.update(cx, |slider, cx| slider.show(number, cx));
                }
                beside(div().w(px(SLIDER_WIDTH)).flex().child(slider))
            }
            // A time and sides are typed as text until settings-core gives
            // them controls of their own.
            Kind::Text | Kind::Time | Kind::Sides => {
                let text = resolved.map(text_of).unwrap_or_default();
                let field = self.field(row, decl, text, cx);
                beside(div().w(px(FIELD_WIDTH)).flex().child(field))
            }
            Kind::Time => {
                let text = resolved.map(text_of).unwrap_or_default();
                let field = self.field(row, decl, text, cx);
                // The hour is stepped on the left of the field, the
                // minutes on its right.
                let [hours, minutes] = [true, false].map(|hours| {
                    [(false, ICON_LESS), (true, ICON_MORE)].map(|(up, glyph)| {
                        let step = deed(Deed::StepTime { hours, up });
                        self.control(step, Face::Glyph(glyph), window, cx)
                    })
                });
                let clock = div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.tiny)
                    .children(hours)
                    .child(div().w(px(TIME_WIDTH)).flex().child(field))
                    .children(minutes);
                beside(clock)
            }
            Kind::Sides => {
                let Held::Sides(sides) = self.held(row, decl, cx) else {
                    return (None, None);
                };
                let sides = sides.clone();
                let value = resolved.cloned();
                sides.update(cx, |sides, cx| sides.show(value, cx));
                (None, Some(sides.into_any_element()))
            }
            Kind::Color => {
                let text = resolved.map(text_of).unwrap_or_default();
                let swatch = { text.strip_prefix('#') }
                    .filter(|_| is_color(&text))
                    .and_then(|digits| u32::from_str_radix(&digits[..6], 16).ok());
                let field = self.field(row, decl, text, cx);
                let swatch = div()
                    .flex_none()
                    .size(SMALL_BUTTON)
                    .rounded(theme.shape.full)
                    .border_1()
                    .border_color(theme.colors.outline)
                    .bg(swatch.map_or(transparent_black(), |color| rgb(color).into()));
                let inline = div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .child(swatch)
                    .child(div().w(px(NUMBER_WIDTH + 24.)).flex().child(field));
                let mut presets = Vec::new();
                for (index, color) in PALETTE.into_iter().enumerate() {
                    presets.push(self.swatch(deed(Deed::Swatch(index)), color, window, cx));
                }
                let presets = wrap(&theme).justify_end().children(presets);
                (
                    Some(inline.into_any_element()),
                    Some(presets.into_any_element()),
                )
            }
            Kind::Path => {
                let text = resolved.map(text_of).unwrap_or_default();
                let field = self.field(row, decl, text, cx);
                let face = Face::Chip {
                    text: language.say("Browse").into(),
                    selected: true,
                };
                let browse = self.control(deed(Deed::Browse), face, window, cx);
                let line = div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .child(icon(&theme, ICON_FOLDER).text_color(theme.colors.on_surface_variant))
                    .child(div().flex_1().min_w_0().flex().child(field))
                    .child(browse);
                (None, Some(line.into_any_element()))
            }
            Kind::Font => {
                let current = resolved.and_then(toml::Value::as_str).map(str::to_owned);
                let open = self.declared.opened.as_ref() == Some(row);
                let shown = current.clone().unwrap_or_else(unset_label);
                let opener = self.opener(deed(Deed::Open), shown, open, window, cx);
                let default = (decl.optional && written.is_some()).then(|| {
                    let face = Face::Chip {
                        text: unset_label().into(),
                        selected: false,
                    };
                    self.control(deed(Deed::Unset), face, window, cx)
                });
                let inline = div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.tiny)
                    .children(default)
                    .child(opener);
                let list = if open {
                    let Held::Fonts(fonts) = self.held(row, decl, cx) else {
                        return (None, None);
                    };
                    let fonts = fonts.clone();
                    fonts.update(cx, |fonts, cx| fonts.mark(current, cx));
                    Some(fonts.into_any_element())
                } else {
                    None
                };
                (Some(inline.into_any_element()), list)
            }
            // A list with choices is a choice of several: a box for each,
            // written in the order of the choices.
            Kind::List if !decl.choices.is_empty() => {
                let held = match resolved {
                    Some(value) => crate::options::parse_list(&text_of(value)),
                    None => Vec::new(),
                };
                let mut boxes = Vec::new();
                let offered = { decl.choices.iter() }.filter(|choice| !choice.is_unset());
                for (index, choice) in offered.enumerate() {
                    let face = Face::Check {
                        text: choice.label(language).into(),
                        checked: held.contains(&text_of(&choice.value)),
                    };
                    boxes.push(self.control(deed(Deed::Check(index)), face, window, cx));
                }
                (None, Some(wrap(&theme).children(boxes).into_any_element()))
            }
            Kind::List | Kind::Pairs => {
                let items = match (decl.kind, resolved) {
                    (Kind::Pairs, Some(toml::Value::Table(table))) => { table.iter() }
                        .map(|(name, value)| (name.clone(), text_of(value)))
                        .collect(),
                    (Kind::Pairs, Some(value)) => {
                        crate::options::parse_pairs(&text_of(value)).unwrap_or_default()
                    }
                    (_, Some(value)) => { crate::options::parse_list(&text_of(value)).into_iter() }
                        .map(|item| (item, String::new()))
                        .collect(),
                    (_, None) => Vec::new(),
                };
                let Held::List(list) = self.held(row, decl, cx) else {
                    return (None, None);
                };
                let list = list.clone();
                list.update(cx, |list, cx| list.show(items, cx));
                (None, Some(list.into_any_element()))
            }
        }
    }

    /// A number between the buttons that take it a step down and up, which
    /// `step` makes the deeds of.
    fn stepper(
        &mut self,
        row: &RowKey,
        step: fn(bool) -> Deed,
        field: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = *cx.global::<Theme>();
        let [less, more] = [(false, ICON_LESS), (true, ICON_MORE)].map(|(up, glyph)| {
            let control = Control::Declared(row.clone(), step(up));
            self.control(control, Face::Glyph(glyph), window, cx)
        });
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .child(less)
            .child(div().w(px(NUMBER_WIDTH)).flex().child(field))
            .child(more)
    }

    /// The field of `row`, showing `text` unless something is being typed
    /// in it.
    fn field(
        &mut self,
        row: &RowKey,
        decl: &OptionDecl,
        text: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Held::Field(field) = self.held(row, decl, cx) else {
            return div().into_any_element();
        };
        let field = field.clone();
        field.update(cx, |field, cx| field.show(&text, cx));
        field.into_any_element()
    }

    /// A chip that shows `current` and opens a list, with a mark that
    /// turns while it is open.
    fn opener(
        &mut self,
        control: Control,
        current: String,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let face = Face::Chip {
            text: format!("{current}  ").into(),
            selected: open,
        };
        let mark =
            icon(&theme, if open { ICON_UP } else { ICON_OPEN }).text_size(small_text(&theme));
        self.button(control, face, window, cx)
            .child(mark)
            .into_any_element()
    }

    /// A round button in `color`.
    fn swatch(
        &mut self,
        control: Control,
        color: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let face = Face::Glyph("");
        self.button(control, face, window, cx)
            .bg(gpui::Hsla::from(rgb(color)))
            .border_1()
            .border_color(theme.colors.outline)
            .into_any_element()
    }
}

/// A chip that holds a text and buttons.
fn chip(theme: &Theme, text: String, buttons: Vec<AnyElement>) -> AnyElement {
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
        .text_color(theme.colors.on_surface)
        .child(div().pr(theme.spacing.tiny).child(text))
        .children(buttons)
        .into_any_element()
}

/// The name of a group of settings.
fn caption(theme: &Theme, text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(small_text(theme))
        .text_color(theme.colors.primary)
        .child(text.into())
}

/// The size of what is said under a setting.
fn small_text(theme: &Theme) -> gpui::Pixels {
    theme.typography.label_size * CAPTION_SHARE
}

/// A field of a declared text, number, color or path. It shows what the
/// file holds until something is typed, and writes what is typed once
/// typing pauses, Enter is pressed or the field is left; Esc gives back
/// what the file holds.
pub(super) struct DeclaredField {
    field: Entity<TextField>,
    /// What the file held when it was shown last.
    written: String,
    /// Whether what is typed has not been written yet.
    typed: bool,
    /// Whether the keys went to the field when it was drawn last.
    focused: bool,
    pause: Option<Task<()>>,
}

impl EventEmitter<Commit> for DeclaredField {}

impl DeclaredField {
    /// A field that says `placeholder` while it is empty, and that writes
    /// what is typed once typing pauses where `on_pause` says so.
    fn new(placeholder: SharedString, on_pause: bool, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| TextField::new(placeholder, cx));
        cx.observe(&field, move |this: &mut Self, field, cx| {
            let text = field.read(cx).text();
            if text == this.written {
                return;
            }
            this.typed = true;
            if !on_pause {
                return;
            }
            this.pause = Some(
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    cx.background_executor().timer(TYPING_PAUSE).await;
                    this.update(cx, |this, cx| this.commit(cx)).ok();
                }),
            );
        })
        .detach();
        Self {
            field,
            written: String::new(),
            typed: false,
            focused: false,
            pause: None,
        }
    }

    /// Shows `text`, what the file holds, unless something is typed.
    fn show(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.typed || self.written == text {
            return;
        }
        self.written = text.to_owned();
        self.field.update(cx, |field, cx| field.put_text(text, cx));
    }

    /// Asks for what is typed to be written: the text, trimmed, or for an
    /// empty field no value.
    fn commit(&mut self, cx: &mut Context<Self>) {
        self.pause = None;
        if !self.typed {
            return;
        }
        self.typed = false;
        let text = self.field.read(cx).text();
        let trimmed = text.trim();
        self.written = text.clone();
        let value = (!trimmed.is_empty()).then(|| toml::Value::String(trimmed.to_owned()));
        cx.emit(Commit(Ok(value)));
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => self.commit(cx),
            "escape" if self.typed => {
                self.typed = false;
                self.pause = None;
                let written = self.written.clone();
                self.field
                    .update(cx, |field, cx| field.put_text(&written, cx));
            }
            _ => {}
        }
    }
}

impl Render for DeclaredField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.field.read(cx).is_focused(window);
        if self.focused && !focused {
            // Leaving the field writes what was typed.
            self.commit(cx);
        }
        self.focused = focused;
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .child(self.field.clone())
    }
}

/// A slider of a declared number. It follows the pointer, and writes the
/// number once it is let go; a turn of the wheel writes once the wheel
/// rests.
pub(super) struct DeclaredSlider {
    decl: OptionDecl,
    slider: Slider,
    /// The number shown.
    number: f64,
    /// Whether the pointer holds the slider.
    held: bool,
    /// Whether the number shown has not been written yet.
    moved: bool,
    pause: Option<Task<()>>,
}

impl EventEmitter<Commit> for DeclaredSlider {}

impl DeclaredSlider {
    fn new(decl: OptionDecl, theme: &Theme) -> Self {
        let number = { decl.default.as_ref() }
            .and_then(OptionDecl::number)
            .or(decl.min)
            .unwrap_or(0.);
        let mut this = Self {
            slider: Slider::new(theme, 0.),
            decl,
            number,
            held: false,
            moved: false,
            pause: None,
        };
        this.slider = Slider::new(theme, this.share());
        this
    }

    fn range(&self) -> (f64, f64) {
        (self.decl.min.unwrap_or(0.), self.decl.max.unwrap_or(1.))
    }

    fn share(&self) -> f32 {
        let (min, max) = self.range();
        if max <= min {
            return 0.;
        }
        ((self.number - min) / (max - min)).clamp(0., 1.) as f32
    }

    /// Shows `number`, what the file holds, unless the slider is moved.
    fn show(&mut self, number: f64, cx: &mut Context<Self>) {
        if !self.held && !self.moved && self.number != number {
            self.number = number;
            cx.notify();
        }
    }

    fn moved(&mut self, share: f32, cx: &mut Context<Self>) {
        let (min, max) = self.range();
        let number = self.decl.fit(min + f64::from(share) * (max - min));
        if number == self.number {
            return;
        }
        self.number = number;
        self.moved = true;
        if !self.held {
            // The wheel: written once it rests.
            self.pause = Some(
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    cx.background_executor().timer(TYPING_PAUSE).await;
                    this.update(cx, |this, cx| this.commit(cx)).ok();
                }),
            );
        }
        cx.notify();
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        self.pause = None;
        if self.moved {
            self.moved = false;
            cx.emit(Commit(Ok(Some(self.decl.number_value(self.number)))));
        }
    }

    fn let_go(&mut self, cx: &mut Context<Self>) {
        if self.held {
            self.held = false;
            self.commit(cx);
        }
    }
}

impl Render for DeclaredSlider {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let share = self.share();
        let track = self.slider.track(
            "declared-slider",
            share,
            window,
            cx,
            |this| &mut this.slider,
            Self::moved,
        );
        div()
            .flex_1()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            // The track is told of the press first, and takes it for a
            // turn of the wheel: the drag that starts writes when it ends.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.held = true;
                    this.pause = None;
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.let_go(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.let_go(cx)),
            )
            .child(track)
            .child(
                div()
                    .flex_none()
                    .w(theme.slider.value_width)
                    .text_color(theme.colors.on_surface_variant)
                    .child(self.decl.shown_number(self.number)),
            )
    }
}

/// One row of a list: its text, and for pairs its value.
struct ListRow {
    name: Entity<TextField>,
    value: Option<Entity<TextField>>,
}

/// A row of a list on its way to another place.
struct CarriedRow {
    from: usize,
    title: SharedString,
}

/// The rows of a declared list or pairs: a field for each text, or a name
/// and a value, with a handle to drag it to another place and a button
/// that takes it out, and a button that adds one.
pub(super) struct DeclaredList {
    pairs: bool,
    /// The fewest and the most items it may have; it shows at least the
    /// fewest rows, and offers no more than the most.
    bounds: (usize, Option<usize>),
    rows: Vec<ListRow>,
    /// What the file held when it was shown last.
    written: Vec<(String, String)>,
    /// Whether what is typed has not been written yet.
    typed: bool,
    /// The row being dragged.
    lifted: Option<usize>,
    pause: Option<Task<()>>,
    focus: FocusHandle,
}

impl EventEmitter<Commit> for DeclaredList {}

impl DeclaredList {
    fn new(pairs: bool, bounds: (usize, Option<usize>), cx: &mut Context<Self>) -> Self {
        Self {
            pairs,
            bounds,
            rows: Vec::new(),
            written: Vec::new(),
            typed: false,
            lifted: None,
            pause: None,
            focus: cx.focus_handle(),
        }
    }

    /// A field that is watched for what is typed in it.
    fn field(
        &mut self,
        text: &str,
        placeholder: &'static str,
        cx: &mut Context<Self>,
    ) -> Entity<TextField> {
        let placeholder = Language::of(cx).say(placeholder);
        let field = cx.new(|cx| {
            let mut field = TextField::new(placeholder.into(), cx);
            field.put_text(text, cx);
            field
        });
        cx.observe(&field, |this: &mut Self, _, cx| this.typing(cx))
            .detach();
        field
    }

    fn row(&mut self, (name, value): &(String, String), cx: &mut Context<Self>) -> ListRow {
        let pairs = self.pairs;
        ListRow {
            name: self.field(name, "Name", cx),
            value: pairs.then(|| self.field(value, "Value", cx)),
        }
    }

    /// What the rows hold, those without a text left out.
    fn items(&self, cx: &App) -> Vec<(String, String)> {
        let text = |field: &Entity<TextField>| field.read(cx).text().trim().to_owned();
        let items = self.rows.iter().map(|row| {
            (
                text(&row.name),
                row.value.as_ref().map(text).unwrap_or_default(),
            )
        });
        items
            .filter(|(name, value)| !name.is_empty() && (!self.pairs || !value.is_empty()))
            .collect()
    }

    /// Shows `items`, what the file holds, unless something is typed.
    fn show(&mut self, items: Vec<(String, String)>, cx: &mut Context<Self>) {
        if self.typed || self.written == items {
            return;
        }
        self.rows = items.iter().map(|item| self.row(item, cx)).collect();
        // Rows to fill in up to the fewest there may be.
        while self.rows.len() < self.bounds.0 {
            let empty = self.row(&(String::new(), String::new()), cx);
            self.rows.push(empty);
        }
        self.written = items;
        cx.notify();
    }

    /// Whether `count` items may be written.
    fn fits(&self, count: usize) -> bool {
        count >= self.bounds.0 && self.bounds.1.is_none_or(|most| count <= most)
    }

    fn typing(&mut self, cx: &mut Context<Self>) {
        if self.items(cx) == self.written {
            return;
        }
        self.typed = true;
        self.pause = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor().timer(TYPING_PAUSE).await;
                this.update(cx, |this, cx| this.commit(cx)).ok();
            }),
        );
    }

    /// Asks for what the rows hold to be written: as a list of texts, or
    /// as pairs; no value where there is nothing. Pairs that would not be
    /// read back as they are, such as a name with a `=`, are not written:
    /// they stay as typed, and the settings say why.
    fn commit(&mut self, cx: &mut Context<Self>) {
        self.pause = None;
        let items = self.items(cx);
        if items == self.written {
            self.typed = false;
            return;
        }
        if self.pairs
            && let Err(error) = crate::options::check_pairs(&items)
        {
            self.typed = true;
            cx.emit(Commit(Err(error)));
            return;
        }
        // A list of a bounded length is written once it has enough items;
        // the rows stay as typed until then.
        if !self.fits(items.len()) && !items.is_empty() {
            self.typed = true;
            return;
        }
        self.typed = false;
        self.written = items.clone();
        let value = (!items.is_empty()).then(|| {
            if self.pairs {
                toml::Value::String(crate::options::write_pairs(&items))
            } else {
                toml::Value::Array(items.into_iter().map(|(name, _)| name.into()).collect())
            }
        });
        cx.emit(Commit(Ok(value)));
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.row(&(String::new(), String::new()), cx);
        row.name.read(cx).focus(window);
        self.rows.push(row);
        cx.notify();
    }

    fn remove(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.rows.len() && self.rows.len() > self.bounds.0 {
            self.rows.remove(index);
            self.commit(cx);
            cx.notify();
        }
    }

    /// Moves the row dragged from `from` to stand before `to`, or last.
    fn land(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        self.lifted = None;
        if from >= self.rows.len() || from == to {
            return;
        }
        let row = self.rows.remove(from);
        let to = if to > from { to - 1 } else { to };
        self.rows.insert(to.min(self.rows.len()), row);
        self.commit(cx);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if event.keystroke.key == "enter" {
            self.commit(cx);
        }
    }
}

/// A small round button with a glyph that does something to a list.
fn list_button(
    theme: &Theme,
    id: (&'static str, usize),
    glyph: &'static str,
) -> gpui::Stateful<gpui::Div> {
    let hover = mix(
        theme.colors.surface_container_highest,
        theme.colors.on_surface,
        crate::ui::HOVER_LAYER,
    );
    div()
        .id(id)
        .flex_none()
        .size(SMALL_BUTTON)
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.shape.full)
        .text_color(theme.colors.on_surface_variant)
        .hover(move |style| style.bg(hover))
        .child(icon(theme, glyph).text_size(theme.typography.label_size))
}

impl Render for DeclaredList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        if self.lifted.is_some() && !cx.has_active_drag() {
            self.lifted = None;
        }
        let mark = theme.colors.primary;
        let mut rows = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            let title: SharedString = row.name.read(cx).text().into();
            let carried = CarriedRow { from: index, title };
            let handle = list_button(&theme, ("list-drag", index), ICON_DRAG)
                .cursor_grab()
                .on_drag(carried, |carried: &CarriedRow, _, _, cx| {
                    let title = carried.title.clone();
                    cx.new(|_| Lifted { title })
                });
            // A list cut to its fewest items keeps them.
            let remove = (self.rows.len() > self.bounds.0).then(|| {
                list_button(&theme, ("list-remove", index), ICON_REMOVE)
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(index, cx)))
            });
            let fields = div()
                .flex_1()
                .min_w_0()
                .flex()
                .gap(theme.spacing.small)
                .child(div().flex_1().min_w_0().flex().child(row.name.clone()))
                .children(
                    row.value
                        .clone()
                        .map(|value| div().flex_1().min_w_0().flex().child(value)),
                );
            rows.push(
                div()
                    .id(("list-row", index))
                    .flex()
                    .items_center()
                    .gap(theme.spacing.tiny)
                    .border_t_2()
                    .border_color(transparent_black())
                    .opacity(if self.lifted == Some(index) {
                        LIFTED
                    } else {
                        1.
                    })
                    .drag_over::<CarriedRow>(move |style, _, _, _| style.border_color(mark))
                    .on_drop(cx.listener(move |this, carried: &CarriedRow, _, cx| {
                        this.land(carried.from, index, cx);
                    }))
                    .child(handle)
                    .child(fields)
                    .children(remove),
            );
        }
        let count = self.rows.len();
        let full = self.bounds.1.is_some_and(|most| count >= most);
        let add = (!full).then(|| {
            div()
                .id("list-add")
                .flex()
                .items_center()
                .gap(theme.spacing.tiny)
                .text_color(theme.colors.primary)
                .cursor_pointer()
                .child(icon(&theme, ICON_MORE).text_size(theme.typography.label_size))
                .child(language.say("Add"))
                .on_click(cx.listener(|this, _, window, cx| this.add(window, cx)))
                .drag_over::<CarriedRow>(move |style, _, _, _| style.text_color(mark))
                .on_drop(cx.listener(move |this, carried: &CarriedRow, _, cx| {
                    this.land(carried.from, count, cx);
                }))
        });
        div()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .children(rows)
            .children(add)
    }
}

/// The sides of a setting of the kind `sides`, in the order of the table
/// the file holds, each with what its field says while it is empty.
const SIDES: [(&str, &str); 4] = [
    ("top", "Top"),
    ("right", "Right"),
    ("bottom", "Bottom"),
    ("left", "Left"),
];

/// The room at the sides of something: one number for every side, or a
/// number for each of top, right, bottom and left. What is typed is
/// written once typing pauses or Enter is pressed; a
/// text that is no number is said and not written.
pub(super) struct DeclaredSides {
    decl: OptionDecl,
    /// Whether each side has a number of its own.
    each: bool,
    /// The field of the number of every side.
    all: Entity<TextField>,
    /// The fields of top, right, bottom and left.
    sides: [Entity<TextField>; 4],
    /// What the file held when it was shown last.
    written: Option<toml::Value>,
    /// Whether what is typed has not been written yet.
    typed: bool,
    pause: Option<Task<()>>,
}

impl EventEmitter<Commit> for DeclaredSides {}

impl DeclaredSides {
    fn new(decl: OptionDecl, cx: &mut Context<Self>) -> Self {
        let language = Language::of(cx);
        let placeholder = { decl.placeholder.as_ref().or(decl.unset_label.as_ref()) }
            .map(|label| label.text(language))
            .unwrap_or_default();
        let all = Self::field(placeholder.into(), cx);
        let sides = SIDES.map(|(_, title)| Self::field(language.say(title).into(), cx));
        Self {
            decl,
            each: false,
            all,
            sides,
            written: None,
            typed: false,
            pause: None,
        }
    }

    /// A field that is watched for what is typed in it.
    fn field(placeholder: SharedString, cx: &mut Context<Self>) -> Entity<TextField> {
        let field = cx.new(|cx| TextField::new(placeholder, cx));
        cx.observe(&field, |this: &mut Self, _, cx| this.typing(cx))
            .detach();
        field
    }

    /// The number `value` holds as the field shows it.
    fn shown(&self, value: Option<&toml::Value>) -> String {
        { value.and_then(OptionDecl::number) }
            .map(|number| self.decl.shown_number(number))
            .map(|text| text.trim_end_matches(['%', ' ']).to_owned())
            .unwrap_or_default()
    }

    /// Shows `value`, what the file holds, unless something is typed.
    fn show(&mut self, value: Option<toml::Value>, cx: &mut Context<Self>) {
        if self.typed || self.written == value {
            return;
        }
        self.each = matches!(value, Some(toml::Value::Table(_)));
        let all = match &value {
            Some(toml::Value::Table(_)) => String::new(),
            other => self.shown(other.as_ref()),
        };
        let texts = SIDES.map(|(side, _)| match &value {
            Some(toml::Value::Table(table)) => self.shown(table.get(side)),
            other => self.shown(other.as_ref()),
        });
        self.all.update(cx, |field, cx| field.put_text(&all, cx));
        for (field, text) in self.sides.iter().zip(texts) {
            field.update(cx, |field, cx| field.put_text(&text, cx));
        }
        self.written = value;
        self.typed = false;
        cx.notify();
    }

    fn typing(&mut self, cx: &mut Context<Self>) {
        self.typed = true;
        self.pause = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor().timer(TYPING_PAUSE).await;
                this.update(cx, |this, cx| this.commit(cx)).ok();
            }),
        );
    }

    /// What the fields hold, as the file is to hold it: the number of
    /// every side, or a table of the sides that have one; no value where
    /// nothing is typed.
    fn value(&self, cx: &App) -> Result<Option<toml::Value>, String> {
        let number = |field: &Entity<TextField>| -> Result<Option<toml::Value>, String> {
            let text = field.read(cx).text();
            let text = text.trim();
            if text.is_empty() {
                return Ok(None);
            }
            let number: f64 = { text.parse() }.map_err(|_| format!("`{text}` is no number"))?;
            Ok(Some(self.decl.number_value(self.decl.fit(number))))
        };
        if !self.each {
            return number(&self.all);
        }
        let mut table = toml::Table::new();
        for ((side, _), field) in SIDES.iter().zip(&self.sides) {
            if let Some(room) = number(field)? {
                table.insert((*side).to_owned(), room);
            }
        }
        Ok((!table.is_empty()).then_some(toml::Value::Table(table)))
    }

    /// Asks for what the fields hold to be written.
    fn commit(&mut self, cx: &mut Context<Self>) {
        self.pause = None;
        if !self.typed {
            return;
        }
        let value = self.value(cx);
        if let Ok(value) = &value {
            self.typed = false;
            self.written.clone_from(value);
        }
        cx.emit(Commit(value));
    }

    /// Gives each side a number of its own, starting from the number of
    /// every side, or one number for all, starting from the top.
    fn split(&mut self, each: bool, cx: &mut Context<Self>) {
        if self.each == each {
            return;
        }
        self.each = each;
        if each {
            let all = self.all.read(cx).text();
            for field in &self.sides {
                field.update(cx, |field, cx| field.put_text(&all, cx));
            }
        } else {
            let top = self.sides[0].read(cx).text();
            self.all.update(cx, |field, cx| field.put_text(&top, cx));
        }
        self.typed = true;
        self.commit(cx);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if event.keystroke.key == "enter" {
            self.commit(cx);
        }
    }
}

impl Render for DeclaredSides {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut modes = Vec::new();
        for (each, title) in [(false, "Every side"), (true, "Each side")] {
            let selected = self.each == each;
            let (ground, color) = if selected {
                (theme.colors.secondary_container, theme.colors.primary)
            } else {
                (theme.colors.surface_container, theme.colors.on_surface)
            };
            let hover = mix(ground, theme.colors.on_surface, crate::ui::HOVER_LAYER);
            modes.push(
                div()
                    .id(("sides-mode", usize::from(each)))
                    .h(CHIP_HEIGHT)
                    .px(theme.spacing.small + theme.spacing.tiny)
                    .flex()
                    .items_center()
                    .rounded(theme.shape.full)
                    .bg(ground)
                    .hover(move |style| style.bg(hover))
                    .text_color(color)
                    .cursor_pointer()
                    .child(language.say(title))
                    .on_click(cx.listener(move |this, _, _, cx| this.split(each, cx))),
            );
        }
        let unit = self.decl.unit.clone().filter(|unit| !unit.is_empty());
        let fields = if self.each {
            let sides = self
                .sides
                .iter()
                .map(|field| div().w(px(NUMBER_WIDTH)).flex().child(field.clone()));
            wrap(&theme).children(sides)
        } else {
            wrap(&theme).child(div().w(px(NUMBER_WIDTH)).flex().child(self.all.clone()))
        };
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .child(wrap(&theme).children(modes))
            .child(fields.children(unit.map(|unit| {
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(unit)
            })))
    }
}
/// The installed font families to choose one of, with a field that
/// narrows them down.
pub(super) struct FontPicker {
    filter: Entity<TextField>,
    /// Every family, once they have been read.
    families: Option<&'static [String]>,
    /// The places of those that match the filter.
    matches: Vec<usize>,
    /// The family the file names.
    current: Option<String>,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<Commit> for FontPicker {}

impl FontPicker {
    fn new(cx: &mut Context<Self>) -> Self {
        let placeholder = Language::of(cx).say("Filter");
        let filter = cx.new(|cx| TextField::new(placeholder.into(), cx));
        cx.observe(&filter, |this: &mut Self, _, cx| this.narrow(cx))
            .detach();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let families = cx
                .background_executor()
                .spawn(async { font_families() })
                .await;
            this.update(cx, |this, cx| {
                this.families = Some(families);
                this.narrow(cx);
            })
            .ok();
        })
        .detach();
        Self {
            filter,
            families: None,
            matches: Vec::new(),
            current: None,
            scroll: UniformListScrollHandle::new(),
        }
    }

    /// Marks `current` as the family the file names.
    fn mark(&mut self, current: Option<String>, cx: &mut Context<Self>) {
        if self.current != current {
            self.current = current;
            cx.notify();
        }
    }

    fn narrow(&mut self, cx: &mut Context<Self>) {
        let wanted = self.filter.read(cx).text().trim().to_lowercase();
        let families = self.families.unwrap_or_default();
        self.matches = { families.iter().enumerate() }
            .filter(|(_, family)| family.to_lowercase().contains(&wanted))
            .map(|(index, _)| index)
            .collect();
        cx.notify();
    }

    fn rows(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = *cx.global::<Theme>();
        let families = self.families.unwrap_or_default();
        let mut rows = Vec::new();
        for at in range {
            let Some(family) = self.matches.get(at).and_then(|index| families.get(*index)) else {
                continue;
            };
            let chosen = self.current.as_deref() == Some(family.as_str());
            let hover = mix(
                theme.colors.surface_container,
                theme.colors.on_surface,
                crate::ui::HOVER_LAYER,
            );
            let ground = if chosen {
                theme.colors.secondary_container
            } else {
                theme.colors.surface_container
            };
            let picked = family.clone();
            rows.push(
                div()
                    .id(("font", at))
                    .h(px(FONT_ROW_HEIGHT))
                    .px(theme.spacing.small)
                    .flex()
                    .items_center()
                    .rounded(theme.shape.small)
                    .bg(ground)
                    .hover(move |style| style.bg(hover))
                    .text_color(if chosen {
                        theme.colors.primary
                    } else {
                        theme.colors.on_surface
                    })
                    .font_family(SharedString::from(family.clone()))
                    .child(family.clone())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(Commit(Ok(Some(picked.clone().into()))))
                    }))
                    .into_any_element(),
            );
        }
        rows
    }
}

impl Render for FontPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let list = match self.families {
            None => {
                note(&theme, language.say("The installed fonts are being read.")).into_any_element()
            }
            Some(_) if self.matches.is_empty() => {
                note(&theme, language.say("Nothing matches")).into_any_element()
            }
            Some(_) => uniform_list(
                "fonts",
                self.matches.len(),
                cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
            )
            .track_scroll(self.scroll.clone())
            .h(px(FONT_LIST_HEIGHT))
            .into_any_element(),
        };
        div()
            .p(theme.spacing.small)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container)
            .child(div().flex().child(self.filter.clone()))
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::Choice;

    #[test]
    fn a_time_typed_with_one_digit_of_hours_is_padded() {
        assert_eq!(padded_time("7:30"), "07:30");
        assert_eq!(padded_time(" 21:05 "), "21:05");
        assert_eq!(padded_time("7"), "7");
        assert_eq!(padded_time("x:30"), "x:30");
    }

    #[test]
    fn a_time_steps_by_hours_and_by_whole_minutes_around_the_clock() {
        assert_eq!(stepped_time(Some("07:30"), true, true), "08:30");
        assert_eq!(stepped_time(Some("00:15"), true, false), "23:15");
        assert_eq!(stepped_time(Some("07:32"), false, true), "07:35");
        assert_eq!(stepped_time(Some("07:32"), false, false), "07:30");
        assert_eq!(stepped_time(Some("07:30"), false, false), "07:25");
        assert_eq!(stepped_time(Some("23:55"), false, true), "00:00");
        assert_eq!(stepped_time(None, false, false), "23:55");
        assert_eq!(stepped_time(Some("late"), true, true), "01:00");
    }

    #[test]
    fn a_choice_of_several_is_written_in_the_order_of_its_choices() {
        let choices: Vec<Choice> = ["mon", "tue", "sun"]
            .iter()
            .map(|value| Choice {
                value: (*value).into(),
                label: None,
                overlay: None,
            })
            .collect();
        let held: toml::Value = toml::Value::Array(vec!["sun".into(), "mon".into()]);
        assert_eq!(
            toggled_items(&choices, Some(&held), 1),
            ["mon", "tue", "sun"]
        );
        assert_eq!(toggled_items(&choices, Some(&held), 2), ["mon"]);
        assert_eq!(toggled_items(&choices, None, 2), ["sun"]);
        // A list written as text by hand reads the same.
        let text: toml::Value = "tue; sun".into();
        assert_eq!(toggled_items(&choices, Some(&text), 1), ["sun"]);
    }

    #[test]
    fn a_value_is_written_as_the_file_writes_it() {
        let sides: toml::Value =
            toml::from_str::<toml::Table>("v = { top = 4, left = 2.5 }").unwrap()["v"].clone();
        let written = edit_value(&sides).to_string();
        assert!(written.contains("top = 4"), "{written}");
        assert!(written.contains("left = 2.5"), "{written}");
        let list = toml::Value::Array(vec!["a".into(), "b".into()]);
        assert_eq!(edit_value(&list).to_string(), r#"["a", "b"]"#);
    }
}
