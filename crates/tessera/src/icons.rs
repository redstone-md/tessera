// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Application icons, with the replacements the user configured.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use gpui::{Global, RenderImage};
use image::imageops::FilterType;

use crate::config::{Config, config_dir};
use crate::icon_packs::{self, IconPacks, PackIcon};
use crate::platform::{IconPixels, Shortcut, app_id_in};

/// Pixel size icons are prepared at; large enough for 200% scaling.
const SOURCE_SIZE: u32 = 64;

/// An image ready to draw, or `None` when the application has none.
pub type Icon = Option<Arc<RenderImage>>;

/// What was found for a file: an image, or the icon of a pack, which is
/// drawn when asked for, in the colors of the theme in use then.
#[derive(Clone)]
enum Found {
    Image(Icon),
    Pack(PackIcon),
}

/// Finds and remembers the icon of each executable.
pub struct AppIcons {
    /// Image files by lowercase executable file name.
    replacements: HashMap<String, PathBuf>,
    /// The icon packs in use, below the replacements.
    packs: IconPacks,
    /// Icons by the file they were asked for, including files known to
    /// have none.
    cache: RefCell<HashMap<PathBuf, Found>>,
}

impl Global for AppIcons {}

impl AppIcons {
    /// `replacements` maps executable file names, in any case, to image
    /// files; relative paths are looked up in the configuration directory.
    #[cfg_attr(not(test), allow(dead_code, reason = "the shell uses from_config"))]
    pub fn new(replacements: &HashMap<String, PathBuf>) -> Self {
        Self::with_packs(replacements, IconPacks::none())
    }

    /// The icons the configuration asks for: the `icons` table first,
    /// then the packs `icon_packs` names, in its order, then those of the
    /// applications and files themselves.
    pub fn from_config(config: &Config) -> Self {
        Self::with_packs(&config.icons, IconPacks::load(&config.icon_packs))
    }

    fn with_packs(replacements: &HashMap<String, PathBuf>, packs: IconPacks) -> Self {
        let base = config_dir().unwrap_or_default();
        Self {
            replacements: replacements
                .iter()
                .map(|(name, image)| (name.to_lowercase(), base.join(image)))
                .collect(),
            packs,
            cache: RefCell::default(),
        }
    }

    /// The same icons with the packs read again from their files, after
    /// one of them changed.
    pub fn reloaded(&self) -> Self {
        Self {
            replacements: self.replacements.clone(),
            packs: IconPacks::load(self.packs.ids()),
            cache: RefCell::default(),
        }
    }

    /// The icon packs in use, for the icons of kinds of files and of the
    /// places of the desktop.
    pub fn packs(&self) -> &IconPacks {
        &self.packs
    }

    /// The icon to show for `executable`: the configured replacement, or
    /// else the one of the first icon pack that has one, or else the one
    /// embedded in the file. The path that names a packaged application
    /// has the icon the shell draws for it unless a pack has one for its
    /// identifier.
    pub fn of_executable(&self, executable: &Path) -> Icon {
        self.of_executable_as(executable, false)
    }

    /// The icon of `executable` as [`Self::of_executable`] finds it; one
    /// of a pack that follows the theme is drawn in its primary color when
    /// `active`, as for an application that runs.
    pub fn of_executable_as(&self, executable: &Path, active: bool) -> Icon {
        self.cached(executable, active, || {
            if let Some(app) = app_id_in(executable) {
                return self
                    .pack_app(&[app])
                    .unwrap_or_else(|| Found::Image(of_app(app)));
            }
            if let Some(icon) = self.replacement(executable) {
                return Found::Image(Some(icon));
            }
            let names: Vec<&str> = file_name(executable).into_iter().collect();
            self.pack_app(&names)
                .unwrap_or_else(|| Found::Image(embedded(executable, 0)))
        })
    }

    /// The icon to show for `shortcut`: the replacement configured for the
    /// file it opens, or else that of the first icon pack that has one for
    /// that file, the name of the shortcut or its file, or else the icon
    /// it asks for, or else that file's. A shortcut that is a program file
    /// itself opens that file.
    pub fn of_shortcut(&self, shortcut: &Shortcut) -> Icon {
        self.of_shortcut_as(shortcut, false)
    }

    /// The icon of `shortcut` as [`Self::of_shortcut`] finds it, drawn as
    /// [`Self::of_executable_as`] says for `active`.
    pub fn of_shortcut_as(&self, shortcut: &Shortcut, active: bool) -> Icon {
        if app_id_in(&shortcut.path).is_some() {
            return self.of_executable_as(&shortcut.path, active);
        }
        self.cached(&shortcut.path, active, || {
            let link = shortcut.link();
            let own = (!is_link(&shortcut.path)).then_some(shortcut.path.as_path());
            let target = link.target.as_deref().or(own);
            if let Some(icon) = target.and_then(|target| self.replacement(target)) {
                return Found::Image(Some(icon));
            }
            let stem = shortcut.path.file_stem().and_then(|stem| stem.to_str());
            let names = [
                target.and_then(file_name),
                Some(shortcut.name.as_str()),
                stem,
            ];
            let names: Vec<&str> = names.into_iter().flatten().collect();
            if let Some(found) = self.pack_app(&names) {
                return found;
            }
            let icon = { link.icon }
                .and_then(|(file, index)| embedded(&file, index))
                .or_else(|| target.and_then(|target| embedded(target, 0)));
            Found::Image(icon)
        })
    }

    /// The icon the shell draws for the file or folder at `path`, unless
    /// an icon pack has one for its kind (its extension, `folder` or
    /// `drive`). Files of a kind share one, but for those that carry
    /// their own.
    pub fn of_file(&self, path: &Path) -> Icon {
        self.cached(&kind_of(path), false, || {
            if let Some(found) = pack_kind(path).and_then(|kind| self.pack_file(&kind)) {
                return found;
            }
            let icon = IconPixels::of_item(path, SOURCE_SIZE)
                .and_then(|pixels| render_image(pixels.width, pixels.height, pixels.bgra));
            Found::Image(icon)
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

    /// The icon remembered for `file`, found with `find` the first time;
    /// that of a pack drawn for `active`.
    fn cached(&self, file: &Path, active: bool, find: impl FnOnce() -> Found) -> Icon {
        let known = self.cache.borrow().get(file).cloned();
        let found = known.unwrap_or_else(|| {
            let found = find();
            { self.cache.borrow_mut() }.insert(file.to_owned(), found.clone());
            found
        });
        match found {
            Found::Image(icon) => icon,
            Found::Pack(icon) => self.packs.image(&icon, SOURCE_SIZE, active),
        }
    }

    /// The icon the first pack that has one gives for an application
    /// known by any of `names`, if its picture can be drawn; one that
    /// cannot was reported and leaves the application its own.
    fn pack_app(&self, names: &[&str]) -> Option<Found> {
        let icon = self.packs.app(names)?;
        self.packs.image(icon, SOURCE_SIZE, false)?;
        Some(Found::Pack(icon.clone()))
    }

    /// The same for a kind of file.
    fn pack_file(&self, kind: &str) -> Option<Found> {
        let icon = self.packs.file(kind)?;
        self.packs.image(icon, SOURCE_SIZE, false)?;
        Some(Found::Pack(icon.clone()))
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

/// The file name of `path`, as text.
fn file_name(path: &Path) -> Option<&str> {
    path.file_name()?.to_str()
}

/// What an icon pack knows the kind of the file at `path` under: `drive`
/// for the root of a drive, `folder` for a folder, or else its extension.
fn pack_kind(path: &Path) -> Option<String> {
    let first = path.components().next();
    if path.parent().is_none() && matches!(first, Some(Component::Prefix(_))) {
        return Some(icon_packs::DRIVE.to_owned());
    }
    if path.is_dir() {
        return Some(icon_packs::FOLDER.to_owned());
    }
    Some(path.extension()?.to_str()?.to_lowercase())
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
pub(crate) fn prepared(mut image: image::DynamicImage, size: u32) -> Icon {
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
        assert!(icons.packs().ids().is_empty());
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
