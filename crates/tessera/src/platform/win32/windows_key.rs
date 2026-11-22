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
//! The same hook takes the key combinations Windows keeps to itself, such
//! as Alt+Tab, which it gives to no program that asks for them: the key
//! that completes one is not passed on, and the combination is reported.
//!
//! The hook has a thread of its own: Windows passes a key on without
//! asking when the thread of a hook is busy for a fraction of a second,
//! and the thread that draws sometimes is.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread;

use anyhow::{Context, anyhow};
use async_channel::{Receiver, Sender};

use super::hotkeys::{Hotkey, Modifiers};
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

    /// Takes in that a key went down that is passed on to no one.
    fn joined(&mut self) {
        self.alone = false;
    }
}

static PRESS: Mutex<LonePress> = Mutex::new(LonePress {
    held: false,
    alone: false,
});
/// Whether the Windows key is taken over.
static WINDOWS_KEY: AtomicBool = AtomicBool::new(false);
/// The combinations taken from Windows.
static TAKEN: Mutex<Vec<Hotkey>> = Mutex::new(Vec::new());
/// The key of a taken combination that went down and was not passed on,
/// so that its going up is not passed on either; zero when there is none.
static SWALLOWED: AtomicU32 = AtomicU32::new(0);
/// Whether the keys of [`ChoiceKey`] are taken, whatever else is held.
static CHOOSING: AtomicBool = AtomicBool::new(false);
/// The thread that has the hook, or zero while there is no hook.
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
        WINDOWS_KEY.store(taken, Ordering::Relaxed);
        if !taken && let Ok(mut press) = PRESS.lock() {
            *press = LonePress::default();
        }
        watch_while_needed()
    }
}

fn taken_presses() -> &'static (Sender<u32>, Receiver<u32>) {
    static PRESSED: OnceLock<(Sender<u32>, Receiver<u32>)> = OnceLock::new();
    PRESSED.get_or_init(|| async_channel::bounded(8))
}

/// A key that moves through a choice on the screen, confirms it or calls
/// it off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceKey {
    Escape,
    Enter,
    Left,
    Up,
    Right,
    Down,
}

impl ChoiceKey {
    fn of(key: u16) -> Option<Self> {
        Some(match key {
            0x1B => Self::Escape,
            0x0D => Self::Enter,
            0x25 => Self::Left,
            0x26 => Self::Up,
            0x27 => Self::Right,
            0x28 => Self::Down,
            _ => return None,
        })
    }
}

fn choice_presses() -> &'static (Sender<ChoiceKey>, Receiver<ChoiceKey>) {
    static PRESSED: OnceLock<(Sender<ChoiceKey>, Receiver<ChoiceKey>)> = OnceLock::new();
    PRESSED.get_or_init(|| async_channel::bounded(8))
}

/// Key combinations Windows keeps to itself, taken from it.
pub struct TakenKeys;

impl TakenKeys {
    /// Where the number of a taken combination arrives whenever it is
    /// pressed.
    pub fn presses() -> Receiver<u32> {
        taken_presses().1.clone()
    }

    /// Where a key of [`ChoiceKey`] arrives whenever it is pressed while
    /// those are taken.
    pub fn choice_presses() -> Receiver<ChoiceKey> {
        choice_presses().1.clone()
    }

    /// Takes the keys of [`ChoiceKey`] from every program if `taken`,
    /// whatever else is held, and gives them back otherwise. They are to
    /// be taken only while a choice is on the screen.
    pub fn take_choice(taken: bool) -> anyhow::Result<()> {
        CHOOSING.store(taken, Ordering::Relaxed);
        watch_while_needed()
    }

    /// Takes the combinations `keys` from Windows, and gives back the ones
    /// taken before that are not among them.
    pub fn set(keys: Vec<Hotkey>) -> anyhow::Result<()> {
        match TAKEN.lock() {
            Ok(mut taken) => *taken = keys,
            Err(_) => return Err(anyhow!("the taken keys are out of reach")),
        }
        watch_while_needed()
    }
}

/// Sets the hook while something needs it and removes it otherwise.
fn watch_while_needed() -> anyhow::Result<()> {
    let needed = WINDOWS_KEY.load(Ordering::Relaxed)
        || CHOOSING.load(Ordering::Relaxed)
        || TAKEN.lock().is_ok_and(|taken| !taken.is_empty());
    let watcher = WATCHER.load(Ordering::Relaxed);
    match (needed, watcher != 0) {
        (true, false) => {
            let (ready, started) = mpsc::channel();
            thread::Builder::new()
                .name("keyboard".to_owned())
                .spawn(move || watch(&ready))
                .context("the thread that watches the keyboard did not start")?;
            let watcher = started
                .recv()
                .map_err(|_| anyhow!("the thread that watches the keyboard ended"))??;
            WATCHER.store(watcher, Ordering::Relaxed);
        }
        (false, true) => {
            WATCHER.store(0, Ordering::Relaxed);
            // SAFETY: posting to a thread that has ended fails without
            // side effects.
            unsafe { PostThreadMessageW(watcher, WM_QUIT, WPARAM(0), LPARAM(0)) }?;
        }
        _ => {}
    }
    Ok(())
}

/// The taken combination among `taken` that `key` completes while `held`
/// are down: the keys to hold are those and no others.
fn completed(taken: &[Hotkey], key: u16, held: Modifiers) -> Option<u32> {
    let complete = |hotkey: &&Hotkey| u16::from(hotkey.key) == key && hotkey.modifiers == held;
    taken.iter().find(complete).map(|hotkey| hotkey.id)
}

/// Whether the key that went down or up belongs to a taken combination
/// and is not to be passed on; a combination that is completed is
/// reported.
fn taken_key(key: VIRTUAL_KEY, down: bool) -> bool {
    let code = u32::from(key.0);
    if !down {
        let swallowed = SWALLOWED.compare_exchange(code, 0, Ordering::Relaxed, Ordering::Relaxed);
        return swallowed.is_ok();
    }
    if CHOOSING.load(Ordering::Relaxed)
        && let Some(choice) = ChoiceKey::of(key.0)
    {
        SWALLOWED.store(code, Ordering::Relaxed);
        choice_presses().0.try_send(choice).ok();
        return true;
    }
    let Ok(taken) = TAKEN.lock() else {
        return false;
    };
    if taken.is_empty() {
        return false;
    }
    let held = Modifiers::down_now();
    let Some(id) = completed(&taken, key.0, held) else {
        return false;
    };
    if held.windows {
        // Windows does not see the key that is kept from it, and would
        // take the Windows key for pressed by itself.
        if let Ok(mut press) = PRESS.lock() {
            press.joined();
        }
        press_unassigned_key();
    }
    SWALLOWED.store(code, Ordering::Relaxed);
    // Presses that still wait to be answered are enough.
    taken_presses().0.try_send(id).ok();
    true
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
pub(super) fn press_unassigned_key() {
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
            if taken_key(key, down) {
                return LRESULT(1);
            }
            let modified = windows && down && modifier_held();
            let alone = WINDOWS_KEY.load(Ordering::Relaxed)
                && PRESS
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
    fn a_taken_combination_is_completed_with_exactly_its_keys_held() {
        let taken = [
            Hotkey::parse(7, "alt+tab").unwrap(),
            Hotkey::parse(8, "alt+shift+tab").unwrap(),
        ];
        let held = |text| Modifiers::parse(text).unwrap();
        assert_eq!(completed(&taken, 0x09, held("alt")), Some(7));
        assert_eq!(completed(&taken, 0x09, held("alt+shift")), Some(8));
        assert_eq!(completed(&taken, 0x09, held("ctrl+alt")), None);
        assert_eq!(completed(&taken, 0x09, Modifiers::default()), None);
        assert_eq!(completed(&taken, 0x20, held("alt")), None);
    }

    #[test]
    fn only_the_keys_of_a_choice_are_such() {
        assert_eq!(ChoiceKey::of(0x1B), Some(ChoiceKey::Escape));
        assert_eq!(ChoiceKey::of(0x28), Some(ChoiceKey::Down));
        assert_eq!(ChoiceKey::of(0x09), None);
    }

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
    fn a_key_kept_from_windows_makes_it_a_combination() {
        let mut press = LonePress::default();
        press.key(true, true, false);
        press.joined();
        assert!(!press.key(true, false, false));
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
