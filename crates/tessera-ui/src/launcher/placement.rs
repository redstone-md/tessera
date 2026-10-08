// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use crate::dock::DockRect;
use crate::{DockContext, LauncherDisplayMode};

/// Physical overlay placement within the supplied monitor bounds, not its work
/// area. The caller currently supplies primary-monitor context; trigger-monitor
/// selection is separate. Product fullscreen never changes toolkit fullscreen.
pub(crate) fn launcher_rect(
    context: DockContext,
    scale: f32,
    mode: LauncherDisplayMode,
) -> DockRect {
    match mode {
        LauncherDisplayMode::Windowed => crate::dock::launcher_rect(context, scale),
        LauncherDisplayMode::Fullscreen => DockRect {
            x: context.x(),
            y: context.y(),
            width: context.width(),
            height: context.height(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_is_exact_monitor_bounds_at_any_scale() {
        let context = DockContext::new(-3840, -1080, 3840, 2160, true).unwrap();
        for scale in [1.0, 2.0, 0.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                launcher_rect(context, scale, LauncherDisplayMode::Fullscreen),
                DockRect {
                    x: -3840,
                    y: -1080,
                    width: 3840,
                    height: 2160,
                }
            );
        }
    }

    #[test]
    fn windowed_preserves_source_fraction_cap_and_sanitized_scale() {
        let context = DockContext::new(-3840, -1080, 3840, 2160, false).unwrap();
        assert_eq!(
            launcher_rect(context, 1.0, LauncherDisplayMode::Windowed),
            DockRect {
                x: -2520,
                y: -594,
                width: 1200,
                height: 1188,
            }
        );
        assert_eq!(
            launcher_rect(context, 2.0, LauncherDisplayMode::Windowed),
            DockRect {
                x: -2976,
                y: -594,
                width: 2112,
                height: 1188,
            }
        );
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                launcher_rect(context, scale, LauncherDisplayMode::Windowed),
                launcher_rect(context, 1.0, LauncherDisplayMode::Windowed)
            );
        }
        let tiny = DockContext::new(-1, -1, 1, 1, false).unwrap();
        for mode in [
            LauncherDisplayMode::Windowed,
            LauncherDisplayMode::Fullscreen,
        ] {
            assert_eq!(
                launcher_rect(tiny, 1.0, mode),
                DockRect {
                    x: -1,
                    y: -1,
                    width: 1,
                    height: 1,
                }
            );
        }
    }
}
