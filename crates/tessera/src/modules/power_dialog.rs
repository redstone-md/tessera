// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The dialog that asks what the computer is to do, as Alt+F4 on the
//! desktop of Windows opens it: switch the user, sign out, sleep,
//! hibernate, shut down or restart, with or without the updates that wait.
//! Widget `power-dialog`, in the popup of the same name in the middle of
//! the display, which the action `power-dialog` opens; nothing is done
//! before OK or Enter. Where the keys of Windows are in use, Alt+F4 on
//! the desktop opens it too.

use gpui::{
    App, AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};

use crate::actions::{Action, Actions};
use crate::config::{PopupConfig, PopupPlace, WidgetSpec};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::Interaction;
use crate::platform::{DesktopClose, PowerAction, PowerChoices, shell_mode};
use crate::popup::Popups;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{Chooser, Pick, choice_button, choice_row, one_line};

/// The name of the popup, of its widget and of the action that opens it.
const NAME: &str = "power-dialog";

/// How large the popup is, in logical pixels.
const WIDTH: f32 = 400.;
const HEIGHT: f32 = 600.;

/// Each choice: what it does, its name, what it is said to do, its icon
/// and the letter that chooses it, in the order the dialog lists them.
const CHOICES: [(PowerAction, &str, &str, &str, char); 8] = [
    (
        PowerAction::SwitchUser,
        "Switch user",
        "Shows the sign-in screen and keeps this session with its apps open.",
        "\u{e748}",
        'w',
    ),
    (
        PowerAction::SignOut,
        "Sign out",
        "Closes all apps and ends the session.",
        "\u{f3b1}",
        'l',
    ),
    (
        PowerAction::Sleep,
        "Sleep",
        "Keeps the session in memory and puts the computer into a low-power state.",
        "\u{e708}",
        's',
    ),
    (
        PowerAction::Hibernate,
        "Hibernate",
        "Saves the session to the disk and turns the computer off.",
        "\u{e74e}",
        'h',
    ),
    (
        PowerAction::UpdateShutDown,
        "Update and shut down",
        "Installs the updates that wait, then turns the computer off.",
        "\u{e895}",
        'p',
    ),
    (
        PowerAction::ShutDown,
        "Shut down",
        "Closes all apps and turns the computer off.",
        "\u{e7e8}",
        'u',
    ),
    (
        PowerAction::UpdateRestart,
        "Update and restart",
        "Installs the updates that wait, then starts the computer again.",
        "\u{e895}",
        'e',
    ),
    (
        PowerAction::Restart,
        "Restart",
        "Closes all apps, turns the computer off and starts it again.",
        "\u{e777}",
        'r',
    ),
];

pub struct PowerDialogModule;

impl Module for PowerDialogModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(NAME, |_, cx| cx.new(PowerDialog::new).into())?;
        widgets.register_popup(PopupConfig {
            name: NAME.to_owned(),
            place: PopupPlace::Center,
            width: WIDTH,
            height: HEIGHT,
            content: vec![WidgetSpec {
                widget: NAME.to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        // Alt+F4 opens it only on the desktop, which `DesktopClosing`
        // looks out for, so it brings no key of its own: Alt+F4 anywhere
        // else closes a window.
        let open = Action::new(|_, cx| Shell::toggle_popup(NAME, cx));
        actions.register(NAME, open)
    }
}

/// Alt+F4 on the desktop, answered with the dialog where the keys of
/// Windows are in use (`windows_keys`, by default when Tessera is the
/// shell), as Explorer answers it.
pub struct DesktopClosing;

impl DesktopClosing {
    /// Takes Alt+F4 on the desktop as `windows_keys` of the configuration
    /// says, and opens the dialog whenever it is pressed.
    pub fn start(windows_keys: Option<bool>, cx: &mut App) {
        Self::configure(windows_keys);
        let presses = DesktopClose::presses();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while presses.recv().await.is_ok() {
                if cx.update(|cx| Actions::run(NAME, None, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Takes Alt+F4 on the desktop, or leaves it to Windows, as
    /// `windows_keys` of the configuration says.
    pub fn configure(windows_keys: Option<bool>) {
        if let Err(error) = DesktopClose::take(windows_keys.unwrap_or_else(shell_mode)) {
            report!("Alt+F4 on the desktop stays with Windows: {error:#}");
        }
    }
}

/// The choices `choices` lets the computer make, in the order of the
/// dialog.
fn offered(choices: PowerChoices) -> Vec<PowerAction> {
    { CHOICES.iter() }
        .map(|&(action, ..)| action)
        .filter(|&action| choices.offers(action))
        .collect()
}

/// What is chosen when the dialog opens: shutting down, with the updates
/// where some wait, as in Windows.
fn first_choice(offered: &[PowerAction]) -> usize {
    let wanted = [PowerAction::UpdateShutDown, PowerAction::ShutDown];
    { wanted.iter() }
        .find_map(|wanted| offered.iter().position(|action| action == wanted))
        .unwrap_or(0)
}

/// The entry of [`CHOICES`] for `action`.
fn described(action: PowerAction) -> (&'static str, &'static str, &'static str, char) {
    let (_, name, note, glyph, letter) = { CHOICES.into_iter() }
        .find(|&(known, ..)| known == action)
        .expect("every offered action is a choice");
    (name, note, glyph, letter)
}

/// The list of choices, the description of the chosen one, and OK and
/// Cancel.
struct PowerDialog {
    /// The choices, once it is known what the computer can do.
    offered: Option<Vec<PowerAction>>,
    chooser: Chooser,
    /// The pointer feedback of OK and Cancel.
    buttons: [Interaction; 2],
}

impl PowerDialog {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let choices = cx
                .background_executor()
                .spawn(async { PowerChoices::read() });
            let offered = offered(choices.await);
            this.update(cx, |this, cx| {
                this.chooser.chosen = first_choice(&offered);
                this.offered = Some(offered);
                cx.notify();
            })
            .ok();
        })
        .detach();
        let motion = cx.global::<Theme>().motion;
        Self {
            offered: None,
            chooser: Chooser::new(cx),
            buttons: [Interaction::new(&motion), Interaction::new(&motion)],
        }
    }

    fn count(&self) -> usize {
        self.offered.as_ref().map_or(0, Vec::len)
    }

    /// Does the chosen action and closes the dialog.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        let chosen = { self.offered.as_ref() }.and_then(|offered| offered.get(self.chooser.chosen));
        if let Some(action) = chosen {
            action.perform();
            cx.defer(Popups::close);
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(pick) = Pick::of(event) else {
            return;
        };
        match pick {
            Pick::Take => self.confirm(cx),
            Pick::Leave => cx.defer(Popups::close),
            // A letter only chooses: nothing ends the session before OK.
            Pick::Letter(letter) => {
                let offered = self.offered.iter().flatten();
                let found = { offered }.position(|&action| described(action).3 == letter);
                if let Some(index) = found {
                    self.chooser.chosen = index;
                }
            }
            pick => self.chooser.pick(pick, self.count()),
        }
        cx.notify();
    }
}

impl Render for PowerDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        self.chooser.take_focus(window);
        let offered = self.offered.clone().unwrap_or_default();
        let mut rows = Vec::with_capacity(offered.len());
        for (index, &action) in offered.iter().enumerate() {
            let (name, _, glyph, letter) = described(action);
            let state = self.chooser.state(index, &theme, window);
            let chosen = index == self.chooser.chosen;
            let row = choice_row(
                &theme,
                glyph,
                language.say(name),
                Some(letter),
                chosen,
                state,
            )
            .id(("power-choice", index))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.chooser.chosen = index;
                cx.notify();
            }));
            rows.push(Chooser::track(row, index, cx, |this: &mut Self| {
                &mut this.chooser
            }));
        }
        let note = offered.get(self.chooser.chosen).map_or_else(
            || language.say("Reading…"),
            |&action| language.say(described(action).1),
        );
        let states = self.buttons.each_mut().map(|button| button.state(window));
        let ok = choice_button(&theme, language.say("OK"), true, states[0])
            .id("power-ok")
            .on_click(cx.listener(|this, _, _, cx| this.confirm(cx)));
        let cancel = choice_button(&theme, language.say("Cancel"), false, states[1])
            .id("power-cancel")
            .on_click(|_, _, cx| cx.defer(Popups::close));
        let ok = Interaction::track(ok, cx, |this: &mut Self| Some(&mut this.buttons[0]));
        let cancel = Interaction::track(cancel, cx, |this: &mut Self| Some(&mut this.buttons[1]));
        div()
            .id(NAME)
            .track_focus(self.chooser.focus())
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(theme.spacing.medium)
            .child(
                div()
                    .flex_none()
                    .px(theme.spacing.small)
                    .font_weight(FontWeight::MEDIUM)
                    .child(one_line(language.say("What should the computer do?"))),
            )
            .child(
                div()
                    .id("power-choices")
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.tiny)
                    .overflow_y_scroll()
                    .children(rows),
            )
            .child(
                div()
                    .flex_none()
                    .px(theme.spacing.small)
                    .text_color(theme.colors.on_surface_variant)
                    .child(note),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .justify_end()
                    .gap(theme.spacing.small)
                    .child(cancel)
                    .child(ok),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialog_offers_what_the_computer_can_do() {
        use PowerAction::{Restart, ShutDown, SignOut, SwitchUser};
        let plain = offered(PowerChoices::default());
        assert_eq!(plain, [SwitchUser, SignOut, ShutDown, Restart]);
        assert_eq!(first_choice(&plain), 2);
        let updating = offered(PowerChoices {
            sleep: true,
            hibernate: false,
            updates: true,
        });
        assert_eq!(updating.len(), 7);
        assert_eq!(
            updating[first_choice(&updating)],
            PowerAction::UpdateShutDown
        );
    }

    #[test]
    fn every_choice_has_a_letter_of_its_own() {
        let mut letters: Vec<char> = CHOICES.iter().map(|choice| choice.4).collect();
        letters.sort_unstable();
        letters.dedup();
        assert_eq!(letters.len(), CHOICES.len());
    }

    #[test]
    fn the_action_opens_the_dialog_and_takes_nothing() {
        let mut actions = Actions::default();
        PowerDialogModule.actions(&mut actions).unwrap();
        assert_eq!(actions.check(NAME, None), Ok(()));
        assert!(actions.check(NAME, Some("now")).is_err());
    }
}
