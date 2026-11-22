// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `hdr` widget: whether the display of its bar shows high dynamic
//! range; a click switches it. A display that is not capable of it has no
//! such widget to see.

use std::time::Duration;

use gpui::{
    AppContext, AsyncApp, Context, Empty, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, WeakEntity, Window,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{DisplayKey, Hdr as Range};
use crate::theme::Theme;
use crate::ui::{one_line, pill, state_layer};

/// How often the display is asked: the settings of Windows and a key
/// combination of its own switch it too, and Windows tells no one.
const CHECK_INTERVAL: Duration = Duration::from_secs(5);

pub struct HdrModule;

impl Module for HdrModule {
    fn name(&self) -> &'static str {
        "hdr"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("hdr", |context, cx| {
            let display = context.display;
            cx.new(|cx| Hdr::new(display, cx)).into()
        })
    }
}

/// Shows what the display last said.
struct Hdr {
    display: Option<DisplayKey>,
    range: Option<Range>,
    interaction: Interaction,
    /// 1 while the display shows high dynamic range, 0 while not.
    lit: Spring,
}

impl Hdr {
    fn new(display: Option<DisplayKey>, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let range = cx
                    .background_executor()
                    .spawn(async move { Range::of(display) });
                let range = range.await;
                if this.update(cx, |this, cx| this.take(range, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(CHECK_INTERVAL).await;
            }
        })
        .detach();
        Self {
            display,
            range: None,
            interaction: Interaction::new(&motion),
            lit: Spring::new(motion.effects, 0.),
        }
    }

    /// Takes in what the display said.
    fn take(&mut self, range: Option<Range>, cx: &mut Context<Self>) {
        if range == self.range {
            return;
        }
        let lit = if range.is_some_and(|range| range.on) {
            1.
        } else {
            0.
        };
        // What is said first is no change to show.
        match self.range {
            Some(_) => self.lit.animate_to(lit),
            None => self.lit.jump_to(lit),
        }
        self.range = range;
        cx.notify();
    }

    /// Switches the high dynamic range and asks the display again.
    fn switch(&mut self, cx: &mut Context<Self>) {
        let Some(range) = self.range else {
            return;
        };
        if let Err(error) = range.set(!range.on) {
            report!("the display did not switch its high dynamic range: {error}");
            return;
        }
        self.take(Range::of(self.display), cx);
    }
}

impl Render for Hdr {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.range.is_none() {
            return Empty.into_any_element();
        }
        let theme = *cx.global::<Theme>();
        let colors = theme.colors;
        let lit = self.lit.value(window).clamp(0., 1.);
        let state = self.interaction.state(window);
        let ground = mix(
            theme.on_bar(colors.surface_container),
            colors.secondary_container,
            lit,
        );
        let button = pill(&theme)
            .id("hdr")
            .bg(state_layer(&theme, ground, state))
            .text_color(mix(colors.on_surface_variant, colors.on_surface, lit))
            .on_click(cx.listener(|this, _, _, cx| this.switch(cx)))
            .child(one_line("HDR"));
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
            .into_any_element()
    }
}
