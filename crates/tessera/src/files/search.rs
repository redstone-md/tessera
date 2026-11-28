// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A search below a folder, written in the Advanced Query Syntax of
//! Explorer ([`aqs`]): words the names hold, and filters of the kind, the
//! date of the last change, the size, the authors, the tags, the
//! extension and what files hold. Where Windows Search indexes the
//! folder, its index answers at once (with the names that have, for every
//! word written, a word beginning with it); elsewhere the folders are
//! read one after another on a thread of their own (finding the words
//! anywhere in a name), what files of text hold is read up to
//! [`content::LARGEST`], properties are read from the files' property
//! stores, and what is found comes in batches while they are. Either way,
//! hidden files, and what is in hidden folders, are found only where the
//! pane shows them. Dropping the search stops it.
//!
//! What a search learns beside what it finds (the files whose contents
//! could not be searched, and the tags of what it found) is kept as
//! [`SearchNotes`] for the bar under the search field.

pub mod aqs;
pub mod content;
pub mod history;
pub mod saved;
pub mod tags;

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use async_channel::Sender;
use chrono::{DateTime, Local, NaiveDate};
use gpui::{AsyncApp, Context, EntityId, Task, WeakEntity};

use self::aqs::Query;
use self::tags::Tally;
use super::model::{Entry, Shown};
use super::pane::FolderPane;
use crate::platform::{ExplorerSettings, FileProperty, NameRules, PropertyReader, SearchIndex};

/// How many results a search gives at most.
pub const FOUND_MOST: usize = 5000;

/// How often what was found is handed over while folders are read.
const BATCH_INTERVAL: Duration = Duration::from_millis(100);

/// How many of the files found are read for their tags at most, where
/// the search did not read them anyway.
const TAGS_READ_MOST: usize = 300;

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

/// What a search learned beside what it found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchNotes {
    /// The search it is of.
    pub search: u64,
    /// How many files might have matched but were not searched for what
    /// they hold, being no text Tessera reads, or too large.
    pub unsearched: usize,
    /// The tags of what it found, where it read them.
    pub tags: Tally,
}

/// The notes of the searches of each pane, by the pane.
static NOTES: Mutex<Vec<(EntityId, SearchNotes)>> = Mutex::new(Vec::new());

/// A number no other search of this process has.
static NEXT_SEARCH: AtomicU64 = AtomicU64::new(1);

/// The notes of the search the pane `pane` runs or ran last.
pub fn notes_of(pane: EntityId) -> Option<SearchNotes> {
    let notes = NOTES.lock().unwrap_or_else(PoisonError::into_inner);
    { notes.iter() }
        .find(|(of, _)| *of == pane)
        .map(|(_, notes)| notes.clone())
}

/// Keeps `notes` as those of the pane `pane`.
fn keep_notes(pane: EntityId, notes: SearchNotes) {
    let mut kept = NOTES.lock().unwrap_or_else(PoisonError::into_inner);
    kept.retain(|(of, _)| *of != pane);
    kept.push((pane, notes));
}

/// A search that runs, until it is done or dropped.
pub struct Search {
    stop: Arc<AtomicBool>,
    pane: EntityId,
    id: u64,
    _receiving: Task<()>,
}

impl Drop for Search {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let mut notes = NOTES.lock().unwrap_or_else(PoisonError::into_inner);
        notes.retain(|(pane, notes)| !(*pane == self.pane && notes.search == self.id));
    }
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
        let (pane, id) = (cx.entity_id(), NEXT_SEARCH.fetch_add(1, Ordering::Relaxed));
        let spawned = thread::Builder::new()
            .name("tessera-file-search".to_owned())
            .spawn(move || {
                history::remember(&query);
                let asked = Asked {
                    root: &root,
                    query: Query::parse(&query),
                    today: Local::now().date_naive(),
                    shown,
                    stop: &stopped,
                };
                let (done, notes) = search(&asked, follow, &sender);
                // The notes are kept before the pane hears that the search
                // is done, so that the bar it draws then has them.
                if !stopped.load(Ordering::Relaxed) {
                    keep_notes(
                        pane,
                        SearchNotes {
                            search: id,
                            ..notes
                        },
                    );
                }
                sender.send_blocking(done).ok();
            });
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
            pane,
            id,
            _receiving: receiving,
        }
    }
}

/// What a search asks, and where.
struct Asked<'a> {
    root: &'a Path,
    query: Query,
    today: NaiveDate,
    shown: Shown,
    stop: &'a AtomicBool,
}

/// The local day `time` is on.
fn day_of(time: Option<SystemTime>) -> Option<NaiveDate> {
    time.map(|time| DateTime::<Local>::from(time).date_naive())
}

/// Searches below the root, through the index where it holds the folder,
/// else by reading the folders, until done or stopped; says how it ended
/// and what it learned.
fn search(asked: &Asked, follow: bool, found: &Sender<Found>) -> (Found, SearchNotes) {
    let mut rules = NameRules::new(ExplorerSettings::read(), follow);
    let notes = SearchNotes::default();
    if asked.query.is_empty() {
        let done = Found::Done {
            indexed: false,
            capped: false,
        };
        return (done, notes);
    }
    let root = asked.root;
    if SearchIndex::covers(root) {
        let question = asked.query.index_search(root, asked.today);
        match SearchIndex::files_matching(&question, FOUND_MOST) {
            Ok(paths) => {
                let capped = paths.len() >= FOUND_MOST;
                let mut folders = ShownFolders::new(root, asked.shown);
                let entries: Vec<Entry> = { paths.into_iter() }
                    .take_while(|_| !asked.stop.load(Ordering::Relaxed))
                    .filter_map(|path| {
                        let metadata = fs::symlink_metadata(&path).ok()?;
                        let entry = Entry::of(path, &metadata, &mut rules);
                        let inside = entry
                            .path
                            .parent()
                            .is_none_or(|folder| folders.shows(folder, &mut rules));
                        (inside && asked.shown.shows(&entry)).then_some(entry)
                    })
                    .collect();
                found.send_blocking(Found::Some(entries)).ok();
                let done = Found::Done {
                    indexed: true,
                    capped,
                };
                return (done, notes);
            }
            Err(error) => report!("Windows Search did not answer, the folders are read: {error:#}"),
        }
    }
    let mut crawl = Crawl::new(asked);
    let capped = crawl.run(&mut rules, found);
    let done = Found::Done {
        indexed: false,
        capped,
    };
    (done, crawl.notes())
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

/// The reading of the folders below the root, and of the files in them
/// where the query asks what they hold or their properties.
struct Crawl<'a> {
    asked: &'a Asked<'a>,
    /// Reads properties on this thread, once one is to be read.
    reader: Option<PropertyReader>,
    unsearched: usize,
    /// The tags of the files whose properties were read.
    tag_lists: Vec<Vec<String>>,
    /// The files found that may have tags, whose tags were not read.
    taggable: Vec<PathBuf>,
}

impl<'a> Crawl<'a> {
    fn new(asked: &'a Asked<'a>) -> Self {
        Self {
            asked,
            reader: None,
            unsearched: 0,
            tag_lists: Vec::new(),
            taggable: Vec::new(),
        }
    }

    fn reader(&mut self) -> &PropertyReader {
        self.reader.get_or_insert_with(PropertyReader::new)
    }

    /// Reads the folders below the root, nearest first, handing over what
    /// matches every [`BATCH_INTERVAL`]. Links and junctions are not
    /// followed, so that no folder is read twice. Says whether it stopped
    /// at [`FOUND_MOST`].
    fn run(&mut self, rules: &mut NameRules, found: &Sender<Found>) -> bool {
        let asked = self.asked;
        let mut folders = VecDeque::from([asked.root.to_path_buf()]);
        let mut batch = Vec::new();
        let mut handed = Instant::now();
        let mut count = 0;
        while let Some(folder) = folders.pop_front() {
            let Ok(items) = fs::read_dir(&folder) else {
                continue;
            };
            for item in items.flatten() {
                if asked.stop.load(Ordering::Relaxed) {
                    return false;
                }
                let (Ok(kind), Ok(metadata)) = (item.file_type(), item.metadata()) else {
                    continue;
                };
                let entry = Entry::of(item.path(), &metadata, rules);
                if !asked.shown.shows(&entry) {
                    continue;
                }
                if kind.is_dir() && !kind.is_symlink() {
                    folders.push_back(entry.path.clone());
                }
                if !self.matches(&entry) {
                    continue;
                }
                let untagged = !entry.folder && !asked.query.reads_properties();
                if untagged
                    && self.taggable.len() < TAGS_READ_MOST
                    && tags::may_have_tags(&entry.name)
                {
                    self.taggable.push(entry.path.clone());
                }
                batch.push(entry);
                count += 1;
                if count >= FOUND_MOST {
                    found.send_blocking(Found::Some(batch)).ok();
                    self.read_tags();
                    return true;
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
        self.read_tags();
        false
    }

    /// Whether `entry` is found: its name, kind, size and date first, then
    /// its properties and what it holds where they are asked for.
    fn matches(&mut self, entry: &Entry) -> bool {
        let asked = self.asked;
        let query = &asked.query;
        let day = day_of(entry.modified);
        if !query.fits(&entry.name, entry.folder, entry.size, day, asked.today) {
            return false;
        }
        let by_name = query.name_matches(&[&entry.name, &entry.display]);
        if !by_name && !query.contents {
            return false;
        }
        if query.reads_properties() {
            let wanted = [FileProperty::Authors, FileProperty::Tags];
            let values = self.reader().read(&entry.path, &wanted);
            let items = |at: usize| {
                { values.get(at).cloned().flatten() }
                    .map(|value| value.items())
                    .unwrap_or_default()
            };
            let (authors, tags) = (items(0), items(1));
            let fits = query.properties_fit(&authors, &tags);
            self.tag_lists.push(tags);
            if !fits {
                return false;
            }
        }
        // The words a name did not hold are looked for in what the file
        // holds, beside the words only what it holds may have.
        let mut held = query.content_words.clone();
        if !by_name {
            held.extend(query.words.iter().cloned());
        }
        if held.is_empty() {
            return true;
        }
        if entry.folder {
            return false;
        }
        match content::holds(&entry.path, &held) {
            Some(holds) => holds,
            None => {
                self.unsearched += 1;
                false
            }
        }
    }

    /// Reads the tags of the files found that may have them, where the
    /// search did not read them, until stopped.
    fn read_tags(&mut self) {
        let taggable = std::mem::take(&mut self.taggable);
        let stop = self.asked.stop;
        for path in taggable {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let value = self.reader().read(&path, &[FileProperty::Tags]).pop();
            if let Some(value) = value.flatten() {
                self.tag_lists.push(value.items());
            }
        }
    }

    fn notes(self) -> SearchNotes {
        SearchNotes {
            search: 0,
            unsearched: self.unsearched,
            tags: tags::tally(self.tag_lists),
        }
    }
}
