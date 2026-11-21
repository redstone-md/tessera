// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What is playing. Widgets: `media`, a compact readout that is there only
//! while an application offers something, and `media-controls`, the title
//! with buttons to skip and pause.

use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Context, Entity, FontWeight, Global, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, div, px,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{Media, MediaCommand, NowPlaying};
use crate::theme::Theme;
use crate::ui::{icon, pill, state_layer};

/// How often it is read what is playing.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How wide the readout gets unless the layout says otherwise.
const READOUT_WIDTH: f32 = 260.;

/// The size of a button, and how much wider the middle one is.
const BUTTON_SIZE: Pixels = px(40.);
const TOGGLE_WIDTH: Pixels = px(64.);

/// How much of its color a button keeps when its command is not accepted.
const DISABLED_OPACITY: f32 = 0.38;

const ICON_NOTE: &str = "\u{ec4f}";
const ICON_PREVIOUS: &str = "\u{e892}";
const ICON_PLAY: &str = "\u{e768}";
const ICON_PAUSE: &str = "\u{e769}";
const ICON_NEXT: &str = "\u{e893}";

pub struct MediaModule;

impl Module for MediaModule {
    fn name(&self) -> &'static str {
        "media"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("media", |context, cx| {
            let player = Player::shared(cx);
            let width = context
                .number("width")
                .map_or(READOUT_WIDTH, |width| width as f32);
            cx.new(|cx| Readout::new(player, px(width.max(0.)), cx))
                .into()
        })?;
        widgets.register("media-controls", |_, cx| {
            let player = Player::shared(cx);
            cx.new(|cx| Controls::new(player, cx)).into()
        })
    }
}

/// The title and the artist on one line, whichever of them there are.
fn headline(now: &NowPlaying) -> String {
    match (now.title.is_empty(), now.artist.is_empty()) {
        (false, false) => format!("{} \u{2013} {}", now.artist, now.title),
        (false, true) => now.title.clone(),
        (true, false) => now.artist.clone(),
        (true, true) => "Unnamed".to_owned(),
    }
}

/// What is playing, shared by every media widget.
struct Player {
    now: Option<NowPlaying>,
    /// Why nothing is known, if reading failed.
    error: Option<String>,
}

struct SharedPlayer(Entity<Player>);

impl Global for SharedPlayer {}

impl Player {
    /// The one instance, created when the first widget asks for it.
    fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(shared) = cx.try_global::<SharedPlayer>() {
            return shared.0.clone();
        }
        let player = cx.new(|cx: &mut Context<Self>| {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    let read = cx
                        .background_executor()
                        .spawn(async { Media::now_playing() });
                    let read = read.await;
                    if this.update(cx, |this, cx| this.take(read, cx)).is_err() {
                        break;
                    }
                    cx.background_executor().timer(POLL_INTERVAL).await;
                }
            })
            .detach();
            Self {
                now: None,
                error: None,
            }
        });
        cx.set_global(SharedPlayer(player.clone()));
        player
    }

    fn take(&mut self, read: Result<Option<NowPlaying>, String>, cx: &mut Context<Self>) {
        match read {
            Ok(now) => {
                self.error = None;
                if now != self.now {
                    self.now = now;
                    cx.notify();
                }
            }
            Err(error) => {
                if self.error.as_ref() != Some(&error) {
                    eprintln!("tessera: {error}");
                    self.error = Some(error);
                }
            }
        }
    }

    /// Passes `command` on and reads what came of it.
    fn send(&mut self, command: MediaCommand, cx: &mut Context<Self>) {
        // The answer takes a moment; the button should not.
        if command == MediaCommand::Toggle
            && let Some(now) = &mut self.now
        {
            now.playing = !now.playing;
            cx.notify();
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let sent = cx.background_executor().spawn(async move {
                if let Err(error) = Media::send(command) {
                    eprintln!("tessera: {error}");
                }
            });
            sent.await;
            // What changed shows at the next reading: an application
            // reports its new state only after a while.
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
}

/// Shows what is playing on one line; grows in when something starts to be
/// offered and shrinks away when nothing is.
struct Readout {
    player: Entity<Player>,
    /// What was offered last, kept while the readout leaves.
    shown: Option<NowPlaying>,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    width: Pixels,
    _changes: Subscription,
}

impl Readout {
    fn new(player: Entity<Player>, width: Pixels, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let mut this = Self {
            _changes: cx.observe(&player, |this: &mut Self, _, cx| this.follow(cx)),
            player,
            shown: None,
            presence: Spring::new(motion.spatial, 0.),
            width,
        };
        this.follow(cx);
        this
    }

    fn follow(&mut self, cx: &mut Context<Self>) {
        let now = self.player.read(cx).now.clone();
        self.presence
            .animate_to(if now.is_some() { 1. } else { 0. });
        if now.is_some() {
            self.shown = now;
        }
        cx.notify();
    }
}

impl Render for Readout {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let presence = self.presence.value(window);
        let gone = self.presence.is_settled() && presence <= 0.;
        let Some(now) = self.shown.as_ref().filter(|_| !gone) else {
            return div();
        };
        let glyph = if now.playing { ICON_NOTE } else { ICON_PAUSE };
        let color = if now.playing {
            theme.colors.primary
        } else {
            theme.colors.on_surface_variant
        };
        div()
            .overflow_hidden()
            .max_w(self.width * presence.max(0.))
            .opacity(presence.clamp(0., 1.))
            .child(
                pill(&theme)
                    .max_w(self.width)
                    .font_weight(FontWeight::MEDIUM)
                    .child(icon(&theme, glyph).flex_none().text_color(color))
                    .child(div().truncate().child(SharedString::from(headline(now)))),
            )
    }
}

/// The title, the artist, and buttons to skip back, pause or play, and
/// skip on.
struct Controls {
    player: Entity<Player>,
    /// 1 while it plays, 0 while it does not; shapes the middle button.
    playing: Spring,
    buttons: [Interaction; 3],
    _changes: Subscription,
}

impl Controls {
    fn new(player: Entity<Player>, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let playing = player.read(cx).now.as_ref().is_some_and(|now| now.playing);
        Self {
            _changes: cx.observe(&player, |_, _, cx| cx.notify()),
            player,
            playing: Spring::new(motion.spatial_fast, if playing { 1. } else { 0. }),
            buttons: [Interaction::new(&motion); 3],
        }
    }

    /// One round button. `emphasis` is 0 for a plain one and up to 1 for
    /// the middle one while it plays, which makes it wider and squarer.
    fn button(
        &mut self,
        glyph: &'static str,
        command: MediaCommand,
        enabled: bool,
        emphasis: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let slot = match command {
            MediaCommand::Previous => 0,
            MediaCommand::Toggle => 1,
            MediaCommand::Next => 2,
        };
        let state = self.buttons[slot].state(window);
        let (base, color, width, radius) = match emphasis {
            Some(playing) => (
                theme.colors.secondary_container,
                theme.colors.primary,
                TOGGLE_WIDTH,
                BUTTON_SIZE / 2. + (theme.shape.medium - BUTTON_SIZE / 2.) * playing,
            ),
            None => (
                theme.colors.surface_container_highest,
                theme.colors.on_surface,
                BUTTON_SIZE,
                BUTTON_SIZE / 2.,
            ),
        };
        // A press squeezes the button a little, as Material buttons do.
        let squeeze = px(4.) * state.press.clamp(0., 1.);
        let button = div()
            .id(("media-button", slot))
            .flex_none()
            .w(width - squeeze)
            .h(BUTTON_SIZE)
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius.max(px(0.)))
            .text_color(color)
            .bg(state_layer(&theme, base, state))
            .opacity(if enabled { 1. } else { DISABLED_OPACITY })
            .child(icon(&theme, glyph));
        let button = if enabled {
            button.on_click(cx.listener(move |this, _, _, cx| {
                this.player
                    .update(cx, |player, cx| player.send(command, cx));
            }))
        } else {
            button
        };
        Interaction::track(button, cx, move |this: &mut Self| {
            Some(&mut this.buttons[slot])
        })
        .into_any_element()
    }
}

impl Render for Controls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let Some(now) = self.player.read(cx).now.clone() else {
            return div()
                .text_color(theme.colors.on_surface_variant)
                .child("Nothing is playing");
        };
        self.playing.animate_to(if now.playing { 1. } else { 0. });
        let playing = self.playing.value(window).clamp(0., 1.);
        let toggle = if now.playing { ICON_PAUSE } else { ICON_PLAY };
        let title = if now.title.is_empty() {
            "Unnamed".to_owned()
        } else {
            now.title.clone()
        };
        let buttons = [
            self.button(
                ICON_PREVIOUS,
                MediaCommand::Previous,
                now.can_previous,
                None,
                window,
                cx,
            ),
            self.button(
                toggle,
                MediaCommand::Toggle,
                now.can_toggle,
                Some(playing),
                window,
                cx,
            ),
            self.button(
                ICON_NEXT,
                MediaCommand::Next,
                now.can_next,
                None,
                window,
                cx,
            ),
        ];
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(
                div()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(mix(theme.colors.on_surface, theme.colors.primary, playing))
                    .child(SharedString::from(title)),
            )
            .children((!now.artist.is_empty()).then(|| {
                div()
                    .truncate()
                    .text_color(theme.colors.on_surface_variant)
                    .child(SharedString::from(now.artist.clone()))
            }))
            .child(
                div()
                    .pt(theme.spacing.small)
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(theme.spacing.small)
                    .children(buttons),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing(title: &str, artist: &str) -> NowPlaying {
        NowPlaying {
            title: title.to_owned(),
            artist: artist.to_owned(),
            playing: true,
            can_toggle: true,
            can_previous: true,
            can_next: true,
        }
    }

    #[test]
    fn the_headline_names_what_is_known() {
        assert_eq!(headline(&playing("Song", "Band")), "Band \u{2013} Song");
        assert_eq!(headline(&playing("Song", "")), "Song");
        assert_eq!(headline(&playing("", "Band")), "Band");
        assert_eq!(headline(&playing("", "")), "Unnamed");
    }
}
