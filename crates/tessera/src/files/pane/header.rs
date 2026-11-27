// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The headings of the details and the cells under them: a click orders
//! by a column, a right click opens the menu that shows and hides
//! columns, a heading is dragged to another place among them, and the
//! edge before a heading is dragged to make its column wider or
//! narrower. The values of the columns of the Shell are read for the
//! rows in sight only, a batch a frame, on a thread of their own, and
//! kept while the folder is shown; ordering by such a column reads them
//! for every row.

use std::path::PathBuf;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, DragMoveEvent, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div, prelude::FluentBuilder, px,
};

use super::super::columns::{Columns, ShownColumn, choosable};
use super::super::model::{Column, Entry, Location, SortKey, SortKeys};
use super::super::session::{FolderViews, columns_key};
use super::{
    FolderPane, HEADER_HEIGHT, ICON_CHECK, ICON_NONE, ICON_SORT_DOWN, ICON_SORT_UP, moment,
};
use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::platform::{CloudState, FileProperty, Properties, PropertyValue};
use crate::popup::PopupAnchor;
use crate::theme::Theme;
use crate::ui::{icon, one_line};

/// How many files have their properties read in one batch.
const BATCH: usize = 64;

/// Characters of the icon font for the states of cloud files.
const ICON_CLOUD: &str = "\u{e753}";
const ICON_KEPT: &str = "\u{ec61}";
const ICON_AVAILABLE: &str = "\u{e930}";

/// A heading being dragged to another place among the headings.
struct HeadingDrag {
    index: usize,
    title: SharedString,
}

/// The edge before the heading at `index`, being dragged.
struct EdgeDrag {
    index: usize,
}

/// What follows the pointer while a heading is dragged: its title.
struct HeadingGhost {
    title: SharedString,
}

impl Render for HeadingGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        div()
            .px(theme.spacing.medium)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.medium)
            .bg(theme.colors.secondary_container)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface)
            .child(self.title.clone())
    }
}

/// What follows the pointer while an edge is dragged: nothing, as the
/// edge itself moves.
struct EdgeGhost;

impl Render for EdgeGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The glyph and the text of the state of a cloud file.
pub(super) fn cloud_glyph(state: CloudState) -> &'static str {
    match state {
        CloudState::OnlineOnly => ICON_CLOUD,
        CloudState::AlwaysKept => ICON_KEPT,
        CloudState::Available => ICON_AVAILABLE,
    }
}

impl FolderPane {
    /// The columns the details show: those chosen for the location, with
    /// the state of cloud files where the folder holds any.
    pub(super) fn shown_columns(&self) -> Columns {
        match self.cloudy {
            true => self.columns.with_status(),
            false => self.columns.clone(),
        }
    }

    /// Takes the columns the location was last shown with, or those it
    /// starts with.
    pub(super) fn take_columns(&mut self, cx: &mut Context<Self>) {
        let location = self.history.current();
        self.columns = FolderViews::columns(&columns_key(location), cx)
            .unwrap_or_else(|| Columns::first_of(location));
    }

    /// Keeps the columns of the location for the next time it is shown.
    pub(super) fn keep_columns(&mut self, cx: &mut Context<Self>) {
        self.columns_changed = false;
        let key = columns_key(self.history.current());
        FolderViews::put_columns(key, &self.columns, cx);
    }

    /// Shows `column`, or hides it where it is shown.
    fn toggle_column(&mut self, column: Column, cx: &mut Context<Self>) {
        // A state shown because the folder holds cloud files is hidden by
        // taking it into the chosen columns first.
        if column == Column::Status && self.cloudy && !self.columns.has(Column::Status) {
            self.columns = self.columns.with_status();
        }
        self.columns.toggle(column);
        self.keep_columns(cx);
        self.read_wanted(cx);
        self.arrange(cx);
    }

    /// Moves the column at `from` among the headings to `to`.
    fn reorder_columns(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        self.columns = self.shown_columns();
        self.columns.reorder(from, to);
        self.keep_columns(cx);
        cx.notify();
    }

    /// Makes the column at `index` as wide as the pointer of `event`
    /// reaches from its right edge, which stays where it is, as the name
    /// takes what the other columns leave.
    fn drag_edge(&mut self, index: usize, event: &DragMoveEvent<EdgeDrag>, cx: &mut Context<Self>) {
        let theme = *cx.global::<Theme>();
        self.columns = self.shown_columns();
        let shown = self.columns.shown();
        let gap = theme.spacing.small / px(1.);
        let after: f32 = { shown.iter().skip(index + 1) }
            .map(|shown| shown.width + gap)
            .sum();
        let right = (event.bounds.right() - theme.spacing.small) / px(1.) - after;
        let width = right - event.event.position.x / px(1.);
        self.columns.resize(index, width);
        self.columns_changed = true;
        cx.notify();
    }

    /// Opens the menu of the headings under `position`: every column that
    /// can be shown, those shown checked.
    fn open_column_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let anchor = match PopupAnchor::under(window, position, cx) {
            Ok(anchor) => anchor,
            Err(error) => {
                report!("the menu has no place: {error:#}");
                return;
            }
        };
        let language = Language::of(cx);
        let shown = self.shown_columns();
        let pane = cx.entity().downgrade();
        let entries = { choosable().into_iter() }
            .map(|column| {
                let pane = pane.clone();
                let mark = match shown.has(column) {
                    true => ICON_CHECK,
                    false => ICON_NONE,
                };
                MenuEntry::new(mark, language.say(column.title()), move |cx| {
                    pane.update(cx, |pane, cx| pane.toggle_column(column, cx))
                        .ok();
                })
            })
            .collect();
        Menus::open(entries, anchor, cx);
    }

    /// The headings of the details, over the cells of the rows.
    pub(super) fn render_header(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let language = Language::of(cx);
        let order = self.order();
        let icon_room = self.look.shape(self.zoom).icon;
        let marker = theme.colors.primary;
        let columns = self.shown_columns();
        let headings = columns.shown().iter().enumerate().map(|(index, shown)| {
            let column = shown.column;
            let title: SharedString = language.say(column.title()).into();
            let arrow = (order.column == column).then(|| {
                let glyph = match order.descending {
                    true => ICON_SORT_DOWN,
                    false => ICON_SORT_UP,
                };
                icon(theme, glyph).text_size(px(9.))
            });
            let edge = (index > 0).then(|| {
                div()
                    .id(("files-heading-edge", index))
                    .absolute()
                    .left(px(-5.))
                    .top_0()
                    .h_full()
                    .w(px(7.))
                    .cursor_col_resize()
                    .hover(|style| style.bg(theme.colors.primary.opacity(0.3)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_drag(EdgeDrag { index }, |_, _, _, cx| cx.new(|_| EdgeGhost))
            });
            let heading = Self::cell(*shown, theme)
                .id(("files-heading", index))
                .relative()
                .h_full()
                .flex()
                .items_center()
                .gap(theme.spacing.tiny)
                .cursor_pointer()
                .hover(|style| style.text_color(theme.colors.on_surface))
                .child(div().min_w_0().child(one_line(title.clone())))
                .children(arrow)
                .children(edge)
                .on_click(cx.listener(move |this, _, _, cx| this.sort_by(column, cx)))
                .when(index > 0, |heading| {
                    heading
                        .on_drag(HeadingDrag { index, title }, |drag, _, _, cx| {
                            let title = drag.title.clone();
                            cx.new(|_| HeadingGhost { title })
                        })
                        .drag_over::<HeadingDrag>(move |style, _, _, _| {
                            style.border_l_2().border_color(marker)
                        })
                        .on_drop(cx.listener(move |this, drag: &HeadingDrag, _, cx| {
                            this.reorder_columns(drag.index, index, cx)
                        }))
                });
            match column.to_the_right() {
                true => heading.justify_end(),
                false => heading,
            }
        });
        div()
            .id("files-header")
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .mx(theme.spacing.small)
            .px(theme.spacing.small)
            .text_size(theme.typography.label_size * 0.85)
            .text_color(theme.colors.on_surface_variant)
            .border_b_1()
            .border_color(theme.colors.outline.opacity(0.25))
            .child(div().w(px(icon_room)).flex_none())
            .children(headings)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_column_menu(event.position, window, cx)
                }),
            )
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<EdgeDrag>, _, cx| {
                let index = event.drag(cx).index;
                this.drag_edge(index, event, cx)
            }))
            .on_drop(cx.listener(|this, _: &EdgeDrag, _, cx| this.keep_columns(cx)))
            .into_any_element()
    }

    /// A cell of the column `shown`, as wide as it is; the name takes
    /// what is left.
    pub(super) fn cell(shown: ShownColumn, theme: &Theme) -> gpui::Div {
        match shown.column {
            Column::Name => div().flex_1().min_w_0(),
            column => div()
                .flex_none()
                .w(px(shown.width))
                .overflow_hidden()
                .when(column.to_the_right(), |cell| cell.flex().justify_end()),
        }
        .px(theme.spacing.tiny)
    }

    /// What the cell `column` of `entry` holds: its text, with the glyph
    /// of its state for a cloud file.
    pub(super) fn cell_content(&self, entry: &Entry, column: Column, theme: &Theme) -> AnyElement {
        if column == Column::Status {
            let Some(state) = entry.cloud else {
                return div().into_any_element();
            };
            let color = match state {
                CloudState::OnlineOnly => theme.colors.primary,
                _ => gpui::rgb(0x3fb950).into(),
            };
            return div()
                .flex()
                .items_center()
                .gap(theme.spacing.tiny)
                .child(
                    div()
                        .text_color(color)
                        .child(icon(theme, cloud_glyph(state))),
                )
                .child(one_line(self.say(state.title())))
                .into_any_element();
        }
        one_line(self.cell_text(entry, column)).into_any_element()
    }

    /// `text` in the language of the user.
    fn say(&self, text: &'static str) -> &'static str {
        self.language.say(text)
    }

    /// The value of `property` of `entry`, where it was read; one that was
    /// not is asked for once the frame is drawn.
    pub(super) fn property_of(
        &self,
        entry: &Entry,
        property: FileProperty,
    ) -> Option<&PropertyValue> {
        match self.properties.get(&(entry.path.clone(), property)) {
            Some(value) => value.as_ref(),
            None => {
                if let Ok(mut wanted) = self.wanted.try_borrow_mut() {
                    wanted.insert(entry.path.clone());
                }
                None
            }
        }
    }

    /// Reads the properties of the shown columns of the rows that wanted
    /// them in the frame drawn, a batch at a time.
    pub(super) fn read_wanted(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<PathBuf> = match self.wanted.try_borrow_mut() {
            Ok(mut wanted) => wanted.drain().collect(),
            Err(_) => return,
        };
        self.read_properties(wanted, cx);
    }

    /// Reads the properties of the shown columns of the files at `paths`
    /// that are neither known nor asked for yet.
    fn read_properties(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let properties = self.shown_columns().properties();
        if properties.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = { paths.into_iter() }
            .filter(|path| {
                { properties.iter() }.any(|property| {
                    let key = (path.clone(), *property);
                    !self.properties.contains_key(&key) && !self.properties_asked.contains(&key)
                })
            })
            .collect();
        for batch in paths.chunks(BATCH) {
            let batch = batch.to_vec();
            for path in &batch {
                for property in &properties {
                    self.properties_asked.insert((path.clone(), *property));
                }
            }
            let answer = Properties::read(batch.clone(), properties.clone());
            let wanted = properties.clone();
            let location = self.history.current().clone();
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let values = answer.recv().await.unwrap_or_default();
                this.update(cx, |this, cx| {
                    for path in &batch {
                        for property in &wanted {
                            this.properties_asked.remove(&(path.clone(), *property));
                        }
                    }
                    // What was read for a place left is of no use.
                    if *this.history.current() != location {
                        return;
                    }
                    for (path, values) in batch.into_iter().zip(values) {
                        for (property, value) in wanted.iter().zip(values) {
                            this.properties.insert((path.clone(), *property), value);
                        }
                    }
                    if this.order().column.property().is_some() {
                        this.arrange(cx);
                    } else {
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    /// What the entries are ordered by where the column of the order holds
    /// values the entries do not: the properties read, for which the rest
    /// are asked, or where an item of the Recycle Bin came from.
    pub(super) fn sort_keys(&mut self, cx: &mut Context<Self>) -> Option<SortKeys> {
        match self.order().column {
            Column::Original => Some(self.bin.keys()),
            Column::Property(property) => {
                let entries = self.listing.entries();
                let mut keys = SortKeys::new();
                let mut missing = Vec::new();
                for entry in entries {
                    match self.properties.get(&(entry.path.clone(), property)) {
                        Some(Some(value)) => {
                            let key = SortKey {
                                number: value.order,
                                text: value.shown.clone(),
                            };
                            keys.insert(entry.path.clone(), key);
                        }
                        Some(None) => {}
                        None => missing.push(entry.path.clone()),
                    }
                }
                if !missing.is_empty() {
                    self.read_properties(missing, cx);
                }
                Some(keys)
            }
            _ => None,
        }
    }

    /// Forgets the properties read, as when another place is shown.
    pub(super) fn forget_properties(&mut self) {
        self.properties.clear();
        self.properties_asked.clear();
        if let Ok(mut wanted) = self.wanted.try_borrow_mut() {
            wanted.clear();
        }
    }

    /// The text of the cell `column` of `entry`.
    pub(super) fn cell_text(&self, entry: &Entry, column: Column) -> String {
        match column {
            Column::Name => entry.display.clone(),
            Column::Modified | Column::Deleted => moment(entry.modified),
            Column::Kind => entry.kind(),
            Column::Size if entry.folder => String::new(),
            Column::Size => super::size_text(entry.size),
            Column::Folder => match (entry.path.parent(), self.history.current().root()) {
                (Some(parent), Some(root)) => match parent.strip_prefix(root) {
                    Ok(inner) if inner.as_os_str().is_empty() => ".".to_owned(),
                    Ok(inner) => inner.display().to_string(),
                    Err(_) => parent.display().to_string(),
                },
                (Some(parent), None) => parent.display().to_string(),
                (None, _) => String::new(),
            },
            Column::Original => self.bin.original_folder(&entry.path).unwrap_or_default(),
            Column::Status => { entry.cloud }
                .map(|state| self.say(state.title()).to_owned())
                .unwrap_or_default(),
            Column::Property(property) => { self.property_of(entry, property) }
                .map(|value| value.shown.clone())
                .unwrap_or_default(),
        }
    }

    /// Whether the location shows columns of the Shell, whose values are
    /// read as rows come in sight.
    pub(super) fn reads_properties(&self) -> bool {
        !matches!(
            self.history.current(),
            Location::Computer | Location::RecycleBin
        ) && !self.shown_columns().properties().is_empty()
    }
}
