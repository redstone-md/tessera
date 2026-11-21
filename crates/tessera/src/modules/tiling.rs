// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `tiling` widget: says whether and how windows are tiled, and goes
//! through the layouts when clicked.

use gpui::{
    AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::theme::Theme;
use crate::tiling::{Tiler, TilingMode};
use crate::ui::{icon, pill, state_layer};

const ICON_TILES: &str = "\u{f0e2}";
const ICON_FLOATING: &str = "\u{e737}";

pub struct TilingModule;

impl Module for TilingModule {
    fn name(&self) -> &'static str {
        "tiling"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("tiling", |_, cx| {
            let motion = cx.global::<Theme>().motion;
            let tiled = cx.global::<TilingMode>().enabled;
            cx.new(|cx| {
                cx.observe_global::<TilingMode>(|_, cx| cx.notify())
                    .detach();
                TilingButton {
                    interaction: Interaction::new(&motion),
                    tiled: Spring::new(motion.effects, if tiled { 1. } else { 0. }),
                }
            })
            .into()
        })
    }
}

/// Shows the mode of tiling; a click goes on to the next one.
struct TilingButton {
    interaction: Interaction,
    /// 1 while windows are tiled.
    tiled: Spring,
}

impl Render for TilingButton {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let mode = *cx.global::<TilingMode>();
        self.tiled.animate_to(if mode.enabled { 1. } else { 0. });
        let tiled = self.tiled.value(window).clamp(0., 1.);
        let state = self.interaction.state(window);
        let colors = theme.colors;
        let ground = mix(colors.surface_container, colors.secondary_container, tiled);
        let (glyph, label) = match mode.enabled {
            true => (ICON_TILES, mode.layout.name()),
            false => (ICON_FLOATING, "Floating"),
        };
        let button = pill(&theme)
            .id("tiling")
            .bg(state_layer(&theme, ground, state))
            .text_color(mix(colors.on_surface_variant, colors.on_surface, tiled))
            .on_click(|_, _, cx| {
                let next = cx.global::<TilingMode>().next();
                Tiler::set_mode(next, cx);
            })
            .child(icon(&theme, glyph))
            .child(label);
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}
