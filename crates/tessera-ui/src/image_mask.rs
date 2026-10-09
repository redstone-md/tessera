// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pure presentation of canonical, already decoded premultiplied RGBA pixels.
//! No decoding, provider authority, caching or state belongs in this module.

use slint::{Rgba8Pixel, SharedPixelBuffer};

#[derive(Clone, Copy)]
pub(crate) enum Mask {
    Circle,
    RoundedSquare { radius_fraction: f32 },
}

/// Center-cover sample and mask a validated canonical image.
///
/// Source dimensions and output axis must be in 1..=512, and `rgba` must
/// contain exactly width * height premultiplied RGBA pixels. Rounded-square
/// radius is a finite fraction of the output axis in 0..=0.5. These invariants
/// are asserted here; this is not a decoder or a provider validation policy.
pub(crate) fn cover_pixels(
    width: u32,
    height: u32,
    rgba: &[u8],
    axis: u32,
    mask: Mask,
) -> SharedPixelBuffer<Rgba8Pixel> {
    assert!((1..=512).contains(&width));
    assert!((1..=512).contains(&height));
    assert!((1..=512).contains(&axis));
    assert_eq!(rgba.len(), width as usize * height as usize * 4);
    if let Mask::RoundedSquare { radius_fraction } = mask {
        assert!((0.0..=0.5).contains(&radius_fraction));
    }
    let width = width as usize;
    let height = height as usize;
    let side = width.min(height) as f32;
    let left = (width as f32 - side) / 2.0;
    let top = (height as f32 - side) / 2.0;
    let output_axis = axis;
    let axis = axis as usize;
    let radius = output_axis as f32 / 2.0;
    let source = rgba.as_chunks::<4>().0;
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(output_axis, output_axis);
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
        let coverage = match mask {
            Mask::Circle => (radius - dx.hypot(dy) + 0.5).clamp(0.0, 1.0),
            Mask::RoundedSquare { radius_fraction } => {
                let corner_radius = output_axis as f32 * radius_fraction;
                let qx = dx.abs() - (radius - corner_radius);
                let qy = dy.abs() - (radius - corner_radius);
                let distance = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - corner_radius;
                (0.5 - distance).clamp(0.0, 1.0)
            }
        };
        if coverage == 0.0 {
            continue;
        }
        // Match Image's centered cover crop, preserving aspect rather than
        // stretching the canonical source. Interpolate premul channels.
        let sx = (left + (x as f32 + 0.5) * side / output_axis as f32 - 0.5)
            .clamp(0.0, (width - 1) as f32);
        let sy = (top + (y as f32 + 0.5) * side / output_axis as f32 - 0.5)
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

    const MEDIA_MASK: Mask = Mask::RoundedSquare {
        radius_fraction: 6.0 / 40.0,
    };

    #[test]
    fn rounded_square_preserves_premultiplied_color_and_source_with_antialiased_corners() {
        let rgba = [96, 64, 32, 128].repeat(4);
        let original = rgba.clone();
        let buffer = cover_pixels(2, 2, &rgba, 40, MEDIA_MASK);
        let data = buffer.as_bytes().as_chunks::<4>().0;
        assert_eq!((buffer.width(), buffer.height()), (40, 40));
        for index in [0, 39, 39 * 40, 40 * 40 - 1] {
            assert_eq!(data[index], [0, 0, 0, 0]);
        }
        assert_eq!(data[20 * 40 + 20], [96, 64, 32, 128]);
        assert_eq!(data[20], [96, 64, 32, 128]);
        let edge = data[3];
        assert!(edge[3] > 0 && edge[3] < 128);
        assert!(edge[0] > edge[1] && edge[1] > edge[2] && edge[2] > 0);
        assert!(
            data.iter()
                .all(|pixel| pixel[..3].iter().all(|value| *value <= pixel[3]))
        );
        assert_eq!(rgba, original);
    }

    #[test]
    fn rounded_square_center_cover_crops_rectangular_source_without_stretching() {
        let rgba: Vec<u8> = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 0, 0, 255],
        ]
        .into_iter()
        .flat_map(|pixel| pixel.repeat(2))
        .collect();
        let buffer = cover_pixels(2, 4, &rgba, 40, MEDIA_MASK);
        let data = buffer.as_bytes().as_chunks::<4>().0;
        let upper = data[10 * 40 + 20];
        let lower = data[29 * 40 + 20];
        assert!(upper[0] <= 1 && upper[1] >= 248 && upper[2] <= 7);
        assert!(lower[0] <= 1 && lower[1] <= 7 && lower[2] >= 248);
        assert_eq!(data[0], [0, 0, 0, 0]);
    }
}
