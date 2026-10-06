// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Encapsulated installed-application records and the portable public API
//! surface for the shell application catalog.
//!
//! The DTOs carry no COM pointers or OS handles; all Win32/COM access lives
//! in [`crate::native_apps`]. Off Windows every native entry point reports
//! [`ApplicationError::UnsupportedPlatform`] (or `None` for the foreground
//! query) and never fabricates data.

/// Errors from the application catalog and launch surface.
#[derive(Debug)]
#[non_exhaustive]
pub enum ApplicationError {
    /// The current OS is not supported by this adapter.
    UnsupportedPlatform,
    /// A fatal Win32/COM operation failed. `operation` names the failing
    /// native call and `code` its HRESULT or Win32 error.
    Windows { operation: &'static str, code: u32 },
    /// The record is not a launchable shell item (e.g. a synthetic or
    /// catalog-internal entry).
    NotLaunchable,
}

impl std::fmt::Display for ApplicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(f, "application catalog requires Windows"),
            Self::Windows { operation, code } => {
                write!(
                    f,
                    "Windows operation `{operation}` failed (code 0x{code:X})"
                )
            }
            Self::NotLaunchable => write!(f, "record is not a launchable application"),
        }
    }
}

impl std::error::Error for ApplicationError {}

/// Bounded premultiplied RGBA icon pixels (at most 128×128).
///
/// Data is shared (`Arc<[u8]>`) because one catalog icon can be reused by
/// several UI surfaces. The byte order is premultiplied-alpha RGBA, top row
/// first, exactly as produced by the bounded native conversions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconPixels {
    width: u32,
    height: u32,
    rgba: std::sync::Arc<[u8]>,
}

impl IconPixels {
    /// Largest supported icon edge.
    pub const MAX_EDGE: u32 = 128;

    /// Validates and builds icon pixels from premultiplied RGBA data.
    ///
    /// Returns `None` for zero/oversized dimensions or a data length that
    /// does not match the dimensions. There is deliberately no public
    /// constructor: icons enter the shell only through the catalog or a
    /// validated class-icon copy.
    #[cfg(any(windows, test))]
    pub(crate) fn new(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        if width == 0 || height == 0 || width > Self::MAX_EDGE || height > Self::MAX_EDGE {
            return None;
        }
        let expected = usize::try_from(width)
            .ok()
            .and_then(|width| width.checked_mul(usize::try_from(height).ok()?))
            .and_then(|pixels| pixels.checked_mul(4))?;
        if rgba.len() != expected {
            return None;
        }
        Some(Self {
            width,
            height,
            rgba: rgba.into(),
        })
    }

    /// Icon width in pixels (never zero, at most [`Self::MAX_EDGE`]).
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Icon height in pixels (never zero, at most [`Self::MAX_EDGE`]).
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Premultiplied RGBA bytes, `width * height * 4` long, top row first.
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

/// One installed application from the shell's `FOLDERID_AppsFolder` catalog
/// (both Win32 and UWP entries).
///
/// Encapsulated value type; the only public way to obtain one is
/// [`catalog`], so the qualified parsing name behind [`Application::id`] is
/// always a trusted enumerated shell item, never an arbitrary caller string.
/// It must never be persisted across process runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    id: String,
    name: String,
    icon: Option<IconPixels>,
}

impl Application {
    #[cfg(windows)]
    pub(crate) fn new(id: String, name: String, icon: Option<IconPixels>) -> Self {
        Self { id, name, icon }
    }

    /// Stable qualified parsing name, e.g.
    /// `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App`. This is the identity
    /// handed to [`launch`]; unknown or stale ids are re-validated by the
    /// shell parse before any side effect. Bounded to 1,024 UTF-16 units.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Normalized display name as shown by the shell.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Icon pixels when the shell provided a readable 48×48-class image.
    pub fn icon(&self) -> Option<&IconPixels> {
        self.icon.as_ref()
    }
}

/// Maximum accepted parsing-name length in UTF-16 code units.
#[cfg(any(windows, test))]
pub(crate) const MAX_PARSING_NAME_LEN: usize = 1024;

/// Maximum accepted display-name length in UTF-16 code units.
#[cfg(windows)]
pub(crate) const MAX_DISPLAY_NAME_LEN: usize = 512;

/// Upper catalog bound; enumeration stops after this many entries.
#[cfg(windows)]
pub(crate) const MAX_CATALOG_ENTRIES: usize = 1024;

/// Validates a qualified parsing name before it is ever handed to the shell.
///
/// Nonempty, within the bound, and free of embedded NUL units (the name is
/// encoded NUL-terminated and an interior NUL would silently truncate the
/// target).
#[cfg(any(windows, test))]
pub(crate) fn valid_parsing_name(id: &str) -> bool {
    !id.is_empty() && id.encode_utf16().count() <= MAX_PARSING_NAME_LEN && !id.contains('\0')
}

/// Enumerates the shell's installed-application catalog on the calling
/// thread.
///
/// Requires a COM apartment on the calling thread; initialization is thread
/// scoped and an existing different apartment is honored. Per-item failures
/// (missing names, missing icons) are skipped, not fatal; whole-catalog
/// failures return an error and no partial list. Entries are deduplicated by
/// parsing name and sorted by display name. At most 1,024 applications are returned.
pub fn catalog() -> Result<Vec<Application>, ApplicationError> {
    #[cfg(windows)]
    {
        crate::native_apps::catalog()
    }
    #[cfg(not(windows))]
    {
        Err(ApplicationError::UnsupportedPlatform)
    }
}

/// Launches an application obtained from [`catalog`] by its qualified parsing
/// name.
///
/// The name is re-parsed by the shell into a PIDL and executed through the
/// documented IDList path; no command strings are evaluated and no elevation
/// is requested. Only freshly enumerated records are trusted — an id the
/// shell can no longer parse is refused rather than executed.
pub fn launch(application: &Application) -> Result<(), ApplicationError> {
    #[cfg(windows)]
    {
        if !valid_parsing_name(application.id()) {
            return Err(ApplicationError::NotLaunchable);
        }
        crate::native_apps::launch(application)
    }
    #[cfg(not(windows))]
    {
        let _ = application;
        Err(ApplicationError::UnsupportedPlatform)
    }
}

/// Copies a target window's class icon into bounded premultiplied RGBA
/// pixels, after revalidating the recorded identity.
///
/// Nonblocking: the shared class icon handle is queried and drawn locally
/// into an owned bitmap; it is never destroyed and no synchronous
/// cross-process message is sent. Returns `None` when the target is no
/// longer a live window of the recorded process or has no readable class
/// icon.
pub fn window_icon(target: crate::ActivationTarget) -> Option<IconPixels> {
    #[cfg(windows)]
    {
        crate::native_apps::window_icon(target)
    }
    #[cfg(not(windows))]
    {
        let _ = target;
        None
    }
}

/// Raw window handle of the current foreground window, when any exists.
///
/// App composition uses this to detect whether the shell's own surface holds
/// the foreground. Non-Windows platforms return `None`. The identity is
/// ephemeral — valid for immediate classification only, never persisted.
pub fn foreground_window_id() -> Option<tessera_core::WindowId> {
    #[cfg(windows)]
    {
        crate::native_apps::foreground_window_id()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_rejects_zero_oversized_and_mismatched_data() {
        assert!(IconPixels::new(0, 4, vec![0; 16]).is_none());
        assert!(IconPixels::new(4, 0, vec![0; 16]).is_none());
        assert!(IconPixels::new(129, 1, vec![0; 516]).is_none());
        assert!(IconPixels::new(4, 4, vec![0; 15]).is_none());
        assert!(IconPixels::new(4, 4, vec![0; 65]).is_none());
    }

    #[test]
    fn icon_accepts_exact_premultiplied_buffer() {
        let icon = IconPixels::new(4, 4, vec![0; 64]).expect("valid");
        assert_eq!((icon.width(), icon.height()), (4, 4));
        assert_eq!(icon.rgba().len(), 64);
    }

    #[test]
    fn icon_edges_bound_buffer_arithmetic() {
        assert!(IconPixels::new(u32::MAX, u32::MAX, Vec::new()).is_none());
        let max_edge = IconPixels::MAX_EDGE as usize;
        assert!(
            IconPixels::new(
                u32::try_from(max_edge).unwrap(),
                u32::try_from(max_edge).unwrap(),
                vec![0; max_edge * max_edge * 4]
            )
            .is_some()
        );
    }

    #[test]
    fn parsing_names_are_bounded_and_nul_free() {
        assert!(valid_parsing_name(
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"
        ));
        assert!(!valid_parsing_name(""));
        assert!(!valid_parsing_name("bad\0name"));
        assert!(!valid_parsing_name(&"x".repeat(MAX_PARSING_NAME_LEN + 1)));
    }
}
