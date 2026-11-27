// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Warnings of a battery running low, which the battery icon of Explorer
//! gives and which are gone once Explorer is not the shell. They follow
//! the levels of the power plan in use, as Windows does: a notice at the
//! low level where the plan says to warn there, an urgent one at the
//! reserve level, and one more at the critical level where the plan has
//! Windows do nothing there. Each is given once on the way down; plugging
//! the machine in starts over.
//!
//! The charge is read when Windows tells of a change of it, of the power
//! source or of the plan, and every minute where it does not tell.

use std::time::Duration;

use async_channel::Receiver;
use gpui::{App, AsyncApp, Global, Task};

use crate::i18n::Language;
use crate::notices::{Notice, NoticeAction, Notices};
use crate::platform::{Battery, BatteryChanges, BatteryLevels, CriticalAction, Launch};

/// How often the charge is read where Windows does not tell of changes.
const READ_INTERVAL: Duration = Duration::from_secs(60);

/// The page of the Windows settings about power and the battery.
const POWER_SETTINGS: &str = "ms-settings:powersleep";

const ICON_LOW: &str = "\u{e851}";
const ICON_EMPTY: &str = "\u{e850}";

/// How low the battery is, as the power plan counts it; later is lower.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Warning {
    Low,
    Reserve,
    Critical,
}

impl Warning {
    /// The warning a battery at `percent` calls for under `levels`.
    fn at(percent: u8, levels: &BatteryLevels) -> Option<Self> {
        let critical =
            levels.critical_action == CriticalAction::Nothing && percent <= levels.critical;
        if critical {
            Some(Self::Critical)
        } else if percent <= levels.reserve {
            Some(Self::Reserve)
        } else if levels.low.is_some_and(|low| percent <= low) {
            Some(Self::Low)
        } else {
            None
        }
    }
}

/// Which warning was given since the machine was last plugged in.
#[derive(Debug, Default)]
struct Given {
    warning: Option<Warning>,
}

impl Given {
    /// The warning to give for `battery` now, if any: one lower than any
    /// given since the machine was last plugged in.
    fn next(&mut self, battery: Option<Battery>, levels: &BatteryLevels) -> Option<Warning> {
        let Some(battery) = battery.filter(|battery| !battery.plugged_in) else {
            self.warning = None;
            return None;
        };
        let now = Warning::at(battery.percent, levels);
        if now > self.warning {
            self.warning = now;
            return now;
        }
        // A charge that rose without the machine being plugged in, as
        // after the battery was measured anew, is warned of again once it
        // falls.
        self.warning = now;
        None
    }
}

/// The watch of the battery, while warnings are to be given.
#[derive(Default)]
pub struct PowerWarnings {
    given: Given,
    /// What Windows tells of changes through, once asked; it cannot be
    /// asked twice.
    changes: Option<Option<Receiver<()>>>,
    watch: Option<Task<()>>,
}

impl Global for PowerWarnings {}

impl PowerWarnings {
    /// Gives warnings from now on if `on`, and stops giving them otherwise.
    pub fn configure(on: bool, cx: &mut App) {
        let this = cx.default_global::<Self>();
        if !on {
            this.watch = None;
            return;
        }
        if this.watch.is_some() {
            return;
        }
        let changes = this
            .changes
            .get_or_insert_with(BatteryChanges::watch)
            .clone();
        let watch = cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                let read = cx
                    .background_executor()
                    .spawn(async { (Battery::read(), BatteryLevels::read()) })
                    .await;
                if cx.update(|cx| Self::take(read, cx)).is_err() {
                    break;
                }
                match &changes {
                    Some(changes) => {
                        if changes.recv().await.is_err() {
                            break;
                        }
                    }
                    None => cx.background_executor().timer(READ_INTERVAL).await,
                }
            }
        });
        cx.global_mut::<Self>().watch = Some(watch);
    }

    fn take((battery, levels): (Option<Battery>, BatteryLevels), cx: &mut App) {
        let Some(warning) = cx.global_mut::<Self>().given.next(battery, &levels) else {
            return;
        };
        let percent = battery.map_or(0, |battery| battery.percent);
        Notices::post(notice(warning, percent, &levels, Language::of(cx)), cx);
    }
}

/// The notice of `warning` for a battery at `percent`.
fn notice(warning: Warning, percent: u8, levels: &BatteryLevels, language: Language) -> Notice {
    let left = format!("{percent}% {}", language.say("of the charge is left."));
    let (title, advice) = match warning {
        Warning::Low => ("The battery is low", "Plug the computer in soon."),
        Warning::Reserve => (
            "The battery is very low",
            match levels.critical_action {
                CriticalAction::Nothing => "Plug the computer in now.",
                CriticalAction::Sleep => "Plug the computer in now; it will soon go to sleep.",
                CriticalAction::Hibernate => "Plug the computer in now; it will soon hibernate.",
                CriticalAction::ShutDown => "Plug the computer in now; it will soon shut down.",
            },
        ),
        Warning::Critical => (
            "The battery is nearly empty",
            "Save your work and plug the computer in.",
        ),
    };
    let settings = NoticeAction::new(language.say("Power and battery"), |_| {
        Launch::new(POWER_SETTINGS).start();
    });
    Notice {
        app: language.say("Battery").to_owned(),
        title: language.say(title).to_owned(),
        body: format!("{left} {}", language.say(advice)),
        glyph: Some(match warning {
            Warning::Low => ICON_LOW,
            Warning::Reserve | Warning::Critical => ICON_EMPTY,
        }),
        actions: vec![settings],
        urgent: warning != Warning::Low,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(action: CriticalAction) -> BatteryLevels {
        BatteryLevels {
            low: Some(20),
            reserve: 10,
            critical: 5,
            critical_action: action,
        }
    }

    fn on_battery(percent: u8) -> Option<Battery> {
        Some(Battery {
            percent,
            plugged_in: false,
        })
    }

    #[test]
    fn the_warning_follows_the_levels_of_the_plan() {
        let hibernates = levels(CriticalAction::Hibernate);
        assert_eq!(Warning::at(50, &hibernates), None);
        assert_eq!(Warning::at(20, &hibernates), Some(Warning::Low));
        assert_eq!(Warning::at(10, &hibernates), Some(Warning::Reserve));
        // Windows hibernates at the critical level; the reserve warning
        // said so.
        assert_eq!(Warning::at(4, &hibernates), Some(Warning::Reserve));
        let stays = levels(CriticalAction::Nothing);
        assert_eq!(Warning::at(5, &stays), Some(Warning::Critical));
        let quiet_low = BatteryLevels {
            low: None,
            ..hibernates
        };
        assert_eq!(Warning::at(15, &quiet_low), None);
    }

    #[test]
    fn each_warning_is_given_once_on_the_way_down() {
        let levels = levels(CriticalAction::Nothing);
        let mut given = Given::default();
        assert_eq!(given.next(on_battery(30), &levels), None);
        assert_eq!(given.next(on_battery(20), &levels), Some(Warning::Low));
        assert_eq!(given.next(on_battery(19), &levels), None);
        assert_eq!(given.next(on_battery(9), &levels), Some(Warning::Reserve));
        assert_eq!(given.next(on_battery(8), &levels), None);
        assert_eq!(given.next(on_battery(3), &levels), Some(Warning::Critical));
        assert_eq!(given.next(on_battery(2), &levels), None);
    }

    #[test]
    fn plugging_in_starts_over() {
        let levels = levels(CriticalAction::Hibernate);
        let mut given = Given::default();
        assert_eq!(given.next(on_battery(18), &levels), Some(Warning::Low));
        let plugged = Some(Battery {
            percent: 18,
            plugged_in: true,
        });
        assert_eq!(given.next(plugged, &levels), None);
        assert_eq!(given.next(on_battery(18), &levels), Some(Warning::Low));
        // No battery, no warning.
        assert_eq!(given.next(None, &levels), None);
        assert_eq!(given.next(on_battery(18), &levels), Some(Warning::Low));
    }

    #[test]
    fn a_charge_that_rose_is_warned_of_again_when_it_falls() {
        let levels = levels(CriticalAction::Hibernate);
        let mut given = Given::default();
        assert_eq!(given.next(on_battery(9), &levels), Some(Warning::Reserve));
        assert_eq!(given.next(on_battery(25), &levels), None);
        assert_eq!(given.next(on_battery(20), &levels), Some(Warning::Low));
    }

    #[test]
    fn the_notice_says_what_is_left_and_what_comes() {
        let levels = levels(CriticalAction::Hibernate);
        let low = notice(Warning::Low, 18, &levels, Language::English);
        assert_eq!(low.title, "The battery is low");
        assert_eq!(
            low.body,
            "18% of the charge is left. Plug the computer in soon."
        );
        assert!(!low.urgent);
        let reserve = notice(Warning::Reserve, 7, &levels, Language::English);
        assert!(reserve.urgent);
        assert!(reserve.body.ends_with("it will soon hibernate."));
        let russian = notice(Warning::Low, 18, &levels, Language::Russian);
        assert_eq!(russian.title, "Батарея разряжается");
    }
}
