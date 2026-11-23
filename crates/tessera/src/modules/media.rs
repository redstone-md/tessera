// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What is playing. Widgets: `media`, a compact readout that is there only
//! while an application offers something, and `media-controls`, the title
//! with its picture and buttons to skip and pause.

use std::cell::Cell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Bounds, Context, CursorStyle, Entity, FontWeight,
    Global, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ObjectFit, ParentElement,
    Pixels, Render, SharedString, StatefulInteractiveElement, Styled, StyledImage, Subscription,
    WeakEntity, Window, canvas, div, img, px,
};

use crate::icons::{Icon, image_of_bytes};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{Media, MediaCommand, NowPlaying, Position};
use crate::theme::Theme;
use crate::ui::{icon, one_line, pill, state_layer, wave_bar};

/// How often it is read what is playing.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How many more times the picture is read after what is playing changed:
/// applications tend to offer it a moment after the title.
const COVER_READS: u8 = 4;

/// The size the picture is shown at, and the pixels it is prepared with,
/// enough for 200% scaling.
const COVER_SIZE: Pixels = px(44.);
const COVER_SOURCE: u32 = 96;

/// The size of the times beside the line that shows the position.
const TIME_SIZE: Pixels = px(11.);

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
        widgets.register_panel("media-controls", |_, cx| {
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

/// What tells one piece from another: its title and its artist.
fn track_of(now: &NowPlaying) -> (String, String) {
    (now.title.clone(), now.artist.clone())
}

/// The picture of what is playing as it was read.
#[derive(Clone, Default)]
struct Cover {
    /// Tells one image file from another; 0 without one.
    digest: u64,
    image: Icon,
}

impl Cover {
    /// Reads and decodes the picture; one that cannot be used is reported
    /// and counts as none.
    fn read() -> Self {
        let bytes = Media::cover().and_then(|bytes| bytes.ok_or_else(String::new));
        let image = bytes.and_then(|bytes| {
            let image = image_of_bytes(&bytes, COVER_SOURCE);
            let image = image.map_err(|error| format!("the picture is not an image: {error}"))?;
            Ok((digest(&bytes), image))
        });
        match image {
            Ok((digest, image)) => Self { digest, image },
            Err(error) => {
                if !error.is_empty() {
                    report!("{error}");
                }
                Self::default()
            }
        }
    }
}

/// A number that differs between different `bytes`, and is never 0.
fn digest(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish().max(1)
}

/// What is playing, shared by every media widget.
struct Player {
    now: Option<NowPlaying>,
    /// The picture of what is playing, if it has one.
    cover: Icon,
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
                let mut track = None;
                let mut cover = Cover::default();
                // How many more readings look at the picture.
                let mut reads = 0;
                loop {
                    let (known, wanted) = (track.clone(), reads > 0);
                    let read = cx.background_executor().spawn(async move {
                        let now = Media::now_playing();
                        let playing = now.as_ref().ok().and_then(|now| now.as_ref());
                        let changed = playing.map(track_of) != known;
                        let cover = (changed || wanted).then(Cover::read);
                        (now, cover)
                    });
                    let (now, read) = read.await;
                    if let Ok(now) = &now {
                        let playing = now.as_ref().map(track_of);
                        if playing != track {
                            (track, reads) = (playing, COVER_READS);
                        } else {
                            reads = reads.saturating_sub(1);
                        }
                    }
                    // The same picture read again is not shown anew.
                    let fresh = read.filter(|read| read.digest != cover.digest);
                    if let Some(fresh) = &fresh {
                        cover = fresh.clone();
                    }
                    let fresh = fresh.map(|cover| cover.image);
                    let taken = this.update(cx, |this, cx| this.take(now, fresh, cx));
                    if taken.is_err() {
                        break;
                    }
                    cx.background_executor().timer(POLL_INTERVAL).await;
                }
            })
            .detach();
            Self {
                now: None,
                cover: None,
                error: None,
            }
        });
        cx.set_global(SharedPlayer(player.clone()));
        player
    }

    /// Takes over what was read, and `cover` if the picture changed.
    fn take(
        &mut self,
        read: Result<Option<NowPlaying>, String>,
        cover: Option<Icon>,
        cx: &mut Context<Self>,
    ) {
        if let Some(cover) = cover {
            self.cover = cover;
            cx.notify();
        }
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
                    report!("{error}");
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
        if let MediaCommand::Seek(seconds) = command
            && let Some(position) = self.now.as_mut().and_then(|now| now.position.as_mut())
        {
            position.elapsed = seconds.min(position.length);
            cx.notify();
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let sent = cx.background_executor().spawn(async move {
                if let Err(error) = Media::send(command) {
                    report!("{error}");
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
                // The room is kept by the element around: a pill with a
                // limit of its own does not pass it on to its text.
                pill(&theme)
                    .font_weight(FontWeight::MEDIUM)
                    .child(icon(&theme, glyph).flex_none().text_color(color))
                    .child(div().min_w_0().child(one_line(headline(now)))),
            )
    }
}

/// The title, the artist, and buttons to skip back, pause or play, and
/// skip on.
struct Controls {
    player: Entity<Player>,
    /// 1 while it plays, 0 while it does not; shapes the middle button.
    playing: Spring,
    /// The picture shown, and how much of it: it fades in when it changes.
    cover: Icon,
    cover_presence: Spring,
    /// How far into the piece it is, from 0 to 1.
    position: Spring,
    /// Where the line of the position was last drawn.
    line: Rc<Cell<Bounds<Pixels>>>,
    buttons: [Interaction; 3],
    _changes: Subscription,
}

impl Controls {
    fn new(player: Entity<Player>, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let playing = player.read(cx).now.as_ref().is_some_and(|now| now.playing);
        let cover = player.read(cx).cover.clone();
        Self {
            _changes: cx.observe(&player, |this: &mut Self, _, cx| this.follow(cx)),
            player,
            playing: Spring::new(motion.spatial_fast, if playing { 1. } else { 0. }),
            cover,
            cover_presence: Spring::new(motion.effects, 1.),
            position: Spring::new(motion.effects, 0.),
            line: Rc::default(),
            buttons: [Interaction::new(&motion); 3],
        }
    }

    /// Takes over the picture if it changed, which then fades in.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let cover = self.player.read(cx).cover.clone();
        if !same_image(&cover, &self.cover) {
            self.cover = cover;
            self.cover_presence.jump_to(0.);
            self.cover_presence.animate_to(1.);
        }
        cx.notify();
    }

    /// How far into the piece it is: the times and a line between them.
    /// Where the player lets the position be set, a press on the line goes
    /// to that place of the piece.
    fn timeline(
        &mut self,
        position: Position,
        seeks: bool,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let level = position.elapsed as f32 / position.length as f32;
        self.position.animate_to(level);
        let level = self.position.value(window).clamp(0., 1.);
        let time = |seconds| {
            div()
                .flex_none()
                .text_size(TIME_SIZE)
                .text_color(theme.colors.on_surface_variant)
                .child(SharedString::from(clock_time(seconds)))
        };
        div()
            .pt(theme.spacing.small)
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(time(position.elapsed))
            .child(self.line(position.length, seeks, level, theme, cx))
            .child(time(position.length))
    }

    /// The line of the position, which takes a press where `seeks`.
    fn line(
        &self,
        length: u32,
        seeks: bool,
        level: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let drawn = self.line.clone();
        let line = div()
            .flex_1()
            .relative()
            .child(wave_bar(theme, level))
            .child(
                canvas(move |bounds, _, _| drawn.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            );
        if !seeks {
            return line;
        }
        line.cursor(CursorStyle::PointingHand).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                let line = this.line.get();
                let to = second_at(
                    f32::from(event.position.x),
                    f32::from(line.origin.x),
                    f32::from(line.size.width),
                    length,
                );
                this.player
                    .update(cx, |player, cx| player.send(MediaCommand::Seek(to), cx));
            }),
        )
    }

    /// The picture, or a note in its place while there is none.
    fn picture(&mut self, theme: &Theme, window: &mut Window) -> AnyElement {
        let presence = self.cover_presence.value(window).clamp(0., 1.);
        let frame = div()
            .size(COVER_SIZE)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.small)
            .overflow_hidden()
            .bg(theme.colors.surface_container_highest)
            .text_color(theme.colors.on_surface_variant);
        match &self.cover {
            Some(cover) => frame.child(
                img(cover.clone())
                    .size_full()
                    .rounded(theme.shape.small)
                    .object_fit(ObjectFit::Cover)
                    .opacity(presence),
            ),
            None => frame.child(icon(theme, ICON_NOTE).opacity(presence)),
        }
        .into_any_element()
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
            // Going to a place of the piece has no button.
            MediaCommand::Next | MediaCommand::Seek(_) => 2,
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
        let names = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(
                div()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(mix(theme.colors.on_surface, theme.colors.primary, playing))
                    .child(one_line(title)),
            )
            .children((!now.artist.is_empty()).then(|| {
                div()
                    .min_w_0()
                    .text_color(theme.colors.on_surface_variant)
                    .child(one_line(now.artist.clone()))
            }));
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.medium)
                    .child(self.picture(&theme, window))
                    .child(names),
            )
            .children(
                now.position
                    .map(|position| self.timeline(position, now.can_seek, &theme, window, cx)),
            )
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

/// The second of a piece `length` seconds long that lies under a pointer
/// at `x`, on a line that starts at `left` and is `width` wide.
fn second_at(x: f32, left: f32, width: f32, length: u32) -> u32 {
    if width <= 0. {
        return 0;
    }
    let share = ((x - left) / width).clamp(0., 1.);
    (share * length as f32).round() as u32
}

/// `seconds` as a player shows them: minutes and seconds, with hours in
/// front once there are any.
fn clock_time(seconds: u32) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Whether both are the same picture, or both none.
fn same_image(one: &Icon, other: &Icon) -> bool {
    match (one, other) {
        (Some(one), Some(other)) => std::sync::Arc::ptr_eq(one, other),
        (None, None) => true,
        _ => false,
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
            can_seek: false,
            position: None,
        }
    }

    #[test]
    fn a_press_on_the_line_names_a_second_of_the_piece() {
        assert_eq!(second_at(100., 100., 200., 240), 0);
        assert_eq!(second_at(200., 100., 200., 240), 120);
        assert_eq!(second_at(300., 100., 200., 240), 240);
        // Beyond the ends is the first and the last second.
        assert_eq!(second_at(50., 100., 200., 240), 0);
        assert_eq!(second_at(900., 100., 200., 240), 240);
        assert_eq!(second_at(10., 0., 0., 240), 0);
    }

    #[test]
    fn a_time_shows_hours_only_when_there_are_any() {
        assert_eq!(clock_time(0), "0:00");
        assert_eq!(clock_time(65), "1:05");
        assert_eq!(clock_time(3600 + 125), "1:02:05");
    }

    #[test]
    fn different_pictures_have_different_digests_and_none_has_zero() {
        assert_ne!(digest(b"one"), digest(b"other"));
        assert_eq!(digest(b"one"), digest(b"one"));
        assert_ne!(digest(b""), 0);
    }

    #[test]
    fn the_headline_names_what_is_known() {
        assert_eq!(headline(&playing("Song", "Band")), "Band \u{2013} Song");
        assert_eq!(headline(&playing("Song", "")), "Song");
        assert_eq!(headline(&playing("", "Band")), "Band");
        assert_eq!(headline(&playing("", "")), "Unnamed");
    }
}
