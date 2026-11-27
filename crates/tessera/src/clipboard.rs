// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The history of the clipboard that Tessera keeps while it runs: texts,
//! lists of files and pictures, newest first, each to be put back on the
//! clipboard and pasted into the window the user was in. Pinned items stay
//! when the history is cleared and when Tessera starts again; the others
//! last until Tessera exits, as in the history of Windows.
//!
//! Texts and lists of files are kept in memory. Pictures are kept as PNG
//! files in `cache/clipboard` of the configuration directory, with a small
//! copy in memory to show; the file of a picture is deleted when the
//! picture leaves the history, and those left from an earlier run when
//! Tessera starts, except those of pinned pictures. The pinned items are
//! written to `clipboard.toml` beside the configuration.
//!
//! The popup that shows the history is in `modules/clipboard.rs`.

use std::collections::HashSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Local, TimeZone};
use gpui::{
    App, AppContext, AsyncApp, Context, Entity, Global, RenderImage, SharedString, Task, WeakEntity,
};
use image::{DynamicImage, ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::config_dir;
use crate::platform::{AppWindowId, Clip, ClipboardWatch, Launch, Offer, paste_into};
use crate::popup::Popups;

/// How many items are kept besides the pinned ones at the most.
const MOST_KEPT: usize = 200;

/// The file the pinned items are written to, in the configuration
/// directory, and the directory of the pictures in it.
const PINS_FILE: &str = "clipboard.toml";
const PICTURES: &str = "cache/clipboard";

/// How many characters of a text a row shows at the most, and how many
/// the search looks through.
const PREVIEW_CHARS: usize = 300;
const SEARCHED_CHARS: usize = 20_000;

/// The size the copy of a picture that is shown fits in, in pixels: twice
/// what a row shows, for displays at twice the density.
const THUMBNAIL: (u32, u32) = (288, 112);

/// How long after the window in front was noted it is still the one to
/// paste into when the history opens.
const AIM_FRESH: Duration = Duration::from_secs(3);

/// What `[clipboard]` of the configuration says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ClipboardConfig {
    /// Whether what is copied is kept in the history.
    pub enabled: bool,
    /// How many items are kept besides the pinned ones, 1 to 200.
    pub size: usize,
    /// Whether pictures are kept.
    pub images: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            size: 25,
            images: true,
        }
    }
}

/// A picture of the history.
#[derive(Clone, Debug)]
pub struct Picture {
    /// The PNG file that holds it.
    pub file: PathBuf,
    pub width: u32,
    pub height: u32,
    /// A small copy to show, where one could be made.
    pub thumbnail: Option<Arc<RenderImage>>,
}

/// What an item of the history holds.
#[derive(Clone, Debug)]
pub enum Content {
    Text(String),
    Files(Vec<PathBuf>),
    Image(Picture),
}

impl Content {
    /// The address this is, when it is a text that is one web address and
    /// nothing else.
    pub fn link(&self) -> Option<&str> {
        let Self::Text(text) = self else {
            return None;
        };
        let text = text.trim();
        let scheme = ["https://", "http://"]
            .into_iter()
            .any(|scheme| text.len() > scheme.len() && text.starts_with(scheme));
        (scheme && !text.contains(char::is_whitespace)).then_some(text)
    }

    /// What a row shows of it: the beginning of a text as one line, or the
    /// names of the first files, at most [`PREVIEW_CHARS`] characters
    /// either way; the caption of a row tells how many files there are.
    fn preview(&self) -> String {
        match self {
            Self::Text(text) => shortened(text.split_whitespace(), " "),
            Self::Files(files) => shortened(files.iter().map(|path| file_name(path)), ", "),
            Self::Image(_) => String::new(),
        }
    }

    /// What the search looks through, in lowercase.
    fn searched(&self) -> String {
        match self {
            Self::Text(text) => text.chars().take(SEARCHED_CHARS).collect::<String>(),
            Self::Files(files) => {
                let paths = files.iter().map(|path| path.to_string_lossy());
                paths.collect::<Vec<_>>().join("\n")
            }
            Self::Image(_) => String::new(),
        }
        .to_lowercase()
    }

    /// The picture file of it, if it is a picture.
    fn file(&self) -> Option<&Path> {
        match self {
            Self::Image(picture) => Some(&picture.file),
            _ => None,
        }
    }
}

/// `parts` with `between` between them, cut after [`PREVIEW_CHARS`]
/// characters; the parts after those are not looked at.
fn shortened(parts: impl IntoIterator<Item = impl AsRef<str>>, between: &str) -> String {
    let mut preview = String::new();
    let mut left = PREVIEW_CHARS;
    for (index, part) in parts.into_iter().enumerate() {
        let before = if index == 0 { "" } else { between };
        for character in before.chars().chain(part.as_ref().chars()) {
            if left == 0 {
                return preview;
            }
            preview.push(character);
            left -= 1;
        }
    }
    preview
}

/// The name a list of files shows of `path`: its last part, or all of it
/// for a drive.
pub fn file_name(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.to_string_lossy().into_owned(),
    }
}

/// What tells two items apart: a hash of what they hold.
fn key_of(kind: u8, parts: &[&[u8]]) -> String {
    let mut sum = Sha256::new();
    sum.update([kind]);
    for part in parts {
        sum.update((part.len() as u64).to_le_bytes());
        sum.update(part);
    }
    let digest = sum.finalize();
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn text_key(text: &str) -> String {
    key_of(b't', &[text.as_bytes()])
}

fn files_key(files: &[PathBuf]) -> String {
    let names: Vec<String> = { files.iter() }
        .map(|path| path.to_string_lossy().to_lowercase())
        .collect();
    let parts: Vec<&[u8]> = names.iter().map(|name| name.as_bytes()).collect();
    key_of(b'f', &parts)
}

fn image_key(pixels: &RgbaImage) -> String {
    let (width, height) = pixels.dimensions();
    key_of(
        b'i',
        &[&width.to_le_bytes(), &height.to_le_bytes(), pixels.as_raw()],
    )
}

/// An item of the history.
#[derive(Clone, Debug)]
pub struct Entry {
    /// What the item is known by while Tessera runs.
    pub id: u64,
    key: String,
    pub content: Content,
    /// When it was copied last.
    pub copied: DateTime<Local>,
    pub pinned: bool,
    /// What its row shows of it.
    pub preview: SharedString,
    searched: String,
}

/// The items of the history, newest first, and what is done with them;
/// what falls out of it is handed back, for its picture to be deleted.
#[derive(Default)]
pub struct History {
    entries: Vec<Entry>,
    next: u64,
}

impl History {
    /// The items, newest first.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    fn position(&self, id: u64) -> Option<usize> {
        self.entries.iter().position(|entry| entry.id == id)
    }

    fn entry(&mut self, key: String, content: Content, copied: DateTime<Local>) -> Entry {
        self.next += 1;
        Entry {
            id: self.next,
            key,
            preview: content.preview().into(),
            searched: content.searched(),
            content,
            copied,
            pinned: false,
        }
    }

    /// Adds what was copied at `copied` as the newest item, keeping
    /// `size` items besides the pinned ones. What the history holds
    /// already comes to the front instead, and keeps its own content and
    /// pin; the content that was given then falls out, with the items
    /// beyond `size`.
    fn add(
        &mut self,
        key: String,
        content: Content,
        copied: DateTime<Local>,
        size: usize,
    ) -> Vec<Content> {
        if let Some(at) = self.entries.iter().position(|entry| entry.key == key) {
            let mut entry = self.entries.remove(at);
            entry.copied = copied;
            self.entries.insert(0, entry);
            return vec![content];
        }
        let entry = self.entry(key, content, copied);
        self.entries.insert(0, entry);
        self.trim(size)
    }

    /// Adds a pinned item of an earlier run where its time puts it, unless
    /// the history holds it already, in which case it is pinned there and
    /// the content given falls out.
    fn restore(&mut self, key: String, content: Content, copied: DateTime<Local>) -> Vec<Content> {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.key == key) {
            entry.pinned = true;
            return vec![content];
        }
        let mut entry = self.entry(key, content, copied);
        entry.pinned = true;
        let at = { self.entries.iter() }
            .position(|other| other.copied < copied)
            .unwrap_or(self.entries.len());
        self.entries.insert(at, entry);
        Vec::new()
    }

    /// Makes the item `id` the newest, copied at `copied`.
    fn bring_forward(&mut self, id: u64, copied: DateTime<Local>) {
        if let Some(at) = self.position(id) {
            let mut entry = self.entries.remove(at);
            entry.copied = copied;
            self.entries.insert(0, entry);
        }
    }

    /// Takes the item `id` out, pinned or not.
    fn remove(&mut self, id: u64) -> Option<Entry> {
        Some(self.entries.remove(self.position(id)?))
    }

    /// Takes out every item that is not pinned.
    fn clear(&mut self) -> Vec<Content> {
        let (pinned, others) = std::mem::take(&mut self.entries)
            .into_iter()
            .partition(|entry| entry.pinned);
        self.entries = pinned;
        others.into_iter().map(|entry| entry.content).collect()
    }

    /// Pins the item `id`, or unpins it, and says whether it is pinned now.
    fn toggle_pin(&mut self, id: u64) -> Option<bool> {
        let at = self.position(id)?;
        let entry = &mut self.entries[at];
        entry.pinned = !entry.pinned;
        Some(entry.pinned)
    }

    /// Takes out the oldest items that are not pinned until `size` of them
    /// are left.
    fn trim(&mut self, size: usize) -> Vec<Content> {
        let mut kept = 0;
        let mut dropped = Vec::new();
        let mut index = 0;
        while index < self.entries.len() {
            if self.entries[index].pinned {
                index += 1;
                continue;
            }
            kept += 1;
            if kept <= size {
                index += 1;
            } else {
                dropped.push(self.entries.remove(index).content);
            }
        }
        dropped
    }

    /// The positions of the items in which every word of `query` is
    /// found, ignoring case, newest first; all of them for an empty query.
    /// A picture is found by none.
    pub fn matching(&self, query: &str) -> Vec<usize> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        { self.entries.iter().enumerate() }
            .filter(|(_, entry)| words.iter().all(|word| entry.searched.contains(word)))
            .map(|(index, _)| index)
            .collect()
    }

    /// The pinned items as the pins file keeps them, newest first.
    fn pins(&self) -> PinsFile {
        let pins = { self.entries.iter() }
            .filter(|entry| entry.pinned)
            .map(|entry| {
                let mut pin = Pin {
                    copied: entry.copied.timestamp(),
                    ..Pin::default()
                };
                match &entry.content {
                    Content::Text(text) => pin.text = Some(text.clone()),
                    Content::Files(files) => pin.files = Some(files.clone()),
                    Content::Image(picture) => pin.image = Some(file_name(&picture.file)),
                }
                pin
            })
            .collect();
        PinsFile { pins }
    }
}

/// What `clipboard.toml` holds: the pinned items, newest first.
#[derive(Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PinsFile {
    #[serde(default, rename = "pin")]
    pins: Vec<Pin>,
}

/// A pinned item: one of a text, files, or the name of a picture file in
/// the directory of the pictures, and when it was copied last, in seconds
/// since 1970.
#[derive(Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    files: Option<Vec<PathBuf>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image: Option<String>,
    #[serde(default)]
    copied: i64,
}

/// What a copy comes to once it is ready to be added: what tells it apart
/// and what it holds.
struct Kept {
    key: String,
    content: Content,
}

impl Kept {
    /// What `clip` is kept as: a picture is written to a file of its own
    /// in `pictures` and a small copy is made to show; this takes a while,
    /// and is done away from the user interface.
    fn of(clip: Clip, pictures: Option<&Path>) -> Result<Self, String> {
        Ok(match clip {
            Clip::Text(text) => Self {
                key: text_key(&text),
                content: Content::Text(text),
            },
            Clip::Files(files) => Self {
                key: files_key(&files),
                content: Content::Files(files),
            },
            Clip::Image(pixels) => {
                let pictures = pictures.ok_or("there is no directory to keep pictures in")?;
                let key = image_key(&pixels);
                let file = pictures.join(format!("{key}-{}.png", unique()));
                let failed = |error: &dyn std::fmt::Display| {
                    format!("{} was not written: {error}", file.display())
                };
                fs::create_dir_all(pictures).map_err(|error| failed(&error))?;
                pixels
                    .save_with_format(&file, ImageFormat::Png)
                    .map_err(|error| failed(&error))?;
                Self {
                    key,
                    content: Content::Image(Picture {
                        file,
                        width: pixels.width(),
                        height: pixels.height(),
                        thumbnail: thumbnail(&pixels),
                    }),
                }
            }
        })
    }
}

/// A number no two calls give while Tessera runs, nor in two runs: what
/// the file of each picture is told apart by, so that no two items share
/// one and the one leaving never takes the file of the one coming.
fn unique() -> String {
    static COUNT: Mutex<u64> = Mutex::new(0);
    let mut count = COUNT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *count += 1;
    let since = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:x}{:04x}", since.as_millis(), *count)
}

/// A copy of `pixels` small enough for a row, in the byte order GPUI
/// draws.
fn thumbnail(pixels: &RgbaImage) -> Option<Arc<RenderImage>> {
    let (width, height) = fitted(pixels.dimensions(), THUMBNAIL);
    let mut small = image::imageops::thumbnail(pixels, width, height);
    for pixel in small.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(small)])))
}

/// The size of a picture of `size` made to fit in `room` without changing
/// its shape; a picture that fits already keeps its size, and no side is
/// made smaller than one pixel.
fn fitted(size: (u32, u32), room: (u32, u32)) -> (u32, u32) {
    let (width, height) = (size.0.max(1) as f64, size.1.max(1) as f64);
    let scale = (room.0 as f64 / width).min(room.1 as f64 / height).min(1.);
    let side = |length: f64| ((length * scale).round() as u32).max(1);
    (side(width), side(height))
}

/// Where the pinned items are written.
fn pins_path() -> Option<PathBuf> {
    Some(config_dir()?.join(PINS_FILE))
}

/// Where the pictures are kept.
fn pictures_path() -> Option<PathBuf> {
    Some(config_dir()?.join(PICTURES))
}

/// Writes `pins` to the pins file, unless pins given later were written
/// already: the writes are done away from the user interface and may
/// overtake each other. `generation` counts the writes asked for.
fn write_pins(pins: &PinsFile, generation: u64) -> Result<(), String> {
    static WRITTEN: Mutex<u64> = Mutex::new(0);
    let mut written = WRITTEN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if generation <= *written {
        return Ok(());
    }
    let path = pins_path().ok_or("there is no configuration directory")?;
    let failed =
        |error: &dyn std::fmt::Display| format!("{} was not written: {error}", path.display());
    let text = toml::to_string(pins).map_err(|error| failed(&error))?;
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory).map_err(|error| failed(&error))?;
    }
    // Written beside it first, so that a crash never leaves half a file.
    let fresh = path.with_extension("toml.new");
    fs::write(&fresh, text).map_err(|error| failed(&error))?;
    fs::rename(&fresh, &path).map_err(|error| failed(&error))?;
    *written = generation;
    Ok(())
}

/// The pinned items of an earlier run, ready to be added, oldest first.
/// The pictures in the directory of the pictures that were written before
/// `started` and are not pinned are deleted: they are what an earlier run
/// left. A pins file that cannot be read is reported and left alone, with
/// the pictures: `None` then says that it is not to be written while
/// Tessera runs, so that the pins in it are there to be read again once
/// it is mended.
fn restore(started: SystemTime) -> Option<Vec<(Kept, DateTime<Local>)>> {
    let not_kept = "pins made now are not kept until Tessera starts again";
    let pins = match pins_path().map(fs::read_to_string) {
        None => return None,
        Some(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => PinsFile::default(),
        Some(Err(error)) => {
            report!("the pinned items of the clipboard are not read, and {not_kept}: {error}");
            return None;
        }
        Some(Ok(text)) => match toml::from_str::<PinsFile>(&text) {
            Ok(pins) => pins,
            Err(error) => {
                report!("{PINS_FILE} is ignored and left as it is, and {not_kept}: {error}");
                return None;
            }
        },
    };
    let pictures = pictures_path();
    let mut kept_files = HashSet::new();
    let mut restored = Vec::new();
    for pin in pins.pins.into_iter().rev() {
        let copied = Local
            .timestamp_opt(pin.copied, 0)
            .single()
            .unwrap_or_else(Local::now);
        let kept = match (pin.text, pin.files, pin.image, &pictures) {
            (Some(text), None, None, _) => Kept {
                key: text_key(&text),
                content: Content::Text(text),
            },
            (None, Some(files), None, _) => Kept {
                key: files_key(&files),
                content: Content::Files(files),
            },
            (None, None, Some(name), Some(pictures)) => match restore_picture(pictures, &name) {
                Ok(kept) => {
                    kept_files.insert(name);
                    kept
                }
                Err(error) => {
                    report!("a pinned picture of the clipboard is lost: {error}");
                    continue;
                }
            },
            _ => {
                report!("a pin of {PINS_FILE} is not one text, files or picture, and is ignored");
                continue;
            }
        };
        restored.push((kept, copied));
    }
    if let Some(pictures) = &pictures {
        delete_left_pictures(pictures, &kept_files, started);
    }
    Some(restored)
}

/// The pinned picture `name` of the directory `pictures`.
fn restore_picture(pictures: &Path, name: &str) -> Result<Kept, String> {
    // A name with a directory in it leads elsewhere.
    if Path::new(name).file_name() != Some(std::ffi::OsStr::new(name)) {
        return Err(format!("`{name}` is no file name"));
    }
    let file = pictures.join(name);
    let pixels = image::open(&file)
        .map_err(|error| format!("{}: {error}", file.display()))?
        .into_rgba8();
    Ok(Kept {
        key: image_key(&pixels),
        content: Content::Image(Picture {
            file,
            width: pixels.width(),
            height: pixels.height(),
            thumbnail: thumbnail(&pixels),
        }),
    })
}

/// Deletes the PNG files of `pictures` written before `started` that are
/// not in `kept`.
fn delete_left_pictures(pictures: &Path, kept: &HashSet<String>, started: SystemTime) {
    let Ok(files) = fs::read_dir(pictures) else {
        return;
    };
    for file in files.flatten() {
        let name = file.file_name().to_string_lossy().into_owned();
        let before = file
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified < started);
        if name.ends_with(".png") && before && !kept.contains(&name) {
            fs::remove_file(file.path()).ok();
        }
    }
}

/// What to put on the clipboard for `content`: for a picture its file is
/// read again.
fn offer_of(content: &Content) -> Result<Offer, String> {
    Ok(match content {
        Content::Text(text) => Offer::text(text),
        Content::Files(files) => Offer::files(files),
        Content::Image(picture) => {
            let failed = |error: &dyn std::fmt::Display| {
                format!(
                    "the picture {} is not there: {error}",
                    picture.file.display()
                )
            };
            let png = fs::read(&picture.file).map_err(|error| failed(&error))?;
            let pixels = image::load(Cursor::new(&png), ImageFormat::Png)
                .map(DynamicImage::into_rgba8)
                .map_err(|error| failed(&error))?;
            Offer::image(&pixels, png)
        }
    })
}

/// Deletes the pictures of `contents`, away from the user interface.
fn forget(contents: Vec<Content>, cx: &App) {
    let files: Vec<PathBuf> = { contents.iter() }
        .filter_map(|content| content.file().map(Path::to_owned))
        .collect();
    if files.is_empty() {
        return;
    }
    cx.background_executor()
        .spawn(async move {
            for file in files {
                fs::remove_file(file).ok();
            }
        })
        .detach();
}

/// The history of the clipboard, shared by every view of it.
pub struct ClipHistory {
    history: History,
    config: ClipboardConfig,
    /// The watch of the clipboard while the history is on.
    watch: Option<ClipboardWatch>,
    /// What takes in what the watch reads.
    taking: Option<Task<()>>,
    /// The window that was in front when the history was asked for last,
    /// and when.
    aim: Option<(AppWindowId, Instant)>,
    /// Whether the pinned items of an earlier run were read; before, and
    /// for good when the pins file could not be read, it is not written,
    /// so that it does not lose them.
    restored: bool,
    /// Whether the pins changed before those of the earlier run were
    /// read, so that the pins file is to be written once they are.
    pins_dirty: bool,
    /// How many writes of the pins file were asked for.
    pin_writes: u64,
}

struct SharedHistory(Entity<ClipHistory>);

impl Global for SharedHistory {}

impl ClipHistory {
    /// Starts the history as `config` says, and reads the pinned items of
    /// the earlier run.
    pub fn start(config: &ClipboardConfig, cx: &mut App) {
        let history = cx.new(|cx| {
            Self::read_pins(cx);
            Self {
                history: History::default(),
                config: ClipboardConfig {
                    enabled: false,
                    ..*config
                },
                watch: None,
                taking: None,
                aim: None,
                restored: false,
                pins_dirty: false,
                pin_writes: 0,
            }
        });
        cx.set_global(SharedHistory(history));
        Self::configure(config, cx);
    }

    /// Takes over a new `[clipboard]`: switching the history off ends the
    /// watch and forgets what is not pinned.
    pub fn configure(config: &ClipboardConfig, cx: &mut App) {
        if let Some(history) = Self::shared(cx) {
            history.update(cx, |this, cx| this.apply(*config, cx));
        }
    }

    /// The history, once it is started.
    pub fn shared(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<SharedHistory>()
            .map(|shared| shared.0.clone())
    }

    /// Notes the window in front as the one to paste into when the history
    /// opens next, unless a popup of Tessera is in front.
    pub fn aim(cx: &mut App) {
        if Popups::is_open(cx) {
            return;
        }
        let front = AppWindowId::foreground();
        if let Some(history) = Self::shared(cx) {
            history.update(cx, |this, _| {
                this.aim = front.map(|window| (window, Instant::now()));
            });
        }
    }

    /// The window to paste into as the history opens: the one in front
    /// when it was asked for a moment ago, which is given once, or else
    /// the window of another program that was in front last, as when a
    /// `popup` key opened it.
    pub fn target(&mut self) -> Option<AppWindowId> {
        let noted = self.aim.take();
        let fresh = noted.filter(|(window, at)| at.elapsed() < AIM_FRESH && window.exists());
        let before = || self.watch.as_ref().and_then(ClipboardWatch::window_before);
        fresh.map(|(window, _)| window).or_else(before)
    }

    /// The items.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Whether what is copied is kept.
    pub fn is_on(&self) -> bool {
        self.watch.is_some()
    }

    fn apply(&mut self, config: ClipboardConfig, cx: &mut Context<Self>) {
        let size = config.size.clamp(1, MOST_KEPT);
        if size != config.size {
            report!("`size` of [clipboard] is from 1 to {MOST_KEPT}, {size} is used");
        }
        let was_on = self.config.enabled;
        self.config = ClipboardConfig { size, ..config };
        match (config.enabled, was_on) {
            (true, false) => self.watch_clipboard(cx),
            (false, true) => {
                self.taking = None;
                self.stop_watching(cx);
            }
            _ => {}
        }
        if let Some(watch) = &self.watch {
            watch.keep_images(config.images);
        }
        let dropped = self.history.trim(size);
        forget(dropped, cx);
        cx.notify();
    }

    /// Ends the watch and forgets what is not pinned.
    fn stop_watching(&mut self, cx: &mut Context<Self>) {
        self.watch = None;
        let dropped = self.history.clear();
        forget(dropped, cx);
    }

    /// Starts the watch of the clipboard and takes in what it reads. A
    /// watch that does not start, or ends by itself, leaves the history
    /// off, as the popup then says, until the configuration turns it on
    /// again.
    fn watch_clipboard(&mut self, cx: &mut Context<Self>) {
        let (watch, copied) = match ClipboardWatch::start(self.config.images) {
            Ok(started) => started,
            Err(error) => {
                report!("the clipboard is not watched: {error}");
                self.config.enabled = false;
                return;
            }
        };
        self.watch = Some(watch);
        let pictures = pictures_path();
        let taking = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(clip) = copied.recv().await {
                let pictures = pictures.clone();
                let kept = cx
                    .background_executor()
                    .spawn(async move { Kept::of(clip, pictures.as_deref()) })
                    .await;
                let kept = match kept {
                    Ok(kept) => kept,
                    Err(error) => {
                        report!("a copy is not kept in the history of the clipboard: {error}");
                        continue;
                    }
                };
                if this.update(cx, |this, cx| this.take(kept, cx)).is_err() {
                    return;
                }
            }
            // The thread of the watch ended, having reported why.
            this.update(cx, |this, cx| {
                this.config.enabled = false;
                this.stop_watching(cx);
                cx.notify();
            })
            .ok();
        });
        self.taking = Some(taking);
    }

    /// Adds what was copied.
    fn take(&mut self, kept: Kept, cx: &mut Context<Self>) {
        if self.watch.is_none() {
            forget(vec![kept.content], cx);
            return;
        }
        let pinned = self
            .history
            .entries()
            .iter()
            .any(|entry| entry.key == kept.key && entry.pinned);
        let dropped =
            { &mut self.history }.add(kept.key, kept.content, Local::now(), self.config.size);
        forget(dropped, cx);
        if pinned {
            self.save_pins(cx);
        }
        cx.notify();
    }

    /// Reads the pinned items of the earlier run, away from the user
    /// interface, and adds them.
    fn read_pins(cx: &mut Context<Self>) {
        let started = SystemTime::now();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let restored = { cx.background_executor() }
                .spawn(async move { restore(started) })
                .await;
            this.update(cx, |this, cx| {
                let Some(restored) = restored else {
                    return;
                };
                // A pin given before, or one read that the history held
                // already, whose picture file is let go, is to be written.
                let mut changed = this.pins_dirty;
                for (kept, copied) in restored {
                    let dropped = this.history.restore(kept.key, kept.content, copied);
                    changed |= !dropped.is_empty();
                    forget(dropped, cx);
                }
                this.restored = true;
                if changed {
                    this.save_pins(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Writes the pinned items, away from the user interface.
    fn save_pins(&mut self, cx: &mut Context<Self>) {
        if !self.restored {
            self.pins_dirty = true;
            return;
        }
        self.pin_writes += 1;
        let (pins, generation) = (self.history.pins(), self.pin_writes);
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = write_pins(&pins, generation) {
                    report!("the pinned items of the clipboard are not kept: {error}");
                }
            })
            .detach();
    }

    /// Puts the item `id` back on the clipboard and makes it the newest,
    /// then pastes it into `target`, if one is given and is still there.
    /// The popup is to be closed by the caller, which gives the keyboard
    /// back to that window.
    pub fn paste(&mut self, id: u64, target: Option<AppWindowId>, cx: &mut Context<Self>) {
        let Some(writer) = self.watch.as_ref().map(ClipboardWatch::writer) else {
            return;
        };
        let Some(entry) = self.history.get(id) else {
            return;
        };
        let (content, pinned) = (entry.content.clone(), entry.pinned);
        self.history.bring_forward(id, Local::now());
        if pinned {
            self.save_pins(cx);
        }
        cx.notify();
        // Reading a picture and waiting for the window to paste into take
        // a moment, done on a thread of its own.
        let started = thread::Builder::new()
            .name("clipboard paste".to_owned())
            .spawn(move || {
                let put = offer_of(&content).and_then(|offer| {
                    let answer = writer.put(offer).recv_blocking();
                    answer.unwrap_or_else(|_| Err("the clipboard did not answer".to_owned()))
                });
                let pasted = put.and_then(|()| target.map_or(Ok(()), paste_into));
                if let Err(error) = pasted {
                    report!("the item of the clipboard was not pasted: {error}");
                }
            });
        if let Err(error) = started {
            report!("the item of the clipboard was not pasted: {error}");
        }
    }

    /// Opens the address the item `id` is, with the program Windows opens
    /// such addresses with.
    pub fn open_link(&self, id: u64) {
        let link = self.history.get(id).and_then(|entry| entry.content.link());
        if let Some(link) = link {
            Launch::new(link).start();
        }
    }

    /// Pins the item `id`, or unpins it. An item unpinned in a full
    /// history stays in sight, so that a second click pins it again; it
    /// falls out with the next copy.
    pub fn toggle_pin(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.history.toggle_pin(id).is_some() {
            self.save_pins(cx);
            cx.notify();
        }
    }

    /// Takes the item `id` out of the history, pinned or not.
    pub fn remove(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(entry) = self.history.remove(id) {
            if entry.pinned {
                self.save_pins(cx);
            }
            forget(vec![entry.content], cx);
            cx.notify();
        }
    }

    /// Takes every item out that is not pinned.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        let dropped = self.history.clear();
        forget(dropped, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(history: &mut History, text: &str, size: usize) -> Vec<Content> {
        let content = Content::Text(text.to_owned());
        history.add(text_key(text), content, Local::now(), size)
    }

    fn texts(history: &History) -> Vec<String> {
        { history.entries().iter() }
            .map(|entry| match &entry.content {
                Content::Text(text) => text.clone(),
                _ => String::new(),
            })
            .collect()
    }

    #[test]
    fn the_newest_copy_comes_first_and_a_copy_again_moves_forward() {
        let mut history = History::default();
        for copied in ["one", "two", "three"] {
            assert!(text(&mut history, copied, 10).is_empty());
        }
        assert_eq!(texts(&history), ["three", "two", "one"]);
        let first = history.entries()[2].id;
        // The same text again is not added twice; it keeps its item.
        assert_eq!(text(&mut history, "one", 10).len(), 1);
        assert_eq!(texts(&history), ["one", "three", "two"]);
        assert_eq!(history.entries()[0].id, first);
    }

    #[test]
    fn the_oldest_unpinned_items_fall_out_and_pins_stay() {
        let mut history = History::default();
        text(&mut history, "a", 2);
        let pinned = history.entries()[0].id;
        assert_eq!(history.toggle_pin(pinned), Some(true));
        text(&mut history, "b", 2);
        text(&mut history, "c", 2);
        let dropped = text(&mut history, "d", 2);
        assert_eq!(dropped.len(), 1);
        assert!(matches!(&dropped[0], Content::Text(text) if text == "b"));
        assert_eq!(texts(&history), ["d", "c", "a"]);
        // Clearing keeps the pinned item, and a smaller size drops more.
        assert_eq!(history.trim(1).len(), 1);
        assert_eq!(texts(&history), ["d", "a"]);
        assert_eq!(history.clear().len(), 1);
        assert_eq!(texts(&history), ["a"]);
        assert!(history.remove(pinned).is_some());
        assert!(history.entries().is_empty());
    }

    #[test]
    fn pins_of_an_earlier_run_come_in_by_their_time() {
        let mut history = History::default();
        let at = |seconds| Local.timestamp_opt(seconds, 0).unwrap();
        history.add(text_key("new"), Content::Text("new".into()), at(300), 5);
        let old = Content::Text("old".into());
        assert!(history.restore(text_key("old"), old, at(100)).is_empty());
        let again = Content::Text("new".into());
        assert_eq!(history.restore(text_key("new"), again, at(200)).len(), 1);
        assert_eq!(texts(&history), ["new", "old"]);
        assert!(history.entries().iter().all(|entry| entry.pinned));
        let pins = history.pins();
        assert_eq!(pins.pins[1].text.as_deref(), Some("old"));
        assert_eq!(pins.pins[1].copied, 100);
    }

    #[test]
    fn the_pins_file_reads_what_it_wrote() {
        let mut history = History::default();
        let files = vec![PathBuf::from("C:\\Users\\me\\a.txt")];
        let key = files_key(&files);
        history.add(key, Content::Files(files), Local::now(), 5);
        text(&mut history, "line one\nline \"two\"", 5);
        for at in 0..2 {
            let id = history.entries()[at].id;
            history.toggle_pin(id);
        }
        let pins = history.pins();
        let written = toml::to_string(&pins).unwrap();
        assert_eq!(toml::from_str::<PinsFile>(&written).unwrap(), pins);
        assert!(toml::from_str::<PinsFile>("[[pin]]\ncolour = 1\n").is_err());
    }

    #[test]
    fn every_word_of_a_search_is_looked_for() {
        let mut history = History::default();
        text(&mut history, "The Quick brown fox", 5);
        text(&mut history, "a lazy dog", 5);
        let files = vec![PathBuf::from("D:\\Photos\\Fox.jpg")];
        history.add(files_key(&files), Content::Files(files), Local::now(), 5);
        assert_eq!(history.matching("FOX"), [0, 2]);
        assert_eq!(history.matching("quick fox"), [2]);
        assert_eq!(history.matching("  "), [0, 1, 2]);
        assert!(history.matching("cat").is_empty());
    }

    #[test]
    fn files_are_the_same_whatever_their_case() {
        let lower = files_key(&[PathBuf::from("c:\\a.txt")]);
        assert_eq!(lower, files_key(&[PathBuf::from("C:\\A.TXT")]));
        assert_ne!(lower, files_key(&[PathBuf::from("c:\\b.txt")]));
        assert_ne!(text_key("c:\\a.txt"), lower);
    }

    #[test]
    fn a_text_that_is_one_address_is_a_link() {
        let link = |text: &str| Content::Text(text.to_owned()).link().map(str::to_owned);
        assert_eq!(
            link(" https://example.org/a?b=c \n").as_deref(),
            Some("https://example.org/a?b=c")
        );
        assert_eq!(link("see https://example.org"), None);
        assert_eq!(link("https://"), None);
        assert_eq!(link("ftp://example.org"), None);
    }

    #[test]
    fn a_preview_is_one_line_and_short() {
        let long = "word ".repeat(200);
        let preview = Content::Text(format!("  first\n\tsecond  {long}")).preview();
        assert!(preview.starts_with("first second word"));
        assert_eq!(preview.chars().count(), PREVIEW_CHARS);
        let files = Content::Files(vec![PathBuf::from("C:\\x\\a.txt"), PathBuf::from("D:\\")]);
        assert_eq!(files.preview(), "a.txt, D:\\");
    }

    #[test]
    fn a_preview_of_one_long_word_or_many_files_is_cut() {
        let word = "\u{e9}".repeat(10_000);
        let preview = Content::Text(word).preview();
        assert_eq!(preview.chars().count(), PREVIEW_CHARS);
        let files = (0..5_000).map(|n| PathBuf::from(format!("C:\\f{n}.txt")));
        let preview = Content::Files(files.collect()).preview();
        assert!(preview.starts_with("f0.txt, f1.txt"));
        assert_eq!(preview.chars().count(), PREVIEW_CHARS);
        assert_eq!(shortened(["ab", "cd"], ", "), "ab, cd");
    }

    #[test]
    fn a_thumbnail_keeps_the_shape_of_its_picture() {
        assert_eq!(fitted((1920, 1080), (288, 112)), (199, 112));
        assert_eq!(fitted((100, 50), (288, 112)), (100, 50));
        assert_eq!(fitted((10_000, 1), (288, 112)), (288, 1));
        assert_eq!(fitted((0, 0), (288, 112)), (1, 1));
    }

    #[test]
    fn no_two_pictures_share_a_file() {
        assert_ne!(unique(), unique());
    }
}
