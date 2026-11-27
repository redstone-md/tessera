// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The guard against windows that take the foreground by themselves, as
//! KWin's focus stealing prevention: when a window comes in front that
//! the user's input did not bring forward, how strictly its rules hold it
//! decides whether it stays there. `low` lets it; `medium` and `high` give
//! the foreground back to the window that had it and flash the newcomer
//! in the taskbar; `extreme` lets it in front only when it was clicked.
//!
//! Input counts as the user's when it came at most half a second ago and
//! the last click went to the program of the newcomer, or the window in
//! front before it, which the keys went to, was of that program; at
//! `medium`, also when the program of the newcomer was started at most ten
//! seconds after the user gave Tessera's launcher, dock or Start menu
//! input, which is how a program started there comes in front.
//!
//! The guard hears of the window in front with a hook of window events on
//! a thread of its own that pumps its messages. That thread has no right
//! of its own to the foreground, which the newcomer now holds; it gives
//! the foreground back by attaching its input to the newcomer's thread for
//! the moment of `SetForegroundWindow`, and where Windows still refuses,
//! with a press of Alt sent just before, which gives the calling process
//! the last input. Whether Windows always lets it is not known until it
//! is tried on Windows.

use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::{Mutex, Once, PoisonError};
use std::thread;

use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND};
use windows::Win32::System::SystemInformation::{GetSystemTimeAsFileTime, GetTickCount};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, GetProcessTimes, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetLastInputInfo, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, LASTINPUTINFO, SendInput, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_SYSTEM_FOREGROUND, FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO,
    FlashWindowEx, GA_ROOT, GetAncestor, GetMessageW, GetWindowThreadProcessId, IsWindowVisible,
    MSG, SetForegroundWindow, TranslateMessage, WINEVENT_OUTOFCONTEXT,
};

use super::{AppWindowId, MouseHook};

/// How strictly a window is kept from taking the foreground by itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FocusLevel {
    /// Windows' own rules only.
    #[default]
    None,
    /// The window is let in front.
    Low,
    /// The window is let in front after the user's input to it, or when
    /// its program was just started from Tessera.
    Medium,
    /// The window is let in front only after the user's input to it.
    High,
    /// The window is let in front only when it was clicked.
    Extreme,
}

impl FocusLevel {
    /// Every level, in the order they are offered.
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Extreme,
    ];

    /// The word a rule writes for it.
    pub fn word(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Extreme => "extreme",
        }
    }

    /// Whether the guard ever gives the foreground back for it.
    fn holds(self) -> bool {
        self >= Self::Medium
    }
}

/// How strictly the window that came in front is held, as the rules say.
pub type FocusJudge = Box<dyn Fn(AppWindowId) -> FocusLevel + Send>;

/// How long ago input may have come and still have brought a window in
/// front, in milliseconds.
const INPUT_COUNTS: u32 = 500;

/// How long after input to Tessera a program may have been started and
/// still count as started there, in milliseconds.
const STARTED_COUNTS: u32 = 10_000;

/// The judge of the windows that come in front, while there is one.
static JUDGE: Mutex<Option<FocusJudge>> = Mutex::new(None);
/// The window that was in front before the one in front now.
static IN_FRONT: AtomicIsize = AtomicIsize::new(0);
/// The window the guard gave the foreground back to, whose coming in front
/// is no newcomer's.
static GIVEN_BACK: AtomicIsize = AtomicIsize::new(0);
/// When the user last gave input while a window of Tessera was in front,
/// in the milliseconds of `GetTickCount`; 0 for never.
static OWN_INPUT: AtomicU32 = AtomicU32::new(0);

/// What is known when a window comes in front.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Arrival {
    /// Now, and when the user last gave input, in the milliseconds of
    /// `GetTickCount`.
    pub now: u32,
    pub last_input: u32,
    /// The program the last click went to, and when.
    pub click: Option<(u32, u32)>,
    /// The program of the window that comes in front, and of the window
    /// that was in front before it.
    pub newcomer: u32,
    pub before: u32,
    /// When the program of the newcomer was started.
    pub started: Option<u32>,
    /// When the user last gave Tessera input.
    pub own_input: Option<u32>,
}

/// Whether `arrival` is let in front at `level`.
pub fn lets_in(level: FocusLevel, arrival: &Arrival) -> bool {
    if !level.holds() {
        return true;
    }
    let ago = |then: u32| arrival.now.wrapping_sub(then);
    let clicked = { arrival.click }
        .is_some_and(|(process, at)| process == arrival.newcomer && ago(at) <= INPUT_COUNTS);
    if level == FocusLevel::Extreme {
        return clicked;
    }
    let typed = ago(arrival.last_input) <= INPUT_COUNTS && arrival.before == arrival.newcomer;
    if clicked || typed {
        return true;
    }
    if level == FocusLevel::High {
        return false;
    }
    // Started at most ten seconds after input to Tessera, and not before it.
    arrival
        .own_input
        .zip(arrival.started)
        .is_some_and(|(input, started)| started.wrapping_sub(input) <= STARTED_COUNTS)
}

/// When something that happened `ago` hundreds of nanoseconds before `now`
/// (as Windows counts file times) happened, in the milliseconds of
/// `GetTickCount`, which is `tick` now.
pub fn tick_of(then: u64, now: u64, tick: u32) -> u32 {
    let ago = now.saturating_sub(then) / 10_000;
    tick.wrapping_sub(ago.min(u64::from(u32::MAX)) as u32)
}

/// Guards the foreground.
pub struct FocusGuard;

impl FocusGuard {
    /// Holds from now on each window that comes in front as `judge` says,
    /// or no window with `None`. Clicks are watched for while there is a
    /// judge.
    pub fn guard(judge: Option<FocusJudge>) {
        let guarding = judge.is_some();
        *JUDGE.lock().unwrap_or_else(PoisonError::into_inner) = judge;
        MouseHook::watch_clicks(guarding);
        if guarding {
            start();
        }
    }
}

/// Starts the thread of the guard, once; it waits for windows that come
/// in front for as long as Tessera runs, and asks the judge of the moment.
fn start() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let started = thread::Builder::new()
            .name("tessera focus guard".to_owned())
            .spawn(watch);
        if let Err(error) = started {
            report!("windows that take the foreground are let: {error}");
        }
    });
}

fn watch() {
    // SAFETY: `came_in_front` is a function of this program, which is
    // never unloaded; no module is injected anywhere.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(came_in_front),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    if hook.is_invalid() {
        report!("Windows does not report windows coming in front, so none is held back");
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

unsafe extern "system" fn came_in_front(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    // SAFETY: the call only reads the top window of a window.
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let root = if root.is_invalid() { hwnd } else { root };
    let newcomer = AppWindowId::from_raw(root.0 as isize);
    let before = AppWindowId::from_raw(IN_FRONT.swap(newcomer.raw(), Ordering::Relaxed));
    let own = std::process::id();
    let now = now();
    if before.raw() != 0 && before.process() == own {
        // The user worked in Tessera until now: its launcher, dock or
        // Start menu may have started what comes.
        OWN_INPUT.store(last_input().max(1), Ordering::Relaxed);
    }
    if newcomer.process() == own || before == newcomer {
        return;
    }
    if GIVEN_BACK.swap(0, Ordering::Relaxed) == newcomer.raw() {
        return;
    }
    let level = {
        let judge = JUDGE.lock().unwrap_or_else(PoisonError::into_inner);
        match judge.as_ref() {
            Some(judge) => judge(newcomer),
            None => return,
        }
    };
    if !level.holds() {
        return;
    }
    let click = MouseHook::last_click();
    let own_click = click
        .filter(|&(process, _)| process == own)
        .map(|(_, at)| at);
    let own_input = { OWN_INPUT.load(Ordering::Relaxed) };
    let own_input = [own_click, (own_input != 0).then_some(own_input)]
        .into_iter()
        .flatten()
        .max_by_key(|&at| std::cmp::Reverse(now.wrapping_sub(at)));
    let arrival = Arrival {
        now,
        last_input: last_input(),
        click,
        newcomer: newcomer.process(),
        before: before.process(),
        started: started(newcomer.process(), now),
        own_input,
    };
    if lets_in(level, &arrival) {
        return;
    }
    give_back(before, newcomer);
    flash(newcomer);
}

/// Now, in the milliseconds of `GetTickCount`.
fn now() -> u32 {
    // SAFETY: the call takes nothing and reads a counter.
    unsafe { GetTickCount() }
}

/// When the user last gave input, in the milliseconds of `GetTickCount`.
fn last_input() -> u32 {
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `info` is as large as the call is told and outlives it.
    match unsafe { GetLastInputInfo(&raw mut info) }.as_bool() {
        true => info.dwTime,
        false => 0,
    }
}

/// When `process` was started, in the milliseconds of `GetTickCount`,
/// which is `tick` now; `None` where Windows does not tell.
fn started(process: u32, tick: u32) -> Option<u32> {
    let as_number =
        |time: FILETIME| u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime);
    // SAFETY: the times outlive the call, and the handle is closed before
    // returning.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process).ok()?;
        let (mut created, mut ended, mut kernel, mut user) = Default::default();
        let read = GetProcessTimes(
            handle,
            &raw mut created,
            &raw mut ended,
            &raw mut kernel,
            &raw mut user,
        );
        let _ = CloseHandle(handle);
        read.ok()?;
        Some(tick_of(
            as_number(created),
            as_number(GetSystemTimeAsFileTime()),
            tick,
        ))
    }
}

/// Gives the foreground back to `before`, the window that had it before
/// `newcomer` took it.
fn give_back(before: AppWindowId, newcomer: AppWindowId) {
    let hwnd = before.hwnd();
    // SAFETY: the calls read the state of a window and the threads of
    // windows, attach this thread's input to another thread's only for the
    // moment of one call and detach it again, and send a press of Alt that
    // outlives the call; a window that is gone makes them fail.
    unsafe {
        if before.raw() == 0 || !IsWindowVisible(hwnd).as_bool() {
            return;
        }
        GIVEN_BACK.store(before.raw(), Ordering::Relaxed);
        let ours = GetCurrentThreadId();
        let theirs = GetWindowThreadProcessId(newcomer.hwnd(), None);
        let attached = theirs != 0 && AttachThreadInput(ours, theirs, true).as_bool();
        let mut given = SetForegroundWindow(hwnd).as_bool();
        if attached {
            let _ = AttachThreadInput(ours, theirs, false);
        }
        if !given {
            let alt = |flags: KEYBD_EVENT_FLAGS| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_MENU,
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            };
            let presses = [alt(KEYBD_EVENT_FLAGS(0)), alt(KEYEVENTF_KEYUP)];
            SendInput(&presses, size_of::<INPUT>() as i32);
            given = SetForegroundWindow(hwnd).as_bool();
        }
        if !given {
            GIVEN_BACK.store(0, Ordering::Relaxed);
        }
    }
}

/// Flashes the button of `window` in the taskbar until it comes in front.
fn flash(window: AppWindowId) {
    let info = FLASHWINFO {
        cbSize: size_of::<FLASHWINFO>() as u32,
        hwnd: window.hwnd(),
        dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
        uCount: 0,
        dwTimeout: 0,
    };
    // SAFETY: `info` is as large as the call is told and outlives it.
    unsafe {
        let _ = FlashWindowEx(&raw const info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROGRAM: u32 = 40;
    const OTHER: u32 = 41;

    fn arrival() -> Arrival {
        Arrival {
            now: 100_000,
            last_input: 90_000,
            click: None,
            newcomer: PROGRAM,
            before: OTHER,
            started: None,
            own_input: None,
        }
    }

    #[test]
    fn a_window_that_comes_by_itself_is_held_back() {
        let came = arrival();
        assert!(lets_in(FocusLevel::None, &came));
        assert!(lets_in(FocusLevel::Low, &came));
        for level in [FocusLevel::Medium, FocusLevel::High, FocusLevel::Extreme] {
            assert!(!lets_in(level, &came), "{level:?}");
        }
    }

    #[test]
    fn a_click_on_its_program_lets_it_in_at_every_level() {
        let clicked = Arrival {
            last_input: 99_800,
            click: Some((PROGRAM, 99_800)),
            ..arrival()
        };
        for level in FocusLevel::ALL {
            assert!(lets_in(level, &clicked), "{level:?}");
        }
        // A click long ago does not.
        let old = Arrival {
            click: Some((PROGRAM, 98_000)),
            ..arrival()
        };
        assert!(!lets_in(FocusLevel::Medium, &old));
    }

    #[test]
    fn keys_to_its_own_program_let_it_in_but_at_extreme() {
        let typed = Arrival {
            last_input: 99_900,
            before: PROGRAM,
            ..arrival()
        };
        assert!(lets_in(FocusLevel::Medium, &typed));
        assert!(lets_in(FocusLevel::High, &typed));
        assert!(!lets_in(FocusLevel::Extreme, &typed));
    }

    #[test]
    fn a_program_started_from_tessera_comes_in_at_medium() {
        let started = Arrival {
            own_input: Some(95_000),
            started: Some(96_000),
            ..arrival()
        };
        assert!(lets_in(FocusLevel::Medium, &started));
        assert!(!lets_in(FocusLevel::High, &started));
        let late = Arrival {
            started: Some(95_000 + STARTED_COUNTS + 1),
            ..started
        };
        assert!(!lets_in(FocusLevel::Medium, &late));
        let earlier = Arrival {
            started: Some(94_000),
            ..started
        };
        assert!(!lets_in(FocusLevel::Medium, &earlier));
    }

    #[test]
    fn a_file_time_is_turned_into_a_tick() {
        // Three seconds ago, in hundreds of nanoseconds.
        assert_eq!(tick_of(1_000, 1_000 + 30_000_000, 50_000), 47_000);
        // A tick counter that went round.
        assert_eq!(tick_of(0, 30_000_000, 1_000), 1_000u32.wrapping_sub(3_000));
        assert_eq!(tick_of(5, 1, 7), 7);
    }

    #[test]
    fn every_level_has_its_word() {
        for level in FocusLevel::ALL {
            let read: FocusLevel = toml::Value::String(level.word().to_owned())
                .try_into()
                .unwrap();
            assert_eq!(read, level);
        }
    }
}
