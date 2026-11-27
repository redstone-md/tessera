// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The modules the shell is assembled from.

mod bluetooth;
mod brightness;
mod calendar;
mod clock;
mod command;
mod connections;
pub mod devices;
mod focused;
mod hdr;
mod keyboard;
mod label;
mod launcher;
mod look;
mod marketplace;
mod media;
mod network;
mod notifications;
mod overview;
mod plan;
mod problems;
mod session;
mod settings;
mod sound;
mod start;
mod system;
mod tiling;
mod trash;
mod tray;
mod update;
mod user;
mod wifi;
pub mod windows;
mod workspaces;

use crate::config::Config;
use crate::extension::ExtensionsModule;
use crate::files::FilesModule;
use crate::module::Module;

/// Every module the shell starts with; the last one loads extensions.
pub fn builtin(config: &Config) -> Vec<Box<dyn Module>> {
    vec![
        Box::new(clock::ClockModule),
        Box::new(bluetooth::BluetoothModule),
        Box::new(calendar::CalendarModule),
        Box::new(command::CommandModule),
        Box::new(connections::ConnectionsModule),
        Box::new(devices::DevicesModule),
        Box::new(FilesModule),
        Box::new(focused::FocusedModule),
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
        Box::new(brightness::BrightnessModule),
        Box::new(system::SystemModule),
        Box::new(tiling::TilingModule),
        Box::new(look::LookModule),
        Box::new(hdr::HdrModule),
        Box::new(plan::PlanModule),
        Box::new(trash::TrashModule),
        Box::new(update::UpdateModule),
        Box::new(tray::TrayModule),
        Box::new(user::UserModule),
        Box::new(wifi::WifiModule),
        Box::new(windows::WindowsModule),
        Box::new(workspaces::WorkspacesModule),
        Box::new(overview::OverviewModule),
        Box::new(ExtensionsModule {
            settings: config.extensions.clone(),
        }),
    ]
}
