// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Power management, as the power settings of KDE Plasma and its battery
//! widget offer it. Widget `power-settings`, in the popup of the same name
//! in the middle of the display, which the action `power-settings` opens:
//! a switch that blocks sleep and screen locking, the performance mode of
//! the shell, after how long the screen goes off and the computer sleeps
//! on mains power and on battery, and what closing the lid and pressing
//! the power and sleep buttons do, as the power plan in use keeps them.
//! Tiles `inhibit` and `performance` for the quick settings, and actions
//! of the same names.
//!
//! A battery charge limit is not offered: Windows has no setting or
//! interface for one, only the makers' own tools.

use std::collections::HashMap;

use gpui::{
    App, AppContext, AsyncApp, Context, Div, Entity, FontWeight, Global, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, div, px,
};

use super::quick::{Face, SettingsPage, TileView, chip, on_or_off, register_tile};
use crate::actions::{Action, Actions};
use crate::config::{PopupConfig, PopupPlace, WidgetSpec};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, InteractionState, Spring};
use crate::performance::{Performance, PerformanceMode};
use crate::platform::{ButtonAction, PowerSetting, PowerSettings, SleepBlock, Supply};
use crate::runners::fill;
use crate::shell::Shell;
use crate::theme::{Motion, Theme};
use crate::ui::{TileFace, list_title, one_line, switch};

/// The name of the popup, of its widget and of the action that opens it.
const NAME: &str = "power-settings";

/// The names of the tile and action that block sleep, and of those of the
/// performance mode.
const INHIBIT: &str = "inhibit";
const PERFORMANCE: &str = "performance";

/// How large the popup is, in logical pixels.
const WIDTH: f32 = 440.;
const HEIGHT: f32 = 720.;

const ICON_INHIBIT: &str = "\u{e708}";
const ICON_PERFORMANCE: &str = "\u{e945}";

/// What Windows lists as the reason of the request that blocks sleep.
const REASON: &str = "Tessera: sleep and screen locking are blocked by hand";

const SETTINGS: SettingsPage = SettingsPage {
    address: "ms-settings:powersleep",
    text: "Power and battery",
};

/// The times offered for the screen to go off and the computer to sleep,
/// in seconds; 0 is never.
const TIMEOUTS: [u32; 10] = [60, 120, 300, 600, 900, 1800, 3600, 7200, 18_000, 0];

pub struct PowerSettingsModule;

impl Module for PowerSettingsModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(NAME, |_, cx| cx.new(PowerPanel::new).into())?;
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
        })?;
        register_tile(widgets, INHIBIT, |_| InhibitTile)?;
        register_tile(widgets, PERFORMANCE, |_| PerformanceTile)
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        actions.register(NAME, Action::new(|_, cx| Shell::toggle_popup(NAME, cx)))?;
        let inhibit = Action::new(inhibit).checking(|arg| match arg {
            None | Some("on" | "off") => Ok(()),
            Some(_) => Err("`inhibit` takes `on`, `off` or no `arg`".to_owned()),
        });
        actions.register(INHIBIT, inhibit)?;
        let performance = Action::new(Performance::perform).checking(Performance::check);
        actions.register(PERFORMANCE, performance)
    }
}

/// The pointer feedback of the controls of a view, each under a key of
/// its own, made as they are first drawn.
pub(super) struct Feedback {
    motion: Motion,
    controls: HashMap<(&'static str, usize), Interaction>,
}

impl Feedback {
    pub(super) fn new(motion: Motion) -> Self {
        Self {
            motion,
            controls: HashMap::new(),
        }
    }

    /// The feedback of the control `key` now.
    pub(super) fn state(
        &mut self,
        key: (&'static str, usize),
        window: &Window,
    ) -> InteractionState {
        let motion = self.motion;
        { self.controls.entry(key) }
            .or_insert_with(|| Interaction::new(&motion))
            .state(window)
    }

    /// `element` made to show the pointer on it as the control `key` of
    /// the view `V`, whose feedback `find` gives.
    pub(super) fn track<V: 'static>(
        element: Stateful<Div>,
        key: (&'static str, usize),
        cx: &mut Context<V>,
        find: impl Fn(&mut V) -> &mut Self + Clone + 'static,
    ) -> Stateful<Div> {
        Interaction::track(element, cx, move |view: &mut V| {
            find(view).controls.get_mut(&key)
        })
    }
}

/// The request that keeps the computer awake, while it is made.
pub struct Inhibit {
    block: Option<SleepBlock>,
}

/// The one [`Inhibit`] of the shell.
struct SharedInhibit(Entity<Inhibit>);

impl Global for SharedInhibit {}

impl Inhibit {
    fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(shared) = cx.try_global::<SharedInhibit>() {
            return shared.0.clone();
        }
        let inhibit = cx.new(|_| Self { block: None });
        cx.set_global(SharedInhibit(inhibit.clone()));
        let ending = inhibit.downgrade();
        // Windows ends the request with the process anyway; this ends it
        // before the shell closes its windows.
        cx.on_app_quit(move |cx| {
            ending.update(cx, |this, _| this.block = None).ok();
            async {}
        })
        .detach();
        inhibit
    }

    fn is_on(&self) -> bool {
        self.block.is_some()
    }

    /// Makes the request, or ends it. The calls return at once.
    fn set(&mut self, on: bool, cx: &mut Context<Self>) {
        if on == self.is_on() {
            return;
        }
        self.block = match on {
            true => SleepBlock::begin(REASON)
                .map_err(|error| report!("sleep is not blocked: {error}"))
                .ok(),
            false => None,
        };
        cx.notify();
    }
}

/// Does the action `inhibit` with `arg`: `on`, `off`, or the other way
/// round from now.
fn inhibit(arg: Option<&str>, cx: &mut App) {
    let inhibit = Inhibit::shared(cx);
    inhibit.update(cx, |this, cx| {
        let on = match arg {
            Some("on") => true,
            Some("off") => false,
            _ => !this.is_on(),
        };
        this.set(on, cx);
    });
}

/// The tile that blocks sleep and screen locking.
struct InhibitTile;

impl Face for InhibitTile {
    type Model = Inhibit;

    fn model(&self, cx: &mut App) -> Entity<Inhibit> {
        Inhibit::shared(cx)
    }

    fn follow(model: &Entity<Inhibit>, cx: &mut Context<TileView<Self>>) -> Subscription {
        cx.observe(model, |_, _, cx| cx.notify())
    }

    fn face(&self, model: &Inhibit, language: Language, _: &App) -> TileFace {
        TileFace {
            glyph: ICON_INHIBIT,
            title: language.say("Block sleep").into(),
            note: on_or_off(model.is_on(), language).into(),
            lit: model.is_on(),
            usable: true,
        }
    }

    fn click(&self, _: &Entity<Inhibit>, cx: &mut App) {
        inhibit(None, cx);
    }
}

/// The tile of the performance mode: shows the mode in force, and a click
/// puts the next one in force.
struct PerformanceTile;

impl Face for PerformanceTile {
    type Model = Performance;

    fn model(&self, cx: &mut App) -> Entity<Performance> {
        Performance::shared(cx)
    }

    fn follow(model: &Entity<Performance>, cx: &mut Context<TileView<Self>>) -> Subscription {
        cx.observe(model, |_, _, cx| cx.notify())
    }

    fn face(&self, model: &Performance, language: Language, _: &App) -> TileFace {
        let mode = model.in_force();
        let note = match model.by_hand() {
            true => format!(
                "{} · {}",
                language.say(mode.label()),
                language.say("By hand")
            ),
            false => language.say(mode.label()).to_owned(),
        };
        TileFace {
            glyph: ICON_PERFORMANCE,
            title: language.say("Performance mode").into(),
            note: note.into(),
            lit: mode != PerformanceMode::Off,
            usable: true,
        }
    }

    fn click(&self, _: &Entity<Performance>, cx: &mut App) {
        Performance::perform(None, cx);
    }
}

/// The times to offer for a timeout that is `current` now: the usual
/// ones, and the one in use where it is none of them, never last.
fn timeout_choices(current: Option<u32>) -> Vec<u32> {
    let mut choices: Vec<u32> = TIMEOUTS.into_iter().filter(|&time| time != 0).collect();
    if let Some(current) = current.filter(|&time| time != 0 && !choices.contains(&time)) {
        choices.push(current);
        choices.sort_unstable();
    }
    choices.push(0);
    choices
}

/// How a timeout of `seconds` is said: in minutes below two hours and in
/// hours above, unless it is no whole number of them.
fn timeout_text(seconds: u32, language: Language) -> String {
    let minutes = seconds.div_ceil(60);
    match seconds {
        0 => language.say("Never").to_owned(),
        _ if minutes >= 120 && minutes.is_multiple_of(60) => {
            fill(language.say("{} h"), &[&(minutes / 60).to_string()])
        }
        _ => fill(language.say("{} min"), &[&minutes.to_string()]),
    }
}

/// The actions to offer for `setting`: the lid cannot only turn the
/// screen off, and hibernation is offered where the machine has it or it
/// is in use.
fn action_choices(setting: PowerSetting, hibernate: bool, current: Option<u32>) -> Vec<u32> {
    let current = current.and_then(ButtonAction::of);
    { ButtonAction::ALL.into_iter() }
        .filter(|action| match action {
            ButtonAction::Hibernate => hibernate || current == Some(*action),
            ButtonAction::ScreenOff => setting != PowerSetting::Lid,
            _ => true,
        })
        .map(ButtonAction::index)
        .collect()
}

/// What an action is called.
fn action_label(action: ButtonAction) -> &'static str {
    match action {
        ButtonAction::Nothing => "Do nothing",
        ButtonAction::Sleep => "Sleep",
        ButtonAction::Hibernate => "Hibernate",
        ButtonAction::ShutDown => "Shut down",
        ButtonAction::ScreenOff => "Turn off the display",
    }
}

/// What the group of a setting is headed by.
fn setting_title(setting: PowerSetting) -> &'static str {
    match setting {
        PowerSetting::ScreenOff => "Turn off the screen after",
        PowerSetting::Sleep => "Put the computer to sleep after",
        PowerSetting::Lid => "When I close the lid",
        PowerSetting::PowerButton => "When I press the power button",
        PowerSetting::SleepButton => "When I press the sleep button",
    }
}

/// What a row of the source is headed by.
fn supply_label(supply: Supply) -> &'static str {
    match supply {
        Supply::Mains => "Plugged in",
        Supply::Battery => "On battery",
    }
}

/// The key of the feedback of each setting's choices.
fn feedback_key(setting: PowerSetting) -> &'static str {
    match setting {
        PowerSetting::ScreenOff => "screen-off",
        PowerSetting::Sleep => "sleep",
        PowerSetting::Lid => "lid",
        PowerSetting::PowerButton => "power-button",
        PowerSetting::SleepButton => "sleep-button",
    }
}

/// The choices of the performance mode: by the configuration, then each
/// mode by hand.
const PERFORMANCE_CHOICES: [Option<PerformanceMode>; 4] = [
    None,
    Some(PerformanceMode::Off),
    Some(PerformanceMode::Minimal),
    Some(PerformanceMode::Extreme),
];

/// The settings of power, for the popup.
struct PowerPanel {
    /// `None` until read; then `None` inside where Windows gave no plan.
    settings: Option<Option<PowerSettings>>,
    inhibit: Entity<Inhibit>,
    performance: Entity<Performance>,
    /// How far the switch of the block is on its way to on.
    travel: Spring,
    feedback: Feedback,
    _follow: [Subscription; 2],
}

impl PowerPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        let inhibit = Inhibit::shared(cx);
        let performance = Performance::shared(cx);
        let motion = cx.global::<Theme>().motion;
        let mut travel = Spring::new(motion.spatial_fast, 0.);
        travel.jump_to(if inhibit.read(cx).is_on() { 1. } else { 0. });
        let follow = [
            cx.observe(&inhibit, |this, inhibit, cx| {
                let on = inhibit.read(cx).is_on();
                this.travel.animate_to(if on { 1. } else { 0. });
                cx.notify();
            }),
            cx.observe(&performance, |_, _, cx| cx.notify()),
        ];
        Self::read(cx);
        Self {
            settings: None,
            inhibit,
            performance,
            travel,
            feedback: Feedback::new(motion),
            _follow: follow,
        }
    }

    /// Reads the settings away from the thread that draws.
    fn read(cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx
                .background_executor()
                .spawn(async { PowerSettings::read() });
            let settings = read.await;
            this.update(cx, |this, cx| {
                this.settings = Some(settings);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows `value` for `setting` on `supply` at once and writes it into
    /// the power plan; where that fails, what the plan keeps is read
    /// again.
    fn choose(
        &mut self,
        setting: PowerSetting,
        supply: Supply,
        value: u32,
        cx: &mut Context<Self>,
    ) {
        let Some(Some(settings)) = &mut self.settings else {
            return;
        };
        if settings.value(setting, supply) == Some(value) {
            return;
        }
        settings.set(setting, supply, value);
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let write = { cx.background_executor() }
                .spawn(async move { PowerSettings::write(setting, supply, value) });
            if let Err(error) = write.await {
                report!("{error}");
                this.update(cx, |_, cx| Self::read(cx)).ok();
            }
        })
        .detach();
    }

    /// The row with the switch that blocks sleep.
    fn inhibit_row(
        &mut self,
        theme: &Theme,
        language: Language,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let key = (INHIBIT, 0);
        let state = self.feedback.state(key, window);
        let travel = self.travel.value(window);
        let row = div()
            .id(INHIBIT)
            .flex_none()
            .px(theme.spacing.medium)
            .py(theme.spacing.small)
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container_highest)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(one_line(language.say("Block sleep and screen locking"))),
                    )
                    .child(
                        div()
                            .text_color(theme.colors.on_surface_variant)
                            .child(language.say(
                                "Keeps the screen on and the computer awake until switched off or Tessera closes.",
                            )),
                    ),
            )
            .child(switch(theme, "inhibit-switch", travel, state))
            .on_click(cx.listener(|this, _, _, cx| {
                this.inhibit.update(cx, |inhibit, cx| {
                    let on = !inhibit.is_on();
                    inhibit.set(on, cx);
                });
            }));
        Feedback::track(row, key, cx, |this: &mut Self| &mut this.feedback)
    }

    /// The chips of the performance mode and what is in force.
    fn performance_rows(
        &mut self,
        theme: &Theme,
        language: Language,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let performance = self.performance.read(cx);
        let (in_force, by_hand) = (performance.in_force(), performance.by_hand());
        let mut chips = Vec::new();
        for (index, choice) in PERFORMANCE_CHOICES.into_iter().enumerate() {
            let key = (PERFORMANCE, index);
            let state = self.feedback.state(key, window);
            let (label, chosen) = match choice {
                None => ("Automatic", !by_hand),
                Some(mode) => (mode.label(), by_hand && mode == in_force),
            };
            let element = chip(
                theme,
                ("performance-choice", index),
                language.say(label),
                chosen,
                state,
            )
            .on_click(cx.listener(move |_, _, _, cx| Performance::choose(choice, cx)));
            chips.push(Feedback::track(element, key, cx, |this: &mut Self| {
                &mut this.feedback
            }));
        }
        let in_force = fill(
            language.say("In force: {}"),
            &[language.say(in_force.label())],
        );
        vec![
            list_title(theme, language.say("Performance mode")).into_any_element(),
            chip_row(theme).children(chips).into_any_element(),
            note(theme, in_force).into_any_element(),
        ]
    }

    /// The group of `setting`: its title and a row of choices for each
    /// power source the machine has.
    fn setting_rows(
        &mut self,
        setting: PowerSetting,
        settings: &PowerSettings,
        theme: &Theme,
        language: Language,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut rows =
            vec![list_title(theme, language.say(setting_title(setting))).into_any_element()];
        let supplies: &[Supply] = match settings.battery {
            true => &Supply::ALL,
            false => &[Supply::Mains],
        };
        for (row, &supply) in supplies.iter().enumerate() {
            let current = settings.value(setting, supply);
            let choices = match setting.is_timeout() {
                true => timeout_choices(current),
                false => action_choices(setting, settings.hibernate, current),
            };
            let mut chips = Vec::new();
            for (index, value) in choices.into_iter().enumerate() {
                let key = (feedback_key(setting), row * 100 + index);
                let state = self.feedback.state(key, window);
                let label: SharedString = match setting.is_timeout() {
                    true => timeout_text(value, language).into(),
                    false => {
                        let action = ButtonAction::of(value).unwrap_or(ButtonAction::Nothing);
                        language.say(action_label(action)).into()
                    }
                };
                let element = chip(theme, key, label, current == Some(value), state).on_click(
                    cx.listener(move |this, _, _, cx| this.choose(setting, supply, value, cx)),
                );
                chips.push(Feedback::track(element, key, cx, |this: &mut Self| {
                    &mut this.feedback
                }));
            }
            if settings.battery {
                rows.push(note(theme, language.say(supply_label(supply))).into_any_element());
            }
            rows.push(chip_row(theme).children(chips).into_any_element());
        }
        rows
    }
}

/// A row of chips that wraps onto more lines where they do not fit.
fn chip_row(theme: &Theme) -> Div {
    div()
        .flex_none()
        .flex()
        .flex_wrap()
        .gap(theme.spacing.small)
        .px(theme.spacing.small)
}

/// A line of text in the quieter color.
fn note(theme: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px(theme.spacing.small)
        .text_color(theme.colors.on_surface_variant)
        .child(text.into())
}

impl Render for PowerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut frame = div()
            .id(NAME)
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .overflow_y_scroll()
            .child(
                div()
                    .flex_none()
                    .px(theme.spacing.small)
                    .pb(theme.spacing.tiny)
                    .font_weight(FontWeight::MEDIUM)
                    .child(one_line(language.say("Power settings"))),
            )
            .child(self.inhibit_row(&theme, language, window, cx))
            .children(self.performance_rows(&theme, language, window, cx));
        match self.settings.clone() {
            None => frame = frame.child(note(&theme, language.say("Reading…"))),
            Some(None) => {
                let text = "Windows gives no power plan in use, so its settings cannot be shown";
                frame = frame.child(note(&theme, language.say(text)));
            }
            Some(Some(settings)) => {
                for setting in PowerSetting::ALL {
                    if settings.applies(setting) {
                        let rows =
                            self.setting_rows(setting, &settings, &theme, language, window, cx);
                        frame = frame.children(rows);
                    }
                }
                if settings.battery {
                    let text = "Windows has no setting for a battery charge limit; where the battery has one, the maker's own app sets it.";
                    frame = frame.child(note(&theme, language.say(text)).pt(theme.spacing.small));
                }
            }
        }
        let key = ("settings-link", 0);
        let state = self.feedback.state(key, window);
        let link = SETTINGS.link(&theme, language, state);
        frame.child(Feedback::track(link, key, cx, |this: &mut Self| {
            &mut this.feedback
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timeout_in_use_is_offered_among_the_usual_ones() {
        let usual = timeout_choices(Some(600));
        assert_eq!(usual.len(), TIMEOUTS.len());
        assert_eq!(usual.last(), Some(&0));
        let odd = timeout_choices(Some(420));
        assert_eq!(odd.len(), TIMEOUTS.len() + 1);
        assert!(
            odd.windows(2)
                .take(odd.len() - 2)
                .all(|pair| pair[0] < pair[1])
        );
        assert!(odd.contains(&420));
        assert_eq!(timeout_choices(None), timeout_choices(Some(0)));
    }

    #[test]
    fn a_timeout_is_said_in_minutes_or_hours() {
        let english = Language::English;
        assert_eq!(timeout_text(0, english), "Never");
        assert_eq!(timeout_text(60, english), "1 min");
        assert_eq!(timeout_text(3600, english), "60 min");
        assert_eq!(timeout_text(7200, english), "2 h");
        assert_eq!(timeout_text(18_000, english), "5 h");
        assert_eq!(timeout_text(9000, english), "150 min");
        assert_eq!(timeout_text(90, english), "2 min");
    }

    #[test]
    fn the_lid_cannot_only_turn_the_screen_off() {
        let lid = action_choices(PowerSetting::Lid, false, Some(1));
        assert!(!lid.contains(&ButtonAction::ScreenOff.index()));
        assert!(!lid.contains(&ButtonAction::Hibernate.index()));
        let button = action_choices(PowerSetting::PowerButton, true, None);
        assert_eq!(button.len(), ButtonAction::ALL.len());
        let hibernating = action_choices(PowerSetting::Lid, false, Some(2));
        assert!(hibernating.contains(&ButtonAction::Hibernate.index()));
    }

    #[test]
    fn the_actions_take_what_they_say() {
        let mut actions = Actions::default();
        PowerSettingsModule.actions(&mut actions).unwrap();
        assert_eq!(actions.check(NAME, None), Ok(()));
        assert!(actions.check(NAME, Some("x")).is_err());
        for arg in [None, Some("on"), Some("off")] {
            assert_eq!(actions.check(INHIBIT, arg), Ok(()));
        }
        assert!(actions.check(INHIBIT, Some("yes")).is_err());
        assert_eq!(actions.check(PERFORMANCE, Some("minimal")), Ok(()));
    }
}
