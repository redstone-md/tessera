// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How many lines, or characters sideways, Windows scrolls for a notch of
//! the mouse wheel: the setting "lines to scroll each time" of the mouse.
//! GPUI multiplies each notch by it, so the widgets the wheel turns divide
//! by it again to count notches.

#![allow(unsafe_code)]

use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SystemParametersInfoW,
};

/// The lines a notch scrolls when Windows does not say: its default.
const DEFAULT_LINES: u32 = 3;

/// The lines a notch of the wheel scrolls, or with `sideways` the
/// characters a notch scrolls sideways, as GPUI counts them. Windows reads
/// this from the settings it keeps in memory; it waits for nothing.
/// `u32::MAX` stands for a page a notch, which GPUI multiplies by all the
/// same; 0, or a failed read, gives the default of 3.
pub fn wheel_lines(sideways: bool) -> f32 {
    let action = if sideways {
        SPI_GETWHEELSCROLLCHARS
    } else {
        SPI_GETWHEELSCROLLLINES
    };
    let mut lines = DEFAULT_LINES;
    // SAFETY: `lines` is the value the call fills and outlives it.
    let read = unsafe {
        SystemParametersInfoW(
            action,
            0,
            Some((&raw mut lines).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    if read.is_err() || lines == 0 {
        lines = DEFAULT_LINES;
    }
    lines as f32
}
