// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `keyboard` widget: the language the window in front is typed in,
//! and a click that asks that window for the next one.

use std::time::Duration;

use gpui::{
    AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, WeakEntity, Window, div,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{InputLanguage, Typing};
use crate::theme::Theme;
use crate::ui::{icon, pill, state_layer};

const ICON_KEYBOARD: &str = "\u{e765}";

/// How often the language is looked at: Windows announces a change only to
/// the window it happens in.
const CHECK_INTERVAL: Duration = Duration::from_millis(400);

pub struct KeyboardModule;

impl Module for KeyboardModule {
    fn name(&self) -> &'static str {
        "keyboard"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("keyboard", |_, cx| cx.new(Keyboard::new).into())
    }
}

/// Shows the language of the window that was last seen in front.
struct Keyboard {
    typing: Option<Typing>,
    interaction: Interaction,
    /// 1 at the moment the language changes, then back to 0.
    changed: Spring,
}

impl Keyboard {
    fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                if this.update(cx, Self::look).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            typing: Typing::in_front(),
            interaction: Interaction::new(&motion),
            changed: Spring::new(motion.effects, 0.),
        }
    }

    /// Takes in the window in front. A window of Tessera itself in front
    /// changes nothing, so that a click here still finds the other one.
    fn look(&mut self, cx: &mut Context<Self>) {
        let Some(typing) = Typing::in_front() else {
            return;
        };
        if self.typing == Some(typing) {
            return;
        }
        let shown = self.typing.map(|typing| typing.language);
        if shown.is_some_and(|shown| shown != typing.language) {
            self.changed.jump_to(1.);
            self.changed.animate_to(0.);
        }
        self.typing = Some(typing);
        cx.notify();
    }

    fn switch(&self) {
        let Some(typing) = self.typing else {
            return;
        };
        let Some(next) = typing.language.next_of(&InputLanguage::installed()) else {
            return;
        };
        if let Err(error) = typing.switch_to(next) {
            report!("the input language was not switched: {error:#}");
        }
    }
}

impl Render for Keyboard {
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
        let code = self
            .typing
            .map_or_else(|| "--".to_owned(), |typing| typing.language.code());
        let button = pill(&theme)
            .id("keyboard")
            .bg(state_layer(&theme, ground, state))
            .on_click(cx.listener(|this, _, _, _| this.switch()))
            .child(icon(&theme, ICON_KEYBOARD))
            .child(div().font_weight(FontWeight::MEDIUM).child(code));
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}
