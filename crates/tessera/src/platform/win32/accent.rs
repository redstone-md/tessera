// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The accent color the user chose in the Windows settings.

use windows::UI::ViewManagement::{UIColorType, UISettings};

/// The accent color as `[red, green, blue]`, or `None` if Windows does not
/// tell it.
pub fn accent_color() -> Option<[u8; 3]> {
    let settings = UISettings::new().ok()?;
    let color = settings.GetColorValue(UIColorType::Accent).ok()?;
    Some([color.R, color.G, color.B])
}
