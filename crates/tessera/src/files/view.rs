// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A window of the file manager: a title bar of its own, the way back and
//! the path of the pane in front above, the places and drives at the
//! side, the pane that shows a folder, and a status bar below.

use gpui::{
    AppContext, Context, Entity, Hsla, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
    WindowControlArea, div, prelude::FluentBuilder, px,
};

use super::model::Location;
use super::pane::{FolderPane, ICON_HIDDEN, ICON_NEW_FOLDER, PaneEvent};
use super::sidebar::{Sidebar, SidebarEvent};
use super::{Files, FilesConfig, Look};
use crate::theme::Theme;
use crate::ui::{icon, logo, one_line};

/// Measures of the window, in logical pixels.
const TITLE_HEIGHT: f32 = 40.;
const TOOLBAR_HEIGHT: f32 = 48.;
const STATUS_HEIGHT: f32 = 28.;
const CAPTION_BUTTON_WIDTH: f32 = 46.;

/// Characters of the icon font.
const ICON_BACK: &str = "\u{e72b}";
const ICON_FORWARD: &str = "\u{e72a}";
const ICON_UP: &str = "\u{e74a}";
const ICON_REFRESH: &str = "\u{e72c}";
const ICON_DETAILS: &str = "\u{e8fd}";
const ICON_TILES: &str = "\u{f0e2}";
const ICON_MINIMIZE: &str = "\u{e921}";
const ICON_MAXIMIZE: &str = "\u{e922}";
const ICON_CLOSE: &str = "\u{e8bb}";

/// The root view of a window of the file manager.
pub struct FilesView {
    pane: Entity<FolderPane>,
    sidebar: Entity<Sidebar>,
    _subscriptions: Vec<Subscription>,
}

impl FilesView {
    pub fn new(
        location: Location,
        config: FilesConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pane = cx.new(|cx| FolderPane::new(location.clone(), config, window, cx));
        let sidebar = cx.new(|cx| Sidebar::new(location, cx));
        let subscriptions = vec![
            cx.subscribe_in(&pane, window, Self::pane_event),
            cx.subscribe_in(&sidebar, window, Self::sidebar_event),
            cx.observe(&pane, |_, _, cx| cx.notify()),
        ];
        pane.read(cx).focus(window);
        let view = Self {
            pane,
            sidebar,
            _subscriptions: subscriptions,
        };
        view.retitle(window, cx);
        view
    }

    fn pane_event(
        &mut self,
        _: &Entity<FolderPane>,
        event: &PaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaneEvent::Navigated | PaneEvent::Focused => self.retitle(window, cx),
        }
    }

    fn sidebar_event(
        &mut self,
        _: &Entity<Sidebar>,
        event: &SidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SidebarEvent::Go(location) => {
                let location = location.clone();
                self.pane
                    .update(cx, |pane, cx| pane.go(location, window, cx));
            }
        }
    }

    /// Titles the window after the pane in front, and lights its place
    /// at the side.
    fn retitle(&self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.pane.read(cx);
        let (title, location) = (pane.title(cx), pane.location().clone());
        window.set_window_title(&title);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_current(location, cx));
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let taken = self
            .pane
            .update(cx, |pane, cx| pane.key_down(event, window, cx));
        if taken {
            return;
        }
        let key = event.keystroke.key.as_str();
        let held = &event.keystroke.modifiers;
        match (key, held.control, held.alt, held.shift) {
            ("n", true, false, false) => {
                Files::open(self.pane.read(cx).location().clone(), cx);
            }
            ("w", true, false, false) => window.remove_window(),
            _ => {}
        }
    }
}

/// A square button of the window with `glyph`, dimmed unless `enabled`.
pub fn tool_button(
    theme: &Theme,
    id: &'static str,
    glyph: &'static str,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    let color = match enabled {
        true => theme.colors.on_surface,
        false => theme.colors.on_surface_variant.opacity(0.45),
    };
    div()
        .id(id)
        .size(px(34.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.shape.small)
        .text_color(color)
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(theme.colors.surface_container_highest))
        })
        .child(icon(theme, glyph))
}

impl FilesView {
    fn render_title(&self, theme: &Theme, window: &Window) -> impl IntoElement + use<> {
        let title: SharedString = window.window_title().into();
        let caption =
            |id: &'static str, glyph: &'static str, area: WindowControlArea, close: bool| {
                let hover: Hsla = match close {
                    true => gpui::rgb(0xc42b1c).into(),
                    false => theme.colors.surface_container_highest,
                };
                div()
                    .id(id)
                    .w(px(CAPTION_BUTTON_WIDTH))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.))
                    .font_family(theme.typography.icon_family)
                    .text_color(theme.colors.on_surface)
                    .hover(move |style| style.bg(hover))
                    .window_control_area(area)
                    .child(glyph)
            };
        div()
            .h(px(TITLE_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .child(
                div()
                    .id("files-drag")
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .px(theme.spacing.medium)
                    .window_control_area(WindowControlArea::Drag)
                    .child(logo(px(16.), theme.colors.primary))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(theme.typography.label_size)
                            .text_color(theme.colors.on_surface)
                            .child(one_line(title)),
                    ),
            )
            .child(caption(
                "files-min",
                ICON_MINIMIZE,
                WindowControlArea::Min,
                false,
            ))
            .child(caption(
                "files-max",
                ICON_MAXIMIZE,
                WindowControlArea::Max,
                false,
            ))
            .child(caption(
                "files-close",
                ICON_CLOSE,
                WindowControlArea::Close,
                true,
            ))
    }

    /// The way back and forth, the path and the filter of the pane in
    /// front, and how it shows what it holds.
    fn render_toolbar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let pane = self.pane.clone();
        let on_pane = |deed: fn(&mut FolderPane, &mut Window, &mut Context<FolderPane>)| {
            let pane = pane.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
                pane.update(cx, |pane, cx| deed(pane, window, cx));
            }
        };
        let (back, forward, up, folder, hidden, look) = {
            let pane = self.pane.read(cx);
            (
                pane.can_go_back(),
                pane.can_go_forward(),
                pane.can_go_up(),
                pane.folder().is_some(),
                pane.hidden(),
                pane.look(),
            )
        };
        let navigation = div()
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .child(
                tool_button(theme, "files-back", ICON_BACK, back)
                    .on_click(on_pane(|pane, window, cx| pane.back(window, cx))),
            )
            .child(
                tool_button(theme, "files-forward", ICON_FORWARD, forward)
                    .on_click(on_pane(|pane, window, cx| pane.forward(window, cx))),
            )
            .child(
                tool_button(theme, "files-up", ICON_UP, up)
                    .on_click(on_pane(|pane, window, cx| pane.up(window, cx))),
            )
            .child(
                tool_button(theme, "files-refresh", ICON_REFRESH, true)
                    .on_click(on_pane(|pane, _, cx| pane.load(cx))),
            );
        let address = self
            .pane
            .update(cx, |pane, cx| pane.render_address(theme, cx));
        let filter = self.pane.read(cx).filter_field();
        let look_button = |id: &'static str, shown: Look, glyph: &'static str| {
            tool_button(theme, id, glyph, true).when(look == shown, |button| {
                button
                    .bg(theme.colors.secondary_container)
                    .text_color(theme.colors.on_surface)
            })
        };
        let looks = div()
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .child(
                tool_button(theme, "files-new", ICON_NEW_FOLDER, folder)
                    .on_click(on_pane(|pane, window, cx| pane.new_folder(window, cx))),
            )
            .child(
                look_button("files-details", Look::Details, ICON_DETAILS)
                    .on_click(on_pane(|pane, _, cx| pane.set_look(Look::Details, cx))),
            )
            .child(
                look_button("files-tiles", Look::Tiles, ICON_TILES)
                    .on_click(on_pane(|pane, _, cx| pane.set_look(Look::Tiles, cx))),
            )
            .child(
                tool_button(theme, "files-hidden", ICON_HIDDEN, true)
                    .when(hidden, |button| button.text_color(theme.colors.primary))
                    .on_click(on_pane(|pane, _, cx| pane.toggle_hidden(cx))),
            );
        div()
            .h(px(TOOLBAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .px(theme.spacing.small)
            .child(navigation)
            .child(address)
            .child(div().w(px(220.)).flex_none().child(filter))
            .child(looks)
    }

    fn render_status(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let text = self.pane.read(cx).status_text(cx);
        div()
            .h(px(STATUS_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .px(theme.spacing.medium)
            .text_size(theme.typography.label_size * 0.85)
            .text_color(theme.colors.on_surface_variant)
            .child(text)
    }
}

impl Render for FilesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        div()
            .id("files")
            .on_key_down(cx.listener(|this, event, window, cx| this.key_down(event, window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.surface_container)
            .font_family(theme.typography.family)
            .text_color(theme.colors.on_surface)
            .child(self.render_title(&theme, window))
            .child(self.render_toolbar(&theme, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.sidebar.clone())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .mr(theme.spacing.small)
                            .rounded(theme.shape.large)
                            .bg(theme.colors.surface_container_highest.opacity(0.35))
                            .overflow_hidden()
                            .child(self.pane.clone()),
                    ),
            )
            .child(self.render_status(&theme, cx))
    }
}
