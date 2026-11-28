// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The modules the shell is assembled from.

mod airplane;
mod bluetooth;
mod brightness;
mod calendar;
mod clipboard;
mod clock;
mod command;
mod connections;
pub mod devices;
mod display_mode;
mod display_settings;
mod focused;
mod hdr;
mod hotspot;
mod keyboard;
mod label;
mod launcher;
mod look;
mod marketplace;
mod media;
mod more_windows_keys;
mod network;
pub mod night;
mod notifications;
mod overview;
mod plan;
mod popup_settings;
mod power_dialog;
mod power_mode;
mod power_settings;
mod privacy;
mod problems;
mod quick;
mod rules_editor;
mod run;
mod runner;
mod runner_settings;
mod session;
mod settings;
mod settings_parts;
mod shortcuts;
mod sound;
mod spacer;
mod start;
mod startup;
pub mod system;
mod tiling;
mod tiling_overrides;
mod trash;
mod tray;
mod update;
mod user;
mod wallpaper;
mod wallpaper_settings;
mod wifi;
pub mod windows;
mod windows_keys;
mod winx;
mod workspaces;

use crate::config::Config;
use crate::extension::ExtensionsModule;
use crate::files::FilesModule;
use crate::module::Module;
use crate::session::ShellModule;
pub use marketplace::Updates;
pub use power_dialog::DesktopClosing;
pub(crate) use runner_settings::RunnerCatalog;
pub use settings::Settings;
#[allow(unused_imports, reason = "for the Shortcuts page of shortcuts-editor")]
pub(crate) use settings::{ActionTitle, action_titles};
pub use shortcuts::record_for_window;

/// Every module the shell starts with; the last one loads extensions.
pub fn builtin(config: &Config) -> Vec<Box<dyn Module>> {
    vec![
        Box::new(clock::ClockModule),
        Box::new(bluetooth::BluetoothModule),
        Box::new(calendar::CalendarModule),
        Box::new(clipboard::ClipboardModule::of(&config.clipboard)),
        Box::new(command::CommandModule),
        Box::new(connections::ConnectionsModule),
        Box::new(display_mode::DisplayModeModule),
        Box::new(display_settings::DisplaySettingsModule),
        Box::new(devices::DevicesModule),
        Box::new(FilesModule),
        Box::new(focused::FocusedModule),
        Box::new(keyboard::KeyboardModule),
        Box::new(label::LabelModule),
        Box::new(launcher::LauncherModule),
        Box::new(run::RunModule),
        Box::new(runner::RunnerModule),
        Box::new(runner_settings::RunnerSettingsModule),
        Box::new(popup_settings::PopupSettingsModule),
        Box::new(rules_editor::RulesEditorModule),
        Box::new(marketplace::MarketplaceModule {
            registries: config.registries(),
        }),
        Box::new(media::MediaModule),
        Box::new(network::NetworkModule),
        Box::new(notifications::NotificationsModule),
        Box::new(problems::ProblemsModule),
        Box::new(session::SessionModule),
        Box::new(ShellModule),
        Box::new(settings::SettingsModule),
        Box::new(start::StartModule),
        Box::new(startup::StartupModule),
        Box::new(sound::SoundModule),
        Box::new(spacer::SpacerModule),
        Box::new(brightness::BrightnessModule),
        Box::new(system::SystemModule),
        Box::new(tiling::TilingModule),
        Box::new(tiling_overrides::TilingOverridesModule),
        Box::new(look::LookModule),
        Box::new(hdr::HdrModule),
        Box::new(plan::PlanModule),
        Box::new(power_dialog::PowerDialogModule),
        Box::new(privacy::PrivacyModule),
        Box::new(trash::TrashModule),
        Box::new(update::UpdateModule),
        Box::new(tray::TrayModule),
        Box::new(user::UserModule),
        Box::new(wallpaper::WallpaperModule),
        Box::new(wallpaper_settings::WallpaperSettingsModule),
        Box::new(wifi::WifiModule),
        Box::new(windows::WindowsModule),
        Box::new(windows_keys::WindowsKeysModule),
        Box::new(more_windows_keys::MoreWindowsKeysModule),
        Box::new(shortcuts::ShortcutsModule),
        Box::new(winx::WinxModule),
        Box::new(workspaces::WorkspacesModule),
        Box::new(overview::OverviewModule),
        Box::new(crate::snap::SnapModule),
        Box::new(quick::QuickModule),
        Box::new(airplane::AirplaneModule),
        Box::new(hotspot::HotspotModule),
        Box::new(power_mode::PowerModeModule),
        Box::new(power_settings::PowerSettingsModule),
        Box::new(night::NightModule),
        Box::new(crate::peek::PeekModule),
        Box::new(ExtensionsModule {
            settings: config.extensions.clone(),
        }),
    ]
}
