// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use slint::language::KeyEvent;
use slint::platform::Key;
use tessera_system::shortcuts::{KeyChord, KeyModifiers, ShortcutError, ShortcutErrorKind};

#[derive(Default)]
pub(super) struct Capture {
    active: bool,
    candidate: Option<(u16, KeyChord)>,
}

pub(super) enum CaptureResult {
    Ignored,
    Waiting,
    Cancelled,
    Chord(KeyChord),
    Invalid(ShortcutError),
}

impl Capture {
    pub fn active(&self) -> bool {
        self.active
    }

    pub fn begin(&mut self) {
        self.active = true;
        self.candidate = None;
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.candidate = None;
    }

    pub fn pressed(&mut self, event: &KeyEvent) -> CaptureResult {
        if !self.active {
            return CaptureResult::Ignored;
        }
        if event.text == slint::SharedString::from(Key::Escape) {
            self.cancel();
            return CaptureResult::Cancelled;
        }
        if event.repeat || modifier(&event.text) {
            return CaptureResult::Waiting;
        }
        // A second ordinary key invalidates the gesture rather than silently
        // replacing its first key. Modifier releases don't alter the down chord.
        if self.candidate.is_some() {
            self.cancel();
            return CaptureResult::Invalid(invalid());
        }
        let Some((key, intrinsic_shift)) = virtual_key(&event.text) else {
            return CaptureResult::Invalid(invalid());
        };
        let modifiers = KeyModifiers {
            control: event.modifiers.control,
            alt: event.modifiers.alt,
            shift: event.modifiers.shift || intrinsic_shift,
            win: event.modifiers.meta,
        };
        match KeyChord::new(modifiers, key) {
            Ok(chord) => {
                self.candidate = Some((key, chord));
                CaptureResult::Waiting
            }
            Err(error) => CaptureResult::Invalid(error),
        }
    }

    pub fn released(&mut self, event: &KeyEvent) -> CaptureResult {
        if !self.active {
            return CaptureResult::Ignored;
        }
        if event.text == slint::SharedString::from(Key::Escape) {
            self.cancel();
            return CaptureResult::Cancelled;
        }
        if modifier(&event.text) {
            return CaptureResult::Waiting;
        }
        let released = virtual_key(&event.text).map(|(key, _)| key);
        if self.candidate.as_ref().map(|(key, _)| *key) != released || released.is_none() {
            self.candidate = None;
            return CaptureResult::Waiting;
        }
        let (_, chord) = self.candidate.take().expect("checked candidate");
        self.active = false;
        CaptureResult::Chord(chord)
    }
}

fn invalid() -> ShortcutError {
    ShortcutError::new(
        ShortcutErrorKind::InvalidData,
        None,
        "Choose a letter, digit, navigation key or supported function key.",
    )
}

fn modifier(text: &slint::SharedString) -> bool {
    [
        Key::Control,
        Key::ControlR,
        Key::Shift,
        Key::ShiftR,
        Key::Alt,
        Key::AltGr,
        Key::Meta,
        Key::MetaR,
    ]
    .into_iter()
    .any(|key| *text == slint::SharedString::from(key))
}

/// Public Slint key values, not cloned SDK event structures or private key codes.
/// Slint's public event does not expose a physical Windows VK. OEM punctuation,
/// IME/composed text and non-Latin keys therefore fail visibly instead of guessing
/// a keyboard layout. Backtab includes the toolkit's intrinsic Shift modifier.
fn virtual_key(text: &slint::SharedString) -> Option<(u16, bool)> {
    let named = [
        (Key::Backspace, 0x08),
        (Key::Tab, 0x09),
        (Key::Return, 0x0d),
        (Key::Space, 0x20),
        (Key::PageUp, 0x21),
        (Key::PageDown, 0x22),
        (Key::End, 0x23),
        (Key::Home, 0x24),
        (Key::LeftArrow, 0x25),
        (Key::UpArrow, 0x26),
        (Key::RightArrow, 0x27),
        (Key::DownArrow, 0x28),
        (Key::Insert, 0x2d),
        (Key::Delete, 0x2e),
        (Key::F1, 0x70),
        (Key::F2, 0x71),
        (Key::F3, 0x72),
        (Key::F4, 0x73),
        (Key::F5, 0x74),
        (Key::F6, 0x75),
        (Key::F7, 0x76),
        (Key::F8, 0x77),
        (Key::F9, 0x78),
        (Key::F10, 0x79),
        (Key::F11, 0x7a),
        (Key::F12, 0x7b),
        (Key::F13, 0x7c),
        (Key::F14, 0x7d),
        (Key::F15, 0x7e),
        (Key::F16, 0x7f),
        (Key::F17, 0x80),
        (Key::F18, 0x81),
        (Key::F19, 0x82),
        (Key::F20, 0x83),
        (Key::F21, 0x84),
        (Key::F22, 0x85),
        (Key::F23, 0x86),
        (Key::F24, 0x87),
    ];
    if *text == slint::SharedString::from(Key::Backtab) {
        return Some((0x09, true));
    }
    if let Some((_, key)) = named
        .into_iter()
        .find(|(key, _)| *text == slint::SharedString::from(*key))
    {
        return Some((key, false));
    }
    let mut chars = text.chars();
    let character = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if character.is_ascii_alphabetic() || character.is_ascii_digit() {
        Some((character.to_ascii_uppercase() as u16, false))
    } else {
        None
    }
}
