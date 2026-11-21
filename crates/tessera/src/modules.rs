// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The modules the shell is assembled from.

mod bluetooth;
mod calendar;
mod clock;
mod command;
mod keyboard;
mod label;
mod launcher;
mod marketplace;
mod media;
mod network;
mod notifications;
mod problems;
mod session;
mod settings;
mod sound;
mod start;
mod system;
mod tiling;
mod tray;
mod wifi;
mod windows;

use crate::config::Config;
use crate::extension::ExtensionsModule;
use crate::module::Module;

/// Every module the shell starts with; the last one loads extensions.
pub fn builtin(config: &Config) -> Vec<Box<dyn Module>> {
    vec![
        Box::new(clock::ClockModule),
        Box::new(bluetooth::BluetoothModule),
        Box::new(calendar::CalendarModule),
        Box::new(command::CommandModule),
        Box::new(keyboard::KeyboardModule),
        Box::new(label::LabelModule),
        Box::new(launcher::LauncherModule),
        Box::new(marketplace::MarketplaceModule {
            registries: config.registries(),
        }),
        Box::new(media::MediaModule),
        Box::new(network::NetworkModule),
        Box::new(notifications::NotificationsModule),
        Box::new(problems::ProblemsModule),
        Box::new(session::SessionModule),
        Box::new(settings::SettingsModule),
        Box::new(start::StartModule),
        Box::new(sound::SoundModule),
        Box::new(system::SystemModule),
        Box::new(tiling::TilingModule),
        Box::new(tray::TrayModule),
        Box::new(wifi::WifiModule),
        Box::new(windows::WindowsModule),
        Box::new(ExtensionsModule {
            settings: config.extensions.clone(),
        }),
    ]
}
