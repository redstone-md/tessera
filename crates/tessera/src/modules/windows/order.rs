// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The order the buttons of applications that are not pinned were dragged
//! into, for `sort = "manual"`: by the application of each button, as the
//! dock groups windows, the same for every dock, and kept across starts in
//! `dock-order.toml` of the configuration directory. The file is
//! Tessera's own and is read and written away from the thread of GPUI.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::config_dir;

/// The file the order is kept in.
const FILE: &str = "dock-order.toml";

/// The most applications whose place is kept; those dragged longest ago
/// are forgotten first.
const MOST: usize = 200;

/// What the file holds.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OrderFile {
    /// The groups of the buttons, as the dock tells applications apart,
    /// in the order they stand.
    #[serde(default)]
    order: Vec<String>,
}

fn path() -> Option<PathBuf> {
    Some(config_dir()?.join(FILE))
}

/// The order kept in the file; empty where there is none or it cannot be
/// read, which is reported.
pub fn load() -> Vec<String> {
    let Some(path) = path() else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    match toml::from_str::<OrderFile>(&text) {
        Ok(file) => file.order,
        Err(error) => {
            report!("`{}` is not read: {error}", path.display());
            Vec::new()
        }
    }
}

/// Keeps `order` in the file, written whole beside it first so that a
/// failure leaves the old file. A failure is reported.
pub fn save(order: &[String]) {
    let Some(path) = path() else {
        return;
    };
    let file = OrderFile {
        order: order.to_vec(),
    };
    let text = match toml::to_string(&file) {
        Ok(text) => text,
        Err(error) => return report!("the order of the dock is not written: {error}"),
    };
    let draft = path.with_extension("toml.new");
    let written = { path.parent().map_or(Ok(()), fs::create_dir_all) }
        .and_then(|()| fs::write(&draft, text))
        .and_then(|()| fs::rename(&draft, &path));
    if let Err(error) = written {
        report!("`{}` is not written: {error}", path.display());
    }
}

/// The order after the buttons of `shown` were put in that order: they
/// come first, then the others that were kept, as they were.
pub fn reordered(order: &[String], shown: &[String]) -> Vec<String> {
    let mut next: Vec<String> = shown.to_vec();
    next.extend(order.iter().filter(|group| !shown.contains(group)).cloned());
    next.truncate(MOST);
    next
}

/// The place of `group` in `order`.
pub fn place(order: &[String], group: &str) -> Option<usize> {
    order.iter().position(|kept| kept == group)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn the_buttons_dragged_go_first_and_the_rest_keep_their_order() {
        let order = list(&["a", "b", "c", "d"]);
        assert_eq!(
            reordered(&order, &list(&["c", "a"])),
            list(&["c", "a", "b", "d"])
        );
        assert_eq!(reordered(&[], &list(&["x"])), list(&["x"]));
        assert_eq!(place(&order, "c"), Some(2));
        assert_eq!(place(&order, "z"), None);
        let long: Vec<String> = (0..300).map(|index| index.to_string()).collect();
        assert_eq!(reordered(&long, &list(&["new"])).len(), MOST);
    }

    #[test]
    fn the_file_reads_back_what_was_written() {
        let file = OrderFile {
            order: list(&["program:c:\\apps\\a.exe", "app:vendor.app!main"]),
        };
        let text = toml::to_string(&file).unwrap();
        let read: OrderFile = toml::from_str(&text).unwrap();
        assert_eq!(read.order, file.order);
    }
}
