// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

#[test]
fn profile_identity_is_bounded_nonempty_and_opaque() {
    for value in [String::new(), "x".repeat(513), "a\0b".into()] {
        assert_eq!(
            ProfileId::new(value).unwrap_err().kind,
            InputLanguageErrorKind::InvalidValue
        );
    }
    let opaque = "native-issued/日本語:not-a-layout-handle";
    assert_eq!(ProfileId::new(opaque).unwrap().as_str(), opaque);
    assert!(ProfileId::new("x".repeat(512)).is_ok());
}

#[test]
fn equal_native_issued_ids_are_equal_without_textual_profile_inference() {
    let first = ProfileId::new("native-a").unwrap();
    assert_eq!(first, ProfileId::new("native-a").unwrap());
    assert_ne!(first, ProfileId::new("native-b").unwrap());
}

#[test]
fn snapshot_preserves_empty_metadata_unicode_names_and_multiple_active_observations() {
    assert!(InputLanguageSnapshot::default().languages.is_empty());
    let snapshot = InputLanguageSnapshot {
        languages: vec![InputLanguage {
            id: "lang-opaque".into(),
            code: String::new(),
            name: "日本語".into(),
            native_name: "日本語".into(),
            profiles: vec![
                InputProfile {
                    id: ProfileId::new("layout-a").unwrap(),
                    display_name: "同じ名前".into(),
                    active: true,
                },
                InputProfile {
                    id: ProfileId::new("tip-b").unwrap(),
                    display_name: "同じ名前".into(),
                    active: true,
                },
            ],
        }],
    };
    assert!(snapshot.languages[0].code.is_empty());
    assert_eq!(
        snapshot.languages[0]
            .profiles
            .iter()
            .filter(|profile| profile.active)
            .count(),
        2
    );
    assert_ne!(
        snapshot.languages[0].profiles[0].id,
        snapshot.languages[0].profiles[1].id
    );
}

#[test]
fn action_interface_contains_only_exact_profile_or_fixed_settings_dispatch() {
    let profile = ProfileId::new("native-key").unwrap();
    assert_eq!(
        InputLanguageAction::Activate {
            profile: profile.clone()
        },
        InputLanguageAction::Activate { profile }
    );
    assert_eq!(
        InputLanguageAction::OpenKeyboardSettings,
        InputLanguageAction::OpenKeyboardSettings
    );
    assert_eq!(
        InputLanguageError::new(
            InputLanguageErrorKind::Unavailable,
            "Confirmation unavailable."
        )
        .to_string(),
        "Confirmation unavailable."
    );
}
