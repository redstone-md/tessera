// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Portable dock and launcher data types.
//!
//! [`PanelPreferences`] is defined in the crate root (with the public API);
//! this module implements its bounded pin/dock behavior.
//!
//! These DTOs cross the UI boundary only: the host adapter maps native
//! application facts (identities, icons, work areas) into them on the
//! observation thread, and the presentation renders and routes them back as
//! opaque keys. Nothing here carries an OS handle, a process id, or a command
//! line; launching is always a host-side dispatch for an enumerated identity.

/// Maximum number of pinned applications the UI will store or render.
pub const MAX_PINS: usize = 32;
/// Upper bound for one opaque identity (pin or application key): 1024 UTF-16
/// code units, matching the native catalog's own identifier bound.
const KEY_BOUND_UTF16: usize = 1024;
/// Upper bound for one raw application title in UTF-16 code units.
const TITLE_BOUND_UTF16: usize = 512;

use crate::PanelPreferences;

/// Validated, premultiplied RGBA pixels for one application icon.
///
/// Built by the UI from host-supplied facts; construction is the validation
/// gate, so a `PixelIcon` always holds a complete bounded buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelIcon {
    width: u32,
    height: u32,
    rgba: std::sync::Arc<[u8]>,
}

impl PixelIcon {
    /// Maximum accepted icon side length in pixels.
    pub const MAX_SIDE: u32 = 128;

    /// Validates and stores premultiplied RGBA icon pixels.
    ///
    /// Returns `None` for oversized dimensions, a length mismatch, or an
    /// empty buffer; the host then simply omits the icon.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        if width == 0 || height == 0 || width > Self::MAX_SIDE || height > Self::MAX_SIDE {
            return None;
        }
        let expected = width as usize * height as usize * 4;
        if rgba.len() != expected {
            return None;
        }
        Some(Self {
            width,
            height,
            rgba: rgba.into(),
        })
    }

    /// Icon width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Icon height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Premultiplied RGBA bytes, row-major, exactly `width * height * 4`.
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

/// One catalogued launchable application as shown to the user.
///
/// `key` is an opaque, host-chosen identifier. It is the only value ever sent
/// back to [`crate::DesktopHost::launch`]; it must not be derived from a
/// display index or the (possibly truncated) title. The raw title is retained
/// for search; presentation sanitizes it for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelApplication {
    key: String,
    title: String,
    icon: Option<PixelIcon>,
}

impl PanelApplication {
    /// Builds one application row from already-collected host facts.
    ///
    /// Empty, control-containing, or over-long (in UTF-16 code units) keys
    /// are rejected (`None`) rather than truncated, so a stored key can never
    /// silently mismatch its persisted pin; Unicode titles are measured in
    /// code units, not bytes.
    pub fn new(key: String, title: String, icon: Option<PixelIcon>) -> Option<Self> {
        if !valid_key(&key) || title.encode_utf16().count() > TITLE_BOUND_UTF16 {
            return None;
        }
        Some(Self { key, title, icon })
    }

    /// Opaque launch key; stable within one catalog, never persisted itself.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Raw application name; search matches this, display sanitizes it.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Icon pixels when the host resolved one for this application.
    pub fn icon(&self) -> Option<&PixelIcon> {
        self.icon.as_ref()
    }
}

/// Which screen edge the dock strip hugs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DockEdge {
    /// Along the bottom of the primary work area (the default).
    #[default]
    Bottom,
    /// Along the top of the primary work area.
    Top,
    /// Along the left of the primary work area.
    Left,
    /// Along the right of the primary work area.
    Right,
}

/// The primary monitor's current work area in physical pixels.
///
/// This is the viewport the dock is bounded to; it never carries monitor
/// enumeration or other desktop geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DockContext {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    fullscreen_active: bool,
}

impl DockContext {
    /// Builds a work-area viewport; a zero-sized area is rejected.
    pub fn new(x: i32, y: i32, width: u32, height: u32, fullscreen_active: bool) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        Some(Self {
            x,
            y,
            width,
            height,
            fullscreen_active,
        })
    }

    /// Left edge of the work area; may be negative.
    pub fn x(&self) -> i32 {
        self.x
    }

    /// Top edge of the work area; may be negative.
    pub fn y(&self) -> i32 {
        self.y
    }

    /// Work-area width in physical pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Work-area height in physical pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Whether a fullscreen window currently covers the work area.
    pub fn fullscreen_active(&self) -> bool {
        self.fullscreen_active
    }
}

/// Which top-level surface [`crate::run`] presents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SurfaceMode {
    /// The original closable panel window: launcher, settings, and switcher.
    #[default]
    Panel,
    /// A slim floating dock strip on the primary monitor work area.
    Dock,
}

/// Which native shell surface a window represents.
///
/// The parent adapter validates and attaches the window's HWND through the
/// native crate's `OwnedShellSurface::attach` (via
/// [`crate::DesktopHost::configure_surface`]); the UI crate never touches
/// native handles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SurfaceKind {
    /// Bottom-center MinContent dock bar.
    #[default]
    Dock,
    /// 32px top toolbar.
    Toolbar,
    /// Frameless centered icon-grid launcher.
    Launcher,
}

/// Genuine identity text the host supplies on observations.
///
/// The UI renders these strings verbatim (bounded); it never invents values
/// when the host reports them absent — the surface simply shows what exists.
/// The clock text is refreshed only through the low-frequency clock path
/// (at most once per minute); it is not part of desktop observations.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellIdentity {
    /// Display name of the current desktop user, when known.
    pub user_name: String,
    /// Native locale clock text, when the host refreshed it.
    pub clock: String,
    /// Keyboard layout language, when known.
    pub language: String,
    /// Opaque key of the focused window from the last accepted snapshot,
    /// when it maps to a known eligible observed window.
    pub focused_window_key: Option<String>,
}

/// How the host drives the presentation loop.
///
/// `heartbeat`, when supplied, is invoked only on the UI event loop: once
/// right after the surface is shown, then every two seconds from an owned
/// Slint timer. It reports supervisor health; it never observes the desktop.
#[derive(Clone, Default)]
pub struct RunOptions {
    /// Which surface to present.
    pub surface: SurfaceMode,
    /// UI-thread heartbeat closure for the host's independent watchdog.
    pub heartbeat: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for RunOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunOptions")
            .field("surface", &self.surface)
            .field("heartbeat", &self.heartbeat.is_some())
            .finish()
    }
}

/// Host-side desktop action dispatched on an explicit user click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAction {
    /// Open the user's file manager (Explorer on Windows).
    OpenFileManager,
    /// Open the task manager.
    OpenTaskManager,
    /// Restore the Windows Explorer shell for this session and future logins.
    RestoreExplorer,
}

/// Validates one opaque key: preserved **exactly** — never sanitized,
/// truncated, or re-encoded — and only empty, control-containing, or
/// over-long (in UTF-16 code units) strings are rejected.
fn valid_key(raw: &str) -> bool {
    !raw.is_empty()
        && raw.encode_utf16().count() <= KEY_BOUND_UTF16
        && !raw.chars().any(|character| character.is_control())
}

/// Validates one persisted pin; see [`valid_key`].
fn valid_pin(raw: &str) -> bool {
    valid_key(raw)
}

impl PanelPreferences {
    /// Restricts pins to the bounded, deduplicated set the UI persists:
    /// exact strings preserved, in first-occurrence order.
    fn clamp_pins(pins: Vec<String>) -> Vec<String> {
        let mut bounded: Vec<String> = Vec::new();
        for pin in pins {
            if valid_pin(&pin) && !bounded.iter().any(|existing| existing == &pin) {
                bounded.push(pin);
                if bounded.len() == MAX_PINS {
                    break;
                }
            }
        }
        bounded
    }

    /// Builder for persisted dock placement: edge plus pinned keys, both
    /// bounded; extra pins are dropped, never silently re-derived.
    pub fn with_dock(mut self, edge: DockEdge, pins: Vec<String>) -> Self {
        self.dock_edge = edge;
        self.pins = Self::clamp_pins(pins);
        self
    }

    /// Screen edge the dock hugs when this preference is applied.
    pub fn dock_edge(&self) -> DockEdge {
        self.dock_edge
    }

    /// Persisted application keys in pin order (bounded; see [`MAX_PINS`]).
    pub fn pinned_apps(&self) -> &[String] {
        &self.pins
    }
}
