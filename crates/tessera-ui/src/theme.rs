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
}

impl PresentationTheme {
    pub(crate) fn uniform(scheme: ColorScheme) -> Self {
        Self {
            toolkit: scheme,
            reference: scheme,
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
        PresentationTheme {
            toolkit: self.global::<Palette>().get_color_scheme(),
            reference: self.global::<SeelenPalette>().get_color_scheme(),
        }
    }

    fn apply_presentation_theme(&self, theme: PresentationTheme) {
        self.global::<Palette>().set_color_scheme(theme.toolkit);
        self.global::<SeelenPalette>()
            .set_color_scheme(theme.reference);
    }
}
