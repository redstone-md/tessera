// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A window of the file manager: a title bar of its own with the tabs,
//! the way back and the path of the pane in front below it, the places
//! and drives at the side, one pane or two side by side, and a status
//! bar.

use std::collections::HashMap;

use gpui::{
    AppContext, Context, DragMoveEvent, Entity, EntityId, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Window, WindowControlArea, div, prelude::FluentBuilder, px, relative,
};

use super::model::Location;
use super::pane::{FolderPane, ICON_HIDDEN, ICON_NEW_FOLDER, PaneEvent};
use super::session::{OpenTab, Opening, Session};
use super::sidebar::{Sidebar, SidebarEvent};
use super::tabs::{Tab, Tabs};
use super::{Files, FilesConfig, Look};
use crate::i18n::Language;
use crate::platform::FileWork;
use crate::theme::Theme;
use crate::ui::{icon, logo, one_line};

/// Measures of the window, in logical pixels.
const TITLE_HEIGHT: f32 = 40.;
const TOOLBAR_HEIGHT: f32 = 48.;
const STATUS_HEIGHT: f32 = 28.;
const CAPTION_BUTTON_WIDTH: f32 = 46.;
const TAB_WIDTH: f32 = 190.;
const DIVIDER_WIDTH: f32 = 6.;

/// Characters of the icon font.
const ICON_BACK: &str = "\u{e72b}";
const ICON_FORWARD: &str = "\u{e72a}";
const ICON_UP: &str = "\u{e74a}";
const ICON_REFRESH: &str = "\u{e72c}";
const ICON_DETAILS: &str = "\u{e8fd}";
const ICON_TILES: &str = "\u{f0e2}";
const ICON_SPLIT: &str = "\u{e89f}";
const ICON_ADD: &str = "\u{e710}";
const ICON_MINIMIZE: &str = "\u{e921}";
const ICON_MAXIMIZE: &str = "\u{e922}";
const ICON_CLOSE: &str = "\u{e8bb}";

/// A tab being dragged to another place in the strip.
struct TabDrag {
    index: usize,
    title: SharedString,
}

/// The divider between the panes of a split tab, being dragged.
struct DividerDrag;

/// What follows the pointer while a tab is dragged: its title.
struct TabGhost {
    title: SharedString,
}

impl Render for TabGhost {
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

/// What follows the pointer while the divider is dragged: nothing, as
/// the divider itself moves.
struct NoGhost;

impl Render for NoGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The root view of a window of the file manager.
pub struct FilesView {
    config: FilesConfig,
    tabs: Tabs<Entity<FolderPane>>,
    sidebar: Entity<Sidebar>,
    /// What the window hears from each of its panes.
    listening: HashMap<EntityId, [Subscription; 2]>,
    _sidebar: Subscription,
}

impl FilesView {
    pub fn new(
        opening: Opening,
        config: FilesConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut listening = HashMap::new();
        let mut tabs: Option<Tabs<Entity<FolderPane>>> = None;
        for open in &opening.tabs {
            let tab = Self::tab_of(open, config, &mut listening, window, cx);
            match &mut tabs {
                None => tabs = Some(Tabs::new(tab)),
                Some(tabs) => {
                    tabs.open(tab, false);
                }
            }
        }
        let mut tabs = tabs.unwrap_or_else(|| {
            let open = OpenTab::at(Files::home());
            Tabs::new(Self::tab_of(&open, config, &mut listening, window, cx))
        });
        tabs.select(opening.active);
        let current = tabs.active().active().read(cx).location().clone();
        let sidebar = cx.new(|cx| Sidebar::new(current, cx));
        let listened = cx.subscribe_in(&sidebar, window, Self::sidebar_event);
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            if let Some(this) = this.upgrade() {
                this.read(cx).keep_session(cx);
            }
            true
        });
        let mut view = Self {
            config,
            tabs,
            sidebar,
            listening,
            _sidebar: listened,
        };
        view.show(window, cx);
        view
    }

    /// A tab of panes at the locations of `open`.
    fn tab_of(
        open: &OpenTab,
        config: FilesConfig,
        listening: &mut HashMap<EntityId, [Subscription; 2]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Tab<Entity<FolderPane>> {
        let mut panes = open.panes.iter().take(2);
        let first = panes.next().cloned().unwrap_or_else(Files::home);
        let mut tab = Tab::single(Self::pane_at(first, config, listening, window, cx));
        if let Some(second) = panes.next() {
            let second = Self::pane_at(second.clone(), config, listening, window, cx);
            tab.split_with(second).ok();
        }
        tab.focus(open.active);
        tab.set_split(open.split);
        tab
    }

    /// A new pane at `location`, which the window listens to.
    fn pane_at(
        location: Location,
        config: FilesConfig,
        listening: &mut HashMap<EntityId, [Subscription; 2]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<FolderPane> {
        let pane = cx.new(|cx| FolderPane::new(location, config, window, cx));
        let subscriptions = [
            cx.subscribe_in(&pane, window, Self::pane_event),
            cx.observe(&pane, |_, _, cx| cx.notify()),
        ];
        listening.insert(pane.entity_id(), subscriptions);
        pane
    }

    fn new_pane(
        &mut self,
        location: Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<FolderPane> {
        Self::pane_at(location, self.config, &mut self.listening, window, cx)
    }

    /// The pane in front.
    fn pane(&self) -> &Entity<FolderPane> {
        self.tabs.active().active()
    }

    /// Tells every pane whether it is in sight, gives the keys to the pane
    /// in front, titles the window after it and lights its place at the
    /// side.
    fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let front = self.tabs.active_index();
        for (index, tab) in self.tabs.all().iter().enumerate() {
            let split = tab.is_split();
            for (place, pane) in tab.panes().iter().enumerate() {
                let lit = split && place == tab.active_index();
                pane.update(cx, |pane, cx| pane.place(index == front, split, lit, cx));
            }
        }
        self.pane().read(cx).focus(window);
        self.retitle(window, cx);
    }

    /// Titles the window after the pane in front, and lights its place
    /// at the side.
    fn retitle(&self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.pane().read(cx);
        let (title, location) = (pane.title(cx), pane.location().clone());
        window.set_window_title(&title);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_current(location, cx));
        cx.notify();
    }

    fn pane_event(
        &mut self,
        pane: &Entity<FolderPane>,
        event: &PaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaneEvent::Navigated => {
                if pane == self.pane() {
                    self.retitle(window, cx);
                }
            }
            PaneEvent::Focused => {
                let tab = self.tabs.active_mut();
                if let Some(place) = tab.panes().iter().position(|each| each == pane)
                    && place != tab.active_index()
                {
                    tab.focus(place);
                    self.show(window, cx);
                }
            }
            PaneEvent::OpenInNewTab(location) => {
                self.open_tab(location.clone(), true, window, cx);
            }
            PaneEvent::ToOtherPane { cut } => self.give_other_pane(*cut, cx),
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
                self.pane()
                    .update(cx, |pane, cx| pane.go(location, window, cx));
            }
        }
    }

    /// Opens a tab at `location`, in front unless `behind`.
    fn open_tab(
        &mut self,
        location: Location,
        behind: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = self.new_pane(location, window, cx);
        self.tabs.open(Tab::single(pane), behind);
        self.show(window, cx);
    }

    /// Closes the tab `index`, and the window with its last tab.
    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.close(index) else {
            self.close_window(window, cx);
            return;
        };
        let locations = { tab.panes().iter() }
            .map(|pane| pane.read(cx).location().clone())
            .collect();
        self.tabs.remember(locations);
        for pane in tab.panes() {
            self.listening.remove(&pane.entity_id());
        }
        self.show(window, cx);
    }

    /// Opens again the tab closed last.
    fn reopen_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(locations) = self.tabs.reopen() else {
            return;
        };
        let open = OpenTab {
            panes: locations,
            ..OpenTab::at(Location::Computer)
        };
        let tab = Self::tab_of(&open, self.config, &mut self.listening, window, cx);
        self.tabs.open(tab, false);
        self.show(window, cx);
    }

    fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.select(index);
        self.show(window, cx);
    }

    /// Splits the tab in front, with a second pane at the same location
    /// as the first, or takes away the pane that is not in front.
    fn toggle_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.tabs.active_mut();
        if let Some(other) = tab.unsplit() {
            self.listening.remove(&other.entity_id());
        } else {
            let location = tab.active().read(cx).location().clone();
            let pane = self.new_pane(location, window, cx);
            self.tabs.active_mut().split_with(pane).ok();
        }
        self.show(window, cx);
    }

    /// Puts the other pane of a split tab in front.
    fn switch_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.active_mut().switch() {
            self.show(window, cx);
        }
    }

    fn swap_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.active_mut().swap();
        self.show(window, cx);
    }

    /// Copies, or with `cut` moves, what is chosen in the pane in front
    /// into the folder of the pane beside it, as Dolphin does with
    /// Shift+F5 and Shift+F6, and reads both when the Shell is done.
    fn give_other_pane(&mut self, cut: bool, cx: &mut Context<Self>) {
        let tab = self.tabs.active();
        let (Some(other), front) = (tab.other().cloned(), tab.active().clone()) else {
            return;
        };
        let from = front.read(cx).chosen();
        let Some(to) = other.read(cx).folder() else {
            return;
        };
        if from.is_empty() {
            return;
        }
        let work = match cut {
            true => FileWork::Move { from, to },
            false => FileWork::Copy { from, to },
        };
        let (done, finished) = async_channel::bounded::<()>(1);
        work.start(move || {
            done.try_send(()).ok();
        });
        cx.spawn(async move |_, cx| {
            finished.recv().await.ok();
            for pane in [front, other] {
                pane.update(cx, |pane, cx| pane.load(cx)).ok();
            }
        })
        .detach();
    }

    /// The tabs as they are, to open again.
    fn opening(&self, cx: &gpui::App) -> Opening {
        let tabs = { self.tabs.all().iter() }
            .map(|tab| OpenTab {
                panes: { tab.panes().iter() }
                    .map(|pane| pane.read(cx).location().clone())
                    .collect(),
                active: tab.active_index(),
                split: tab.split(),
            })
            .collect();
        Opening {
            tabs,
            active: self.tabs.active_index(),
        }
    }

    /// Keeps the tabs for the next window, where the configuration asks
    /// for that, away from the user interface.
    fn keep_session(&self, cx: &gpui::App) {
        if !self.config.restore {
            return;
        }
        let opening = self.opening(cx);
        cx.background_executor()
            .spawn(async move {
                let mut session = Session::load();
                session.set_tabs(&opening);
                session.save();
            })
            .detach();
    }

    fn close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.keep_session(cx);
        window.remove_window();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let taken = self
            .pane()
            .clone()
            .update(cx, |pane, cx| pane.key_down(event, window, cx));
        if taken {
            return;
        }
        let key = event.keystroke.key.as_str();
        let held = &event.keystroke.modifiers;
        match (key, held.control, held.alt, held.shift) {
            ("n", true, false, false) => {
                Files::open(self.pane().read(cx).location().clone(), cx);
            }
            ("t", true, false, false) => self.open_tab(Files::home(), false, window, cx),
            ("t", true, false, true) => self.reopen_tab(window, cx),
            ("w", true, false, false) | ("f4", true, false, false) => {
                self.close_tab(self.tabs.active_index(), window, cx)
            }
            ("tab", true, false, back) => {
                self.tabs.step(if back { -1 } else { 1 });
                self.show(window, cx);
            }
            ("tab", false, false, false) => self.switch_pane(window, cx),
            ("f3", false, false, false) => self.toggle_split(window, cx),
            ("f5", false, false, true) => self.give_other_pane(false, cx),
            ("f6", false, false, true) => self.give_other_pane(true, cx),
            ("u", true, false, false) => self.swap_panes(window, cx),
            (digit, true, false, false) if digit.len() == 1 => {
                let Some(number) = digit.chars().next().and_then(|digit| digit.to_digit(10)) else {
                    return;
                };
                match number {
                    0 => {}
                    9 => self.select_tab(self.tabs.len() - 1, window, cx),
                    number => self.select_tab(number as usize - 1, window, cx),
                }
            }
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
    /// The tab `index` in the strip of the title bar: the title of its
    /// pane in front and a button that closes it.
    fn render_tab(&self, index: usize, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = &self.tabs.all()[index];
        let title: SharedString = tab.active().read(cx).title(cx).into();
        let front = index == self.tabs.active_index();
        let marker = theme.colors.primary;
        let drag = TabDrag {
            index,
            title: title.clone(),
        };
        let close = div()
            .id(("files-tab-close", index))
            .size(px(20.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.small)
            .font_family(theme.typography.icon_family)
            .text_size(px(9.))
            .text_color(theme.colors.on_surface_variant)
            .hover(|style| style.bg(theme.colors.surface_container_highest))
            .child(ICON_CLOSE)
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.close_tab(index, window, cx)
            }));
        div()
            .id(("files-tab", index))
            .h(px(32.))
            .w(px(TAB_WIDTH))
            .min_w(px(72.))
            .flex_shrink()
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .pl(theme.spacing.medium)
            .pr(theme.spacing.tiny)
            .rounded_t(theme.shape.medium)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface)
            .cursor_pointer()
            .when(front, |tab| {
                tab.bg(theme.colors.surface_container_highest.opacity(0.8))
            })
            .when(!front, |tab| {
                tab.hover(|style| style.bg(theme.colors.surface_container_highest.opacity(0.4)))
            })
            .when(tab.is_split(), |tab| {
                tab.child(
                    div()
                        .text_color(theme.colors.primary)
                        .child(icon(theme, ICON_SPLIT).text_size(px(11.))),
                )
            })
            .child(div().flex_1().min_w_0().child(one_line(title)))
            .child(close)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| this.select_tab(index, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| this.close_tab(index, window, cx)),
            )
            .on_drag(drag, |drag: &TabDrag, _, _, cx| {
                let title = drag.title.clone();
                cx.new(|_| TabGhost { title })
            })
            .drag_over::<TabDrag>(move |style, _, _, _| style.border_l_2().border_color(marker))
            .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                this.tabs.reorder(drag.index, index);
                this.show(window, cx);
            }))
    }

    fn render_title(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
        let tabs: Vec<_> = (0..self.tabs.len())
            .map(|index| self.render_tab(index, theme, cx).into_any_element())
            .collect();
        let add = tool_button(theme, "files-tab-add", ICON_ADD, true)
            .size(px(28.))
            .on_click(
                cx.listener(|this, _, window, cx| this.open_tab(Files::home(), false, window, cx)),
            );
        div()
            .h(px(TITLE_HEIGHT))
            .flex_none()
            .flex()
            .items_end()
            .child(
                div()
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(theme.spacing.medium)
                    .child(logo(px(16.), theme.colors.primary)),
            )
            .child(
                div()
                    .flex_shrink()
                    .min_w_0()
                    .flex()
                    .items_end()
                    .gap(px(2.))
                    .overflow_hidden()
                    .children(tabs),
            )
            .child(
                div()
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(theme.spacing.tiny)
                    .child(add),
            )
            .child(
                div()
                    .id("files-drag")
                    .flex_1()
                    .min_w(px(48.))
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(
                div()
                    .h_full()
                    .flex()
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
                    )),
            )
    }

    /// The way back and forth, the path and the filter of the pane in
    /// front, and how it shows what it holds.
    fn render_toolbar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let pane = self.pane().clone();
        let on_pane = |deed: fn(&mut FolderPane, &mut Window, &mut Context<FolderPane>)| {
            let pane = pane.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
                pane.update(cx, |pane, cx| deed(pane, window, cx));
            }
        };
        let (back, forward, up, folder, hidden, look) = {
            let pane = self.pane().read(cx);
            (
                pane.can_go_back(),
                pane.can_go_forward(),
                pane.can_go_up(),
                pane.folder().is_some(),
                pane.hidden(),
                pane.look(),
            )
        };
        let split = self.tabs.active().is_split();
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
            .pane()
            .update(cx, |pane, cx| pane.render_address(theme, cx));
        let filter = self.pane().read(cx).filter_field();
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
            )
            .child(
                tool_button(theme, "files-split", ICON_SPLIT, true)
                    .when(split, |button| button.text_color(theme.colors.primary))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_split(window, cx))),
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

    /// One pane, or the two of a split tab with the divider between them.
    fn render_body(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tab = self.tabs.active();
        let frame = |pane: &Entity<FolderPane>| {
            div()
                .h_full()
                .min_w_0()
                .rounded(theme.shape.large)
                .bg(theme.colors.surface_container_highest.opacity(0.35))
                .overflow_hidden()
                .child(pane.clone())
        };
        let body = div()
            .id("files-body")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .mr(theme.spacing.small);
        match tab.panes() {
            [first, second] => {
                let divider = div()
                    .id("files-divider")
                    .w(px(DIVIDER_WIDTH))
                    .h_full()
                    .flex_none()
                    .cursor_col_resize()
                    .hover(|style| style.bg(theme.colors.primary.opacity(0.3)))
                    .on_drag(DividerDrag, |_, _, _, cx| cx.new(|_| NoGhost));
                body.on_drag_move(
                    cx.listener(|this, event: &DragMoveEvent<DividerDrag>, _, cx| {
                        let bounds = event.bounds;
                        let width = bounds.size.width / px(1.);
                        if width > 0. {
                            let share = (event.event.position.x - bounds.left()) / px(1.) / width;
                            this.tabs.active_mut().set_split(share);
                            cx.notify();
                        }
                    }),
                )
                .child(frame(first).flex_none().w(relative(tab.split())))
                .child(divider)
                .child(frame(second).flex_1())
                .into_any_element()
            }
            [only, ..] => body.child(frame(only).flex_1()).into_any_element(),
            [] => body.into_any_element(),
        }
    }

    fn render_status(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let text = self.pane().read(cx).status_text(cx);
        let tabs = match self.tabs.len() {
            1 => None,
            count => Some(format!("{count} {}", Language::of(cx).say("tabs"))),
        };
        div()
            .h(px(STATUS_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .px(theme.spacing.medium)
            .text_size(theme.typography.label_size * 0.85)
            .text_color(theme.colors.on_surface_variant)
            .child(div().flex_1().min_w_0().child(one_line(text)))
            .children(tabs)
    }
}

impl Render for FilesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(self.render_title(&theme, cx))
            .child(self.render_toolbar(&theme, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.sidebar.clone())
                    .child(self.render_body(&theme, cx)),
            )
            .child(self.render_status(&theme, cx))
    }
}
