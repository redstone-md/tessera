// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The preview pane of a window of the file manager (Alt+P), at the right
//! of the panes, as in Explorer: the file the cursor is on, shown without
//! opening it.
//!
//! Pictures are read by Tessera itself (the `image` crate, and the codecs
//! of Windows for HEIC, AVIF and JPEG XR) and drawn fitted to the pane;
//! text and code show their first 64 KB, in UTF-8 or the UTF-16 a byte
//! order mark says, in a list of lines of which only those in sight are
//! drawn. Every other kind is drawn by the preview handler a program
//! registered for it, in a popup of its own over the pane
//! ([`PreviewHost`]); a handler that does not answer in time is given up
//! without holding the window, and the pane says so.

use std::cell::Cell;
use std::fs::File;
use std::io::Read as _;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui::{
    AnyElement, AsyncApp, Bounds, Context, InteractiveElement, IntoElement, ObjectFit,
    ParentElement, Pixels, Render, RenderImage, SharedString, Styled, StyledImage, Subscription,
    Task, WeakEntity, Window, canvas, div, img, px, uniform_list,
};

use super::model::Entry;
use super::pane::within;
use super::thumbs::FileViewConfig;
use crate::i18n::Language;
use crate::platform::{PreviewColors, PreviewHost, PreviewShown, decode_with_wic};
use crate::theme::Theme;
use crate::ui::one_line;

/// How wide the pane is, in logical pixels.
pub const PREVIEW_WIDTH: f32 = 340.;

/// How much of a text is read: what Explorer's preview of text shows.
const TEXT_READ: usize = 64 * 1024;

/// The longest side a picture is drawn at, in pixels; larger ones are
/// made smaller as they are read.
const PICTURE_SIDE: u32 = 1600;

/// How tall a line of text is drawn.
const LINE_HEIGHT: f32 = 18.;

/// The font text is drawn in, which every Windows has.
const MONOSPACE: &str = "Consolas";

/// The kinds of pictures Tessera reads itself, and those the codecs of
/// Windows read.
const PICTURES: [&str; 10] = [
    "png", "jpg", "jpeg", "jfif", "gif", "bmp", "webp", "tif", "tiff", "ico",
];
const WINDOWS_PICTURES: [&str; 5] = ["heic", "heif", "avif", "jxr", "wdp"];

/// The kinds that are text, whatever they hold.
const TEXTS: [&str; 46] = [
    "txt",
    "md",
    "markdown",
    "log",
    "ini",
    "cfg",
    "conf",
    "toml",
    "yaml",
    "yml",
    "json",
    "jsonc",
    "xml",
    "csv",
    "tsv",
    "rs",
    "c",
    "h",
    "cpp",
    "hpp",
    "cc",
    "cs",
    "java",
    "kt",
    "go",
    "py",
    "rb",
    "js",
    "mjs",
    "ts",
    "tsx",
    "jsx",
    "css",
    "scss",
    "html",
    "htm",
    "sh",
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "sql",
    "lua",
    "php",
    "swift",
    "gitignore",
];

/// What the pane shows.
enum Shown {
    /// Nothing is chosen, or a folder.
    Nothing,
    Reading,
    Picture(Arc<RenderImage>),
    Text {
        lines: Arc<Vec<SharedString>>,
        /// Whether the file goes on past what was read.
        cut: bool,
    },
    /// What the handler of a program draws over the pane.
    Handler(HandlerState),
    /// Why it cannot be shown.
    Unavailable(&'static str),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HandlerState {
    Waiting,
    Drawn,
}

/// What the pane reads a file as, by its kind and its first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reading {
    Picture,
    WindowsPicture,
    Text,
    Handler,
}

/// The preview pane of a window.
pub struct PreviewPane {
    /// The file shown, with when it was changed, so that it is read again
    /// when it changes.
    file: Option<(PathBuf, Option<SystemTime>)>,
    shown: Shown,
    host: Option<PreviewHost>,
    reading: Option<Task<()>>,
    /// Where the pane was drawn last, in the coordinates of the window.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Whether the pane is shown.
    open: bool,
    _moves: Option<Subscription>,
}

impl PreviewPane {
    pub fn new(open: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The popup of a handler follows the window as it moves.
        let moves = cx.observe_window_bounds(window, |this, window, _| this.place(window));
        Self {
            file: None,
            shown: Shown::Nothing,
            host: None,
            reading: None,
            bounds: Rc::new(Cell::new(None)),
            open,
            _moves: Some(moves),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Shows the pane, or hides it and lets go of what it shows.
    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        if !open {
            self.clear();
        }
        cx.notify();
    }

    /// Lets go of the file shown, and hides the popup of its handler.
    fn clear(&mut self) {
        self.file = None;
        self.shown = Shown::Nothing;
        self.reading = None;
        if let Some(host) = &mut self.host {
            host.unload();
            host.hide();
        }
    }

    /// Shows `entry`, the one the cursor is on, where it is not shown yet.
    pub fn show(&mut self, entry: Option<&Entry>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        let file = entry
            .filter(|entry| !entry.folder)
            .map(|entry| (entry.path.clone(), entry.modified));
        if file == self.file {
            return;
        }
        self.clear();
        let Some((path, modified)) = file else {
            cx.notify();
            return;
        };
        self.file = Some((path.clone(), modified));
        let config = FileViewConfig::of(cx);
        self.shown = Shown::Reading;
        let reading = reading_of(&path);
        let read = cx.background_executor().spawn({
            let path = path.clone();
            async move { read(&path, reading) }
        });
        self.reading = Some(cx.spawn_in(window, async move |this, cx| {
            let read = read.await;
            this.update_in(cx, |this, window, cx| {
                this.reading = None;
                match read {
                    Read::Shown(shown) => this.shown = shown,
                    Read::Handler if config.preview_handlers => {
                        this.draw_with_handler(path, config, window, cx)
                    }
                    Read::Handler => {
                        this.shown = Shown::Unavailable("There is no preview of this kind of file")
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Has the preview handler of the kind of `path` draw it, waiting for
    /// it as long as the configuration says before it is given up.
    fn draw_with_handler(
        &mut self,
        path: PathBuf,
        config: FileViewConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.host.is_none() {
            match PreviewHost::new(window) {
                Ok(host) => self.host = Some(host),
                Err(error) => {
                    self.shown = Shown::Failed(format!("{error:#}"));
                    return;
                }
            }
        }
        let Some(host) = &mut self.host else {
            return;
        };
        let theme = cx.global::<Theme>();
        let colors = PreviewColors {
            background: rgb_of(theme.colors.surface_container),
            text: rgb_of(theme.colors.on_surface),
        };
        let answer = host.show(path, colors);
        self.shown = Shown::Handler(HandlerState::Waiting);
        self.place(window);
        let wait = Duration::from_secs_f32(config.preview_timeout.clamp(1., 60.));
        self.reading = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let until = std::time::Instant::now() + wait;
                let shown = within(answer, until, cx).await;
                this.update(cx, |this, cx| {
                    this.reading = None;
                    match shown {
                        Some(PreviewShown::Drawn) => {
                            this.shown = Shown::Handler(HandlerState::Drawn);
                        }
                        Some(PreviewShown::NoHandler) => {
                            this.hide_host();
                            this.shown =
                                Shown::Unavailable("There is no preview of this kind of file");
                        }
                        Some(PreviewShown::Failed(error)) => {
                            this.hide_host();
                            this.shown = Shown::Failed(error);
                        }
                        None => {
                            // The handler hangs: its popup is given up, and the
                            // next file gets a new one.
                            if let Some(host) = this.host.take() {
                                host.abandon();
                            }
                            this.shown = Shown::Unavailable("The preview handler did not answer");
                        }
                    }
                    cx.notify();
                })
                .ok();
            }),
        );
    }

    fn hide_host(&mut self) {
        if let Some(host) = &mut self.host {
            host.unload();
            host.hide();
        }
    }

    /// Puts the popup of the handler over the pane, where a handler draws
    /// and the pane is shown.
    fn place(&mut self, window: &mut Window) {
        let drawing = matches!(self.shown, Shown::Handler(_)) && self.open;
        let Some(host) = &mut self.host else {
            return;
        };
        match (drawing, self.bounds.get()) {
            (true, Some(bounds)) if bounds.size.width > px(1.) => host.place(window, bounds),
            _ => host.hide(),
        }
    }

    fn render_text(&self, lines: Arc<Vec<SharedString>>, theme: &Theme) -> AnyElement {
        let count = lines.len();
        let color = theme.colors.on_surface;
        uniform_list(
            "files-preview-text",
            count,
            move |range: Range<usize>, _, _| {
                range
                    .map(|index| {
                        div()
                            .h(px(LINE_HEIGHT))
                            .whitespace_nowrap()
                            .text_color(color)
                            .child(lines[index].clone())
                    })
                    .collect()
            },
        )
        .size_full()
        .font_family(MONOSPACE)
        .text_size(px(12.))
        .into_any_element()
    }
}

impl Render for PreviewPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        if !self.open {
            return div().into_any_element();
        }
        let note = |text: String| {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p(theme.spacing.medium)
                .text_color(theme.colors.on_surface_variant)
                .text_size(theme.typography.label_size)
                .child(text)
                .into_any_element()
        };
        let body = match &self.shown {
            Shown::Nothing => note(language.say("Select a file to preview it").to_owned()),
            Shown::Reading => note(language.say("Reading…").to_owned()),
            Shown::Picture(image) => div()
                .size_full()
                .p(theme.spacing.small)
                .child(
                    img(image.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            Shown::Text { lines, cut } => {
                let cut = cut.then(|| {
                    div()
                        .flex_none()
                        .pt(theme.spacing.tiny)
                        .text_size(theme.typography.label_size * 0.85)
                        .text_color(theme.colors.on_surface_variant)
                        .child(language.say("Only the beginning of the file is shown"))
                });
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .p(theme.spacing.small)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_text(lines.clone(), &theme)),
                    )
                    .children(cut)
                    .into_any_element()
            }
            Shown::Handler(state) => {
                let bounds = self.bounds.clone();
                let entity = cx.entity().downgrade();
                let waiting = (*state == HandlerState::Waiting)
                    .then(|| note(language.say("Reading…").to_owned()));
                div()
                    .size_full()
                    .relative()
                    .children(waiting)
                    .child(
                        canvas(
                            move |area, window, cx| {
                                if bounds.get() != Some(area) {
                                    bounds.set(Some(area));
                                    entity.update(cx, |this, _| this.place(window)).ok();
                                }
                            },
                            |_, (), _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .into_any_element()
            }
            Shown::Unavailable(text) => note(language.say(text).to_owned()),
            Shown::Failed(error) => note(format!(
                "{}: {error}",
                language.say("The preview cannot be shown")
            )),
        };
        // Where no handler draws, its popup is hidden at once.
        if !matches!(self.shown, Shown::Handler(_)) {
            self.place(window);
        }
        let title = self
            .file
            .as_ref()
            .and_then(|(path, _)| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        div()
            .id("files-preview")
            .w(px(PREVIEW_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .mr(theme.spacing.small)
            .rounded(theme.shape.large)
            .overflow_hidden()
            .bg(theme.colors.surface_container_highest.opacity(0.35))
            .child(
                div()
                    .flex_none()
                    .px(theme.spacing.medium)
                    .py(theme.spacing.small)
                    .text_size(theme.typography.label_size)
                    .text_color(theme.colors.on_surface)
                    .child(one_line(title)),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

/// How `path` is read, by its kind.
fn reading_of(path: &Path) -> Reading {
    let kind = { path.extension() }
        .map(|kind| kind.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if PICTURES.contains(&kind.as_str()) {
        Reading::Picture
    } else if WINDOWS_PICTURES.contains(&kind.as_str()) {
        Reading::WindowsPicture
    } else if TEXTS.contains(&kind.as_str()) {
        Reading::Text
    } else {
        Reading::Handler
    }
}

/// What reading a file gave.
enum Read {
    Shown(Shown),
    /// It is for the handler of its kind to draw.
    Handler,
}

/// Reads the file at `path` as `reading` says, away from the user
/// interface. A file whose kind is not known as text is shown as text
/// where its beginning is text.
fn read(path: &Path, reading: Reading) -> Read {
    match reading {
        Reading::Picture => Read::Shown(picture(path)),
        Reading::WindowsPicture => Read::Shown(windows_picture(path)),
        Reading::Text => Read::Shown(text(path, true)),
        Reading::Handler => match text(path, false) {
            Shown::Text { lines, cut } => Read::Shown(Shown::Text { lines, cut }),
            _ => Read::Handler,
        },
    }
}

/// A picture Tessera reads itself, made smaller where it is large.
fn picture(path: &Path) -> Shown {
    let decoded = image::ImageReader::open(path)
        .map_err(|error| error.to_string())
        .and_then(|reader| {
            reader
                .with_guessed_format()
                .map_err(|error| error.to_string())
        })
        .and_then(|reader| reader.decode().map_err(|error| error.to_string()));
    match decoded {
        Ok(picture) => {
            let picture = match picture.width().max(picture.height()) > PICTURE_SIDE {
                true => picture.thumbnail(PICTURE_SIDE, PICTURE_SIDE),
                false => picture,
            };
            let mut pixels = picture.into_rgba8();
            for pixel in pixels.pixels_mut() {
                pixel.0.swap(0, 2);
            }
            Shown::Picture(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
        }
        Err(error) => Shown::Failed(error),
    }
}

/// A picture the codecs of Windows read.
fn windows_picture(path: &Path) -> Shown {
    match decode_with_wic(path) {
        Ok((width, height, rgb)) => {
            let mut bgra = Vec::with_capacity(rgb.len() / 3 * 4);
            for pixel in rgb.chunks_exact(3) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 0xff]);
            }
            let Some(buffer) = image::RgbaImage::from_raw(width, height, bgra) else {
                return Shown::Failed("the picture has no pixels".to_owned());
            };
            let mut buffer = image::DynamicImage::ImageRgba8(buffer);
            if width.max(height) > PICTURE_SIDE {
                buffer = buffer.thumbnail(PICTURE_SIDE, PICTURE_SIDE);
            }
            let frame = image::Frame::new(buffer.into_rgba8());
            Shown::Picture(Arc::new(RenderImage::new(vec![frame])))
        }
        Err(error) => Shown::Failed(format!("{error:#}")),
    }
}

/// The beginning of the file at `path` as lines of text; where `surely`
/// is false, only where it is text, else [`Shown::Nothing`].
fn text(path: &Path, surely: bool) -> Shown {
    let mut bytes = Vec::with_capacity(TEXT_READ);
    let read = File::open(path).and_then(|file| {
        file.take(TEXT_READ as u64 + 1).read_to_end(&mut bytes)?;
        Ok(())
    });
    if let Err(error) = read {
        return Shown::Failed(error.to_string());
    }
    let cut = bytes.len() > TEXT_READ;
    bytes.truncate(TEXT_READ);
    match decode_text(&bytes, surely) {
        Some(text) => Shown::Text {
            lines: Arc::new(lines_of(&text)),
            cut,
        },
        None => Shown::Nothing,
    }
}

/// `bytes` as text: UTF-16 where a byte order mark says so, else UTF-8,
/// with what is not replaced. Where `surely` is false, `None` for bytes
/// that are not text: with a NUL in them, or more than a few that are not
/// UTF-8.
pub fn decode_text(bytes: &[u8], surely: bool) -> Option<String> {
    let utf16 = |bytes: &[u8], little: bool| {
        let units: Vec<u16> = { bytes.chunks_exact(2) }
            .map(|pair| match little {
                true => u16::from_le_bytes([pair[0], pair[1]]),
                false => u16::from_be_bytes([pair[0], pair[1]]),
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xff, 0xfe, rest @ ..] => return Some(utf16(rest, true)),
        [0xfe, 0xff, rest @ ..] => return Some(utf16(rest, false)),
        _ => {}
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let text = String::from_utf8_lossy(bytes);
    if !surely {
        let broken = text.chars().filter(|letter| *letter == '\u{fffd}').count();
        // A character cut at the end of what was read is no sign.
        if bytes.contains(&0) || broken > 4 {
            return None;
        }
    }
    Some(text.into_owned())
}

/// The lines of `text`, with tabs as four spaces, as the list draws them.
fn lines_of(text: &str) -> Vec<SharedString> {
    { text.lines() }
        .map(|line| SharedString::from(line.replace('\t', "    ")))
        .collect()
}

/// A color as `0xRRGGBB`.
fn rgb_of(color: gpui::Hsla) -> u32 {
    let color = gpui::Rgba::from(color);
    let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u32;
    (channel(color.r) << 16) | (channel(color.g) << 8) | channel(color.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_read_as_its_byte_order_mark_says() {
        let utf16: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain("Hé".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(decode_text(&utf16, false).as_deref(), Some("Hé"));
        assert_eq!(
            decode_text(&[0xef, 0xbb, 0xbf, b'a'], false).as_deref(),
            Some("a")
        );
        assert_eq!(decode_text(b"plain", false).as_deref(), Some("plain"));
    }

    #[test]
    fn what_is_no_text_is_not_shown_as_text() {
        assert_eq!(decode_text(&[0x4d, 0x5a, 0x00, 0x90], false), None);
        assert!(decode_text(&[0x4d, 0x5a, 0x00, 0x90], true).is_some());
    }

    #[test]
    fn kinds_are_read_as_pictures_text_or_by_their_handlers() {
        assert_eq!(reading_of(Path::new("a.JPG")), Reading::Picture);
        assert_eq!(reading_of(Path::new("a.heic")), Reading::WindowsPicture);
        assert_eq!(reading_of(Path::new("main.rs")), Reading::Text);
        assert_eq!(reading_of(Path::new("report.pdf")), Reading::Handler);
        assert_eq!(lines_of("a\tb\nc"), ["a    b", "c"]);
    }
}
