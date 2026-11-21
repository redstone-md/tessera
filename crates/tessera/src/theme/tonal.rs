// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Color roles derived from one seed color.
//!
//! The roles are tones of palettes that share the hue of the seed, as in
//! Material 3, but the palettes are built in the OKLCH color space rather
//! than in Material's HCT, so the results are close to those of the
//! Material tools without being equal to them.

use gpui::rgb;

use super::ColorRoles;

/// How colorful each palette is, as OKLCH chroma.
const PRIMARY_CHROMA: f32 = 0.11;
const SECONDARY_CHROMA: f32 = 0.045;
const NEUTRAL_CHROMA: f32 = 0.012;
const NEUTRAL_VARIANT_CHROMA: f32 = 0.022;

/// A seed less colorful than this has no hue to follow, and gives grays.
const GRAY_SEED: f32 = 0.01;

/// The roles for the seed `[red, green, blue]`: light text on dark
/// surfaces, or, if `light`, dark text on light surfaces.
pub fn roles(seed: [u8; 3], light: bool) -> ColorRoles {
    let [_, a, b] = oklab(seed.map(|channel| linear(f32::from(channel) / 255.)));
    let colorful = a.hypot(b) >= GRAY_SEED;
    let hue = b.atan2(a);
    let role = |chroma: f32, tone: f32| {
        let chroma = if colorful { chroma } else { 0. };
        rgb(color(hue, chroma, tone)).into()
    };
    // The tones of the Material 3 schemes.
    let tone = |dark: f32, in_light: f32| if light { in_light } else { dark };
    ColorRoles {
        surface_container: role(NEUTRAL_CHROMA, tone(12., 94.)),
        surface_container_highest: role(NEUTRAL_CHROMA, tone(22., 90.)),
        on_surface: role(NEUTRAL_CHROMA, tone(90., 10.)),
        on_surface_variant: role(NEUTRAL_VARIANT_CHROMA, tone(80., 30.)),
        outline: role(NEUTRAL_VARIANT_CHROMA, tone(60., 50.)),
        primary: role(PRIMARY_CHROMA, tone(80., 40.)),
        secondary_container: role(SECONDARY_CHROMA, tone(30., 90.)),
    }
}

/// The color of the hue `hue`, in radians, that is as light as the tone
/// `tone`, 0 to 100, and as colorful as `chroma` or as the screen allows,
/// as `0xrrggbb`.
fn color(hue: f32, chroma: f32, tone: f32) -> u32 {
    let lightness = luminance(tone).cbrt();
    let at = |chroma: f32| srgb([lightness, chroma * hue.cos(), chroma * hue.sin()]);
    let shown = |channels: [f32; 3]| channels.iter().all(|c| (-0.0005..=1.0005).contains(c));
    let mut channels = at(chroma);
    if !shown(channels) {
        // The most colorful one the screen shows lies between none and all
        // of the chroma.
        let (mut low, mut high) = (0., chroma);
        for _ in 0..16 {
            let middle = (low + high) / 2.;
            if shown(at(middle)) {
                low = middle;
            } else {
                high = middle;
            }
        }
        channels = at(low);
    }
    channels.iter().fold(0, |packed, &channel| {
        let byte = (encoded(channel.clamp(0., 1.)) * 255.).round() as u32;
        packed << 8 | byte
    })
}

/// The share of light the tone `tone` reflects: the inverse of CIE L*.
fn luminance(tone: f32) -> f32 {
    if tone > 8. {
        ((tone + 16.) / 116.).powi(3)
    } else {
        tone / 903.3
    }
}

/// A channel of sRGB as the amount of light it stands for.
fn linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

/// The inverse of [`linear`].
fn encoded(light: f32) -> f32 {
    if light <= 0.003_130_8 {
        light * 12.92
    } else {
        1.055 * light.powf(1. / 2.4) - 0.055
    }
}

/// Linear sRGB as OKLab `[lightness, a, b]`.
fn oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OKLab as linear sRGB, which is outside 0 to 1 for a color no screen
/// shows.
fn srgb([lightness, a, b]: [f32; 3]) -> [f32; 3] {
    let l = (lightness + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (lightness - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (lightness - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_93 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channels(packed: u32) -> [u32; 3] {
        [packed >> 16 & 0xff, packed >> 8 & 0xff, packed & 0xff]
    }

    /// CIE L* of a packed color.
    fn tone(packed: u32) -> f32 {
        let [r, g, b] = channels(packed).map(|channel| linear(channel as f32 / 255.));
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        if y > 0.008_856 {
            116. * y.cbrt() - 16.
        } else {
            903.3 * y
        }
    }

    #[test]
    fn the_ends_of_the_tones_are_black_and_white() {
        assert_eq!(color(1., 0.1, 0.), 0x000000);
        assert_eq!(color(1., 0.1, 100.), 0xffffff);
    }

    #[test]
    fn a_color_without_chroma_is_a_gray_of_its_tone() {
        for wanted in [12., 22., 60., 90.] {
            let gray = color(0., 0., wanted);
            let [r, g, b] = channels(gray);
            assert!(r == g && g == b, "{gray:06x} is tinted");
            assert!(
                (tone(gray) - wanted).abs() < 1.,
                "{gray:06x} is not tone {wanted}"
            );
        }
    }

    #[test]
    fn a_color_keeps_the_hue_and_about_the_tone_asked_for() {
        // Around the circle, with more chroma than a screen shows for some.
        for step in 0..12 {
            let hue = step as f32 * std::f32::consts::TAU / 12.;
            let packed = color(hue, 0.2, 80.);
            assert!(
                (tone(packed) - 80.).abs() < 6.,
                "{packed:06x} is not near tone 80"
            );
            let [_, a, b] = oklab(channels(packed).map(|channel| linear(channel as f32 / 255.)));
            let turn = (b.atan2(a) - hue).rem_euclid(std::f32::consts::TAU);
            let off = turn.min(std::f32::consts::TAU - turn);
            assert!(off < 0.05, "{packed:06x} left the hue by {off}");
        }
    }

    #[test]
    fn the_roles_follow_the_seed() {
        let blue = roles([0x00, 0x78, 0xd4], false);
        let red = roles([0xe8, 0x11, 0x23], false);
        assert_ne!(blue.primary, red.primary);
        assert_ne!(blue.surface_container, red.surface_container);
        // Text stands out from the surface it is on.
        assert!(blue.on_surface.l > blue.surface_container.l + 0.5);
        assert!(blue.primary.l > blue.secondary_container.l + 0.3);

        let gray = roles([0x80, 0x80, 0x80], false);
        assert_eq!(gray.primary.s, 0.);
        assert_eq!(gray.surface_container.s, 0.);
    }

    #[test]
    fn a_light_scheme_puts_dark_text_on_light_surfaces() {
        let light = roles([0x00, 0x78, 0xd4], true);
        assert!(light.surface_container.l > light.on_surface.l + 0.5);
        assert!(light.surface_container.l > light.primary.l + 0.3);
        assert!(light.surface_container.l > light.surface_container_highest.l);
        assert!(light.secondary_container.l > light.primary.l + 0.3);
    }
}
