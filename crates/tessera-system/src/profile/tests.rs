// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

#[test]
fn photo_requires_bounded_exact_premultiplied_pixels() {
    for (width, height, rgba) in [
        (0, 1, vec![]),
        (513, 1, vec![0; 513 * 4]),
        (1, 1, vec![0; 3]),
        (1, 1, vec![129, 0, 0, 128]),
        (u32::MAX, u32::MAX, vec![]),
    ] {
        assert_eq!(
            ProfilePhoto::new(width, height, rgba).unwrap_err().kind(),
            ProfileErrorKind::InvalidData
        );
    }
    let photo = ProfilePhoto::new(1, 1, vec![128, 0, 0, 128]).unwrap();
    assert_eq!(
        (photo.width(), photo.height(), photo.rgba()),
        (1, 1, &[128, 0, 0, 128][..])
    );
    let clone = photo.clone();
    assert_eq!(photo.rgba().as_ptr(), clone.rgba().as_ptr());
}

#[test]
fn absence_and_failed_photo_are_not_ready_and_do_not_erase_identity() {
    for state in [
        ProfilePhotoState::Absent,
        ProfilePhotoState::Unavailable(ProfileErrorKind::AccessDenied),
    ] {
        let snapshot =
            ProfileSnapshot::new("Genuine 名字".into(), state.clone(), None, None).unwrap();
        assert_eq!(snapshot.display_name(), "Genuine 名字");
        assert_eq!(snapshot.photo(), &state);
        assert_eq!(snapshot.personal_email(), None);
        assert!(snapshot.onedrive().is_none());
    }
}

#[test]
fn snapshot_ingress_is_bounded_and_debug_redacts_private_fields() {
    let snapshot = ProfileSnapshot::new(
        "Private Name".into(),
        ProfilePhotoState::Absent,
        Some("private@example.test".into()),
        Some(ProfileTarget::new("C:\\private\\path".to_owned())),
    )
    .unwrap();
    let debug = format!("{snapshot:?}");
    for secret in ["Private Name", "private@example.test", "private\\path"] {
        assert!(!debug.contains(secret));
    }
    assert_eq!(
        snapshot
            .onedrive()
            .unwrap()
            .downcast_ref::<String>()
            .unwrap(),
        "C:\\private\\path"
    );
    for name in ["".into(), "x".repeat(1_025), "name\0suffix".into()] {
        assert!(ProfileSnapshot::new(name, ProfilePhotoState::Absent, None, None).is_err());
    }
    assert!(
        ProfileSnapshot::new("名字".repeat(170), ProfilePhotoState::Absent, None, None).is_ok()
    );
    assert!(
        ProfileSnapshot::new(
            "Name".into(),
            ProfilePhotoState::Absent,
            Some("bad\nmail".into()),
            None
        )
        .is_err()
    );
}
