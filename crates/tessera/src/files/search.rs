// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A search below a folder: the files and folders at any depth whose
//! names hold what was written. Where Windows Search indexes the folder,
//! its index answers at once (with the names that have, for every word
//! written, a word beginning with it); elsewhere the folders are read one
//! after another on a thread of their own (finding the words anywhere in
//! a name), and what is found comes in batches while they are. Either
//! way, hidden files, and what is in hidden folders, are found only where
//! the pane shows them. Dropping the search stops it.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use async_channel::Sender;
use gpui::{AsyncApp, Context, Task, WeakEntity};

use super::model::{Entry, Shown};
use super::pane::FolderPane;
use crate::platform::{ExplorerSettings, NameRules, SearchIndex};

/// How many results a search gives at most.
pub const FOUND_MOST: usize = 5000;

/// How often what was found is handed over while folders are read.
const BATCH_INTERVAL: Duration = Duration::from_millis(100);

/// What a search hands over.
pub enum Found {
    /// More of what it found.
    Some(Vec<Entry>),
    /// It is done: through the index or by reading the folders, and
    /// whether it stopped at [`FOUND_MOST`].
    Done { indexed: bool, capped: bool },
}

/// How a search went, for the status bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchState {
    Running,
    Done { indexed: bool, capped: bool },
}

/// A search that runs, until it is done or dropped.
pub struct Search {
    stop: Arc<AtomicBool>,
    _receiving: Task<()>,
}

impl Drop for Search {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The words of `query`, in small letters, which a name must all hold.
pub fn words_of(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether `name` holds every one of `words`, in any case.
pub fn name_matches(name: &str, words: &[String]) -> bool {
    let name = name.to_lowercase();
    !words.is_empty() && words.iter().all(|word| name.contains(word.as_str()))
}

impl Search {
    /// Searches below `root` for `query`, handing what it finds to the
    /// pane `cx` is of. `follow` says whether names follow Explorer, and
    /// `shown` which hidden folders are read.
    pub fn start(
        root: PathBuf,
        query: String,
        follow: bool,
        shown: Shown,
        cx: &mut Context<FolderPane>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = async_channel::unbounded();
        let stopped = stop.clone();
        let spawned = thread::Builder::new()
            .name("tessera-file-search".to_owned())
            .spawn(move || search(&root, &query, follow, shown, &stopped, &sender));
        if let Err(error) = spawned {
            report!("the search cannot start: {error}");
        }
        let receiving = cx.spawn(
            async move |this: WeakEntity<FolderPane>, cx: &mut AsyncApp| {
                while let Ok(found) = receiver.recv().await {
                    if this.update(cx, |pane, cx| pane.found(found, cx)).is_err() {
                        break;
                    }
                }
            },
        );
        Self {
            stop,
            _receiving: receiving,
        }
    }
}

/// Searches below `root`, through the index where it holds the folder,
/// else by reading the folders, until done or `stop` is set.
fn search(
    root: &Path,
    query: &str,
    follow: bool,
    shown: Shown,
    stop: &AtomicBool,
    found: &Sender<Found>,
) {
    let mut rules = NameRules::new(ExplorerSettings::read(), follow);
    if SearchIndex::covers(root) {
        match SearchIndex::files_under(root, query, FOUND_MOST) {
            Ok(paths) => {
                let capped = paths.len() >= FOUND_MOST;
                let mut folders = ShownFolders::new(root, shown);
                let entries: Vec<Entry> = { paths.into_iter() }
                    .take_while(|_| !stop.load(Ordering::Relaxed))
                    .filter_map(|path| {
                        let metadata = fs::symlink_metadata(&path).ok()?;
                        let entry = Entry::of(path, &metadata, &mut rules);
                        let inside = entry
                            .path
                            .parent()
                            .is_none_or(|folder| folders.shows(folder, &mut rules));
                        (inside && shown.shows(&entry)).then_some(entry)
                    })
                    .collect();
                found.send_blocking(Found::Some(entries)).ok();
                let done = Found::Done {
                    indexed: true,
                    capped,
                };
                found.send_blocking(done).ok();
                return;
            }
            Err(error) => report!("Windows Search did not answer, the folders are read: {error:#}"),
        }
    }
    let capped = crawl(root, &words_of(query), &mut rules, shown, stop, found);
    let done = Found::Done {
        indexed: false,
        capped,
    };
    found.send_blocking(done).ok();
}

/// Which folders below a root the crawl would go into: those shown, in
/// folders shown. For the answers of the index, which hold what is in
/// hidden folders too.
struct ShownFolders<'a> {
    root: &'a Path,
    shown: Shown,
    /// Whether each folder asked about is shown, with those above it.
    known: HashMap<PathBuf, bool>,
}

impl<'a> ShownFolders<'a> {
    fn new(root: &'a Path, shown: Shown) -> Self {
        Self {
            root,
            shown,
            known: HashMap::new(),
        }
    }

    /// Whether `folder`, and every folder between it and the root, is
    /// shown; the root itself always is.
    fn shows(&mut self, folder: &Path, rules: &mut NameRules) -> bool {
        if folder == self.root || !folder.starts_with(self.root) {
            return true;
        }
        if let Some(&known) = self.known.get(folder) {
            return known;
        }
        let itself = fs::symlink_metadata(folder)
            .map(|metadata| {
                self.shown
                    .shows(&Entry::of(folder.to_owned(), &metadata, rules))
            })
            .unwrap_or(true);
        let above = folder
            .parent()
            .is_none_or(|parent| self.shows(parent, rules));
        let shows = itself && above;
        self.known.insert(folder.to_owned(), shows);
        shows
    }
}

/// Reads the folders below `root`, nearest first, handing over what
/// matches `words` every [`BATCH_INTERVAL`]. Links and junctions are not
/// followed, so that no folder is read twice. Says whether it stopped at
/// [`FOUND_MOST`].
fn crawl(
    root: &Path,
    words: &[String],
    rules: &mut NameRules,
    shown: Shown,
    stop: &AtomicBool,
    found: &Sender<Found>,
) -> bool {
    let mut folders = VecDeque::from([root.to_path_buf()]);
    let mut batch = Vec::new();
    let mut handed = Instant::now();
    let mut count = 0;
    while let Some(folder) = folders.pop_front() {
        let Ok(items) = fs::read_dir(&folder) else {
            continue;
        };
        for item in items.flatten() {
            if stop.load(Ordering::Relaxed) {
                return false;
            }
            let (Ok(kind), Ok(metadata)) = (item.file_type(), item.metadata()) else {
                continue;
            };
            let entry = Entry::of(item.path(), &metadata, rules);
            if !shown.shows(&entry) {
                continue;
            }
            if kind.is_dir() && !kind.is_symlink() {
                folders.push_back(entry.path.clone());
            }
            if name_matches(&entry.name, words) || name_matches(&entry.display, words) {
                batch.push(entry);
                count += 1;
                if count >= FOUND_MOST {
                    found.send_blocking(Found::Some(batch)).ok();
                    return true;
                }
            }
        }
        if !batch.is_empty() && handed.elapsed() >= BATCH_INTERVAL {
            if found
                .send_blocking(Found::Some(std::mem::take(&mut batch)))
                .is_err()
            {
                return false;
            }
            handed = Instant::now();
        }
    }
    found.send_blocking(Found::Some(batch)).ok();
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_holds_every_word_in_any_case() {
        let words = words_of("  Report  2026 ");
        assert_eq!(words, ["report", "2026"]);
        assert!(name_matches("Annual REPORT for 2026.pdf", &words));
        assert!(!name_matches("Report 2025.pdf", &words));
        assert!(!name_matches("anything", &[]));
    }
}
