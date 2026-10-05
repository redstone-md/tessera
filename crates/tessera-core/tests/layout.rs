//! Focused integration tests through the public interface only.

use tessera_core::{LayoutError, MainStack, Placement, Rect, Window, WindowId, WindowMode};

fn tiled(id: u64) -> Window {
    Window::new(WindowId::new(id), WindowMode::Tiled)
}

fn floating(id: u64) -> Window {
    Window::new(WindowId::new(id), WindowMode::Floating)
}

fn fullscreen(id: u64) -> Window {
    Window::new(WindowId::new(id), WindowMode::Fullscreen)
}

fn area() -> Rect {
    Rect::new(0, 0, 1000, 600).unwrap()
}

fn expected(x: i32, y: i32, w: u32, h: u32) -> Rect {
    Rect::new(x, y, w, h).unwrap()
}

#[test]
fn empty_and_all_excluded_yield_no_placements() {
    let stack = MainStack::default();
    assert!(stack.arrange(area(), &[]).unwrap().is_empty());

    let excluded = [floating(1), fullscreen(2)];
    assert!(stack.arrange(area(), &excluded).unwrap().is_empty());
}

#[test]
fn single_tiled_window_takes_whole_area() {
    let stack = MainStack::default();
    let placements = stack.arrange(area(), &[tiled(7)]).unwrap();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].window_id(), WindowId::new(7));
    assert_eq!(placements[0].rect(), area());
}

#[test]
fn two_windows_split_at_configured_percent_with_gap() {
    // Default: usable width 992, main 595, stack 397.
    // Two windows have no horizontal stack gaps, so full height for both.
    let stack = MainStack::default();
    let placements = stack.arrange(area(), &[tiled(1), tiled(2)]).unwrap();

    assert_eq!(placements[0].rect(), expected(0, 0, 595, 600));
    assert_eq!(placements[1].rect(), expected(595 + 8, 0, 397, 600));
}

#[test]
fn three_windows_split_stack_height_evenly() {
    // 3 windows: master + 2 stack panes, 1 stack gap of 8.
    // Usable stack height 600 - 8 = 592 -> 296 each (exact).
    let stack = MainStack::default();
    let placements = stack
        .arrange(area(), &[tiled(1), tiled(2), tiled(3)])
        .unwrap();

    let rects: Vec<Rect> = placements.iter().map(Placement::rect).collect();
    assert_eq!(
        rects,
        vec![
            expected(0, 0, 595, 600),
            expected(603, 0, 397, 296),
            expected(603, 296 + 8, 397, 296),
        ]
    );
}

#[test]
fn four_windows_distribute_remainder_top_down() {
    // 4 windows: master + 3 stack panes, 2 stack gaps of 8.
    // Usable stack height 600 - 16 = 584 -> base 194, remainder 2:
    // heights 195, 195, 194 top to bottom.
    let stack = MainStack::default();
    let placements = stack
        .arrange(area(), &[tiled(1), tiled(2), tiled(3), tiled(4)])
        .unwrap();

    let rects: Vec<Rect> = placements.iter().map(Placement::rect).collect();
    assert_eq!(
        rects,
        vec![
            expected(0, 0, 595, 600),
            expected(603, 0, 397, 195),
            expected(603, 195 + 8, 397, 195),
            expected(603, 195 + 8 + 195 + 8, 397, 194),
        ]
    );
}

#[test]
fn custom_ratio_floor_rounding() {
    // 33% of (100 - 8) = 92 * 33 / 100 = 30 (floor), stack 62.
    // Two panes have no stack gaps: full height 200 each.
    let stack = MainStack::new(33, 8).unwrap();
    let a = Rect::new(0, 0, 100, 200).unwrap();
    let placements = stack.arrange(a, &[tiled(1), tiled(2)]).unwrap();
    assert_eq!(placements[0].rect(), expected(0, 0, 30, 200));
    assert_eq!(placements[1].rect(), expected(38, 0, 62, 200));

    // Master plus two stack panes: (200 - 8) / 2 = 96 each.
    let placements = stack.arrange(a, &[tiled(1), tiled(2), tiled(3)]).unwrap();
    let rects: Vec<Rect> = placements.iter().map(Placement::rect).collect();
    assert_eq!(
        rects,
        vec![
            expected(0, 0, 30, 200),
            expected(38, 0, 62, 96),
            expected(38, 96 + 8, 62, 96),
        ]
    );
}

#[test]
fn zero_gap_partitions_exactly() {
    let stack = MainStack::new(60, 0).unwrap();
    let a = Rect::new(0, 0, 101, 100).unwrap();
    // main = 101 * 60 / 100 = 60, stack 41. Three panes: 100 / 3 -> 34, 33, 33.
    let placements = stack
        .arrange(a, &[tiled(1), tiled(2), tiled(3), tiled(4)])
        .unwrap();
    let rects: Vec<Rect> = placements.iter().map(Placement::rect).collect();
    assert_eq!(
        rects,
        vec![
            expected(0, 0, 60, 100),
            expected(60, 0, 41, 34),
            expected(60, 34, 41, 33),
            expected(60, 67, 41, 33),
        ]
    );
}

#[test]
fn negative_origin_monitor_shifts_all_windows() {
    let stack = MainStack::default();
    let a = Rect::new(-1920, -100, 800, 400).unwrap();
    // main = floor(792 * 0.6) = 475, stack 317. Three windows:
    // usable stack height 392 -> 196 each (exact).
    let placements = stack.arrange(a, &[tiled(1), tiled(2), tiled(3)]).unwrap();
    let rects: Vec<Rect> = placements.iter().map(Placement::rect).collect();
    assert_eq!(
        rects,
        vec![
            expected(-1920, -100, 475, 400),
            expected(-1920 + 475 + 8, -100, 317, 196),
            expected(-1920 + 475 + 8, -100 + 196 + 8, 317, 196),
        ]
    );
}

#[test]
fn excluded_windows_preserve_tiling_order() {
    let stack = MainStack::default();
    let windows = [floating(9), tiled(3), tiled(1), floating(4), tiled(2)];
    let placements = stack.arrange(area(), &windows).unwrap();
    let ids: Vec<u64> = placements.iter().map(|p| p.window_id().value()).collect();
    assert_eq!(ids, vec![3, 1, 2]);
}

#[test]
fn duplicate_ids_rejected_including_excluded_windows() {
    let stack = MainStack::default();
    let dup_tiled = [tiled(1), tiled(1)];
    assert_eq!(
        stack.arrange(area(), &dup_tiled),
        Err(LayoutError::DuplicateWindowId)
    );
    // Duplicate hidden among excluded windows is still an error.
    let dup_excluded = [tiled(1), floating(1)];
    assert_eq!(
        stack.arrange(area(), &dup_excluded),
        Err(LayoutError::DuplicateWindowId)
    );
}

#[test]
fn invalid_rect_and_settings_rejected() {
    assert_eq!(Rect::new(0, 0, 0, 10), Err(LayoutError::InvalidRect));
    assert_eq!(Rect::new(0, 0, 10, 0), Err(LayoutError::InvalidRect));
    // right edge would exceed i32::MAX
    assert_eq!(
        Rect::new(i32::MAX - 1, 0, 5, 5),
        Err(LayoutError::InvalidRect)
    );
    assert_eq!(
        Rect::new(0, 0, u32::MAX, u32::MAX),
        Err(LayoutError::InvalidRect)
    );
    assert_eq!(MainStack::new(0, 8), Err(LayoutError::InvalidMainPercent));
    assert_eq!(MainStack::new(100, 8), Err(LayoutError::InvalidMainPercent));
}

#[test]
fn maximal_valid_signed_rectangle_is_usable() {
    // Both exclusive edges reach i32::MAX without overflowing intermediates.
    let a = Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX).unwrap();
    assert_eq!(a.right(), i32::MAX);
    assert_eq!(a.bottom(), i32::MAX);
    let stack = MainStack::new(50, 0).unwrap();
    let placements = stack.arrange(a, &[tiled(1), tiled(2)]).unwrap();
    assert_eq!(
        placements[0].rect(),
        expected(i32::MIN, i32::MIN, 2_147_483_647, u32::MAX)
    );
    assert_eq!(
        placements[1].rect(),
        expected(-1, i32::MIN, 2_147_483_648, u32::MAX)
    );
}

#[test]
fn one_pixel_height_two_columns_is_valid() {
    // No horizontal gaps for two windows, so height 1 is fine even with gap 8.
    let stack = MainStack::default();
    let placements = stack
        .arrange(Rect::new(0, 0, 100, 1).unwrap(), &[tiled(1), tiled(2)])
        .unwrap();
    // main = 92 * 60 / 100 = 55, stack 37.
    assert_eq!(placements[0].rect(), expected(0, 0, 55, 1));
    assert_eq!(placements[1].rect(), expected(63, 0, 37, 1));
}

#[test]
fn too_small_area_and_extreme_gap_rejected() {
    let stack = MainStack::default();
    // Width 9: usable 1 after column gap; main = 0 -> no room for columns.
    assert_eq!(
        stack.arrange(Rect::new(0, 0, 9, 600).unwrap(), &[tiled(1), tiled(2)]),
        Err(LayoutError::InsufficientArea)
    );
    // Width 10 with gap 8: usable 2 -> main 1, stack 1: valid.
    let ok = stack
        .arrange(Rect::new(0, 0, 10, 600).unwrap(), &[tiled(1), tiled(2)])
        .unwrap();
    assert_eq!(ok[0].rect(), expected(0, 0, 1, 600));
    assert_eq!(ok[1].rect(), expected(9, 0, 1, 600));

    // Gap larger than area height with stack gaps required.
    let big_gap = MainStack::new(50, 1000).unwrap();
    assert_eq!(
        big_gap.arrange(area(), &[tiled(1), tiled(2), tiled(3)]),
        Err(LayoutError::InsufficientArea)
    );
    // Extreme gap: multiplication is checked, error not overflow.
    let huge_gap = MainStack::new(50, u32::MAX).unwrap();
    assert_eq!(
        huge_gap.arrange(area(), &[tiled(1), tiled(2), tiled(3)]),
        Err(LayoutError::InsufficientArea)
    );
    // A single window needs no internal gap, regardless of gap configuration.
    assert!(huge_gap.arrange(area(), &[tiled(1)]).is_ok());
}

#[test]
fn many_windows_collapse_pane_height_to_error() {
    // Twenty windows have nineteen stack panes and eighteen stack gaps.
    let stack = MainStack::default();
    let windows: Vec<Window> = (1..=20).map(tiled).collect();
    assert_eq!(
        stack.arrange(Rect::new(0, 0, 1000, 100).unwrap(), &windows),
        Err(LayoutError::InsufficientArea)
    );
    // Even without gaps there must be at least one pixel per stack pane.
    assert_eq!(
        MainStack::new(60, 0).unwrap().arrange(
            Rect::new(0, 0, 1000, 1).unwrap(),
            &[tiled(1), tiled(2), tiled(3)]
        ),
        Err(LayoutError::InsufficientArea)
    );
}
