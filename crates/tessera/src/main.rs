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

use std::time::Duration;

use gpui::{App, Application, AsyncApp};

use crate::bar::Bar;
use crate::dock::Dock;
use crate::platform::{ScreenEdge, Taskbar};
use crate::surface::EdgeSurface;
use crate::theme::Theme;

/// How often the taskbar is checked for having been shown again.
const TASKBAR_CHECK_INTERVAL: Duration = Duration::from_secs(1);

fn main() {
    if Taskbar::run_watchdog_if_requested(std::env::args().skip(1)) {
        return;
    }
    Application::new().run(|cx: &mut App| {
        cx.set_global(Theme::default());
        match Taskbar::hide_until_exit() {
            Ok(()) => cx
                .spawn(async move |cx: &mut AsyncApp| {
                    loop {
                        cx.background_executor().timer(TASKBAR_CHECK_INTERVAL).await;
                        Taskbar::keep_hidden();
                    }
                })
                .detach(),
            Err(error) => eprintln!("tessera: the Windows taskbar stays visible: {error:#}"),
        }
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
        bar.open(&display, cx, |_, cx| Bar::new(cx))?;
        dock.open(&display, cx, Dock::new)?;
    }
    Ok(())
}
