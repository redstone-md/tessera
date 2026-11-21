// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Tessera desktop shell entry point.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod bar;
mod clock;
mod platform;
mod theme;

use gpui::{App, Application};

use crate::bar::Bar;
use crate::theme::Theme;

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.set_global(Theme::default());
        if let Err(error) = Bar::open(cx) {
            eprintln!("tessera: the bar could not be opened: {error:#}");
            cx.quit();
        }
    });
}
