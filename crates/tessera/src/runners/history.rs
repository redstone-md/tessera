// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the user chose in the launcher, the runner and the Run dialog,
//! each with how often and when last, so that it is ranked higher the
//! next time: by its frecency, which grows with every choice and halves
//! every two weeks it is not chosen. It is kept in
//! `launcher-history.toml` in the configuration directory, read and
//! written on the background executor.

use std::collections::BTreeMap;
use std::fs;
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

/// How often something was chosen, and when last, in seconds since 1970.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Use {
    count: u32,
    last: i64,
}

impl Use {
    /// How often it was chosen, each choice counting less the longer ago
    /// the last one was, as of `now`.
    fn frecency(self, now: i64) -> f64 {
        let days = (now - self.last).max(0) as f64 / 86_400.;
        f64::from(self.count) * 0.5f64.powf(days / HALF_LIFE_DAYS)
    }

    /// Both uses as one.
    fn merged(self, other: Self) -> Self {
        Self {
            count: self.count.saturating_add(other.count),
            last: self.last.max(other.last),
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

/// The history, shared by every launcher, runner and Run dialog.
#[derive(Default)]
pub struct History {
    file: HistoryFile,
    /// Whether the file was asked to be read.
    read: bool,
}

impl Global for History {}

/// The seconds since 1970.
fn now() -> i64 {
    { SystemTime::now().duration_since(UNIX_EPOCH) }.map_or(0, |since| since.as_secs() as i64)
}

impl History {
    /// Has the file be read aside, once; until it is, nothing ranks
    /// higher. What is chosen meanwhile is added to what it holds.
    pub fn read(cx: &mut App) {
        let history = cx.default_global::<Self>();
        if history.read {
            return;
        }
        history.read = true;
        let read = cx
            .background_executor()
            .spawn(async move { read_user_file::<HistoryFile>(FILE).unwrap_or_default() });
        cx.spawn(async move |cx: &mut AsyncApp| {
            let file = read.await;
            cx.update(|cx| {
                let history = cx.global_mut::<Self>();
                for (key, chosen) in file.chosen {
                    let merged = match history.file.chosen.get(&key) {
                        Some(&known) => known.merged(chosen),
                        None => chosen,
                    };
                    history.file.chosen.insert(key, merged);
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

    /// Remembers that `key` was chosen now, and writes the file aside.
    /// An empty key is not remembered.
    pub fn chose(key: &str, cx: &mut App) {
        if key.is_empty() {
            return;
        }
        let history = cx.default_global::<Self>();
        history.choose_at(key, now());
        let file = history.file.clone();
        cx.background_executor()
            .spawn(async move { write(&file) })
            .detach();
    }

    fn choose_at(&mut self, key: &str, now: i64) {
        let chosen = Use {
            count: 1,
            last: now,
        };
        let merged = match self.file.chosen.get(key) {
            Some(&known) => known.merged(chosen),
            None => chosen,
        };
        self.file.chosen.insert(key.to_owned(), merged);
        self.file.prune(now);
    }
}

/// Writes `file` to the configuration directory, through a file beside
/// it that takes its place, so that a reader never finds half of it. A
/// failure is reported.
fn write(file: &HistoryFile) {
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
