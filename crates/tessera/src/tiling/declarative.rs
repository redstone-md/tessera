// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Layouts written in files: a tree of containers that lie side by side
//! (`horizontal`), one above the other (`vertical`), or hold one window
//! (`leaf`), in the shape of the layouts of Seelen UI's window manager.
//!
//! Tessera brings a few, and `%APPDATA%\tessera\tiling-layouts\*.toml`
//! adds more; `layout = "<file name>"` chooses one. The windows go to the
//! parts by their `priority`, the lowest first, each part taking as many
//! as it holds: a leaf one, a container without parts of its own as many
//! as its `max`, or all that are left without one. A part whose
//! `condition` does not hold for the number of windows is left out, and
//! so is one that gets no window; `grow` is what a part weighs against
//! the others of its container.
//!
//! ```toml
//! name = "Tall"
//! [structure]
//! kind = "horizontal"
//! [[structure.children]]
//! kind = "leaf"
//! [[structure.children]]
//! kind = "vertical"
//! priority = 2
//! ```

use std::cmp::Reverse;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use serde::Deserialize;

use super::layouts::{Generated, TileLayout};
use super::tree::{Axis, Node};

/// The layouts that come with Tessera, by the name of their file.
const BUNDLED: [(&str, &str); 3] = [
    ("tall", include_str!("layouts/tall.toml")),
    ("wide", include_str!("layouts/wide.toml")),
    ("three-columns", include_str!("layouts/three-columns.toml")),
];

/// The titles of the bundled layouts, which are shown in the language of
/// the user.
const BUNDLED_TITLES: [&str; 3] = ["Tall", "Wide", "Three columns"];

/// The largest a layout file may be, and the most parts it may have.
const LARGEST_FILE: u64 = 64 * 1024;
const MOST_PARTS: usize = 256;
const DEEPEST: usize = 16;

/// The names of the layouts there are, those of the files of the user
/// included, for commands and keys that name one.
static KNOWN: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Horizontal,
    Vertical,
    Leaf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutFile {
    /// What the layout is called where it is shown.
    name: Option<String>,
    structure: PartFile,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartFile {
    kind: Kind,
    grow: Option<f32>,
    max: Option<usize>,
    priority: Option<u32>,
    condition: Option<Condition>,
    #[serde(default)]
    children: Vec<PartFile>,
}

/// For how many windows a part is there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Condition {
    windows_at_least: Option<usize>,
    windows_at_most: Option<usize>,
}

impl Condition {
    fn holds(self, count: usize) -> bool {
        self.windows_at_least.is_none_or(|least| count >= least)
            && self.windows_at_most.is_none_or(|most| count <= most)
    }
}

/// A part of a layout, as read and checked.
#[derive(Clone, Debug, PartialEq)]
struct Part {
    kind: Kind,
    grow: f32,
    /// How many windows a container without parts holds; `None` for all
    /// that are left.
    max: Option<usize>,
    priority: u32,
    condition: Condition,
    children: Vec<Part>,
}

/// A layout of a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Declared {
    /// The name of the file without its extension, in lowercase.
    pub name: String,
    /// What the file calls the layout.
    pub title: Option<String>,
    /// Whether it comes with Tessera.
    pub bundled: bool,
    structure: Part,
}

impl Declared {
    /// The layout `text` describes, for the file named `name`, or why it is
    /// none.
    pub fn parse(name: &str, text: &str) -> Result<Self, String> {
        let file: LayoutFile = toml::from_str(text).map_err(|error| error.message().to_owned())?;
        let mut parts = 0;
        let structure = part(file.structure, 0, &mut parts)?;
        if !open_ended(&structure) {
            return Err("it needs a container without `max` and without `condition` (in containers without one either), for the windows the other parts do not take".to_owned());
        }
        Ok(Self {
            name: name.to_lowercase(),
            title: file.name.filter(|title| !title.trim().is_empty()),
            bundled: false,
            structure,
        })
    }

    /// The tree of `count` windows.
    pub fn generate(&self, count: usize) -> Generated {
        if count == 0 {
            return Generated {
                tree: None,
                at: Vec::new(),
            };
        }
        // The parts that take windows, in the order of the file.
        let mut holders = Vec::new();
        gather(&self.structure, count, &mut holders);
        // Each takes its windows in the order of its priority.
        let mut order: Vec<usize> = (0..holders.len()).collect();
        order.sort_by_key(|&index| holders[index].0);
        let mut given = vec![(0, 0); holders.len()];
        let mut next = 0;
        for index in order {
            let room = holders[index].1.unwrap_or(usize::MAX);
            let taken = room.min(count - next);
            given[index] = (next, taken);
            next += taken;
        }
        let mut at = Vec::with_capacity(count);
        let mut holder = 0;
        let tree = build(&self.structure, count, &given, &mut holder, &mut at);
        Generated { tree, at }
    }
}

/// Reads and checks a part, counting the parts in `parts`.
fn part(file: PartFile, depth: usize, parts: &mut usize) -> Result<Part, String> {
    *parts += 1;
    if *parts > MOST_PARTS {
        return Err(format!("it has more than {MOST_PARTS} parts"));
    }
    if depth > DEEPEST {
        return Err(format!("its parts are nested more than {DEEPEST} deep"));
    }
    let grow = file.grow.unwrap_or(1.);
    if !grow.is_finite() || grow <= 0. || grow > 1000. {
        return Err(format!(
            "`grow` is a number above 0, up to 1000, not {grow}"
        ));
    }
    match (file.kind, file.max, file.children.is_empty()) {
        (Kind::Leaf, Some(_), _) => return Err("a leaf holds one window and has no `max`".into()),
        (Kind::Leaf, _, false) => return Err("a leaf has no parts".to_owned()),
        (_, Some(0), _) => return Err("`max` is at least 1".to_owned()),
        (_, Some(_), false) => {
            return Err("`max` is for a container without parts of its own".to_owned());
        }
        _ => {}
    }
    let condition = file.condition.unwrap_or_default();
    if let Condition {
        windows_at_least: Some(least),
        windows_at_most: Some(most),
    } = condition
        && least > most
    {
        return Err(format!(
            "a condition of at least {least} and at most {most} windows never holds"
        ));
    }
    let children = { file.children.into_iter() }
        .map(|child| part(child, depth + 1, parts))
        .collect::<Result<_, _>>()?;
    Ok(Part {
        kind: file.kind,
        grow,
        max: file.max,
        priority: file.priority.unwrap_or(u32::MAX - 1),
        condition,
        children,
    })
}

/// Whether `part`, or one of its parts, takes every window that is left
/// whatever their number.
fn open_ended(part: &Part) -> bool {
    if part.condition != Condition::default() {
        return false;
    }
    match (part.kind, part.children.is_empty()) {
        (Kind::Leaf, _) => false,
        (_, true) => part.max.is_none(),
        (_, false) => part.children.iter().any(open_ended),
    }
}

/// Lists the parts that take windows when there are `count`, in the
/// order of the file: each with its priority and its place, by which the
/// windows are given out, and how many it holds.
fn gather(part: &Part, count: usize, holders: &mut Vec<((u32, usize), Option<usize>)>) {
    if !part.condition.holds(count) {
        return;
    }
    let held = match (part.kind, part.children.is_empty()) {
        (Kind::Leaf, _) => Some(1),
        (_, true) => part.max,
        (_, false) => {
            for child in &part.children {
                gather(child, count, holders);
            }
            return;
        }
    };
    let place = holders.len();
    // Parts of the same priority fill in the order of the file.
    holders.push(((part.priority, place), held));
}

/// The tree of `part` once the windows were given out: `given[holder]` is
/// the first window and how many the holder at that place took; the
/// windows of the leaves are added to `at` in their order.
fn build(
    part: &Part,
    count: usize,
    given: &[(usize, usize)],
    holder: &mut usize,
    at: &mut Vec<usize>,
) -> Option<Node> {
    if !part.condition.holds(count) {
        return None;
    }
    let axis = match part.kind {
        Kind::Horizontal => Axis::Horizontal,
        Kind::Vertical => Axis::Vertical,
        Kind::Leaf => {
            let (first, taken) = given.get(*holder).copied().unwrap_or_default();
            *holder += 1;
            return (taken > 0).then(|| {
                at.push(first);
                Node::Leaf
            });
        }
    };
    if part.children.is_empty() {
        let (first, taken) = given.get(*holder).copied().unwrap_or_default();
        *holder += 1;
        at.extend(first..first + taken);
        return (taken > 0).then(|| Node::even(axis, taken));
    }
    let children: Vec<(Node, f32)> = { part.children.iter() }
        .filter_map(|child| Some((build(child, count, given, holder, at)?, child.grow)))
        .collect();
    (!children.is_empty()).then_some(Node::Split { axis, children })
}

/// What a scan of the directory of layout files found.
pub struct Scan {
    /// What the files were when they were read: their names, sizes and
    /// times of change.
    pub stamp: u64,
    pub layouts: Vec<Declared>,
    /// Why each file that cannot be used cannot be, by its name.
    pub problems: Vec<String>,
}

/// What the files of `directory` are now, cheaply: a change of a name,
/// size or time of change changes it.
pub fn stamp(directory: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut files: Vec<(String, u64, u128)> = layout_files(directory)
        .into_iter()
        .map(|(path, size, changed)| (path.to_string_lossy().into_owned(), size, changed))
        .collect();
    files.sort();
    files.hash(&mut hasher);
    hasher.finish()
}

/// The `.toml` files of `directory`, each with its size and the time it
/// was changed at.
fn layout_files(directory: &Path) -> Vec<(std::path::PathBuf, u64, u128)> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    { entries.flatten() }
        .filter_map(|entry| {
            let path = entry.path();
            let toml = path.extension()?.eq_ignore_ascii_case("toml");
            let meta = entry
                .metadata()
                .ok()
                .filter(|meta| meta.is_file() && toml)?;
            let changed = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
            Some((path, meta.len(), changed.as_nanos()))
        })
        .collect()
}

/// Reads the layout files of `directory`. A file named as a layout of
/// Tessera's own, or one that cannot be used, is left out and said why.
pub fn scan(directory: &Path) -> Scan {
    let stamp = stamp(directory);
    let mut layouts = Vec::new();
    let mut problems = Vec::new();
    for (path, size, _) in layout_files(directory) {
        let Some(name) = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_lowercase())
        else {
            continue;
        };
        let shown = path.display();
        if TileLayout::named(&name).is_some() {
            problems.push(format!(
                "{shown} is named as a layout of Tessera's own, so it is not used"
            ));
            continue;
        }
        if size > LARGEST_FILE {
            problems.push(format!(
                "{shown} is larger than {LARGEST_FILE} bytes, so it is not used"
            ));
            continue;
        }
        let read = fs::read_to_string(&path).map_err(|error| error.to_string());
        match read.and_then(|text| Declared::parse(&name, &text)) {
            Ok(layout) => layouts.push(layout),
            Err(why) => problems.push(format!("the tiling layout {shown} is not used: {why}")),
        }
    }
    layouts.sort_by(|first, second| first.name.cmp(&second.name));
    Scan {
        stamp,
        layouts,
        problems,
    }
}

/// The layouts that come with Tessera.
pub fn bundled() -> Vec<Declared> {
    { BUNDLED.iter() }
        .filter_map(|&(name, text)| match Declared::parse(name, text) {
            Ok(layout) => Some(Declared {
                bundled: true,
                ..layout
            }),
            Err(why) => {
                report!("the bundled tiling layout `{name}` is not used: {why}");
                None
            }
        })
        .collect()
}

/// The title of a bundled layout as the language of the user says it,
/// for the bundled layout called `title`.
pub fn bundled_title(title: &str) -> Option<&'static str> {
    BUNDLED_TITLES.into_iter().find(|&known| known == title)
}

/// The layouts of files there are: the bundled ones, and those of the
/// user, which take the place of a bundled one of the same name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Library {
    layouts: Vec<Declared>,
}

impl Library {
    /// The bundled layouts and `own`.
    pub fn with(own: Vec<Declared>) -> Self {
        let mut layouts: Vec<Declared> = { bundled().into_iter() }
            .filter(|bundled| !own.iter().any(|layout| layout.name == bundled.name))
            .collect();
        layouts.extend(own);
        layouts.sort_by_key(|layout| (Reverse(layout.bundled), layout.name.clone()));
        let library = Self { layouts };
        if let Ok(mut known) = KNOWN.lock() {
            *known = library.names();
        }
        library
    }

    /// The layout of the file called `name`.
    pub fn get(&self, name: &str) -> Option<&Declared> {
        self.layouts.iter().find(|layout| layout.name == name)
    }

    /// The names of the layouts, the bundled ones first.
    pub fn names(&self) -> Vec<String> {
        self.layouts
            .iter()
            .map(|layout| layout.name.clone())
            .collect()
    }
}

/// Whether a layout of a file is called `name`: one that comes with
/// Tessera, or one of the user that was read.
pub fn is_known(name: &str) -> bool {
    let name = name.to_lowercase();
    BUNDLED.iter().any(|&(bundled, _)| bundled == name)
        || KNOWN.lock().is_ok_and(|known| known.contains(&name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::ScreenRect;

    const AREA: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1200,
        bottom: 600,
    };

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    /// The tile of each window, in the order of the windows.
    fn tiles(layout: &Declared, count: usize) -> Vec<ScreenRect> {
        let made = layout.generate(count);
        let places = made
            .tree
            .map(|tree| tree.place(AREA, 0))
            .unwrap_or_default();
        assert_eq!(places.len(), count);
        let mut by_window = vec![AREA; count];
        for (leaf, &window) in made.at.iter().enumerate() {
            by_window[window] = places[leaf];
        }
        by_window
    }

    #[test]
    fn the_bundled_layouts_are_usable() {
        let layouts = bundled();
        assert_eq!(layouts.len(), BUNDLED.len());
        for layout in &layouts {
            assert!(layout.bundled);
            let title = layout.title.as_deref().unwrap();
            assert_eq!(bundled_title(title), Some(title));
            for count in 1..=8 {
                let made = layout.generate(count);
                let mut at = made.at.clone();
                at.sort_unstable();
                assert_eq!(
                    at,
                    (0..count).collect::<Vec<_>>(),
                    "{} of {count}",
                    layout.name
                );
            }
        }
        assert!(is_known("tall") && is_known("Three-Columns") && !is_known("spiral"));
    }

    #[test]
    fn windows_go_to_the_parts_by_their_priority() {
        let tall = Declared::parse("tall", include_str!("layouts/tall.toml")).unwrap();
        assert_eq!(tiles(&tall, 1), [AREA]);
        assert_eq!(
            tiles(&tall, 3),
            [
                rect(0, 0, 720, 600),
                rect(720, 0, 1200, 300),
                rect(720, 300, 1200, 600)
            ]
        );
        let three = Declared::parse("three", include_str!("layouts/three-columns.toml")).unwrap();
        // The first window in the middle, the second on its left once
        // there are three.
        let placed = tiles(&three, 4);
        assert_eq!(placed[0], rect(300, 0, 900, 600));
        assert_eq!(placed[1], rect(0, 0, 300, 600));
        assert_eq!(placed[2], rect(900, 0, 1200, 300));
        assert_eq!(placed[3], rect(900, 300, 1200, 600));
        // With two, the left column is not there.
        assert_eq!(
            tiles(&three, 2),
            [rect(0, 0, 800, 600), rect(800, 0, 1200, 600)]
        );
    }

    #[test]
    fn a_container_takes_as_many_windows_as_its_max() {
        let text = r#"
            [structure]
            kind = "vertical"
            [[structure.children]]
            kind = "horizontal"
            max = 2
            priority = 1
            [[structure.children]]
            kind = "horizontal"
            grow = 2
        "#;
        let layout = Declared::parse("rows", text).unwrap();
        let placed = tiles(&layout, 5);
        assert_eq!(placed[0], rect(0, 0, 600, 200));
        assert_eq!(placed[1], rect(600, 0, 1200, 200));
        assert_eq!(placed[2], rect(0, 200, 400, 600));
        assert_eq!(placed[4], rect(800, 200, 1200, 600));
    }

    #[test]
    fn a_layout_that_cannot_be_used_says_why() {
        let wrong = [
            "kind = \"leaf\"",
            "[structure]\nkind = \"leaf\"",
            "[structure]\nkind = \"vertical\"\nmax = 3",
            "[structure]\nkind = \"vertical\"\ncondition = { windows_at_least = 2 }",
            "[structure]\nkind = \"vertical\"\ngrow = 0",
            "[structure]\nkind = \"vertical\"\nside = 1",
            "[structure]\nkind = \"leaf\"\nmax = 1",
            "[structure]\nkind = \"diagonal\"",
            "[structure]\nkind = \"horizontal\"\n[[structure.children]]\nkind = \"leaf\"",
            "[structure]\nkind = \"horizontal\"\nmax = 2\n[[structure.children]]\nkind = \"vertical\"",
            "[structure]\nkind = \"horizontal\"\n[[structure.children]]\nkind = \"vertical\"\ncondition = { windows_at_least = 4, windows_at_most = 2 }",
        ];
        for text in wrong {
            assert!(Declared::parse("wrong", text).is_err(), "{text}");
        }
        let open = "name = \"Open\"\n[structure]\nkind = \"vertical\"";
        let layout = Declared::parse("Open", open).unwrap();
        assert_eq!(
            (layout.name.as_str(), layout.title.as_deref()),
            ("open", Some("Open"))
        );
    }

    #[test]
    fn a_layout_of_the_user_takes_the_place_of_a_bundled_one() {
        let own = Declared::parse("tall", "[structure]\nkind = \"horizontal\"").unwrap();
        let library = Library::with(vec![own.clone()]);
        assert_eq!(library.get("tall"), Some(&own));
        assert_eq!(library.names(), ["three-columns", "wide", "tall"]);
        assert!(library.get("wide").is_some_and(|layout| layout.bundled));
    }
}
