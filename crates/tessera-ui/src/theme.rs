// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Theme propagation for separately owned native component windows.

use slint::language::ColorScheme;
use slint::{ComponentHandle, Global};

use crate::generated::{Palette, SeelenPalette};

#[derive(Clone, Copy)]
pub(crate) struct PresentationTheme {
    toolkit: ColorScheme,
    reference: ColorScheme,
    reference_mode: bool,
}

impl PresentationTheme {
    pub(crate) fn uniform(scheme: ColorScheme) -> Self {
        Self {
            toolkit: scheme,
            reference: scheme,
            reference_mode: false,
        }
    }

    /// Retained neutral source fixture, selected explicitly in all builds.
    /// Product windows use `uniform`; children inherit the captured skin.
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
}

impl<C: ComponentHandle> ThemedComponent for C
where
    for<'a> Palette<'a>: Global<'a, C>,
    for<'a> SeelenPalette<'a>: Global<'a, C>,
{
    fn presentation_theme(&self) -> PresentationTheme {
        let palette = self.global::<SeelenPalette>();
        let mut theme = if palette.get_reference_mode() {
            PresentationTheme::seelen_reference(palette.get_color_scheme())
        } else {
            PresentationTheme::uniform(palette.get_color_scheme())
        };
        theme.toolkit = self.global::<Palette>().get_color_scheme();
        theme
    }

    fn apply_presentation_theme(&self, theme: PresentationTheme) {
        self.global::<Palette>().set_color_scheme(theme.toolkit);
        self.global::<SeelenPalette>()
            .set_color_scheme(theme.reference);
        self.global::<SeelenPalette>()
            .set_reference_mode(theme.reference_mode);
    }
}
