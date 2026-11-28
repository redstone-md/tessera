// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Search page of the settings, as the Search settings of KDE Plasma
//! have one: widget `runner-settings`, drawn by the settings in their
//! section Search. It lists every runner of the launcher with what the
//! help runner says of it (what it is called, the short name and the
//! colon that ask it alone, what it answers and examples), switches each
//! on or off and puts them in an order, all of which is written as the
//! option `runners` of every launcher (and of the runners that name their
//! own). It also edits the web shortcuts of the launchers, the fetching of
//! exchange rates and the recent documents, and clears the history the
//! launchers rank answers by. The runners of a single launcher are set on
//! its card in the Widgets section, which the page opens.
//!
//! What it shows is read from the configuration the shell holds; what is
//! changed is written to the file off the thread of the interface, with
//! the rest of the file as it was, and the shell takes it over at once.

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, AsyncApp, Context, Div, Entity, FontWeight, Global, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, div, px,
};
use toml_edit::{InlineTable, Value};

use super::settings_parts::Feedback;
use crate::config::Config;
use crate::config::edit::ConfigFile;
use crate::field::TextField;
use crate::i18n::Language;
use crate::module::{Module, WidgetContext, WidgetRegistry};
use crate::modules::Settings;
use crate::options::{Choice, Label};
use crate::runners::{ALL, Answer, History, Query, RunnerOptions, Runners, fill};
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, one_line, state_layer, switch};

/// The name of the page.
const NAME: &str = "runner-settings";

/// The widgets whose runners the page sets: every launcher, and the
/// runners that name runners of their own.
const LAUNCHER: &str = "launcher";
const RUNNER: &str = "runner";

/// The runner that tells of the others.
const HELP: &str = "help";

/// The runner whose answers the option `settings` takes away.
const SETTINGS_RUNNER: &str = "settings";

/// The runner of the web shortcuts, and the option that adds some.
const WEB: &str = "web";

const ICON_UP: &str = "\u{e70e}";
const ICON_DOWN: &str = "\u{e70d}";
const ICON_REMOVE: &str = "\u{e74d}";
const ICON_ADD: &str = "\u{e710}";

/// The side of a round button and the height of a chip of the page.
const BUTTON: f32 = 28.;
const CHIP: f32 = 30.;

/// How wide the keyword of a web shortcut stands.
const KEYWORD_WIDTH: f32 = 120.;

pub struct RunnerSettingsModule;

impl Module for RunnerSettingsModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(NAME, |context, cx| {
            // The settings name a runner to light, where they name one.
            let lit = context.text("item").map(str::to_owned);
            cx.new(|cx| RunnerSettings::new(lit, cx)).into()
        })
    }
}

/// What the help runner tells of a runner, in the language of the user
/// interface.
#[derive(Clone, Debug, PartialEq)]
pub struct RunnerAbout {
    /// Its name, one of [`ALL`].
    pub id: &'static str,
    /// What it is called, with the short name and the colon that ask it
    /// alone, as `Calculator (calc:)`.
    pub title: String,
    /// What it answers, after examples of how it is asked where it has a
    /// syntax of its own.
    pub detail: String,
}

/// What the help runner tells of every runner, asked once for each
/// language of the user interface; until it has answered, each runner is
/// known by its name.
#[derive(Default)]
pub struct RunnerCatalog {
    /// The language the help was asked in.
    language: Option<String>,
    runners: Vec<RunnerAbout>,
}

impl Global for RunnerCatalog {}

impl RunnerCatalog {
    /// Has the help runner be asked about every runner, unless it was
    /// asked in the language now in use. It answers on the background
    /// executor; the catalog changes when it has.
    pub fn read(cx: &mut App) {
        let language = Language::of(cx);
        let code = language.code().to_owned();
        let catalog = cx.default_global::<Self>();
        if catalog.language.as_deref() == Some(code.as_str()) {
            return;
        }
        catalog.language = Some(code);
        // Every runner, so that the help tells of every runner; none of
        // them fetches anything for a question mark.
        let mut options = toml::Table::new();
        let all: Vec<toml::Value> = ALL.iter().map(|id| (*id).into()).collect();
        options.insert("runners".to_owned(), toml::Value::Array(all));
        options.insert("currency".to_owned(), false.into());
        let context = WidgetContext {
            display: None,
            options,
        };
        let runners = Runners::of_widget(&context, RunnerOptions::default());
        let Some(help) = runners.ids().iter().position(|id| *id == HELP) else {
            return;
        };
        let answers = runners.ask(Query::new("?", language), cx.background_executor());
        cx.spawn(async move |cx: &mut AsyncApp| {
            let mut told = Vec::new();
            while let Ok((index, said)) = answers.recv().await {
                if index == help {
                    told = said;
                    break;
                }
            }
            cx.update(|cx| cx.global_mut::<Self>().runners = described(&told))
                .ok();
        })
        .detach();
    }

    /// What is told of the runner `id`.
    fn about<'a>(&'a self, id: &str) -> Option<&'a RunnerAbout> {
        self.runners.iter().find(|about| about.id == id)
    }

    /// The runners as choices of the option `runners`, each with what the
    /// help calls it, or its name before the help has answered.
    pub fn choices(cx: &App) -> Vec<Choice> {
        let catalog = cx.try_global::<Self>();
        { ALL.iter() }
            .map(|id| Choice {
                value: (*id).into(),
                label: { catalog.and_then(|catalog| catalog.about(id)) }
                    .map(|about| Label::Plain(about.title.clone())),
                overlay: None,
            })
            .collect()
    }
}

/// What the help answered, runner by runner: it tells of them in the
/// order of [`ALL`]. Where it did not tell of each, every runner is known
/// by its name.
fn described(told: &[Answer]) -> Vec<RunnerAbout> {
    if told.len() != ALL.len() {
        return { ALL.iter() }
            .map(|id| RunnerAbout {
                id,
                title: (*id).to_owned(),
                detail: String::new(),
            })
            .collect();
    }
    { ALL.iter().zip(told) }
        .map(|(id, answer)| RunnerAbout {
            id,
            title: answer.title.clone(),
            detail: answer.detail.clone(),
        })
        .collect()
}

/// A runner of the list, switched on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Row {
    id: &'static str,
    on: bool,
}

/// The runners as the option `runners` lists them: those it names, on,
/// in its order, then the others, off; every runner on, in the order
/// they are asked, without the option. A name of no runner is left out.
fn rows_of(named: Option<&[String]>) -> Vec<Row> {
    let Some(named) = named else {
        return ALL.iter().map(|id| Row { id, on: true }).collect();
    };
    let mut rows: Vec<Row> = { named.iter() }
        .filter_map(|name| ALL.iter().find(|id| **id == name.as_str()))
        .map(|id| Row { id, on: true })
        .collect();
    rows.dedup_by_key(|row| row.id);
    for id in ALL {
        if !rows.iter().any(|row| row.id == id) {
            rows.push(Row { id, on: false });
        }
    }
    rows
}

/// What the option `runners` is written as for `rows`: the runners on, in
/// their order; nothing where every runner is on in the order they are
/// asked, which the launcher takes without the option.
fn written(rows: &[Row]) -> Option<Vec<String>> {
    let every =
        rows.len() == ALL.len() && { rows.iter().zip(ALL) }.all(|(row, id)| row.on && row.id == id);
    if every {
        return None;
    }
    let on = rows.iter().filter(|row| row.on);
    Some(on.map(|row| row.id.to_owned()).collect())
}

/// `rows` with the one at `index` a place nearer the top, or the bottom.
fn moved(rows: &[Row], index: usize, up: bool) -> Vec<Row> {
    let mut rows = rows.to_vec();
    let other = if up {
        index.checked_sub(1)
    } else {
        Some(index + 1)
    };
    if let Some(other) = other.filter(|other| *other < rows.len() && index < rows.len()) {
        rows.swap(index, other);
    }
    rows
}

/// The web shortcuts of the option `web`: the keywords with addresses
/// that are text, in the order of their keywords.
fn shortcuts_of(web: Option<&toml::Value>) -> Vec<(String, String)> {
    let table = web.and_then(toml::Value::as_table);
    { table.into_iter().flatten() }
        .filter_map(|(keyword, address)| Some((keyword.clone(), address.as_str()?.to_owned())))
        .collect()
}

/// A web shortcut as the web runner takes it: a keyword without spaces or
/// a colon, which it compares in small letters, and an address with `{}`
/// for what is searched. What is wrong is said in English.
fn shortcut(keyword: &str, address: &str) -> Result<(String, String), &'static str> {
    let keyword = keyword.trim().to_lowercase();
    let address = address.trim();
    if keyword.is_empty() || keyword.contains(|c: char| c.is_whitespace() || c == ':') {
        return Err("A keyword is one word without a colon.");
    }
    if !address.contains("{}") {
        return Err("The address needs {} where what is searched goes.");
    }
    Ok((keyword, address.to_owned()))
}

/// The option `web` written for `shortcuts`; nothing for none.
fn web_value(shortcuts: &[(String, String)]) -> Option<Value> {
    if shortcuts.is_empty() {
        return None;
    }
    let mut table = InlineTable::new();
    for (keyword, address) in shortcuts {
        table.insert(keyword, address.as_str().into());
    }
    table.fmt();
    Some(Value::InlineTable(table))
}

/// The launchers and runners the layout places, each with the item of
/// `Settings::open_at` that opens its card and what the page calls it.
fn instances(config: &Config, bar: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let named = |widget: &str| widget == LAUNCHER || widget == RUNNER;
    for (number, surface) in config.surfaces.iter().enumerate() {
        let slots = [
            ("start", &surface.start),
            ("center", &surface.center),
            ("end", &surface.end),
        ];
        for (slot, widgets) in slots {
            for (index, spec) in widgets.iter().enumerate() {
                if named(&spec.widget) {
                    found.push((
                        format!("surface/{number}/{slot}/{index}"),
                        format!("{} \u{b7} {bar} {}", spec.widget, number + 1),
                    ));
                }
            }
        }
    }
    for popup in &config.popups {
        for (index, spec) in popup.widgets().enumerate() {
            if named(&spec.widget) {
                found.push((
                    format!("popup/{}/{index}", popup.name),
                    format!("{} \u{b7} {}", spec.widget, popup.name),
                ));
            }
        }
    }
    found
}

/// An option of the launchers to write, or to take out with `None`.
type Edit = (&'static str, Option<Value>);

/// How a control of a page looks.
pub(super) enum Face {
    /// A round button with an icon.
    Glyph(&'static str),
    /// A word, filled while it is chosen.
    Chip(SharedString, bool),
}

/// A control of a page of the settings, under the pointer in `state`.
pub(super) fn control(
    theme: &Theme,
    id: impl Into<gpui::ElementId>,
    face: Face,
    state: crate::motion::InteractionState,
) -> Stateful<Div> {
    let colors = theme.colors;
    let base = div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.shape.full);
    match face {
        Face::Glyph(glyph) => base
            .size(px(BUTTON))
            .bg(state_layer(theme, colors.surface_container, state))
            .text_color(colors.on_surface_variant)
            .child(icon(theme, glyph).text_size(theme.typography.label_size)),
        Face::Chip(text, chosen) => {
            let (ground, color) = match chosen {
                true => (colors.secondary_container, colors.primary),
                false => (colors.surface_container, colors.on_surface),
            };
            base.h(px(CHIP))
                .px(theme.spacing.small + theme.spacing.tiny)
                .bg(state_layer(theme, ground, state))
                .text_color(color)
                .child(one_line(text))
        }
    }
}

/// A card of a page of the settings, under its title.
pub(super) fn page_card(theme: &Theme, title: impl Into<SharedString>) -> Div {
    div()
        .p(theme.spacing.medium)
        .flex()
        .flex_col()
        .gap(theme.spacing.small + theme.spacing.tiny)
        .rounded(theme.shape.large)
        .bg(theme.colors.surface_container_highest)
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.primary)
                .child(one_line(title)),
        )
}

/// A remark of a page in the quieter color.
pub(super) fn page_note(theme: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(theme.typography.label_size)
        .text_color(theme.colors.on_surface_variant)
        .child(text.into())
}

/// What the page shows of the configuration.
struct Shown {
    rows: Vec<Row>,
    shortcuts: Vec<(String, String)>,
    /// Whether the first launcher fetches exchange rates, and lists the
    /// recent documents while nothing is typed.
    currency: bool,
    recent: bool,
    /// The launchers and runners placed, as `instances` gives them.
    places: Vec<(String, String)>,
}

/// The page.
struct RunnerSettings {
    /// The runner the settings asked to light.
    lit: Option<String>,
    /// The runners as last changed here, until the shell holds what was
    /// written.
    pending: Option<Vec<Row>>,
    keyword: Entity<TextField>,
    address: Entity<TextField>,
    /// What is wrong with what was last asked, or why it was not written.
    failure: Option<SharedString>,
    /// Whether the history was cleared since the page was opened.
    cleared: bool,
    feedback: Feedback,
    _catalog: Subscription,
}

impl RunnerSettings {
    fn new(lit: Option<String>, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let language = Language::of(cx);
        RunnerCatalog::read(cx);
        let keyword = language.say("Keyword");
        let address = language.say("Address with {}");
        Self {
            lit,
            pending: None,
            keyword: cx.new(|cx| TextField::new(keyword.into(), cx)),
            address: cx.new(|cx| TextField::new(address.into(), cx)),
            failure: None,
            cleared: false,
            feedback: Feedback::new(motion),
            _catalog: cx.observe_global::<RunnerCatalog>(|_, cx| cx.notify()),
        }
    }

    /// The runners of the launchers, as the configuration or the last
    /// change here has them. Those the option `settings` leaves out are
    /// off.
    fn rows(&self, config: &Config) -> Vec<Row> {
        if let Some(pending) = &self.pending {
            return pending.clone();
        }
        let named = config.option(LAUNCHER, "runners").and_then(texts);
        let mut rows = rows_of(named.as_deref());
        if flag(config, "settings", true) == Some(false) {
            for row in rows.iter_mut().filter(|row| row.id == SETTINGS_RUNNER) {
                row.on = false;
            }
        }
        rows
    }

    /// What the page shows of `config`, read once as it is drawn.
    fn shown(&self, config: &Config, language: Language) -> Shown {
        Shown {
            rows: self.rows(config),
            shortcuts: shortcuts_of(config.option(LAUNCHER, WEB)),
            currency: flag(config, "currency", true).unwrap_or(true),
            recent: flag(config, "recent", false).unwrap_or(false),
            places: instances(config, language.say("Bar")),
        }
    }

    /// Writes `rows` as the runners of the launchers; a runner switched
    /// on that `settings = false` left out is let in again.
    fn take_rows(&mut self, rows: Vec<Row>, cx: &mut Context<Self>) {
        let value = written(&rows).map(|names| {
            let list: toml_edit::Array = names.iter().map(String::as_str).collect();
            Value::Array(list)
        });
        let mut edits: Vec<Edit> = vec![("runners", value)];
        let settings_on = rows.iter().any(|row| row.id == SETTINGS_RUNNER && row.on);
        if settings_on {
            edits.push(("settings", None));
        }
        self.pending = Some(rows);
        self.write(edits, cx);
    }

    /// Writes `edits` to every launcher, and to the runners that already
    /// set an option of them, away from the thread of the interface.
    fn write(&mut self, edits: Vec<Edit>, cx: &mut Context<Self>) {
        self.failure = None;
        let written = cx.background_executor().spawn(async move {
            ConfigFile::change(|file| {
                let config = file.config()?;
                let mut placed = false;
                for (key, value) in edits {
                    placed |= file.set_option(LAUNCHER, key, value.clone());
                    if config.option(RUNNER, key).is_some() {
                        placed |= file.set_option(RUNNER, key, value);
                    }
                }
                match placed {
                    true => Ok(()),
                    false => Err("the layout has no launcher".to_owned()),
                }
            })
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let written = written.await;
            this.update(cx, |this, cx| {
                if let Err(error) = written {
                    report!("the runners are not written: {error}");
                    this.failure = Some(error.into());
                    this.pending = None;
                    cx.notify();
                    return;
                }
                // What was changed here is shown until the shell holds it.
                let page = cx.entity().downgrade();
                cx.defer(move |cx| {
                    Shell::config_changed(cx);
                    page.update(cx, |this, cx| {
                        this.pending = None;
                        cx.notify();
                    })
                    .ok();
                });
            })
            .ok();
        })
        .detach();
    }

    /// Adds the web shortcut typed into the two fields.
    fn add_shortcut(&mut self, cx: &mut Context<Self>) {
        let language = Language::of(cx);
        let keyword = self.keyword.read(cx).text();
        let address = self.address.read(cx).text();
        let (keyword, address) = match shortcut(&keyword, &address) {
            Ok(shortcut) => shortcut,
            Err(error) => {
                self.failure = Some(language.say(error).into());
                cx.notify();
                return;
            }
        };
        let config = Shell::config(cx);
        let mut shortcuts = shortcuts_of(config.option(LAUNCHER, WEB));
        shortcuts.retain(|(known, _)| *known != keyword);
        shortcuts.push((keyword, address));
        self.keyword.update(cx, |field, cx| field.clear(cx));
        self.address.update(cx, |field, cx| field.clear(cx));
        self.write(vec![(WEB, web_value(&shortcuts))], cx);
    }

    fn remove_shortcut(&mut self, keyword: &str, cx: &mut Context<Self>) {
        let config = Shell::config(cx);
        let mut shortcuts = shortcuts_of(config.option(LAUNCHER, WEB));
        shortcuts.retain(|(known, _)| known != keyword);
        self.write(vec![(WEB, web_value(&shortcuts))], cx);
    }

    /// A switch of `key`, which is `on`, that does `flip` when clicked.
    fn switch(
        &mut self,
        key: (&'static str, usize),
        on: bool,
        flip: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let state = self.feedback.state(key, window);
        let travel = if on { 1. } else { 0. };
        let element = switch(&theme, (key.0, key.1), travel, state)
            .on_click(cx.listener(move |this, _, _, cx| flip(this, cx)));
        Feedback::track(element, key, cx, |this: &mut Self| &mut this.feedback)
    }

    /// A button of `key`, looking as `face` says, that does `act`.
    fn button(
        &mut self,
        key: (&'static str, usize),
        face: Face,
        act: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let state = self.feedback.state(key, window);
        let element = control(&theme, (key.0, key.1), face, state)
            .on_click(cx.listener(move |this, _, _, cx| act(this, cx)));
        Feedback::track(element, key, cx, |this: &mut Self| &mut this.feedback)
    }

    /// The runners, each with its switch and the buttons that move it.
    fn runners(&mut self, shown: &Shown, window: &Window, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let rows = shown.rows.clone();
        let count = rows.len();
        let mut card = page_card(&theme, language.say("Runners")).child(page_note(
            &theme,
            language.say(
                "A runner switched off is not asked. Answers are ranked by how sure each runner is; the order is kept in the file.",
            ),
        ));
        for (index, row) in rows.iter().enumerate() {
            let about =
                { cx.try_global::<RunnerCatalog>() }.and_then(|catalog| catalog.about(row.id));
            let title = about.map_or_else(|| row.id.to_owned(), |about| about.title.clone());
            let detail = about.map(|about| about.detail.clone()).unwrap_or_default();
            let toggled = rows.clone();
            let on = self.switch(
                ("runner-on", index),
                row.on,
                move |this, cx| {
                    let mut rows = toggled.clone();
                    rows[index].on = !rows[index].on;
                    this.take_rows(rows, cx);
                },
                window,
                cx,
            );
            let mut buttons = Vec::new();
            for (up, glyph, key) in [
                (true, ICON_UP, "runner-up"),
                (false, ICON_DOWN, "runner-down"),
            ] {
                let possible = if up { index > 0 } else { index + 1 < count };
                if !possible {
                    buttons.push(div().size(px(BUTTON)).into_any_element());
                    continue;
                }
                let order = rows.clone();
                let button = self.button(
                    (key, index),
                    Face::Glyph(glyph),
                    move |this, cx| this.take_rows(moved(&order, index, up), cx),
                    window,
                    cx,
                );
                buttons.push(button.into_any_element());
            }
            let lit = self.lit.as_deref() == Some(row.id);
            let text = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(match row.on {
                            true => theme.colors.on_surface,
                            false => theme.colors.on_surface_variant,
                        })
                        .child(one_line(title)),
                )
                .children((!detail.is_empty()).then(|| page_note(&theme, detail)));
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .px(theme.spacing.small)
                    .py(theme.spacing.tiny)
                    .rounded(theme.shape.medium)
                    .when(lit, |row| row.bg(theme.colors.secondary_container))
                    .child(on)
                    .child(text)
                    .children(buttons),
            );
        }
        let places = shown.places.clone();
        if !places.is_empty() {
            let mut chips = div().flex().flex_wrap().gap(theme.spacing.tiny);
            for (index, (item, label)) in places.into_iter().enumerate() {
                let chip = self.button(
                    ("instance", index),
                    Face::Chip(label.into(), false),
                    move |_, cx| {
                        let item = item.clone();
                        cx.defer(move |cx| Settings::open_at("widgets", Some(&item), cx));
                    },
                    window,
                    cx,
                );
                chips = chips.child(chip);
            }
            card = card
                .child(page_note(
                    &theme,
                    language.say(
                        "These are the runners of every launcher. One launcher alone is set on its card:",
                    ),
                ))
                .child(chips);
        }
        card
    }

    /// The web shortcuts of the launchers, and the fields that add one.
    fn web(&mut self, shown: &Shown, window: &Window, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let builtin = { cx.try_global::<RunnerCatalog>() }
            .and_then(|catalog| catalog.about(WEB))
            .map(|about| about.detail.clone());
        let mut card = page_card(&theme, language.say("Web shortcuts")).children(
            builtin
                .map(|builtin| page_note(&theme, fill(language.say("Built in: {}"), &[&builtin]))),
        );
        let shortcuts = shown.shortcuts.clone();
        for (index, (keyword, address)) in shortcuts.into_iter().enumerate() {
            let named = div()
                .flex_none()
                .w(px(KEYWORD_WIDTH))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.primary)
                .child(one_line(format!("{keyword}:")));
            let remove = self.button(
                ("web-remove", index),
                Face::Glyph(ICON_REMOVE),
                move |this, cx| this.remove_shortcut(&keyword, cx),
                window,
                cx,
            );
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .child(named)
                    .child(div().flex_1().min_w_0().child(one_line(address)))
                    .child(remove),
            );
        }
        let add = self.button(
            ("web-add", 0),
            Face::Glyph(ICON_ADD),
            |this, cx| this.add_shortcut(cx),
            window,
            cx,
        );
        let field = |field: &Entity<TextField>| {
            div()
                .h(theme.launcher.field_height)
                .min_w_0()
                .flex()
                .items_center()
                .child(field.clone())
        };
        card.child(
            div()
                .flex()
                .items_center()
                .gap(theme.spacing.small)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "enter" {
                        this.add_shortcut(cx);
                    }
                }))
                .child(field(&self.keyword).w(px(KEYWORD_WIDTH)).flex_none())
                .child(field(&self.address).flex_1())
                .child(add),
        )
        .child(page_note(
            &theme,
            language
                .say("A keyword of your own takes the place of a built-in one of the same name."),
        ))
    }

    /// A switch of an option of the launchers with its label.
    fn option_switch(
        &mut self,
        key: &'static str,
        label: &'static str,
        on: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let toggle = self.switch(
            (key, 0),
            on,
            move |this, cx| this.write(vec![(key, Some((!on).into()))], cx),
            window,
            cx,
        );
        div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(one_line(language.say(label))),
            )
            .child(toggle)
    }
}

/// The option `key` of the first launcher as a flag; `default` where it
/// does not say.
fn flag(config: &Config, key: &str, default: bool) -> Option<bool> {
    Some(match config.option(LAUNCHER, key) {
        Some(value) => value.as_bool()?,
        None => default,
    })
}

/// A list of texts of the file.
fn texts(value: &toml::Value) -> Option<Vec<String>> {
    let items = value.as_array()?;
    items
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
}

impl Render for RunnerSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let shown = self.shown(Shell::config(cx), language);
        let runners = self.runners(&shown, window, cx);
        let web = self.web(&shown, window, cx);
        let currency = shown.currency;
        let currency = self.option_switch("currency", "Fetch exchange rates", currency, window, cx);
        let units = page_card(&theme, language.say("Exchange rates"))
            .child(currency)
            .child(page_note(
                &theme,
                language.say(
                    "The daily rates of the European Central Bank, fetched again when a conversion is asked and they are older than 12 hours; rates kept from before are still used.",
                ),
            ));
        let recent = shown.recent;
        let recent = self.option_switch(
            "recent",
            "List the documents opened lately while nothing is typed",
            recent,
            window,
            cx,
        );
        let documents = page_card(&theme, language.say("Recent documents")).child(recent);
        let clear = self.button(
            ("clear-history", 0),
            Face::Chip(language.say("Clear history").into(), false),
            |this, cx| {
                History::clear(cx);
                this.cleared = true;
                cx.notify();
            },
            window,
            cx,
        );
        let said = match self.cleared {
            true => "The history is cleared.",
            false => {
                "What is chosen in the launchers, the runner and the Run dialog ranks higher the next time; it is kept in launcher-history.toml."
            }
        };
        let history = page_card(&theme, language.say("History"))
            .child(page_note(&theme, language.say(said)))
            .child(div().flex().child(clear));
        let failure = self.failure.clone().map(|failure| {
            div()
                .px(theme.spacing.small)
                .text_color(theme.colors.primary)
                .child(failure)
        });
        div()
            .id(NAME)
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .text_color(theme.colors.on_surface)
            .children(failure)
            .child(runners)
            .child(web)
            .child(units)
            .child(documents)
            .child(history)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(rows: &[Row]) -> Vec<(&str, bool)> {
        rows.iter().map(|row| (row.id, row.on)).collect()
    }

    #[test]
    fn without_the_option_every_runner_is_on_in_its_order() {
        let rows = rows_of(None);
        assert_eq!(rows.len(), ALL.len());
        assert!(rows.iter().all(|row| row.on));
        assert_eq!(written(&rows), None);
    }

    #[test]
    fn the_runners_named_come_first_and_the_others_are_off() {
        let named = [
            "web".to_owned(),
            "nothing".to_owned(),
            "calculator".to_owned(),
        ];
        let rows = rows_of(Some(&named));
        assert_eq!(rows.len(), ALL.len());
        assert_eq!(
            &names(&rows)[..3],
            [("web", true), ("calculator", true), ("units", false)]
        );
        assert_eq!(
            written(&rows),
            Some(vec!["web".to_owned(), "calculator".to_owned()])
        );
    }

    #[test]
    fn a_runner_is_moved_within_the_list() {
        let rows = rows_of(None);
        let down = moved(&rows, 0, false);
        assert_eq!(down[0].id, ALL[1]);
        assert_eq!(down[1].id, ALL[0]);
        assert_eq!(written(&down).map(|list| list.len()), Some(ALL.len()));
        assert_eq!(moved(&rows, 0, true), rows);
        assert_eq!(moved(&rows, ALL.len() - 1, false), rows);
        assert_eq!(written(&moved(&down, 1, true)), None);
    }

    #[test]
    fn a_web_shortcut_needs_a_word_and_a_place_for_the_search() {
        assert_eq!(
            shortcut(" SO ", " https://stackoverflow.com/search?q={} "),
            Ok((
                "so".to_owned(),
                "https://stackoverflow.com/search?q={}".to_owned()
            ))
        );
        assert!(shortcut("two words", "https://x.org/?q={}").is_err());
        assert!(shortcut("a:b", "https://x.org/?q={}").is_err());
        assert!(shortcut("x", "https://x.org/").is_err());
        let web: toml::Value = toml::from_str::<toml::Table>(
            "web = { so = \"https://stackoverflow.com/search?q={}\", bad = 3 }",
        )
        .unwrap()["web"]
            .clone();
        let shortcuts = shortcuts_of(Some(&web));
        assert_eq!(shortcuts.len(), 1);
        let written = web_value(&shortcuts).unwrap().to_string();
        assert!(written.contains("so = \"https://stackoverflow.com/search?q={}\""));
        assert!(web_value(&[]).is_none());
    }

    #[test]
    fn the_help_is_read_in_the_order_of_the_runners() {
        let answer =
            |title: &str| Answer::new(title, "what", "", 0, crate::runners::AnswerAction::Nothing);
        let told: Vec<Answer> = ALL.iter().map(|id| answer(&id.to_uppercase())).collect();
        let about = described(&told);
        assert_eq!(about[3].id, ALL[3]);
        assert_eq!(about[3].title, ALL[3].to_uppercase());
        let partial = described(&told[..2]);
        assert_eq!(partial.len(), ALL.len());
        assert_eq!(partial[0].title, ALL[0]);
    }

    #[test]
    fn every_placed_launcher_can_be_found() {
        let config: Config = toml::from_str(
            "[[surface]]\nedge = \"top\"\nthickness = 40\nend = [\"clock\", \"launcher\"]\n\
             [[popup]]\nname = \"start\"\nwidth = 1\nheight = 1\ncontent = [\"apps\", \"runner\"]\n",
        )
        .unwrap();
        let found = instances(&config, "Bar");
        assert_eq!(
            found,
            [
                (
                    "surface/0/end/1".to_owned(),
                    "launcher \u{b7} Bar 1".to_owned()
                ),
                ("popup/start/1".to_owned(), "runner \u{b7} start".to_owned()),
            ]
        );
    }
}
