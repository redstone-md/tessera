// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Renderer-independent circular presentation, not photo decoding or authority.
//! Slint 1.18's software renderer does not support rounded Rectangle clipping.
//! Prepare one bounded premultiplied image per accepted photo and cache it in
//! Projection. The 512px mask preserves antialiasing at native display scales;
//! no allocation or pixel work occurs on refit, theme, idle or ordinary updates.

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use tessera_system::profile::ProfilePhoto;

const AVATAR_AXIS: u32 = 512;

pub(crate) fn prepare(photo: &ProfilePhoto) -> Image {
    Image::from_rgba8_premultiplied(pixels(photo))
}

fn pixels(photo: &ProfilePhoto) -> SharedPixelBuffer<Rgba8Pixel> {
    let width = photo.width() as usize;
    let height = photo.height() as usize;
    let side = width.min(height) as f32;
    let left = (width as f32 - side) / 2.0;
    let top = (height as f32 - side) / 2.0;
    let axis = AVATAR_AXIS as usize;
    let radius = AVATAR_AXIS as f32 / 2.0;
    let source = photo.rgba().as_chunks::<4>().0;
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(AVATAR_AXIS, AVATAR_AXIS);
    for (index, pixel) in buffer
        .make_mut_bytes()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .enumerate()
    {
        let x = index % axis;
        let y = index / axis;
        let dx = x as f32 + 0.5 - radius;
        let dy = y as f32 + 0.5 - radius;
        let coverage = (radius - dx.hypot(dy) + 0.5).clamp(0.0, 1.0);
        if coverage == 0.0 {
            continue;
        }
        // Match Image's centered cover crop, preserving aspect rather than
        // stretching the canonical provider photo. Interpolate premul channels.
        let sx = (left + (x as f32 + 0.5) * side / AVATAR_AXIS as f32 - 0.5)
            .clamp(0.0, (width - 1) as f32);
        let sy = (top + (y as f32 + 0.5) * side / AVATAR_AXIS as f32 - 0.5)
            .clamp(0.0, (height - 1) as f32);
        let x0 = sx.floor() as usize;
        let y0 = sy.floor() as usize;
        let x1 = (x0 + 1).min(width - 1);
        let y1 = (y0 + 1).min(height - 1);
        let tx = sx - x0 as f32;
        let ty = sy - y0 as f32;
        for (channel, output) in pixel.iter_mut().enumerate() {
            let upper = source[y0 * width + x0][channel] as f32 * (1.0 - tx)
                + source[y0 * width + x1][channel] as f32 * tx;
            let lower = source[y1 * width + x0][channel] as f32 * (1.0 - tx)
                + source[y1 * width + x1][channel] as f32 * tx;
            *output = ((upper * (1.0 - ty) + lower * ty) * coverage).round() as u8;
        }
    }
    buffer
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
