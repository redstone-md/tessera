// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The modules that ship inside the shell.

mod clock;
mod session;
mod windows;

use crate::module::Module;

/// Every built-in module.
pub fn builtin() -> Vec<Box<dyn Module>> {
    vec![
        Box::new(clock::ClockModule),
        Box::new(session::SessionModule),
        Box::new(windows::WindowsModule),
    ]
}
