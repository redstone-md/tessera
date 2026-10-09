// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::collections::HashSet;
use std::sync::atomic::AtomicU64;

use super::*;

fn session() -> MediaSession {
    MediaSession {
        key: MediaSessionKey::issue().unwrap(),
        source_app_id: "player".into(),
        title: "Track".into(),
        author: "Artist".into(),
        playback: MediaPlayback::Changing,
        capabilities: MediaCapabilities {
            previous: true,
            toggle: true,
            next: false,
        },
        artwork: None,
        artwork_notice: None,
    }
}

#[test]
fn issued_keys_are_distinct_and_counter_exhaustion_never_wraps() {
    let keys: HashSet<_> = (0..256)
        .map(|_| MediaSessionKey::issue().unwrap())
        .collect();
    assert_eq!(keys.len(), 256);
    let counter = AtomicU64::new(u64::MAX - 1);
    let last = MediaSessionKey::issue_from(&counter).unwrap();
    assert_eq!(last.0.get(), u64::MAX - 1);
    assert_eq!(
        MediaSessionKey::issue_from(&counter).unwrap_err().kind,
        MediaErrorKind::Other
    );
    assert!(MediaSessionKey::issue_from(&counter).is_err());
    assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    let zero = AtomicU64::new(0);
    assert!(MediaSessionKey::issue_from(&zero).is_err());
    assert_eq!(zero.load(Ordering::Relaxed), 0);
}

#[test]
fn artwork_has_exact_bounded_premultiplied_rgba_storage() {
    let pixels = vec![80, 40, 20, 128, 0, 0, 0, 0];
    let artwork = MediaArtwork::new(2, 1, pixels.clone()).unwrap();
    assert_eq!(artwork.width(), 2);
    assert_eq!(artwork.height(), 1);
    assert_eq!(artwork.rgba(), pixels);
    assert!(MediaArtwork::new(128, 128, vec![255; 128 * 128 * 4]).is_ok());
    for (width, height) in [(0, 1), (1, 0), (129, 1), (1, 129), (u32::MAX, u32::MAX)] {
        assert!(MediaArtwork::new(width, height, Vec::new()).is_err());
    }
    for length in [0, 3, 5, 8] {
        assert!(MediaArtwork::new(1, 1, vec![0; length]).is_err());
    }
    for pixel in [[129, 0, 0, 128], [0, 1, 0, 0], [0, 0, 255, 254]] {
        assert!(MediaArtwork::new(1, 1, pixel.to_vec()).is_err());
    }
}

#[test]
fn unicode_ingress_bounds_only_metadata_and_preserves_artwork_notice() {
    let notice = MediaError::with_hresult(
        MediaErrorKind::Unavailable,
        "Artwork stream unavailable",
        -2147024891,
    );
    let mut original = session();
    original.source_app_id = "界".repeat(257);
    original.title = "🎧".repeat(513);
    original.author = "e\u{301}".repeat(300);
    original.artwork_notice = Some(notice.clone());
    let expected_key = original.key;
    let bounded = original.bounded();
    assert!(bounded.source_app_id.is_empty());
    assert_eq!(bounded.title.chars().count(), 512);
    assert_eq!(bounded.author.chars().count(), 512);
    assert_eq!(bounded.key, expected_key);
    assert_eq!(bounded.playback, MediaPlayback::Changing);
    assert_eq!(bounded.artwork_notice, Some(notice));
    assert!(bounded.artwork.is_none());
    assert_eq!(bounded.clone().bounded(), bounded);
    let original = session();
    assert_eq!(original.clone().bounded(), original);
}

#[test]
fn overlong_source_id_never_becomes_a_different_exact_catalog_identity() {
    let trusted = "界".repeat(256);
    let mut value = session();
    value.source_app_id = trusted.clone();
    assert_eq!(value.clone().bounded().source_app_id, trusted);
    value.source_app_id.push('外');
    let bounded = value.bounded();
    assert!(bounded.source_app_id.is_empty());
    assert_ne!(bounded.source_app_id, trusted);
    assert_eq!(bounded.title, "Track");
}

#[test]
fn playback_capabilities_absence_and_native_error_are_not_synthesized() {
    let states = [
        MediaPlayback::Closed,
        MediaPlayback::Opened,
        MediaPlayback::Changing,
        MediaPlayback::Stopped,
        MediaPlayback::Playing,
        MediaPlayback::Paused,
    ];
    for playback in states {
        let mut value = session();
        value.playback = playback;
        assert_eq!(value.bounded().playback, playback);
    }
    let capabilities = MediaCapabilities {
        previous: true,
        toggle: false,
        next: true,
    };
    assert!(capabilities.allows(MediaAction::Previous));
    assert!(!capabilities.allows(MediaAction::Toggle));
    assert!(capabilities.allows(MediaAction::Next));
    let absent: Result<MediaSnapshot, MediaError> = Ok(MediaSnapshot { current: None });
    let error = MediaError::with_hresult(
        MediaErrorKind::Unavailable,
        "GSMTC access denied",
        -2147024891,
    );
    assert_ne!(absent, Err(error.clone()));
    assert_eq!(error.hresult, Some(-2147024891));
    assert_eq!(error.to_string(), "GSMTC access denied");
    assert_eq!(
        MediaError::new(MediaErrorKind::Rejected, "OS refused transport").hresult,
        None
    );
}
