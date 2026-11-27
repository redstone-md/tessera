// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Presses of the mouse buttons that shortcuts can be bound to: the middle
//! button and the two side buttons, with keys held or without.
//!
//! Only the types and the way to bind are here so far; nothing is reported
//! until the presses are taken in the mouse hook (mouse_hook.rs).

use async_channel::{Receiver, Sender};
use std::sync::{Mutex, PoisonError};

use super::Modifiers;

/// A mouse button a shortcut can be bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(
    dead_code,
    reason = "bound by shortcuts-editor, reported by window-rules-editor"
)]
pub enum MouseButtonName {
    Middle,
    /// The first side button (`XBUTTON1`).
    Back,
    /// The second side button (`XBUTTON2`).
    Forward,
}

/// A press of a mouse button with the keys held during it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(
    dead_code,
    reason = "bound by shortcuts-editor, reported by window-rules-editor"
)]
pub struct MouseBinding {
    pub held: Modifiers,
    pub button: MouseButtonName,
}

/// The presses of mouse buttons that shortcuts are bound to.
pub struct MouseButtons;

/// The sender of the set bound last, kept so that its receiver stays open.
static BOUND: Mutex<Option<Sender<usize>>> = Mutex::new(None);

#[allow(
    dead_code,
    reason = "bound by shortcuts-editor, reported by window-rules-editor"
)]
impl MouseButtons {
    /// From now on the presses of `bindings` are swallowed and each is
    /// reported as its index in `bindings`; a new call replaces the set.
    /// Stub: nothing is reported until window-rules-editor implements it
    /// in mouse_hook.rs.
    pub fn bind(bindings: Vec<MouseBinding>) -> Receiver<usize> {
        drop(bindings);
        let (sender, receiver) = async_channel::unbounded();
        *BOUND.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender);
        receiver
    }
}
