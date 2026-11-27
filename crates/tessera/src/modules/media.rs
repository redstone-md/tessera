// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What is playing. Widgets: `media`, a compact readout that is there only
//! while an application offers something, and `media-controls`, the title
//! with its picture, buttons to skip and pause, to shuffle and repeat where
//! the player lets them, the position, and a choice between the players
//! while several have something. Action: `media`, which passes a command to
//! the player chosen there, or to the one Windows considers current.

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

use crate::actions::{Action, Actions};
use crate::flyout::media_app_name;
use crate::i18n::Language;
use crate::icons::{Icon, image_of_bytes};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{Media, MediaCommand, NowPlaying, Position, Repeat, SessionInfo, SessionKey};
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
const ICON_SHUFFLE: &str = "\u{e8b1}";
const ICON_REPEAT_ALL: &str = "\u{e8ee}";
const ICON_REPEAT_ONE: &str = "\u{e8ed}";

/// The height of a chip that chooses a player, and the most it takes.
const CHIP_HEIGHT: Pixels = px(28.);
const CHIP_WIDTH: Pixels = px(120.);

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

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        let media = Action::new(|arg, cx| {
            if let Some(command) = command_named(arg) {
                send_to_chosen(command, cx);
            }
        })
        .checking(|arg| match command_named(arg) {
            Some(_) => Ok(()),
            None => Err(
                "`media` takes `toggle`, `play`, `pause`, `stop`, `next` or `previous`".to_owned(),
            ),
        });
        actions.register("media", media)
    }
}

/// The command the `arg` of the action `media` names.
fn command_named(arg: Option<&str>) -> Option<MediaCommand> {
    match arg? {
        "toggle" => Some(MediaCommand::Toggle),
        "play" => Some(MediaCommand::Play),
        "pause" => Some(MediaCommand::Pause),
        "stop" => Some(MediaCommand::Stop),
        "next" => Some(MediaCommand::Next),
        "previous" => Some(MediaCommand::Previous),
        _ => None,
    }
}

/// Passes `command` to the player chosen in the media widgets, or to the
/// one Windows considers current while none is chosen or none is shown.
fn send_to_chosen(command: MediaCommand, cx: &mut App) {
    match cx.try_global::<SharedPlayer>() {
        Some(shared) => {
            let player = shared.0.clone();
            player.update(cx, |player, cx| player.send(command, cx));
        }
        None => cx
            .background_executor()
            .spawn(async move {
                if let Err(error) = Media::send(None, command) {
                    report!("{error}");
                }
            })
            .detach(),
    }
}

/// What a chip that chooses `session` says: the name of its application,
/// or what it plays where the application is known only by a number, as
/// some browsers are.
fn chip_label(session: &SessionInfo) -> String {
    let name = media_app_name(&session.key.app);
    let number = name.len() >= 12 && name.chars().all(|letter| letter.is_ascii_hexdigit());
    if (number || name.is_empty()) && !session.title.is_empty() {
        return session.title.clone();
    }
    name.to_owned()
}

/// The title and the artist on one line, whichever of them there are.
fn headline(now: &NowPlaying, language: Language) -> String {
    match (now.title.is_empty(), now.artist.is_empty()) {
        (false, false) => format!("{} \u{2013} {}", now.artist, now.title),
        (false, true) => now.title.clone(),
        (true, false) => now.artist.clone(),
        (true, true) => language.say("Unnamed").to_owned(),
    }
}

/// What tells one piece from another: its player, its title and its
/// artist.
fn track_of(now: &NowPlaying) -> (String, String, String) {
    (now.app.clone(), now.title.clone(), now.artist.clone())
}

/// The picture of what is playing as it was read.
#[derive(Clone, Default)]
struct Cover {
    /// Tells one image file from another; 0 without one.
    digest: u64,
    image: Icon,
}

impl Cover {
    /// Reads and decodes the picture of what the session `key` plays; one
    /// that cannot be used is reported and counts as none.
    fn read(key: Option<&SessionKey>) -> Self {
        let bytes = Media::cover(key).and_then(|bytes| bytes.ok_or_else(String::new));
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
    /// Every session, read only while a choice between them is shown.
    sessions: Vec<SessionInfo>,
    /// The session chosen among them; `None` for the one Windows considers
    /// current.
    chosen: Option<SessionKey>,
    /// Held by each view that shows the choice between the sessions; the
    /// sessions are read while one does, and not otherwise, as each takes
    /// a few calls that wait for Windows.
    viewers: Rc<()>,
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
                    let asked = this.read_with(cx, |this, _| (this.chosen.clone(), this.viewed()));
                    let Ok((chosen, viewed)) = asked else {
                        break;
                    };
                    let asked = chosen.clone();
                    let read = cx.background_executor().spawn(async move {
                        let chosen = chosen.as_ref();
                        let now = Media::now_playing(chosen);
                        let playing = now.as_ref().ok().and_then(|now| now.as_ref());
                        let changed = playing.map(track_of) != known;
                        let cover = (changed || wanted).then(|| Cover::read(chosen));
                        let sessions = viewed.then(Media::sessions);
                        (now, cover, sessions)
                    });
                    let (now, read, sessions) = read.await;
                    // The same picture read again is not shown anew.
                    let fresh = read.filter(|read| read.digest != cover.digest);
                    let playing = now.as_ref().ok().map(|now| now.as_ref().map(track_of));
                    let taken = this.update(cx, |this, cx| {
                        if let Some(sessions) = sessions {
                            this.take_sessions(sessions, cx);
                        }
                        // Another session was chosen while this one was
                        // read: what it plays is of no use.
                        let current = this.chosen == asked;
                        if current {
                            let image = fresh.as_ref().map(|cover| cover.image.clone());
                            this.take(now, image, cx);
                        }
                        current
                    });
                    let Ok(current) = taken else {
                        break;
                    };
                    if current {
                        if let Some(playing) = playing {
                            if playing != track {
                                (track, reads) = (playing, COVER_READS);
                            } else {
                                reads = reads.saturating_sub(1);
                            }
                        }
                        if let Some(fresh) = fresh {
                            cover = fresh;
                        }
                    }
                    cx.background_executor().timer(POLL_INTERVAL).await;
                }
            })
            .detach();
            Self {
                now: None,
                sessions: Vec::new(),
                chosen: None,
                viewers: Rc::new(()),
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

    /// Whether a view shows the choice between the sessions.
    fn viewed(&self) -> bool {
        Rc::strong_count(&self.viewers) > 1
    }

    /// Has the sessions read from now on, until what this returns is
    /// dropped; the first view has them read at once.
    fn view(&mut self, cx: &mut Context<Self>) -> Rc<()> {
        if !self.viewed() {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let read = cx.background_executor().spawn(async { Media::sessions() });
                let read = read.await;
                this.update(cx, |this, cx| this.take_sessions(read, cx))
                    .ok();
            })
            .detach();
        }
        self.viewers.clone()
    }

    /// Takes over the sessions that were read; a chosen session that is
    /// gone is chosen no more.
    fn take_sessions(&mut self, read: Result<Vec<SessionInfo>, String>, cx: &mut Context<Self>) {
        // A failure is reported with what is playing, which fails too.
        let Ok(sessions) = read else {
            return;
        };
        if let Some(chosen) = &self.chosen
            && !sessions.iter().any(|session| &session.key == chosen)
        {
            self.chosen = None;
        }
        if sessions != self.sessions {
            self.sessions = sessions;
            cx.notify();
        }
    }

    /// Has the widgets show what the session `key` plays from now on, and
    /// its keys go to it.
    fn choose(&mut self, key: SessionKey, cx: &mut Context<Self>) {
        let current = self.sessions.iter().find(|session| session.current);
        // The current one is followed as Windows changes it.
        let chosen = (current.map(|session| &session.key) != Some(&key)).then_some(key);
        if chosen == self.chosen {
            return;
        }
        self.chosen = chosen.clone();
        cx.notify();
        // What it plays is read at once; its picture comes with the next
        // reading.
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let asked = chosen.clone();
            let read = cx
                .background_executor()
                .spawn(async move { Media::now_playing(asked.as_ref()) });
            let read = read.await;
            this.update(cx, |this, cx| {
                // Unless yet another one was chosen meanwhile.
                if this.chosen == chosen {
                    this.take(read, None, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Whether `session` is the one shown: the one chosen, or the one
    /// Windows considers current.
    fn shows(&self, session: &SessionInfo) -> bool {
        match &self.chosen {
            Some(chosen) => chosen == &session.key,
            None => session.current,
        }
    }

    /// Passes `command` on and reads what came of it.
    fn send(&mut self, command: MediaCommand, cx: &mut Context<Self>) {
        // The answer takes a moment; the button should not.
        if let Some(now) = &mut self.now {
            match command {
                MediaCommand::Toggle => now.playing = !now.playing,
                MediaCommand::Play => now.playing = true,
                MediaCommand::Pause | MediaCommand::Stop => now.playing = false,
                MediaCommand::Shuffle(on) => now.shuffle = Some(on),
                MediaCommand::Repeat(repeat) => now.repeat = Some(repeat),
                MediaCommand::Seek(seconds) => {
                    if let Some(position) = now.position.as_mut() {
                        position.elapsed = seconds.min(position.length);
                    }
                }
                MediaCommand::Previous | MediaCommand::Next => {}
            }
            cx.notify();
        }
        let key = self.chosen.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let sent = cx.background_executor().spawn(async move {
                if let Err(error) = Media::send(key.as_ref(), command) {
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
        let presence = if now.is_some() { 1. } else { 0. };
        // The position goes on every second and is not shown here: only
        // what is shown has the bar drawn again.
        let changed = self.presence.target() != presence
            || now.as_ref().is_some_and(|now| {
                !self
                    .shown
                    .as_ref()
                    .is_some_and(|shown| reads_as(shown, now))
            });
        if !changed {
            return;
        }
        self.presence.animate_to(presence);
        if now.is_some() {
            self.shown = now;
        }
        cx.notify();
    }
}

/// Whether the readout of a bar shows `one` and `other` alike.
fn reads_as(one: &NowPlaying, other: &NowPlaying) -> bool {
    one.playing == other.playing && one.title == other.title && one.artist == other.artist
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
                    .child(
                        div()
                            .min_w_0()
                            .child(one_line(headline(now, Language::of(cx)))),
                    ),
            )
    }
}

/// The title, the artist, buttons to skip back, pause or play, and skip
/// on, and to shuffle and repeat where the player lets them; over them a
/// chip for each player while there are several.
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
    buttons: [Interaction; 5],
    /// The chips of the players, one for each session.
    chips: Vec<Interaction>,
    /// Has the sessions read for the chips while this lives.
    _viewing: Rc<()>,
    _changes: Subscription,
}

impl Controls {
    fn new(player: Entity<Player>, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let playing = player.read(cx).now.as_ref().is_some_and(|now| now.playing);
        let cover = player.read(cx).cover.clone();
        let viewing = player.update(cx, Player::view);
        Self {
            _viewing: viewing,
            _changes: cx.observe(&player, |this: &mut Self, _, cx| this.follow(cx)),
            player,
            playing: Spring::new(motion.spatial_fast, if playing { 1. } else { 0. }),
            cover,
            cover_presence: Spring::new(motion.effects, 1.),
            position: Spring::new(motion.effects, 0.),
            line: Rc::default(),
            buttons: [Interaction::new(&motion); 5],
            chips: Vec::new(),
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

    /// A chip for each player, the one shown lit; a click shows another.
    /// Nothing while there is only one.
    fn chips(&mut self, theme: &Theme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let player = self.player.read(cx);
        let sessions = player.sessions.clone();
        let lit: Vec<bool> = sessions
            .iter()
            .map(|session| player.shows(session))
            .collect();
        if sessions.len() < 2 {
            return div().into_any_element();
        }
        self.chips
            .resize_with(sessions.len(), || Interaction::new(&theme.motion));
        let chips = sessions.into_iter().enumerate().map(|(index, session)| {
            let lit = lit[index];
            let state = self.chips[index].state(window);
            let (ground, text) = if lit {
                (theme.colors.secondary_container, theme.colors.on_surface)
            } else {
                (
                    theme.colors.surface_container_highest,
                    theme.colors.on_surface_variant,
                )
            };
            let glyph = if session.playing {
                ICON_NOTE
            } else {
                ICON_PAUSE
            };
            let chip = div()
                .id(("media-player", index))
                .h(CHIP_HEIGHT)
                .max_w(CHIP_WIDTH)
                .min_w_0()
                .flex_none()
                .px(theme.spacing.small)
                .flex()
                .items_center()
                .gap(theme.spacing.tiny)
                .rounded(theme.shape.full)
                .bg(state_layer(theme, ground, state))
                .text_color(text)
                .text_size(TIME_SIZE)
                .child(icon(theme, glyph).flex_none())
                .child(div().min_w_0().child(one_line(chip_label(&session))));
            let key = session.key;
            Interaction::track(chip, cx, move |this: &mut Self| this.chips.get_mut(index))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let key = key.clone();
                    this.player.update(cx, |player, cx| player.choose(key, cx));
                }))
                .into_any_element()
        });
        div()
            .id("media-players")
            .pb(theme.spacing.small)
            .flex()
            .gap(theme.spacing.tiny)
            .overflow_x_scroll()
            .children(chips.collect::<Vec<_>>())
            .into_any_element()
    }

    /// One round button. `emphasis` is 0 for a plain one and up to 1 for
    /// the middle one while it plays, which makes it wider and squarer;
    /// `lit` marks a shuffle or repeat that is on.
    #[allow(clippy::too_many_arguments, reason = "each says how the button looks")]
    fn button(
        &mut self,
        glyph: &'static str,
        command: MediaCommand,
        enabled: bool,
        emphasis: Option<f32>,
        lit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let slot = match command {
            MediaCommand::Shuffle(_) => 0,
            MediaCommand::Previous => 1,
            MediaCommand::Toggle | MediaCommand::Play | MediaCommand::Pause => 2,
            // Going to a place of the piece and stopping have no button.
            MediaCommand::Next | MediaCommand::Seek(_) | MediaCommand::Stop => 3,
            MediaCommand::Repeat(_) => 4,
        };
        let state = self.buttons[slot].state(window);
        let (base, color, width, radius) = match emphasis {
            Some(playing) => (
                theme.colors.secondary_container,
                theme.colors.primary,
                TOGGLE_WIDTH,
                BUTTON_SIZE / 2. + (theme.shape.medium - BUTTON_SIZE / 2.) * playing,
            ),
            None if lit => (
                theme.colors.secondary_container,
                theme.colors.primary,
                BUTTON_SIZE,
                BUTTON_SIZE / 2.,
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
        let language = Language::of(cx);
        let chips = self.chips(&theme, window, cx);
        let Some(now) = self.player.read(cx).now.clone() else {
            return div().flex().flex_col().child(chips).child(
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(language.say("Nothing is playing")),
            );
        };
        self.playing.animate_to(if now.playing { 1. } else { 0. });
        let playing = self.playing.value(window).clamp(0., 1.);
        let toggle = if now.playing { ICON_PAUSE } else { ICON_PLAY };
        let title = if now.title.is_empty() {
            language.say("Unnamed").to_owned()
        } else {
            now.title.clone()
        };
        // Shuffle and repeat are there only where the player tells them.
        let shuffle = now.shuffle.map(|on| {
            let command = MediaCommand::Shuffle(!on);
            self.button(ICON_SHUFFLE, command, now.can_shuffle, None, on, window, cx)
        });
        let repeat = now.repeat.map(|repeat| {
            let glyph = match repeat {
                Repeat::Track => ICON_REPEAT_ONE,
                Repeat::Off | Repeat::List => ICON_REPEAT_ALL,
            };
            let command = MediaCommand::Repeat(repeat.next());
            let on = repeat != Repeat::Off;
            self.button(glyph, command, now.can_repeat, None, on, window, cx)
        });
        let buttons = [
            shuffle,
            Some(self.button(
                ICON_PREVIOUS,
                MediaCommand::Previous,
                now.can_previous,
                None,
                false,
                window,
                cx,
            )),
            Some(self.button(
                toggle,
                MediaCommand::Toggle,
                now.can_toggle,
                Some(playing),
                false,
                window,
                cx,
            )),
            Some(self.button(
                ICON_NEXT,
                MediaCommand::Next,
                now.can_next,
                None,
                false,
                window,
                cx,
            )),
            repeat,
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
            .child(chips)
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
                    .children(buttons.into_iter().flatten()),
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
            app: "player.exe".to_owned(),
            title: title.to_owned(),
            artist: artist.to_owned(),
            playing: true,
            can_toggle: true,
            can_previous: true,
            can_next: true,
            can_seek: false,
            position: None,
            shuffle: None,
            can_shuffle: false,
            repeat: None,
            can_repeat: false,
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
        let headline = |now| headline(&now, Language::English);
        assert_eq!(headline(playing("Song", "Band")), "Band \u{2013} Song");
        assert_eq!(headline(playing("Song", "")), "Song");
        assert_eq!(headline(playing("", "Band")), "Band");
        assert_eq!(headline(playing("", "")), "Unnamed");
    }

    #[test]
    fn a_chip_names_the_application() {
        let session = |app: &str, title: &str| SessionInfo {
            key: SessionKey {
                app: app.to_owned(),
                occurrence: 0,
            },
            title: title.to_owned(),
            artist: String::new(),
            playing: true,
            current: false,
        };
        let label = |app, title| chip_label(&session(app, title));
        assert_eq!(label("Spotify.exe", "Song"), "Spotify");
        assert_eq!(label("chrome.EXE", "Song"), "chrome");
        assert_eq!(label(r"C:\Apps\vlc.exe", "Song"), "vlc");
        assert_eq!(
            label(
                "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic",
                "Song"
            ),
            "ZuneMusic"
        );
        // Known by a number: what it plays tells it.
        assert_eq!(label("308046B0AF4A39CB", "Video"), "Video");
        assert_eq!(label("308046B0AF4A39CB", ""), "308046B0AF4A39CB");
        // A name in letters of more than one byte each, whose last four
        // bytes lie inside a letter.
        assert_eq!(label("网易云音乐", "Song"), "网易云音乐");
        assert_eq!(label("网易云音乐.exe", "Song"), "网易云音乐");
    }

    #[test]
    fn the_session_shown_is_the_one_chosen_or_the_current_one() {
        let session = |occurrence, current| SessionInfo {
            key: SessionKey {
                app: "chrome.exe".to_owned(),
                occurrence,
            },
            title: String::new(),
            artist: String::new(),
            playing: true,
            current,
        };
        let (first, second) = (session(0, true), session(1, false));
        let mut player = Player {
            now: None,
            sessions: vec![first.clone(), second.clone()],
            chosen: None,
            viewers: Rc::new(()),
            cover: None,
            error: None,
        };
        assert!(player.shows(&first) && !player.shows(&second));
        // Two sessions of the same application are told apart.
        player.chosen = Some(second.key.clone());
        assert!(!player.shows(&first) && player.shows(&second));
    }

    #[test]
    fn the_media_action_names_its_command() {
        assert_eq!(command_named(Some("toggle")), Some(MediaCommand::Toggle));
        assert_eq!(command_named(Some("stop")), Some(MediaCommand::Stop));
        assert_eq!(command_named(Some("seek")), None);
        assert_eq!(command_named(None), None);
    }
}
