// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the menu of a button of the dock offers: the documents and tasks
//! of its application, its windows, another window, starting it as an
//! administrator, its file, the window menu, pinning, and closing or
//! ending it.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{App, AsyncApp};

use super::{DockItem, FILES_APP, pin_name, repin, with, without};
use crate::actions::Actions;
use crate::i18n::Language;
use crate::menu::MenuEntry;
use crate::platform::{
    AppWindow, AppWindowId, JumpTask, Launch, RecentDocument, RunningProcess, Shortcut,
    app_id_of_program, reveal_in_explorer,
};

/// How many of the documents an application opened lately its menu
/// offers.
const RECENT_SHOWN: u32 = 5;

/// How long End task waits for a window to close before it ends its
/// process: as long as Windows waits before it calls a window not
/// responding.
const HUNG_WAIT: Duration = Duration::from_secs(5);

const ICON_CLOSE: &str = "\u{e8bb}";
const ICON_DOCUMENT: &str = "\u{e8a5}";
const ICON_TASK: &str = "\u{e945}";
const ICON_NEW_WINDOW: &str = "\u{e78b}";
const ICON_PIN: &str = "\u{e718}";
const ICON_UNPIN: &str = "\u{e77a}";
const ICON_WINDOW: &str = "\u{e7c4}";
const ICON_ADMIN: &str = "\u{e7ef}";
const ICON_FOLDER: &str = "\u{e838}";
const ICON_WINDOW_MENU: &str = "\u{e700}";
const ICON_END: &str = "\u{e711}";
const ICON_KILL: &str = "\u{e7ba}";

/// The documents the application of `item` opened lately, each to be
/// opened again by it, and then the tasks it put in its jump list.
fn recent_entries(item: &DockItem) -> Vec<MenuEntry> {
    let window = item.front();
    let pin = item.pin.as_ref();
    let packaged = window
        .and_then(|window| window.app_id.clone())
        .or_else(|| pin.and_then(|pin| pin.app.clone()));
    // A packaged application has no program file to hand a document to.
    let program = match packaged {
        Some(_) => None,
        None => window
            .and_then(|window| window.executable.clone())
            .or_else(|| pin.and_then(|pin| pin.program.clone())),
    };
    let app = packaged
        .or_else(|| window.and_then(|window| window.id.stated_app_id()))
        .or_else(|| program.as_deref().map(app_id_of_program));
    let Some(app) = app else {
        return Vec::new();
    };
    let documents = RecentDocument::of_app(&app, RECENT_SHOWN);
    let documents = documents.into_iter().map(|document| {
        let program = program.clone();
        let label = document.name.clone();
        MenuEntry::new(ICON_DOCUMENT, label, move |_| match &program {
            Some(program) => document.open_with(program),
            None => Shortcut {
                name: document.name.clone(),
                path: document.path.clone(),
            }
            .launch(),
        })
    });
    let tasks = JumpTask::of_app(&app).into_iter().map(|task| {
        let label = task.name.clone();
        MenuEntry::new(ICON_TASK, label, move |_| task.start())
    });
    documents.chain(tasks).collect()
}

/// What starts another window of the application of `item`: the pinned
/// shortcut, or the program of its window in front. `None` for the file
/// manager, which is Tessera, and for a window nothing is known of.
pub(super) fn another(item: &DockItem) -> Option<Shortcut> {
    if let Some(pin) = &item.pin {
        return Some(pin.shortcut.clone());
    }
    let front = item.front().filter(|window| !window.is_own())?;
    let executable = front.program()?;
    Some(Shortcut {
        name: pin_name(&executable),
        path: executable,
    })
}

/// The file the application of `item` is started from, as Open file
/// location shows it: the pinned shortcut, or else the program file.
/// `None` for a packaged application and the file manager.
fn file_of(item: &DockItem) -> Option<PathBuf> {
    if item.is_files() {
        return None;
    }
    if let Some(pin) = &item.pin {
        return pin.app.is_none().then(|| pin.shortcut.path.clone());
    }
    let front = item.front()?;
    match front.app_id {
        Some(_) => None,
        None => front.executable.clone(),
    }
}

/// The processes of the windows of `item`, without Tessera's own.
fn processes(windows: &[AppWindow]) -> Vec<u32> {
    let own = std::process::id();
    let set: BTreeSet<u32> = { windows.iter() }
        .map(|window| window.id.process())
        .filter(|&process| process != 0 && process != own)
        .collect();
    set.into_iter().collect()
}

/// Ends the processes `processes` at once.
fn kill(processes: Vec<u32>, cx: &mut App) {
    cx.background_executor()
        .spawn(async move {
            for process in processes {
                if let Err(error) = RunningProcess::kill(process) {
                    report!("{error:#}");
                }
            }
        })
        .detach();
}

/// Asks `windows` to close, and ends the processes of those that are
/// still there after [`HUNG_WAIT`], as End task of Windows does.
fn end_task(windows: Vec<AppWindowId>, cx: &mut App) {
    for id in &windows {
        id.close();
    }
    cx.spawn(async move |cx: &mut AsyncApp| {
        cx.background_executor().timer(HUNG_WAIT).await;
        let own = std::process::id();
        let left: BTreeSet<u32> = { windows.iter() }
            .filter(|id| id.exists())
            .map(|id| id.process())
            .filter(|&process| process != 0 && process != own)
            .collect();
        if left.is_empty() {
            return;
        }
        cx.update(|cx| kill(left.into_iter().collect(), cx)).ok();
    })
    .detach();
}

/// What the menu of `item` offers: the documents its application opened
/// lately and its tasks, its windows if it has several, another window,
/// starting it as an administrator, its file, the window menu of its
/// window, pinning or unpinning it, closing its windows, and ending its
/// task or its process.
pub(super) fn entries(item: &DockItem, cx: &App) -> Vec<MenuEntry> {
    let language = Language::of(cx);
    let say = |text: &'static str| language.say(text);
    let mut entries = recent_entries(item);
    if item.windows.len() > 1 {
        entries.extend(item.windows.iter().map(|window| {
            let id = window.id;
            MenuEntry::new(ICON_WINDOW, id.title(), move |_| id.activate())
        }));
    }
    let runs = item.front().is_some();
    if item.is_files() || another(item).is_some() {
        let label = match runs {
            true => say("New window"),
            false => say("Open"),
        };
        let starts = StartAnother::of(item);
        entries.push(MenuEntry::new(ICON_NEW_WINDOW, label, move |cx| {
            starts.start(cx);
        }));
    }
    if let Some(file) = file_of(item) {
        let elevated = file.clone();
        entries.push(MenuEntry::new(
            ICON_ADMIN,
            say("Run as administrator"),
            move |_| {
                Launch::new(&elevated)
                    .verb("runas")
                    .named(format!("`{}`", elevated.display()))
                    .start();
            },
        ));
        entries.push(MenuEntry::new(
            ICON_FOLDER,
            say("Open file location"),
            move |cx| {
                let file = file.clone();
                cx.background_executor()
                    .spawn(async move {
                        if let Err(error) = reveal_in_explorer(&file) {
                            report!(
                                "`{}` was not shown in its folder: {error:#}",
                                file.display()
                            );
                        }
                    })
                    .detach();
            },
        ));
    }
    if let Some(front) = item.front().map(|window| window.id) {
        entries.push(MenuEntry::new(
            ICON_WINDOW_MENU,
            say("Window menu"),
            move |cx| {
                // The menu of the dock closes first.
                cx.defer(move |cx| crate::window_menu::open(front, cx));
            },
        ));
    }
    let executable = item
        .front()
        .filter(|window| !window.is_own())
        .and_then(AppWindow::program);
    match (&item.pin, executable) {
        (Some(pin), _) => {
            let name = pin.shortcut.name.clone();
            entries.push(MenuEntry::new(ICON_UNPIN, say("Unpin"), move |cx| {
                repin(cx, |pins| without(pins, &name));
            }));
        }
        (None, Some(executable)) => {
            let name = pin_name(&executable);
            entries.push(MenuEntry::new(
                ICON_PIN,
                say("Pin to the dock"),
                move |cx| {
                    repin(cx, |pins| with(pins, &name));
                },
            ));
        }
        (None, None) => {}
    }
    let ids: Vec<AppWindowId> = item.windows.iter().map(|window| window.id).collect();
    match ids.as_slice() {
        [] => {}
        &[id] => entries.push(MenuEntry::new(ICON_CLOSE, say("Close window"), move |_| {
            id.close();
        })),
        _ => {
            let all = ids.clone();
            entries.push(MenuEntry::new(
                ICON_CLOSE,
                say("Close all windows"),
                move |_| {
                    for id in &all {
                        id.close();
                    }
                },
            ));
        }
    }
    let processes = processes(&item.windows);
    if !processes.is_empty() {
        let windows = ids;
        entries.push(MenuEntry::new(ICON_END, say("End task"), move |cx| {
            end_task(windows.clone(), cx);
        }));
        entries.push(MenuEntry::new(ICON_KILL, say("Kill process"), move |cx| {
            kill(processes.clone(), cx);
        }));
    }
    entries
}

/// What starts another window of an application, kept for a menu entry
/// or a key after the button it was read from changed.
pub(super) enum StartAnother {
    Files,
    Shortcut(Shortcut),
    Nothing,
}

impl StartAnother {
    pub(super) fn of(item: &DockItem) -> Self {
        if item.is_files() {
            return Self::Files;
        }
        match another(item) {
            Some(shortcut) => Self::Shortcut(shortcut),
            None => Self::Nothing,
        }
    }

    pub(super) fn start(&self, cx: &mut App) {
        match self {
            Self::Files => Actions::run("files", None, cx),
            Self::Shortcut(shortcut) => shortcut.launch(),
            Self::Nothing => {}
        }
    }
}

/// Whether `window` is one of the file manager of Tessera.
pub(super) fn is_files(window: &AppWindow) -> bool {
    window.is_own() && window.app_id.as_deref() == Some(FILES_APP)
}
