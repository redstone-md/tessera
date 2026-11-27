// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How the displays show the desktop, as Win+P chooses it: the widget
//! `display-mode`, in the popup of the same name, lists PC screen only,
//! Duplicate, Extend and Second screen only with the one in use marked,
//! and a way to a wireless display. The action `display-mode` opens the
//! popup (Win+P where the keys of Windows are in use), or with a mode as
//! `arg` switches to it at once.

use gpui::{
    App, AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};

use crate::actions::{Action, Actions};
use crate::config::{PopupConfig, WidgetSpec};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::platform::{DisplayMode, Launch};
use crate::popup::Popups;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{Chooser, Pick, choice_row, icon, one_line};

/// The name of the popup, of its widget and of the action.
const NAME: &str = "display-mode";

/// What opens the choice of a wireless display in the settings of Windows.
const WIRELESS: &str = "ms-settings-connectabledevices:devicediscovery";

const ICON_CHECK: &str = "\u{e73e}";
const ICON_WIRELESS: &str = "\u{e703}";

/// Each mode with the name an `arg` gives it, its label and its icon.
const MODES: [(DisplayMode, &str, &str, &str); 4] = [
    (
        DisplayMode::PcScreenOnly,
        "pc-screen-only",
        "PC screen only",
        "\u{e7f8}",
    ),
    (DisplayMode::Duplicate, "duplicate", "Duplicate", "\u{e8c8}"),
    (DisplayMode::Extend, "extend", "Extend", "\u{e740}"),
    (
        DisplayMode::SecondScreenOnly,
        "second-screen-only",
        "Second screen only",
        "\u{e7f4}",
    ),
];

pub struct DisplayModeModule;

impl Module for DisplayModeModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(NAME, |_, cx| cx.new(DisplayModes::new).into())?;
        widgets.register_popup(PopupConfig {
            name: NAME.to_owned(),
            place: Default::default(),
            width: 320.,
            height: 336.,
            content: vec![WidgetSpec {
                widget: NAME.to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        // The flyout of Win+P belongs to Explorer, which keeps the keys.
        let action = Action::new(display_mode)
            .taking_reserved_keys()
            .checking(|arg| match arg {
                None => Ok(()),
                Some(name) if mode_named(name).is_some() => Ok(()),
                Some(_) => Err(
                    "`display-mode` takes `pc-screen-only`, `duplicate`, `extend`, `second-screen-only` or no `arg`"
                        .to_owned(),
                ),
            })
            .by_default("win+p", None);
        actions.register(NAME, action)
    }
}

/// The mode an `arg` calls `name`.
fn mode_named(name: &str) -> Option<DisplayMode> {
    { MODES.iter() }
        .find(|(_, known, ..)| *known == name)
        .map(|&(mode, ..)| mode)
}

/// The entry of [`MODES`] for `mode`.
fn look(mode: DisplayMode) -> (&'static str, &'static str) {
    let (_, _, label, glyph) = { MODES.into_iter() }
        .find(|&(known, ..)| known == mode)
        .expect("every mode is listed");
    (label, glyph)
}

/// Does the action `display-mode` with `arg`.
fn display_mode(arg: Option<&str>, cx: &mut App) {
    match arg.and_then(mode_named) {
        Some(mode) => switch(mode, cx),
        None => Shell::toggle_popup(NAME, cx),
    }
}

/// Switches the displays to `mode` away from the thread that draws.
fn switch(mode: DisplayMode, cx: &mut App) {
    cx.background_executor()
        .spawn(async move {
            if let Err(error) = mode.apply() {
                report!("{error:#}");
            }
        })
        .detach();
}

/// What was read of the displays: the mode in use and the modes possible.
#[derive(Clone, Debug, Default, PartialEq)]
struct Reading {
    current: Option<DisplayMode>,
    possible: Vec<DisplayMode>,
}

impl Reading {
    fn now() -> Self {
        Self {
            current: DisplayMode::current(),
            possible: { DisplayMode::ALL.into_iter() }
                .filter(|mode| mode.possible())
                .collect(),
        }
    }
}

/// The modes to choose from, and the way to a wireless display after
/// them.
struct DisplayModes {
    /// `None` until the displays have been asked.
    reading: Option<Reading>,
    chooser: Chooser,
}

impl DisplayModes {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let reading = cx.background_executor().spawn(async { Reading::now() });
            let reading = reading.await;
            this.update(cx, |this, cx| {
                let current = reading.current;
                let index = { DisplayMode::ALL.iter() }.position(|&mode| Some(mode) == current);
                this.chooser.chosen = index.unwrap_or(0);
                this.reading = Some(reading);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            reading: None,
            chooser: Chooser::new(cx),
        }
    }

    /// How many rows there are: the four modes and the wireless display.
    const ROWS: usize = DisplayMode::ALL.len() + 1;

    /// Takes the row `index`: switches to its mode, where that is
    /// possible, or opens the choice of a wireless display.
    fn take(&mut self, index: usize, cx: &mut Context<Self>) {
        match DisplayMode::ALL.get(index) {
            Some(&mode) => {
                let possible = self
                    .reading
                    .as_ref()
                    .is_none_or(|reading| reading.possible.contains(&mode));
                if !possible {
                    return;
                }
                switch(mode, cx);
            }
            None => Launch::new(WIRELESS).named("The wireless displays").start(),
        }
        cx.defer(Popups::close);
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match Pick::of(event) {
            Some(Pick::Take) => self.take(self.chooser.chosen, cx),
            Some(Pick::Leave) => cx.defer(Popups::close),
            Some(Pick::Letter(_)) | None => return,
            Some(pick) => self.chooser.pick(pick, Self::ROWS),
        }
        cx.notify();
    }
}

impl Render for DisplayModes {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        self.chooser.take_focus(window);
        let reading = self.reading.clone();
        let mut rows = Vec::with_capacity(Self::ROWS);
        for (index, mode) in DisplayMode::ALL.into_iter().enumerate() {
            let (label, glyph) = look(mode);
            let state = self.chooser.state(index, &theme, window);
            let chosen = index == self.chooser.chosen;
            let possible =
                { reading.as_ref() }.is_none_or(|reading| reading.possible.contains(&mode));
            let current = reading.as_ref().and_then(|reading| reading.current) == Some(mode);
            let row = choice_row(&theme, glyph, language.say(label), None, chosen, state)
                .opacity(if possible { 1. } else { 0.5 })
                .children(current.then(|| {
                    icon(&theme, ICON_CHECK)
                        .flex_none()
                        .text_size(theme.typography.label_size)
                        .text_color(theme.colors.primary)
                }))
                .id(("display-mode", index))
                .on_click(cx.listener(move |this, _, _, cx| this.take(index, cx)));
            rows.push(Chooser::track(row, index, cx, |this: &mut Self| {
                &mut this.chooser
            }));
        }
        let index = DisplayMode::ALL.len();
        let state = self.chooser.state(index, &theme, window);
        let chosen = index == self.chooser.chosen;
        let label = language.say("Connect to a wireless display");
        let wireless = choice_row(&theme, ICON_WIRELESS, label, None, chosen, state)
            .id(("display-mode", index))
            .on_click(cx.listener(move |this, _, _, cx| this.take(index, cx)));
        rows.push(Chooser::track(wireless, index, cx, |this: &mut Self| {
            &mut this.chooser
        }));
        let alone = reading
            .as_ref()
            .is_some_and(|reading| reading.possible.len() < 2);
        div()
            .id(NAME)
            .track_focus(self.chooser.focus())
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .flex_none()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(
                div()
                    .flex_none()
                    .px(theme.spacing.small)
                    .pb(theme.spacing.tiny)
                    .font_weight(FontWeight::MEDIUM)
                    .child(one_line(language.say("Project"))),
            )
            .children(rows)
            .children(alone.then(|| {
                div()
                    .flex_none()
                    .px(theme.spacing.small)
                    .pt(px(2.))
                    .text_color(theme.colors.on_surface_variant)
                    .child(one_line(language.say("Only one display is connected.")))
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_action_takes_the_names_of_the_modes() {
        let mut actions = Actions::default();
        DisplayModeModule.actions(&mut actions).unwrap();
        for (_, name, ..) in MODES {
            assert_eq!(actions.check(NAME, Some(name)), Ok(()));
        }
        assert_eq!(actions.check(NAME, None), Ok(()));
        assert!(actions.check(NAME, Some("mirror")).is_err());
        assert!(actions.takes_reserved_keys(NAME));
        let keys = actions.default_keys();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].press, "win+p");
    }

    #[test]
    fn every_mode_has_a_name_and_a_look() {
        for mode in DisplayMode::ALL {
            assert!(!look(mode).0.is_empty());
        }
        assert_eq!(mode_named("extend"), Some(DisplayMode::Extend));
        assert_eq!(mode_named("Extend"), None);
    }
}
