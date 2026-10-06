// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pure dock placement math.
//!
//! [`dock_rect`] maps a [`DockContext`] work-area viewport (physical pixels,
//! origins may be negative) and the configured edge into the strip rectangle
//! the presentation positions its window at. The strip always spans the full
//! length of the chosen edge; only its thickness varies with density, edge,
//! and DPI scale. Oversized thickness is clamped into the work area.

use crate::{DockContext, DockEdge};

/// A dock strip rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DockRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Logical thickness of the dock strip at normal density.
pub(crate) const DOCK_THICKNESS: u32 = 72;
/// Logical thickness of the dock strip at compact density.
pub(crate) const DOCK_THICKNESS_COMPACT: u32 = 56;

/// Computes the dock strip for `edge` inside `area`, scaled by `scale`.
///
/// A non-positive or missing scale falls back to 1.0; the thickness is scaled,
/// rounded to whole physical pixels, and clamped into the work area so the
/// strip is always fully inside the viewport (including negative origins).
pub(crate) fn dock_rect(
    area: DockContext,
    edge: DockEdge,
    thickness_logical: u32,
    scale: f32,
) -> DockRect {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let scaled = (thickness_logical as f32 * scale).round() as u32;
    let thickness = scaled.clamp(1, area.width().min(area.height()));

    let horizontal = matches!(edge, DockEdge::Bottom | DockEdge::Top);
    let (x, y, width, height) = if horizontal {
        let y = match edge {
            DockEdge::Bottom => area.y() + (area.height() - thickness) as i32,
            _ => area.y(),
        };
        (area.x(), y, area.width(), thickness)
    } else {
        let x = match edge {
            DockEdge::Right => area.x() + (area.width() - thickness) as i32,
            _ => area.x(),
        };
        (x, area.y(), thickness, area.height())
    };
    DockRect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DockContext, DockEdge};

    fn area(x: i32, y: i32, width: u32, height: u32) -> DockContext {
        DockContext::new(x, y, width, height, false).unwrap()
    }

    #[test]
    fn bottom_strip_hugs_work_area_with_scaled_thickness() {
        let rect = dock_rect(area(0, 0, 1920, 1040), DockEdge::Bottom, 56, 1.0);
        assert_eq!(
            rect,
            DockRect {
                x: 0,
                y: 984,
                width: 1920,
                height: 56
            }
        );

        let scaled = dock_rect(area(0, 0, 1920, 1040), DockEdge::Bottom, 56, 2.0);
        assert_eq!(
            scaled,
            DockRect {
                x: 0,
                y: 928,
                width: 1920,
                height: 112
            }
        );
    }

    #[test]
    fn negative_origins_stay_inside_the_viewport() {
        let rect = dock_rect(area(-1920, -1080, 1920, 2160), DockEdge::Bottom, 40, 1.0);
        assert_eq!(rect.y, -1080 + 2160 - 40);
        assert_eq!(rect.x, -1920);

        let left = dock_rect(area(-1920, 0, 1920, 1040), DockEdge::Left, 40, 1.0);
        assert_eq!(
            left,
            DockRect {
                x: -1920,
                y: 0,
                width: 40,
                height: 1040
            }
        );

        let right = dock_rect(area(-1920, 0, 1920, 1040), DockEdge::Right, 40, 1.0);
        assert_eq!(right.x, -1920 + 1920 - 40);
        assert_eq!(right.height, 1040);
    }

    #[test]
    fn oversized_thickness_is_clamped_and_bad_scale_falls_back() {
        let clamped = dock_rect(area(0, 0, 100, 100), DockEdge::Top, 500, 1.0);
        assert_eq!(
            clamped,
            DockRect {
                x: 0,
                y: 0,
                width: 100,
                height: 100
            }
        );

        let fallback = dock_rect(area(0, 0, 1920, 1040), DockEdge::Top, 56, 0.0);
        assert_eq!(fallback.height, 56);
    }

    #[test]
    fn left_and_right_strips_are_vertical() {
        let left = dock_rect(area(0, 0, 1920, 1040), DockEdge::Left, 56, 1.0);
        assert_eq!(left.width, 56);
        assert_eq!(left.height, 1040);
        assert_eq!(left.x, 0);

        let right = dock_rect(area(0, 0, 1920, 1040), DockEdge::Right, 56, 1.0);
        assert_eq!(right.x, 1920 - 56);
    }
}
