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
//! are put away. This part says which of them are given, and gives the
//! balloons applications show at their icons of the notification area
//! where Tessera serves the icons itself, which Explorer would show.

use gpui::{App, AsyncApp, Global};
use serde::Deserialize;

use crate::banners::{Banners, Notice, NoticeAction};
use crate::i18n::Language;
use crate::modules::devices::Drives;
use crate::platform::{Balloon, BalloonEvent, Tray, shell_mode};
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

/// Marks that the balloons of the notification area are read.
struct ReadingBalloons;

impl Global for ReadingBalloons {}

impl Notices {
    /// Takes over what the configuration says: drives that arrive and
    /// batteries that run low are looked out for while they are to be told
    /// of.
    pub fn configure(config: &NoticesConfig, cx: &mut App) {
        Drives::tell_arrivals(config.drives(), cx);
        PowerWarnings::configure(config.battery(), cx);
        Self::read_balloons(cx);
    }

    /// Posts the balloons that applications show at their icons as
    /// notices, once for the whole run. Only where Tessera serves the
    /// icons itself do balloons arrive; beside Explorer, Explorer shows
    /// them.
    fn read_balloons(cx: &mut App) {
        if cx.has_global::<ReadingBalloons>() {
            return;
        }
        cx.set_global(ReadingBalloons);
        let balloons = Tray::balloons();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(balloon) = balloons.recv().await {
                if balloon.is_withdrawal() {
                    continue;
                }
                if cx.update(|cx| Self::post_balloon(balloon, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Posts `balloon` as a notice whose button, and a click on its
    /// banner, tell the application that it was clicked, and tells the
    /// application that it is shown.
    fn post_balloon(balloon: Balloon, cx: &mut App) {
        let id = balloon.id;
        let language = Language::of(cx);
        let open = NoticeAction::new(language.say("Open"), move |_| {
            Tray::balloon_event(id, BalloonEvent::Clicked);
        });
        let app = match balloon.tip.trim() {
            "" => language.say("Notifications").to_owned(),
            tip => tip.lines().next().unwrap_or(tip).to_owned(),
        };
        let notice = Notice {
            app,
            title: balloon.title,
            body: balloon.text,
            actions: vec![open],
            ..Notice::default()
        };
        Banners::post(notice, cx);
        Tray::balloon_event(id, BalloonEvent::Shown);
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
