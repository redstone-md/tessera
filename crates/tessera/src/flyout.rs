// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The flyouts: a pill that shows for a moment what just changed, at a
//! side of what the bars leave of the display the pointer is on. Each kind
//! is a [`Gauge`]: the volume of the default output, the brightness of the
//! built-in panel, the mute of the microphone, Caps Lock, Num Lock, the
//! keyboard layout of the window in front and what is playing; the
//! workspace shown is watched on its own, since Tessera's own workspaces
//! are kept on the thread of the windows, while the virtual desktops of
//! Windows are read from the registry on the background executor. While
//! Win+Space is held, the action
//! `input-language` has the installed layouts shown as a list, the one
//! switched to lit ([`Flyouts::show_layouts`]).
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

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AsyncApp, Context, FontWeight, Global, IntoElement, ObjectFit, ParentElement,
    Render, RenderImage, Styled, StyledImage, Task, WeakEntity, Window, WindowHandle, div, img,
    point, px, size,
};
use serde::Deserialize;

use crate::actions::Actions;
use crate::i18n::Language;
use crate::icons::image_of_bytes;
use crate::motion::Spring;
use crate::platform::{
    AppCommand, AudioOutput, DisplayKey, ImeMode, ImeState, InputLanguage, InputName, KeyLocks,
    Media, Modifiers, NativeSurface, ScreenBrightness, ScreenRect, Typing, VirtualDesktops,
    window_in_front,
};
use crate::popup::{MARGIN, Popups, open_floating, retire_floating};
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, one_line, wave_bar};
use crate::workspaces::Workspaces;

/// How often the volume and the microphone are read while their flyouts
/// are wanted.
const AUDIO_INTERVAL: Duration = Duration::from_millis(250);

/// How often the brightness of the panel is read: its keys change it a
/// step at a time, and the flyout follows them.
const BRIGHTNESS_INTERVAL: Duration = Duration::from_millis(500);

/// How often the keys and the layout are read.
const KEYS_INTERVAL: Duration = Duration::from_millis(250);

/// How often it is read what is playing.
const MEDIA_INTERVAL: Duration = Duration::from_secs(1);

/// How often the workspace shown is looked at; Windows announces no change
/// of its desktops.
const WORKSPACE_INTERVAL: Duration = Duration::from_millis(250);

/// How often the flyout counts down the time it has left.
const TICK: Duration = Duration::from_millis(100);

/// How long the flyout stays after the last change unless `seconds`
/// says otherwise.
const STAY: Duration = Duration::from_millis(1600);

/// The fewest and the most seconds `seconds` may give.
const SECONDS_RANGE: (u32, u32) = (1, 10);

/// The size of the pill, in logical pixels.
const WIDTH: f32 = 280.;
const HEIGHT: f32 = 48.;

/// The size of the card of what is playing.
const MEDIA_WIDTH: f32 = 340.;
const MEDIA_HEIGHT: f32 = 72.;

/// The size of the picture on that card.
const COVER: f32 = 48.;

/// The pixels the picture is prepared with, enough for 200% scaling.
const COVER_SOURCE: u32 = 96;

/// How high a row of the list of layouts is, the room around the rows and
/// the most rows shown.
const LAYOUT_ROW: f32 = 36.;
const LIST_PADDING: f32 = 8.;
const MOST_ROWS: usize = 12;

/// The width of the letters of a layout in that list.
const CODE_WIDTH: f32 = 40.;

/// The size of a dot of the strip of workspaces, and how much wider the
/// one shown is.
const STRIP_DOT: f32 = 8.;
const STRIP_SHOWN: f32 = 20.;

/// How far from the side of the space the pill hangs unless `margin`
/// says otherwise.
const LIFT: f32 = 24.;

/// The most `margin` may be.
const MOST_MARGIN: f32 = 400.;

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
const ICON_NOTE: &str = "\u{ec4f}";
const ICON_WORKSPACES: &str = "\u{e7c4}";

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
    /// Whether another keyboard layout for the window in front is shown,
    /// and the list of layouts while Win+Space is held.
    #[serde(default)]
    layout: bool,
    /// Whether another piece being played is shown, with its picture.
    #[serde(default)]
    media: bool,
    /// Whether going to another workspace is shown.
    #[serde(default)]
    workspace: bool,
    #[serde(default)]
    place: FlyoutPlace,
    /// How many seconds a flyout stays after the last change, from 1 to
    /// 10; 1.6 without the key.
    seconds: Option<u32>,
    /// How far from the sides of the space a flyout hangs, in logical
    /// pixels; 24 without the key.
    margin: Option<u32>,
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
            Kind::Media => self.media,
            Kind::Workspace => self.workspace,
        }
    }

    /// How long a flyout stays after the last change.
    fn stay(&self) -> Duration {
        let (least, most) = SECONDS_RANGE;
        { self.seconds }.map_or(STAY, |seconds| {
            Duration::from_secs(seconds.clamp(least, most).into())
        })
    }

    /// How far from the sides of the space a flyout hangs.
    fn margin(&self) -> f32 {
        { self.margin }.map_or(LIFT, |margin| (margin as f32).min(MOST_MARGIN))
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
    /// In the middle of the left side.
    Left,
    /// In the middle of the right side.
    Right,
    /// In the middle of the space.
    Center,
}

/// From which side a flyout comes in, and goes back to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Travel {
    /// Down from above.
    Down,
    /// Up from below.
    Up,
    /// Right from the left.
    Right,
    /// Left from the right.
    Left,
}

impl FlyoutPlace {
    /// Every place, in the order a choice offers them.
    #[allow(dead_code, reason = "for the rows of [flyout] in the settings")]
    pub const ALL: [Self; 9] = [
        Self::Bottom,
        Self::Top,
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
        Self::Left,
        Self::Right,
        Self::Center,
    ];

    /// What the configuration calls it.
    #[allow(dead_code, reason = "for the rows of [flyout] in the settings")]
    pub fn name(self) -> &'static str {
        match self {
            Self::Bottom => "bottom",
            Self::Top => "top",
            Self::TopLeft => "top-left",
            Self::TopRight => "top-right",
            Self::BottomLeft => "bottom-left",
            Self::BottomRight => "bottom-right",
            Self::Left => "left",
            Self::Right => "right",
            Self::Center => "center",
        }
    }

    /// From where the flyout comes in: the side it hangs at.
    fn travel(self) -> Travel {
        match self {
            Self::Top | Self::TopLeft | Self::TopRight => Travel::Down,
            Self::Left => Travel::Right,
            Self::Right => Travel::Left,
            Self::Bottom | Self::BottomLeft | Self::BottomRight | Self::Center => Travel::Up,
        }
    }
}

impl Travel {
    /// Whether it goes along the width rather than the height.
    fn sideways(self) -> bool {
        matches!(self, Self::Right | Self::Left)
    }

    /// Where the flyout is, `away` from its place, and the room kept for
    /// the motion before it: (left, top, room on the left, room on top).
    fn offset(self, away: f32) -> (f32, f32, f32, f32) {
        match self {
            Self::Down => (0., -away, 0., TRAVEL),
            Self::Up => (0., away, 0., 0.),
            Self::Right => (-away, 0., TRAVEL, 0.),
            Self::Left => (away, 0., 0., 0.),
        }
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
    Media,
    Workspace,
}

const KINDS: [Kind; 8] = [
    Kind::Volume,
    Kind::Brightness,
    Kind::Microphone,
    Kind::CapsLock,
    Kind::NumLock,
    Kind::Layout,
    Kind::Media,
    Kind::Workspace,
];

/// The picture of what is playing, as it is drawn. Two are alike only
/// where they are the same picture.
#[derive(Clone, Debug, Default)]
pub struct Cover(Option<Arc<RenderImage>>);

impl PartialEq for Cover {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(one), Some(other)) => Arc::ptr_eq(one, other),
            (None, None) => true,
            _ => false,
        }
    }
}

/// A keyboard layout in the list shown while Win+Space is held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutChoice {
    /// The letters of its language, as `EN`.
    pub code: String,
    /// What Windows calls it, as "English (United States) · US"; may be
    /// empty.
    pub name: String,
}

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
    /// The installed layouts, the one at `lit` the one switched to.
    Layouts {
        choices: Vec<LayoutChoice>,
        lit: usize,
    },
    /// Another piece being played.
    Media {
        title: String,
        artist: String,
        /// The name of the application that plays it.
        app: String,
        cover: Cover,
    },
    /// The workspace shown, at `current` of those `labels` name.
    Workspace {
        current: usize,
        labels: Vec<String>,
    },
}

impl Reading {
    fn kind(&self) -> Kind {
        match self {
            Self::Volume { .. } => Kind::Volume,
            Self::Brightness(_) => Kind::Brightness,
            Self::Microphone { .. } => Kind::Microphone,
            Self::CapsLock(_) => Kind::CapsLock,
            Self::NumLock(_) => Kind::NumLock,
            Self::Layout(_) | Self::Layouts { .. } => Kind::Layout,
            Self::Media { .. } => Kind::Media,
            Self::Workspace { .. } => Kind::Workspace,
        }
    }

    /// The logical size of what shows it.
    fn size(&self) -> (f32, f32) {
        match self {
            Self::Media { .. } => (MEDIA_WIDTH, MEDIA_HEIGHT),
            Self::Layouts { choices, .. } => {
                let rows = choices.len().clamp(1, MOST_ROWS) as f32;
                (WIDTH, rows * LAYOUT_ROW + LIST_PADDING * 2.)
            }
            _ => (WIDTH, HEIGHT),
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            Self::Volume { level, muted } => volume_icon(*level, *muted),
            Self::Brightness(_) => ICON_BRIGHTNESS,
            Self::Microphone { muted: true } => ICON_MICROPHONE_OFF,
            Self::Microphone { muted: false } => ICON_MICROPHONE,
            Self::CapsLock(_) | Self::NumLock(_) | Self::Layout(_) | Self::Layouts { .. } => {
                ICON_KEYBOARD
            }
            Self::Media { .. } => ICON_NOTE,
            Self::Workspace { .. } => ICON_WORKSPACES,
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
            _ => false,
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
            Self::Layouts { choices, lit } => {
                let choice = choices.get(*lit);
                return choice.map(|choice| choice.code.clone()).unwrap_or_default();
            }
            Self::Media { title, artist, .. } => return headline(title, artist),
            Self::Workspace { current, labels } => {
                let label = labels.get(*current).cloned();
                return label.unwrap_or_else(|| (current + 1).to_string());
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
        let layout = layout_switched(before, now)?;
        // The mode of the input method of a Japanese, Korean or Chinese
        // layout comes after its letters.
        let mode = ImeState::in_front().and_then(|ime| ime.mode());
        Some(Reading::Layout(layout_words(layout.code(), mode)))
    }
}

/// What the layout flyout says: the letters of the language, and the mode
/// of its input method where it has one.
fn layout_words(code: String, mode: Option<ImeMode>) -> String {
    match mode {
        Some(mode) => format!("{code}  {}", mode.glyph()),
        None => code,
    }
}

/// The new layout, when the window in front was switched to another one.
/// Going to another window that is typed in another layout is no switch,
/// and neither is a window of Tessera in front, which tells no layout.
fn layout_switched<L: PartialEq>(before: Option<(isize, L)>, now: Option<(isize, L)>) -> Option<L> {
    let ((window, layout), (now_window, now_layout)) = (before?, now?);
    (window == now_window && layout != now_layout).then_some(now_layout)
}

/// The title and the artist on one line, whichever of them there are.
fn headline(title: &str, artist: &str) -> String {
    match (title.is_empty(), artist.is_empty()) {
        (false, false) => format!("{artist} \u{2013} {title}"),
        (false, true) => title.to_owned(),
        (true, _) => artist.to_owned(),
    }
}

/// The name of the application of a media session as Windows knows it
/// (`Spotify.exe`, or the identifier of a packaged application): the name
/// of its program file without `.exe`, or the name of its package without
/// the publisher before it and the hash after it.
pub fn media_app_name(app: &str) -> &str {
    match app.split_once('!') {
        Some((family, _)) => {
            let package = family.split('_').next().unwrap_or(family);
            package.rsplit('.').next().unwrap_or(package)
        }
        None => {
            let file = app.rsplit(['\\', '/']).next().unwrap_or(app);
            // An application names itself as it likes, in any letters: the
            // end is looked at only where it lies between two of them.
            let stem = file.len().saturating_sub(4);
            match file.get(stem..) {
                Some(end) if end.eq_ignore_ascii_case(".exe") => &file[..stem],
                _ => file,
            }
        }
    }
}

/// What tells one piece being played from another: its application, its
/// title and its artist.
type Track = (String, String, String);

/// Whether `now` is another piece than the one played before, which `last`
/// holds from then on. A piece without a title is not shown, and neither
/// is the first one seen: what plays when Tessera starts is no change.
/// Nothing playing in between makes the next piece a change.
fn track_changed(last: &mut Option<Option<Track>>, now: Option<Track>) -> bool {
    let named = now.as_ref().is_some_and(|(_, title, _)| !title.is_empty());
    let before = last.replace(now.clone());
    match before {
        None => false,
        Some(before) => named && before != now,
    }
}

/// What the session Windows considers current plays, and its picture.
#[derive(Default)]
struct MediaGauge(Option<Option<Track>>);

impl Gauge for MediaGauge {
    const EVERY: Duration = MEDIA_INTERVAL;

    fn next(&mut self) -> Option<Reading> {
        // A failure to ask is no change: the last piece stays known.
        let now = Media::now_playing(None).ok()?;
        let track =
            { now.as_ref() }.map(|now| (now.app.clone(), now.title.clone(), now.artist.clone()));
        if !track_changed(&mut self.0, track) {
            return None;
        }
        let now = now?;
        let bytes = Media::cover(None).ok().flatten();
        let cover = bytes.and_then(|bytes| match image_of_bytes(&bytes, COVER_SOURCE) {
            Ok(image) => image,
            Err(error) => {
                report!("the picture of what is playing is not an image: {error:#}");
                None
            }
        });
        Some(Reading::Media {
            app: media_app_name(&now.app).to_owned(),
            title: now.title,
            artist: now.artist,
            cover: Cover(cover),
        })
    }
}

/// Looks at the workspace shown on the display the pointer is on and
/// shows when another one is. Tessera's own workspaces are looked at on
/// the thread of the windows, where they are kept; the virtual desktops of
/// Windows are read from the registry on the background executor first.
/// Going to another display is no change.
fn watch_workspaces(cx: &mut App) -> Task<()> {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut last: Option<(Option<DisplayKey>, VirtualDesktops)> = None;
        loop {
            let owned = cx.update(|cx| cx.has_global::<Workspaces>().then(|| Workspaces::own(cx)));
            let Ok(owned) = owned else {
                break;
            };
            let of_windows = match owned {
                Some(false) => {
                    let read = cx
                        .background_executor()
                        .spawn(async { VirtualDesktops::now() });
                    Some(read.await)
                }
                _ => None,
            };
            let looked = cx.update(|cx| {
                if owned.is_none() {
                    return;
                }
                let display = DisplayKey::under_pointer();
                let now = of_windows.unwrap_or_else(|| Workspaces::now(display, cx));
                let before = last.replace((display, now));
                if !workspace_changed(before, (display, now)) {
                    return;
                }
                let labels = { 0..now.count }
                    .map(|index| Workspaces::label(index, cx))
                    .collect();
                let current = now.current;
                Flyouts::show(Reading::Workspace { current, labels }, cx);
            });
            if looked.is_err() {
                break;
            }
            cx.background_executor().timer(WORKSPACE_INTERVAL).await;
        }
    })
}

/// Whether another workspace is shown than before, on the same display.
fn workspace_changed(
    before: Option<(Option<DisplayKey>, VirtualDesktops)>,
    now: (Option<DisplayKey>, VirtualDesktops),
) -> bool {
    let Some((display, desktops)) = before else {
        return false;
    };
    display == now.0 && desktops.current != now.1.current && now.1.current < now.1.count
}

/// The names of the installed layouts, as last read.
#[derive(Default)]
struct LayoutNames(Vec<InputName>);

impl Global for LayoutNames {}

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
                Kind::Media => watch::<MediaGauge>(cx),
                Kind::Workspace => watch_workspaces(cx),
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
            let stay = this.config.stay();
            let shown = open.update(cx, |view, window, cx| {
                view.show(reading.clone(), stay, window, cx)
            });
            if matches!(shown, Ok(true)) {
                return;
            }
        }
        Self::open(reading, cx);
    }

    /// Shows the layouts `installed` as a list, `lit` among them, where
    /// layout flyouts are wanted: the action `input-language` switched to
    /// it. The list stays while the Windows key is held. The names are
    /// read on another thread, since reading them changes the layout of the
    /// thread that reads, the first time and when another number of
    /// layouts is installed.
    pub fn show_layouts(installed: Vec<InputLanguage>, lit: InputLanguage, cx: &mut App) {
        let wanted =
            { cx.try_global::<Self>() }.is_some_and(|this| this.config.wants(Kind::Layout));
        if !wanted || installed.is_empty() {
            return;
        }
        let known = cx.default_global::<LayoutNames>().0.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            let read = cx.background_executor().spawn(async move {
                let names = match known.len() == installed.len() {
                    true => known,
                    false => InputName::installed(),
                };
                let codes: Vec<String> = installed.iter().map(|layout| layout.code()).collect();
                let lit = installed.iter().position(|layout| *layout == lit);
                (names, codes, lit.unwrap_or(0))
            });
            let (names, codes, lit) = read.await;
            cx.update(|cx| {
                let choices = layout_choices(codes, names.clone());
                cx.global_mut::<LayoutNames>().0 = names;
                Self::show(Reading::Layouts { choices, lit }, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Opens the window of the flyout with `reading` in it.
    fn open(reading: Reading, cx: &mut App) {
        let Some(anchor) = Shell::middle_anchor(cx) else {
            return;
        };
        let config = &cx.global::<Self>().config;
        let area = anchor.display.and_then(|display| display.work_area());
        let spot = Spot {
            area: area.unwrap_or(anchor.screen),
            scale: anchor.scale,
            place: config.place,
            margin: config.margin(),
        };
        let stay = config.stay();
        let frame = spot.frame(reading.size());
        let rough = gpui::Bounds {
            origin: point(
                px(frame.left as f32 / anchor.scale),
                px(frame.top as f32 / anchor.scale),
            ),
            size: size(
                px((frame.right - frame.left) as f32 / anchor.scale),
                px((frame.bottom - frame.top) as f32 / anchor.scale),
            ),
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
            FlyoutView::new(reading, spot, stay, cx)
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

/// Each of `layouts` with the names Windows gives it, paired in the order
/// both come in, which is the order of Windows. Where the two lists differ
/// in length, as when a layout was added in between, the names are left
/// out, empty, and the letters of the language stand for them.
pub fn with_names<T>(layouts: Vec<T>, names: Vec<InputName>) -> Vec<(T, InputName)> {
    let paired = layouts.len() == names.len();
    let mut names = names.into_iter().filter(|_| paired);
    { layouts.into_iter() }
        .map(|layout| (layout, names.next().unwrap_or_default()))
        .collect()
}

/// The layouts with their names, as [`with_names`] pairs them.
fn layout_choices(codes: Vec<String>, names: Vec<InputName>) -> Vec<LayoutChoice> {
    { with_names(codes, names).into_iter() }
        .map(|(code, name)| {
            let name = match (name.language.is_empty(), name.layout.is_empty()) {
                (false, false) => format!("{} \u{00b7} {}", name.language, name.layout),
                (false, true) => name.language,
                (true, _) => name.layout,
            };
            LayoutChoice { code, name }
        })
        .collect()
}

/// Where a flyout hangs: the space of a display it hangs in, in physical
/// pixels, of which there are `scale` in a logical one, the place in it
/// and how far from its sides, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Spot {
    area: ScreenRect,
    scale: f32,
    place: FlyoutPlace,
    margin: f32,
}

impl Spot {
    /// The frame of the window of a flyout of the logical size `shown`,
    /// in physical pixels: at the place, `margin` away from the sides, with
    /// room for the motion along the side it comes from.
    fn frame(self, shown: (f32, f32)) -> ScreenRect {
        let Self {
            area,
            scale,
            place,
            margin,
        } = self;
        let physical = |length: f32| (length * scale).round() as i32;
        let (travel_x, travel_y) = match place.travel().sideways() {
            true => (TRAVEL, 0.),
            false => (0., TRAVEL),
        };
        let width = physical(shown.0 + MARGIN * 2. + travel_x);
        let height = physical(shown.1 + MARGIN * 2. + travel_y);
        let inset = physical(margin - MARGIN);
        let middle_x = (area.left + area.right - width) / 2;
        let middle_y = (area.top + area.bottom - height) / 2;
        let (left, right) = (area.left + inset, area.right - inset - width);
        let (top, bottom) = (area.top + inset, area.bottom - inset - height);
        let (left, top) = match place {
            FlyoutPlace::Bottom => (middle_x, bottom),
            FlyoutPlace::Top => (middle_x, top),
            FlyoutPlace::TopLeft => (left, top),
            FlyoutPlace::TopRight => (right, top),
            FlyoutPlace::BottomLeft => (left, bottom),
            FlyoutPlace::BottomRight => (right, bottom),
            FlyoutPlace::Left => (left, middle_y),
            FlyoutPlace::Right => (right, middle_y),
            FlyoutPlace::Center => (middle_x, middle_y),
        };
        ScreenRect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }
}

/// The frame of the window of the pill in the space `area`, at `place`
/// and as far from the sides as without `margin`.
#[cfg(test)]
fn frame(area: ScreenRect, scale: f32, place: FlyoutPlace) -> ScreenRect {
    let spot = Spot {
        area,
        scale,
        place,
        margin: LIFT,
    };
    spot.frame((WIDTH, HEIGHT))
}

/// The root view of the window of the flyout.
pub struct FlyoutView {
    reading: Reading,
    /// Where it hangs, which says from where it comes in and how large its
    /// window is.
    spot: Spot,
    /// The length of the line, which follows the level.
    line: Spring,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    /// How long it stays after the last change.
    stay: Duration,
    /// How long it still stays.
    left: Duration,
    /// The window was put away and shows nothing more.
    gone: bool,
}

impl FlyoutView {
    fn new(reading: Reading, spot: Spot, stay: Duration, cx: &mut Context<Self>) -> Self {
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
            spot,
            presence,
            stay,
            left: stay,
            gone: false,
        }
    }

    /// Shows `reading` and stays on for `stay` from now. `false` when the
    /// window was put away and another one is needed.
    fn show(
        &mut self,
        reading: Reading,
        stay: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.gone {
            return false;
        }
        // The list of layouts shows the layout switched to already; the
        // layout the window then tells is no news.
        if matches!(self.reading, Reading::Layouts { .. })
            && matches!(reading, Reading::Layout(_))
            && !self.left.is_zero()
        {
            return true;
        }
        match reading.share() {
            // From one kind to another the line starts anew.
            Some(share) if reading.kind() == self.reading.kind() => self.line.animate_to(share),
            Some(share) => self.line.jump_to(share),
            None => {}
        }
        if reading.size() != self.reading.size() {
            self.resize(reading.size(), window, cx);
        }
        self.reading = reading;
        self.presence.animate_to(1.);
        (self.stay, self.left) = (stay, stay);
        cx.notify();
        true
    }

    /// Makes the window as large as a flyout of the logical size `shown`
    /// needs, where it hangs.
    fn resize(&self, shown: (f32, f32), window: &mut Window, cx: &mut Context<Self>) {
        let frame = self.spot.frame(shown);
        match NativeSurface::of(window) {
            // The window learns its new size only if it is given while
            // nothing of the user interface is being worked on.
            Ok(native) => cx
                .spawn(async move |_, _| {
                    if let Err(error) = native.place(frame) {
                        report!("the flyout is not where it belongs: {error:#}");
                    }
                })
                .detach(),
            Err(error) => report!("the flyout is not where it belongs: {error:#}"),
        }
    }

    /// Counts down the time the flyout has left; the list of layouts
    /// stays while the Windows key is held. `false` once the window was
    /// put away.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        if self.gone {
            return false;
        }
        if matches!(self.reading, Reading::Layouts { .. })
            && !self.left.is_zero()
            && Modifiers::down_now().windows
        {
            self.left = self.stay;
            return true;
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

    /// The pill of a reading with a level or words.
    fn pill(&self, theme: &Theme, language: Language, line: f32) -> gpui::Div {
        let reading = &self.reading;
        let pill = div()
            .w(px(WIDTH))
            .h(px(HEIGHT))
            .px(px(PADDING))
            .gap(theme.spacing.medium)
            .flex()
            .items_center()
            .rounded(theme.shape.full)
            .bg(theme.colors.surface_container_highest)
            .child(icon(theme, reading.icon()));
        let words = reading.words(language);
        match reading.share() {
            Some(_) => pill
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(wave_bar(theme, if reading.dimmed() { 0. } else { line })),
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
        }
    }

    /// The card of what is playing: its picture, its title, and its artist
    /// with its application.
    fn media_card(&self, theme: &Theme, language: Language) -> gpui::Div {
        let Reading::Media {
            title,
            artist,
            app,
            cover,
        } = &self.reading
        else {
            return div();
        };
        let picture = div()
            .size(px(COVER))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.small)
            .overflow_hidden()
            .bg(theme.colors.surface_container)
            .text_color(theme.colors.on_surface_variant);
        let picture = match &cover.0 {
            Some(image) => picture.child(
                img(image.clone())
                    .size_full()
                    .rounded(theme.shape.small)
                    .object_fit(ObjectFit::Cover),
            ),
            None => picture.child(icon(theme, ICON_NOTE)),
        };
        let app = match app.is_empty() {
            true => language.say("Media").to_owned(),
            false => app.clone(),
        };
        let below = match artist.is_empty() {
            true => app,
            false => format!("{artist} \u{00b7} {app}"),
        };
        div()
            .w(px(MEDIA_WIDTH))
            .h(px(MEDIA_HEIGHT))
            .px(px(LIST_PADDING * 1.5))
            .gap(theme.spacing.medium)
            .flex()
            .items_center()
            .rounded(theme.shape.large)
            .bg(theme.colors.surface_container_highest)
            .child(picture)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(one_line(title.clone())),
                    )
                    .child(
                        div()
                            .text_color(theme.colors.on_surface_variant)
                            .child(one_line(below)),
                    ),
            )
    }

    /// The pill of the workspace shown: its name and a dot for each
    /// workspace, the one shown long and lit.
    fn workspace_pill(&self, theme: &Theme, language: Language) -> gpui::Div {
        let Reading::Workspace { current, labels } = &self.reading else {
            return div();
        };
        let dots = (0..labels.len()).map(|index| {
            let shown = index == *current;
            div()
                .h(px(STRIP_DOT))
                .w(px(if shown { STRIP_SHOWN } else { STRIP_DOT }))
                .flex_none()
                .rounded(theme.shape.full)
                .bg(match shown {
                    true => theme.colors.primary,
                    false => theme.colors.outline,
                })
        });
        div()
            .w(px(WIDTH))
            .h(px(HEIGHT))
            .px(px(PADDING))
            .gap(theme.spacing.medium)
            .flex()
            .items_center()
            .rounded(theme.shape.full)
            .bg(theme.colors.surface_container_highest)
            .child(icon(theme, ICON_WORKSPACES))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .child(one_line(self.reading.words(language))),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(STRIP_DOT / 2.))
                    .children(dots),
            )
    }

    /// The list of the installed layouts, the one switched to lit.
    fn layout_list(&self, theme: &Theme) -> gpui::Div {
        let Reading::Layouts { choices, lit } = &self.reading else {
            return div();
        };
        let (width, height) = self.reading.size();
        let rows = choices
            .iter()
            .enumerate()
            .take(MOST_ROWS)
            .map(|(index, choice)| {
                let chosen = index == *lit;
                let row = div()
                    .h(px(LAYOUT_ROW))
                    .px(theme.spacing.medium)
                    .gap(theme.spacing.medium)
                    .flex()
                    .items_center()
                    .rounded(theme.shape.full)
                    .child(
                        div()
                            .w(px(CODE_WIDTH))
                            .flex_none()
                            .font_weight(FontWeight::MEDIUM)
                            .child(choice.code.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme.colors.on_surface_variant)
                            .child(one_line(choice.name.clone())),
                    );
                match chosen {
                    true => row.bg(theme.colors.secondary_container),
                    false => row,
                }
            });
        div()
            .w(px(width))
            .h(px(height))
            .p(px(LIST_PADDING))
            .flex()
            .flex_col()
            .rounded(theme.shape.large)
            .bg(theme.colors.surface_container_highest)
            .children(rows)
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
        let colors = theme.colors;
        let shown = match self.reading.dimmed() {
            true => colors.on_surface_variant,
            false => colors.on_surface,
        };
        // It comes in from the side it hangs at.
        let away = TRAVEL * (1. - presence);
        let (left, top, room_left, room_top) = self.spot.place.travel().offset(away);
        let content: AnyElement = match &self.reading {
            Reading::Media { .. } => self.media_card(&theme, language).into_any_element(),
            Reading::Workspace { .. } => self.workspace_pill(&theme, language).into_any_element(),
            Reading::Layouts { .. } => self.layout_list(&theme).into_any_element(),
            _ => self.pill(&theme, language, line).into_any_element(),
        };
        div()
            .size_full()
            .p(px(MARGIN))
            .pt(px(MARGIN + room_top))
            .pl(px(MARGIN + room_left))
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .text_color(shown)
            .opacity(presence.clamp(0., 1.))
            .child(div().relative().left(px(left)).top(px(top)).child(content))
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

    #[test]
    fn the_flyout_stays_and_hangs_as_configured() {
        let config: FlyoutConfig = toml::from_str("").unwrap();
        assert_eq!(config.stay(), STAY);
        assert_eq!(config.margin(), LIFT);
        let config: FlyoutConfig = toml::from_str("seconds = 4\nmargin = 40").unwrap();
        assert_eq!(config.stay(), Duration::from_secs(4));
        assert_eq!(config.margin(), 40.);
        let config: FlyoutConfig = toml::from_str("seconds = 60\nmargin = 9000").unwrap();
        assert_eq!(config.stay(), Duration::from_secs(10));
        assert_eq!(config.margin(), MOST_MARGIN);
        assert!(toml::from_str::<FlyoutConfig>("margin = -5").is_err());
        let config: FlyoutConfig = toml::from_str("seconds = 0").unwrap();
        assert_eq!(config.stay(), Duration::from_secs(1));
        let config: FlyoutConfig = toml::from_str("media = true\nworkspace = true").unwrap();
        assert!(config.wants(Kind::Media) && config.wants(Kind::Workspace));
        assert!(!config.wants(Kind::Volume));
    }

    #[test]
    fn every_place_is_named_as_the_configuration_writes_it() {
        #[derive(Deserialize)]
        struct Written {
            place: FlyoutPlace,
        }
        for place in FlyoutPlace::ALL {
            let text = format!("place = \"{}\"", place.name());
            let read: Written = toml::from_str(&text).unwrap();
            assert_eq!(read.place, place);
        }
    }

    #[test]
    fn the_flyout_hangs_at_the_middle_of_a_side() {
        let left = frame(AREA, 1., FlyoutPlace::Left);
        // The motion goes along the width there.
        assert_eq!((left.left, left.right - left.left), (112, 320));
        assert_eq!(left.bottom - left.top, 72);
        assert_eq!(left.top - AREA.top, AREA.bottom - left.bottom);
        let right = frame(AREA, 1., FlyoutPlace::Right);
        assert_eq!(right.right, 1088);
        let center = frame(AREA, 1., FlyoutPlace::Center);
        assert_eq!(center.left - AREA.left, AREA.right - center.right);
        assert_eq!(center.top - AREA.top, AREA.bottom - center.bottom);
        // Farther from the side with a larger margin.
        let spot = Spot {
            area: AREA,
            scale: 1.,
            place: FlyoutPlace::Bottom,
            margin: 64.,
        };
        assert_eq!(spot.frame((WIDTH, HEIGHT)).bottom, 748);
        // A taller flyout has a taller window.
        let tall = spot.frame((WIDTH, 200.));
        assert_eq!(tall.bottom - tall.top, 240);
    }

    #[test]
    fn a_new_piece_is_shown_but_not_the_first_one() {
        let track = |title: &str| Some(("app".to_owned(), title.to_owned(), "artist".to_owned()));
        let mut last = None;
        assert!(!track_changed(&mut last, track("one")));
        assert!(!track_changed(&mut last, track("one")));
        assert!(track_changed(&mut last, track("two")));
        assert!(!track_changed(&mut last, None));
        assert!(track_changed(&mut last, track("two")));
        // A piece without a title is not shown.
        assert!(!track_changed(&mut last, track("")));
        // Nothing playing when Tessera starts: the first piece is new.
        let mut last = None;
        assert!(!track_changed(&mut last, None));
        assert!(track_changed(&mut last, track("one")));
    }

    #[test]
    fn another_workspace_is_shown_on_the_same_display_only() {
        let desktops = |current| VirtualDesktops { count: 4, current };
        assert!(!workspace_changed(None, (None, desktops(1))));
        assert!(workspace_changed(
            Some((None, desktops(0))),
            (None, desktops(1))
        ));
        assert!(!workspace_changed(
            Some((None, desktops(1))),
            (None, desktops(1))
        ));
        let display = DisplayKey::primary();
        if display.is_some() {
            assert!(!workspace_changed(
                Some((None, desktops(0))),
                (display, desktops(1))
            ));
        }
    }

    #[test]
    fn layouts_are_listed_with_their_names_when_both_lists_agree() {
        let name = |language: &str, layout: &str| InputName {
            language: language.to_owned(),
            layout: layout.to_owned(),
        };
        let codes = vec!["EN".to_owned(), "RU".to_owned()];
        let names = [name("English", "US"), name("Russian", "")];
        let choices = layout_choices(codes.clone(), names.to_vec());
        assert_eq!(choices[0].name, "English \u{00b7} US");
        assert_eq!(choices[1].name, "Russian");
        let unpaired = layout_choices(codes, names[..1].to_vec());
        assert!(unpaired.iter().all(|choice| choice.name.is_empty()));
        let list = Reading::Layouts { choices, lit: 1 };
        assert_eq!(list.words(Language::English), "RU");
        assert_eq!(list.kind(), Kind::Layout);
        assert_eq!(list.size(), (WIDTH, 2. * LAYOUT_ROW + 2. * LIST_PADDING));
    }

    #[test]
    fn the_layout_flyout_gives_the_mode_of_the_input_method() {
        assert_eq!(
            layout_words("JA".to_owned(), Some(ImeMode::Hiragana)),
            "JA  \u{3042}"
        );
        assert_eq!(layout_words("EN".to_owned(), None), "EN");
    }

    #[test]
    fn a_piece_is_named_by_its_title_artist_and_application() {
        assert_eq!(headline("Song", "Band"), "Band \u{2013} Song");
        assert_eq!(headline("Song", ""), "Song");
        assert_eq!(media_app_name("Spotify.exe"), "Spotify");
        assert_eq!(media_app_name(r"C:\Program Files\VLC\vlc.EXE"), "vlc");
        assert_eq!(
            media_app_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"),
            "SpotifyMusic"
        );
        let workspace = Reading::Workspace {
            current: 2,
            labels: vec!["web".to_owned(), "code".to_owned()],
        };
        assert_eq!(workspace.words(Language::English), "3");
    }
}
