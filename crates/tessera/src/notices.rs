// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Notices of Tessera itself, given where Explorer gives them and nothing
//! else does once Explorer is not the shell: a drive that was plugged in,
//! a device that may now be pulled out or could not be ejected, a battery
//! running low.
//!
//! A notice is a [`Notice`] of the banners, posted with
//! [`Banners::post`](crate::banners::Banners::post): it is listed with the
//! notifications and comes as a banner where banners are shown, with
//! buttons ([`NoticeAction`](crate::banners::NoticeAction)) for what can
//! be done about it. Do not disturb holds the banners back but for urgent
//! notices, such as a battery about to run out, which also stay until they
//! are put away. This part only says which of them are given.

use gpui::App;
use serde::Deserialize;

use crate::modules::devices::Drives;
use crate::platform::shell_mode;
use crate::power_warnings::PowerWarnings;

/// What the configuration says about the notices Tessera gives in place of
/// Explorer, under `[notices]`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoticesConfig {
    /// Whether a drive that is plugged in is told of, with a way to open
    /// it; without the key, only while Tessera is the shell.
    drives: Option<bool>,
    /// Whether a battery running low is warned of at the levels of the
    /// power plan; without the key, only while Tessera is the shell.
    battery: Option<bool>,
}

impl NoticesConfig {
    /// Whether drives that arrive are told of.
    pub fn drives(&self) -> bool {
        self.drives.unwrap_or_else(shell_mode)
    }

    /// Whether a battery running low is warned of.
    pub fn battery(&self) -> bool {
        self.battery.unwrap_or_else(shell_mode)
    }
}

/// Which notices are given.
pub struct Notices;

impl Notices {
    /// Takes over what the configuration says: drives that arrive and
    /// batteries that run low are looked out for while they are to be told
    /// of.
    pub fn configure(config: &NoticesConfig, cx: &mut App) {
        Drives::tell_arrivals(config.drives(), cx);
        PowerWarnings::configure(config.battery(), cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_told_of_can_be_chosen() {
        let config: NoticesConfig =
            toml::from_str("drives = false\nbattery = true\n").expect("a valid table");
        assert!(!config.drives());
        assert!(config.battery());
        assert!(toml::from_str::<NoticesConfig>("sound = true\n").is_err());
        assert!(toml::from_str::<NoticesConfig>("seconds = 8\n").is_err());
    }
}
