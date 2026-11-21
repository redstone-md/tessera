// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Tessera desktop shell entry point.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod bar;
mod clock;
mod dock;
mod platform;
mod surface;
mod theme;

use gpui::{App, Application};

use crate::bar::Bar;
use crate::dock::Dock;
use crate::platform::ScreenEdge;
use crate::surface::EdgeSurface;
use crate::theme::Theme;

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.set_global(Theme::default());
        if let Err(error) = open_surfaces(cx) {
            eprintln!("tessera: the shell surfaces could not be opened: {error:#}");
            cx.quit();
        }
    });
}

/// Opens a bar and a dock on every display.
fn open_surfaces(cx: &mut App) -> anyhow::Result<()> {
    let theme = *cx.global::<Theme>();
    let bar = EdgeSurface {
        edge: ScreenEdge::Top,
        thickness: theme.bar_height,
    };
    let dock = EdgeSurface {
        edge: ScreenEdge::Bottom,
        thickness: theme.dock.height,
    };
    for display in cx.displays() {
        bar.open(&display, cx, Bar::new)?;
        dock.open(&display, cx, Dock::new)?;
    }
    Ok(())
}
