use tessera_core::{LayoutError, Rect};

/// Win32 edges are exclusive. Widen before subtraction to support monitors
/// left/above the origin and the entire signed-coordinate range.
pub(crate) fn rect_from_edges(
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) -> Result<Rect, LayoutError> {
    let width =
        u32::try_from(i64::from(right) - i64::from(left)).map_err(|_| LayoutError::InvalidRect)?;
    let height =
        u32::try_from(i64::from(bottom) - i64::from(top)).map_err(|_| LayoutError::InvalidRect)?;
    Rect::new(left, top, width, height)
}

/// Geometric hint, not a fullscreen classification. Minimized windows do not
/// cover a monitor even if Windows reports a restored rectangle that would.
pub(crate) fn covers_monitor(window: Rect, minimized: bool, monitor: Rect) -> bool {
    !minimized
        && window.x() <= monitor.x()
        && window.y() <= monitor.y()
        && window.right() >= monitor.right()
        && window.bottom() >= monitor.bottom()
}

pub(crate) fn utf16_to_string_lossy(buffer: &[u16]) -> String {
    let length = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..length])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_conversion_supports_negative_and_maximal_extents() {
        assert_eq!(
            rect_from_edges(-1920, -100, 0, 900).unwrap(),
            Rect::new(-1920, -100, 1920, 1000).unwrap()
        );
        assert_eq!(
            rect_from_edges(i32::MIN, i32::MIN, i32::MAX, i32::MAX).unwrap(),
            Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX).unwrap()
        );
    }

    #[test]
    fn zero_and_inverted_edges_are_rejected() {
        assert_eq!(rect_from_edges(0, 0, 0, 5), Err(LayoutError::InvalidRect));
        assert_eq!(rect_from_edges(0, 5, 10, 5), Err(LayoutError::InvalidRect));
        assert_eq!(rect_from_edges(10, 10, 5, 5), Err(LayoutError::InvalidRect));
    }

    #[test]
    fn utf16_stops_at_nul_and_replaces_unpaired_surrogates() {
        assert_eq!(utf16_to_string_lossy(&[0x48, 0x69, 0, 0x21]), "Hi");
        assert_eq!(utf16_to_string_lossy(&[]), "");
        assert_eq!(utf16_to_string_lossy(&[0xD800, 0x41, 0]), "\u{FFFD}A");
        assert_eq!(utf16_to_string_lossy(&[0xD840, 0xDC00, 0]), "\u{20000}");
    }

    #[test]
    fn bounded_buffer_without_terminator_is_decoded_without_overread() {
        assert_eq!(utf16_to_string_lossy(&[0x61; 8]), "aaaaaaaa");
    }

    #[test]
    fn coverage_requires_full_monitor_bounds_and_not_minimized() {
        let monitor = Rect::new(-1920, -100, 1920, 1080).unwrap();
        let oversized = Rect::new(-1930, -110, 1940, 1100).unwrap();
        let work_area = Rect::new(-1920, -100, 1920, 1040).unwrap();
        assert!(covers_monitor(monitor, false, monitor));
        assert!(covers_monitor(oversized, false, monitor));
        assert!(!covers_monitor(work_area, false, monitor));
        assert!(!covers_monitor(monitor, true, monitor));
    }
}
