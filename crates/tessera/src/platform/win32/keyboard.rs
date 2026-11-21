// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The input language of the window that is typed into.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Globalization::{GetLocaleInfoW, LOCALE_SISO639LANGNAME};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardLayoutList, HKL};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, PostMessageW, WM_INPUTLANGCHANGEREQUEST,
};

/// A keyboard layout of a language, as Windows has it installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputLanguage(isize);

impl InputLanguage {
    /// The layouts the user can switch between, in the order of Windows.
    pub fn installed() -> Vec<Self> {
        // SAFETY: without a buffer the call only counts the layouts.
        let count = unsafe { GetKeyboardLayoutList(None) };
        let mut layouts = vec![HKL::default(); count.max(0) as usize];
        if layouts.is_empty() {
            return Vec::new();
        }
        // SAFETY: the buffer has the length that is passed along with it.
        let written = unsafe { GetKeyboardLayoutList(Some(&mut layouts)) };
        layouts.truncate(written.max(0) as usize);
        layouts
            .into_iter()
            .map(|layout| Self(layout.0 as isize))
            .collect()
    }

    /// The language as Windows numbers it: the lower half of the layout.
    fn language_id(self) -> u16 {
        (self.0 & 0xffff) as u16
    }

    /// The two or three letters of the language, in capitals, such as `EN`.
    /// Two layouts of one language have the same letters.
    pub fn code(self) -> String {
        let mut letters = [0u16; 9];
        // SAFETY: the buffer outlives the call, which writes no more than
        // its length.
        let written = unsafe {
            GetLocaleInfoW(
                self.language_id().into(),
                LOCALE_SISO639LANGNAME,
                Some(&mut letters),
            )
        };
        // The count includes the terminating zero.
        match usize::try_from(written - 1) {
            Ok(length) if length > 0 => String::from_utf16_lossy(&letters[..length]).to_uppercase(),
            _ => format!("{:04X}", self.language_id()),
        }
    }

    /// The one of `languages` that follows this one, going around at the
    /// end; the first one when this one is not among them.
    pub fn next_of(self, languages: &[Self]) -> Option<Self> {
        let here = languages.iter().position(|&language| language == self);
        let next = here.map_or(0, |here| (here + 1) % languages.len().max(1));
        languages.get(next).copied()
    }
}

/// A window that is typed into, and the language it is typed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Typing {
    window: isize,
    pub language: InputLanguage,
}

impl Typing {
    /// The window in front and its language. `None` when no window is in
    /// front, when it is one of this program, or when Windows does not
    /// tell its language, as for some console windows.
    pub fn in_front() -> Option<Self> {
        // SAFETY: the calls only read system state; `process` outlives the
        // call that writes it.
        unsafe {
            let window = GetForegroundWindow();
            if window.is_invalid() {
                return None;
            }
            let mut process = 0;
            let thread = GetWindowThreadProcessId(window, Some(&raw mut process));
            if thread == 0 || process == GetCurrentProcessId() {
                return None;
            }
            let layout = GetKeyboardLayout(thread);
            (!layout.is_invalid()).then_some(Self {
                window: window.0 as isize,
                language: InputLanguage(layout.0 as isize),
            })
        }
    }

    /// Asks the window to be typed into in `language` from now on. The
    /// window is free to refuse, and one that is gone gets no request.
    pub fn switch_to(self, language: InputLanguage) -> windows::core::Result<()> {
        // SAFETY: posting to a window that no longer exists fails without
        // side effects; the message carries plain numbers.
        unsafe {
            PostMessageW(
                Some(HWND(self.window as *mut c_void)),
                WM_INPUTLANGCHANGEREQUEST,
                WPARAM(0),
                LPARAM(language.0),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_language_goes_around() {
        let languages = [InputLanguage(1), InputLanguage(2), InputLanguage(3)];
        assert_eq!(InputLanguage(1).next_of(&languages), Some(InputLanguage(2)));
        assert_eq!(InputLanguage(3).next_of(&languages), Some(InputLanguage(1)));
        assert_eq!(InputLanguage(9).next_of(&languages), Some(InputLanguage(1)));
        assert_eq!(InputLanguage(1).next_of(&[]), None);
    }

    #[test]
    fn a_language_is_spelled_with_its_letters() {
        assert_eq!(InputLanguage(0x0409_0409).code(), "EN");
        assert_eq!(InputLanguage(0x0419_0419).code(), "RU");
    }
}
