// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The power mode of Windows, for the quick settings. Widgets:
//! `power-mode`, a tile that shows the mode in use and puts the next one in
//! use with a click, and `power-modes`, which it unfolds: the three modes
//! to choose from, and whether battery saver is on. Both share one reading,
//! asked every five seconds while one is shown. Windows offers the modes
//! only while the Balanced power plan is in use; with another plan the tile
//! says so and changes nothing.

use std::time::Duration;

use gpui::{
    App, AppContext, Context, Entity, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Subscription, Window, div,
};

use super::quick::{
    Face, Reading, SettingsPage, Source, TileView, chip, register_tile, unusable_note,
};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::Interaction;
use crate::platform::{PowerMode, PowerModeState};
use crate::theme::Theme;
use crate::ui::TileFace;

const ICON_POWER_MODE: &str = "\u{ec4a}";

const SETTINGS: SettingsPage = SettingsPage {
    address: "ms-settings:powersleep",
    text: "Power and battery",
};

/// The power mode, as the quick settings read it.
pub struct PowerModeSource;

impl Source for PowerModeSource {
    type State = Option<PowerModeState>;

    const INTERVAL: Duration = Duration::from_secs(5);

    fn read() -> Self::State {
        PowerModeState::read()
    }
}

type PowerModeReading = Reading<PowerModeSource>;

/// What Windows calls `mode` in its settings.
fn mode_name(mode: PowerMode) -> &'static str {
    match mode {
        PowerMode::Efficiency => "Best power efficiency",
        PowerMode::Balanced => "Balanced",
        PowerMode::Performance => "Best performance",
    }
}

/// What a chip of `mode` says: shorter than its name, so that three fit
/// in a row.
fn chip_name(mode: PowerMode) -> &'static str {
    match mode {
        PowerMode::Efficiency => "Efficiency",
        PowerMode::Balanced => "Balanced",
        PowerMode::Performance => "Performance",
    }
}

/// Shows `mode` at once and puts it in use.
fn choose(reading: &mut PowerModeReading, mode: PowerMode, cx: &mut Context<PowerModeReading>) {
    let Some(Some(state)) = reading.state().copied() else {
        return;
    };
    if state.mode == Some(mode) {
        return;
    }
    let shown = PowerModeState {
        mode: Some(mode),
        ..state
    };
    reading.change(Some(Some(shown)), move || mode.activate(), |(), _| {}, cx);
}

/// The tile of the power mode.
struct PowerModeTile;

impl Face for PowerModeTile {
    type Model = PowerModeReading;

    fn model(&self, cx: &mut App) -> Entity<PowerModeReading> {
        Reading::shared(cx)
    }

    fn follow(model: &Entity<PowerModeReading>, cx: &mut Context<TileView<Self>>) -> Subscription {
        Reading::follow(model, cx)
    }

    fn face(&self, model: &PowerModeReading, language: Language, _: &App) -> TileFace {
        let state = model.state().copied().flatten();
        let note = match state {
            None => unusable_note(model.state().is_some(), language),
            Some(PowerModeState { mode: None, .. }) => language.say("Set by the power plan"),
            Some(PowerModeState {
                mode: Some(mode), ..
            }) => language.say(mode_name(mode)),
        };
        let mode = state.and_then(|state| state.mode);
        TileFace {
            glyph: ICON_POWER_MODE,
            title: language.say("Power mode").into(),
            note: note.into(),
            // Lit while the machine is not balanced, as a reminder.
            lit: mode.is_some_and(|mode| mode != PowerMode::Balanced),
            usable: mode.is_some(),
        }
    }

    fn click(&self, model: &Entity<PowerModeReading>, cx: &mut App) {
        model.update(cx, |reading, cx| {
            let Some(mode) = reading
                .state()
                .copied()
                .flatten()
                .and_then(|state| state.mode)
            else {
                return;
            };
            choose(reading, mode.next(), cx);
        });
    }
}

/// The three modes to choose from.
struct PowerModes {
    reading: Entity<PowerModeReading>,
    chips: [Interaction; 3],
    settings: Interaction,
    _follow: Subscription,
}

impl PowerModes {
    fn new(cx: &mut Context<Self>) -> Self {
        let reading = Reading::<PowerModeSource>::shared(cx);
        let motion = cx.global::<Theme>().motion;
        Self {
            _follow: Reading::follow(&reading, cx),
            reading,
            chips: [Interaction::new(&motion); 3],
            settings: Interaction::new(&motion),
        }
    }
}

impl Render for PowerModes {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let state = self.reading.read(cx).state().copied();
        let link_state = self.settings.state(window);
        let settings = SETTINGS.link(&theme, language, link_state);
        let settings = Interaction::track(settings, cx, |this: &mut Self| Some(&mut this.settings));
        let frame = div().w_full().flex().flex_col().gap(theme.spacing.small);
        let note = |text: &'static str| {
            div()
                .px(theme.spacing.small)
                .text_color(theme.colors.on_surface_variant)
                .child(language.say(text))
        };
        let state = match state {
            None => {
                return frame
                    .child(note("Reading…"))
                    .child(settings)
                    .into_any_element();
            }
            Some(None) => {
                let text = "This version of Windows has no power modes";
                return frame.child(note(text)).child(settings).into_any_element();
            }
            Some(Some(state)) => state,
        };
        let frame = match state.mode {
            Some(_) => frame,
            None => frame.child(note(
                "The power plan in use is not Balanced, so Windows offers no power mode",
            )),
        };
        let mut chips = Vec::new();
        for (index, mode) in PowerMode::ALL.into_iter().enumerate() {
            let shown = self.chips[index].state(window);
            let chosen = state.mode == Some(mode);
            let mut element = chip(
                &theme,
                ("power-mode", index),
                language.say(chip_name(mode)),
                chosen,
                shown,
            );
            if state.mode.is_some() {
                element = element.on_click(cx.listener(move |this, _, _, cx| {
                    this.reading
                        .update(cx, |reading, cx| choose(reading, mode, cx));
                }));
                element = Interaction::track(element, cx, move |this: &mut Self| {
                    this.chips.get_mut(index)
                });
            }
            chips.push(element.into_any_element());
        }
        let frame = frame.child(
            div()
                .flex()
                .flex_wrap()
                .gap(theme.spacing.small)
                .px(theme.spacing.small)
                .children(chips),
        );
        let frame = match state.saver {
            true => frame.child(note("Battery saver is on")),
            false => frame,
        };
        frame.child(settings).into_any_element()
    }
}

pub struct PowerModeModule;

impl Module for PowerModeModule {
    fn name(&self) -> &'static str {
        "power-mode"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        register_tile(widgets, "power-mode", |_| PowerModeTile)?;
        widgets.register_panel("power-modes", |_, cx| cx.new(PowerModes::new).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_has_its_name_and_a_shorter_one() {
        for mode in PowerMode::ALL {
            assert!(chip_name(mode).len() <= mode_name(mode).len());
        }
    }
}
