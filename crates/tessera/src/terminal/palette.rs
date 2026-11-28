// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The colours of the terminal: the 16 named ones, for a dark theme or a
//! light one, or as `[terminal] palette` gives them; the 240 more of the
//! 256 that programs pick by number; the foreground, background, cursor
//! and selection from the theme; and true colour as programs give it.
//! Colours are kept as `0xRRGGBB`.

use gpui::Hsla;

use crate::theme::Theme;

/// The named colours for a dark background, those of Windows Terminal.
const DARK: [u32; 16] = [
    0x0c0c0c, 0xc50f1f, 0x13a10e, 0xc19c00, 0x0037da, 0x881798, 0x3a96dd, 0xcccccc, 0x767676,
    0xe74856, 0x16c60c, 0xf9f1a5, 0x3b78ff, 0xb4009e, 0x61d6d6, 0xf2f2f2,
];

/// The named colours for a light background.
const LIGHT: [u32; 16] = [
    0x383a42, 0xe45649, 0x50a14f, 0xc18301, 0x0184bc, 0xa626a4, 0x0997b3, 0xa0a1a7, 0x4f525d,
    0xdf6c75, 0x3e953a, 0xb08000, 0x2f6fd8, 0xc577dd, 0x2d9cab, 0x202020,
];

/// How much of a colour a dimmed one keeps.
const DIM_SHARE: f32 = 0.66;

/// The colours one terminal draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub named: [u32; 16],
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
    pub selection: u32,
}

impl Palette {
    /// The colours for `theme`: its own for the text, the background, the
    /// cursor and the selection, and the named ones for its darkness,
    /// each of them replaced where `configured` gives one (`#rrggbb`, in
    /// the order of the 16).
    pub fn of(theme: &Theme, configured: &[String]) -> Self {
        let background = packed(theme.colors.surface_container);
        let dark = luminance(background) < 0.5;
        let mut named = if dark { DARK } else { LIGHT };
        for (slot, color) in named.iter_mut().zip(configured) {
            if let Some(color) = parse_hex(color) {
                *slot = color;
            }
        }
        Self {
            named,
            foreground: packed(theme.colors.on_surface),
            background,
            cursor: packed(theme.colors.primary),
            selection: packed(theme.colors.primary),
        }
    }

    /// The colour of the number `index` of the 256.
    pub fn indexed(&self, index: u8) -> u32 {
        match index {
            0..16 => self.named[index as usize],
            16..232 => {
                let cube = index - 16;
                let level = |step: u8| if step == 0 { 0 } else { 55 + 40 * step as u32 };
                let (red, green, blue) = (cube / 36, (cube / 6) % 6, cube % 6);
                level(red) << 16 | level(green) << 8 | level(blue)
            }
            232.. => {
                let gray = 8 + 10 * (index - 232) as u32;
                gray << 16 | gray << 8 | gray
            }
        }
    }
}

/// `0xRRGGBB` of `#rrggbb`; none for anything else.
pub fn parse_hex(text: &str) -> Option<u32> {
    let hex = text.trim().strip_prefix('#')?;
    (hex.len() == 6 && hex.chars().all(|digit| digit.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
}

/// `color` with a third less light, as dim text is drawn.
pub fn dimmed(color: u32) -> u32 {
    let part = |shift: u32| ((((color >> shift) & 0xff) as f32 * DIM_SHARE) as u32) << shift;
    part(16) | part(8) | part(0)
}

/// `0xRRGGBB` of a colour of GPUI.
pub fn packed(color: Hsla) -> u32 {
    let rgba = color.to_rgb();
    let part = |value: f32| (value.clamp(0., 1.) * 255.).round() as u32;
    part(rgba.r) << 16 | part(rgba.g) << 8 | part(rgba.b)
}

/// A colour of GPUI of `0xRRGGBB`.
pub fn hsla(color: u32) -> Hsla {
    gpui::rgb(color).into()
}

/// How light `color` looks, from 0 to 1.
fn luminance(color: u32) -> f32 {
    let part = |shift: u32| ((color >> shift) & 0xff) as f32 / 255.;
    0.2126 * part(16) + 0.7152 * part(8) + 0.0722 * part(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        Palette {
            named: DARK,
            foreground: 0xffffff,
            background: 0,
            cursor: 0xff0000,
            selection: 0xff0000,
        }
    }

    #[test]
    fn the_256_colours_are_the_named_a_cube_and_grays() {
        let palette = palette();
        assert_eq!(palette.indexed(1), DARK[1]);
        assert_eq!(palette.indexed(16), 0x000000);
        assert_eq!(palette.indexed(21), 0x0000ff);
        assert_eq!(palette.indexed(196), 0xff0000);
        assert_eq!(palette.indexed(231), 0xffffff);
        assert_eq!(palette.indexed(232), 0x080808);
        assert_eq!(palette.indexed(255), 0xeeeeee);
    }

    #[test]
    fn colours_are_read_and_dimmed() {
        assert_eq!(parse_hex("#1E1e1e"), Some(0x1e1e1e));
        assert_eq!(parse_hex("1e1e1e"), None);
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#gggggg"), None);
        assert_eq!(dimmed(0xffffff), 0xa8a8a8);
        assert!(luminance(0xffffff) > 0.99 && luminance(0) < 0.01);
    }

    #[test]
    fn a_colour_of_gpui_is_packed_and_back() {
        for color in [0x000000, 0x3a96dd, 0xffffff, 0xc50f1f] {
            assert_eq!(packed(hsla(color)), color);
        }
    }
}
