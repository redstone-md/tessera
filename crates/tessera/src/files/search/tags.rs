// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The tags used below a folder (`System.Keywords`), which the filters of
//! the search bar list and the sidebar may show as places: the files with
//! tags are found through the index of Windows Search where it holds the
//! folder, and else among the files below it whose kinds can carry tags;
//! the tags are read from their property stores, on a thread of their
//! own. What was found is kept per folder for the life of Tessera.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use async_channel::Receiver;

use crate::files::Location;
use crate::platform::{FileProperty, FileSearch, PropertyReader, SearchIndex};

/// The tags and how many files have each, the most used first.
pub type Tally = Vec<(String, usize)>;

/// How many files are read for their tags at most, how many items are
/// looked at outside the index, and for how long.
const READ_MOST: usize = 1000;
const LOOKED_MOST: usize = 20_000;
const LONGEST: Duration = Duration::from_secs(10);

/// The extensions of the kinds of files whose property handlers keep
/// tags, in small letters.
const TAGGED_KINDS: [&str; 22] = [
    "jpg", "jpeg", "jfif", "tif", "tiff", "heic", "heif", "jxr", "wdp", "docx", "docm", "xlsx",
    "xlsm", "pptx", "pptm", "mp3", "wma", "wmv", "m4a", "mp4", "m4v", "mov",
];

/// What was found below each folder.
static KNOWN: Mutex<Option<HashMap<PathBuf, Tally>>> = Mutex::new(None);

/// The tags found below `root` before, where they were looked for.
pub fn known(root: &Path) -> Option<Tally> {
    let known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner);
    known.as_ref()?.get(root).cloned()
}

/// Keeps `tally` as the tags below `root`.
pub fn keep(root: &Path, tally: Tally) {
    let mut known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner);
    known
        .get_or_insert_with(HashMap::new)
        .insert(root.to_owned(), tally);
}

/// Looks for the tags used below `root` on a thread of its own; the
/// receiver yields once. Hidden items are passed over unless `hidden`.
pub fn gather(root: PathBuf, hidden: bool) -> Receiver<Tally> {
    let (answer, answered) = async_channel::bounded(1);
    let spawned = thread::Builder::new()
        .name("tessera-file-tags".to_owned())
        .spawn(move || {
            let tally = tags_below(&root, hidden);
            keep(&root, tally.clone());
            answer.send_blocking(tally).ok();
        });
    if let Err(error) = spawned {
        report!("the tags are not looked for: {error}");
    }
    answered
}

/// The tags of the files below `root`, which the calling thread reads.
fn tags_below(root: &Path, hidden: bool) -> Tally {
    let reader = PropertyReader::new();
    let files = match SearchIndex::covers(root) {
        true => {
            let search = FileSearch {
                tagged: true,
                folder: Some(root.to_owned()),
                ..FileSearch::default()
            };
            SearchIndex::files_matching(&search, READ_MOST).unwrap_or_else(|error| {
                report!("Windows Search did not answer, the folders are read: {error:#}");
                taggable_below(root, hidden)
            })
        }
        false => taggable_below(root, hidden),
    };
    let started = Instant::now();
    let lists: Vec<Vec<String>> = { files.iter() }
        .take_while(|_| started.elapsed() < LONGEST)
        .filter_map(|path| reader.read(path, &[FileProperty::Tags]).pop().flatten())
        .map(|value| value.items())
        .collect();
    tally(lists)
}

/// Whether a file named `name` is of a kind that can carry tags.
pub fn may_have_tags(name: &str) -> bool {
    super::aqs::extension_of(name).is_some_and(|ext| TAGGED_KINDS.contains(&ext.as_str()))
}

/// The files below `root`, nearest first, of the kinds that can carry
/// tags, at most [`READ_MOST`] of them.
fn taggable_below(root: &Path, hidden: bool) -> Vec<PathBuf> {
    let mut folders = VecDeque::from([root.to_path_buf()]);
    let mut found = Vec::new();
    let mut looked = 0;
    let started = Instant::now();
    while let Some(folder) = folders.pop_front() {
        let Ok(items) = fs::read_dir(&folder) else {
            continue;
        };
        for item in items.flatten() {
            looked += 1;
            if looked > LOOKED_MOST || found.len() >= READ_MOST || started.elapsed() > LONGEST {
                return found;
            }
            let (Ok(kind), Ok(metadata)) = (item.file_type(), item.metadata()) else {
                continue;
            };
            if !hidden && crate::platform::is_hidden(&metadata) {
                continue;
            }
            if kind.is_dir() && !kind.is_symlink() {
                folders.push_back(item.path());
            } else if may_have_tags(&item.file_name().to_string_lossy()) {
                found.push(item.path());
            }
        }
    }
    found
}

/// The tags of `lists`, each the tags of a file, in the case first met,
/// with how many files have each: the most used first, then by name.
pub fn tally(lists: impl IntoIterator<Item = Vec<String>>) -> Tally {
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for list in lists {
        let mut seen: Vec<String> = Vec::new();
        for tag in list {
            let tag = tag.trim().to_owned();
            let key = tag.to_lowercase();
            if tag.is_empty() || seen.contains(&key) {
                continue;
            }
            seen.push(key.clone());
            match at.get(&key) {
                Some(&index) => counts[index].1 += 1,
                None => {
                    at.insert(key, counts.len());
                    counts.push((tag, 1));
                }
            }
        }
    }
    counts.sort_by(|a, b| {
        { b.1.cmp(&a.1) }.then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    counts
}

/// The tags used in the folder of the user, as places that search for
/// each there, for the sidebar of the file manager. What is not known yet
/// is looked for once, away from the user interface, and given by a later
/// call.
#[allow(
    dead_code,
    reason = "the sidebar of files-namespace shows them once both are merged"
)]
pub fn tag_places() -> Vec<(String, Location)> {
    let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) else {
        return Vec::new();
    };
    match known(&home) {
        Some(tally) => { tally.into_iter() }
            .map(|(tag, _)| {
                let query = super::aqs::with_filter("", super::aqs::Filter::Tag, Some(&tag));
                let location = Location::Search {
                    root: home.clone(),
                    query,
                };
                (tag, location)
            })
            .collect(),
        None => {
            // Kept empty meanwhile, so that it is looked for once.
            keep(&home, Vec::new());
            drop(gather(home, false));
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_counted_once_a_file_in_any_case() {
        let lists = vec![
            vec!["Sea".to_owned(), "sun".to_owned(), "sea".to_owned()],
            vec!["sea".to_owned(), " ".to_owned()],
            vec!["Alps".to_owned(), "Sun".to_owned()],
        ];
        assert_eq!(
            tally(lists),
            [
                ("Sea".to_owned(), 2),
                ("sun".to_owned(), 2),
                ("Alps".to_owned(), 1)
            ]
        );
        assert!(may_have_tags("IMG_1.JPG"));
        assert!(!may_have_tags("notes.txt"));
    }
}
