// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the keyboard layouts the user can switch between are called, as
//! the switcher of Windows names them: the language, such as "English
//! (United States)", and the layout, such as "US".
//!
//! Windows names a layout only for the thread that has it active, so the
//! names are read on a short thread of its own that takes on each layout
//! in turn; no window of anyone else is touched. Reading them asks the
//! registry, so it is done away from the thread that draws.
//!
//! [`OwnInput`] switches the layout of Tessera's own windows, which a
//! request to a window of another program does not reach.

use std::ffi::c_void;
use std::thread;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Globalization::{
    GetLocaleInfoEx, LCIDToLocaleName, LOCALE_ALLOW_NEUTRAL_NAMES, LOCALE_SLOCALIZEDDISPLAYNAME,
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ActivateKeyboardLayout, GetKeyboardLayoutList, GetKeyboardLayoutNameW, HKL, KLF_NOTELLSHELL,
    KLF_SETFORPROCESS,
};
use windows::Win32::UI::Shell::SHLoadIndirectString;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, HKL_NEXT, HKL_PREV,
};
use windows::core::PCWSTR;

/// Where Windows describes each keyboard layout, under its identifier.
const LAYOUTS: &str = r"SYSTEM\CurrentControlSet\Control\Keyboard Layouts";

/// The longest name that is read, in UTF-16 units.
const NAME_LENGTH: usize = 256;

/// The names of a keyboard layout; either may be empty where Windows does
/// not say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputName {
    pub language: String,
    pub layout: String,
}

impl InputName {
    /// The names of the layouts the user can switch between, in the order
    /// [`InputLanguage::installed`](super::InputLanguage::installed) gives
    /// them, which is the order of Windows.
    pub fn installed() -> Vec<Self> {
        let named = thread::Builder::new()
            .name("input names".to_owned())
            .spawn(read_all);
        match named.map(thread::JoinHandle::join) {
            Ok(Ok(names)) => names,
            _ => Vec::new(),
        }
    }
}

/// The keyboard layout Tessera's own windows are typed in.
pub struct OwnInput;

impl OwnInput {
    /// Whether the window in front is one of Tessera, as the launcher is
    /// while its search field is typed into.
    pub fn in_front() -> bool {
        // SAFETY: the calls only read system state; `process` outlives the
        // call that writes it.
        unsafe {
            let window = GetForegroundWindow();
            let mut process = 0;
            !window.is_invalid()
                && GetWindowThreadProcessId(window, Some(&raw mut process)) != 0
                && process == GetCurrentProcessId()
        }
    }

    /// Takes on the layout after the one in use, or the one before it when
    /// `back`, in every window of Tessera. It is to be called on the
    /// thread of the windows, whose focused window Windows tells.
    pub fn step(back: bool) -> windows::core::Result<()> {
        let step = if back { HKL_PREV } else { HKL_NEXT };
        // SAFETY: the next and the previous layout are values Windows
        // defines for this call, which changes only this program.
        unsafe { ActivateKeyboardLayout(HKL(step as usize as *mut c_void), KLF_SETFORPROCESS) }
            .map(|_| ())
    }
}

/// Reads the names on this thread, whose layout it changes.
fn read_all() -> Vec<InputName> {
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
        .map(|layout| InputName {
            language: language_name((layout.0 as usize & 0xffff) as u32),
            layout: layout_name(layout),
        })
        .collect()
}

fn text_of(units: &[u16]) -> String {
    let length = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..length])
}

/// What Windows calls the language `language_id` in the language of its
/// own user interface.
fn language_name(language_id: u32) -> String {
    let mut locale = [0_u16; 85];
    // SAFETY: the buffer outlives the call, which writes no more than its
    // length.
    let written =
        unsafe { LCIDToLocaleName(language_id, Some(&mut locale), LOCALE_ALLOW_NEUTRAL_NAMES) };
    if written <= 0 {
        return String::new();
    }
    let mut name = [0_u16; NAME_LENGTH];
    // SAFETY: the locale name is NUL-terminated, and the buffer outlives
    // the call, which writes no more than its length.
    let written = unsafe {
        GetLocaleInfoEx(
            PCWSTR(locale.as_ptr()),
            LOCALE_SLOCALIZEDDISPLAYNAME,
            Some(&mut name),
        )
    };
    if written <= 0 {
        return String::new();
    }
    text_of(&name)
}

/// What Windows calls the layout `layout`: its display name, which may be
/// a reference to a text of a library, or else its plain text.
fn layout_name(layout: HKL) -> String {
    let mut identifier = [0_u16; 9];
    // SAFETY: the layout is one of the list, which this thread may take
    // on; the shell is not told. The buffer has the length the call
    // writes.
    let named = unsafe {
        ActivateKeyboardLayout(layout, KLF_NOTELLSHELL)
            .and_then(|_| GetKeyboardLayoutNameW(&mut identifier))
    };
    if named.is_err() {
        return String::new();
    }
    let key = format!("{LAYOUTS}\\{}", text_of(&identifier));
    let shown = registry_text(&key, "Layout Display Name").and_then(|text| indirect(&text));
    shown
        .or_else(|| registry_text(&key, "Layout Text"))
        .unwrap_or_default()
}

/// The text value `name` of the key `key` of the machine.
fn registry_text(key: &str, name: &str) -> Option<String> {
    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain([0]).collect() };
    let (key, name) = (wide(key), wide(name));
    let mut text = [0_u16; NAME_LENGTH];
    let mut size = size_of_val(&text) as u32;
    // SAFETY: the names are NUL-terminated, and `text` and `size`, its
    // size in bytes, outlive the call.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    (status == ERROR_SUCCESS)
        .then(|| text_of(&text))
        .filter(|text| !text.is_empty())
}

/// The text a reference such as `@%SystemRoot%\system32\input.dll,-5000`
/// stands for; a plain text stays as it is.
fn indirect(reference: &str) -> Option<String> {
    if !reference.starts_with('@') {
        return Some(reference.to_owned());
    }
    let source: Vec<u16> = reference.encode_utf16().chain([0]).collect();
    let mut text = [0_u16; NAME_LENGTH];
    // SAFETY: the reference is NUL-terminated, and the buffer outlives
    // the call, which writes no more than its length.
    unsafe { SHLoadIndirectString(PCWSTR(source.as_ptr()), &mut text, None) }.ok()?;
    Some(text_of(&text)).filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_ends_at_its_first_zero() {
        let units: Vec<u16> = "US\0junk".encode_utf16().collect();
        assert_eq!(text_of(&units), "US");
        let whole: Vec<u16> = "US".encode_utf16().collect();
        assert_eq!(text_of(&whole), "US");
    }

    #[test]
    fn a_plain_text_is_no_reference() {
        assert_eq!(indirect("US").as_deref(), Some("US"));
    }
}
