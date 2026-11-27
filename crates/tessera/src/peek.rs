// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A look through the windows, as Aero Peek of Windows has it.
//!
//! [`Peek::show`] dims every display and shows one window where it is, in
//! front of the others, as the taskbar of Windows does while the pointer
//! rests on a preview. [`Peek::desktop`] shows the desktop in front of
//! every window, with a thin outline where each window is, as Win+comma
//! does while it is held: the action `peek-desktop`, which brings that key
//! where the keys of Windows are in use.
//!
//! Each display gets a window of its own that lies above the ordinary
//! windows and under what is kept above them, so Tessera's bars, previews
//! and popups stay in sight over a peek, and a window that is kept above
//! the others itself stays so. The pointer and the keyboard stay where
//! they are. The window looked at is a picture the compositor draws (a
//! DWM thumbnail), so it is seen live; the desktop is the picture Windows
//! paints behind the windows, which is that of Windows: the wallpaper
//! Tessera draws itself is not in it, nor are icons on the desktop.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Bounds, Context, DisplayId, Global, IntoElement, ParentElement,
    Pixels, Render, RenderImage, Styled, WeakEntity, Window, WindowHandle, black, div, img, point,
    px, size,
};

use crate::actions::{Action, Actions};
use crate::module::{Module, WidgetRegistry};
use crate::motion::Spring;
use crate::platform::{
    AppWindow, AppWindowId, Display, Modifiers, NativeSurface, ScreenRect, SmallBirth, Thumbnail,
    paint_desktop,
};
use crate::popup::floating_window;
use crate::theme::Theme;

/// How dark the displays get behind the window looked at, from 0 to 1.
const DIM: f32 = 0.6;

/// How see-through the inside of the outline of a window is over the
/// desktop, from 0 to 1.
const OUTLINE_FILL: f32 = 0.16;

/// How often it is checked whether the keys that hold the peek at the
/// desktop are still down.
const RELEASE_CHECK: Duration = Duration::from_millis(30);

/// What a peek shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Looking {
    /// One window, in front of the dimmed displays.
    Window(AppWindowId),
    /// The desktop, in front of every window.
    Desktop,
}

/// The peek that is shown, if one is.
#[derive(Default)]
pub struct Peek {
    /// One window for each display, while a peek is shown.
    views: Vec<WindowHandle<PeekView>>,
    looking: Option<Looking>,
    /// Counts the peeks, so that a peek held by keys ends only itself.
    shown: u64,
    /// Whether the peek shown is held by keys.
    held: bool,
}

impl Global for Peek {}

impl Peek {
    /// Shows `id` where it is, in front of the other windows, over dimmed
    /// displays, until [`Self::hide`] or another peek.
    #[allow(dead_code, reason = "the previews of the dock call it once they peek")]
    pub fn show(id: AppWindowId, cx: &mut App) {
        Self::look(Looking::Window(id), cx);
    }

    /// Shows the desktop in front of every window, with the outlines of
    /// the windows, until [`Self::hide`] or another peek.
    pub fn desktop(cx: &mut App) {
        Self::look(Looking::Desktop, cx);
    }

    /// Ends the peek, if one is shown: everything is seen as before.
    pub fn hide(cx: &mut App) {
        let peek = cx.default_global::<Self>();
        peek.looking = None;
        peek.held = false;
        for view in std::mem::take(&mut peek.views) {
            // A view that is gone has closed already.
            view.update(cx, |view, _, cx| view.close(cx)).ok();
        }
    }

    /// Shows what `looking` names, on the windows of the peek shown where
    /// the displays are the same, or else on new ones.
    fn look(looking: Looking, cx: &mut App) {
        let peek = cx.default_global::<Self>();
        if peek.looking == Some(looking) {
            return;
        }
        peek.shown += 1;
        peek.held = false;
        let displays = Display::all();
        let views = std::mem::take(&mut peek.views);
        let mut kept = Vec::new();
        for view in views {
            let looked = view.update(cx, |view, window, cx| {
                let there = displays.contains(&view.display);
                there.then(|| view.look_at(looking, window, cx))
            });
            match looked {
                Ok(Some(())) => kept.push(view),
                Ok(None) => {
                    view.update(cx, |view, _, cx| view.close(cx)).ok();
                }
                Err(_) => {}
            }
        }
        let shown = kept.clone();
        for display in displays {
            let seen = shown.iter().any(|view| {
                view.read_with(cx, |view, _| view.display == display)
                    .unwrap_or(false)
            });
            if seen {
                continue;
            }
            match open(display, looking, cx) {
                Ok(view) => kept.push(view),
                Err(error) => report!("a display shows no peek: {error:#}"),
            }
        }
        let peek = cx.global_mut::<Self>();
        peek.views = kept;
        peek.looking = Some(looking);
    }

    /// The action `peek-desktop`: shows the desktop until none of the keys
    /// to hold of the combination that did it is down; done otherwise, as
    /// by `tessera do`, it shows the desktop or ends that peek. It takes
    /// its keys from Windows and brings Win+comma where the keys of
    /// Windows are in use.
    fn action() -> Action {
        Action::new(|_, cx| {
            let held = Actions::held(cx).unwrap_or_else(Modifiers::down_now);
            Self::desktop_while(held, cx);
        })
        .taking_reserved_keys()
        .by_default("win+comma", None)
    }

    /// Shows the desktop while one of `held` is down, or, with none, shows
    /// it or ends that peek.
    fn desktop_while(held: Modifiers, cx: &mut App) {
        let peek = cx.default_global::<Self>();
        let peeking = peek.looking == Some(Looking::Desktop);
        if held == Modifiers::default() {
            match peeking {
                true => Self::hide(cx),
                false => Self::desktop(cx),
            }
            return;
        }
        // A key held down repeats itself.
        if peeking && peek.held {
            return;
        }
        Self::desktop(cx);
        let peek = cx.global_mut::<Self>();
        peek.held = true;
        let shown = peek.shown;
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(RELEASE_CHECK).await;
                let still = cx.update(|cx| cx.global::<Self>().shown == shown);
                if !matches!(still, Ok(true)) {
                    return;
                }
                if !held.any_down() {
                    break;
                }
            }
            cx.update(|cx| {
                if cx.global::<Self>().shown == shown {
                    Self::hide(cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

/// Opens the window of a peek at `looking` over `display`.
fn open(
    display: Display,
    looking: Looking,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<PeekView>> {
    let frame = display.frame;
    let scale = display.scale;
    let rough = Bounds {
        origin: point(px(frame.left as f32 / scale), px(frame.top as f32 / scale)),
        size: size(
            px((frame.right - frame.left) as f32 / scale),
            px((frame.bottom - frame.top) as f32 / scale),
        ),
    };
    let options = floating_window(rough, gpui_display(&display, cx), false);
    let _small = SmallBirth::begin();
    cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            match NativeSurface::of(window) {
                // The window learns its size only if it is given while
                // nothing of the user interface is being worked on, as it
                // is right after this.
                Ok(native) => cx
                    .spawn(async move |_, _| {
                        let settled = native.adopt_as_shell_surface().and_then(|()| {
                            native.place(frame)?;
                            native.reveal(false);
                            native.lay_over_windows();
                            Ok(())
                        });
                        if let Err(error) = settled {
                            report!("a peek is not where it belongs: {error:#}");
                        }
                    })
                    .detach(),
                Err(error) => report!("a peek is not where it belongs: {error:#}"),
            }
            let motion = cx.global::<Theme>().motion;
            let mut view = PeekView {
                display,
                looking,
                thumbnail: None,
                picture: None,
                outlines: Vec::new(),
                visibility: Spring::new(motion.effects, 0.),
                pictured: Spring::new(motion.effects, 0.),
                closing: false,
                looks: 0,
            };
            view.look_at(looking, window, cx);
            view
        })
    })
}

/// The display of GPUI that is at `display`, for a window to open on.
fn gpui_display(display: &Display, cx: &App) -> Option<DisplayId> {
    let (x, y) = (
        display.frame.left as f32 / display.scale,
        display.frame.top as f32 / display.scale,
    );
    let off = |bounds: Bounds<Pixels>| {
        (bounds.origin.x / px(1.) - x).abs() + (bounds.origin.y / px(1.) - y).abs()
    };
    let displays = cx.displays();
    let nearest = { displays.iter() }
        .min_by(|first, second| off(first.bounds()).total_cmp(&off(second.bounds())));
    nearest.map(|display| display.id())
}

/// Whether `first` and `second` share some of their area.
fn meet(first: ScreenRect, second: ScreenRect) -> bool {
    first.left < second.right
        && second.left < first.right
        && first.top < second.bottom
        && second.top < first.bottom
}

/// `rect` from the top left corner of `frame` instead of the screen.
fn within(rect: ScreenRect, frame: ScreenRect) -> ScreenRect {
    ScreenRect {
        left: rect.left - frame.left,
        top: rect.top - frame.top,
        right: rect.right - frame.left,
        bottom: rect.bottom - frame.top,
    }
}

/// The window of a peek over one display.
pub struct PeekView {
    display: Display,
    looking: Looking,
    /// The picture of the window looked at, and where it goes from the
    /// corner of the display, while that window is on this display.
    thumbnail: Option<(Thumbnail, ScreenRect)>,
    /// The picture of the desktop, once it is painted.
    picture: Option<Arc<RenderImage>>,
    /// Where the windows are, from the corner of the display, while the
    /// desktop is looked at.
    outlines: Vec<ScreenRect>,
    /// 0 while hidden, 1 when fully shown.
    visibility: Spring,
    /// The same for the picture of the desktop, which comes later.
    pictured: Spring,
    closing: bool,
    /// Counts what was looked at, so that a picture painted for an earlier
    /// look is not shown.
    looks: u64,
}

impl PeekView {
    /// Shows what `looking` names over this display.
    fn look_at(&mut self, looking: Looking, window: &Window, cx: &mut Context<Self>) {
        self.looks += 1;
        self.looking = looking;
        self.thumbnail = None;
        self.outlines.clear();
        let frame = self.display.frame;
        match looking {
            Looking::Window(id) => {
                // A window that stands for a tab draws no picture of its
                // own until asked for it.
                Thumbnail::ask_for_iconic(id, None);
                let thumbnail = NativeSurface::of(window)
                    .ok()
                    .and_then(|native| Thumbnail::of(id, native).ok());
                self.thumbnail = thumbnail.and_then(|thumbnail| {
                    let pictured = id.pictured_frame(thumbnail.source_size()?)?;
                    meet(pictured, frame).then(|| (thumbnail, within(pictured, frame)))
                });
            }
            Looking::Desktop => {
                let own = std::process::id();
                self.outlines = { AppWindow::enumerate().into_iter() }
                    .filter(|window| window.id.process() != own && !window.id.is_minimized())
                    .filter_map(|window| window.id.frame())
                    .filter(|seen| meet(*seen, frame))
                    .map(|seen| within(seen, frame))
                    .collect();
                if self.picture.is_none() {
                    self.paint(cx);
                }
            }
        }
        self.visibility.animate_to(1.);
        cx.notify();
    }

    /// Has the desktop painted for this display, away from the thread of
    /// the interface, and shown once it is.
    fn paint(&mut self, cx: &mut Context<Self>) {
        let (frame, looks) = (self.display.frame, self.looks);
        let painted = cx.background_executor().spawn(async move {
            let (width, height, pixels) = paint_desktop(frame)?;
            let buffer = image::RgbaImage::from_raw(width, height, pixels)?;
            Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let Some(picture) = painted.await else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.looks == looks && !this.closing {
                    this.picture = Some(picture);
                    this.pictured.animate_to(1.);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Starts going away; the window is removed once it has faded out.
    fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.visibility.animate_to(0.);
        cx.notify();
    }
}

impl Render for PeekView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let visibility = self.visibility.value(window);
        if self.closing && self.visibility.is_settled() {
            self.thumbnail = None;
            window.remove_window();
        }
        if let Some((thumbnail, area)) = &self.thumbnail {
            thumbnail.place(*area, visibility);
        }
        let scale = self.display.scale;
        let logical = |length: i32| px(length as f32 / scale);
        let sheet = div().size_full().relative();
        match self.looking {
            Looking::Window(_) => sheet.bg(black().opacity(DIM * visibility)),
            Looking::Desktop => {
                let pictured = self.pictured.value(window);
                let picture = self
                    .picture
                    .clone()
                    .map(|picture| img(picture).absolute().size_full().opacity(pictured));
                let outlines = self.outlines.iter().map(|outline| {
                    div()
                        .absolute()
                        .left(logical(outline.left))
                        .top(logical(outline.top))
                        .w(logical(outline.right - outline.left))
                        .h(logical(outline.bottom - outline.top))
                        .rounded(theme.shape.small)
                        .border_1()
                        .border_color(theme.colors.outline)
                        .bg(theme.colors.surface_container.opacity(OUTLINE_FILL))
                });
                sheet
                    .opacity(visibility)
                    .bg(black().opacity(DIM))
                    .children(picture)
                    .children(outlines)
            }
        }
    }
}

/// The module that brings the action `peek-desktop`.
pub struct PeekModule;

impl Module for PeekModule {
    fn name(&self) -> &'static str {
        "peek"
    }

    fn register(&self, _widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        Ok(())
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        actions.register("peek-desktop", Peek::action())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn a_window_is_pictured_on_the_displays_it_reaches() {
        let left = rect(0, 0, 1920, 1080);
        let right = rect(1920, 0, 3840, 1080);
        let window = rect(1800, 100, 2200, 500);
        assert!(meet(window, left));
        assert!(meet(window, right));
        assert!(!meet(rect(0, 0, 1920, 1080), right));
        assert_eq!(within(window, right), rect(-120, 100, 280, 500));
    }

    #[test]
    fn the_peek_at_the_desktop_brings_win_comma() {
        let mut actions = Actions::default();
        PeekModule.actions(&mut actions).unwrap();
        assert!(actions.takes_reserved_keys("peek-desktop"));
        assert!(actions.check("peek-desktop", None).is_ok());
        assert!(actions.check("peek-desktop", Some("now")).is_err());
        let keys = actions.default_keys();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].press, "win+comma");
    }
}
