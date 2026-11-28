// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Schemes of keys: a TOML file of `[[key]]` entries, written from the
//! entries of the user and read back into them, adding to them or in
//! place of them.

use serde::Deserialize;

use crate::config::KeyConfig;
use crate::config::edit::key_table;
use crate::keys::written_press;

/// What a scheme file holds.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scheme {
    #[serde(default, rename = "key")]
    keys: Vec<KeyConfig>,
}

/// The name a scheme is offered to be saved as.
pub const FILE_NAME: &str = "tessera-keys.toml";

/// The entries the scheme `text` holds, or why it cannot be read.
pub fn read(text: &str) -> Result<Vec<KeyConfig>, String> {
    let scheme: Scheme = toml::from_str(text).map_err(|error| format!("{error}"))?;
    if let Some(key) = { scheme.keys.iter() }.find(|key| written_press(&key.press).is_none()) {
        return Err(format!("`{}` is no press", key.press));
    }
    Ok(scheme.keys)
}

/// `keys` as a scheme file, the comment `note` at its top.
pub fn written(keys: &[KeyConfig], note: &str) -> String {
    let mut document = toml_edit::DocumentMut::new();
    let mut entries = toml_edit::ArrayOfTables::new();
    for key in keys {
        entries.push(key_table(key));
    }
    if !entries.is_empty() {
        document.insert("key", toml_edit::Item::ArrayOfTables(entries));
    }
    let note: String = note.lines().map(|line| format!("# {line}\n")).collect();
    format!("{note}\n{document}")
}

/// The entries of the user once the scheme `scheme` is taken in: in place
/// of `own` with `replace`, and otherwise added to them, where an entry
/// of the scheme takes the place of one of `own` that presses the same
/// keys.
pub fn merged(own: &[KeyConfig], scheme: &[KeyConfig], replace: bool) -> Vec<KeyConfig> {
    if replace {
        return scheme.to_vec();
    }
    let pressed = |key: &KeyConfig| written_press(&key.press);
    let mut merged: Vec<KeyConfig> = { own.iter() }
        .filter(|key| !scheme.iter().any(|new| pressed(new) == pressed(key)))
        .cloned()
        .collect();
    merged.extend(scheme.iter().cloned());
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn popup(press: &str, name: &str) -> KeyConfig {
        KeyConfig {
            press: press.to_owned(),
            popup: Some(name.to_owned()),
            ..KeyConfig::default()
        }
    }

    #[test]
    fn a_scheme_is_written_and_read_back() {
        let keys = [
            popup("ctrl+alt+l", "launcher"),
            KeyConfig {
                press: "ctrl+alt+k, t".to_owned(),
                run: Some(vec!["wt.exe".to_owned()]),
                ..KeyConfig::default()
            },
        ];
        let text = written(&keys, "Keys of Tessera.\nTwo of them.");
        assert!(
            text.starts_with("# Keys of Tessera.\n# Two of them.\n"),
            "{text}"
        );
        assert_eq!(read(&text).unwrap(), keys);
        assert_eq!(read(&written(&[], "None.")).unwrap(), []);
        assert!(read("[[key]]\npress = \"k\"\npopup = \"x\"\n").is_err());
        assert!(read("[[keys]]\npress = \"ctrl+k\"\n").is_err());
    }

    #[test]
    fn a_scheme_adds_to_the_keys_or_takes_their_place() {
        let own = [
            popup("ctrl+alt+l", "launcher"),
            popup("ctrl+alt+m", "media"),
        ];
        let scheme = [
            popup("Ctrl+Alt+M", "calendar"),
            popup("ctrl+alt+n", "network"),
        ];
        let added = merged(&own, &scheme, false);
        let names: Vec<&str> = { added.iter() }
            .map(|key| key.popup.as_deref().unwrap())
            .collect();
        assert_eq!(names, ["launcher", "calendar", "network"]);
        assert_eq!(merged(&own, &scheme, true), scheme);
    }
}
