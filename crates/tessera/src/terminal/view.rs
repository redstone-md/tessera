// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A terminal as a view of GPUI: it starts its shell once it knows how
//! large it is drawn, redraws at most 60 times a second while the shell
//! writes, and takes the keys, the text typed through the input methods
//! of Windows, the pointer and the wheel.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::Rgb;
use async_channel::Receiver;
use gpui::{
    App, AsyncApp, Bounds, ClipboardItem, Context, EntityInputHandler, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, ScrollWheelEvent, Size, Styled, Subscription, Task,
    UTF16Selection, WeakEntity, Window, div, point, px,
};

use super::element;
use super::keys::{Held, Sent, encode, pasted};
use super::links::{Link, link_at};
use super::palette::{Palette, hsla};
use super::screen::Screen;
use super::session::{GridSize, Happened, Session};
use super::shell::{ShellPlan, same_folder};
use super::{TerminalConfig, Terminals};
use crate::files::Files;
use crate::i18n::Language;
use crate::platform::{Launch, shell_is_idle};
use crate::runners::fill;
use crate::theme::Theme;

/// The least time between two frames drawn for output: 60 a second.
const FRAME: Duration = Duration::from_millis(16);

/// How long the cursor shows and then hides while it blinks.
const BLINK: Duration = Duration::from_millis(530);

/// How long the terminal flashes for a bell.
pub(super) const FLASH: Duration = Duration::from_millis(150);

/// How soon the grid is copied again when the thread that parses holds
/// it.
const RETRY: Duration = Duration::from_millis(4);

/// The room around the grid, in logical pixels.
pub(super) const PADDING: f32 = 6.;

/// What a terminal tells the view it is in.
#[derive(Clone, Debug, PartialEq)]
pub enum TerminalEvent {
    /// Its title changed.
    Titled,
    /// The shell said it went to this folder.
    Folder(PathBuf),
    /// The shell ended.
    Exited,
}

/// Where the shell of a terminal is.
enum State {
    /// The terminal waits to be drawn, to start its shell at its size.
    Waiting,
    Starting,
    Running(Session),
    /// The shell could not be started.
    Failed(String),
    /// The shell ended, with its code where Windows gave one.
    Exited(Option<u32>),
}

/// How the grid was laid out, as last drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// Where the first cell is, in the window.
    pub origin: Point<Pixels>,
    /// How wide and high a cell is.
    pub cell: Size<Pixels>,
    pub size: GridSize,
    /// Physical pixels per logical one.
    pub scale: f32,
}

impl Layout {
    /// The line and the column at `position` of the window, kept within
    /// the grid, and which half of the cell it is in.
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize, Side) {
        let x = ((position.x - self.origin.x) / self.cell.width).max(0.);
        let y = ((position.y - self.origin.y) / self.cell.height).max(0.);
        let column = (x as usize).min(self.size.columns.saturating_sub(1));
        let line = (y as usize).min(self.size.lines.saturating_sub(1));
        let side = match x.fract() < 0.5 {
            true => Side::Left,
            false => Side::Right,
        };
        (line, column, side)
    }

    /// Where the cell at `line` and `column` is drawn, in the window.
    pub fn bounds_of(&self, line: usize, column: usize, cells: usize) -> Bounds<Pixels> {
        Bounds {
            origin: point(
                self.origin.x + self.cell.width * column as f32,
                self.origin.y + self.cell.height * line as f32,
            ),
            size: gpui::size(self.cell.width * cells as f32, self.cell.height),
        }
    }
}

/// A terminal: a shell, the grid it writes and how it is drawn.
pub struct TerminalView {
    pub(super) focus: FocusHandle,
    /// Where the shell starts.
    start_in: Option<PathBuf>,
    state: State,
    pub(super) screen: Screen,
    pub(super) palette: Palette,
    pub(super) layout: Option<Layout>,
    title: Option<String>,
    /// The folder the shell said it is in last, or was sent to.
    reported: Option<PathBuf>,
    /// Until when the terminal flashes for a bell.
    pub(super) flash_until: Option<Instant>,
    /// Whether the cursor shows, as it blinks.
    pub(super) blink_shown: bool,
    blinking: Option<Task<()>>,
    focused: bool,
    /// What an input method composes, before it is typed.
    pub(super) composing: String,
    selecting: bool,
    /// The lines the wheel turned that were not scrolled yet.
    wheel: f32,
    /// Whether the next copy of the grid copies all of it.
    everything: bool,
    retrying: bool,
    _heard: Option<Task<()>>,
    _settings: [Subscription; 2],
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl TerminalView {
    /// A terminal whose shell starts in `start_in`, once it is drawn.
    pub fn new(start_in: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let settings = [
            cx.observe_global::<Theme>(|this, cx| this.restyle(cx)),
            cx.observe_global::<Terminals>(|this, cx| this.restyle(cx)),
        ];
        let palette = Palette::of(cx.global::<Theme>(), &Terminals::config(cx).palette);
        Self {
            focus: cx.focus_handle(),
            start_in,
            state: State::Waiting,
            screen: Screen::default(),
            palette,
            layout: None,
            title: None,
            reported: None,
            flash_until: None,
            blink_shown: true,
            blinking: None,
            focused: false,
            composing: String::new(),
            selecting: false,
            wheel: 0.,
            everything: true,
            retrying: false,
            _heard: None,
            _settings: settings,
        }
    }

    /// Gives the keys to the terminal.
    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    /// Whether the keys go to the terminal.
    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// What the terminal is called: what the shell named it, else its
    /// folder, else "Terminal".
    pub fn title(&self, cx: &App) -> String {
        if let Some(title) = self.title.as_ref().filter(|title| !title.trim().is_empty()) {
            return title.clone();
        }
        { self.folder() }
            .and_then(|folder| folder.file_name().or(Some(folder.as_os_str())))
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| Language::of(cx).say("Terminal").to_owned())
    }

    /// The folder the shell is in, as far as it is known.
    pub fn folder(&self) -> Option<&Path> {
        self.reported.as_deref().or(self.start_in.as_deref())
    }

    /// Sends the shell to `folder`, where it waits at its prompt: what is
    /// typed on its line is cleared and the folder changed as the shell
    /// changes it. A shell that runs a program is left where it is, and
    /// one that has not started yet starts there.
    pub fn go_to(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        if self
            .folder()
            .is_some_and(|known| same_folder(known, &folder))
        {
            return;
        }
        let session = match &self.state {
            State::Running(session) => session,
            State::Waiting | State::Starting => {
                self.start_in = Some(folder);
                return;
            }
            State::Failed(_) | State::Exited(_) => return,
        };
        let Some(line) = session.plan.going_to(&folder) else {
            return;
        };
        let shell = session.shell_id();
        let idle = cx
            .background_executor()
            .spawn(async move { shell_is_idle(shell) });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if !idle.await {
                return;
            }
            this.update(cx, |this, _| {
                if let State::Running(session) = &this.state
                    && session.shell_id() == shell
                {
                    session.write(line);
                    this.reported = Some(folder);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Takes the colours and the settings of the terminal anew.
    fn restyle(&mut self, cx: &mut Context<Self>) {
        let config = Terminals::config(cx);
        self.palette = Palette::of(cx.global::<Theme>(), &config.palette);
        self.everything = true;
        if let State::Running(session) = &self.state {
            session
                .term
                .lock_unfair()
                .set_options(config.term_options());
        }
        self.refresh(cx);
        cx.notify();
    }

    /// Notes how the grid was laid out as it was drawn: starts the shell
    /// at that size the first time, and resizes it when it changed.
    pub(super) fn laid_out(&mut self, layout: Layout, cx: &mut Context<Self>) {
        let resized = self.layout.is_none_or(|known| known.size != layout.size);
        self.layout = Some(layout);
        if !resized {
            return;
        }
        match &self.state {
            State::Waiting => self.start(layout.size, cx),
            State::Running(session) => {
                session.resize(layout.size);
                self.everything = true;
                self.refresh(cx);
            }
            State::Starting | State::Failed(_) | State::Exited(_) => {}
        }
    }

    /// Starts the shell of the configuration at `size`, away from the
    /// user interface.
    fn start(&mut self, size: GridSize, cx: &mut Context<Self>) {
        self.state = State::Starting;
        let config = Terminals::config(cx);
        // A shell that knows no folder starts in that of the user.
        let folder =
            { self.start_in.clone() }.or_else(|| Files::home().folder().map(PathBuf::from));
        let started = cx.background_executor().spawn(async move {
            let plan = ShellPlan::resolve(config.shell.as_deref());
            Session::start(plan, folder, size, config.term_options())
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let started = started.await;
            this.update(cx, |this, cx| match started {
                Ok((session, heard)) => {
                    this.state = State::Running(session);
                    this.everything = true;
                    this.listen(heard, cx);
                    // The size may have changed while the shell started.
                    if let (Some(layout), State::Running(session)) = (this.layout, &this.state)
                        && layout.size != size
                    {
                        session.resize(layout.size);
                    }
                    this.refresh(cx);
                }
                Err(error) => {
                    report!("the shell of the terminal did not start: {error}");
                    this.state = State::Failed(error.to_string());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Starts the shell again, after it ended or did not start.
    fn restart(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::default();
        self.title = None;
        self.reported = None;
        self.state = State::Waiting;
        self._heard = None;
        if let Some(layout) = self.layout {
            self.start(layout.size, cx);
        }
        cx.emit(TerminalEvent::Titled);
        cx.notify();
    }

    /// Hears what the terminal tells, at most once a frame for output.
    fn listen(&mut self, heard: Receiver<Happened>, cx: &mut Context<Self>) {
        let listening = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut drawn = Instant::now() - FRAME;
            while let Ok(first) = heard.recv().await {
                if matches!(first, Happened::Woke) {
                    let wait = FRAME.saturating_sub(drawn.elapsed());
                    if !wait.is_zero() {
                        cx.background_executor().timer(wait).await;
                    }
                }
                let mut batch = vec![first];
                while let Ok(more) = heard.try_recv() {
                    batch.push(more);
                }
                if this.update(cx, |this, cx| this.heard(batch, cx)).is_err() {
                    break;
                }
                drawn = Instant::now();
            }
        });
        self._heard = Some(listening);
    }

    fn heard(&mut self, batch: Vec<Happened>, cx: &mut Context<Self>) {
        let mut woke = false;
        for happened in batch {
            match happened {
                Happened::Woke => woke = true,
                Happened::Titled(title) => {
                    self.title = title;
                    cx.emit(TerminalEvent::Titled);
                }
                Happened::Bell => self.flash(cx),
                Happened::Folder(reported) => self.reported_folder(&reported, cx),
                Happened::Copied(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                Happened::ClipboardWanted(answer) => {
                    let text = { cx.read_from_clipboard() }
                        .and_then(|item| item.text())
                        .unwrap_or_default();
                    self.answer(answer(&text));
                }
                Happened::ColorWanted(index, answer) => {
                    let color = self.color_numbered(index);
                    let rgb = Rgb {
                        r: (color >> 16) as u8,
                        g: (color >> 8) as u8,
                        b: color as u8,
                    };
                    self.answer(answer(rgb));
                }
                Happened::SizeWanted(answer) => {
                    if let Some(layout) = self.layout {
                        let cell = |length: Pixels| (length / px(1.) * layout.scale) as u16;
                        self.answer(answer(WindowSize {
                            num_lines: layout.size.lines as u16,
                            num_cols: layout.size.columns as u16,
                            cell_width: cell(layout.cell.width),
                            cell_height: cell(layout.cell.height),
                        }));
                    }
                }
                Happened::Exited(code) => {
                    self.state = State::Exited(code);
                    self.selecting = false;
                    cx.emit(TerminalEvent::Exited);
                    cx.notify();
                }
            }
        }
        if woke {
            self.refresh(cx);
        }
    }

    /// Writes what the terminal answers a program, unseen by the user.
    fn answer(&self, text: String) {
        if let State::Running(session) = &self.state {
            session.write(text.into_bytes());
        }
    }

    /// The colour a program asks for by its number: of the 256, or the
    /// foreground, background or cursor after them.
    fn color_numbered(&self, index: usize) -> u32 {
        match index {
            0..=255 => self.palette.indexed(index as u8),
            257 => self.palette.background,
            258 => self.palette.cursor,
            _ => self.palette.foreground,
        }
    }

    /// Notes the folder the shell reported, and tells of it where it is
    /// another one.
    fn reported_folder(&mut self, reported: &str, cx: &mut Context<Self>) {
        let State::Running(session) = &self.state else {
            return;
        };
        let Some(folder) = session.plan.windows_folder(reported) else {
            return;
        };
        if self
            .reported
            .as_deref()
            .is_some_and(|known| same_folder(known, &folder))
        {
            return;
        }
        self.reported = Some(folder.clone());
        cx.emit(TerminalEvent::Folder(folder));
        cx.emit(TerminalEvent::Titled);
    }

    /// Flashes the terminal for a bell.
    fn flash(&mut self, cx: &mut Context<Self>) {
        self.flash_until = Some(Instant::now() + FLASH);
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(FLASH).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// Copies what changed in the grid, to be drawn; when the thread that
    /// parses holds the grid, a moment later.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let State::Running(session) = &self.state else {
            return;
        };
        session.drawn();
        let copied = match session.term.try_lock_unfair() {
            Some(mut term) => {
                self.screen
                    .copy_from(&mut term, &self.palette, self.everything);
                true
            }
            None => false,
        };
        if copied {
            self.everything = false;
            cx.notify();
            return;
        }
        if self.retrying {
            return;
        }
        self.retrying = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(RETRY).await;
            this.update(cx, |this, cx| {
                this.retrying = false;
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Does `deed` to the grid of the running shell, then copies it anew.
    fn with_term(
        &mut self,
        cx: &mut Context<Self>,
        deed: impl FnOnce(&mut alacritty_terminal::Term<super::session::Listener>),
    ) {
        if let State::Running(session) = &self.state {
            deed(&mut session.term.lock_unfair());
            self.refresh(cx);
        }
    }

    /// Writes what was typed: the view goes back to the end of the output
    /// and the cursor shows.
    fn typed(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if self.screen.scrolled > 0 {
            self.with_term(cx, |term| term.scroll_display(Scroll::Bottom));
        }
        if let State::Running(session) = &self.state {
            session.write(bytes);
        }
        self.stir(cx);
    }

    /// Shows the cursor and has it blink anew, as after typing.
    fn stir(&mut self, cx: &mut Context<Self>) {
        self.blink_shown = true;
        self.blinking = None;
        cx.notify();
    }

    /// Has the cursor blink while the terminal has the keys.
    fn keep_blinking(&mut self, focused: bool, cx: &mut Context<Self>) {
        self.focused = focused;
        if !focused {
            self.blinking = None;
            self.blink_shown = true;
            return;
        }
        if self.blinking.is_some() {
            return;
        }
        let blinking = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(BLINK).await;
                let goes_on = this.update(cx, |this, cx| {
                    let blinks = this.screen.caret.is_some_and(|caret| caret.blinking);
                    if this.focused && blinks {
                        this.blink_shown = !this.blink_shown;
                        cx.notify();
                    }
                    this.focused
                });
                if !goes_on.unwrap_or(false) {
                    break;
                }
            }
        });
        self.blinking = Some(blinking);
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let key = keystroke.key.as_str();
        let held = Held {
            control: keystroke.modifiers.control,
            alt: keystroke.modifiers.alt,
            shift: keystroke.modifiers.shift,
        };
        match &self.state {
            State::Exited(_) | State::Failed(_) => {
                self.restart(cx);
                cx.stop_propagation();
                return;
            }
            State::Waiting | State::Starting => return,
            State::Running(_) => {}
        }
        let shortcut = match (key, held.control, held.alt, held.shift) {
            ("c", true, false, true) => {
                self.copy(cx);
                true
            }
            ("v", true, false, true) | ("insert", false, false, true) => {
                self.paste(cx);
                true
            }
            ("pageup", false, false, true) => {
                self.with_term(cx, |term| term.scroll_display(Scroll::PageUp));
                true
            }
            ("pagedown", false, false, true) => {
                self.with_term(cx, |term| term.scroll_display(Scroll::PageDown));
                true
            }
            _ => false,
        };
        if shortcut {
            cx.stop_propagation();
            return;
        }
        let app_cursor = self.screen.mode.contains(TermMode::APP_CURSOR);
        match encode(key, keystroke.key_char.as_deref(), held, app_cursor) {
            Sent::Bytes(bytes) => {
                self.typed(bytes, cx);
                cx.stop_propagation();
            }
            // The text comes through the input handler of the window.
            Sent::Text | Sent::Nothing => {}
        }
    }

    /// Puts what is selected on the clipboard.
    fn copy(&mut self, cx: &mut Context<Self>) {
        let State::Running(session) = &self.state else {
            return;
        };
        let text = session.term.lock_unfair().selection_to_string();
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Writes the text on the clipboard, as the program in the terminal
    /// takes a paste.
    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let bracketed = self.screen.mode.contains(TermMode::BRACKETED_PASTE);
        self.typed(pasted(&text, bracketed), cx);
    }

    /// The point of the grid at the line and column of the screen.
    fn grid_point(&self, line: usize, column: usize) -> GridPoint {
        GridPoint::new(
            Line(line as i32 - self.screen.scrolled as i32),
            Column(column),
        )
    }

    fn pressed(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let Some(layout) = self.layout else {
            return;
        };
        let (line, column, side) = layout.cell_at(event.position);
        if event.modifiers.control {
            self.open_link(line, column, cx);
            return;
        }
        let kind = match event.click_count {
            0 | 1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        let at = self.grid_point(line, column);
        let extend = event.modifiers.shift;
        self.with_term(cx, |term| match term.selection.as_mut() {
            Some(selection) if extend => selection.update(at, side),
            _ => term.selection = Some(Selection::new(kind, at, side)),
        });
        self.selecting = true;
    }

    fn dragged(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(layout) = self.layout else {
            return;
        };
        let (line, column, side) = layout.cell_at(event.position);
        let at = self.grid_point(line, column);
        self.with_term(cx, |term| {
            if let Some(selection) = term.selection.as_mut() {
                selection.update(at, side);
            }
        });
    }

    fn released(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.selecting) {
            return;
        }
        if Terminals::config(cx).copy_on_select {
            self.copy(cx);
        }
    }

    /// A click with the right button copies what is selected and lets go
    /// of it, or pastes where nothing is.
    fn pressed_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        if self.screen.chosen.is_some() {
            self.copy(cx);
            self.with_term(cx, |term| term.selection = None);
        } else {
            self.paste(cx);
        }
    }

    fn scrolled(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.layout else {
            return;
        };
        let delta = event.delta.pixel_delta(layout.cell.height);
        self.wheel += delta.y / layout.cell.height;
        let lines = self.wheel.trunc() as i32;
        self.wheel -= lines as f32;
        if lines == 0 {
            return;
        }
        let mode = self.screen.mode;
        let alternate = mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL);
        if alternate && !event.modifiers.shift {
            // A program of the whole screen, as a pager, takes the wheel
            // as the arrows.
            let app_cursor = mode.contains(TermMode::APP_CURSOR);
            let key = if lines > 0 { "up" } else { "down" };
            if let Sent::Bytes(bytes) = encode(key, None, Held::default(), app_cursor) {
                let all = bytes.repeat(lines.unsigned_abs() as usize);
                if let State::Running(session) = &self.state {
                    session.write(all);
                }
            }
            return;
        }
        self.with_term(cx, |term| term.scroll_display(Scroll::Delta(lines)));
    }

    /// Opens the address or the path at the line and column, as Ctrl+click
    /// asks: a path that is not whole is looked for in the folder of the
    /// shell, away from the user interface.
    fn open_link(&self, line: usize, column: usize, cx: &mut Context<Self>) {
        let letters = self.screen.letters(line);
        let Some(link) = link_at(&letters, column) else {
            return;
        };
        let path = match link {
            Link::Address(address) => {
                Launch::new(address).start();
                return;
            }
            Link::Path(path) => PathBuf::from(path),
        };
        let folder = self.folder().map(Path::to_owned);
        let found = cx.background_executor().spawn(async move {
            let candidates = [Some(path.clone()), folder.map(|folder| folder.join(&path))];
            candidates
                .into_iter()
                .flatten()
                .find(|candidate| candidate.is_absolute() && candidate.exists())
        });
        cx.spawn(async move |_, _: &mut AsyncApp| {
            if let Some(path) = found.await {
                Launch::new(path).start();
            }
        })
        .detach();
    }

    /// What the terminal shows below its grid: why the shell did not
    /// start, or that it ended and how it is started again.
    fn message(&self, cx: &App) -> Option<String> {
        let language = Language::of(cx);
        match &self.state {
            State::Failed(error) => Some(format!(
                "{} {}",
                fill(language.say("The shell did not start: {}"), &[error]),
                language.say("Press any key to start the shell again.")
            )),
            State::Exited(code) => {
                let code = code.map_or_else(|| "?".to_owned(), |code| code.to_string());
                Some(format!(
                    "{} {}",
                    fill(language.say("[exited {}]"), &[&code]),
                    language.say("Press any key to start the shell again.")
                ))
            }
            _ => None,
        }
    }
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

/// Text input, through the input methods of Windows: what is composed is
/// drawn at the cursor until it is typed. Positions count UTF-16 code
/// units of what is being composed.
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let units: Vec<u16> = self.composing.encode_utf16().collect();
        let end = range.end.min(units.len());
        Some(String::from_utf16_lossy(&units[range.start.min(end)..end]))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.composing.encode_utf16().count();
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
        let length = self.composing.encode_utf16().count();
        (length > 0).then_some(0..length)
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let composed = std::mem::take(&mut self.composing);
        if !composed.is_empty() {
            self.typed(composed.into_bytes(), cx);
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // What replaces a part of what was composed is typed with the rest.
        let mut typed = match range {
            Some(range) if !self.composing.is_empty() => {
                let mut composed = self.composing.clone();
                let start = place_of(&composed, range.start);
                let end = place_of(&composed, range.end).max(start);
                composed.replace_range(start..end, text);
                composed
            }
            _ => text.to_owned(),
        };
        self.composing.clear();
        typed.retain(|letter| letter != '\u{7f}');
        if !matches!(self.state, State::Running(_)) || typed.is_empty() {
            return cx.notify();
        }
        self.typed(typed.into_bytes(), cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        new_text.clone_into(&mut self.composing);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // The input method puts its window by the cursor.
        let (Some(layout), Some(caret)) = (self.layout, self.screen.caret) else {
            return Some(element_bounds);
        };
        Some(layout.bounds_of(caret.line, caret.column, 1))
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

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        self.keep_blinking(focused, cx);
        let theme = *cx.global::<Theme>();
        let message = self.message(cx).map(|message| {
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .px(theme.spacing.medium)
                .py(theme.spacing.small)
                .bg(theme.colors.surface_container_highest)
                .text_size(theme.typography.label_size)
                .text_color(theme.colors.on_surface)
                .font_family(theme.typography.family)
                .child(message)
        });
        div()
            .id("terminal")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(hsla(self.palette.background))
            .cursor_text()
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::pressed))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| this.pressed_right(window, cx)),
            )
            .on_mouse_move(cx.listener(Self::dragged))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.released(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.released(cx)),
            )
            .on_scroll_wheel(cx.listener(Self::scrolled))
            .child(element::grid(cx.entity(), focused))
            .children(message)
    }
}

impl TerminalConfig {
    /// What the grid of a terminal is set to by the configuration.
    pub fn term_options(&self) -> alacritty_terminal::term::Config {
        alacritty_terminal::term::Config {
            scrolling_history: self.scrollback,
            default_cursor_style: self.cursor.style(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cell_under_the_pointer_is_kept_within_the_grid() {
        let layout = Layout {
            origin: point(px(10.), px(20.)),
            cell: gpui::size(px(8.), px(16.)),
            size: GridSize {
                columns: 80,
                lines: 24,
            },
            scale: 1.,
        };
        let (line, column, side) = layout.cell_at(point(px(10. + 8. * 3. + 6.), px(20. + 17.)));
        assert_eq!((line, column, side), (1, 3, Side::Right));
        let (line, column, side) = layout.cell_at(point(px(0.), px(0.)));
        assert_eq!((line, column, side), (0, 0, Side::Left));
        let (line, column, _) = layout.cell_at(point(px(5000.), px(5000.)));
        assert_eq!((line, column), (23, 79));
        let bounds = layout.bounds_of(2, 4, 2);
        assert_eq!(bounds.origin, point(px(42.), px(52.)));
        assert_eq!(bounds.size.width, px(16.));
    }

    #[test]
    fn places_are_found_by_their_utf16_units() {
        assert_eq!(place_of("a😀я", 1), 1);
        assert_eq!(place_of("a😀я", 3), 5);
        assert_eq!(place_of("a😀я", 9), 7);
    }
}
