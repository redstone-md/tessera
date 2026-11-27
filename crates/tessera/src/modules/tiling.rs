// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `tiling` widget: says whether windows are tiled and in which layout
//! on the workspace its display shows, and goes through the layouts there
//! when clicked.

use gpui::{
    AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{DisplayKey, NativeSurface};
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

/// Shows the mode of tiling of its display; a click goes on to the next
/// one there.
struct TilingButton {
    interaction: Interaction,
    /// 1 while windows are tiled.
    tiled: Spring,
}

/// The display the window of the widget is on.
fn display_of(window: &Window) -> Option<DisplayKey> {
    NativeSurface::of(window).ok().map(NativeSurface::display)
}

impl Render for TilingButton {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let enabled = cx.global::<TilingMode>().enabled;
        self.tiled.animate_to(if enabled { 1. } else { 0. });
        let tiled = self.tiled.value(window).clamp(0., 1.);
        let state = self.interaction.state(window);
        let colors = theme.colors;
        let ground = mix(
            theme.on_bar(colors.surface_container),
            colors.secondary_container,
            tiled,
        );
        let glyph = if enabled { ICON_TILES } else { ICON_FLOATING };
        let label = Tiler::label_on(display_of(window), cx);
        let button = pill(&theme)
            .id("tiling")
            .bg(state_layer(&theme, ground, state))
            .text_color(mix(colors.on_surface_variant, colors.on_surface, tiled))
            .on_click(|_, window, cx| Tiler::next_on(display_of(window), cx))
            .child(icon(&theme, glyph))
            .child(label);
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}
