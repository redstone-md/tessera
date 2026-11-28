// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Screenshots as Spectacle of KDE takes them: of all displays, of the
//! display under the pointer, of the window in front, of the window under
//! the pointer, or of a rectangle chosen on a frozen picture of the
//! screen; at once or after a delay counted down on the screen; then
//! saved, copied, or opened in an editor to draw on.
//!
//! The action `screenshot` takes what to take and what to do with it, as
//! `region,edit,delay=3` ([`Request`]). [`Screenshots`] does it: it counts
//! down in a window of its own ([`countdown`]), has the picture taken away
//! from the user interface (`platform::capture`), and then shows the
//! region tool ([`region`]), opens the editor ([`editor`]), saves the
//! picture and says so in a banner with what can be done with it, or puts
//! it on the clipboard. `[screenshot]` of the configuration says where
//! pictures are saved, how they are named and in which format, whether the
//! editor opens after each one, whether the pointer is in them, and which
//! keys of Windows Tessera takes for them.

mod countdown;
mod editor;
mod marks;
mod region;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use anyhow::{Context as _, Result};
use chrono::format::{Item, StrftimeItems};
use chrono::{DateTime, Local};
use gpui::{App, AsyncApp, Global, RenderImage};
use image::{ImageFormat, RgbaImage};
use serde::Deserialize;

use crate::banners::{Banners, Notice, NoticeAction};
use crate::i18n::Language;
use crate::platform::{
    Capture, CaptureArea, DisplayKey, Launch, Offer, ScreenRect, capture, front_window,
    put_on_clipboard, reveal, save_screenshot, screenshots_folder, virtual_screen, window_frames,
    window_under_pointer,
};

pub use editor::Editor;

/// The glyph of a screenshot in banners and buttons.
pub const ICON_SCREENSHOT: &str = "\u{e722}";

/// The longest delay, in seconds.
pub const LONGEST_DELAY: u32 = 10;

/// What a file is named while `[screenshot] name` says nothing usable.
const DEFAULT_NAME: &str = "Screenshot %Y-%m-%d %H%M%S";

/// The side of the small copy of a picture a banner shows, in pixels.
const BANNER_PICTURE: u32 = 96;

/// What `[screenshot]` of the configuration says.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScreenshotConfig {
    /// Where pictures are saved; the Screenshots folder of Pictures while
    /// it is not set.
    pub folder: Option<PathBuf>,
    /// The name of a file without its extension: the codes of strftime,
    /// as `%Y-%m-%d`, for the time it was taken, and `%N` for the number
    /// Windows counts its own screenshots with.
    pub name: String,
    pub format: ShotFormat,
    /// Whether the editor opens after each screenshot that is not told
    /// what to do, instead of saving it.
    pub editor_after: bool,
    /// Whether the pointer is in the picture.
    pub cursor: bool,
    /// Whether Tessera takes PrtScn for the region tool and Alt+PrtScn
    /// for the window in front, where the keys of Windows are in use.
    pub own_print_screen: bool,
    /// What Win+Shift+S opens.
    pub snip: Snip,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            folder: None,
            name: DEFAULT_NAME.to_owned(),
            format: ShotFormat::Png,
            editor_after: false,
            cursor: false,
            own_print_screen: false,
            snip: Snip::Windows,
        }
    }
}

/// The format a picture is saved in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShotFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
}

impl ShotFormat {
    /// The extension of its files, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }

    pub fn image_format(self) -> ImageFormat {
        match self {
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Webp => ImageFormat::WebP,
        }
    }

    /// The format a file of `path` is written in, by its extension.
    pub fn of_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_string_lossy().to_lowercase();
        match extension.as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }
}

/// What Win+Shift+S opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Snip {
    /// The Snipping Tool of Windows (`ms-screenclip:`).
    #[default]
    Windows,
    /// Tessera's region tool.
    Tessera,
}

/// What a screenshot takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Every display.
    All,
    /// The display under the pointer.
    Display,
    /// The window in front.
    Window,
    /// The window under the pointer.
    UnderPointer,
    /// A rectangle chosen on a frozen picture of the screen.
    Region,
}

impl Mode {
    const NAMES: [(&'static str, Self); 5] = [
        ("all", Self::All),
        ("display", Self::Display),
        ("window", Self::Window),
        ("under-pointer", Self::UnderPointer),
        ("region", Self::Region),
    ];

    pub fn name(self) -> &'static str {
        { Self::NAMES.iter() }
            .find(|(_, mode)| *mode == self)
            .map_or("all", |(name, _)| name)
    }

    /// Every mode, in the order the widget offers them.
    pub fn all() -> impl Iterator<Item = Self> {
        Self::NAMES.into_iter().map(|(_, mode)| mode)
    }

    /// The English title of the mode, a text of the RUSSIAN table.
    pub fn title(self) -> &'static str {
        match self {
            Self::All => "All displays",
            Self::Display => "Display under the pointer",
            Self::Window => "Window in front",
            Self::UnderPointer => "Window under the pointer",
            Self::Region => "Rectangle",
        }
    }
}

/// What is done with a screenshot once it is taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Then {
    /// It opens in the editor.
    Edit,
    /// It goes on the clipboard.
    Copy,
    /// It is saved, and a banner says so.
    Save,
}

/// A screenshot asked for: the argument of the action `screenshot`,
/// `<mode>[,edit|,copy|,save][,delay=N]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub mode: Mode,
    /// What is done with it; without it, the editor opens where
    /// `editor_after` says so, and else it is saved.
    pub then: Option<Then>,
    /// Seconds counted down before it is taken, 0 to [`LONGEST_DELAY`].
    pub delay: u32,
}

impl Request {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            then: None,
            delay: 0,
        }
    }

    /// The request `arg` writes; without one, all displays saved.
    pub fn parse(arg: Option<&str>) -> Result<Self, String> {
        let Some(arg) = arg.map(str::trim).filter(|arg| !arg.is_empty()) else {
            return Ok(Self::new(Mode::All));
        };
        let mut parts = arg.split(',').map(str::trim);
        let first = parts.next().unwrap_or_default().to_lowercase();
        let mode = { Mode::NAMES.iter() }
            .find(|(name, _)| *name == first)
            .map(|(_, mode)| *mode)
            .ok_or_else(|| {
                format!(
                    "`screenshot` takes all, display, window, under-pointer or region first, not `{first}`"
                )
            })?;
        let mut request = Self::new(mode);
        for part in parts {
            let part = part.to_lowercase();
            let then = match part.as_str() {
                "edit" => Some(Then::Edit),
                "copy" => Some(Then::Copy),
                "save" => Some(Then::Save),
                _ => None,
            };
            if let Some(then) = then {
                if request.then.replace(then).is_some() {
                    return Err("`screenshot` takes one of edit, copy and save".to_owned());
                }
                continue;
            }
            let seconds = part
                .strip_prefix("delay=")
                .and_then(|n| n.trim().parse().ok());
            match seconds {
                Some(seconds) if seconds <= LONGEST_DELAY => request.delay = seconds,
                _ => {
                    return Err(format!(
                        "`screenshot` takes edit, copy, save or delay=0 to delay={LONGEST_DELAY} after the mode, not `{part}`"
                    ));
                }
            }
        }
        Ok(request)
    }

    /// The argument that asks for this request again.
    #[cfg(test)]
    pub fn arg(self) -> String {
        let mut arg = self.mode.name().to_owned();
        if let Some(then) = self.then {
            arg.push_str(match then {
                Then::Edit => ",edit",
                Then::Copy => ",copy",
                Then::Save => ",save",
            });
        }
        if self.delay > 0 {
            arg.push_str(&format!(",delay={}", self.delay));
        }
        arg
    }
}

/// The name of a file taken at `taken`, from `template` with the codes of
/// strftime, `%N` as `number`, and the extension `extension`. A template
/// whose codes chrono does not know is reported and the default is used;
/// the characters no file name may have become `-`.
pub fn file_name(template: &str, taken: DateTime<Local>, number: u32, extension: &str) -> String {
    let template = match template.trim() {
        "" => DEFAULT_NAME,
        template => template,
    };
    // `%N` first, so that chrono does not read it; `%%N` stays a `%N`.
    let numbered = template
        .replace("%%", "\u{0}")
        .replace("%N", &number.to_string());
    let numbered = numbered.replace('\u{0}', "%%");
    let valid = StrftimeItems::new(&numbered).all(|item| !matches!(item, Item::Error));
    let stem = match valid {
        true => taken.format(&numbered).to_string(),
        false => {
            report!("`name` of [screenshot] is not understood, `{DEFAULT_NAME}` is used");
            taken.format(DEFAULT_NAME).to_string()
        }
    };
    let stem: String = { stem.chars() }
        .map(|character| match character {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            character if character.is_control() => '-',
            character => character,
        })
        .collect();
    let stem = stem.trim().trim_end_matches('.');
    let stem = if stem.is_empty() { "Screenshot" } else { stem };
    format!("{stem}.{extension}")
}

/// The name for the number `number` of a file whose first choice
/// `first` is taken: `first` with ` (n)` before the extension, from 2.
fn numbered_name(first: &str, number: u32) -> String {
    if number <= 1 {
        return first.to_owned();
    }
    match first.rsplit_once('.') {
        Some((stem, extension)) => format!("{stem} ({number}).{extension}"),
        None => format!("{first} ({number})"),
    }
}

/// A picture taken, and where it was on the screen.
#[derive(Clone, Debug)]
pub struct Shot {
    pub pixels: Arc<RgbaImage>,
    pub frame: ScreenRect,
    pub taken: DateTime<Local>,
}

impl Shot {
    fn of(capture: Capture) -> Self {
        Self {
            pixels: Arc::new(capture.pixels),
            frame: capture.frame,
            taken: Local::now(),
        }
    }
}

/// The screenshots: what `[screenshot]` says and whether one is being
/// taken.
#[derive(Default)]
pub struct Screenshots {
    config: ScreenshotConfig,
    /// Whether a screenshot is being counted down, taken or chosen, so
    /// that a key pressed again meanwhile does not start another.
    busy: bool,
    /// Whether the seconds before one are being counted down, and whether
    /// the count is to stop.
    counting: bool,
    stop: bool,
}

impl Global for Screenshots {}

impl Screenshots {
    /// Takes over `[screenshot]`.
    pub fn configure(config: &ScreenshotConfig, cx: &mut App) {
        cx.default_global::<Self>().config = config.clone();
    }

    /// What `[screenshot]` says.
    pub fn config(cx: &App) -> ScreenshotConfig {
        { cx.try_global::<Self>() }
            .map(|this| this.config.clone())
            .unwrap_or_default()
    }

    /// Takes the screenshot `request` asks for, unless one is being taken.
    pub fn take(request: Request, cx: &mut App) {
        let this = cx.default_global::<Self>();
        if this.busy {
            // Asked again while the seconds run: the count stops.
            this.stop |= this.counting;
            return;
        }
        this.busy = true;
        let then = request.then.unwrap_or(match this.config.editor_after {
            true => Then::Edit,
            false => Then::Save,
        });
        let cursor = this.config.cursor;
        let (mode, delay) = (request.mode, request.delay);
        cx.spawn(async move |cx: &mut AsyncApp| {
            if delay > 0 {
                let counted = countdown::Countdown::run(delay, cx).await;
                if !counted {
                    cx.update(Self::done).ok();
                    return;
                }
            }
            let taken = away("screenshot", move || take(mode, cursor)).recv().await;
            let taken = taken.unwrap_or_else(|_| Err(anyhow::anyhow!("the capture ended")));
            cx.update(|cx| match taken {
                Ok(Taken::Shot(shot)) => {
                    Self::done(cx);
                    Self::finish(shot, then, cx);
                }
                Ok(Taken::Screen(shot, windows)) => {
                    if !region::RegionTool::open(shot, windows, then, cx) {
                        Self::done(cx);
                    }
                }
                Err(error) => {
                    Self::done(cx);
                    report!("no screenshot was taken: {error:#}");
                }
            })
            .ok();
        })
        .detach();
    }

    /// The screenshot being taken is over, whatever came of it.
    fn done(cx: &mut App) {
        let this = cx.default_global::<Self>();
        this.busy = false;
        this.stop = false;
    }

    /// The count before a screenshot starts, or ends.
    fn counting(counting: bool, cx: &mut App) {
        let this = cx.default_global::<Self>();
        this.counting = counting;
        if !counting {
            this.stop = false;
        }
    }

    /// Whether the count before a screenshot was asked to stop.
    fn count_stopped(cx: &mut App) -> bool {
        cx.default_global::<Self>().stop
    }

    /// Does `then` with `shot`.
    fn finish(shot: Shot, then: Then, cx: &mut App) {
        match then {
            Then::Edit => Editor::open(shot, cx),
            Then::Copy => Self::copy(shot.pixels, true, cx),
            Then::Save => Self::save_and_tell(shot, cx),
        }
    }

    /// The folder pictures are saved in.
    pub fn folder(cx: &App) -> Option<PathBuf> {
        Self::config(cx).folder.or_else(screenshots_folder)
    }

    /// Saves `pixels`, taken at `taken`, into the folder as `[screenshot]`
    /// names it, away from the user interface; the answer is the file.
    pub fn save(
        pixels: Arc<RgbaImage>,
        taken: DateTime<Local>,
        cx: &App,
    ) -> async_channel::Receiver<Result<PathBuf>> {
        let config = Self::config(cx);
        let folder = config.folder.clone();
        away("screenshot save", move || {
            let folder = folder
                .or_else(screenshots_folder)
                .context("there is no folder for screenshots")?;
            let format = config.format;
            let counted = config.name.contains("%N");
            let first = |number| file_name(&config.name, taken, number, format.extension());
            let names = |number| match counted {
                true => first(number),
                false => numbered_name(&first(0), number),
            };
            save_screenshot(&pixels, &folder, format.image_format(), counted, names)
        })
    }

    /// Saves `pixels` as the file `path`, in the format its extension
    /// names, or as PNG with that extension added.
    pub fn save_as(
        pixels: Arc<RgbaImage>,
        path: PathBuf,
    ) -> async_channel::Receiver<Result<PathBuf>> {
        away("screenshot save", move || {
            let (path, format) = match ShotFormat::of_path(&path) {
                Some(format) => (path, format),
                None => (path.with_extension("png"), ShotFormat::Png),
            };
            let bytes = crate::platform::encode_picture(&pixels, format.image_format())?;
            std::fs::write(&path, bytes)
                .with_context(|| format!("{} could not be written", path.display()))?;
            Ok(path)
        })
    }

    /// Saves `shot` and says so in a banner with what can be done with the
    /// file.
    fn save_and_tell(shot: Shot, cx: &mut App) {
        let saved = Self::save(shot.pixels.clone(), shot.taken, cx);
        let pixels = shot.pixels.clone();
        let small = away("screenshot banner", move || banner_picture(&pixels));
        cx.spawn(async move |cx: &mut AsyncApp| {
            let saved = saved.recv().await;
            let picture = small.recv().await.ok();
            cx.update(|cx| match saved {
                Ok(Ok(path)) => Self::tell_saved(path, shot, picture, cx),
                Ok(Err(error)) => report!("the screenshot was not saved: {error:#}"),
                Err(_) => report!("the screenshot was not saved"),
            })
            .ok();
        })
        .detach();
    }

    /// The banner of a screenshot saved as `path`: Open, Edit, Show in
    /// folder and Copy.
    fn tell_saved(path: PathBuf, shot: Shot, picture: Option<Arc<RenderImage>>, cx: &mut App) {
        let language = Language::of(cx);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        let (opened, revealed) = (path.clone(), path.clone());
        let (edited, copied) = (shot.clone(), shot.pixels.clone());
        let notice = Notice {
            app: language.say("Screenshots").to_owned(),
            title: language.say("Screenshot saved").to_owned(),
            body: name.unwrap_or_default(),
            glyph: Some(ICON_SCREENSHOT),
            picture,
            actions: vec![
                NoticeAction::new(language.say("Open"), move |_| {
                    Launch::new(&opened).start();
                }),
                NoticeAction::new(language.say("Edit"), move |cx| {
                    Editor::open(edited.clone(), cx);
                }),
                NoticeAction::new(language.say("Show in folder"), move |_| {
                    if let Err(error) = reveal(&revealed) {
                        report!("the screenshot is not shown in its folder: {error:#}");
                    }
                }),
                NoticeAction::new(language.say("Copy"), move |cx| {
                    Self::copy(copied.clone(), false, cx);
                }),
            ],
            ..Notice::default()
        };
        Banners::post(notice, cx);
    }

    /// Puts `pixels` on the clipboard, as a bitmap that keeps its
    /// transparency and as PNG, away from the user interface; with `tell`
    /// a banner says so.
    pub fn copy(pixels: Arc<RgbaImage>, tell: bool, cx: &mut App) {
        let put = away("screenshot copy", move || {
            let png = crate::platform::encode_picture(&pixels, ImageFormat::Png)
                .map_err(|error| format!("{error:#}"))?;
            put_on_clipboard(Offer::image(&pixels, png))
        });
        cx.spawn(async move |cx: &mut AsyncApp| {
            let put = put.recv().await;
            cx.update(|cx| match put {
                Ok(Ok(())) if tell => {
                    let language = Language::of(cx);
                    let notice = Notice {
                        app: language.say("Screenshots").to_owned(),
                        title: language
                            .say("Screenshot copied to the clipboard")
                            .to_owned(),
                        glyph: Some(ICON_SCREENSHOT),
                        ..Notice::default()
                    };
                    Banners::post(notice, cx);
                }
                Ok(Ok(())) => {}
                Ok(Err(error)) => report!("the screenshot was not copied: {error}"),
                Err(_) => report!("the screenshot was not copied"),
            })
            .ok();
        })
        .detach();
    }
}

/// What a capture brought: a picture to use as it is, or the picture of
/// the whole screen with the windows on it, for the region tool.
enum Taken {
    Shot(Shot),
    Screen(Shot, Vec<ScreenRect>),
}

/// Takes the picture `mode` asks for, with the pointer if `cursor`. It
/// waits for Windows, on a thread of its own.
fn take(mode: Mode, cursor: bool) -> Result<Taken> {
    let area = match mode {
        Mode::All | Mode::Region => CaptureArea::Screen(virtual_screen()),
        Mode::Display => CaptureArea::Display(
            DisplayKey::under_pointer().context("there is no display under the pointer")?,
        ),
        Mode::Window => CaptureArea::Window(front_window().context("no window is in front")?),
        Mode::UnderPointer => CaptureArea::Window(
            window_under_pointer().context("there is no window under the pointer")?,
        ),
    };
    // The windows are read before the picture is taken, so that they are
    // where the picture shows them.
    let windows = match mode {
        Mode::Region => window_frames()
            .into_iter()
            .map(|(_, frame)| frame)
            .collect(),
        _ => Vec::new(),
    };
    // The region tool shows the pointer where it is; the picture it cuts
    // from has it only where it is asked for.
    let shot = Shot::of(capture(area, cursor)?);
    Ok(match mode {
        Mode::Region => Taken::Screen(shot, windows),
        _ => Taken::Shot(shot),
    })
}

/// Does `work` on a thread of its own named `name`; the answer comes on
/// the receiver, which closes without one where the thread could not
/// start.
pub fn away<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> async_channel::Receiver<T> {
    let (answer, answered) = async_channel::bounded(1);
    let started = thread::Builder::new().name(name.to_owned()).spawn(move || {
        answer.send_blocking(work()).ok();
    });
    if let Err(error) = started {
        report!("{name} did not start: {error}");
    }
    answered
}

/// `pixels` as GPUI draws a picture, in the order blue, green, red and
/// alpha.
pub fn render_image(pixels: &RgbaImage) -> Arc<RenderImage> {
    let mut bgra = pixels.clone();
    for pixel in bgra.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(bgra)]))
}

/// The display of GPUI that is at `display`, for a window to open on.
pub(crate) fn gpui_display(display: &crate::platform::Display, cx: &App) -> Option<gpui::DisplayId> {
    let (x, y) = (
        display.frame.left as f32 / display.scale,
        display.frame.top as f32 / display.scale,
    );
    let off = |bounds: gpui::Bounds<gpui::Pixels>| {
        (bounds.origin.x / gpui::px(1.) - x).abs() + (bounds.origin.y / gpui::px(1.) - y).abs()
    };
    let displays = cx.displays();
    let nearest = { displays.iter() }
        .min_by(|first, second| off(first.bounds()).total_cmp(&off(second.bounds())));
    nearest.map(|display| display.id())
}

/// A small copy of `pixels` for a banner.
fn banner_picture(pixels: &RgbaImage) -> Arc<RenderImage> {
    let (width, height) = pixels.dimensions();
    let scale = (BANNER_PICTURE as f32 / width.max(height).max(1) as f32).min(1.);
    let side = |length: u32| ((length as f32 * scale).round() as u32).max(1);
    let small = image::imageops::thumbnail(pixels, side(width), side(height));
    render_image(&small)
}

/// The part of `pixels`, which shows `frame` of the screen, that shows
/// `wanted`; `None` where they do not meet.
pub fn cut(pixels: &RgbaImage, frame: ScreenRect, wanted: ScreenRect) -> Option<RgbaImage> {
    let left = wanted.left.max(frame.left);
    let top = wanted.top.max(frame.top);
    let right = wanted.right.min(frame.right);
    let bottom = wanted.bottom.min(frame.bottom);
    if right <= left || bottom <= top {
        return None;
    }
    let (x, y) = ((left - frame.left) as u32, (top - frame.top) as u32);
    let (width, height) = ((right - left) as u32, (bottom - top) as u32);
    let (x, y) = (x.min(pixels.width()), y.min(pixels.height()));
    let width = width.min(pixels.width() - x);
    let height = height.min(pixels.height() - y);
    (width > 0 && height > 0)
        .then(|| image::imageops::crop_imm(pixels, x, y, width, height).to_image())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn a_request_reads_its_mode_and_what_follows() {
        assert_eq!(Request::parse(None), Ok(Request::new(Mode::All)));
        let request = Request::parse(Some(" region , edit , delay=3 ")).unwrap();
        assert_eq!(request.mode, Mode::Region);
        assert_eq!(request.then, Some(Then::Edit));
        assert_eq!(request.delay, 3);
        assert_eq!(request.arg(), "region,edit,delay=3");
        assert_eq!(
            Request::parse(Some("under-pointer,copy")).unwrap().arg(),
            "under-pointer,copy"
        );
        assert!(Request::parse(Some("now")).is_err());
        assert!(Request::parse(Some("all,delay=11")).is_err());
        assert!(Request::parse(Some("all,edit,save")).is_err());
        assert!(Request::parse(Some("window,soon")).is_err());
        for mode in Mode::all() {
            assert_eq!(Request::parse(Some(mode.name())).unwrap().mode, mode);
        }
    }

    #[test]
    fn a_file_is_named_by_its_time_and_number() {
        let taken = Local.with_ymd_and_hms(2026, 3, 4, 5, 6, 7).unwrap();
        assert_eq!(
            file_name(DEFAULT_NAME, taken, 1, "png"),
            "Screenshot 2026-03-04 050607.png"
        );
        assert_eq!(
            file_name("Screenshot (%N)", taken, 12, "png"),
            "Screenshot (12).png"
        );
        assert_eq!(
            file_name("100%% at %H:%M", taken, 1, "jpg"),
            "100% at 05-06.jpg"
        );
        assert_eq!(
            file_name("", taken, 1, "webp"),
            "Screenshot 2026-03-04 050607.webp"
        );
        assert_eq!(
            file_name("%Q bad", taken, 1, "png"),
            "Screenshot 2026-03-04 050607.png"
        );
        assert_eq!(file_name(" .. ", taken, 1, "png"), "Screenshot.png");
    }

    #[test]
    fn a_name_that_is_taken_gets_a_number() {
        assert_eq!(numbered_name("a.png", 1), "a.png");
        assert_eq!(numbered_name("a.b.png", 3), "a.b (3).png");
        assert_eq!(numbered_name("a", 2), "a (2)");
    }

    #[test]
    fn the_format_follows_the_extension() {
        assert_eq!(
            ShotFormat::of_path(Path::new("x.JPEG")),
            Some(ShotFormat::Jpeg)
        );
        assert_eq!(
            ShotFormat::of_path(Path::new("x.webp")),
            Some(ShotFormat::Webp)
        );
        assert_eq!(ShotFormat::of_path(Path::new("x.bmp")), None);
        assert_eq!(ShotFormat::Jpeg.extension(), "jpg");
    }

    #[test]
    fn a_cut_is_what_both_rectangles_share() {
        let pixels = RgbaImage::from_fn(10, 10, |x, y| image::Rgba([x as u8, y as u8, 0, 255]));
        let frame = ScreenRect {
            left: -5,
            top: 0,
            right: 5,
            bottom: 10,
        };
        let wanted = ScreenRect {
            left: 0,
            top: 2,
            right: 20,
            bottom: 4,
        };
        let cut = cut(&pixels, frame, wanted).unwrap();
        assert_eq!(cut.dimensions(), (5, 2));
        assert_eq!(cut.get_pixel(0, 0).0, [5, 2, 0, 255]);
        let away = ScreenRect {
            left: 50,
            top: 50,
            right: 60,
            bottom: 60,
        };
        assert!(super::cut(&pixels, frame, away).is_none());
    }

    #[test]
    fn the_configuration_reads_its_keys() {
        let config: ScreenshotConfig = toml::from_str(
            "folder = 'D:\\\\Shots'\nformat = 'webp'\nsnip = 'tessera'\nown_print_screen = true",
        )
        .unwrap();
        assert_eq!(config.folder, Some(PathBuf::from("D:\\Shots")));
        assert_eq!(config.format, ShotFormat::Webp);
        assert_eq!(config.snip, Snip::Tessera);
        assert_eq!(config.name, DEFAULT_NAME);
        assert!(toml::from_str::<ScreenshotConfig>("colour = 1").is_err());
    }
}
