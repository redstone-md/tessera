// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Starting the programs that start with the session, when Tessera is the
//! shell, once per session and a moment after the bars are up; beside
//! Explorer, Explorer starts them and Tessera never does. What starts and
//! in which order is told in `platform::win32::startup`.

use std::thread;
use std::time::Duration;

use gpui::{App, AsyncApp, Global};
use serde::Deserialize;

use crate::platform::{SessionStartup, shell_mode};

/// What `[startup]` in the configuration says.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StartupConfig {
    /// Whether Tessera, as the shell, starts the programs that start with
    /// the session.
    pub run: bool,
    /// How many seconds after the bars opened they start.
    pub delay: u64,
    /// Whether the components of Active Setup run first.
    pub active_setup: bool,
    /// Whether the startup tasks of packaged (Store) applications start.
    pub packaged: bool,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            run: true,
            delay: 2,
            active_setup: true,
            packaged: true,
        }
    }
}

/// What the start of this Tessera did about the programs of the session,
/// for the settings to tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Startup {
    /// Explorer is the shell and starts them.
    BesideExplorer,
    /// `[startup]` says `run = false`.
    Off,
    /// Tessera starts them, or started them earlier in the session.
    Starting,
}

impl Global for Startup {}

impl Startup {
    /// Starts the programs of the session after `config.delay` seconds,
    /// on a thread of their own, where Tessera is the shell and the
    /// configuration does not say otherwise.
    pub fn start(config: &StartupConfig, cx: &mut App) {
        let this = match (shell_mode(), config.run) {
            (false, _) => Self::BesideExplorer,
            (true, false) => Self::Off,
            (true, true) => Self::Starting,
        };
        cx.set_global(this);
        if this != Self::Starting {
            return;
        }
        let startup = SessionStartup {
            active_setup: config.active_setup,
            packaged: config.packaged,
        };
        let delay = Duration::from_secs(config.delay);
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor().timer(delay).await;
            // Programs are waited for, for a minute at most each, so the
            // work is on a thread of its own and not one of GPUI's.
            let spawned = thread::Builder::new()
                .name("startup".to_owned())
                .spawn(move || {
                    if let Err(error) = startup.run_once_per_session() {
                        report!("the programs of the session do not start: {error:#}");
                    }
                });
            if let Err(error) = spawned {
                report!("the programs of the session do not start: {error}");
            }
        })
        .detach();
    }

    /// What the start did, as the settings say it.
    pub fn told(cx: &App) -> &'static str {
        match cx.try_global::<Self>() {
            Some(Self::Starting) => "Tessera starts the programs that are on when you sign in.",
            Some(Self::Off) => "Tessera starts none of them, as [startup] says run = false.",
            Some(Self::BesideExplorer) | None => {
                "Explorer starts these programs when you sign in; Tessera starts them only as the shell."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_programs_start_a_moment_after_the_bars_unless_told_otherwise() {
        let config: StartupConfig = toml::from_str("").unwrap();
        assert_eq!(config, StartupConfig::default());
        assert!(config.run && config.active_setup && config.packaged);
        assert_eq!(config.delay, 2);
        let config: StartupConfig = toml::from_str("run = false\ndelay = 10").unwrap();
        assert!(!config.run);
        assert_eq!(config.delay, 10);
        assert!(toml::from_str::<StartupConfig>("wait = 3").is_err());
    }
}
