// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Tessera desktop shell entry point.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[macro_use]
mod report;

mod actions;
mod badges;
mod banners;
mod config;
mod corners;
mod desk;
mod extension;
mod field;
mod files;
mod flyout;
mod i18n;
mod icons;
mod keys;
mod menu;
mod module;
mod modules;
mod motion;
mod panel;
mod platform;
mod popup;
mod preview;
mod rules;
mod runners;
mod session;
mod shell;
mod snap;
mod startup;
mod surface;
mod switcher;
mod theme;
mod tiling;
mod tip;
mod ui;
mod wallpaper;
mod window_menu;
mod workspaces;

use std::time::Duration;

use gpui::{App, Application, AsyncApp};

use crate::config::Config;
use crate::platform::Taskbar;
use crate::shell::Shell;

/// How often the taskbar is checked for having been shown again.
const TASKBAR_CHECK_INTERVAL: Duration = Duration::from_secs(1);

fn main() {
    if Taskbar::run_watchdog_if_requested(std::env::args().skip(1))
        || platform::read_desktop_if_requested(std::env::args().skip(1))
        || session::hand_over_if_requested(std::env::args().skip(1))
    {
        return;
    }
    if let Some(code) = session::run_command_if_requested(std::env::args().skip(1)) {
        std::process::exit(code);
    }
    if let Some(handed) = keys::send_if_requested(std::env::args().skip(1)) {
        std::process::exit(if handed { 0 } else { 1 });
    }
    session::detect(std::env::args().skip(1));
    // As the shell, the start is written down before anything else, and
    // the session may go to Explorer instead.
    if !session::guard(std::env::args().skip(1)) {
        return;
    }
    Taskbar::wait_for_predecessor(std::env::args().skip(1));
    platform::Pace::keep();
    // Decided before anything is opened; as the shell of the session
    // Tessera finds no taskbar of Explorer to hide.
    let shell = platform::shell_mode();
    let mut config = Config::load();
    session::adjust(&mut config);
    // As the shell, the desktop window is made (waiting at most two
    // seconds) before the wallpaper looks for it.
    session::start_host(&config.shell);
    Application::new().run(move |cx: &mut App| {
        match (!shell).then(Taskbar::hide_until_exit) {
            None => {}
            Some(Ok(())) => cx
                .spawn(async move |cx: &mut AsyncApp| {
                    loop {
                        cx.background_executor().timer(TASKBAR_CHECK_INTERVAL).await;
                        Taskbar::keep_hidden();
                    }
                })
                .detach(),
            Some(Err(error)) => report!("the Windows taskbar stays visible: {error:#}"),
        }
        // Before the surfaces open, so that a start that fails also
        // leaves the session to Explorer.
        session::app_started(&config.shell, cx);
        if let Err(error) = Shell::start(config, cx) {
            report!("the shell does not start: {error:#}");
            cx.quit();
        }
    });
}
