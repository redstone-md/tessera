// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Windows key pressed and released by itself, which Windows answers
//! with its Start menu unless another key went down in between.
//!
//! A low-level keyboard hook sees every key in this process, without any
//! code entering another one. It lets every key through, so combinations
//! with the Windows key stay what they are; a press of the key alone gets
//! a key nothing listens to pressed inside it, which is what keeps Windows
//! from opening its menu.
//!
//! The hook has a thread of its own: Windows passes a key on without
//! asking when the thread of a hook is busy for a fraction of a second,
//! and the thread that draws sometimes is.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread;

use anyhow::{Context, anyhow};
use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG, PostThreadMessageW,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
};

/// A key code no keyboard has and no program listens to.
const UNASSIGNED_KEY: VIRTUAL_KEY = VIRTUAL_KEY(0xe8);

/// What marks the key presses made here, so that the hook leaves them out.
pub(super) const MADE_HERE: usize = 0x5445_5353;

/// Whether the Windows key is down and nothing else was pressed since.
#[derive(Debug, Default)]
struct LonePress {
    held: bool,
    alone: bool,
}

impl LonePress {
    /// Takes in that a key went down or up: the Windows key if `windows`,
    /// with a modifier key held at that moment if `modified`. The answer
    /// is whether this ends a press of the Windows key by itself.
    fn key(&mut self, windows: bool, down: bool, modified: bool) -> bool {
        match (windows, down) {
            // A key held down repeats, which starts nothing new.
            (true, true) if !self.held => {
                self.held = true;
                self.alone = !modified;
                false
            }
            (true, true) => false,
            (true, false) => {
                let alone = self.held && self.alone;
                *self = Self::default();
                alone
            }
            (false, true) => {
                self.alone = false;
                false
            }
            (false, false) => false,
        }
    }
}

static PRESS: Mutex<LonePress> = Mutex::new(LonePress {
    held: false,
    alone: false,
});
/// The thread that has the hook, or zero while the key is with Windows.
static WATCHER: AtomicU32 = AtomicU32::new(0);
static PRESSED: OnceLock<(Sender<()>, Receiver<()>)> = OnceLock::new();

fn pressed() -> &'static (Sender<()>, Receiver<()>) {
    PRESSED.get_or_init(|| async_channel::bounded(1))
}

/// The Windows key as something Tessera answers instead of Windows.
pub struct WindowsKey;

impl WindowsKey {
    /// Where a message arrives whenever the key was pressed by itself
    /// while it is taken over.
    pub fn presses() -> Receiver<()> {
        pressed().1.clone()
    }

    /// Takes the key over if `taken`, and gives it back to Windows
    /// otherwise.
    pub fn take_over(taken: bool) -> anyhow::Result<()> {
        let watcher = WATCHER.load(Ordering::Relaxed);
        match (taken, watcher != 0) {
            (true, false) => {
                let (ready, started) = mpsc::channel();
                thread::Builder::new()
                    .name("windows key".to_owned())
                    .spawn(move || watch(&ready))
                    .context("the thread that watches the keyboard did not start")?;
                let watcher = started
                    .recv()
                    .map_err(|_| anyhow!("the thread that watches the keyboard ended"))??;
                WATCHER.store(watcher, Ordering::Relaxed);
            }
            (false, true) => {
                WATCHER.store(0, Ordering::Relaxed);
                if let Ok(mut press) = PRESS.lock() {
                    *press = LonePress::default();
                }
                // SAFETY: posting to a thread that has ended fails without
                // side effects.
                unsafe { PostThreadMessageW(watcher, WM_QUIT, WPARAM(0), LPARAM(0)) }?;
            }
            _ => {}
        }
        Ok(())
    }
}

/// Sets the hook, tells `ready` which thread this is or why there is no
/// hook, and handles what Windows sends that thread until it is told to
/// quit, when the hook is removed.
fn watch(ready: &mpsc::Sender<windows::core::Result<u32>>) {
    // SAFETY: `key_event` is a function of this program, which is never
    // unloaded; a low-level hook is called in this process and injects no
    // module anywhere.
    let hook = match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_event), None, 0) } {
        Ok(hook) => hook,
        Err(error) => {
            ready.send(Err(error)).ok();
            return;
        }
    };
    // SAFETY: the call only reads which thread this is.
    ready.send(Ok(unsafe { GetCurrentThreadId() })).ok();
    let mut message = MSG::default();
    // SAFETY: `message` outlives the calls that fill it. Windows calls the
    // hook from inside them; a failure ends the loop like a request to quit.
    while unsafe { GetMessageW(&raw mut message, None, 0, 0) }.0 > 0 {}
    // SAFETY: the handle is the one the hook was set with above.
    if let Err(error) = unsafe { UnhookWindowsHookEx(hook) } {
        report!("the keyboard is still watched: {error:#}");
    }
}

fn modifier_held() -> bool {
    [VK_SHIFT, VK_CONTROL, VK_MENU].into_iter().any(|key| {
        // SAFETY: the call only reads the state of the keyboard.
        unsafe { GetAsyncKeyState(key.0.into()) < 0 }
    })
}

/// Presses and releases the key nothing listens to. Windows takes it in
/// before the key that is being handled, so the Windows key is no longer
/// alone when it comes up.
fn press_unassigned_key() {
    let event = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: UNASSIGNED_KEY,
                dwFlags: flags,
                dwExtraInfo: MADE_HERE,
                ..Default::default()
            },
        },
    };
    let events = [event(KEYBD_EVENT_FLAGS(0)), event(KEYEVENTF_KEYUP)];
    // SAFETY: the events outlive the call, which copies them.
    unsafe {
        SendInput(&events, size_of::<INPUT>() as i32);
    }
}

unsafe extern "system" fn key_event(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for this code Windows passes a pointer to the key event,
        // which is valid for the duration of the call.
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if event.dwExtraInfo != MADE_HERE {
            let key = VIRTUAL_KEY(event.vkCode as u16);
            let windows = key == VK_LWIN || key == VK_RWIN;
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let modified = windows && down && modifier_held();
            let alone = PRESS
                .lock()
                .is_ok_and(|mut press| press.key(windows, down, modified));
            if alone {
                press_unassigned_key();
                // A press that is still waiting to be answered is enough.
                pressed().0.try_send(()).ok();
            }
        }
    }
    // SAFETY: the arguments are passed on as they came.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_by_itself_is_a_lone_press() {
        let mut press = LonePress::default();
        assert!(!press.key(true, true, false));
        assert!(!press.key(true, true, false));
        assert!(press.key(true, false, false));
        assert!(!press.key(true, false, false));
    }

    #[test]
    fn another_key_in_between_makes_it_a_combination() {
        let mut press = LonePress::default();
        press.key(true, true, false);
        press.key(false, true, false);
        press.key(false, false, false);
        assert!(!press.key(true, false, false));
        press.key(true, true, false);
        assert!(press.key(true, false, false));
    }

    #[test]
    fn a_modifier_held_before_makes_it_a_combination() {
        let mut press = LonePress::default();
        press.key(true, true, true);
        assert!(!press.key(true, false, false));
    }

    #[test]
    fn other_keys_alone_are_nothing() {
        let mut press = LonePress::default();
        assert!(!press.key(false, true, false));
        assert!(!press.key(false, false, false));
        assert!(!press.key(true, false, false));
    }
}
