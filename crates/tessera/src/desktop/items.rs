// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the desktop shows: the files and folders of the user's desktop
//! folder and of the one all users share, merged (an item of the user's
//! hides one of the same name of everyone's), and the items of the Shell
//! the user has Windows show there, named and hidden as Explorer names
//! and hides them.

use std::cmp::Ordering;
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::platform::{
    Attributes, DesktopFolders, DesktopOptions, ExplorerSettings, NameRules, VirtualKind,
    shell_names, virtual_items,
};

/// The file that gives a folder its look, which Explorer never shows on
/// the desktop unless it shows the protected files of the system.
const DESKTOP_INI: &str = "desktop.ini";

/// What the icons are sorted by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortBy {
    #[default]
    Name,
    Size,
    Type,
    Date,
}

impl SortBy {
    pub const ALL: [Self; 4] = [Self::Name, Self::Size, Self::Type, Self::Date];

    pub fn name(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Size => "size",
            Self::Type => "type",
            Self::Date => "date",
        }
    }

    /// What the menu calls it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size",
            Self::Type => "Type",
            Self::Date => "Modified",
        }
    }
}

/// What an icon stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKind {
    File {
        path: PathBuf,
        folder: bool,
        /// Whether it is a shortcut, over whose icon an arrow is drawn.
        link: bool,
        /// Whether it lies in the desktop folder all users share.
        public: bool,
    },
    /// An item of the Shell without a path of its own.
    Virtual(VirtualKind),
}

/// An icon of the desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopItem {
    /// What it is known by among the icons, and its place kept by: the
    /// file name, or the parsing name of an item of the Shell.
    pub key: String,
    /// The name as Explorer shows it.
    pub name: String,
    pub kind: ItemKind,
    /// In bytes; 0 for a folder and an item of the Shell.
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl DesktopItem {
    /// The file or folder it is, where it is one.
    pub fn path(&self) -> Option<&Path> {
        match &self.kind {
            ItemKind::File { path, .. } => Some(path),
            ItemKind::Virtual(_) => None,
        }
    }

    pub fn is_folder(&self) -> bool {
        matches!(self.kind, ItemKind::File { folder: true, .. })
    }

    pub fn is_link(&self) -> bool {
        matches!(self.kind, ItemKind::File { link: true, .. })
    }

    pub fn virtual_kind(&self) -> Option<VirtualKind> {
        match self.kind {
            ItemKind::Virtual(kind) => Some(kind),
            ItemKind::File { .. } => None,
        }
    }

    /// What the icon is sorted among the others by first: the items of the
    /// Shell in Explorer's order, then folders, then files.
    fn rank(&self) -> usize {
        match &self.kind {
            ItemKind::Virtual(kind) => VirtualKind::ALL
                .iter()
                .position(|other| other == kind)
                .unwrap_or(0),
            ItemKind::File { folder: true, .. } => VirtualKind::ALL.len(),
            ItemKind::File { .. } => VirtualKind::ALL.len() + 1,
        }
    }

    /// The extension of the file in small letters; empty for others.
    fn kind_key(&self) -> String {
        match &self.kind {
            ItemKind::File {
                path,
                folder: false,
                ..
            } => { path.extension() }
                .map(|extension| extension.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// The item of the file or folder at `path`, of which `metadata` was
    /// read, named `name`.
    fn of_file(path: PathBuf, metadata: &Metadata, name: String, public: bool) -> Self {
        let folder = metadata.is_dir();
        let key = { path.file_name() }
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let link = !folder && is_link_name(&key);
        Self {
            key,
            name,
            kind: ItemKind::File {
                path,
                folder,
                link,
                public,
            },
            size: if folder { 0 } else { metadata.len() },
            modified: metadata.modified().ok(),
        }
    }
}

/// Whether a file of this name is a shortcut: a `.lnk` or a `.url`.
fn is_link_name(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("lnk") || extension.eq_ignore_ascii_case("url")
    })
}

/// Whether Explorer shows a file of the desktop called `name` with
/// `attributes`, under the folder options `settings`.
fn shown(name: &str, attributes: Attributes, settings: ExplorerSettings) -> bool {
    if attributes.protected() || name.eq_ignore_ascii_case(DESKTOP_INI) {
        return settings.show_protected;
    }
    !attributes.hidden || settings.show_hidden
}

/// The keys of `items` in the order `by` sorts them: the items of the
/// Shell first, then folders, then files, each by `by` and then by name.
pub fn sorted_keys(items: &[DesktopItem], by: SortBy) -> Vec<String> {
    let mut sorted: Vec<&DesktopItem> = items.iter().collect();
    sorted.sort_by(|one, other| compare(one, other, by));
    sorted.into_iter().map(|item| item.key.clone()).collect()
}

fn compare(one: &DesktopItem, other: &DesktopItem, by: SortBy) -> Ordering {
    let name = || {
        let ordered = one.name.to_lowercase().cmp(&other.name.to_lowercase());
        ordered.then_with(|| one.key.cmp(&other.key))
    };
    let first = one.rank().cmp(&other.rank());
    let second = match by {
        SortBy::Name => Ordering::Equal,
        // The largest first, as Explorer sorts them.
        SortBy::Size => other.size.cmp(&one.size),
        SortBy::Type => one.kind_key().cmp(&other.kind_key()),
        // The newest first.
        SortBy::Date => other.modified.cmp(&one.modified),
    };
    first.then(second).then_with(name)
}

/// `user` and then those of `public` that the user has none of the same
/// name of, as Explorer merges the two folders.
pub fn merge(user: Vec<DesktopItem>, public: Vec<DesktopItem>) -> Vec<DesktopItem> {
    let mut merged = user;
    for item in public {
        let taken = { merged.iter() }.any(|kept| kept.key.eq_ignore_ascii_case(&item.key));
        if !taken {
            merged.push(item);
        }
    }
    merged
}

/// The first of `order`, after `after` and around, whose item's name
/// begins with `typed`, in any case: what typing on the desktop chooses.
pub fn typed_match(
    items: &[DesktopItem],
    order: &[String],
    typed: &str,
    after: Option<&str>,
) -> Option<String> {
    if typed.is_empty() || order.is_empty() {
        return None;
    }
    let typed = typed.to_lowercase();
    let start = after
        .and_then(|after| order.iter().position(|key| key == after))
        .map_or(0, |at| at + 1);
    (0..order.len())
        .map(|offset| &order[(start + offset) % order.len()])
        .find(|key| {
            { items.iter() }
                .find(|item| &item.key == *key)
                .is_some_and(|item| item.name.to_lowercase().starts_with(&typed))
        })
        .cloned()
}

/// The name a file called `file_name`, shown as `shown`, gets when the
/// user types `typed` over its name: the extension Explorer hid stays.
pub fn renamed(file_name: &str, shown: &str, typed: &str) -> String {
    let typed = typed.trim();
    match Path::new(file_name).extension() {
        Some(extension)
            if shown != file_name
                && Path::new(file_name)
                    .file_stem()
                    .is_some_and(|stem| stem == shown) =>
        {
            format!("{typed}.{}", extension.to_string_lossy())
        }
        _ => typed.to_owned(),
    }
}

/// What the field of a rename starts with: the name as it is shown where
/// that is the file name without the extension Explorer hides, and the
/// file name otherwise, as a name given by a `desktop.ini` is not the
/// name of the file.
pub fn rename_text(file_name: &str, shown: &str) -> String {
    let stem = Path::new(file_name).file_stem();
    match stem.is_some_and(|stem| stem == shown) {
        true => shown.to_owned(),
        false => file_name.to_owned(),
    }
}

/// The items of `folder`, shown as Explorer shows them under `settings`;
/// `None` where the folder cannot be read now, as a desktop on a network
/// share that is away. Blocks on the disk.
fn read_folder(
    folder: &Path,
    public: bool,
    settings: ExplorerSettings,
) -> Option<Vec<DesktopItem>> {
    let Ok(listing) = fs::read_dir(folder) else {
        report!("the desktop folder {} cannot be read", folder.display());
        return None;
    };
    let mut rules = NameRules::new(settings, true);
    let mut found = Vec::new();
    for entry in listing.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !shown(&name, Attributes::of(&metadata), settings) {
            continue;
        }
        let display = rules.shown(&name, metadata.is_dir());
        found.push(DesktopItem::of_file(
            entry.path(),
            &metadata,
            display,
            public,
        ));
    }
    // The Shell names them as Explorer does: in the language of the user
    // where a `desktop.ini` gives a name, without the extension of a
    // shortcut.
    let paths: Vec<PathBuf> = { found.iter() }
        .filter_map(|item| item.path().map(Path::to_owned))
        .collect();
    for (item, name) in found.iter_mut().zip(shell_names(&paths)) {
        if let Some(name) = name {
            item.name = name;
        }
    }
    Some(found)
}

/// What a reading of the desktop found.
pub struct DesktopReading {
    pub items: Vec<DesktopItem>,
    /// Whether every desktop folder was read: where one could not be,
    /// its items are missing, but they are not gone.
    pub complete: bool,
    /// What the user chose in Windows for the desktop when it was read.
    pub options: DesktopOptions,
}

/// Everything the desktop shows: the items of the Shell the user has
/// Windows show there, the user's files and folders and then those of
/// everyone's desktop. It reads the disk and asks the Shell: it is called
/// on a thread of the Shell, never on the thread of GPUI.
pub fn read_desktop(folders: &DesktopFolders) -> DesktopReading {
    let options = DesktopOptions::read();
    let settings = options.folders;
    let mut items: Vec<DesktopItem> = { virtual_items(&options).into_iter() }
        .filter(|item| item.shown)
        .map(|item| DesktopItem {
            key: item.kind.parsing_name(),
            name: item.name,
            kind: ItemKind::Virtual(item.kind),
            size: 0,
            modified: None,
        })
        .collect();
    let user = read_folder(&folders.user, false, settings);
    let public = { folders.public.as_deref() }.map(|public| read_folder(public, true, settings));
    let complete = user.is_some() && !matches!(public, Some(None));
    items.extend(merge(
        user.unwrap_or_default(),
        public.flatten().unwrap_or_default(),
    ));
    DesktopReading {
        items,
        complete,
        options,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn file(name: &str, size: u64, age: u64) -> DesktopItem {
        DesktopItem {
            key: name.to_owned(),
            name: name.to_owned(),
            kind: ItemKind::File {
                path: PathBuf::from(format!(r"C:\Users\a\Desktop\{name}")),
                folder: false,
                link: is_link_name(name),
                public: false,
            },
            size,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1000 - age)),
        }
    }

    fn folder(name: &str) -> DesktopItem {
        DesktopItem {
            kind: ItemKind::File {
                path: PathBuf::from(format!(r"C:\Users\a\Desktop\{name}")),
                folder: true,
                link: false,
                public: false,
            },
            size: 0,
            ..file(name, 0, 0)
        }
    }

    fn bin() -> DesktopItem {
        DesktopItem {
            key: VirtualKind::RecycleBin.parsing_name(),
            name: "Recycle Bin".to_owned(),
            kind: ItemKind::Virtual(VirtualKind::RecycleBin),
            size: 0,
            modified: None,
        }
    }

    fn computer() -> DesktopItem {
        DesktopItem {
            key: VirtualKind::Computer.parsing_name(),
            name: "This PC".to_owned(),
            kind: ItemKind::Virtual(VirtualKind::Computer),
            ..bin()
        }
    }

    #[test]
    fn the_items_of_the_shell_come_first_then_folders_then_files() {
        let items = vec![
            file("b.txt", 1, 1),
            folder("Zeta"),
            bin(),
            file("A.txt", 1, 1),
            computer(),
            folder("alpha"),
        ];
        let order = sorted_keys(&items, SortBy::Name);
        assert_eq!(
            order,
            [
                computer().key,
                bin().key,
                "alpha".to_owned(),
                "Zeta".to_owned(),
                "A.txt".to_owned(),
                "b.txt".to_owned()
            ]
        );
    }

    #[test]
    fn files_are_sorted_by_size_type_and_date_as_explorer_sorts_them() {
        let items = vec![
            file("small.txt", 1, 10),
            file("large.zip", 900, 300),
            file("middle.doc", 50, 1),
        ];
        assert_eq!(
            sorted_keys(&items, SortBy::Size),
            ["large.zip", "middle.doc", "small.txt"]
        );
        assert_eq!(
            sorted_keys(&items, SortBy::Type),
            ["middle.doc", "small.txt", "large.zip"]
        );
        assert_eq!(
            sorted_keys(&items, SortBy::Date),
            ["middle.doc", "small.txt", "large.zip"]
        );
    }

    #[test]
    fn an_item_of_the_user_hides_one_of_everyone_of_the_same_name() {
        let mut shared = file("Edge.lnk", 1, 1);
        shared.key = "EDGE.lnk".to_owned();
        let merged = merge(
            vec![file("Edge.lnk", 1, 1)],
            vec![shared, file("Zoom.lnk", 1, 1)],
        );
        let keys: Vec<_> = merged.iter().map(|item| item.key.as_str()).collect();
        assert_eq!(keys, ["Edge.lnk", "Zoom.lnk"]);
    }

    #[test]
    fn hidden_and_protected_files_follow_the_folder_options() {
        let plain = Attributes::default();
        let hidden = Attributes {
            hidden: true,
            ..plain
        };
        let protected = Attributes {
            hidden: true,
            system: true,
            ..plain
        };
        let defaults = ExplorerSettings::default();
        let everything = ExplorerSettings {
            show_hidden: true,
            show_protected: true,
            ..defaults
        };
        assert!(shown("a.txt", plain, defaults));
        assert!(!shown("a.txt", hidden, defaults));
        assert!(shown("a.txt", hidden, everything));
        assert!(!shown(
            "a.txt",
            protected,
            ExplorerSettings {
                show_hidden: true,
                ..defaults
            }
        ));
        assert!(shown("a.txt", protected, everything));
        // desktop.ini is hidden even where it is not marked so.
        assert!(!shown("Desktop.ini", plain, defaults));
        assert!(shown("desktop.ini", plain, everything));
    }

    #[test]
    fn typing_chooses_the_next_icon_whose_name_begins_so() {
        let items = vec![
            file("apple", 0, 0),
            file("Avocado", 0, 0),
            file("banana", 0, 0),
        ];
        let order = sorted_keys(&items, SortBy::Name);
        assert_eq!(
            typed_match(&items, &order, "a", None).as_deref(),
            Some("apple")
        );
        assert_eq!(
            typed_match(&items, &order, "a", Some("apple")).as_deref(),
            Some("Avocado")
        );
        assert_eq!(
            typed_match(&items, &order, "a", Some("Avocado")).as_deref(),
            Some("apple")
        );
        assert_eq!(
            typed_match(&items, &order, "BAN", None).as_deref(),
            Some("banana")
        );
        assert_eq!(typed_match(&items, &order, "cherry", None), None);
        assert_eq!(typed_match(&items, &order, "", None), None);
    }

    #[test]
    fn a_rename_keeps_the_extension_explorer_hid() {
        assert_eq!(renamed("Notes.lnk", "Notes", "Plans"), "Plans.lnk");
        assert_eq!(renamed("report.docx", "report", " draft "), "draft.docx");
        assert_eq!(
            renamed("report.docx", "report.docx", "draft.txt"),
            "draft.txt"
        );
        // A name a desktop.ini gives is not the name of the file.
        assert_eq!(renamed("Docs", "Документы", "Papers"), "Papers");
        assert_eq!(rename_text("Notes.lnk", "Notes"), "Notes");
        assert_eq!(rename_text("report.docx", "report.docx"), "report.docx");
        assert_eq!(rename_text("Docs", "Документы"), "Docs");
    }
}
