// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the configuration asks each display to show ([`Look`]), and what
//! that comes to at one moment ([`Plan`]): the workspace a display shows,
//! whether it is dark, which picture of a list is due and what Windows
//! has as its wallpaper decide it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use super::compose::{Fit, Ground};
use super::daily::Daily;
use crate::platform::{ScreenRect, SystemWallpaper};
use crate::theme::parse_rgb;

/// How long a picture stays unless the configuration says, in seconds.
const INTERVAL: u64 = 600;

/// The shortest time a picture stays, in seconds: reading the next one
/// takes time and memory.
const SHORTEST_INTERVAL: u64 = 5;

/// The kinds of file taken from a directory, by extension: those Tessera
/// reads itself, and those a codec of Windows may read.
const KINDS: [&str; 14] = [
    "jpg", "jpeg", "png", "bmp", "gif", "tif", "tiff", "webp", "heic", "heif", "avif", "jxr",
    "wdp", "jfif",
];

/// Where the wallpaper is taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
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
    /// far left.
    number: usize,
    /// Picture files and directories, as `images` of the wallpaper.
    #[serde(default)]
    images: Vec<PathBuf>,
    /// A video, as `video` of the wallpaper.
    video: Option<PathBuf>,
    /// Videos, as `videos` of the wallpaper.
    #[serde(default)]
    videos: Vec<PathBuf>,
    /// How its pictures are fitted, in place of `fit` of the wallpaper.
    fit: Option<Fit>,
}

/// What a display shows as the configuration says, before the moment
/// decides on it.
#[derive(Clone, Debug, PartialEq)]
pub enum Look {
    /// The wallpaper of Windows.
    Windows,
    /// A color or a gradient alone.
    Fill(Ground),
    /// Pictures that take turns, the first list while Windows is light
    /// and the second while it is dark.
    Pictures {
        light: Vec<PathBuf>,
        dark: Vec<PathBuf>,
        fit: Fit,
    },
    /// Videos one after the other, and what is shown where none of them
    /// can be played.
    Videos {
        files: Vec<PathBuf>,
        then: Box<Look>,
    },
    /// A picture of the day.
    Daily { daily: Daily, fit: Fit },
}

/// What every display shows as the configuration says.
#[derive(Clone, Debug, PartialEq)]
pub struct Looks {
    all: Look,
    displays: Vec<(usize, Look)>,
    workspaces: Vec<(usize, Look)>,
    /// What lies around a picture that does not cover its display.
    pub ground: Ground,
    pub interval: Duration,
    pub shuffle: bool,
    /// From when to when it is dark, in minutes of the day; `None` for as
    /// long as Windows is dark.
    pub dark_hours: Option<(u32, u32)>,
    pub pause_on_battery: bool,
}

/// What decides on the looks at one moment.
pub struct Moment<'a> {
    /// Whether the dark pictures are shown.
    pub dark: bool,
    /// How many times the pictures have changed.
    pub turn: usize,
    /// What the random order of the pictures and the videos comes from.
    pub seed: u64,
    /// The wallpaper of Windows, where it was read.
    pub windows: Option<&'a WindowsNow>,
    /// Whether the desktop window draws the wallpaper of Windows itself.
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
    },
    /// A color or a gradient.
    Fill(Ground),
    /// Videos, in the order they are played.
    Video(Vec<PathBuf>),
}

impl WallpaperConfig {
    /// What the configuration has every display show. Directories are
    /// read for this and what cannot be used is reported, so it is not
    /// for the thread of the interface.
    pub fn looks(&self) -> Looks {
        let ground = self.ground();
        let pictures = |light: Vec<PathBuf>, dark: Vec<PathBuf>, fit: Fit| {
            if light.is_empty() && dark.is_empty() {
                None
            } else {
                Some(Look::Pictures { light, dark, fit })
            }
        };
        let with_videos = |video: &Option<PathBuf>, videos: &[PathBuf], then: Look| {
            let files: Vec<PathBuf> = video.iter().chain(videos).cloned().collect();
            if files.is_empty() {
                then
            } else {
                Look::Videos {
                    files,
                    then: Box::new(then),
                }
            }
        };
        let images = pictures_in(&self.images);
        let light = self.images_light.is_empty().then(|| images.clone());
        let light = light.unwrap_or_else(|| pictures_in(&self.images_light));
        let dark = self.images_dark.is_empty().then(|| images.clone());
        let dark = dark.unwrap_or_else(|| pictures_in(&self.images_dark));
        let source = self.source.unwrap_or_else(|| self.implied_source());
        let all = match source {
            Source::Windows => Look::Windows,
            Source::Color => Look::Fill(ground),
            Source::Daily => Look::Daily {
                daily: self.daily.unwrap_or_default(),
                fit: self.fit,
            },
            Source::Files => {
                let unfilled = if self.color.is_some() || self.gradient.is_some() {
                    Look::Fill(ground)
                } else {
                    Look::Windows
                };
                let shown = pictures(light, dark, self.fit).unwrap_or(unfilled);
                with_videos(&self.video, &self.videos, shown)
            }
        };
        let place = |place: &PlaceWallpaper| {
            let images = pictures_in(&place.images);
            let fit = place.fit.unwrap_or(self.fit);
            let shown = pictures(images.clone(), images, fit).unwrap_or(Look::Windows);
            (
                place.number,
                with_videos(&place.video, &place.videos, shown),
            )
        };
        Looks {
            all,
            displays: self.displays.iter().map(place).collect(),
            workspaces: self.workspaces.iter().map(place).collect(),
            ground,
            interval: self.interval(),
            shuffle: self.shuffle,
            dark_hours: self.dark_hours(),
            pause_on_battery: self.pause_on_battery,
        }
    }

    /// Where the wallpaper is taken from when `source` is not given.
    fn implied_source(&self) -> Source {
        let files = !self.images.is_empty()
            || !self.images_light.is_empty()
            || !self.images_dark.is_empty()
            || self.video.is_some()
            || !self.videos.is_empty();
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

    fn interval(&self) -> Duration {
        let seconds = self.interval.unwrap_or(INTERVAL);
        if seconds < SHORTEST_INTERVAL {
            report!(
                "`wallpaper.interval` is under {SHORTEST_INTERVAL} seconds, which it is taken as"
            );
        }
        Duration::from_secs(seconds.max(SHORTEST_INTERVAL))
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

    /// Whether pictures take turns somewhere.
    pub fn rotates(&self) -> bool {
        self.every().any(Look::rotates)
    }

    /// Whether the wallpaper of Windows is shown somewhere.
    pub fn shows_windows(&self) -> bool {
        self.every().any(Look::shows_windows)
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
    fn rotates(&self) -> bool {
        match self {
            Self::Pictures { light, dark, .. } => light.len() > 1 || dark.len() > 1,
            Self::Videos { then, .. } => then.rotates(),
            _ => false,
        }
    }

    fn shows_windows(&self) -> bool {
        match self {
            Self::Windows | Self::Daily { .. } => true,
            Self::Pictures { light, dark, .. } => light.is_empty() || dark.is_empty(),
            Self::Videos { then, .. } => then.shows_windows(),
            Self::Fill(_) => false,
        }
    }

    /// What a display at `frame` with this look shows at `moment`.
    pub fn plan(&self, frame: ScreenRect, looks: &Looks, moment: &Moment) -> Plan {
        match self {
            Self::Windows => windows_plan(frame, moment),
            Self::Fill(ground) => Plan::Fill(*ground),
            Self::Pictures { light, dark, fit } => {
                let list = if moment.dark { dark } else { light };
                if list.is_empty() {
                    return windows_plan(frame, moment);
                }
                let at = pick(list.len(), moment.turn, looks.shuffle, moment.seed);
                Plan::Picture {
                    file: list[at].clone(),
                    fit: *fit,
                    ground: looks.ground,
                    written: None,
                    own: true,
                }
            }
            Self::Videos { files, then } => {
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
                let order = if looks.shuffle {
                    permutation(left.len(), moment.seed)
                } else {
                    (0..left.len()).collect()
                };
                Plan::Video(order.into_iter().map(|at| left[at].clone()).collect())
            }
            Self::Daily { fit, .. } => match moment.daily {
                Some(file) => Plan::Picture {
                    file: file.to_path_buf(),
                    fit: *fit,
                    ground: looks.ground,
                    written: None,
                    own: true,
                },
                None => windows_plan(frame, moment),
            },
        }
    }
}

/// The look of the place with the number `number` among `places`.
fn look_of(places: &[(usize, Look)], number: usize) -> Option<&Look> {
    let found = places.iter().find(|(of, _)| *of == number);
    found.map(|(_, look)| look)
}

/// What a display at `frame` shows of the wallpaper of Windows: nothing
/// of Tessera's where the desktop window draws it, or until it was read.
fn windows_plan(frame: ScreenRect, moment: &Moment) -> Plan {
    let Some(now) = moment.windows.filter(|_| !moment.painted) else {
        return Plan::Nothing;
    };
    let ground = Ground::Solid(now.wallpaper.color);
    match now.wallpaper.picture_of(frame) {
        Some(file) => Plan::Picture {
            file: file.to_path_buf(),
            fit: Fit::of_windows(now.wallpaper.position),
            ground,
            written: now.written.get(file).copied(),
            own: false,
        },
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
/// drawn anew each time all were shown.
pub fn pick(length: usize, turn: usize, shuffle: bool, seed: u64) -> usize {
    let length = length.max(1);
    if !shuffle {
        return turn % length;
    }
    let round = (turn / length) as u64;
    permutation(length, seed ^ round.wrapping_mul(0x9E37_79B9_7F4A_7C15))[turn % length]
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

    fn moment<'a>(failed: &'a HashSet<PathBuf>) -> Moment<'a> {
        Moment {
            dark: false,
            turn: 0,
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
        assert!(looks.rotates());
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
        assert!(!looks.rotates());
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
        assert_eq!(
            plan(&failed),
            Plan::Video(vec![
                PathBuf::from(r"C:\one.mp4"),
                PathBuf::from(r"C:\two.mp4")
            ])
        );
        failed.insert(PathBuf::from(r"C:\one.mp4"));
        assert_eq!(
            plan(&failed),
            Plan::Video(vec![PathBuf::from(r"C:\two.mp4")])
        );
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
            }
        );
        assert_eq!(
            looks.of(2, None).plan(frame(100), &looks, &now),
            Plan::Fill(Ground::Solid([1, 2, 3]))
        );
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
        let mut order = permutation(6, 42);
        order.sort_unstable();
        assert_eq!(order, [0, 1, 2, 3, 4, 5]);
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
