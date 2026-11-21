// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The modules the shell is assembled from.

mod calendar;
mod clock;
mod label;
mod launcher;
mod notifications;
mod session;
mod sound;
mod system;
mod tray;
mod windows;

use crate::config::Config;
use crate::extension::ExtensionsModule;
use crate::module::Module;

/// Every module the shell starts with; the last one loads extensions.
pub fn builtin(config: &Config) -> Vec<Box<dyn Module>> {
    vec![
        Box::new(clock::ClockModule),
        Box::new(calendar::CalendarModule),
        Box::new(label::LabelModule),
        Box::new(launcher::LauncherModule),
        Box::new(notifications::NotificationsModule),
        Box::new(session::SessionModule),
        Box::new(sound::SoundModule),
        Box::new(system::SystemModule),
        Box::new(tray::TrayModule),
        Box::new(windows::WindowsModule),
        Box::new(ExtensionsModule {
            settings: config.extensions.clone(),
        }),
    ]
}
