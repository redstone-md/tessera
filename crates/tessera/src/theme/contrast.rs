// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Keeping what is read apart from what it lies on, measured as the
//! contrast ratio of WCAG 2.1.

use gpui::{Hsla, Rgba};

use super::ColorRoles;

/// The least ratio WCAG 2.1 asks of text of ordinary size (level AA).
pub const TEXT: f32 = 4.5;

/// The least ratio it asks of what is not text, such as a line.
pub const GRAPHIC: f32 = 3.;

/// How closely the least change of lightness that is enough is found.
const STEPS: usize = 16;

/// The relative luminance of `color`, from 0 for black to 1 for white.
fn luminance(color: Hsla) -> f32 {
    let Rgba { r, g, b, .. } = color.into();
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// The contrast ratio of two colors, from 1 to 21.
pub fn ratio(one: Hsla, other: Hsla) -> f32 {
    let (one, other) = (luminance(one), luminance(other));
    (one.max(other) + 0.05) / (one.min(other) + 0.05)
}

/// The least ratio `content` has to any of `grounds`.
fn least(content: Hsla, grounds: &[Hsla]) -> f32 {
    grounds
        .iter()
        .map(|&ground| ratio(content, ground))
        .fold(f32::INFINITY, f32::min)
}

/// `content`, made lighter or darker by as little as gives it the ratio
/// `wanted` to each of `grounds`; its hue and saturation stay. Where
/// neither white nor black reaches the ratio, the one that comes closer.
pub fn legible(content: Hsla, grounds: &[Hsla], wanted: f32) -> Hsla {
    if least(content, grounds) >= wanted {
        return content;
    }
    let with = |lightness: f32| Hsla {
        l: lightness,
        ..content
    };
    let end = if least(with(1.), grounds) >= least(with(0.), grounds) {
        1.
    } else {
        0.
    };
    if least(with(end), grounds) < wanted {
        return with(end);
    }
    // Luminance grows with lightness, so the ratio grows toward `end`.
    let (mut short, mut enough) = (content.l, end);
    for _ in 0..STEPS {
        let middle = f32::midpoint(short, enough);
        if least(with(middle), grounds) >= wanted {
            enough = middle;
        } else {
            short = middle;
        }
    }
    with(enough)
}

impl ColorRoles {
    /// Gives the roles that are read the contrast WCAG 2.1 asks for to
    /// the roles they lie on. Roles that already have it stay as they are.
    pub fn make_legible(&mut self) {
        let grounds = [
            self.surface_container,
            self.surface_container_highest,
            self.secondary_container,
        ];
        self.on_surface = legible(self.on_surface, &grounds, TEXT);
        self.on_surface_variant = legible(self.on_surface_variant, &grounds, TEXT);
        // Also a field with the color of the surface written on it.
        self.primary = legible(self.primary, &grounds[..2], TEXT);
        self.outline = legible(self.outline, &grounds[..1], GRAPHIC);
    }
}

#[cfg(test)]
mod tests {
    use gpui::rgb;

    use super::*;
    use crate::theme::Theme;

    #[test]
    fn black_and_white_are_as_far_apart_as_colors_get() {
        let (black, white) = (rgb(0x000000).into(), rgb(0xffffff).into());
        assert!((ratio(black, white) - 21.).abs() < 0.01);
        assert!((ratio(white, white) - 1.).abs() < 0.01);
        // The gray known as the lightest that reads on white.
        assert!((ratio(rgb(0x767676).into(), white) - 4.54).abs() < 0.02);
    }

    #[test]
    fn a_color_that_reads_stays_and_one_that_does_not_moves_just_enough() {
        let ground: Hsla = rgb(0x1e211a).into();
        let fine: Hsla = rgb(0xe2e3d9).into();
        assert_eq!(legible(fine, &[ground], TEXT), fine);

        let dim: Hsla = rgb(0x4a4f45).into();
        let moved = legible(dim, &[ground], TEXT);
        let reached = ratio(moved, ground);
        assert!((TEXT..TEXT + 0.1).contains(&reached), "{reached}");
        assert_eq!((moved.h, moved.s), (dim.h, dim.s));

        // On a light ground it goes the other way.
        let paper: Hsla = rgb(0xf2f2ee).into();
        let moved = legible(rgb(0xb0b0b0).into(), &[paper], TEXT);
        assert!(moved.l < 0.5 && ratio(moved, paper) >= TEXT);
    }

    #[test]
    fn where_nothing_reads_the_closest_end_is_taken() {
        let grounds = [rgb(0x000000).into(), rgb(0xffffff).into()];
        let moved = legible(rgb(0x808080).into(), &grounds, TEXT);
        assert!(moved.l == 0. || moved.l == 1.);
    }

    #[test]
    fn the_built_in_colors_already_read() {
        let mut colors = Theme::default().colors;
        let before = colors;
        colors.make_legible();
        assert_eq!(colors.on_surface, before.on_surface);
        assert_eq!(colors.on_surface_variant, before.on_surface_variant);
        assert_eq!(colors.primary, before.primary);
        assert_eq!(colors.outline, before.outline);
    }

    #[test]
    fn roles_of_a_theme_that_do_not_read_are_moved_apart() {
        let mut colors = Theme::default().colors;
        colors.on_surface = rgb(0x30332b).into();
        colors.primary = rgb(0x2a3a20).into();
        colors.outline = rgb(0x22251e).into();
        colors.make_legible();
        for ground in [colors.surface_container, colors.surface_container_highest] {
            assert!(ratio(colors.on_surface, ground) >= TEXT);
            assert!(ratio(colors.primary, ground) >= TEXT);
        }
        assert!(ratio(colors.on_surface, colors.secondary_container) >= TEXT);
        assert!(ratio(colors.outline, colors.surface_container) >= GRAPHIC);
    }
}
