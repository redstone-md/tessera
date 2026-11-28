// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The searches made last, which the bar under the search field offers
//! when the field gets the focus: the last [`KEPT`], newest first, in
//! `files-searches.toml` of the local data directory of Tessera. The file
//! is read and written away from the user interface; what it holds is
//! kept in memory once read.

use std::fs;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::files::session::state_dir;

/// The file of the searches, in the local data directory.
const FILE: &str = "files-searches.toml";

/// How many searches are kept.
pub const KEPT: usize = 20;

#[derive(Debug, Default, Deserialize, Serialize)]
struct Searches {
    #[serde(default)]
    history: Vec<String>,
}

/// The searches, once read.
static KNOWN: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// The searches made last, newest first, where they were read already.
pub fn recent() -> Option<Vec<String>> {
    KNOWN.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Reads the searches from the file, where they were not read yet. It
/// reads the disk: never call it on the thread of the user interface.
pub fn load() {
    let mut known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner);
    if known.is_none() {
        *known = Some(read());
    }
}

/// Notes that `text` was searched for, and writes the file. It writes the
/// disk: never call it on the thread of the user interface.
pub fn remember(text: &str) {
    change(|history| added(history, text));
}

/// Forgets every search.
pub fn clear() {
    change(|_| Vec::new());
}

/// Changes the searches kept and writes them.
fn change(change: impl FnOnce(Vec<String>) -> Vec<String>) {
    let mut known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner);
    let history = change(known.take().unwrap_or_else(read));
    write(&history);
    *known = Some(history);
}

/// `history` with `text` first, once, and at most [`KEPT`] searches.
fn added(mut history: Vec<String>, text: &str) -> Vec<String> {
    let text = super::aqs::shown(text);
    if text.is_empty() {
        return history;
    }
    history.retain(|other| !other.eq_ignore_ascii_case(&text));
    history.insert(0, text);
    history.truncate(KEPT);
    history
}

fn read() -> Vec<String> {
    let Some(path) = state_dir().map(|dir| dir.join(FILE)) else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    match toml::from_str::<Searches>(&text) {
        Ok(searches) => searches.history,
        Err(error) => {
            report!("{} cannot be read: {error}", path.display());
            Vec::new()
        }
    }
}

fn write(history: &[String]) {
    let Some(dir) = state_dir() else {
        return;
    };
    let searches = Searches {
        history: history.to_vec(),
    };
    let written = toml::to_string(&searches)
        .map_err(|error| error.to_string())
        .and_then(|text| {
            fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
            let draft = dir.join(format!("{FILE}.new"));
            fs::write(&draft, text).map_err(|error| error.to_string())?;
            fs::rename(&draft, dir.join(FILE)).map_err(|error| error.to_string())
        });
    if let Err(error) = written {
        report!("the searches made last are not kept: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_comes_first_once_and_the_oldest_go() {
        let history = added(vec!["a".to_owned(), "Plan".to_owned()], "  plan ");
        assert_eq!(history, ["plan", "a"]);
        assert_eq!(added(history.clone(), "   "), history);
        let full: Vec<String> = (0..KEPT).map(|n| n.to_string()).collect();
        let history = added(full, "new");
        assert_eq!(history.len(), KEPT);
        assert_eq!(history[0], "new");
        assert!(!history.contains(&(KEPT - 1).to_string()));
        let text = toml::to_string(&Searches { history }).unwrap();
        assert_eq!(toml::from_str::<Searches>(&text).unwrap().history[0], "new");
    }
}
