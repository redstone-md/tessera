// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A wallpaper of Tessera's own: pictures shown behind the icons of the
//! desktop on every display, one fading into the next, or a video that
//! plays there again and again.
//!
//! The wallpaper of Windows is left as it is and shows again when Tessera
//! exits, because the pictures are drawn in windows of Tessera that sit
//! over it. Those are plain windows, not ones of the user interface: a
//! picture is kept once for each display, at the size of the display, and
//! a video is drawn by Windows.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui::{App, AsyncApp, Global};
use image::RgbImage;
use serde::Deserialize;

use crate::motion::{Spring, SpringSpec};
use crate::platform::{
    AppWindow, BackdropPicture, DesktopBackdrop, DesktopVideo, ScreenRect, VideoState,
    display_frames,
};
use crate::theme::Theme;

/// How long a picture stays unless the configuration says, in seconds.
const INTERVAL: u64 = 600;

/// The shortest time a picture stays, in seconds: reading the next one
/// takes time and memory.
const SHORTEST_INTERVAL: u64 = 5;

/// How long one step of a fade stays on the screen.
const FRAME: Duration = Duration::from_millis(16);

/// How often the displays are looked at for a change while a picture
/// stays.
const DISPLAY_CHECK: Duration = Duration::from_secs(2);

/// How often a video is asked how far it has come while it is read.
const VIDEO_CHECK: Duration = Duration::from_millis(50);

/// How long a video may take to have its first frame.
const VIDEO_PATIENCE: Duration = Duration::from_secs(15);

/// The kinds of file taken from a directory, by extension.
const KINDS: [&str; 3] = ["jpg", "jpeg", "png"];

/// What the configuration says about the wallpaper.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WallpaperConfig {
    /// Picture files, and directories whose picture files are taken, in
    /// the order of their names. Without any, Tessera draws no wallpaper.
    #[serde(default)]
    images: Vec<PathBuf>,
    /// How many seconds a picture stays before the next one.
    interval: Option<u64>,
    /// A video file to play instead of the pictures, which are shown
    /// where it cannot be played.
    video: Option<PathBuf>,
}

impl WallpaperConfig {
    fn interval(&self) -> Duration {
        let seconds = self.interval.unwrap_or(INTERVAL);
        if seconds < SHORTEST_INTERVAL {
            report!(
                "`wallpaper.interval` is under {SHORTEST_INTERVAL} seconds, which it is taken as"
            );
        }
        Duration::from_secs(seconds.max(SHORTEST_INTERVAL))
    }

    /// The picture files the configuration names, those of a directory in
    /// the order of their names. What cannot be read is reported.
    fn pictures(&self) -> Vec<PathBuf> {
        let mut pictures = Vec::new();
        for path in &self.images {
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
}

/// Whether the name of `path` ends as a picture file's does.
fn is_picture(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        KINDS
            .iter()
            .any(|kind| extension.eq_ignore_ascii_case(kind))
    })
}

/// The pixels of the picture in the file `path`.
fn read(path: &Path) -> anyhow::Result<RgbImage> {
    let image = image::open(path).with_context(|| format!("{} cannot be read", path.display()))?;
    if image.width() == 0 || image.height() == 0 {
        anyhow::bail!("{} is an empty picture", path.display());
    }
    Ok(image.into_rgb8())
}

/// For each of `to` pixels in a row that `from` pixels are scaled to: the
/// first of the pixels it is mixed from, and the share of that one and of
/// each after it. The shares fall off evenly with the distance and sum
/// to 1.
fn shares(from: u32, to: u32) -> Vec<(usize, Vec<f32>)> {
    let ratio = from as f32 / to as f32;
    // How far a pixel reaches: over its neighbours, and when the row gets
    // shorter over all the pixels that go into one.
    let reach = ratio.max(1.);
    (0..to)
        .map(|index| {
            let centre = (index as f32 + 0.5) * ratio;
            let first = (centre - reach).floor().max(0.) as usize;
            let end = ((centre + reach).ceil() as usize).clamp(first + 1, from as usize);
            let mut shares: Vec<f32> = (first..end)
                .map(|source| (1. - ((source as f32 + 0.5 - centre) / reach).abs()).max(0.))
                .collect();
            let sum: f32 = shares.iter().sum();
            if sum > 0. {
                shares.iter_mut().for_each(|share| *share /= sum);
            } else {
                let even = 1. / shares.len() as f32;
                shares.fill(even);
            }
            (first, shares)
        })
        .collect()
}

/// `image` scaled so that it just covers `width` by `height` pixels, and
/// what then sticks out cut off evenly at both sides.
///
/// The scaling is done here, a few rows at a time: that of the `image`
/// crate first makes a copy of the whole picture that is four times its
/// size in memory.
fn fit(image: &RgbImage, width: u32, height: u32) -> BackdropPicture {
    let (left, top, wide, high) = covering_part(image.width(), image.height(), width, height);
    let across = shares(wide, width);
    let down = shares(high, height);
    let source = image.as_raw();
    let source_row = image.width() as usize * 3;
    let row = width as usize * 3;
    let stride = BackdropPicture::stride(width);
    let mut pixels = vec![0; stride * height as usize];
    // Rows of the source scaled across, with their number, kept as long
    // as a row of the picture is still mixed from them.
    let mut scaled: VecDeque<(usize, Vec<f32>)> = VecDeque::new();
    let mut mixed = vec![0.; row];
    for ((first, shares), out) in down.iter().zip(pixels.chunks_exact_mut(stride)) {
        while scaled.front().is_some_and(|(number, _)| number < first) {
            scaled.pop_front();
        }
        let mut next = scaled.back().map_or(*first, |(number, _)| number + 1);
        while next < first + shares.len() {
            let start = (top as usize + next) * source_row + left as usize * 3;
            let line = &source[start..start + wide as usize * 3];
            let mut wide_row = vec![0.; row];
            for ((first, shares), to) in across.iter().zip(wide_row.as_chunks_mut::<3>().0) {
                for (share, from) in shares.iter().zip(line[first * 3..].as_chunks::<3>().0) {
                    // Windows expects blue first.
                    to[0] += share * f32::from(from[2]);
                    to[1] += share * f32::from(from[1]);
                    to[2] += share * f32::from(from[0]);
                }
            }
            scaled.push_back((next, wide_row));
            next += 1;
        }
        mixed.fill(0.);
        for (share, (_, line)) in shares.iter().zip(&scaled) {
            for (to, from) in mixed.iter_mut().zip(line) {
                *to += share * from;
            }
        }
        for (to, from) in out.iter_mut().zip(&mixed) {
            *to = from.round().clamp(0., 255.) as u8;
        }
    }
    BackdropPicture::new(width, height, pixels)
}

/// The part of a picture of `width` by `height` pixels that has the shape
/// of `over_width` by `over_height` and is as large as can be, in the
/// middle of the picture: its left, top, width and height.
fn covering_part(
    width: u32,
    height: u32,
    over_width: u32,
    over_height: u32,
) -> (u32, u32, u32, u32) {
    let (wide, high) = (u64::from(width), u64::from(height));
    let (over_wide, over_high) = (u64::from(over_width.max(1)), u64::from(over_height.max(1)));
    if wide * over_high > high * over_wide {
        // Wider in shape: the sides go.
        let kept = ((high * over_wide / over_high) as u32).clamp(1, width);
        ((width - kept) / 2, 0, kept, height)
    } else {
        let kept = ((wide * over_high / over_wide) as u32).clamp(1, height);
        (0, (height - kept) / 2, width, kept)
    }
}

/// Whether one of `windows` hides a display at `frame`: lies over nine
/// tenths of it or more.
fn hidden(frame: ScreenRect, windows: &[ScreenRect]) -> bool {
    let area = |frame: ScreenRect| {
        let wide = i64::from((frame.right - frame.left).max(0));
        wide * i64::from((frame.bottom - frame.top).max(0))
    };
    windows.iter().any(|window| {
        let shared = ScreenRect {
            left: window.left.max(frame.left),
            top: window.top.max(frame.top),
            right: window.right.min(frame.right),
            bottom: window.bottom.min(frame.bottom),
        };
        area(shared) * 10 >= area(frame) * 9 && area(frame) > 0
    })
}

/// The wallpaper windows and the pictures they go through.
pub struct Wallpaper {
    config: WallpaperConfig,
    backdrops: Vec<DesktopBackdrop>,
    /// The video, where one plays.
    video: Option<DesktopVideo>,
    /// The displays the windows were opened for.
    frames: Vec<ScreenRect>,
    /// Counts configurations, so that the task of an earlier one stops.
    task: usize,
}

impl Global for Wallpaper {}

impl Wallpaper {
    /// Draws the wallpaper `config` asks for, if it asks for one.
    pub fn start(config: &WallpaperConfig, cx: &mut App) {
        cx.set_global(Self {
            config: WallpaperConfig::default(),
            backdrops: Vec::new(),
            video: None,
            frames: Vec::new(),
            task: 0,
        });
        Self::configure(config, cx);
    }

    /// Takes over a configuration that was read again, if it says
    /// something else about the wallpaper than the one before.
    pub fn configure(config: &WallpaperConfig, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if this.config == *config {
            return;
        }
        this.config = config.clone();
        this.task += 1;
        let task = this.task;
        let pictures = config.pictures();
        let video = config.video.clone();
        if this.video.is_some() != video.is_some() || pictures.is_empty() {
            // The windows of the other kind go, or all of them.
            this.video = None;
            this.backdrops.clear();
            this.frames.clear();
        }
        if pictures.is_empty() && video.is_none() {
            return;
        }
        let interval = config.interval();
        let fade = cx.global::<Theme>().motion.effects;
        cx.spawn(async move |cx: &mut AsyncApp| {
            if let Some(video) = video {
                match Self::play(task, &video, fade, cx).await {
                    Ok(()) => return,
                    Err(error) => report!("the wallpaper video is left out: {error:#}"),
                }
                let current = cx.update(|cx| {
                    let this = cx.global_mut::<Self>();
                    if this.task == task {
                        this.video = None;
                        this.frames.clear();
                    }
                    this.task == task
                });
                if !matches!(current, Ok(true)) || pictures.is_empty() {
                    return;
                }
            }
            let mut next = 0;
            loop {
                let Ok(Some(sizes)) = cx.update(|cx| Self::sizes(task, cx)) else {
                    return;
                };
                let path = &pictures[next % pictures.len()];
                match Self::show(task, path, &sizes, fade, cx).await {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(error) => report!("a wallpaper picture is left out: {error:#}"),
                }
                // The picture stays, unless the displays change: then the
                // windows are opened anew and get the same picture.
                let until = (pictures.len() > 1).then(|| Instant::now() + interval);
                loop {
                    cx.background_executor().timer(DISPLAY_CHECK).await;
                    match cx.update(|cx| Self::displays_changed(task, cx)) {
                        Ok(Some(true)) => break,
                        Ok(Some(false)) => {}
                        _ => return,
                    }
                    if until.is_some_and(|until| Instant::now() >= until) {
                        next += 1;
                        break;
                    }
                }
            }
        })
        .detach();
    }

    /// Plays the video in the file at `path` on every display until the
    /// wallpaper is configured anew, and returns then. Fails where the
    /// video cannot be played.
    async fn play(
        task: usize,
        path: &Path,
        fade: SpringSpec,
        cx: &mut AsyncApp,
    ) -> anyhow::Result<()> {
        loop {
            // The windows are opened for the displays there are.
            let opened = cx.update(|cx| {
                let this = cx.global_mut::<Self>();
                if this.task != task {
                    return Ok(false);
                }
                this.video = None;
                let frames = display_frames();
                this.video = Some(DesktopVideo::open(&frames, path)?);
                this.frames = frames;
                anyhow::Ok(true)
            })??;
            if !opened {
                return Ok(());
            }

            let began = Instant::now();
            loop {
                cx.background_executor().timer(VIDEO_CHECK).await;
                let state = cx.update(|cx| {
                    let this = cx.global::<Self>();
                    let video = this.video.as_ref().filter(|_| this.task == task);
                    video.map(DesktopVideo::state)
                })?;
                match state {
                    None => return Ok(()),
                    Some(VideoState::Failed(why)) => {
                        anyhow::bail!("{} cannot be played: {why}", path.display())
                    }
                    Some(VideoState::Playing) => break,
                    Some(VideoState::Loading) => {}
                }
                if began.elapsed() > VIDEO_PATIENCE {
                    anyhow::bail!("{} did not begin to play", path.display());
                }
            }

            let mut presence = Spring::new(fade, 0.);
            presence.animate_to(1.);
            let mut shown = 0.;
            while shown < 1. {
                cx.background_executor().timer(FRAME).await;
                crate::platform::Pace::quicken();
                let mut now = presence.value_at(Instant::now()).clamp(shown, 1.);
                if presence.is_settled() {
                    now = 1.;
                }
                let current = cx.update(|cx| {
                    let this = cx.global::<Self>();
                    if let Some(video) = this.video.as_ref().filter(|_| this.task == task) {
                        video.advance(now);
                    }
                    this.task == task
                })?;
                if !current {
                    return Ok(());
                }
                shown = now;
            }

            // The video goes on where it is seen, until the displays
            // change: then the windows are opened anew.
            loop {
                cx.background_executor().timer(DISPLAY_CHECK).await;
                let windows = cx
                    .background_executor()
                    .spawn(async {
                        let windows = AppWindow::enumerate();
                        let shown = windows.iter().filter(|window| window.id.is_shown());
                        shown
                            .filter_map(|window| window.id.frame())
                            .collect::<Vec<_>>()
                    })
                    .await;
                let failed = cx.update(|cx| {
                    let this = cx.global::<Self>();
                    if this.task != task {
                        return None;
                    }
                    if this.frames != display_frames() {
                        return Some(Ok(true));
                    }
                    let video = this.video.as_ref()?;
                    if let VideoState::Failed(why) = video.state() {
                        return Some(Err(why));
                    }
                    let mut displays = 0;
                    for (index, frame) in this.frames.iter().enumerate().take(32) {
                        if hidden(*frame, &windows) {
                            displays |= 1 << index;
                        }
                    }
                    video.hide(displays);
                    Some(Ok(false))
                })?;
                match failed {
                    None => return Ok(()),
                    Some(Ok(true)) => break,
                    Some(Ok(false)) => {}
                    Some(Err(why)) => anyhow::bail!("{} stopped playing: {why}", path.display()),
                }
            }
        }
    }

    /// Whether the displays are no longer the ones the windows were
    /// opened for, unless `task` is not the current one.
    fn displays_changed(task: usize, cx: &App) -> Option<bool> {
        let this = cx.global::<Self>();
        (this.task == task).then(|| this.frames != display_frames())
    }

    /// The sizes of the wallpaper windows, which are opened anew first if
    /// the displays changed. Nothing if `task` is not the current one or
    /// no display takes a window.
    fn sizes(task: usize, cx: &mut App) -> Option<Vec<(u32, u32)>> {
        let this = cx.global_mut::<Self>();
        if this.task != task {
            return None;
        }
        let frames = display_frames();
        if this.frames != frames {
            // The old windows go first, and their pictures with them.
            this.backdrops.clear();
            this.backdrops = frames
                .iter()
                .filter_map(|frame| {
                    DesktopBackdrop::open(*frame)
                        .inspect_err(|error| report!("a display gets no wallpaper: {error:#}"))
                        .ok()
                })
                .collect();
            this.frames = frames;
        }
        (!this.backdrops.is_empty())
            .then(|| this.backdrops.iter().map(DesktopBackdrop::size).collect())
    }

    /// Reads the picture at `path` and fades every window, of the `sizes`
    /// given, over to it. Says whether `task` was the current one to the
    /// end.
    async fn show(
        task: usize,
        path: &Path,
        sizes: &[(u32, u32)],
        fade: SpringSpec,
        cx: &mut AsyncApp,
    ) -> anyhow::Result<bool> {
        let file = path.to_path_buf();
        let image = cx
            .background_executor()
            .spawn(async move { read(&file) })
            .await?;
        let image = Arc::new(image);
        // Displays of one size share the scaled picture.
        let mut fitted: Option<Arc<BackdropPicture>> = None;
        for (index, &(width, height)) in sizes.iter().enumerate() {
            let picture = match fitted.take() {
                Some(picture) if picture.size() == (width, height) => picture,
                // The one of another size is let go of before the next
                // is made.
                _ => {
                    let image = image.clone();
                    let scaled = cx
                        .background_executor()
                        .spawn(async move { fit(&image, width, height) })
                        .await;
                    Arc::new(scaled)
                }
            };
            let taken = cx.update(|cx| {
                let this = cx.global::<Self>();
                (this.task == task).then(|| match this.backdrops.get(index) {
                    Some(backdrop) => backdrop.set(&picture),
                    None => Ok(()),
                })
            })?;
            match taken {
                Some(taken) => taken?,
                None => return Ok(false),
            }
            fitted = Some(picture);
        }
        drop(fitted);
        drop(image);

        let mut presence = Spring::new(fade, 0.);
        presence.animate_to(1.);
        let mut shown = 0.;
        while shown < 1. {
            cx.background_executor().timer(FRAME).await;
            crate::platform::Pace::quicken();
            let mut now = presence.value_at(Instant::now()).clamp(shown, 1.);
            if presence.is_settled() {
                now = 1.;
            }
            let current = cx.update(|cx| {
                let this = cx.global::<Self>();
                if this.task != task {
                    return false;
                }
                for backdrop in &this.backdrops {
                    backdrop.advance(shown, now);
                }
                true
            })?;
            if !current {
                return Ok(false);
            }
            shown = now;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_part_of_a_picture_that_covers_has_the_shape_of_the_display() {
        // Wider in shape: the sides go.
        assert_eq!(covering_part(8000, 2000, 2000, 1000), (2000, 0, 4000, 2000));
        // Higher in shape: top and bottom go.
        assert_eq!(covering_part(4000, 6000, 2000, 1000), (0, 2000, 4000, 2000));
        // The same shape: all of it, larger or smaller.
        assert_eq!(covering_part(1000, 500, 2000, 1000), (0, 0, 1000, 500));
        assert_eq!(covering_part(1, 900, 3440, 1440), (0, 449, 1, 1));
    }

    #[test]
    fn the_shares_of_the_pixels_mixed_into_one_sum_to_one() {
        for (from, to) in [(3840, 2560), (1000, 3440), (7, 7), (1, 5), (5, 1)] {
            let shares = shares(from, to);
            assert_eq!(shares.len(), to as usize);
            for (first, shares) in shares {
                assert!(first + shares.len() <= from as usize);
                assert!((shares.iter().sum::<f32>() - 1.).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn a_picture_scaled_down_keeps_its_halves() {
        // Left half dark, right half bright.
        let image = RgbImage::from_fn(8, 4, |x, _| image::Rgb([if x < 4 { 0 } else { 200 }; 3]));
        let picture = fit(&image, 4, 2);
        let row = &picture.pixels()[..12];
        assert_eq!(row[..3], [0, 0, 0]);
        assert_eq!(row[9..], [200, 200, 200]);
    }

    #[test]
    fn a_picture_is_fitted_to_the_display_with_blue_first() {
        let mut image = RgbImage::new(4, 2);
        for pixel in image.pixels_mut() {
            pixel.0 = [10, 20, 30];
        }
        let picture = fit(&image, 3, 3);
        assert_eq!(picture.size(), (3, 3));
        // Three pixels take nine bytes, a row twelve.
        assert_eq!(BackdropPicture::stride(3), 12);
        assert_eq!(
            picture.pixels()[..12],
            [30, 20, 10, 30, 20, 10, 30, 20, 10, 0, 0, 0]
        );
    }

    #[test]
    fn a_display_is_hidden_by_a_window_over_nearly_all_of_it() {
        let frame = ScreenRect {
            left: 100,
            top: 0,
            right: 1100,
            bottom: 1000,
        };
        let window = |left, top, right, bottom| ScreenRect {
            left,
            top,
            right,
            bottom,
        };
        // A maximized window under a bar, and one that reaches past the display.
        assert!(hidden(frame, &[window(100, 40, 1100, 1000)]));
        assert!(hidden(frame, &[window(0, -8, 3000, 1008)]));
        assert!(!hidden(
            frame,
            &[window(100, 0, 600, 1000), window(600, 0, 1100, 1000)]
        ));
        assert!(!hidden(frame, &[window(1100, 0, 2100, 1000)]));
        assert!(!hidden(frame, &[]));
    }

    #[test]
    fn pictures_are_known_by_their_extension_in_any_case() {
        assert!(is_picture(Path::new(r"C:\Pictures\sea.JPG")));
        assert!(is_picture(Path::new("hill.png")));
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
        let config = WallpaperConfig {
            images: vec![single.clone(), directory.clone()],
            interval: None,
            video: None,
        };
        let pictures = config.pictures();
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(
            pictures,
            [single, directory.join("a.jpg"), directory.join("b.png")]
        );
    }
}
