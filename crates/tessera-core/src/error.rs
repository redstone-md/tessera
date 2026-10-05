use std::fmt;

/// Errors produced by layout construction and arrangement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LayoutError {
    /// A rectangle extent was zero, or a coordinate/edge could not be
    /// represented as `i32`.
    InvalidRect,
    /// `main_percent` must be in `1..=99`.
    InvalidMainPercent,
    /// The work area is too small to fit the requested windows with the
    /// configured gap; every window must get a positive-size rectangle.
    InsufficientArea,
    /// The same [`crate::WindowId`] appeared more than once in the request.
    DuplicateWindowId,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRect => write!(
                f,
                "invalid rectangle: nonzero extent and representable edges required"
            ),
            Self::InvalidMainPercent => write!(f, "main percent must be between 1 and 99"),
            Self::InsufficientArea => write!(
                f,
                "work area too small for the requested windows with the configured gap"
            ),
            Self::DuplicateWindowId => write!(f, "duplicate window id in layout request"),
        }
    }
}

impl std::error::Error for LayoutError {}
