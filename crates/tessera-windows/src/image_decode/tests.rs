// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::{DecodePlan, ImageDecodeLimits, MAX_ENCODED_BYTES};

#[test]
fn both_policies_preserve_all_encoded_and_source_budgets() {
    for limits in [
        ImageDecodeLimits::media_thumbnail(),
        ImageDecodeLimits::profile_photo(),
    ] {
        assert!(limits.read_count(0).is_err());
        assert!(limits.read_count(MAX_ENCODED_BYTES + 1).is_err());
        assert!(limits.read_count(u64::MAX).is_err());
        assert_eq!(
            limits.read_count(MAX_ENCODED_BYTES).unwrap(),
            MAX_ENCODED_BYTES as u32 + 1
        );
        assert!(limits.copied_size(4, 4, 4).is_ok());
        assert!(limits.copied_size(4, 3, 4).is_err());
        assert!(limits.copied_size(4, 5, 4).is_err());
        assert!(limits.copied_size(4, 4, 5).is_err());
        for (width, height) in [
            (0, 1),
            (1, 0),
            (u32::MAX, u32::MAX),
            (16_385, 1),
            (8_192, 8_192),
        ] {
            assert!(DecodePlan::new(width, height, limits).is_err());
        }
        assert!(DecodePlan::new(4096, 4096, limits).is_ok());
        assert!(DecodePlan::new(4096, 4097, limits).is_err());
    }
}

#[test]
fn profile_policy_only_expands_output_and_never_upscales_or_crops() {
    let limits = ImageDecodeLimits::profile_photo();
    let square = DecodePlan::new(4096, 4096, limits).unwrap();
    assert_eq!(
        (square.width, square.height, square.bytes),
        (512, 512, 1_048_576)
    );
    let wide = DecodePlan::new(1024, 2, limits).unwrap();
    assert_eq!((wide.width, wide.height, wide.bytes), (512, 1, 2048));
    let tall = DecodePlan::new(1, 16_384, limits).unwrap();
    assert_eq!((tall.width, tall.height), (1, 512));
    let small = DecodePlan::new(2, 1, limits).unwrap();
    assert_eq!((small.width, small.height, small.bytes), (2, 1, 8));
    let rgba = [128, 64, 0, 128, 9, 10, 11, 255];
    let image = small.accept(&rgba).unwrap();
    assert_eq!((image.width(), image.height()), (2, 1));
    assert_eq!(image.rgba(), &rgba);
    assert_eq!(image.into_rgba(), rgba);
    assert!(small.accept(&[]).is_err());
    assert!(small.accept(&rgba[..4]).is_err());
    assert!(small.accept(&[255, 0, 0, 128, 9, 10, 11, 255]).is_err());
}

#[test]
fn decoded_images_cannot_be_forged_with_invalid_layout_or_transparent_rgb() {
    for plan in [
        DecodePlan {
            width: 0,
            height: 1,
            bytes: 0,
        },
        DecodePlan {
            width: 513,
            height: 1,
            bytes: 2052,
        },
        DecodePlan {
            width: 1,
            height: 1,
            bytes: 3,
        },
    ] {
        assert!(plan.accept(&vec![0; plan.bytes]).is_err());
    }
    let plan = DecodePlan::new(1, 1, ImageDecodeLimits::profile_photo()).unwrap();
    assert!(plan.accept(&[1, 0, 0, 0]).is_err());
    assert_eq!(plan.accept(&[0, 0, 0, 0]).unwrap().rgba(), &[0, 0, 0, 0]);
    let error = ImageDecodeLimits::profile_photo()
        .read_count(0)
        .unwrap_err();
    assert_eq!(error.hresult(), None);
    assert!(error.message().contains("empty"));
}
