// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The window switcher: the application windows as tiles in the middle of
//! a display, gone through with a key combination while its keys to hold
//! stay down, and the chosen one brought in front once they are let go.

use std::time::Duration;

use gpui::{
    App, AsyncApp, Bounds, Context, CursorStyle, FontWeight, Global, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, WindowHandle, div, img, point, px, size,
};

use crate::icons::{AppIcons, Icon};
use crate::motion::{Spring, mix, staggered};
use crate::platform::{AppWindow, AppWindowId, Modifiers, NativeSurface, ScreenEdge};
use crate::popup::{floating_panel, open_floating, retire_floating};
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{one_line, release_hover};

/// The width and the height of a tile, in logical pixels.
const TILE_WIDTH: f32 = 136.;
const TILE_HEIGHT: f32 = 112.;

/// The length of the sides of an icon.
const ICON: f32 = 48.;

/// How many tiles stand in a row at most.
const COLUMNS: usize = 8;

/// How many windows are offered at most: the ones used last.
const SHOWN: usize = 40;

/// How often it is asked whether the keys to hold are still down.
const RELEASE_CHECK: Duration = Duration::from_millis(15);

/// The open switcher.
#[derive(Default)]
pub struct Switcher {
    open: Option<WindowHandle<SwitcherView>>,
}

impl Global for Switcher {}

impl Switcher {
    /// Goes one window on in the open switcher, or back with `back`; opens
    /// it when it is not open, with the window used before the one in
    /// front chosen. The choice is taken once none of `held` is down.
    pub fn step(back: bool, held: Modifiers, cx: &mut App) {
        if let Some(open) = cx.default_global::<Self>().open {
            let stepped = open.update(cx, |view, _, cx| view.step(back, cx));
            if matches!(stepped, Ok(true)) {
                return;
            }
        }
        let own = std::process::id();
        let windows: Vec<_> = AppWindow::enumerate()
            .into_iter()
            .filter(|window| window.id.process() != own)
            .take(SHOWN)
            .collect();
        if windows.is_empty() {
            return;
        }
        let Some(anchor) = Shell::middle_anchor(cx) else {
            report!("the window switcher has nowhere to open");
            return;
        };
        let front = AppWindowId::foreground();
        let front = windows.iter().position(|window| Some(window.id) == front);
        let chosen = first_choice(windows.len(), front, back);
        let icons = cx.global::<AppIcons>();
        let entries: Vec<_> = windows
            .iter()
            .map(|window| Entry {
                id: window.id,
                title: window.id.title().into(),
                icon: window
                    .program()
                    .and_then(|program| icons.of_executable(&program)),
            })
            .collect();
        let theme = *cx.global::<Theme>();
        let (columns, rows) = grid(entries.len());
        let gap = theme.spacing.small / px(1.);
        let padding = theme.spacing.medium / px(1.);
        let (width, height) = extent(columns, rows, gap, padding);
        let frame = anchor.middle(width, height);
        let rough = Bounds {
            origin: point(
                px(frame.left as f32 / anchor.scale),
                px(frame.top as f32 / anchor.scale),
            ),
            size: size(px(width), px(height)),
        };
        let opened = open_floating(rough, anchor.display_id, true, cx, |window, cx| {
            let adopted = NativeSurface::of(window).and_then(|native| {
                native.adopt_as_popup()?;
                native.place(frame)?;
                native.reveal(true);
                Ok(())
            });
            if let Err(error) = adopted {
                report!("the window switcher is not where it belongs: {error:#}");
            }
            SwitcherView::new(entries, chosen, columns, held, window, cx)
        });
        match opened {
            Ok(window) => cx.global_mut::<Self>().open = Some(window),
            Err(error) => report!("the window switcher did not open: {error:#}"),
        }
    }
}

/// The window chosen when the switcher opens over `count` windows, the
/// one in front being at `front` of them: the one after it, or before it
/// with `back`; the first where none of them is in front.
fn first_choice(count: usize, front: Option<usize>, back: bool) -> usize {
    front.map_or(0, |front| stepped(front, count, back))
}

/// The place after `at` among `count`, or before it with `back`; past
/// either end comes the other one.
fn stepped(at: usize, count: usize, back: bool) -> usize {
    let count = count.max(1);
    let step = if back { count - 1 } else { 1 };
    (at + step) % count
}

/// In how many columns and rows `count` tiles stand.
fn grid(count: usize) -> (usize, usize) {
    let columns = count.clamp(1, COLUMNS);
    (columns, count.max(1).div_ceil(columns))
}

/// The logical width and height of the panel around `columns` by `rows`
/// tiles, `gap` apart and `padding` from its border.
fn extent(columns: usize, rows: usize, gap: f32, padding: f32) -> (f32, f32) {
    let along = |tiles: usize, length: f32| {
        tiles as f32 * length + tiles.saturating_sub(1) as f32 * gap + padding * 2.
    };
    (along(columns, TILE_WIDTH), along(rows, TILE_HEIGHT))
}

/// A window to switch to.
struct Entry {
    id: AppWindowId,
    title: SharedString,
    icon: Icon,
}

/// The root view of the switcher window.
pub struct SwitcherView {
    entries: Vec<Entry>,
    /// Which entry is brought in front when the keys are let go.
    chosen: usize,
    columns: usize,
    /// The column and the row of the mark behind the chosen tile, which
    /// travels from tile to tile.
    column: Spring,
    row: Spring,
    /// 0 while hidden, 1 when fully open.
    visibility: Spring,
    closing: bool,
    /// Whether the window has had keyboard focus; before that, not having
    /// it does not mean the user turned away.
    was_active: bool,
    _activation: Subscription,
}

impl SwitcherView {
    fn new(
        entries: Vec<Entry>,
        chosen: usize,
        columns: usize,
        held: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let motion = cx.global::<Theme>().motion;
        let mut visibility = Spring::new(motion.spatial, 0.);
        visibility.animate_to(1.);
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.was_active = true;
            } else if this.was_active {
                this.close(cx);
            }
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(RELEASE_CHECK).await;
                if !held.any_down() {
                    break;
                }
                if this.upgrade().is_none() {
                    return;
                }
            }
            this.update(cx, |this, cx| this.choose(this.chosen, cx))
                .ok();
        })
        .detach();
        Self {
            column: Spring::new(motion.spatial, (chosen % columns) as f32),
            row: Spring::new(motion.spatial, (chosen / columns) as f32),
            entries,
            chosen,
            columns,
            visibility,
            closing: false,
            was_active: false,
            _activation: activation,
        }
    }

    /// Chooses the entry after the chosen one, or before it with `back`.
    /// `false` when the switcher is closing and takes no more of that.
    fn step(&mut self, back: bool, cx: &mut Context<Self>) -> bool {
        if self.closing {
            return false;
        }
        self.chosen = stepped(self.chosen, self.entries.len(), back);
        self.column.animate_to((self.chosen % self.columns) as f32);
        self.row.animate_to((self.chosen / self.columns) as f32);
        cx.notify();
        true
    }

    /// Brings the window of the entry at `index` in front and closes.
    fn choose(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        if let Some(entry) = self.entries.get(index) {
            entry.id.activate();
        }
        self.close(cx);
    }

    /// Starts closing; the window goes away once the motion has finished.
    fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.visibility.animate_to(0.);
        cx.notify();
    }
}

impl Render for SwitcherView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let visibility = self.visibility.value(window);
        if self.closing && self.visibility.is_settled() {
            retire_floating(window, cx);
        }
        let gap = theme.spacing.small / px(1.);
        let place = |column: f32, row: f32| {
            (
                px(column * (TILE_WIDTH + gap)),
                px(row * (TILE_HEIGHT + gap)),
            )
        };
        let (left, top) = place(self.column.value(window), self.row.value(window));
        let mark = div()
            .absolute()
            .left(left)
            .top(top)
            .w(px(TILE_WIDTH))
            .h(px(TILE_HEIGHT))
            .rounded(theme.shape.large)
            .bg(theme.colors.secondary_container);
        let count = self.entries.len();
        let tiles: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let (left, top) =
                    place((index % self.columns) as f32, (index / self.columns) as f32);
                let arrival = staggered(visibility, index, count);
                let chosen = index == self.chosen;
                let color = match chosen {
                    true => theme.colors.on_surface,
                    false => theme.colors.on_surface_variant,
                };
                div()
                    .id(("switcher-tile", index))
                    .absolute()
                    .left(left)
                    .top(top + px(8. * (1. - arrival)))
                    .w(px(TILE_WIDTH))
                    .h(px(TILE_HEIGHT))
                    .px(theme.spacing.small)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(theme.spacing.small)
                    .opacity(arrival)
                    .cursor(CursorStyle::PointingHand)
                    .text_color(mix(theme.colors.on_surface_variant, color, visibility))
                    .when_weight(chosen)
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(index, cx)))
                    .child(match entry.icon.clone() {
                        Some(icon) => img(icon).size(px(ICON)).into_any_element(),
                        None => div()
                            .size(px(ICON))
                            .rounded(theme.shape.small)
                            .bg(theme.colors.outline)
                            .into_any_element(),
                    })
                    .child(div().max_w_full().child(one_line(entry.title.clone())))
            })
            .collect();
        floating_panel(&theme, ScreenEdge::Top, visibility, |panel| {
            panel.child(div().relative().size_full().child(mark).children(tiles))
        })
    }
}

/// The weight of the title of the chosen tile.
trait Weighted: Styled {
    fn when_weight(self, chosen: bool) -> Self {
        match chosen {
            true => self.font_weight(FontWeight::MEDIUM),
            false => self,
        }
    }
}

impl<T: Styled> Weighted for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switcher_opens_on_the_window_used_before_the_one_in_front() {
        assert_eq!(first_choice(3, Some(0), false), 1);
        assert_eq!(first_choice(3, Some(0), true), 2);
        // With the desktop in front, the window used last is chosen.
        assert_eq!(first_choice(3, None, false), 0);
        assert_eq!(first_choice(3, None, true), 0);
        assert_eq!(first_choice(1, Some(0), false), 0);
    }

    #[test]
    fn stepping_past_an_end_comes_to_the_other_one() {
        assert_eq!(stepped(0, 4, false), 1);
        assert_eq!(stepped(3, 4, false), 0);
        assert_eq!(stepped(0, 4, true), 3);
        assert_eq!(stepped(2, 4, true), 1);
        assert_eq!(stepped(0, 0, false), 0);
    }

    #[test]
    fn tiles_fill_a_row_before_another_one_begins() {
        assert_eq!(grid(1), (1, 1));
        assert_eq!(grid(8), (8, 1));
        assert_eq!(grid(9), (8, 2));
        assert_eq!(grid(40), (8, 5));
        assert_eq!(extent(2, 1, 8., 12.), (2. * 136. + 8. + 24., 112. + 24.));
    }
}
