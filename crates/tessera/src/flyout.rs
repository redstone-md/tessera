// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The flyouts: a pill that shows for a moment what just changed, at a
//! side of what the bars leave of the display the pointer is on. Each kind
//! is a [`Gauge`]: the volume of the default output, the brightness of the
//! built-in panel, the mute of the microphone, Caps Lock, Num Lock and the
//! keyboard layout of the window in front.
//!
//! Tessera is not told of a change; it reads each gauge a few times a
//! second while its flyout is wanted, and not at all otherwise. A change
//! Tessera makes itself, as the action `volume` does, is shown at once
//! with [`Flyouts::show`].
//!
//! In shell mode, where no Explorer answers them, the keys of the keyboard
//! for sound and media are to be answered by the actions that do the same,
//! `volume` or `media`, whose flyouts show what came of them: the shell
//! host, which is told of the keys, hands each to [`Flyouts::answer`].

use std::time::Duration;

use gpui::{
    App, AsyncApp, Context, FontWeight, Global, IntoElement, ParentElement, Render, Styled, Task,
    WeakEntity, Window, WindowHandle, div, point, px, size,
};
use serde::Deserialize;

use crate::actions::Actions;
use crate::i18n::Language;
use crate::motion::Spring;
use crate::platform::{
    AppCommand, AudioOutput, InputLanguage, KeyLocks, NativeSurface, ScreenBrightness, ScreenRect,
    Typing, window_in_front,
};
use crate::popup::{MARGIN, Popups, open_floating, retire_floating};
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, one_line, wave_bar};

/// How often the volume and the microphone are read while their flyouts
/// are wanted.
const AUDIO_INTERVAL: Duration = Duration::from_millis(250);

/// How often the brightness of the panel is read: its keys change it a
/// step at a time, and the flyout follows them.
const BRIGHTNESS_INTERVAL: Duration = Duration::from_millis(500);

/// How often the keys and the layout are read.
const KEYS_INTERVAL: Duration = Duration::from_millis(250);

/// How often the flyout counts down the time it has left.
const TICK: Duration = Duration::from_millis(100);

/// How long the flyout stays after the last change.
const STAY: Duration = Duration::from_millis(1600);

/// The size of the pill, in logical pixels.
const WIDTH: f32 = 280.;
const HEIGHT: f32 = 48.;

/// How far from the side of the space the pill hangs.
const LIFT: f32 = 24.;

/// How far from its place the pill starts and ends.
const TRAVEL: f32 = 16.;

/// The room between the ends of the pill and what is in it.
const PADDING: f32 = 20.;

/// The room of the number beside the line.
const NUMBER_WIDTH: f32 = 40.;

pub const ICON_MUTED: &str = "\u{e74f}";
pub const ICON_LEVELS: [&str; 4] = ["\u{e992}", "\u{e993}", "\u{e994}", "\u{e995}"];
const ICON_BRIGHTNESS: &str = "\u{e706}";
const ICON_MICROPHONE: &str = "\u{e720}";
const ICON_MICROPHONE_OFF: &str = "\u{ec54}";
const ICON_KEYBOARD: &str = "\u{e765}";

/// The icon of a sound output at `volume`, from 0 to 1.
pub fn volume_icon(volume: f32, muted: bool) -> &'static str {
    if muted {
        return ICON_MUTED;
    }
    let step = (volume * 3.).ceil().clamp(0., 3.) as usize;
    ICON_LEVELS[step]
}

/// What the configuration says about flyouts: which kinds are shown, and
/// where.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlyoutConfig {
    /// Whether a change of the volume is shown.
    #[serde(default)]
    volume: bool,
    /// Whether a change of the brightness of the built-in panel is shown.
    #[serde(default)]
    brightness: bool,
    /// Whether the microphone being muted, or no longer, is shown.
    #[serde(default)]
    microphone: bool,
    /// Whether Caps Lock and Num Lock being switched are shown.
    #[serde(default)]
    locks: bool,
    /// Whether another keyboard layout for the window in front is shown.
    #[serde(default)]
    layout: bool,
    #[serde(default)]
    place: FlyoutPlace,
}

impl FlyoutConfig {
    /// Whether flyouts of `kind` are shown.
    fn wants(&self, kind: Kind) -> bool {
        match kind {
            Kind::Volume => self.volume,
            Kind::Brightness => self.brightness,
            Kind::Microphone => self.microphone,
            Kind::CapsLock | Kind::NumLock => self.locks,
            Kind::Layout => self.layout,
        }
    }
}

/// Where in what the bars leave of a display the flyout hangs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlyoutPlace {
    /// In the middle of the lower side, as the flyout of Windows 11.
    #[default]
    Bottom,
    Top,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl FlyoutPlace {
    fn at_top(self) -> bool {
        matches!(self, Self::Top | Self::TopLeft | Self::TopRight)
    }
}

/// The kinds of flyouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Volume,
    Brightness,
    Microphone,
    CapsLock,
    NumLock,
    Layout,
}

const KINDS: [Kind; 6] = [
    Kind::Volume,
    Kind::Brightness,
    Kind::Microphone,
    Kind::CapsLock,
    Kind::NumLock,
    Kind::Layout,
];

/// What a flyout shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Reading {
    /// The default output, at a volume from 0 to 1.
    Volume {
        level: f32,
        muted: bool,
    },
    /// The display, from 0 to 1.
    Brightness(f32),
    Microphone {
        muted: bool,
    },
    CapsLock(bool),
    NumLock(bool),
    /// The letters of the language the window in front is typed in.
    Layout(String),
}

impl Reading {
    fn kind(&self) -> Kind {
        match self {
            Self::Volume { .. } => Kind::Volume,
            Self::Brightness(_) => Kind::Brightness,
            Self::Microphone { .. } => Kind::Microphone,
            Self::CapsLock(_) => Kind::CapsLock,
            Self::NumLock(_) => Kind::NumLock,
            Self::Layout(_) => Kind::Layout,
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            Self::Volume { level, muted } => volume_icon(*level, *muted),
            Self::Brightness(_) => ICON_BRIGHTNESS,
            Self::Microphone { muted: true } => ICON_MICROPHONE_OFF,
            Self::Microphone { muted: false } => ICON_MICROPHONE,
            Self::CapsLock(_) | Self::NumLock(_) | Self::Layout(_) => ICON_KEYBOARD,
        }
    }

    /// The share the wavy line shows, for what has a level.
    fn share(&self) -> Option<f32> {
        match self {
            Self::Volume { level, .. } => Some(*level),
            Self::Brightness(level) => Some(*level),
            _ => None,
        }
    }

    /// Whether it is shown in the colors of what is switched off.
    fn dimmed(&self) -> bool {
        match self {
            Self::Volume { muted, .. } | Self::Microphone { muted } => *muted,
            Self::CapsLock(on) | Self::NumLock(on) => !on,
            Self::Brightness(_) | Self::Layout(_) => false,
        }
    }

    /// The words shown for what has no level.
    fn words(&self, language: Language) -> String {
        let said = match self {
            Self::Microphone { muted: true } => "Microphone off",
            Self::Microphone { muted: false } => "Microphone on",
            Self::CapsLock(true) => "Caps Lock on",
            Self::CapsLock(false) => "Caps Lock off",
            Self::NumLock(true) => "Num Lock on",
            Self::NumLock(false) => "Num Lock off",
            Self::Layout(letters) => return letters.clone(),
            Self::Volume { level, .. } | Self::Brightness(level) => {
                return format!("{:.0}", level * 100.);
            }
        };
        language.say(said).to_owned()
    }
}

/// Something a flyout shows when it changes. It is read on a thread that
/// may wait, every [`Gauge::EVERY`], while its flyout is wanted.
trait Gauge: Default + Send + 'static {
    /// How often it is read.
    const EVERY: Duration;

    /// Reads it, and gives what to show if it changed since the reading
    /// before. The first reading is no change, and nothing to read is
    /// none.
    fn next(&mut self) -> Option<Reading>;
}

/// `now`, when it differs from what `last` held, which it then holds; the
/// first value and the going and coming of a value are no change.
fn changed<T: Clone + PartialEq>(last: &mut Option<T>, now: Option<T>) -> Option<T> {
    let before = std::mem::replace(last, now.clone());
    match (before, now) {
        (Some(before), Some(now)) if before != now => Some(now),
        _ => None,
    }
}

/// The volume and the mute of the default output.
#[derive(Default)]
struct VolumeGauge(Option<(u32, bool)>);

impl Gauge for VolumeGauge {
    const EVERY: Duration = AUDIO_INTERVAL;

    fn next(&mut self) -> Option<Reading> {
        let read = AudioOutput::default_device().ok().and_then(|output| {
            let level = output.volume().ok()?;
            Some((level, output.is_muted().ok()?))
        });
        // Compared as bits, so that a float is compared exactly.
        let now = read.map(|(level, muted)| (level.to_bits(), muted));
        let (level, muted) = changed(&mut self.0, now)?;
        Some(Reading::Volume {
            level: f32::from_bits(level),
            muted,
        })
    }
}

/// The brightness of the built-in panel, which its keys change without a
/// word. A monitor is not watched: asking one over DDC/CI takes long, and
/// it has no keys Windows sees.
#[derive(Default)]
struct BrightnessGauge(Option<u32>);

impl Gauge for BrightnessGauge {
    const EVERY: Duration = BRIGHTNESS_INTERVAL;

    fn next(&mut self) -> Option<Reading> {
        let now = ScreenBrightness::panel().map(f32::to_bits);
        changed(&mut self.0, now).map(|share| Reading::Brightness(f32::from_bits(share)))
    }
}

/// The mute of the default microphone.
#[derive(Default)]
struct MicrophoneGauge(Option<bool>);

impl Gauge for MicrophoneGauge {
    const EVERY: Duration = AUDIO_INTERVAL;

    fn next(&mut self) -> Option<Reading> {
        let now = AudioOutput::default_input()
            .ok()
            .and_then(|input| input.is_muted().ok());
        changed(&mut self.0, now).map(|muted| Reading::Microphone { muted })
    }
}

#[derive(Default)]
struct CapsLockGauge(Option<bool>);

impl Gauge for CapsLockGauge {
    const EVERY: Duration = KEYS_INTERVAL;

    fn next(&mut self) -> Option<Reading> {
        changed(&mut self.0, Some(KeyLocks::read().caps)).map(Reading::CapsLock)
    }
}

#[derive(Default)]
struct NumLockGauge(Option<bool>);

impl Gauge for NumLockGauge {
    const EVERY: Duration = KEYS_INTERVAL;

    fn next(&mut self) -> Option<Reading> {
        changed(&mut self.0, Some(KeyLocks::read().num)).map(Reading::NumLock)
    }
}

/// The layout the window in front is typed in, with that window. The
/// layout itself is compared, so a switch between two layouts of the same
/// language shows too; its letters are asked for only then.
#[derive(Default)]
struct LayoutGauge(Option<(isize, InputLanguage)>);

impl Gauge for LayoutGauge {
    const EVERY: Duration = KEYS_INTERVAL;

    fn next(&mut self) -> Option<Reading> {
        let window = window_in_front();
        let now = Typing::in_front().map(|typing| (window, typing.language));
        let before = std::mem::replace(&mut self.0, now);
        layout_switched(before, now).map(|layout| Reading::Layout(layout.code()))
    }
}

/// The new layout, when the window in front was switched to another one.
/// Going to another window that is typed in another layout is no switch,
/// and neither is a window of Tessera in front, which tells no layout.
fn layout_switched<L: PartialEq>(before: Option<(isize, L)>, now: Option<(isize, L)>) -> Option<L> {
    let ((window, layout), (now_window, now_layout)) = (before?, now?);
    (window == now_window && layout != now_layout).then_some(now_layout)
}

/// The flyout, what looks out for changes to show, and what answers the
/// keys of the keyboard for sound and media in shell mode.
#[derive(Default)]
pub struct Flyouts {
    config: FlyoutConfig,
    open: Option<WindowHandle<FlyoutView>>,
    /// A task for each kind that is wanted, which reads its gauge.
    watches: Vec<(Kind, Task<()>)>,
}

impl Global for Flyouts {}

impl Flyouts {
    /// Takes over what the configuration says: each gauge is read while
    /// its flyout is wanted.
    pub fn configure(config: &FlyoutConfig, cx: &mut App) {
        let this = cx.default_global::<Self>();
        this.config = config.clone();
        this.watches.retain(|(kind, _)| config.wants(*kind));
        for kind in KINDS {
            let this = cx.global::<Self>();
            let watched = this.watches.iter().any(|(watched, _)| *watched == kind);
            if !config.wants(kind) || watched {
                continue;
            }
            let watch = match kind {
                Kind::Volume => watch::<VolumeGauge>(cx),
                Kind::Brightness => watch::<BrightnessGauge>(cx),
                Kind::Microphone => watch::<MicrophoneGauge>(cx),
                Kind::CapsLock => watch::<CapsLockGauge>(cx),
                Kind::NumLock => watch::<NumLockGauge>(cx),
                Kind::Layout => watch::<LayoutGauge>(cx),
            };
            cx.global_mut::<Self>().watches.push((kind, watch));
        }
    }

    /// Does what Explorer would for the key `command` of the keyboard: runs
    /// the action that does the same, whose flyout shows what came of it.
    /// For shell mode, where the shell host takes the keys and no
    /// Explorer answers them ([`ShellEvent::Key`](crate::platform::ShellEvent)).
    pub fn answer(command: AppCommand, cx: &mut App) {
        let (action, arg) = action_of(command);
        Actions::run(action, Some(arg), cx);
    }

    /// Shows `reading`, if its kind is wanted and no popup is open, whose
    /// own controls show it.
    pub fn show(reading: Reading, cx: &mut App) {
        let Some(this) = cx.try_global::<Self>() else {
            return;
        };
        if !this.config.wants(reading.kind()) || Popups::is_open(cx) {
            return;
        }
        if let Some(open) = this.open {
            let shown = open.update(cx, |view, _, cx| view.show(reading.clone(), cx));
            if matches!(shown, Ok(true)) {
                return;
            }
        }
        Self::open(reading, cx);
    }

    /// Opens the window of the flyout with `reading` in it.
    fn open(reading: Reading, cx: &mut App) {
        let Some(anchor) = Shell::middle_anchor(cx) else {
            return;
        };
        let place = cx.global::<Self>().config.place;
        let area = anchor.display.and_then(|display| display.work_area());
        let area = area.unwrap_or(anchor.screen);
        let frame = frame(area, anchor.scale, place);
        let rough = gpui::Bounds {
            origin: point(
                px(frame.left as f32 / anchor.scale),
                px(frame.top as f32 / anchor.scale),
            ),
            size: size(px(WIDTH), px(HEIGHT)),
        };
        let opened = open_floating(rough, anchor.display_id, false, cx, |window, cx| {
            let adopted = NativeSurface::of(window).and_then(|native| {
                native.adopt_as_popup()?;
                native.place(frame)?;
                native.reveal(false);
                Ok(())
            });
            if let Err(error) = adopted {
                report!("the flyout is not where it belongs: {error:#}");
            }
            FlyoutView::new(reading, place, cx)
        });
        match opened {
            Ok(window) => cx.global_mut::<Self>().open = Some(window),
            Err(error) => report!("the flyout did not open: {error:#}"),
        }
    }
}

/// Reads the gauge `G` every [`Gauge::EVERY`] and shows what changed,
/// until the task is dropped.
fn watch<G: Gauge>(cx: &mut App) -> Task<()> {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut gauge = G::default();
        loop {
            // Asking Windows takes a while, and nothing is drawn meanwhile
            // if the thread of the windows asks.
            let read = cx.background_executor().spawn(async move {
                let shown = gauge.next();
                (gauge, shown)
            });
            let (read, shown) = read.await;
            gauge = read;
            if let Some(shown) = shown
                && cx.update(|cx| Flyouts::show(shown, cx)).is_err()
            {
                break;
            }
            cx.background_executor().timer(G::EVERY).await;
        }
    })
}

/// The action, and its `arg`, that does what the key `command` says.
fn action_of(command: AppCommand) -> (&'static str, &'static str) {
    match command {
        AppCommand::VolumeUp => ("volume", "up"),
        AppCommand::VolumeDown => ("volume", "down"),
        AppCommand::VolumeMute => ("volume", "mute"),
        AppCommand::MicrophoneMute => ("volume", "mute-microphone"),
        AppCommand::PlayPause => ("media", "toggle"),
        AppCommand::Play => ("media", "play"),
        AppCommand::Pause => ("media", "pause"),
        AppCommand::Stop => ("media", "stop"),
        AppCommand::Next => ("media", "next"),
        AppCommand::Previous => ("media", "previous"),
    }
}

/// The frame of the window of the flyout in the space `area`, in physical
/// pixels, of which there are `scale` in a logical one: at `place`, a
/// little away from the sides.
fn frame(area: ScreenRect, scale: f32, place: FlyoutPlace) -> ScreenRect {
    let physical = |length: f32| (length * scale).round() as i32;
    let width = physical(WIDTH + MARGIN * 2.);
    let height = physical(HEIGHT + MARGIN * 2. + TRAVEL);
    let inset = physical(LIFT - MARGIN);
    let left = match place {
        FlyoutPlace::Bottom | FlyoutPlace::Top => (area.left + area.right - width) / 2,
        FlyoutPlace::TopLeft | FlyoutPlace::BottomLeft => area.left + inset,
        FlyoutPlace::TopRight | FlyoutPlace::BottomRight => area.right - inset - width,
    };
    let top = if place.at_top() {
        area.top + inset
    } else {
        area.bottom - inset - height
    };
    ScreenRect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// The root view of the window of the flyout.
pub struct FlyoutView {
    reading: Reading,
    /// Where it hangs, which says from where it comes in.
    place: FlyoutPlace,
    /// The length of the line, which follows the level.
    line: Spring,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    /// How long it still stays.
    left: Duration,
    /// The window was put away and shows nothing more.
    gone: bool,
}

impl FlyoutView {
    fn new(reading: Reading, place: FlyoutPlace, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(TICK).await;
                if !matches!(this.update(cx, |this, cx| this.tick(cx)), Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        let motion = cx.global::<Theme>().motion;
        let mut presence = Spring::new(motion.spatial, 0.);
        presence.animate_to(1.);
        Self {
            line: Spring::new(motion.spatial, reading.share().unwrap_or(0.)),
            reading,
            place,
            presence,
            left: STAY,
            gone: false,
        }
    }

    /// Shows `reading` and stays on. `false` when the window was put away
    /// and another one is needed.
    fn show(&mut self, reading: Reading, cx: &mut Context<Self>) -> bool {
        if self.gone {
            return false;
        }
        match reading.share() {
            // From one kind to another the line starts anew.
            Some(share) if reading.kind() == self.reading.kind() => self.line.animate_to(share),
            Some(share) => self.line.jump_to(share),
            None => {}
        }
        self.reading = reading;
        self.presence.animate_to(1.);
        self.left = STAY;
        cx.notify();
        true
    }

    /// Counts down the time the flyout has left. `false` once the window
    /// was put away.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        if self.gone {
            return false;
        }
        if !self.left.is_zero() {
            self.left = self.left.saturating_sub(TICK);
            if self.left.is_zero() {
                self.presence.animate_to(0.);
                cx.notify();
            }
        }
        true
    }
}

impl Render for FlyoutView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let presence = self.presence.value(window);
        let line = self.line.value(window).clamp(0., 1.);
        if self.left.is_zero() && self.presence.is_settled() && !self.gone {
            self.gone = true;
            retire_floating(window, cx);
        }
        let reading = &self.reading;
        let colors = theme.colors;
        let shown = match reading.dimmed() {
            true => colors.on_surface_variant,
            false => colors.on_surface,
        };
        // It comes in from the side it hangs at.
        let away = TRAVEL * (1. - presence);
        let (top, pad) = if self.place.at_top() {
            (-away, TRAVEL)
        } else {
            (away, 0.)
        };
        let pill = div()
            .relative()
            .top(px(top))
            .w(px(WIDTH))
            .h(px(HEIGHT))
            .px(px(PADDING))
            .gap(theme.spacing.medium)
            .flex()
            .items_center()
            .rounded(theme.shape.full)
            .bg(colors.surface_container_highest)
            .text_color(shown)
            .child(icon(&theme, reading.icon()));
        let words = reading.words(language);
        let pill = match reading.share() {
            Some(_) => pill
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(wave_bar(&theme, if reading.dimmed() { 0. } else { line })),
                )
                .child(
                    div()
                        .w(px(NUMBER_WIDTH))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .font_weight(FontWeight::MEDIUM)
                        .child(words),
                ),
            None => pill.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .child(one_line(words)),
            ),
        };
        div()
            .size_full()
            .p(px(MARGIN))
            .pt(px(MARGIN + pad))
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .opacity(presence.clamp(0., 1.))
            .child(pill)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: ScreenRect = ScreenRect {
        left: 100,
        top: 0,
        right: 1100,
        bottom: 800,
    };

    #[test]
    fn the_flyout_hangs_in_the_middle_above_the_lower_side() {
        let frame = frame(AREA, 1., FlyoutPlace::Bottom);
        assert_eq!(frame.right - frame.left, 304);
        assert_eq!(frame.left - AREA.left, AREA.right - frame.right);
        assert_eq!(frame.bottom, 788);
        assert_eq!(frame.bottom - frame.top, 88);
        let doubled = super::frame(AREA, 2., FlyoutPlace::Bottom);
        assert_eq!(doubled.right - doubled.left, 608);
    }

    #[test]
    fn the_flyout_hangs_where_it_is_placed() {
        let top = frame(AREA, 1., FlyoutPlace::Top);
        assert_eq!((top.top, top.bottom), (12, 100));
        assert_eq!(top.left - AREA.left, AREA.right - top.right);
        let corner = frame(AREA, 1., FlyoutPlace::TopRight);
        assert_eq!((corner.right, corner.top), (1088, 12));
        let corner = frame(AREA, 1., FlyoutPlace::BottomLeft);
        assert_eq!((corner.left, corner.bottom), (112, 788));
        let corner = frame(AREA, 1., FlyoutPlace::BottomRight);
        assert_eq!((corner.right, corner.bottom), (1088, 788));
        let corner = frame(AREA, 2., FlyoutPlace::TopLeft);
        assert_eq!((corner.left, corner.top), (124, 24));
    }

    #[test]
    fn the_place_is_written_in_words() {
        #[derive(Deserialize)]
        struct Written {
            place: FlyoutPlace,
        }
        let read = |text: &str| toml::from_str::<Written>(text).map(|written| written.place);
        assert_eq!(
            read("place = \"top-right\"").ok(),
            Some(FlyoutPlace::TopRight)
        );
        assert_eq!(read("place = \"bottom\"").ok(), Some(FlyoutPlace::Bottom));
        assert!(read("place = \"middle\"").is_err());
        let config: FlyoutConfig = toml::from_str("volume = true\nlocks = true").unwrap();
        assert!(config.wants(Kind::Volume) && config.wants(Kind::NumLock));
        assert!(!config.wants(Kind::Layout));
        assert_eq!(config.place, FlyoutPlace::Bottom);
    }

    #[test]
    fn the_icon_follows_the_volume() {
        assert_eq!(volume_icon(0.5, true), ICON_MUTED);
        assert_eq!(volume_icon(0., false), ICON_LEVELS[0]);
        assert_eq!(volume_icon(1., false), ICON_LEVELS[3]);
    }

    #[test]
    fn only_a_change_is_shown() {
        let mut last = None;
        assert_eq!(changed(&mut last, Some(1)), None);
        assert_eq!(changed(&mut last, Some(1)), None);
        assert_eq!(changed(&mut last, Some(2)), Some(2));
        // A gauge that cannot be read for a while, as the volume while no
        // output is plugged in, shows nothing when it comes back.
        assert_eq!(changed(&mut last, None), None);
        assert_eq!(changed(&mut last, Some(3)), None);
        assert_eq!(changed(&mut last, Some(4)), Some(4));
    }

    #[test]
    fn a_layout_is_switched_only_in_the_same_window() {
        // Layouts by their number, as Windows tells them apart.
        let typing = |window, layout: isize| Some((window, layout));
        let (english, dvorak, russian) = (0x0409_0409, -0x0fe5_fbf7, 0x0419_0419);
        assert_eq!(
            layout_switched(typing(1, english), typing(1, russian)),
            Some(russian)
        );
        // Two layouts of the same language are a switch too.
        assert_eq!(
            layout_switched(typing(1, english), typing(1, dvorak)),
            Some(dvorak)
        );
        assert_eq!(
            layout_switched(typing(1, english), typing(2, russian)),
            None
        );
        assert_eq!(
            layout_switched(typing(1, english), typing(1, english)),
            None
        );
        assert_eq!(layout_switched(None, typing(1, russian)), None);
    }

    #[test]
    fn a_reading_shows_a_level_or_words() {
        let volume = Reading::Volume {
            level: 0.42,
            muted: true,
        };
        assert_eq!(volume.share(), Some(0.42));
        assert!(volume.dimmed());
        assert_eq!(volume.words(Language::English), "42");
        assert_eq!(volume.icon(), ICON_MUTED);
        let caps = Reading::CapsLock(true);
        assert_eq!(caps.share(), None);
        assert_eq!(caps.words(Language::English), "Caps Lock on");
        assert!(Reading::NumLock(false).dimmed());
        assert_eq!(
            Reading::Layout("DE".to_owned()).words(Language::English),
            "DE"
        );
        assert_eq!(Reading::Brightness(0.7).kind(), Kind::Brightness);
    }

    #[test]
    fn a_key_runs_the_action_that_does_the_same() {
        assert_eq!(action_of(AppCommand::VolumeUp), ("volume", "up"));
        assert_eq!(
            action_of(AppCommand::MicrophoneMute),
            ("volume", "mute-microphone")
        );
        assert_eq!(action_of(AppCommand::PlayPause), ("media", "toggle"));
    }
}
