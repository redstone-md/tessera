// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Two small buttons for the look of Windows. Widget `appearance`: whether
//! Windows is light or dark, and a click that makes it the other. Widget
//! `desktop`: a click shows the desktop or brings the windows back.

use std::time::Duration;

use gpui::{
    AppContext, AsyncApp, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, WeakEntity, Window,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{Taskbar, light_mode, set_light_mode};
use crate::theme::Theme;
use crate::ui::{icon, pill, state_layer};

const ICON_LIGHT: &str = "\u{e706}";
const ICON_DARK: &str = "\u{e708}";
const ICON_DESKTOP: &str = "\u{e7f4}";

/// How often the mode is read: the settings of Windows may change it.
const CHECK_INTERVAL: Duration = Duration::from_secs(3);

pub struct LookModule;

impl Module for LookModule {
    fn name(&self) -> &'static str {
        "look"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("appearance", |_, cx| cx.new(Appearance::new).into())?;
        widgets.register("desktop", |_, cx| {
            let motion = cx.global::<Theme>().motion;
            cx.new(|_| DesktopButton {
                interaction: Interaction::new(&motion),
                pressed: Spring::new(motion.effects, 0.),
            })
            .into()
        })
    }
}

/// The icon of the mode: a sun while Windows is light, a moon while it is
/// dark or does not tell.
fn icon_of(light: Option<bool>) -> &'static str {
    match light {
        Some(true) => ICON_LIGHT,
        Some(false) | None => ICON_DARK,
    }
}

/// Shows the mode that was last read.
struct Appearance {
    /// `None` until Windows has told.
    light: Option<bool>,
    interaction: Interaction,
    /// 1 at the moment the mode changes, then back to 0.
    changed: Spring,
}

impl Appearance {
    fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let light = cx.background_executor().spawn(async { light_mode() });
                let light = light.await;
                if this.update(cx, |this, cx| this.take(light, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(CHECK_INTERVAL).await;
            }
        })
        .detach();
        Self {
            light: None,
            interaction: Interaction::new(&motion),
            changed: Spring::new(motion.effects, 0.),
        }
    }

    /// Takes in what was read; a reading that failed keeps the last one.
    fn take(&mut self, light: Option<bool>, cx: &mut Context<Self>) {
        if light.is_none() || light == self.light {
            return;
        }
        // The first reading is no change.
        if self.light.is_some() {
            self.changed.jump_to(1.);
            self.changed.animate_to(0.);
        }
        self.light = light;
        cx.notify();
    }

    /// Makes Windows the other of light and dark.
    fn turn(&mut self, cx: &mut Context<Self>) {
        let Some(light) = self.light else {
            return;
        };
        match set_light_mode(!light) {
            Ok(()) => self.take(Some(!light), cx),
            Err(error) => report!("Windows was not made light or dark: {error}"),
        }
    }
}

impl Render for Appearance {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let colors = theme.colors;
        let changed = self.changed.value(window).clamp(0., 1.);
        let state = self.interaction.state(window);
        let ground = mix(
            theme.on_bar(colors.surface_container),
            colors.secondary_container,
            changed,
        );
        let button = pill(&theme)
            .id("appearance")
            .bg(state_layer(&theme, ground, state))
            .text_color(colors.on_surface)
            .on_click(cx.listener(|this, _, _, cx| this.turn(cx)))
            .child(icon(&theme, icon_of(self.light)));
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}

/// A click shows the desktop or brings the windows back.
struct DesktopButton {
    interaction: Interaction,
    /// 1 at the moment of a click, then back to 0.
    pressed: Spring,
}

impl DesktopButton {
    fn toggle(&mut self, cx: &mut Context<Self>) {
        if !Taskbar::toggle_desktop() {
            report!("there is no taskbar to ask for the desktop");
            return;
        }
        self.pressed.jump_to(1.);
        self.pressed.animate_to(0.);
        cx.notify();
    }
}

impl Render for DesktopButton {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let colors = theme.colors;
        let pressed = self.pressed.value(window).clamp(0., 1.);
        let state = self.interaction.state(window);
        let ground = mix(
            theme.on_bar(colors.surface_container),
            colors.secondary_container,
            pressed,
        );
        let button = pill(&theme)
            .id("desktop")
            .bg(state_layer(&theme, ground, state))
            .text_color(colors.on_surface)
            .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)))
            .child(icon(&theme, ICON_DESKTOP));
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_follows_the_mode() {
        assert_eq!(icon_of(Some(true)), ICON_LIGHT);
        assert_eq!(icon_of(Some(false)), ICON_DARK);
        assert_eq!(icon_of(None), ICON_DARK);
    }
}
