// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The window switcher: the application windows as tiles in the middle of
//! a display, gone through with a key combination while its keys to hold
//! stay down, and the chosen one brought in front once they are let go.
//!
//! Where the configuration asks for it, a tile shows a picture of its
//! window, which Windows draws and keeps up to date.

use std::time::Duration;

use gpui::{
    App, AsyncApp, Bounds, Context, CursorStyle, FontWeight, Global, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, WindowHandle, div, img, point, px, size,
};

use serde::Deserialize;

use crate::icons::{AppIcons, Icon};
use crate::motion::{Spring, mix, staggered};
use crate::platform::{
    AppWindow, AppWindowId, ChoiceKey, Modifiers, NativeSurface, ScreenEdge, ScreenRect, TakenKeys,
    Thumbnail,
};
use crate::popup::{MARGIN, floating_panel, floating_travel, open_floating, retire_floating};
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

/// The width and the height of a tile with a picture of its window, the
/// room the picture has in it, and what is under the picture: the height
/// of the line with the name and the length of the sides of its icon.
const PICTURE_TILE: (f32, f32) = (208., 172.);
const PICTURE: (f32, f32) = (192., 120.);
const NAME_HEIGHT: f32 = 28.;
const SMALL_ICON: f32 = 20.;

/// How many tiles with pictures stand in a row at most.
const PICTURE_COLUMNS: usize = 6;

/// What `[switcher]` of the configuration says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitcherConfig {
    /// Whether a tile shows a picture of its window.
    #[serde(default)]
    previews: bool,
}

/// The measures of the tiles of a switcher.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tiles {
    width: f32,
    height: f32,
    /// How many stand in a row at most.
    columns: usize,
    /// Whether they show pictures of their windows.
    pictures: bool,
}

impl Tiles {
    fn of(pictures: bool) -> Self {
        let ((width, height), columns) = match pictures {
            true => (PICTURE_TILE, PICTURE_COLUMNS),
            false => ((TILE_WIDTH, TILE_HEIGHT), COLUMNS),
        };
        Self {
            width,
            height,
            columns,
            pictures,
        }
    }
}

/// The logical size of the picture of a window of `source` physical
/// pixels on a display of `scale`: as large as its room in a tile allows,
/// never enlarged.
fn fitted((width, height): (i32, i32), scale: f32) -> (f32, f32) {
    let (width, height) = (width as f32 / scale, height as f32 / scale);
    let factor = (PICTURE.0 / width).min(PICTURE.1 / height).min(1.);
    (width * factor, height * factor)
}

/// How many windows are offered at most: the ones used last.
const SHOWN: usize = 40;

/// How often it is asked whether the keys to hold are still down.
const RELEASE_CHECK: Duration = Duration::from_millis(15);

/// The open switcher.
#[derive(Default)]
pub struct Switcher {
    open: Option<WindowHandle<SwitcherView>>,
    /// Whether the keys that move through the tiles are listened for.
    listening: bool,
    /// Whether the tiles show pictures of their windows.
    previews: bool,
}

impl Global for Switcher {}

impl Switcher {
    /// Takes in the configuration; a switcher that is open stays as it is.
    pub fn configure(config: &SwitcherConfig, cx: &mut App) {
        cx.default_global::<Self>().previews = config.previews;
    }

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
        let tiles = Tiles::of(cx.global::<Self>().previews);
        let icons = cx.global::<AppIcons>();
        let mut entries: Vec<_> = windows
            .iter()
            .map(|window| Entry {
                id: window.id,
                title: window.id.title().into(),
                icon: window
                    .program()
                    .and_then(|program| icons.of_executable(&program)),
                picture: None,
            })
            .collect();
        let theme = *cx.global::<Theme>();
        let (columns, rows) = grid(entries.len(), tiles.columns);
        let gap = theme.spacing.small / px(1.);
        let padding = theme.spacing.medium / px(1.);
        let (width, height) = extent(columns, rows, gap, padding, tiles);
        let scale = anchor.scale;
        let frame = anchor.middle(width, height);
        let rough = Bounds {
            origin: point(
                px(frame.left as f32 / anchor.scale),
                px(frame.top as f32 / anchor.scale),
            ),
            size: size(px(width), px(height)),
        };
        let opened = open_floating(rough, anchor.display_id, true, cx, |window, cx| {
            let native = NativeSurface::of(window);
            if let (true, Ok(native)) = (tiles.pictures, &native) {
                for entry in &mut entries {
                    let thumbnail = Thumbnail::of(entry.id, *native).ok();
                    entry.picture = thumbnail.and_then(|thumbnail| {
                        let source = thumbnail.source_size()?;
                        Some((thumbnail, fitted(source, scale)))
                    });
                }
            }
            let adopted = native.and_then(|native| {
                native.adopt_as_popup()?;
                native.place(frame)?;
                native.reveal(true);
                Ok(())
            });
            if let Err(error) = adopted {
                report!("the window switcher is not where it belongs: {error:#}");
            }
            SwitcherView::new(entries, chosen, (columns, tiles), held, window, cx)
        });
        match opened {
            Ok(window) => {
                cx.global_mut::<Self>().open = Some(window);
                Self::take_keys(cx);
            }
            Err(error) => report!("the window switcher did not open: {error:#}"),
        }
    }

    /// Takes the arrow keys, Enter and Esc from the keyboard for the open
    /// switcher; its view gives them back when it closes.
    fn take_keys(cx: &mut App) {
        if let Err(error) = TakenKeys::take_choice(true) {
            report!("the arrow keys do not reach the window switcher: {error:#}");
            return;
        }
        if std::mem::replace(&mut cx.global_mut::<Self>().listening, true) {
            return;
        }
        let presses = TakenKeys::choice_presses();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(key) = presses.recv().await {
                let pressed = cx.update(|cx| {
                    if let Some(open) = cx.global::<Self>().open {
                        open.update(cx, |view, _, cx| view.key(key, cx)).ok();
                    }
                });
                if pressed.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}

/// Where the choice goes from `at` of `count` tiles, `columns` in a row,
/// with an arrow key: to the tile beside it, around the ends of the list,
/// or to the one above or below it, where there is one.
fn moved(at: usize, count: usize, columns: usize, key: ChoiceKey) -> usize {
    match key {
        ChoiceKey::Left => stepped(at, count, true),
        ChoiceKey::Right => stepped(at, count, false),
        ChoiceKey::Up if at >= columns => at - columns,
        ChoiceKey::Down if at + columns < count => at + columns,
        _ => at,
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

/// In how many columns and rows `count` tiles stand, at most `most` in
/// a row.
fn grid(count: usize, most: usize) -> (usize, usize) {
    let columns = count.clamp(1, most.max(1));
    (columns, count.max(1).div_ceil(columns))
}

/// The logical width and height of the panel around `columns` by `rows`
/// of `tiles`, `gap` apart and `padding` from its border.
fn extent(columns: usize, rows: usize, gap: f32, padding: f32, tiles: Tiles) -> (f32, f32) {
    let along = |tiles: usize, length: f32| {
        tiles as f32 * length + tiles.saturating_sub(1) as f32 * gap + padding * 2.
    };
    (along(columns, tiles.width), along(rows, tiles.height))
}

/// A window to switch to.
struct Entry {
    id: AppWindowId,
    title: SharedString,
    icon: Icon,
    /// The picture of the window and its logical size, where the tiles
    /// show pictures and Windows gives one.
    picture: Option<(Thumbnail, (f32, f32))>,
}

/// The root view of the switcher window.
pub struct SwitcherView {
    entries: Vec<Entry>,
    /// Which entry is brought in front when the keys are let go.
    chosen: usize,
    columns: usize,
    tiles: Tiles,
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
        (columns, tiles): (usize, Tiles),
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
            tiles,
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
        self.mark(stepped(self.chosen, self.entries.len(), back), cx);
        true
    }

    /// Has the entry at `chosen` be the chosen one.
    fn mark(&mut self, chosen: usize, cx: &mut Context<Self>) {
        self.chosen = chosen;
        self.column.animate_to((self.chosen % self.columns) as f32);
        self.row.animate_to((self.chosen / self.columns) as f32);
        cx.notify();
    }

    /// Takes a key pressed while the switcher is open: an arrow moves the
    /// choice, Enter takes it and Esc closes without bringing a window in
    /// front.
    fn key(&mut self, key: ChoiceKey, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        match key {
            ChoiceKey::Escape => self.close(cx),
            ChoiceKey::Enter => self.choose(self.chosen, cx),
            arrow => self.mark(
                moved(self.chosen, self.entries.len(), self.columns, arrow),
                cx,
            ),
        }
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
        give_keys_back();
        self.closing = true;
        self.visibility.animate_to(0.);
        cx.notify();
    }
}

/// Gives the keys taken for the open switcher back to the programs.
fn give_keys_back() {
    if let Err(error) = TakenKeys::take_choice(false) {
        report!("the arrow keys stay with the window switcher: {error:#}");
    }
}

impl Drop for SwitcherView {
    /// However the view goes, the keys do not stay taken.
    fn drop(&mut self) {
        if !self.closing {
            give_keys_back();
        }
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
        let tiles = self.tiles;
        let place = |column: f32, row: f32| {
            (
                px(column * (tiles.width + gap)),
                px(row * (tiles.height + gap)),
            )
        };
        let (left, top) = place(self.column.value(window), self.row.value(window));
        let mark = div()
            .absolute()
            .left(left)
            .top(top)
            .w(px(tiles.width))
            .h(px(tiles.height))
            .rounded(theme.shape.large)
            .bg(theme.colors.secondary_container);
        let count = self.entries.len();
        // Where the tiles begin, in logical pixels of the window.
        let padding = theme.spacing.medium / px(1.);
        let travel = floating_travel(ScreenEdge::Top, visibility) / px(1.);
        let (first_left, first_top) = (MARGIN + padding, MARGIN + travel + padding);
        let scale = window.scale_factor();
        let physical = |length: f32| (length * scale).round() as i32;
        let elements: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let (column, row) = (index % self.columns, index / self.columns);
                let (left, top) = place(column as f32, row as f32);
                let arrival = staggered(visibility, index, count);
                let rise = 8. * (1. - arrival);
                let chosen = index == self.chosen;
                let color = match chosen {
                    true => theme.colors.on_surface,
                    false => theme.colors.on_surface_variant,
                };
                if let Some((thumbnail, (width, height))) = &entry.picture {
                    let left = first_left + left / px(1.) + (tiles.width - width) / 2.;
                    let top = first_top + top / px(1.) + rise + gap + (PICTURE.1 - height) / 2.;
                    let area = ScreenRect {
                        left: physical(left),
                        top: physical(top),
                        right: physical(left + width),
                        bottom: physical(top + height),
                    };
                    thumbnail.place(area, arrival * visibility.clamp(0., 1.));
                }
                let icon = |side: f32| match entry.icon.clone() {
                    Some(icon) => img(icon).size(px(side)).flex_none().into_any_element(),
                    None => div()
                        .size(px(side))
                        .flex_none()
                        .rounded(theme.shape.small)
                        .bg(theme.colors.outline)
                        .into_any_element(),
                };
                let title = div().max_w_full().child(one_line(entry.title.clone()));
                let tile = div()
                    .id(("switcher-tile", index))
                    .absolute()
                    .left(left)
                    .top(top + px(rise))
                    .w(px(tiles.width))
                    .h(px(tiles.height))
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
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(index, cx)));
                if !tiles.pictures {
                    return tile.child(icon(ICON)).child(title);
                }
                // Windows draws the picture over this room; a window it
                // gives none of shows its icon there.
                let room = div()
                    .w(px(PICTURE.0))
                    .h(px(PICTURE.1))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(entry.picture.is_none().then(|| icon(ICON)));
                let name = div()
                    .max_w_full()
                    .h(px(NAME_HEIGHT))
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .child(icon(SMALL_ICON))
                    .child(div().min_w_0().child(one_line(entry.title.clone())));
                tile.child(room).child(name)
            })
            .collect();
        let tiles = elements;
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
    fn arrows_go_beside_and_to_the_row_above_or_below() {
        assert_eq!(moved(0, 11, 8, ChoiceKey::Left), 10);
        assert_eq!(moved(10, 11, 8, ChoiceKey::Right), 0);
        assert_eq!(moved(1, 11, 8, ChoiceKey::Down), 9);
        assert_eq!(moved(5, 11, 8, ChoiceKey::Down), 5);
        assert_eq!(moved(9, 11, 8, ChoiceKey::Up), 1);
        assert_eq!(moved(1, 11, 8, ChoiceKey::Up), 1);
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
        assert_eq!(grid(1, COLUMNS), (1, 1));
        assert_eq!(grid(8, COLUMNS), (8, 1));
        assert_eq!(grid(9, COLUMNS), (8, 2));
        assert_eq!(grid(40, COLUMNS), (8, 5));
        assert_eq!(grid(9, PICTURE_COLUMNS), (6, 2));
        let plain = Tiles::of(false);
        assert_eq!(
            extent(2, 1, 8., 12., plain),
            (2. * 136. + 8. + 24., 112. + 24.)
        );
        let pictured = Tiles::of(true);
        assert_eq!(
            extent(2, 1, 8., 12., pictured),
            (2. * 208. + 8. + 24., 172. + 24.)
        );
    }

    #[test]
    fn a_picture_fills_its_room_and_is_never_enlarged() {
        assert_eq!(fitted((1920, 1080), 1.), (192., 108.));
        assert_eq!(fitted((600, 1200), 1.), (60., 120.));
        assert_eq!(fitted((100, 50), 1.), (100., 50.));
        assert_eq!(fitted((200, 100), 2.), (100., 50.));
        let config: SwitcherConfig = toml::from_str("previews = true").unwrap();
        assert!(config.previews);
        assert!(!SwitcherConfig::default().previews);
    }
}
