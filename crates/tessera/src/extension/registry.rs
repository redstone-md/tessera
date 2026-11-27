// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Registries: lists of extensions, themes, icon packs, wallpapers and
//! tiling layouts that can be installed. A registry can name further
//! registries, whose offers are made along with its own.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::access::Permissions;
use super::{DeviceKind, MANIFEST_FILE, Manifest, STATE_FILE, WASM_FILE, http, is_identifier};
use crate::config::edit::Slot;
use crate::i18n::Language;
use crate::icon_packs::{self, PackManifest, check_picture_name};
use crate::theme::Theme;

/// The registry that is read when the configuration names none.
pub const OFFICIAL_REGISTRY: &str =
    "https://raw.githubusercontent.com/redstone-md/tessera/main/registry/registry.toml";

/// Whether `name` can name what a registry offers: a-z, 0-9 and `-`.
pub fn is_plain_id(name: &str) -> bool {
    is_identifier(name)
}

/// Written next to an extension or an icon pack that was installed from
/// a registry.
const RECEIPT_FILE: &str = "installed.toml";

/// What a file installed from a registry ends in, and the file written
/// next to one that was: `<id>.installed`.
const THEME_EXTENSION: &str = "toml";
const THEME_RECEIPT_EXTENSION: &str = "installed";

/// The kinds of pictures a wallpaper may be.
const WALLPAPER_EXTENSIONS: [&str; 5] = ["jpg", "jpeg", "png", "bmp", "webp"];

/// The most that is read of a registry, a manifest, a WebAssembly file,
/// a theme, a picture of an offer, a wallpaper, a tiling layout, and of a
/// picture of an icon pack, all of them together, and how many those are.
const REGISTRY_LIMIT: u64 = 1 << 20;
const MANIFEST_LIMIT: u64 = 64 << 10;
const WASM_LIMIT: u64 = 32 << 20;
const THEME_LIMIT: u64 = 64 << 10;
const PREVIEW_LIMIT: u64 = 2 << 20;
const WALLPAPER_LIMIT: u64 = 24 << 20;
const LAYOUT_LIMIT: u64 = 64 << 10;
const PACK_PICTURE_LIMIT: u64 = 4 << 20;
const PACK_LIMIT: u64 = 32 << 20;
const PACK_FILES: usize = 1024;

/// What a registry offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Code that adds widgets; kept in a directory of its own.
    Extension,
    /// A theme file.
    Theme,
    /// A directory of pictures in place of the icons of applications and
    /// files.
    IconPack,
    /// A picture for the desktop.
    Wallpaper,
    /// A file that describes how windows are tiled.
    TilingLayout,
}

impl Kind {
    pub const ALL: [Self; 5] = [
        Self::Extension,
        Self::Theme,
        Self::IconPack,
        Self::Wallpaper,
        Self::TilingLayout,
    ];

    /// What the marketplace's `show` calls this kind, which is also the
    /// directory it is installed into within that of the configuration
    /// (but for extensions, which have their own).
    pub fn name(self) -> &'static str {
        match self {
            Self::Extension => "extensions",
            Self::Theme => "themes",
            Self::IconPack => "icon-packs",
            Self::Wallpaper => "wallpapers",
            Self::TilingLayout => "tiling-layouts",
        }
    }

    /// What a `tessera://` link calls one of this kind.
    pub fn singular(self) -> &'static str {
        match self {
            Self::Extension => "extension",
            Self::Theme => "theme",
            Self::IconPack => "icon-pack",
            Self::Wallpaper => "wallpaper",
            Self::TilingLayout => "tiling-layout",
        }
    }

    /// The kind `text` names, as [`Self::name`] or [`Self::singular`].
    pub fn named(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.name() == text || kind.singular() == text)
    }

    /// The heading of the kind, in English.
    pub fn label(self) -> &'static str {
        match self {
            Self::Extension => "Extensions",
            Self::Theme => "Themes",
            Self::IconPack => "Icon packs",
            Self::Wallpaper => "Wallpapers",
            Self::TilingLayout => "Tiling layouts",
        }
    }

    /// The extension of the one file an offer of this kind is, if it is
    /// one file of a fixed kind.
    fn file_extension(self) -> Option<&'static str> {
        match self {
            Self::Theme | Self::TilingLayout => Some(THEME_EXTENSION),
            Self::Extension | Self::IconPack | Self::Wallpaper => None,
        }
    }
}

/// How many registries are read at most, and through how many registries
/// one may be reached from those of the configuration.
const MOST_REGISTRIES: usize = 16;
const DEEPEST_REGISTRY: usize = 3;

/// Where a registry or one of its files is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Location {
    /// An `https://` address.
    Web(String),
    File(PathBuf),
}

impl Location {
    /// The location a configuration names: an `https://` address or the
    /// full path of a file.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        if text.starts_with("https://") {
            return Ok(Self::Web(text.to_owned()));
        }
        let path = Path::new(text);
        if text.contains("://") || !path.is_absolute() {
            bail!("`{text}` is neither an https:// address nor the full path of a file");
        }
        Ok(Self::File(path.to_owned()))
    }

    /// The location a registry here means by `reference`: an `https://`
    /// address, or a path below the registry's own directory. A registry
    /// on the web can therefore not point at a file of this computer.
    fn join(&self, reference: &str) -> anyhow::Result<Self> {
        if reference.starts_with("https://") {
            return Ok(Self::Web(reference.to_owned()));
        }
        let plain =
            |part: &str| !matches!(part, "" | "." | "..") && !part.contains([':', '\\', '?', '#']);
        if !reference.split('/').all(plain) {
            bail!("`{reference}` is neither an https:// address nor a path below the registry");
        }
        Ok(match self {
            Self::Web(base) => {
                let base = base.split(['?', '#']).next().unwrap_or(base);
                let host = "https://".len();
                let directory = match base[host..].rfind('/') {
                    Some(slash) => &base[..host + slash],
                    None => base,
                };
                Self::Web(format!("{directory}/{reference}"))
            }
            Self::File(base) => {
                let directory = base.parent().unwrap_or(Path::new(""));
                Self::File(directory.join(reference))
            }
        })
    }

    /// What is there, if it is no longer than `limit`.
    fn read(&self, limit: u64) -> anyhow::Result<Vec<u8>> {
        let content = match self {
            Self::Web(address) => http::download(address, limit).map_err(|error| anyhow!(error)),
            Self::File(path) => fs::metadata(path)
                .and_then(|file| match file.len() > limit {
                    true => Err(ErrorKind::FileTooLarge.into()),
                    false => fs::read(path),
                })
                .map_err(Into::into),
        };
        content.with_context(|| format!("cannot read {self}"))
    }

    /// The extension of the file named, in lowercase.
    fn extension(&self) -> Option<String> {
        let text = match self {
            Self::Web(address) => address.split(['?', '#']).next()?.rsplit('/').next()?,
            Self::File(path) => path.file_name()?.to_str()?,
        };
        let (_, extension) = text.rsplit_once('.')?;
        Some(extension.to_ascii_lowercase())
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Web(address) => f.write_str(address),
            Self::File(path) => write!(f, "{}", path.display()),
        }
    }
}

/// A registry as its file is written.
#[derive(Deserialize)]
struct Registry {
    name: String,
    #[serde(default, rename = "extension")]
    extensions: Vec<Entry>,
    #[serde(default, rename = "theme")]
    themes: Vec<FileEntry>,
    #[serde(default, rename = "icon_pack")]
    icon_packs: Vec<PackEntry>,
    #[serde(default, rename = "wallpaper")]
    wallpapers: Vec<FileEntry>,
    #[serde(default, rename = "tiling_layout")]
    tiling_layouts: Vec<FileEntry>,
    #[serde(default, rename = "registry")]
    registries: Vec<Link>,
}

/// An offer that is one file: a theme, a wallpaper, a tiling layout.
#[derive(Deserialize)]
struct FileEntry {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    version: String,
    file: String,
    sha256: String,
    #[serde(flatten)]
    preview: PreviewEntry,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    version: String,
    manifest: String,
    manifest_sha256: String,
    wasm: String,
    wasm_sha256: String,
    /// Where the extension suggests a widget of it is put. Read as it
    /// is and checked when the offer is fetched, so a place this version
    /// cannot read leaves out only the place, not the registry.
    place: Option<toml::Value>,
    #[serde(flatten)]
    preview: PreviewEntry,
}

/// An icon pack as a registry offers it: its manifest and every picture
/// the manifest names, each by its path within the pack.
#[derive(Deserialize)]
struct PackEntry {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    version: String,
    manifest: String,
    manifest_sha256: String,
    #[serde(default)]
    files: Vec<PackFile>,
    #[serde(flatten)]
    preview: PreviewEntry,
}

#[derive(Deserialize)]
struct PackFile {
    path: String,
    sha256: String,
}

/// Where a registry suggests a widget of an extension it offers is put:
/// `place = { surface = "bar", slot = "end" }`, with `widget` naming one
/// of the extension's widgets (the first one where it does not).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub widget: Option<String>,
    /// `bar`, the first surface along the top of the display, or `dock`,
    /// the first along the bottom; another surface where there is none.
    #[serde(default = "bar")]
    pub surface: String,
    /// `start`, `center` or `end`.
    pub slot: String,
}

fn bar() -> String {
    "bar".to_owned()
}

impl Placement {
    /// The part of the surface it names.
    pub fn slot(&self) -> Option<Slot> {
        Slot::ALL
            .into_iter()
            .find(|slot| slot.label().eq_ignore_ascii_case(&self.slot))
    }

    /// Whether it names the dock rather than the bar.
    pub fn in_dock(&self) -> bool {
        self.surface == "dock"
    }

    fn check(&self, widgets: &[String]) -> anyhow::Result<()> {
        if self.slot().is_none() {
            bail!("`{}` is not `start`, `center` or `end`", self.slot);
        }
        if !matches!(self.surface.as_str(), "bar" | "dock") {
            bail!("`{}` is neither `bar` nor `dock`", self.surface);
        }
        if let Some(widget) = &self.widget
            && !widgets
                .iter()
                .any(|known| known.ends_with(&format!(".{widget}")))
        {
            bail!("the extension has no widget `{widget}`");
        }
        Ok(())
    }
}

/// A picture of what an offer looks like: a JPEG or PNG file and its
/// SHA-256 checksum. Both or neither are given.
#[derive(Default, Deserialize)]
struct PreviewEntry {
    preview: Option<String>,
    preview_sha256: Option<String>,
}

impl PreviewEntry {
    /// The picture to download, where the registry names one. One that
    /// cannot be used leaves the offer without a picture, and says why.
    fn download(&self, registry: &Location, id: &str) -> Option<Download> {
        let (file, checksum) = match (&self.preview, &self.preview_sha256) {
            (None, None) => return None,
            (Some(file), Some(checksum)) => (file, checksum),
            _ => {
                report!("the picture of `{id}` needs both `preview` and `preview_sha256`");
                return None;
            }
        };
        Download::new(registry, file, checksum)
            .inspect_err(|error| report!("the picture of `{id}` is left out: {error:#}"))
            .ok()
    }
}

#[derive(Deserialize)]
struct Link {
    url: String,
}

/// A file of an offer and the SHA-256 checksum its registry gives.
#[derive(Clone, Debug)]
struct Download {
    location: Location,
    checksum: String,
}

impl Download {
    fn new(registry: &Location, reference: &str, checksum: &str) -> anyhow::Result<Self> {
        Ok(Self {
            checksum: checked_sum(checksum)?,
            location: registry.join(reference)?,
        })
    }

    /// The file, if it is the one the registry listed.
    fn fetch(&self, limit: u64) -> anyhow::Result<Vec<u8>> {
        let content = self.location.read(limit)?;
        let checksum: String = Sha256::digest(&content)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if checksum != self.checksum {
            bail!(
                "{} is not the file its registry lists: the checksums differ",
                self.location
            );
        }
        Ok(content)
    }
}

/// The files of an offer.
#[derive(Clone, Debug)]
enum Files {
    Extension {
        manifest: Download,
        wasm: Download,
    },
    /// One file, of the offer's kind.
    Single(Download),
    /// The manifest of an icon pack and its pictures by their paths.
    IconPack {
        manifest: Download,
        pictures: Vec<(String, Download)>,
    },
}

/// What a registry offers, of any kind.
#[derive(Clone, Debug)]
pub struct Offer {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    /// The name of the registry that offers it.
    pub registry: String,
    /// Where the registry that offers it is.
    pub source: Location,
    /// Where the registry suggests a widget of an extension is put, as
    /// the registry writes it.
    place: Option<toml::Value>,
    kind: Kind,
    files: Files,
    /// A picture of what it looks like.
    preview: Option<Download>,
}

impl Offer {
    fn new(entry: Entry, registry: &str, location: &Location) -> anyhow::Result<Self> {
        check_id(&entry.id)?;
        let download = |reference: &str, checksum: &str| {
            Download::new(location, reference, checksum)
                .with_context(|| format!("the extension `{}` is left out", entry.id))
        };
        Ok(Self {
            preview: entry.preview.download(location, &entry.id),
            files: Files::Extension {
                manifest: download(&entry.manifest, &entry.manifest_sha256)?,
                wasm: download(&entry.wasm, &entry.wasm_sha256)?,
            },
            kind: Kind::Extension,
            place: entry.place,
            id: entry.id,
            name: entry.name,
            description: entry.description,
            version: entry.version,
            registry: registry.to_owned(),
            source: location.clone(),
        })
    }

    /// An offer of one file of `kind`.
    fn single(
        kind: Kind,
        entry: FileEntry,
        registry: &str,
        location: &Location,
    ) -> anyhow::Result<Self> {
        check_id(&entry.id)?;
        let left_out = || format!("the {} `{}` is left out", kind.singular(), entry.id);
        let file = Download::new(location, &entry.file, &entry.sha256).with_context(left_out)?;
        if kind == Kind::Wallpaper {
            let extension = file.location.extension().unwrap_or_default();
            if !WALLPAPER_EXTENSIONS.contains(&extension.as_str()) {
                bail!("{}: {} is no picture file", left_out(), file.location);
            }
        }
        Ok(Self {
            preview: entry.preview.download(location, &entry.id),
            files: Files::Single(file),
            kind,
            place: None,
            id: entry.id,
            name: entry.name,
            description: entry.description,
            version: entry.version,
            registry: registry.to_owned(),
            source: location.clone(),
        })
    }

    fn icon_pack(entry: PackEntry, registry: &str, location: &Location) -> anyhow::Result<Self> {
        check_id(&entry.id)?;
        let left_out = || format!("the icon pack `{}` is left out", entry.id);
        let manifest = Download::new(location, &entry.manifest, &entry.manifest_sha256)
            .with_context(left_out)?;
        if entry.files.len() > PACK_FILES {
            bail!("{}: it lists more than {PACK_FILES} files", left_out());
        }
        let mut pictures = Vec::new();
        for file in &entry.files {
            check_picture_name(&file.path).with_context(left_out)?;
            let picture = Download {
                location: manifest.location.join(&file.path).with_context(left_out)?,
                checksum: checked_sum(&file.sha256).with_context(left_out)?,
            };
            pictures.push((file.path.clone(), picture));
        }
        Ok(Self {
            preview: entry.preview.download(location, &entry.id),
            files: Files::IconPack { manifest, pictures },
            kind: Kind::IconPack,
            place: None,
            id: entry.id,
            name: entry.name,
            description: entry.description,
            version: entry.version,
            registry: registry.to_owned(),
            source: location.clone(),
        })
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Whether the registry shows a picture of the offer.
    pub fn has_preview(&self) -> bool {
        self.preview.is_some()
    }

    /// Downloads the picture of the offer, if it is the one the registry
    /// lists.
    pub fn fetch_preview(&self) -> anyhow::Result<Option<Vec<u8>>> {
        self.preview
            .as_ref()
            .map(|preview| preview.fetch(PREVIEW_LIMIT))
            .transpose()
    }

    /// Downloads the offer and checks it against what the registry lists.
    /// Nothing is installed yet; what an extension asks for is said in
    /// `language`.
    pub fn fetch(&self, language: Language) -> anyhow::Result<Package> {
        let receipt = Receipt {
            version: self.version.clone(),
            registry: self.registry.clone(),
            files: Vec::new(),
        };
        let package = |content| Package {
            id: self.id.clone(),
            kind: self.kind,
            receipt,
            wishes: Vec::new(),
            widgets: Vec::new(),
            place: None,
            content,
        };
        match &self.files {
            Files::Extension { manifest, wasm } => {
                let manifest = String::from_utf8(manifest.fetch(MANIFEST_LIMIT)?)
                    .context("the manifest is not text")?;
                let wasm = wasm.fetch(WASM_LIMIT)?;
                let parsed = Manifest::parse(&manifest).context("the manifest is invalid")?;
                if parsed.id != self.id {
                    bail!(
                        "the registry offers `{}` but the manifest is that of `{}`",
                        self.id,
                        parsed.id
                    );
                }
                let widgets: Vec<String> = { parsed.widgets.iter() }
                    .map(|widget| parsed.qualified(&widget.name))
                    .collect();
                let place = self.place.clone().and_then(|place| {
                    Placement::deserialize(place)
                        .map_err(anyhow::Error::from)
                        .and_then(|place| place.check(&widgets).map(|()| place))
                        .inspect_err(|error| {
                            report!("the place `{}` suggests is ignored: {error:#}", self.id)
                        })
                        .ok()
                });
                Ok(Package {
                    wishes: wishes(&parsed.permissions, language),
                    widgets,
                    place,
                    ..package(Content::Extension { manifest, wasm })
                })
            }
            Files::Single(file) => {
                let (limit, extension) = match self.kind {
                    Kind::Theme => (THEME_LIMIT, THEME_EXTENSION.to_owned()),
                    Kind::TilingLayout => (LAYOUT_LIMIT, THEME_EXTENSION.to_owned()),
                    _ => (
                        WALLPAPER_LIMIT,
                        file.location.extension().unwrap_or_default(),
                    ),
                };
                let content = file.fetch(limit)?;
                check_single(self.kind, &content)?;
                Ok(package(Content::File { extension, content }))
            }
            Files::IconPack { manifest, pictures } => {
                let text = String::from_utf8(manifest.fetch(MANIFEST_LIMIT)?)
                    .context("the manifest is not text")?;
                let parsed = PackManifest::parse(&text).context("the manifest is invalid")?;
                if parsed.id != self.id {
                    bail!(
                        "the registry offers `{}` but the manifest is that of `{}`",
                        self.id,
                        parsed.id
                    );
                }
                for named in parsed.files() {
                    if !pictures.iter().any(|(path, _)| *path == named) {
                        bail!("the manifest names `{named}`, which the registry does not list");
                    }
                }
                let mut total = 0;
                let mut files = Vec::new();
                for (path, picture) in pictures {
                    let content = picture.fetch(PACK_PICTURE_LIMIT)?;
                    total += content.len() as u64;
                    if total > PACK_LIMIT {
                        bail!("the icon pack is larger than 32 MiB");
                    }
                    files.push((path.clone(), content));
                }
                Ok(package(Content::IconPack {
                    manifest: text,
                    files,
                }))
            }
        }
    }
}

/// `checksum`, if it is a SHA-256 checksum, in lowercase.
fn checked_sum(checksum: &str) -> anyhow::Result<String> {
    let checksum = checksum.to_ascii_lowercase();
    if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("`{checksum}` is not a SHA-256 checksum");
    }
    Ok(checksum)
}

fn check_id(id: &str) -> anyhow::Result<()> {
    if !is_identifier(id) {
        bail!("`{id}` is not a name made of a-z, 0-9 and `-`");
    }
    Ok(())
}

/// Whether `content` is what an offer of `kind` that is one file has to
/// be: a theme file, a picture, or a TOML file.
fn check_single(kind: Kind, content: &[u8]) -> anyhow::Result<()> {
    match kind {
        Kind::Theme => {
            let theme = std::str::from_utf8(content).context("the theme is not text")?;
            Theme::check(theme).context("the theme is invalid")?;
        }
        Kind::TilingLayout => {
            let layout = std::str::from_utf8(content).context("the layout is not text")?;
            toml::from_str::<toml::Table>(layout).context("the layout is no TOML file")?;
        }
        Kind::Wallpaper => {
            image::guess_format(content).context("the wallpaper is no picture")?;
        }
        Kind::Extension | Kind::IconPack => {}
    }
    Ok(())
}

/// What an extension with `permissions` asks for, one wish a line, in
/// `language`.
///
/// Stand-in for `Permissions::wishes`, which the extension API adds with
/// the further kinds of wishes; it gives the same lines for the three
/// kinds there are now. To be removed when that is merged.
fn wishes(permissions: &Permissions, language: Language) -> Vec<String> {
    let say = |text| language.say(text);
    let mut wishes = Vec::new();
    let network = permissions.network();
    if !network.is_empty() {
        wishes.push(format!("{} {}", say("contact"), network.join(", ")));
    }
    let files = permissions.files();
    if !files.is_empty() {
        wishes.push(format!("{} {}", say("read"), files.join(", ")));
    }
    let devices = permissions.devices();
    if !devices.is_empty() {
        let kinds: Vec<_> = devices
            .iter()
            .map(|&kind: &DeviceKind| kind.name())
            .collect();
        let kinds = kinds.join(", ");
        // English names the kinds in the middle of the phrase.
        wishes.push(match language {
            Language::English => format!("be told of the {kinds} devices"),
            _ => format!("{} {kinds}", say("be told of the devices")),
        });
    }
    wishes
}

/// What the registries offer, and what went wrong while reading them.
#[derive(Debug, Default)]
pub struct Catalog {
    pub offers: Vec<Offer>,
    pub problems: Vec<String>,
}

impl Catalog {
    /// Reads the registries at `roots` and those they name. What
    /// several registries offer is taken from the one read first:
    /// `roots` in their order, then the registries they name.
    pub fn browse(roots: &[String]) -> Self {
        let mut catalog = Self::default();
        let mut waiting = VecDeque::new();
        for root in roots {
            match Location::parse(root) {
                Ok(location) => waiting.push_back((location, 0)),
                Err(error) => catalog.problems.push(format!("{error:#}")),
            }
        }
        let mut read = HashSet::new();
        while let Some((location, depth)) = waiting.pop_front() {
            if read.contains(&location) {
                continue;
            }
            if read.len() == MOST_REGISTRIES {
                let problem = format!("only the first {MOST_REGISTRIES} registries are read");
                catalog.problems.push(problem);
                break;
            }
            read.insert(location.clone());
            let text = location
                .read(REGISTRY_LIMIT)
                .and_then(|text| String::from_utf8(text).context("it is not text"))
                .and_then(|text| catalog.add(&location, &text));
            match text {
                Ok(links) if depth < DEEPEST_REGISTRY => {
                    waiting.extend(links.into_iter().map(|link| (link, depth + 1)));
                }
                Ok(links) if links.is_empty() => {}
                Ok(_) => catalog.problems.push(format!(
                    "the registries {location} names are not read: they are nested too deeply"
                )),
                Err(error) => catalog
                    .problems
                    .push(format!("{location} is not read: {error:#}")),
            }
        }
        catalog
    }

    /// Adds the offers of the registry written as `text` and answers the
    /// registries it names.
    fn add(&mut self, location: &Location, text: &str) -> anyhow::Result<Vec<Location>> {
        let registry: Registry = toml::from_str(text)?;
        let name = &registry.name;
        let mut offers = Vec::new();
        offers.extend(
            { registry.extensions.into_iter() }.map(|entry| Offer::new(entry, name, location)),
        );
        let singles = [
            (Kind::Theme, registry.themes),
            (Kind::Wallpaper, registry.wallpapers),
            (Kind::TilingLayout, registry.tiling_layouts),
        ];
        for (kind, entries) in singles {
            let made = entries.into_iter();
            offers.extend(made.map(|entry| Offer::single(kind, entry, name, location)));
        }
        offers.extend(
            { registry.icon_packs.into_iter() }
                .map(|entry| Offer::icon_pack(entry, name, location)),
        );
        for offer in offers {
            match offer {
                Ok(offer) => {
                    let same = |known: &Offer| known.kind() == offer.kind() && known.id == offer.id;
                    if !self.offers.iter().any(same) {
                        self.offers.push(offer);
                    }
                }
                Err(error) => self.problems.push(format!("{location}: {error:#}")),
            }
        }
        let mut links = Vec::new();
        for link in registry.registries {
            match location.join(&link.url) {
                Ok(link) => links.push(link),
                Err(error) => self.problems.push(format!("{location}: {error:#}")),
            }
        }
        Ok(links)
    }
}

/// Where an installed offer came from, and the files Tessera wrote for
/// it where it is a directory.
#[derive(Debug, Deserialize, Serialize)]
struct Receipt {
    version: String,
    registry: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<String>,
}

/// What a package holds.
#[derive(Debug)]
enum Content {
    Extension {
        manifest: String,
        wasm: Vec<u8>,
    },
    /// One file, which ends in `extension`.
    File {
        extension: String,
        content: Vec<u8>,
    },
    IconPack {
        manifest: String,
        files: Vec<(String, Vec<u8>)>,
    },
}

/// A file complete before it takes the place of the one at `file`.
fn replace(file: &Path, content: &[u8]) -> anyhow::Result<()> {
    let mut part = file.as_os_str().to_owned();
    part.push(".part");
    fs::write(&part, content)
        .and_then(|()| fs::rename(&part, file))
        .with_context(|| format!("cannot write {}", file.display()))
}

/// A downloaded offer that matches what its registry lists.
#[derive(Debug)]
pub struct Package {
    id: String,
    kind: Kind,
    receipt: Receipt,
    /// What an extension asks to be allowed, one wish a line.
    pub wishes: Vec<String>,
    /// The widgets of an extension as layouts know them.
    pub widgets: Vec<String>,
    /// Where its registry suggests a widget of an extension is put.
    pub place: Option<Placement>,
    content: Content,
}

impl Package {
    /// A package of an extension that asks for `wishes` and holds
    /// nothing.
    #[cfg(test)]
    pub fn asking(wishes: Vec<String>) -> Self {
        Self {
            id: String::new(),
            kind: Kind::Extension,
            receipt: Receipt {
                version: String::new(),
                registry: String::new(),
                files: Vec::new(),
            },
            wishes,
            widgets: Vec::new(),
            place: None,
            content: Content::Extension {
                manifest: String::new(),
                wasm: Vec::new(),
            },
        }
    }

    /// The widget of an extension its registry suggests placing, and
    /// where.
    pub fn suggested_widget(&self) -> Option<(&str, &Placement)> {
        let place = self.place.as_ref()?;
        let widget = match &place.widget {
            Some(name) => self
                .widgets
                .iter()
                .find(|known| known.ends_with(&format!(".{name}")))?,
            None => self.widgets.first()?,
        };
        Some((widget, place))
    }

    /// Puts the package into `directory`, that of its kind, in place of
    /// an earlier version from a registry. What someone put there by hand
    /// is not replaced. Answers where it went: the directory of an
    /// extension or an icon pack, or the file of the others.
    pub fn install(&self, directory: &Path) -> anyhow::Result<PathBuf> {
        let receipt = toml::to_string(&self.receipt)?;
        match &self.content {
            Content::File { extension, content } => {
                let file = directory.join(format!("{}.{extension}", self.id));
                let proof = file.with_extension(THEME_RECEIPT_EXTENSION);
                if file.exists() && !proof.exists() {
                    bail!(
                        "{} exists and was not installed from a registry",
                        file.display()
                    );
                }
                fs::create_dir_all(directory)
                    .with_context(|| format!("cannot create {}", directory.display()))?;
                // A wallpaper of another kind of picture than before
                // leaves the earlier one.
                for earlier in Installed::singles(directory, self.kind) {
                    if earlier.id == self.id && earlier.place != file && earlier.version.is_some() {
                        earlier.remove()?;
                    }
                }
                replace(&proof, receipt.as_bytes())?;
                replace(&file, content)?;
                Ok(file)
            }
            Content::Extension { manifest, wasm } => {
                let directory = self.own_directory(directory)?;
                let files: [(&str, &[u8]); 3] = [
                    (WASM_FILE, wasm),
                    (MANIFEST_FILE, manifest.as_bytes()),
                    (RECEIPT_FILE, receipt.as_bytes()),
                ];
                for (name, content) in files {
                    replace(&directory.join(name), content)?;
                }
                Ok(directory)
            }
            Content::IconPack { manifest, files } => {
                let directory = self.own_directory(directory)?;
                // What the version before had and this one has not goes.
                let earlier = read_receipt(&directory.join(RECEIPT_FILE));
                let kept = |path: &String| files.iter().any(|(known, _)| known == path);
                for gone in earlier.iter().flat_map(|receipt| &receipt.files) {
                    if !kept(gone) && check_picture_name(gone).is_ok() {
                        delete(&directory.join(gone))?;
                    }
                }
                for (path, content) in files {
                    let file = directory.join(path);
                    if let Some(parent) = file.parent() {
                        fs::create_dir_all(parent)
                            .with_context(|| format!("cannot create {}", parent.display()))?;
                    }
                    replace(&file, content)?;
                }
                replace(
                    &directory.join(icon_packs::MANIFEST_FILE),
                    manifest.as_bytes(),
                )?;
                let receipt = Receipt {
                    files: files.iter().map(|(path, _)| path.clone()).collect(),
                    version: self.receipt.version.clone(),
                    registry: self.receipt.registry.clone(),
                };
                replace(
                    &directory.join(RECEIPT_FILE),
                    toml::to_string(&receipt)?.as_bytes(),
                )?;
                Ok(directory)
            }
        }
    }

    /// The directory of the package within `directory`, made where it is
    /// missing; one that was not installed from a registry is refused.
    fn own_directory(&self, directory: &Path) -> anyhow::Result<PathBuf> {
        let directory = directory.join(&self.id);
        if directory.exists() && !directory.join(RECEIPT_FILE).exists() {
            bail!(
                "{} exists and was not installed from a registry",
                directory.display()
            );
        }
        fs::create_dir_all(&directory)
            .with_context(|| format!("cannot create {}", directory.display()))?;
        Ok(directory)
    }
}

fn read_receipt(file: &Path) -> Option<Receipt> {
    let text = fs::read_to_string(file).ok()?;
    toml::from_str(&text).ok()
}

/// Deletes `file`; one that is not there is no failure.
fn delete(file: &Path) -> anyhow::Result<()> {
    match fs::remove_file(file) {
        Err(error) if error.kind() != ErrorKind::NotFound => {
            Err(error).context(format!("cannot delete {}", file.display()))
        }
        _ => Ok(()),
    }
}

/// Something installed: an extension in the extensions directory, or a
/// theme, an icon pack, a wallpaper or a tiling layout in the directory
/// of its kind.
#[derive(Debug)]
pub struct Installed {
    pub id: String,
    /// The widgets of an extension as layouts know them.
    pub widgets: Vec<String>,
    /// The version a registry gave it; `None` if it was put there by hand.
    pub version: Option<String>,
    /// The directory of an extension or an icon pack, or the file of the
    /// others.
    place: PathBuf,
    kind: Kind,
    /// What an extension is permitted.
    permissions: Option<Permissions>,
}

impl Installed {
    /// What of `kind` is in `directory`, the directory of that kind.
    pub fn of(kind: Kind, directory: &Path) -> Vec<Self> {
        match kind {
            Kind::Extension => Self::list(directory),
            Kind::IconPack => Self::icon_packs(directory),
            Kind::Theme | Kind::Wallpaper | Kind::TilingLayout => Self::singles(directory, kind),
        }
    }

    /// The extensions in the directory `extensions` that have a usable
    /// manifest.
    pub fn list(extensions: &Path) -> Vec<Self> {
        let entries = fs::read_dir(extensions).into_iter().flatten().flatten();
        let installed = entries.filter_map(|entry| {
            let directory = entry.path();
            let manifest = fs::read_to_string(directory.join(MANIFEST_FILE)).ok()?;
            let manifest = Manifest::parse(&manifest).ok()?;
            let receipt = read_receipt(&directory.join(RECEIPT_FILE));
            Some(Self {
                widgets: manifest
                    .widgets
                    .iter()
                    .map(|widget| manifest.qualified(&widget.name))
                    .collect(),
                id: manifest.id,
                version: receipt.map(|receipt| receipt.version),
                place: directory,
                kind: Kind::Extension,
                permissions: Some(manifest.permissions),
            })
        });
        installed.collect()
    }

    /// The icon packs in the directory `packs` that have a usable
    /// manifest, by the names of their directories.
    fn icon_packs(packs: &Path) -> Vec<Self> {
        let entries = fs::read_dir(packs).into_iter().flatten().flatten();
        let installed = entries.filter_map(|entry| {
            let directory = entry.path();
            let manifest = fs::read_to_string(directory.join(icon_packs::MANIFEST_FILE)).ok()?;
            PackManifest::parse(&manifest).ok()?;
            let receipt = read_receipt(&directory.join(RECEIPT_FILE));
            Some(Self {
                id: directory.file_name()?.to_str()?.to_owned(),
                widgets: Vec::new(),
                version: receipt.map(|receipt| receipt.version),
                place: directory,
                kind: Kind::IconPack,
                permissions: None,
            })
        });
        installed.collect()
    }

    /// The files of `kind` in `directory`, each offer that is one file.
    fn singles(directory: &Path, kind: Kind) -> Vec<Self> {
        let fits = |extension: &str| match kind.file_extension() {
            Some(own) => extension == own,
            None => WALLPAPER_EXTENSIONS.contains(&extension),
        };
        let entries = fs::read_dir(directory).into_iter().flatten().flatten();
        let installed = entries.filter_map(|entry| {
            let file = entry.path();
            let extension = file.extension()?.to_str()?.to_ascii_lowercase();
            if !fits(&extension) {
                return None;
            }
            let receipt = read_receipt(&file.with_extension(THEME_RECEIPT_EXTENSION));
            Some(Self {
                id: file.file_stem()?.to_str()?.to_owned(),
                widgets: Vec::new(),
                version: receipt.map(|receipt| receipt.version),
                place: file,
                kind,
                permissions: None,
            })
        });
        installed.collect()
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// What an installed extension is permitted, as
    /// [`Package::wishes`] says it.
    pub fn wishes(&self, language: Language) -> Vec<String> {
        { self.permissions.as_ref() }
            .map(|permissions| wishes(permissions, language))
            .unwrap_or_default()
    }

    /// The directory of an extension or an icon pack, or the file of the
    /// others.
    pub fn place(&self) -> &Path {
        &self.place
    }

    /// Deletes what was installed, and what an extension remembered.
    /// Only what came from a registry is deleted, and only the files
    /// Tessera put there.
    pub fn remove(&self) -> anyhow::Result<()> {
        if self.version.is_none() {
            bail!("`{}` was not installed from a registry", self.id);
        }
        match self.kind {
            Kind::Theme | Kind::Wallpaper | Kind::TilingLayout => {
                delete(&self.place.with_extension(THEME_RECEIPT_EXTENSION))?;
                delete(&self.place)
            }
            Kind::Extension => {
                for name in [WASM_FILE, MANIFEST_FILE, STATE_FILE, RECEIPT_FILE] {
                    delete(&self.place.join(name))?;
                }
                fs::remove_dir(&self.place)
                    .with_context(|| format!("cannot delete {}", self.place.display()))
            }
            Kind::IconPack => {
                let receipt = read_receipt(&self.place.join(RECEIPT_FILE));
                let mut own: Vec<PathBuf> =
                    { receipt.into_iter().flat_map(|receipt| receipt.files) }
                        .filter(|file| check_picture_name(file).is_ok())
                        .map(|file| self.place.join(file))
                        .collect();
                own.extend(
                    [icon_packs::MANIFEST_FILE, RECEIPT_FILE].map(|name| self.place.join(name)),
                );
                let (files, mut directories) = tree(&self.place);
                if let Some(other) = files.iter().find(|file| !own.contains(file)) {
                    bail!(
                        "{} holds {}, which was not installed from a registry",
                        self.place.display(),
                        other.display()
                    );
                }
                for file in &files {
                    delete(file)?;
                }
                // The deepest first, so that each is empty when its turn
                // comes.
                directories
                    .sort_by_key(|directory| std::cmp::Reverse(directory.components().count()));
                for directory in directories.iter().chain([&self.place]) {
                    fs::remove_dir(directory)
                        .with_context(|| format!("cannot delete {}", directory.display()))?;
                }
                Ok(())
            }
        }
    }
}

/// The files and the directories below `root`.
fn tree(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut files, mut directories) = (Vec::new(), Vec::new());
    let mut waiting = vec![root.to_owned()];
    while let Some(directory) = waiting.pop() {
        for entry in fs::read_dir(&directory).into_iter().flatten().flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => {
                    directories.push(path.clone());
                    waiting.push(path);
                }
                _ => files.push(path),
            }
        }
    }
    (files, directories)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "id = \"lights\"\n[[widget]]\nname = \"button\"\n[permissions]\nnetwork = [\"example.org\"]\n";
    const WASM: &[u8] = b"not really WebAssembly";

    /// An empty directory for one test.
    fn scratch(test: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("tessera-test-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn checksum(content: &[u8]) -> String {
        Sha256::digest(content)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Writes a registry named `name` offering the extension `lights`,
    /// followed by `rest`, and answers where it is.
    fn registry(directory: &Path, name: &str, rest: &str) -> String {
        fs::create_dir_all(directory.join("lights")).unwrap();
        fs::write(directory.join("lights/extension.toml"), MANIFEST).unwrap();
        fs::write(directory.join("lights/extension.wasm"), WASM).unwrap();
        let text = format!(
            concat!(
                "name = \"{}\"\n[[extension]]\nid = \"lights\"\nname = \"Lights\"\n",
                "version = \"1.0.0\"\nmanifest = \"lights/extension.toml\"\n",
                "manifest_sha256 = \"{}\"\nwasm = \"lights/extension.wasm\"\n",
                "wasm_sha256 = \"{}\"\n{}"
            ),
            name,
            checksum(MANIFEST.as_bytes()),
            checksum(WASM),
            rest
        );
        let file = directory.join("registry.toml");
        fs::write(&file, text).unwrap();
        file.to_str().unwrap().to_owned()
    }

    #[test]
    fn a_registry_refers_to_addresses_and_to_files_below_it() {
        let web = Location::parse("https://example.org/a/registry.toml?x=1/2").unwrap();
        let joined = |reference| web.join(reference).map(|location| location.to_string());
        assert_eq!(
            joined("b/c.wasm").unwrap(),
            "https://example.org/a/b/c.wasm"
        );
        assert_eq!(
            joined("https://other.example/r.toml").unwrap(),
            "https://other.example/r.toml"
        );
        for outside in ["../c", "/c", "C:/c", "a\\b", "http://example.org/c", ""] {
            assert!(joined(outside).is_err(), "{outside}");
        }
        let bare = Location::parse("https://example.org").unwrap();
        assert_eq!(
            bare.join("r.toml").unwrap().to_string(),
            "https://example.org/r.toml"
        );
        assert!(Location::parse("http://example.org/r.toml").is_err());
        assert!(Location::parse("registry.toml").is_err());
    }

    #[test]
    fn registries_named_by_a_registry_are_read_once() {
        let root = scratch("nested");
        let inner = root.join("inner");
        // The inner registry offers an extension with a bad name and names
        // a registry outside its directory.
        registry(
            &inner,
            "Inner",
            concat!(
                "[[extension]]\nid = \"Bad Name\"\nname = \"x\"\nversion = \"1\"\n",
                "manifest = \"m\"\nmanifest_sha256 = \"0\"\nwasm = \"w\"\nwasm_sha256 = \"0\"\n",
                "[[registry]]\nurl = \"../registry.toml\"\n",
            ),
        );
        let outer = registry(
            &root,
            "Outer",
            "[[registry]]\nurl = \"inner/registry.toml\"\n[[registry]]\nurl = \"missing.toml\"\n",
        );
        let catalog = Catalog::browse(&[outer.clone(), outer]);
        let [offer] = &catalog.offers[..] else {
            panic!("{:?}", catalog.offers);
        };
        assert_eq!(
            (offer.id.as_str(), offer.registry.as_str()),
            ("lights", "Outer")
        );
        // The bad extension, the path out of the inner registry and the
        // missing registry.
        assert_eq!(catalog.problems.len(), 3, "{:?}", catalog.problems);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_extension_is_installed_updated_and_removed() {
        let root = scratch("install");
        let extensions = root.join("extensions");
        let catalog = Catalog::browse(&[registry(&root, "Local", "")]);
        assert!(catalog.problems.is_empty(), "{:?}", catalog.problems);
        let package = catalog.offers[0].fetch(Language::English).unwrap();
        assert_eq!(package.wishes, ["contact example.org"]);

        package.install(&extensions).unwrap();
        package.install(&extensions).unwrap();
        let [installed] = &Installed::list(&extensions)[..] else {
            panic!("one extension is installed");
        };
        assert_eq!(installed.id, "lights");
        assert_eq!(installed.widgets, ["lights.button"]);
        assert_eq!(installed.version.as_deref(), Some("1.0.0"));

        installed.remove().unwrap();
        assert!(!extensions.join("lights").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_theme_is_installed_and_removed() {
        const THEME: &str = "seed = \"#336699\"\n[bar]\nflat = true\n";
        let root = scratch("theme");
        let themes = root.join("themes");
        fs::create_dir_all(root.join("offered")).unwrap();
        fs::write(root.join("offered/night.toml"), THEME).unwrap();
        fs::write(root.join("offered/broken.toml"), "sead = 1\n").unwrap();
        let entry = |id: &str, content: &str| {
            format!(
                concat!(
                    "[[theme]]\nid = \"{id}\"\nname = \"{id}\"\nversion = \"2\"\n",
                    "file = \"offered/{id}.toml\"\nsha256 = \"{sum}\"\n"
                ),
                id = id,
                sum = checksum(content.as_bytes())
            )
        };
        let rest = entry("night", THEME) + &entry("broken", "sead = 1\n");
        let catalog = Catalog::browse(&[registry(&root, "Local", &rest)]);
        assert!(catalog.problems.is_empty(), "{:?}", catalog.problems);
        let kinds: Vec<_> = catalog.offers.iter().map(Offer::kind).collect();
        assert_eq!(kinds, [Kind::Extension, Kind::Theme, Kind::Theme]);

        // What is not a theme file is not installed.
        assert!(catalog.offers[2].fetch(Language::English).is_err());
        let package = catalog.offers[1].fetch(Language::English).unwrap();
        assert!(package.wishes.is_empty());
        package.install(&themes).unwrap();
        package.install(&themes).unwrap();
        assert_eq!(
            fs::read_to_string(themes.join("night.toml")).unwrap(),
            THEME
        );

        // A theme someone wrote stays, and so does its name.
        fs::write(themes.join("own.toml"), THEME).unwrap();
        let mut installed = Installed::of(Kind::Theme, &themes);
        installed.sort_by(|a, b| a.id.cmp(&b.id));
        let [night, own] = &installed[..] else {
            panic!("{installed:?}");
        };
        assert_eq!(
            (night.id.as_str(), night.version.as_deref()),
            ("night", Some("2"))
        );
        assert_eq!((own.id.as_str(), own.version.as_deref()), ("own", None));
        assert!(own.remove().is_err());
        night.remove().unwrap();
        let left: Vec<_> = fs::read_dir(&themes).unwrap().flatten().collect();
        assert_eq!(left.len(), 1, "{left:?}");

        fs::rename(themes.join("own.toml"), themes.join("night.toml")).unwrap();
        assert!(package.install(&themes).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_extension_put_there_by_hand_is_kept() {
        let root = scratch("by-hand");
        let extensions = root.join("extensions");
        fs::create_dir_all(extensions.join("lights")).unwrap();
        fs::write(extensions.join("lights").join(MANIFEST_FILE), MANIFEST).unwrap();
        let catalog = Catalog::browse(&[registry(&root, "Local", "")]);
        assert!(
            catalog.offers[0]
                .fetch(Language::English)
                .unwrap()
                .install(&extensions)
                .is_err()
        );
        let [installed] = &Installed::list(&extensions)[..] else {
            panic!("one extension is installed");
        };
        assert_eq!(installed.version, None);
        assert!(installed.remove().is_err());
        assert!(extensions.join("lights").join(MANIFEST_FILE).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_file_that_differs_from_the_listed_one_is_refused() {
        let root = scratch("changed");
        let catalog = Catalog::browse(&[registry(&root, "Local", "")]);
        fs::write(root.join("lights/extension.wasm"), b"something else").unwrap();
        let error = catalog.offers[0].fetch(Language::English).unwrap_err();
        assert!(
            format!("{error:#}").contains("checksums differ"),
            "{error:#}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// How many offers of `catalog` are other versions of what is
    /// installed from a registry.
    fn updates(catalog: &Catalog, installed: &[Installed]) -> usize {
        let offers = catalog.offers.iter();
        offers
            .filter(|offer| {
                installed.iter().any(|known| {
                    known.kind == offer.kind
                        && known.id == offer.id
                        && known
                            .version
                            .as_ref()
                            .is_some_and(|version| *version != offer.version)
                })
            })
            .count()
    }

    /// Writes the icon pack `line` with `pictures` (path and content) into
    /// `directory/packs/line`, and answers its registry entry.
    fn pack_entry(directory: &Path, version: &str, pictures: &[(&str, &[u8])]) -> String {
        let pack = directory.join("packs/line");
        let _ = fs::remove_dir_all(&pack);
        let mut manifest =
            format!("id = \"line\"\nname = \"Line\"\nversion = \"{version}\"\n[files]\n");
        let mut files = String::new();
        for (index, (path, content)) in pictures.iter().enumerate() {
            let file = pack.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, content).unwrap();
            if !path.ends_with("-mask.png") {
                manifest.push_str(&format!("k{index} = \"{path}\"\n"));
            }
            files.push_str(&format!(
                "{{ path = \"{path}\", sha256 = \"{}\" }}, ",
                checksum(content)
            ));
        }
        fs::write(pack.join("icon-pack.toml"), &manifest).unwrap();
        format!(
            concat!(
                "[[icon_pack]]\nid = \"line\"\nname = \"Line\"\nversion = \"{}\"\n",
                "manifest = \"packs/line/icon-pack.toml\"\nmanifest_sha256 = \"{}\"\n",
                "files = [{}]\n"
            ),
            version,
            checksum(manifest.as_bytes()),
            files
        )
    }

    #[test]
    fn an_icon_pack_is_installed_updated_and_removed() {
        let root = scratch("icon-pack");
        let packs = root.join("icon-packs");
        let first = pack_entry(
            &root,
            "1",
            &[
                ("apps/a.png", b"a"),
                ("apps/a-mask.png", b"m"),
                ("b.svg", b"b"),
            ],
        );
        let catalog = Catalog::browse(&[registry(&root, "Local", &first)]);
        assert!(catalog.problems.is_empty(), "{:?}", catalog.problems);
        let offer = &catalog.offers[1];
        assert_eq!(offer.kind(), Kind::IconPack);
        let package = offer.fetch(Language::English).unwrap();
        assert!(package.wishes.is_empty());
        assert_eq!(package.install(&packs).unwrap(), packs.join("line"));
        assert!(packs.join("line/apps/a-mask.png").exists());
        let [installed] = &Installed::of(Kind::IconPack, &packs)[..] else {
            panic!("one icon pack is installed");
        };
        assert_eq!(
            (installed.id.as_str(), installed.version.as_deref()),
            ("line", Some("1"))
        );

        // The next version has no mask: the one before is deleted.
        let second = pack_entry(&root, "2", &[("apps/a.png", b"a2"), ("b.svg", b"b")]);
        let catalog = Catalog::browse(&[registry(&root, "Local", &second)]);
        assert_eq!(updates(&catalog, &Installed::of(Kind::IconPack, &packs)), 1);
        let package = catalog.offers[1].fetch(Language::English).unwrap();
        package.install(&packs).unwrap();
        assert!(!packs.join("line/apps/a-mask.png").exists());
        assert_eq!(fs::read(packs.join("line/apps/a.png")).unwrap(), b"a2");
        let installed = Installed::of(Kind::IconPack, &packs);
        assert_eq!(updates(&catalog, &installed), 0);

        // A file of the user's keeps the pack whole.
        fs::write(packs.join("line/apps/mine.png"), b"mine").unwrap();
        assert!(installed[0].remove().is_err());
        assert!(packs.join("line/b.svg").exists());
        fs::remove_file(packs.join("line/apps/mine.png")).unwrap();
        installed[0].remove().unwrap();
        assert!(!packs.join("line").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_icon_pack_that_names_an_unlisted_picture_is_refused() {
        let root = scratch("icon-pack-unlisted");
        let entry = pack_entry(&root, "1", &[("a.png", b"a")]);
        let manifest = root.join("packs/line/icon-pack.toml");
        let before = fs::read_to_string(&manifest).unwrap();
        let text = before.clone() + "other = \"b.png\"\n";
        fs::write(&manifest, &text).unwrap();
        let entry = entry.replace(&checksum(before.as_bytes()), &checksum(text.as_bytes()));
        let catalog = Catalog::browse(&[registry(&root, "Local", &entry)]);
        let error = catalog.offers[1].fetch(Language::English).unwrap_err();
        assert!(format!("{error:#}").contains("b.png"), "{error:#}");
        let outside = format!(
            concat!(
                "[[icon_pack]]\nid = \"x\"\nname = \"x\"\nversion = \"1\"\n",
                "manifest = \"m.toml\"\nmanifest_sha256 = \"{sum}\"\n",
                "files = [{{ path = \"../a.png\", sha256 = \"{sum}\" }}]\n"
            ),
            sum = "0".repeat(64)
        );
        let catalog = Catalog::browse(&[registry(&root, "Local", &outside)]);
        assert_eq!(catalog.offers.len(), 1);
        assert_eq!(catalog.problems.len(), 1, "{:?}", catalog.problems);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wallpapers_and_tiling_layouts_are_single_files() {
        let root = scratch("singles");
        fs::create_dir_all(root.join("offered")).unwrap();
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let layout = b"[[container]]\nkind = \"leaf\"\n";
        fs::write(root.join("offered/hills.png"), &png).unwrap();
        fs::write(root.join("offered/notes.txt"), b"text").unwrap();
        fs::write(root.join("offered/wide.toml"), layout).unwrap();
        let entry = |table: &str, id: &str, file: &str, content: &[u8]| {
            format!(
                "[[{table}]]\nid = \"{id}\"\nname = \"{id}\"\nversion = \"1\"\nfile = \"offered/{file}\"\nsha256 = \"{}\"\n",
                checksum(content)
            )
        };
        let rest = entry("wallpaper", "hills", "hills.png", &png)
            + &entry("wallpaper", "notes", "notes.txt", b"text")
            + &entry("tiling_layout", "wide", "wide.toml", layout);
        let catalog = Catalog::browse(&[registry(&root, "Local", &rest)]);
        let kinds: Vec<_> = catalog.offers.iter().map(Offer::kind).collect();
        assert_eq!(
            kinds,
            [Kind::Extension, Kind::Wallpaper, Kind::TilingLayout]
        );
        assert_eq!(catalog.problems.len(), 1, "{:?}", catalog.problems);

        let wallpapers = root.join("wallpapers");
        let file = { catalog.offers[1].fetch(Language::English).unwrap() }
            .install(&wallpapers)
            .unwrap();
        assert_eq!(file, wallpapers.join("hills.png"));
        let layouts = root.join("layouts");
        let file = { catalog.offers[2].fetch(Language::English).unwrap() }
            .install(&layouts)
            .unwrap();
        assert_eq!(file, layouts.join("wide.toml"));
        let [wallpaper] = &Installed::of(Kind::Wallpaper, &wallpapers)[..] else {
            panic!("one wallpaper is installed");
        };
        assert_eq!(wallpaper.place(), wallpapers.join("hills.png"));
        wallpaper.remove().unwrap();
        assert_eq!(fs::read_dir(&wallpapers).unwrap().count(), 0);
        assert_eq!(Installed::of(Kind::TilingLayout, &layouts).len(), 1);

        // What is not the picture it says it is is not installed.
        fs::write(root.join("offered/hills.png"), b"not a picture").unwrap();
        let rest = entry("wallpaper", "hills", "hills.png", b"not a picture");
        let catalog = Catalog::browse(&[registry(&root, "Local", &rest)]);
        assert!(catalog.offers[1].fetch(Language::English).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_registry_suggests_where_a_widget_goes() {
        let root = scratch("place");
        let file = registry(&root, "Local", "");
        let text = fs::read_to_string(&file).unwrap();
        let placed = |place: &str| {
            fs::write(
                &file,
                text.replace(
                    "name = \"Lights\"\n",
                    &format!("name = \"Lights\"\n{place}\n"),
                ),
            )
            .unwrap();
            let catalog = Catalog::browse(std::slice::from_ref(&file));
            let package = catalog.offers[0].fetch(Language::English).unwrap();
            package
                .suggested_widget()
                .map(|(widget, place)| (widget.to_owned(), place.slot(), place.in_dock()))
        };
        assert_eq!(
            placed("place = { surface = \"dock\", slot = \"end\" }"),
            Some(("lights.button".to_owned(), Some(Slot::End), true))
        );
        assert_eq!(
            placed("place = { widget = \"button\", slot = \"start\" }"),
            Some(("lights.button".to_owned(), Some(Slot::Start), false))
        );
        assert_eq!(
            placed("place = { widget = \"other\", slot = \"start\" }"),
            None
        );
        assert_eq!(placed("place = { slot = \"middle\" }"), None);
        assert_eq!(placed("place = { surface = \"bar\" }"), None);
        assert_eq!(placed("place = { slot = \"end\", size = \"large\" }"), None);
        assert_eq!(placed("place = \"end\""), None);
        assert_eq!(placed(""), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn kinds_are_named_one_and_many() {
        for kind in Kind::ALL {
            assert_eq!(Kind::named(kind.name()), Some(kind));
            assert_eq!(Kind::named(kind.singular()), Some(kind));
        }
        assert_eq!(Kind::named("icon-pack"), Some(Kind::IconPack));
        assert_eq!(Kind::named("plugins"), None);
    }

    #[test]
    fn the_registry_of_this_repository_lists_its_files() {
        let registry = concat!(env!("CARGO_MANIFEST_DIR"), "/../../registry/registry.toml");
        let registry = Path::new(registry).canonicalize().unwrap();
        let catalog = Catalog::browse(&[registry.to_str().unwrap().to_owned()]);
        assert!(catalog.problems.is_empty(), "{:?}", catalog.problems);
        assert!(!catalog.offers.is_empty());
        for offer in &catalog.offers {
            offer.fetch(Language::English).unwrap();
        }
    }
}
