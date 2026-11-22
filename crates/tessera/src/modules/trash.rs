// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `trash` widget: how many items lie in the Recycle Bin, and a click
//! that opens it.

use std::time::Duration;

use gpui::{
    AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, WeakEntity, Window, div,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::RecycleBin;
use crate::theme::Theme;
use crate::ui::{icon, pill, state_layer};

const ICON_BIN: &str = "\u{e74d}";

/// How often the bin is looked into: Windows does not announce a change
/// to a program that shows no folder.
const CHECK_INTERVAL: Duration = Duration::from_secs(3);

pub struct TrashModule;

impl Module for TrashModule {
    fn name(&self) -> &'static str {
        "trash"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("trash", |_, cx| cx.new(Trash::new).into())
    }
}

/// Shows the number of items that was last counted.
struct Trash {
    /// `None` until Windows has told.
    items: Option<u64>,
    interaction: Interaction,
    /// 1 at the moment the number changes, then back to 0.
    changed: Spring,
}

impl Trash {
    fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let items = cx
                    .background_executor()
                    .spawn(async { RecycleBin::items() });
                let items = items.await;
                if this.update(cx, |this, cx| this.take(items, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(CHECK_INTERVAL).await;
            }
        })
        .detach();
        Self {
            items: None,
            interaction: Interaction::new(&motion),
            changed: Spring::new(motion.effects, 0.),
        }
    }

    /// Takes in what was counted; a count that failed keeps the last one.
    fn take(&mut self, items: Option<u64>, cx: &mut Context<Self>) {
        if items.is_none() || items == self.items {
            return;
        }
        // The first count is no change.
        if self.items.is_some() {
            self.changed.jump_to(1.);
            self.changed.animate_to(0.);
        }
        self.items = items;
        cx.notify();
    }
}

/// What the widget says next to its icon: the number of items, and
/// nothing for an empty bin or one that was not counted.
fn label(items: Option<u64>) -> Option<String> {
    items
        .filter(|items| *items > 0)
        .map(|items| items.to_string())
}

impl Render for Trash {
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
        let label = label(self.items);
        let ink = match label {
            Some(_) => colors.on_surface,
            None => colors.on_surface_variant,
        };
        let button = pill(&theme)
            .id("trash")
            .bg(state_layer(&theme, ground, state))
            .text_color(ink)
            .on_click(|_, _, _| {
                if let Err(error) = RecycleBin::open() {
                    report!("the Recycle Bin was not opened: {error:#}");
                }
            })
            .child(icon(&theme, ICON_BIN))
            .children(label.map(|label| div().font_weight(FontWeight::MEDIUM).child(label)));
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_bin_shows_no_number() {
        assert_eq!(label(None), None);
        assert_eq!(label(Some(0)), None);
        assert_eq!(label(Some(12)).as_deref(), Some("12"));
    }
}
