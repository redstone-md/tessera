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

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, AsyncApp, Global};
use serde::Deserialize;

use crate::banners::{Banners, Notice, NoticeAction, OnGone};
use crate::i18n::Language;
use crate::modules::devices::Drives;
use crate::notifications_store::NotificationStore;
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

/// Marks that the balloons of the notification area are read, and keeps
/// the balloon each icon shows: the icon, by its id in the tray, and the
/// notice of the balloon in the store of notifications.
#[derive(Default)]
struct ReadingBalloons {
    shown: HashMap<u64, u64>,
}

impl ReadingBalloons {
    /// Forgets that the icon `id` shows the balloon of the notice `key`;
    /// `false` where it shows another, or none.
    fn forget(id: u64, key: u64, cx: &mut App) -> bool {
        let shown = &mut cx.default_global::<Self>().shown;
        match shown.get(&id) == Some(&key) {
            true => shown.remove(&id).is_some(),
            false => false,
        }
    }
}

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
        cx.set_global(ReadingBalloons::default());
        let balloons = Tray::balloons();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(balloon) = balloons.recv().await {
                let done = match balloon.is_withdrawal() {
                    true => cx.update(|cx| Self::end_balloon(balloon.id, false, cx)),
                    false => cx.update(|cx| Self::post_balloon(balloon, cx)),
                };
                if done.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Takes away the banner of the balloon the icon `id` shows, and tells
    /// the application that it is hidden. Where the application took it
    /// back or removed its icon, its notice goes from the list as well;
    /// with `keep`, as a new balloon replaces it, the notice stays listed.
    fn end_balloon(id: u64, keep: bool, cx: &mut App) {
        let Some(key) = cx.default_global::<ReadingBalloons>().shown.remove(&id) else {
            return;
        };
        Banners::withdraw(key, cx);
        if !keep {
            NotificationStore::dismiss(&[key], cx);
        }
        Tray::balloon_event(id, BalloonEvent::Hidden);
    }

    /// Posts `balloon` as a notice whose button, and a click on its
    /// banner, tell the application that it was clicked. The application
    /// is told that it is shown, and when its banner goes by itself or is
    /// closed, that it timed out; where it comes as no banner, as while
    /// banners are off or held back, that it timed out at once, and a
    /// balloon that is to be shown at once or not at all is not kept.
    fn post_balloon(balloon: Balloon, cx: &mut App) {
        let id = balloon.id;
        let realtime = balloon.realtime;
        // An icon shows one balloon at a time.
        Self::end_balloon(id, true, cx);
        // The key of the notice is known once it is posted.
        let posted_key = Rc::new(Cell::new(None));
        let language = Language::of(cx);
        let open = NoticeAction::new(language.say("Open"), {
            let posted_key = posted_key.clone();
            move |cx| {
                if let Some(key) = posted_key.get() {
                    ReadingBalloons::forget(id, key, cx);
                }
                Tray::balloon_event(id, BalloonEvent::Clicked);
            }
        });
        let app = match balloon.tip.trim() {
            "" => language.say("Notifications").to_owned(),
            tip => tip.lines().next().unwrap_or(tip).to_owned(),
        };
        let on_gone = OnGone::new({
            let posted_key = posted_key.clone();
            move |cx| {
                if let Some(key) = posted_key.get()
                    && ReadingBalloons::forget(id, key, cx)
                {
                    Tray::balloon_event(id, BalloonEvent::TimedOut);
                }
            }
        });
        let notice = Notice {
            app,
            title: balloon.title,
            body: balloon.text,
            actions: vec![open],
            on_gone,
            ..Notice::default()
        };
        let posted = Banners::post(notice, cx);
        posted_key.set(Some(posted.key));
        if !posted.shown {
            if realtime {
                NotificationStore::dismiss(&[posted.key], cx);
            }
            Tray::balloon_event(id, BalloonEvent::TimedOut);
            return;
        }
        cx.default_global::<ReadingBalloons>()
            .shown
            .insert(id, posted.key);
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
