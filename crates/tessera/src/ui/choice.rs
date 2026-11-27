// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Lists to choose one entry of with the keyboard as well as the pointer,
//! as the menu of Win+X, the dialog to shut down and the choice of a
//! display mode are: the arrows move the choice, Enter takes it, Esc
//! leaves, and a letter takes the entry it stands for.

use gpui::{
    App, Context, Div, FocusHandle, FontWeight, KeyDownEvent, ParentElement, Pixels, SharedString,
    Stateful, Styled, Window, div, px,
};

use super::{icon, one_line, state_layer};
use crate::motion::{Interaction, InteractionState};
use crate::theme::Theme;

/// How high a row of such a list is.
pub const CHOICE_HEIGHT: Pixels = px(40.);

/// The side of the key that stands at the end of a row for its letter.
const KEY_SIDE: Pixels = px(20.);

/// What a key does to a list of choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The choice moves by this many entries.
    Move(isize),
    /// The choice goes to the first entry, or the last.
    First,
    Last,
    /// The chosen entry is taken.
    Take,
    /// The list is left without taking anything.
    Leave,
    /// The entry this letter or digit stands for is taken.
    Letter(char),
}

impl Pick {
    /// What the key of `event` does; `None` for a key that does nothing
    /// here, or one pressed with Ctrl, Alt or the Windows key.
    pub fn of(event: &KeyDownEvent) -> Option<Self> {
        let held = &event.keystroke.modifiers;
        if held.control || held.alt || held.platform {
            return None;
        }
        Self::named(&event.keystroke.key, held.shift)
    }

    /// What the key GPUI calls `key` does, with Shift held if `shift`.
    fn named(key: &str, shift: bool) -> Option<Self> {
        Some(match key {
            "up" => Self::Move(-1),
            "down" => Self::Move(1),
            "tab" if shift => Self::Move(-1),
            "tab" => Self::Move(1),
            "home" | "pageup" => Self::First,
            "end" | "pagedown" => Self::Last,
            "enter" | "space" => Self::Take,
            "escape" => Self::Leave,
            _ => {
                let mut letters = key.chars();
                match (letters.next(), letters.next()) {
                    (Some(letter), None) if letter.is_ascii_alphanumeric() => {
                        Self::Letter(letter.to_ascii_lowercase())
                    }
                    _ => return None,
                }
            }
        })
    }
}

/// The entry chosen after `pick` moved the choice from `chosen` among
/// `count` entries; it goes round at both ends. Other picks leave it.
pub fn moved(chosen: usize, count: usize, pick: Pick) -> usize {
    if count == 0 {
        return 0;
    }
    let chosen = chosen.min(count - 1);
    match pick {
        Pick::Move(by) => (chosen as isize + by).rem_euclid(count as isize) as usize,
        Pick::First => 0,
        Pick::Last => count - 1,
        Pick::Take | Pick::Leave | Pick::Letter(_) => chosen,
    }
}

/// The choice of a list in a view: which entry is chosen, the pointer
/// feedback of each row, and the keyboard focus the list takes once it is
/// shown.
pub struct Chooser {
    pub chosen: usize,
    rows: Vec<Interaction>,
    focus: FocusHandle,
    focused: bool,
}

impl Chooser {
    pub fn new(cx: &mut App) -> Self {
        Self {
            chosen: 0,
            rows: Vec::new(),
            focus: cx.focus_handle(),
            focused: false,
        }
    }

    /// The handle the list is to track, so that its keys reach it.
    pub fn focus(&self) -> &FocusHandle {
        &self.focus
    }

    /// Takes the keyboard the first time the list is drawn.
    pub fn take_focus(&mut self, window: &mut Window) {
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus);
        }
    }

    /// Moves the choice as `pick` says among `count` entries.
    pub fn pick(&mut self, pick: Pick, count: usize) {
        self.chosen = moved(self.chosen, count, pick);
    }

    /// The pointer feedback of the row `index`, which is made where it is
    /// not there yet.
    pub fn state(&mut self, index: usize, theme: &Theme, window: &Window) -> InteractionState {
        while self.rows.len() <= index {
            self.rows.push(Interaction::new(&theme.motion));
        }
        self.rows[index].state(window)
    }

    /// The feedback of the row `index`, for [`Interaction::track`].
    pub fn row(&mut self, index: usize) -> Option<&mut Interaction> {
        self.rows.get_mut(index)
    }

    /// `row` made to show the pointer on it, tracked in the view `V`
    /// whose chooser `find` gives.
    pub fn track<V: 'static>(
        row: Stateful<Div>,
        index: usize,
        cx: &mut Context<V>,
        find: impl Fn(&mut V) -> &mut Self + Clone + 'static,
    ) -> Stateful<Div> {
        Interaction::track(row, cx, move |view: &mut V| find(view).row(index))
    }
}

/// A row of a list of choices: an icon, its label and, at the end, the
/// letter that takes it, where it has one. The chosen row stands out.
pub fn choice_row(
    theme: &Theme,
    glyph: &'static str,
    label: impl Into<SharedString>,
    letter: Option<char>,
    chosen: bool,
    state: InteractionState,
) -> Div {
    let (ground, mark) = if chosen {
        (theme.colors.secondary_container, theme.colors.primary)
    } else {
        (
            theme.colors.surface_container,
            theme.colors.on_surface_variant,
        )
    };
    let key = letter.map(|letter| {
        div()
            .flex_none()
            .size(KEY_SIDE)
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.small)
            .border_1()
            .border_color(theme.colors.outline)
            .text_color(theme.colors.on_surface_variant)
            .font_weight(FontWeight::MEDIUM)
            .child(SharedString::from(letter.to_ascii_uppercase().to_string()))
    });
    div()
        .h(CHOICE_HEIGHT)
        .flex_none()
        .px(theme.spacing.medium)
        .flex()
        .items_center()
        .gap(theme.spacing.medium)
        .rounded(theme.shape.medium)
        .bg(state_layer(theme, ground, state))
        .text_color(theme.colors.on_surface)
        .child(
            icon(theme, glyph)
                .flex_none()
                .text_size(theme.typography.label_size)
                .text_color(mark),
        )
        .child(div().flex_1().min_w_0().child(one_line(label)))
        .children(key)
}

/// A button under a list of choices, such as OK or Cancel; `prominent`
/// for the one Enter presses.
pub fn choice_button(
    theme: &Theme,
    label: impl Into<SharedString>,
    prominent: bool,
    state: InteractionState,
) -> Div {
    let (ground, text) = if prominent {
        (theme.colors.primary, theme.colors.surface_container)
    } else {
        (
            theme.colors.surface_container_highest,
            theme.colors.on_surface,
        )
    };
    div()
        .flex_none()
        .h(CHOICE_HEIGHT)
        .min_w(px(96.))
        .px(theme.spacing.medium)
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.shape.full)
        .bg(state_layer(theme, ground, state))
        .text_color(text)
        .font_weight(FontWeight::MEDIUM)
        .child(one_line(label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_read_as_what_they_do_to_a_choice() {
        assert_eq!(Pick::named("down", false), Some(Pick::Move(1)));
        assert_eq!(Pick::named("tab", true), Some(Pick::Move(-1)));
        assert_eq!(Pick::named("enter", false), Some(Pick::Take));
        assert_eq!(Pick::named("escape", false), Some(Pick::Leave));
        assert_eq!(Pick::named("u", false), Some(Pick::Letter('u')));
        assert_eq!(Pick::named("7", false), Some(Pick::Letter('7')));
        assert_eq!(Pick::named("f4", false), None);
        assert_eq!(Pick::named("ü", false), None);
    }

    #[test]
    fn the_choice_goes_round_at_both_ends() {
        assert_eq!(moved(0, 4, Pick::Move(-1)), 3);
        assert_eq!(moved(3, 4, Pick::Move(1)), 0);
        assert_eq!(moved(1, 4, Pick::Move(1)), 2);
        assert_eq!(moved(2, 4, Pick::First), 0);
        assert_eq!(moved(0, 4, Pick::Last), 3);
        assert_eq!(moved(9, 4, Pick::Take), 3);
        assert_eq!(moved(0, 0, Pick::Move(1)), 0);
    }
}
