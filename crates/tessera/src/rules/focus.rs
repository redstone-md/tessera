// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The rule `focus`: how strictly a window is kept from taking the
//! foreground by itself, from `none`, which leaves it to Windows, to
//! `extreme`. The guard of the foreground (platform/win32/focus_guard.rs)
//! asks these rules of every window of another program that comes in
//! front, on a thread of its own, so a rule decides at once and without
//! the shell; as everywhere, the last rule that names the window has the
//! last word. A rule that applies `now` counts here as any other.

use crate::platform::{AppWindow, FocusGuard, FocusLevel};

use super::matching::Named;
use super::{Effective, RuleConfig};

/// Has the guard of the foreground hold windows as `rules` say from now
/// on; while no rule holds any window at `medium` or above, it is off.
pub(super) fn guard(rules: &[RuleConfig]) {
    let held: Vec<RuleConfig> = { rules.iter() }
        .filter(|rule| rule.says.focus.is_some())
        .cloned()
        .collect();
    let holds = { held.iter() }.any(|rule| rule.says.focus >= Some(FocusLevel::Medium));
    if !holds {
        FocusGuard::guard(None);
        return;
    }
    FocusGuard::guard(Some(Box::new(move |id| {
        let Some(window) = AppWindow::of_id(id) else {
            return FocusLevel::None;
        };
        level_of(&held, &Named::of(&window))
    })));
}

/// How strictly `rules` hold the window `named`.
fn level_of(rules: &[RuleConfig], named: &Named) -> FocusLevel {
    Effective::of(rules, named).focus.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_rule_that_names_a_window_says_how_it_is_held() {
        let rules: Vec<RuleConfig> = toml::from_str::<toml::Table>(
            r#"
            [[rule]]
            exe = "chat.exe"
            focus = "high"

            [[rule]]
            exe = "chat.exe"
            title = "Call"
            focus = "low"
            "#,
        )
        .unwrap()["rule"]
            .as_array()
            .unwrap()
            .iter()
            .map(|rule| RuleConfig::of_table(rule.as_table().unwrap().clone()).unwrap())
            .collect();
        let named = |title: &str| Named {
            exe: Some("chat.exe".to_owned()),
            title: title.to_owned(),
            ..Named::default()
        };
        assert_eq!(level_of(&rules, &named("Inbox")), FocusLevel::High);
        assert_eq!(level_of(&rules, &named("Call")), FocusLevel::Low);
        let other = Named {
            exe: Some("other.exe".to_owned()),
            ..Named::default()
        };
        assert_eq!(level_of(&rules, &other), FocusLevel::None);
        assert!(toml::from_str::<RuleConfig>("exe = \"a.exe\"\nfocus = \"total\"").is_err());
    }
}
