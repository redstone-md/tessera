// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! When the user begins and ends moving or resizing a window of another
//! program, by its title bar, its border or the keyboard.
//!
//! Windows reports both through the message queue of the thread that asked
//! for them. That thread is one of its own here, which does nothing else,
//! so that nothing the shell draws holds the reports up and no other part
//! of the platform code has to be changed for them. Every part of Tessera
//! that subscribes gets every report.

use std::sync::{Mutex, Once, PoisonError};
use std::thread;

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZESTART, GetMessageW, MSG,
    OBJID_WINDOW, TranslateMessage, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

use super::{AppWindowId, ScreenRect};

/// The beginning or the end of a window being moved or resized, and the
/// frames it has while it is resized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveEvent {
    Start(AppWindowId),
    End(AppWindowId),
    /// The window being resized has this frame now; only between its Start
    /// and its End, at most 60 times a second. Fired by edges-snap's hook.
    #[allow(dead_code, reason = "fired once edges-snap watches the resizing")]
    Resizing {
        id: AppWindowId,
        frame: ScreenRect,
    },
}

/// Reports the windows of other programs that the user moves or resizes.
pub struct MoveWatch;

/// Hands each report to every receiver there still is.
struct Fanout<T> {
    subscribers: Mutex<Vec<Sender<T>>>,
}

impl<T: Clone> Fanout<T> {
    const fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
        }
    }

    /// A receiver of every report from now on.
    fn subscribe(&self) -> Receiver<T> {
        let (sender, receiver) = async_channel::unbounded();
        { self.subscribers.lock() }
            .unwrap_or_else(PoisonError::into_inner)
            .push(sender);
        receiver
    }

    /// Gives `event` to every receiver; one that was dropped is forgotten.
    fn publish(&self, event: T) {
        { self.subscribers.lock() }
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|subscriber| subscriber.try_send(event.clone()).is_ok());
    }
}

static SUBSCRIBERS: Fanout<MoveEvent> = Fanout::new();

impl MoveWatch {
    /// A receiver of every beginning and end from now on, in the order
    /// they happen; each caller gets its own. The thread that watches is
    /// started the first time one is asked for.
    pub fn subscribe() -> Receiver<MoveEvent> {
        static STARTED: Once = Once::new();
        STARTED.call_once(|| {
            let started = thread::Builder::new()
                .name("tessera move watch".to_owned())
                .spawn(watch);
            if let Err(error) = started {
                report!("windows that are moved are not seen: {error}");
            }
        });
        SUBSCRIBERS.subscribe()
    }

    /// The same as [`MoveWatch::subscribe`].
    pub fn events() -> Receiver<MoveEvent> {
        Self::subscribe()
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
        report!(
            "Windows does not report windows that are moved, so they are neither snapped nor sized as tiles"
        );
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
    SUBSCRIBERS.publish(happened);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_subscriber_gets_every_report() {
        let fanout = Fanout::new();
        let (first, second) = (fanout.subscribe(), fanout.subscribe());
        let window = AppWindowId::from_raw(7);
        fanout.publish(MoveEvent::Start(window));
        fanout.publish(MoveEvent::End(window));
        for receiver in [&first, &second] {
            assert_eq!(receiver.try_recv(), Ok(MoveEvent::Start(window)));
            assert_eq!(receiver.try_recv(), Ok(MoveEvent::End(window)));
            assert!(receiver.try_recv().is_err());
        }
        // A receiver that is gone is forgotten; the other still hears.
        drop(first);
        fanout.publish(MoveEvent::Start(window));
        assert_eq!(second.try_recv(), Ok(MoveEvent::Start(window)));
        assert_eq!(fanout.subscribers.lock().unwrap().len(), 1);
    }
}
