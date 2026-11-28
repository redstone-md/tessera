// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the terminal shows, copied out of the grid of the terminal while
//! it is locked, so that drawing never waits for the thread that parses
//! the output: each cell's letter, colours and look, the cursor and the
//! selection. Only the lines that changed since the last copy are copied
//! again, as the terminal tells them.

use std::ops::Range;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

use super::palette::{Palette, dimmed};

/// How a letter is drawn, beside its colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Look(u8);

impl Look {
    pub const BOLD: Self = Self(1);
    pub const ITALIC: Self = Self(1 << 1);
    pub const UNDERLINE: Self = Self(1 << 2);
    pub const STRIKE: Self = Self(1 << 3);
    /// The letter takes two cells.
    pub const WIDE: Self = Self(1 << 4);
    /// The cell is the second of a wide letter, and draws nothing.
    pub const SPACER: Self = Self(1 << 5);
    pub const CURLY: Self = Self(1 << 6);

    pub fn has(self, look: Self) -> bool {
        self.0 & look.0 == look.0
    }

    fn with(self, look: Self, on: bool) -> Self {
        match on {
            true => Self(self.0 | look.0),
            false => self,
        }
    }

    /// The look without being wide or a spacer, which decide where a
    /// letter stands, not how it is drawn.
    pub fn drawn(self) -> Self {
        Self(self.0 & !(Self::WIDE.0 | Self::SPACER.0))
    }
}

/// One cell as it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub letter: char,
    pub fg: u32,
    pub bg: u32,
    pub look: Look,
}

/// One line as it is drawn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub glyphs: Vec<Glyph>,
    /// The letters of no width drawn over the letter of a column, such as
    /// accents that combine.
    pub marks: Vec<(usize, String)>,
}

/// Where the cursor is drawn, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caret {
    pub line: usize,
    pub column: usize,
    pub shape: CursorShape,
    pub blinking: bool,
    /// Whether it stands on a wide letter, and so is two cells wide.
    pub wide: bool,
}

/// What is selected, from the first cell to the last, by lines of the
/// screen (which may lie above or below it while it is scrolled).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chosen {
    pub start: (i32, usize),
    pub end: (i32, usize),
    pub block: bool,
}

/// What the terminal shows.
#[derive(Clone, Debug, Default)]
pub struct Screen {
    pub columns: usize,
    pub lines: usize,
    pub rows: Vec<Row>,
    pub caret: Option<Caret>,
    pub chosen: Option<Chosen>,
    /// How many lines of the history are scrolled into sight.
    pub scrolled: usize,
    pub mode: TermMode,
    /// The background where cells have none of their own, which is not
    /// drawn cell by cell.
    pub background: u32,
}

impl Screen {
    /// Copies what changed in `term` since the last copy, or all of it
    /// with `everything`, its colours as `palette` gives them.
    pub fn copy_from<T: EventListener>(
        &mut self,
        term: &mut Term<T>,
        palette: &Palette,
        everything: bool,
    ) {
        let (columns, lines) = (term.columns(), term.screen_lines());
        let resized = (columns, lines) != (self.columns, self.lines);
        let changed: Option<Vec<usize>> = match term.damage() {
            TermDamage::Full => None,
            TermDamage::Partial(_) if everything || resized => None,
            TermDamage::Partial(damaged) => Some(damaged.map(|line| line.line).collect()),
        };
        term.reset_damage();
        let colors = *term.colors();
        self.background = color_of(Color::Named(NamedColor::Background), &colors, palette);
        self.columns = columns;
        self.lines = lines;
        self.rows.resize_with(lines, Row::default);
        self.scrolled = term.grid().display_offset();
        let copy = |line: usize, row: &mut Row| {
            let grid_line = Line(line as i32 - self.scrolled as i32);
            let cells = &term.grid()[grid_line];
            row.glyphs.clear();
            row.marks.clear();
            for column in 0..columns {
                let cell = &cells[Column(column)];
                row.glyphs.push(glyph_of(cell, &colors, palette));
                if let Some(marks) = cell.zerowidth() {
                    row.marks.push((column, marks.iter().collect()));
                }
            }
        };
        match changed {
            None => {
                for (line, row) in self.rows.iter_mut().enumerate() {
                    copy(line, row);
                }
            }
            Some(lines) => {
                for line in lines {
                    if let Some(row) = self.rows.get_mut(line) {
                        copy(line, row);
                    }
                }
            }
        }
        let content = term.renderable_content();
        self.mode = content.mode;
        self.chosen = content.selection.map(|range| Chosen {
            start: (
                range.start.line.0 + self.scrolled as i32,
                range.start.column.0,
            ),
            end: (range.end.line.0 + self.scrolled as i32, range.end.column.0),
            block: range.is_block,
        });
        let point = content.cursor.point;
        let line = point.line.0 + self.scrolled as i32;
        let style = term.cursor_style();
        self.caret = (content.cursor.shape != CursorShape::Hidden
            && (0..lines as i32).contains(&line))
        .then(|| {
            let wide = { self.rows.get(line as usize) }
                .and_then(|row| row.glyphs.get(point.column.0))
                .is_some_and(|glyph| glyph.look.has(Look::WIDE));
            Caret {
                line: line as usize,
                column: point.column.0,
                shape: content.cursor.shape,
                blinking: style.blinking,
                wide,
            }
        });
    }

    /// The columns of the line `line` of the screen that are selected.
    pub fn chosen_in(&self, line: usize) -> Option<Range<usize>> {
        chosen_columns(self.chosen?, line as i32, self.columns)
    }

    /// The letters of the line `line`, one per column.
    pub fn letters(&self, line: usize) -> Vec<char> {
        { self.rows.get(line) }
            .map(|row| row.glyphs.iter().map(|glyph| glyph.letter).collect())
            .unwrap_or_default()
    }
}

/// The columns of `line` within `chosen`, on a screen `columns` wide.
fn chosen_columns(chosen: Chosen, line: i32, columns: usize) -> Option<Range<usize>> {
    let (start, end) = (chosen.start, chosen.end);
    if line < start.0 || line > end.0 || columns == 0 {
        return None;
    }
    let range = match chosen.block {
        true => start.1.min(end.1)..start.1.max(end.1) + 1,
        false => {
            let from = if line == start.0 { start.1 } else { 0 };
            let to = if line == end.0 { end.1 + 1 } else { columns };
            from..to
        }
    };
    let range = range.start.min(columns)..range.end.min(columns);
    (!range.is_empty()).then_some(range)
}

/// How `cell` is drawn.
fn glyph_of(cell: &Cell, colors: &Colors, palette: &Palette) -> Glyph {
    let flags = cell.flags;
    let mut fg = cell.fg;
    // Bold text in one of the first eight colours takes its bright one.
    if flags.contains(Flags::BOLD)
        && let Color::Named(named) = fg
        && (named as usize) < 8
    {
        fg = Color::Named(named.to_bright());
    }
    let mut fg = color_of(fg, colors, palette);
    let mut bg = color_of(cell.bg, colors, palette);
    if flags.contains(Flags::DIM) {
        fg = dimmed(fg);
    }
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    let letter = match flags.intersects(Flags::HIDDEN | Flags::WIDE_CHAR_SPACER) {
        true => ' ',
        false => cell.c,
    };
    let underlined = flags.intersects(
        Flags::UNDERLINE
            | Flags::DOUBLE_UNDERLINE
            | Flags::DOTTED_UNDERLINE
            | Flags::DASHED_UNDERLINE,
    );
    let look = Look::default()
        .with(Look::BOLD, flags.contains(Flags::BOLD))
        .with(Look::ITALIC, flags.contains(Flags::ITALIC))
        .with(Look::UNDERLINE, underlined)
        .with(Look::CURLY, flags.contains(Flags::UNDERCURL))
        .with(Look::STRIKE, flags.contains(Flags::STRIKEOUT))
        .with(Look::WIDE, flags.contains(Flags::WIDE_CHAR))
        .with(Look::SPACER, flags.contains(Flags::WIDE_CHAR_SPACER));
    Glyph {
        letter,
        fg,
        bg,
        look,
    }
}

fn rgb(color: Rgb) -> u32 {
    (color.r as u32) << 16 | (color.g as u32) << 8 | color.b as u32
}

/// The colour `color` is, as the programs changed the colours (`colors`)
/// over `palette`.
fn color_of(color: Color, colors: &Colors, palette: &Palette) -> u32 {
    match color {
        Color::Spec(color) => rgb(color),
        Color::Indexed(index) => colors[index as usize].map_or_else(|| palette.indexed(index), rgb),
        Color::Named(named) => {
            if let Some(set) = colors[named] {
                return rgb(set);
            }
            let index = named as usize;
            match named {
                _ if index < 16 => palette.named[index],
                NamedColor::Foreground | NamedColor::BrightForeground => palette.foreground,
                NamedColor::Background => palette.background,
                NamedColor::Cursor => palette.cursor,
                NamedColor::DimForeground => dimmed(palette.foreground),
                // The dim ones of the first eight.
                _ => {
                    let first = NamedColor::DimBlack as usize;
                    palette
                        .named
                        .get(index.wrapping_sub(first))
                        .map_or(palette.foreground, |&color| dimmed(color))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use alacritty_terminal::term::test::mock_term;

    fn palette() -> Palette {
        Palette {
            named: [0x10; 16],
            foreground: 0xeeeeee,
            background: 0x111111,
            cursor: 0xff0000,
            selection: 0x0000ff,
        }
    }

    #[test]
    fn the_grid_is_copied_with_its_colours() {
        let mut term = mock_term("ab\r\ncd");
        let mut screen = Screen::default();
        screen.copy_from(&mut term, &palette(), true);
        assert_eq!((screen.columns, screen.lines), (2, 2));
        assert_eq!(screen.letters(0), ['a', 'b']);
        assert_eq!(screen.letters(1), ['c', 'd']);
        assert_eq!(screen.rows[0].glyphs[0].fg, 0xeeeeee);
        assert_eq!(screen.background, 0x111111);
        assert!(screen.caret.is_some());
    }

    #[test]
    fn the_selected_columns_of_a_line_follow_the_selection() {
        let chosen = Chosen {
            start: (1, 3),
            end: (3, 2),
            block: false,
        };
        assert_eq!(chosen_columns(chosen, 0, 10), None);
        assert_eq!(chosen_columns(chosen, 1, 10), Some(3..10));
        assert_eq!(chosen_columns(chosen, 2, 10), Some(0..10));
        assert_eq!(chosen_columns(chosen, 3, 10), Some(0..3));
        let block = Chosen {
            block: true,
            ..chosen
        };
        assert_eq!(chosen_columns(block, 2, 10), Some(2..4));
    }

    #[test]
    fn named_and_numbered_colours_take_the_palette() {
        let colors = Colors::default();
        let palette = palette();
        assert_eq!(
            color_of(Color::Named(NamedColor::Red), &colors, &palette),
            0x10
        );
        assert_eq!(
            color_of(Color::Indexed(196), &colors, &palette),
            palette.indexed(196)
        );
        assert_eq!(
            color_of(Color::Spec(Rgb { r: 1, g: 2, b: 3 }), &colors, &palette),
            0x010203
        );
        assert_eq!(
            color_of(Color::Named(NamedColor::DimForeground), &colors, &palette),
            dimmed(0xeeeeee)
        );
    }
}
