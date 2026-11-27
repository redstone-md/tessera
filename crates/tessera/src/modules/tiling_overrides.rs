// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the tiling does otherwise on some displays and workspaces, as a
//! page of the settings: widget `tiling-overrides`, drawn in the section
//! Windows under the card of the tiling. It lists the entries of
//! `[[tiling.display]]` and `[[tiling.workspace]]`, each with what it
//! names (a display, or a workspace on every display or on one) and what
//! it sets of the layout, the gap and the room around the tiles and at
//! the sides of the display, each of them left to `[tiling]` until set.
//! Entries are added, changed, moved and removed; a later entry has the
//! last word over an earlier one.
//!
//! The entries are read from the file away from the thread of the
//! interface, as the file has them, and every change is checked as the
//! tiling reads it before it is written, with the rest of the file and
//! its comments as they were. The shell takes the file over at once.

use gpui::{
    AppContext, AsyncApp, Context, Div, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Stateful, StatefulInteractiveElement, Styled, WeakEntity, Window, div,
    px,
};

use super::quick::chip;
use super::settings_parts::{Feedback, chip_row, note_line};
use crate::config::edit::ConfigFile;
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::platform::DisplayName;
use crate::runners::fill;
use crate::shell::{KnownDisplays, Shell};
use crate::theme::Theme;
use crate::tiling::{OverrideKind, Tiler};
use crate::ui::{icon, list_title, one_line, state_layer};

/// The name of the page.
const NAME: &str = "tiling-overrides";

/// The gap the tiling leaves where nothing says one, and the widest an
/// entry may give, in pixels of the screen; the room at the sides is
/// changed by the same steps up to the most it may be.
const GAP: f64 = 8.;
const WIDEST_GAP: f64 = 200.;
const WIDEST_ROOM: f64 = 1000.;
const STEP: f64 = 2.;

/// The most workspaces there are.
const MOST_WORKSPACES: i64 = 16;

const ICON_UP: &str = "\u{e70e}";
const ICON_DOWN: &str = "\u{e70d}";
const ICON_REMOVE: &str = "\u{e74d}";
const ICON_MINUS: &str = "\u{e738}";
const ICON_PLUS: &str = "\u{e710}";
const ICON_FOLDED: &str = "\u{e76c}";
const ICON_UNFOLDED: &str = "\u{e70d}";

/// The side of a round button with an icon.
const BUTTON: f32 = 28.;

pub struct TilingOverridesModule;

impl Module for TilingOverridesModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(NAME, |_, cx| cx.new(TilingOverrides::new).into())
    }
}

/// A change of the entries of the file.
#[derive(Clone, Debug)]
enum Change {
    Add(OverrideKind),
    Set(OverrideKind, usize, toml::Table),
    /// Two entries that trade places, each with what the other said.
    Swap(OverrideKind, (usize, toml::Table), (usize, toml::Table)),
    Remove(OverrideKind, usize),
}

impl Change {
    /// Makes the change in `file`, or says why it cannot be made.
    fn apply(self, file: &mut ConfigFile) -> Result<(), String> {
        match self {
            Self::Add(kind) => file
                .add_tiling_override(kind)
                .map(drop)
                .ok_or_else(|| "`tiling` is not a table in the file".to_owned()),
            Self::Set(kind, index, entry) => file.set_tiling_override(kind, index, &entry),
            Self::Swap(kind, (first, said), (second, other)) => {
                file.set_tiling_override(kind, first, &other)?;
                file.set_tiling_override(kind, second, &said)
            }
            Self::Remove(kind, index) => match file.remove_tiling_override(kind, index) {
                true => Ok(()),
                false => Err("the entries of the tiling have changed in the meantime".to_owned()),
            },
        }
    }
}

/// The entries of both lists in `file`, a configuration as a table.
fn entries_of(file: &toml::Table) -> [Vec<toml::Table>; 2] {
    let tiling = file.get("tiling").and_then(toml::Value::as_table);
    OverrideKind::ALL.map(|kind| {
        let list = tiling.and_then(|tiling| tiling.get(kind.key()));
        { list.and_then(toml::Value::as_array).into_iter().flatten() }
            .filter_map(|entry| entry.as_table().cloned())
            .collect()
    })
}

/// The number `entry` gives `key`, where it gives one.
fn number(entry: &toml::Table, key: &str) -> Option<f64> {
    match entry.get(key)? {
        toml::Value::Integer(number) => Some(*number as f64),
        toml::Value::Float(number) => Some(*number),
        _ => None,
    }
}

/// `value` as the file writes a number: whole where it is.
fn number_value(value: f64) -> toml::Value {
    match value.fract() == 0. {
        true => toml::Value::Integer(value as i64),
        false => toml::Value::Float(value),
    }
}

/// `entry` with `key` set to `value`, or taken out without one.
fn with(entry: &toml::Table, key: &str, value: Option<toml::Value>) -> toml::Table {
    let mut entry = entry.clone();
    match value {
        Some(value) => entry.insert(key.to_owned(), value),
        None => entry.remove(key),
    };
    entry
}

/// The value a step from `now` makes, `up` or down, within `0..=most`.
fn stepped(now: f64, up: bool, most: f64) -> f64 {
    let step = if up { STEP } else { -STEP };
    ((now / STEP).round() * STEP + step).clamp(0., most)
}

/// What the page calls the display `name`.
fn display_label(name: &DisplayName, language: Language) -> String {
    let numbered = fill(language.say("Display {}"), &[&name.position.to_string()]);
    match name.friendly.trim() {
        "" => numbered,
        friendly => format!("{numbered} \u{b7} {friendly}"),
    }
}

/// What the page calls the display a selector written as `text` names,
/// among `known`.
fn selector_label(text: &str, known: &[DisplayName], language: Language) -> String {
    match text.trim().parse::<usize>() {
        Ok(position) => { known.iter() }
            .find(|name| name.position == position)
            .map(|name| display_label(name, language))
            .unwrap_or_else(|| fill(language.say("Display {}"), &[&position.to_string()])),
        Err(_) => text.to_owned(),
    }
}

/// The page.
struct TilingOverrides {
    /// The entries of each list, in the order of [`OverrideKind::ALL`],
    /// once they were read.
    entries: Option<[Vec<toml::Table>; 2]>,
    /// The entry whose settings are unfolded.
    open: Option<(OverrideKind, usize)>,
    /// The entry that waits for a second click to be removed.
    removing: Option<(OverrideKind, usize)>,
    /// What is wrong with what was last asked, or why it was not written.
    failure: Option<SharedString>,
    feedback: Feedback,
}

impl TilingOverrides {
    fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let mut this = Self {
            entries: None,
            open: None,
            removing: None,
            failure: None,
            feedback: Feedback::new(motion),
        };
        this.read(cx);
        this
    }

    /// Reads the entries from the file away from the thread of the
    /// interface.
    fn read(&mut self, cx: &mut Context<Self>) {
        let read = cx.background_executor().spawn(async move {
            let file = ConfigFile::open()?;
            file.table()
                .map(|table| entries_of(&table))
                .ok_or_else(|| "the configuration cannot be read".to_owned())
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = read.await;
            this.update(cx, |this, cx| {
                match read {
                    Ok(entries) => this.entries = Some(entries),
                    Err(error) => this.failure = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Writes `change` to the file away from the thread of the interface,
    /// has the shell take it over and reads the entries again.
    fn write(&mut self, change: Change, cx: &mut Context<Self>) {
        self.failure = None;
        self.removing = None;
        let written = cx
            .background_executor()
            .spawn(async move { ConfigFile::change(|file| change.apply(file)) });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let written = written.await;
            this.update(cx, |this, cx| {
                match written {
                    Ok(()) => cx.defer(Shell::config_changed),
                    Err(error) => {
                        report!("the tiling of a display or workspace is not written: {error}");
                        this.failure = Some(error.into());
                    }
                }
                this.read(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The entries of `kind`, as last read.
    fn list(&self, kind: OverrideKind) -> &[toml::Table] {
        let at = OverrideKind::ALL.iter().position(|&one| one == kind);
        match (&self.entries, at) {
            (Some(entries), Some(at)) => &entries[at],
            _ => &[],
        }
    }

    /// Sets `key` of the entry of `kind` at `index`; `None` takes it out.
    fn set(
        &mut self,
        kind: OverrideKind,
        index: usize,
        key: &str,
        value: Option<toml::Value>,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.list(kind).get(index) else {
            return;
        };
        let entry = with(entry, key, value);
        self.write(Change::Set(kind, index, entry), cx);
    }

    /// Moves the entry of `kind` at `index` one place nearer the start,
    /// or the end.
    fn shift(&mut self, kind: OverrideKind, index: usize, up: bool, cx: &mut Context<Self>) {
        let list = self.list(kind);
        let other = if up {
            index.checked_sub(1)
        } else {
            Some(index + 1)
        };
        let (Some(other), Some(said)) = (other, list.get(index).cloned()) else {
            return;
        };
        let Some(other_said) = list.get(other).cloned() else {
            return;
        };
        if self.open == Some((kind, index)) {
            self.open = Some((kind, other));
        }
        self.write(Change::Swap(kind, (index, said), (other, other_said)), cx);
    }

    /// A key of a control of the entry of `kind` at `index`.
    fn slot(kind: OverrideKind, index: usize, part: usize) -> usize {
        let list = usize::from(kind == OverrideKind::Workspace);
        (list * 1000 + index) * 100 + part
    }

    /// A chip that does `act` when clicked.
    fn chip(
        &mut self,
        key: (&'static str, usize),
        label: impl Into<SharedString>,
        chosen: bool,
        act: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let state = self.feedback.state(key, window);
        let element = chip(&theme, key, label, chosen, state)
            .on_click(cx.listener(move |this, _, _, cx| act(this, cx)));
        Feedback::track(element, key, cx, |this: &mut Self| &mut this.feedback)
    }

    /// A round button with the icon `glyph` that does `act`.
    fn glyph(
        &mut self,
        key: (&'static str, usize),
        glyph: &'static str,
        act: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let state = self.feedback.state(key, window);
        let element = div()
            .id(key)
            .flex_none()
            .size(px(BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(&theme, theme.colors.surface_container, state))
            .text_color(theme.colors.on_surface_variant)
            .child(icon(&theme, glyph).text_size(theme.typography.label_size))
            .on_click(cx.listener(move |this, _, _, cx| act(this, cx)));
        Feedback::track(element, key, cx, |this: &mut Self| &mut this.feedback)
    }

    /// A row of settings under `label`.
    fn labelled(theme: &Theme, label: &str, control: impl IntoElement) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(note_line(theme, label.to_owned()))
            .child(control)
    }

    /// What the row of the entry says of it, folded.
    fn summary(kind: OverrideKind, entry: &toml::Table, cx: &Context<Self>) -> (String, String) {
        let language = Language::of(cx);
        let known = KnownDisplays::all(cx);
        let text = |key: &str| entry.get(key).and_then(toml::Value::as_str);
        let title = match kind {
            OverrideKind::Display => {
                selector_label(text("match").unwrap_or_default(), known, language)
            }
            OverrideKind::Workspace => {
                let number = number(entry, "number").unwrap_or(1.) as i64;
                let workspace = format!("{} {number}", language.say("Workspace"));
                match text("display") {
                    Some(display) => {
                        format!(
                            "{workspace} \u{b7} {}",
                            selector_label(display, known, language)
                        )
                    }
                    None => workspace,
                }
            }
        };
        let layouts = Tiler::layout_names(cx);
        let mut said = Vec::new();
        if let Some(layout) = text("layout") {
            let named = layouts.iter().find(|(name, _)| name == layout);
            said.push(named.map_or_else(|| layout.to_owned(), |(_, title)| title.clone()));
        }
        if let Some(gap) = number(entry, "gap") {
            said.push(format!("{} {gap} px", language.say("Gap")));
        }
        for (key, label) in [
            ("padding", "Room around the tiles"),
            ("margin", "Room at the sides of the display"),
        ] {
            if entry.contains_key(key) {
                said.push(language.say(label).to_owned());
            }
        }
        (title, said.join(" \u{b7} "))
    }

    /// The row of an entry, which folds and unfolds its settings.
    fn header(
        &mut self,
        kind: OverrideKind,
        index: usize,
        entry: &toml::Table,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let unfolded = self.open == Some((kind, index));
        let (title, note) = Self::summary(kind, entry, cx);
        let key = ("entry", Self::slot(kind, index, 0));
        let state = self.feedback.state(key, window);
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
                    .child(div().font_weight(FontWeight::MEDIUM).child(one_line(title)))
                    .children((!note.is_empty()).then(|| {
                        div()
                            .text_size(theme.typography.label_size)
                            .text_color(theme.colors.on_surface_variant)
                            .child(one_line(note))
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.removing = None;
                this.open = match this.open == Some((kind, index)) {
                    true => None,
                    false => Some((kind, index)),
                };
                cx.notify();
            }));
        Feedback::track(row, key, cx, |this: &mut Self| &mut this.feedback)
    }

    /// The chips that choose a display: by its place, as `match` and the
    /// `display` of a workspace name it, `key` of the entry; `every`
    /// offers every display as well, which takes the key out.
    #[allow(clippy::too_many_arguments)]
    fn displays(
        &mut self,
        kind: OverrideKind,
        index: usize,
        entry: &toml::Table,
        key: &'static str,
        every: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let known: Vec<DisplayName> = KnownDisplays::all(cx).to_vec();
        let current = entry
            .get(key)
            .and_then(toml::Value::as_str)
            .map(str::to_owned);
        let mut row = chip_row(&theme);
        if every {
            let chip = self.chip(
                ("display", Self::slot(kind, index, 0)),
                language.say("All displays"),
                current.is_none(),
                move |this, cx| this.set(kind, index, key, None, cx),
                window,
                cx,
            );
            row = row.child(chip);
        }
        let mut shown = false;
        for name in &known {
            let value = name.position.to_string();
            let chosen = current.as_deref().map(str::trim) == Some(value.as_str());
            shown |= chosen;
            let chip = self.chip(
                ("display", Self::slot(kind, index, name.position.min(98))),
                display_label(name, language),
                chosen,
                move |this, cx| {
                    let value = toml::Value::String(value.clone());
                    this.set(kind, index, key, Some(value), cx);
                },
                window,
                cx,
            );
            row = row.child(chip);
        }
        // A display named otherwise in the file, as by its name, is shown
        // as it is written.
        if let Some(current) = current.filter(|_| !shown) {
            let chip = self.chip(
                ("display", Self::slot(kind, index, 99)),
                selector_label(&current, &known, language),
                true,
                |_, _| {},
                window,
                cx,
            );
            row = row.child(chip);
        }
        row
    }

    /// A number of the entry, `key`, shown and changed by steps, with a
    /// chip that leaves it to `[tiling]`; `start` is what it is there.
    #[allow(clippy::too_many_arguments)]
    fn stepper(
        &mut self,
        kind: OverrideKind,
        index: usize,
        entry: &toml::Table,
        (key, part): (&'static str, usize),
        start: f64,
        most: f64,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let value = number(entry, key);
        // A room given for each side apart is set alike at every side.
        let apart = entry.get(key).is_some_and(toml::Value::is_table);
        let now = value.unwrap_or(start);
        let mut row = div().flex().items_center().gap(theme.spacing.small);
        for (up, glyph) in [(false, ICON_MINUS), (true, ICON_PLUS)] {
            let next = stepped(now, up, most);
            let button = self.glyph(
                (key, Self::slot(kind, index, part * 2 + usize::from(up))),
                glyph,
                move |this, cx| this.set(kind, index, key, Some(number_value(next)), cx),
                window,
                cx,
            );
            row = row.child(button);
            if !up {
                let shown = match (value, apart) {
                    (Some(value), _) => format!("{value} px"),
                    (None, true) => language.say("Each side apart").to_owned(),
                    (None, false) => format!("{} ({start} px)", language.say("Default")),
                };
                row = row.child(div().min_w(px(96.)).flex().justify_center().child(shown));
            }
        }
        let unset = self.chip(
            ("unset", Self::slot(kind, index, part)),
            language.say("Default"),
            value.is_none() && !apart,
            move |this, cx| this.set(kind, index, key, None, cx),
            window,
            cx,
        );
        row.child(unset)
    }

    /// The settings of the entry of `kind` at `index`, unfolded under its
    /// row.
    fn editor(
        &mut self,
        kind: OverrideKind,
        index: usize,
        entry: &toml::Table,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut editor = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .pl(theme.spacing.medium);

        match kind {
            OverrideKind::Display => {
                let chips = self.displays(kind, index, entry, "match", false, window, cx);
                editor = editor.child(Self::labelled(&theme, language.say("Display"), chips));
            }
            OverrideKind::Workspace => {
                let number = number(entry, "number").unwrap_or(1.) as i64;
                let mut row = div().flex().items_center().gap(theme.spacing.small);
                for (up, glyph) in [(false, ICON_MINUS), (true, ICON_PLUS)] {
                    let next = if up { number + 1 } else { number - 1 }.clamp(1, MOST_WORKSPACES);
                    let button = self.glyph(
                        ("number", Self::slot(kind, index, usize::from(up))),
                        glyph,
                        move |this, cx| {
                            let value = toml::Value::Integer(next);
                            this.set(kind, index, "number", Some(value), cx);
                        },
                        window,
                        cx,
                    );
                    row = row.child(button);
                    if !up {
                        row = row.child(
                            div()
                                .min_w(px(48.))
                                .flex()
                                .justify_center()
                                .child(number.to_string()),
                        );
                    }
                }
                editor = editor.child(Self::labelled(&theme, language.say("Workspace"), row));
                let chips = self.displays(kind, index, entry, "display", true, window, cx);
                editor = editor.child(Self::labelled(&theme, language.say("Display"), chips));
            }
        }

        let current = entry
            .get("layout")
            .and_then(toml::Value::as_str)
            .map(str::to_owned);
        let mut layouts = chip_row(&theme);
        let unset = self.chip(
            ("layout", Self::slot(kind, index, 0)),
            language.say("Default"),
            current.is_none(),
            move |this, cx| this.set(kind, index, "layout", None, cx),
            window,
            cx,
        );
        layouts = layouts.child(unset);
        for (at, (name, title)) in Tiler::layout_names(cx).into_iter().enumerate() {
            let chosen = current.as_deref() == Some(name.as_str());
            let chip = self.chip(
                ("layout", Self::slot(kind, index, (at + 1).min(99))),
                title,
                chosen,
                move |this, cx| {
                    let value = toml::Value::String(name.clone());
                    this.set(kind, index, "layout", Some(value), cx);
                },
                window,
                cx,
            );
            layouts = layouts.child(chip);
        }
        editor = editor.child(Self::labelled(&theme, language.say("Layout"), layouts));

        let gap = number(entry, "gap").unwrap_or(GAP);
        let rows: [(&'static str, &str, f64, f64); 3] = [
            ("gap", "Gap", GAP, WIDEST_GAP),
            ("padding", "Room around the tiles", gap, WIDEST_ROOM),
            (
                "margin",
                "Room at the sides of the display",
                0.,
                WIDEST_ROOM,
            ),
        ];
        for (part, (key, label, start, most)) in rows.into_iter().enumerate() {
            let stepper = self.stepper(kind, index, entry, (key, part), start, most, window, cx);
            editor = editor.child(Self::labelled(&theme, language.say(label), stepper));
        }
        editor.child(self.actions(kind, index, window, cx))
    }

    /// The buttons that move the entry and take it away, which asks a
    /// second click.
    fn actions(
        &mut self,
        kind: OverrideKind,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let count = self.list(kind).len();
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.spacing.small);
        if index > 0 {
            let up = self.glyph(
                ("up", Self::slot(kind, index, 0)),
                ICON_UP,
                move |this, cx| this.shift(kind, index, true, cx),
                window,
                cx,
            );
            row = row.child(up);
        }
        if index + 1 < count {
            let down = self.glyph(
                ("down", Self::slot(kind, index, 0)),
                ICON_DOWN,
                move |this, cx| this.shift(kind, index, false, cx),
                window,
                cx,
            );
            row = row.child(down);
        }
        if self.removing == Some((kind, index)) {
            let remove = self.chip(
                ("remove", Self::slot(kind, index, 1)),
                language.say("Remove"),
                true,
                move |this, cx| {
                    this.open = None;
                    this.write(Change::Remove(kind, index), cx);
                },
                window,
                cx,
            );
            let cancel = self.chip(
                ("remove", Self::slot(kind, index, 2)),
                language.say("Cancel"),
                false,
                |this, cx| {
                    this.removing = None;
                    cx.notify();
                },
                window,
                cx,
            );
            return row
                .child(one_line(language.say("Remove this entry?")))
                .child(remove)
                .child(cancel);
        }
        let remove = self.glyph(
            ("remove", Self::slot(kind, index, 0)),
            ICON_REMOVE,
            move |this, cx| {
                this.removing = Some((kind, index));
                cx.notify();
            },
            window,
            cx,
        );
        row.child(remove)
    }

    /// The list of entries of `kind`, with the chip that adds one.
    fn section(&mut self, kind: OverrideKind, window: &Window, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let (title, empty, add) = match kind {
            OverrideKind::Display => (
                "Displays",
                "No display has a tiling of its own.",
                "Add a display",
            ),
            OverrideKind::Workspace => (
                "Workspaces",
                "No workspace has a tiling of its own.",
                "Add a workspace",
            ),
        };
        let entries = self.list(kind).to_vec();
        let mut list = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(list_title(&theme, language.say(title)));
        if entries.is_empty() {
            list = list.child(note_line(&theme, language.say(empty)));
        }
        for (index, entry) in entries.iter().enumerate() {
            list = list.child(self.header(kind, index, entry, window, cx));
            if self.open == Some((kind, index)) {
                list = list.child(self.editor(kind, index, entry, window, cx));
            }
        }
        let add = self.chip(
            ("add", Self::slot(kind, 0, 0)),
            language.say(add),
            false,
            move |this, cx| {
                let count = this.list(kind).len();
                this.open = Some((kind, count));
                this.write(Change::Add(kind), cx);
            },
            window,
            cx,
        );
        list.child(div().flex().px(theme.spacing.small).child(add))
    }
}

impl Render for TilingOverrides {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut page = div()
            .id(NAME)
            .flex()
            .flex_col()
            .gap(theme.spacing.medium)
            .p(theme.spacing.medium)
            .rounded(theme.shape.large)
            .bg(theme.colors.surface_container_highest)
            .text_color(theme.colors.on_surface)
            .child(note_line(
                &theme,
                language.say(
                    "What is set here has the last word over the tiling of every display; a later entry over an earlier one.",
                ),
            ));
        if let Some(failure) = &self.failure {
            page = page.child(note_line(&theme, failure.clone()).text_color(theme.colors.primary));
        }
        // Nothing is listed until the entries were read, a moment later.
        if self.entries.is_none() {
            return page;
        }
        for kind in OverrideKind::ALL {
            page = page.child(self.section(kind, window, cx));
        }
        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn the_entries_are_read_from_the_file() {
        let file = table(
            "[tiling]\ngap = 4\n[[tiling.display]]\nmatch = \"2\"\n[[tiling.workspace]]\nnumber = 1\n[[tiling.workspace]]\nnumber = 3\nlayout = \"rows\"\n",
        );
        let [displays, workspaces] = entries_of(&file);
        assert_eq!(displays.len(), 1);
        assert_eq!(workspaces.len(), 2);
        assert_eq!(number(&workspaces[1], "number"), Some(3.));
        let [none, also] = entries_of(&table("[tiling]\nenabled = true\n"));
        assert!(none.is_empty() && also.is_empty());
    }

    #[test]
    fn numbers_go_by_steps_within_their_range() {
        assert_eq!(stepped(8., true, WIDEST_GAP), 10.);
        assert_eq!(stepped(7., false, WIDEST_GAP), 6.);
        assert_eq!(stepped(0., false, WIDEST_GAP), 0.);
        assert_eq!(stepped(200., true, WIDEST_GAP), 200.);
        assert_eq!(number_value(4.), toml::Value::Integer(4));
        assert_eq!(number_value(0.5), toml::Value::Float(0.5));
        let entry = table("number = 2\ngap = 4");
        assert_eq!(with(&entry, "gap", None), table("number = 2"));
        assert_eq!(
            with(&entry, "layout", Some(toml::Value::String("rows".into()))),
            table("number = 2\ngap = 4\nlayout = \"rows\"")
        );
    }
}
