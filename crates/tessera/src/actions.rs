// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Things Tessera does that have a name: a `[[key]]` names one with
//! `action` (and `arg`), and `tessera do <action> [arg]` asks the running
//! Tessera for one.
//!
//! The actions are collected once, when the shell starts. A module adds
//! its own in [`Module::actions`](crate::module::Module::actions); a part
//! of Tessera that is no module calls [`Actions::add`] from its `start`,
//! which [`Shell::start`](crate::shell::Shell::start) calls before the
//! keys are configured. Nothing else needs to change for a new action:
//! keys.rs does not know any of them by name.
//!
//! An action takes no argument unless it says which it takes, with
//! [`Action::checking`], or that it takes any, with
//! [`Action::taking_any_arg`].
//!
//! ```ignore
//! fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
//!     actions.register("snap", Action::new(|side, cx| Snapper::snap(side, cx))
//!         .taking_reserved_keys()
//!         .checking(|side| match side {
//!             Some("left" | "right") => Ok(()),
//!             _ => Err("`snap` takes `left` or `right`".to_owned()),
//!         }))
//! }
//! ```

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::bail;
use gpui::{App, Global};

use crate::config::KeyConfig;
use crate::module::Module;
use crate::platform::Modifiers;
use crate::shell::Shell;

/// What an action does, given the `arg` of its `[[key]]` or the words
/// after its name in `tessera do`, if there are any.
pub type ActionRun = Rc<dyn Fn(Option<&str>, &mut App)>;

/// Whether an action can be done with an argument, and why not. It is
/// asked when the configuration is read, so that a mistake is reported
/// then instead of when the key is pressed.
pub type ActionCheck = Rc<dyn Fn(Option<&str>) -> Result<(), String>>;

/// A thing Tessera does, under the name it is registered with.
#[derive(Clone)]
pub struct Action {
    /// Whether the keys of this action are taken from Windows where it
    /// keeps them to itself, as it does with Win+D, Win+E or Alt+Tab. Such
    /// keys are taken with a keyboard hook instead of being reported as
    /// held by someone else.
    pub takes_reserved_keys: bool,
    pub run: ActionRun,
    /// Which arguments the action takes; `None` takes none.
    pub check: Option<ActionCheck>,
    /// The combinations bound to the action unless the user binds them
    /// to something else, each with its `arg`; see
    /// [`Action::by_default`].
    pub default_keys: Vec<(String, Option<String>)>,
}

impl Action {
    /// An action that does `run`, takes no argument (an `arg` given to it
    /// is reported when the configuration is read) and leaves the keys
    /// Windows keeps to Windows.
    pub fn new(run: impl Fn(Option<&str>, &mut App) + 'static) -> Self {
        Self {
            takes_reserved_keys: false,
            run: Rc::new(run),
            check: None,
            default_keys: Vec::new(),
        }
    }

    /// The action, with `press` (as `win+m`) bound to it with `arg` where
    /// the keys of Windows are in use (`windows_keys`, on by default when
    /// Tessera is the shell) and no `[[key]]` of the user presses the same
    /// keys. This is how a feature brings its keys of Windows without the
    /// keys of the configuration knowing it.
    pub fn by_default(mut self, press: &str, arg: Option<&str>) -> Self {
        self.default_keys
            .push((press.to_owned(), arg.map(str::to_owned)));
        self
    }

    /// The action, taking any argument, or none.
    pub fn taking_any_arg(self) -> Self {
        self.checking(|_| Ok(()))
    }

    /// The action, taking its keys from Windows where it keeps them.
    pub fn taking_reserved_keys(mut self) -> Self {
        self.takes_reserved_keys = true;
        self
    }

    /// The action, with `check` telling which arguments it can be done
    /// with.
    pub fn checking(
        mut self,
        check: impl Fn(Option<&str>) -> Result<(), String> + 'static,
    ) -> Self {
        self.check = Some(Rc::new(check));
        self
    }

    /// Whether the action, registered as `name`, can be done with `arg`,
    /// and why not.
    pub fn accepts(&self, name: &str, arg: Option<&str>) -> Result<(), String> {
        match (&self.check, arg) {
            (Some(check), arg) => check(arg),
            (None, None) => Ok(()),
            (None, Some(_)) => Err(format!("`{name}` takes no `arg`")),
        }
    }
}

/// Every action there is, by name.
#[derive(Default)]
pub struct Actions {
    actions: HashMap<String, Action>,
}

impl Global for Actions {}

/// The keys to hold of the combination that is being answered, while it
/// is; see [`Actions::pressing`].
#[derive(Default)]
struct Pressing(Option<Modifiers>);

impl Global for Pressing {}

impl Actions {
    /// Collects the actions of Tessera itself and of `modules`, and adds
    /// them to those added before. A module that fails to register an
    /// action is reported; the actions it added before stay.
    pub fn start(modules: &[Box<dyn Module>], cx: &mut App) {
        for (name, action) in Self::builtin() {
            Self::add(name, action, cx);
        }
        let actions = cx.default_global::<Self>();
        for module in modules {
            if let Err(error) = module.actions(actions) {
                report!(
                    "module `{}` has not all its actions: {error:#}",
                    module.name()
                );
            }
        }
    }

    /// The actions that belong to no module.
    ///
    /// `popup` opens the popup its argument names, or closes it, as the
    /// `popup` of a `[[key]]` does; as an action it also takes its keys
    /// from Windows where it keeps them, such as Win+V for a clipboard
    /// popup.
    fn builtin() -> [(&'static str, Action); 1] {
        let popup = Action::new(|name, cx| Shell::toggle_popup(name.unwrap_or_default(), cx))
            .taking_reserved_keys()
            .checking(|name| match name {
                Some(name) if !name.is_empty() => Ok(()),
                _ => Err("`popup` takes the name of a popup as `arg`".to_owned()),
            });
        [("popup", popup)]
    }

    /// Makes `action` available as `name`. A name that is taken already
    /// keeps its action, and the answer says so.
    pub fn register(&mut self, name: &str, action: Action) -> anyhow::Result<()> {
        if self.actions.contains_key(name) {
            bail!("an action named `{name}` is already registered");
        }
        self.actions.insert(name.to_owned(), action);
        Ok(())
    }

    /// Makes `action` available as `name`, for a part of Tessera that is
    /// no module. It is to be called before the keys are configured; a
    /// name that is taken is reported.
    pub fn add(name: &str, action: Action, cx: &mut App) {
        if let Err(error) = cx.default_global::<Self>().register(name, action) {
            report!("{error:#}");
        }
    }

    /// The action registered as `name`.
    pub fn get(&self, name: &str) -> Option<&Action> {
        self.actions.get(name)
    }

    /// The names of the actions, in order.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.actions.keys().cloned().collect();
        names.sort();
        names
    }

    /// Whether `name` is an action that can be done with `arg`, and why
    /// not.
    pub fn check(&self, name: &str, arg: Option<&str>) -> Result<(), String> {
        match self.get(name) {
            Some(action) => action.accepts(name, arg),
            None => Err(format!(
                "there is no action named `{name}`; there are `{}`",
                self.names().join("`, `")
            )),
        }
    }

    /// The `[[key]]` entries the actions bind by default, by the name of
    /// the action and then in the order they were given.
    pub fn default_keys(&self) -> Vec<KeyConfig> {
        let mut keys = Vec::new();
        for name in self.names() {
            for (press, arg) in &self.actions[&name].default_keys {
                keys.push(KeyConfig {
                    press: press.clone(),
                    action: Some(name.clone()),
                    arg: arg.clone(),
                    ..KeyConfig::default()
                });
            }
        }
        keys
    }

    /// Whether the keys of the action `name` are taken from Windows where
    /// it keeps them.
    pub fn takes_reserved_keys(&self, name: &str) -> bool {
        self.get(name)
            .is_some_and(|action| action.takes_reserved_keys)
    }

    /// Does `done` as the answer to a combination whose keys to hold are
    /// `held`, which an action done meanwhile reads with
    /// [`Actions::held`]: the keys may be let go before the answer comes,
    /// so asking Windows which are down then does not tell them.
    pub fn pressing(held: Modifiers, cx: &mut App, done: impl FnOnce(&mut App)) {
        let before = cx.default_global::<Pressing>().0.replace(held);
        done(cx);
        cx.global_mut::<Pressing>().0 = before;
    }

    /// The keys to hold of the combination the action being done answers;
    /// `None` when it was asked for otherwise, as by `tessera do`.
    pub fn held(cx: &App) -> Option<Modifiers> {
        cx.try_global::<Pressing>().and_then(|pressing| pressing.0)
    }

    /// Does the action `name` with `arg`. A name that is no action is
    /// reported.
    pub fn run(name: &str, arg: Option<&str>, cx: &mut App) {
        let run = cx
            .try_global::<Self>()
            .and_then(|actions| actions.get(name))
            .map(|action| action.run.clone());
        match run {
            Some(run) => run(arg, cx),
            None => report!("there is no action named `{name}`"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_registered_once() {
        let mut actions = Actions::default();
        actions.register("nothing", Action::new(|_, _| {})).unwrap();
        assert!(actions.register("nothing", Action::new(|_, _| {})).is_err());
        assert_eq!(actions.names(), ["nothing"]);
        assert!(!actions.takes_reserved_keys("nothing"));
        assert!(actions.check("elsewhere", None).is_err());
    }

    #[test]
    fn an_action_brings_the_keys_it_has_by_default() {
        let mut actions = Actions::default();
        let both = |arg: Option<&str>| match arg {
            None | Some("all") => Ok(()),
            Some(_) => Err("none or all".to_owned()),
        };
        let fold = Action::new(|_, _| {})
            .checking(both)
            .by_default("win+m", Some("all"))
            .by_default("win+home", None);
        actions.register("fold", fold).unwrap();
        actions.register("bare", Action::new(|_, _| {})).unwrap();
        let keys = actions.default_keys();
        let pressed: Vec<(&str, Option<&str>, Option<&str>)> = { keys.iter() }
            .map(|key| {
                (
                    key.press.as_str(),
                    key.action.as_deref(),
                    key.arg.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            pressed,
            [
                ("win+m", Some("fold"), Some("all")),
                ("win+home", Some("fold"), None)
            ]
        );
    }

    #[test]
    fn an_action_takes_no_argument_unless_it_says_so() {
        let mut actions = Actions::default();
        actions.register("still", Action::new(|_, _| {})).unwrap();
        let open = Action::new(|_, _| {}).taking_any_arg();
        actions.register("open", open).unwrap();
        assert_eq!(actions.check("still", None), Ok(()));
        let refused = actions.check("still", Some("minimize"));
        assert_eq!(refused, Err("`still` takes no `arg`".to_owned()));
        assert_eq!(actions.check("open", None), Ok(()));
        assert_eq!(actions.check("open", Some(r"D:\Projects")), Ok(()));
    }

    #[test]
    fn an_action_says_which_arguments_it_takes() {
        let mut actions = Actions::default();
        let sides = |side: Option<&str>| match side {
            Some("left" | "right") => Ok(()),
            _ => Err("a side".to_owned()),
        };
        let action = Action::new(|_, _| {})
            .taking_reserved_keys()
            .checking(sides);
        actions.register("lean", action).unwrap();
        assert!(actions.takes_reserved_keys("lean"));
        assert_eq!(actions.check("lean", Some("left")), Ok(()));
        assert!(actions.check("lean", Some("up")).is_err());
        assert!(actions.check("lean", None).is_err());
    }

    #[test]
    fn the_shell_has_actions_of_its_own() {
        let mut actions = Actions::default();
        for (name, action) in Actions::builtin() {
            actions.register(name, action).unwrap();
        }
        assert!(actions.takes_reserved_keys("popup"));
        assert_eq!(actions.check("popup", Some("launcher")), Ok(()));
        assert!(actions.check("popup", None).is_err());
    }
}
