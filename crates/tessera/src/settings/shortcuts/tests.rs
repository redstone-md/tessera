// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::settings::{
    DockEdge, GeneralPreferences, LauncherDisplayMode, Preferences, SettingsStore, StartOfWeek,
    Theme,
};
use std::fs;
use std::io;
use tessera_system::shortcuts::ShortcutAction;

fn record(shortcuts: &str) -> String {
    format!(
        r#"{{"schema_version":5,"theme":"dark","compact":true,"dock_edge":"right","pinned_apps":["dock-only"],"launcher":{{"display_mode":"fullscreen","favorites":["missing","exact","EXACT"]}},"general":{{"start_of_week":"saturday"}},"dock":{{"media_enabled":true}},"shortcuts":{shortcuts}}}"#
    )
}

fn chord(key: &str, modifiers: &str) -> String {
    format!(r#"{{"key":{key},"modifiers":{modifiers}}}"#)
}

fn shortcut(chord: &str) -> String {
    format!(r#"{{"enabled":true,"settings_override":{chord}}}"#)
}

const MODIFIERS: &str = r#"{"control":false,"alt":false,"shift":false,"win":true}"#;

fn complete(config: ShortcutConfig) -> Preferences {
    Preferences::new(Theme::Dark, true)
        .with_dock(DockEdge::Right, vec!["dock-only".into()])
        .with_launcher_favorites(vec!["missing".into(), "exact".into(), "EXACT".into()])
        .with_launcher_display_mode(LauncherDisplayMode::Fullscreen)
        .with_general(GeneralPreferences::default().with_start_of_week(StartOfWeek::Saturday))
        .with_media_enabled(true)
        .with_shortcuts(config)
}

#[test]
fn exact_v4_migrates_defaults_read_only_then_explicitly_saves_complete_v5() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let store = SettingsStore::new(path.clone());
    let original = record(r#"{"enabled":true,"settings_override":null}"#)
        .replace(r#""schema_version":5"#, r#""schema_version":4"#)
        .replace(
            r#","shortcuts":{"enabled":true,"settings_override":null}"#,
            "",
        );
    fs::write(&path, &original).unwrap();
    let migrated = store.load().unwrap();
    assert_eq!(migrated, complete(ShortcutConfig::default()));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    let saved = migrated.with_shortcuts(ShortcutConfig::default().with_enabled(false));
    store.save(&saved).unwrap();
    let bytes = fs::read(&path).unwrap();
    assert_eq!(bytes.last(), Some(&b'\n'));
    assert_eq!(store.load().unwrap(), saved);
    let persisted: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(persisted["schema_version"], 5);
    assert_eq!(
        persisted["shortcuts"],
        serde_json::json!({"enabled":false,"settings_override":null})
    );
}

#[test]
fn all_modifier_combinations_and_canonical_keys_round_trip_without_touching_other_groups() {
    for bits in 0..16 {
        let modifiers = KeyModifiers {
            control: bits & 1 != 0,
            alt: bits & 2 != 0,
            shift: bits & 4 != 0,
            win: bits & 8 != 0,
        };
        for enabled in [false, true] {
            for override_chord in [None, Some(KeyChord::new(modifiers, 0x4B).unwrap())] {
                let config = ShortcutConfig::default()
                    .with_enabled(enabled)
                    .with_settings_override(override_chord)
                    .unwrap();
                let preferences = complete(config.clone());
                let bytes = serde_json::to_vec(&preferences).unwrap();
                assert_eq!(Preferences::decode(&bytes).unwrap(), preferences);
                assert_eq!(preferences.shortcuts(), &config);
                assert_eq!(
                    config.effective_chord(ShortcutAction::ToggleLauncher),
                    KeyChord::BareWin
                );
                assert_eq!(
                    preferences
                        .clone()
                        .with_shortcuts(ShortcutConfig::default()),
                    complete(ShortcutConfig::default())
                );
            }
        }
    }
    for key in 0..=255 {
        let encoded = record(&shortcut(&chord(&key.to_string(), MODIFIERS)));
        let expected = KeyChord::new(
            KeyModifiers {
                win: true,
                ..KeyModifiers::default()
            },
            key,
        );
        match expected {
            Ok(chord) => assert_eq!(
                Preferences::decode(encoded.as_bytes())
                    .unwrap()
                    .shortcuts()
                    .settings_override(),
                Some(&chord)
            ),
            Err(_) => assert!(
                Preferences::decode(encoded.as_bytes()).is_err(),
                "key {key}"
            ),
        }
    }
}

#[test]
fn shortcut_objects_require_every_field_and_distinguish_null_from_missing() {
    let valid = r#"{"enabled":true,"settings_override":null}"#;
    assert_eq!(
        Preferences::decode(record(valid).as_bytes()).unwrap(),
        complete(ShortcutConfig::default())
    );
    let mut invalid = vec![
        "null".to_owned(),
        "[]".into(),
        "[true,null]".into(),
        "true".into(),
        "0".into(),
        r#""enabled""#.into(),
        "{}".into(),
        r#"{"enabled":true}"#.into(),
        r#"{"settings_override":null}"#.into(),
        r#"{"enabled":true,"enabled":false,"settings_override":null}"#.into(),
        r#"{"enabled":true,"settings_override":null,"settings_override":null}"#.into(),
    ];
    for enabled in ["null", "0", "1", r#""true""#, "[]", "{}"] {
        invalid.push(format!(
            r#"{{"enabled":{enabled},"settings_override":null}}"#
        ));
    }
    for field in [
        "launcher",
        "bare_win",
        "paused",
        "status",
        "generation",
        "display_name",
        "photo",
        "personal_email",
        "onedrive",
        "path",
        "command",
    ] {
        invalid.push(format!(
            r#"{{"enabled":true,"settings_override":null,"{field}":null}}"#
        ));
    }
    for override_chord in [
        "true",
        "123",
        r#""bare_win""#,
        r#""Win+K""#,
        "[]",
        "[75,{}]",
        "{}",
        r#"{"bare_win":null}"#,
    ] {
        invalid.push(shortcut(override_chord));
    }
    for invalid in invalid {
        assert!(
            Preferences::decode(record(&invalid).as_bytes()).is_err(),
            "{invalid}"
        );
    }
    let complete = record(valid);
    for invalid in [
        complete.replace(
            r#","shortcuts":{"enabled":true,"settings_override":null}"#,
            "",
        ),
        complete.replace(
            r#""shortcuts":{"enabled":true,"settings_override":null}"#,
            r#""shortcuts":null"#,
        ),
        complete.replace(
            r#""shortcuts":"#,
            r#""shortcuts":{"enabled":true,"settings_override":null},"shortcuts":"#,
        ),
        complete.replace(r#""schema_version":5"#, r#""schema_version":6"#),
        complete.replace(r#""schema_version":5"#, r#""schema_version":4"#),
    ] {
        assert_ne!(invalid, complete);
        assert!(Preferences::decode(invalid.as_bytes()).is_err());
    }
}

#[test]
fn chord_rows_and_modifier_flags_are_strict_and_cannot_encode_readonly_launcher() {
    let valid = chord("75", MODIFIERS);
    let mut invalid = Vec::new();
    for key in [
        "null",
        "false",
        r#""75""#,
        "75.0",
        "-1",
        "0",
        "16",
        "17",
        "18",
        "91",
        "92",
        "123",
        "160",
        "165",
        "256",
        "65535",
        "65536",
        "18446744073709551615",
    ] {
        invalid.push(chord(key, MODIFIERS));
    }
    invalid.extend([
        valid.replace(r#""key":75,"#, ""),
        valid.replace(
            r#","modifiers":{"control":false,"alt":false,"shift":false,"win":true}"#,
            "",
        ),
        valid.replace(r#""key":75"#, r#""key":75,"key":76"#),
        valid.replace(r#""key":75"#, r#""key":75,"command":"open""#),
        valid.replace(r#""modifiers":"#, r#""modifiers":{},"modifiers":"#),
    ]);
    for modifiers in [
        "null",
        "[]",
        "[false,false,false,true]",
        "15",
        r#""win""#,
        "{}",
    ] {
        invalid.push(chord("75", modifiers));
    }
    for flag in ["control", "alt", "shift", "win"] {
        let value = if flag == "win" { "true" } else { "false" };
        for wrong_type in ["null", "0", "1", r#""false""#, "[]", "{}"] {
            invalid.push(chord(
                "75",
                &MODIFIERS.replace(
                    &format!(r#""{flag}":{value}"#),
                    &format!(r#""{flag}":{wrong_type}"#),
                ),
            ));
        }
        invalid.push(chord(
            "75",
            &MODIFIERS.replace(
                &format!(r#""{flag}":{value}"#),
                &format!(r#""{flag}":{value},"{flag}":{value}"#),
            ),
        ));
        let mut fields: serde_json::Value = serde_json::from_str(MODIFIERS).unwrap();
        fields.as_object_mut().unwrap().remove(flag);
        invalid.push(chord("75", &fields.to_string()));
    }
    for field in ["unknown", "flags", "meta", "super", "caps_lock"] {
        invalid.push(chord(
            "75",
            &MODIFIERS.replace('}', &format!(r#", "{field}":true}}"#)),
        ));
    }
    for invalid in invalid {
        assert!(
            Preferences::decode(record(&shortcut(&invalid)).as_bytes()).is_err(),
            "{invalid}"
        );
    }
    assert!(
        ShortcutConfig::default()
            .with_settings_override(Some(KeyChord::BareWin))
            .is_err()
    );
}

#[test]
fn invalid_v5_and_future_bytes_are_preserved_without_startup_migration_writes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let store = SettingsStore::new(path.clone());
    for original in [
        record(r#"{"enabled":true}"#),
        record(&shortcut(&chord("123", MODIFIERS))),
        record(r#"{"enabled":true,"settings_override":"bare_win"}"#),
        record(r#"{"enabled":true,"settings_override":null}"#)
            .replace(r#""schema_version":5"#, r#""schema_version":6"#),
    ] {
        fs::write(&path, &original).unwrap();
        assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}

#[test]
fn saved_config_has_no_profile_or_runtime_fields_and_default_override_is_explicit_null() {
    let preferences = complete(ShortcutConfig::default());
    let value = serde_json::to_value(&preferences).unwrap();
    let fields = value.as_object().unwrap();
    assert_eq!(fields.len(), 9);
    let shortcuts = fields["shortcuts"].as_object().unwrap();
    assert_eq!(shortcuts.len(), 2);
    assert_eq!(shortcuts["enabled"], true);
    assert_eq!(shortcuts["settings_override"], serde_json::Value::Null);
    for forbidden in [
        "display_name",
        "photo",
        "personal_email",
        "onedrive",
        "paused",
        "generation",
        "status",
        "path",
    ] {
        assert!(!fields.contains_key(forbidden));
        assert!(!shortcuts.contains_key(forbidden));
    }
}

#[test]
fn current_schema_keeps_every_old_group_strict_and_required() {
    let valid = record(r#"{"enabled":true,"settings_override":null}"#);
    for (name, contents) in [
        ("general", r#"{"start_of_week":"saturday"}"#),
        ("dock", r#"{"media_enabled":true}"#),
        (
            "launcher",
            r#"{"display_mode":"fullscreen","favorites":["missing","exact","EXACT"]}"#,
        ),
    ] {
        let field = format!(r#""{name}":{contents}"#);
        for malformed in [
            "null".to_owned(),
            "[]".into(),
            "{}".into(),
            "true".into(),
            contents.replacen('{', r#"{"unknown":true,"#, 1),
        ] {
            let invalid = valid.replace(&field, &format!(r#""{name}":{malformed}"#));
            assert_ne!(invalid, valid);
            assert!(Preferences::decode(invalid.as_bytes()).is_err());
        }
        let missing = valid.replace(&format!(",{field}"), "");
        assert_ne!(missing, valid);
        assert!(Preferences::decode(missing.as_bytes()).is_err());
        let duplicate = valid.replace(&field, &format!("{field},{field}"));
        assert!(Preferences::decode(duplicate.as_bytes()).is_err());
    }
}
