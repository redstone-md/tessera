// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The pictures of the image nodes of a widget: decoded and fetched off
//! the thread that draws, and kept for as long as the widget shows them.
//!
//! What an image may be was checked when the extension described it
//! ([`Content::checked`](super::nodes::Content::checked)): a PNG or JPEG
//! of at most 2 MiB, an address on a permitted host, or an application.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, AsyncApp, Context, RenderImage, WeakEntity};

use super::http;
use super::nodes::{Content, IMAGE_LIMIT, picture_kind};
use super::runtime::{ImageSource, NodeKind};
use super::widget::ExtensionWidget;
use crate::icons::{AppIcons, image_of_bytes};
use crate::platform::app_path;

/// The most pixels a picture is decoded to, in width and height: enough
/// for the highest image at twice the scale.
const DECODED: u32 = 1024;

/// The most pixels a picture may say it has in width and in height. Two
/// megabytes of one color can describe a picture of eleven thousand
/// pixels square, half a gigabyte once decoded; this keeps one at 64 MiB.
const PICTURE_SIDE: u32 = 4096;

/// What tells the picture of an image node apart.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PictureKey {
    /// A picture of bytes, by their digest.
    Bytes(u64),
    Url(String),
    App(String),
}

impl PictureKey {
    /// The key of the picture of the node at `index` of `content`, if it
    /// is an image that is shown.
    pub fn of(content: &Content, index: usize) -> Option<Self> {
        let NodeKind::Image(image) = &content.nodes.get(index)?.kind else {
            return None;
        };
        if !content.shows(index) {
            return None;
        }
        Some(match &image.source {
            ImageSource::Bytes(_) => Self::Bytes(*content.digests.get(&index)?),
            ImageSource::Url(url) => Self::Url(url.clone()),
            ImageSource::App(app) => Self::App(app.clone()),
        })
    }
}

/// How far a picture is.
#[derive(Clone)]
enum Picture {
    Loading,
    Ready(Arc<RenderImage>),
    /// It cannot be had; it is not asked for again while it is shown.
    Missing,
}

/// The pictures of one widget.
#[derive(Default)]
pub struct Pictures {
    pictures: HashMap<PictureKey, Picture>,
}

impl Pictures {
    /// The picture of the node at `index` of `content` if it is ready;
    /// one not asked for yet is started.
    pub fn get(
        &mut self,
        content: &Content,
        index: usize,
        cx: &mut Context<ExtensionWidget>,
    ) -> Option<Arc<RenderImage>> {
        let key = PictureKey::of(content, index)?;
        match self.pictures.get(&key) {
            Some(Picture::Ready(image)) => return Some(image.clone()),
            Some(Picture::Loading | Picture::Missing) => return None,
            None => {}
        }
        if let PictureKey::App(app) = &key {
            // The icons of applications are kept by the shell for all.
            let icon = icon_of_app(app, cx);
            let picture = icon.clone().map_or(Picture::Missing, Picture::Ready);
            self.pictures.insert(key, picture);
            return icon;
        }
        let source = match &content.nodes[index].kind {
            NodeKind::Image(image) => image.source.clone(),
            _ => return None,
        };
        self.pictures.insert(key.clone(), Picture::Loading);
        cx.spawn(
            async move |this: WeakEntity<ExtensionWidget>, cx: &mut AsyncApp| {
                let loaded = cx
                    .background_executor()
                    .spawn(async move { load(&source) })
                    .await;
                let picture = match loaded {
                    Ok(image) => Picture::Ready(image),
                    Err(error) => {
                        report!("a picture of an extension is not shown: {error}");
                        Picture::Missing
                    }
                };
                this.update(cx, |this, cx| {
                    this.pictures().settle(key, picture);
                    cx.notify();
                })
                .ok();
            },
        )
        .detach();
        None
    }

    /// Puts in what a load came to, unless the picture was let go
    /// meanwhile.
    fn settle(&mut self, key: PictureKey, picture: Picture) {
        if let Some(slot) = self.pictures.get_mut(&key) {
            *slot = picture;
        }
    }

    /// Lets go of the pictures `content` does not show.
    pub fn keep_only(&mut self, content: &Content) {
        let shown: Vec<PictureKey> = (0..content.nodes.len())
            .filter_map(|index| PictureKey::of(content, index))
            .collect();
        self.pictures.retain(|key, _| shown.contains(key));
    }
}

/// Decodes or fetches and decodes the picture `source` names. It reads
/// the network: not for the thread that draws.
fn load(source: &ImageSource) -> Result<Arc<RenderImage>, String> {
    let fetched;
    let bytes = match source {
        ImageSource::Bytes(bytes) => bytes.as_slice(),
        ImageSource::Url(url) => {
            fetched = http::fetch_bytes(url, IMAGE_LIMIT as u64)?;
            if picture_kind(&fetched).is_none() {
                return Err(format!("{url} is neither a PNG nor a JPEG file"));
            }
            fetched.as_slice()
        }
        ImageSource::App(app) => return Err(format!("{app} is an application")),
    };
    check_size(bytes)?;
    image_of_bytes(bytes, DECODED)
        .map_err(|error| format!("the picture cannot be read: {error:#}"))?
        .ok_or_else(|| "the picture is empty".to_owned())
}

/// Fails unless the picture of `bytes` says it is at most
/// [`PICTURE_SIDE`] pixels in width and height. Only its header is read.
fn check_size(bytes: &[u8]) -> Result<(), String> {
    let (width, height) = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map(image::ImageReader::into_dimensions)
        .map_err(|error| format!("the picture cannot be read: {error}"))?
        .map_err(|error| format!("the picture cannot be read: {error}"))?;
    match width <= PICTURE_SIDE && height <= PICTURE_SIDE {
        true => Ok(()),
        false => Err(format!(
            "a picture of {width} by {height} pixels is larger than {PICTURE_SIDE} by {PICTURE_SIDE}"
        )),
    }
}

/// The icon of the application `app`: a program file or the
/// AppUserModelID of a packaged application.
fn icon_of_app(app: &str, cx: &App) -> Option<Arc<RenderImage>> {
    let path = match app.contains('!') {
        true => app_path(app),
        false => PathBuf::from(app),
    };
    cx.global::<AppIcons>().of_executable(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::new(width, height)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn pictures_are_at_most_the_picture_side_in_each_direction() {
        assert!(check_size(&png(PICTURE_SIDE, 1)).is_ok());
        assert!(check_size(&png(1, PICTURE_SIDE)).is_ok());
        assert!(check_size(&png(PICTURE_SIDE + 1, 1)).is_err());
        assert!(check_size(&png(1, PICTURE_SIDE + 1)).is_err());
        assert!(check_size(b"\x89PNG\r\n\x1a\nrest").is_err());
    }
}
