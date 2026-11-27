// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Previews: live pictures of the windows of what the pointer rests on,
//! side by side, each with its title, a button that closes it, and the
//! buttons its application put under its picture; under them, the
//! controls of what the application plays.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Context, Global,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, WindowHandle, div, img, point, px,
    size,
};

use crate::icons::{Icon, icon_of_pixels};
use crate::motion::{Interaction, Spring};
use crate::platform::{
    AppWindowId, Media, MediaCommand, NativeSurface, ScreenEdge, ScreenRect, SessionKey,
    TaskbarList, ThumbButton, Thumbnail,
};
use crate::popup::{
    MARGIN, PopupAnchor, floating_panel, floating_travel, open_floating, retire_floating,
};
use crate::theme::{Motion, Theme};
use crate::tip::Tips;
use crate::ui::{icon, one_line, release_hover, state_layer};

/// How long the pointer rests before a preview opens.
const OPEN_DELAY: Duration = Duration::from_millis(400);

/// How long it rests before an open preview shows another window.
const SWITCH_DELAY: Duration = Duration::from_millis(120);

/// How long after the pointer left a preview stays, so that the pointer
/// can travel onto it.
const CLOSE_DELAY: Duration = Duration::from_millis(300);

/// How often an open preview looks whether an application changed the
/// buttons under its picture or its title.
const TOLD_CHECK: Duration = Duration::from_millis(250);

/// The largest picture, in logical pixels.
const PICTURE_WIDTH: f32 = 260.;
const PICTURE_HEIGHT: f32 = 160.;

/// The width of a preview whose picture is narrower or missing.
const LEAST_WIDTH: f32 = 160.;

/// The height of the row with the title.
const TITLE_HEIGHT: f32 = 24.;

/// The side of a button under a picture, and of the icon on it.
const BUTTON_SIDE: f32 = 28.;
const BUTTON_ICON: f32 = 16.;

/// The height of the row with the controls of what is played.
const MEDIA_HEIGHT: f32 = 32.;

const ICON_CLOSE: &str = "\u{e8bb}";
const ICON_PREVIOUS: &str = "\u{e892}";
const ICON_PLAY: &str = "\u{e768}";
const ICON_PAUSE: &str = "\u{e769}";
const ICON_NEXT: &str = "\u{e893}";

/// The application whose windows a preview shows, so that the controls of
/// what it plays can be found: its packaged identifier, or its program.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreviewOwner {
    pub app: Option<String>,
    pub program: Option<PathBuf>,
}

impl PreviewOwner {
    /// Whether the media session of `app`, as Windows names it, is one of
    /// this application: the same packaged identifier, or the same name
    /// of a program file.
    fn plays(&self, app: &str) -> bool {
        let same_app = self
            .app
            .as_deref()
            .is_some_and(|own| own.eq_ignore_ascii_case(app));
        let program = self
            .program
            .as_deref()
            .and_then(|program| program.file_name());
        let same_program = program.is_some_and(|name| name.eq_ignore_ascii_case(app));
        same_app || same_program
    }
}

/// The open preview and what the pointer last asked for.
#[derive(Default)]
pub struct Previews {
    open: Option<(Vec<AppWindowId>, WindowHandle<PreviewView>)>,
    /// The first window of what the pointer rests on.
    wanted: Option<AppWindowId>,
    /// Counts the requests, so that a delayed one that was overtaken by
    /// another is dropped.
    requests: u64,
}

impl Global for Previews {}

impl Previews {
    /// The pointer rests on what stands for `target`: its preview opens at
    /// `anchor` after a moment, unless the pointer moves on before.
    #[allow(dead_code, reason = "for what stands for one window")]
    pub fn request(target: AppWindowId, anchor: PopupAnchor, cx: &mut App) {
        Self::request_group(vec![target], PreviewOwner::default(), anchor, cx);
    }

    /// The pointer rests on what stands for `targets`, the windows of
    /// `owner`, the one used last first: their previews open side by side
    /// at `anchor` after a moment, unless the pointer moves on before.
    pub fn request_group(
        targets: Vec<AppWindowId>,
        owner: PreviewOwner,
        anchor: PopupAnchor,
        cx: &mut App,
    ) {
        let Some(&first) = targets.first() else {
            return;
        };
        let this = cx.global_mut::<Self>();
        this.wanted = Some(first);
        if this.open.as_ref().is_some_and(|(open, _)| *open == targets) {
            Self::keep(cx);
            return;
        }
        let delay = if this.open.is_some() {
            SWITCH_DELAY
        } else {
            OPEN_DELAY
        };
        Self::later(delay, cx, move |cx| {
            Self::close(cx);
            match Self::open(targets.clone(), owner, &anchor, cx) {
                Ok(window) => cx.global_mut::<Self>().open = Some((targets, window)),
                Err(error) => report!("a preview did not open: {error:#}"),
            }
        });
    }

    /// The pointer left what stands for `target`, or rests on something
    /// that stands for no window.
    pub fn leave(target: Option<AppWindowId>, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if target.is_none() || this.wanted == target {
            this.wanted = None;
        }
        Self::dismiss(cx);
    }

    /// The pointer left: the preview closes after a moment, unless the
    /// pointer comes back or reaches the preview before. Where the pointer
    /// went may have been told before it is told to have left.
    pub fn dismiss(cx: &mut App) {
        Self::later(CLOSE_DELAY, cx, |cx| {
            let this = cx.global::<Self>();
            if this.wanted.is_some() {
                return;
            }
            let open = this.open.as_ref().map(|(_, window)| *window);
            let hovered = open.and_then(|window| {
                let hovered = |_: &mut PreviewView, window: &mut Window, _: &mut Context<_>| {
                    window.is_window_hovered()
                };
                window.update(cx, hovered).ok()
            });
            if hovered != Some(true) {
                Self::close(cx);
            }
        });
    }

    /// Drops what was asked for and has not happened yet.
    pub fn keep(cx: &mut App) {
        cx.global_mut::<Self>().requests += 1;
    }

    /// Closes the preview now, and drops what was asked for.
    pub fn close(cx: &mut App) {
        let this = cx.global_mut::<Self>();
        this.requests += 1;
        if let Some((_, window)) = this.open.take() {
            window.update(cx, |view, _, cx| view.close(cx)).ok();
        }
        Tips::close(cx);
    }

    /// The window whose preview is open, the first where several are.
    pub fn shown(cx: &App) -> Option<AppWindowId> {
        let open = cx.global::<Self>().open.as_ref();
        open.and_then(|(open, _)| open.first().copied())
    }

    /// Runs `act` after `delay` unless something else is asked for before.
    fn later(delay: Duration, cx: &mut App, act: impl FnOnce(&mut App) + 'static) {
        let this = cx.global_mut::<Self>();
        this.requests += 1;
        let request = this.requests;
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor().timer(delay).await;
            cx.update(|cx| {
                if cx.global::<Self>().requests == request {
                    act(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn open(
        targets: Vec<AppWindowId>,
        owner: PreviewOwner,
        anchor: &PopupAnchor,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<PreviewView>> {
        let padding = cx.global::<Theme>().spacing.medium / px(1.);
        let (edge, scale, anchor) = (anchor.edge, anchor.scale, *anchor);
        let rough = Bounds {
            origin: point(
                px(anchor.x as f32 / scale),
                px(anchor.surface.top as f32 / scale),
            ),
            size: size(px(PICTURE_WIDTH), px(PICTURE_HEIGHT)),
        };
        // The room the display leaves for the pictures side by side.
        let room = (anchor.screen.right - anchor.screen.left) as f32 / scale - MARGIN * 2.;
        let room = room - padding * (targets.len() as f32 + 1.);
        open_floating(rough, anchor.display_id, false, cx, |window, cx| {
            let native = NativeSurface::of(window);
            let thumbnails: Vec<Option<Thumbnail>> = { targets.iter() }
                .map(|&target| {
                    let native = native.as_ref().ok()?;
                    Thumbnail::of(target, *native).ok()
                })
                .collect();
            let sources: Vec<Option<(f32, f32)>> = { thumbnails.iter() }
                .map(|thumbnail| {
                    thumbnail
                        .as_ref()?
                        .source_size()
                        .map(|source| fit(source, scale))
                })
                .collect();
            let pictures = lay_out(&sources, room);
            let theme = cx.global::<Theme>();
            let tiles: Vec<Tile> = { targets.iter().zip(thumbnails).zip(pictures) }
                .map(|((&id, thumbnail), picture)| Tile {
                    id,
                    title: id.title().into(),
                    thumbnail: thumbnail.filter(|_| picture.is_some()),
                    picture: picture.unwrap_or_default(),
                    close: Interaction::new(&theme.motion),
                    buttons: Vec::new(),
                })
                .collect();
            let mut visibility = Spring::new(theme.motion.spatial, 0.);
            visibility.animate_to(1.);
            let mut view = PreviewView {
                tiles,
                edge,
                visibility,
                closing: false,
                media: None,
                told: None,
            };
            view.take_told(&theme.motion);
            let (width, height) = view.extent(padding);
            let placed = native.and_then(|native| {
                native.adopt_as_shell_surface()?;
                native.place(anchor.frame(width, height))?;
                native.reveal(false);
                Ok(())
            });
            if let Err(error) = placed {
                report!("a preview is not where it belongs: {error:#}");
            }
            view.watch(owner, padding, anchor, window.window_handle(), cx);
            view
        })
    }
}

/// The logical size of the picture of a window of `source` physical pixels
/// on a display of `scale`: as large as the limits allow, never enlarged.
fn fit((width, height): (i32, i32), scale: f32) -> (f32, f32) {
    let (width, height) = (width as f32 / scale, height as f32 / scale);
    let factor = (PICTURE_WIDTH / width).min(PICTURE_HEIGHT / height).min(1.);
    (width * factor, height * factor)
}

/// The pictures `pictures` shrunk alike until they fit side by side into
/// `room`, each as wide as [`LEAST_WIDTH`] at least; a missing picture
/// takes that width.
fn lay_out(pictures: &[Option<(f32, f32)>], room: f32) -> Vec<Option<(f32, f32)>> {
    let width =
        |picture: &Option<(f32, f32)>| picture.map_or(LEAST_WIDTH, |(w, _)| w.max(LEAST_WIDTH));
    let whole: f32 = pictures.iter().map(width).sum();
    let factor = match whole > room && whole > 0. {
        true => (room / whole).max(0.1),
        false => 1.,
    };
    { pictures.iter() }
        .map(|picture| picture.map(|(width, height)| (width * factor, height * factor)))
        .collect()
}

/// The width of the tile of a picture `picture` wide, at least
/// [`LEAST_WIDTH`] where it was not shrunk to fit.
fn tile_width(picture: f32, shrunk: bool) -> f32 {
    match shrunk {
        true => picture,
        false => picture.max(LEAST_WIDTH),
    }
}

/// The height of a tile: the title, the picture and, where any tile has
/// them, the buttons under the picture.
fn tile_height(picture: f32, buttons: bool, padding: f32) -> f32 {
    let buttons = if buttons { padding + BUTTON_SIDE } else { 0. };
    TITLE_HEIGHT + padding + picture + buttons
}

/// The controls of what the application of a preview plays.
struct MediaRow {
    key: SessionKey,
    playing: bool,
    buttons: [Interaction; 3],
}

/// The picture of one window and what belongs to it.
struct Tile {
    id: AppWindowId,
    /// The tooltip its application gave its picture, or its title.
    title: SharedString,
    /// Absent when Windows gives no picture.
    thumbnail: Option<Thumbnail>,
    /// The logical size of the picture.
    picture: (f32, f32),
    close: Interaction,
    /// The buttons its application put under its picture, each with its
    /// icon and its pointer feedback.
    buttons: Vec<(ThumbButton, Icon, Interaction)>,
}

/// The root view of a preview: the tiles of the windows side by side, and
/// under them the controls of what their application plays.
pub struct PreviewView {
    tiles: Vec<Tile>,
    /// The edge the preview's surface is docked to; it moves in from there.
    edge: ScreenEdge,
    /// 0 while hidden, 1 when fully open.
    visibility: Spring,
    closing: bool,
    media: Option<MediaRow>,
    /// What the applications had told when it was read last.
    told: Option<u64>,
}

impl PreviewView {
    /// Starts closing; the window goes away once the motion has finished.
    fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.visibility.animate_to(0.);
        cx.notify();
    }

    /// Whether any tile has buttons under its picture.
    fn has_buttons(&self) -> bool {
        { self.tiles.iter() }.any(|tile| tile.buttons.iter().any(|(button, ..)| !button.hidden))
    }

    /// Whether the pictures were shrunk to fit the display.
    fn shrunk(&self) -> bool {
        { self.tiles.iter() }.any(|tile| tile.thumbnail.is_some() && tile.picture.0 < LEAST_WIDTH)
            && self.tiles.len() > 1
    }

    /// The logical width and height of the panel around the tiles.
    fn extent(&self, padding: f32) -> (f32, f32) {
        let shrunk = self.shrunk();
        let widths = self
            .tiles
            .iter()
            .map(|tile| tile_width(tile.picture.0, shrunk));
        let count = self.tiles.len().max(1) as f32;
        let width = widths.sum::<f32>() + padding * (count - 1.);
        let tallest = { self.tiles.iter() }
            .filter(|tile| tile.thumbnail.is_some())
            .map(|tile| tile.picture.1)
            .fold(0., f32::max);
        let buttons = self.has_buttons();
        let mut height = match tallest > 0. {
            true => tile_height(tallest, buttons, padding),
            false if buttons => TITLE_HEIGHT + padding + BUTTON_SIDE,
            false => TITLE_HEIGHT,
        };
        if self.media.is_some() {
            height += padding + MEDIA_HEIGHT;
        }
        (width.max(LEAST_WIDTH) + padding * 2., height + padding * 2.)
    }

    /// Reads what the applications told the taskbar of the windows: the
    /// tooltips of their pictures and the buttons under them. Says
    /// whether it changed.
    fn take_told(&mut self, motion: &Motion) -> bool {
        let generation = TaskbarList::generation();
        if self.told == Some(generation) {
            return false;
        }
        self.told = Some(generation);
        for tile in &mut self.tiles {
            let told = TaskbarList::state(tile.id).unwrap_or_default();
            tile.title = told.tooltip.unwrap_or_else(|| tile.id.title()).into();
            let kept = std::mem::take(&mut tile.buttons);
            tile.buttons = { told.buttons.into_iter() }
                .map(|button| {
                    // A button keeps its picture and its motion while its
                    // icon stays the same.
                    let stamp = button.icon.as_ref().map(|icon| icon.stamp);
                    let before = kept.iter().find(|(old, ..)| {
                        old.id == button.id && old.icon.as_ref().map(|icon| icon.stamp) == stamp
                    });
                    let (icon, interaction) = match before {
                        Some((_, icon, interaction)) => (icon.clone(), *interaction),
                        None => {
                            let icon = button.icon.as_ref();
                            let icon = icon.and_then(|icon| icon_of_pixels(&icon.pixels));
                            (icon, Interaction::new(motion))
                        }
                    };
                    (button, icon, interaction)
                })
                .collect();
        }
        true
    }

    /// Looks for what the application plays, and goes on reading what the
    /// applications tell while the preview is open, placing it again
    /// where its size changes.
    fn watch(
        &self,
        owner: PreviewOwner,
        padding: f32,
        anchor: PopupAnchor,
        surface: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        let sessions = cx.background_executor().spawn(async { Media::sessions() });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let found = match sessions.await {
                Ok(sessions) => sessions
                    .into_iter()
                    .find(|session| owner.plays(&session.key.app)),
                Err(_) => None,
            };
            if let Some(session) = found {
                let motion = cx.update(|cx| cx.global::<Theme>().motion).ok();
                let Some(motion) = motion else {
                    return;
                };
                let row = MediaRow {
                    key: session.key,
                    playing: session.playing,
                    buttons: [Interaction::new(&motion); 3],
                };
                let placed = this.update(cx, |this, cx| {
                    this.media = Some(row);
                    cx.notify();
                });
                if placed.is_err() {
                    return;
                }
            }
            let mut extent = None;
            loop {
                let changed = this.update(cx, |this, cx| {
                    if this.closing {
                        return None;
                    }
                    let motion = cx.global::<Theme>().motion;
                    if this.take_told(&motion) {
                        cx.notify();
                    }
                    Some(this.extent(padding))
                });
                let Ok(Some(now)) = changed else {
                    break;
                };
                if extent != Some(now) {
                    extent = Some(now);
                    let frame = anchor.frame(now.0, now.1);
                    let placed = cx.update_window(surface, |_, window, _| place(window, frame));
                    if placed.is_err() {
                        break;
                    }
                }
                cx.background_executor().timer(TOLD_CHECK).await;
            }
        })
        .detach();
    }

    /// Where Windows draws the picture of the tile at `index`, in physical
    /// pixels of the window.
    fn picture_area(
        &self,
        index: usize,
        padding: f32,
        visibility: f32,
        window: &Window,
    ) -> ScreenRect {
        let shrunk = self.shrunk();
        let before: f32 = { self.tiles[..index].iter() }
            .map(|tile| tile_width(tile.picture.0, shrunk) + padding)
            .sum();
        let tile = &self.tiles[index];
        let (width, height) = tile.picture;
        let inner = tile_width(width, shrunk);
        let left = MARGIN + padding + before + (inner - width) / 2.;
        let travel = floating_travel(self.edge, visibility) / px(1.);
        let top = MARGIN + travel + padding + TITLE_HEIGHT + padding;
        let physical = |length: f32| (length * window.scale_factor()).round() as i32;
        ScreenRect {
            left: physical(left),
            top: physical(top),
            right: physical(left + width),
            bottom: physical(top + height),
        }
    }

    /// The tile at `index`: the title, a button that closes the window,
    /// the picture, which switches to it, and the buttons under it.
    fn render_tile(
        &mut self,
        index: usize,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shrunk = self.shrunk();
        let buttons_row = self.has_buttons();
        let tallest = { self.tiles.iter() }
            .filter(|tile| tile.thumbnail.is_some())
            .map(|tile| tile.picture.1)
            .fold(0., f32::max);
        let tile = &mut self.tiles[index];
        let target = tile.id;
        let (width, height) = tile.picture;
        let state = tile.close.state(window);
        let close = div()
            .id(("preview-close", index))
            .size(px(TITLE_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(theme, theme.colors.surface_container, state))
            .text_color(theme.colors.on_surface_variant)
            .on_click(cx.listener(move |this, _, _, cx| {
                target.close();
                this.forget(target, cx);
            }))
            .child(icon(theme, ICON_CLOSE).text_size(theme.typography.label_size));
        let close = Interaction::track(close, cx, move |this: &mut Self| {
            Some(&mut this.tiles.get_mut(index)?.close)
        });
        let title = div()
            .h(px(TITLE_HEIGHT))
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(div().flex_1().min_w_0().child(one_line(tile.title.clone())))
            .child(close);
        // Windows draws the picture over this.
        let picture = tile.thumbnail.is_some().then(|| {
            div()
                .id(("preview-picture", index))
                .w(px(width))
                .h(px(height))
                .mx_auto()
                .flex_none()
                .bg(theme.colors.surface_container_highest)
                .on_click(move |_, _, cx| {
                    target.activate();
                    cx.defer(Previews::close);
                })
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, _, cx| {
                        target.close();
                        this.forget(target, cx);
                    }),
                )
        });
        // Pictures of different heights leave their buttons level.
        let filler = (tallest > height && tile.thumbnail.is_some())
            .then(|| div().h(px(tallest - height)).flex_none());
        let buttons = buttons_row.then(|| self.render_buttons(index, theme, window, cx));
        let tile = &self.tiles[index];
        div()
            .w(px(tile_width(tile.picture.0, shrunk)))
            .flex_none()
            .flex()
            .flex_col()
            .gap(theme.spacing.medium)
            .child(title)
            .children(picture)
            .children(filler)
            .children(buttons)
            .into_any_element()
    }

    /// The buttons the application of the tile at `index` put under its
    /// picture, which tell it of a click.
    fn render_buttons(
        &mut self,
        index: usize,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = self.tiles[index].id;
        let mut row = div()
            .h(px(BUTTON_SIDE))
            .flex()
            .items_center()
            .justify_center()
            .gap(theme.spacing.tiny);
        let count = self.tiles[index].buttons.len();
        for place in 0..count {
            let (button, shown, interaction) = &mut self.tiles[index].buttons[place];
            if button.hidden {
                continue;
            }
            let state = interaction.state(window);
            let (id, dismiss, enabled) =
                (button.id, button.dismiss, button.enabled && !button.inert);
            let tip: SharedString = button.tip.clone().into();
            let tip_key = (
                "thumb-button",
                (target.raw() as u64) << 8 | u64::from(id & 0xFF),
            );
            let background = match button.plain {
                true => theme.colors.surface_container,
                false => theme.colors.surface_container_highest,
            };
            let face = match shown.clone() {
                Some(picture) => img(picture).size(px(BUTTON_ICON)).into_any_element(),
                // An image of the application's image list cannot be read:
                // the first letter of its tooltip stands for it.
                None => div()
                    .text_size(theme.typography.label_size)
                    .child(SharedString::from(
                        button
                            .tip
                            .chars()
                            .next()
                            .map(String::from)
                            .unwrap_or_default(),
                    ))
                    .into_any_element(),
            };
            let element = div()
                .id(("thumb-button", place))
                .size(px(BUTTON_SIDE))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme.shape.small)
                .bg(state_layer(theme, background, state))
                .opacity(if enabled { 1. } else { 0.4 })
                .child(face)
                .on_hover(move |hovered, window, cx| {
                    if !*hovered {
                        Tips::leave(tip_key, cx);
                        return;
                    }
                    let x = window.mouse_position().x;
                    if let Ok(anchor) = PopupAnchor::at(window, x, cx) {
                        Tips::request(tip_key, tip.clone(), anchor, cx);
                    }
                });
            let element = match enabled {
                true => element.on_click(move |_, _, cx| {
                    TaskbarList::click(target, id);
                    if dismiss {
                        cx.defer(Previews::close);
                    }
                }),
                false => element,
            };
            let element = Interaction::track(element, cx, move |this: &mut Self| {
                let (_, _, interaction) = this.tiles.get_mut(index)?.buttons.get_mut(place)?;
                Some(interaction)
            });
            row = row.child(element);
        }
        row.into_any_element()
    }

    /// The controls of what the application plays.
    fn render_media(
        &mut self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let media = self.media.as_mut()?;
        let toggle = if media.playing { ICON_PAUSE } else { ICON_PLAY };
        let commands = [
            (ICON_PREVIOUS, MediaCommand::Previous),
            (toggle, MediaCommand::Toggle),
            (ICON_NEXT, MediaCommand::Next),
        ];
        let mut row = div()
            .h(px(MEDIA_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .gap(theme.spacing.small);
        for (place, (glyph, command)) in commands.into_iter().enumerate() {
            let state = media.buttons[place].state(window);
            let key = media.key.clone();
            let button = div()
                .id(("preview-media", place))
                .size(px(MEDIA_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme.shape.full)
                .bg(state_layer(
                    theme,
                    theme.colors.surface_container_highest,
                    state,
                ))
                .text_color(theme.colors.on_surface)
                .child(icon(theme, glyph))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let key = key.clone();
                    cx.background_executor()
                        .spawn(async move {
                            if let Err(error) = Media::send(Some(&key), command) {
                                report!("{error}");
                            }
                        })
                        .detach();
                    if command == MediaCommand::Toggle
                        && let Some(media) = &mut this.media
                    {
                        media.playing = !media.playing;
                        cx.notify();
                    }
                }));
            let button = Interaction::track(button, cx, move |this: &mut Self| {
                Some(&mut this.media.as_mut()?.buttons[place])
            });
            row = row.child(button);
        }
        Some(row.into_any_element())
    }

    /// The window `target` is closed: its tile goes, and the preview with
    /// the last one.
    fn forget(&mut self, target: AppWindowId, cx: &mut Context<Self>) {
        self.tiles.retain(|tile| tile.id != target);
        if self.tiles.is_empty() {
            // This window cannot be told to close while it handles the
            // click.
            cx.defer(Previews::close);
        }
        cx.notify();
    }
}

/// Moves the preview's window to `frame`, for a size that changed.
fn place(window: &mut Window, frame: ScreenRect) {
    if let Ok(native) = NativeSurface::of(window)
        && let Err(error) = native.place(frame)
    {
        report!("a preview is not where it belongs: {error:#}");
    }
}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let padding = theme.spacing.medium / px(1.);
        let visibility = self.visibility.value(window);
        if self.closing && self.visibility.is_settled() {
            // The pictures of the windows go with the preview, not when
            // their windows are opened again.
            for tile in &mut self.tiles {
                tile.thumbnail = None;
            }
            retire_floating(window, cx);
        }
        // A window that closed meanwhile loses its tile.
        let before = self.tiles.len();
        self.tiles.retain(|tile| tile.id.exists());
        if self.tiles.is_empty() && before > 0 && !self.closing {
            cx.defer(Previews::close);
        }
        for index in 0..self.tiles.len() {
            let area = self.picture_area(index, padding, visibility, window);
            if let Some(thumbnail) = &self.tiles[index].thumbnail {
                thumbnail.place(area, visibility);
            }
        }
        let tiles: Vec<AnyElement> = { 0..self.tiles.len() }
            .map(|index| self.render_tile(index, &theme, window, cx))
            .collect();
        let media = self.render_media(&theme, window, cx);
        let row = div().flex().gap(theme.spacing.medium).children(tiles);
        floating_panel(&theme, self.edge, visibility, |panel| {
            panel.child(row).children(media)
        })
        .id("preview")
        .on_hover(|hovered, _, cx| {
            if *hovered {
                Previews::keep(cx);
            } else {
                Previews::dismiss(cx);
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_keeps_its_shape_within_the_limits() {
        // A wide window is as wide as allowed, a tall one as tall.
        assert_eq!(fit((2600, 1000), 1.), (260., 100.));
        assert_eq!(fit((800, 1600), 1.), (80., 160.));
        // On a denser display the same pixels are a smaller window.
        assert_eq!(fit((2600, 1000), 2.), (260., 100.));
        // A small window is not enlarged.
        assert_eq!(fit((100, 50), 1.), (100., 50.));
    }

    #[test]
    fn pictures_side_by_side_shrink_alike_to_fit_the_display() {
        let pictures = [Some((260., 100.)), Some((260., 160.)), None];
        let room = 260. * 2. + LEAST_WIDTH;
        assert_eq!(lay_out(&pictures, room), pictures.to_vec());
        let halved = lay_out(&pictures, room / 2.);
        assert_eq!(halved[0], Some((130., 50.)));
        assert_eq!(halved[1], Some((130., 80.)));
        assert_eq!(halved[2], None);
    }

    #[test]
    fn a_tile_leaves_room_for_the_buttons_under_its_picture() {
        assert_eq!(tile_height(100., false, 12.), TITLE_HEIGHT + 112.);
        assert_eq!(
            tile_height(100., true, 12.),
            TITLE_HEIGHT + 124. + BUTTON_SIDE
        );
        assert_eq!(tile_width(80., false), LEAST_WIDTH);
        assert_eq!(tile_width(80., true), 80.);
    }

    #[test]
    fn the_media_of_an_application_is_found_by_its_names() {
        let packaged = PreviewOwner {
            app: Some("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify".to_owned()),
            program: None,
        };
        assert!(packaged.plays("spotifyab.spotifymusic_zpdnekdrzrea0!spotify"));
        assert!(!packaged.plays("Spotify.exe"));
        let program = PreviewOwner {
            app: None,
            program: Some(PathBuf::from(r"C:\Apps\Spotify.exe")),
        };
        assert!(program.plays("spotify.EXE"));
        assert!(!PreviewOwner::default().plays("Spotify.exe"));
    }
}
