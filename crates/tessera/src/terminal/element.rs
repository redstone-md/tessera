// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The grid of a terminal as GPUI draws it: the backgrounds of the cells
//! that have one, the selection, the cursor in its shape, and the letters
//! in runs of cells that look alike, each run shaped with every letter one
//! cell wide; a letter two cells wide, or with marks over it, is a run of
//! its own. The font is `[terminal] font`, which falls back to Consolas.

use std::time::Instant;

use alacritty_terminal::vte::ansi::CursorShape;
use gpui::{
    App, BorderStyle, Bounds, ContentMask, ElementInputHandler, Entity, Font, FontFallbacks,
    FontStyle, FontWeight, Hsla, IntoElement, Pixels, SharedString, StrikethroughStyle, Styled,
    TextRun, UnderlineStyle, Window, canvas, fill, outline, point, px, size,
};

use super::Terminals;
use super::palette::hsla;
use super::screen::{Glyph, Look, Row};
use super::session::GridSize;
use super::view::{Layout, PADDING, TerminalView};

/// The families letters a font lacks are looked for in.
const FALLBACKS: [&str; 4] = [
    "Consolas",
    "Segoe UI Symbol",
    "Segoe UI Emoji",
    "Courier New",
];

/// How much of the colour of the selection is laid over what is selected.
const SELECTION_SHARE: f32 = 0.4;

/// How strongly a bell flashes the terminal.
const FLASH_SHARE: f32 = 0.12;

/// How thick the lines of the cursor and of underlines are.
const LINE: Pixels = px(1.);
const BEAM: Pixels = px(2.);

/// The grid of the terminal `view`, which fills what holds it.
pub fn grid(view: Entity<TerminalView>, focused: bool) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| paint(&view, bounds, focused, window, cx),
    )
    .size_full()
}

/// The fonts a terminal draws with, by how bold and slanted.
struct Fonts {
    family: SharedString,
    fallbacks: FontFallbacks,
}

impl Fonts {
    fn font(&self, look: Look) -> Font {
        Font {
            family: self.family.clone(),
            features: Default::default(),
            fallbacks: Some(self.fallbacks.clone()),
            weight: match look.has(Look::BOLD) {
                true => FontWeight::BOLD,
                false => FontWeight::NORMAL,
            },
            style: match look.has(Look::ITALIC) {
                true => FontStyle::Italic,
                false => FontStyle::Normal,
            },
        }
    }
}

/// Letters of one line that are shaped together, from a column on.
struct Segment {
    start: usize,
    text: String,
    runs: Vec<TextRun>,
}

impl Segment {
    fn new(start: usize) -> Self {
        Self {
            start,
            text: String::new(),
            runs: Vec::new(),
        }
    }

    /// Adds `letters`, drawn in `color` with `look`, to the last run where
    /// it looks the same, or as a run of its own.
    fn push(&mut self, letters: &str, color: Hsla, look: Look, fonts: &Fonts) {
        let font = fonts.font(look);
        let underline =
            (look.has(Look::UNDERLINE) || look.has(Look::CURLY)).then(|| UnderlineStyle {
                thickness: LINE,
                color: Some(color),
                wavy: look.has(Look::CURLY),
            });
        let strikethrough = look.has(Look::STRIKE).then_some(StrikethroughStyle {
            thickness: LINE,
            color: Some(color),
        });
        self.text.push_str(letters);
        if let Some(last) = self.runs.last_mut()
            && last.color == color
            && last.font == font
            && last.underline == underline
            && last.strikethrough == strikethrough
        {
            last.len += letters.len();
            return;
        }
        self.runs.push(TextRun {
            len: letters.len(),
            font,
            color,
            background_color: None,
            underline,
            strikethrough,
        });
    }
}

/// Paints the terminal `view` over `bounds`, and tells it how its grid was
/// laid out where that changed.
fn paint(
    view: &Entity<TerminalView>,
    bounds: Bounds<Pixels>,
    focused: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let config = Terminals::config(cx);
    let fonts = Fonts {
        family: Terminals::family(cx),
        fallbacks: FontFallbacks::from_fonts(FALLBACKS.map(str::to_owned).to_vec()),
    };
    let font_size = px(config.font_size);
    let regular = window
        .text_system()
        .resolve_font(&fonts.font(Look::default()));
    let width = { window.text_system().advance(regular, font_size, 'M') }
        .map(|advance| advance.width)
        .unwrap_or(font_size * 0.6)
        .max(px(1.));
    let height = (font_size * config.line_height).round().max(px(1.));
    let padding = px(PADDING);
    let room = size(
        (bounds.size.width - padding * 2.).max(px(0.)),
        (bounds.size.height - padding * 2.).max(px(0.)),
    );
    let layout = Layout {
        origin: point(bounds.origin.x + padding, bounds.origin.y + padding),
        cell: size(width, height),
        size: GridSize {
            columns: ((room.width / width) as usize).max(2),
            lines: ((room.height / height) as usize).max(1),
        },
        scale: window.scale_factor(),
    };
    let focus = view.read(cx).focus.clone();
    window.handle_input(&focus, ElementInputHandler::new(bounds, view.clone()), cx);
    if view.read(cx).layout != Some(layout) {
        let view = view.clone();
        cx.defer(move |cx| view.update(cx, |view, cx| view.laid_out(layout, cx)));
    }
    // What the terminal shows is borrowed for the frame, as drawing
    // letters needs the application.
    let (screen, palette, blink_shown, flash_until, composing) = view.update(cx, |terminal, _| {
        (
            std::mem::take(&mut terminal.screen),
            terminal.palette,
            terminal.blink_shown,
            terminal.flash_until,
            terminal.composing.clone(),
        )
    });
    let caret = screen.caret.filter(|_| !focused || blink_shown);
    let block = caret.filter(|caret| {
        focused && matches!(caret.shape, CursorShape::Block) && composing.is_empty()
    });
    let flashing = flash_until.is_some_and(|until| Instant::now() < until);
    let lines = screen.lines.min(layout.size.lines);
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for (line, row) in screen.rows.iter().take(lines).enumerate() {
            paint_backgrounds(row, line, screen.background, &layout, window);
            if let Some(chosen) = screen.chosen_in(line) {
                let area = layout.bounds_of(line, chosen.start, chosen.len());
                window.paint_quad(fill(area, hsla(palette.selection).opacity(SELECTION_SHARE)));
            }
            let under = block.filter(|caret| caret.line == line);
            if let Some(caret) = under {
                let cells = if caret.wide { 2 } else { 1 };
                let area = layout.bounds_of(line, caret.column, cells);
                window.paint_quad(fill(area, hsla(palette.cursor)));
            }
            let covered = under.map(|caret| caret.column);
            paint_letters(
                row,
                line,
                covered,
                palette.background,
                &fonts,
                font_size,
                &layout,
                window,
                cx,
            );
        }
        if let Some(caret) = caret.filter(|caret| caret.line < lines) {
            let cells = if caret.wide { 2 } else { 1 };
            let cell = layout.bounds_of(caret.line, caret.column, cells);
            let color = hsla(palette.cursor);
            match (focused, caret.shape) {
                (false, _) | (true, CursorShape::HollowBlock) => {
                    window.paint_quad(outline(cell, color, BorderStyle::Solid));
                }
                (true, CursorShape::Underline) => {
                    let line = Bounds {
                        origin: point(cell.origin.x, cell.origin.y + cell.size.height - BEAM),
                        size: size(cell.size.width, BEAM),
                    };
                    window.paint_quad(fill(line, color));
                }
                (true, CursorShape::Beam) => {
                    let beam = Bounds {
                        origin: cell.origin,
                        size: size(BEAM, cell.size.height),
                    };
                    window.paint_quad(fill(beam, color));
                }
                (true, CursorShape::Block | CursorShape::Hidden) => {}
            }
        }
        if let (false, Some(caret)) = (composing.is_empty(), screen.caret) {
            paint_composing(
                &composing,
                caret.line,
                caret.column,
                &palette,
                &fonts,
                font_size,
                &layout,
                window,
                cx,
            );
        }
        if flashing {
            window.paint_quad(fill(bounds, hsla(palette.foreground).opacity(FLASH_SHARE)));
        }
    });
    view.update(cx, |terminal, _| terminal.screen = screen);
}

/// Paints the backgrounds of the cells of `row` that have their own, one
/// box for each run of cells of one colour.
fn paint_backgrounds(
    row: &Row,
    line: usize,
    background: u32,
    layout: &Layout,
    window: &mut Window,
) {
    let mut run: Option<(usize, u32)> = None;
    let flush = |run: Option<(usize, u32)>, end: usize, window: &mut Window| {
        if let Some((start, color)) = run
            && end > start
        {
            let area = layout.bounds_of(line, start, end - start);
            window.paint_quad(fill(area, hsla(color)));
        }
    };
    for (column, glyph) in row.glyphs.iter().enumerate() {
        let color = (glyph.bg != background).then_some(glyph.bg);
        match (run, color) {
            (Some((_, open)), Some(color)) if open == color => {}
            (_, color) => {
                flush(run, column, window);
                run = color.map(|color| (column, color));
            }
        }
    }
    flush(run, row.glyphs.len(), window);
}

/// Whether a glyph draws nothing: a space with no line through or under it.
fn is_blank(glyph: &Glyph) -> bool {
    glyph.letter == ' '
        && !glyph.look.has(Look::UNDERLINE)
        && !glyph.look.has(Look::CURLY)
        && !glyph.look.has(Look::STRIKE)
}

/// Paints the letters of `row`, the one in the column `covered` by the
/// block cursor in `on_cursor`.
#[allow(clippy::too_many_arguments)]
fn paint_letters(
    row: &Row,
    line: usize,
    covered: Option<usize>,
    on_cursor: u32,
    fonts: &Fonts,
    font_size: Pixels,
    layout: &Layout,
    window: &mut Window,
    cx: &mut App,
) {
    let mut segment: Option<Segment> = None;
    let flush = |segment: &mut Option<Segment>, window: &mut Window, cx: &mut App| {
        if let Some(segment) = segment.take().filter(|segment| !segment.text.is_empty()) {
            let shaped = window.text_system().shape_line(
                segment.text.into(),
                font_size,
                &segment.runs,
                Some(layout.cell.width),
            );
            let origin = layout.bounds_of(line, segment.start, 1).origin;
            let _ = shaped.paint(origin, layout.cell.height, window, cx);
        }
    };
    for (column, glyph) in row.glyphs.iter().enumerate() {
        if glyph.look.has(Look::SPACER) {
            flush(&mut segment, window, cx);
            continue;
        }
        let color = hsla(match covered == Some(column) {
            true => on_cursor,
            false => glyph.fg,
        });
        let marks = { row.marks.iter() }.find(|(at, _)| *at == column);
        // A wide letter, or one with marks, takes more than one cell's
        // room when shaped, and is shaped by itself.
        if glyph.look.has(Look::WIDE) || marks.is_some() {
            flush(&mut segment, window, cx);
            let mut alone = Segment::new(column);
            let mut letters = glyph.letter.to_string();
            if let Some((_, marks)) = marks {
                letters.push_str(marks);
            }
            alone.push(&letters, color, glyph.look.drawn(), fonts);
            segment = Some(alone);
            flush(&mut segment, window, cx);
            continue;
        }
        if segment.is_none() && is_blank(glyph) && covered != Some(column) {
            continue;
        }
        let open = segment.get_or_insert_with(|| Segment::new(column));
        let mut letter = [0_u8; 4];
        open.push(
            glyph.letter.encode_utf8(&mut letter),
            color,
            glyph.look.drawn(),
            fonts,
        );
    }
    flush(&mut segment, window, cx);
}

/// Paints what an input method composes, at the cursor, underlined.
#[allow(clippy::too_many_arguments)]
fn paint_composing(
    composing: &str,
    line: usize,
    column: usize,
    palette: &super::palette::Palette,
    fonts: &Fonts,
    font_size: Pixels,
    layout: &Layout,
    window: &mut Window,
    cx: &mut App,
) {
    let color = hsla(palette.foreground);
    let run = TextRun {
        len: composing.len(),
        font: fonts.font(Look::default()),
        color,
        background_color: None,
        underline: Some(UnderlineStyle {
            thickness: LINE,
            color: Some(color),
            wavy: false,
        }),
        strikethrough: None,
    };
    let shaped = { window.text_system() }.shape_line(
        SharedString::from(composing.to_owned()),
        font_size,
        &[run],
        None,
    );
    let origin = layout.bounds_of(line, column, 1).origin;
    let area = Bounds {
        origin,
        size: size(shaped.width.max(layout.cell.width), layout.cell.height),
    };
    window.paint_quad(fill(area, hsla(palette.background)));
    let _ = shaped.paint(origin, layout.cell.height, window, cx);
}
