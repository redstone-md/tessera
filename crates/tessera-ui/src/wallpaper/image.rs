// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Bounded presentation of canonical native thumbnails, never file decoding or OS authority.

use material_colors::color::Argb;
use material_colors::quantize::{Quantizer, QuantizerCelebi};
use material_colors::score::Score;
use tessera_system::wallpaper::WallpaperPreview;

use crate::SourceSeed;

/// Pure CPU preparation on the choose-completion thread. Only fully opaque
/// native samples enter Material's maintained quantizer; no background is invented.
pub(super) fn source_seed(preview: &WallpaperPreview) -> Option<SourceSeed> {
    let opaque = preview
        .rgba()
        .chunks_exact(4)
        .filter(|pixel| pixel[3] == 255);
    let count = opaque.clone().count();
    if count == 0 {
        return None;
    }
    let stride = count.div_ceil(1024);
    let pixels: Vec<_> = opaque
        .step_by(stride)
        .take(1024)
        .map(|pixel| Argb::new(255, pixel[0], pixel[1], pixel[2]))
        .collect();
    let palette = QuantizerCelebi::quantize(&pixels, 64).color_to_count;
    let fallback = palette
        .iter()
        .filter(|(color, population)| color.alpha == 255 && **population > 0)
        .max_by_key(|(color, population)| (**population, **color))
        .map(|(color, _)| *color)?;
    // Score's default blue is never allowed to masquerade as image-derived.
    let color = Score::score(&palette, Some(1), Some(fallback), Some(true))
        .into_iter()
        .find(|color| {
            color.alpha == 255 && palette.get(color).is_some_and(|population| *population > 0)
        })?;
    SourceSeed::from_rgb(
        (u32::from(color.red) << 16) | (u32::from(color.green) << 8) | u32::from(color.blue),
    )
}

/// Construct Slint's thread-local image once, on the UI thread, after receipt
/// admission. WallpaperPreview already enforces exact canonical premultiplied bytes.
pub(super) fn slint_image(preview: &WallpaperPreview) -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        preview.rgba(),
        preview.width(),
        preview.height(),
    );
    slint::Image::from_rgba8_premultiplied(buffer)
}
