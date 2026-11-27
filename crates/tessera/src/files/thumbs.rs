// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Thumbnails of what files hold, for the larger looks of the file
//! manager, and how `[file_view]` of the configuration sets them and the
//! panes beside the list.
//!
//! Each window keeps the thumbnails it drew last, by path, size and the
//! time the file was changed, up to 64 MB of pixels; the one not drawn
//! longest goes first. A thumbnail is asked of the Shell the first time
//! a row in sight wants it ([`Thumbs::picture`]); the same request is not
//! made twice while it runs, and a request for a row that scrolled out of
//! sight before it was done is called off at the next frame
//! ([`Thumbs::sweep`]). Until a thumbnail comes, and where a file has
//! none, its icon is drawn.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use gpui::{App, AsyncApp, Context, Global, RenderImage, Task, WeakEntity};
use serde::Deserialize;

use super::model::Entry;
use crate::platform::{IconPixels, LARGEST_THUMBNAIL, ThumbnailKind, Thumbnails};

/// The most bytes of pixels a window keeps in thumbnails.
const MOST_BYTES: usize = 64 * 1024 * 1024;

/// The sizes thumbnails are asked for at, in physical pixels; a picture
/// is asked for at the first of them that is at least as large as it is
/// drawn, so that a few sizes serve every zoom.
const SIZES: [u32; 7] = [32, 48, 64, 96, 128, 192, 256];

/// The smallest icon, in logical pixels, that is drawn as a thumbnail;
/// smaller ones stay icons, as in Explorer.
pub const SMALLEST_THUMBNAIL: f32 = 32.;

/// What `[file_view]` of the configuration says: thumbnails, overlays and
/// the panes beside the list of the file manager.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FileViewConfig {
    /// Whether files are drawn with a picture of what they hold where
    /// their icons are large enough, as Explorer draws photos.
    pub thumbnails: bool,
    /// Whether folders are drawn with a picture of what they hold too.
    pub folder_thumbnails: bool,
    /// The largest side a thumbnail is asked for at, in physical pixels,
    /// from 32 to 256.
    pub thumbnail_size: u32,
    /// Whether the overlays that programs register (OneDrive, Git
    /// clients) are drawn over icons.
    pub overlays: bool,
    /// Whether a window opens with the preview pane (Alt+P) shown.
    pub preview: bool,
    /// Whether a window opens with the details pane (Alt+Shift+P) shown.
    pub details: bool,
    /// Whether the preview handlers of programs draw the kinds of files
    /// Tessera does not show itself (PDF, documents of Office).
    pub preview_handlers: bool,
    /// How long a preview handler is waited for, in seconds, before it is
    /// given up.
    pub preview_timeout: f32,
}

impl Default for FileViewConfig {
    fn default() -> Self {
        Self {
            thumbnails: true,
            folder_thumbnails: true,
            thumbnail_size: LARGEST_THUMBNAIL,
            overlays: true,
            preview: false,
            details: false,
            preview_handlers: true,
            preview_timeout: 5.,
        }
    }
}

/// How the file manager is configured to show files, for its windows to
/// read.
#[derive(Default)]
struct FileView(FileViewConfig);

impl Global for FileView {}

impl FileViewConfig {
    /// Takes this as what the file manager shows from now on; windows
    /// that are open take it the next time they draw.
    pub fn configure(&self, cx: &mut App) {
        cx.set_global(FileView(*self));
    }

    /// What the file manager is configured to show.
    pub fn of(cx: &App) -> Self {
        cx.try_global::<FileView>()
            .map(|view| view.0)
            .unwrap_or_default()
    }

    /// The largest side a thumbnail is asked for at, within what the
    /// Shell gives.
    fn largest(&self) -> u32 {
        self.thumbnail_size.clamp(SIZES[0], LARGEST_THUMBNAIL)
    }
}

/// What a thumbnail is kept under: the file, the size it was asked for
/// at, and when the file was changed, so that a changed file is pictured
/// anew.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    size: u32,
    modified: Option<SystemTime>,
    kind: ThumbnailKind,
}

/// A thumbnail kept, or what was learned that there is none.
struct Kept {
    image: Option<Arc<RenderImage>>,
    bytes: usize,
    /// When it was drawn last, counted in pictures given out.
    used: u64,
}

/// A request that runs, and the frame it was last wanted in.
struct Asked {
    _task: Task<()>,
    frame: u64,
}

/// What [`Thumbs::picture`] has for a file.
#[derive(Clone, Debug)]
pub enum Picture {
    /// Its thumbnail.
    Drawn(Arc<RenderImage>),
    /// None, or none yet: its icon is drawn.
    Icon,
}

/// The thumbnails of a window, shared by its panes, which observe it and
/// draw again when one comes.
#[derive(Default)]
pub struct Thumbs {
    kept: HashMap<Key, Kept>,
    bytes: usize,
    clock: u64,
    asked: HashMap<Key, Asked>,
    /// The frame being drawn, counted by [`Self::sweep`].
    frame: u64,
}

impl Thumbs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Calls off the requests of rows that were not drawn in the frame
    /// before; called once as a window starts drawing a frame.
    pub fn sweep(&mut self) {
        let frame = self.frame;
        self.asked.retain(|_, asked| asked.frame >= frame);
        self.frame += 1;
    }

    /// The thumbnail of `entry` for an icon `side` logical pixels wide at
    /// `scale`, asked of the Shell where it was not yet; `priority` is the
    /// place of its row among those in sight, the first done first.
    pub fn picture(
        &mut self,
        entry: &Entry,
        side: f32,
        scale: f32,
        priority: u32,
        cx: &mut Context<Self>,
    ) -> Picture {
        self.picture_of(entry, side, scale, priority, ThumbnailKind::Picture, cx)
    }

    /// The thumbnail of `entry` as [`Self::picture`] gives it, or where it
    /// has none, the icon of its kind as the Shell draws it at that size,
    /// as a pane draws one large picture of a file.
    pub fn picture_or_icon(
        &mut self,
        entry: &Entry,
        side: f32,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> Picture {
        self.picture_of(entry, side, scale, 0, ThumbnailKind::PictureOrIcon, cx)
    }

    fn picture_of(
        &mut self,
        entry: &Entry,
        side: f32,
        scale: f32,
        priority: u32,
        kind: ThumbnailKind,
        cx: &mut Context<Self>,
    ) -> Picture {
        let config = FileViewConfig::of(cx);
        let wanted = config.thumbnails && (!entry.folder || config.folder_thumbnails);
        if !wanted || side < SMALLEST_THUMBNAIL {
            return Picture::Icon;
        }
        let key = Key {
            path: entry.path.clone(),
            size: size_for(side * scale, config.largest()),
            modified: entry.modified,
            kind,
        };
        self.clock += 1;
        if let Some(kept) = self.kept.get_mut(&key) {
            kept.used = self.clock;
            return kept.image.clone().map_or(Picture::Icon, Picture::Drawn);
        }
        let frame = self.frame;
        if let Some(asked) = self.asked.get_mut(&key) {
            asked.frame = frame;
            return Picture::Icon;
        }
        let answer = Thumbnails::request(key.path.clone(), key.size, kind, priority);
        let asked = key.clone();
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let Ok(pixels) = answer.recv().await else {
                return;
            };
            this.update(cx, |this, cx| this.take(asked, pixels, cx))
                .ok();
        });
        self.asked.insert(key, Asked { _task: task, frame });
        Picture::Icon
    }

    /// Keeps the thumbnail that came for `key`, and has the panes draw it.
    fn take(&mut self, key: Key, pixels: Option<IconPixels>, cx: &mut Context<Self>) {
        self.asked.remove(&key);
        let image = pixels.and_then(|pixels| crate::icons::icon_of_pixels(&pixels));
        let bytes = { image.as_ref() }.map_or(0, |image| {
            let size = image.size(0);
            (size.width.0 as usize) * (size.height.0 as usize) * 4
        });
        self.clock += 1;
        self.bytes += bytes;
        let drawn = image.is_some();
        self.kept.insert(
            key,
            Kept {
                image,
                bytes,
                used: self.clock,
            },
        );
        self.evict(cx);
        if drawn {
            cx.notify();
        }
    }

    /// Lets go of the thumbnails drawn longest ago while they take more
    /// than a window keeps, and of their textures.
    fn evict(&mut self, cx: &mut Context<Self>) {
        while self.bytes > MOST_BYTES {
            let oldest = { self.kept.iter() }
                .min_by_key(|(_, kept)| kept.used)
                .map(|(key, _)| key.clone());
            let Some(kept) = oldest.and_then(|key| self.kept.remove(&key)) else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(kept.bytes);
            if let Some(image) = kept.image {
                cx.drop_image(image, None);
            }
        }
    }

    /// Forgets what was learned of the files at `paths`, as after they
    /// changed, so that they are pictured anew.
    pub fn forget(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let gone: Vec<Key> = { self.kept.keys() }
            .filter(|key| paths.contains(&key.path))
            .cloned()
            .collect();
        for key in gone {
            if let Some(kept) = self.kept.remove(&key) {
                self.bytes = self.bytes.saturating_sub(kept.bytes);
                if let Some(image) = kept.image {
                    cx.drop_image(image, None);
                }
            }
        }
    }
}

/// The size a thumbnail drawn `pixels` physical pixels wide is asked for
/// at: the first of the sizes that is as large, at most `largest`.
fn size_for(pixels: f32, largest: u32) -> u32 {
    let wanted = pixels.ceil().max(0.) as u32;
    { SIZES.into_iter() }
        .find(|size| *size >= wanted)
        .unwrap_or(LARGEST_THUMBNAIL)
        .min(largest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thumbnail_is_asked_for_at_the_next_size_up_within_the_largest() {
        assert_eq!(size_for(40., 256), 48);
        assert_eq!(size_for(48., 256), 48);
        assert_eq!(size_for(128. * 1.5, 256), 192);
        assert_eq!(size_for(512., 256), 256);
        assert_eq!(size_for(200., 128), 128);
    }

    #[test]
    fn the_configuration_reads_its_table() {
        let config: FileViewConfig =
            toml::from_str("thumbnails = false\nthumbnail_size = 96\npreview = true\n")
                .expect("the table can be read");
        assert!(!config.thumbnails && config.preview);
        assert_eq!(config.largest(), 96);
        assert!(toml::from_str::<FileViewConfig>("thumbs = true").is_err());
    }
}
