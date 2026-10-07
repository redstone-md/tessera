// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use crate::DockContext;
use crate::generated::TileBounds;
use crate::popup_placement::{PopupRect, physical_anchor};
use slint::{PhysicalPosition, PhysicalSize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

impl Side {
    fn opposite(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

pub(super) fn tile_rect(
    owner: PhysicalPosition,
    scale: f32,
    bounds: &TileBounds,
) -> Result<PopupRect, String> {
    if !bounds.width.is_finite()
        || !bounds.height.is_finite()
        || bounds.width <= 0.0
        || bounds.height <= 0.0
    {
        return Err("Tooltip tile dimensions are invalid.".into());
    }
    let start = physical_anchor(owner, scale, (bounds.origin.x, bounds.origin.y))?;
    let end = physical_anchor(
        owner,
        scale,
        (
            bounds.origin.x + bounds.width,
            bounds.origin.y + bounds.height,
        ),
    )?;
    let extent = |end: i32, start: i32| {
        u32::try_from(i64::from(end) - i64::from(start))
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| "Tooltip tile dimensions are invalid.".to_string())
    };
    Ok(PopupRect {
        position: start,
        size: PhysicalSize::new(extent(end.x, start.x)?, extent(end.y, start.y)?),
    })
}

pub(super) fn place(
    context: DockContext,
    tile: PopupRect,
    logical_size: (f32, f32),
    scale: f32,
    preferred: Side,
) -> Result<PopupRect, String> {
    let fitted = crate::popup_placement::place(context, tile.position, logical_size, scale)?;
    let gap = (8.0 * f64::from(scale)).ceil() as i64;
    let left = i64::from(tile.position.x);
    let top = i64::from(tile.position.y);
    let right = left + i64::from(tile.size.width);
    let bottom = top + i64::from(tile.size.height);
    let monitor_left = i64::from(context.x());
    let monitor_top = i64::from(context.y());
    let monitor_right = monitor_left + i64::from(context.width());
    let monitor_bottom = monitor_top + i64::from(context.height());
    let width = i64::from(fitted.size.width);
    let height = i64::from(fitted.size.height);
    let room = |side| match side {
        Side::Top => top - monitor_top,
        Side::Bottom => monitor_bottom - bottom,
        Side::Left => left - monitor_left,
        Side::Right => monitor_right - right,
    };
    let needed = match preferred {
        Side::Top | Side::Bottom => height + gap,
        Side::Left | Side::Right => width + gap,
    };
    let side = if room(preferred) < needed && room(preferred.opposite()) > room(preferred) {
        preferred.opposite()
    } else {
        preferred
    };
    let centered_x = left + (i64::from(tile.size.width) - width) / 2;
    let centered_y = top + (i64::from(tile.size.height) - height) / 2;
    let (x, y) = match side {
        Side::Top => (centered_x, top - height - gap),
        Side::Bottom => (centered_x, bottom + gap),
        Side::Left => (left - width - gap, centered_y),
        Side::Right => (right + gap, centered_y),
    };
    let coordinate = |value: i64| value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    crate::popup_placement::place(
        context,
        PhysicalPosition::new(coordinate(x), coordinate(y)),
        logical_size,
        scale,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: f32, y: f32) -> TileBounds {
        TileBounds {
            origin: slint::LogicalPosition::new(x, y),
            width: 40.0,
            height: 40.0,
        }
    }

    #[test]
    fn uses_actual_negative_origin_scale_and_flips_at_the_monitor_edge() {
        let context = DockContext::new(-1920, 0, 1920, 1080, false).unwrap();
        let tile = tile_rect(PhysicalPosition::new(-1920, 900), 1.5, &bounds(16.0, 16.0)).unwrap();
        assert_eq!(tile.position, PhysicalPosition::new(-1896, 924));
        assert_eq!(tile.size, PhysicalSize::new(60, 60));
        let rect = place(context, tile, (50.0, 40.0), 1.5, Side::Top).unwrap();
        assert_eq!(rect.position, PhysicalPosition::new(-1903, 852));
        assert_eq!(rect.size, PhysicalSize::new(75, 60));
        let tile = tile_rect(PhysicalPosition::new(-100, 4), 1.0, &bounds(0.0, 0.0)).unwrap();
        let rect = place(context, tile, (100.0, 50.0), 1.0, Side::Top).unwrap();
        assert_eq!(rect.position.y, 52);
    }

    #[test]
    fn invalid_input_is_rejected_and_all_sides_stay_inside_small_monitors() {
        let context = DockContext::new(0, 0, 100, 80, false).unwrap();
        for side in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
            let tile = tile_rect(PhysicalPosition::new(90, 70), 1.0, &bounds(0.0, 0.0)).unwrap();
            let rect = place(context, tile, (266.0, 191.0), 2.0, side).unwrap();
            assert_eq!(rect.position, PhysicalPosition::new(0, 0));
            assert_eq!(rect.size, PhysicalSize::new(100, 80));
        }
        assert!(tile_rect(PhysicalPosition::default(), f32::NAN, &bounds(0.0, 0.0)).is_err());
        assert!(tile_rect(PhysicalPosition::default(), 1.0, &bounds(f32::MAX, 0.0)).is_err());
        let mut invalid = bounds(0.0, 0.0);
        invalid.width = 0.0;
        assert!(tile_rect(PhysicalPosition::default(), 1.0, &invalid).is_err());
    }
}
