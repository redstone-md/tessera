// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The services Explorer hosts for other programs, hosted by Tessera
//! where it is the shell of the session, as `[shell_services]` says.
//!
//! - The recorder of the messages the desktop and the task manager window
//!   leave unanswered (`log_messages`), so that one run on Windows shows
//!   which protocols programs still speak to the shell.
//! - The server of the Shell's change notifications (`change_server`):
//!   its delivery is written, its windows wait until the recorder shows
//!   that Windows speaks the protocol.
//! - The program manager's DDE (`dde`), through which old installers make
//!   the groups and shortcuts of the Start menu.
//! - The objects of `ShellServiceObjects` (`service_objects`), which
//!   programs register to be started with the shell.
//!
//! The automation of the folder windows (`ShellWindows`, the desktop's
//! view, `Shell.Application`) needs no setting: it is served where Tessera
//! is the shell, and this part follows the windows of the file manager for
//! it.

use std::time::Duration;

use gpui::{App, Global};
use serde::Deserialize;

use crate::files::{Files, WindowsChange};
use crate::platform::{
    Launch, ProgmanDde, ShellHost, ShellServiceObjects, ShellWindows, shell_mode,
};

/// What `[shell_services]` of the configuration says.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ShellServicesConfig {
    /// Whether the messages the windows of the shell leave unanswered are
    /// written to `%LOCALAPPDATA%\tessera\shell-messages.log`, as the
    /// shell.
    pub log_messages: bool,
    /// Whether the desktop serves the change notifications of the Shell.
    /// The server waits for the protocol to be confirmed on Windows; until
    /// then this only says so.
    pub change_server: bool,
    /// Whether the program manager's DDE (`PROGMAN`) is answered, as the
    /// shell, so that old installers can make groups and shortcuts.
    pub dde: bool,
    /// Whether the objects registered under `ShellServiceObjects` are
    /// started with the shell and closed at its end.
    pub service_objects: bool,
}

impl Default for ShellServicesConfig {
    fn default() -> Self {
        Self {
            log_messages: true,
            change_server: false,
            dde: true,
            service_objects: false,
        }
    }
}

/// How long quitting waits for the objects of `ShellServiceObjects` to
/// close.
const CLOSING_TIME: Duration = Duration::from_millis(1500);

/// The `[shell_services]` in use.
struct ShellServices(ShellServicesConfig);

impl Global for ShellServices {}

/// Starts the services `config` asks for, once the desktop is ready.
pub fn start(config: &ShellServicesConfig, cx: &mut App) {
    ShellHost::record_messages(config.log_messages);
    cx.set_global(ShellServices(config.clone()));
    if !shell_mode() {
        return;
    }
    if config.change_server {
        report!(
            "the change notifications of the Shell are not served yet: the protocol waits to be confirmed on Windows"
        );
    }
    if config.service_objects {
        ShellServiceObjects::start();
    }
    if config.dde {
        ProgmanDde::start();
    }
    show_program_groups(cx);
    follow_files_windows(cx);
    // Quitting closes the objects away from the thread of GPUI, which
    // waits for that at most as long as it waits for any part.
    cx.on_app_quit(|cx| {
        let closing = cx
            .background_executor()
            .spawn(async { ShellServiceObjects::close(CLOSING_TIME) });
        async move {
            if !closing.await {
                report!("the objects that start with the shell did not close in time");
            }
        }
    })
    .detach();
}

/// Applies a new `[shell_services]`: a service turned on starts, one
/// turned off stops.
pub fn configure(config: &ShellServicesConfig, cx: &mut App) {
    ShellHost::record_messages(config.log_messages);
    let before = cx
        .try_global::<ShellServices>()
        .map(|services| services.0.clone())
        .unwrap_or_default();
    cx.set_global(ShellServices(config.clone()));
    if !shell_mode() {
        return;
    }
    match (config.dde, before.dde) {
        (true, false) => ProgmanDde::start(),
        (false, true) => ProgmanDde::stop(),
        _ => {}
    }
    if config.service_objects && !before.service_objects {
        ShellServiceObjects::start();
    } else if !config.service_objects && before.service_objects {
        cx.background_executor()
            .spawn(async { ShellServiceObjects::close(CLOSING_TIME) })
            .detach();
    }
}

/// Shows the folders the program manager's commands ask for, as
/// `ShowGroup` does after an installer made a group: in the file manager
/// where it takes folders, and through the Shell otherwise.
fn show_program_groups(cx: &mut App) {
    let folders = ProgmanDde::folders();
    cx.spawn(async move |cx| {
        while let Ok(folder) = folders.recv().await {
            let shown = cx.update(|cx| {
                if !Files::open_folder(&folder, cx) {
                    Launch::new(&folder).start();
                }
            });
            if shown.is_err() {
                break;
            }
        }
    })
    .detach();
}

/// Tells the list of folder windows of the Shell the parsing name of the
/// place each window of the file manager shows, and sends a window where
/// a program asked it to go (`Navigate`, `Navigate2`, `Refresh`); a place
/// the window cannot show, and one asked of the desktop, opens in a new
/// window. Nothing here waits for the list.
fn follow_files_windows(cx: &mut App) {
    for window in Files::windows(cx) {
        ShellWindows::named(window.hwnd, window.parsing.clone());
    }
    Files::observe_windows(cx, |change, _| match change {
        WindowsChange::Opened(window) | WindowsChange::Navigated(window) => {
            ShellWindows::named(window.hwnd, window.parsing.clone());
        }
        WindowsChange::Closed(_) => {}
    });
    let navigations = ShellWindows::navigations();
    cx.spawn(async move |cx| {
        while let Ok(navigation) = navigations.recv().await {
            let went = cx.update(|cx| {
                let moved = !navigation.new_window
                    && Files::navigate(navigation.window, &navigation.parsing, cx);
                if !moved {
                    Files::open_named(&navigation.parsing, cx);
                }
            });
            if went.is_err() {
                break;
            }
        }
    })
    .detach();
}
