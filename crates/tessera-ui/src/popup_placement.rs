// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use crate::DockContext;
use slint::{PhysicalPosition, PhysicalSize};

#[derive(Debug, PartialEq)]
pub(crate) struct PopupRect {
    pub(crate) position: PhysicalPosition,
    pub(crate) size: PhysicalSize,
}

pub(crate) fn place(
    context: DockContext,
    anchor: PhysicalPosition,
    logical_size: (f32, f32),
    scale: f32,
) -> Result<PopupRect, String> {
    place_aligned(context, anchor, logical_size, scale, false)
}

/// An interactive popup is centered on its tile, while menus retain start alignment.
pub(crate) fn place_centered(
    context: DockContext,
    anchor: PhysicalPosition,
    logical_size: (f32, f32),
    scale: f32,
) -> Result<PopupRect, String> {
    place_aligned(context, anchor, logical_size, scale, true)
}

fn place_aligned(
    context: DockContext,
    anchor: PhysicalPosition,
    logical_size: (f32, f32),
    scale: f32,
    centered: bool,
) -> Result<PopupRect, String> {
    const INVALID: &str = "Popup monitor bounds or scale are invalid.";
    if !scale.is_finite() || scale <= 0.0 {
        return Err(INVALID.into());
    }
    let dimension = |logical: f32, limit: u32| {
        let pixels = (f64::from(logical) * f64::from(scale)).ceil();
        if !pixels.is_finite() || pixels < 1.0 || pixels > f64::from(u32::MAX) || limit == 0 {
            return Err(INVALID.to_string());
        }
        Ok((pixels as u32).min(limit))
    };
    let width = dimension(logical_size.0, context.width())?;
    let height = dimension(logical_size.1, context.height())?;
    let axis = |origin: i32, extent: u32, size: u32, anchor: i64| {
        let end = i64::from(origin) + i64::from(extent);
        if end - 1 > i64::from(i32::MAX) {
            return Err(INVALID.to_string());
        }
        i32::try_from(anchor.clamp(i64::from(origin), end - i64::from(size)))
            .map_err(|_| INVALID.to_string())
    };
    Ok(PopupRect {
        position: PhysicalPosition::new(
            axis(
                context.x(),
                context.width(),
                width,
                i64::from(anchor.x) - if centered { i64::from(width) / 2 } else { 0 },
            )?,
            axis(context.y(), context.height(), height, i64::from(anchor.y))?,
        ),
        size: PhysicalSize::new(width, height),
    })
}

pub(crate) fn physical_anchor(
    origin: slint::PhysicalPosition,
    scale: f32,
    point: (f32, f32),
) -> Result<slint::PhysicalPosition, &'static str> {
    const INVALID: &str = "The context-menu input position is invalid.";
    if !scale.is_finite() || scale <= 0.0 || !point.0.is_finite() || !point.1.is_finite() {
        return Err(INVALID);
    }
    let coordinate = |origin: i32, offset: f32| {
        let value = (f64::from(origin) + f64::from(offset) * f64::from(scale)).round();
        (value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX))
            .then_some(value as i32)
            .ok_or(INVALID)
    };
    Ok(slint::PhysicalPosition::new(
        coordinate(origin.x, point.0)?,
        coordinate(origin.y, point.1)?,
    ))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_stays_on_negative_origin_monitor_at_native_scale() {
        let context = DockContext::new(-1920, -1080, 1920, 1080, false).unwrap();
        let rect = place(
            context,
            PhysicalPosition::new(-10, -10),
            (220.0, 182.0),
            1.5,
        )
        .unwrap();
        assert_eq!(rect.position, PhysicalPosition::new(-330, -273));
        assert_eq!(rect.size, PhysicalSize::new(330, 273));
        let rect = place(
            context,
            PhysicalPosition::new(-3000, -3000),
            (180.0, 92.0),
            2.0,
        )
        .unwrap();
        assert_eq!(rect.position, PhysicalPosition::new(-1920, -1080));
        assert_eq!(rect.size, PhysicalSize::new(360, 184));
    }

    #[test]
    fn rejects_unknown_or_overflowed_geometry_and_bounds_tiny_monitors() {
        let context = DockContext::new(0, 0, 100, 80, false).unwrap();
        let rect = place(context, PhysicalPosition::new(99, 79), (220.0, 182.0), 2.0).unwrap();
        assert_eq!(rect.position, PhysicalPosition::new(0, 0));
        assert_eq!(rect.size, PhysicalSize::new(100, 80));
        for (size, scale) in [
            ((220.0, 182.0), 0.0),
            ((220.0, 182.0), f32::NAN),
            ((f32::INFINITY, 182.0), 1.0),
            ((220.0, -1.0), 1.0),
            ((f32::MAX, 182.0), 2.0),
        ] {
            assert!(place(context, PhysicalPosition::new(0, 0), size, scale).is_err());
        }
    }

    #[test]
    fn centered_popup_uses_fitted_native_width_and_keeps_negative_monitor_bounds() {
        let context = DockContext::new(-1920, -1080, 1920, 1080, false).unwrap();
        let rect = place_centered(
            context,
            PhysicalPosition::new(-1200, -1030),
            (320.0, 160.0),
            1.5,
        )
        .unwrap();
        assert_eq!(rect.position, PhysicalPosition::new(-1440, -1030));
        assert_eq!(rect.size, PhysicalSize::new(480, 240));
        let rect = place_centered(
            context,
            PhysicalPosition::new(-10, -10),
            (320.0, 160.0),
            1.5,
        )
        .unwrap();
        assert_eq!(rect.position, PhysicalPosition::new(-480, -240));
        let tiny = DockContext::new(-80, -60, 80, 60, false).unwrap();
        let rect =
            place_centered(tiny, PhysicalPosition::new(-40, -30), (320.0, 160.0), 2.0).unwrap();
        assert_eq!(rect.position, PhysicalPosition::new(-80, -60));
        assert_eq!(rect.size, PhysicalSize::new(80, 60));
    }

    #[test]
    fn centered_popup_rejects_overflow_and_shadow_offset_uses_real_dpi() {
        let overflowing = DockContext::new(i32::MAX - 10, 0, 20, 80, false).unwrap();
        assert!(
            place_centered(
                overflowing,
                PhysicalPosition::new(i32::MAX, 0),
                (10.0, 10.0),
                1.0
            )
            .is_err()
        );
        let context = DockContext::new(-1920, 0, 1920, 1080, false).unwrap();
        let anchor = physical_anchor(PhysicalPosition::new(-960, 60), 1.5, (0.0, -10.0)).unwrap();
        let rect = place_centered(context, anchor, (320.0, 160.0), 1.5).unwrap();
        assert_eq!(rect.position, PhysicalPosition::new(-1200, 45));
        assert!(physical_anchor(PhysicalPosition::new(0, i32::MIN), 2.0, (0.0, -10.0)).is_err());
    }
}
