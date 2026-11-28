// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One shell in one terminal: the grid of `alacritty_terminal`, which the
//! thread that reads the console parses the output into, the console the
//! shell runs in, and what the terminal tells the window that shows it:
//! that there is something new to draw, a title, a bell, the folder the
//! shell reports, the clipboard, and the end of the shell.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::vte::ansi::{Processor, Rgb, StdSyncHandler};
use async_channel::{Receiver, Sender};

use super::osc::{OscScanner, reported_folder};
use super::shell::ShellPlan;
use crate::platform::{Console, ConsoleSize, ConsoleStart, ConsoleWriter};

/// How large the grid is, in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub columns: usize,
    pub lines: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

impl GridSize {
    fn console(self) -> ConsoleSize {
        ConsoleSize {
            columns: self.columns.min(u16::MAX as usize) as u16,
            lines: self.lines.min(u16::MAX as usize) as u16,
        }
    }
}

/// What formats an answer for the program in the terminal.
pub type Answer<T> = Arc<dyn Fn(T) -> String + Send + Sync>;

/// What formats the text of the clipboard for the program that asked.
pub type TextAnswer = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// What the terminal tells the window that shows it.
pub enum Happened {
    /// There is something new to draw.
    Woke,
    /// The shell named the terminal, or took its name back.
    Titled(Option<String>),
    Bell,
    /// The folder the shell said it is in, as it wrote it.
    Folder(String),
    /// A program put this text on the clipboard.
    Copied(String),
    /// A program asks for the text on the clipboard.
    ClipboardWanted(TextAnswer),
    /// A program asks for the colour of a number.
    ColorWanted(usize, Answer<Rgb>),
    /// A program asks how large the terminal is.
    SizeWanted(Answer<WindowSize>),
    /// The shell ended, with its code where Windows gave one.
    Exited(Option<u32>),
}

/// Where the terminal writes its answers to programs, once the console is
/// open; what it answers before is kept until then.
#[derive(Default)]
struct Answers {
    writer: Option<ConsoleWriter>,
    waiting: Vec<u8>,
}

/// What hears the terminal, on the thread that reads the console.
#[derive(Clone)]
pub struct Listener {
    happened: Sender<Happened>,
    answers: Arc<Mutex<Answers>>,
    /// Whether the window was told there is something new to draw and has
    /// not drawn it yet, so that it is told once.
    woken: Arc<AtomicBool>,
}

impl Listener {
    fn tell(&self, happened: Happened) {
        let _ = self.happened.try_send(happened);
    }

    fn wake(&self) {
        if !self.woken.swap(true, Ordering::AcqRel) {
            self.tell(Happened::Woke);
        }
    }

    fn answer(&self, text: &str) {
        let mut answers = self
            .answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &answers.writer {
            Some(writer) => writer.write(text.as_bytes().to_vec()),
            None => answers.waiting.extend(text.as_bytes()),
        }
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup => self.wake(),
            Event::Title(title) => self.tell(Happened::Titled(Some(title))),
            Event::ResetTitle => self.tell(Happened::Titled(None)),
            Event::Bell => self.tell(Happened::Bell),
            Event::PtyWrite(text) => self.answer(&text),
            Event::ClipboardStore(_, text) => self.tell(Happened::Copied(text)),
            Event::ClipboardLoad(_, answer) => self.tell(Happened::ClipboardWanted(answer)),
            Event::ColorRequest(index, answer) => {
                self.tell(Happened::ColorWanted(index, answer));
            }
            Event::TextAreaSizeRequest(answer) => self.tell(Happened::SizeWanted(answer)),
            // The end of the shell is told by the console.
            Event::Exit | Event::ChildExit(_) => {}
            Event::MouseCursorDirty | Event::CursorBlinkingChange => {}
        }
    }
}

/// A shell running in a terminal; it ends when this is dropped.
pub struct Session {
    pub term: Arc<FairMutex<Term<Listener>>>,
    console: Console,
    writer: ConsoleWriter,
    pub plan: ShellPlan,
    woken: Arc<AtomicBool>,
}

impl Session {
    /// Starts the shell of `plan` in `folder`, in a terminal of `size` set
    /// as `options` say. Blocks while Windows starts it: not for the
    /// thread of GPUI.
    pub fn start(
        plan: ShellPlan,
        folder: Option<PathBuf>,
        size: GridSize,
        options: term::Config,
    ) -> io::Result<(Self, Receiver<Happened>)> {
        let (happened, heard) = async_channel::unbounded();
        let answers = Arc::new(Mutex::new(Answers::default()));
        let woken = Arc::new(AtomicBool::new(false));
        let listener = Listener {
            happened: happened.clone(),
            answers: answers.clone(),
            woken: woken.clone(),
        };
        let term = Arc::new(FairMutex::new(Term::new(options, &size, listener.clone())));
        let parsed = term.clone();
        let mut processor: Processor<StdSyncHandler> = Processor::new();
        let mut scanner = OscScanner::default();
        let reader = listener.clone();
        let output = move |bytes: &[u8]| -> Option<Instant> {
            let mut term = parsed.lock();
            match bytes.is_empty() {
                // The time a synchronized update may take has passed.
                true => processor.stop_sync(&mut *term),
                false => {
                    scanner.feed(bytes, |command| {
                        if let Some(folder) = reported_folder(command) {
                            reader.tell(Happened::Folder(folder));
                        }
                    });
                    processor.advance(&mut *term, bytes);
                }
            }
            drop(term);
            reader.wake();
            processor.sync_timeout().sync_timeout()
        };
        let ended = happened.clone();
        let exited = move |code: Option<u32>| {
            let _ = ended.try_send(Happened::Exited(code));
        };
        let folder = folder.filter(|folder| folder.is_dir());
        let start = ConsoleStart {
            command_line: &plan.command_line,
            folder: folder.as_deref(),
            environment: &plan.environment,
        };
        let console = Console::open(&start, size.console(), output, exited)?;
        let writer = console.writer();
        {
            let mut answers = answers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            writer.write(std::mem::take(&mut answers.waiting));
            answers.writer = Some(writer.clone());
        }
        let session = Self {
            term,
            console,
            writer,
            plan,
            woken,
        };
        Ok((session, heard))
    }

    /// Writes `bytes` to the shell, as typed.
    pub fn write(&self, bytes: Vec<u8>) {
        self.writer.write(bytes);
    }

    /// Makes the terminal and the console `size` large.
    pub fn resize(&self, size: GridSize) {
        self.term.lock_unfair().resize(size);
        self.console.resize(size.console());
    }

    /// The number of the shell's process.
    pub fn shell_id(&self) -> u32 {
        self.console.shell_id()
    }

    /// Notes that what was new has been drawn, so that the next output
    /// wakes the window again.
    pub fn drawn(&self) {
        self.woken.store(false, Ordering::Release);
    }
}
