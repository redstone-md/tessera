// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Closed shortcut intents and truthful registration observations.
//! Labels are presentation only, never command authority or persisted syntax.

use std::fmt;
use std::sync::Arc;

mod chord;
#[cfg(any(test, feature = "test-support"))]
pub mod mocks;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ShortcutAction {
    ToggleLauncher,
    OpenSettings,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct KeyModifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeyChord {
    BareWin,
    Chord { modifiers: KeyModifiers, key: u16 },
}

/// Source launcher is immutable; only the Settings chord is editable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShortcutConfig {
    enabled: bool,
    settings_override: Option<KeyChord>,
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            settings_override: None,
        }
    }
}

impl ShortcutConfig {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn settings_override(&self) -> Option<&KeyChord> {
        self.settings_override.as_ref()
    }

    pub fn effective_chord(&self, action: ShortcutAction) -> KeyChord {
        match action {
            ShortcutAction::ToggleLauncher => KeyChord::BareWin,
            ShortcutAction::OpenSettings => self.settings_override.unwrap_or(KeyChord::Chord {
                modifiers: KeyModifiers {
                    win: true,
                    ..KeyModifiers::default()
                },
                key: 0x4B,
            }),
        }
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_settings_override(
        mut self,
        chord: Option<KeyChord>,
    ) -> Result<Self, ShortcutError> {
        if let Some(chord) = chord {
            chord.validate()?;
            if chord == KeyChord::BareWin {
                return Err(ShortcutError::new(
                    ShortcutErrorKind::Conflict,
                    None,
                    "Settings cannot replace the launcher shortcut",
                ));
            }
        }
        self.settings_override = chord;
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShortcutErrorKind {
    Unsupported,
    AccessDenied,
    Conflict,
    InvalidData,
    Busy,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShortcutError {
    kind: ShortcutErrorKind,
    native_code: Option<u32>,
    message: &'static str,
}

impl ShortcutError {
    /// Canonical diagnostics deliberately discard arbitrary caller/native text.
    /// A bound alone cannot prevent short paths, identities or captured input
    /// leaking through Display/Debug; the typed kind and code retain diagnosis.
    pub fn new(
        kind: ShortcutErrorKind,
        native_code: Option<u32>,
        _message: impl Into<String>,
    ) -> Self {
        let message = match kind {
            ShortcutErrorKind::Unsupported => "Shortcuts are not supported.",
            ShortcutErrorKind::AccessDenied => "Shortcut access was denied.",
            ShortcutErrorKind::Conflict => "The shortcut is already in use.",
            ShortcutErrorKind::InvalidData => "The shortcut configuration is invalid.",
            ShortcutErrorKind::Busy => "The shortcut host is not accepting requests.",
            ShortcutErrorKind::Other => "The shortcut request could not be completed.",
        };
        Self {
            kind,
            native_code,
            message,
        }
    }

    pub fn kind(&self) -> ShortcutErrorKind {
        self.kind
    }

    pub fn native_code(&self) -> Option<u32> {
        self.native_code
    }

    pub fn message(&self) -> &str {
        self.message
    }
}

impl fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for ShortcutError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShortcutBindingState {
    Registered,
    Paused,
    Conflict,
    Denied,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShortcutBindingStatus {
    pub action: ShortcutAction,
    pub chord: KeyChord,
    pub state: ShortcutBindingState,
    pub error: Option<ShortcutError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShortcutSnapshot {
    pub enabled: bool,
    pub generation: u64,
    pub bindings: [ShortcutBindingStatus; 2],
}

impl ShortcutSnapshot {
    /// Results describe actual registration, not mere request acceptance.
    /// Disabled configuration has no live binding even if cleanup succeeded.
    pub fn new(
        config: &ShortcutConfig,
        generation: u64,
        launcher: Result<(), ShortcutError>,
        settings: Result<(), ShortcutError>,
    ) -> Self {
        let status = |action, result: Result<(), ShortcutError>| {
            let (state, error) = if !config.enabled() {
                (ShortcutBindingState::Paused, result.err())
            } else {
                match result {
                    Ok(()) => (ShortcutBindingState::Registered, None),
                    Err(error) => {
                        let state = match error.kind() {
                            ShortcutErrorKind::Conflict => ShortcutBindingState::Conflict,
                            ShortcutErrorKind::AccessDenied => ShortcutBindingState::Denied,
                            _ => ShortcutBindingState::Unavailable,
                        };
                        (state, Some(error))
                    }
                }
            };
            ShortcutBindingStatus {
                action,
                chord: config.effective_chord(action),
                state,
                error,
            }
        };
        Self {
            enabled: config.enabled(),
            generation,
            bindings: [
                status(ShortcutAction::ToggleLauncher, launcher),
                status(ShortcutAction::OpenSettings, settings),
            ],
        }
    }

    pub fn binding(&self, action: ShortcutAction) -> &ShortcutBindingStatus {
        self.bindings
            .iter()
            .find(|binding| binding.action == action)
            .expect("shortcut snapshots contain both fixed actions")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShortcutTrigger {
    pub generation: u64,
    pub action: ShortcutAction,
    /// Missing pointer observation stays unknown, never a fabricated monitor.
    pub cursor: Option<TriggerPoint>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShortcutEvent {
    Triggered(ShortcutTrigger),
    Unavailable(ShortcutError),
}

pub type ShortcutCompletion =
    Box<dyn FnOnce(Result<ShortcutSnapshot, ShortcutError>) + Send + 'static>;
pub type ShortcutSink = Arc<dyn Fn(ShortcutEvent) + Send + Sync + 'static>;

/// Drop disables new event admission before scheduling native cleanup.
/// An already admitted callback may finish; consumers must generation-gate.
pub trait ShortcutSubscription: Send {}

/// Prompt, nonblocking host seam. `Ok` accepts exactly one completion; `Err`
/// accepts none. Completion may run inline, after close or on an owner thread.
/// Reconfiguration invalidates old sequences; generations must never wrap.
pub trait ShortcutHost: Send + Sync + 'static {
    fn configure(
        &self,
        config: ShortcutConfig,
        completion: ShortcutCompletion,
    ) -> Result<(), ShortcutError>;

    fn subscribe(&self, sink: ShortcutSink)
    -> Result<Box<dyn ShortcutSubscription>, ShortcutError>;
}

#[cfg(test)]
#[path = "shortcuts/tests.rs"]
mod tests;

// This portable tracer does not complete the full native Rust/Slint GPL
// Seelen 1:1 goal. No WebView/Tauri/Electron/HTML or AGPL implementation/art.
// Original VM offset, exact Remix Settings-Unpin and exact bin artwork remain
// blockers; ambiguous moon stays empty. Full source matrix remains required.
