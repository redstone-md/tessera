// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The input method of the window in front, as the input indicator of
//! Explorer shows it: whether the IME of a Japanese, Korean or Chinese
//! layout is on, and in what mode it converts (あ, カ, 가, 中 and the
//! like, or A for letters as they are typed).
//!
//! The IME is asked through the default IME window of the window in front
//! (`ImmGetDefaultIMEWnd`) with `WM_IME_CONTROL`, which waits for the
//! thread of that window: each call waits at most [`ANSWER_TIME`] and
//! belongs on a thread that may wait. Input methods of the Text Services
//! Framework answer it as the IMM32 ones do on the builds it was written
//! for; that every one does is not verified.

#![allow(unsafe_code)]

use std::ffi::c_void;

use anyhow::{Result, bail};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::Ime::{
    IMC_SETOPENSTATUS, IME_CMODE_FULLSHAPE, IME_CMODE_KATAKANA, IME_CMODE_NATIVE,
    ImmGetDefaultIMEWnd,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
    WM_IME_CONTROL,
};

/// How long the window in front may take to answer, in milliseconds.
const ANSWER_TIME: u32 = 100;

/// What `WM_IME_CONTROL` is asked for: the conversion mode, and whether
/// the IME is on. The `windows` crate gives only the one that sets it.
const IMC_GETCONVERSIONMODE: usize = 0x0001;
const IMC_GETOPENSTATUS: usize = 0x0005;

/// The languages whose layouts come with an input method that converts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Script {
    Japanese,
    Korean,
    Chinese,
}

impl Script {
    /// The script of the language Windows numbers `language`.
    fn of(language: u16) -> Option<Self> {
        // The lower ten bits name the language, the rest its country.
        match language & 0x03ff {
            0x11 => Some(Self::Japanese),
            0x12 => Some(Self::Korean),
            0x04 => Some(Self::Chinese),
            _ => None,
        }
    }
}

/// What the IME of the window in front makes of what is typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImeMode {
    /// Letters as they are typed.
    Letters,
    /// Letters of the full width, as Japanese text sets them.
    WideLetters,
    /// Hiragana.
    Hiragana,
    /// Katakana.
    Katakana,
    /// Hangul.
    Hangul,
    /// Chinese characters.
    Chinese,
    /// English letters of a Chinese IME that is on.
    English,
}

impl ImeMode {
    /// What the input indicator shows for it.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Letters => "A",
            Self::WideLetters => "\u{ff21}",
            Self::Hiragana => "\u{3042}",
            Self::Katakana => "\u{30ab}",
            Self::Hangul => "\u{ac00}",
            Self::Chinese => "\u{4e2d}",
            Self::English => "\u{82f1}",
        }
    }

    /// The mode an IME of `script` is in, on or not, with the conversion
    /// bits `conversion`.
    fn of(script: Script, open: bool, conversion: u32) -> Self {
        let native = conversion & IME_CMODE_NATIVE.0 != 0;
        match script {
            _ if !open => Self::Letters,
            Script::Japanese if native && conversion & IME_CMODE_KATAKANA.0 != 0 => Self::Katakana,
            Script::Japanese if native => Self::Hiragana,
            Script::Japanese if conversion & IME_CMODE_FULLSHAPE.0 != 0 => Self::WideLetters,
            Script::Korean if native => Self::Hangul,
            Script::Chinese if native => Self::Chinese,
            Script::Chinese => Self::English,
            _ => Self::Letters,
        }
    }
}

/// The IME of the window in front, as it was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImeState {
    /// The window it is of.
    window: isize,
    /// `None` for a layout without an input method that converts.
    script: Option<Script>,
    /// Whether it is on.
    pub open: bool,
    conversion: u32,
}

impl ImeState {
    /// The IME of the window in front. `None` where no window is in front
    /// or it is one of Tessera, whose IME is not the one meant; a window
    /// typed in a layout without an IME has a state without a mode. Waits
    /// for the window in front, a moment at most.
    pub fn in_front() -> Option<Self> {
        // SAFETY: the calls only read which window is in front, its thread
        // and its layout; `process` outlives the call that writes it.
        let (window, thread, process) = unsafe {
            let window = GetForegroundWindow();
            let mut process = 0;
            let thread = GetWindowThreadProcessId(window, Some(&raw mut process));
            (window, thread, process)
        };
        // SAFETY: as above.
        if window.is_invalid() || thread == 0 || process == unsafe { GetCurrentProcessId() } {
            return None;
        }
        // SAFETY: the call reads the layout of a thread.
        let layout = unsafe { GetKeyboardLayout(thread) };
        let mut state = Self {
            window: window.0 as isize,
            script: Script::of((layout.0 as usize & 0xffff) as u16),
            open: false,
            conversion: 0,
        };
        if state.script.is_none() {
            return Some(state);
        }
        let ime = ime_window(window)?;
        state.open = ask(ime, IMC_GETOPENSTATUS, 0)? != 0;
        state.conversion = ask(ime, IMC_GETCONVERSIONMODE, 0)? as u32;
        Some(state)
    }

    /// What the IME makes of what is typed; `None` for a layout without
    /// one.
    pub fn mode(&self) -> Option<ImeMode> {
        Some(ImeMode::of(self.script?, self.open, self.conversion))
    }

    /// The state it is in once it was switched on or off.
    pub fn switched(self) -> Self {
        Self {
            open: !self.open,
            ..self
        }
    }

    /// Switches the IME of the window on, or off where it was on. Waits for
    /// the window, a moment at most.
    pub fn toggle(self) -> Result<()> {
        if self.script.is_none() {
            bail!("the window is typed in a layout without an input method");
        }
        let window = HWND(self.window as *mut c_void);
        let Some(ime) = ime_window(window) else {
            bail!("the window has no input method to switch");
        };
        let open = usize::from(!self.open);
        if ask_to(ime, IMC_SETOPENSTATUS as usize, open).is_none() {
            bail!("the window did not answer");
        }
        Ok(())
    }
}

/// The default IME window of `window`.
fn ime_window(window: HWND) -> Option<HWND> {
    // SAFETY: the call takes any window and only looks it up.
    let ime = unsafe { ImmGetDefaultIMEWnd(window) };
    (!ime.is_invalid()).then_some(ime)
}

/// Asks the IME window `ime` the question `what`, with `value`; `None`
/// where it did not answer in time.
fn ask(ime: HWND, what: usize, value: isize) -> Option<usize> {
    let mut answer = 0usize;
    // SAFETY: the message carries plain numbers, and `answer` outlives the
    // call, which waits at most `ANSWER_TIME`.
    let sent = unsafe {
        SendMessageTimeoutW(
            ime,
            WM_IME_CONTROL,
            WPARAM(what),
            LPARAM(value),
            SMTO_ABORTIFHUNG,
            ANSWER_TIME,
            Some(&raw mut answer),
        )
    };
    (sent.0 != 0).then_some(answer)
}

/// Tells the IME window `ime` to do `what` with `value`.
fn ask_to(ime: HWND, what: usize, value: usize) -> Option<usize> {
    ask(ime, what, value as isize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_is_told_by_the_language() {
        assert_eq!(Script::of(0x0411), Some(Script::Japanese));
        assert_eq!(Script::of(0x0412), Some(Script::Korean));
        assert_eq!(Script::of(0x0804), Some(Script::Chinese));
        assert_eq!(Script::of(0x0404), Some(Script::Chinese));
        assert_eq!(Script::of(0x0409), None);
        assert_eq!(Script::of(0x0419), None);
    }

    #[test]
    fn the_mode_follows_the_conversion_bits() {
        let native = IME_CMODE_NATIVE.0;
        let katakana = native | IME_CMODE_KATAKANA.0;
        assert_eq!(
            ImeMode::of(Script::Japanese, false, native),
            ImeMode::Letters
        );
        assert_eq!(
            ImeMode::of(Script::Japanese, true, native),
            ImeMode::Hiragana
        );
        assert_eq!(
            ImeMode::of(Script::Japanese, true, katakana),
            ImeMode::Katakana
        );
        assert_eq!(
            ImeMode::of(Script::Japanese, true, IME_CMODE_FULLSHAPE.0),
            ImeMode::WideLetters
        );
        assert_eq!(ImeMode::of(Script::Korean, true, native), ImeMode::Hangul);
        assert_eq!(ImeMode::of(Script::Korean, true, 0), ImeMode::Letters);
        assert_eq!(ImeMode::of(Script::Chinese, true, native), ImeMode::Chinese);
        assert_eq!(ImeMode::of(Script::Chinese, true, 0), ImeMode::English);
        assert_eq!(ImeMode::Hiragana.glyph(), "\u{3042}");
    }
}
