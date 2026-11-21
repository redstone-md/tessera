// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The modules the shell is assembled from.

mod calendar;
mod clock;
mod session;
mod windows;

use crate::extension::ExtensionsModule;
use crate::module::Module;

/// Every module the shell starts with; the last one loads extensions.
pub fn builtin() -> Vec<Box<dyn Module>> {
    vec![
        Box::new(clock::ClockModule),
        Box::new(calendar::CalendarModule),
        Box::new(session::SessionModule),
        Box::new(windows::WindowsModule),
        Box::new(ExtensionsModule),
    ]
}
