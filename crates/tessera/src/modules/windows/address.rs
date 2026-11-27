// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The prompt that pins a web address to the dock: a field that opens
//! where the menu of the dock was opened. Enter pins what it holds, a web
//! address or a link of any scheme (`https://…`, `mailto:…`), with
//! `https://` put before a bare name such as `example.org`; Escape, and
//! turning to another window, close it without a change.

use gpui::{
    App, AppContext, Bounds, Context, Entity, Global, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, Styled, Subscription, Window, WindowHandle,
    div, point, px, size,
};

use super::pins::scheme_of;
use super::{repin, with};
use crate::field::TextField;
use crate::i18n::Language;
use crate::menu::Menus;
use crate::motion::Spring;
use crate::platform::{NativeSurface, ScreenEdge};
use crate::popup::{PopupAnchor, floating_panel, open_floating, retire_floating};
use crate::theme::Theme;

/// The width of the prompt, in logical pixels.
const WIDTH: f32 = 360.;

/// The globe of the icon font, before the field.
const ICON_LINK: &str = "\u{e774}";

/// The link `typed` stands for: itself where it names a scheme, a web
/// address where it is a bare name with a dot and no spaces, as
/// `example.org/news`; `None` for anything else.
pub(super) fn address_of(typed: &str) -> Option<String> {
    let typed = typed.trim();
    // A host with a port, as `example.org:8080`, looks like a scheme.
    if scheme_of(typed).is_some_and(|scheme| !scheme.contains('.')) {
        return Some(typed.to_owned());
    }
    let bare =
        typed.contains('.') && !typed.contains(char::is_whitespace) && !typed.starts_with('.');
    bare.then(|| format!("https://{typed}"))
}

/// The prompt that is open.
#[derive(Default)]
struct OpenPrompt(Option<WindowHandle<AddressPrompt>>);

impl Global for OpenPrompt {}

/// The root view of the window of the prompt.
pub(super) struct AddressPrompt {
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

impl AddressPrompt {
    /// Opens the prompt at `anchor`; another one and an open menu close.
    pub(super) fn open(anchor: PopupAnchor, cx: &mut App) {
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
                report!("the prompt for a web address is not where it belongs: {error:#}");
            }
            Self::new(edge, window, cx)
        });
        match opened {
            Ok(window) => cx.default_global::<OpenPrompt>().0 = Some(window),
            Err(error) => report!("the prompt for a web address did not open: {error:#}"),
        }
    }

    fn close(cx: &mut App) {
        if let Some(window) = cx.default_global::<OpenPrompt>().0.take() {
            // A prompt that has closed by itself is gone, which is fine.
            window.update(cx, |view, _, cx| view.leave(cx)).ok();
        }
    }

    fn new(edge: ScreenEdge, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = SharedString::new(Language::of(cx).say("Web address"));
        let field = cx.new(|cx| TextField::new(placeholder, cx).with_glyph(ICON_LINK));
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

    /// Pins what the field holds and closes, where it is a link; a text
    /// that is none stays for the user to mend.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let Some(link) = address_of(&self.field.read(cx).text()) else {
            return;
        };
        cx.defer(move |cx| repin(cx, move |pins| with(pins, &link)));
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

impl Render for AddressPrompt {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_name_becomes_a_web_address() {
        assert_eq!(
            address_of(" example.org/news ").as_deref(),
            Some("https://example.org/news")
        );
        assert_eq!(
            address_of("mailto:me@example.org").as_deref(),
            Some("mailto:me@example.org")
        );
        assert_eq!(
            address_of("http://localhost:8080").as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(
            address_of("example.org:8080").as_deref(),
            Some("https://example.org:8080")
        );
        assert_eq!(address_of("not a link"), None);
        assert_eq!(address_of("word"), None);
        assert_eq!(address_of(".hidden"), None);
    }
}
