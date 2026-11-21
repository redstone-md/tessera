// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Tessera desktop shell entry point.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod extension;
mod icons;
mod module;
mod modules;
mod motion;
mod panel;
mod platform;
mod popup;
mod surface;
mod theme;
mod ui;

use std::time::Duration;

use gpui::{App, Application, AsyncApp, px};

use crate::config::{Config, Displays};
use crate::icons::AppIcons;
use crate::module::WidgetRegistry;
use crate::panel::Panel;
use crate::platform::Taskbar;
use crate::popup::Popups;
use crate::surface::EdgeSurface;
use crate::theme::Theme;

/// How often the taskbar is checked for having been shown again.
const TASKBAR_CHECK_INTERVAL: Duration = Duration::from_secs(1);

fn main() {
    if Taskbar::run_watchdog_if_requested(std::env::args().skip(1)) {
        return;
    }
    let config = Config::load();
    Application::new().run(move |cx: &mut App| {
        cx.set_global(Theme::load(config.theme.as_deref()));
        cx.set_global(WidgetRegistry::from_modules(&modules::builtin()));
        cx.set_global(Popups::new(&config.popups));
        cx.set_global(AppIcons::new(&config.icons));
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
        if let Err(error) = open_surfaces(&config, cx) {
            eprintln!("tessera: the shell surfaces could not be opened: {error:#}");
            cx.quit();
        }
    });
}

/// Opens the configured surfaces on the displays each one asks for.
fn open_surfaces(config: &Config, cx: &mut App) -> anyhow::Result<()> {
    let primary = cx.primary_display().map(|display| display.id());
    for display in cx.displays() {
        for layout in &config.surfaces {
            if layout.displays == Displays::Primary && Some(display.id()) != primary {
                continue;
            }
            let surface = EdgeSurface {
                edge: layout.edge,
                thickness: px(layout.thickness),
            };
            surface.open(&display, cx, |display, cx| Panel::new(layout, display, cx))?;
        }
    }
    Ok(())
}
