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
//! of Explorer, or in shell mode Tessera's own once it says it paints it
//! with `PaintDesktop`. Tessera opens no window for it there, nor where
//! there is no desktop window at all, and draws it itself only where
//! another desktop window is in its place.
//!
//! A task looks at the displays, the workspaces, the time of day and the
//! wallpaper of Windows every few seconds, works out what each display is
//! to show ([`looks::Plan`]) and changes only the windows whose plan
//! changed. Everything that waits, reading files and asking Windows, is
//! done off the thread of the interface, and so are the adjustments of a
//! picture (`blur`, `brightness`, `overlay` and the others, see
//! [`adjust`]), which are made once each time a picture is shown.
//!
//! In shell mode, where Explorer does not answer for the wallpaper,
//! Tessera answers the programs that ask for `IDesktopWallpaper` itself
//! (see [`served`]): what they set is written to the configuration.

mod accent;
mod adjust;
mod compose;
mod daily;
mod edit;
mod looks;
mod served;

use std::collections::HashSet;
use std::future::Future as _;
use std::path::{Path, PathBuf};
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context as _;
use chrono::{Local, NaiveDate, Timelike as _};
use gpui::{App, AsyncApp, Global, RenderImage};
use image::{ImageReader, RgbImage};
use serde::{Deserialize, Serialize};

use self::adjust::Adjustments;
pub use self::adjust::{Blend, Flip, Place, Position, parse_rgba};
pub use self::compose::Fit;
use self::compose::{Ground, around, compose, still};
use self::daily::{Daily, DailyPicture};
pub use self::edit::{Edit, read as read_settings, write as write_settings};
pub use self::looks::WallpaperConfig;
pub use self::looks::{FASTEST, PAUSE_COVERAGE, SHORTEST_INTERVAL, SLOWEST, Shown, Source};
use self::looks::{Looks, Moment, Plan, Turns, VideoPlan, WindowsNow};
use self::served::Served;
use crate::motion::{Spring, SpringSpec};
use crate::performance::Performance;
use crate::platform::{
    AppWindow, BackdropPicture, Battery, DesktopBackdrop, DesktopVideo, DisplayKey, Poster,
    ScreenRect, ShellThread, SystemWallpaper, VideoLook, VideoOverlay, VideoState, decode_with_wic,
    display_at, display_frames, light_mode, memory_held, user_locale,
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

/// Whether a still of what each display shows is kept, for
/// [`Wallpaper::picture_of`]: from the first time one is asked for.
static STILLS_WANTED: AtomicBool = AtomicBool::new(false);

/// The longer side of such a still at most, in pixels.
const STILL_LONGEST: u32 = 2560;

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

/// The share of the display at `frame` that `windows` cover together,
/// from 0 to 1: where they lie over each other it is counted once.
fn covered(frame: ScreenRect, windows: &[ScreenRect]) -> f32 {
    let area = |frame: &ScreenRect| {
        let wide = i64::from((frame.right - frame.left).max(0));
        wide * i64::from((frame.bottom - frame.top).max(0))
    };
    let whole = area(&frame);
    if whole == 0 {
        return 0.;
    }
    let within: Vec<ScreenRect> = windows
        .iter()
        .map(|window| ScreenRect {
            left: window.left.max(frame.left),
            top: window.top.max(frame.top),
            right: window.right.min(frame.right),
            bottom: window.bottom.min(frame.bottom),
        })
        .filter(|window| area(window) > 0)
        .collect();
    // The display is cut into strips at every side of a window; in each
    // the windows across all of it cover one length of it or several.
    let mut sides: Vec<i32> = within.iter().flat_map(|w| [w.left, w.right]).collect();
    sides.sort_unstable();
    sides.dedup();
    let mut shared = 0_i64;
    for strip in sides.windows(2) {
        let (from, to) = (strip[0], strip[1]);
        let mut lengths: Vec<(i32, i32)> = { within.iter() }
            .filter(|window| window.left <= from && window.right >= to)
            .map(|window| (window.top, window.bottom))
            .collect();
        lengths.sort_unstable();
        let (mut down, mut reach) = (0_i64, i32::MIN);
        for (top, bottom) in lengths {
            let start = top.max(reach);
            if bottom > start {
                down += i64::from(bottom - start);
            }
            reach = reach.max(bottom);
        }
        shared += i64::from(to - from) * down;
    }
    (shared as f64 / whole as f64) as f32
}

/// Whether windows that cover `share` of a display hold its video still,
/// where they do from `pause_at` on; at 0 they never do.
fn holds(share: f32, pause_at: f32) -> bool {
    pause_at > 0. && share >= pause_at
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
fn video_groups(plans: &[Plan]) -> Vec<(VideoPlan, Vec<usize>)> {
    let mut groups: Vec<(VideoPlan, Vec<usize>)> = Vec::new();
    for (index, plan) in plans.iter().enumerate() {
        let Plan::Video(videos) = plan else {
            continue;
        };
        match groups.iter_mut().find(|(of, _)| of == videos) {
            Some((_, displays)) => displays.push(index),
            None => groups.push((videos.clone(), vec![index])),
        }
    }
    groups
}

/// How the platform is to play `plan`.
fn video_look(plan: &VideoPlan) -> VideoLook {
    if let Some(overlay) = plan.adjust.overlay
        && !matches!(overlay.blend, Blend::Normal | Blend::Darken)
    {
        report!(
            "the overlay of a video is laid on as `normal`: the compositor has no `{}`",
            overlay.blend.name()
        );
    }
    let adjust = plan.adjust.as_video_shows();
    let overlay = adjust.overlay.map(|overlay| VideoOverlay {
        color: overlay.color,
        darken: overlay.blend == Blend::Darken,
    });
    VideoLook {
        speed: plan.sound.speed,
        sound: plan.sound.sound,
        volume: plan.sound.volume,
        flip_across: adjust.flip.across(),
        flip_down: adjust.flip.down(),
        anchor: (adjust.anchor.x, adjust.anchor.y),
        overlay,
    }
}

/// `pixels` as a still for the interface.
fn render_image(pixels: image::RgbaImage) -> Arc<RenderImage> {
    Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]))
}

/// A still of what the display at `frame` shows of a video whose frame
/// is `poster`, mirrored, placed and overlaid as `adjust` says and as
/// the video itself is.
fn poster_still(
    poster: &Poster,
    frame: ScreenRect,
    adjust: &Adjustments,
) -> Option<Arc<RenderImage>> {
    let rgb: Vec<u8> = { poster.pixels.as_chunks::<4>().0.iter() }
        .flat_map(|&[blue, green, red, _]| [red, green, blue])
        .collect();
    let image = RgbImage::from_raw(poster.width, poster.height, rgb)?;
    // At the size of the still, not of the display: a frame is no
    // sharper than it is.
    let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
    let (wide, high) =
        compose::shrunk_size(width.unsigned_abs(), height.unsigned_abs(), STILL_LONGEST);
    let shown = ScreenRect {
        left: 0,
        top: 0,
        right: wide as i32,
        bottom: high as i32,
    };
    let fitted = compose(
        Some(&image),
        shown,
        shown,
        (Fit::Cover, Ground::default()),
        &adjust.as_video_shows(),
    );
    Some(render_image(still(&fitted, STILL_LONGEST)))
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

/// A still of what a display shows, where stills are kept.
type Still = Option<Arc<RenderImage>>;

/// A picture made for a plan, with the color it stands out with and its
/// still, which displays of the same size share.
type Fitted = (Plan, Arc<BackdropPicture>, Option<[u8; 3]>, Still);

/// What one display shows: the plan it was last brought to, and the
/// window of its picture.
#[derive(Default)]
struct Slot {
    plan: Plan,
    backdrop: Option<DesktopBackdrop>,
    /// The window of the picture before, which a new one fades in over
    /// and which goes after.
    under: Option<DesktopBackdrop>,
    /// A still of the picture, where stills are kept.
    still: Option<Arc<RenderImage>>,
}

/// Videos that play on some displays.
struct Playing {
    plan: VideoPlan,
    /// The displays, by their place among the frames.
    displays: Vec<usize>,
    video: DesktopVideo,
    /// Whether the memory was looked at once more after it came in.
    weighed: bool,
    /// A still of a frame for each of the displays, where stills are
    /// kept and a frame was taken.
    stills: Vec<Option<Arc<RenderImage>>>,
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
    /// The displays the slots are for, and how other parts of Tessera
    /// know them.
    frames: Vec<ScreenRect>,
    keys: Vec<DisplayKey>,
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
    /// What answers for the wallpaper as Explorer would, in shell mode.
    served: Option<Served>,
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
            keys: Vec::new(),
            slots: Vec::new(),
            videos: Vec::new(),
            failed: HashSet::new(),
            step: 0,
            poke,
            poked,
            daily: None,
            mirrored: None,
            served: None,
        });
        cx.on_app_quit(|cx| {
            cx.global_mut::<Self>().served = None;
            Mirror::restore_before_exit();
            async {}
        })
        .detach();
        Served::start(config, cx);
        if !config.mirror {
            Mirror::restore_later();
        }
        // Without a `[wallpaper]` the wallpaper of Windows is followed,
        // for a theme of its colors.
        Self::begin(config, cx);
        // A change of performance mode is looked at at once.
        if let Some(performance) = Performance::entity(cx) {
            cx.observe(&performance, |_, cx| {
                cx.global::<Self>().poke.try_send(()).ok();
            })
            .detach();
        }
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
        Served::configure(config, cx);
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

    /// Has a still kept of what each display shows from now on, for
    /// [`Self::picture_of`], which does this itself the first time. The
    /// stills of what is shown already are made in the background, in a
    /// moment. A part of Tessera that is to show the wallpaper calls this
    /// as it starts, so that the first picture it asks for is there.
    pub fn keep_pictures(cx: &App) {
        if STILLS_WANTED.swap(true, Ordering::Relaxed) {
            return;
        }
        let Some(this) = cx.try_global::<Self>() else {
            return;
        };
        for playing in &this.videos {
            playing.video.want_posters();
        }
        this.poke.try_send(()).ok();
    }

    /// The picture Tessera shows on the display `display`: the composed
    /// still, adjusted, or a frame of the video there, mirrored, placed
    /// and overlaid. `None` where Tessera draws no wallpaper of its own
    /// there, as where the desktop window draws the wallpaper of Windows,
    /// and until the still is made: stills are kept only once
    /// [`Self::keep_pictures`] was called, and this calls it. A still is
    /// no larger than 2560 pixels on its longer side.
    ///
    /// A view that paints the still keeps the `Arc` only while it shows
    /// it, and lets GPUI drop it from its window then (`Window::drop_image`).
    pub fn picture_of(display: DisplayKey, cx: &App) -> Option<Arc<RenderImage>> {
        Self::keep_pictures(cx);
        let this = cx.try_global::<Self>()?;
        let index = this.keys.iter().position(|key| *key == display)?;
        let slot = this.slots.get(index)?;
        match &slot.plan {
            Plan::Nothing => None,
            Plan::Video(_) => this.videos.iter().find_map(|playing| {
                let at = playing.displays.iter().position(|shown| *shown == index)?;
                playing.stills.get(at)?.clone()
            }),
            Plan::Picture { .. } | Plan::Fill(_) => slot.still.clone(),
        }
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
        let intervals = looks.intervals();
        let mut turns = Turns::default();
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
                // Where no window can be opened for the wallpaper of
                // Windows, it is neither read nor made for one.
                painted = cx
                    .background_executor()
                    .spawn(async {
                        SystemWallpaper::painted_by_desktop() || !DesktopBackdrop::possible()
                    })
                    .await;
                let follows = cx.update(|cx| cx.global::<Theme>().follows_wallpaper())?;
                let drawn = !painted || follows || looks.adjusts_windows();
                if looks.shows_windows() && drawn {
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
            turns.tick(&intervals, now);
            if step != 0 {
                turns.step(step as i64, now);
            }

            let plans = cx.update(|cx| {
                let this = cx.global::<Self>();
                (this.task == task).then(|| {
                    let moment = Moment {
                        dark: Self::dark(&looks),
                        turns: &turns,
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
                Served::shown(&frames, &wanted, windows.as_ref(), cx)?;
                planned = Some(plans);
            }
            if let Some((frames, plans)) = &planned {
                let windows = windows.as_ref();
                Self::follow_color(task, frames, plans, windows, &mut colored, cx).await?;
                if STILLS_WANTED.load(Ordering::Relaxed) {
                    Self::fill_stills(task, frames, plans, cx).await?;
                }
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

    /// What each display at `frames` is to show at `moment`; pictures
    /// are not blurred while performance mode saves on looks.
    fn plans(looks: &Looks, frames: &[ScreenRect], moment: &Moment, cx: &App) -> Vec<Plan> {
        let blur = !Performance::minimal(cx);
        numbers(frames)
            .into_iter()
            .zip(frames)
            .map(|(number, frame)| {
                let workspace = looks
                    .by_workspace()
                    .then(|| Workspaces::now(Some(display_at(*frame)), cx).current);
                let plan = looks.of(number, workspace).plan(*frame, looks, moment);
                if blur { plan } else { plan.unblurred() }
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
                this.keys = frames.iter().map(|frame| display_at(*frame)).collect();
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
        for (videos, displays) in video_groups(plans) {
            let playing = cx.update(|cx| {
                let this = cx.global::<Self>();
                let same =
                    |playing: &Playing| playing.plan == videos && playing.displays == displays;
                this.videos.iter().any(same)
            })?;
            if playing {
                continue;
            }
            match Self::play(task, frames, &videos, &displays, fade, cx).await {
                Ok(true) => {}
                Ok(false) => return Ok(false),
                Err(error) => {
                    report!("the wallpaper video is left out: {error:#}");
                    cx.update(|cx| cx.global_mut::<Self>().failed.extend(videos.files))?;
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
        let mut fitted: Option<Fitted> = None;
        let mut fading = Vec::new();
        let stills = STILLS_WANTED.load(Ordering::Relaxed);
        for &index in changed {
            let plan = &plans[index];
            let frame = frames[index];
            let Some((file, fit, ground, adjust)) = Self::drawn(plan) else {
                continue;
            };
            let size = (
                (frame.right - frame.left).unsigned_abs(),
                (frame.bottom - frame.top).unsigned_abs(),
            );
            let (picture, color, kept) = match fitted.take() {
                Some((of, picture, color, kept))
                    if of == *plan && picture.size() == size && fit != Fit::Span =>
                {
                    (picture, color, kept)
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
                    let (shown, color, kept) = cx
                        .background_executor()
                        .spawn(async move {
                            let color = match &image {
                                Some(image) => accent::dominant_color(image),
                                None => Some(ground.middle()),
                            };
                            let shown =
                                compose(image.as_deref(), frame, desktop, (fit, ground), &adjust);
                            let kept = stills.then(|| render_image(still(&shown, STILL_LONGEST)));
                            (shown, color, kept)
                        })
                        .await;
                    (Arc::new(shown), color, kept)
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
                slot.still = kept.clone();
                slot.backdrop
                    .as_ref()
                    .map(|backdrop| backdrop.set(&picture))
            })?;
            match taken {
                None => return Ok(false),
                Some(Ok(())) => fading.push(index),
                Some(Err(error)) => report!("a display gets no wallpaper: {error:#}"),
            }
            fitted = Some((plan.clone(), picture, color, kept));
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
        videos: &VideoPlan,
        displays: &[usize],
        fade: SpringSpec,
        cx: &mut AsyncApp,
    ) -> anyhow::Result<bool> {
        let files = &videos.files;
        let shown: Vec<ScreenRect> = displays.iter().map(|&index| frames[index]).collect();
        let video = DesktopVideo::open(&shown, files, video_look(videos))?;
        if STILLS_WANTED.load(Ordering::Relaxed) {
            video.want_posters();
        }
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
                slot.still = None;
                slot.plan = Plan::Video(videos.clone());
            }
            this.videos
                .retain(|playing| !playing.displays.iter().any(|at| displays.contains(at)));
            this.videos.push(Playing {
                plan: videos.clone(),
                displays: displays.to_vec(),
                video,
                weighed: false,
                stills: vec![None; displays.len()],
            });
            true
        })
    }

    /// What a display draws itself for `plan`: the file of its picture,
    /// if it has one, how it is fitted, what lies under it and the
    /// adjustments; `None` where it draws nothing itself.
    fn drawn(plan: &Plan) -> Option<(Option<&PathBuf>, Fit, Ground, Adjustments)> {
        match plan {
            Plan::Picture {
                file,
                fit,
                ground,
                adjust,
                ..
            } => Some((Some(file), *fit, *ground, *adjust)),
            Plan::Fill(ground) => Some((None, Fit::Cover, *ground, Adjustments::default())),
            Plan::Nothing | Plan::Video(_) => None,
        }
    }

    /// Makes the stills that are missing of what the displays at `frames`
    /// show as `plans` say: of pictures that were shown before stills
    /// were kept, and of the poster frames videos took since. Says
    /// nothing of whether `task` is the current one; the next look does.
    async fn fill_stills(
        task: usize,
        frames: &[ScreenRect],
        plans: &[Plan],
        cx: &mut AsyncApp,
    ) -> anyhow::Result<()> {
        let desktop = around(frames).unwrap_or(ScreenRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        });
        let missing: Vec<usize> = cx.update(|cx| {
            let this = cx.global::<Self>();
            { this.slots.iter().enumerate() }
                .filter(|(index, slot)| {
                    slot.still.is_none() && plans.get(*index) == Some(&slot.plan)
                })
                .filter(|(_, slot)| Self::drawn(&slot.plan).is_some())
                .map(|(index, _)| index)
                .collect()
        })?;
        for index in missing {
            let plan = plans[index].clone();
            let frame = frames[index];
            let made = cx
                .background_executor()
                .spawn(async move {
                    let (file, fit, ground, adjust) = Self::drawn(&plan)?;
                    let image = match file {
                        Some(file) => Some(read(file).ok()?),
                        None => None,
                    };
                    let shown = compose(image.as_ref(), frame, desktop, (fit, ground), &adjust);
                    Some((plan, render_image(still(&shown, STILL_LONGEST))))
                })
                .await;
            let Some((plan, made)) = made else {
                continue;
            };
            cx.update(|cx| {
                let this = cx.global_mut::<Self>();
                if let Some(slot) = this.slots.get_mut(index)
                    && this.task == task
                    && slot.plan == plan
                {
                    slot.still = Some(made);
                }
            })?;
        }
        // The poster frames the videos took since they were asked last.
        let posters: Vec<(usize, Poster)> = cx.update(|cx| {
            let this = cx.global::<Self>();
            { this.videos.iter().enumerate() }
                .filter_map(|(at, playing)| Some((at, playing.video.take_poster()?)))
                .collect()
        })?;
        for (at, poster) in posters {
            let shown = cx.update(|cx| {
                let this = cx.global::<Self>();
                let playing = this.videos.get(at)?;
                let frames: Vec<ScreenRect> = { playing.displays.iter() }
                    .filter_map(|&index| this.frames.get(index).copied())
                    .collect();
                Some((playing.plan.clone(), frames))
            })?;
            let Some((plan, shown)) = shown else {
                continue;
            };
            let adjust = plan.adjust;
            let stills = cx
                .background_executor()
                .spawn(async move {
                    { shown.iter() }
                        .map(|frame| poster_still(&poster, *frame, &adjust))
                        .collect::<Vec<_>>()
                })
                .await;
            cx.update(|cx| {
                let this = cx.global_mut::<Self>();
                if let Some(playing) = this.videos.get_mut(at)
                    && playing.plan == plan
                    && stills.len() == playing.displays.len()
                {
                    playing.stills = stills;
                }
            })?;
        }
        Ok(())
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
            let same = |(videos, displays): &(VideoPlan, Vec<usize>)| {
                *videos == playing.plan && *displays == playing.displays
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
                    slot.still = None;
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
    /// cover as much of each of their displays as `pause_coverage` says
    /// or, as configured, while the machine runs on its battery. Says
    /// whether `task` is still the current one.
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
            // Performance mode at its most pauses the videos too.
            let still = on_battery || Performance::extreme(cx);
            let this = cx.global_mut::<Self>();
            if this.task != task {
                return false;
            }
            let mut failed = Vec::new();
            for (at, playing) in this.videos.iter_mut().enumerate() {
                if let VideoState::Failed(why) = playing.video.state() {
                    report!("{} stopped playing: {why}", names(&playing.plan.files));
                    failed.push(at);
                    continue;
                }
                if !playing.weighed {
                    // Once more when it has played a while; what Tessera
                    // holds later for other things is not laid to it.
                    playing.weighed = true;
                    if let Err(error) = video_fits(&playing.plan.files) {
                        report!("the wallpaper video is left out: {error:#}");
                        failed.push(at);
                        continue;
                    }
                }
                let mut hides = 0;
                for (bit, index) in playing.displays.iter().enumerate().take(32) {
                    let share =
                        { this.frames.get(*index) }.map_or(0., |frame| covered(*frame, &windows));
                    if holds(share, looks.pause_coverage) {
                        hides |= 1 << bit;
                    }
                }
                playing.video.hide(hides);
                playing.video.pause(still);
            }
            for at in failed.into_iter().rev() {
                let playing = this.videos.remove(at);
                for index in &playing.displays {
                    if let Some(slot) = this.slots.get_mut(*index) {
                        slot.plan = Plan::Nothing;
                    }
                }
                this.failed.extend(playing.plan.files);
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
    fn windows_cover_a_display_together_counted_once() {
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
        let close = |share: f32, wanted: f32| (share - wanted).abs() < 1e-4;
        // A maximized window under a bar, and one that reaches past the display.
        assert!(close(covered(frame, &[window(100, 40, 1100, 1000)]), 0.96));
        assert!(close(covered(frame, &[window(0, -8, 3000, 1008)]), 1.));
        // Two halves side by side cover all of it.
        let halves = [window(100, 0, 600, 1000), window(600, 0, 1100, 1000)];
        assert!(close(covered(frame, &halves), 1.));
        // Where they lie over each other, it is counted once.
        let over = [window(100, 0, 700, 500), window(400, 0, 1100, 500)];
        assert!(close(covered(frame, &over), 0.5));
        assert!(close(covered(frame, &[window(1100, 0, 2100, 1000)]), 0.));
        assert!(close(covered(frame, &[]), 0.));
        assert!(holds(0.96, 0.9) && !holds(0.5, 0.9));
        // At 0 a video is never held still.
        assert!(!holds(1., 0.));
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
        let videos = |file: &str| VideoPlan {
            files: vec![PathBuf::from(file)],
            adjust: Adjustments::default(),
            sound: looks::VideoSound::default(),
        };
        let one = videos("a.mp4");
        let two = videos("b.mp4");
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
