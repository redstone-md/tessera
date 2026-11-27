// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The bar of tabs over a stack of tiled windows: a tab for each window,
//! with its icon and title, the one in front marked.
//!
//! Each bar is a small window of Tessera's own, laid over the top of the
//! tile of its stack, kept just above the window in front of the stack in
//! the order of windows, so that a window that covers the stack covers
//! its bar too. It never takes the keyboard. A click on a tab brings its
//! window in front, a middle click closes it, the wheel goes through the
//! tabs, and a tab dragged down or up out of the bar takes its window out
//! of the stack into a tile of its own.

use std::path::PathBuf;

use gpui::{
    App, Bounds, Context, FontWeight, Global, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseUpEvent, ParentElement, Pixels, Render, ScrollDelta, ScrollWheelEvent,
    Styled, Window, WindowHandle, div, img, point, prelude::FluentBuilder, px, size,
};

use crate::icons::AppIcons;
use crate::platform::{AppWindowId, Display, NativeSurface, ScreenRect};
use crate::popup::{open_floating, retire_floating};
use crate::snap::gpui_display;
use crate::switcher::choice::notches;
use crate::theme::Theme;
use crate::ui::one_line;

/// How many pixels of scrolling make a notch of the wheel.
const WHEEL_LINE: f32 = 40.;

/// How far beyond the bar, in heights of the bar, a tab is to be dragged
/// to leave it.
const DRAGGED_OUT: f32 = 1.;

/// A window of a stack, as its tab shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub id: AppWindowId,
    pub title: String,
    /// The program whose icon the tab shows.
    pub program: Option<PathBuf>,
}

/// A bar as it is to be shown.
#[derive(Clone, Debug, PartialEq)]
pub struct BarLook {
    /// Where the bar is on the screen, in physical pixels.
    pub rect: ScreenRect,
    /// The display it is on.
    pub display: Display,
    pub tabs: Vec<Tab>,
    /// The tab of the window in front.
    pub active: usize,
}

impl BarLook {
    /// The window in front of the stack.
    pub fn front(&self) -> Option<AppWindowId> {
        self.tabs.get(self.active).map(|tab| tab.id)
    }
}

/// What a bar asks of the tiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackDeed {
    /// This window in front of its stack, and of the other windows.
    Activate(AppWindowId),
    /// This window closed.
    Close(AppWindowId),
    /// The window of the stack of this one after the one in front, or
    /// before it with `true`, in front.
    Cycle(AppWindowId, bool),
    /// This window out of its stack, into a tile of its own.
    Unstack(AppWindowId),
}

/// The bars that are shown, and how each was last shown.
#[derive(Default)]
pub struct StackBars {
    shown: Vec<(WindowHandle<BarView>, BarLook)>,
}

impl Global for StackBars {}

impl StackBars {
    /// Shows a bar for each of `looks`, using the bars there are and
    /// opening or putting away the others; `act` is told what the bars
    /// ask for. A bar that is shown as it was is only kept above its
    /// stack.
    pub fn show(looks: Vec<BarLook>, act: fn(StackDeed, &mut App), cx: &mut App) {
        let mut shown = std::mem::take(&mut cx.default_global::<Self>().shown);
        let mut kept = Vec::with_capacity(looks.len());
        let mut looks = looks.into_iter();
        for look in looks.by_ref() {
            let Some((handle, before)) = shown.pop() else {
                if let Some(opened) = open(look.clone(), act, cx) {
                    kept.push((opened, look));
                }
                continue;
            };
            let again = look.clone();
            let updated = handle.update(cx, |view, window, cx| {
                if before.rect != look.rect
                    && let Ok(native) = NativeSurface::of(window)
                    && let Err(error) = native.place(look.rect)
                {
                    report!("the bar of a stack is not where it belongs: {error:#}");
                }
                if before != look {
                    view.look = look;
                    cx.notify();
                }
                view.lift();
            });
            match updated {
                Ok(()) => kept.push((handle, again)),
                // The window went: one is opened in its place.
                Err(_) => {
                    if let Some(opened) = open(again.clone(), act, cx) {
                        kept.push((opened, again));
                    }
                }
            }
        }
        for (handle, _) in shown {
            let _ = handle.update(cx, |_, window, cx| retire_floating(window, cx));
        }
        cx.default_global::<Self>().shown = kept;
    }
}

/// Opens a bar shown as `look` says.
fn open(
    look: BarLook,
    act: fn(StackDeed, &mut App),
    cx: &mut App,
) -> Option<WindowHandle<BarView>> {
    let scale = look.display.scale.max(0.5);
    let rect = look.rect;
    let rough = Bounds {
        origin: point(px(rect.left as f32 / scale), px(rect.top as f32 / scale)),
        size: size(
            px((rect.right - rect.left) as f32 / scale),
            px((rect.bottom - rect.top) as f32 / scale),
        ),
    };
    let display_id = gpui_display(&look.display, cx);
    let opened = open_floating(rough, display_id, false, cx, move |window, cx| {
        let native = NativeSurface::of(window);
        let id = AppWindowId::of(window);
        match native {
            // The window learns its size only if it is given while nothing
            // of the user interface is being worked on, as it is right
            // after this.
            Ok(native) => cx
                .spawn(async move |this, cx| {
                    let settled = (|| {
                        native.adopt_as_card()?;
                        native.place(rect)?;
                        native.reveal(false);
                        anyhow::Ok(())
                    })();
                    if let Err(error) = settled {
                        report!("the bar of a stack is not where it belongs: {error:#}");
                    }
                    this.update(cx, |view: &mut BarView, _| view.lift()).ok();
                })
                .detach(),
            Err(error) => report!("the bar of a stack is not where it belongs: {error:#}"),
        }
        BarView {
            look,
            own: id,
            pressed: None,
            wheel: 0.,
            act,
        }
    });
    match opened {
        Ok(handle) => Some(handle),
        Err(error) => {
            report!("the bar of a stack could not be opened: {error:#}");
            None
        }
    }
}

/// The view of a bar.
pub struct BarView {
    look: BarLook,
    /// The window of the bar itself.
    own: Option<AppWindowId>,
    /// The tab the left button went down on, and where.
    pressed: Option<(usize, f32)>,
    /// What was scrolled and not counted as a notch yet.
    wheel: f32,
    act: fn(StackDeed, &mut App),
}

impl BarView {
    /// Puts the bar just above the window in front of its stack.
    fn lift(&self) {
        if let (Some(own), Some(front)) = (self.own, self.look.front()) {
            own.lift_above(front);
        }
    }

    /// Asks the tiling for `deed`, once nothing of the bar is being
    /// worked on.
    fn ask(&self, deed: StackDeed, cx: &mut Context<Self>) {
        let act = self.act;
        cx.defer(move |cx| act(deed, cx));
    }

    fn pressed(&mut self, tab: usize, event: &MouseDownEvent) {
        self.pressed = Some((tab, f32::from(event.position.y)));
    }

    /// The left button went up at `event`: a tab dragged out of the bar
    /// leaves its stack, and one let go of where it was pressed is
    /// clicked.
    fn released(&mut self, event: &MouseUpEvent, window: &Window, cx: &mut Context<Self>) {
        let Some((tab, _)) = self.pressed.take() else {
            return;
        };
        let Some(id) = self.look.tabs.get(tab).map(|tab| tab.id) else {
            return;
        };
        let height = f32::from(window.bounds().size.height);
        let y = f32::from(event.position.y);
        let out = height * DRAGGED_OUT;
        if y > height + out || y < -out {
            self.ask(StackDeed::Unstack(id), cx);
            return;
        }
        let width = f32::from(window.bounds().size.width);
        let count = self.look.tabs.len().max(1) as f32;
        let x = f32::from(event.position.x);
        let under = (x / (width / count)).floor();
        if (0. ..count).contains(&under) && under as usize == tab {
            self.ask(StackDeed::Activate(id), cx);
        }
    }

    fn wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let distance = match event.delta {
            ScrollDelta::Lines(lines) => lines.y,
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / WHEEL_LINE,
        };
        let (count, left) = notches(self.wheel, distance);
        self.wheel = left;
        let Some(front) = self.look.front() else {
            return;
        };
        for _ in 0..count.unsigned_abs() {
            self.ask(StackDeed::Cycle(front, count < 0), cx);
        }
    }
}

impl Render for BarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let colors = theme.colors;
        let height: Pixels = window.bounds().size.height;
        let icons = cx.global::<AppIcons>();
        let side = (height * 0.55).min(px(20.));
        let tabs: Vec<_> = { self.look.tabs.iter().enumerate() }
            .map(|(index, tab)| {
                let active = index == self.look.active;
                let (ground, color) = match active {
                    true => (colors.secondary_container, colors.primary),
                    false => (colors.surface_container, colors.on_surface_variant),
                };
                let icon = tab
                    .program
                    .as_deref()
                    .and_then(|program| icons.of_executable(program));
                let id = tab.id;
                div()
                    .id(("tab", index))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.tiny)
                    .px(theme.spacing.small)
                    .rounded(theme.shape.small)
                    .bg(ground)
                    .text_color(color)
                    .text_size(theme.typography.label_size)
                    .when(active, |tab| {
                        tab.font_weight(FontWeight::MEDIUM)
                            .border_b_2()
                            .border_color(colors.primary)
                    })
                    .when(!active, |tab| {
                        tab.hover(|tab| tab.bg(colors.surface_container_highest))
                    })
                    .children(icon.map(|icon| img(icon).size(side).flex_none()))
                    .child(div().flex_1().min_w_0().child(one_line(tab.title.clone())))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            this.pressed(index, event);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.ask(StackDeed::Close(id), cx);
                        }),
                    )
            })
            .collect();
        div()
            .size_full()
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .p(px(2.))
            .bg(colors.surface_container)
            .text_color(colors.on_surface)
            .children(tabs)
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.released(event, window, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.released(event, window, cx);
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                this.wheel(event, cx);
            }))
    }
}
