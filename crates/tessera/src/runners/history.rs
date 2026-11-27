// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the user chose in the launcher, the runner and the Run dialog,
//! each with how often and when last, so that it is ranked higher the
//! next time: by its frecency, which grows with every choice and halves
//! every two weeks it is not chosen. It is kept in
//! `launcher-history.toml` in the configuration directory, read and
//! written on the background executor: nothing is written before the
//! file was read and merged, the newest history is written last, and a
//! file that cannot be read is not written over.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{App, AsyncApp, Global};
use serde::{Deserialize, Serialize};

use crate::config::{config_dir, read_user_file};

/// The file of the history, in the configuration directory.
const FILE: &str = "launcher-history.toml";

/// How many choices are kept at the most; the least used go first.
const KEPT: usize = 300;

/// After how many days without a choice the frecency of an entry is half
/// what it was.
const HALF_LIFE_DAYS: f64 = 14.;

/// The most an entry is ranked higher, in points of a score, and how
/// many points a frecency of one gives.
const MOST_BOOST: u16 = 150;
const POINTS: f64 = 25.;

/// The frecency something had when it was last chosen, and when that
/// was, in seconds since 1970.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Use {
    /// Each choice counts one when it is made and halves every
    /// [`HALF_LIFE_DAYS`] after it; this is their sum at `last`. Read
    /// also from `count`, as the file was first written.
    #[serde(alias = "count")]
    score: f64,
    last: i64,
}

impl Use {
    /// The choices of now.
    fn now(now: i64) -> Self {
        Self {
            score: 1.,
            last: now,
        }
    }

    /// How often it was chosen, each choice counting less the longer ago
    /// it was made, as of `now`.
    fn frecency(self, now: i64) -> f64 {
        let days = (now - self.last).max(0) as f64 / 86_400.;
        self.score * 0.5f64.powf(days / HALF_LIFE_DAYS)
    }

    /// Both uses as one: each decayed to the later of the two and added.
    fn merged(self, other: Self) -> Self {
        let last = self.last.max(other.last);
        Self {
            score: self.frecency(last) + other.frecency(last),
            last,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryFile {
    #[serde(default)]
    chosen: BTreeMap<String, Use>,
}

impl HistoryFile {
    /// Keeps only the [`KEPT`] entries of the highest frecency.
    fn prune(&mut self, now: i64) {
        if self.chosen.len() <= KEPT {
            return;
        }
        let mut ranked: Vec<(String, f64)> = { self.chosen.iter() }
            .map(|(key, chosen)| (key.clone(), chosen.frecency(now)))
            .collect();
        ranked.sort_by(|(_, one), (_, other)| other.total_cmp(one));
        for (key, _) in ranked.into_iter().skip(KEPT) {
            self.chosen.remove(&key);
        }
    }
}

/// Where the history stands with its file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Disk {
    /// It was not asked to be read.
    #[default]
    Unread,
    /// It is being read; what is chosen meanwhile is written once it was
    /// merged with what the file holds.
    Reading,
    /// It was read and merged, or was not there: the history is written
    /// after every choice.
    Read,
    /// It is there but could not be read, which was reported; it is not
    /// written over.
    Unusable,
}

/// The history, shared by every launcher, runner and Run dialog.
#[derive(Default)]
pub struct History {
    file: HistoryFile,
    disk: Disk,
    /// Whether something was chosen that is not written yet.
    unsaved: bool,
    /// How many times the history was sent to be written, which orders
    /// the writes.
    sent: u64,
}

impl Global for History {}

/// The seconds since 1970.
fn now() -> i64 {
    { SystemTime::now().duration_since(UNIX_EPOCH) }.map_or(0, |since| since.as_secs() as i64)
}

impl History {
    /// Has the file be read aside, once; until it is, nothing ranks
    /// higher. What is chosen meanwhile is added to what it holds, and
    /// written then.
    pub fn read(cx: &mut App) {
        let history = cx.default_global::<Self>();
        if history.disk != Disk::Unread {
            return;
        }
        history.disk = Disk::Reading;
        let read = cx.background_executor().spawn(async move { read_file() });
        cx.spawn(async move |cx: &mut AsyncApp| {
            let file = read.await;
            cx.update(|cx| {
                let history = cx.global_mut::<Self>();
                let Some(file) = file else {
                    history.disk = Disk::Unusable;
                    return;
                };
                for (key, chosen) in file.chosen {
                    let merged = match history.file.chosen.get(&key) {
                        Some(&known) => known.merged(chosen),
                        None => chosen,
                    };
                    history.file.chosen.insert(key, merged);
                }
                history.file.prune(now());
                history.disk = Disk::Read;
                if history.unsaved {
                    Self::save(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// How many points `key` is ranked higher by, up to [`MOST_BOOST`].
    pub fn boost(&self, key: &str) -> u16 {
        self.boost_at(key, now())
    }

    fn boost_at(&self, key: &str, now: i64) -> u16 {
        let Some(chosen) = self.file.chosen.get(key) else {
            return 0;
        };
        let points = (chosen.frecency(now) * POINTS).round();
        (points as u16).min(MOST_BOOST)
    }

    /// Remembers that `key` was chosen now, and writes the file aside
    /// once it was read. An empty key is not remembered.
    pub fn chose(key: &str, cx: &mut App) {
        if key.is_empty() {
            return;
        }
        let history = cx.default_global::<Self>();
        history.choose_at(key, now());
        history.unsaved = true;
        let disk = history.disk;
        match disk {
            Disk::Read => Self::save(cx),
            Disk::Unread => Self::read(cx),
            Disk::Reading | Disk::Unusable => {}
        }
    }

    fn choose_at(&mut self, key: &str, now: i64) {
        let merged = match self.file.chosen.get(key) {
            Some(&known) => known.merged(Use::now(now)),
            None => Use::now(now),
        };
        self.file.chosen.insert(key.to_owned(), merged);
        self.file.prune(now);
    }

    /// Forgets every choice, of every launcher, runner and Run dialog:
    /// the history is emptied and its file written empty aside, also one
    /// that could not be read. Asked while the file is being read, it
    /// waits until that is done, so that what the file held is not merged
    /// back afterwards.
    pub fn clear(cx: &mut App) {
        let history = cx.default_global::<Self>();
        if history.disk == Disk::Reading {
            cx.spawn(async move |cx: &mut AsyncApp| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                cx.update(Self::clear).ok();
            })
            .detach();
            return;
        }
        history.file = HistoryFile::default();
        history.disk = Disk::Read;
        Self::save(cx);
    }

    /// Sends the history as it is to be written aside.
    fn save(cx: &mut App) {
        let history = cx.global_mut::<Self>();
        history.unsaved = false;
        history.sent += 1;
        let (file, sent) = (history.file.clone(), history.sent);
        cx.background_executor()
            .spawn(async move { write(&file, sent) })
            .detach();
    }
}

/// The file of the history: empty where it is not there yet, `None`
/// where it is there but cannot be read.
fn read_file() -> Option<HistoryFile> {
    if let Some(file) = read_user_file::<HistoryFile>(FILE) {
        return Some(file);
    }
    let path = config_dir()?.join(FILE);
    match fs::metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Some(HistoryFile::default()),
        _ => None,
    }
}

/// The last of the histories sent that was written, by the order it was
/// sent in. Holding it keeps one write at a time.
static WRITTEN: Mutex<u64> = Mutex::new(0);

/// Writes `file`, the history as it was sent the `sent`th time, to the
/// configuration directory, through a file beside it that takes its
/// place, so that a reader never finds half of it. One is written at a
/// time, and a history older than the one written last is not written.
/// A failure is reported.
fn write(file: &HistoryFile, sent: u64) {
    let mut written = WRITTEN.lock().unwrap_or_else(PoisonError::into_inner);
    if *written >= sent {
        return;
    }
    *written = sent;
    let Some(directory) = config_dir() else {
        return;
    };
    let written = toml::to_string(file)
        .map_err(|error| error.to_string())
        .and_then(|text| {
            fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
            let fresh = directory.join(format!("{FILE}.new"));
            fs::write(&fresh, text).map_err(|error| error.to_string())?;
            fs::rename(&fresh, directory.join(FILE)).map_err(|error| error.to_string())
        });
    if let Err(error) = written {
        report!("the history of the launcher was not written: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn what_is_chosen_often_and_lately_ranks_higher() {
        let mut history = History::default();
        let now = 100 * DAY;
        history.choose_at("app:terminal", now);
        history.choose_at("app:terminal", now);
        history.choose_at("app:paint", now - 28 * DAY);
        assert_eq!(history.boost_at("app:terminal", now), 50);
        // Two half-lives ago one choice counts a quarter.
        assert_eq!(history.boost_at("app:paint", now), 6);
        assert_eq!(history.boost_at("app:never", now), 0);
        for _ in 0..20 {
            history.choose_at("app:terminal", now);
        }
        assert_eq!(history.boost_at("app:terminal", now), MOST_BOOST);
    }

    #[test]
    fn old_choices_stay_old_after_a_new_one() {
        let mut history = History::default();
        let now = 1000 * DAY;
        for _ in 0..100 {
            history.choose_at("app:paint", now - 365 * DAY);
        }
        history.choose_at("app:paint", now);
        // A year is 26 half-lives: the hundred old choices are nearly
        // nothing beside the new one.
        assert_eq!(history.boost_at("app:paint", now), 25);
    }

    #[test]
    fn uses_merged_are_decayed_to_the_later_one() {
        let later = 14 * DAY;
        let old = Use { score: 4., last: 0 };
        let new = Use::now(later);
        let merged = old.merged(new);
        assert_eq!(merged.last, later);
        assert!((merged.score - 3.).abs() < 1e-9);
        assert_eq!(new.merged(old), merged);
    }

    #[test]
    fn a_file_with_counts_is_read() {
        let read: HistoryFile =
            toml::from_str("[chosen.\"app:paint\"]\ncount = 3\nlast = 7\n").unwrap();
        assert_eq!(read.chosen["app:paint"].score, 3.);
    }

    #[test]
    fn the_least_used_are_forgotten_first() {
        let mut history = History::default();
        let now = 1000 * DAY;
        for index in 0..KEPT as i64 + 5 {
            history.choose_at(
                &format!("entry:{index}"),
                now - (KEPT as i64 + 5 - index) * DAY,
            );
        }
        assert_eq!(history.file.chosen.len(), KEPT);
        assert!(!history.file.chosen.contains_key("entry:0"));
        assert!(
            history
                .file
                .chosen
                .contains_key(&format!("entry:{}", KEPT + 4))
        );
    }

    #[test]
    fn the_file_is_written_as_it_is_read() {
        let mut history = History::default();
        history.choose_at("command:notepad todo.txt", 42);
        let text = toml::to_string(&history.file).unwrap();
        let read: HistoryFile = toml::from_str(&text).unwrap();
        assert_eq!(read, history.file);
    }
}
