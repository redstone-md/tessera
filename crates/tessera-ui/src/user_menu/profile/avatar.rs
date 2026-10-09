// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Renderer-independent circular presentation, not photo decoding or authority.
//! Slint 1.18's software renderer does not support rounded Rectangle clipping.
//! Prepare one bounded premultiplied image per accepted photo and cache it in
//! Projection. The 512px mask preserves antialiasing at native display scales;
//! no allocation or pixel work occurs on refit, theme, idle or ordinary updates.

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use tessera_system::profile::ProfilePhoto;

use crate::image_mask::{Mask, cover_pixels};

const AVATAR_AXIS: u32 = 512;

pub(crate) fn prepare(photo: &ProfilePhoto) -> Image {
    Image::from_rgba8_premultiplied(pixels(photo))
}

fn pixels(photo: &ProfilePhoto) -> SharedPixelBuffer<Rgba8Pixel> {
    cover_pixels(
        photo.width(),
        photo.height(),
        photo.rgba(),
        AVATAR_AXIS,
        Mask::Circle,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circular_mask_keeps_premultiplied_color_and_antialiased_alpha_without_mutating_photo() {
        let rgba = [128, 0, 0, 128].repeat(4);
        let photo = ProfilePhoto::new(2, 2, rgba.clone()).unwrap();
        let buffer = pixels(&photo);
        assert_eq!((buffer.width(), buffer.height()), (512, 512));
        let data = buffer.as_bytes().as_chunks::<4>().0;
        assert_eq!(data[0], [0, 0, 0, 0]);
        assert_eq!(data[256 * 512 + 256], [128, 0, 0, 128]);
        assert!(data.iter().any(|pixel| pixel[3] > 0 && pixel[3] < 128));
        assert!(
            data.iter()
                .all(|pixel| pixel[0] <= pixel[3] && pixel[1] <= pixel[3] && pixel[2] <= pixel[3])
        );
        assert_eq!((photo.width(), photo.height()), (2, 2));
        assert_eq!(photo.rgba(), rgba);
    }

    #[test]
    fn rectangular_photo_is_center_cover_cropped_not_stretched_or_replaced() {
        let rgba = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 0, 0, 255],
        ]
        .into_iter()
        .flat_map(|pixel| pixel.repeat(2))
        .collect();
        let photo = ProfilePhoto::new(2, 4, rgba).unwrap();
        let buffer = pixels(&photo);
        let data = buffer.as_bytes().as_chunks::<4>().0;
        let upper = data[128 * 512 + 256];
        let lower = data[383 * 512 + 256];
        assert!(upper[0] <= 1 && upper[1] >= 254 && upper[2] <= 1);
        assert!(lower[0] <= 1 && lower[1] <= 1 && lower[2] >= 254);
        assert_eq!(data[0], [0, 0, 0, 0]);
    }
}
