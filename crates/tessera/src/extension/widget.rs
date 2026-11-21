// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Draws the content an extension describes, with the shell's theme.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, AsyncApp, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div,
};

use super::runtime::{Instance, Node, NodeKind};
use crate::motion::Interaction;
use crate::theme::Theme;
use crate::ui::{pill, state_layer};

/// One placed instance of a widget that an extension provides.
pub struct ExtensionWidget {
    instance: Rc<RefCell<Instance>>,
    /// The widget's name inside its extension.
    widget: String,
    /// The name layouts use, for messages.
    id: String,
    /// The last content the extension returned; `None` once a call failed.
    content: Option<Vec<Node>>,
    /// Pointer feedback of the buttons, by node index.
    buttons: HashMap<usize, Interaction>,
}

impl ExtensionWidget {
    pub fn new(
        instance: Rc<RefCell<Instance>>,
        widget: String,
        id: String,
        refresh: Option<Duration>,
        cx: &mut Context<Self>,
    ) -> Self {
        if let Some(interval) = refresh {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor().timer(interval).await;
                    let alive = this.update(cx, |this, cx| {
                        this.refresh();
                        cx.notify();
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        let mut widget = Self {
            instance,
            widget,
            id,
            content: Some(Vec::new()),
            buttons: HashMap::new(),
        };
        widget.refresh();
        widget
    }

    /// Asks the extension for its content, unless it has failed before.
    fn refresh(&mut self) {
        if self.content.is_none() {
            return;
        }
        let content = self.instance.borrow_mut().render(&self.widget);
        self.content = self.checked(content);
    }

    fn activate(&mut self, action: &str) {
        if self.content.is_none() {
            return;
        }
        let outcome = self.instance.borrow_mut().activate(&self.widget, action);
        if self.checked(outcome).is_none() {
            self.content = None;
        }
        self.refresh();
    }

    /// Reports a failed call into the extension.
    fn checked<T>(&self, outcome: anyhow::Result<T>) -> Option<T> {
        outcome
            .inspect_err(|error| eprintln!("tessera: widget `{}` stopped: {error:#}", self.id))
            .ok()
    }

    fn element(
        &mut self,
        nodes: &[Node],
        index: usize,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let node = nodes.get(index)?;
        Some(match &node.kind {
            NodeKind::Text(text) => div().child(text.clone()).into_any_element(),
            NodeKind::Button(button) => {
                let action = button.action.clone();
                let state = self
                    .buttons
                    .entry(index)
                    .or_insert_with(|| Interaction::new(&theme.motion))
                    .state(window);
                let element = div()
                    .id(("extension-button", index))
                    .px(theme.spacing.small)
                    .rounded(theme.shape.full)
                    .bg(state_layer(theme, theme.colors.surface_container, state))
                    .text_color(theme.colors.primary)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate(&action);
                        cx.notify();
                    }))
                    .child(button.label.clone());
                Interaction::track(element, cx, move |this: &mut Self| {
                    this.buttons.get_mut(&index)
                })
                .into_any_element()
            }
            NodeKind::Row => {
                // Children must come later in the list, which rules out cycles.
                let children: Vec<AnyElement> = node
                    .children
                    .iter()
                    .map(|&child| child as usize)
                    .filter(|&child| child > index)
                    .filter_map(|child| self.element(nodes, child, theme, window, cx))
                    .collect();
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .children(children)
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
                .child(format!("{} stopped working", self.id));
        };
        pill(&theme).children(self.element(&nodes, 0, &theme, window, cx))
    }
}
