// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Draws the content an extension describes, with the shell's theme.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    AnyElement, AsyncApp, Context, Div, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, WeakEntity, Window, div, relative,
};

use super::host::Host;
use super::runtime::{Node, NodeKind};
use crate::motion::{Interaction, Spring};
use crate::theme::Theme;
use crate::ui::{pill, state_layer};

/// How a node is laid out, which depends on what contains it.
#[derive(Clone, Copy, PartialEq)]
enum Placement {
    /// The first node: it is the whole widget.
    Root,
    InRow,
    InColumn,
}

/// One placed instance of a widget that an extension provides.
pub struct ExtensionWidget {
    host: Entity<Host>,
    /// The widget's name inside its extension.
    widget: String,
    /// The name layouts use, for messages.
    id: String,
    /// The last content the extension returned; `None` once it has failed.
    content: Option<Vec<Node>>,
    /// Pointer feedback of the buttons, by node index.
    buttons: HashMap<usize, Interaction>,
    /// The shown fill of the progress bars, by node index.
    progress: HashMap<usize, Spring>,
}

impl ExtensionWidget {
    pub fn new(
        host: Entity<Host>,
        widget: String,
        id: String,
        refresh: Option<Duration>,
        cx: &mut Context<Self>,
    ) -> Self {
        if let Some(interval) = refresh {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor().timer(interval).await;
                    if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.observe(&host, |this, _, cx| this.refresh(cx)).detach();
        let mut widget = Self {
            host,
            widget,
            id,
            content: None,
            buttons: HashMap::new(),
            progress: HashMap::new(),
        };
        widget.refresh(cx);
        widget
    }

    /// Asks the extension for the content again.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let widget = &self.widget;
        self.content = self.host.update(cx, |host, cx| host.render(widget, cx));
        cx.notify();
    }

    fn children(
        &mut self,
        nodes: &[Node],
        index: usize,
        placement: Placement,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // Children must come later in the list, which rules out cycles.
        nodes[index]
            .children
            .iter()
            .map(|&child| child as usize)
            .filter(|&child| child > index)
            .filter_map(|child| self.element(nodes, child, placement, theme, window, cx))
            .collect()
    }

    /// A text. In a row it stays on one line and keeps its width, so that
    /// a full bar does not break it into lines; in a column it wraps.
    fn words(text: &str, placement: Placement) -> Div {
        let words = div().child(text.to_owned());
        match placement {
            Placement::Root | Placement::InRow => words.flex_none().whitespace_nowrap(),
            Placement::InColumn => words,
        }
    }

    fn element(
        &mut self,
        nodes: &[Node],
        index: usize,
        placement: Placement,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let node = nodes.get(index)?;
        Some(match &node.kind {
            NodeKind::Text(text) => Self::words(text, placement).into_any_element(),
            NodeKind::Caption(text) => Self::words(text, placement)
                .text_color(theme.colors.on_surface_variant)
                .into_any_element(),
            NodeKind::Row => {
                let row = match placement {
                    Placement::Root => pill(theme).flex_none(),
                    Placement::InRow => div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(theme.spacing.small),
                    Placement::InColumn => div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(theme.spacing.small),
                };
                row.children(self.children(nodes, index, Placement::InRow, theme, window, cx))
                    .into_any_element()
            }
            NodeKind::Column => {
                let children = self.children(nodes, index, Placement::InColumn, theme, window, cx);
                let column = div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.small)
                    .children(children);
                match placement {
                    // The whole widget: what does not fit is scrolled to.
                    Placement::Root => column
                        .id("extension-content")
                        .size_full()
                        .overflow_y_scroll()
                        .into_any_element(),
                    Placement::InRow | Placement::InColumn => column.into_any_element(),
                }
            }
            NodeKind::Progress(share) => {
                let share = if share.is_nan() {
                    0.
                } else {
                    share.clamp(0., 1.)
                };
                let shown = self
                    .progress
                    .entry(index)
                    .or_insert_with(|| Spring::new(theme.motion.effects, 0.));
                shown.animate_to(share);
                let shown = shown.value(window).clamp(0., 1.);
                let track = div()
                    .h(theme.spacing.small)
                    .rounded(theme.shape.full)
                    .overflow_hidden()
                    .bg(theme.colors.surface_container_highest)
                    .child(
                        div()
                            .h_full()
                            .w(relative(shown))
                            .rounded(theme.shape.full)
                            .bg(theme.colors.primary),
                    );
                match placement {
                    Placement::InRow => track.w(theme.bar.pill_height * 1.5),
                    Placement::Root | Placement::InColumn => track.w_full(),
                }
                .into_any_element()
            }
            NodeKind::Button(button) => {
                let action = button.action.clone();
                let state = self
                    .buttons
                    .entry(index)
                    .or_insert_with(|| Interaction::new(&theme.motion))
                    .state(window);
                let element = div()
                    .id(("extension-button", index))
                    .flex_none()
                    .whitespace_nowrap()
                    .px(theme.spacing.small)
                    .rounded(theme.shape.full)
                    .bg(state_layer(theme, theme.colors.surface_container, state))
                    .text_color(theme.colors.primary)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let widget = &this.widget;
                        this.host
                            .update(cx, |host, cx| host.activate(widget, &action, cx));
                    }))
                    .child(button.label.clone());
                Interaction::track(element, cx, move |this: &mut Self| {
                    this.buttons.get_mut(&index)
                })
                .into_any_element()
            }
        })
    }
}

impl Render for ExtensionWidget {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let Some(nodes) = self.content.clone() else {
            return pill(&theme)
                .text_color(theme.colors.on_surface_variant)
                .child(format!("{} stopped working", self.id))
                .into_any_element();
        };
        self.element(&nodes, 0, Placement::Root, &theme, window, cx)
            .unwrap_or_else(|| div().into_any_element())
    }
}
