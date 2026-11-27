// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The details pane of a window of the file manager (Alt+Shift+P, or F11
//! as in Dolphin), at the right of the panes: a large picture of the file
//! the cursor is on, what it is, and the properties the Shell knows of
//! it, as Explorer's details pane and Dolphin's information panel show
//! them. Its title, rating, tags and comment can be written there; they
//! are written into the file through its property store, away from the
//! user interface, once Save or Enter is pressed.

use std::path::PathBuf;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, Entity, EventEmitter, InteractiveElement,
    IntoElement, KeyDownEvent, ObjectFit, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, Task, WeakEntity, Window, div, img,
    prelude::FluentBuilder, px,
};

use super::model::{Entry, size_text};
use super::thumbs::{Picture, Thumbs};
use crate::field::TextField;
use crate::i18n::Language;
use crate::icons::AppIcons;
use crate::platform::{FileProperty, Properties, PropertyEdit, PropertyValue};
use crate::theme::Theme;
use crate::ui::{icon, one_line};

/// How wide the pane is, in logical pixels.
pub const DETAILS_WIDTH: f32 = 300.;

/// How large the picture of the file is drawn.
const PICTURE_SIDE: f32 = 160.;

/// Characters of the icon font.
const ICON_STAR: &str = "\u{e735}";
const ICON_STAR_EMPTY: &str = "\u{e734}";
const ICON_FOLDER: &str = "\u{e8b7}";

/// The properties written in fields of their own.
const WRITTEN: [FileProperty; 3] = [
    FileProperty::Title,
    FileProperty::Tags,
    FileProperty::Comment,
];

/// What the details pane tells the window it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailsEvent {
    /// The properties of the file at this path were written.
    Written(PathBuf),
}

/// The details pane of a window.
pub struct DetailsPane {
    open: bool,
    /// The file shown, where one is: the one the cursor is on.
    entry: Option<Entry>,
    /// How many items are chosen, which the pane says where there are
    /// several.
    chosen: usize,
    /// The properties of the file as they were read, in the order the
    /// pane shows them; `None` until they are.
    values: Option<Vec<(FileProperty, Option<PropertyValue>)>>,
    /// The fields the written properties are edited in.
    fields: Vec<(FileProperty, Entity<TextField>)>,
    /// The stars of the rating, as chosen and as read.
    stars: u8,
    stars_read: u8,
    reading: Option<Task<()>>,
    saving: bool,
    problem: Option<String>,
    thumbs: Entity<Thumbs>,
}

impl EventEmitter<DetailsEvent> for DetailsPane {}

impl DetailsPane {
    pub fn new(open: bool, thumbs: Entity<Thumbs>, cx: &mut Context<Self>) -> Self {
        let language = Language::of(cx);
        let fields = { WRITTEN.into_iter() }
            .map(|property| {
                let placeholder: SharedString = language.say(property.title()).into();
                let field = cx.new(|cx| TextField::new(placeholder, cx));
                cx.observe(&field, |_, _, cx| cx.notify()).detach();
                (property, field)
            })
            .collect();
        cx.observe(&thumbs, |_, _, cx| cx.notify()).detach();
        Self {
            open,
            entry: None,
            chosen: 0,
            values: None,
            fields,
            stars: 0,
            stars_read: 0,
            reading: None,
            saving: false,
            problem: None,
            thumbs,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        if !open {
            self.entry = None;
            self.values = None;
            self.reading = None;
        }
        cx.notify();
    }

    /// Shows `entry`, the one the cursor is on, of `chosen` items chosen.
    pub fn show(&mut self, entry: Option<Entry>, chosen: usize, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        let same = match (&self.entry, &entry) {
            (Some(shown), Some(entry)) => {
                shown.path == entry.path && shown.modified == entry.modified
            }
            (None, None) => true,
            _ => false,
        };
        self.chosen = chosen;
        if same {
            return;
        }
        self.entry = entry;
        self.read(cx);
    }

    /// Reads the properties of the file shown, away from the user
    /// interface.
    fn read(&mut self, cx: &mut Context<Self>) {
        self.values = None;
        self.problem = None;
        self.reading = None;
        let Some(entry) = self.entry.clone() else {
            cx.notify();
            return;
        };
        let answer = Properties::read(vec![entry.path.clone()], FileProperty::DETAILS.to_vec());
        self.reading = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let values = answer.recv().await.unwrap_or_default();
                this.update(cx, |this, cx| {
                    if this.entry.as_ref().map(|shown| &shown.path) != Some(&entry.path) {
                        return;
                    }
                    this.reading = None;
                    let values: Vec<(FileProperty, Option<PropertyValue>)> = { values.into_iter() }
                        .next()
                        .map(|values| FileProperty::DETAILS.into_iter().zip(values).collect())
                        .unwrap_or_default();
                    this.take_values(values, cx);
                    cx.notify();
                })
                .ok();
            }),
        );
        cx.notify();
    }

    /// Puts the values read into the fields that edit them.
    fn take_values(
        &mut self,
        values: Vec<(FileProperty, Option<PropertyValue>)>,
        cx: &mut Context<Self>,
    ) {
        let value_of = |property: FileProperty| {
            { values.iter() }
                .find(|(each, _)| *each == property)
                .and_then(|(_, value)| value.clone())
        };
        for (property, field) in &self.fields {
            let text = match value_of(*property) {
                Some(value) if *property == FileProperty::Tags => value.items().join("; "),
                Some(value) => value.shown,
                None => String::new(),
            };
            field.update(cx, |field, cx| field.set_text(&text, false, cx));
        }
        self.stars_read = { value_of(FileProperty::Rating) }
            .and_then(|value| value.stars)
            .unwrap_or_default();
        self.stars = self.stars_read;
        self.values = Some(values);
    }

    /// What was changed and not yet written.
    fn edits(&self, cx: &gpui::App) -> Vec<(FileProperty, PropertyEdit)> {
        let Some(values) = &self.values else {
            return Vec::new();
        };
        let read = |property: FileProperty| {
            { values.iter() }
                .find(|(each, _)| *each == property)
                .and_then(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        let mut edits = Vec::new();
        for (property, field) in &self.fields {
            let text = field.read(cx).text();
            let was = read(*property);
            let edit = match property {
                FileProperty::Tags => {
                    let items = crate::platform::split_items(&text.replace(',', ";"));
                    (items != was.items()).then_some(PropertyEdit::Items(items))
                }
                _ => (text.trim() != was.shown.trim()).then_some(PropertyEdit::Text(text)),
            };
            edits.extend(edit.map(|edit| (*property, edit)));
        }
        if self.stars != self.stars_read {
            edits.push((FileProperty::Rating, PropertyEdit::Stars(self.stars)));
        }
        edits
    }

    /// Writes what was changed into the file, and reads it again once it
    /// is written.
    fn save(&mut self, cx: &mut Context<Self>) {
        let edits = self.edits(cx);
        let Some(entry) = self.entry.clone() else {
            return;
        };
        if edits.is_empty() || self.saving {
            return;
        }
        self.saving = true;
        let written = Properties::write(entry.path.clone(), edits);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let written = written.recv().await;
            this.update(cx, |this, cx| {
                this.saving = false;
                match written {
                    Ok(Ok(())) => {
                        cx.emit(DetailsEvent::Written(entry.path.clone()));
                        this.read(cx)
                    }
                    Ok(Err(error)) => this.problem = Some(error),
                    Err(_) => this.problem = Some("the properties were not written".to_owned()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Puts the fields back as the file has them.
    fn revert(&mut self, cx: &mut Context<Self>) {
        if let Some(values) = self.values.take() {
            self.take_values(values, cx);
        }
        cx.notify();
    }

    /// Whether a field of the pane has the keys.
    pub fn typing(&self, window: &Window, cx: &gpui::App) -> bool {
        { self.fields.iter() }.any(|(_, field)| field.read(cx).is_focused(window))
    }

    /// Does what a key asks while a field of the pane has the keys: Enter
    /// writes, Escape puts the fields back.
    pub fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => self.save(cx),
            "escape" => self.revert(cx),
            _ => {}
        }
    }

    fn render_picture(&self, entry: &Entry, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let side = px(PICTURE_SIDE);
        let scale = window.scale_factor();
        let picture = { self.thumbs.clone() }.update(cx, |thumbs, cx| {
            thumbs.picture_or_icon(entry, PICTURE_SIDE, scale, cx)
        });
        let element = match picture {
            Picture::Drawn(image) => img(image)
                .size(side)
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            Picture::Icon if entry.folder => div()
                .font_family(theme.typography.icon_family)
                .text_size(side * 0.6)
                .text_color(theme.colors.primary)
                .child(ICON_FOLDER)
                .into_any_element(),
            Picture::Icon => match cx.global::<AppIcons>().of_file(&entry.path) {
                Some(image) => img(image).size(side * 0.5).into_any_element(),
                None => div().into_any_element(),
            },
        };
        div()
            .h(side)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(element)
            .into_any_element()
    }

    fn render_stars(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let stars = (1..=5u8).map(|star| {
            let glyph = match star <= self.stars {
                true => ICON_STAR,
                false => ICON_STAR_EMPTY,
            };
            div()
                .id(("files-details-star", star as usize))
                .cursor_pointer()
                .text_color(theme.colors.primary)
                .child(icon(theme, glyph))
                .on_click(cx.listener(move |this, _, _, cx| {
                    // A click on the last star lit takes the rating away.
                    this.stars = if this.stars == star { 0 } else { star };
                    cx.notify();
                }))
        });
        div()
            .flex()
            .gap(theme.spacing.tiny)
            .children(stars)
            .into_any_element()
    }
}

impl Render for DetailsPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        if !self.open {
            return div().into_any_element();
        }
        let frame = div()
            .id("files-details")
            .w(px(DETAILS_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .mr(theme.spacing.small)
            .p(theme.spacing.medium)
            .rounded(theme.shape.large)
            .overflow_y_scroll()
            .bg(theme.colors.surface_container_highest.opacity(0.35))
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface);
        let Some(entry) = self.entry.clone() else {
            let text = match self.chosen {
                0 => language.say("Select an item to see its details").to_owned(),
                count => format!("{count} {}", language.say("selected")),
            };
            return frame
                .items_center()
                .justify_center()
                .text_color(theme.colors.on_surface_variant)
                .child(text)
                .into_any_element();
        };
        let dim = theme.colors.on_surface_variant;
        let small = theme.typography.label_size * 0.85;
        let row = |label: &'static str, value: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(small)
                        .text_color(dim)
                        .child(language.say(label)),
                )
                .child(value)
        };
        let mut facts = vec![one_line(entry.kind()).into_any_element()];
        if !entry.folder {
            facts.push(one_line(size_text(entry.size)).into_any_element());
        }
        if self.chosen > 1 {
            let text = format!("{} {}", self.chosen, language.say("selected"));
            facts.push(one_line(text).into_any_element());
        }
        let mut rows: Vec<AnyElement> = Vec::new();
        match &self.values {
            None => rows.push(
                div()
                    .text_color(dim)
                    .child(language.say("Reading…"))
                    .into_any_element(),
            ),
            Some(values) => {
                for (property, value) in values {
                    let element = match property {
                        FileProperty::Rating => Some(self.render_stars(&theme, cx)),
                        property if WRITTEN.contains(property) => { self.fields.iter() }
                            .find(|(each, _)| each == property)
                            .map(|(_, field)| field.clone().into_any_element()),
                        _ => value
                            .as_ref()
                            .map(|value| one_line(value.shown.clone()).into_any_element()),
                    };
                    if let Some(element) = element {
                        rows.push(row(property.title(), element).into_any_element());
                    }
                }
            }
        }
        let changed = !self.edits(cx).is_empty();
        let save = changed.then(|| {
            div()
                .id("files-details-save")
                .flex_none()
                .px(theme.spacing.medium)
                .py(theme.spacing.tiny)
                .rounded(theme.shape.medium)
                .cursor_pointer()
                .bg(theme.colors.secondary_container)
                .hover(|style| style.bg(theme.colors.surface_container_highest))
                .when(self.saving, |button| button.opacity(0.6))
                .child(language.say("Save"))
                .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
        });
        let problem = self.problem.clone().map(|problem| {
            div()
                .text_size(small)
                .text_color(gpui::rgb(0xe5534b))
                .child(problem)
        });
        frame
            .child(self.render_picture(&entry, window, cx))
            .child(
                div()
                    .text_size(theme.typography.label_size * 1.1)
                    .child(one_line(entry.display.clone())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .text_size(small)
                    .text_color(dim)
                    .children(facts),
            )
            .children(rows)
            .children(save)
            .children(problem)
            .into_any_element()
    }
}
