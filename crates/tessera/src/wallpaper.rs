// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The wallpaper behind the icons of the desktop, on every display:
//! pictures that fade one into the next, a color or a gradient, videos,
//! a picture of the day, or the wallpaper of Windows.
//!
//! What Tessera draws lies in windows of its own over the wallpaper of
//! Windows, which is left as it is and shows again when Tessera exits,
//! unless `mirror` makes Tessera's picture the wallpaper of Windows too;
//! the one before is then put back as Tessera exits. The windows are
//! plain ones, not ones of the user interface: a picture is kept once for
//! each display, at the size of the display, and a video is drawn by
//! Windows.
//!
//! The wallpaper of Windows itself is drawn by the desktop window: that
//! of Explorer, or in shell mode Tessera's own, which paints it with
//! `PaintDesktop`. Tessera opens no window for it there, and draws it
//! itself only where another program's desktop window is in its place.
//!
//! A task looks at the displays, the workspaces, the time of day and the
//! wallpaper of Windows every few seconds, works out what each display is
//! to show ([`looks::Plan`]) and changes only the windows whose plan
//! changed. Everything that waits, reading files and asking Windows, is
//! done off the thread of the interface.

mod accent;
mod compose;
mod daily;
mod looks;

use std::collections::HashSet;
use std::future::Future as _;
use std::path::{Path, PathBuf};
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context as _;
use chrono::{Local, NaiveDate, Timelike as _};
use gpui::{App, AsyncApp, Global};
use image::{ImageReader, RgbImage};
use serde::{Deserialize, Serialize};

use self::compose::{Fit, around, compose};
use self::daily::{Daily, DailyPicture};
pub use self::looks::WallpaperConfig;
use self::looks::{Looks, Moment, Plan, WindowsNow};
use crate::motion::{Spring, SpringSpec};
use crate::platform::{
    AppWindow, BackdropPicture, Battery, DesktopBackdrop, DesktopVideo, ScreenRect, ShellThread,
    SystemWallpaper, VideoState, decode_with_wic, display_at, display_frames, light_mode,
    memory_held, user_locale,
};
use crate::theme::Theme;
use crate::workspaces::Workspaces;

/// How long one step of a fade stays on the screen.
const FRAME: Duration = Duration::from_millis(16);

/// How often the displays, the wallpaper of Windows and the videos are
/// looked at.
const DISPLAY_CHECK: Duration = Duration::from_secs(2);

/// How often the workspaces are looked at, where some has a wallpaper of
/// its own.
const WORKSPACE_CHECK: Duration = Duration::from_millis(250);

/// How often a video is asked how far it has come while it is read.
const VIDEO_CHECK: Duration = Duration::from_millis(50);

/// How long Tessera has run before a video is opened. Starting takes
/// memory for a few seconds that is given back after, and so does opening
/// a video: one after the other stays under what both at once take.
const VIDEO_AFTER_START: Duration = Duration::from_secs(8);

/// How long a video may take to have its first frame.
const VIDEO_PATIENCE: Duration = Duration::from_secs(15);

/// The memory Tessera may hold with its videos, in bytes. A video takes
/// memory by its size and by how Windows decodes it, which is known only
/// once it plays: one that takes Tessera over this is left out.
const VIDEO_MEMORY: u64 = 200 * 1024 * 1024;

/// How long after a failed try the picture of the day is asked for again.
const DAILY_RETRY: Duration = Duration::from_secs(30 * 60);

/// How long Tessera waits as it exits for the wallpaper of Windows to be
/// put back.
const RESTORE_PATIENCE: Duration = Duration::from_secs(2);

/// Has the work of `mirror` done one at a time.
static MIRRORING: Mutex<()> = Mutex::new(());

/// Counts the work asked of `mirror`; work that finds a later one asked
/// for is left undone.
static MIRROR_ASKED: AtomicU64 = AtomicU64::new(0);

/// When the wallpaper was first asked for, which is as Tessera starts.
static STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// The color the wallpaper stands out with, as `0x1RRGGBB`, or 0 where it
/// is not known.
static COLOR: AtomicU32 = AtomicU32::new(0);

/// The color the wallpaper of the first display stands out with, as red,
/// green and blue, for a theme whose seed is `wallpaper`; `None` until a
/// picture was read, and while a video plays.
pub fn wallpaper_color() -> Option<[u8; 3]> {
    let value = COLOR.load(Ordering::Relaxed);
    let [_, red, green, blue] = value.to_be_bytes();
    (value != 0).then_some([red, green, blue])
}

fn set_wallpaper_color(color: Option<[u8; 3]>) {
    let value = color.map_or(0, |[red, green, blue]| {
        u32::from_be_bytes([1, red, green, blue])
    });
    COLOR.store(value, Ordering::Relaxed);
}

/// The directory of what Tessera keeps for this machine,
/// `%LOCALAPPDATA%\tessera`.
fn local_data() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("tessera"))
}

/// Fails where Tessera holds more memory with the videos of `files` than
/// it may.
fn video_fits(files: &[PathBuf]) -> anyhow::Result<()> {
    match memory_held() {
        Some(held) if held > VIDEO_MEMORY => anyhow::bail!(
            "{} has Tessera hold {} MB, more than the {} MB it stays under",
            names(files),
            held / (1024 * 1024),
            VIDEO_MEMORY / (1024 * 1024)
        ),
        _ => Ok(()),
    }
}

/// The files `files`, as they are named in a report.
fn names(files: &[PathBuf]) -> String {
    let names: Vec<String> = files
        .iter()
        .map(|file| file.display().to_string())
        .collect();
    names.join(", ")
}

/// The pixels of the picture in the file `path`, whatever its name says:
/// read by Tessera where it knows the format, and otherwise by a codec of
/// Windows (HEIC, AVIF, JPEG XR).
fn read(path: &Path) -> anyhow::Result<RgbImage> {
    let own = ImageReader::open(path)
        .and_then(ImageReader::with_guessed_format)
        .map_err(anyhow::Error::from)
        .and_then(|reader| Ok(reader.decode()?));
    let image = match own {
        Ok(image) => image.into_rgb8(),
        Err(error) => {
            let (width, height, pixels) = decode_with_wic(path)
                .map_err(|wic| error.context(format!("{wic:#}")))
                .with_context(|| format!("{} cannot be read", path.display()))?;
            RgbImage::from_raw(width, height, pixels).context("Windows gave too few pixels")?
        }
    };
    if image.width() == 0 || image.height() == 0 {
        anyhow::bail!("{} is an empty picture", path.display());
    }
    Ok(image)
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

/// The number of each of the displays with the bounds `frames`: they are
/// counted from the left, and from the top where two begin equally far
/// left, the first being 1.
fn numbers(frames: &[ScreenRect]) -> Vec<usize> {
    let before = |frame: &ScreenRect| {
        let earlier = |other: &&ScreenRect| (other.left, other.top) < (frame.left, frame.top);
        frames.iter().filter(earlier).count()
    };
    frames.iter().map(|frame| before(frame) + 1).collect()
}

/// Which of `frames` is the primary display, whose corner is where the
/// desktop begins; the first where none is.
fn primary(frames: &[ScreenRect]) -> usize {
    let at_origin = |frame: &ScreenRect| frame.left == 0 && frame.top == 0;
    frames.iter().position(at_origin).unwrap_or(0)
}

/// The displays, by their place in `plans`, that are to play each list of
/// videos; displays with the same list share one decode.
fn video_groups(plans: &[Plan]) -> Vec<(Vec<PathBuf>, Vec<usize>)> {
    let mut groups: Vec<(Vec<PathBuf>, Vec<usize>)> = Vec::new();
    for (index, plan) in plans.iter().enumerate() {
        let Plan::Video(files) = plan else {
            continue;
        };
        match groups.iter_mut().find(|(of, _)| of == files) {
            Some((_, displays)) => displays.push(index),
            None => groups.push((files.clone(), vec![index])),
        }
    }
    groups
}

/// Waits for `duration`, or until `poke` gets something or is closed.
async fn nap(duration: Duration, poke: &async_channel::Receiver<()>, cx: &AsyncApp) {
    let mut timer = cx.background_executor().timer(duration);
    let mut poked = pin!(poke.recv());
    std::future::poll_fn(|context| {
        let done = Pin::new(&mut timer).poll(context).is_ready()
            || poked.as_mut().poll(context).is_ready();
        if done { Poll::Ready(()) } else { Poll::Pending }
    })
    .await;
}

/// What one display shows: the plan it was last brought to, and the
/// window of its picture.
#[derive(Default)]
struct Slot {
    plan: Plan,
    backdrop: Option<DesktopBackdrop>,
    /// The window of the picture before, which a new one fades in over
    /// and which goes after.
    under: Option<DesktopBackdrop>,
}

/// Videos that play on some displays.
struct Playing {
    files: Vec<PathBuf>,
    /// The displays, by their place among the frames.
    displays: Vec<usize>,
    video: DesktopVideo,
    /// Whether the memory was looked at once more after it came in.
    weighed: bool,
}

/// What is known of the picture of the day.
#[derive(Default)]
struct DailyWatch {
    /// The day whose picture is at hand.
    day: Option<NaiveDate>,
    /// When the picture of which day was last fetched.
    asked: Option<(NaiveDate, Instant)>,
}

/// The wallpaper windows and what they show.
pub struct Wallpaper {
    config: WallpaperConfig,
    /// Counts configurations, so that the task of an earlier one stops.
    task: usize,
    /// The displays the slots are for.
    frames: Vec<ScreenRect>,
    slots: Vec<Slot>,
    videos: Vec<Playing>,
    /// Videos that could not be played, and are not tried again until the
    /// wallpaper is configured anew.
    failed: HashSet<PathBuf>,
    /// How many pictures the action `wallpaper` asked to go on by, not yet
    /// done.
    step: isize,
    /// Wakes the task early, for the action.
    poke: async_channel::Sender<()>,
    poked: async_channel::Receiver<()>,
    /// The picture of the day, where there is one.
    daily: Option<DailyPicture>,
    /// What was last made the wallpaper of Windows with `mirror`.
    mirrored: Option<SystemWallpaper>,
}

impl Global for Wallpaper {}

impl Wallpaper {
    /// Draws the wallpaper `config` asks for, and puts back a wallpaper of
    /// Windows that a Tessera before this one left changed.
    pub fn start(config: &WallpaperConfig, cx: &mut App) {
        STARTED.get_or_init(Instant::now);
        let (poke, poked) = async_channel::bounded(1);
        cx.set_global(Self {
            config: WallpaperConfig::default(),
            task: 0,
            frames: Vec::new(),
            slots: Vec::new(),
            videos: Vec::new(),
            failed: HashSet::new(),
            step: 0,
            poke,
            poked,
            daily: None,
            mirrored: None,
        });
        cx.on_app_quit(|_| {
            Mirror::restore_before_exit();
            async {}
        })
        .detach();
        if !config.mirror {
            Mirror::restore_later();
        }
        // Without a `[wallpaper]` the wallpaper of Windows is followed,
        // for a theme of its colors.
        Self::begin(config, cx);
    }

    /// Takes over a configuration that was read again, if it says
    /// something else about the wallpaper than the one before. What every
    /// display shows is worked out anew, and only what changed fades.
    pub fn configure(config: &WallpaperConfig, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if this.config == *config {
            return;
        }
        if this.config.mirror && !config.mirror {
            this.mirrored = None;
            Mirror::restore_later();
        }
        Self::begin(config, cx);
    }

    /// Starts the task that keeps the wallpaper as `config` says, and has
    /// the one before stop.
    fn begin(config: &WallpaperConfig, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        this.config = config.clone();
        this.task += 1;
        this.failed.clear();
        let task = this.task;
        let config = config.clone();
        let fade = cx.global::<Theme>().motion.effects;
        cx.spawn(async move |cx: &mut AsyncApp| {
            if let Err(error) = Self::keep(task, config, fade, cx).await {
                report!("the wallpaper stopped: {error:#}");
            }
        })
        .detach();
    }

    /// Has every display show the next picture of its list, or with a
    /// negative `by` the one before, at once; the interval begins anew.
    pub fn go_on(by: isize, cx: &mut App) {
        if !cx.has_global::<Self>() {
            return;
        }
        let this = cx.global_mut::<Self>();
        this.step += by;
        this.poke.try_send(()).ok();
    }

    /// Keeps the wallpaper as the configuration of `task` says, until the
    /// wallpaper is configured anew.
    async fn keep(
        task: usize,
        config: WallpaperConfig,
        fade: SpringSpec,
        cx: &mut AsyncApp,
    ) -> anyhow::Result<()> {
        let mirror = config.mirror;
        let looks = cx
            .background_executor()
            .spawn(async move { config.looks() })
            .await;
        let poked = cx.update(|cx| cx.global::<Self>().poked.clone())?;
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64);
        let mut turn = 0_i64;
        let mut turned = Instant::now();
        let mut windows: Option<WindowsNow> = None;
        let mut painted = true;
        let mut looked: Option<Instant> = None;
        let mut daily_watch = DailyWatch::default();
        let mut planned: Option<(Vec<ScreenRect>, Vec<Plan>)> = None;
        let mut colored = None;
        loop {
            let now = Instant::now();
            if looked.is_none_or(|looked| now >= looked + DISPLAY_CHECK) {
                looked = Some(now);
                painted = cx
                    .background_executor()
                    .spawn(async { SystemWallpaper::painted_by_desktop() })
                    .await;
                let follows = cx.update(|cx| cx.global::<Theme>().follows_wallpaper())?;
                if looks.shows_windows() && (!painted || follows) {
                    let frames = display_frames();
                    let read = ShellThread::call(move || WindowsNow::read(&frames));
                    windows = read.recv().await.ok();
                }
                if let Some(daily) = looks.daily() {
                    Self::keep_daily(task, daily, &mut daily_watch, cx).await?;
                }
                if !Self::tend_videos(task, &looks, cx).await? {
                    return Ok(());
                }
            }

            let step = cx.update(|cx| std::mem::take(&mut cx.global_mut::<Self>().step))?;
            if step != 0 {
                turn = turn.saturating_add(step as i64);
                turned = now;
            } else if looks.rotates() && turned.elapsed() >= looks.interval {
                turn = turn.saturating_add(1);
                turned = now;
            }

            let plans = cx.update(|cx| {
                let this = cx.global::<Self>();
                (this.task == task).then(|| {
                    let moment = Moment {
                        dark: Self::dark(&looks),
                        turn,
                        seed,
                        windows: windows.as_ref(),
                        painted,
                        daily: this.daily.as_ref().map(|daily| daily.file.as_path()),
                        failed: &this.failed,
                        videos: STARTED.get_or_init(Instant::now).elapsed() >= VIDEO_AFTER_START,
                    };
                    let frames = display_frames();
                    let plans = Self::plans(&looks, &frames, &moment, cx);
                    (frames, plans)
                })
            })?;
            let Some(plans) = plans else {
                return Ok(());
            };
            if planned.as_ref() != Some(&plans) {
                let (frames, wanted) = plans.clone();
                if !Self::apply(task, &frames, &wanted, fade, cx).await? {
                    return Ok(());
                }
                if mirror {
                    Self::mirror(task, &frames, &wanted, cx)?;
                }
                planned = Some(plans);
            }
            if let Some((frames, plans)) = &planned {
                let windows = windows.as_ref();
                Self::follow_color(task, frames, plans, windows, &mut colored, cx).await?;
            }

            let pause = if looks.by_workspace() {
                WORKSPACE_CHECK
            } else {
                DISPLAY_CHECK
            };
            nap(pause, &poked, cx).await;
        }
    }

    /// Whether the dark pictures are due: in the dark hours, or without
    /// them while Windows is dark.
    fn dark(looks: &Looks) -> bool {
        match looks.dark_hours {
            Some(hours) => {
                let now = Local::now();
                looks::is_dark(now.hour() * 60 + now.minute(), hours)
            }
            None => light_mode() == Some(false),
        }
    }

    /// What each display at `frames` is to show at `moment`.
    fn plans(looks: &Looks, frames: &[ScreenRect], moment: &Moment, cx: &App) -> Vec<Plan> {
        numbers(frames)
            .into_iter()
            .zip(frames)
            .map(|(number, frame)| {
                let workspace = looks
                    .by_workspace()
                    .then(|| Workspaces::now(Some(display_at(*frame)), cx).current);
                looks.of(number, workspace).plan(*frame, looks, moment)
            })
            .collect()
    }

    /// Brings the windows of the displays at `frames` to `plans`: new
    /// pictures and colors fade in over what was there, videos fade in
    /// once they play, and what is to show nothing fades out. Says whether
    /// `task` was the current one to the end.
    async fn apply(
        task: usize,
        frames: &[ScreenRect],
        plans: &[Plan],
        fade: SpringSpec,
        cx: &mut AsyncApp,
    ) -> anyhow::Result<bool> {
        let changed = cx.update(|cx| {
            let this = cx.global_mut::<Self>();
            if this.task != task {
                return None;
            }
            if this.frames != frames {
                // The old windows go first, and their pictures with them.
                this.videos.clear();
                this.slots.clear();
                this.slots.resize_with(frames.len(), Slot::default);
                this.frames = frames.to_vec();
            }
            let changed = this.slots.iter().zip(plans).enumerate();
            let changed = changed.filter(|(_, (slot, plan))| {
                slot.plan != **plan && matches!(plan, Plan::Picture { .. } | Plan::Fill(_))
            });
            Some(changed.map(|(index, _)| index).collect::<Vec<_>>())
        })?;
        let Some(changed) = changed else {
            return Ok(false);
        };
        if !Self::show_pictures(task, frames, plans, &changed, fade, cx).await? {
            return Ok(false);
        }
        for (files, displays) in video_groups(plans) {
            let playing = cx.update(|cx| {
                let this = cx.global::<Self>();
                let same =
                    |playing: &Playing| playing.files == files && playing.displays == displays;
                this.videos.iter().any(same)
            })?;
            if playing {
                continue;
            }
            match Self::play(task, frames, &files, &displays, fade, cx).await {
                Ok(true) => {}
                Ok(false) => return Ok(false),
                Err(error) => {
                    report!("the wallpaper video is left out: {error:#}");
                    cx.update(|cx| cx.global_mut::<Self>().failed.extend(files))?;
                }
            }
        }
        Self::clear_away(task, plans, fade, cx).await
    }

    /// Fades the displays `changed` over to the pictures and colors of
    /// `plans`. A picture that cannot be read is reported and its display
    /// keeps what it shows. Says whether `task` was the current one to
    /// the end.
    async fn show_pictures(
        task: usize,
        frames: &[ScreenRect],
        plans: &[Plan],
        changed: &[usize],
        fade: SpringSpec,
        cx: &mut AsyncApp,
    ) -> anyhow::Result<bool> {
        if changed.is_empty() {
            return Ok(true);
        }
        let desktop = around(frames).unwrap_or(ScreenRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        });
        let first = primary(frames);
        // Displays that show the same file share what was read, and those
        // of one size the fitted picture as well.
        let mut read_last: Option<(PathBuf, Arc<RgbImage>)> = None;
        // With the color it stands out with, which the primary display
        // gives the theme whether it made the picture or took it over.
        let mut fitted: Option<(Plan, Arc<BackdropPicture>, Option<[u8; 3]>)> = None;
        let mut fading = Vec::new();
        for &index in changed {
            let plan = &plans[index];
            let frame = frames[index];
            let (file, fit, ground) = match plan {
                Plan::Picture {
                    file, fit, ground, ..
                } => (Some(file), *fit, *ground),
                Plan::Fill(ground) => (None, Fit::Cover, *ground),
                _ => continue,
            };
            let size = (
                (frame.right - frame.left).unsigned_abs(),
                (frame.bottom - frame.top).unsigned_abs(),
            );
            let (picture, color) = match fitted.take() {
                Some((of, picture, color))
                    if of == *plan && picture.size() == size && fit != Fit::Span =>
                {
                    (picture, color)
                }
                // The one of another size is let go of before the next
                // is made.
                _ => {
                    let image = match file {
                        None => None,
                        Some(file) => match read_last.take() {
                            Some((of, image)) if of == *file => Some(image),
                            _ => {
                                let path = file.clone();
                                let image = cx
                                    .background_executor()
                                    .spawn(async move { read(&path) })
                                    .await;
                                match image {
                                    Ok(image) => Some(Arc::new(image)),
                                    Err(error) => {
                                        report!("a wallpaper picture is left out: {error:#}");
                                        continue;
                                    }
                                }
                            }
                        },
                    };
                    if let (Some(file), Some(image)) = (file, &image) {
                        read_last = Some((file.clone(), image.clone()));
                    }
                    let (shown, color) = cx
                        .background_executor()
                        .spawn(async move {
                            let color = match &image {
                                Some(image) => accent::dominant_color(image),
                                None => Some(ground.middle()),
                            };
                            let shown = compose(image.as_deref(), frame, desktop, fit, ground);
                            (shown, color)
                        })
                        .await;
                    (Arc::new(shown), color)
                }
            };
            if index == first {
                set_wallpaper_color(color);
            }
            let taken = cx.update(|cx| {
                let this = cx.global_mut::<Self>();
                if this.task != task {
                    return None;
                }
                let slot = &mut this.slots[index];
                if slot.backdrop.is_none() || matches!(slot.plan, Plan::Video(_)) {
                    // A new window lies over every one before it, a video
                    // as well, which goes once the picture is in.
                    match DesktopBackdrop::open(frame) {
                        Ok(backdrop) => slot.under = slot.backdrop.replace(backdrop),
                        Err(error) => return Some(Err(error)),
                    }
                }
                slot.backdrop
                    .as_ref()
                    .map(|backdrop| backdrop.set(&picture))
            })?;
            match taken {
                None => return Ok(false),
                Some(Ok(())) => fading.push(index),
                Some(Err(error)) => report!("a display gets no wallpaper: {error:#}"),
            }
            fitted = Some((plan.clone(), picture, color));
        }
        drop(fitted);
        drop(read_last);

        let faded = Self::fade(task, fade, cx, |this, from, to| {
            for &index in &fading {
                if let Some(backdrop) = &this.slots[index].backdrop {
                    backdrop.advance(from, to);
                }
            }
        })
        .await?;
        if faded {
            cx.update(|cx| {
                let this = cx.global_mut::<Self>();
                for &index in &fading {
                    let slot = &mut this.slots[index];
                    slot.plan = plans[index].clone();
                    slot.under = None;
                }
            })?;
        }
        Ok(faded)
    }

    /// Plays `files` on the displays `displays` of `frames`, and once it
    /// has faded in lets go of what they showed before. Fails where the
    /// videos cannot be played. Says whether `task` was the current one to
    /// the end.
    async fn play(
        task: usize,
        frames: &[ScreenRect],
        files: &[PathBuf],
        displays: &[usize],
        fade: SpringSpec,
        cx: &mut AsyncApp,
    ) -> anyhow::Result<bool> {
        let shown: Vec<ScreenRect> = displays.iter().map(|&index| frames[index]).collect();
        let video = DesktopVideo::open(&shown, files)?;
        let began = Instant::now();
        loop {
            cx.background_executor().timer(VIDEO_CHECK).await;
            match video.state() {
                VideoState::Failed(why) => {
                    anyhow::bail!("{} cannot be played: {why}", names(files))
                }
                VideoState::Playing => break,
                VideoState::Loading => {}
            }
            video_fits(files)?;
            if began.elapsed() > VIDEO_PATIENCE {
                anyhow::bail!("{} did not begin to play", names(files));
            }
            if !cx.update(|cx| cx.global::<Self>().task == task)? {
                return Ok(false);
            }
        }
        let mut presence = Spring::new(fade, 0.);
        presence.animate_to(1.);
        loop {
            cx.background_executor().timer(FRAME).await;
            crate::platform::Pace::quicken();
            let now = presence.value_at(Instant::now()).clamp(0., 1.);
            let now = if presence.is_settled() { 1. } else { now };
            video.advance(now);
            if now >= 1. {
                break;
            }
        }
        video_fits(files)?;
        cx.update(|cx| {
            let this = cx.global_mut::<Self>();
            if this.task != task {
                return false;
            }
            for &index in displays {
                let slot = &mut this.slots[index];
                slot.backdrop = None;
                slot.under = None;
                slot.plan = Plan::Video(files.to_vec());
            }
            this.videos
                .retain(|playing| !playing.displays.iter().any(|at| displays.contains(at)));
            this.videos.push(Playing {
                files: files.to_vec(),
                displays: displays.to_vec(),
                video,
                weighed: false,
            });
            true
        })
    }

    /// Fades out the windows of displays that are to show nothing of
    /// Tessera's, and lets go of videos no display plays any more. Says
    /// whether `task` was the current one to the end.
    async fn clear_away(
        task: usize,
        plans: &[Plan],
        fade: SpringSpec,
        cx: &mut AsyncApp,
    ) -> anyhow::Result<bool> {
        let groups = video_groups(plans);
        let wanted = |playing: &Playing| {
            let same = |(files, displays): &(Vec<PathBuf>, Vec<usize>)| {
                *files == playing.files && *displays == playing.displays
            };
            groups.iter().any(same)
        };
        let nothing = |index: usize| matches!(plans.get(index), Some(Plan::Nothing));
        // What goes is taken out of the slots and the videos before it
        // fades, so that a configuration read again meanwhile finds it
        // gone and shows anew what it wants there, instead of leaving it
        // half faded.
        let taken = cx.update(|cx| {
            let this = cx.global_mut::<Self>();
            if this.task != task {
                return None;
            }
            // Videos that something else was faded in over go at once.
            let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut this.videos)
                .into_iter()
                .partition(|playing| {
                    !wanted(playing) && !playing.displays.iter().copied().all(nothing)
                });
            drop(gone);
            let (fading, kept): (Vec<_>, Vec<_>) =
                kept.into_iter().partition(|playing| !wanted(playing));
            this.videos = kept;
            for playing in &fading {
                for &index in &playing.displays {
                    this.slots[index].plan = Plan::Nothing;
                }
            }
            let going: Vec<DesktopBackdrop> = (0..this.slots.len())
                .filter(|&index| nothing(index))
                .filter_map(|index| {
                    let slot = &mut this.slots[index];
                    slot.plan = Plan::Nothing;
                    slot.under = None;
                    slot.backdrop.take()
                })
                .collect();
            Some((going, fading))
        })?;
        let Some((going, fading)) = taken else {
            return Ok(false);
        };
        if going.is_empty() && fading.is_empty() {
            return cx.update(|cx| cx.global::<Self>().task == task);
        }
        // The windows and videos go once they faded out, or at once where
        // the fade is cut short.
        Self::fade(task, fade, cx, |_, _, to| {
            for backdrop in &going {
                backdrop.recede(to);
            }
            for playing in &fading {
                playing.video.advance(1. - to);
            }
        })
        .await
    }

    /// Moves `step` from 0 to 1 with the spring `fade`, a frame at a
    /// time: it is given the global, how far it was and how far it is to
    /// be. Says whether `task` was the current one to the end.
    async fn fade(
        task: usize,
        fade: SpringSpec,
        cx: &mut AsyncApp,
        step: impl Fn(&Self, f32, f32),
    ) -> anyhow::Result<bool> {
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
                step(this, shown, now);
                true
            })?;
            if !current {
                return Ok(false);
            }
            shown = now;
        }
        Ok(true)
    }

    /// Looks after the videos that play: one that failed or takes too
    /// much memory is left out, the others are held still where windows
    /// hide all their displays or, as configured, while the machine runs
    /// on its battery. Says whether `task` is still the current one.
    async fn tend_videos(task: usize, looks: &Looks, cx: &mut AsyncApp) -> anyhow::Result<bool> {
        let playing = cx.update(|cx| !cx.global::<Self>().videos.is_empty())?;
        if !playing {
            return cx.update(|cx| cx.global::<Self>().task == task);
        }
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
        let on_battery =
            looks.pause_on_battery && Battery::read().is_some_and(|battery| !battery.plugged_in);
        cx.update(|cx| {
            let this = cx.global_mut::<Self>();
            if this.task != task {
                return false;
            }
            let mut failed = Vec::new();
            for (at, playing) in this.videos.iter_mut().enumerate() {
                if let VideoState::Failed(why) = playing.video.state() {
                    report!("{} stopped playing: {why}", names(&playing.files));
                    failed.push(at);
                    continue;
                }
                if !playing.weighed {
                    // Once more when it has played a while; what Tessera
                    // holds later for other things is not laid to it.
                    playing.weighed = true;
                    if let Err(error) = video_fits(&playing.files) {
                        report!("the wallpaper video is left out: {error:#}");
                        failed.push(at);
                        continue;
                    }
                }
                let mut hides = 0;
                for (bit, index) in playing.displays.iter().enumerate().take(32) {
                    if this
                        .frames
                        .get(*index)
                        .is_some_and(|frame| hidden(*frame, &windows))
                    {
                        hides |= 1 << bit;
                    }
                }
                playing.video.hide(hides);
                playing.video.pause(on_battery);
            }
            for at in failed.into_iter().rev() {
                let playing = this.videos.remove(at);
                for index in &playing.displays {
                    if let Some(slot) = this.slots.get_mut(*index) {
                        slot.plan = Plan::Nothing;
                    }
                }
                this.failed.extend(playing.files);
            }
            true
        })
    }

    /// Has the picture of the day of `daily` at hand: the one of today
    /// where it was fetched, or else the newest kept while today's is
    /// fetched in the background, once a day and again half an hour after
    /// a failed try.
    async fn keep_daily(
        task: usize,
        daily: Daily,
        watch: &mut DailyWatch,
        cx: &mut AsyncApp,
    ) -> anyhow::Result<()> {
        let today = Local::now().date_naive();
        if watch.day == Some(today) {
            return Ok(());
        }
        let (of_today, newest) = cx
            .background_executor()
            .spawn(async move { (daily.of(today), daily.newest()) })
            .await;
        if of_today.is_some() {
            watch.day = Some(today);
        }
        let shown = of_today.or(newest);
        cx.update(|cx| {
            let this = cx.global_mut::<Self>();
            if this.task == task && shown.is_some() && this.daily != shown {
                this.daily = shown;
            }
        })?;
        let due = watch
            .asked
            .is_none_or(|(day, when)| day != today || when.elapsed() >= DAILY_RETRY);
        if watch.day == Some(today) || !due {
            return Ok(());
        }
        watch.asked = Some((today, Instant::now()));
        cx.spawn(async move |cx: &mut AsyncApp| {
            let picture = cx
                .background_executor()
                .spawn(async move { daily.fetch(today, user_locale().as_deref()) })
                .await;
            match picture {
                Ok(picture) => {
                    let _ = cx.update(|cx| {
                        let this = cx.global_mut::<Self>();
                        if this.task == task {
                            this.daily = Some(picture);
                            this.poke.try_send(()).ok();
                        }
                    });
                }
                Err(error) => report!("the picture of the day is not there: {error:#}"),
            }
        })
        .detach();
        Ok(())
    }

    /// Keeps the color the wallpaper stands out with for the theme: from
    /// the picture or the color of the first display, which
    /// [`Self::show_pictures`] took as it showed it, or from the wallpaper
    /// of Windows where Tessera draws nothing there and the theme follows
    /// the wallpaper. A video gives none. `read` is the picture of Windows
    /// the color was last taken from, which is not read again.
    async fn follow_color(
        task: usize,
        frames: &[ScreenRect],
        plans: &[Plan],
        windows: Option<&WindowsNow>,
        read_from: &mut Option<(PathBuf, Option<SystemTime>)>,
        cx: &mut AsyncApp,
    ) -> anyhow::Result<()> {
        let first = primary(frames);
        let Some(frame) = frames.get(first) else {
            return Ok(());
        };
        match plans.get(first) {
            Some(Plan::Video(_)) => {
                *read_from = None;
                set_wallpaper_color(None);
            }
            Some(Plan::Nothing) => {
                let follows = cx.update(|cx| {
                    let this = cx.global::<Self>();
                    this.task == task && cx.global::<Theme>().follows_wallpaper()
                })?;
                let Some(now) = windows.filter(|_| follows) else {
                    return Ok(());
                };
                let Some(file) = now.wallpaper.picture_of(*frame) else {
                    *read_from = None;
                    set_wallpaper_color(Some(now.wallpaper.color));
                    return Ok(());
                };
                let key = (file.to_path_buf(), now.written.get(file).copied());
                if read_from.as_ref() == Some(&key) {
                    return Ok(());
                }
                let path = key.0.clone();
                *read_from = Some(key);
                let found = cx
                    .background_executor()
                    .spawn(async move {
                        read(&path)
                            .ok()
                            .and_then(|image| accent::dominant_color(&image))
                    })
                    .await;
                set_wallpaper_color(found.or(Some(now.wallpaper.color)));
            }
            _ => *read_from = None,
        }
        Ok(())
    }

    /// Makes the pictures of `plans` the wallpaper of Windows, where they
    /// are Tessera's own and changed since the last time, remembering the
    /// wallpaper there was before the first time.
    fn mirror(
        task: usize,
        frames: &[ScreenRect],
        plans: &[Plan],
        cx: &mut AsyncApp,
    ) -> anyhow::Result<()> {
        let first = primary(frames);
        let (fit, ground) = match plans.get(first) {
            Some(Plan::Picture {
                fit,
                ground,
                own: true,
                ..
            }) => (*fit, *ground),
            _ => return Ok(()),
        };
        let pictures: Vec<(ScreenRect, Option<PathBuf>)> = frames
            .iter()
            .zip(plans)
            .map(|(frame, plan)| match plan {
                Plan::Picture {
                    file, own: true, ..
                } => (*frame, Some(file.clone())),
                _ => (*frame, None),
            })
            .collect();
        let wanted = SystemWallpaper {
            pictures,
            position: fit.to_windows(),
            color: ground.middle(),
        };
        let changed = cx.update(|cx| {
            let this = cx.global_mut::<Self>();
            let changed = this.task == task && this.mirrored.as_ref() != Some(&wanted);
            if changed {
                this.mirrored = Some(wanted.clone());
            }
            changed
        })?;
        if changed {
            Mirror::queue(move || {
                if let Err(error) = Mirror::set(&wanted) {
                    report!("the wallpaper of Windows was not changed: {error:#}");
                }
            });
        }
        Ok(())
    }
}

/// The wallpaper of Windows that `mirror` replaced, kept in a file until
/// it is put back, so that it is put back after Tessera ended without
/// doing so too.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Mirror {
    fit: Fit,
    color: [u8; 3],
    #[serde(default, rename = "picture")]
    pictures: Vec<MirroredPicture>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct MirroredPicture {
    /// The left, top, right and bottom of the display.
    display: [i32; 4],
    file: Option<PathBuf>,
}

impl Mirror {
    /// The file the wallpaper before is kept in.
    fn file() -> Option<PathBuf> {
        Some(local_data()?.join("wallpaper-before.toml"))
    }

    fn of(wallpaper: &SystemWallpaper) -> Self {
        Self {
            fit: Fit::of_windows(wallpaper.position),
            color: wallpaper.color,
            pictures: wallpaper
                .pictures
                .iter()
                .map(|(frame, file)| MirroredPicture {
                    display: [frame.left, frame.top, frame.right, frame.bottom],
                    file: file.clone(),
                })
                .collect(),
        }
    }

    fn to_wallpaper(&self) -> SystemWallpaper {
        SystemWallpaper {
            pictures: self
                .pictures
                .iter()
                .map(|picture| {
                    let [left, top, right, bottom] = picture.display;
                    let frame = ScreenRect {
                        left,
                        top,
                        right,
                        bottom,
                    };
                    (frame, picture.file.clone())
                })
                .collect(),
            position: self.fit.to_windows(),
            color: self.color,
        }
    }

    /// Does `work` on a thread of its own with COM, after the work of
    /// `mirror` asked for before has ended, and only where no other was
    /// asked for since: what was asked for last is what Windows is left
    /// with, and a wallpaper is never read as the one before while
    /// another is being written.
    fn queue(work: impl FnOnce() + Send + 'static) {
        let asked = MIRROR_ASKED.fetch_add(1, Ordering::SeqCst) + 1;
        ShellThread::run_alone("wallpaper", move || {
            let _alone = MIRRORING.lock().unwrap_or_else(PoisonError::into_inner);
            if MIRROR_ASKED.load(Ordering::SeqCst) == asked {
                work();
            }
        });
    }

    /// Makes `wanted` the wallpaper of Windows, after keeping the one
    /// there is where none is kept yet. For [`Self::queue`].
    fn set(wanted: &SystemWallpaper) -> anyhow::Result<()> {
        let file = Self::file().context("there is no directory for local data")?;
        if !file.exists() {
            let frames: Vec<ScreenRect> = wanted.pictures.iter().map(|(frame, _)| *frame).collect();
            let mut before = SystemWallpaper::read(&frames);
            if let Some(directory) = file.parent() {
                std::fs::create_dir_all(directory)?;
                Self::keep_copies(&mut before, directory)?;
            }
            std::fs::write(&file, toml::to_string(&Self::of(&before))?)
                .with_context(|| format!("{} cannot be written", file.display()))?;
        }
        wanted.write()?;
        SystemWallpaper::announce();
        Ok(())
    }

    /// Copies each picture of `wallpaper` that Windows writes over as the
    /// wallpaper changes into `directory`, and names the copy in its
    /// place, so that the picture there was before can be put back. The
    /// copies stay after that, since Windows then names them as the
    /// files of its wallpaper.
    fn keep_copies(wallpaper: &mut SystemWallpaper, directory: &Path) -> anyhow::Result<()> {
        let mut taken: Vec<PathBuf> = wallpaper
            .pictures
            .iter()
            .filter_map(|(_, picture)| picture.clone())
            .collect();
        let mut number = 0;
        for (_, picture) in &mut wallpaper.pictures {
            let Some(original) = picture.as_ref().filter(|picture| {
                SystemWallpaper::overwritten_by_windows(picture) && picture.is_file()
            }) else {
                continue;
            };
            // Named by what it is, which the copies of Windows are not.
            let kind = ImageReader::open(original)
                .and_then(ImageReader::with_guessed_format)
                .ok()
                .and_then(|reader| reader.format())
                .and_then(|format| format.extensions_str().first().copied())
                .unwrap_or("jpg");
            let copy = loop {
                let copy = directory.join(format!("wallpaper-before-{number}.{kind}"));
                number += 1;
                if !taken.contains(&copy) {
                    break copy;
                }
            };
            std::fs::copy(original, &copy).with_context(|| {
                format!(
                    "{} cannot be kept in {}",
                    original.display(),
                    copy.display()
                )
            })?;
            taken.push(copy.clone());
            *picture = Some(copy);
        }
        Ok(())
    }

    /// Puts back the wallpaper that was kept, if one is, and forgets it.
    /// Programs are yet to be told. For [`Self::queue`].
    fn restore() -> anyhow::Result<()> {
        let Some(file) = Self::file().filter(|file| file.exists()) else {
            return Ok(());
        };
        let text = std::fs::read_to_string(&file)?;
        let before: Self = toml::from_str(&text)
            .with_context(|| format!("{} is not a wallpaper", file.display()))?;
        before.to_wallpaper().put_back()?;
        std::fs::remove_file(&file)?;
        Ok(())
    }

    /// Puts back the wallpaper that was kept, if one is, after the work
    /// of `mirror` asked for before.
    fn restore_later() {
        Self::queue(|| {
            if let Err(error) = Self::restore() {
                report!("the wallpaper of Windows was not put back: {error:#}");
            }
            SystemWallpaper::announce();
        });
    }

    /// Puts back the wallpaper that was kept, waiting for it a moment,
    /// as Tessera exits. Programs are told after that, without waiting:
    /// the windows of Tessera are among them, and do not answer while
    /// this waits.
    fn restore_before_exit() {
        if Self::file().is_none_or(|file| !file.exists()) {
            return;
        }
        let (answer, answered) = async_channel::bounded(1);
        Self::queue(move || {
            answer.try_send(Self::restore()).ok();
            SystemWallpaper::announce();
        });
        let until = Instant::now() + RESTORE_PATIENCE;
        while Instant::now() < until {
            match answered.try_recv() {
                Ok(Ok(())) => return,
                Ok(Err(error)) => {
                    report!("the wallpaper of Windows was not put back: {error:#}");
                    return;
                }
                Err(async_channel::TryRecvError::Closed) => return,
                Err(async_channel::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        report!("the wallpaper of Windows is put back after Tessera exits");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn displays_are_counted_from_the_left_and_then_from_the_top() {
        let frame = |left, top| ScreenRect {
            left,
            top,
            right: left + 100,
            bottom: top + 100,
        };
        let frames = [
            frame(0, 0),
            frame(-100, 50),
            frame(100, 0),
            frame(-100, -50),
        ];
        assert_eq!(numbers(&frames), [3, 2, 4, 1]);
        assert_eq!(primary(&frames), 0);
        assert_eq!(primary(&frames[1..]), 0);
    }

    #[test]
    fn displays_with_the_same_videos_share_them() {
        let one = vec![PathBuf::from("a.mp4")];
        let two = vec![PathBuf::from("b.mp4")];
        let plans = [
            Plan::Video(one.clone()),
            Plan::Nothing,
            Plan::Video(two.clone()),
            Plan::Video(one.clone()),
        ];
        assert_eq!(video_groups(&plans), [(one, vec![0, 3]), (two, vec![2])]);
    }

    #[test]
    fn the_color_of_the_wallpaper_is_kept_with_a_mark() {
        set_wallpaper_color(Some([0, 0, 0]));
        assert_eq!(wallpaper_color(), Some([0, 0, 0]));
        set_wallpaper_color(Some([1, 2, 3]));
        assert_eq!(wallpaper_color(), Some([1, 2, 3]));
        set_wallpaper_color(None);
        assert_eq!(wallpaper_color(), None);
    }

    #[test]
    fn the_wallpaper_before_is_written_and_read_back() {
        let before = Mirror {
            fit: Fit::Span,
            color: [10, 20, 30],
            pictures: vec![
                MirroredPicture {
                    display: [0, 0, 2560, 1440],
                    file: Some(PathBuf::from(r"C:\Pictures\one.jpg")),
                },
                MirroredPicture {
                    display: [2560, 0, 5120, 1440],
                    file: None,
                },
            ],
        };
        let text = toml::to_string(&before).unwrap();
        assert_eq!(toml::from_str::<Mirror>(&text).unwrap(), before);
        let wallpaper = before.to_wallpaper();
        assert_eq!(Mirror::of(&wallpaper), before);
    }
}
