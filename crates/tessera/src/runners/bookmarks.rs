// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The bookmarks of the browsers: those of every profile of Edge,
//! Chrome, Brave, Vivaldi and Chromium, from the `Bookmarks` file each
//! keeps, and those of every profile of Firefox, from its latest backup
//! of them (`bookmarkbackups\*.jsonlz4`, which Firefox writes once a
//! day). A bookmark is found by a part of its title or its address and
//! opened with the browser Windows opens addresses with.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use std::{env, fs};

use super::command::Command;
use super::json::Json;
use super::{Answer, AnswerAction, Kept, Query, Runner, fill, rank, ranked_score};

const ICON_BOOKMARK: &str = "\u{e728}";

/// The score of a bookmark whose title starts with what is typed, how
/// many letters the search waits for and how many are shown at most.
const SCORE: u16 = 480;
const LETTERS: usize = 2;
const SHOWN: usize = 10;

/// How long the bookmarks read last are used again.
const KEPT: Duration = Duration::from_secs(60);

/// The most bookmarks kept, against a file that holds a great many.
const MOST: usize = 20_000;

/// The browsers built on Chromium: their names, and the folders of their
/// profiles under the local application data of the user.
const CHROMIUM: [(&str, &str); 5] = [
    ("Edge", r"Microsoft\Edge\User Data"),
    ("Chrome", r"Google\Chrome\User Data"),
    ("Brave", r"BraveSoftware\Brave-Browser\User Data"),
    ("Vivaldi", r"Vivaldi\User Data"),
    ("Chromium", r"Chromium\User Data"),
];

/// The folder of the profiles of Firefox under the roaming application
/// data of the user.
const FIREFOX: &str = r"Mozilla\Firefox\Profiles";

/// What begins a file compressed the way Firefox compresses its backups.
const MOZ_LZ4: &[u8] = b"mozLz40\0";

/// A bookmark of a browser.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Bookmark {
    title: String,
    address: String,
    /// The browser it was found in.
    browser: &'static str,
}

/// The bookmarks under `node` of a `Bookmarks` file of Chromium, which
/// is a folder with `children` or a bookmark with a `url`.
fn chromium_node(node: &Json, browser: &'static str, into: &mut Vec<Bookmark>) {
    if into.len() >= MOST {
        return;
    }
    match node.text("type") {
        Some("url") => {
            if let (Some(title), Some(address)) = (node.text("name"), node.text("url")) {
                into.push(Bookmark {
                    title: title.to_owned(),
                    address: address.to_owned(),
                    browser,
                });
            }
        }
        _ => {
            for child in node.get("children").map_or(&[][..], Json::items) {
                chromium_node(child, browser, into);
            }
        }
    }
}

/// The bookmarks of the `Bookmarks` file `text` of Chromium.
fn chromium_bookmarks(text: &str, browser: &'static str) -> Vec<Bookmark> {
    let mut found = Vec::new();
    let roots = Json::parse(text);
    let roots = roots.as_ref().and_then(|file| file.get("roots"));
    for root in roots
        .and_then(Json::members)
        .into_iter()
        .flat_map(|roots| roots.values())
    {
        chromium_node(root, browser, &mut found);
    }
    found
}

/// The bookmarks under `node` of a backup of Firefox, which is a folder
/// with `children` or a bookmark with a `uri`; the addresses of Firefox
/// itself (`place:` queries) are left out.
fn firefox_node(node: &Json, into: &mut Vec<Bookmark>) {
    if into.len() >= MOST {
        return;
    }
    if let Some(address) = node.text("uri") {
        let title = node.text("title").filter(|title| !title.is_empty());
        if !address.starts_with("place:") {
            into.push(Bookmark {
                title: title.unwrap_or(address).to_owned(),
                address: address.to_owned(),
                browser: "Firefox",
            });
        }
        return;
    }
    for child in node.get("children").map_or(&[][..], Json::items) {
        firefox_node(child, into);
    }
}

/// The text of a file compressed as Firefox compresses its backups: the
/// mark, the length of the text and a block of LZ4.
fn moz_lz4(bytes: &[u8]) -> Option<String> {
    let rest = bytes.strip_prefix(MOZ_LZ4)?;
    let (length, block) = rest.split_first_chunk::<4>()?;
    let length = u32::from_le_bytes(*length) as usize;
    // A length beyond reason is not a backup of bookmarks.
    if length > 256 << 20 {
        return None;
    }
    let text = lz4_flex::block::decompress(block, length).ok()?;
    String::from_utf8(text).ok()
}

/// The bookmarks of the backup `bytes` of Firefox.
fn firefox_bookmarks(bytes: &[u8]) -> Vec<Bookmark> {
    let mut found = Vec::new();
    if let Some(root) = moz_lz4(bytes).as_deref().and_then(Json::parse) {
        firefox_node(&root, &mut found);
    }
    found
}

/// The subfolders of `folder`.
fn folders_in(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    { entries.flatten() }
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

/// The latest of the backups of bookmarks in the profile `profile` of
/// Firefox.
fn latest_backup(profile: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(profile.join("bookmarkbackups")).ok()?;
    { entries.flatten() }
        .filter(|entry| {
            let name = entry.file_name();
            name.to_string_lossy().ends_with(".jsonlz4")
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .max_by_key(|&(modified, _): &(SystemTime, _)| modified)
        .map(|(_, path)| path)
}

/// The bookmarks of every browser of the user.
fn read_all() -> Vec<Bookmark> {
    let mut found = Vec::new();
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        for (browser, folder) in CHROMIUM {
            for profile in folders_in(&Path::new(&local).join(folder)) {
                if let Ok(text) = fs::read_to_string(profile.join("Bookmarks")) {
                    found.extend(chromium_bookmarks(&text, browser));
                }
            }
        }
    }
    if let Some(roaming) = env::var_os("APPDATA") {
        for profile in folders_in(&Path::new(&roaming).join(FIREFOX)) {
            if let Some(bytes) = latest_backup(&profile).and_then(|file| fs::read(file).ok()) {
                found.extend(firefox_bookmarks(&bytes));
            }
        }
    }
    found.truncate(MOST);
    found
}

/// The bookmarks, read again once what was read is older than [`KEPT`].
fn bookmarks() -> Arc<Vec<Bookmark>> {
    static READ: Kept<Vec<Bookmark>> = Kept::new(KEPT);
    READ.get(read_all)
}

/// The bookmarks of `all` that `lower` finds, by the title or else the
/// address, the best first, each address once.
fn matching(all: &[Bookmark], lower: &str) -> Vec<(u8, Bookmark)> {
    let mut found: Vec<(u8, &Bookmark)> = { all.iter() }
        .filter_map(|bookmark| {
            let by_title = rank(&bookmark.title, lower);
            let by_address = || bookmark.address.to_lowercase().contains(lower).then_some(3);
            Some((by_title.or_else(by_address)?, bookmark))
        })
        .collect();
    // Stable, so of the same rank the first read comes first.
    found.sort_by_key(|&(rank, _)| rank);
    let mut seen: HashSet<&str> = HashSet::new();
    { found.into_iter() }
        .filter(|(_, bookmark)| seen.insert(&bookmark.address))
        .take(SHOWN)
        .map(|(rank, bookmark)| (rank, bookmark.clone()))
        .collect()
}

pub struct Bookmarks;

impl Runner for Bookmarks {
    fn id(&self) -> &'static str {
        "bookmarks"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        if query.letters() < LETTERS {
            return Vec::new();
        }
        { matching(&bookmarks(), query.lower()).into_iter() }
            .map(|(rank, bookmark)| {
                let command = Command {
                    target: bookmark.address.clone(),
                    arguments: None,
                    folder: false,
                    terminal: false,
                    typed: bookmark.address.clone(),
                };
                let detail = fill("{} · {}", &[&bookmark.address, bookmark.browser]);
                let key = format!("bookmark:{}", bookmark.address);
                let score = ranked_score(SCORE, rank);
                Answer::new(bookmark.title, detail, ICON_BOOKMARK, score, {
                    AnswerAction::Run(command)
                })
                .copying(bookmark.address)
                .keyed(key)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bookmarks_of_chromium_are_read_from_every_root() {
        let text = r#"{
            "checksum": "x", "version": 1,
            "roots": {
                "bookmark_bar": { "type": "folder", "name": "Bar", "children": [
                    { "type": "url", "name": "Rust", "url": "https://www.rust-lang.org/" },
                    { "type": "folder", "name": "Work", "children": [
                        { "type": "url", "name": "Tracker", "url": "https://tracker.example/" }
                    ] }
                ] },
                "other": { "type": "folder", "name": "Other", "children": [] },
                "synced": { "type": "folder", "name": "Mobile", "children": [
                    { "type": "url", "name": "News", "url": "https://news.example/" }
                ] }
            }
        }"#;
        let titles: Vec<String> = { chromium_bookmarks(text, "Edge").into_iter() }
            .map(|bookmark| bookmark.title)
            .collect();
        assert_eq!(titles, ["Rust", "Tracker", "News"]);
    }

    #[test]
    fn a_backup_of_firefox_is_unpacked_and_read() {
        let json = r#"{ "title": "", "type": "text/x-moz-place-container", "children": [
            { "title": "menu", "children": [
                { "title": "Mozilla", "uri": "https://www.mozilla.org/", "type": "text/x-moz-place" },
                { "title": "Recent", "uri": "place:sort=8", "type": "text/x-moz-place" },
                { "title": "", "uri": "https://untitled.example/" }
            ] }
        ] }"#;
        let mut file = MOZ_LZ4.to_vec();
        file.extend((json.len() as u32).to_le_bytes());
        file.extend(lz4_flex::block::compress(json.as_bytes()));
        let found = firefox_bookmarks(&file);
        let titles: Vec<&str> = found
            .iter()
            .map(|bookmark| bookmark.title.as_str())
            .collect();
        assert_eq!(titles, ["Mozilla", "https://untitled.example/"]);
        assert!(firefox_bookmarks(b"not a backup").is_empty());
        let mut wrong = file.clone();
        wrong[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(firefox_bookmarks(&wrong).is_empty());
    }

    #[test]
    fn bookmarks_are_found_by_title_then_by_address_each_once() {
        let bookmark = |title: &str, address: &str| Bookmark {
            title: title.to_owned(),
            address: address.to_owned(),
            browser: "Chrome",
        };
        let all = vec![
            bookmark("Docs of Rust", "https://doc.rust-lang.org/"),
            bookmark("Rust", "https://www.rust-lang.org/"),
            bookmark("Home", "https://rustacean.example/"),
            bookmark("Rust again", "https://www.rust-lang.org/"),
        ];
        let found = matching(&all, "rust");
        let titles: Vec<&str> = found.iter().map(|(_, b)| b.title.as_str()).collect();
        assert_eq!(titles, ["Rust", "Docs of Rust", "Home"]);
    }
}
