// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pure layout domain for the Tessera shell.
//!
//! No OS access, no I/O: plain value types with invariants enforced by
//! constructors, and a deterministic tiling layout ([`MainStack`]).
//!
//! All coordinates are physical pixels. Negative origins are valid; extents
//! must be nonzero and the rectangle edges must fit in `i32`.

#![forbid(unsafe_code)]

mod error;
mod layout;
mod primitive;
mod window;

pub use error::LayoutError;
pub use layout::{MainStack, Placement};
pub use primitive::Rect;
pub use window::{Window, WindowId, WindowMode};
