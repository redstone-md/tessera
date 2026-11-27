// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Icon packs: directories of pictures that take the place of the icons
//! of applications and of kinds of files. Each pack is a directory of
//! `%APPDATA%\tessera\icon-packs\` with an `icon-pack.toml` that says which
//! picture stands for what; `icon_packs` in the configuration names the
//! packs in use, and the first of them that has an icon for something
//! gives it. docs/ICON-PACKS.md describes the format.
//!
//! An icon can follow the colors of the theme: one marked `monochrome`
//! is drawn in the color of text, its own colors left out and only its
//! shape kept; one with a `mask` gets the parts its mask covers painted
//! in that color over its own picture. Either is drawn in the primary
//! color instead where it stands for something active.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context as _, bail};
use gpui::{App, AsyncApp, Hsla, ImageFormat, RenderImage};
use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};
use serde::Deserialize;

use crate::config::config_dir;
use crate::extension::registry::is_plain_id;
use crate::icons::{AppIcons, Icon};
use crate::shell::Shell;
use crate::theme::Theme;

/// The file of a pack that describes it.
pub const MANIFEST_FILE: &str = "icon-pack.toml";

/// The directory of the packs within that of the configuration.
const PACKS_DIRECTORY: &str = "icon-packs";

/// The kinds of things `[files]` names besides extensions of files.
pub const FOLDER: &str = "folder";
pub const DRIVE: &str = "drive";
#[allow(dead_code, reason = "for the icons of the desktop")]
pub const RECYCLE_BIN_FULL: &str = "recycle-bin-full";
#[allow(dead_code, reason = "for the icons of the desktop")]
pub const RECYCLE_BIN_EMPTY: &str = "recycle-bin-empty";

/// What a mask is called that lies next to its icon without being named:
/// the name of the icon with this before its extension.
const MASK_SUFFIX: &str = "-mask";

/// The kinds of picture files a pack may hold.
const PICTURE_KINDS: [&str; 5] = ["png", "ico", "svg", "jpg", "jpeg"];

/// The largest picture file of a pack that is read.
const PICTURE_LIMIT: u64 = 4 << 20;

/// How often the files of the packs in use are looked at for changes. A
/// change is taken once the files stayed the same for one interval.
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// The directory installed icon packs are kept in.
pub fn icon_packs_dir() -> Option<PathBuf> {
    config_dir().map(|directory| directory.join(PACKS_DIRECTORY))
}

/// An `icon-pack.toml` as it is written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    dead_code,
    reason = "the name and the rest are those of the format; a registry gives what is shown"
)]
pub struct PackManifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub author: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Icons of applications, by the file name of the program, the
    /// identifier of a packaged application or the name of a shortcut.
    #[serde(default)]
    apps: HashMap<String, EntryText>,
    /// Icons of kinds of files, by extension, or of `folder`, `drive`,
    /// `recycle-bin-full` and `recycle-bin-empty`.
    #[serde(default)]
    files: HashMap<String, EntryText>,
    /// Kept for icons of the shell itself; nothing reads it yet.
    #[serde(default)]
    #[allow(dead_code, reason = "reserved for icons of the shell")]
    shell: toml::Table,
}

/// An icon of a pack: the name of its file alone, or a table.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum EntryText {
    File(String),
    Full(EntryTable),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryTable {
    icon: String,
    /// A picture whose covered parts are painted in a color of the theme
    /// over the icon.
    mask: Option<String>,
    /// Whether the icon is drawn in a color of the theme, by its shape.
    #[serde(default)]
    monochrome: bool,
}

impl PackManifest {
    /// The manifest written as `text`, if it is usable: a plain id and
    /// pictures named by plain paths within the pack.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let manifest: Self = toml::from_str(text)?;
        if !is_plain_id(&manifest.id) {
            bail!("`{}` is not a name made of a-z, 0-9 and `-`", manifest.id);
        }
        let entries = manifest.apps.values().chain(manifest.files.values());
        for entry in entries {
            let (icon, mask) = match entry {
                EntryText::File(icon) => (icon, None),
                EntryText::Full(table) => (&table.icon, table.mask.as_ref()),
            };
            for file in std::iter::once(icon).chain(mask) {
                check_picture_name(file)?;
            }
        }
        Ok(manifest)
    }

    /// Every file the manifest names, relative to the pack.
    pub fn files(&self) -> Vec<String> {
        let entries = self.apps.values().chain(self.files.values());
        let mut files: Vec<String> = entries
            .flat_map(|entry| match entry {
                EntryText::File(icon) => vec![icon.clone()],
                EntryText::Full(table) => std::iter::once(table.icon.clone())
                    .chain(table.mask.clone())
                    .collect(),
            })
            .collect();
        files.sort();
        files.dedup();
        files
    }
}

/// Whether `name` is a path a pack may name: parts separated by `/`, none
/// of them empty, `.` or `..`, and a picture file at the end.
pub fn check_picture_name(name: &str) -> anyhow::Result<()> {
    let plain =
        |part: &str| !matches!(part, "" | "." | "..") && !part.contains([':', '\\', '?', '#', '*']);
    if !name.split('/').all(plain) {
        bail!("`{name}` is not a path within the pack");
    }
    let kind = Path::new(name).extension().and_then(|kind| kind.to_str());
    let kind = kind.map(str::to_ascii_lowercase);
    if !kind.is_some_and(|kind| PICTURE_KINDS.contains(&kind.as_str())) {
        bail!("`{name}` is no PNG, ICO, SVG or JPEG file");
    }
    Ok(())
}

/// An icon a pack gives: its picture and how it follows the theme.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PackIcon {
    pub file: PathBuf,
    /// The mask the manifest names, or the one that may lie next to the
    /// icon.
    mask: Option<PathBuf>,
    /// Whether the manifest named the mask, so that a missing one is a
    /// mistake.
    mask_named: bool,
    pub monochrome: bool,
}

impl PackIcon {
    fn of(directory: &Path, entry: &EntryText) -> Self {
        let (icon, mask, monochrome) = match entry {
            EntryText::File(icon) => (icon, None, false),
            EntryText::Full(table) => (&table.icon, table.mask.as_ref(), table.monochrome),
        };
        let file = directory.join(icon);
        let mask_named = mask.is_some();
        let mask = match mask {
            Some(mask) => Some(directory.join(mask)),
            None if monochrome => None,
            None => mask_beside(&file),
        };
        Self {
            file,
            mask,
            mask_named,
            monochrome,
        }
    }

    /// The mask of the icon, if there is one: the one the manifest names,
    /// or else a file next to the icon named like it with `-mask`.
    pub fn mask_file(&self) -> Option<&Path> {
        let mask = self.mask.as_deref()?;
        (self.mask_named || mask.is_file()).then_some(mask)
    }

    /// Whether it is drawn in a color of the theme, and so drawn again
    /// when the theme changes. Looks for a mask on the disk.
    fn follows_theme(&self) -> bool {
        self.monochrome || self.mask_file().is_some()
    }
}

/// Where a mask of `icon` lies when the manifest names none.
fn mask_beside(icon: &Path) -> Option<PathBuf> {
    let stem = icon.file_stem()?.to_str()?;
    Some(icon.with_file_name(format!("{stem}{MASK_SUFFIX}.png")))
}

/// One pack, read from its directory.
#[derive(Debug)]
struct IconPack {
    /// By the lowercase names `[apps]` gives.
    apps: HashMap<String, PackIcon>,
    /// By lowercase extension without its dot, or kind.
    files: HashMap<String, PackIcon>,
}

impl IconPack {
    fn read(directory: &Path) -> anyhow::Result<Self> {
        let manifest = directory.join(MANIFEST_FILE);
        let text = fs::read_to_string(&manifest)
            .with_context(|| format!("{} cannot be read", manifest.display()))?;
        let manifest = PackManifest::parse(&text)
            .with_context(|| format!("{} is not usable", manifest.display()))?;
        Ok(Self::of(directory, &manifest))
    }

    fn of(directory: &Path, manifest: &PackManifest) -> Self {
        let apps = { manifest.apps.iter() }
            .map(|(name, entry)| (name.to_lowercase(), PackIcon::of(directory, entry)))
            .collect();
        let files = { manifest.files.iter() }
            .map(|(kind, entry)| (file_key(kind), PackIcon::of(directory, entry)))
            .collect();
        Self { apps, files }
    }
}

/// What `[files]` knows a kind under: lowercase, an extension without its
/// dot.
fn file_key(kind: &str) -> String {
    kind.trim_start_matches('.').to_lowercase()
}

/// The color an icon that follows the theme is drawn in, and the one it
/// is drawn in where it stands for something active, as RGBA.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Tints {
    rest: [u8; 4],
    active: [u8; 4],
}

impl Tints {
    fn of(theme: &Theme) -> Self {
        Self {
            rest: rgba8(theme.colors.on_surface),
            active: rgba8(theme.colors.primary),
        }
    }
}

fn rgba8(color: Hsla) -> [u8; 4] {
    let color = color.to_rgb();
    [color.r, color.g, color.b, color.a].map(|part| (part.clamp(0., 1.) * 255.).round() as u8)
}

/// Draws an SVG file into an image, through GPUI; set when the shell
/// starts, as it needs the application.
type SvgDrawing = Box<dyn Fn(&[u8]) -> anyhow::Result<Arc<RenderImage>> + Send + Sync>;

static SVG: OnceLock<SvgDrawing> = OnceLock::new();

/// A picture drawn of an icon: the icon, the size, and whether active.
type DrawnKey = (PackIcon, u32, bool);

/// The packs in use, the first one first, and the pictures drawn of
/// their icons.
pub struct IconPacks {
    /// What the configuration names, in its order.
    ids: Vec<String>,
    packs: Vec<IconPack>,
    tints: Cell<Tints>,
    /// Pictures by icon, size and whether active; `None` for a picture
    /// that cannot be used, which was reported.
    /// Each with whether it is in the colors of the theme.
    drawn: RefCell<HashMap<DrawnKey, (Icon, bool)>>,
}

impl IconPacks {
    /// The packs `ids` names, from the directory of the icon packs. One
    /// that is missing or cannot be read is reported and left out.
    pub fn load(ids: &[String]) -> Self {
        match icon_packs_dir() {
            Some(root) => Self::load_from(&root, ids),
            None => Self::from_packs(ids, Vec::new()),
        }
    }

    fn load_from(root: &Path, ids: &[String]) -> Self {
        let mut packs = Vec::new();
        for id in ids {
            if !is_plain_id(id) {
                report!("`{id}` in `icon_packs` is not the name of an icon pack");
                continue;
            }
            match IconPack::read(&root.join(id)) {
                Ok(pack) => packs.push(pack),
                Err(error) => report!("the icon pack `{id}` is left out: {error:#}"),
            }
        }
        Self::from_packs(ids, packs)
    }

    fn from_packs(ids: &[String], packs: Vec<IconPack>) -> Self {
        Self {
            ids: ids.to_vec(),
            packs,
            tints: Cell::default(),
            drawn: RefCell::default(),
        }
    }

    /// No pack at all.
    pub fn none() -> Self {
        Self::from_packs(&[], Vec::new())
    }

    /// The packs as the configuration names them.
    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// The directories of the packs named, whether they are there or not.
    pub fn directories(&self) -> Vec<PathBuf> {
        let Some(root) = icon_packs_dir() else {
            return Vec::new();
        };
        let named = self.ids.iter().filter(|id| is_plain_id(id));
        named.map(|id| root.join(id)).collect()
    }

    /// The icon of an application known by any of `names` (the file name
    /// of its program, its identifier as a packaged application, the name
    /// of its shortcut), from the first pack that has one by any of them.
    pub fn app(&self, names: &[&str]) -> Option<&PackIcon> {
        let names: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
        self.packs
            .iter()
            .find_map(|pack| names.iter().find_map(|name| pack.apps.get(name)))
    }

    /// The icon of the kind of file `kind`, an extension with or without
    /// its dot, or `folder`, `drive`, `recycle-bin-full` or
    /// `recycle-bin-empty`, from the first pack that has one.
    pub fn file(&self, kind: &str) -> Option<&PackIcon> {
        let key = file_key(kind);
        self.packs.iter().find_map(|pack| pack.files.get(&key))
    }

    /// The picture a pack gives for the kind of file `ext_or_kind` (see
    /// [`Self::file`]), fitted into a square of `size` pixels; `None`
    /// where no pack has one or its picture cannot be used.
    #[allow(
        dead_code,
        reason = "for the icons of the desktop and the file manager"
    )]
    pub fn file_icon(&self, ext_or_kind: &str, size: u32) -> Icon {
        let icon = self.file(ext_or_kind)?.clone();
        self.image(&icon, size, false)
    }

    /// The picture of `icon` fitted into a square of `size` pixels, in
    /// the colors of the theme where it follows them, the active one if
    /// `active`.
    pub fn image(&self, icon: &PackIcon, size: u32, active: bool) -> Icon {
        let key = (icon.clone(), size, active);
        if let Some((found, _)) = self.drawn.borrow().get(&key) {
            return found.clone();
        }
        // An icon in its own colors is the same picture when active.
        let resting = (icon.clone(), size, false);
        let known = self.drawn.borrow().get(&resting).cloned();
        if active && let Some((found, false)) = known {
            self.drawn.borrow_mut().insert(key, (found.clone(), false));
            return found;
        }
        let tinted = icon.follows_theme();
        let tints = self.tints.get();
        let tint = if active { tints.active } else { tints.rest };
        let drawn = draw(icon, size, tint)
            .inspect_err(|error| report!("the icon {} is not used: {error:#}", icon.file.display()))
            .ok()
            .flatten();
        self.drawn.borrow_mut().insert(key, (drawn.clone(), tinted));
        drawn
    }

    /// Takes the colors of `theme` for the icons that follow it; those
    /// drawn in other colors are drawn again when next asked for.
    pub fn follow_theme(&self, theme: &Theme) {
        let tints = Tints::of(theme);
        if self.tints.replace(tints) != tints {
            self.drawn.borrow_mut().retain(|_, (_, tinted)| !*tinted);
        }
    }

    /// Starts what the packs need from the application: SVG pictures are
    /// drawn through GPUI, the icons follow the colors of the theme, and
    /// a change to the files of a pack in use is shown without a restart.
    pub fn start(cx: &mut App) {
        let renderer = cx.svg_renderer();
        let drawing: SvgDrawing = Box::new(move |bytes| {
            gpui::Image::from_bytes(ImageFormat::Svg, bytes.to_vec())
                .to_image_data(renderer.clone())
        });
        // A second start keeps the first drawing, which is the same.
        SVG.set(drawing).ok();
        Self::follow(cx);
        cx.observe_global::<Theme>(Self::follow).detach();
        cx.observe_global::<AppIcons>(Self::follow).detach();
        Self::watch(cx);
    }

    fn follow(cx: &mut App) {
        let theme = *cx.global::<Theme>();
        if let Some(icons) = cx.try_global::<AppIcons>() {
            icons.packs().follow_theme(&theme);
        }
    }

    /// Looks at the files of the packs in use every few seconds, away
    /// from the user interface, and loads the packs again once a change
    /// has settled. Widgets keep the icons they were built with, so they
    /// are built again too, as after an extension changed.
    fn watch(cx: &mut App) {
        cx.spawn(async move |cx: &mut AsyncApp| {
            let mut applied: Option<(Vec<PathBuf>, u64)> = None;
            let mut seen = None;
            loop {
                cx.background_executor().timer(WATCH_INTERVAL).await;
                let Ok(directories) = cx.update(|cx| cx.global::<AppIcons>().packs().directories())
                else {
                    break;
                };
                let looked = directories.clone();
                let stamp = cx
                    .background_executor()
                    .spawn(async move { stamp_of(&looked) })
                    .await;
                let settled = seen == Some(stamp);
                seen = Some(stamp);
                match &applied {
                    // Other packs were named meanwhile, and loaded then.
                    Some((known, _)) if *known != directories => {
                        applied = Some((directories, stamp));
                    }
                    Some((_, known)) if *known != stamp && settled => {
                        applied = Some((directories, stamp));
                        let reloaded = cx.update(|cx| {
                            let icons = cx.global::<AppIcons>().reloaded();
                            cx.set_global(icons);
                            Shell::extensions_changed(cx);
                        });
                        if reloaded.is_err() {
                            break;
                        }
                    }
                    Some(_) => {}
                    None => applied = Some((directories, stamp)),
                }
            }
        })
        .detach();
    }
}

/// What the files below `directories` are: it differs after any of them
/// was written, added or removed.
fn stamp_of(directories: &[PathBuf]) -> u64 {
    let mut files = Vec::new();
    let mut waiting: Vec<(PathBuf, usize)> = directories.iter().map(|d| (d.clone(), 0)).collect();
    while let Some((directory, depth)) = waiting.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                if depth < 3 {
                    waiting.push((path, depth + 1));
                }
                continue;
            }
            files.push((path, metadata.modified().ok(), metadata.len()));
        }
    }
    files.sort();
    let mut hasher = DefaultHasher::new();
    directories.hash(&mut hasher);
    files.hash(&mut hasher);
    hasher.finish()
}

/// The picture of `icon` in a square of `size` pixels, painted with
/// `tint` where it follows the theme.
fn draw(icon: &PackIcon, size: u32, tint: [u8; 4]) -> anyhow::Result<Icon> {
    let picture = read_picture(&icon.file, size)?;
    let picture = if icon.monochrome {
        monochrome(&picture, tint)
    } else if let Some(mask) = icon.mask_file() {
        let mask = read_picture(mask, size)
            .with_context(|| format!("its mask {} cannot be used", mask.display()))?;
        masked(picture, &mask, tint)
    } else {
        picture.into_rgba8()
    };
    Ok(crate::icons::prepared(
        DynamicImage::ImageRgba8(picture),
        size,
    ))
}

/// The picture file at `path`, an SVG drawn at `size` pixels.
fn read_picture(path: &Path, size: u32) -> anyhow::Result<DynamicImage> {
    let length = fs::metadata(path)
        .with_context(|| format!("{} cannot be read", path.display()))?
        .len();
    if length > PICTURE_LIMIT {
        bail!("{} is larger than 4 MiB", path.display());
    }
    let svg = { path.extension() }.is_some_and(|kind| kind.eq_ignore_ascii_case("svg"));
    if !svg {
        return image::open(path).with_context(|| format!("{} cannot be read", path.display()));
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("{} cannot be read", path.display()))?;
    let Some(drawing) = SVG.get() else {
        bail!("SVG pictures are drawn only once the shell runs");
    };
    let text = sized_svg(&text, size).unwrap_or(text);
    let drawn =
        drawing(text.as_bytes()).with_context(|| format!("{} is no SVG", path.display()))?;
    let dimensions = drawn.size(0);
    let pixels = drawn.as_bytes(0).context("the SVG drew nothing")?.to_vec();
    let mut pixels = RgbaImage::from_raw(
        dimensions.width.0 as u32,
        dimensions.height.0 as u32,
        pixels,
    )
    .context("the SVG drew nothing")?;
    unpremultiply(&mut pixels);
    Ok(DynamicImage::ImageRgba8(pixels))
}

/// GPUI draws an SVG as red first with the colors multiplied by their
/// coverage; images here keep the colors whole.
fn unpremultiply(pixels: &mut RgbaImage) {
    for pixel in pixels.pixels_mut() {
        let alpha = u32::from(pixel.0[3]);
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for part in &mut pixel.0[..3] {
            *part = ((u32::from(*part) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

/// `svg` with its outermost element made `size` pixels wide and high, so
/// that it is drawn sharp at that size instead of at the size it states.
/// What it showed keeps its place: a stated size without a `viewBox`
/// becomes one. `None` where `svg` has no `<svg` element.
fn sized_svg(svg: &str, size: u32) -> Option<String> {
    let start = svg.match_indices("<svg").map(|(at, _)| at).find(|&at| {
        let next = svg[at + 4..].chars().next();
        next.is_some_and(|next| next.is_whitespace() || next == '>' || next == '/')
    })?;
    let end = start + svg[start..].find('>')?;
    let inner = svg[start + 4..end].trim_end();
    let (inner, closed) = match inner.strip_suffix('/') {
        Some(inner) => (inner, "/"),
        None => (inner, ""),
    };
    let attributes = attributes(inner)?;
    let value = |name: &str| {
        let found = attributes.iter().find(|(known, _)| *known == name);
        found.map(|(_, value)| *value)
    };
    let length = |name: &str| -> Option<f32> {
        let text = value(name)?.trim();
        let number = text.strip_suffix("px").unwrap_or(text);
        number.parse().ok().filter(|number: &f32| *number > 0.)
    };
    let mut tag = String::from("<svg");
    for (name, value) in &attributes {
        if matches!(*name, "width" | "height") {
            continue;
        }
        tag.push_str(&attribute(name, value));
    }
    if value("viewBox").is_none()
        && let (Some(width), Some(height)) = (length("width"), length("height"))
    {
        tag.push_str(&attribute("viewBox", &format!("0 0 {width} {height}")));
    }
    tag.push_str(&format!(" width=\"{size}\" height=\"{size}\"{closed}"));
    Some(format!("{}{tag}{}", &svg[..start], &svg[end..]))
}

/// ` name="value"`, with the quotes `value` does not hold.
fn attribute(name: &str, value: &str) -> String {
    match value.contains('"') {
        true => format!(" {name}='{value}'"),
        false => format!(" {name}=\"{value}\""),
    }
}

/// The attributes of a start tag after its name, as written; `None` where
/// they cannot be told apart.
fn attributes(mut text: &str) -> Option<Vec<(&str, &str)>> {
    let mut found = Vec::new();
    loop {
        text = text.trim_start();
        if text.is_empty() {
            return Some(found);
        }
        let name_end = text.find(|c: char| c == '=' || c.is_whitespace())?;
        let name = &text[..name_end];
        text = text[name_end..]
            .trim_start()
            .strip_prefix('=')?
            .trim_start();
        let quote = text.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        text = &text[1..];
        let value_end = text.find(quote)?;
        found.push((name, &text[..value_end]));
        text = &text[value_end + 1..];
    }
}

/// The shape of `picture`, every pixel of it in `tint` with the coverage
/// of the picture there.
fn monochrome(picture: &DynamicImage, tint: [u8; 4]) -> RgbaImage {
    let coverage = coverage(picture);
    let mut out = RgbaImage::new(picture.width(), picture.height());
    for (pixel, covered) in out.pixels_mut().zip(coverage) {
        pixel.0 = [tint[0], tint[1], tint[2], scaled(covered, tint[3])];
    }
    out
}

/// `picture` with `tint` laid over the parts `mask` covers.
fn masked(picture: DynamicImage, mask: &DynamicImage, tint: [u8; 4]) -> RgbaImage {
    let mut picture = picture.into_rgba8();
    let (width, height) = picture.dimensions();
    let mask = match mask.width() == width && mask.height() == height {
        true => mask.clone(),
        false => mask.resize_exact(width, height, FilterType::Triangle),
    };
    for (pixel, covered) in picture.pixels_mut().zip(coverage(&mask)) {
        pixel.0 = over(pixel.0, tint, scaled(covered, tint[3]));
    }
    picture
}

/// How much of each pixel `picture` covers: its alpha, or its brightness
/// where it has no alpha.
fn coverage(picture: &DynamicImage) -> Vec<u8> {
    if picture.color().has_alpha() {
        picture
            .to_rgba8()
            .pixels()
            .map(|pixel| pixel.0[3])
            .collect()
    } else {
        picture
            .to_luma8()
            .pixels()
            .map(|pixel| pixel.0[0])
            .collect()
    }
}

fn scaled(value: u8, by: u8) -> u8 {
    ((u32::from(value) * u32::from(by) + 127) / 255) as u8
}

/// `color` laid with `coverage` over `under`, neither premultiplied.
fn over(under: [u8; 4], color: [u8; 4], coverage: u8) -> [u8; 4] {
    let top = f32::from(coverage) / 255.;
    let bottom = f32::from(under[3]) / 255. * (1. - top);
    let alpha = top + bottom;
    if alpha <= 0. {
        return [0; 4];
    }
    let mix = |index: usize| {
        let value = (f32::from(color[index]) * top + f32::from(under[index]) * bottom) / alpha;
        value.round().clamp(0., 255.) as u8
    };
    [mix(0), mix(1), mix(2), (alpha * 255.).round() as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty directory for one test.
    fn scratch(test: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("tessera-packs-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn write_pack(root: &Path, id: &str, body: &str) {
        let directory = root.join(id);
        fs::create_dir_all(&directory).unwrap();
        let text = format!("id = \"{id}\"\nname = \"{id}\"\nversion = \"1\"\n{body}");
        fs::write(directory.join(MANIFEST_FILE), text).unwrap();
    }

    #[test]
    fn a_manifest_names_pictures_within_its_pack() {
        let manifest = PackManifest::parse(concat!(
            "id = \"line\"\nname = \"Line\"\nauthor = \"A\"\nversion = \"1.0.0\"\n",
            "[apps]\n\"notepad.exe\" = \"apps/notepad.svg\"\n",
            "[files]\nfolder = { icon = \"folder.png\", mask = \"folder-tint.png\" }\n",
            "txt = { icon = \"text.svg\", monochrome = true }\n",
        ))
        .unwrap();
        assert_eq!(manifest.author, "A");
        assert_eq!(
            manifest.files(),
            [
                "apps/notepad.svg",
                "folder-tint.png",
                "folder.png",
                "text.svg"
            ]
        );
        let bad = |body: &str| {
            PackManifest::parse(&format!(
                "id = \"x\"\nname = \"x\"\nversion = \"1\"\n{body}"
            ))
            .is_err()
        };
        assert!(bad("[apps]\na = \"../a.png\"\n"));
        assert!(bad("[apps]\na = \"C:/a.png\"\n"));
        assert!(bad("[apps]\na = \"a.txt\"\n"));
        assert!(bad("[files]\ntxt = { icon = \"a.png\", tint = true }\n"));
        assert!(bad("colour = 1\n"));
        assert!(PackManifest::parse("id = \"Bad Id\"\nname = \"x\"\nversion = \"1\"\n").is_err());
    }

    #[test]
    fn the_first_pack_that_has_an_icon_gives_it() {
        let root = scratch("order");
        write_pack(
            &root,
            "first",
            "[apps]\n\"Code.exe\" = \"code.png\"\n[files]\n\".TXT\" = \"text.png\"\n",
        );
        write_pack(
            &root,
            "second",
            concat!(
                "[apps]\n\"code.exe\" = \"other.png\"\n\"notepad.exe\" = \"notepad.png\"\n",
                "\"Microsoft.WindowsCalculator_8wekyb3d8bbwe!App\" = \"calc.png\"\n",
                "[files]\nfolder = \"folder.png\"\n",
            ),
        );
        let ids = ["missing", "first", "second"].map(str::to_owned);
        let packs = IconPacks::load_from(&root, &ids);
        assert_eq!(packs.ids(), ids);
        let app = |names: &[&str]| packs.app(names).map(|icon| icon.file.clone());
        assert_eq!(app(&["CODE.EXE"]), Some(root.join("first/code.png")));
        // The second pack gives what the first has not, by any name.
        assert_eq!(
            app(&["Notepad", "notepad.exe"]),
            Some(root.join("second/notepad.png"))
        );
        assert_eq!(
            app(&["microsoft.windowscalculator_8wekyb3d8bbwe!app"]),
            Some(root.join("second/calc.png"))
        );
        assert_eq!(app(&["paint.exe"]), None);
        let file = |kind: &str| packs.file(kind).map(|icon| icon.file.clone());
        assert_eq!(file("txt"), Some(root.join("first/text.png")));
        assert_eq!(file(".Txt"), Some(root.join("first/text.png")));
        assert_eq!(file(FOLDER), Some(root.join("second/folder.png")));
        assert_eq!(file(DRIVE), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_mask_is_the_named_one_or_the_one_next_to_the_icon() {
        let root = scratch("masks");
        write_pack(
            &root,
            "masks",
            concat!(
                "[files]\nfolder = \"folder.png\"\ntxt = \"text.png\"\n",
                "zip = { icon = \"zip.png\", mask = \"tint/zip.png\" }\n",
                "md = { icon = \"md.svg\", monochrome = true }\n",
            ),
        );
        fs::write(root.join("masks/folder-mask.png"), b"").unwrap();
        let packs = IconPacks::load_from(&root, &["masks".to_owned()]);
        let mask = |kind: &str| packs.file(kind).unwrap().mask_file().map(Path::to_owned);
        assert_eq!(mask("folder"), Some(root.join("masks/folder-mask.png")));
        assert_eq!(mask("txt"), None);
        // A named mask is used even before it is there, and then reported.
        assert_eq!(mask("zip"), Some(root.join("masks/tint/zip.png")));
        assert_eq!(mask("md"), None);
        assert!(packs.file("md").unwrap().monochrome);
        assert!(packs.file("folder").unwrap().follows_theme());
        assert!(!packs.file("txt").unwrap().follows_theme());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_icon_in_the_colors_of_the_theme_keeps_its_shape() {
        let tint = [10, 20, 30, 255];
        let mut picture = RgbaImage::from_pixel(2, 1, image::Rgba([200, 0, 0, 255]));
        picture.put_pixel(1, 0, image::Rgba([0, 200, 0, 64]));
        let drawn = monochrome(&DynamicImage::ImageRgba8(picture.clone()), tint);
        assert_eq!(drawn.get_pixel(0, 0).0, [10, 20, 30, 255]);
        assert_eq!(drawn.get_pixel(1, 0).0, [10, 20, 30, 64]);

        // A mask without alpha covers by its brightness.
        let mut mask = image::GrayImage::new(2, 1);
        mask.put_pixel(0, 0, image::Luma([255]));
        let drawn = masked(
            DynamicImage::ImageRgba8(picture),
            &DynamicImage::ImageLuma8(mask),
            tint,
        );
        assert_eq!(drawn.get_pixel(0, 0).0, [10, 20, 30, 255]);
        assert_eq!(drawn.get_pixel(1, 0).0, [0, 200, 0, 64]);
        assert_eq!(over([0, 0, 0, 0], [9, 9, 9, 255], 0), [0; 4]);
        assert_eq!(over([0, 0, 0, 255], [255, 255, 255, 255], 128)[3], 255);
    }

    #[test]
    fn an_svg_is_drawn_at_the_size_asked_for() {
        assert_eq!(
            sized_svg(
                "<?xml version=\"1.0\"?>\n<svg xmlns='http://www.w3.org/2000/svg' width=\"24px\" height=\"16\"><path d=\"M0 0\"/></svg>",
                64
            )
            .unwrap(),
            "<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 16\" width=\"64\" height=\"64\"><path d=\"M0 0\"/></svg>"
        );
        assert_eq!(
            sized_svg("<svg viewBox=\"0 0 8 8\" width=\"100%\"/>", 32).unwrap(),
            "<svg viewBox=\"0 0 8 8\" width=\"32\" height=\"32\"/>"
        );
        assert_eq!(
            sized_svg("<svgx/><svg\n  a = 'say \"hi\"'>", 8).unwrap(),
            "<svgx/><svg a='say \"hi\"' width=\"8\" height=\"8\">"
        );
        assert!(sized_svg("<html></html>", 8).is_none());
        assert!(sized_svg("<svg width=24>", 8).is_none());
    }

    #[test]
    fn an_svg_drawn_by_gpui_gets_its_colors_whole() {
        let mut pixels = RgbaImage::from_pixel(1, 1, image::Rgba([64, 32, 0, 128]));
        unpremultiply(&mut pixels);
        assert_eq!(pixels.get_pixel(0, 0).0, [128, 64, 0, 128]);
    }

    #[test]
    fn a_change_to_a_pack_changes_its_stamp() {
        let root = scratch("stamp");
        write_pack(&root, "a", "");
        let directories = [root.join("a"), root.join("not-there")];
        let before = stamp_of(&directories);
        assert_eq!(before, stamp_of(&directories));
        fs::create_dir_all(root.join("a/apps")).unwrap();
        fs::write(root.join("a/apps/x.png"), b"1").unwrap();
        assert_ne!(before, stamp_of(&directories));
        fs::remove_dir_all(root).unwrap();
    }
}
