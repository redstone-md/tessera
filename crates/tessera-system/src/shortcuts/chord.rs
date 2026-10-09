// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::{KeyChord, KeyModifiers, ShortcutError, ShortcutErrorKind};

impl KeyChord {
    pub fn new(modifiers: KeyModifiers, key: u16) -> Result<Self, ShortcutError> {
        let chord = Self::Chord { modifiers, key };
        chord.validate()?;
        Ok(chord)
    }

    /// Validate even values constructed directly through the public enum.
    pub fn validate(&self) -> Result<(), ShortcutError> {
        if let Self::Chord { key, .. } = self
            && (!assigned_key(*key)
                || matches!(*key, 0x10..=0x12 | 0x5B..=0x5C | 0x7B | 0xA0..=0xA5))
        {
            return Err(ShortcutError::new(
                ShortcutErrorKind::InvalidData,
                None,
                "The main key must be a defined non-modifier virtual key other than F12",
            ));
        }
        Ok(())
    }

    pub fn label(&self) -> String {
        if self.validate().is_err() {
            return "Invalid shortcut".into();
        }
        let Self::Chord { modifiers, key } = *self else {
            return "Win".into();
        };
        let mut parts = Vec::with_capacity(5);
        for (enabled, label) in [
            (modifiers.control, "Ctrl"),
            (modifiers.alt, "Alt"),
            (modifiers.shift, "Shift"),
            (modifiers.win, "Win"),
        ] {
            if enabled {
                parts.push(label.to_owned());
            }
        }
        parts.push(key_label(key));
        parts.join(" + ")
    }
}

// Microsoft virtual-key table and pinned windows-sys 0.61.2 named OEM keys.
// Reserved 0xC1/C2 and VK_NONAME remain excluded even though SDK names exist.
// https://learn.microsoft.com/en-us/windows/win32/inputdev/virtual-key-codes
fn assigned_key(key: u16) -> bool {
    matches!(
        key,
        0x01..=0x06 | 0x08..=0x09 | 0x0C..=0x0D | 0x10..=0x39
            | 0x41..=0x5D | 0x5F..=0x87 | 0x90..=0x96 | 0xA0..=0xB7
            | 0xBA..=0xC0 | 0xC3..=0xDF | 0xE1..=0xE7 | 0xE9..=0xFB
            | 0xFD..=0xFE
    )
}

fn key_label(key: u16) -> String {
    match key {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(u32::from(key)).unwrap().to_string(),
        0x60..=0x69 => format!("Num {}", key - 0x60),
        0x70..=0x87 => format!("F{}", key - 0x70 + 1),
        0xC3..=0xDA => GAMEPAD_LABELS[usize::from(key - 0xC3)].into(),
        _ => named_key(key).into(),
    }
}

const GAMEPAD_LABELS: [&str; 24] = [
    "Gamepad A",
    "Gamepad B",
    "Gamepad X",
    "Gamepad Y",
    "Gamepad right shoulder",
    "Gamepad left shoulder",
    "Gamepad left trigger",
    "Gamepad right trigger",
    "Gamepad up",
    "Gamepad down",
    "Gamepad left",
    "Gamepad right",
    "Gamepad menu",
    "Gamepad view",
    "Gamepad left stick",
    "Gamepad right stick",
    "Gamepad left stick up",
    "Gamepad left stick down",
    "Gamepad left stick right",
    "Gamepad left stick left",
    "Gamepad right stick up",
    "Gamepad right stick down",
    "Gamepad right stick right",
    "Gamepad right stick left",
];

fn named_key(key: u16) -> &'static str {
    match key {
        0x01 => "Mouse left",
        0x02 => "Mouse right",
        0x03 => "Cancel",
        0x04 => "Mouse middle",
        0x05 => "Mouse X1",
        0x06 => "Mouse X2",
        0x08 => "Backspace",
        0x09 => "Tab",
        0x0C => "Clear",
        0x0D => "Enter",
        0x13 => "Pause",
        0x14 => "Caps Lock",
        0x15 => "Kana / Hangul",
        0x16 => "IME On",
        0x17 => "Junja",
        0x18 => "IME Final",
        0x19 => "Hanja / Kanji",
        0x1A => "IME Off",
        0x1B => "Esc",
        0x1C => "IME Convert",
        0x1D => "IME Nonconvert",
        0x1E => "IME Accept",
        0x1F => "IME Mode Change",
        0x20 => "Space",
        0x21 => "Page Up",
        0x22 => "Page Down",
        0x23 => "End",
        0x24 => "Home",
        0x25 => "Left",
        0x26 => "Up",
        0x27 => "Right",
        0x28 => "Down",
        0x29 => "Select",
        0x2A => "Print",
        0x2B => "Execute",
        0x2C => "Print Screen",
        0x2D => "Insert",
        0x2E => "Delete",
        0x2F => "Help",
        0x5D => "Menu",
        0x5F => "Sleep",
        0x6A => "Num Multiply",
        0x6B => "Num Add",
        0x6C => "Num Separator",
        0x6D => "Num Subtract",
        0x6E => "Num Decimal",
        0x6F => "Num Divide",
        0x90 => "Num Lock",
        0x91 => "Scroll Lock",
        0x92 => "OEM NEC Equal / Jisho",
        0x93 => "OEM Masshou",
        0x94 => "OEM Touroku",
        0x95 => "OEM Loya",
        0x96 => "OEM Roya",
        0xA6 => "Browser Back",
        0xA7 => "Browser Forward",
        0xA8 => "Browser Refresh",
        0xA9 => "Browser Stop",
        0xAA => "Browser Search",
        0xAB => "Browser Favorites",
        0xAC => "Browser Home",
        0xAD => "Volume Mute",
        0xAE => "Volume Down",
        0xAF => "Volume Up",
        0xB0 => "Media Next",
        0xB1 => "Media Previous",
        0xB2 => "Media Stop",
        0xB3 => "Media Play / Pause",
        0xB4 => "Launch Mail",
        0xB5 => "Launch Media",
        0xB6 => "Launch App 1",
        0xB7 => "Launch App 2",
        0xBA => "OEM 1",
        0xBB => "OEM Plus",
        0xBC => "OEM Comma",
        0xBD => "OEM Minus",
        0xBE => "OEM Period",
        0xBF => "OEM 2",
        0xC0 => "OEM 3",
        0xDB => "OEM 4",
        0xDC => "OEM 5",
        0xDD => "OEM 6",
        0xDE => "OEM 7",
        0xDF => "OEM 8",
        0xE1 => "OEM AX",
        0xE2 => "OEM 102",
        0xE3 => "ICO Help",
        0xE4 => "ICO 00",
        0xE5 => "IME Process",
        0xE6 => "ICO Clear",
        0xE7 => "Unicode Packet",
        0xE9 => "OEM Reset",
        0xEA => "OEM Jump",
        0xEB => "OEM PA1",
        0xEC => "OEM PA2",
        0xED => "OEM PA3",
        0xEE => "OEM WS Ctrl",
        0xEF => "OEM CuSel",
        0xF0 => "OEM Attn",
        0xF1 => "OEM Finish",
        0xF2 => "OEM Copy",
        0xF3 => "OEM Auto",
        0xF4 => "OEM Enlw",
        0xF5 => "OEM Backtab",
        0xF6 => "Attn",
        0xF7 => "CrSel",
        0xF8 => "ExSel",
        0xF9 => "Erase EOF",
        0xFA => "Play",
        0xFB => "Zoom",
        0xFD => "PA1",
        0xFE => "OEM Clear",
        _ => "Invalid key",
    }
}
