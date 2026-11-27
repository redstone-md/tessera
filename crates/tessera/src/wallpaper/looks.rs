// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the configuration asks each display to show ([`Look`]), and what
//! that comes to at one moment ([`Plan`]): the workspace a display shows,
//! whether it is dark, which picture of a list is due and what Windows
//! has as its wallpaper decide it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;

use super::adjust::{AdjustKeys, Adjustments, Flip, OverlayConfig, Position};
use super::compose::{Fit, Ground};
use super::daily::Daily;
use crate::platform::{ScreenRect, SystemWallpaper};
use crate::theme::parse_rgb;

/// How long a picture stays unless the configuration says, in seconds.
const INTERVAL: u64 = 600;

/// The shortest time a picture stays, in seconds: reading the next one
/// takes time and memory.
pub const SHORTEST_INTERVAL: u64 = 5;

/// The kinds of file taken from a directory, by extension: those Tessera
/// reads itself, and those a codec of Windows may read.
const KINDS: [&str; 14] = [
    "jpg", "jpeg", "png", "bmp", "gif", "tif", "tiff", "webp", "heic", "heif", "avif", "jxr",
    "wdp", "jfif",
];

/// The share of a display that windows cover at which a video there
/// stands still, where the configuration does not say.
pub const PAUSE_COVERAGE: f32 = 0.9;

/// The slowest and the fastest a video is played, as a factor.
pub const SLOWEST: f32 = 0.25;
pub const FASTEST: f32 = 4.;

/// Where the wallpaper is taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The pictures and videos the configuration lists.
    Files,
    /// The wallpaper of Windows.
    Windows,
    /// A picture of the day from the network.
    Daily,
    /// The color or the gradient alone.
    Color,
}

impl Source {
    /// Every source, in the order the settings offer them.
    pub const ALL: [Self; 4] = [Self::Files, Self::Windows, Self::Daily, Self::Color];

    /// The name the configuration gives it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Windows => "windows",
            Self::Daily => "daily",
            Self::Color => "color",
        }
    }
}

/// What the wallpaper settings show of the configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub source: Source,
    /// Each collection by its name, with its first picture or video.
    pub collections: Vec<(String, Option<PathBuf>)>,
    /// The collection all displays show.
    pub collection: Option<String>,
    /// How many seconds a picture stays.
    pub interval: u64,
    pub shuffle: bool,
    /// The collection all displays show where it has an `interval` or a
    /// `shuffle` of its own: those are what is shown above, and changed.
    pub interval_in: Option<String>,
    pub shuffle_in: Option<String>,
    pub fit: Fit,
    pub position: Option<Position>,
    pub blur: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub flip: Flip,
    pub overlay: Option<OverlayConfig>,
    pub speed: f32,
    pub sound: bool,
    pub volume: f32,
    pub pause_coverage: f32,
    /// The entries of single displays, by number, with the collection
    /// each shows.
    pub displays: Vec<(usize, Option<String>)>,
}

/// What the configuration says about the wallpaper.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WallpaperConfig {
    /// Where the wallpaper is taken from; without it, from what else is
    /// given, and from Windows where nothing is.
    source: Option<Source>,
    /// Picture files, and directories whose picture files are taken, in
    /// the order of their names.
    #[serde(default)]
    images: Vec<PathBuf>,
    /// Pictures in place of `images` while Windows is light.
    #[serde(default)]
    images_light: Vec<PathBuf>,
    /// Pictures in place of `images` while Windows is dark.
    #[serde(default)]
    images_dark: Vec<PathBuf>,
    /// When it is dark, as two times of day: from the first to the
    /// second. Without it, it is dark while Windows is.
    dark_hours: Option<[String; 2]>,
    /// How many seconds a picture stays before the next one.
    interval: Option<u64>,
    /// The pictures and the videos in a random order.
    #[serde(default)]
    shuffle: bool,
    /// A video file to play instead of the pictures, which are shown
    /// where it cannot be played.
    video: Option<PathBuf>,
    /// Videos played one after the other, after `video`.
    #[serde(default)]
    videos: Vec<PathBuf>,
    /// The collection shown, by its name, in place of `images`, `video`
    /// and `videos`.
    collection: Option<String>,
    /// Named lists of pictures and videos to choose from.
    #[serde(default)]
    collections: Vec<Collection>,
    /// How a picture that has not the shape of its display is fitted.
    #[serde(default)]
    fit: Fit,
    /// The color around a picture, or the wallpaper without one.
    color: Option<String>,
    /// Two colors to go from at the top to at the bottom, in place of
    /// `color`.
    gradient: Option<[String; 2]>,
    /// Where a picture of the day comes from.
    daily: Option<Daily>,
    /// Whether the picture shown is made the wallpaper of Windows too.
    #[serde(default)]
    pub mirror: bool,
    /// Whether videos stand still while the machine runs on its battery.
    #[serde(default)]
    pause_on_battery: bool,
    /// The share of a display, from 0 to 1, that windows cover at which a
    /// video there stands still; 0 never holds it.
    pause_coverage: Option<f32>,
    /// The adjustments of the picture, see [`AdjustKeys`].
    blur: Option<f32>,
    saturation: Option<f32>,
    contrast: Option<f32>,
    brightness: Option<f32>,
    flip: Option<Flip>,
    overlay: Option<OverlayConfig>,
    position: Option<Position>,
    /// How fast videos play, as a factor.
    speed: Option<f32>,
    /// Whether the sound of videos is heard, and how loud, from 0 to 1.
    sound: Option<bool>,
    volume: Option<f32>,
    /// The wallpapers of single displays, in place of the one of all.
    #[serde(default, rename = "display")]
    displays: Vec<PlaceWallpaper>,
    /// The wallpapers of single workspaces, in place of those of the
    /// displays that show them.
    #[serde(default, rename = "workspace")]
    workspaces: Vec<PlaceWallpaper>,
}

/// What the configuration says about the wallpaper of one display or of
/// one workspace.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaceWallpaper {
    /// Which display or workspace, the first being 1. Displays are
    /// counted from the left, and from the top where two begin equally
    /// far left. A workspace is its place in the row of workspaces.
    number: usize,
    /// Picture files and directories, as `images` of the wallpaper.
    #[serde(default)]
    images: Vec<PathBuf>,
    /// A video, as `video` of the wallpaper.
    video: Option<PathBuf>,
    /// Videos, as `videos` of the wallpaper.
    #[serde(default)]
    videos: Vec<PathBuf>,
    /// A collection, in place of `images`, `video` and `videos`.
    collection: Option<String>,
    /// How its pictures are fitted, in place of `fit` of the wallpaper.
    fit: Option<Fit>,
    /// Adjustments and the sound of videos in place of those of the
    /// wallpaper; what is left out is taken from there.
    blur: Option<f32>,
    saturation: Option<f32>,
    contrast: Option<f32>,
    brightness: Option<f32>,
    flip: Option<Flip>,
    overlay: Option<OverlayConfig>,
    position: Option<Position>,
    speed: Option<f32>,
    sound: Option<bool>,
    volume: Option<f32>,
}

/// A list of pictures and videos with a name, as the collections of
/// wallpapers of Seelen UI are.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Collection {
    name: String,
    /// Picture files and directories, as `images` of the wallpaper.
    #[serde(default)]
    images: Vec<PathBuf>,
    video: Option<PathBuf>,
    #[serde(default)]
    videos: Vec<PathBuf>,
    /// In place of `interval` and `shuffle` of the wallpaper.
    interval: Option<u64>,
    shuffle: Option<bool>,
}

/// How fast a video plays and how it sounds, as one place of the
/// configuration gives it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct VideoKeys {
    speed: Option<f32>,
    sound: Option<bool>,
    volume: Option<f32>,
}

impl VideoKeys {
    fn over(self, under: Self) -> Self {
        Self {
            speed: self.speed.or(under.speed),
            sound: self.sound.or(under.sound),
            volume: self.volume.or(under.volume),
        }
    }

    /// What the keys come to; a value out of its range is reported and
    /// taken as the nearest in it. `of` names the place for the report.
    fn resolve(self, of: &str) -> VideoSound {
        let within = |key: &str, value: Option<f32>, (least, most): (f32, f32), unset: f32| {
            let Some(value) = value.filter(|value| !value.is_nan()) else {
                return unset;
            };
            if !(least..=most).contains(&value) {
                report!(
                    "`{of}.{key}` goes from {least} to {most}, {value} is taken as the nearest"
                );
            }
            value.clamp(least, most)
        };
        VideoSound {
            speed: within("speed", self.speed, (SLOWEST, FASTEST), 1.),
            sound: self.sound.unwrap_or(false),
            volume: within("volume", self.volume, (0., 1.), 1.),
        }
    }
}

/// How fast a video plays and how it sounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoSound {
    /// 1 for as it was made.
    pub speed: f32,
    /// Whether the sound is heard at all.
    pub sound: bool,
    /// How loud, from 0 to 1, where it is heard.
    pub volume: f32,
}

impl Default for VideoSound {
    fn default() -> Self {
        Self {
            speed: 1.,
            sound: false,
            volume: 1.,
        }
    }
}

/// How the pictures of a list take turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Turning {
    /// How long one stays.
    pub interval: Duration,
    /// Whether they come in a random order.
    pub shuffle: bool,
}

/// What a display shows as the configuration says, before the moment
/// decides on it.
#[derive(Clone, Debug, PartialEq)]
pub enum Look {
    /// The wallpaper of Windows, adjusted; Tessera draws it where the
    /// desktop window does not, or where it is to look otherwise.
    Windows(Adjustments),
    /// A color or a gradient alone.
    Fill(Ground),
    /// Pictures that take turns, the first list while Windows is light
    /// and the second while it is dark.
    Pictures {
        light: Vec<PathBuf>,
        dark: Vec<PathBuf>,
        fit: Fit,
        turning: Turning,
        adjust: Adjustments,
    },
    /// Videos one after the other, and what is shown where none of them
    /// can be played.
    Videos {
        files: Vec<PathBuf>,
        then: Box<Look>,
        shuffle: bool,
        adjust: Adjustments,
        sound: VideoSound,
    },
    /// A picture of the day.
    Daily {
        daily: Daily,
        fit: Fit,
        adjust: Adjustments,
    },
}

/// What every display shows as the configuration says.
#[derive(Clone, Debug, PartialEq)]
pub struct Looks {
    all: Look,
    displays: Vec<(usize, Look)>,
    workspaces: Vec<(usize, Look)>,
    /// What lies around a picture that does not cover its display.
    pub ground: Ground,
    /// From when to when it is dark, in minutes of the day; `None` for as
    /// long as Windows is dark.
    pub dark_hours: Option<(u32, u32)>,
    pub pause_on_battery: bool,
    /// The share of a display windows cover at which its video stands
    /// still; 0 for never.
    pub pause_coverage: f32,
}

/// How many times the pictures of each interval have changed, and since
/// when the one shown is there.
#[derive(Debug, Default)]
pub struct Turns {
    counts: Vec<(Duration, i64, Instant)>,
}

impl Turns {
    /// How many times pictures that stay for `interval` have changed:
    /// less than 0 where they went back from the first.
    pub fn turn(&self, interval: Duration) -> i64 {
        let found = self.counts.iter().find(|(of, ..)| *of == interval);
        found.map_or(0, |(_, turn, _)| *turn)
    }

    /// Goes on by one for each of `intervals` that has passed by `now`;
    /// one not counted before begins at `now`.
    pub fn tick(&mut self, intervals: &[Duration], now: Instant) {
        for interval in intervals {
            match self.counts.iter_mut().find(|(of, ..)| of == interval) {
                Some((_, turn, since)) => {
                    if now.saturating_duration_since(*since) >= *interval {
                        *turn = turn.saturating_add(1);
                        *since = now;
                    }
                }
                None => self.counts.push((*interval, 0, now)),
            }
        }
    }

    /// Goes on, or back, by `by` everywhere at once; every interval
    /// begins anew at `now`.
    pub fn step(&mut self, by: i64, now: Instant) {
        for (_, turn, since) in &mut self.counts {
            *turn = turn.saturating_add(by);
            *since = now;
        }
    }
}

/// What decides on the looks at one moment.
pub struct Moment<'a> {
    /// Whether the dark pictures are shown.
    pub dark: bool,
    /// How many times the pictures have changed.
    pub turns: &'a Turns,
    /// What the random order of the pictures and the videos comes from.
    pub seed: u64,
    /// The wallpaper of Windows, where it was read.
    pub windows: Option<&'a WindowsNow>,
    /// Whether the wallpaper of Windows is left to the desktop window:
    /// it draws it itself, or there is none for Tessera to draw it under.
    pub painted: bool,
    /// The picture of the day, where there is one.
    pub daily: Option<&'a Path>,
    /// Videos that could not be played.
    pub failed: &'a HashSet<PathBuf>,
    /// Whether videos may be opened yet.
    pub videos: bool,
}

/// The wallpaper of Windows as it was read, with the time each of its
/// files was last written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowsNow {
    pub wallpaper: SystemWallpaper,
    pub written: HashMap<PathBuf, SystemTime>,
}

impl WindowsNow {
    /// Reads the wallpaper of Windows for the displays at `frames`. For
    /// a thread with COM; see [`SystemWallpaper::read`].
    pub fn read(frames: &[ScreenRect]) -> Self {
        let wallpaper = SystemWallpaper::read(frames);
        let written = wallpaper
            .pictures
            .iter()
            .filter_map(|(_, picture)| {
                let picture = picture.as_ref()?;
                let written = std::fs::metadata(picture).ok()?.modified().ok()?;
                Some((picture.clone(), written))
            })
            .collect();
        Self { wallpaper, written }
    }
}

/// Videos as one display plays them.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoPlan {
    /// In the order they are played.
    pub files: Vec<PathBuf>,
    /// What of the adjustments a video gets: see
    /// [`Adjustments::for_video`].
    pub adjust: Adjustments,
    pub sound: VideoSound,
}

/// What one display is to show now.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Plan {
    /// No window of Tessera: what the desktop window draws is seen.
    #[default]
    Nothing,
    /// A picture. `written` tells a file of Windows that was written
    /// anew from the same one; `own` is false for the wallpaper of
    /// Windows.
    Picture {
        file: PathBuf,
        fit: Fit,
        ground: Ground,
        written: Option<SystemTime>,
        own: bool,
        adjust: Adjustments,
    },
    /// A color or a gradient.
    Fill(Ground),
    /// Videos.
    Video(VideoPlan),
}

impl Plan {
    /// The same plan without the blur of its picture, as performance mode
    /// asks where it saves on looks.
    pub fn unblurred(mut self) -> Self {
        if let Self::Picture { adjust, .. } = &mut self {
            adjust.blur = 0.;
        }
        self
    }
}

/// A look of videos, where `video` and `videos` give some, or else
/// `then`.
fn with_videos(
    video: &Option<PathBuf>,
    videos: &[PathBuf],
    then: Look,
    (shuffle, adjust, sound): (bool, Adjustments, VideoSound),
) -> Look {
    let files: Vec<PathBuf> = video.iter().chain(videos).cloned().collect();
    if files.is_empty() {
        return then;
    }
    Look::Videos {
        files,
        then: Box::new(then),
        shuffle,
        adjust,
        sound,
    }
}

/// A look of pictures, where `light` or `dark` has one.
fn pictures(
    (light, dark): (Vec<PathBuf>, Vec<PathBuf>),
    fit: Fit,
    turning: Turning,
    adjust: Adjustments,
) -> Option<Look> {
    if light.is_empty() && dark.is_empty() {
        return None;
    }
    Some(Look::Pictures {
        light,
        dark,
        fit,
        turning,
        adjust,
    })
}

/// How long a picture stays where the configuration at `key` says
/// `seconds`, or the default without it; less than the shortest time is
/// reported and taken as that.
fn interval_of(seconds: Option<u64>, key: &str) -> Duration {
    let seconds = seconds.unwrap_or(INTERVAL);
    if seconds < SHORTEST_INTERVAL {
        report!("`{key}` is under {SHORTEST_INTERVAL} seconds, which it is taken as");
    }
    Duration::from_secs(seconds.max(SHORTEST_INTERVAL))
}

impl Collection {
    /// What the collection shows, fitted as `fit` says, taking turns as
    /// `turning` says where it does not say itself; `unfilled` where it
    /// has neither pictures nor videos.
    fn look(
        &self,
        fit: Fit,
        turning: Turning,
        (adjust, sound): (Adjustments, VideoSound),
        unfilled: Look,
    ) -> Look {
        let turning = Turning {
            interval: self.interval.map_or(turning.interval, |seconds| {
                interval_of(Some(seconds), "wallpaper.collections.interval")
            }),
            shuffle: self.shuffle.unwrap_or(turning.shuffle),
        };
        let images = pictures_in(&self.images);
        let shown = pictures((images.clone(), images), fit, turning, adjust).unwrap_or(unfilled);
        with_videos(
            &self.video,
            &self.videos,
            shown,
            (turning.shuffle, adjust, sound),
        )
    }
}

impl PlaceWallpaper {
    fn adjust_keys(&self) -> AdjustKeys {
        AdjustKeys {
            blur: self.blur,
            saturation: self.saturation,
            contrast: self.contrast,
            brightness: self.brightness,
            flip: self.flip,
            overlay: self.overlay.clone(),
            position: self.position,
        }
    }

    fn video_keys(&self) -> VideoKeys {
        VideoKeys {
            speed: self.speed,
            sound: self.sound,
            volume: self.volume,
        }
    }
}

impl WallpaperConfig {
    /// What the configuration has every display show. Directories are
    /// read for this and what cannot be used is reported, so it is not
    /// for the thread of the interface.
    pub fn looks(&self) -> Looks {
        let ground = self.ground();
        let adjust_keys = self.adjust_keys();
        let video_keys = self.video_keys();
        let adjust = adjust_keys.resolve("wallpaper");
        let sound = video_keys.resolve("wallpaper");
        let turning = Turning {
            interval: interval_of(self.interval, "wallpaper.interval"),
            shuffle: self.shuffle,
        };
        let source = self.source.unwrap_or_else(|| self.implied_source());
        let all = match source {
            Source::Windows => Look::Windows(adjust),
            Source::Color => Look::Fill(ground),
            Source::Daily => Look::Daily {
                daily: self.daily.unwrap_or_default(),
                fit: self.fit,
                adjust,
            },
            Source::Files => {
                let unfilled = if self.color.is_some() || self.gradient.is_some() {
                    Look::Fill(ground)
                } else {
                    Look::Windows(adjust)
                };
                match self.collection_named(self.collection.as_deref(), "wallpaper") {
                    Some(collection) => {
                        collection.look(self.fit, turning, (adjust, sound), unfilled)
                    }
                    None => {
                        let images = pictures_in(&self.images);
                        let light = self.images_light.is_empty().then(|| images.clone());
                        let light = light.unwrap_or_else(|| pictures_in(&self.images_light));
                        let dark = self.images_dark.is_empty().then(|| images.clone());
                        let dark = dark.unwrap_or_else(|| pictures_in(&self.images_dark));
                        let shown =
                            pictures((light, dark), self.fit, turning, adjust).unwrap_or(unfilled);
                        let videos = (turning.shuffle, adjust, sound);
                        with_videos(&self.video, &self.videos, shown, videos)
                    }
                }
            }
        };
        let place = |kind: &str, place: &PlaceWallpaper| {
            let of = format!("wallpaper.{kind}");
            let adjust = place.adjust_keys().over(&adjust_keys).resolve(&of);
            let sound = place.video_keys().over(video_keys).resolve(&of);
            let fit = place.fit.unwrap_or(self.fit);
            let unfilled = Look::Windows(adjust);
            let look = match self.collection_named(place.collection.as_deref(), &of) {
                Some(collection) => collection.look(fit, turning, (adjust, sound), unfilled),
                None => {
                    let images = pictures_in(&place.images);
                    let shown = pictures((images.clone(), images), fit, turning, adjust);
                    let videos = (turning.shuffle, adjust, sound);
                    let shown = shown.unwrap_or(unfilled);
                    with_videos(&place.video, &place.videos, shown, videos)
                }
            };
            (place.number, look)
        };
        Looks {
            all,
            displays: { self.displays.iter() }
                .map(|entry| place("display", entry))
                .collect(),
            workspaces: { self.workspaces.iter() }
                .map(|entry| place("workspace", entry))
                .collect(),
            ground,
            dark_hours: self.dark_hours(),
            pause_on_battery: self.pause_on_battery,
            pause_coverage: self.pause_coverage(),
        }
    }

    /// The pictures and folders that take turns on every display, whether
    /// they are shuffled and how long one stays, as `IDesktopWallpaper`
    /// tells of a slideshow: those of the collection shown, else `images`.
    pub fn slideshow(&self) -> (Vec<PathBuf>, bool, Duration) {
        let named = { self.collections.iter() }
            .find(|collection| Some(&collection.name) == self.collection.as_ref());
        let interval = |seconds: u64| Duration::from_secs(seconds.max(SHORTEST_INTERVAL));
        match named {
            Some(collection) => (
                collection.images.clone(),
                collection.shuffle.unwrap_or(self.shuffle),
                interval(collection.interval.or(self.interval).unwrap_or(INTERVAL)),
            ),
            None => (
                self.images.clone(),
                self.shuffle,
                interval(self.interval.unwrap_or(INTERVAL)),
            ),
        }
    }

    /// Whether the wallpaper is the color or the gradient alone.
    pub fn shows_color_alone(&self) -> bool {
        self.source() == Source::Color
    }

    /// Where the wallpaper is taken from, given or implied.
    pub fn source(&self) -> Source {
        self.source.unwrap_or_else(|| self.implied_source())
    }

    /// Whether `source` is written, not left to the other keys.
    pub fn names_source(&self) -> bool {
        self.source.is_some()
    }

    /// What the wallpaper settings show of this, as it is written: the
    /// values of the keys that are given, and what is used without them.
    pub fn shown(&self) -> Shown {
        let chosen = { self.collections.iter() }
            .find(|collection| self.collection.as_ref() == Some(&collection.name));
        let own = |has: fn(&Collection) -> bool| {
            chosen
                .filter(|collection| has(collection))
                .map(|collection| collection.name.clone())
        };
        let interval = chosen.and_then(|collection| collection.interval);
        Shown {
            source: self.source(),
            collections: { self.collections.iter() }
                .map(|collection| {
                    let first = collection.images.first().or(collection.video.as_ref());
                    let first = first.or(collection.videos.first());
                    (collection.name.clone(), first.cloned())
                })
                .collect(),
            collection: self.collection.clone(),
            interval: { interval.or(self.interval) }
                .unwrap_or(INTERVAL)
                .max(SHORTEST_INTERVAL),
            shuffle: { chosen.and_then(|collection| collection.shuffle) }.unwrap_or(self.shuffle),
            interval_in: own(|collection| collection.interval.is_some()),
            shuffle_in: own(|collection| collection.shuffle.is_some()),
            fit: self.fit,
            position: self.position,
            blur: self.blur.unwrap_or(0.),
            brightness: self.brightness.unwrap_or(1.),
            contrast: self.contrast.unwrap_or(1.),
            saturation: self.saturation.unwrap_or(1.),
            flip: self.flip.unwrap_or_default(),
            overlay: self.overlay.clone(),
            speed: self.speed.unwrap_or(1.),
            sound: self.sound.unwrap_or(false),
            volume: self.volume.unwrap_or(1.),
            pause_coverage: self.pause_coverage.unwrap_or(PAUSE_COVERAGE),
            displays: { self.displays.iter() }
                .map(|display| (display.number, display.collection.clone()))
                .collect(),
        }
    }

    /// The collection called `name`; one there is none of is reported,
    /// and `of` names the place that asks for it.
    fn collection_named(&self, name: Option<&str>, of: &str) -> Option<&Collection> {
        let name = name?;
        let found = self.collections.iter().find(|known| known.name == name);
        if found.is_none() {
            report!(
                "`{of}.collection` names `{name}`, which no [[wallpaper.collections]] is; its own pictures are shown"
            );
        }
        found
    }

    /// Where the wallpaper is taken from when `source` is not given.
    fn implied_source(&self) -> Source {
        let files = !self.images.is_empty()
            || !self.images_light.is_empty()
            || !self.images_dark.is_empty()
            || self.video.is_some()
            || !self.videos.is_empty()
            || self.collection.is_some();
        if files {
            Source::Files
        } else if self.daily.is_some() {
            Source::Daily
        } else if self.color.is_some() || self.gradient.is_some() {
            Source::Color
        } else {
            Source::Windows
        }
    }

    fn adjust_keys(&self) -> AdjustKeys {
        AdjustKeys {
            blur: self.blur,
            saturation: self.saturation,
            contrast: self.contrast,
            brightness: self.brightness,
            flip: self.flip,
            overlay: self.overlay.clone(),
            position: self.position,
        }
    }

    fn video_keys(&self) -> VideoKeys {
        VideoKeys {
            speed: self.speed,
            sound: self.sound,
            volume: self.volume,
        }
    }

    /// The share of a display windows cover at which its video stands
    /// still; one out of its range is reported and taken as the nearest.
    fn pause_coverage(&self) -> f32 {
        let Some(share) = self.pause_coverage.filter(|share| !share.is_nan()) else {
            return PAUSE_COVERAGE;
        };
        if !(0. ..=1.).contains(&share) {
            report!("`wallpaper.pause_coverage` goes from 0 to 1, {share} is taken as the nearest");
        }
        share.clamp(0., 1.)
    }

    /// What lies under the pictures; a color that is not one is reported
    /// and black.
    fn ground(&self) -> Ground {
        let color = |text: &str| {
            parse_rgb(text)
                .inspect_err(|error| report!("`wallpaper`: {error:#}, black is used"))
                .unwrap_or([0; 3])
        };
        match (&self.gradient, &self.color) {
            (Some([top, bottom]), _) => Ground::Gradient(color(top), color(bottom)),
            (None, Some(solid)) => Ground::Solid(color(solid)),
            (None, None) => Ground::default(),
        }
    }

    fn dark_hours(&self) -> Option<(u32, u32)> {
        let [from, until] = self.dark_hours.as_ref()?;
        match (minutes(from), minutes(until)) {
            (Some(from), Some(until)) => Some((from, until)),
            _ => {
                report!(
                    "`wallpaper.dark_hours` takes two times of day as \"19:00\", it follows Windows instead"
                );
                None
            }
        }
    }
}

impl Looks {
    /// What the display with the number `number` shows while it shows
    /// the workspace counted from 0 as `workspace`.
    pub fn of(&self, number: usize, workspace: Option<usize>) -> &Look {
        let on_workspace = workspace.and_then(|index| look_of(&self.workspaces, index + 1));
        on_workspace
            .or_else(|| look_of(&self.displays, number))
            .unwrap_or(&self.all)
    }

    /// Every look there is.
    fn every(&self) -> impl Iterator<Item = &Look> {
        let places = self.displays.iter().chain(&self.workspaces);
        std::iter::once(&self.all).chain(places.map(|(_, look)| look))
    }

    /// Whether some workspace has a look of its own.
    pub fn by_workspace(&self) -> bool {
        !self.workspaces.is_empty()
    }

    /// How long the pictures stay that take turns somewhere, each once.
    pub fn intervals(&self) -> Vec<Duration> {
        let mut intervals = Vec::new();
        for look in self.every() {
            if let Some(interval) = look.turns_every()
                && !intervals.contains(&interval)
            {
                intervals.push(interval);
            }
        }
        intervals
    }

    /// Whether the wallpaper of Windows is shown somewhere.
    pub fn shows_windows(&self) -> bool {
        self.every().any(Look::shows_windows)
    }

    /// Whether the wallpaper of Windows is shown somewhere other than the
    /// desktop window draws it, adjusted, so that Tessera draws it there.
    pub fn adjusts_windows(&self) -> bool {
        self.every().any(Look::adjusts_windows)
    }

    /// Where the picture of the day is shown from, if it is.
    pub fn daily(&self) -> Option<Daily> {
        self.every().find_map(|look| match look {
            Look::Daily { daily, .. } => Some(*daily),
            _ => None,
        })
    }
}

impl Look {
    /// How long a picture stays, where pictures take turns.
    fn turns_every(&self) -> Option<Duration> {
        match self {
            Self::Pictures {
                light,
                dark,
                turning,
                ..
            } => (light.len() > 1 || dark.len() > 1).then_some(turning.interval),
            Self::Videos { then, .. } => then.turns_every(),
            _ => None,
        }
    }

    fn shows_windows(&self) -> bool {
        match self {
            Self::Windows(_) | Self::Daily { .. } => true,
            Self::Pictures { light, dark, .. } => light.is_empty() || dark.is_empty(),
            Self::Videos { then, .. } => then.shows_windows(),
            Self::Fill(_) => false,
        }
    }

    fn adjusts_windows(&self) -> bool {
        match self {
            Self::Windows(adjust) | Self::Daily { adjust, .. } => !adjust.is_plain(),
            Self::Pictures {
                light,
                dark,
                adjust,
                ..
            } => (light.is_empty() || dark.is_empty()) && !adjust.is_plain(),
            Self::Videos { then, .. } => then.adjusts_windows(),
            Self::Fill(_) => false,
        }
    }

    /// What a display at `frame` with this look shows at `moment`.
    pub fn plan(&self, frame: ScreenRect, looks: &Looks, moment: &Moment) -> Plan {
        match self {
            Self::Windows(adjust) => windows_plan(frame, moment, adjust),
            Self::Fill(ground) => Plan::Fill(*ground),
            Self::Pictures {
                light,
                dark,
                fit,
                turning,
                adjust,
            } => {
                let list = if moment.dark { dark } else { light };
                if list.is_empty() {
                    return windows_plan(frame, moment, adjust);
                }
                let turn = moment.turns.turn(turning.interval);
                let at = pick(list.len(), turn, turning.shuffle, moment.seed);
                Plan::Picture {
                    file: list[at].clone(),
                    fit: *fit,
                    ground: looks.ground,
                    written: None,
                    own: true,
                    adjust: *adjust,
                }
            }
            Self::Videos {
                files,
                then,
                shuffle,
                adjust,
                sound,
            } => {
                let left: Vec<&PathBuf> = files
                    .iter()
                    .filter(|file| !moment.failed.contains(*file))
                    .collect();
                if left.is_empty() {
                    return then.plan(frame, looks, moment);
                }
                if !moment.videos {
                    // Nothing is opened until a video may be.
                    return Plan::Nothing;
                }
                let order = if *shuffle {
                    permutation(left.len(), moment.seed)
                } else {
                    (0..left.len()).collect()
                };
                Plan::Video(VideoPlan {
                    files: order.into_iter().map(|at| left[at].clone()).collect(),
                    adjust: adjust.for_video(),
                    sound: *sound,
                })
            }
            Self::Daily { fit, adjust, .. } => match moment.daily {
                Some(file) => Plan::Picture {
                    file: file.to_path_buf(),
                    fit: *fit,
                    ground: looks.ground,
                    written: None,
                    own: true,
                    adjust: *adjust,
                },
                None => windows_plan(frame, moment, adjust),
            },
        }
    }
}

/// The look of the place with the number `number` among `places`.
fn look_of(places: &[(usize, Look)], number: usize) -> Option<&Look> {
    let found = places.iter().find(|(of, _)| *of == number);
    found.map(|(_, look)| look)
}

/// What a display at `frame` shows of the wallpaper of Windows adjusted
/// as `adjust` says: nothing of Tessera's where the desktop window draws
/// it and it is not to look otherwise, or until it was read.
fn windows_plan(frame: ScreenRect, moment: &Moment, adjust: &Adjustments) -> Plan {
    let Some(now) = moment.windows else {
        return Plan::Nothing;
    };
    if moment.painted && adjust.is_plain() {
        return Plan::Nothing;
    }
    let ground = Ground::Solid(now.wallpaper.color);
    match now.wallpaper.picture_of(frame) {
        Some(file) => Plan::Picture {
            file: file.to_path_buf(),
            fit: Fit::of_windows(now.wallpaper.position),
            ground,
            written: now.written.get(file).copied(),
            own: false,
            adjust: *adjust,
        },
        // A color is not adjusted: the desktop window draws it as well.
        None if moment.painted => Plan::Nothing,
        None => Plan::Fill(ground),
    }
}

/// The picture files `named` are, those of a directory in the order of
/// their names. What cannot be read is reported.
pub fn pictures_in(named: &[PathBuf]) -> Vec<PathBuf> {
    let mut pictures = Vec::new();
    for path in named {
        if !path.is_dir() {
            pictures.push(path.clone());
            continue;
        }
        match std::fs::read_dir(path) {
            Ok(entries) => {
                let mut found: Vec<PathBuf> = entries
                    .filter_map(|entry| Some(entry.ok()?.path()))
                    .filter(|path| is_picture(path))
                    .collect();
                found.sort();
                pictures.extend(found);
            }
            Err(error) => report!(
                "the wallpaper directory {} cannot be read: {error}",
                path.display()
            ),
        }
    }
    pictures
}

/// Whether the name of `path` ends as a picture file's does.
pub fn is_picture(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        KINDS
            .iter()
            .any(|kind| extension.eq_ignore_ascii_case(kind))
    })
}

/// The minute of the day `text`, a time as `19:30`, stands for.
fn minutes(text: &str) -> Option<u32> {
    let (hours, minutes) = text.trim().split_once(':')?;
    let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

/// Whether the minute of the day `now` lies in the dark hours that go from
/// the minute `from` to the minute `until`, over midnight where `until`
/// comes first.
pub fn is_dark(now: u32, (from, until): (u32, u32)) -> bool {
    if from <= until {
        (from..until).contains(&now)
    } else {
        now >= from || now < until
    }
}

/// Which of `length` pictures is shown at the `turn`th change: one after
/// the other, or with `shuffle` in an order drawn from `seed` that is
/// drawn anew each time all were shown. A `turn` below 0 counts back
/// from the first, to the last and on.
pub fn pick(length: usize, turn: i64, shuffle: bool, seed: u64) -> usize {
    let count = i64::try_from(length.max(1)).unwrap_or(i64::MAX);
    let at = turn.rem_euclid(count) as usize;
    if !shuffle {
        return at;
    }
    let round = turn.div_euclid(count) as u64;
    permutation(
        length.max(1),
        seed ^ round.wrapping_mul(0x9E37_79B9_7F4A_7C15),
    )[at]
}

/// The numbers from 0 to `length` in an order drawn from `seed`.
pub fn permutation(length: usize, seed: u64) -> Vec<usize> {
    let mut state = seed;
    // SplitMix64: enough for an order nobody guesses.
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut mixed = state;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ (mixed >> 31)
    };
    let mut order: Vec<usize> = (0..length).collect();
    for at in (1..length).rev() {
        let other = (next() % (at as u64 + 1)) as usize;
        order.swap(at, other);
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> WallpaperConfig {
        toml::from_str(text).unwrap()
    }

    fn frame(left: i32) -> ScreenRect {
        ScreenRect {
            left,
            top: 0,
            right: left + 100,
            bottom: 100,
        }
    }

    /// No picture has changed yet.
    static NO_TURNS: Turns = Turns { counts: Vec::new() };

    fn moment<'a>(failed: &'a HashSet<PathBuf>) -> Moment<'a> {
        Moment {
            dark: false,
            turns: &NO_TURNS,
            seed: 7,
            windows: None,
            painted: true,
            daily: None,
            failed,
            videos: true,
        }
    }

    fn file(plan: &Plan) -> Option<&Path> {
        match plan {
            Plan::Picture { file, .. } => Some(file),
            _ => None,
        }
    }

    #[test]
    fn a_display_with_pictures_of_its_own_does_not_take_the_others() {
        let looks = config(
            r#"
            images = ['C:\all.png']
            [[display]]
            number = 2
            images = ['C:\two.png', 'C:\more.png']
            [[display]]
            number = 3
            "#,
        )
        .looks();
        let failed = HashSet::new();
        let now = moment(&failed);
        let plan = |number| looks.of(number, None).plan(frame(0), &looks, &now);
        assert_eq!(file(&plan(1)), Some(Path::new(r"C:\all.png")));
        assert_eq!(file(&plan(2)), Some(Path::new(r"C:\two.png")));
        // An entry without pictures shows the wallpaper of Windows, which
        // the desktop window draws here.
        assert_eq!(plan(3), Plan::Nothing);
        assert_eq!(looks.intervals(), [Duration::from_secs(INTERVAL)]);
    }

    #[test]
    fn a_workspace_with_a_look_of_its_own_wins_over_its_display() {
        let looks = config(
            r#"
            images = ['C:\all.png']
            [[display]]
            number = 1
            images = ['C:\one.png']
            [[workspace]]
            number = 2
            images = ['C:\work.png']
            fit = "center"
            "#,
        )
        .looks();
        let failed = HashSet::new();
        let now = moment(&failed);
        let plan = |workspace| looks.of(1, workspace).plan(frame(0), &looks, &now);
        assert_eq!(file(&plan(Some(0))), Some(Path::new(r"C:\one.png")));
        let on_second = plan(Some(1));
        assert_eq!(file(&on_second), Some(Path::new(r"C:\work.png")));
        assert!(matches!(
            on_second,
            Plan::Picture {
                fit: Fit::Center,
                ..
            }
        ));
        assert!(looks.by_workspace());
        assert!(looks.intervals().is_empty());
    }

    #[test]
    fn the_dark_pictures_replace_the_others_while_it_is_dark() {
        let looks = config(
            r#"
            images = ['C:\day.png']
            images_dark = ['C:\night.png']
            dark_hours = ["19:00", "07:30"]
            "#,
        )
        .looks();
        let failed = HashSet::new();
        let mut now = moment(&failed);
        let plan = |now: &Moment| looks.of(1, None).plan(frame(0), &looks, now);
        assert_eq!(file(&plan(&now)), Some(Path::new(r"C:\day.png")));
        now.dark = true;
        assert_eq!(file(&plan(&now)), Some(Path::new(r"C:\night.png")));
        assert_eq!(looks.dark_hours, Some((19 * 60, 7 * 60 + 30)));
    }

    #[test]
    fn dark_hours_may_go_over_midnight() {
        let night = (19 * 60, 7 * 60);
        assert!(is_dark(23 * 60, night));
        assert!(is_dark(3 * 60, night));
        assert!(!is_dark(12 * 60, night));
        assert!(!is_dark(7 * 60, night));
        let afternoon = (13 * 60, 15 * 60);
        assert!(is_dark(14 * 60, afternoon));
        assert!(!is_dark(16 * 60, afternoon));
        assert_eq!(minutes("7:05"), Some(425));
        assert_eq!(minutes("24:00"), None);
        assert_eq!(minutes("evening"), None);
    }

    #[test]
    fn videos_that_failed_give_way_to_the_pictures() {
        let looks = config(
            r#"
            images = ['C:\still.png']
            video = 'C:\one.mp4'
            videos = ['C:\two.mp4']
            "#,
        )
        .looks();
        let mut failed = HashSet::new();
        let plan =
            |failed: &HashSet<PathBuf>| looks.of(1, None).plan(frame(0), &looks, &moment(failed));
        let videos = |files: &[&str]| {
            Plan::Video(VideoPlan {
                files: files.iter().map(PathBuf::from).collect(),
                adjust: Adjustments::default(),
                sound: VideoSound::default(),
            })
        };
        assert_eq!(plan(&failed), videos(&[r"C:\one.mp4", r"C:\two.mp4"]));
        failed.insert(PathBuf::from(r"C:\one.mp4"));
        assert_eq!(plan(&failed), videos(&[r"C:\two.mp4"]));
        failed.insert(PathBuf::from(r"C:\two.mp4"));
        assert_eq!(file(&plan(&failed)), Some(Path::new(r"C:\still.png")));
        // Before videos may be opened, nothing is.
        let none = HashSet::new();
        let mut early = moment(&none);
        early.videos = false;
        assert_eq!(
            looks.of(1, None).plan(frame(0), &looks, &early),
            Plan::Nothing
        );
    }

    #[test]
    fn the_source_follows_from_what_is_given() {
        let failed = HashSet::new();
        let now = moment(&failed);
        let plan = |text: &str| {
            let looks = config(text).looks();
            looks.of(1, None).plan(frame(0), &looks, &now)
        };
        assert_eq!(plan(""), Plan::Nothing);
        assert_eq!(
            plan("color = \"#102030\""),
            Plan::Fill(Ground::Solid([0x10, 0x20, 0x30]))
        );
        assert_eq!(
            plan("gradient = [\"#000000\", \"#ffffff\"]"),
            Plan::Fill(Ground::Gradient([0; 3], [255; 3]))
        );
        // Without a picture yet, the picture of the day shows Windows.
        assert_eq!(plan("daily = \"bing\""), Plan::Nothing);
        assert_eq!(
            config("daily = \"wikimedia\"").looks().daily(),
            Some(Daily::Wikimedia)
        );
        assert_eq!(
            config("source = \"windows\"\nimages = ['a.png']")
                .looks()
                .daily(),
            None
        );
        assert!(config("").looks().shows_windows());
        assert!(!config("color = \"#000000\"").looks().shows_windows());
    }

    #[test]
    fn the_wallpaper_of_windows_is_drawn_only_where_the_desktop_does_not() {
        let looks = config("source = \"windows\"").looks();
        let failed = HashSet::new();
        let windows = WindowsNow {
            wallpaper: SystemWallpaper {
                pictures: vec![
                    (frame(0), Some(PathBuf::from(r"C:\w.jpg"))),
                    (frame(100), None),
                ],
                position: crate::platform::WallpaperPosition::Tile,
                color: [1, 2, 3],
            },
            written: HashMap::new(),
        };
        let mut now = moment(&failed);
        now.windows = Some(&windows);
        assert_eq!(
            looks.of(1, None).plan(frame(0), &looks, &now),
            Plan::Nothing
        );
        now.painted = false;
        assert_eq!(
            looks.of(1, None).plan(frame(0), &looks, &now),
            Plan::Picture {
                file: PathBuf::from(r"C:\w.jpg"),
                fit: Fit::Tile,
                ground: Ground::Solid([1, 2, 3]),
                written: None,
                own: false,
                adjust: Adjustments::default(),
            }
        );
        assert_eq!(
            looks.of(2, None).plan(frame(100), &looks, &now),
            Plan::Fill(Ground::Solid([1, 2, 3]))
        );
        // Adjusted, Tessera draws it over the desktop window too.
        let blurred = config("source = \"windows\"\nblur = 12").looks();
        assert!(blurred.adjusts_windows() && !looks.adjusts_windows());
        now.painted = true;
        let plan = blurred.of(1, None).plan(frame(0), &blurred, &now);
        assert!(matches!(plan, Plan::Picture { own: false, adjust, .. } if adjust.blur == 12.));
        assert_eq!(
            blurred.of(2, None).plan(frame(100), &blurred, &now),
            Plan::Nothing
        );
    }

    #[test]
    fn a_collection_is_chosen_by_its_name_everywhere() {
        let looks = config(
            r#"
            images = ['C:\all.png']
            collection = "sea"
            interval = 60
            [[collections]]
            name = "sea"
            images = ['C:\wave.png', 'C:\shore.png']
            shuffle = false
            [[collections]]
            name = "hills"
            images = ['C:\hill.png', 'C:\valley.png']
            interval = 30
            [[display]]
            number = 2
            collection = "hills"
            brightness = 0.5
            [[workspace]]
            number = 3
            collection = "nowhere"
            images = ['C:\own.png']
            "#,
        )
        .looks();
        let failed = HashSet::new();
        let now = moment(&failed);
        let plan = |number, workspace| looks.of(number, workspace).plan(frame(0), &looks, &now);
        assert_eq!(file(&plan(1, None)), Some(Path::new(r"C:\wave.png")));
        let second = plan(2, None);
        assert_eq!(file(&second), Some(Path::new(r"C:\hill.png")));
        assert!(matches!(second, Plan::Picture { adjust, .. } if adjust.brightness == 0.5));
        // A name no collection has leaves the entry its own pictures.
        assert_eq!(file(&plan(1, Some(2))), Some(Path::new(r"C:\own.png")));
        let mut intervals = looks.intervals();
        intervals.sort();
        assert_eq!(
            intervals,
            [Duration::from_secs(30), Duration::from_secs(60)]
        );
    }

    #[test]
    fn pictures_of_each_interval_take_turns_on_their_own() {
        let (minute, hour) = (Duration::from_secs(60), Duration::from_secs(3600));
        let start = Instant::now();
        let mut turns = Turns::default();
        turns.tick(&[minute, hour], start);
        assert_eq!((turns.turn(minute), turns.turn(hour)), (0, 0));
        turns.tick(&[minute, hour], start + Duration::from_secs(61));
        assert_eq!((turns.turn(minute), turns.turn(hour)), (1, 0));
        turns.step(-2, start + Duration::from_secs(62));
        assert_eq!((turns.turn(minute), turns.turn(hour)), (-1, -2));
        // The interval begins anew at the step.
        turns.tick(&[minute, hour], start + Duration::from_secs(100));
        assert_eq!(turns.turn(minute), -1);
        assert_eq!(turns.turn(Duration::from_secs(5)), 0);
    }

    #[test]
    fn every_key_the_documentation_shows_is_taken() {
        let full = config(
            r##"
            collection = "Sea"
            blur = 8
            brightness = 0.9
            contrast = 1.1
            saturation = 1.2
            flip = "horizontal"
            overlay = { color = "#00000060", blend = "multiply" }
            position = "top"
            speed = 1.5
            sound = true
            volume = 0.3
            pause_coverage = 0.9
            [[collections]]
            name = "Sea"
            images = ["C:/Users/me/Pictures/Sea"]
            video = "C:/Users/me/Videos/waves.mp4"
            interval = 300
            shuffle = true
            [[display]]
            number = 2
            collection = "Sea"
            blur = 0
            position = [30, 70]
            "##,
        );
        let shown = full.shown();
        assert_eq!(shown.collection.as_deref(), Some("Sea"));
        assert_eq!(shown.blur, 8.);
        assert_eq!(shown.flip, Flip::Horizontal);
        assert_eq!(shown.displays, [(2, Some("Sea".to_owned()))]);
        assert_eq!(shown.source, Source::Files);
        // The collection chosen for all has its own turns: they are shown.
        assert_eq!(shown.interval, 300);
        assert!(shown.shuffle);
        assert_eq!(shown.interval_in.as_deref(), Some("Sea"));
        assert_eq!(shown.shuffle_in.as_deref(), Some("Sea"));
        let (slideshow, shuffle, interval) = full.slideshow();
        assert_eq!(slideshow, [PathBuf::from("C:/Users/me/Pictures/Sea")]);
        assert!(shuffle);
        assert_eq!(interval, Duration::from_secs(300));
        assert!(toml::from_str::<WallpaperConfig>("overlay = { colour = \"#000000\" }").is_err());
        assert!(
            toml::from_str::<WallpaperConfig>("[[collections]]\nname = \"a\"\nfit = \"tile\"")
                .is_err()
        );
    }

    #[test]
    fn the_keys_of_videos_are_kept_within_their_range() {
        let looks = config(
            r#"
            video = 'C:\one.mp4'
            speed = 9
            sound = true
            volume = 0.4
            flip = "horizontal"
            blur = 3
            pause_coverage = 0.5
            "#,
        )
        .looks();
        let failed = HashSet::new();
        let Plan::Video(plan) = looks.of(1, None).plan(frame(0), &looks, &moment(&failed)) else {
            panic!("no video");
        };
        assert_eq!(plan.sound.speed, FASTEST);
        assert!(plan.sound.sound);
        assert_eq!(plan.sound.volume, 0.4);
        // A video is mirrored, but not blurred.
        assert_eq!(plan.adjust.flip, Flip::Horizontal);
        assert_eq!(plan.adjust.blur, 0.);
        assert_eq!(looks.pause_coverage, 0.5);
        assert_eq!(config("").looks().pause_coverage, PAUSE_COVERAGE);
    }

    #[test]
    fn a_shuffled_list_shows_every_picture_once_a_round() {
        for seed in [0, 1, 99, u64::MAX] {
            let round: HashSet<usize> = (0..5).map(|turn| pick(5, turn, true, seed)).collect();
            assert_eq!(round.len(), 5);
            let next: HashSet<usize> = (5..10).map(|turn| pick(5, turn, true, seed)).collect();
            assert_eq!(next.len(), 5);
        }
        assert_eq!(pick(3, 4, false, 0), 1);
        assert_eq!(pick(0, 4, true, 0), 0);
        let before: HashSet<usize> = (-5..0).map(|turn| pick(5, turn, true, 3)).collect();
        assert_eq!(before.len(), 5);
        let mut order = permutation(6, 42);
        order.sort_unstable();
        assert_eq!(order, [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn going_back_from_the_first_picture_shows_the_last() {
        for length in [1, 2, 3, 5, 7] {
            assert_eq!(pick(length, -1, false, 0), length - 1);
            assert_eq!(pick(length, 0, false, 0), 0);
        }
        assert_eq!(pick(3, -4, false, 0), 2);
    }

    #[test]
    fn pictures_are_known_by_their_extension_in_any_case() {
        assert!(is_picture(Path::new(r"C:\Pictures\sea.JPG")));
        assert!(is_picture(Path::new("hill.png")));
        assert!(is_picture(Path::new("hill.webp")));
        assert!(is_picture(Path::new("phone.HEIC")));
        assert!(!is_picture(Path::new("notes.txt")));
        assert!(!is_picture(Path::new("png")));
    }

    #[test]
    fn a_directory_gives_its_pictures_in_the_order_of_their_names() {
        let directory =
            std::env::temp_dir().join(format!("tessera-wallpaper-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        for name in ["b.png", "a.jpg", "c.txt"] {
            std::fs::write(directory.join(name), []).unwrap();
        }
        let single = PathBuf::from(r"C:\elsewhere\one.png");
        let pictures = pictures_in(&[single.clone(), directory.clone()]);
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(
            pictures,
            [single, directory.join("a.jpg"), directory.join("b.png")]
        );
    }

    #[test]
    fn a_configuration_with_unknown_keys_is_refused() {
        assert!(toml::from_str::<WallpaperConfig>("pictures = []").is_err());
        assert!(toml::from_str::<WallpaperConfig>("[[workspace]]\nnumber = 1\nsize = 2").is_err());
        let full = config(
            r##"
            source = "files"
            images = ["a.png"]
            fit = "span"
            color = "#000000"
            shuffle = true
            mirror = true
            pause_on_battery = true
            videos = ["b.mp4"]
            [[display]]
            number = 1
            video = "c.mp4"
            fit = "tile"
            "##,
        );
        assert!(full.mirror);
        assert!(full.looks().pause_on_battery);
    }
}
