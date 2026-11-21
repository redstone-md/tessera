// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Registries: lists of extensions that can be installed. A registry can
//! name further registries, whose extensions are offered along with its
//! own.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{DeviceKind, MANIFEST_FILE, Manifest, STATE_FILE, WASM_FILE, http, is_identifier};

/// The registry that is read when the configuration names none.
pub const OFFICIAL_REGISTRY: &str =
    "https://raw.githubusercontent.com/redstone-md/tessera/main/registry/registry.toml";

/// Written next to an extension that was installed from a registry.
const RECEIPT_FILE: &str = "installed.toml";

/// The most that is read of a registry, a manifest and a WebAssembly file.
const REGISTRY_LIMIT: u64 = 1 << 20;
const MANIFEST_LIMIT: u64 = 64 << 10;
const WASM_LIMIT: u64 = 32 << 20;

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
    #[serde(default, rename = "registry")]
    registries: Vec<Link>,
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
}

#[derive(Deserialize)]
struct Link {
    url: String,
}

/// A file of an extension and the SHA-256 checksum its registry gives.
#[derive(Clone, Debug)]
struct Download {
    location: Location,
    checksum: String,
}

impl Download {
    fn new(registry: &Location, reference: &str, checksum: &str) -> anyhow::Result<Self> {
        let checksum = checksum.to_ascii_lowercase();
        if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("`{checksum}` is not a SHA-256 checksum");
        }
        Ok(Self {
            location: registry.join(reference)?,
            checksum,
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

/// An extension a registry offers.
#[derive(Clone, Debug)]
pub struct Offer {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    /// The name of the registry that offers it.
    pub registry: String,
    manifest: Download,
    wasm: Download,
}

impl Offer {
    fn new(entry: Entry, registry: &str, location: &Location) -> anyhow::Result<Self> {
        if !is_identifier(&entry.id) {
            bail!("`{}` is not a name made of a-z, 0-9 and `-`", entry.id);
        }
        let download = |reference: &str, checksum: &str| {
            Download::new(location, reference, checksum)
                .with_context(|| format!("the extension `{}` is left out", entry.id))
        };
        Ok(Self {
            manifest: download(&entry.manifest, &entry.manifest_sha256)?,
            wasm: download(&entry.wasm, &entry.wasm_sha256)?,
            id: entry.id,
            name: entry.name,
            description: entry.description,
            version: entry.version,
            registry: registry.to_owned(),
        })
    }

    /// Downloads the extension and checks it against what the registry
    /// lists. Nothing is installed yet.
    pub fn fetch(&self) -> anyhow::Result<Package> {
        let manifest = String::from_utf8(self.manifest.fetch(MANIFEST_LIMIT)?)
            .context("the manifest is not text")?;
        let wasm = self.wasm.fetch(WASM_LIMIT)?;
        let parsed = Manifest::parse(&manifest).context("the manifest is invalid")?;
        if parsed.id != self.id {
            bail!(
                "the registry offers `{}` but the manifest is that of `{}`",
                self.id,
                parsed.id
            );
        }
        Ok(Package {
            id: self.id.clone(),
            receipt: Receipt {
                version: self.version.clone(),
                registry: self.registry.clone(),
            },
            network: parsed.permissions.network().to_vec(),
            files: parsed.permissions.files().to_vec(),
            devices: parsed.permissions.devices().to_vec(),
            manifest,
            wasm,
        })
    }
}

/// What the registries offer, and what went wrong while reading them.
#[derive(Debug, Default)]
pub struct Catalog {
    pub offers: Vec<Offer>,
    pub problems: Vec<String>,
}

impl Catalog {
    /// Reads the registries at `roots` and those they name. An extension
    /// that several registries offer is taken from the one read first:
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
        for entry in registry.extensions {
            match Offer::new(entry, &registry.name, location) {
                Ok(offer) if self.offers.iter().any(|known| known.id == offer.id) => {}
                Ok(offer) => self.offers.push(offer),
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

/// Where an installed extension came from.
#[derive(Debug, Deserialize, Serialize)]
struct Receipt {
    version: String,
    registry: String,
}

/// A downloaded extension that matches what its registry lists.
#[derive(Debug)]
pub struct Package {
    id: String,
    receipt: Receipt,
    /// The hosts the extension asks to contact.
    pub network: Vec<String>,
    /// The files the extension asks to read.
    pub files: Vec<String>,
    /// The kinds of devices the extension asks to be told of.
    pub devices: Vec<DeviceKind>,
    manifest: String,
    wasm: Vec<u8>,
}

impl Package {
    /// A package that asks for `network`, `files` and `devices` and holds
    /// nothing.
    #[cfg(test)]
    pub fn asking(network: Vec<String>, files: Vec<String>, devices: Vec<DeviceKind>) -> Self {
        Self {
            id: String::new(),
            receipt: Receipt {
                version: String::new(),
                registry: String::new(),
            },
            network,
            files,
            devices,
            manifest: String::new(),
            wasm: Vec::new(),
        }
    }

    /// Puts the extension into the directory `extensions`, in place of an
    /// earlier version from a registry. An extension that someone put
    /// there by hand is not replaced.
    pub fn install(&self, extensions: &Path) -> anyhow::Result<()> {
        let directory = extensions.join(&self.id);
        if directory.exists() && !directory.join(RECEIPT_FILE).exists() {
            bail!(
                "{} exists and was not installed from a registry",
                directory.display()
            );
        }
        fs::create_dir_all(&directory)
            .with_context(|| format!("cannot create {}", directory.display()))?;
        let receipt = toml::to_string(&self.receipt)?;
        let files: [(&str, &[u8]); 3] = [
            (WASM_FILE, &self.wasm),
            (MANIFEST_FILE, self.manifest.as_bytes()),
            (RECEIPT_FILE, receipt.as_bytes()),
        ];
        for (name, content) in files {
            // A file is complete before it takes the place of the old one.
            let (file, part) = (directory.join(name), directory.join(format!("{name}.part")));
            fs::write(&part, content)
                .and_then(|()| fs::rename(&part, &file))
                .with_context(|| format!("cannot write {}", file.display()))?;
        }
        Ok(())
    }
}

/// An extension in the extensions directory.
#[derive(Debug)]
pub struct Installed {
    pub id: String,
    /// Its widgets as layouts know them.
    pub widgets: Vec<String>,
    /// The version a registry gave it; `None` if it was put there by hand.
    pub version: Option<String>,
    directory: PathBuf,
}

impl Installed {
    /// The extensions in the directory `extensions` that have a usable
    /// manifest.
    pub fn list(extensions: &Path) -> Vec<Self> {
        let entries = fs::read_dir(extensions).into_iter().flatten().flatten();
        let installed = entries.filter_map(|entry| {
            let directory = entry.path();
            let manifest = fs::read_to_string(directory.join(MANIFEST_FILE)).ok()?;
            let manifest = Manifest::parse(&manifest).ok()?;
            let receipt = fs::read_to_string(directory.join(RECEIPT_FILE)).ok();
            let receipt = receipt.and_then(|text| toml::from_str::<Receipt>(&text).ok());
            Some(Self {
                widgets: manifest
                    .widgets
                    .iter()
                    .map(|widget| manifest.qualified(&widget.name))
                    .collect(),
                id: manifest.id,
                version: receipt.map(|receipt| receipt.version),
                directory,
            })
        });
        installed.collect()
    }

    /// Deletes the extension and what it remembered. Only one that came
    /// from a registry is deleted, and only the files Tessera put there.
    pub fn remove(&self) -> anyhow::Result<()> {
        if self.version.is_none() {
            bail!("`{}` was not installed from a registry", self.id);
        }
        for name in [WASM_FILE, MANIFEST_FILE, STATE_FILE, RECEIPT_FILE] {
            match fs::remove_file(self.directory.join(name)) {
                Err(error) if error.kind() != ErrorKind::NotFound => {
                    return Err(error).context(format!("cannot delete {name}"));
                }
                _ => {}
            }
        }
        fs::remove_dir(&self.directory)
            .with_context(|| format!("cannot delete {}", self.directory.display()))
    }
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
        let package = catalog.offers[0].fetch().unwrap();
        assert_eq!(package.network, ["example.org"]);
        assert!(package.files.is_empty());

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
    fn an_extension_put_there_by_hand_is_kept() {
        let root = scratch("by-hand");
        let extensions = root.join("extensions");
        fs::create_dir_all(extensions.join("lights")).unwrap();
        fs::write(extensions.join("lights").join(MANIFEST_FILE), MANIFEST).unwrap();
        let catalog = Catalog::browse(&[registry(&root, "Local", "")]);
        assert!(
            catalog.offers[0]
                .fetch()
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
        let error = catalog.offers[0].fetch().unwrap_err();
        assert!(
            format!("{error:#}").contains("checksums differ"),
            "{error:#}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_registry_of_this_repository_lists_its_files() {
        let registry = concat!(env!("CARGO_MANIFEST_DIR"), "/../../registry/registry.toml");
        let registry = Path::new(registry).canonicalize().unwrap();
        let catalog = Catalog::browse(&[registry.to_str().unwrap().to_owned()]);
        assert!(catalog.problems.is_empty(), "{:?}", catalog.problems);
        assert!(!catalog.offers.is_empty());
        for offer in &catalog.offers {
            offer.fetch().unwrap();
        }
    }
}
