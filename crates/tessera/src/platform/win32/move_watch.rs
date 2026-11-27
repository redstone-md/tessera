// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! When the user begins and ends moving or resizing a window of another
//! program, by its title bar, its border or the keyboard, and the frames
//! it has while it is resized.
//!
//! Windows reports all of it through the message queue of the thread that
//! asked for them. That thread is one of its own here, which does nothing
//! else, so that nothing the shell draws holds the reports up and no other
//! part of the platform code has to be changed for them. Every part of
//! Tessera that subscribes gets every report.
//!
//! The changes of place of a window are asked for only between its start
//! and its end, and only of that window: the hook for them is set when
//! the window begins to be moved or resized and taken away when it ends.
//! A change that keeps the size, as moving does, is not reported, and the
//! sizes are reported at most sixty times a second, the last one always.

use std::cell::RefCell;
use std::sync::{Mutex, Once, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, DispatchMessageW, EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_MOVESIZEEND,
    EVENT_SYSTEM_MOVESIZESTART, GetMessageW, GetWindowThreadProcessId, KillTimer, MSG,
    OBJID_WINDOW, SetTimer, TranslateMessage, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

use super::{AppWindowId, ScreenRect};

/// The least time between two frames reported of a window being resized:
/// a sixtieth of a second.
const BETWEEN_FRAMES: Duration = Duration::from_micros(16_667);

/// The beginning or the end of a window being moved or resized, and the
/// frames it has while it is resized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveEvent {
    Start(AppWindowId),
    End(AppWindowId),
    /// The window being resized has this frame now, what is seen of it on
    /// the screen; only between its Start and its End, at most 60 times a
    /// second, and only when its size changed.
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
    if object != OBJID_WINDOW.0 || child != CHILDID_SELF as i32 || hwnd.is_invalid() {
        return;
    }
    let id = AppWindowId::from_raw(hwnd.0 as isize);
    match event {
        EVENT_SYSTEM_MOVESIZESTART => {
            SUBSCRIBERS.publish(MoveEvent::Start(id));
            Sizing::begin(id);
        }
        EVENT_SYSTEM_MOVESIZEEND => {
            Sizing::end(id);
            SUBSCRIBERS.publish(MoveEvent::End(id));
        }
        _ => {}
    }
}

/// Which sizes of a window being resized are reported, and when: a size
/// that is new, no sooner than [`BETWEEN_FRAMES`] after the one before;
/// one that comes sooner waits and is reported once the time has come,
/// unless the size went back meanwhile.
#[derive(Debug)]
struct Pace {
    /// The width and height last reported, or the window had at its start.
    size: (i32, i32),
    /// When a frame was reported last.
    reported: Option<Instant>,
    /// A frame of a new size that waits to be reported.
    waiting: Option<ScreenRect>,
}

impl Pace {
    fn new(frame: ScreenRect) -> Self {
        Self {
            size: extent(frame),
            reported: None,
            waiting: None,
        }
    }

    /// Takes in that the window has `frame` at `now`; the frame to report
    /// now is returned.
    fn see(&mut self, frame: ScreenRect, now: Instant) -> Option<ScreenRect> {
        if extent(frame) == self.size {
            self.waiting = None;
            return None;
        }
        self.waiting = Some(frame);
        self.due(now)
    }

    /// The frame that waits, if its time has come at `now`.
    fn due(&mut self, now: Instant) -> Option<ScreenRect> {
        let frame = self.waiting?;
        let soon =
            { self.reported }.is_some_and(|reported| now.duration_since(reported) < BETWEEN_FRAMES);
        if soon {
            return None;
        }
        self.waiting = None;
        self.reported = Some(now);
        self.size = extent(frame);
        Some(frame)
    }

    /// How long until the frame that waits is due, if one waits.
    fn wait(&self, now: Instant) -> Option<Duration> {
        self.waiting?;
        let since =
            { self.reported }.map_or(BETWEEN_FRAMES, |reported| now.duration_since(reported));
        Some(BETWEEN_FRAMES.saturating_sub(since))
    }
}

/// The width and height of `rect`.
fn extent(rect: ScreenRect) -> (i32, i32) {
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// The window being moved or resized, as the thread that watches knows
/// it; its hook and its timer belong to that thread.
struct Sizing {
    id: AppWindowId,
    hook: HWINEVENTHOOK,
    pace: Pace,
    /// The timer that reports a frame that waits, while there is one.
    timer: usize,
}

thread_local! {
    /// The window whose changes of place are being watched, on the thread
    /// that watches.
    static SIZING: RefCell<Option<Sizing>> = const { RefCell::new(None) };
}

impl Sizing {
    /// Begins to watch the changes of place of the window `id`, which
    /// begins to be moved or resized, and of nothing else.
    fn begin(id: AppWindowId) {
        Self::end_any();
        let Some(frame) = id.frame() else {
            return;
        };
        let mut process = 0;
        // SAFETY: the call only reads to which thread and process the
        // window belongs; one that is gone gives 0.
        let thread = unsafe { GetWindowThreadProcessId(id.hwnd(), Some(&raw mut process)) };
        if thread == 0 {
            return;
        }
        // SAFETY: `located` is a function of this program, which is never
        // unloaded; no module is injected anywhere.
        let hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                None,
                Some(located),
                process,
                thread,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if hook.is_invalid() {
            return;
        }
        let sizing = Self {
            id,
            hook,
            pace: Pace::new(frame),
            timer: 0,
        };
        SIZING.with(|watched| *watched.borrow_mut() = Some(sizing));
    }

    /// Stops watching the window `id`, which is no longer moved or
    /// resized, after reporting the size that still waited.
    fn end(id: AppWindowId) {
        let ended = SIZING.with(|watched| {
            let mut watched = watched.borrow_mut();
            match watched.as_ref() {
                Some(sizing) if sizing.id == id => watched.take(),
                _ => None,
            }
        });
        if let Some(mut sizing) = ended {
            if let Some(frame) = sizing.pace.waiting.take() {
                SUBSCRIBERS.publish(MoveEvent::Resizing { id, frame });
            }
            sizing.release();
        }
    }

    /// Stops watching whatever window was watched, as when another one
    /// begins to be moved without the end of the first having come.
    fn end_any() {
        if let Some(sizing) = SIZING.with(|watched| watched.borrow_mut().take()) {
            sizing.release();
        }
    }

    /// Takes the hook and the timer away.
    fn release(self) {
        // SAFETY: the hook and the timer were made on this thread, which
        // is the one that takes them away; a timer of 0 is none.
        unsafe {
            let _ = UnhookWinEvent(self.hook);
            if self.timer != 0 {
                let _ = KillTimer(None, self.timer);
            }
        }
    }

    /// Reports `frame` of the window, now or once its time has come; with
    /// no frame, the one that waits if it is due.
    fn take(&mut self, frame: Option<ScreenRect>, now: Instant) {
        let report = match frame {
            Some(frame) => self.pace.see(frame, now),
            None => self.pace.due(now),
        };
        if let Some(frame) = report {
            SUBSCRIBERS.publish(MoveEvent::Resizing { id: self.id, frame });
        }
        match self.pace.wait(now) {
            Some(wait) => {
                let millis = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX).max(1);
                // SAFETY: a timer of this thread, with a function of this
                // program; giving the id of the timer there is renews it.
                self.timer = unsafe { SetTimer(None, self.timer, millis, Some(tick)) };
            }
            None if self.timer != 0 => {
                // SAFETY: the timer was made on this thread.
                unsafe {
                    let _ = KillTimer(None, self.timer);
                }
                self.timer = 0;
            }
            None => {}
        }
    }
}

unsafe extern "system" fn located(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if object != OBJID_WINDOW.0 || child != CHILDID_SELF as i32 {
        return;
    }
    let id = AppWindowId::from_raw(hwnd.0 as isize);
    SIZING.with(|watched| {
        let mut watched = watched.borrow_mut();
        if let Some(sizing) = watched.as_mut().filter(|sizing| sizing.id == id) {
            sizing.take(id.frame(), Instant::now());
        }
    });
}

unsafe extern "system" fn tick(_hwnd: HWND, _message: u32, timer: usize, _time: u32) {
    SIZING.with(|watched| {
        let mut watched = watched.borrow_mut();
        match watched.as_mut() {
            Some(sizing) if sizing.timer == timer => sizing.take(None, Instant::now()),
            // SAFETY: a timer of this thread that nothing waits for any
            // more.
            _ => unsafe {
                let _ = KillTimer(None, timer);
            },
        }
    });
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

    #[test]
    fn only_new_sizes_are_reported_and_at_most_sixty_times_a_second() {
        let rect = |right: i32, bottom: i32| ScreenRect {
            left: 0,
            top: 0,
            right,
            bottom,
        };
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);
        let mut pace = Pace::new(rect(800, 600));
        // A move keeps the size and is not reported.
        let moved = ScreenRect {
            left: 10,
            right: 810,
            ..rect(800, 600)
        };
        assert_eq!(pace.see(moved, at(0)), None);
        assert_eq!(pace.wait(at(0)), None);
        assert_eq!(pace.see(rect(820, 600), at(1)), Some(rect(820, 600)));
        // Too soon: it waits, and the latest of those that came is due.
        assert_eq!(pace.see(rect(830, 600), at(5)), None);
        assert_eq!(pace.see(rect(840, 600), at(9)), None);
        assert!(
            pace.wait(at(9))
                .is_some_and(|wait| wait <= Duration::from_millis(9))
        );
        assert_eq!(pace.due(at(12)), None);
        assert_eq!(pace.due(at(18)), Some(rect(840, 600)));
        assert_eq!(pace.wait(at(18)), None);
        // A size that went back to the one reported is not reported.
        assert_eq!(pace.see(rect(850, 600), at(20)), None);
        assert_eq!(pace.see(rect(840, 600), at(22)), None);
        assert_eq!(pace.due(at(60)), None);
    }
}
