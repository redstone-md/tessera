// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The side of a window of the file manager: the places and drives the
//! pane in front can be sent to.

use std::path::PathBuf;

use gpui::{
    AsyncApp, Context, EventEmitter, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div, prelude::FluentBuilder, px,
};

use super::model::Location;
use super::pane::{ICON_BIN, ICON_COMPUTER, drive_glyph};
use crate::i18n::Language;
use crate::platform::{Drive, Place, RecycleBin};
use crate::theme::Theme;
use crate::ui::{icon, one_line, place_look};

/// How wide the side is, in logical pixels.
pub const SIDEBAR_WIDTH: f32 = 220.;

/// What the side asks of the window.
#[derive(Clone, Debug, PartialEq)]
pub enum SidebarEvent {
    /// To show `Location` in the pane in front.
    Go(Location),
}

/// The side of a window: the known folders, the computer and its drives.
pub struct Sidebar {
    folders: Vec<(Place, PathBuf)>,
    drives: Vec<Drive>,
    /// Where the pane in front is, which is lit.
    current: Location,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

impl Sidebar {
    pub fn new(current: Location, cx: &mut Context<Self>) -> Self {
        let mut sidebar = Self {
            folders: Vec::new(),
            drives: Vec::new(),
            current,
        };
        sidebar.read(cx);
        sidebar
    }

    /// Lights the place the pane in front is at.
    pub fn set_current(&mut self, current: Location, cx: &mut Context<Self>) {
        if self.current != current {
            self.current = current;
            cx.notify();
        }
    }

    /// Reads the known folders and the drives, away from the user
    /// interface, as a drive may take its time to answer.
    pub fn read(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (folders, drives) = cx
                .background_executor()
                .spawn(async {
                    let found = |place: Place| Some((place, place.path()?));
                    let folders: Vec<_> = Place::ALL.into_iter().filter_map(found).collect();
                    (folders, Drive::all())
                })
                .await;
            this.update(cx, |this, cx| {
                this.folders = folders;
                this.drives = drives;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let theme = &theme;
        let language = Language::of(cx);
        let current = self.current.clone();
        let item = |id: (&'static str, usize), glyph: &'static str, label: String, lit: bool| {
            div()
                .id(id)
                .h(px(34.))
                .flex_none()
                .flex()
                .items_center()
                .gap(theme.spacing.small)
                .px(theme.spacing.medium)
                .rounded(theme.shape.medium)
                .cursor_pointer()
                .text_color(theme.colors.on_surface)
                .when(lit, |item| item.bg(theme.colors.secondary_container))
                .when(!lit, |item| {
                    item.hover(|style| style.bg(theme.colors.surface_container_highest))
                })
                .child(
                    div()
                        .text_color(theme.colors.primary)
                        .child(icon(theme, glyph)),
                )
                .child(div().flex_1().min_w_0().child(one_line(label)))
        };
        let heading = |text: &'static str| {
            div()
                .pt(theme.spacing.medium)
                .pb(theme.spacing.tiny)
                .px(theme.spacing.medium)
                .text_size(theme.typography.label_size * 0.85)
                .text_color(theme.colors.on_surface_variant)
                .child(language.say(text))
        };
        let go = |target: Location| {
            cx.listener(move |_, _, _, cx: &mut Context<Self>| {
                cx.emit(SidebarEvent::Go(target.clone()))
            })
        };
        let mut column = div()
            .id("files-side")
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(theme.spacing.small)
            .overflow_y_scroll()
            .text_size(theme.typography.label_size)
            .child(heading("Places"));
        for (index, (place, path)) in self.folders.iter().enumerate() {
            let (label, glyph) = place_look(*place);
            let target = Location::Folder(path.clone());
            let lit = current == target;
            column = column.child(
                item(
                    ("files-place", index),
                    glyph,
                    language.say(label).to_owned(),
                    lit,
                )
                .on_click(go(target)),
            );
        }
        column = column.child(heading("This PC")).child(
            item(
                ("files-computer", 0),
                ICON_COMPUTER,
                language.say("This PC").to_owned(),
                current == Location::Computer,
            )
            .on_click(go(Location::Computer)),
        );
        for (index, drive) in self.drives.iter().enumerate() {
            let target = Location::Folder(drive.root.clone());
            let lit = current == target;
            column = column.child(
                item(
                    ("files-drive", index),
                    drive_glyph(drive.kind),
                    drive.title(),
                    lit,
                )
                .on_click(go(target)),
            );
        }
        column.child(
            item(
                ("files-bin", 0),
                ICON_BIN,
                language.say("Recycle Bin").to_owned(),
                false,
            )
            .on_click(|_, _, _| {
                if let Err(error) = RecycleBin::open() {
                    report!("the Recycle Bin did not open: {error}");
                }
            }),
        )
    }
}
