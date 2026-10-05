// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use crate::error::LayoutError;
use crate::primitive::Rect;
use crate::window::{Window, WindowId, WindowMode};

/// Master-and-stack tiling layout.
///
/// The first eligible (tiled) window becomes the master on the left; the rest
/// are stacked vertically on the right. Gap is applied only between windows,
/// never at the outer edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MainStack {
    main_percent: u8,
    gap: u32,
}

/// A proposed rectangle for one window, produced by [`MainStack::arrange`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    window_id: WindowId,
    rect: Rect,
}

impl Placement {
    pub fn window_id(&self) -> WindowId {
        self.window_id
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }
}

impl MainStack {
    pub const DEFAULT_MAIN_PERCENT: u8 = 60;
    pub const DEFAULT_GAP: u32 = 8;

    /// Creates a layout with `main_percent` in `1..=99` and an arbitrary gap.
    pub fn new(main_percent: u8, gap: u32) -> Result<Self, LayoutError> {
        if !(1..=99).contains(&main_percent) {
            return Err(LayoutError::InvalidMainPercent);
        }
        Ok(Self { main_percent, gap })
    }

    pub fn main_percent(&self) -> u8 {
        self.main_percent
    }

    pub fn gap(&self) -> u32 {
        self.gap
    }

    /// Proposes rectangles for the tiled windows in `windows`, in request
    /// order. Floating/fullscreen windows are excluded. Duplicate ids anywhere
    /// in the request (including excluded ones) are rejected; on any error no
    /// partial plan is produced.
    pub fn arrange(&self, area: Rect, windows: &[Window]) -> Result<Vec<Placement>, LayoutError> {
        // Duplicate ids are an error even among excluded windows: the request
        // as a whole is ill-formed.
        let mut seen = std::collections::HashSet::with_capacity(windows.len());
        if !windows.iter().all(|w| seen.insert(w.id())) {
            return Err(LayoutError::DuplicateWindowId);
        }

        let tiled: Vec<WindowId> = windows
            .iter()
            .filter(|w| w.mode() == WindowMode::Tiled)
            .map(|w| w.id())
            .collect();

        if tiled.is_empty() {
            return Ok(Vec::new());
        }

        if tiled.len() == 1 {
            return Ok(vec![Placement {
                window_id: tiled[0],
                rect: area,
            }]);
        }

        // Two columns separated by one vertical gap. Master plus
        // (count - 1) stack panes separated by (count - 2) horizontal gaps.
        // No gaps at the outer edges.
        let column_gap = u64::from(self.gap);
        let stack_panes = (tiled.len() - 1) as u64;
        let stack_gaps = stack_panes - 1;

        // Column split: usable width excludes only the column gap.
        let area_w = u64::from(area.width());
        if area_w <= column_gap {
            return Err(LayoutError::InsufficientArea);
        }
        let usable_w = area_w - column_gap;
        // floor split at the configured percent; for percent <= 99 the main
        // column is always strictly narrower than the usable width.
        let main_w = usable_w * u64::from(self.main_percent) / 100;
        if main_w == 0 {
            return Err(LayoutError::InsufficientArea);
        }
        let stack_w = usable_w - main_w;

        // Stack heights: usable height excludes the stack gaps. For exactly
        // two windows there are no horizontal gaps, so any positive height
        // works regardless of the gap setting.
        let area_h = u64::from(area.height());
        let total_stack_gaps = match column_gap.checked_mul(stack_gaps) {
            Some(total) => total,
            None => return Err(LayoutError::InsufficientArea),
        };
        if area_h <= total_stack_gaps {
            return Err(LayoutError::InsufficientArea);
        }
        let usable_h = area_h - total_stack_gaps;

        // Distribute stack height top to bottom: each pane gets
        // floor(usable_h / panes), the first (usable_h % panes) panes get one
        // extra pixel. Deterministic and sum-exact.
        let base = usable_h / stack_panes;
        if base == 0 {
            return Err(LayoutError::InsufficientArea);
        }
        let remainder = usable_h % stack_panes;

        // All extent values fit u32 by construction: main_w + stack_w <=
        // area.width() and the pane heights sum to at most area.height().
        // Coordinates accumulate in i64; conversions are checked so an
        // impossible overflow surfaces as an error instead of a panic.
        let to_i32 = |v: i64| i32::try_from(v).map_err(|_| LayoutError::InvalidRect);
        let ax = i64::from(area.x());
        let ay = i64::from(area.y());
        let stack_x = ax + i64::from(area.width() - stack_w as u32);
        let stack_x = to_i32(stack_x)?;

        let mut placements = Vec::with_capacity(tiled.len());
        // Master on the left, full work-area height.
        placements.push(Placement {
            window_id: tiled[0],
            rect: Rect::new(area.x(), area.y(), main_w as u32, area.height())?,
        });

        let mut y = ay;
        for (i, id) in tiled[1..].iter().enumerate() {
            let extra = if (i as u64) < remainder { 1 } else { 0 };
            let h = base + extra;
            let y32 = to_i32(y)?;
            placements.push(Placement {
                window_id: *id,
                rect: Rect::new(stack_x, y32, stack_w as u32, h as u32)?,
            });
            y += i64::from(h as u32) + i64::from(self.gap);
        }

        Ok(placements)
    }
}

impl Default for MainStack {
    fn default() -> Self {
        Self {
            main_percent: Self::DEFAULT_MAIN_PERCENT,
            gap: Self::DEFAULT_GAP,
        }
    }
}
