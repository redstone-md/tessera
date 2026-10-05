use std::fmt;

/// Errors from [`crate::observe`].
#[derive(Debug)]
#[non_exhaustive]
pub enum ObservationError {
    /// The current OS is not supported by this adapter.
    UnsupportedPlatform,
    /// A fatal Win32 (or COM/HRESULT-carrying) operation failed during
    /// enumeration or monitor collection. No partial snapshot is returned.
    Windows { operation: &'static str, code: u32 },
    /// A monitor or window reported impossible geometry.
    InvalidGeometry { operation: &'static str },
    /// A desktop-enumeration callback panicked; the observation is aborted.
    CallbackPanicked,
}

impl fmt::Display for ObservationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(f, "desktop observation requires Windows"),
            Self::Windows { operation, code } => {
                write!(
                    f,
                    "Windows operation `{operation}` failed (code 0x{code:X})"
                )
            }
            Self::InvalidGeometry { operation } => {
                write!(f, "invalid geometry reported during `{operation}`")
            }
            Self::CallbackPanicked => write!(f, "desktop enumeration callback panicked"),
        }
    }
}

impl std::error::Error for ObservationError {}

/// A non-fatal per-window read failure recorded in the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservationWarning {
    window_id: tessera_core::WindowId,
    operation: &'static str,
    code: u32,
}

impl ObservationWarning {
    #[cfg(windows)]
    pub(crate) fn new(
        window_id: tessera_core::WindowId,
        operation: &'static str,
        code: u32,
    ) -> Self {
        Self {
            window_id,
            operation,
            code,
        }
    }

    /// The window the failed read concerned.
    pub fn window_id(&self) -> tessera_core::WindowId {
        self.window_id
    }

    /// The failed operation, e.g. `"DwmGetWindowAttribute"`.
    pub fn operation(&self) -> &'static str {
        self.operation
    }

    /// Win32 error or HRESULT. Geometry validation uses ERROR_INVALID_DATA.
    pub fn code(&self) -> u32 {
        self.code
    }
}
