// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Key combinations that Windows reports wherever they are pressed.
//!
//! Windows gives a combination to the thread that asked for it, as a
//! message, and to nobody else while that lasts. The combinations have a
//! thread of their own, which waits for those messages and nothing else.
//!
//! A combination can also be recorded: every combination is given up for
//! that time, and a keyboard hook on a thread of its own takes the next
//! one pressed, so that the settings learn which keys to write.

use std::sync::{Mutex, MutexGuard, OnceLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, anyhow};
use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
    RegisterHotKey, UnregisterHotKey, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, KillTimer, MSG, PM_NOREMOVE,
    PeekMessageW, PostThreadMessageW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN, WM_TIMER,
};

/// The keys held while another one is pressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub alt: bool,
    pub control: bool,
    pub shift: bool,
    pub windows: bool,
}

impl Modifiers {
    /// What `text` names: `alt`, `ctrl`, `shift` and `win`, joined by `+`,
    /// each at most once and at least one of them.
    pub fn parse(text: &str) -> Option<Self> {
        let mut modifiers = Self::default();
        for name in text.split('+') {
            let held = match name.trim().to_ascii_lowercase().as_str() {
                "alt" => &mut modifiers.alt,
                "ctrl" => &mut modifiers.control,
                "shift" => &mut modifiers.shift,
                "win" => &mut modifiers.windows,
                _ => return None,
            };
            if *held {
                return None;
            }
            *held = true;
        }
        Some(modifiers)
    }

    /// The keys as the configuration writes them, in the order Ctrl, Alt,
    /// Shift and Win, joined by `+`; empty where none is held.
    pub fn written(self) -> String {
        let names = [
            (self.control, "ctrl"),
            (self.alt, "alt"),
            (self.shift, "shift"),
            (self.windows, "win"),
        ];
        let held: Vec<&str> = { names.into_iter() }
            .filter(|(held, _)| *held)
            .map(|(_, name)| name)
            .collect();
        held.join("+")
    }

    /// The keys that are down at this moment.
    pub fn down_now() -> Self {
        let down = |key: u16| {
            // SAFETY: the call takes a key and only reads its state.
            unsafe { GetAsyncKeyState(key.into()) < 0 }
        };
        Self {
            alt: down(VK_MENU.0),
            control: down(VK_CONTROL.0),
            shift: down(VK_SHIFT.0),
            windows: down(VK_LWIN.0) || down(VK_RWIN.0),
        }
    }

    /// Whether one of these keys is down at this moment.
    pub fn any_down(self) -> bool {
        self.shares(Self::down_now())
    }

    /// Whether one of these keys is among `others`.
    fn shares(self, others: Self) -> bool {
        (self.alt && others.alt)
            || (self.control && others.control)
            || (self.shift && others.shift)
            || (self.windows && others.windows)
    }

    fn flags(self) -> HOT_KEY_MODIFIERS {
        [
            (self.alt, MOD_ALT),
            (self.control, MOD_CONTROL),
            (self.shift, MOD_SHIFT),
            (self.windows, MOD_WIN),
        ]
        .into_iter()
        .filter(|(held, _)| *held)
        .fold(MOD_NOREPEAT, |flags, (_, flag)| flags | flag)
    }
}

/// A key combination and the number it is reported by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub id: u32,
    pub modifiers: Modifiers,
    /// The key as the digit or the capital letter on it.
    pub key: u8,
}

/// The keys with a name, and the virtual-key code of each. The same code
/// is asked of `RegisterHotKey` and looked for by the keyboard hook that
/// takes the combinations Windows keeps to itself, so both know the same
/// keys.
const NAMED_KEYS: &[(&str, u8)] = &[
    ("space", 0x20),
    ("tab", 0x09),
    ("enter", 0x0d),
    ("left", 0x25),
    ("up", 0x26),
    ("right", 0x27),
    ("down", 0x28),
    ("home", 0x24),
    ("end", 0x23),
    ("pageup", 0x21),
    ("pagedown", 0x22),
    ("insert", 0x2d),
    ("delete", 0x2e),
    ("backspace", 0x08),
    ("escape", 0x1b),
    ("pause", 0x13),
    ("printscreen", 0x2c),
    // The keys of the punctuation, as on a US layout; another layout
    // has them where these are.
    ("period", 0xbe),
    ("comma", 0xbc),
    ("plus", 0xbb),
    ("minus", 0xbd),
    ("backquote", 0xc0),
    ("semicolon", 0xba),
    ("slash", 0xbf),
    ("backslash", 0xdc),
    ("leftbracket", 0xdb),
    ("rightbracket", 0xdd),
    ("quote", 0xde),
    // The keys of the number pad, which Windows tells apart from the
    // digits and the signs above the letters.
    ("num0", 0x60),
    ("num1", 0x61),
    ("num2", 0x62),
    ("num3", 0x63),
    ("num4", 0x64),
    ("num5", 0x65),
    ("num6", 0x66),
    ("num7", 0x67),
    ("num8", 0x68),
    ("num9", 0x69),
    ("multiply", 0x6a),
    ("add", 0x6b),
    ("subtract", 0x6d),
    ("decimal", 0x6e),
    ("divide", 0x6f),
    ("scrolllock", 0x91),
    ("menu", 0x5d),
];

/// The keys that are held rather than pressed, and the locks, which a
/// recording lets through: Shift, Ctrl, Alt and the Windows keys, each
/// as either key and as both, Caps Lock and Num Lock.
const PASSED_THROUGH: [u8; 13] = [
    0x10, 0x11, 0x12, 0x5b, 0x5c, 0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0x14, 0x90,
];

/// How long a recording waits for a combination.
const RECORDING_TIME: Duration = Duration::from_secs(10);

/// What the hook of a recording posts to its thread once it has an answer.
const RECORDED: u32 = WM_APP + 1;

impl Hotkey {
    /// The combination `text` names: keys to hold as [`Modifiers::parse`]
    /// takes them, then `+` and the key to press: a digit, a letter, `f1`
    /// to `f24`, or one of the names of [`Self::key_names`], such as
    /// `space`, `left`, `pageup`, `escape`, `printscreen`, `period`,
    /// `plus`, `num0` or `add`.
    pub fn parse(id: u32, text: &str) -> Option<Self> {
        let (held, key) = text.rsplit_once('+')?;
        Some(Self {
            id,
            modifiers: Modifiers::parse(held)?,
            key: Self::key_code(key)?,
        })
    }

    /// The combination `text` names as [`Self::parse`] takes it, or a key
    /// pressed by itself, as a step of a sequence after its first may be:
    /// `t`, `escape`.
    pub fn step(id: u32, text: &str) -> Option<Self> {
        if let Some(hotkey) = Self::parse(id, text) {
            return Some(hotkey);
        }
        Some(Self {
            id,
            modifiers: Modifiers::default(),
            key: Self::key_code(text)?,
        })
    }

    /// The virtual-key code of the key the configuration calls `name`.
    pub fn key_code(name: &str) -> Option<u8> {
        let name = name.trim().to_ascii_lowercase();
        if let Some(&(_, code)) = NAMED_KEYS.iter().find(|(known, _)| *known == name) {
            return Some(code);
        }
        match name.as_bytes() {
            [key] if key.is_ascii_alphanumeric() => Some(key.to_ascii_uppercase()),
            [b'f', b'1'..=b'9', ..] => match name[1..].parse::<u8>() {
                Ok(number @ 1..=24) => Some(0x6f + number),
                _ => None,
            },
            _ => None,
        }
    }

    /// The name the configuration gives the key of the virtual-key code
    /// `code`, where it is one a combination can end with.
    pub fn key_name(code: u8) -> Option<String> {
        if let Some((name, _)) = NAMED_KEYS.iter().find(|(_, known)| *known == code) {
            return Some((*name).to_owned());
        }
        match code {
            b'0'..=b'9' | b'A'..=b'Z' => Some((code as char).to_ascii_lowercase().to_string()),
            0x70..=0x87 => Some(format!("f{}", code - 0x6f)),
            _ => None,
        }
    }

    /// The names of the keys that are neither a letter, nor a digit, nor
    /// one of `f1` to `f24`.
    #[cfg(test)]
    pub fn key_names() -> impl Iterator<Item = &'static str> {
        NAMED_KEYS.iter().map(|(name, _)| *name)
    }
}

/// The combinations of each part of Tessera that has some, and the thread
/// that holds them all with what ends with it.
struct Held {
    owned: Vec<(&'static str, Vec<Hotkey>)>,
    watcher: Option<(u32, JoinHandle<()>)>,
    /// While the combinations are paused, the numbers of those that are
    /// held all the same.
    kept: Option<Vec<u32>>,
    /// Whether a combination is being recorded, while which none is held.
    recording: bool,
}

static HELD: Mutex<Held> = Mutex::new(Held {
    owned: Vec::new(),
    watcher: None,
    kept: None,
    recording: false,
});

impl Held {
    /// Gives up the combinations held now and asks Windows for those of
    /// every part again, but for those a pause leaves out, and for none
    /// while a combination is recorded. The numbers of those Windows
    /// refused are returned.
    fn hold_again(&mut self) -> anyhow::Result<Vec<u32>> {
        // The combinations are free again once their thread has ended.
        if let Some((thread, ended)) = self.watcher.take() {
            // SAFETY: posting to a thread that has ended fails without
            // side effects.
            unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) }
                .context("the key combinations before could not be given up")?;
            ended.join().ok();
        }
        if self.recording {
            return Ok(Vec::new());
        }
        let kept = |key: &Hotkey| { self.kept.as_ref() }.is_none_or(|kept| kept.contains(&key.id));
        let keys: Vec<Hotkey> = { self.owned.iter() }
            .flat_map(|(_, keys)| keys.iter().copied())
            .filter(kept)
            .collect();
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let (ready, started) = mpsc::channel();
        let ended = thread::Builder::new()
            .name("key combinations".to_owned())
            .spawn(move || watch(&keys, &ready))
            .context("the thread that holds the key combinations did not start")?;
        let (thread, refused): (u32, Vec<u32>) = started
            .recv()
            .map_err(|_| anyhow!("the thread that holds the key combinations ended"))?;
        self.watcher = Some((thread, ended));
        Ok(refused)
    }
}

fn held() -> anyhow::Result<MutexGuard<'static, Held>> {
    HELD.lock()
        .map_err(|_| anyhow!("the thread that holds the key combinations failed"))
}

fn pressed() -> &'static (Sender<u32>, Receiver<u32>) {
    static PRESSED: OnceLock<(Sender<u32>, Receiver<u32>)> = OnceLock::new();
    PRESSED.get_or_init(|| async_channel::bounded(8))
}

/// The key combinations Tessera answers wherever they are pressed.
pub struct Hotkeys;

impl Hotkeys {
    /// Where the number of a combination arrives whenever it is pressed.
    pub fn presses() -> Receiver<u32> {
        pressed().1.clone()
    }

    /// Makes `keys` the combinations that are answered for `owner`, a part
    /// of Tessera, in place of its ones before; those of the other parts
    /// stay. The numbers, which no two parts share, of the ones of `keys`
    /// that Windows refused, as it does with a combination another program
    /// holds, are returned. While a combination is being recorded, none is
    /// held and none is refused; they are held again once it is recorded.
    pub fn set(owner: &'static str, keys: Vec<Hotkey>) -> anyhow::Result<Vec<u32>> {
        let mut held = held()?;
        let own: Vec<u32> = keys.iter().map(|key| key.id).collect();
        held.owned.retain(|(name, _)| *name != owner);
        held.owned.push((owner, keys));
        let mut refused = held.hold_again()?;
        refused.retain(|id| own.contains(id));
        Ok(refused)
    }

    /// Holds, of every part's combinations, only those whose numbers are
    /// `kept`, until it is called with `None`, which holds all of them
    /// again: the pause of `pause-keys`, which keeps its own keys.
    pub fn pause(kept: Option<Vec<u32>>) -> anyhow::Result<()> {
        let mut held = held()?;
        if held.kept == kept {
            return Ok(());
        }
        held.kept = kept;
        held.hold_again().map(drop)
    }

    /// Records the next combination pressed anywhere, as the configuration
    /// writes it (`ctrl+alt+k`): for that time every combination of
    /// Tessera is given up, and a keyboard hook takes the keys pressed,
    /// which reach no program, the Windows key with a letter included.
    /// Esc by itself, ten seconds without a combination, a second
    /// recording asked meanwhile and a hook Windows refuses answer `None`.
    /// The combinations are held again afterwards.
    pub fn record() -> Receiver<Option<String>> {
        let (answer, answered) = async_channel::bounded(1);
        let started = thread::Builder::new()
            .name("key recording".to_owned())
            .spawn(move || {
                answer.try_send(record_here()).ok();
            });
        if let Err(error) = started {
            report!("no combination can be recorded: {error}");
        }
        answered
    }
}

/// Records a combination on this thread, with every combination given up
/// meanwhile.
fn record_here() -> Option<String> {
    {
        let mut held = held().ok()?;
        if held.recording {
            return None;
        }
        held.recording = true;
        if let Err(error) = held.hold_again() {
            report!("the key combinations stay held while one is recorded: {error:#}");
        }
    }
    let recorded = listen();
    match held() {
        Ok(mut held) => {
            held.recording = false;
            if let Err(error) = held.hold_again() {
                report!("the key combinations are not held again: {error:#}");
            }
        }
        Err(error) => report!("{error:#}"),
    }
    recorded
}

/// What the hook of a recording found, and the thread it tells.
struct Recording {
    thread: u32,
    /// `Some` once it has an answer: a combination, or `None` to cancel.
    answer: Option<Option<String>>,
}

static RECORDING: Mutex<Recording> = Mutex::new(Recording {
    thread: 0,
    answer: None,
});

/// What a recording makes of the key of `code` going down while `held` are
/// down: `None` to wait on, as for a key that cannot end a combination or
/// one pressed by itself; `Some(None)` for Esc by itself, which cancels;
/// and the combination otherwise.
fn recorded(code: u8, held: Modifiers) -> Option<Option<String>> {
    let name = Hotkey::key_name(code)?;
    if held != Modifiers::default() {
        return Some(Some(format!("{}+{name}", held.written())));
    }
    (name == "escape").then_some(None)
}

/// Sets the hook of a recording on this thread and waits for its answer
/// or for the time to run out; `None` without a combination.
fn listen() -> Option<String> {
    let mut message = MSG::default();
    // SAFETY: `message` outlives the call, which only makes sure the
    // thread has a queue to post to; the call after it only reads which
    // thread this is.
    let thread = unsafe {
        let _ = PeekMessageW(&raw mut message, None, 0, 0, PM_NOREMOVE);
        GetCurrentThreadId()
    };
    if let Ok(mut recording) = RECORDING.lock() {
        *recording = Recording {
            thread,
            answer: None,
        };
    }
    // SAFETY: `recorded_key` is a function of this program, which is never
    // unloaded; a low-level hook is called in this process and injects no
    // module anywhere.
    let hook = match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(recorded_key), None, 0) } {
        Ok(hook) => hook,
        Err(error) => {
            report!("the keyboard cannot be listened to for a combination: {error}");
            return None;
        }
    };
    let time = RECORDING_TIME.as_millis() as u32;
    // SAFETY: a timer of no window posts its message to this thread's
    // queue; `message` outlives the calls that fill it, and Windows calls
    // the hook from inside them. The hook is removed with the handle it
    // was set with.
    unsafe {
        let timer = SetTimer(None, 0, time, None);
        while GetMessageW(&raw mut message, None, 0, 0).0 > 0 {
            if message.message == WM_TIMER || message.message == RECORDED {
                break;
            }
        }
        let _ = KillTimer(None, timer);
        if let Err(error) = UnhookWindowsHookEx(hook) {
            report!("the keyboard is still listened to: {error:#}");
        }
    }
    let mut recording = RECORDING.lock().ok()?;
    recording.thread = 0;
    recording.answer.take().flatten()
}

unsafe extern "system" fn recorded_key(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for this code Windows passes a pointer to the key event,
        // which is valid for the duration of the call.
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let key = event.vkCode.min(0xff) as u8;
        let made_here = event.dwExtraInfo == super::windows_key::MADE_HERE;
        if !made_here && !PASSED_THROUGH.contains(&key) {
            if matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
                take_recorded(key);
            }
            // No other key reaches a program while a combination is
            // recorded.
            return LRESULT(1);
        }
    }
    // SAFETY: the arguments are passed on as they came.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Takes in that the key `key` went down while recording, and tells the
/// thread of the recording once it has its answer.
fn take_recorded(key: u8) {
    let Ok(mut recording) = RECORDING.lock() else {
        return;
    };
    if recording.answer.is_some() {
        return;
    }
    let held = Modifiers::down_now();
    let Some(answer) = recorded(key, held) else {
        return;
    };
    if held.windows {
        // Windows does not see the key that is kept from it, and would
        // take the Windows key for pressed by itself.
        super::windows_key::press_unassigned_key();
    }
    recording.answer = Some(answer);
    // SAFETY: posting to a thread that has ended fails without side
    // effects.
    unsafe {
        let _ = PostThreadMessageW(recording.thread, RECORDED, WPARAM(0), LPARAM(0));
    }
}

/// Asks Windows for `keys`, tells `ready` which thread this is and which
/// keys were refused, and passes on every press until told to quit.
fn watch(keys: &[Hotkey], ready: &mpsc::Sender<(u32, Vec<u32>)>) {
    let mut message = MSG::default();
    // SAFETY: `message` outlives the calls that fill it; the first one
    // only makes sure the thread has a queue to post to. The combinations
    // belong to this thread, which gives them up before it ends.
    unsafe {
        let _ = PeekMessageW(&raw mut message, None, 0, 0, PM_NOREMOVE);
        let refused = keys
            .iter()
            .filter(|key| {
                RegisterHotKey(None, key.id as i32, key.modifiers.flags(), key.key.into()).is_err()
            })
            .map(|key| key.id)
            .collect();
        ready.send((GetCurrentThreadId(), refused)).ok();
        while GetMessageW(&raw mut message, None, 0, 0).0 > 0 {
            if message.message == WM_HOTKEY {
                // A press that is still waiting to be answered is enough.
                pressed().0.try_send(message.wParam.0 as u32).ok();
            }
        }
        for key in keys {
            let _ = UnregisterHotKey(None, key.id as i32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_keys_share_a_key_or_none() {
        let held = Modifiers::parse("alt+shift").unwrap();
        assert!(held.shares(Modifiers::parse("alt").unwrap()));
        assert!(held.shares(Modifiers::parse("ctrl+shift").unwrap()));
        assert!(!held.shares(Modifiers::parse("ctrl+win").unwrap()));
        assert!(!held.shares(Modifiers::default()));
    }

    #[test]
    fn modifiers_are_named_joined_by_plus() {
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        assert_eq!(Modifiers::parse("alt"), Some(alt));
        assert_eq!(
            Modifiers::parse("Ctrl + Alt"),
            Some(Modifiers {
                control: true,
                ..alt
            })
        );
        assert_eq!(
            Modifiers::parse("win+shift"),
            Some(Modifiers {
                windows: true,
                shift: true,
                ..Modifiers::default()
            })
        );
    }

    #[test]
    fn a_combination_is_keys_to_hold_and_a_key_to_press() {
        let held = Modifiers::parse("ctrl+alt").unwrap();
        let key = |text| Hotkey::parse(7, text).map(|key| (key.id, key.modifiers, key.key));
        assert_eq!(key("ctrl+alt+t"), Some((7, held, b'T')));
        assert_eq!(key("Ctrl + Alt + Space"), Some((7, held, 0x20)));
        assert_eq!(key("ctrl+alt+1"), Some((7, held, b'1')));
        assert_eq!(key("ctrl+alt+f1"), Some((7, held, 0x70)));
        assert_eq!(key("ctrl+alt+f12"), Some((7, held, 0x7b)));
        assert_eq!(key("ctrl+alt+left"), Some((7, held, 0x25)));
        assert_eq!(key("ctrl+alt+f13"), Some((7, held, 0x7c)));
        assert_eq!(key("ctrl+alt+f24"), Some((7, held, 0x87)));
        assert_eq!(key("ctrl+alt+escape"), Some((7, held, 0x1b)));
        assert_eq!(key("ctrl+alt+PageDown"), Some((7, held, 0x22)));
        assert_eq!(key("ctrl+alt+period"), Some((7, held, 0xbe)));
        assert_eq!(key("ctrl+alt+printscreen"), Some((7, held, 0x2c)));
    }

    #[test]
    fn a_combination_without_a_key_or_without_keys_to_hold_is_not_taken() {
        for text in [
            "t",
            "ctrl+alt",
            "ctrl+alt+",
            "+t",
            "ctrl+f25",
            "ctrl+f0",
            "ctrl+f01",
            "ctrl+tt",
        ] {
            assert_eq!(Hotkey::parse(0, text), None, "{text}");
        }
    }

    #[test]
    fn a_step_of_a_sequence_may_be_a_key_by_itself() {
        let step = |text| Hotkey::step(3, text).map(|key| (key.modifiers, key.key));
        assert_eq!(step("t"), Some((Modifiers::default(), b'T')));
        assert_eq!(step("escape"), Some((Modifiers::default(), 0x1b)));
        let ctrl = Modifiers::parse("ctrl").unwrap();
        assert_eq!(step("ctrl+num5"), Some((ctrl, 0x65)));
        assert_eq!(step("capslock"), None);
        assert_eq!(step(""), None);
    }

    #[test]
    fn every_key_has_its_name_back() {
        for code in 0..=255u8 {
            if let Some(name) = Hotkey::key_name(code) {
                assert_eq!(Hotkey::key_code(&name), Some(code), "{name}");
            }
        }
        assert_eq!(Hotkey::key_name(b'K').as_deref(), Some("k"));
        assert_eq!(Hotkey::key_name(0x7b).as_deref(), Some("f12"));
        assert_eq!(Hotkey::key_name(0x6b).as_deref(), Some("add"));
        assert_eq!(Hotkey::key_name(0x10), None, "Shift ends no combination");
        let names: Vec<&str> = Hotkey::key_names().collect();
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "a key is named twice");
    }

    #[test]
    fn a_recording_takes_a_key_with_keys_held() {
        let held = |text| Modifiers::parse(text).unwrap();
        assert_eq!(
            recorded(b'K', held("win+ctrl")),
            Some(Some("ctrl+win+k".to_owned()))
        );
        assert_eq!(
            recorded(0x1b, held("win")),
            Some(Some("win+escape".to_owned()))
        );
        assert_eq!(recorded(0x1b, Modifiers::default()), Some(None));
        assert_eq!(recorded(b'K', Modifiers::default()), None);
        assert_eq!(recorded(0x14, held("ctrl")), None, "Caps Lock ends none");
        assert_eq!(held("shift+alt+ctrl+win").written(), "ctrl+alt+shift+win");
        assert_eq!(Modifiers::default().written(), "");
    }

    #[test]
    fn what_names_no_modifier_is_not_taken() {
        assert_eq!(Modifiers::parse(""), None);
        assert_eq!(Modifiers::parse("alt+1"), None);
        assert_eq!(Modifiers::parse("alt+alt"), None);
        assert_eq!(Modifiers::parse("alt+"), None);
    }
}
