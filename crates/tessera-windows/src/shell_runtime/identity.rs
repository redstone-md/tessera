// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Bounded native user/clock/foreground-input-language display values.
//! Accent is already provided by Slint's palette; this module does not query it.

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Globalization::{
    DATE_LONGDATE, GetDateFormatEx, GetLocaleInfoW, GetTimeFormatEx, LOCALE_SABBREVLANGNAME,
    TIME_NOSECONDS,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::WindowsProgramming::GetUserNameW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
};

use crate::apps::ApplicationError;
use crate::helpers::utf16_to_string_lossy;

/// Bounded display identity for the shell surfaces.
///
/// `foreground_window` is `None` when no eligible foreground window exists;
/// the id is the same raw HWND identity that observation uses, so the parent
/// can match it against known observed targets. Accent color is deliberately
/// absent: Slint's winit backend already queries DwmGetColorizationColor and
/// std-widgets `Palette.accent-background` consumes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopIdentity {
    pub user_name: String,
    pub clock: String,
    pub language: String,
    pub foreground_window: Option<tessera_core::WindowId>,
}

/// One bounded user-name read. Empty result is an error, never a guess.
pub(crate) fn user_name() -> Result<String, ApplicationError> {
    let mut size: u32 = 0;
    // SAFETY: null buffer query for the required size is the documented
    // two-step pattern; size receives UNLEN+1 bytes.
    if unsafe { GetUserNameW(std::ptr::null_mut(), &mut size) } == 0 {
        let code = last_error();
        if size == 0 || code == 0 {
            return Err(ApplicationError::Windows {
                operation: "GetUserNameW(size)",
                code,
            });
        }
    }
    // Bound the allocation: UNLEN is 256, so 512 wide units is generous.
    if size == 0 || size > 512 {
        return Err(ApplicationError::Windows {
            operation: "GetUserNameW(size)",
            code: 0,
        });
    }
    let mut buffer = vec![0u16; size as usize];
    // SAFETY: buffer and size agree for the documented single-step call.
    if unsafe { GetUserNameW(buffer.as_mut_ptr(), &mut size) } == 0 {
        return Err(ApplicationError::Windows {
            operation: "GetUserNameW",
            code: last_error(),
        });
    }
    buffer.truncate(size.saturating_sub(1) as usize); // drop the NUL
    let name = utf16_to_string_lossy(&buffer);
    if name.is_empty() {
        return Err(ApplicationError::Windows {
            operation: "GetUserNameW(empty)",
            code: 0,
        });
    }
    Ok(name)
}

/// Abbreviated native language of the actual foreground thread's input layout.
/// No foreground means no display value, never the supervisor's/default locale.
pub(crate) fn language_display() -> Result<String, ApplicationError> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return Ok(String::new());
    }
    let thread = unsafe { GetWindowThreadProcessId(foreground, std::ptr::null_mut()) };
    if thread == 0 {
        return Ok(String::new());
    }
    let layout = unsafe { GetKeyboardLayout(thread) };
    if layout.is_null() {
        return Ok(String::new());
    }
    let locale = (layout as usize & 0xffff) as u32;
    let mut buffer = [0u16; 16];
    // LOWORD(HKL) is LANGID; MAKELCID with SORT_DEFAULT keeps those same bits.
    let written = unsafe {
        GetLocaleInfoW(
            locale,
            LOCALE_SABBREVLANGNAME,
            buffer.as_mut_ptr(),
            buffer.len() as i32,
        )
    };
    if written <= 1 || written as usize > buffer.len() {
        return Err(ApplicationError::Windows {
            operation: "GetLocaleInfoW(input language)",
            code: last_error(),
        });
    }
    Ok(utf16_to_string_lossy(&buffer[..written as usize - 1]))
}

/// Current native clock text in the user locale, minutes granularity.
///
/// This is an explicit clock update (the UI calls it at most once per
/// minute); it reads only date/time APIs, never user/foreground/locale
/// layout state.
pub(crate) fn clock_text() -> Result<String, ApplicationError> {
    let mut time = windows_sys::Win32::Foundation::SYSTEMTIME::default();
    // SAFETY: valid writable SYSTEMTIME output.
    unsafe { GetLocalTime(&mut time) };
    let mut date = [0u16; 128];
    // SAFETY: bounded writable output; DATE_LONGDATE uses the locale format.
    let date_len = unsafe {
        GetDateFormatEx(
            std::ptr::null(), // LOCALE_NAME_USER_DEFAULT; no extra identity query
            DATE_LONGDATE,
            &time,
            std::ptr::null(),
            date.as_mut_ptr(),
            date.len() as i32,
            std::ptr::null(),
        )
    };
    if date_len <= 0 {
        return Err(ApplicationError::Windows {
            operation: "GetDateFormatEx",
            code: last_error(),
        });
    }
    let mut clock = [0u16; 64];
    // SAFETY: bounded writable output; TIME_NOSECONDS keeps toolbar text slim.
    let time_len = unsafe {
        GetTimeFormatEx(
            std::ptr::null(),
            TIME_NOSECONDS,
            &time,
            std::ptr::null(),
            clock.as_mut_ptr(),
            clock.len() as i32,
        )
    };
    if time_len <= 0 {
        return Err(ApplicationError::Windows {
            operation: "GetTimeFormatEx",
            code: last_error(),
        });
    }
    Ok(format!(
        "{} {}",
        utf16_to_string_lossy(&date[..(date_len - 1) as usize]),
        utf16_to_string_lossy(&clock[..(time_len - 1) as usize])
    ))
}

/// The foreground window of the current session, when a live visible
/// top-level window holds it. The id is the documented raw-HWND identity,
/// immediately matchable against observation targets.
pub(crate) fn foreground_window_id() -> Option<tessera_core::WindowId> {
    // SAFETY: pure query with no parameters.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    // SAFETY: user32 validates the transient handle; queries never mutate.
    if unsafe { IsWindow(hwnd) } == 0 || unsafe { IsWindowVisible(hwnd) } == 0 {
        return None;
    }
    let mut process_id = 0;
    // SAFETY: writable DWORD output.
    unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) };
    if process_id == std::process::id() {
        // Own shell surfaces are not application targets.
        return None;
    }
    let mut class = [0u16; 256];
    // SAFETY: bounded live UTF-16 buffer.
    if unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) } <= 0 {
        return None;
    }
    Some(tessera_core::WindowId::new(hwnd as usize as u64))
}

/// Collects the whole identity in one bounded pass.
pub(crate) fn desktop_identity() -> Result<DesktopIdentity, ApplicationError> {
    let identity = DesktopIdentity {
        user_name: user_name()?,
        clock: clock_text()?,
        language: language_display()?,
        foreground_window: foreground_window_id(),
    };
    Ok(identity)
}

fn last_error() -> u32 {
    // SAFETY: thread-local error query has no preconditions.
    unsafe { GetLastError() }
}
