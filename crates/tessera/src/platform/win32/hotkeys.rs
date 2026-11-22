// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Key combinations that Windows reports wherever they are pressed.
//!
//! Windows gives a combination to the thread that asked for it, as a
//! message, and to nobody else while that lasts. The combinations have a
//! thread of their own, which waits for those messages and nothing else.

use std::sync::{Mutex, OnceLock, mpsc};
use std::thread::{self, JoinHandle};

use anyhow::{Context, anyhow};
use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
    RegisterHotKey, UnregisterHotKey, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
};

/// The keys held while another one is pressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

impl Hotkey {
    /// The combination `text` names: keys to hold as [`Modifiers::parse`]
    /// takes them, then `+` and the key to press: a digit, a letter, `f1`
    /// to `f12`, `space`, `tab`, `enter`, `left`, `up`, `right` or `down`.
    pub fn parse(id: u32, text: &str) -> Option<Self> {
        let (held, key) = text.rsplit_once('+')?;
        let key = key.trim().to_ascii_lowercase();
        let key = match key.as_str() {
            "space" => 0x20,
            "tab" => 0x09,
            "enter" => 0x0d,
            "left" => 0x25,
            "up" => 0x26,
            "right" => 0x27,
            "down" => 0x28,
            name => match name.as_bytes() {
                [key] if key.is_ascii_alphanumeric() => key.to_ascii_uppercase(),
                [b'f', b'1'..=b'9', ..] => match name[1..].parse::<u8>() {
                    Ok(number @ 1..=12) => 0x6f + number,
                    _ => return None,
                },
                _ => return None,
            },
        };
        Some(Self {
            id,
            modifiers: Modifiers::parse(held)?,
            key,
        })
    }
}

/// The combinations of each part of Tessera that has some, and the thread
/// that holds them all with what ends with it.
struct Held {
    owned: Vec<(&'static str, Vec<Hotkey>)>,
    watcher: Option<(u32, JoinHandle<()>)>,
}

static HELD: Mutex<Held> = Mutex::new(Held {
    owned: Vec::new(),
    watcher: None,
});

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
    /// holds, are returned.
    pub fn set(owner: &'static str, keys: Vec<Hotkey>) -> anyhow::Result<Vec<u32>> {
        let mut held = HELD
            .lock()
            .map_err(|_| anyhow!("the thread that holds the key combinations failed"))?;
        let own: Vec<u32> = keys.iter().map(|key| key.id).collect();
        held.owned.retain(|(name, _)| *name != owner);
        held.owned.push((owner, keys));
        let keys: Vec<Hotkey> = { held.owned.iter() }
            .flat_map(|(_, keys)| keys.iter().copied())
            .collect();
        let watcher = &mut held.watcher;
        // The combinations are free again once their thread has ended.
        if let Some((thread, ended)) = watcher.take() {
            // SAFETY: posting to a thread that has ended fails without
            // side effects.
            unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) }
                .context("the key combinations before could not be given up")?;
            ended.join().ok();
        }
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let (ready, started) = mpsc::channel();
        let ended = thread::Builder::new()
            .name("key combinations".to_owned())
            .spawn(move || watch(&keys, &ready))
            .context("the thread that holds the key combinations did not start")?;
        let (thread, mut refused): (u32, Vec<u32>) = started
            .recv()
            .map_err(|_| anyhow!("the thread that holds the key combinations ended"))?;
        *watcher = Some((thread, ended));
        refused.retain(|id| own.contains(id));
        Ok(refused)
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
    }

    #[test]
    fn a_combination_without_a_key_or_without_keys_to_hold_is_not_taken() {
        for text in [
            "t",
            "ctrl+alt",
            "ctrl+alt+",
            "+t",
            "ctrl+f13",
            "ctrl+f0",
            "ctrl+f01",
            "ctrl+tt",
        ] {
            assert_eq!(Hotkey::parse(0, text), None, "{text}");
        }
    }

    #[test]
    fn what_names_no_modifier_is_not_taken() {
        assert_eq!(Modifiers::parse(""), None);
        assert_eq!(Modifiers::parse("alt+1"), None);
        assert_eq!(Modifiers::parse("alt+alt"), None);
        assert_eq!(Modifiers::parse("alt+"), None);
    }
}
