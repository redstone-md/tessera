// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const MAX_SETTINGS_BYTES: u64 = 16 * 1024;
const MAX_PINS: usize = 32;
const MAX_APPLICATION_ID_UNITS: usize = 1024;

// The persisted schema is independent of the toolkit's theme enum.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DockEdge {
    #[default]
    Bottom,
    Top,
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preferences {
    schema_version: u32,
    theme: Theme,
    compact: bool,
    #[serde(default)]
    dock_edge: DockEdge,
    #[serde(default)]
    pinned_apps: Vec<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self::new(Theme::System, false)
    }
}

impl Preferences {
    pub(crate) fn new(theme: Theme, compact: bool) -> Self {
        Self {
            schema_version: 1,
            theme,
            compact,
            dock_edge: DockEdge::default(),
            pinned_apps: Vec::new(),
        }
    }

    pub(crate) fn theme(&self) -> Theme {
        self.theme
    }

    pub(crate) fn compact(&self) -> bool {
        self.compact
    }

    pub(crate) fn with_dock(mut self, edge: DockEdge, pins: Vec<String>) -> Self {
        self.dock_edge = edge;
        self.pinned_apps = pins;
        self
    }

    pub(crate) fn dock_edge(&self) -> DockEdge {
        self.dock_edge
    }

    pub(crate) fn pinned_apps(&self) -> &[String] {
        &self.pinned_apps
    }

    fn validate(&self) -> io::Result<()> {
        if self.schema_version != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unsupported settings schema version",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        if self.pinned_apps.len() > MAX_PINS
            || self.pinned_apps.iter().any(|id| {
                id.is_empty()
                    || id.encode_utf16().count() > MAX_APPLICATION_ID_UNITS
                    || id.chars().any(char::is_control)
                    || !seen.insert(id.to_lowercase())
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Pinned application identities are invalid or exceed the limit",
            ));
        }
        Ok(())
    }
}

pub(crate) struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[cfg(windows)]
    pub(crate) fn for_current_user() -> io::Result<Self> {
        let directory = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| io::Error::other("LOCALAPPDATA is unavailable or not absolute"))?;
        Ok(Self::new(directory.join("Tessera").join("settings.json")))
    }

    /// Missing preferences mean defaults. Invalid/future data is never silently overwritten.
    pub(crate) fn load(&self) -> io::Result<Preferences> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Preferences::default());
            }
            Err(error) => return Err(error),
        };
        let mut contents = Vec::new();
        file.take(MAX_SETTINGS_BYTES + 1)
            .read_to_end(&mut contents)?;
        if contents.len() as u64 > MAX_SETTINGS_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Settings file exceeds 16 KiB",
            ));
        }
        let preferences: Preferences = serde_json::from_slice(&contents)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        preferences.validate()?;
        Ok(preferences)
    }

    /// Explicit saves replace one small file atomically; no startup or shell settings are stored.
    pub(crate) fn save(&self, preferences: &Preferences) -> io::Result<()> {
        preferences.validate()?;
        let contents = serde_json::to_vec_pretty(preferences).map_err(io::Error::other)?;
        if contents.len() as u64 + 1 > MAX_SETTINGS_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Settings file would exceed 16 KiB",
            ));
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("Settings path has no parent"))?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&contents)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_do_not_create_files_and_explicit_saves_replace_previous_preferences() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Tessera/settings.json");
        let store = SettingsStore::new(path.clone());
        assert_eq!(store.load().unwrap(), Preferences::default());
        assert!(!path.exists());
        let dark = Preferences::new(Theme::Dark, true);
        store.save(&dark).unwrap();
        assert_eq!(store.load().unwrap(), dark);
        assert_eq!(store.load().unwrap().theme(), Theme::Dark);
        assert!(store.load().unwrap().compact());
        store.save(&Preferences::new(Theme::Light, false)).unwrap();
        assert_eq!(store.load().unwrap(), Preferences::new(Theme::Light, false));
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn invalid_unknown_and_oversized_settings_are_preserved_for_explicit_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let store = SettingsStore::new(path.clone());
        for invalid in [
            "not json",
            r#"{"schema_version":2,"theme":"system","compact":false}"#,
            r#"{"schema_version":1,"theme":"neon","compact":false}"#,
            r#"{"schema_version":1,"theme":"dark","compact":false,"autostart":true}"#,
        ] {
            fs::write(&path, invalid).unwrap();
            assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
            assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
        }
        fs::write(&path, vec![b' '; MAX_SETTINGS_BYTES as usize + 1]).unwrap();
        assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn legacy_preferences_load_defaults_and_dock_pins_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let store = SettingsStore::new(path.clone());
        fs::write(
            &path,
            r#"{"schema_version":1,"theme":"dark","compact":true}"#,
        )
        .unwrap();
        let legacy = store.load().unwrap();
        assert_eq!(legacy.dock_edge(), DockEdge::Bottom);
        assert!(legacy.pinned_apps().is_empty());
        let dock = legacy.with_dock(
            DockEdge::Left,
            vec!["shell:AppsFolder\\Application".to_owned()],
        );
        store.save(&dock).unwrap();
        assert_eq!(store.load().unwrap(), dock);
    }

    #[test]
    fn invalid_pins_and_oversized_serialization_cannot_replace_preferences() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let store = SettingsStore::new(path.clone());
        store.save(&Preferences::default()).unwrap();
        let original = fs::read(&path).unwrap();
        for pins in [
            vec!["duplicate".to_owned(), "DUPLICATE".to_owned()],
            vec!["nul\0identity".to_owned()],
            (0..MAX_PINS + 1).map(|index| index.to_string()).collect(),
            (0..MAX_PINS)
                .map(|index| format!("{index}{}", "a".repeat(1000)))
                .collect(),
        ] {
            assert!(
                store
                    .save(&Preferences::default().with_dock(DockEdge::Top, pins))
                    .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), original);
        }
    }

    #[test]
    fn unwritable_location_reports_error_without_truncating_existing_data() {
        let directory = tempfile::tempdir().unwrap();
        let blocker = directory.path().join("not-a-directory");
        fs::write(&blocker, b"keep").unwrap();
        let store = SettingsStore::new(blocker.join("settings.json"));
        assert!(store.save(&Preferences::default()).is_err());
        assert_eq!(fs::read(&blocker).unwrap(), b"keep");
    }
}
