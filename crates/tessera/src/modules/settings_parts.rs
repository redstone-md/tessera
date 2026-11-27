// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The parts the settings popups of power and of the displays are made
//! of: the pointer feedback of their controls, their rows of chips and
//! their quieter lines of text.

use std::collections::HashMap;

use gpui::{Context, Div, ParentElement, SharedString, Stateful, Styled, Window, div};

use crate::motion::{Interaction, InteractionState};
use crate::theme::{Motion, Theme};

/// The pointer feedback of the controls of a view, each under a key of
/// its own, made as they are first drawn.
pub(super) struct Feedback {
    motion: Motion,
    controls: HashMap<(&'static str, usize), Interaction>,
}

impl Feedback {
    pub(super) fn new(motion: Motion) -> Self {
        Self {
            motion,
            controls: HashMap::new(),
        }
    }

    /// The feedback of the control `key` now.
    pub(super) fn state(
        &mut self,
        key: (&'static str, usize),
        window: &Window,
    ) -> InteractionState {
        let motion = self.motion;
        { self.controls.entry(key) }
            .or_insert_with(|| Interaction::new(&motion))
            .state(window)
    }

    /// `element` made to show the pointer on it as the control `key` of
    /// the view `V`, whose feedback `find` gives.
    pub(super) fn track<V: 'static>(
        element: Stateful<Div>,
        key: (&'static str, usize),
        cx: &mut Context<V>,
        find: impl Fn(&mut V) -> &mut Self + Clone + 'static,
    ) -> Stateful<Div> {
        Interaction::track(element, cx, move |view: &mut V| {
            find(view).controls.get_mut(&key)
        })
    }
}

/// A row of chips that wraps onto more lines where they do not fit.
pub(super) fn chip_row(theme: &Theme) -> Div {
    div()
        .flex_none()
        .flex()
        .flex_wrap()
        .gap(theme.spacing.small)
        .px(theme.spacing.small)
}

/// A line of text in the quieter color.
pub(super) fn note_line(theme: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px(theme.spacing.small)
        .text_color(theme.colors.on_surface_variant)
        .child(text.into())
}
