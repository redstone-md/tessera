// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the rules with `apply = "remember"` keep of their windows: where
//! the last window a rule named was and whether it was maximized, so that
//! the next window it names opens there. It is kept by rule in
//! `rules-state.toml` in the configuration directory, read once and
//! written two seconds after a change, both on the background executor; a
//! file that cannot be read is not written over.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use gpui::{App, AsyncApp, Global};
use serde::{Deserialize, Serialize};

use crate::config::{config_dir, read_user_file};
use crate::platform::ScreenRect;

/// The file, in the configuration directory.
const FILE: &str = "rules-state.toml";

/// How long after a change the file is written, so that a window that is
/// dragged is written once it rests.
const WRITE_AFTER: Duration = Duration::from_secs(2);

/// How many rules are remembered at the most; the file is for the rules
/// of the configuration, which are few.
const KEPT: usize = 200;

/// Where a window was, on the screen, and whether it was maximized: then
/// `frame` is where it goes back to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kept {
    pub frame: [i32; 4],
    #[serde(default)]
    pub maximized: bool,
}

impl Kept {
    pub fn new(frame: ScreenRect, maximized: bool) -> Self {
        Self {
            frame: [frame.left, frame.top, frame.right, frame.bottom],
            maximized,
        }
    }

    pub fn rect(self) -> ScreenRect {
        let [left, top, right, bottom] = self.frame;
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateFile {
    /// By the rule, written out as its names are.
    #[serde(default)]
    rule: BTreeMap<String, Kept>,
}

/// Where the memory stands with its file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Disk {
    #[default]
    Unread,
    Reading,
    Read,
    /// It is there but could not be read, which was reported.
    Unusable,
}

/// The frames the rules remember.
#[derive(Default)]
pub struct Memory {
    file: StateFile,
    disk: Disk,
    /// Whether a write is waiting for its time.
    waiting: bool,
    /// How many times the memory was sent to be written.
    sent: u64,
}

impl Global for Memory {}

impl Memory {
    /// Has the file be read aside, once.
    pub fn read(cx: &mut App) {
        let memory = cx.default_global::<Self>();
        if memory.disk != Disk::Unread {
            return;
        }
        memory.disk = Disk::Reading;
        let read = cx.background_executor().spawn(async { read_file() });
        cx.spawn(async move |cx: &mut AsyncApp| {
            let file = read.await;
            cx.update(|cx| {
                let memory = cx.global_mut::<Self>();
                let Some(file) = file else {
                    memory.disk = Disk::Unusable;
                    return;
                };
                // What was kept meanwhile is newer than the file.
                for (rule, kept) in file.rule {
                    memory.file.rule.entry(rule).or_insert(kept);
                }
                memory.disk = Disk::Read;
            })
            .ok();
        })
        .detach();
    }

    /// Where the last window `rule` named was.
    pub fn get(rule: &str, cx: &App) -> Option<Kept> {
        cx.try_global::<Self>()?.file.rule.get(rule).copied()
    }

    /// Remembers `kept` for `rule`, and has the file be written a moment
    /// later if that changed it.
    pub fn keep(rule: &str, kept: Kept, cx: &mut App) {
        let memory = cx.default_global::<Self>();
        if memory.file.rule.get(rule) == Some(&kept) {
            return;
        }
        if memory.file.rule.len() >= KEPT && !memory.file.rule.contains_key(rule) {
            memory.file.rule.pop_first();
        }
        memory.file.rule.insert(rule.to_owned(), kept);
        if memory.waiting || memory.disk == Disk::Unusable {
            return;
        }
        memory.waiting = true;
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor().timer(WRITE_AFTER).await;
            cx.update(Self::save).ok();
        })
        .detach();
    }

    /// Sends the memory as it is to be written aside, once the file was
    /// read; until then it waits a little more.
    fn save(cx: &mut App) {
        let memory = cx.global_mut::<Self>();
        match memory.disk {
            Disk::Read => {}
            Disk::Unusable => {
                memory.waiting = false;
                return;
            }
            Disk::Unread | Disk::Reading => {
                cx.spawn(async move |cx: &mut AsyncApp| {
                    cx.background_executor().timer(WRITE_AFTER).await;
                    cx.update(Self::save).ok();
                })
                .detach();
                return;
            }
        }
        memory.waiting = false;
        memory.sent += 1;
        let (file, sent) = (memory.file.clone(), memory.sent);
        cx.background_executor()
            .spawn(async move { write(&file, sent) })
            .detach();
    }
}

/// The file: empty where it is not there yet, `None` where it is there
/// but cannot be read.
fn read_file() -> Option<StateFile> {
    if let Some(file) = read_user_file::<StateFile>(FILE) {
        return Some(file);
    }
    let path = config_dir()?.join(FILE);
    match fs::metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Some(StateFile::default()),
        _ => None,
    }
}

/// The last of the memories sent that was written, by the order it was
/// sent in. Holding it keeps one write at a time.
static WRITTEN: Mutex<u64> = Mutex::new(0);

/// Writes `file`, the memory as it was sent the `sent`th time, through a
/// file beside it that takes its place, so that a reader never finds half
/// of it; an older memory than the one written last is not written. A
/// failure is reported.
fn write(file: &StateFile, sent: u64) {
    let mut written = WRITTEN.lock().unwrap_or_else(PoisonError::into_inner);
    if *written >= sent {
        return;
    }
    *written = sent;
    let Some(directory) = config_dir() else {
        return;
    };
    let result = toml::to_string(file)
        .map_err(|error| error.to_string())
        .and_then(|text| {
            fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
            let fresh = directory.join(format!("{FILE}.new"));
            fs::write(&fresh, text).map_err(|error| error.to_string())?;
            fs::rename(&fresh, directory.join(FILE)).map_err(|error| error.to_string())
        });
    if let Err(error) = result {
        report!("where windows were is not remembered in {FILE}: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_kept_as_it_was() {
        let frame = ScreenRect {
            left: -10,
            top: 20,
            right: 800,
            bottom: 600,
        };
        let kept = Kept::new(frame, true);
        assert_eq!(kept.rect(), frame);
        let mut file = StateFile::default();
        file.rule.insert("exe glob \"a.exe\"".to_owned(), kept);
        let text = toml::to_string(&file).unwrap();
        assert_eq!(toml::from_str::<StateFile>(&text).unwrap(), file);
        assert!(toml::from_str::<StateFile>("[rule.x]\nframe = [0, 0, 1, 1]\nsize = 3").is_err());
    }
}
