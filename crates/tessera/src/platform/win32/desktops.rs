// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The virtual desktops of Windows: how many there are, which one is
//! shown, and going to another one.
//!
//! Windows offers programs no way to ask for either. Explorer keeps the
//! list in the registry, which is read here, and goes to the desktop next
//! to the current one when its keyboard shortcut is pressed, which is
//! done here as often as it takes.

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RegGetValueW};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_LEFT, VK_LWIN, VK_RIGHT,
};
use windows::core::{PCWSTR, w};

use super::windows_key::MADE_HERE;

/// The bytes that identify one desktop.
const ID_LENGTH: usize = 16;

/// More desktops than anyone keeps; the list is read into room for these.
const MOST: usize = 128;

/// The virtual desktops at one moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualDesktops {
    /// How many desktops there are, at least one.
    pub count: usize,
    /// Which one is shown, counted from zero.
    pub current: usize,
}

impl VirtualDesktops {
    /// The desktops as Explorer has them on record. Explorer records
    /// nothing until a second desktop was made, which reads as one desktop.
    pub fn now() -> Self {
        let ids = binary(w!("VirtualDesktopIDs"));
        let current = binary(w!("CurrentVirtualDesktop"));
        Self::of_record(&ids, &current)
    }

    /// The desktops of a record: `ids` is the identifiers of all desktops
    /// one after another, `current` the identifier of the one shown.
    fn of_record(ids: &[u8], current: &[u8]) -> Self {
        let ids = ids.as_chunks::<ID_LENGTH>().0;
        let found = ids.iter().position(|id| id.as_slice() == current);
        Self {
            count: ids.len().max(1),
            current: found.unwrap_or(0),
        }
    }

    /// Goes from the current desktop to the one at `index`, by pressing
    /// the shortcut for the next or the previous desktop as often as that
    /// is away. Whether Windows took every key is returned.
    pub fn show(self, index: usize) -> bool {
        let index = index.min(self.count - 1);
        let (arrow, steps) = match index.checked_sub(self.current) {
            Some(steps) => (VK_RIGHT, steps),
            None => (VK_LEFT, self.current - index),
        };
        if steps == 0 {
            return true;
        }
        let mut keys = vec![key(VK_LWIN, true), key(VK_CONTROL, true)];
        for _ in 0..steps {
            keys.extend([key(arrow, true), key(arrow, false)]);
        }
        keys.extend([key(VK_CONTROL, false), key(VK_LWIN, false)]);
        // SAFETY: the events outlive the call, which copies them.
        let sent = unsafe { SendInput(&keys, size_of::<INPUT>() as i32) };
        sent as usize == keys.len()
    }
}

/// A key going down if `down`, and up otherwise, marked as made here so
/// that the watcher of the Windows key leaves it alone.
fn key(key: VIRTUAL_KEY, down: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key == VK_LEFT || key == VK_RIGHT || key == VK_LWIN {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: flags,
                dwExtraInfo: MADE_HERE,
                ..Default::default()
            },
        },
    }
}

/// The bytes Explorer keeps under `name` about virtual desktops; none if
/// there is no such record.
fn binary(name: PCWSTR) -> Vec<u8> {
    let mut bytes = vec![0u8; ID_LENGTH * MOST];
    let mut size = bytes.len() as u32;
    // SAFETY: the out-parameters outlive the call, and `size` is the size
    // of what the buffer holds.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VirtualDesktops"),
            name,
            RRF_RT_REG_BINARY,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    if status.is_err() {
        return Vec::new();
    }
    bytes.truncate(size as usize);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(desktops: &[u8]) -> Vec<u8> {
        desktops
            .iter()
            .flat_map(|&desktop| [desktop; ID_LENGTH])
            .collect()
    }

    #[test]
    fn the_shown_desktop_is_found_in_the_list() {
        let desktops = VirtualDesktops::of_record(&ids(&[7, 8, 9]), &ids(&[8]));
        assert_eq!(
            desktops,
            VirtualDesktops {
                count: 3,
                current: 1
            }
        );
        let last = VirtualDesktops::of_record(&ids(&[7, 8, 9]), &ids(&[9]));
        assert_eq!((last.count, last.current), (3, 2));
    }

    #[test]
    fn no_record_is_one_desktop() {
        let desktops = VirtualDesktops::of_record(&[], &[]);
        assert_eq!((desktops.count, desktops.current), (1, 0));
    }

    #[test]
    fn an_unknown_shown_desktop_counts_as_the_first() {
        let desktops = VirtualDesktops::of_record(&ids(&[7, 8]), &ids(&[5]));
        assert_eq!((desktops.count, desktops.current), (2, 0));
    }
}
