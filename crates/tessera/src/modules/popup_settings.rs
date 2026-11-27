// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The popups of the configuration, as a page of the settings: widget
//! `popup-settings`, drawn in their section Widgets. It lists each
//! `[[popup]]` of the file with where it opens, its size and its widgets.
//! A popup is added, renamed (and so is whatever opens it) and removed;
//! its widgets are added from those Tessera registers, moved, taken out,
//! and each opens its card of options in the Widgets section. A popup a
//! module brings is copied into the file to be changed there, and is used
//! as it is otherwise.
//!
//! What it shows is read from the configuration the shell holds; what is
//! changed is written to the file off the thread of the interface, with
//! the rest of the file and its comments as they were, and the shell
//! takes it over at once.

use gpui::{
    AppContext, AsyncApp, Context, Div, Entity, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement,
    Styled, WeakEntity, Window, div, px,
};
use toml_edit::Value;

use super::runner_settings::{Face, control, page_card, page_note};
use super::settings_parts::Feedback;
use crate::config::edit::{ConfigFile, PopupColumn, PopupWidget, popup_place_name};
use crate::config::{PopupConfig, PopupPlace};
use crate::field::TextField;
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::modules::Settings;
use crate::runners::fill;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, one_line, state_layer};

/// The name of the page.
const NAME: &str = "popup-settings";

/// The widget whose popup is the way back into the settings, which is
/// not taken away while no other popup holds one.
const SETTINGS_WIDGET: &str = "settings";

/// How much a step makes a popup wider or higher, and the least and the
/// most it can be made so.
const SIZE_STEP: f32 = 20.;
const SIZE_RANGE: (f32, f32) = (120., 2000.);

/// The size of a popup the page adds.
const NEW_SIZE: (f32, f32) = (360., 480.);

/// The places a popup opens at, with what the page calls them.
const PLACES: [(PopupPlace, &str); 3] = [
    (PopupPlace::Surface, "Next to the bar"),
    (PopupPlace::Center, "In the middle"),
    (PopupPlace::Top, "Near the top"),
];

const ICON_UP: &str = "\u{e70e}";
const ICON_DOWN: &str = "\u{e70d}";
const ICON_REMOVE: &str = "\u{e74d}";
const ICON_OPTIONS: &str = "\u{e713}";
const ICON_MINUS: &str = "\u{e738}";
const ICON_PLUS: &str = "\u{e710}";
const ICON_FOLDED: &str = "\u{e76c}";
const ICON_UNFOLDED: &str = "\u{e70d}";

pub struct PopupSettingsModule;

impl Module for PopupSettingsModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(NAME, |context, cx| {
            // The settings name a popup to unfold, where they name one.
            let open = context.text("item").and_then(popup_of_item);
            cx.new(|cx| PopupSettings::new(open, cx)).into()
        })
    }
}

/// The popup an item of the settings names: `popup/<name>/<i>`,
/// `popup/<name>` or the name alone.
fn popup_of_item(item: &str) -> Option<String> {
    let name = item.strip_prefix("popup/").unwrap_or(item);
    let name = match name.rsplit_once('/') {
        Some((name, index)) if index.parse::<usize>().is_ok() => name,
        _ => name,
    };
    (!name.is_empty()).then(|| name.to_owned())
}

/// A change of the popups of the file.
#[derive(Clone, Debug)]
enum Change {
    Add(PopupConfig),
    Remove(String),
    Rename(String, String),
    Field(String, &'static str, Option<Value>),
    Widgets(String, PopupColumn, Vec<PopupWidget>),
}

impl Change {
    /// Makes the change in `file`; whether it could be made.
    fn apply(self, file: &mut ConfigFile) -> bool {
        match self {
            Self::Add(popup) => file.add_popup(&popup),
            Self::Remove(name) => file.remove_popup(&name),
            Self::Rename(from, to) => file.rename_popup(&from, &to),
            Self::Field(name, key, value) => file.set_popup_field(&name, key, value),
            Self::Widgets(name, column, widgets) => file.set_popup_widgets(&name, column, &widgets),
        }
    }
}

/// Every widget of a column of `count`, but the one at `index`.
fn without(count: usize, index: usize) -> Vec<PopupWidget> {
    { (0..count).filter(|kept| *kept != index) }
        .map(PopupWidget::Kept)
        .collect()
}

/// Every widget of a column of `count`, the one at `index` a place nearer
/// the top, or the bottom.
fn swapped(count: usize, index: usize, up: bool) -> Vec<PopupWidget> {
    let mut order: Vec<usize> = (0..count).collect();
    let other = if up {
        index.checked_sub(1)
    } else {
        Some(index + 1)
    };
    if let Some(other) = other.filter(|other| *other < count && index < count) {
        order.swap(index, other);
    }
    order.into_iter().map(PopupWidget::Kept).collect()
}

/// Every widget of a column of `count`, and `widget` after them.
fn appended(count: usize, widget: &str) -> Vec<PopupWidget> {
    let mut order: Vec<PopupWidget> = (0..count).map(PopupWidget::Kept).collect();
    order.push(PopupWidget::New(widget.to_owned()));
    order
}

/// A width or a height a step larger, or smaller, within the range a
/// popup is made by steps.
fn stepped(length: f32, up: bool) -> f32 {
    let (least, most) = SIZE_RANGE;
    let step = if up { SIZE_STEP } else { -SIZE_STEP };
    ((length / SIZE_STEP).round() * SIZE_STEP + step).clamp(least, most)
}

/// Whether taking out the popup `name`, or only its widget at `widget` of
/// `column`, would leave no popup of `popups` holding the settings, which
/// would leave no way back into them.
fn takes_the_way_back(
    popups: &[PopupConfig],
    name: &str,
    widget: Option<(PopupColumn, usize)>,
) -> bool {
    // Whether `popup` holds the settings but at `skipped`.
    let holds = |popup: &PopupConfig, skipped: Option<(PopupColumn, usize)>| {
        let columns = [
            (PopupColumn::Content, &popup.content),
            (PopupColumn::Side, &popup.side),
        ];
        columns.into_iter().any(|(column, specs)| {
            { specs.iter().enumerate() }.any(|(index, spec)| {
                spec.widget == SETTINGS_WIDGET && skipped != Some((column, index))
            })
        })
    };
    let had = popups.iter().any(|popup| holds(popup, None));
    let left = popups
        .iter()
        .any(|popup| match (popup.name == name, widget) {
            (true, None) => false,
            (true, Some(taken)) => holds(popup, Some(taken)),
            (false, _) => holds(popup, None),
        });
    had && !left
}

/// A name of no popup of `taken`: `popup`, `popup 2` and so on.
fn free_name(taken: &[String]) -> String {
    std::iter::once("popup".to_owned())
        .chain((2..).map(|number| format!("popup {number}")))
        .find(|name| !taken.contains(name))
        .unwrap_or_default()
}

/// The place of a widget of `popup` among all of them, as
/// `Settings::open_at` counts them: the first column, then the second.
fn item_of(popup: &PopupConfig, column: PopupColumn, index: usize) -> String {
    let at = match column {
        PopupColumn::Content => index,
        PopupColumn::Side => popup.content.len() + index,
    };
    format!("popup/{}/{at}", popup.name)
}

/// The page.
struct PopupSettings {
    /// The popup whose settings are unfolded.
    open: Option<String>,
    /// The column of the open popup whose list of widgets to add is open.
    adding: Option<PopupColumn>,
    /// The popup that waits for a second click to be removed.
    removing: Option<String>,
    /// The name of a popup to add, and the new name of the open one.
    new_name: Entity<TextField>,
    rename: Entity<TextField>,
    /// What is wrong with what was last asked, or why it was not written.
    failure: Option<SharedString>,
    feedback: Feedback,
}

impl PopupSettings {
    fn new(open: Option<String>, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let language = Language::of(cx);
        let rename = cx.new(|cx| TextField::new(language.say("New name").into(), cx));
        if let Some(open) = &open {
            rename.update(cx, |field, cx| field.set_text(open, false, cx));
        }
        Self {
            open,
            adding: None,
            removing: None,
            new_name: cx.new(|cx| TextField::new(language.say("Name of a new popup").into(), cx)),
            rename,
            failure: None,
            feedback: Feedback::new(motion),
        }
    }

    /// Writes `change` to the file away from the thread of the interface,
    /// and has the shell take it over.
    fn write(&mut self, change: Change, cx: &mut Context<Self>) {
        self.failure = None;
        self.removing = None;
        let written = cx.background_executor().spawn(async move {
            ConfigFile::change(|file| match change.apply(file) {
                true => Ok(()),
                false => Err("the popups of the file have changed in the meantime".to_owned()),
            })
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let written = written.await;
            this.update(cx, |this, cx| {
                match written {
                    Ok(()) => cx.defer(Shell::config_changed),
                    Err(error) => {
                        report!("the popups are not written: {error}");
                        this.failure = Some(error.into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Unfolds the popup `name`, or folds it where it is unfolded.
    fn unfold(&mut self, name: &str, cx: &mut Context<Self>) {
        self.adding = None;
        self.removing = None;
        if self.open.as_deref() == Some(name) {
            self.open = None;
        } else {
            self.open = Some(name.to_owned());
            self.rename
                .update(cx, |field, cx| field.set_text(name, false, cx));
        }
        cx.notify();
    }

    /// Adds a popup named as the field says, or by a free name.
    fn add(&mut self, cx: &mut Context<Self>) {
        let typed = self.new_name.read(cx).text().trim().to_owned();
        let taken: Vec<String> = { Shell::config(cx).popups.iter() }
            .map(|popup| popup.name.clone())
            .collect();
        let name = if typed.is_empty() {
            free_name(&taken)
        } else {
            typed
        };
        if taken.contains(&name) {
            let said = fill(
                Language::of(cx).say("There is a popup named {} already."),
                &[&name],
            );
            self.failure = Some(said.into());
            cx.notify();
            return;
        }
        self.new_name.update(cx, |field, cx| field.clear(cx));
        let (width, height) = NEW_SIZE;
        let popup = PopupConfig {
            name: name.clone(),
            place: PopupPlace::Surface,
            width,
            height,
            content: Vec::new(),
            side: Vec::new(),
        };
        self.open = Some(name.clone());
        self.rename
            .update(cx, |field, cx| field.set_text(&name, false, cx));
        self.write(Change::Add(popup), cx);
    }

    /// Gives the open popup the name the field says.
    fn rename(&mut self, from: &str, cx: &mut Context<Self>) {
        let to = self.rename.read(cx).text().trim().to_owned();
        if to.is_empty() || to == from {
            return;
        }
        if Shell::config(cx)
            .popups
            .iter()
            .any(|popup| popup.name == to)
        {
            let said = fill(
                Language::of(cx).say("There is a popup named {} already."),
                &[&to],
            );
            self.failure = Some(said.into());
            cx.notify();
            return;
        }
        self.open = Some(to.clone());
        self.write(Change::Rename(from.to_owned(), to), cx);
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

    /// A field framed so that it stands out from the card.
    fn field(theme: &Theme, field: &Entity<TextField>) -> Div {
        div()
            .flex_1()
            .min_w_0()
            .rounded(theme.shape.full)
            .border_1()
            .border_color(theme.colors.outline)
            .child(field.clone())
    }

    /// The row of a popup, which folds and unfolds its settings.
    fn header(
        &mut self,
        index: usize,
        popup: &PopupConfig,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let unfolded = self.open.as_deref() == Some(popup.name.as_str());
        let place = { PLACES.iter() }
            .find(|(place, _)| *place == popup.place)
            .map_or("", |(_, label)| language.say(label));
        let count = popup.content.len() + popup.side.len();
        let note = format!(
            "{} \u{d7} {} px \u{b7} {} \u{b7} {}",
            popup.width,
            popup.height,
            place,
            fill(language.say("{} widgets"), &[&count.to_string()]),
        );
        let key = ("popup", index);
        let state = self.feedback.state(key, window);
        let name = popup.name.clone();
        let ground = match unfolded {
            true => theme.colors.secondary_container,
            false => theme.colors.surface_container,
        };
        let row = div()
            .id(key)
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .px(theme.spacing.small)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.medium)
            .bg(state_layer(&theme, ground, state))
            .child(
                icon(&theme, if unfolded { ICON_UNFOLDED } else { ICON_FOLDED })
                    .text_color(theme.colors.on_surface_variant),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(one_line(popup.name.clone())),
                    )
                    .child(page_note(&theme, note)),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.unfold(&name, cx)));
        Feedback::track(row, key, cx, |this: &mut Self| &mut this.feedback)
    }

    /// The settings of the popup `popup`, unfolded under its row.
    fn editor(
        &mut self,
        popup: &PopupConfig,
        popups: &[PopupConfig],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let name = popup.name.clone();
        let mut editor = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .pl(theme.spacing.medium);

        let from = name.clone();
        let rename = self.button(
            ("rename", 0),
            Face::Chip(language.say("Rename").into(), false),
            move |this, cx| this.rename(&from, cx),
            window,
            cx,
        );
        let from = name.clone();
        editor = editor.child(
            div()
                .flex()
                .items_center()
                .gap(theme.spacing.small)
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "enter" {
                        this.rename(&from, cx);
                    }
                }))
                .child(Self::field(&theme, &self.rename))
                .child(rename),
        );

        let mut places = div().flex().flex_wrap().gap(theme.spacing.tiny);
        for (index, (place, label)) in PLACES.into_iter().enumerate() {
            let target = name.clone();
            let chip = self.button(
                ("place", index),
                Face::Chip(language.say(label).into(), popup.place == place),
                move |this, cx| {
                    let value = popup_place_name(place).map(Value::from);
                    this.write(Change::Field(target.clone(), "place", value), cx);
                },
                window,
                cx,
            );
            places = places.child(chip);
        }
        editor = editor.child(labelled(&theme, language.say("Place"), places));

        for (key, label, length) in [
            ("width", "Width", popup.width),
            ("height", "Height", popup.height),
        ] {
            let mut stepper = div().flex().items_center().gap(theme.spacing.small);
            for (up, glyph) in [(false, ICON_MINUS), (true, ICON_PLUS)] {
                let target = name.clone();
                let next = stepped(length, up);
                let button = self.button(
                    (key, usize::from(up)),
                    Face::Glyph(glyph),
                    move |this, cx| {
                        let value = Some(Value::from(next as i64));
                        this.write(Change::Field(target.clone(), key, value), cx);
                    },
                    window,
                    cx,
                );
                stepper = stepper.child(button);
                if !up {
                    stepper = stepper.child(
                        div()
                            .min_w(px(64.))
                            .flex()
                            .justify_center()
                            .child(format!("{length} px")),
                    );
                }
            }
            editor = editor.child(labelled(&theme, language.say(label), stepper));
        }

        for column in PopupColumn::ALL {
            editor = editor.child(self.column(popup, popups, column, window, cx));
        }

        editor.child(self.removal(popup, popups, window, cx))
    }

    /// The widgets of one column of `popup`, and the choice of one to add.
    fn column(
        &mut self,
        popup: &PopupConfig,
        popups: &[PopupConfig],
        column: PopupColumn,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let (specs, title, key) = match column {
            PopupColumn::Content => (&popup.content, "Widgets", 0),
            PopupColumn::Side => (&popup.side, "Second column", 1),
        };
        let count = specs.len();
        let mut list = div().flex().flex_col().gap(theme.spacing.tiny).child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.on_surface_variant)
                .child(one_line(language.say(title))),
        );
        for (index, spec) in specs.iter().enumerate() {
            let slot = key * 1000 + index;
            let name = popup.name.clone();
            let item = item_of(popup, column, index);
            let configure = self.button(
                ("configure", slot),
                Face::Glyph(ICON_OPTIONS),
                move |_, cx| {
                    let item = item.clone();
                    cx.defer(move |cx| Settings::open_at("widgets", Some(&item), cx));
                },
                window,
                cx,
            );
            let mut buttons = vec![configure.into_any_element()];
            for (up, glyph, id) in [(true, ICON_UP, "up"), (false, ICON_DOWN, "down")] {
                let possible = if up { index > 0 } else { index + 1 < count };
                if !possible {
                    buttons.push(div().size(px(28.)).into_any_element());
                    continue;
                }
                let target = name.clone();
                let button = self.button(
                    (id, slot),
                    Face::Glyph(glyph),
                    move |this, cx| {
                        let order = swapped(count, index, up);
                        this.write(Change::Widgets(target.clone(), column, order), cx);
                    },
                    window,
                    cx,
                );
                buttons.push(button.into_any_element());
            }
            let last = takes_the_way_back(popups, &popup.name, Some((column, index)));
            if !last {
                let target = name.clone();
                let remove = self.button(
                    ("remove", slot),
                    Face::Glyph(ICON_REMOVE),
                    move |this, cx| {
                        let order = without(count, index);
                        this.write(Change::Widgets(target.clone(), column, order), cx);
                    },
                    window,
                    cx,
                );
                buttons.push(remove.into_any_element());
            }
            let options = spec.options.len();
            let note = match (options, &spec.popup) {
                (0, None) => String::new(),
                (_, Some(opens)) => fill(language.say("Opens {}"), &[opens]),
                (options, None) => fill(language.say("{} options"), &[&options.to_string()]),
            };
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .child(div().flex_none().child(one_line(spec.widget.clone())))
                    .child(div().flex_1().min_w_0().child(page_note(&theme, note)))
                    .children(buttons),
            );
        }
        let adding = self.adding == Some(column);
        let toggle = self.button(
            ("add-widget", key),
            Face::Chip(language.say("Add widget").into(), adding),
            move |this, cx| {
                this.adding = if this.adding == Some(column) {
                    None
                } else {
                    Some(column)
                };
                cx.notify();
            },
            window,
            cx,
        );
        list = list.child(div().flex().child(toggle));
        if adding {
            let names = cx.global::<WidgetRegistry>().names();
            let mut chips = div().flex().flex_wrap().gap(theme.spacing.tiny);
            for (index, widget) in names.into_iter().enumerate() {
                let target = popup.name.clone();
                let chip = self.button(
                    ("widget", key * 1000 + index),
                    Face::Chip(widget.clone().into(), false),
                    move |this, cx| {
                        this.adding = None;
                        let order = appended(count, &widget);
                        this.write(Change::Widgets(target.clone(), column, order), cx);
                    },
                    window,
                    cx,
                );
                chips = chips.child(chip);
            }
            list = list.child(chips);
        }
        list
    }

    /// The removal of `popup`, which asks a second click, unless it holds
    /// the last way back into the settings.
    fn removal(
        &mut self,
        popup: &PopupConfig,
        popups: &[PopupConfig],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        if takes_the_way_back(popups, &popup.name, None) {
            return page_note(
                &theme,
                language.say("It holds the settings, which no other popup holds, so it stays."),
            );
        }
        let name = popup.name.clone();
        let asked = self.removing.as_deref() == Some(popup.name.as_str());
        let mut row = div().flex().items_center().gap(theme.spacing.small);
        if asked {
            let target = name.clone();
            let remove = self.button(
                ("remove-popup", 1),
                Face::Chip(language.say("Remove").into(), true),
                move |this, cx| {
                    this.open = None;
                    this.write(Change::Remove(target.clone()), cx);
                },
                window,
                cx,
            );
            let cancel = self.button(
                ("remove-popup", 2),
                Face::Chip(language.say("Cancel").into(), false),
                |this, cx| {
                    this.removing = None;
                    cx.notify();
                },
                window,
                cx,
            );
            row = row
                .child(one_line(language.say("Remove this popup?")))
                .child(remove)
                .child(cancel);
        } else {
            let remove = self.button(
                ("remove-popup", 0),
                Face::Chip(language.say("Remove popup").into(), false),
                move |this, cx| {
                    this.removing = Some(name.clone());
                    cx.notify();
                },
                window,
                cx,
            );
            row = row.child(remove);
        }
        row
    }

    /// The popups the modules bring that the file does not replace, each
    /// with a button that copies it into the file.
    fn builtin(&mut self, popups: &[PopupConfig], window: &Window, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let registry = cx.global::<WidgetRegistry>();
        let mut names = registry.popup_names();
        names.retain(|name| !popups.iter().any(|popup| popup.name == *name));
        names.sort();
        let brought: Vec<PopupConfig> = { names.iter() }
            .filter_map(|name| registry.popup(name).cloned())
            .collect();
        let mut card =
            page_card(&theme, language.say("Popups that come with Tessera")).child(page_note(
                &theme,
                language.say(
                    "They are used as they are until they are copied into the file to be changed.",
                ),
            ));
        for (index, popup) in brought.into_iter().enumerate() {
            let widgets: Vec<&str> = popup.widgets().map(|spec| spec.widget.as_str()).collect();
            let copied = popup.clone();
            let copy = self.button(
                ("copy", index),
                Face::Chip(language.say("Change").into(), false),
                move |this, cx| {
                    this.open = Some(copied.name.clone());
                    this.rename
                        .update(cx, |field, cx| field.set_text(&copied.name, false, cx));
                    this.write(Change::Add(copied.clone()), cx);
                },
                window,
                cx,
            );
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(one_line(popup.name.clone())),
                            )
                            .child(page_note(&theme, widgets.join(", "))),
                    )
                    .child(copy),
            );
        }
        card
    }
}

/// `item` after a label.
fn labelled(theme: &Theme, label: &'static str, item: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(theme.spacing.small)
        .child(div().flex_none().w(px(96.)).child(one_line(label)))
        .child(div().flex_1().min_w_0().child(item))
}

impl Render for PopupSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let popups = Shell::config(cx).popups.clone();
        let mut card = page_card(&theme, language.say("Popups"));
        for (index, popup) in popups.iter().enumerate() {
            card = card.child(self.header(index, popup, window, cx));
            if self.open.as_deref() == Some(popup.name.as_str()) {
                card = card.child(self.editor(popup, &popups, window, cx));
            }
        }
        let add = self.button(
            ("add-popup", 0),
            Face::Chip(language.say("Add popup").into(), false),
            |this, cx| this.add(cx),
            window,
            cx,
        );
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(theme.spacing.small)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "enter" {
                        this.add(cx);
                    }
                }))
                .child(Self::field(&theme, &self.new_name))
                .child(add),
        );
        let failure = self.failure.clone().map(|failure| {
            div()
                .px(theme.spacing.small)
                .text_color(theme.colors.primary)
                .child(failure)
        });
        let builtin = self.builtin(&popups, window, cx);
        div()
            .id(NAME)
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .text_color(theme.colors.on_surface)
            .children(failure)
            .child(card)
            .child(builtin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WidgetSpec;

    fn popup(name: &str, content: &[&str], side: &[&str]) -> PopupConfig {
        let specs = |names: &[&str]| {
            { names.iter() }
                .map(|name| WidgetSpec {
                    widget: (*name).to_owned(),
                    popup: None,
                    options: toml::Table::new(),
                })
                .collect()
        };
        PopupConfig {
            name: name.to_owned(),
            place: PopupPlace::Surface,
            width: 300.,
            height: 300.,
            content: specs(content),
            side: specs(side),
        }
    }

    #[test]
    fn an_item_of_the_settings_names_its_popup() {
        assert_eq!(
            popup_of_item("popup/calendar/2").as_deref(),
            Some("calendar")
        );
        assert_eq!(
            popup_of_item("popup/quick settings").as_deref(),
            Some("quick settings")
        );
        assert_eq!(popup_of_item("calendar").as_deref(), Some("calendar"));
        assert_eq!(popup_of_item("popup/"), None);
    }

    #[test]
    fn widgets_are_moved_taken_out_and_added_by_their_places() {
        use PopupWidget::{Kept, New};
        assert_eq!(swapped(3, 1, true), [Kept(1), Kept(0), Kept(2)]);
        assert_eq!(swapped(3, 1, false), [Kept(0), Kept(2), Kept(1)]);
        assert_eq!(swapped(3, 0, true), [Kept(0), Kept(1), Kept(2)]);
        assert_eq!(swapped(3, 2, false), [Kept(0), Kept(1), Kept(2)]);
        assert_eq!(without(3, 1), [Kept(0), Kept(2)]);
        assert_eq!(appended(1, "clock"), [Kept(0), New("clock".to_owned())]);
    }

    #[test]
    fn a_popup_is_sized_by_steps_within_its_range() {
        assert_eq!(stepped(360., true), 380.);
        assert_eq!(stepped(365., false), 340.);
        assert_eq!(stepped(120., false), 120.);
        assert_eq!(stepped(1995., true), 2000.);
    }

    #[test]
    fn the_last_way_back_into_the_settings_stays() {
        let popups = [
            popup("settings", &["settings"], &[]),
            popup("calendar", &["calendar"], &["notifications"]),
        ];
        assert!(takes_the_way_back(&popups, "settings", None));
        assert!(takes_the_way_back(
            &popups,
            "settings",
            Some((PopupColumn::Content, 0))
        ));
        assert!(!takes_the_way_back(&popups, "calendar", None));
        assert!(!takes_the_way_back(
            &popups,
            "calendar",
            Some((PopupColumn::Side, 0))
        ));
        let twice = [
            popup("settings", &["settings"], &[]),
            popup("more", &["clock"], &["settings"]),
        ];
        assert!(!takes_the_way_back(&twice, "settings", None));
        // A layout without the settings in a popup loses nothing.
        let none = [popup("calendar", &["calendar"], &[])];
        assert!(!takes_the_way_back(&none, "calendar", None));
    }

    #[test]
    fn a_new_popup_gets_a_free_name_and_its_widgets_their_items() {
        assert_eq!(free_name(&[]), "popup");
        assert_eq!(
            free_name(&["popup".to_owned(), "popup 2".to_owned()]),
            "popup 3"
        );
        let calendar = popup("calendar", &["calendar", "clock"], &["notifications"]);
        assert_eq!(
            item_of(&calendar, PopupColumn::Content, 1),
            "popup/calendar/1"
        );
        assert_eq!(item_of(&calendar, PopupColumn::Side, 0), "popup/calendar/2");
    }
}
