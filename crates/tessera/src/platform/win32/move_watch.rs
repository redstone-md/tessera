// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! When the user begins and ends moving or resizing a window of another
//! program, by its title bar, its border or the keyboard.
//!
//! Windows reports both through the message queue of the thread that asked
//! for them. That thread is one of its own here, which does nothing else,
//! so that nothing the shell draws holds the reports up and no other part
//! of the platform code has to be changed for them.

use std::sync::{Once, OnceLock};
use std::thread;

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZESTART, GetMessageW, MSG,
    OBJID_WINDOW, TranslateMessage, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

use super::AppWindowId;

/// The beginning or the end of a window being moved or resized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveEvent {
    Start(AppWindowId),
    End(AppWindowId),
}

/// Reports the windows of other programs that the user moves or resizes.
pub struct MoveWatch;

fn channel() -> &'static (Sender<MoveEvent>, Receiver<MoveEvent>) {
    static CHANNEL: OnceLock<(Sender<MoveEvent>, Receiver<MoveEvent>)> = OnceLock::new();
    CHANNEL.get_or_init(async_channel::unbounded)
}

impl MoveWatch {
    /// Where the beginnings and ends arrive, in the order they happen. The
    /// thread that watches is started the first time this is asked for.
    /// Every event goes to one receiver, so there is to be one that takes
    /// them.
    pub fn events() -> Receiver<MoveEvent> {
        static STARTED: Once = Once::new();
        STARTED.call_once(|| {
            let started = thread::Builder::new()
                .name("tessera move watch".to_owned())
                .spawn(watch);
            if let Err(error) = started {
                report!("windows that are moved are not seen: {error}");
            }
        });
        channel().1.clone()
    }
}

/// Asks Windows for the reports and hands them on, for as long as the
/// process runs.
fn watch() {
    let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
    // SAFETY: `moved` is a function of this program, which is never
    // unloaded; no module is injected anywhere.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_MOVESIZESTART,
            EVENT_SYSTEM_MOVESIZEEND,
            None,
            Some(moved),
            0,
            0,
            flags,
        )
    };
    if hook.is_invalid() {
        report!("Windows does not report windows that are moved, so they are not snapped");
        return;
    }
    let mut message = MSG::default();
    // SAFETY: `message` outlives every call; the loop only hands the
    // messages of this thread on to where they belong.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

unsafe extern "system" fn moved(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if object != OBJID_WINDOW.0 || child != 0 || hwnd.is_invalid() {
        return;
    }
    let id = AppWindowId::from_raw(hwnd.0 as isize);
    let happened = match event {
        EVENT_SYSTEM_MOVESIZESTART => MoveEvent::Start(id),
        EVENT_SYSTEM_MOVESIZEEND => MoveEvent::End(id),
        _ => return,
    };
    // The channel has no bound, and nobody listening is no error.
    let _ = channel().0.try_send(happened);
}
