// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A field text is typed into: one line, with the caret always at its end.
//! Whoever shows one observes it to hear of what is typed, and handles the
//! keys that are no text, which pass on to the elements around the field.

use std::ops::Range;

use gpui::{
    App, Bounds, Context, ElementInputHandler, EntityInputHandler, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Pixels, Point, Render, SharedString, Styled,
    UTF16Selection, Window, canvas, div, px,
};

use crate::theme::Theme;
use crate::ui::icon;

/// What stands for each letter of a field that hides what is typed.
const HIDDEN_LETTER: &str = "\u{2022}";

/// A field of one line.
pub struct TextField {
    text: String,
    /// Text an input method is still composing; it follows the text.
    composing: String,
    /// What the empty field says.
    placeholder: SharedString,
    /// The icon the field begins with, of the Segoe Fluent Icons font.
    glyph: Option<&'static str>,
    /// Whether dots stand for what is typed.
    hidden: bool,
    focus: FocusHandle,
}

impl TextField {
    pub fn new(placeholder: SharedString, cx: &mut App) -> Self {
        Self {
            text: String::new(),
            composing: String::new(),
            placeholder,
            glyph: None,
            hidden: false,
            focus: cx.focus_handle(),
        }
    }

    /// The field with `glyph` at its start.
    pub fn with_glyph(mut self, glyph: &'static str) -> Self {
        self.glyph = Some(glyph);
        self
    }

    /// The field with dots for what is typed.
    pub fn hiding(mut self) -> Self {
        self.hidden = true;
        self
    }

    /// What is typed, with what is still being composed.
    pub fn text(&self) -> String {
        format!("{}{}", self.text, self.composing)
    }

    /// Whether dots stand for what is typed.
    pub fn hides(&self) -> bool {
        self.hidden
    }

    /// Shows what is typed, or hides it behind dots.
    pub fn hide(&mut self, hidden: bool, cx: &mut Context<Self>) {
        if self.hidden != hidden {
            self.hidden = hidden;
            cx.notify();
        }
    }

    /// Empties the field.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set(String::new(), "", cx);
    }

    /// Has the keys go to the field.
    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    fn set(&mut self, text: String, composing: &str, cx: &mut Context<Self>) {
        if self.text == text && self.composing == composing {
            return;
        }
        self.text = text;
        composing.clone_into(&mut self.composing);
        cx.notify();
    }

    /// Handles the one key that changes the text without being text.
    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if event.keystroke.key == "backspace" && self.composing.is_empty() {
            let mut text = self.text.clone();
            text.pop();
            self.set(text, "", cx);
        }
    }
}

/// What a field shows for `text`: the text, or a dot for each letter.
fn shown(text: &str, hidden: bool) -> String {
    if hidden {
        HIDDEN_LETTER.repeat(text.chars().count())
    } else {
        text.to_owned()
    }
}

/// Text input, with the caret always at the end. Positions count UTF-16
/// code units.
impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        // What is hidden is not handed out either.
        if self.hidden {
            return None;
        }
        let units: Vec<u16> = self.text().encode_utf16().collect();
        let end = range.end.min(units.len());
        Some(String::from_utf16_lossy(&units[range.start.min(end)..end]))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.text().encode_utf16().count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let start = self.text.encode_utf16().count();
        let length = self.composing.encode_utf16().count();
        (length > 0).then(|| start..start + length)
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.set(self.text(), "", cx);
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let typed: String = text.chars().filter(|typed| !typed.is_control()).collect();
        self.set(format!("{}{typed}", self.text), "", cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set(self.text.clone(), new_text, cx);
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let typed = self.text();
        let is_empty = typed.is_empty();
        let text = if is_empty {
            div()
                .text_color(theme.colors.on_surface_variant)
                .child(self.placeholder.clone())
        } else {
            div().child(SharedString::new(shown(&typed, self.hidden)))
        };
        // Typed text is delivered to whoever handles input while painting.
        let (focus, this) = (self.focus.clone(), cx.entity());
        let input = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                window.handle_input(&focus, ElementInputHandler::new(bounds, this), cx);
            },
        )
        .absolute()
        .size_full();
        // The caret shows only while the keys go to the field.
        let caret = div()
            .w(px(2.))
            .h(theme.typography.icon_size)
            .rounded(theme.shape.full)
            .bg(theme.colors.primary)
            .opacity(if self.focus.is_focused(window) {
                1.
            } else {
                0.
            });
        let glyph = self
            .glyph
            .map(|glyph| icon(&theme, glyph).text_color(theme.colors.on_surface_variant));
        let field = div()
            .h(theme.launcher.field_height)
            .flex_1()
            .min_w_0()
            .relative()
            .overflow_hidden()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .child(input)
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .rounded(theme.shape.full)
            .bg(theme.colors.surface_container_highest)
            .children(glyph);
        // The caret stands before the placeholder and after typed text.
        if is_empty {
            field.child(caret).child(text)
        } else {
            field.child(text).child(caret)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hidden_text_shows_a_dot_for_each_letter() {
        assert_eq!(shown("päss", true), HIDDEN_LETTER.repeat(4));
        assert_eq!(shown("päss", false), "päss");
        assert_eq!(shown("", true), "");
    }
}
