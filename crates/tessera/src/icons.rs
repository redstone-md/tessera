// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Application icons, with the replacements the user configured.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use gpui::{Global, RenderImage};
use image::imageops::FilterType;

use crate::config::config_dir;
use crate::platform::{IconPixels, Shortcut, app_id_in};

/// Pixel size icons are prepared at; large enough for 200% scaling.
const SOURCE_SIZE: u32 = 64;

/// An image ready to draw, or `None` when the application has none.
pub type Icon = Option<Arc<RenderImage>>;

/// Finds and remembers the icon of each executable.
pub struct AppIcons {
    /// Image files by lowercase executable file name.
    replacements: HashMap<String, PathBuf>,
    /// Icons by the file they were asked for, including files known to
    /// have none.
    cache: RefCell<HashMap<PathBuf, Icon>>,
}

impl Global for AppIcons {}

impl AppIcons {
    /// `replacements` maps executable file names, in any case, to image
    /// files; relative paths are looked up in the configuration directory.
    pub fn new(replacements: &HashMap<String, PathBuf>) -> Self {
        let base = config_dir().unwrap_or_default();
        Self {
            replacements: replacements
                .iter()
                .map(|(name, image)| (name.to_lowercase(), base.join(image)))
                .collect(),
            cache: RefCell::default(),
        }
    }

    /// The icon to show for `executable`: the configured replacement, or
    /// else the one embedded in the file. The path that names a packaged
    /// application has the icon the shell draws for it.
    pub fn of_executable(&self, executable: &Path) -> Icon {
        self.cached(executable, || match app_id_in(executable) {
            Some(app) => of_app(app),
            None => self
                .replacement(executable)
                .or_else(|| embedded(executable, 0)),
        })
    }

    /// The icon to show for `shortcut`: the replacement configured for the
    /// file it opens, or else the icon it asks for, or else that file's.
    /// A shortcut that is a program file itself opens that file.
    pub fn of_shortcut(&self, shortcut: &Shortcut) -> Icon {
        if app_id_in(&shortcut.path).is_some() {
            return self.of_executable(&shortcut.path);
        }
        self.cached(&shortcut.path, || {
            let link = shortcut.link();
            let own = (!is_link(&shortcut.path)).then_some(shortcut.path.as_path());
            let target = link.target.as_deref().or(own);
            target
                .and_then(|target| self.replacement(target))
                .or_else(|| link.icon.and_then(|(file, index)| embedded(&file, index)))
                .or_else(|| target.and_then(|target| embedded(target, 0)))
        })
    }

    /// The icon the shell draws for the file or folder at `path`. Files
    /// of a kind share one, but for those that carry their own.
    pub fn of_file(&self, path: &Path) -> Icon {
        self.cached(&kind_of(path), || {
            let pixels = IconPixels::of_item(path, SOURCE_SIZE)?;
            render_image(pixels.width, pixels.height, pixels.bgra)
        })
    }

    /// Whether the icon of the shortcut or program at `file` was looked
    /// for already, so that asking for it costs nothing.
    pub fn has(&self, file: &Path) -> bool {
        self.cache.borrow().contains_key(file)
    }

    /// The same for the icon the shell draws for the file at `path`.
    pub fn has_of_file(&self, path: &Path) -> bool {
        self.has(&kind_of(path))
    }

    /// The icon remembered for `file`, found with `find` the first time.
    fn cached(&self, file: &Path, find: impl FnOnce() -> Icon) -> Icon {
        if let Some(icon) = self.cache.borrow().get(file) {
            return icon.clone();
        }
        let icon = find();
        self.cache
            .borrow_mut()
            .insert(file.to_owned(), icon.clone());
        icon
    }

    /// The image configured for `executable`. One that cannot be used is
    /// reported and skipped.
    fn replacement(&self, executable: &Path) -> Icon {
        let name = executable.file_name()?.to_str()?.to_lowercase();
        let path = self.replacements.get(&name)?;
        load_image(path)
            .inspect_err(|error| report!("the icon for `{name}` is ignored: {error:#}"))
            .ok()
            .flatten()
    }
}

/// Reads an image file and scales it down to icon size.
fn load_image(path: &Path) -> anyhow::Result<Icon> {
    let image = image::open(path).with_context(|| format!("{} cannot be read", path.display()))?;
    Ok(prepared(image, SOURCE_SIZE))
}

/// Decodes the image file `bytes` hold and scales it down to fit a square
/// of `size` pixels.
pub fn image_of_bytes(bytes: &[u8], size: u32) -> anyhow::Result<Icon> {
    Ok(prepared(image::load_from_memory(bytes)?, size))
}

/// `image` ready to draw, scaled down to fit a square of `size` pixels.
fn prepared(mut image: image::DynamicImage, size: u32) -> Icon {
    if image.width() > size || image.height() > size {
        image = image.resize(size, size, FilterType::Triangle);
    }
    let mut pixels = image.into_rgba8();
    // The renderer expects blue first.
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    let (width, height) = pixels.dimensions();
    render_image(width, height, pixels.into_raw())
}

/// An image of `pixels`, ready to draw.
pub fn icon_of_pixels(pixels: &IconPixels) -> Icon {
    render_image(pixels.width, pixels.height, pixels.bgra.clone())
}

/// Whether `path` names a shortcut file, which points at what it opens.
pub fn is_link(path: &Path) -> bool {
    let extension = path.extension();
    extension.is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
}

/// The icon at `index` inside `file`.
fn embedded(file: &Path, index: i32) -> Icon {
    let pixels = IconPixels::of_file(file, index, SOURCE_SIZE)?;
    render_image(pixels.width, pixels.height, pixels.bgra)
}

/// The kinds of files that each have an icon of their own.
const OWN_ICONS: [&str; 5] = ["exe", "lnk", "ico", "url", "scr"];

/// What the icon of the file at `path` is remembered under: the file for
/// a folder and for a kind with icons of its own, or else its kind alone.
fn kind_of(path: &Path) -> PathBuf {
    let kind = path.extension().and_then(|kind| kind.to_str());
    match kind.map(str::to_lowercase) {
        Some(kind) if !OWN_ICONS.contains(&kind.as_str()) && !path.is_dir() => {
            PathBuf::from(format!("*.{kind}"))
        }
        _ => path.to_owned(),
    }
}

/// The icon of the packaged application `id`.
fn of_app(id: &str) -> Icon {
    let pixels = IconPixels::of_app(id, SOURCE_SIZE)?;
    render_image(pixels.width, pixels.height, pixels.bgra)
}

fn render_image(width: u32, height: u32, bgra: Vec<u8>) -> Icon {
    let buffer = image::RgbaImage::from_raw(width, height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_of_a_kind_share_an_icon() {
        assert_eq!(kind_of(Path::new("C:/a/One.TXT")), PathBuf::from("*.txt"));
        assert_eq!(kind_of(Path::new("C:/b/two.txt")), PathBuf::from("*.txt"));
        assert_eq!(
            kind_of(Path::new("C:/a/tool.exe")),
            PathBuf::from("C:/a/tool.exe")
        );
        assert_eq!(
            kind_of(Path::new("C:/a/plain")),
            PathBuf::from("C:/a/plain")
        );
    }

    #[test]
    fn a_replacement_is_found_by_file_name_in_any_case() {
        let directory = std::env::temp_dir().join(format!("tessera-icons-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("icon.png");
        image::RgbaImage::from_pixel(200, 100, image::Rgba([10, 20, 30, 255]))
            .save(&file)
            .unwrap();

        let icons = AppIcons::new(&HashMap::from([("Editor.EXE".to_owned(), file)]));
        let icon = icons
            .replacement(Path::new(r"C:\Apps\editor.exe"))
            .expect("the replacement loads");
        std::fs::remove_dir_all(&directory).unwrap();

        // Scaled into the icon size, keeping its shape, with blue first.
        let size = icon.size(0);
        assert_eq!((size.width.0, size.height.0), (64, 32));
        assert_eq!(icon.as_bytes(0).unwrap()[..4], [30, 20, 10, 255]);
        assert!(icons.replacement(Path::new(r"C:\Apps\other.exe")).is_none());
    }
}
