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

use crate::platform::{ShellHost, ShellServiceObjects, shell_mode};

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
    if config.service_objects && !before.service_objects {
        ShellServiceObjects::start();
    } else if !config.service_objects && before.service_objects {
        cx.background_executor()
            .spawn(async { ShellServiceObjects::close(CLOSING_TIME) })
            .detach();
    }
}
