// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shell-wide appearance tokens, following Material 3 role names.

use gpui::{Global, Hsla, Pixels, px, rgb};

/// Material 3 color roles used by shell surfaces.
#[derive(Clone, Copy, Debug)]
pub struct ColorRoles {
    pub surface_container: Hsla,
    pub on_surface: Hsla,
}

/// The appearance shared by every shell surface.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub colors: ColorRoles,
    pub bar_height: Pixels,
    pub bar_inset: Pixels,
    pub body_size: Pixels,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            colors: ColorRoles {
                surface_container: rgb(0x1e211a).into(),
                on_surface: rgb(0xe2e3d9).into(),
            },
            bar_height: px(40.),
            bar_inset: px(16.),
            body_size: px(14.),
        }
    }
}

impl Global for Theme {}
