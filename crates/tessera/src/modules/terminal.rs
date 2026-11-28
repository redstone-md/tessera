// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `terminal` widget and popup, terminals in tabs; and the drop-down
//! terminal of the action `terminal-dropdown`, as Yakuake has it: it slides
//! down from the top of the display the pointer is on, keeps its shells
//! while it is hidden, and hides when another window takes the keys where
//! `[terminal] dropdown_hides` says so. Ctrl+Shift+T opens a tab, Ctrl+
//! Shift+W closes one and Ctrl+PageUp and PageDown go from one to the next.

use std::path::PathBuf;

use gpui::{
    App, AppContext, Bounds, Context, Entity, Global, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Window, WindowHandle, div, point, prelude::FluentBuilder, px, size,
};

use crate::actions::{Action, Actions};
use crate::config::{PopupConfig, WidgetSpec};
use crate::files::Files;
use crate::module::{Module, WidgetRegistry};
use crate::motion::Spring;
use crate::platform::{Display, DisplayKey, NativeSurface, ScreenRect, SmallBirth};
use crate::popup::floating_window;
use crate::terminal::{TerminalEvent, TerminalView, Terminals};
use crate::theme::Theme;
use crate::ui::{icon, one_line};

/// The name of the widget, of its popup and of the action.
const WIDGET: &str = "terminal";
const ACTION: &str = "terminal-dropdown";

/// The size of the popup, in logical pixels.
const POPUP_SIZE: (f32, f32) = (760., 440.);

/// How high the strip of tabs is, and how wide a tab.
const STRIP_HEIGHT: f32 = 32.;
const TAB_WIDTH: f32 = 180.;

const ICON_ADD: &str = "\u{e710}";
const ICON_CLOSE: &str = "\u{e8bb}";
const ICON_TERMINAL: &str = "\u{e756}";

pub struct TerminalModule;

impl Module for TerminalModule {
    fn name(&self) -> &'static str {
        WIDGET
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(WIDGET, |context, cx| {
            // The folder of the user where the option names none.
            let location = Files::named(context.text("folder").unwrap_or_default());
            let folder = location.folder().map(PathBuf::from);
            cx.new(|cx| TerminalTabs::new(folder, cx)).into()
        })?;
        widgets.register_popup(PopupConfig {
            name: WIDGET.to_owned(),
            place: Default::default(),
            width: POPUP_SIZE.0,
            height: POPUP_SIZE.1,
            content: vec![WidgetSpec {
                widget: WIDGET.to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        actions.register(ACTION, Action::new(|_, cx| Dropdown::toggle(cx)))
    }
}

/// What the terminals in tabs tell the view they are in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabsEvent {
    /// The last tab was closed; a new one stands in its place.
    Emptied,
}

/// Terminals in tabs, one of them in front.
pub struct TerminalTabs {
    terminals: Vec<(Entity<TerminalView>, Subscription)>,
    active: usize,
    /// Where the first terminal starts, and where a new one starts when
    /// the one in front knows no folder.
    start_in: Option<PathBuf>,
}

impl gpui::EventEmitter<TabsEvent> for TerminalTabs {}

impl TerminalTabs {
    pub fn new(start_in: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let mut tabs = Self {
            terminals: Vec::new(),
            active: 0,
            start_in,
        };
        let first = tabs.start_in.clone();
        tabs.add(first, cx);
        tabs
    }

    /// Adds a terminal starting in `folder` after the one in front, and
    /// puts it in front.
    fn add(&mut self, folder: Option<PathBuf>, cx: &mut Context<Self>) {
        let terminal = cx.new(|cx| TerminalView::new(folder, cx));
        let heard = cx.subscribe(&terminal, |_, _, event: &TerminalEvent, cx| {
            if matches!(event, TerminalEvent::Titled | TerminalEvent::Exited) {
                cx.notify();
            }
        });
        let at = (self.active + 1).min(self.terminals.len());
        self.terminals.insert(at, (terminal, heard));
        self.active = at;
        cx.notify();
    }

    /// The terminal in front.
    fn front(&self) -> &Entity<TerminalView> {
        &self.terminals[self.active].0
    }

    /// Gives the keys to the terminal in front.
    pub fn focus(&self, window: &mut Window, cx: &App) {
        self.front().read(cx).focus(window);
    }

    /// Opens a tab in the folder of the one in front.
    fn open_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folder = { self.front().read(cx).folder() }
            .map(PathBuf::from)
            .or_else(|| self.start_in.clone());
        self.add(folder, cx);
        self.focus(window, cx);
    }

    /// Closes the tab `index`, which ends its shell; the last one is
    /// replaced by a new terminal, and the view is told.
    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.terminals.len() {
            return;
        }
        // Its shell ends with it.
        drop(self.terminals.remove(index));
        if self.terminals.is_empty() {
            self.active = 0;
            let folder = self.start_in.clone();
            self.add(folder, cx);
            cx.emit(TabsEvent::Emptied);
        } else if index < self.active || self.active >= self.terminals.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.focus(window, cx);
        cx.notify();
    }

    fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.terminals.len() {
            self.active = index;
            self.focus(window, cx);
            cx.notify();
        }
    }

    /// Puts the tab `step` places on in front, going round at the ends.
    fn step(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.terminals.len() as isize;
        let index = (self.active as isize + step).rem_euclid(count) as usize;
        self.select(index, window, cx);
    }

    /// Takes the keys of the tabs before the terminal does.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let held = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        match (key, held.control, held.shift, held.alt) {
            ("t", true, true, false) => self.open_tab(window, cx),
            ("w", true, true, false) => self.close_tab(self.active, window, cx),
            ("pageup", true, false, false) => self.step(-1, window, cx),
            ("pagedown", true, false, false) => self.step(1, window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_tab(&self, index: usize, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let title: SharedString = self.terminals[index].0.read(cx).title(cx).into();
        let front = index == self.active;
        let close = div()
            .id(("terminal-tab-close", index))
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
                this.close_tab(index, window, cx);
            }));
        div()
            .id(("terminal-tab", index))
            .h(px(STRIP_HEIGHT - 4.))
            .w(px(TAB_WIDTH))
            .min_w(px(64.))
            .flex_shrink()
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .pl(theme.spacing.small)
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
            .child(
                div()
                    .text_color(theme.colors.primary)
                    .child(icon(theme, ICON_TERMINAL).text_size(px(12.))),
            )
            .child(div().flex_1().min_w_0().child(one_line(title)))
            .child(close)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| this.select(index, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| this.close_tab(index, window, cx)),
            )
    }
}

impl Render for TerminalTabs {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let tabs: Vec<_> = (0..self.terminals.len())
            .map(|index| self.render_tab(index, &theme, cx).into_any_element())
            .collect();
        let add = div()
            .id("terminal-tab-add")
            .size(px(26.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.small)
            .cursor_pointer()
            .hover(|style| style.bg(theme.colors.surface_container_highest))
            .child(icon(&theme, ICON_ADD))
            .on_click(cx.listener(|this, _, window, cx| this.open_tab(window, cx)));
        div()
            .id("terminal-tabs")
            .size_full()
            .min_h(px(120.))
            .flex()
            .flex_col()
            .font_family(theme.typography.family)
            .capture_key_down(cx.listener(Self::key_down))
            .child(
                div()
                    .h(px(STRIP_HEIGHT))
                    .flex_none()
                    .flex()
                    .items_end()
                    .gap(px(2.))
                    .px(theme.spacing.small)
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
                    .child(div().h_full().flex().items_center().child(add)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .rounded_b(theme.shape.medium)
                    .overflow_hidden()
                    .child(self.front().clone()),
            )
    }
}

/// The drop-down terminal: its window, kept while it is hidden.
#[derive(Default)]
struct Dropdown {
    window: Option<WindowHandle<DropdownView>>,
}

impl Global for Dropdown {}

impl Dropdown {
    /// Slides the drop-down terminal down on the display the pointer is on,
    /// or up and out of sight where it is shown.
    fn toggle(cx: &mut App) {
        let Some(frame) = Self::frame(cx) else {
            report!("the drop-down terminal finds no display");
            return;
        };
        let window = cx.default_global::<Self>().window;
        if let Some(window) = window {
            let toggled = window.update(cx, |view, window, cx| match view.shown() {
                true => view.hide(window, cx),
                false => view.show(frame, window, cx),
            });
            if toggled.is_ok() {
                return;
            }
        }
        match Self::open(frame, cx) {
            Ok(window) => cx.global_mut::<Self>().window = Some(window),
            Err(error) => report!("the drop-down terminal did not open: {error:#}"),
        }
    }

    /// Where the drop-down terminal stands: at the top of what the bars
    /// leave of the display the pointer is on, as wide and high as the
    /// configuration says.
    fn frame(cx: &App) -> Option<(ScreenRect, f32)> {
        let pointed = DisplayKey::under_pointer()?;
        let display = { Display::all().into_iter() }.find(|display| display.key == pointed)?;
        let config = Terminals::config(cx);
        let shares = (config.dropdown_width, config.dropdown_height);
        Some((dropdown_frame(display.work, shares), display.scale))
    }

    fn open(frame: (ScreenRect, f32), cx: &mut App) -> anyhow::Result<WindowHandle<DropdownView>> {
        let (rect, scale) = frame;
        let rough = Bounds {
            origin: point(px(rect.left as f32 / scale), px(rect.top as f32 / scale)),
            size: size(
                px((rect.right - rect.left) as f32 / scale),
                px((rect.bottom - rect.top) as f32 / scale),
            ),
        };
        let _small = SmallBirth::begin();
        cx.open_window(floating_window(rough, None, true), |window, cx| {
            let view = cx.new(|cx| DropdownView::new(window, cx));
            view.update(cx, |view, cx| view.show(frame, window, cx));
            view
        })
    }
}

/// The frame of the drop-down terminal in `work`, the room the bars leave
/// of a display: `shares` of its width and height, in the middle of the
/// width, at the top.
fn dropdown_frame(work: ScreenRect, shares: (f32, f32)) -> ScreenRect {
    let (width, height) = (work.right - work.left, work.bottom - work.top);
    let wide = ((width as f32 * shares.0).round() as i32).clamp(1, width.max(1));
    let high = ((height as f32 * shares.1).round() as i32).clamp(1, height.max(1));
    let left = work.left + (width - wide) / 2;
    ScreenRect {
        left,
        top: work.top,
        right: left + wide,
        bottom: work.top + high,
    }
}

/// The root view of the drop-down terminal's window.
struct DropdownView {
    tabs: Entity<TerminalTabs>,
    /// 0 while hidden, 1 when fully down.
    shown: Spring,
    hiding: bool,
    /// Whether the window has had the keys since it was shown; before
    /// that, not having them does not mean another window took them.
    was_active: bool,
    _activation: Subscription,
    _tabs: Subscription,
}

impl DropdownView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tabs = cx.new(|cx| TerminalTabs::new(Files::home().folder().map(PathBuf::from), cx));
        let emptied = cx.subscribe_in(&tabs, window, |this, _, event: &TabsEvent, window, cx| {
            if *event == TabsEvent::Emptied {
                this.hide(window, cx);
            }
        });
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.was_active = true;
            } else if this.was_active && !this.hiding && Terminals::config(cx).dropdown_hides {
                this.hide(window, cx);
            }
        });
        Self {
            tabs,
            shown: Spring::new(cx.global::<Theme>().motion.spatial, 0.),
            hiding: true,
            was_active: false,
            _activation: activation,
            _tabs: emptied,
        }
    }

    /// Whether it is down or coming down.
    fn shown(&self) -> bool {
        !self.hiding
    }

    /// Puts the window over `frame` and slides it down, with the keys
    /// going to the terminal in front.
    fn show(&mut self, frame: (ScreenRect, f32), window: &mut Window, cx: &mut Context<Self>) {
        let placed = NativeSurface::of(window).and_then(|native| {
            native.adopt_as_popup()?;
            native.place(frame.0)?;
            native.reveal(true);
            Ok(())
        });
        if let Err(error) = placed {
            report!("the drop-down terminal is not where it belongs: {error:#}");
        }
        self.hiding = false;
        self.was_active = false;
        self.shown.animate_to(1.);
        self.tabs.read(cx).focus(window, cx);
        cx.notify();
    }

    /// Slides it up and out of sight; its shells go on running.
    fn hide(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.hiding = true;
        self.shown.animate_to(0.);
        cx.notify();
    }
}

impl Render for DropdownView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let shown = self.shown.value(window).clamp(0., 1.);
        if self.hiding
            && self.shown.is_settled()
            && let Ok(native) = NativeSurface::of(window)
        {
            native.conceal();
        }
        let height = window.viewport_size().height;
        div().size_full().overflow_hidden().child(
            div()
                .size_full()
                .relative()
                .top(-height * (1. - shown))
                .opacity(shown)
                .flex()
                .flex_col()
                .bg(theme.colors.surface_container)
                .rounded_b(theme.shape.large)
                .border_b_1()
                .border_color(theme.colors.outline.opacity(0.4))
                .child(div().flex_1().min_h_0().child(self.tabs.clone())),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drop_down_hangs_from_the_top_of_the_room_left_to_windows() {
        let work = ScreenRect {
            left: 0,
            top: 40,
            right: 2000,
            bottom: 1040,
        };
        let frame = dropdown_frame(work, (1., 0.45));
        assert_eq!(
            (frame.left, frame.top, frame.right, frame.bottom),
            (0, 40, 2000, 490)
        );
        let narrow = dropdown_frame(work, (0.5, 1.));
        assert_eq!(
            (narrow.left, narrow.right, narrow.bottom),
            (500, 1500, 1040)
        );
    }
}
