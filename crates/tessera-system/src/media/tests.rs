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
        timeline: Ok(MediaTimeline {
            start_ticks: 0,
            end_ticks: 900_000_000,
            position_ticks: 300_000_000,
            min_seek_ticks: 0,
            max_seek_ticks: 900_000_000,
            last_updated_utc_ticks: Some(133_000_000_000_000_000),
        }),
        seek: Ok(MediaSeekObservation {
            revision: MediaObservationRevision::issue().unwrap(),
            min_ticks: 0,
            max_ticks: 900_000_000,
        }),
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
fn observation_revisions_are_unique_and_exhaustion_never_reuses_authority() {
    let revisions: Vec<_> = (0..256)
        .map(|_| MediaObservationRevision::issue().unwrap())
        .collect();
    for (index, revision) in revisions.iter().enumerate() {
        assert!(!revisions[..index].contains(revision));
    }
    let counter = AtomicU64::new(u64::MAX - 1);
    let last = MediaObservationRevision::issue_from(&counter).unwrap();
    assert_eq!(last.0.get(), u64::MAX - 1);
    for _ in 0..2 {
        assert_eq!(
            MediaObservationRevision::issue_from(&counter)
                .unwrap_err()
                .kind,
            MediaErrorKind::Other
        );
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    }
    let zero = AtomicU64::new(0);
    assert!(MediaObservationRevision::issue_from(&zero).is_err());
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

#[test]
fn timeline_failure_is_independent_of_metadata_artwork_and_transports() {
    let mut original = session();
    let expected = original.clone();
    let notice = MediaError::with_hresult(
        MediaErrorKind::Unavailable,
        "Recorded timeline failure",
        -2147024891,
    );
    original.timeline = Err(notice.clone());
    let bounded = original.bounded();
    assert_eq!(bounded.timeline, Err(notice));
    assert_eq!(bounded.key, expected.key);
    assert_eq!(bounded.title, expected.title);
    assert_eq!(bounded.author, expected.author);
    assert_eq!(bounded.playback, expected.playback);
    assert_eq!(bounded.capabilities, expected.capabilities);
    assert_eq!(bounded.seek, expected.seek);
    assert_eq!(bounded.artwork, expected.artwork);
    assert_eq!(bounded.artwork_notice, expected.artwork_notice);
}

#[test]
fn seek_failure_is_independent_of_timeline_metadata_and_transport_capabilities() {
    let mut original = session();
    let expected = original.clone();
    let notice = MediaError::new(MediaErrorKind::Unavailable, "Recorded seek failure");
    original.seek = Err(notice.clone());
    let bounded = original.bounded();
    assert_eq!(bounded.seek, Err(notice));
    assert_eq!(bounded.timeline, expected.timeline);
    assert_eq!(bounded.key, expected.key);
    assert_eq!(bounded.title, expected.title);
    assert_eq!(bounded.author, expected.author);
    assert_eq!(bounded.playback, expected.playback);
    assert_eq!(bounded.capabilities, expected.capabilities);
    assert_eq!(bounded.artwork, expected.artwork);
    assert_eq!(bounded.artwork_notice, expected.artwork_notice);
}

#[test]
fn timeline_ingress_preserves_zero_signed_invalid_and_extreme_observations() {
    for (start, end, position, min_seek, max_seek, utc) in [
        (0, 0, 0, 0, 0, None),
        (-50, 50, -25, -40, 40, Some(-1)),
        (10, -10, 20, 15, -15, Some(0)),
        (
            i64::MIN,
            i64::MAX,
            i64::MIN,
            i64::MIN,
            i64::MAX,
            Some(i64::MAX),
        ),
        (
            i64::MAX,
            i64::MIN,
            i64::MAX,
            i64::MAX,
            i64::MIN,
            Some(i64::MIN),
        ),
    ] {
        let facts = MediaTimeline {
            start_ticks: start,
            end_ticks: end,
            position_ticks: position,
            min_seek_ticks: min_seek,
            max_seek_ticks: max_seek,
            last_updated_utc_ticks: utc,
        };
        let mut original = session();
        original.timeline = Ok(facts);
        let bounded = original.bounded();
        assert_eq!(bounded.timeline, Ok(facts));
        assert!(
            MediaSnapshot {
                current: Some(bounded)
            }
            .current
            .is_some()
        );
    }
}

#[test]
fn missing_utc_timestamp_keeps_five_timeline_facts_available() {
    let mut original = session();
    let mut facts = *original.timeline.as_ref().unwrap();
    let expected = facts;
    facts.last_updated_utc_ticks = None;
    original.timeline = Ok(facts);
    let observed = original.bounded().timeline.unwrap();
    assert_eq!(observed.start_ticks, expected.start_ticks);
    assert_eq!(observed.end_ticks, expected.end_ticks);
    assert_eq!(observed.position_ticks, expected.position_ticks);
    assert_eq!(observed.min_seek_ticks, expected.min_seek_ticks);
    assert_eq!(observed.max_seek_ticks, expected.max_seek_ticks);
    assert_eq!(observed.last_updated_utc_ticks, None);
}
