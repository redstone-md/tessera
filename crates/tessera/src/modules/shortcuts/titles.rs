// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the Shortcuts page calls each action: its `[[action]]` title from
//! the rows files of the settings, in the language shown. An action with
//! none, as the shortcuts of an extension, is called by its name.

use crate::i18n::Language;
use crate::modules::{ActionTitle, action_titles};

/// The `[[action]]` entry of the action `name`.
pub fn entry(name: &str) -> Option<&'static ActionTitle> {
    action_titles().iter().find(|title| title.name == name)
}

/// The title of the action `name` in `language`, or its name where it has
/// none.
pub fn title(name: &str, language: Language) -> String {
    match entry(name) {
        Some(entry) => language.say(&entry.title).to_owned(),
        None => name.to_owned(),
    }
}

/// What the argument of the action `name` is, in `language`, where its
/// entry says.
pub fn argument(name: &str, language: Language) -> Option<String> {
    let arg = entry(name)?.arg.as_deref()?;
    Some(language.say(arg).to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;
    use crate::actions::Actions;
    use crate::config::Config;
    use crate::keys::written_press;
    use crate::switcher::Switcher;
    use crate::taskview::TaskView;
    use crate::tiling::Tiler;

    /// The presses the shell host holds itself, for recovery: Ctrl+Win+Alt+E
    /// hands the session to Explorer and Ctrl+Win+Alt+R starts Tessera
    /// again (shell_host.rs).
    const SHELL_HOST_KEYS: [&str; 2] = ["ctrl+win+alt+e", "ctrl+win+alt+r"];

    /// Every action Tessera registers, without an App: those that belong
    /// to no module, those of every built-in module and those parts of
    /// Tessera add as they start. The shortcuts of extensions, named
    /// `<extension>.<shortcut>`, come from what is installed and are left
    /// out.
    fn every_action() -> Actions {
        let mut actions = Actions::default();
        for (name, action) in Actions::builtin() {
            actions.register(name, action).unwrap();
        }
        let config: Config = toml::from_str("").expect("an empty configuration is one");
        for module in crate::modules::builtin(&config) {
            if let Err(error) = module.actions(&mut actions) {
                panic!(
                    "module `{}` has not all its actions: {error:#}",
                    module.name()
                );
            }
        }
        actions.register("tiling", Tiler::action()).unwrap();
        actions.register("switcher", Switcher::action()).unwrap();
        for (name, action) in TaskView::actions() {
            actions.register(name, action).unwrap();
        }
        actions
    }

    fn of_tessera(name: &str) -> bool {
        !name.contains('.')
    }

    #[test]
    fn every_action_tessera_registers_has_a_title() {
        let actions = every_action();
        let untitled: Vec<String> = { actions.names().into_iter() }
            .filter(|name| of_tessera(name) && entry(name).is_none())
            .collect();
        assert!(
            untitled.is_empty(),
            "these actions have no [[action]] title in a rows file: {untitled:?}"
        );
        let known: HashSet<String> = actions.names().into_iter().collect();
        for title in action_titles() {
            assert!(
                known.contains(&title.name) || !of_tessera(&title.name),
                "the [[action]] title of `{}` names no action",
                title.name
            );
        }
    }

    #[test]
    fn no_two_actions_bring_the_same_key() {
        let actions = every_action();
        let mut brought: HashMap<String, String> = HashMap::new();
        for press in SHELL_HOST_KEYS {
            brought.insert(written_press(press).unwrap(), "the shell host".to_owned());
        }
        for key in actions.default_keys() {
            let name = key.action.clone().unwrap_or_default();
            if !of_tessera(&name) {
                continue;
            }
            let Some(press) = written_press(&key.press) else {
                panic!("`{name}` brings `{}`, which is no press", key.press);
            };
            let by = format!("`{name}` {}", key.arg.as_deref().unwrap_or_default());
            if let Some(other) = brought.insert(press.clone(), by.clone()) {
                panic!("{press} is brought by {other} and by {by}");
            }
        }
    }
}
