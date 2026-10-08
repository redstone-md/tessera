// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Source-backed Seelen surface geometry.
//!
//! All functions take the primary **monitor bounds** (physical pixels, origins
//! may be negative) — never the Explorer work area — plus the window DPI
//! scale, and return the rectangle the presentation positions its window at.
//!
//! Logical tokens are the Seelen defaults (`SeelenWegSettings`: size 40,
//! margin 8, padding 8, space_between_items 8; `FancyToolbarSettings`:
//! item_size 16, padding 8, margin 0; default theme radius scale: bar radius
//! 16, item radius 10, icon padding 6; launcher: 7 columns, side
//! `min(0.55 * monitor side, 1200 px scaled)`). The dock is `MinContent`: its
//! length comes from the actual tile count and is centered on the monitor
//! bounds; an oversized strip is clamped into the bounds with the outer
//! margin preserved, so overflow tiles scroll inside a bounded window.
//!
//! Compact density shrinks every token by the same [`COMPACT_FACTOR`]; the
//! normal tokens stay exact.

use crate::{DockContext, DockEdge};

/// A surface rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DockRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

// Logical design tokens (Seelen defaults; see module docs).
pub(crate) const DOCK_ITEM: f32 = 40.0;
pub(crate) const DOCK_PAD: f32 = 8.0;
pub(crate) const DOCK_GAP: f32 = 8.0;
pub(crate) const DOCK_MARGIN: f32 = 8.0;
/// Toolbar: item 16 + padding 8 * 2 + margin 0 (Seelen default).
pub(crate) const TOOLBAR_HEIGHT: f32 = 32.0;
/// Launcher: Seelen centered size cap and monitor fraction.
pub(crate) const LAUNCHER_MAX_SIDE: f32 = 1200.0;
pub(crate) const LAUNCHER_MONITOR_FRACTION: f32 = 0.55;
/// Proportional shrink for compact density.
pub(crate) const COMPACT_FACTOR: f32 = 0.8;
/// Visible window-slot cap; retained overflow content scrolls, never truncates.
pub(crate) const MAX_DOCK_TILES: usize = 32;
/// Start, Show Desktop and Recycle Bin are fixed, not application model rows.
pub(crate) const RESERVED_DOCK_TILES: usize = 3;

/// A non-positive or non-finite scale falls back to 1.0.
pub(crate) fn scale_or_fallback(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Scales one logical length to whole physical pixels.
fn physical(logical: f32, scale: f32) -> i32 {
    (logical * scale).round() as i32
}

/// Logical thickness (both axes of the cross direction) of the dock window:
/// margin on each side plus the bar (bar padding + tile + bar padding).
pub(crate) fn dock_thickness(compact: bool) -> f32 {
    let factor = if compact { COMPACT_FACTOR } else { 1.0 };
    (2.0 * DOCK_MARGIN + 2.0 * DOCK_PAD + DOCK_ITEM) * factor
}

/// Logical window length for content plus the fixed reserved Dock items.
/// Overflow content scrolls; the cap bounds visible slots, not saved pins.
pub(crate) fn dock_length(tile_count: usize, compact: bool) -> f32 {
    let factor = if compact { COMPACT_FACTOR } else { 1.0 };
    let tiles = (tile_count.min(MAX_DOCK_TILES - RESERVED_DOCK_TILES) + RESERVED_DOCK_TILES) as f32;
    (2.0 * DOCK_MARGIN + 2.0 * DOCK_PAD + tiles * DOCK_ITEM + (tiles - 1.0) * DOCK_GAP) * factor
}

/// Computes the dock window rectangle for `edge` inside monitor `bounds`,
/// centered along the edge (`MinContent` by the actual tile count).
///
/// The window is clamped into the bounds so the bar (plus its margins) stays
/// fully visible; clamping keeps the outer margin where it fits.
pub(crate) fn dock_rect(
    bounds: DockContext,
    edge: DockEdge,
    tile_count: usize,
    compact: bool,
    scale: f32,
) -> DockRect {
    let scale = scale_or_fallback(scale);
    let thickness_logical = dock_thickness(compact).max(1.0);
    let mut length = (dock_length(tile_count, compact).max(1.0) * scale).round() as u32;

    let horizontal = matches!(edge, DockEdge::Bottom | DockEdge::Top);
    // Bound overflow: never longer than the strip axis minus the side margins.
    let axis = if horizontal {
        bounds.width()
    } else {
        bounds.height()
    };
    let max_length = axis.saturating_sub(2 * physical(DOCK_MARGIN, scale) as u32);
    length = length.min(max_length.max(1));

    // The window spans the outer margin twice plus the bar (bar padding,
    // tile, bar padding) across the edge; the bar rectangle itself is inside.
    let thickness = (physical(thickness_logical, scale) as u32).clamp(
        1,
        if horizontal {
            bounds.height()
        } else {
            bounds.width()
        },
    );
    let (x, y, width, height) = if horizontal {
        let y = match edge {
            DockEdge::Bottom => bounds.y() + (bounds.height() - thickness) as i32,
            _ => bounds.y(),
        };
        let x = bounds.x() + ((bounds.width() as i32 - length as i32) / 2).max(0);
        (x, y, length, thickness)
    } else {
        let x = match edge {
            DockEdge::Right => bounds.x() + (bounds.width() - thickness) as i32,
            _ => bounds.x(),
        };
        let y = bounds.y() + ((bounds.height() as i32 - length as i32) / 2).max(0);
        (x, y, thickness, length)
    };
    DockRect {
        x,
        y,
        width,
        height,
    }
}

/// Computes the toolbar window rectangle: the full monitor bounds width at the
/// top edge (Seelen default `position: Top`, margin 0), height 32 logical.
pub(crate) fn toolbar_rect(bounds: DockContext, scale: f32) -> DockRect {
    let scale = scale_or_fallback(scale);
    let height = (physical(TOOLBAR_HEIGHT, scale) as u32).clamp(1, bounds.height());
    DockRect {
        x: bounds.x(),
        y: bounds.y(),
        width: bounds.width(),
        height,
    }
}

/// Computes the centered launcher rectangle (Seelen: side
/// `min(0.55 * monitor side, 1200 * scale)`, centered on the monitor bounds).
pub(crate) fn launcher_rect(bounds: DockContext, scale: f32) -> DockRect {
    let scale = scale_or_fallback(scale);
    let cap = physical(LAUNCHER_MAX_SIDE, scale) as u32;
    let width = ((bounds.width() as f32 * LAUNCHER_MONITOR_FRACTION).round() as u32).min(cap);
    let height = ((bounds.height() as f32 * LAUNCHER_MONITOR_FRACTION).round() as u32).min(cap);
    let x = bounds.x() + ((bounds.width() as i32 - width as i32) / 2).max(0);
    let y = bounds.y() + ((bounds.height() as i32 - height as i32) / 2).max(0);
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

    fn bounds(x: i32, y: i32, width: u32, height: u32) -> DockContext {
        DockContext::new(x, y, width, height, false).unwrap()
    }

    fn tile_count(pins: usize, windows: usize) -> usize {
        (pins + windows).min(MAX_DOCK_TILES)
    }

    #[test]
    fn mincontent_dock_is_centered_and_sized_by_tile_count() {
        // Five content tiles + three reserved: margins/padding + 8*40 + 7*8.
        let rect = dock_rect(
            bounds(0, 0, 1920, 1040),
            DockEdge::Bottom,
            tile_count(3, 2),
            false,
            1.0,
        );
        assert_eq!(rect.width, 408);
        assert_eq!(rect.height, 72); // 2*8 margin + 2*8 pad + 40 item
        // Centered on the monitor bounds, hugging the bottom edge.
        assert_eq!(rect.x, (1920 - 408) / 2);
        assert_eq!(rect.y + rect.height as i32, 1040);

        // One content tile + three reserved: 16 + 16 + 4*40 + 3*8 = 216.
        let one = dock_rect(bounds(0, 0, 1920, 1040), DockEdge::Bottom, 1, false, 1.0);
        assert_eq!(one.width, 216);
        assert_eq!(one.x, (1920 - 216) / 2);
        assert_eq!(one.height, 72);

        let empty = dock_rect(bounds(0, 0, 1920, 1040), DockEdge::Bottom, 0, false, 1.0);
        assert_eq!(empty.width, 168);
        assert_eq!(empty.height, 72);
    }

    #[test]
    fn negative_origins_stay_inside_the_bounds() {
        let rect = dock_rect(
            bounds(-1920, -1080, 1920, 2160),
            DockEdge::Bottom,
            5,
            false,
            1.0,
        );
        assert_eq!(rect.x, -1920 + (1920 - 408) / 2);
        assert_eq!(rect.y + rect.height as i32, -1080 + 2160);
        assert!(rect.x >= -1920);
        assert!(rect.width >= 1 && rect.height >= 1);
    }

    #[test]
    fn dpi_scale_multiplies_tokens() {
        let rect = dock_rect(bounds(0, 0, 3840, 2160), DockEdge::Bottom, 1, false, 2.0);
        assert_eq!(rect.height, 144); // 72 * 2
        assert_eq!(rect.width, 432); // (three reserved + one content tile) * 2
        assert_eq!(rect.y + rect.height as i32, 2160);
    }

    #[test]
    fn compact_shrinks_proportionally_and_bad_scale_falls_back() {
        let compact = dock_rect(bounds(0, 0, 1920, 1040), DockEdge::Bottom, 1, true, 1.0);
        assert_eq!(compact.height, 58); // 72 * 0.8, rounded from 57.6
        assert_eq!(compact.width, 173); // 216 * 0.8, rounded from 172.8
        let fallback = dock_rect(bounds(0, 0, 1920, 1040), DockEdge::Bottom, 1, false, 0.0);
        assert_eq!(fallback.width, 216);
        assert_eq!(fallback.height, 72);
    }

    #[test]
    fn oversized_dock_is_clamped_with_margin_preserved() {
        // 32+ tiles on a tiny monitor: clamped to bounds minus side margins.
        let rect = dock_rect(bounds(0, 0, 200, 100), DockEdge::Bottom, 40, false, 1.0);
        assert_eq!(rect.width, 200 - 2 * 8);
        assert_eq!(rect.x, 8);
        assert!(rect.y + rect.height as i32 <= 100);
        assert_eq!(
            dock_length(MAX_DOCK_TILES - RESERVED_DOCK_TILES, false),
            1560.0
        );
        assert_eq!(dock_length(usize::MAX, false), 1560.0);
    }

    #[test]
    fn every_edge_is_centered_along_its_strip_axis() {
        // Top: hugging the top, centered horizontally.
        let top = dock_rect(bounds(0, 0, 1920, 1040), DockEdge::Top, 1, false, 1.0);
        assert_eq!(top.y, 0);
        assert_eq!(top.x, (1920 - 216) / 2);
        assert_eq!(top.height, 72);
        // Left: hugging the left edge, centered vertically.
        let left = dock_rect(bounds(0, 0, 1920, 1040), DockEdge::Left, 1, false, 1.0);
        assert_eq!(left.x, 0);
        assert_eq!(left.width, 72);
        assert_eq!(left.y, (1040 - 216) / 2);
        assert_eq!(left.height, 216);
        // Right: hugging the right edge, centered vertically.
        let right = dock_rect(bounds(0, 0, 1920, 1040), DockEdge::Right, 1, false, 1.0);
        assert_eq!(right.x + right.width as i32, 1920);
        assert_eq!(right.y, (1040 - 216) / 2);
        // Negative origin on a vertical edge stays inside the bounds.
        let off = dock_rect(
            bounds(-1920, -1080, 1920, 2160),
            DockEdge::Left,
            5,
            false,
            1.0,
        );
        assert_eq!(off.x, -1920);
        assert_eq!(off.y, -1080 + (2160 - 408) / 2);
    }

    #[test]
    fn toolbar_spans_bounds_at_top_with_32px_height() {
        let rect = toolbar_rect(bounds(-1920, -1080, 1920, 2160), 1.0);
        assert_eq!(rect.x, -1920);
        assert_eq!(rect.y, -1080);
        assert_eq!(rect.width, 1920);
        assert_eq!(rect.height, 32);
        let scaled = toolbar_rect(bounds(0, 0, 1920, 1040), 2.0);
        assert_eq!(scaled.height, 64);
    }

    #[test]
    fn launcher_is_centered_and_capped() {
        let rect = launcher_rect(bounds(0, 0, 1920, 1040), 1.0);
        assert_eq!(rect.width, (1920.0_f32 * 0.55).round() as u32); // 1056
        assert_eq!(rect.height, (1040.0_f32 * 0.55).round() as u32); // 572
        assert_eq!(rect.x, (1920 - 1056) / 2);
        assert_eq!(rect.y, (1040 - 572) / 2);

        // Small monitors: fraction applies; huge monitors: 1200px cap wins.
        let small = launcher_rect(bounds(0, 0, 800, 600), 1.0);
        assert_eq!(small.width, 440);
        let huge = launcher_rect(bounds(0, 0, 4000, 4000), 1.0);
        assert_eq!(huge.width, 1200);
        assert_eq!(huge.height, 1200);
        assert_eq!(huge.x, (4000 - 1200) / 2);

        // Negative origins stay centered inside the bounds.
        let off = launcher_rect(bounds(-1920, -1080, 1920, 2160), 1.0);
        assert_eq!(off.x, -1920 + (1920 - 1056) / 2);
    }
}
