// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `workspaces` widget: a button for each workspace, the one shown
//! standing out; a click goes to that workspace, and a click with the
//! other button sends the window in front there.

use std::time::Duration;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, StatefulInteractiveElement, Styled, WeakEntity, Window,
    div,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::VirtualDesktops;
use crate::theme::Theme;
use crate::ui::state_layer;
use crate::workspaces::Workspaces as Kept;

/// How often the workspaces are looked at; Windows announces no change
/// of its desktops.
const CHECK_INTERVAL: Duration = Duration::from_millis(300);

/// The height of a button, as a share of the height of a pill.
const BUTTON_SHARE: f32 = 0.66;

/// How much wider than high the button of the shown desktop is.
const SHOWN_STRETCH: f32 = 0.8;

pub struct WorkspacesModule;

impl Module for WorkspacesModule {
    fn name(&self) -> &'static str {
        "workspaces"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("workspaces", |_, cx| cx.new(Workspaces::new).into())
    }
}

/// The button of one desktop and the motion it is in.
struct Workspace {
    /// 0 while absent, 1 when fully there.
    presence: Spring,
    /// 1 while this is the desktop shown.
    shown: Spring,
    interaction: Interaction,
}

struct Workspaces {
    desktops: VirtualDesktops,
    /// One for each desktop, and for those that are still going away.
    buttons: Vec<Workspace>,
}

impl Workspaces {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                if this.update(cx, Self::look).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut this = Self {
            desktops: Kept::now(cx),
            buttons: Vec::new(),
        };
        this.follow(cx);
        // What is there at the start does not arrive.
        for (index, button) in this.buttons.iter_mut().enumerate() {
            button.presence.jump_to(1.);
            let shown = index == this.desktops.current;
            button.shown.jump_to(if shown { 1. } else { 0. });
        }
        this
    }

    fn look(&mut self, cx: &mut Context<Self>) {
        let desktops = Kept::now(cx);
        if desktops != self.desktops {
            self.desktops = desktops;
            self.follow(cx);
            cx.notify();
        }
    }

    /// Sends the buttons where the desktops are: new ones come, the ones
    /// over the count go, and the shown one stands out.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let motion = cx.global::<Theme>().motion;
        while self.buttons.len() < self.desktops.count {
            self.buttons.push(Workspace {
                presence: Spring::new(motion.spatial, 0.),
                shown: Spring::new(motion.spatial, 0.),
                interaction: Interaction::new(&motion),
            });
        }
        for (index, button) in self.buttons.iter_mut().enumerate() {
            let there = index < self.desktops.count;
            let shown = there && index == self.desktops.current;
            button.presence.animate_to(if there { 1. } else { 0. });
            button.shown.animate_to(if shown { 1. } else { 0. });
        }
    }

    fn render_button(
        &mut self,
        index: usize,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = &mut self.buttons[index];
        let presence = button.presence.value(window);
        let shown = button.shown.value(window);
        let state = button.interaction.state(window);
        let colors = theme.colors;
        let height = theme.bar.pill_height * BUTTON_SHARE;
        let width = height * (1. + SHOWN_STRETCH * shown.max(0.));
        let lit = shown.clamp(0., 1.);
        let ground = mix(
            colors.surface_container_highest,
            colors.secondary_container,
            lit,
        );
        let face = div()
            .id(("workspace", index))
            .w(width)
            .h(height)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(&theme, ground, state))
            .text_size(theme.typography.label_size)
            .text_color(mix(colors.on_surface_variant, colors.on_surface, lit))
            .font_weight(FontWeight::MEDIUM)
            .on_click(cx.listener(move |this, _, _, cx| {
                Kept::show(index, cx);
                this.look(cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |_, _, _, cx| Kept::send_front(index, cx)),
            )
            .child((index + 1).to_string());
        let face = Interaction::track(face, cx, move |this: &mut Self| {
            Some(&mut this.buttons.get_mut(index)?.interaction)
        });
        div()
            .w((width + theme.spacing.tiny) * presence.max(0.))
            .h(height)
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .opacity(presence.clamp(0., 1.))
            .child(face)
            .into_any_element()
    }
}

impl Render for Workspaces {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let buttons: Vec<AnyElement> = (0..self.buttons.len())
            .map(|index| self.render_button(index, theme, window, cx))
            .collect();
        // A button over the count that has finished going was just drawn
        // at no width.
        let count = self.desktops.count;
        while self.buttons.len() > count
            && self
                .buttons
                .last()
                .is_some_and(|button| button.presence.is_settled())
        {
            self.buttons.pop();
        }
        div()
            .h(theme.bar.pill_height)
            .px(theme.spacing.small - theme.spacing.tiny / 2.)
            .flex()
            .items_center()
            .rounded(theme.shape.full)
            .bg(theme.colors.surface_container)
            .children(buttons)
    }
}
