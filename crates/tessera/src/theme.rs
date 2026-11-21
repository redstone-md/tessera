// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shell-wide appearance tokens, following Material 3 role names.

use std::time::Duration;

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
    pub secondary_container: Hsla,
}

/// Corner radii.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub small: Pixels,
    pub medium: Pixels,
    pub large: Pixels,
    pub full: Pixels,
}

/// Distances between and inside components.
#[derive(Clone, Copy, Debug)]
pub struct Spacing {
    pub tiny: Pixels,
    pub small: Pixels,
    pub medium: Pixels,
}

/// Text styles.
#[derive(Clone, Copy, Debug)]
pub struct Typography {
    pub family: &'static str,
    pub label_size: Pixels,
}

/// Durations of state changes.
#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub short: Duration,
}

/// Bar measurements.
#[derive(Clone, Copy, Debug)]
pub struct BarMetrics {
    /// Screen space reserved for the bar, including the gap around its pills.
    pub height: Pixels,
    pub pill_height: Pixels,
}

/// Dock measurements.
#[derive(Clone, Copy, Debug)]
pub struct DockMetrics {
    /// Screen space reserved for the dock, including the gap around it.
    pub height: Pixels,
    pub container_height: Pixels,
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
    pub typography: Typography,
    pub motion: Motion,
    pub bar: BarMetrics,
    pub dock: DockMetrics,
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
                secondary_container: rgb(0x404a36).into(),
            },
            shape: Shape {
                small: px(8.),
                medium: px(16.),
                large: px(28.),
                full: px(9999.),
            },
            spacing: Spacing {
                tiny: px(4.),
                small: px(8.),
                medium: px(16.),
            },
            typography: Typography {
                family: "Segoe UI Variable Display",
                label_size: px(14.),
            },
            motion: Motion {
                short: Duration::from_millis(250),
            },
            bar: BarMetrics {
                height: px(48.),
                pill_height: px(36.),
            },
            dock: DockMetrics {
                height: px(76.),
                container_height: px(64.),
                item_size: px(48.),
                icon_size: px(32.),
                indicator_height: px(4.),
                indicator_active_width: px(16.),
            },
        }
    }
}

impl Global for Theme {}
