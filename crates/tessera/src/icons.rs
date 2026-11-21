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
use crate::platform::{IconPixels, Shortcut};

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
    /// else the one embedded in the file.
    pub fn of_executable(&self, executable: &Path) -> Icon {
        self.cached(executable, || {
            self.replacement(executable)
                .or_else(|| embedded(executable, 0))
        })
    }

    /// The icon to show for `shortcut`: the replacement configured for the
    /// file it opens, or else the icon it asks for, or else that file's.
    pub fn of_shortcut(&self, shortcut: &Shortcut) -> Icon {
        self.cached(&shortcut.path, || {
            let link = shortcut.link();
            let target = link.target.as_deref();
            target
                .and_then(|target| self.replacement(target))
                .or_else(|| link.icon.and_then(|(file, index)| embedded(&file, index)))
                .or_else(|| target.and_then(|target| embedded(target, 0)))
        })
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
            .inspect_err(|error| eprintln!("tessera: the icon for `{name}` is ignored: {error:#}"))
            .ok()
            .flatten()
    }
}

/// Reads an image file and scales it down to icon size.
fn load_image(path: &Path) -> anyhow::Result<Icon> {
    let mut image =
        image::open(path).with_context(|| format!("{} cannot be read", path.display()))?;
    if image.width() > SOURCE_SIZE || image.height() > SOURCE_SIZE {
        image = image.resize(SOURCE_SIZE, SOURCE_SIZE, FilterType::Triangle);
    }
    let mut pixels = image.into_rgba8();
    // The renderer expects blue first.
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    let (width, height) = pixels.dimensions();
    Ok(render_image(width, height, pixels.into_raw()))
}

/// The icon at `index` inside `file`.
fn embedded(file: &Path, index: i32) -> Icon {
    let pixels = IconPixels::of_file(file, index, SOURCE_SIZE)?;
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
