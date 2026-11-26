// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A field text is typed into: one line with a caret that moves and a
//! selection, by the keys and by the pointer, and the clipboard. Whoever
//! shows one observes it, asks for its text to hear of what is typed, and
//! handles the keys the field has no use for, which pass on to the
//! elements around it.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{
    App, AsyncApp, Bounds, ClipboardItem, ContentMask, Context, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point, Render, ShapedLine, SharedString,
    Styled, Task, TextRun, UTF16Selection, WeakEntity, Window, canvas, div, fill, point, px, size,
};

use crate::theme::Theme;
use crate::ui::icon;

/// What stands for each letter of a field that hides what is typed.
const HIDDEN_LETTER: &str = "\u{2022}";

/// How long the caret shows and then hides while nothing is typed.
const BLINK: Duration = Duration::from_millis(530);

/// How wide the caret is, and how far the placeholder stands from it.
const CARET_WIDTH: Pixels = px(2.);
const PLACEHOLDER_GAP: Pixels = px(6.);

/// How much of the color of the caret what is selected is filled with.
const SELECTION_SHARE: f32 = 0.35;

/// The text of a field, where the caret stands in it and where what is
/// selected begins; both count bytes and stand between letters.
#[derive(Default)]
struct Line {
    text: String,
    caret: usize,
    anchor: usize,
}

impl Line {
    /// What is selected; empty where nothing is.
    fn selected(&self) -> Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    /// Puts `typed` in place of what is selected, the caret after it.
    fn write(&mut self, typed: &str) {
        let selected = self.selected();
        self.caret = selected.start + typed.len();
        self.anchor = self.caret;
        self.text.replace_range(selected, typed);
    }

    /// Moves the caret to `to`; what it passes is selected if `selecting`.
    fn go(&mut self, to: usize, selecting: bool) {
        self.caret = to.min(self.text.len());
        if !selecting {
            self.anchor = self.caret;
        }
    }

    /// Where the caret goes one step back: over a letter, or over a word
    /// with what parts it from the caret.
    fn back(&self, word: bool) -> usize {
        let mut before = self.text[..self.caret].char_indices().rev().peekable();
        let Some((mut at, _)) = before.peek().copied() else {
            return 0;
        };
        if word {
            while before
                .next_if(|(_, letter)| !letter.is_alphanumeric())
                .is_some()
            {}
            at = before
                .peek()
                .map_or(0, |(at, letter)| at + letter.len_utf8());
            while let Some((start, _)) = before.next_if(|(_, letter)| letter.is_alphanumeric()) {
                at = start;
            }
        }
        at
    }

    /// Where the caret goes one step on: over a letter, or over a word
    /// with what parts it from the next.
    fn on(&self, word: bool) -> usize {
        let mut after = self.text[self.caret..].char_indices().peekable();
        let end =
            |at: Option<&(usize, char)>| at.map_or(self.text.len(), |(at, _)| self.caret + at);
        if word {
            while after
                .next_if(|(_, letter)| letter.is_alphanumeric())
                .is_some()
            {}
            while after
                .next_if(|(_, letter)| !letter.is_alphanumeric())
                .is_some()
            {}
        } else {
            after.next();
        }
        end(after.peek())
    }

    /// Takes away what is selected, or else what lies between the caret
    /// and `to`.
    fn erase(&mut self, to: usize) {
        if self.selected().is_empty() {
            self.anchor = to;
        }
        self.write("");
    }

    /// Selects the word `at` stands in or beside.
    fn select_word(&mut self, at: usize) {
        self.go(at, false);
        let start = self.back(true);
        self.go(start, false);
        let end = self.on(true);
        let end = self.text[..end].trim_end().len().max(start);
        self.anchor = start;
        self.caret = end;
    }

    fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }
}

/// A line as it was drawn last: where a press of the pointer lands in it.
struct Drawn {
    line: ShapedLine,
    /// Where the line begins in the window, scrolled as it is.
    left: Pixels,
}

/// A field of one line.
pub struct TextField {
    line: Line,
    /// Text an input method is still composing; it stands at the caret.
    composing: String,
    /// What the empty field says.
    placeholder: SharedString,
    /// The icon the field begins with, of the Segoe Fluent Icons font.
    glyph: Option<&'static str>,
    /// Whether dots stand for what is typed.
    hidden: bool,
    focus: FocusHandle,
    /// Whether the keys went to the field when it was drawn last.
    focused: bool,
    /// When the caret was moved last: it shows from then, and blinks.
    stirred: Instant,
    /// What redraws the caret while it blinks.
    blinking: Option<Task<()>>,
    /// How far the text is moved to the left to keep the caret in sight.
    scrolled: Pixels,
    drawn: Option<Drawn>,
    /// Whether the pointer is selecting, pressed in the field.
    dragging: bool,
}

impl TextField {
    pub fn new(placeholder: SharedString, cx: &mut App) -> Self {
        Self {
            line: Line::default(),
            composing: String::new(),
            placeholder,
            glyph: None,
            hidden: false,
            focus: cx.focus_handle(),
            focused: false,
            stirred: Instant::now(),
            blinking: None,
            scrolled: px(0.),
            drawn: None,
            dragging: false,
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
        let (before, after) = self.line.text.split_at(self.line.caret);
        format!("{before}{}{after}", self.composing)
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
        self.line = Line::default();
        self.composing.clear();
        self.stir(cx);
    }

    /// Has the keys go to the field.
    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    /// Whether the keys go to the field.
    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// Puts `text` in the field, all of it selected, as a field to be
    /// written over is shown; with `stem`, only the part of it before the
    /// last dot, as a file name to rename.
    pub fn set_text(&mut self, text: &str, stem: bool, cx: &mut Context<Self>) {
        let end = match stem {
            true => text.rfind('.').filter(|&dot| dot > 0).unwrap_or(text.len()),
            false => text.len(),
        };
        self.line = Line {
            text: text.to_owned(),
            caret: end,
            anchor: 0,
        };
        self.composing.clear();
        self.scrolled = px(0.);
        self.stir(cx);
    }

    /// Something of the field changed: it is drawn anew, the caret shown.
    fn stir(&mut self, cx: &mut Context<Self>) {
        self.stirred = Instant::now();
        cx.notify();
    }

    /// Whether the caret is in the part of its blinking that shows.
    fn caret_shows(&self) -> bool {
        self.focused && (self.stirred.elapsed().as_millis() / BLINK.as_millis()).is_multiple_of(2)
    }

    /// Redraws the field as often as the caret blinks, for as long as the
    /// keys go to it.
    fn blink(&mut self, cx: &mut Context<Self>) {
        if self.blinking.is_some() || !self.focused {
            return;
        }
        let blinking = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(BLINK).await;
                let goes_on = this.update(cx, |this, cx| {
                    if !this.focused {
                        this.blinking = None;
                    }
                    cx.notify();
                    this.focused
                });
                if !goes_on.unwrap_or(false) {
                    break;
                }
            }
        });
        self.blinking = Some(blinking);
    }

    /// What is selected, unless the field hides what is typed.
    fn copied(&self) -> Option<String> {
        let selected = self.line.selected();
        (!self.hidden && !selected.is_empty()).then(|| self.line.text[selected].to_owned())
    }

    /// Handles the keys that move the caret or change the text without
    /// being text.
    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if !self.composing.is_empty() {
            return;
        }
        let keys = &event.keystroke.modifiers;
        let (word, selecting) = (keys.control, keys.shift);
        let selected = self.line.selected();
        match event.keystroke.key.as_str() {
            // Without Shift the caret leaves what is selected at its end.
            "left" if !selecting && !selected.is_empty() => self.line.go(selected.start, false),
            "right" if !selecting && !selected.is_empty() => self.line.go(selected.end, false),
            "left" => self.line.go(self.line.back(word), selecting),
            "right" => self.line.go(self.line.on(word), selecting),
            "home" => self.line.go(0, selecting),
            "end" => self.line.go(usize::MAX, selecting),
            "backspace" => self.line.erase(self.line.back(word)),
            "delete" => self.line.erase(self.line.on(word)),
            "a" if keys.control => self.line.select_all(),
            "c" | "x" if keys.control => {
                let Some(copied) = self.copied() else {
                    return;
                };
                cx.write_to_clipboard(ClipboardItem::new_string(copied));
                if event.keystroke.key == "x" {
                    self.line.write("");
                }
            }
            "v" if keys.control => {
                let pasted = cx.read_from_clipboard().and_then(|item| item.text());
                self.line.write(&one_line_of(&pasted.unwrap_or_default()));
            }
            _ => return,
        }
        self.stir(cx);
    }

    /// Where in the text a press at `position` of the window lands.
    fn pointed(&self, position: Point<Pixels>) -> usize {
        let Some(drawn) = &self.drawn else {
            return self.line.text.len();
        };
        let shown = drawn.line.closest_index_for_x(position.x - drawn.left);
        match self.hidden {
            true => {
                let letters = shown / HIDDEN_LETTER.len();
                let mut starts = self.line.text.char_indices().map(|(at, _)| at);
                starts.nth(letters).unwrap_or(self.line.text.len())
            }
            false => shown.min(self.line.text.len()),
        }
    }

    /// A press puts the caret, two select a word and three everything.
    fn pressed(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        if !self.composing.is_empty() {
            return;
        }
        let at = self.pointed(event.position);
        match event.click_count {
            0 | 1 => self.line.go(at, event.modifiers.shift),
            2 => self.line.select_word(at),
            _ => self.line.select_all(),
        }
        self.dragging = event.click_count <= 1;
        self.stir(cx);
    }

    /// The pointer moved while pressed: what it passed is selected.
    fn dragged(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.dragging {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.dragging = false;
            return;
        }
        let at = self.pointed(event.position);
        if at != self.line.caret {
            self.line.go(at, true);
            self.stir(cx);
        }
    }

    /// Where the place `at` of the text is in what the field shows, which
    /// has the composed text at the caret and may be dots.
    fn place(&self, at: usize) -> usize {
        let composed = match at >= self.line.caret {
            true => self.composing.as_str(),
            false => "",
        };
        match self.hidden {
            true => {
                let letters = self.line.text[..at].chars().count() + composed.chars().count();
                letters * HIDDEN_LETTER.len()
            }
            false => at + composed.len(),
        }
    }

    /// Draws the text, what is selected in it and the caret into `bounds`,
    /// and takes the text that is typed from then on.
    fn paint(this: &Entity<Self>, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let theme = *cx.global::<Theme>();
        let field = this.read(cx);
        window.handle_input(
            &field.focus,
            ElementInputHandler::new(bounds, this.clone()),
            cx,
        );
        let typed = field.text();
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let (shown, color) = match typed.is_empty() {
            true => (field.placeholder.clone(), theme.colors.on_surface_variant),
            false => (shown(&typed, field.hidden).into(), style.color),
        };
        let run = TextRun {
            len: shown.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(shown, font_size, &[run], None);
        let selected = field.line.selected();
        let (from, to, caret) = match typed.is_empty() {
            true => (px(0.), px(0.), px(0.)),
            false => (
                line.x_for_index(field.place(selected.start)),
                line.x_for_index(field.place(selected.end)),
                line.x_for_index(field.place(field.line.caret)),
            ),
        };
        // The text moves only as far as the caret needs to stay in sight.
        let room = (bounds.size.width - CARET_WIDTH).max(px(0.));
        let scrolled = match typed.is_empty() || line.width <= room {
            true => px(0.),
            false => field.scrolled.clamp(caret - room, caret).max(px(0.)),
        };
        let caret_shows = field.caret_shows();
        let top = bounds.origin.y + (bounds.size.height - line_height) / 2.;
        let left = bounds.origin.x - scrolled;
        let start = match typed.is_empty() {
            true => left + PLACEHOLDER_GAP,
            false => left,
        };
        let tall = size(CARET_WIDTH, line_height);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if to > from {
                let chosen = Bounds::new(point(left + from, top), size(to - from, line_height));
                window.paint_quad(fill(chosen, theme.colors.primary.opacity(SELECTION_SHARE)));
            }
            let _ = line.paint(point(start, top), line_height, window, cx);
            if caret_shows {
                let caret = Bounds::new(point(left + caret, top), tall);
                window.paint_quad(fill(caret, theme.colors.primary));
            }
        });
        this.update(cx, |field, _| {
            field.scrolled = scrolled;
            field.drawn = (!typed.is_empty()).then_some(Drawn { line, left });
        });
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

/// `text` as one line can hold it: what breaks a line is a space, and
/// nothing else that is no letter stays.
fn one_line_of(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let text = lines.join(" ");
    text.chars().filter(|letter| !letter.is_control()).collect()
}

/// The place in `text` that is `units` UTF-16 code units from its start.
fn place_of(text: &str, units: usize) -> usize {
    let mut passed = 0;
    for (at, letter) in text.char_indices() {
        if passed >= units {
            return at;
        }
        passed += letter.len_utf16();
    }
    text.len()
}

/// Text input. Positions count UTF-16 code units of what is typed with
/// what is being composed.
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
        let units = |to: usize| self.line.text[..to].encode_utf16().count();
        let selected = self.line.selected();
        let composed = self.composing.encode_utf16().count();
        Some(UTF16Selection {
            range: units(selected.start) + composed..units(selected.end) + composed,
            reversed: self.line.caret < self.line.anchor,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let start = self.line.text[..self.line.caret].encode_utf16().count();
        let length = self.composing.encode_utf16().count();
        (length > 0).then(|| start..start + length)
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let composed = std::mem::take(&mut self.composing);
        self.line.write(&composed);
        self.stir(cx);
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A range is of the text with what was being composed, which the
        // new text takes the place of; only one of plain text is kept.
        if let Some(range) = range.filter(|_| self.composing.is_empty()) {
            self.line.anchor = place_of(&self.line.text, range.start);
            self.line.caret = place_of(&self.line.text, range.end);
        }
        self.composing.clear();
        let typed: String = text.chars().filter(|typed| !typed.is_control()).collect();
        if typed.is_empty() && self.line.selected().is_empty() {
            return cx.notify();
        }
        self.line.write(&typed);
        self.stir(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // What is composed takes the place of what was selected.
        self.line.write("");
        new_text.clone_into(&mut self.composing);
        self.stir(cx);
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
        // The caret shows, and blinks, only while the keys go to the field.
        let focused = self.focus.is_focused(window);
        if focused && !self.focused {
            self.stirred = Instant::now();
        }
        self.focused = focused;
        self.blink(cx);
        let this = cx.entity();
        let text = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| Self::paint(&this, bounds, window, cx),
        )
        .flex_1()
        .min_w_0()
        .h_full();
        let glyph = self
            .glyph
            .map(|glyph| icon(&theme, glyph).text_color(theme.colors.on_surface_variant));
        div()
            .h(theme.launcher.field_height)
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .track_focus(&self.focus)
            .cursor_text()
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.pressed(event, window, cx)),
            )
            .on_mouse_move(cx.listener(|this, event, _, cx| this.dragged(event, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .rounded(theme.shape.full)
            .bg(theme.colors.surface_container_highest)
            .children(glyph)
            .child(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, caret: usize) -> Line {
        Line {
            text: text.to_owned(),
            caret,
            anchor: caret,
        }
    }

    #[test]
    fn a_hidden_text_shows_a_dot_for_each_letter() {
        assert_eq!(shown("päss", true), HIDDEN_LETTER.repeat(4));
        assert_eq!(shown("päss", false), "päss");
        assert_eq!(shown("", true), "");
    }

    #[test]
    fn what_is_typed_takes_the_place_of_what_is_selected() {
        let mut typed = line("one two", 7);
        typed.write("!");
        assert_eq!((typed.text.as_str(), typed.caret), ("one two!", 8));
        typed.select_all();
        assert_eq!(typed.selected(), 0..8);
        typed.write("я");
        assert_eq!(
            (typed.text.as_str(), typed.caret, typed.anchor),
            ("я", 2, 2)
        );
    }

    #[test]
    fn the_caret_steps_over_letters_and_over_words() {
        let typed = line("да, two  three", 16);
        assert_eq!(typed.back(false), 15);
        assert_eq!(typed.back(true), 11);
        let typed = line("да, two  three", 11);
        assert_eq!(typed.back(true), 6);
        let typed = line("да, two  three", 0);
        assert_eq!((typed.back(false), typed.back(true)), (0, 0));
        assert_eq!(typed.on(false), 2);
        assert_eq!(typed.on(true), 6);
        let typed = line("да, two  three", 16);
        assert_eq!((typed.on(false), typed.on(true)), (16, 16));
    }

    #[test]
    fn erasing_takes_what_is_selected_or_else_up_to_a_place() {
        let mut typed = line("one two", 7);
        typed.erase(typed.back(true));
        assert_eq!((typed.text.as_str(), typed.caret), ("one ", 4));
        typed.go(0, true);
        typed.erase(typed.back(false));
        assert_eq!((typed.text.as_str(), typed.caret, typed.anchor), ("", 0, 0));
        typed.erase(typed.on(false));
        assert_eq!(typed.text, "");
    }

    #[test]
    fn two_presses_select_the_word_under_them() {
        let mut typed = line("one two  three", 0);
        typed.select_word(5);
        assert_eq!(typed.selected(), 4..7);
        typed.select_word(14);
        assert_eq!(typed.selected(), 9..14);
    }

    #[test]
    fn what_is_pasted_becomes_one_line() {
        assert_eq!(one_line_of("one\r\ntwo\tthree"), "one twothree");
    }

    #[test]
    fn places_are_found_by_their_utf16_units() {
        assert_eq!(place_of("a😀я", 0), 0);
        assert_eq!(place_of("a😀я", 1), 1);
        assert_eq!(place_of("a😀я", 3), 5);
        assert_eq!(place_of("a😀я", 9), 7);
    }
}
