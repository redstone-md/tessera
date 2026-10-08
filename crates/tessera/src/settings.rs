// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::de::{MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Deserializer, Serialize};

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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LauncherDisplayMode {
    #[default]
    Windowed,
    Fullscreen,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LauncherPreferences {
    display_mode: LauncherDisplayMode,
    favorites: Vec<String>,
}

/// Serde's struct derive also accepts positional arrays; persisted groups require objects.
fn deserialize_launcher_preferences<'de, D>(
    deserializer: D,
) -> Result<LauncherPreferences, D::Error>
where
    D: Deserializer<'de>,
{
    struct LauncherObject;

    impl<'de> Visitor<'de> for LauncherObject {
        type Value = LauncherPreferences;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a launcher preferences object")
        }

        fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            LauncherPreferences::deserialize(MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_map(LauncherObject)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preferences {
    schema_version: u32,
    theme: Theme,
    compact: bool,
    dock_edge: DockEdge,
    pinned_apps: Vec<String>,
    #[serde(deserialize_with = "deserialize_launcher_preferences")]
    launcher: LauncherPreferences,
}

/// Only the discriminator is read here; the selected typed record below
/// rejects unknown and duplicate fields without passing through JSON Value.
#[derive(Deserialize)]
struct SchemaVersion {
    schema_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyPreferencesV1 {
    #[serde(rename = "schema_version")]
    _schema_version: u32,
    theme: Theme,
    compact: bool,
    #[serde(default)]
    dock_edge: DockEdge,
    #[serde(default)]
    pinned_apps: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyPreferencesV2 {
    #[serde(rename = "schema_version")]
    _schema_version: u32,
    theme: Theme,
    compact: bool,
    #[serde(default)]
    dock_edge: DockEdge,
    #[serde(default)]
    pinned_apps: Vec<String>,
    launcher_favorites: Vec<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self::new(Theme::System, false)
    }
}

impl Preferences {
    pub(crate) fn new(theme: Theme, compact: bool) -> Self {
        Self {
            schema_version: 3,
            theme,
            compact,
            dock_edge: DockEdge::default(),
            pinned_apps: Vec::new(),
            launcher: LauncherPreferences::default(),
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

    pub(crate) fn with_launcher_favorites(mut self, favorites: Vec<String>) -> Self {
        self.launcher.favorites = favorites;
        self
    }

    pub(crate) fn launcher_favorites(&self) -> &[String] {
        &self.launcher.favorites
    }

    pub(crate) fn with_launcher_display_mode(mut self, mode: LauncherDisplayMode) -> Self {
        self.launcher.display_mode = mode;
        self
    }

    pub(crate) fn launcher_display_mode(&self) -> LauncherDisplayMode {
        self.launcher.display_mode
    }

    /// Migrates legacy data in memory only. No caller writes until explicit save.
    fn decode(contents: &[u8]) -> io::Result<Self> {
        let schema: SchemaVersion = serde_json::from_slice(contents)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let preferences = match schema.schema_version {
            1 => {
                let legacy: LegacyPreferencesV1 = serde_json::from_slice(contents)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                Self::new(legacy.theme, legacy.compact)
                    .with_dock(legacy.dock_edge, legacy.pinned_apps)
            }
            2 => {
                let legacy: LegacyPreferencesV2 = serde_json::from_slice(contents)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                Self::new(legacy.theme, legacy.compact)
                    .with_dock(legacy.dock_edge, legacy.pinned_apps)
                    .with_launcher_favorites(legacy.launcher_favorites)
            }
            3 => serde_json::from_slice(contents)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unsupported settings schema version",
                ));
            }
        };
        preferences.validate()?;
        Ok(preferences)
    }

    fn validate(&self) -> io::Result<()> {
        if self.schema_version != 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unsupported settings schema version",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        if self.pinned_apps.len() > MAX_PINS
            || self
                .pinned_apps
                .iter()
                .any(|id| !valid_application_id(id) || !seen.insert(id.to_lowercase()))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Pinned application identities are invalid or exceed the limit",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        if self
            .launcher
            .favorites
            .iter()
            .any(|id| !valid_application_id(id) || !seen.insert(id.as_str()))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Launcher favorite identities are invalid or duplicated",
            ));
        }
        Ok(())
    }
}

fn valid_application_id(id: &str) -> bool {
    !id.is_empty()
        && id.encode_utf16().count() <= MAX_APPLICATION_ID_UNITS
        && !id.chars().any(char::is_control)
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
        Preferences::decode(&contents)
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
            r#"{"schema_version":0,"theme":"system","compact":false}"#,
            r#"{"schema_version":3,"theme":"system","compact":false}"#,
            r#"{"schema_version":4,"theme":"system","compact":false}"#,
            r#"{"schema_version":1,"theme":"dark","compact":false,"launcher_favorites":[]}"#,
            r#"{"schema_version":2,"theme":"dark","compact":false,"launcher_favorites":[],"extra":0}"#,
            r#"{"schema_version":1,"schema_version":1,"theme":"dark","compact":false}"#,
            r#"{"schema_version":1,"theme":"dark","theme":"light","compact":false}"#,
            r#"{"schema_version":2,"theme":"dark","compact":false,"launcher_favorites":[],"launcher_favorites":[]}"#,
            r#"{"schema_version":2,"theme":"dark","compact":false,"launcher_favorites":["same","same"]}"#,
            r#"{"schema_version":1,"theme":"neon","compact":false}"#,
            r#"{"schema_version":1,"theme":"dark","compact":false,"autostart":true}"#,
            r#"{"schema_version":1,"theme":"dark","compact":false,"launcher":{"display_mode":"fullscreen","favorites":[]}}"#,
            r#"{"schema_version":2,"theme":"dark","compact":false,"launcher_favorites":[],"launcher":{"display_mode":"fullscreen","favorites":[]}}"#,
            r#"{"schema_version":2,"schema_version":2,"theme":"dark","compact":false,"launcher_favorites":[]}"#,
            r#"{"schema_version":2,"theme":"dark","theme":"light","compact":false,"launcher_favorites":[]}"#,
        ] {
            fs::write(&path, invalid).unwrap();
            assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
            assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
        }
        fs::write(&path, vec![b' '; MAX_SETTINGS_BYTES as usize + 1]).unwrap();
        assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn legacy_v1_migration_is_read_only_until_complete_v3_explicit_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let store = SettingsStore::new(path.clone());
        for (original, edge, pins) in [
            (
                r#"{"schema_version":1,"theme":"dark","compact":true}"#,
                DockEdge::Bottom,
                Vec::<String>::new(),
            ),
            (
                r#"{"schema_version":1,"theme":"dark","compact":true,"dock_edge":"left","pinned_apps":["dock-only"]}"#,
                DockEdge::Left,
                vec!["dock-only".to_owned()],
            ),
        ] {
            fs::write(&path, original).unwrap();
            let migrated = store.load().unwrap();
            assert_eq!(migrated.dock_edge(), edge);
            assert_eq!(
                migrated,
                Preferences::new(Theme::Dark, true).with_dock(edge, pins)
            );
            assert!(migrated.launcher_favorites().is_empty());
            assert_eq!(
                migrated.launcher_display_mode(),
                LauncherDisplayMode::Windowed
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), original);

            let complete = migrated
                .with_launcher_favorites(vec!["favorite-b".into(), "Favorite-A".into()])
                .with_launcher_display_mode(LauncherDisplayMode::Fullscreen);
            store.save(&complete).unwrap();
            let persisted = fs::read_to_string(&path).unwrap();
            assert!(persisted.contains("\"schema_version\": 3"));
            assert!(persisted.contains("\"launcher\""));
            assert!(persisted.contains("\"display_mode\": \"fullscreen\""));
            assert!(persisted.contains("\"favorites\""));
            assert!(!persisted.contains("\"launcher_favorites\""));
            assert_eq!(store.load().unwrap(), complete);
        }
    }

    #[test]
    fn legacy_v2_migration_retains_exact_favorites_and_only_explicit_save_writes_v3() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let store = SettingsStore::new(path.clone());
        let mut favorites = vec!["exact".to_owned(), "EXACT".into(), "missing-app".into()];
        favorites.extend((0..80).map(|index| format!("favorite-{index}")));
        let original = format!(
            r#"{{"schema_version":2,"theme":"dark","compact":true,"dock_edge":"left","pinned_apps":["dock-only"],"launcher_favorites":{}}}"#,
            serde_json::to_string(&favorites).unwrap()
        );
        fs::write(&path, &original).unwrap();
        let migrated = store.load().unwrap();
        assert_eq!(migrated.launcher_favorites(), favorites);
        assert_eq!(
            migrated.launcher_display_mode(),
            LauncherDisplayMode::Windowed
        );
        assert_eq!(migrated.theme(), Theme::Dark);
        assert!(migrated.compact());
        assert_eq!(migrated.dock_edge(), DockEdge::Left);
        assert_eq!(migrated.pinned_apps(), ["dock-only"]);
        assert_eq!(fs::read_to_string(&path).unwrap(), original);

        let complete = migrated.with_launcher_display_mode(LauncherDisplayMode::Fullscreen);
        store.save(&complete).unwrap();
        let persisted = fs::read_to_string(&path).unwrap();
        assert!(persisted.contains("\"schema_version\": 3"));
        assert!(!persisted.contains("\"launcher_favorites\""));
        assert_eq!(store.load().unwrap(), complete);

        let minimal_v2 = br#"{"schema_version":2,"theme":"light","compact":false,"launcher_favorites":["missing"]}"#;
        let minimal = Preferences::decode(minimal_v2).unwrap();
        assert_eq!(minimal.dock_edge(), DockEdge::Bottom);
        assert!(minimal.pinned_apps().is_empty());
        assert_eq!(minimal.launcher_favorites(), ["missing"]);
        assert_eq!(
            minimal.launcher_display_mode(),
            LauncherDisplayMode::Windowed
        );
    }

    #[test]
    fn v3_requires_complete_strict_launcher_group_and_rejects_duplicate_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let store = SettingsStore::new(path.clone());
        let record = |launcher: &str| {
            format!(
                r#"{{"schema_version":3,"theme":"dark","compact":true,"dock_edge":"top","pinned_apps":["dock-only"],"launcher":{launcher}}}"#
            )
        };
        for launcher in [
            "null",
            "[]",
            r#"["fullscreen",["A"]]"#,
            r#"["windowed",[]]"#,
            r#""fullscreen""#,
            "{}",
            r#"{"display_mode":"windowed"}"#,
            r#"{"favorites":[]}"#,
            r#"{"display_mode":null,"favorites":[]}"#,
            r#"{"display_mode":true,"favorites":[]}"#,
            r#"{"display_mode":"normal","favorites":[]}"#,
            r#"{"display_mode":"Fullscreen","favorites":[]}"#,
            r#"{"display_mode":"windowed","favorites":null}"#,
            r#"{"display_mode":"windowed","favorites":[],"extra":true}"#,
            r#"{"display_mode":"windowed","display_mode":"fullscreen","favorites":[]}"#,
            r#"{"display_mode":"windowed","favorites":[],"favorites":[]}"#,
            r#"{"display_mode":"fullscreen","favorites":["same","same"]}"#,
        ] {
            let invalid = record(launcher);
            fs::write(&path, &invalid).unwrap();
            assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
            assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
        }
        let valid = record(r#"{"display_mode":"windowed","favorites":[]}"#);
        for invalid in [
            valid.replace(r#""dock_edge":"top","#, ""),
            valid.replace(r#""pinned_apps":["dock-only"],"#, ""),
            valid.replace(
                r#","launcher":{"display_mode":"windowed","favorites":[]}"#,
                "",
            ),
            valid.replace(
                r#""schema_version":3"#,
                r#""schema_version":3,"schema_version":3"#,
            ),
            valid.replace(r#""launcher":"#, r#""launcher_favorites":[],"launcher":"#),
            valid.replace(
                r#""launcher":"#,
                r#""launcher":{"display_mode":"windowed","favorites":[]},"launcher":"#,
            ),
        ] {
            fs::write(&path, &invalid).unwrap();
            assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
            assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
        }
        assert_eq!(
            Preferences::decode(valid.as_bytes()).unwrap(),
            Preferences::new(Theme::Dark, true).with_dock(DockEdge::Top, vec!["dock-only".into()])
        );
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
            vec![String::new()],
            vec!["\u{1f600}".repeat(513)],
            (0..MAX_PINS + 1).map(|index| index.to_string()).collect(),
            (0..MAX_PINS)
                .map(|index| format!("{index}{}", "a".repeat(1000)))
                .collect(),
        ] {
            assert!(
                store
                    .save(&Preferences::default().with_dock(DockEdge::Top, pins.clone()))
                    .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), original);
            let encoded_pins = serde_json::to_string(&pins).unwrap();
            for (version, launcher_fields) in [
                (1, ""),
                (2, r#","launcher_favorites":[]"#),
                (
                    3,
                    r#","launcher":{"display_mode":"fullscreen","favorites":[]}"#,
                ),
            ] {
                let invalid = format!(
                    r#"{{"schema_version":{version},"theme":"dark","compact":true,"dock_edge":"top","pinned_apps":{encoded_pins}{launcher_fields}}}"#
                );
                fs::write(&path, &invalid).unwrap();
                assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
                assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
            }
            fs::write(&path, &original).unwrap();
        }
    }

    #[test]
    fn favorites_round_trip_exact_case_order_and_more_than_dock_capacity() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(directory.path().join("settings.json"));
        let mut favorites = vec!["exact".into(), "EXACT".into(), "\u{1f600}".repeat(512)];
        favorites.extend((0..100).map(|index| format!("favorite-{index}")));
        let preferences = Preferences::new(Theme::Light, true)
            .with_dock(DockEdge::Right, vec!["dock-only".into()])
            .with_launcher_display_mode(LauncherDisplayMode::Fullscreen)
            .with_launcher_favorites(favorites.clone());
        store.save(&preferences).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded, preferences);
        assert_eq!(loaded.launcher_favorites(), favorites);
        assert_eq!(loaded.pinned_apps(), ["dock-only"]);
        assert_eq!(
            loaded.launcher_display_mode(),
            LauncherDisplayMode::Fullscreen
        );
        let changed = loaded
            .with_launcher_favorites(vec!["replacement".into()])
            .with_dock(DockEdge::Left, vec!["new-dock".into()]);
        store.save(&changed).unwrap();
        assert_eq!(
            store.load().unwrap().launcher_display_mode(),
            LauncherDisplayMode::Fullscreen
        );
    }

    #[test]
    fn invalid_favorites_reject_complete_load_and_save_without_replacing_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let store = SettingsStore::new(path.clone());
        let original = r#"{"schema_version":1,"theme":"dark","compact":true}"#;
        fs::write(&path, original).unwrap();
        for favorites in [
            vec![String::new()],
            vec!["control\u{0007}".into()],
            vec!["\u{1f600}".repeat(513)],
            vec!["duplicate".into(), "duplicate".into()],
        ] {
            let encoded_favorites = serde_json::to_string(&favorites).unwrap();
            let preferences = Preferences::default()
                .with_launcher_display_mode(LauncherDisplayMode::Fullscreen)
                .with_launcher_favorites(favorites);
            assert_eq!(
                store.save(&preferences).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), original);
            let invalid = serde_json::to_vec(&preferences).unwrap();
            fs::write(&path, &invalid).unwrap();
            assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
            assert_eq!(fs::read(&path).unwrap(), invalid);
            fs::write(&path, original).unwrap();
            let invalid_v2 = format!(
                r#"{{"schema_version":2,"theme":"dark","compact":true,"launcher_favorites":{encoded_favorites}}}"#
            );
            fs::write(&path, &invalid_v2).unwrap();
            assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
            assert_eq!(fs::read_to_string(&path).unwrap(), invalid_v2);
            fs::write(&path, original).unwrap();
        }
    }

    #[test]
    fn whole_record_budget_accepts_exact_limit_and_rejects_tail_without_loss() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let store = SettingsStore::new(path.clone());
        let mut favorites: Vec<_> = (0..16)
            .map(|index| format!("{index:02}{}", "a".repeat(958)))
            .collect();
        favorites.push("tail".into());
        let mut preferences = Preferences::default().with_launcher_favorites(favorites);
        let size = serde_json::to_vec_pretty(&preferences).unwrap().len() + 1;
        let padding = MAX_SETTINGS_BYTES as usize - size;
        preferences
            .launcher
            .favorites
            .last_mut()
            .unwrap()
            .push_str(&"z".repeat(padding));
        preferences.validate().unwrap();
        store.save(&preferences).unwrap();
        let original = fs::read(&path).unwrap();
        assert_eq!(original.len(), MAX_SETTINGS_BYTES as usize);
        assert_eq!(store.load().unwrap(), preferences);
        assert_eq!(
            store
                .save(
                    &preferences
                        .clone()
                        .with_launcher_display_mode(LauncherDisplayMode::Fullscreen)
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        preferences.launcher.favorites.last_mut().unwrap().push('z');
        assert_eq!(
            store.save(&preferences).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(fs::read(&path).unwrap(), original);

        let pins: Vec<_> = (0..9)
            .map(|index| format!("pin-{index}{}", "p".repeat(995)))
            .collect();
        let favorites: Vec<_> = (0..9)
            .map(|index| format!("favorite-{index}{}", "f".repeat(989)))
            .collect();
        let combined = Preferences::default()
            .with_dock(DockEdge::Bottom, pins)
            .with_launcher_favorites(favorites);
        combined.validate().unwrap();
        assert!(store.save(&combined).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);

        let mut too_large = original;
        too_large.push(b' ');
        fs::write(&path, &too_large).unwrap();
        assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&path).unwrap(), too_large);
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
