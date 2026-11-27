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
    AnyElement, App, AppContext, AsyncApp, Context, Entity, EventEmitter, FocusHandle, Global,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, PathPromptOptions,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Task,
    UniformListScrollHandle, WeakEntity, Window, div, px, rgb, transparent_black, uniform_list,
};

use super::{
    CAPTION_SHARE, CHIP_HEIGHT, CHIP_TINT, Control, Face, ICON_LESS, ICON_MORE, ICON_REMOVE,
    LIFTED, Lifted, SMALL_BUTTON, Section, Settings, card, note, wrap,
};
use crate::config::Config;
use crate::config::edit::ConfigFile;
use crate::extension::{InstalledExtension, installed_extensions};
use crate::field::TextField;
use crate::i18n::Language;
use crate::motion::{Spring, SpringSpec, mix};
use crate::options::{Kind, OptionDecl, OptionSet, PathPick, SEGMENTS, is_color, text_of};
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

/// The widget the settings are, which a popup holds.
const WIDGET: &str = "settings";

/// Whose a declared setting is, and so where its value is written.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Owner {
    /// Under `[extension.<id>]`.
    Extension(String),
    /// Under `[theme_options.<name>]`.
    Theme(String),
}

impl Owner {
    /// What the file holds for it.
    fn values<'a>(&self, config: &'a Config) -> Option<&'a toml::Table> {
        match self {
            Self::Extension(id) => config.extensions.get(id),
            Self::Theme(name) => config.theme_options.get(name),
        }
    }

    /// The extension or theme by its name, as [`Settings::open_at`] names it.
    fn item(&self) -> &str {
        match self {
            Self::Extension(id) | Self::Theme(id) => id,
        }
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
        }
    }
}

/// A value of the file as the writer of the file takes it.
fn edit_value(value: &toml::Value) -> toml_edit::Value {
    match value {
        toml::Value::String(text) => text.as_str().into(),
        toml::Value::Integer(number) => (*number).into(),
        toml::Value::Float(number) => (*number).into(),
        toml::Value::Boolean(flag) => (*flag).into(),
        toml::Value::Array(items) => {
            let items: toml_edit::Array = items.iter().map(edit_value).collect();
            toml_edit::Value::Array(items)
        }
        other => { other.to_string().parse() }.unwrap_or_else(|_| other.to_string().into()),
    }
}

/// A declared setting: whose it is, and its key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct RowKey {
    owner: Owner,
    key: String,
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
    reading: Option<Task<()>>,
}

/// What a theme declares, and what is wrong with the settings left out;
/// `None` for a theme that is not there.
type ThemeDeclared = Option<(OptionSet, Vec<String>)>;

/// A control of a declared setting that keeps something between frames.
enum Held {
    Switch(Spring),
    Field(Entity<DeclaredField>),
    Slider(Entity<DeclaredSlider>),
    List(Entity<DeclaredList>),
    Fonts(Entity<FontPicker>),
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
    seek: Option<String>,
    /// The page of cards, scrolled to that card.
    pub(super) scroll: ScrollHandle,
}

impl DeclaredState {
    pub(super) fn new() -> Self {
        Self {
            read: Declared::default(),
            held: HashMap::new(),
            opened: None,
            lit: None,
            seek: None,
            scroll: ScrollHandle::new(),
        }
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
/// a change made while the settings are not shown.
fn write_now(row: &RowKey, value: Option<&toml::Value>, cx: &mut App) {
    let written = ConfigFile::open().and_then(|mut file| {
        if !row.owner.write(&mut file, &row.key, value) {
            return Err("the configuration has changed in the meantime".to_owned());
        }
        file.save()
    });
    match written {
        Ok(()) => Shell::config_changed(cx),
        Err(error) => report!("{error}"),
    }
}

impl Settings {
    /// Opens the settings at `section`, named by its English title in any
    /// case, such as `extensions`, and brings the card of `item` into
    /// sight, lit for a moment: an extension by its identifier or a theme
    /// by its name. Settings that are shown go there; otherwise the popup
    /// that holds them opens.
    pub fn open_at(section: &str, item: Option<&str>, cx: &mut App) {
        let found = Section::ALL
            .into_iter()
            .find(|known| known.title().eq_ignore_ascii_case(section));
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

    /// Shows `section`, and the card of `item` lit.
    fn go_to(&mut self, (section, item): (Section, Option<String>), cx: &mut Context<Self>) {
        self.show(section, cx);
        if let Some(item) = item {
            let mut light = Spring::new(FADE, 1.);
            light.animate_to(0.);
            self.declared.lit = Some((item.clone(), light));
            self.declared.seek = Some(item);
        }
        cx.notify();
    }

    /// Reads again, away from the thread that draws, what the extensions
    /// and the themes in use declare.
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
                (installed_extensions(), themes)
            });
            let (extensions, themes) = read.await;
            this.update(cx, |this, cx| {
                this.declared.read.extensions = Some(extensions);
                this.declared.read.themes = themes;
                cx.notify();
            })
            .ok();
        });
        self.declared.read.reading = Some(reading);
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
        };
        set.get(&row.key).cloned()
    }

    /// What the file holds at `row`, as written.
    fn written(&self, row: &RowKey) -> Option<toml::Value> {
        let config = self.config.as_ref()?;
        row.owner.values(config)?.get(&row.key).cloned()
    }

    /// Checks `value` against the declaration of `row` and writes it; one
    /// that does not fit is said and not written.
    fn write_declared(&mut self, row: &RowKey, value: Option<toml::Value>, cx: &mut Context<Self>) {
        let Some(decl) = self.declaration(row) else {
            return;
        };
        let value = match value.map(|value| decl.validate(&value)) {
            Some(Ok(value)) => Some(value),
            Some(Err(error)) => {
                self.refuse(&decl, &error, cx);
                return;
            }
            None => None,
        };
        if value.as_ref() == self.written(row).as_ref() {
            return;
        }
        let row = row.clone();
        self.change(cx, move |file| {
            row.owner.write(file, &row.key, value.as_ref())
        });
    }

    /// Says that what the control of `decl` holds is not written, and why.
    fn refuse(&mut self, decl: &OptionDecl, error: &str, cx: &mut Context<Self>) {
        let label = decl.label.text(Language::of(cx));
        self.failure = Some(format!("{label}: {error}").into());
        cx.notify();
    }

    /// Does what the controls of declared settings and of the layers of
    /// themes do; whether `control` is one of them.
    pub(super) fn act_declared(&mut self, control: &Control, cx: &mut Context<Self>) -> bool {
        match control {
            Control::Declared(row, deed) => self.deed(row, deed, cx),
            Control::ResetAll(owner) => {
                let keys = self.declared_keys(owner);
                let owner = owner.clone();
                self.change(cx, move |file| {
                    keys.iter().all(|key| owner.write(file, key, None))
                });
            }
            Control::AddLayer(index) => {
                let Some(name) = self.themes.get(*index).cloned() else {
                    return true;
                };
                let mut layers = self.layers();
                layers.push(name);
                self.change(cx, move |file| {
                    file.set_theme_layers(&layers);
                    true
                });
            }
            Control::DropLayer(index) => {
                let mut layers = self.layers();
                if *index < layers.len() {
                    layers.remove(*index);
                    self.change(cx, move |file| {
                        file.set_theme_layers(&layers);
                        true
                    });
                }
            }
            Control::ShiftLayer(index, up) => {
                let mut layers = self.layers();
                let other = if *up {
                    index.checked_sub(1)
                } else {
                    Some(index + 1)
                };
                if let Some(other) = other.filter(|other| *other < layers.len()) {
                    layers.swap(*index, other);
                    self.change(cx, move |file| {
                        file.set_theme_layers(&layers);
                        true
                    });
                }
            }
            _ => return false,
        }
        true
    }

    /// Brings the card at `place` of the page into sight if it is the card
    /// of `item` that was asked for.
    fn seek(&mut self, item: &str, place: usize) {
        if self.declared.seek.as_deref() == Some(item) {
            self.declared.scroll.scroll_to_item(place);
            self.declared.seek = None;
        }
    }

    /// The themes stacked over the base theme.
    fn layers(&self) -> Vec<String> {
        let config = self.config.as_ref();
        config
            .map(|config| config.theme_layers.clone())
            .unwrap_or_default()
    }

    /// The keys `owner` declares that the file holds.
    fn declared_keys(&self, owner: &Owner) -> Vec<String> {
        let Some(values) = self.config.as_ref().and_then(|config| owner.values(config)) else {
            return Vec::new();
        };
        let declared = |key: &String| {
            let row = RowKey {
                owner: owner.clone(),
                key: key.clone(),
            };
            self.declaration(&row).is_some()
        };
        values.keys().filter(|key| declared(key)).cloned().collect()
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
                self.write_declared(row, Some((!on).into()), cx);
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
                let now = shown.as_ref().and_then(OptionDecl::number);
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
                Self::open_at(section.title(), Some(row.owner.item()), cx);
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
                    let list = cx.new(|cx| DeclaredList::new(pairs, cx));
                    self.listen(row, &list, cx);
                    Held::List(list)
                }
                Kind::Font => {
                    let fonts = cx.new(FontPicker::new);
                    self.listen(row, &fonts, cx);
                    Held::Fonts(fonts)
                }
                _ => {
                    let field = cx.new(|cx| DeclaredField::new(placeholder.into(), cx));
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
                    Err(error) => {
                        if let Some(decl) = this.declaration(&row) {
                            this.refuse(&decl, error, cx);
                        }
                    }
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
        config: &Config,
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
            cards.push(self.declared_card(
                owner,
                extension.id.clone().into(),
                &extension.settings,
                &[],
                remark,
                &affects,
                config,
                window,
                cx,
            ));
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
            cards.push(self.declared_card(
                owner,
                title.into(),
                set,
                problems,
                lacking,
                &affects,
                config,
                window,
                cx,
            ));
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
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut stacked = Vec::new();
        let count = config.theme_layers.len();
        for (index, name) in config.theme_layers.iter().enumerate() {
            let mut buttons = Vec::new();
            if index > 0 {
                let up = Control::ShiftLayer(index, true);
                buttons.push(self.control(up, Face::Glyph(ICON_UP), window, cx));
            }
            if index + 1 < count {
                let down = Control::ShiftLayer(index, false);
                buttons.push(self.control(down, Face::Glyph(ICON_DOWN), window, cx));
            }
            let remove = Control::DropLayer(index);
            buttons.push(self.control(remove, Face::Glyph(ICON_REMOVE), window, cx));
            stacked.push(chip(&theme, format!("{}. {name}", index + 1), buttons));
        }
        let base = config.theme.as_deref();
        let mut offers = Vec::new();
        for index in 0..self.themes.len() {
            let name = self.themes[index].clone();
            if Some(name.as_str()) == base || config.theme_layers.contains(&name) {
                continue;
            }
            let face = Face::Chip {
                text: super::theme_title(&name, language),
                selected: true,
            };
            offers.push(self.control(Control::AddLayer(index), face, window, cx));
        }
        let about = "Themes stacked over the theme, in order: each changes only what it sets, and the last one wins.";
        let mut layers =
            card(&theme, language.say("Layers")).child(note(&theme, language.say(about)));
        layers = if stacked.is_empty() {
            layers.child(note(
                &theme,
                language.say("Nothing is stacked over the theme."),
            ))
        } else {
            layers.child(wrap(&theme).children(stacked))
        };
        if !offers.is_empty() {
            layers = layers
                .child(caption(&theme, language.say("Add a layer")))
                .child(wrap(&theme).children(offers));
        }
        layers.into_any_element()
    }

    /// A card of the settings `set` declares for `owner`: each row with
    /// its control, what it affects as `affects` names it, and Reset where
    /// the file sets it; Reset all over them. `remark` is said under the
    /// settings, such as that there is nothing to set, and `problems` are
    /// the declarations left out.
    #[allow(clippy::too_many_arguments, reason = "the parts of one card")]
    fn declared_card(
        &mut self,
        owner: Owner,
        title: SharedString,
        set: &OptionSet,
        problems: &[String],
        remark: Option<&'static str>,
        affects: &dyn Fn(&OptionDecl) -> Vec<String>,
        config: &Config,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let empty = toml::Table::new();
        let written = owner.values(config).unwrap_or(&empty).clone();
        let resolved = set.resolve_table(&written).values;
        let any_written = set.decls.iter().any(|decl| written.contains_key(&decl.key));
        let reset_all = any_written.then(|| {
            let face = Face::Chip {
                text: language.say("Reset all").into(),
                selected: false,
            };
            self.control(Control::ResetAll(owner.clone()), face, window, cx)
        });
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(theme.spacing.small)
            .child(super::heading(&theme, title))
            .children(reset_all);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.small + theme.spacing.tiny)
            .child(header);
        let mut group: Option<String> = None;
        for decl in &set.decls {
            if !OptionSet::visible(decl, &resolved) {
                continue;
            }
            let section = decl.section.as_ref().map(|section| section.text(language));
            if section.is_some() && section != group {
                group.clone_from(&section);
                body = body.children(section.map(|section| caption(&theme, section)));
            }
            let row = RowKey {
                owner: owner.clone(),
                key: decl.key.clone(),
            };
            let line = self.declared_row(
                &row,
                decl,
                written.get(&decl.key),
                resolved.get(&decl.key),
                affects(decl),
                window,
                cx,
            );
            body = body.child(line);
        }
        body = body.children(remark.map(|remark| note(&theme, language.say(remark))));
        if !problems.is_empty() {
            let left_out = language.say("Some settings are declared wrongly and are left out:");
            body = body.child(note(&theme, left_out));
            for problem in problems {
                body = body.child(note(&theme, problem.clone()).text_size(small_text(&theme)));
            }
        }
        let light = match &mut self.declared.lit {
            Some((item, light)) if item == owner.item() => light.value(window).clamp(0., 1.),
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
    /// control, what it does and what it affects.
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
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(line)
            .children(description)
            .children(affects)
            .children(below)
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
                (
                    Some(
                        wrap(&theme)
                            .justify_end()
                            .children(chips)
                            .into_any_element(),
                    ),
                    None,
                )
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
                    return (
                        Some(
                            wrap(&theme)
                                .justify_end()
                                .children(chips)
                                .into_any_element(),
                        ),
                        None,
                    );
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
                let shown = resolved.and_then(OptionDecl::number).map(|number| {
                    let mut plain = decl.clone();
                    plain.unit = None;
                    plain.shown_number(number)
                });
                let field = self.field(row, decl, shown.unwrap_or_default(), cx);
                let [less, more] = [(false, ICON_LESS), (true, ICON_MORE)].map(|(up, glyph)| {
                    self.control(deed(Deed::Step(up)), Face::Glyph(glyph), window, cx)
                });
                let unit = decl.unit.clone().filter(|unit| !unit.is_empty());
                let stepper = div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.tiny)
                    .child(less)
                    .child(div().w(px(NUMBER_WIDTH)).flex().child(field))
                    .child(more)
                    .children(unit.map(|unit| {
                        div()
                            .text_color(theme.colors.on_surface_variant)
                            .child(unit)
                    }));
                (Some(stepper.into_any_element()), None)
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
                let track = div().w(px(SLIDER_WIDTH)).flex().child(slider);
                (Some(track.into_any_element()), None)
            }
            Kind::Text => {
                let text = resolved.map(text_of).unwrap_or_default();
                let field = self.field(row, decl, text, cx);
                (
                    Some(
                        div()
                            .w(px(FIELD_WIDTH))
                            .flex()
                            .child(field)
                            .into_any_element(),
                    ),
                    None,
                )
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
            Kind::List | Kind::Pairs => {
                let items = match (decl.kind, resolved) {
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
    fn new(placeholder: SharedString, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| TextField::new(placeholder, cx));
        cx.observe(&field, |this: &mut Self, field, cx| {
            let text = field.read(cx).text();
            if text == this.written {
                return;
            }
            this.typed = true;
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
    fn new(pairs: bool, cx: &mut Context<Self>) -> Self {
        Self {
            pairs,
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
        self.written = items;
        cx.notify();
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
        if index < self.rows.len() {
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
            let remove = list_button(&theme, ("list-remove", index), ICON_REMOVE)
                .on_click(cx.listener(move |this, _, _, cx| this.remove(index, cx)));
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
                    .child(remove),
            );
        }
        let count = self.rows.len();
        let add = div()
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
            }));
        div()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .children(rows)
            .child(add)
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
