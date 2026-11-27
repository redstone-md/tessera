// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The prompt that names a workspace: a field with the name of the
//! workspace in it, all of it selected, which opens next to the button of
//! the workspace, or near the top of the display for the action. Enter
//! writes the name into the configuration, an empty one giving the
//! workspace its number back; Escape, and turning to another window,
//! close it without a change.

use gpui::{
    App, AppContext, Bounds, Context, Entity, Global, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, Styled, Subscription, Window, WindowHandle,
    div, point, px, size,
};

use crate::field::TextField;
use crate::i18n::Language;
use crate::menu::Menus;
use crate::motion::Spring;
use crate::platform::{Display, DisplayKey, NativeSurface, ScreenEdge};
use crate::popup::{PopupAnchor, floating_panel, open_floating, retire_floating};
use crate::snap::anchor_at;
use crate::theme::Theme;

use super::{Workspaces, named};

/// The width of the prompt, in logical pixels.
const WIDTH: f32 = 280.;

/// The pencil of the icon font, before the field.
const ICON_RENAME: &str = "\u{e8ac}";

/// The prompt that is open.
#[derive(Default)]
struct OpenPrompt(Option<WindowHandle<RenamePrompt>>);

impl Global for OpenPrompt {}

/// The root view of the window of a prompt.
pub struct RenamePrompt {
    /// The place of the workspace named.
    index: usize,
    field: Entity<TextField>,
    /// The edge the prompt moves in from.
    edge: ScreenEdge,
    /// 0 while hidden, 1 when fully open.
    visibility: Spring,
    closing: bool,
    /// Whether the keys were given to the field yet.
    focused: bool,
    /// Whether the window has had the keyboard; before that, not having it
    /// does not mean the user turned away.
    was_active: bool,
    _activation: Subscription,
}

impl RenamePrompt {
    /// Opens the prompt for the workspace at `index` at `anchor`; another
    /// prompt and an open menu close.
    pub fn open(index: usize, anchor: PopupAnchor, cx: &mut App) {
        Self::close(cx);
        Menus::close(cx);
        let theme = *cx.global::<Theme>();
        let height = theme.launcher.field_height / px(1.) + theme.spacing.medium / px(1.) * 2.;
        let frame = anchor.frame(WIDTH, height);
        let rough = Bounds {
            origin: point(
                px(frame.left as f32 / anchor.scale),
                px(frame.top as f32 / anchor.scale),
            ),
            size: size(px(WIDTH), px(height)),
        };
        let edge = anchor.edge;
        let opened = open_floating(rough, anchor.display_id, true, cx, |window, cx| {
            let adopted = NativeSurface::of(window).and_then(|native| {
                native.adopt_as_popup()?;
                native.place(frame)?;
                native.reveal(true);
                Ok(())
            });
            if let Err(error) = adopted {
                report!("the prompt for a name is not where it belongs: {error:#}");
            }
            Self::new(index, edge, window, cx)
        });
        match opened {
            Ok(window) => cx.default_global::<OpenPrompt>().0 = Some(window),
            Err(error) => report!("the prompt for a name did not open: {error:#}"),
        }
    }

    /// Opens the prompt for the workspace at `index` of `display` near the
    /// top of that display, or of the first one.
    pub fn open_near(index: usize, display: Option<DisplayKey>, cx: &mut App) {
        let displays = Display::all();
        let Some(display) = { displays.iter() }
            .find(|shown| Some(shown.key) == display)
            .or(displays.first())
        else {
            return;
        };
        let work = display.work;
        let point = (
            (work.left + work.right) / 2,
            work.top + (work.bottom - work.top) / 6,
        );
        let anchor = anchor_at(display, point, cx);
        Self::open(index, anchor, cx);
    }

    /// Closes the open prompt, if there is one.
    fn close(cx: &mut App) {
        if let Some(window) = cx.default_global::<OpenPrompt>().0.take() {
            // A prompt that has closed by itself is gone, which is fine.
            window.update(cx, |view, _, cx| view.leave(cx)).ok();
        }
    }

    fn new(index: usize, edge: ScreenEdge, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let language = Language::of(cx);
        let placeholder = SharedString::new(language.say("Name"));
        let name = named(&cx.global::<Workspaces>().looks.names, index).unwrap_or_default();
        let field = cx.new(|cx| {
            let mut field = TextField::new(placeholder, cx).with_glyph(ICON_RENAME);
            field.set_text(&name, false, cx);
            field
        });
        let motion = cx.global::<Theme>().motion;
        let mut visibility = Spring::new(motion.spatial, 0.);
        visibility.animate_to(1.);
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.was_active = true;
            } else if this.was_active {
                this.leave(cx);
            }
        });
        Self {
            index,
            field,
            edge,
            visibility,
            closing: false,
            focused: false,
            was_active: false,
            _activation: activation,
        }
    }

    /// Starts closing; the window goes away once the motion has finished.
    fn leave(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.visibility.animate_to(0.);
        cx.notify();
    }

    /// Names the workspace as the field says, and closes.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let name = self.field.read(cx).text();
        let index = self.index;
        cx.defer(move |cx| Workspaces::rename(index, &name, cx));
        self.leave(cx);
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => self.confirm(cx),
            "escape" => self.leave(cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for RenamePrompt {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let visibility = self.visibility.value(window);
        if self.closing && self.visibility.is_settled() {
            retire_floating(window, cx);
        }
        if !self.focused {
            self.focused = true;
            self.field.read(cx).focus(window);
        }
        floating_panel(&theme, self.edge, visibility, |panel| {
            panel.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
                    .child(self.field.clone()),
            )
        })
    }
}
