// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The documents opened lately, from the shortcuts Windows keeps for
//! them in the Recent folder (`shell:recent`): found by their names, and
//! the latest shown while nothing is typed where the widget asks for it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use crate::platform::recent_folder;

use super::command::Command;
use super::{Answer, AnswerAction, Query, Runner, rank, ranked_score};

const ICON_RECENT: &str = "\u{e81c}";

/// How many documents are shown while nothing is typed, and how many
/// that match what is.
const SHOWN_EMPTY: usize = 8;
const SHOWN: usize = 10;

/// How many letters the search waits for.
const LETTERS: usize = 2;

/// The scores of a document shown while nothing is typed, and of one
/// whose name starts with what is.
const SCORE_EMPTY: u16 = 600;
const SCORE_FOUND: u16 = 450;

/// For how long the folder read last is used again.
const KEPT: Duration = Duration::from_secs(20);

/// The most shortcuts read from the folder.
const MOST: usize = 500;

/// A document opened lately.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RecentFile {
    /// The name of the document, as its shortcut is named.
    name: String,
    /// The shortcut, which opens it.
    link: PathBuf,
    modified: SystemTime,
}

/// The shortcuts in `folder`, the latest first, at most [`MOST`].
fn read_folder(folder: &Path) -> Vec<RecentFile> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut files: Vec<RecentFile> = entries
        .flatten()
        .filter_map(|entry| {
            let link = entry.path();
            let is_link = { link.extension() }.is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"));
            let metadata = entry.metadata().ok()?;
            if !is_link || !metadata.is_file() {
                return None;
            }
            let name = link.file_stem()?.to_string_lossy().into_owned();
            let modified = metadata.modified().ok()?;
            Some(RecentFile {
                name,
                link,
                modified,
            })
        })
        .collect();
    files.sort_by_key(|file| std::cmp::Reverse(file.modified));
    files.truncate(MOST);
    files
}

/// The documents opened lately, read again once what was read last is
/// older than [`KEPT`].
fn recent_files() -> Vec<RecentFile> {
    static READ: Mutex<Option<(Instant, Vec<RecentFile>)>> = Mutex::new(None);
    let mut read = READ.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((at, files)) = read.as_ref()
        && at.elapsed() < KEPT
    {
        return files.clone();
    }
    let files = recent_folder()
        .map(|folder| read_folder(&folder))
        .unwrap_or_default();
    *read = Some((Instant::now(), files.clone()));
    files
}

pub struct Recent {
    /// Whether the latest are shown while nothing is typed.
    when_empty: bool,
}

impl Recent {
    pub fn new(when_empty: bool) -> Self {
        Self { when_empty }
    }
}

impl Runner for Recent {
    fn id(&self) -> &'static str {
        "recent"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        let empty = query.text().is_empty();
        if (empty && !self.when_empty) || (!empty && query.letters() < LETTERS) {
            return Vec::new();
        }
        let files = recent_files();
        let found: Vec<(RecentFile, u16)> = match empty {
            true => { files.into_iter().take(SHOWN_EMPTY) }
                .enumerate()
                .map(|(index, file)| (file, SCORE_EMPTY.saturating_sub(index as u16)))
                .collect(),
            false => {
                let mut found: Vec<(RecentFile, u8)> = { files.into_iter() }
                    .filter_map(|file| Some((rank(&file.name, query.lower())?, file)))
                    .map(|(rank, file)| (file, rank))
                    .collect();
                // The latest first among the equally good.
                found.sort_by_key(|&(_, rank)| rank);
                { found.into_iter().take(SHOWN) }
                    .map(|(file, rank)| (file, ranked_score(SCORE_FOUND, rank)))
                    .collect()
            }
        };
        { found.into_iter() }
            .map(|(file, score)| {
                let target = file.link.to_string_lossy().into_owned();
                let command = Command {
                    typed: target.clone(),
                    target,
                    arguments: None,
                    folder: false,
                    terminal: false,
                };
                let detail = query.say("Opened lately");
                Answer::new(
                    file.name,
                    detail,
                    ICON_RECENT,
                    score,
                    AnswerAction::Run(command),
                )
                .with_icon(file.link)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shortcuts_of_the_folder_are_read_latest_first() {
        let folder = std::env::temp_dir().join(format!("tessera-recent-{}", std::process::id()));
        fs::create_dir_all(folder.join("AutomaticDestinations")).unwrap();
        fs::write(folder.join("old.txt.lnk"), "").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(folder.join("new.docx.lnk"), "").unwrap();
        fs::write(folder.join("desktop.ini"), "").unwrap();
        let files = read_folder(&folder);
        fs::remove_dir_all(&folder).unwrap();
        let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, ["new.docx", "old.txt"]);
    }
}
