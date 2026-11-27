// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The keys of the dock, as actions a `[[key]]` can name as well, each
//! bound by default where the keys of Windows are in use (`windows_keys`)
//! as Explorer binds them:
//!
//! - `dock-new` (Win+Shift+number) starts another window of the
//!   application of that button;
//! - `dock-cycle` (Win+Ctrl+number) goes through the windows of that
//!   button;
//! - `dock-menu` (Win+Alt+number) opens the menu of that button;
//! - `dock-focus` (Win+T) puts the keyboard on the dock: the arrows go
//!   from button to button, Enter does what a click does, the Menu key
//!   or Shift+F10 opens the menu of the button, and Escape gives the
//!   keyboard back.
//!
//! Win+number itself is the action `dock` of the keys of Windows.

use crate::actions::{Action, Actions};

use super::{Deed, Docks};

/// The actions that take the number of a button, what each does and the
/// keys held with the number by default.
const NUMBERED: [(&str, Deed, &str); 3] = [
    ("dock-new", Deed::New, "win+shift+"),
    ("dock-cycle", Deed::Cycle, "win+ctrl+"),
    ("dock-menu", Deed::Menu, "win+alt+"),
];

/// Registers the actions of the dock.
pub fn register(actions: &mut Actions) -> anyhow::Result<()> {
    for (name, deed, held) in NUMBERED {
        let mut action = Action::new(move |number, cx| {
            let number = number.and_then(|number| number.trim().parse().ok());
            Docks::act(number.unwrap_or(0), deed, cx);
        })
        .taking_reserved_keys()
        .checking(check_number);
        for number in 1..=9 {
            let number = number.to_string();
            action = action.by_default(&format!("{held}{number}"), Some(&number));
        }
        actions.register(name, action)?;
    }
    let focus = Action::new(|_, cx| Docks::focus(cx))
        .taking_reserved_keys()
        .by_default("win+t", None);
    actions.register("dock-focus", focus)
}

/// Whether `number` names a button the keys can reach.
fn check_number(number: Option<&str>) -> Result<(), String> {
    match number.map(|number| number.trim().parse::<usize>()) {
        Some(Ok(1..=9)) => Ok(()),
        _ => Err("the actions of the dock take a number from 1 to 9 as `arg`".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions() -> Actions {
        let mut actions = Actions::default();
        register(&mut actions).unwrap();
        actions
    }

    #[test]
    fn the_keys_of_the_taskbar_are_bound_as_explorer_binds_them() {
        let keys = actions().default_keys();
        let bound = |press: &str| {
            let key = keys.iter().find(|key| key.press == press)?;
            Some((key.action.clone()?, key.arg.clone()))
        };
        assert_eq!(
            bound("win+shift+3"),
            Some(("dock-new".to_owned(), Some("3".to_owned())))
        );
        assert_eq!(
            bound("win+ctrl+9"),
            Some(("dock-cycle".to_owned(), Some("9".to_owned())))
        );
        assert_eq!(
            bound("win+alt+1"),
            Some(("dock-menu".to_owned(), Some("1".to_owned())))
        );
        assert_eq!(bound("win+t"), Some(("dock-focus".to_owned(), None)));
        for key in &keys {
            assert!(
                crate::keys::Press::parse(&key.press).is_some(),
                "`{}` is no combination",
                key.press
            );
        }
    }

    #[test]
    fn the_numbered_actions_take_a_number_of_a_button() {
        let actions = actions();
        assert_eq!(actions.check("dock-new", Some("4")), Ok(()));
        assert!(actions.check("dock-cycle", Some("0")).is_err());
        assert!(actions.check("dock-menu", None).is_err());
        assert!(actions.check("dock-focus", Some("1")).is_err());
        for name in ["dock-new", "dock-cycle", "dock-menu", "dock-focus"] {
            assert!(actions.takes_reserved_keys(name));
        }
    }
}
