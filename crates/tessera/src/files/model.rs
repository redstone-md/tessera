// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the file manager knows apart from how it is drawn: where it is and
//! has been, what a folder holds, in which order and which of it is
//! chosen.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::platform::is_hidden;

/// Where a window of the file manager is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Location {
    /// The drives of the computer.
    Computer,
    Folder(PathBuf),
}

impl Location {
    /// The location one level up: the folder around a folder, and the
    /// computer around the root of a drive. The computer has none.
    pub fn up(&self) -> Option<Self> {
        match self {
            Self::Computer => None,
            Self::Folder(path) => Some(match path.parent() {
                Some(parent) if parent != path && !parent.as_os_str().is_empty() => {
                    Self::Folder(parent.to_owned())
                }
                _ => Self::Computer,
            }),
        }
    }

    pub fn folder(&self) -> Option<&Path> {
        match self {
            Self::Computer => None,
            Self::Folder(path) => Some(path),
        }
    }

    /// The location as a command names it, which `Files::named` reads
    /// back: `computer`, or the path of the folder.
    pub fn name(&self) -> String {
        match self {
            Self::Computer => "computer".to_owned(),
            Self::Folder(path) => path.display().to_string(),
        }
    }
}

/// The locations a window went through, to go back and forth among them
/// as a browser does.
#[derive(Debug)]
pub struct History {
    back: Vec<Location>,
    current: Location,
    forward: Vec<Location>,
}

/// How many locations are kept to go back to.
const HISTORY_KEPT: usize = 64;

impl History {
    pub fn new(start: Location) -> Self {
        Self {
            back: Vec::new(),
            current: start,
            forward: Vec::new(),
        }
    }

    pub fn current(&self) -> &Location {
        &self.current
    }

    /// Goes to `to`; where it is the location already, nothing changes.
    /// Says whether it went.
    pub fn go(&mut self, to: Location) -> bool {
        if to == self.current {
            return false;
        }
        self.back.push(std::mem::replace(&mut self.current, to));
        if self.back.len() > HISTORY_KEPT {
            self.back.remove(0);
        }
        self.forward.clear();
        true
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn back(&mut self) -> bool {
        let Some(previous) = self.back.pop() else {
            return false;
        };
        self.forward
            .push(std::mem::replace(&mut self.current, previous));
        true
    }

    pub fn forward(&mut self) -> bool {
        let Some(next) = self.forward.pop() else {
            return false;
        };
        self.back.push(std::mem::replace(&mut self.current, next));
        true
    }

    /// Goes one level up, which counts as going somewhere.
    pub fn up(&mut self) -> bool {
        self.current.up().is_some_and(|up| self.go(up))
    }
}

/// Something a folder holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub folder: bool,
    /// In bytes; 0 for a folder.
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub hidden: bool,
}

impl Entry {
    /// What kind of file it is, as a column says it: `Folder`, or the
    /// extension of its name in capitals.
    pub fn kind(&self) -> String {
        if self.folder {
            return "Folder".to_owned();
        }
        match Path::new(&self.name).extension() {
            Some(extension) => extension.to_string_lossy().to_uppercase(),
            None => String::new(),
        }
    }
}

/// What a folder holds, with its folders and files mixed, in no order.
/// A thing whose metadata cannot be read is left out.
pub fn read_folder(path: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(path)? {
        let Ok(item) = item else {
            continue;
        };
        let Ok(metadata) = item.metadata() else {
            continue;
        };
        let folder = metadata.is_dir();
        entries.push(Entry {
            name: item.file_name().to_string_lossy().into_owned(),
            path: item.path(),
            folder,
            size: if folder { 0 } else { metadata.len() },
            modified: metadata.modified().ok(),
            hidden: is_hidden(&metadata),
        });
    }
    Ok(entries)
}

/// When a folder was last changed, which it is whenever something in it
/// is added, removed or renamed.
pub fn stamp_of(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|folder| folder.modified()).ok()
}

/// A column of the list, which it can be ordered by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Name,
    Modified,
    Kind,
    Size,
}

impl Column {
    pub const ALL: [Self; 4] = [Self::Name, Self::Modified, Self::Kind, Self::Size];

    pub fn title(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Modified => "Modified",
            Self::Kind => "Type",
            Self::Size => "Size",
        }
    }
}

/// The order of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Order {
    pub column: Column,
    pub descending: bool,
    /// Whether folders come before files whatever the column.
    pub folders_first: bool,
}

impl Default for Order {
    fn default() -> Self {
        Self {
            column: Column::Name,
            descending: false,
            folders_first: true,
        }
    }
}

impl Order {
    /// The order after a click on the heading of `column`: by that column,
    /// and the other way round when it was by it already.
    pub fn toggled(self, column: Column) -> Self {
        Self {
            column,
            descending: self.column == column && !self.descending,
            ..self
        }
    }

    pub fn compare(&self, a: &Entry, b: &Entry) -> Ordering {
        if self.folders_first && a.folder != b.folder {
            return b.folder.cmp(&a.folder);
        }
        let by_column = match self.column {
            Column::Name => Ordering::Equal,
            Column::Modified => a.modified.cmp(&b.modified),
            Column::Kind => a.kind().cmp(&b.kind()),
            Column::Size => a.size.cmp(&b.size),
        };
        let order = by_column.then_with(|| natural(&a.name, &b.name));
        if self.descending {
            order.reverse()
        } else {
            order
        }
    }
}

/// Compares names as Explorer does: without regard to case, and with the
/// numbers in them by their value, so that `2` comes before `10`.
pub fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |chars: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(digit) = chars.next_if(char::is_ascii_digit) {
                        digits.push(digit);
                    }
                    digits
                };
                let (x, y) = (number(&mut a), number(&mut b));
                let (tx, ty) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = tx.len().cmp(&ty.len()).then_with(|| tx.cmp(ty));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// What a folder holds and which of it is shown, in which order.
#[derive(Debug, Default)]
pub struct Listing {
    entries: Vec<Entry>,
    /// Indices into `entries` of what is shown, in order.
    shown: Vec<usize>,
}

impl Listing {
    pub fn new(entries: Vec<Entry>) -> Self {
        Self {
            entries,
            shown: Vec::new(),
        }
    }

    /// Shows what matches `filter` (a part of the name, in any case),
    /// hidden files only with `hidden`, in `order`.
    pub fn arrange(&mut self, order: &Order, filter: &str, hidden: bool) {
        let filter = filter.trim().to_lowercase();
        let entries = &self.entries;
        self.shown = (0..entries.len())
            .filter(|&index| {
                let entry = &entries[index];
                (hidden || !entry.hidden)
                    && (filter.is_empty() || entry.name.to_lowercase().contains(&filter))
            })
            .collect();
        self.shown
            .sort_unstable_by(|&a, &b| order.compare(&entries[a], &entries[b]));
    }

    /// How many are shown.
    pub fn len(&self) -> usize {
        self.shown.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    /// What is shown at `position`.
    pub fn get(&self, position: usize) -> Option<&Entry> {
        self.shown.get(position).map(|&index| &self.entries[index])
    }

    /// Where `path` is shown, if it is.
    pub fn position_of(&self, path: &Path) -> Option<usize> {
        self.shown
            .iter()
            .position(|&index| self.entries[index].path == path)
    }

    /// The bytes the files that are shown take.
    pub fn size_of(&self, paths: &HashSet<PathBuf>) -> u64 {
        { self.shown.iter().map(|&index| &self.entries[index]) }
            .filter(|entry| paths.contains(&entry.path))
            .map(|entry| entry.size)
            .sum()
    }
}

/// Which of the shown entries are chosen, by path, so that the choice
/// outlasts a new order or a new reading of the folder; and the one the
/// keys move from.
#[derive(Debug, Default)]
pub struct Selection {
    pub chosen: HashSet<PathBuf>,
    /// The position the arrow keys move from.
    pub cursor: Option<usize>,
    /// Where a choice with Shift reaches from.
    anchor: Option<usize>,
}

impl Selection {
    pub fn contains(&self, path: &Path) -> bool {
        self.chosen.contains(path)
    }

    pub fn clear(&mut self) {
        self.chosen.clear();
        self.cursor = None;
        self.anchor = None;
    }

    /// The choice after a click on `position`: that one alone, or with
    /// `toggle` it in or out of the choice, or with `extend` everything
    /// from the anchor to it.
    pub fn click(&mut self, listing: &Listing, position: usize, toggle: bool, extend: bool) {
        let Some(entry) = listing.get(position) else {
            return;
        };
        if extend && let Some(anchor) = self.anchor {
            self.choose_range(listing, anchor, position);
        } else if toggle {
            if !self.chosen.remove(&entry.path) {
                self.chosen.insert(entry.path.clone());
            }
            self.anchor = Some(position);
        } else {
            self.chosen.clear();
            self.chosen.insert(entry.path.clone());
            self.anchor = Some(position);
        }
        self.cursor = Some(position);
    }

    /// Moves the cursor `by` positions, choosing what it lands on alone,
    /// or with `extend` everything from the anchor to it.
    pub fn step(&mut self, listing: &Listing, by: isize, extend: bool) {
        if listing.is_empty() {
            return;
        }
        let last = listing.len() - 1;
        let to = match self.cursor {
            None if by < 0 => last,
            None => 0,
            Some(cursor) => cursor.saturating_add_signed(by).min(last),
        };
        if extend {
            let anchor = *self.anchor.get_or_insert(self.cursor.unwrap_or(to));
            self.choose_range(listing, anchor, to);
            self.cursor = Some(to);
        } else {
            self.click(listing, to, false, false);
        }
    }

    pub fn all(&mut self, listing: &Listing) {
        self.chosen = (0..listing.len())
            .filter_map(|position| listing.get(position))
            .map(|entry| entry.path.clone())
            .collect();
    }

    fn choose_range(&mut self, listing: &Listing, from: usize, to: usize) {
        self.chosen.clear();
        for position in from.min(to)..=from.max(to) {
            if let Some(entry) = listing.get(position) {
                self.chosen.insert(entry.path.clone());
            }
        }
    }

    /// Keeps what is still shown, after the listing changed, and moves
    /// the cursor along with what it was on.
    pub fn follow(&mut self, before: Option<PathBuf>, listing: &Listing) {
        self.chosen
            .retain(|path| listing.position_of(path).is_some());
        self.cursor = before.and_then(|path| listing.position_of(&path));
        self.anchor = self.cursor;
    }

    /// The chosen paths, in the order of the listing.
    pub fn in_order(&self, listing: &Listing) -> Vec<PathBuf> {
        (0..listing.len())
            .filter_map(|position| listing.get(position))
            .filter(|entry| self.chosen.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect()
    }
}

/// The steps of the path of a folder, from its drive, each with where it
/// leads: what the bar over the list is made of.
pub fn crumbs(path: &Path) -> Vec<(String, PathBuf)> {
    let mut crumbs = Vec::new();
    let mut reached = PathBuf::new();
    for component in path.components() {
        reached.push(component.as_os_str());
        let name = match component {
            Component::Prefix(prefix) => prefix.as_os_str().to_string_lossy().into_owned(),
            Component::RootDir => {
                // The root joins the drive before it: `C:` and `\` are one step.
                if crumbs.is_empty() {
                    crumbs.push(("\\".to_owned(), reached.clone()));
                } else if let Some(last) = crumbs.last_mut() {
                    last.1 = reached.clone();
                }
                continue;
            }
            Component::CurDir | Component::ParentDir => continue,
            Component::Normal(name) => name.to_string_lossy().into_owned(),
        };
        crumbs.push((name, reached.clone()));
    }
    crumbs
}

/// A number of bytes as people read it.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    match unit {
        0 => format!("{bytes} B"),
        _ if value < 10. => format!("{value:.1} {}", UNITS[unit]),
        _ => format!("{value:.0} {}", UNITS[unit]),
    }
}

/// A name in `folder` that nothing there has yet: `base`, or `base (2)`,
/// `base (3)` and so on.
pub fn free_name(folder: &Path, base: &str) -> PathBuf {
    let first = folder.join(base);
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|number| folder.join(format!("{base} ({number})")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

/// Whether `name` can be the name of a file: not empty, no `\ / : * ? " < > |`
/// in it, and not ending in a dot or a space.
pub fn is_valid_name(name: &str) -> bool {
    !name.trim().is_empty()
        && !name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|'])
        && !name.ends_with(['.', ' '])
        && !matches!(name, "." | "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, folder: bool, size: u64) -> Entry {
        Entry {
            name: name.to_owned(),
            path: PathBuf::from(name),
            folder,
            size,
            modified: None,
            hidden: name.starts_with('.'),
        }
    }

    fn names(listing: &Listing) -> Vec<&str> {
        (0..listing.len())
            .map(|position| listing.get(position).unwrap().name.as_str())
            .collect()
    }

    #[test]
    fn names_are_ordered_by_their_numbers() {
        assert_eq!(natural("file2", "file10"), Ordering::Less);
        assert_eq!(natural("File10", "file9"), Ordering::Greater);
        assert_eq!(natural("a", "B"), Ordering::Less);
        assert_eq!(natural("x01", "x1"), Ordering::Equal);
        assert_eq!(natural("ab", "abc"), Ordering::Less);
    }

    #[test]
    fn folders_come_first_and_the_order_turns() {
        let mut listing = Listing::new(vec![
            entry("b.txt", false, 5),
            entry("Zeta", true, 0),
            entry("a.txt", false, 50),
            entry(".hidden", false, 1),
        ]);
        let order = Order::default();
        listing.arrange(&order, "", false);
        assert_eq!(names(&listing), ["Zeta", "a.txt", "b.txt"]);
        listing.arrange(&order.toggled(Column::Name), "", false);
        assert_eq!(names(&listing), ["b.txt", "a.txt", "Zeta"]);
        let by_size = order.toggled(Column::Size);
        listing.arrange(&by_size, "", true);
        assert_eq!(names(&listing), ["Zeta", ".hidden", "b.txt", "a.txt"]);
        listing.arrange(&order, "TXT", false);
        assert_eq!(names(&listing), ["a.txt", "b.txt"]);
    }

    #[test]
    fn history_goes_back_and_forth() {
        let folder = |name: &str| Location::Folder(PathBuf::from(name));
        let mut history = History::new(folder("a"));
        assert!(history.go(folder("b")));
        assert!(!history.go(folder("b")));
        history.go(folder("c"));
        assert!(history.back());
        assert_eq!(history.current(), &folder("b"));
        assert!(history.forward());
        assert_eq!(history.current(), &folder("c"));
        history.back();
        history.go(folder("d"));
        assert!(!history.can_go_forward());
    }

    #[test]
    fn up_from_a_drive_is_the_computer() {
        let drive = Location::Folder(PathBuf::from("C:\\"));
        assert_eq!(drive.up(), Some(Location::Computer));
        assert_eq!(Location::Computer.up(), None);
        let inner = Location::Folder(PathBuf::from("C:\\Users"));
        assert_eq!(inner.up(), Some(drive));
    }

    #[test]
    fn crumbs_join_the_drive_and_its_root() {
        let crumbs = crumbs(Path::new("C:\\Users\\me"));
        let names: Vec<&str> = crumbs.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["C:", "Users", "me"]);
        assert_eq!(crumbs[0].1, PathBuf::from("C:\\"));
        assert_eq!(crumbs[2].1, PathBuf::from("C:\\Users\\me"));
    }

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(1536), "1.5 KB");
        assert_eq!(size_text(50 * 1024 * 1024), "50 MB");
    }

    #[test]
    fn names_with_reserved_characters_are_refused() {
        assert!(is_valid_name("Report 2026.txt"));
        assert!(!is_valid_name("a:b"));
        assert!(!is_valid_name("trailing."));
        assert!(!is_valid_name("  "));
    }

    #[test]
    fn a_selection_reaches_from_its_anchor() {
        let mut listing = Listing::new((0..5).map(|n| entry(&format!("f{n}"), false, 0)).collect());
        listing.arrange(&Order::default(), "", false);
        let mut selection = Selection::default();
        selection.click(&listing, 1, false, false);
        selection.click(&listing, 3, false, true);
        assert_eq!(selection.chosen.len(), 3);
        selection.step(&listing, 1, false);
        assert_eq!(selection.cursor, Some(4));
        assert_eq!(selection.chosen.len(), 1);
        selection.click(&listing, 0, true, false);
        assert_eq!(
            selection.in_order(&listing),
            [PathBuf::from("f0"), PathBuf::from("f4")]
        );
        selection.step(&listing, -10, false);
        assert_eq!(selection.cursor, Some(0));
    }
}
