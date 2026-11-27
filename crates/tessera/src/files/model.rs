// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the file manager knows apart from how it is drawn: where it is and
//! has been, what a folder holds, in which order and which of it is
//! chosen.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::io;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Datelike, Local, Months, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::platform::{Attributes, NameRules, is_hidden, localized_candidates, shell_names};

/// How many names of a folder are asked of the Shell at most, as each
/// takes it a moment.
const SHELL_NAMES_ASKED: usize = 512;

/// Where a window of the file manager is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Location {
    /// Home: the folders of Quick access and the files used lately.
    Home,
    /// The drives of the computer.
    Computer,
    Folder(PathBuf),
    /// What a search below the folder `root` finds of `query`.
    Search {
        root: PathBuf,
        query: String,
    },
}

impl Location {
    /// The location one level up: the folder around a folder, and the
    /// computer around the root of a drive. The computer has none.
    pub fn up(&self) -> Option<Self> {
        match self {
            Self::Home | Self::Computer => None,
            Self::Folder(path) => Some(match path.parent() {
                Some(parent) if parent != path && !parent.as_os_str().is_empty() => {
                    Self::Folder(parent.to_owned())
                }
                _ => Self::Computer,
            }),
            Self::Search { root, .. } => Some(Self::Folder(root.clone())),
        }
    }

    pub fn folder(&self) -> Option<&Path> {
        match self {
            Self::Home | Self::Computer | Self::Search { .. } => None,
            Self::Folder(path) => Some(path),
        }
    }

    /// The folder the location is in: the folder itself, or the one a
    /// search is below.
    pub fn root(&self) -> Option<&Path> {
        match self {
            Self::Search { root, .. } => Some(root),
            other => other.folder(),
        }
    }

    /// The location as a command names it, which `Files::named` reads
    /// back: `home`, `computer`, or the path of the folder.
    pub fn name(&self) -> String {
        match self {
            Self::Home => "home".to_owned(),
            Self::Computer => "computer".to_owned(),
            Self::Folder(path) | Self::Search { root: path, .. } => path.display().to_string(),
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
    /// The name of the file itself.
    pub name: String,
    /// The name as it is shown: the one the Shell gives in the language
    /// of the user, or without an extension Explorer hides.
    pub display: String,
    pub path: PathBuf,
    pub folder: bool,
    /// In bytes; 0 for a folder.
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Whether it is hidden or of the system.
    pub hidden: bool,
    /// Whether it is a protected file of the system, hidden and of the
    /// system both, which Explorer hides even where hidden files show.
    pub protected: bool,
    /// Whether it is a shortcut, over whose icon an arrow is drawn.
    pub link: bool,
}

impl Entry {
    /// The entry of the file or folder at `path`, of which `metadata` was
    /// read, its name shown as `rules` say.
    pub fn of(path: PathBuf, metadata: &Metadata, rules: &mut NameRules) -> Self {
        let folder = metadata.is_dir();
        let attributes = Attributes::of(metadata);
        let name = { path.file_name() }
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let link = !folder
            && Path::new(&name).extension().is_some_and(|extension| {
                extension.eq_ignore_ascii_case("lnk") || extension.eq_ignore_ascii_case("url")
            });
        Self {
            display: rules.shown(&name, folder),
            name,
            path,
            folder,
            size: if folder { 0 } else { metadata.len() },
            modified: metadata.modified().ok(),
            hidden: is_hidden(metadata),
            protected: attributes.protected(),
            link,
        }
    }

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

/// What a folder holds, with its folders and files mixed, in no order,
/// named as `rules` say and, where they follow Explorer, as the Shell
/// names what a `desktop.ini` gives a name of its own. A thing whose
/// metadata cannot be read is left out. Blocks on the disk.
pub fn read_folder(path: &Path, rules: &mut NameRules) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut attributes = Vec::new();
    for item in fs::read_dir(path)? {
        let Ok(item) = item else {
            continue;
        };
        let Ok(metadata) = item.metadata() else {
            continue;
        };
        attributes.push(Attributes::of(&metadata));
        entries.push(Entry::of(item.path(), &metadata, rules));
    }
    if rules.follows() {
        let items = { entries.iter().zip(attributes) }
            .map(|(entry, attributes)| (entry.path.clone(), entry.folder, attributes));
        let mut asked = localized_candidates(path, items);
        asked.truncate(SHELL_NAMES_ASKED);
        let named = shell_names(&asked);
        for (asked, name) in asked.iter().zip(named) {
            let entry = entries.iter_mut().find(|entry| &entry.path == asked);
            if let (Some(entry), Some(name)) = (entry, name) {
                entry.display = name;
            }
        }
    }
    Ok(entries)
}

/// When a folder was last changed, which it is whenever something in it
/// is added, removed or renamed.
pub fn stamp_of(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|folder| folder.modified()).ok()
}

/// A column of the list, which it can be ordered by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Column {
    Name,
    Modified,
    #[serde(rename = "type")]
    Kind,
    Size,
    /// The folder a file is in, which results of a search show.
    Folder,
}

impl Column {
    /// The columns of a folder.
    pub const ALL: [Self; 4] = [Self::Name, Self::Modified, Self::Kind, Self::Size];
    /// The columns of the results of a search, which come from many
    /// folders.
    pub const FOUND: [Self; 4] = [Self::Name, Self::Folder, Self::Modified, Self::Size];

    pub fn title(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Modified => "Modified",
            Self::Kind => "Type",
            Self::Size => "Size",
            Self::Folder => "Folder",
        }
    }
}

/// How a folder shows what it holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Look {
    /// A row per item with its details.
    #[default]
    Details,
    /// Small icons with their names, in columns.
    List,
    /// Icons with the name, the type and the size beside them.
    Tiles,
    /// Large icons with their names below.
    Icons,
}

/// The measures of an item in a look, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    /// How wide and high the icon is.
    pub icon: f32,
    /// How wide an item is; 0 for a row as wide as the list.
    pub width: f32,
    pub height: f32,
}

impl Look {
    pub const ALL: [Self; 4] = [Self::Details, Self::List, Self::Tiles, Self::Icons];

    /// The sizes of icons the look zooms through.
    fn icons(self) -> &'static [f32] {
        match self {
            Self::Details => &[16., 20., 24., 32.],
            Self::List => &[16., 20., 24.],
            Self::Tiles => &[32., 48., 64.],
            Self::Icons => &[32., 48., 64., 96., 128.],
        }
    }

    /// The zoom the look starts at: the sizes Explorer starts at.
    pub fn first_zoom(self) -> usize {
        match self {
            Self::Details | Self::Tiles => 1,
            Self::List => 0,
            Self::Icons => 1,
        }
    }

    /// The zoom one step in, with `bigger`, or out, held within the sizes
    /// of the look.
    pub fn zoomed(self, zoom: usize, bigger: bool) -> usize {
        let last = self.icons().len() - 1;
        match bigger {
            true => (zoom + 1).min(last),
            false => zoom.min(last).saturating_sub(1),
        }
    }

    /// The measures of an item at `zoom`.
    pub fn shape(self, zoom: usize) -> Shape {
        let icons = self.icons();
        let icon = icons[zoom.min(icons.len() - 1)];
        match self {
            Self::Details => Shape {
                icon,
                width: 0.,
                height: icon + 12.,
            },
            Self::List => Shape {
                icon,
                width: 240.,
                height: icon + 10.,
            },
            Self::Tiles => Shape {
                icon,
                width: 280.,
                height: icon + 24.,
            },
            Self::Icons => Shape {
                icon,
                width: icon + 64.,
                height: icon + 58.,
            },
        }
    }

    /// Whether the items stand side by side, in a grid.
    pub fn is_grid(self) -> bool {
        self != Self::Details
    }

    /// The name the look has in the menu.
    pub fn title(self) -> &'static str {
        match self {
            Self::Details => "Details",
            Self::List => "List",
            Self::Tiles => "Tiles",
            Self::Icons => "Large icons",
        }
    }
}

/// What a folder is grouped by, each group under a heading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Grouping {
    #[default]
    None,
    /// By type: folders, then each extension.
    #[serde(rename = "type")]
    Kind,
    /// By when it was changed, as Explorer groups by date.
    #[serde(rename = "date")]
    Modified,
    /// Quick access, then the files used lately, as Home is.
    #[serde(skip)]
    Home,
}

impl Grouping {
    /// The groupings the user chooses from.
    pub const CHOSEN: [Self; 3] = [Self::None, Self::Kind, Self::Modified];

    pub fn title(self) -> &'static str {
        match self {
            Self::None => "No groups",
            Self::Kind => "Group by type",
            Self::Modified => "Group by date",
            Self::Home => "Quick access",
        }
    }
}

/// What a group is called: a text of Tessera, said in the language of
/// the user, or one of the files, such as an extension.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GroupLabel {
    Said(&'static str),
    Text(String),
}

/// Where a group stands among the others, and what it is called.
pub type GroupKey = (u8, GroupLabel);

/// The groups by date, as Explorer has them, the latest first.
const DATE_GROUPS: [&str; 9] = [
    "Today",
    "Yesterday",
    "Earlier this week",
    "Last week",
    "Earlier this month",
    "Last month",
    "Earlier this year",
    "A long time ago",
    "Unknown date",
];

/// The group by date of a moment, `today` being the day it is.
pub fn date_group(moment: Option<NaiveDate>, today: NaiveDate) -> usize {
    let Some(day) = moment else {
        return 8;
    };
    let week = |day: NaiveDate| day.week(chrono::Weekday::Mon).first_day();
    let month = |day: NaiveDate| (day.year(), day.month());
    let last_month = today.checked_sub_months(Months::new(1)).map(month);
    match day {
        day if day >= today => 0,
        day if today.pred_opt() == Some(day) => 1,
        day if week(day) == week(today) => 2,
        day if week(today).checked_sub_days(chrono::Days::new(7)) == Some(week(day)) => 3,
        day if month(day) == month(today) => 4,
        day if Some(month(day)) == last_month => 5,
        day if day.year() == today.year() => 6,
        _ => 7,
    }
}

/// The group of `entry` by `grouping`, `today` being the day it is.
pub fn group_of(entry: &Entry, grouping: Grouping, today: NaiveDate) -> GroupKey {
    match grouping {
        Grouping::None => (0, GroupLabel::Said("")),
        Grouping::Kind => match (entry.folder, entry.kind()) {
            (true, _) => (0, GroupLabel::Said("Folder")),
            (false, kind) if kind.is_empty() => (2, GroupLabel::Said("File")),
            (false, kind) => (1, GroupLabel::Text(kind)),
        },
        Grouping::Modified => {
            let day = entry
                .modified
                .map(|time| DateTime::<Local>::from(time).date_naive());
            let group = date_group(day, today);
            (group as u8, GroupLabel::Said(DATE_GROUPS[group]))
        }
        Grouping::Home => match entry.folder {
            true => (0, GroupLabel::Said("Quick access")),
            false => (1, GroupLabel::Said("Recent files")),
        },
    }
}

/// A line of the list as it is drawn: the heading of a group, or the
/// items that stand side by side, by their positions.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Heading { label: GroupLabel, count: usize },
    Items(Range<usize>),
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
            Column::Folder => natural(
                &a.path.parent().unwrap_or(&a.path).to_string_lossy(),
                &b.path.parent().unwrap_or(&b.path).to_string_lossy(),
            ),
            Column::Modified => a.modified.cmp(&b.modified),
            Column::Kind => a.kind().cmp(&b.kind()),
            Column::Size => a.size.cmp(&b.size),
        };
        let order = by_column.then_with(|| natural(&a.display, &b.display));
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

/// Which of the hidden files are shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// Hidden files and files of the system.
    pub hidden: bool,
    /// Protected files of the system too, where hidden files are shown.
    pub protected: bool,
}

impl Shown {
    /// Shows every file there is.
    #[cfg(test)]
    pub const ALL: Self = Self {
        hidden: true,
        protected: true,
    };

    pub fn shows(self, entry: &Entry) -> bool {
        match (entry.hidden, entry.protected) {
            (false, _) => true,
            (true, false) => self.hidden,
            (true, true) => self.hidden && self.protected,
        }
    }
}

/// What a folder holds and which of it is shown, in which order.
#[derive(Debug, Default)]
pub struct Listing {
    entries: Vec<Entry>,
    /// Indices into `entries` of what is shown, in order.
    shown: Vec<usize>,
    /// The group of each that is shown, where they are grouped.
    groups: Vec<GroupKey>,
}

/// How a listing is arranged: in which order, narrowed to what, with
/// which hidden files, in which groups.
#[derive(Clone, Copy, Debug)]
pub struct Arrangement<'a> {
    pub order: &'a Order,
    /// A part of the name as shown, in any case.
    pub filter: &'a str,
    pub shown: Shown,
    pub grouping: Grouping,
    /// The day it is, which groups by date reckon from.
    pub today: NaiveDate,
}

impl Listing {
    pub fn new(entries: Vec<Entry>) -> Self {
        Self {
            entries,
            shown: Vec::new(),
            groups: Vec::new(),
        }
    }

    /// Everything the listing holds, shown or not.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Adds `entries` to what the listing holds; it is arranged again
    /// afterwards.
    pub fn extend(&mut self, entries: impl IntoIterator<Item = Entry>) {
        self.entries.extend(entries);
    }

    /// Shows what matches the filter of `arrangement`, the hidden files
    /// it shows, group by group and in its order within each.
    pub fn arrange(&mut self, arrangement: Arrangement) {
        let filter = arrangement.filter.trim().to_lowercase();
        let entries = &self.entries;
        let shown: Vec<usize> = (0..entries.len())
            .filter(|&index| {
                let entry = &entries[index];
                arrangement.shown.shows(entry)
                    && (filter.is_empty() || entry.display.to_lowercase().contains(&filter))
            })
            .collect();
        let (grouping, today) = (arrangement.grouping, arrangement.today);
        let mut keyed: Vec<(GroupKey, usize)> = { shown.into_iter() }
            .map(|index| (group_of(&entries[index], grouping, today), index))
            .collect();
        keyed.sort_unstable_by(|(a_group, a), (b_group, b)| {
            { a_group.cmp(b_group) }
                .then_with(|| arrangement.order.compare(&entries[*a], &entries[*b]))
        });
        (self.groups, self.shown) = keyed.into_iter().unzip();
        if grouping == Grouping::None {
            self.groups.clear();
        }
    }

    /// The lines the shown entries are drawn in, `across` side by side,
    /// with a heading over each group where they are grouped.
    pub fn lines(&self, across: usize) -> Vec<Line> {
        let across = across.max(1);
        let mut lines = Vec::new();
        let mut start = 0;
        while start < self.shown.len() {
            let end = match self.groups.get(start) {
                Some(group) => { self.groups[start..].iter() }
                    .position(|other| other != group)
                    .map_or(self.shown.len(), |length| start + length),
                None => self.shown.len(),
            };
            if let Some((_, label)) = self.groups.get(start) {
                lines.push(Line::Heading {
                    label: label.clone(),
                    count: end - start,
                });
            }
            let mut from = start;
            while from < end {
                let to = (from + across).min(end);
                lines.push(Line::Items(from..to));
                from = to;
            }
            start = end;
        }
        lines
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

    /// Chooses what is shown and was not chosen, and nothing of what was.
    pub fn invert(&mut self, listing: &Listing) {
        self.chosen = (0..listing.len())
            .filter_map(|position| listing.get(position))
            .filter(|entry| !self.chosen.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect();
        self.anchor = None;
    }

    /// Adds to the choice what is shown whose name matches `patterns`:
    /// patterns with `*` and `?`, in any case, separated by `;`. Says how
    /// many it added.
    pub fn add_matching(&mut self, listing: &Listing, patterns: &str) -> usize {
        let patterns: Vec<&str> = { patterns.split(';') }
            .map(str::trim)
            .filter(|pattern| !pattern.is_empty())
            .collect();
        let before = self.chosen.len();
        for entry in (0..listing.len()).filter_map(|position| listing.get(position)) {
            let matches = |pattern: &&str| {
                glob_matches(pattern, &entry.name) || glob_matches(pattern, &entry.display)
            };
            if patterns.iter().any(matches) {
                self.chosen.insert(entry.path.clone());
            }
        }
        self.chosen.len() - before
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

/// Whether `name` matches `pattern`, in which `*` stands for any letters
/// and `?` for one, in any case.
pub fn glob_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let name: Vec<char> = name.to_lowercase().chars().collect();
    // Where the last star was, and the letter of the name it reached.
    let (mut star, mut reached) = (None, 0);
    let (mut p, mut n) = (0, 0);
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some(p);
                reached = n;
                p += 1;
            }
            Some(&letter) if letter == '?' || letter == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match star {
                Some(at) => {
                    p = at + 1;
                    reached += 1;
                    n = reached;
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&letter| letter == '*')
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

/// The share of another computer `path` lies in, as `\\server\share`;
/// `None` for a path on a drive of this one.
pub fn share_root(path: &Path) -> Option<PathBuf> {
    use std::path::Prefix;
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return None;
    };
    match prefix.kind() {
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            let mut root = std::ffi::OsString::from("\\\\");
            root.push(server);
            root.push("\\");
            root.push(share);
            Some(PathBuf::from(root))
        }
        _ => None,
    }
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
            display: name.to_owned(),
            path: PathBuf::from(name),
            folder,
            size,
            modified: None,
            hidden: name.starts_with('.'),
            protected: name.starts_with("~"),
            link: false,
        }
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 26).unwrap()
    }

    fn arranged(listing: &mut Listing, order: &Order, filter: &str, shown: Shown) {
        listing.arrange(Arrangement {
            order,
            filter,
            shown,
            grouping: Grouping::None,
            today: today(),
        });
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
        let plain = Shown::default();
        arranged(&mut listing, &order, "", plain);
        assert_eq!(names(&listing), ["Zeta", "a.txt", "b.txt"]);
        arranged(&mut listing, &order.toggled(Column::Name), "", plain);
        assert_eq!(names(&listing), ["b.txt", "a.txt", "Zeta"]);
        let by_size = order.toggled(Column::Size);
        arranged(&mut listing, &by_size, "", Shown::ALL);
        assert_eq!(names(&listing), ["Zeta", ".hidden", "b.txt", "a.txt"]);
        arranged(&mut listing, &order, "TXT", plain);
        assert_eq!(names(&listing), ["a.txt", "b.txt"]);
    }

    #[test]
    fn protected_files_need_both_switches() {
        let mut protected = entry("~protected", false, 0);
        protected.hidden = true;
        let mut listing = Listing::new(vec![
            entry("plain", false, 0),
            entry(".hidden", false, 0),
            protected,
        ]);
        let shown = |listing: &mut Listing, hidden, protected| {
            arranged(listing, &Order::default(), "", Shown { hidden, protected });
            listing.len()
        };
        assert_eq!(shown(&mut listing, false, true), 1);
        assert_eq!(shown(&mut listing, true, false), 2);
        assert_eq!(shown(&mut listing, true, true), 3);
    }

    #[test]
    fn the_shown_name_orders_and_filters() {
        let mut documents = entry("Documents", true, 0);
        documents.display = "Документы".to_owned();
        let mut listing = Listing::new(vec![documents, entry("Zeta", true, 0)]);
        arranged(&mut listing, &Order::default(), "доку", Shown::default());
        assert_eq!(names(&listing), ["Documents"]);
        arranged(&mut listing, &Order::default(), "", Shown::default());
        assert_eq!(names(&listing), ["Zeta", "Documents"]);
    }

    #[test]
    fn groups_stand_under_headings_and_fill_their_lines() {
        let mut listing = Listing::new(vec![
            entry("b.txt", false, 0),
            entry("Folder", true, 0),
            entry("a.txt", false, 0),
            entry("c.png", false, 0),
            entry("README", false, 0),
        ]);
        listing.arrange(Arrangement {
            order: &Order::default(),
            filter: "",
            shown: Shown::default(),
            grouping: Grouping::Kind,
            today: today(),
        });
        assert_eq!(
            names(&listing),
            ["Folder", "c.png", "a.txt", "b.txt", "README"]
        );
        let heading = |label: GroupLabel, count| Line::Heading { label, count };
        assert_eq!(
            listing.lines(2),
            [
                heading(GroupLabel::Said("Folder"), 1),
                Line::Items(0..1),
                heading(GroupLabel::Text("PNG".to_owned()), 1),
                Line::Items(1..2),
                heading(GroupLabel::Text("TXT".to_owned()), 2),
                Line::Items(2..4),
                heading(GroupLabel::Said("File"), 1),
                Line::Items(4..5),
            ]
        );
        arranged(&mut listing, &Order::default(), "", Shown::default());
        assert_eq!(listing.lines(3), [Line::Items(0..3), Line::Items(3..5)]);
    }

    #[test]
    fn dates_are_grouped_as_explorer_groups_them() {
        let day = |month, day| NaiveDate::from_ymd_opt(2026, month, day);
        // Thursday the 26th of November 2026.
        let today = today();
        let group = |moment| DATE_GROUPS[date_group(moment, today)];
        assert_eq!(group(day(11, 26)), "Today");
        assert_eq!(group(day(11, 27)), "Today");
        assert_eq!(group(day(11, 25)), "Yesterday");
        assert_eq!(group(day(11, 23)), "Earlier this week");
        assert_eq!(group(day(11, 17)), "Last week");
        assert_eq!(group(day(11, 2)), "Earlier this month");
        assert_eq!(group(day(10, 31)), "Last month");
        assert_eq!(group(day(2, 1)), "Earlier this year");
        assert_eq!(
            group(NaiveDate::from_ymd_opt(2025, 12, 31)),
            "A long time ago"
        );
        assert_eq!(group(None), "Unknown date");
    }

    #[test]
    fn a_look_zooms_within_its_sizes() {
        assert_eq!(Look::Icons.zoomed(0, false), 0);
        assert_eq!(Look::Icons.zoomed(4, true), 4);
        assert_eq!(Look::List.zoomed(9, false), 1);
        let first = Look::Details.shape(Look::Details.first_zoom());
        assert_eq!((first.icon, first.width), (20., 0.));
        assert!(
            Look::ALL
                .iter()
                .all(|look| look.is_grid() == (*look != Look::Details))
        );
    }

    #[test]
    fn patterns_match_names_with_stars_and_marks() {
        assert!(glob_matches("*.TXT", "notes.txt"));
        assert!(glob_matches("IMG_????.jpg", "img_0042.JPG"));
        assert!(!glob_matches("IMG_????.jpg", "img_042.jpg"));
        assert!(glob_matches("*a*b*", "xaybz"));
        assert!(!glob_matches("a*", "ba"));
        assert!(glob_matches("*", ""));
    }

    #[test]
    fn a_choice_turns_and_grows_by_patterns() {
        let mut listing = Listing::new(vec![
            entry("a.txt", false, 0),
            entry("b.png", false, 0),
            entry("c.txt", false, 0),
        ]);
        arranged(&mut listing, &Order::default(), "", Shown::default());
        let mut selection = Selection::default();
        assert_eq!(selection.add_matching(&listing, "*.txt; nothing"), 2);
        selection.invert(&listing);
        assert_eq!(selection.in_order(&listing), [PathBuf::from("b.png")]);
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
    fn up_from_a_search_is_its_folder() {
        let search = Location::Search {
            root: PathBuf::from("C:\\Users"),
            query: "notes".to_owned(),
        };
        assert_eq!(
            search.up(),
            Some(Location::Folder(PathBuf::from("C:\\Users")))
        );
        assert_eq!(search.folder(), None);
        assert_eq!(search.root(), Some(Path::new("C:\\Users")));
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
    fn a_share_of_another_computer_has_a_root() {
        let inner = Path::new(r"\\server\share\folder\file.txt");
        assert_eq!(share_root(inner), Some(PathBuf::from(r"\\server\share")));
        assert_eq!(share_root(Path::new(r"C:\Users")), None);
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
        arranged(&mut listing, &Order::default(), "", Shown::default());
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
