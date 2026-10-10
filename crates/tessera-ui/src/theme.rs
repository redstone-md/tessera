// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Source intent and complete theme propagation for separately owned native windows.

use material_colors::{color::Argb, scheme::Scheme, theme::ThemeBuilder};
use slint::language::ColorScheme;
use slint::{Color, ComponentHandle, Global};

use crate::SourceSeed;
use crate::generated::{MaterialRoles, Palette, SeelenPalette};

// Only consumed roles cross the presentation seam; the engine computes all 49.
macro_rules! roles {
    ($($field:ident),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq)]
        struct Roles { $( $field: Color, )+ }

        impl From<MaterialRoles> for Roles {
            fn from(roles: MaterialRoles) -> Self { Self { $( $field: roles.$field, )+ } }
        }
        impl From<Roles> for MaterialRoles {
            fn from(roles: Roles) -> Self { Self { $( $field: roles.$field, )+ } }
        }
    };
}
roles!(
    surface,
    surface_container_low,
    surface_container,
    surface_container_high,
    surface_container_highest,
    on_surface,
    on_surface_variant,
    primary,
    on_primary,
    primary_container,
    on_primary_container,
    outline,
    outline_variant,
    error,
    on_error,
    error_container,
    on_error_container,
    surface_hover,
    surface_pressed,
    primary_container_hover,
    primary_container_pressed
);

fn color(argb: Argb) -> Color {
    Color::from_rgb_u8(argb.red, argb.green, argb.blue)
}

/// Material interaction layers, orthogonal to semantic role generation.
/// Opaque sRGB channels round to nearest, independent of renderer interpolation.
fn state_paint(base: Argb, foreground: Argb, opacity: u16) -> Color {
    let channel = |base: u8, foreground: u8| {
        ((u16::from(base) * (100 - opacity) + u16::from(foreground) * opacity + 50) / 100) as u8
    };
    Color::from_rgb_u8(
        channel(base.red, foreground.red),
        channel(base.green, foreground.green),
        channel(base.blue, foreground.blue),
    )
}

impl From<Scheme> for Roles {
    fn from(s: Scheme) -> Self {
        Self {
            surface: color(s.surface),
            surface_container_low: color(s.surface_container_low),
            surface_container: color(s.surface_container),
            surface_container_high: color(s.surface_container_high),
            surface_container_highest: color(s.surface_container_highest),
            on_surface: color(s.on_surface),
            on_surface_variant: color(s.on_surface_variant),
            primary: color(s.primary),
            on_primary: color(s.on_primary),
            primary_container: color(s.primary_container),
            on_primary_container: color(s.on_primary_container),
            outline: color(s.outline),
            outline_variant: color(s.outline_variant),
            error: color(s.error),
            on_error: color(s.on_error),
            error_container: color(s.error_container),
            on_error_container: color(s.on_error_container),
            surface_hover: state_paint(s.surface, s.on_surface, 8),
            surface_pressed: state_paint(s.surface, s.on_surface, 12),
            primary_container_hover: state_paint(s.primary_container, s.on_primary_container, 8),
            primary_container_pressed: state_paint(s.primary_container, s.on_primary_container, 12),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PresentationTheme {
    toolkit: ColorScheme,
    reference: ColorScheme,
    reference_mode: bool,
    seed: SourceSeed,
    light: Roles,
    dark: Roles,
}

impl Default for PresentationTheme {
    fn default() -> Self {
        Self::uniform(ColorScheme::Unknown)
    }
}

#[cfg(test)]
impl From<crate::Theme> for PresentationTheme {
    fn from(theme: crate::Theme) -> Self {
        Self::uniform(match theme {
            crate::Theme::System => ColorScheme::Unknown,
            crate::Theme::Light => ColorScheme::Light,
            crate::Theme::Dark => ColorScheme::Dark,
        })
    }
}

impl PresentationTheme {
    pub(crate) fn uniform(scheme: ColorScheme) -> Self {
        Self::from_source(scheme, SourceSeed::default())
    }

    pub(crate) fn from_source(scheme: ColorScheme, seed: SourceSeed) -> Self {
        // 0.4.2's default is TonalSpot, standard contrast, opaque source.
        let theme = ThemeBuilder::with_source(Argb::from_u32(0xff000000 | seed.rgb())).build();
        Self {
            toolkit: scheme,
            reference: scheme,
            reference_mode: false,
            seed,
            light: theme.schemes.light.into(),
            dark: theme.schemes.dark.into(),
        }
    }

    /// Retained neutral source fixture, selected explicitly by renderer tests.
    #[cfg(test)]
    pub(crate) fn seelen_reference(scheme: ColorScheme) -> Self {
        Self {
            reference_mode: true,
            ..Self::uniform(scheme)
        }
    }
}

pub(crate) trait ThemedComponent: ComponentHandle {
    fn presentation_theme(&self) -> PresentationTheme;
    fn apply_presentation_theme(&self, theme: PresentationTheme);
    fn apply_presentation_theme_scoped(
        &self,
        theme: PresentationTheme,
        current: impl FnMut() -> bool,
    );
}

impl<C: ComponentHandle> ThemedComponent for C
where
    for<'a> Palette<'a>: Global<'a, C>,
    for<'a> SeelenPalette<'a>: Global<'a, C>,
{
    fn presentation_theme(&self) -> PresentationTheme {
        let palette = self.global::<SeelenPalette>();
        PresentationTheme {
            toolkit: self.global::<Palette>().get_color_scheme(),
            reference: palette.get_color_scheme(),
            reference_mode: palette.get_reference_mode(),
            seed: SourceSeed::from_rgb(palette.get_source_rgb() as u32)
                .expect("presentation source is validated RGB"),
            light: palette.get_light_roles().into(),
            dark: palette.get_dark_roles().into(),
        }
    }

    fn apply_presentation_theme(&self, theme: PresentationTheme) {
        self.apply_presentation_theme_scoped(theme, || true);
    }

    fn apply_presentation_theme_scoped(
        &self,
        theme: PresentationTheme,
        mut current: impl FnMut() -> bool,
    ) {
        // Reentrant generated effects may retire an actor between properties.
        macro_rules! apply {
            ($effect:expr) => {
                if !current() {
                    return;
                }
                $effect;
                if !current() {
                    return;
                }
            };
        }
        // Keep toolkit/native controls native; only its scheme is selected.
        apply!(self.global::<Palette>().set_color_scheme(theme.toolkit));
        let palette = self.global::<SeelenPalette>();
        apply!(palette.set_light_roles(theme.light.into()));
        apply!(palette.set_dark_roles(theme.dark.into()));
        apply!(palette.set_source_rgb(theme.seed.rgb() as i32));
        apply!(palette.set_color_scheme(theme.reference));
        apply!(palette.set_reference_mode(theme.reference_mode));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::{ContextMenuSurface, Panel, PowerMenuSurface, TooltipSurface};

    #[test]
    fn default_tonal_spot_roles_and_state_paint_match_independent_published_probe() {
        // Independent material-colors 0.4.2 locked probe, seed #7ca45c.
        // Expected pixels are literals, not calls to the production role helper.
        let theme = PresentationTheme::uniform(ColorScheme::Light);
        assert_eq!(theme.seed.rgb(), 0x7ca45c);
        assert_eq!(theme.light.primary, Color::from_rgb_u8(0x48, 0x67, 0x2f));
        assert_eq!(theme.light.surface, Color::from_rgb_u8(0xf9, 0xfa, 0xef));
        assert_eq!(theme.dark.primary, Color::from_rgb_u8(0xad, 0xd2, 0x8e));
        assert_eq!(theme.dark.surface, Color::from_rgb_u8(0x11, 0x14, 0x0e));
        for (actual, [r, g, b]) in [
            (theme.light.surface_hover, [0xe7, 0xe8, 0xde]),
            (theme.light.surface_pressed, [0xde, 0xdf, 0xd5]),
            (theme.light.primary_container_hover, [0xb9, 0xde, 0x9b]),
            (theme.light.primary_container_pressed, [0xb1, 0xd5, 0x94]),
            (theme.dark.surface_hover, [0x22, 0x25, 0x1e]),
            (theme.dark.surface_pressed, [0x2a, 0x2d, 0x26]),
            (theme.dark.primary_container_hover, [0x3d, 0x5c, 0x24]),
            (theme.dark.primary_container_pressed, [0x43, 0x62, 0x2a]),
        ] {
            assert_eq!(actual, Color::from_rgb_u8(r, g, b));
        }
    }

    #[test]
    fn chosen_sources_generate_distinct_deterministic_contrasting_role_pairs() {
        use material_colors::contrast::ratio_of_tones;
        let tone = |color: Color| {
            let rgb = color.to_argb_u8();
            Argb::new(255, rgb.red, rgb.green, rgb.blue).as_lstar()
        };
        let mut primaries = std::collections::HashSet::new();
        for rgb in [0x7ca45c, 0xffbd59, 0x4d90fe, 0xff7849, 0xe78cba] {
            let seed = SourceSeed::from_rgb(rgb).unwrap();
            let theme = PresentationTheme::from_source(ColorScheme::Dark, seed);
            assert_eq!(
                theme,
                PresentationTheme::from_source(ColorScheme::Dark, seed)
            );
            let primary = theme.light.primary.to_argb_u8();
            assert!(primaries.insert((primary.red, primary.green, primary.blue)));
            for roles in [theme.light, theme.dark] {
                for (foreground, background) in [
                    (roles.on_primary, roles.primary),
                    (roles.on_primary_container, roles.primary_container),
                    (roles.on_surface, roles.surface),
                    (roles.on_error, roles.error),
                ] {
                    assert!(ratio_of_tones(tone(foreground), tone(background)) >= 4.5);
                }
            }
        }
    }

    #[test]
    fn source_pair_capture_restoration_and_lazy_children_preserve_exact_paint() {
        i_slint_backend_testing::init_no_event_loop();
        let panel = Panel::new().unwrap();
        let original = PresentationTheme::from_source(
            ColorScheme::Dark,
            SourceSeed::from_rgb(0x123456).unwrap(),
        );
        panel.apply_presentation_theme(original);
        // Capture transports the actual role pair, including a retained override;
        // it must not silently regenerate paint from seed intent.
        let palette = panel.global::<SeelenPalette>();
        let mut light = palette.get_light_roles();
        light.primary = Color::from_rgb_u8(1, 2, 3);
        palette.set_light_roles(light);
        let captured = panel.presentation_theme();
        assert_eq!(captured.seed.rgb(), 0x123456);
        let context = ContextMenuSurface::new().unwrap();
        context.apply_presentation_theme(captured);
        assert_eq!(context.presentation_theme(), captured);
        let tooltip = TooltipSurface::new().unwrap();
        tooltip.apply_presentation_theme(captured);
        assert_eq!(tooltip.presentation_theme(), captured);
        let power = PowerMenuSurface::new().unwrap();
        power.apply_presentation_theme(captured);
        assert_eq!(power.presentation_theme(), captured);
        panel.apply_presentation_theme(PresentationTheme::from_source(
            ColorScheme::Dark,
            SourceSeed::from_rgb(0xffbd59).unwrap(),
        ));
        assert_ne!(panel.presentation_theme(), captured);
        panel.apply_presentation_theme(captured);
        assert_eq!(panel.presentation_theme(), captured);
        assert_eq!(
            panel.global::<Palette>().get_color_scheme(),
            ColorScheme::Dark
        );
        let neutral = PresentationTheme::seelen_reference(ColorScheme::Light);
        panel.apply_presentation_theme(neutral);
        tooltip.apply_presentation_theme(panel.presentation_theme());
        assert_eq!(tooltip.presentation_theme(), neutral);
        assert_eq!(
            tooltip.global::<SeelenPalette>().get_surface(),
            Color::from_rgb_u8(0xf2, 0xf2, 0xf2)
        );
    }
}
