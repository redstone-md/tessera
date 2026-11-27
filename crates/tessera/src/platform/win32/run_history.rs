// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The commands of the Run dialog the user typed last, which Windows
//! keeps under `RunMRU`: Tessera's Run dialog reads and writes them the
//! way Explorer's does, so both offer the same history.
//!
//! The key holds up to 26 values named `a` to `z`, each a command
//! followed by `\1`, and `MRUList`, the letters of the values from the
//! newest to the oldest. A command run again keeps its letter and moves
//! to the front; a new one takes a free letter, or that of the oldest
//! once all are taken.

#![allow(unsafe_code)]

use anyhow::{Result, bail};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
use windows::core::PCWSTR;

use super::registry::registry_text;

/// The key of the history, under the user's hive.
const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\RunMRU";

/// The value that orders the letters.
const ORDER: &str = "MRUList";

/// What Explorer writes after each command.
const SUFFIX: &str = r"\1";

/// The longest value read, in UTF-16 units.
const VALUE_LENGTH: usize = 4096;

/// The letters the values are named with.
fn letters() -> std::ops::RangeInclusive<char> {
    'a'..='z'
}

/// The history of the Run dialog.
pub struct RunHistory;

impl RunHistory {
    /// The commands, the newest first. Empty where there are none or the
    /// key cannot be read.
    pub fn read() -> Vec<String> {
        let (values, order) = Self::read_raw();
        ordered(&values, &order)
    }

    /// Puts `command` at the front of the history, as running it from
    /// the Run dialog of Explorer does.
    pub fn add(command: &str) -> Result<()> {
        let command = command.trim();
        if command.is_empty() {
            return Ok(());
        }
        let (values, order) = Self::read_raw();
        let (letter, order) = after_adding(&values, &order, command);
        write(&letter.to_string(), &format!("{command}{SUFFIX}"))?;
        write(ORDER, &order)
    }

    /// The values `a` to `z` that are there, as they are written, and the
    /// order.
    fn read_raw() -> (Vec<(char, String)>, String) {
        let values =
            { letters().filter_map(|letter| Some((letter, read(&letter.to_string())?))) }.collect();
        (values, read(ORDER).unwrap_or_default())
    }
}

/// The text value `name` of the history.
fn read(name: &str) -> Option<String> {
    registry_text(HKEY_CURRENT_USER, KEY, Some(name), VALUE_LENGTH)
}

/// Writes the text value `name` of the history; the key is made if it
/// is not there.
fn write(name: &str, value: &str) -> Result<()> {
    let key: Vec<u16> = KEY.encode_utf16().chain([0]).collect();
    let name_wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let data: Vec<u16> = value.encode_utf16().chain([0]).collect();
    // SAFETY: the names and the data are NUL-terminated and outlive the
    // call, and the size given is that of the data in bytes.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(name_wide.as_ptr()),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * size_of::<u16>()) as u32,
        )
    };
    if status.is_err() {
        bail!("the Run history was not written ({name}): {status:?}");
    }
    Ok(())
}

/// `command` without the `\1` Explorer writes after it.
fn plain(command: &str) -> &str {
    command.strip_suffix(SUFFIX).unwrap_or(command)
}

/// The commands of `values` in the order of the letters of `order`, the
/// newest first; letters without a value, and values twice, are passed
/// over.
fn ordered(values: &[(char, String)], order: &str) -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    for letter in order.chars() {
        let Some((_, value)) = values.iter().find(|(known, _)| *known == letter) else {
            continue;
        };
        let command = plain(value).to_owned();
        let seen = commands
            .iter()
            .any(|known| known.eq_ignore_ascii_case(&command));
        if !command.is_empty() && !seen {
            commands.push(command);
        }
    }
    commands
}

/// The letter `command` is to be written under, and the order after it,
/// given the `values` and the `order` there are: the letter it has
/// already, whatever the case it was typed in, or a free one, or that
/// of the oldest command; that letter comes first and the others keep
/// their order.
fn after_adding(values: &[(char, String)], order: &str, command: &str) -> (char, String) {
    let kept: Vec<char> = {
        let mut kept: Vec<char> = Vec::new();
        for letter in order.chars() {
            let known = values.iter().any(|(value, _)| *value == letter);
            if known && !kept.contains(&letter) {
                kept.push(letter);
            }
        }
        kept
    };
    let same = values
        .iter()
        .find(|(_, value)| plain(value).eq_ignore_ascii_case(command))
        .map(|&(letter, _)| letter);
    let free = || letters().find(|letter| !kept.contains(letter));
    let letter = same
        .or_else(free)
        .or_else(|| kept.last().copied())
        .unwrap_or('a');
    let mut order = String::from(letter);
    order.extend(kept.into_iter().filter(|&known| known != letter));
    (letter, order)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(char, &str)]) -> Vec<(char, String)> {
        { pairs.iter() }
            .map(|&(letter, value)| (letter, value.to_owned()))
            .collect()
    }

    #[test]
    fn the_history_is_read_newest_first_without_the_suffix() {
        let written = values(&[('a', r"notepad\1"), ('b', r"cmd\1"), ('c', r"regedit\1")]);
        assert_eq!(ordered(&written, "cab"), ["regedit", "notepad", "cmd"]);
        // A letter without a value is passed over, and so is a repeat.
        assert_eq!(ordered(&written, "dcaa"), ["regedit", "notepad"]);
        assert!(ordered(&written, "").is_empty());
    }

    #[test]
    fn a_command_run_again_keeps_its_letter_and_comes_first() {
        let written = values(&[('a', r"notepad\1"), ('b', r"cmd\1"), ('c', r"regedit\1")]);
        assert_eq!(
            after_adding(&written, "cab", "CMD"),
            ('b', "bca".to_owned())
        );
        assert_eq!(
            after_adding(&written, "cab", "calc"),
            ('d', "dcab".to_owned())
        );
        assert_eq!(after_adding(&[], "", "calc"), ('a', "a".to_owned()));
    }

    #[test]
    fn a_full_history_gives_the_letter_of_the_oldest() {
        let full: Vec<(char, String)> = letters()
            .map(|letter| (letter, format!(r"{letter}\1")))
            .collect();
        let order: String = letters().rev().collect();
        let (letter, after) = after_adding(&full, &order, "winver");
        assert_eq!(letter, 'a');
        assert!(after.starts_with("az"));
        assert_eq!(after.len(), 26);
    }
}
