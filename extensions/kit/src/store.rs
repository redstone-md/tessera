// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! State kept across restarts, as lines of `key=value`.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::str::FromStr;

/// Named values, written to text for `save-state` and read back from what
/// `load-state` gives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record {
    values: BTreeMap<String, String>,
}

impl Record {
    /// What `save-state` stored last; empty when nothing was.
    pub fn load() -> Self {
        Self::parse(&crate::load_state())
    }

    /// Stores the record for [`Record::load`], logging why when it cannot.
    pub fn save(&self) {
        if let Err(error) = crate::save_state(&self.to_text()) {
            crate::log(&format!("the state was not saved: {error}"));
        }
    }

    /// Reads lines of `key=value`; other lines are skipped.
    pub fn parse(text: &str) -> Self {
        let values = text
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect();
        Self { values }
    }

    pub fn to_text(&self) -> String {
        let lines = self
            .values
            .iter()
            .map(|(key, value)| format!("{key}={value}\n"));
        lines.collect()
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// The value of `key` read as a `T`, unless it is missing or unreadable.
    pub fn get<T: FromStr>(&self, key: &str) -> Option<T> {
        self.text(key)?.parse().ok()
    }

    /// Sets `key` to `value`. A `=` in the key and line breaks anywhere
    /// would not read back, so they are replaced.
    pub fn set(&mut self, key: &str, value: impl Display) {
        let key = key.replace(['=', '\n', '\r'], "_");
        let value = value.to_string().replace(['\n', '\r'], " ");
        self.values.insert(key, value);
    }

    pub fn remove(&mut self, key: &str) {
        self.values.remove(key);
    }

    /// Keeps only the keys `keep` accepts.
    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.values.retain(|key, _| keep(key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_reads_back_what_it_wrote() {
        let mut record = Record::default();
        record.set("count", 3);
        record.set("name=odd", "two\nlines");
        let back = Record::parse(&record.to_text());
        assert_eq!(back, record);
        assert_eq!(back.get::<u32>("count"), Some(3));
        assert_eq!(back.text("name_odd"), Some("two lines"));
        assert_eq!(back.get::<u32>("missing"), None);
    }

    #[test]
    fn keys_can_be_dropped() {
        let mut record = Record::parse("a.1=x\nb=y\njunk\n");
        record.retain(|key| key.starts_with("a."));
        assert_eq!(record.to_text(), "a.1=x\n");
    }
}
