// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shell-wide appearance tokens, following Material 3 role names.

use gpui::{Global, Hsla, Pixels, px, rgb};

/// Material 3 color roles used by shell surfaces.
#[derive(Clone, Copy, Debug)]
pub struct ColorRoles {
    pub surface_container: Hsla,
    pub surface_container_highest: Hsla,
    pub on_surface: Hsla,
    pub on_surface_variant: Hsla,
    pub outline: Hsla,
    pub primary: Hsla,
}

/// Corner radii.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub small: Pixels,
    pub medium: Pixels,
    pub full: Pixels,
}

/// Distances between and inside components.
#[derive(Clone, Copy, Debug)]
pub struct Spacing {
    pub tiny: Pixels,
    pub small: Pixels,
    pub medium: Pixels,
}

/// Dock measurements.
#[derive(Clone, Copy, Debug)]
pub struct DockMetrics {
    pub height: Pixels,
    pub item_size: Pixels,
    pub icon_size: Pixels,
    pub indicator_height: Pixels,
    pub indicator_active_width: Pixels,
}

/// The appearance shared by every shell surface.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub colors: ColorRoles,
    pub shape: Shape,
    pub spacing: Spacing,
    pub dock: DockMetrics,
    pub bar_height: Pixels,
    pub body_size: Pixels,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            colors: ColorRoles {
                surface_container: rgb(0x1e211a).into(),
                surface_container_highest: rgb(0x33362e).into(),
                on_surface: rgb(0xe2e3d9).into(),
                on_surface_variant: rgb(0xc4c8ba).into(),
                outline: rgb(0x8e9286).into(),
                primary: rgb(0xadd28e).into(),
            },
            shape: Shape {
                small: px(8.),
                medium: px(16.),
                full: px(9999.),
            },
            spacing: Spacing {
                tiny: px(4.),
                small: px(8.),
                medium: px(16.),
            },
            dock: DockMetrics {
                height: px(64.),
                item_size: px(52.),
                icon_size: px(32.),
                indicator_height: px(4.),
                indicator_active_width: px(16.),
            },
            bar_height: px(40.),
            body_size: px(14.),
        }
    }
}

impl Global for Theme {}
