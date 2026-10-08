// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::collections::HashSet;

/// Window-logical coordinates; no renderer, scale or row handle is retained.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Bounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Bounds {
    fn valid(self) -> bool {
        [
            self.x,
            self.y,
            self.width,
            self.height,
            self.x + self.width,
            self.y + self.height,
        ]
        .into_iter()
        .all(f32::is_finite)
            && self.width > 0.0
            && self.height > 0.0
    }

    fn contains(self, point: Point) -> bool {
        point.finite()
            && point.x >= self.x
            && point.x <= self.x + self.width
            && point.y >= self.y
            && point.y <= self.y + self.height
    }

    fn center(self) -> Point {
        Point {
            x: self.x + self.width / 2.0,
            y: self.y + self.height / 2.0,
        }
    }

    fn intersection(self, other: Self) -> f64 {
        let width = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let height = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        f64::from(width.max(0.0)) * f64::from(height.max(0.0))
    }
}

impl Point {
    fn finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// Public native ListView geometry. `content_y` is its nonpositive offset, not
/// a second scroll position; `source_width` excludes present native overlays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GridMetrics {
    pub viewport: Bounds,
    pub source_width: f32,
    pub tile: f32,
    pub gap: f32,
    pub gutter: f32,
    pub columns: usize,
    pub content_y: f32,
    pub content_height: f32,
}

impl GridMetrics {
    fn valid(&self) -> bool {
        self.viewport.valid()
            && [
                self.source_width,
                self.tile,
                self.gap,
                self.gutter,
                self.content_y,
                self.content_height,
            ]
            .into_iter()
            .all(f32::is_finite)
            && self.source_width > 0.0
            && self.source_width <= self.viewport.width
            && self.tile > 0.0
            && self.gap >= 0.0
            && self.gutter >= 0.0
            && self.columns > 0
            && self.content_height >= 0.0
            && self.content_y <= 0.0
            && self.content_y >= -self.extent()
            && (self.tile + self.gap).is_finite()
            && (2.0 * self.gutter
                + self.columns as f32 * self.tile
                + self.columns.saturating_sub(1) as f32 * self.gap)
                .is_finite()
    }

    fn extent(&self) -> f32 {
        (self.content_height - self.viewport.height).max(0.0)
    }

    fn tile_bounds(&self, index: usize) -> Bounds {
        Bounds {
            x: self.viewport.x
                + self.gutter
                + (index % self.columns) as f32 * (self.tile + self.gap),
            y: self.viewport.y
                + self.gutter
                + (index / self.columns) as f32 * (self.tile + self.gap)
                + self.content_y,
            width: self.tile,
            height: self.tile,
        }
    }

    fn eligible_pointer(&self, pointer: Point) -> bool {
        self.viewport.contains(pointer) && pointer.x < self.viewport.x + self.source_width
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelativePlacement {
    Before,
    After,
}

/// An absolute identity operation, never an index into virtualized rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReorderIntent {
    pub source_key: String,
    pub anchor_key: String,
    pub placement: RelativePlacement,
}

/// A single-source transient order. The caller owns session/catalog authority
/// and cancellation; this module only translates validated geometry into IDs.
#[derive(Debug)]
pub(crate) struct FavoriteDrag {
    original: Vec<String>,
    preview: Vec<String>,
    source: String,
    bounds: Bounds,
    press: Point,
}

impl FavoriteDrag {
    pub(crate) fn begin(
        keys: &[String],
        source: &str,
        bounds: Bounds,
        press: Point,
    ) -> Option<Self> {
        if !unique_keys(keys)
            || !keys.iter().any(|key| key == source)
            || !bounds.valid()
            || !bounds.contains(press)
        {
            return None;
        }
        Some(Self {
            original: keys.to_vec(),
            preview: keys.to_vec(),
            source: source.to_owned(),
            bounds,
            press,
        })
    }

    pub(crate) fn preview_keys(&self) -> &[String] {
        &self.preview
    }

    /// Captured window-logical bounds follow the original press offset, even
    /// outside the grid or after the source delegate has been evicted.
    pub(crate) fn translated_bounds(&self, pointer: Point) -> Option<Bounds> {
        let translated = Bounds {
            x: self.bounds.x + pointer.x - self.press.x,
            y: self.bounds.y + pointer.y - self.press.y,
            ..self.bounds
        };
        translated.valid().then_some(translated)
    }

    /// Immediate edge sorting uses the translated SOURCE center, retaining the
    /// original press offset even when rows disappear or the content scrolls.
    /// False means the preview did not change, not that the gesture was ended.
    pub(crate) fn hover(&mut self, metrics: &GridMetrics, pointer: Point) -> bool {
        if !metrics.valid() || !metrics.eligible_pointer(pointer) {
            return false;
        }
        let Some(translated) = self.translated_bounds(pointer) else {
            return false;
        };
        let Some(target) = collision_target(metrics, pointer, translated, self.preview.len())
        else {
            return false;
        };
        let source = self
            .preview
            .iter()
            .position(|key| key == &self.source)
            .expect("source identity is retained in the preview");
        let target_bounds = metrics.tile_bounds(target);
        let center = translated.center().x;
        // Direct logical threshold comparisons keep exact 20%/80% equality a
        // no-op instead of introducing a division rounding decision.
        if (source > target && center < target_bounds.x + 0.2 * target_bounds.width)
            || (source < target && center > target_bounds.x + 0.8 * target_bounds.width)
        {
            let key = self.preview.remove(source);
            self.preview.insert(target, key);
            true
        } else {
            false
        }
    }

    /// Recover the entire final preview with one move relative to the original
    /// order, including reversal back to the starting slot (no completed move).
    pub(crate) fn final_intent(&self) -> Option<ReorderIntent> {
        let original = self.original.iter().position(|key| key == &self.source)?;
        let current = self.preview.iter().position(|key| key == &self.source)?;
        if original == current {
            return None;
        }
        let (anchor, placement) = if current < original {
            (self.preview.get(current + 1)?, RelativePlacement::Before)
        } else {
            (self.preview.get(current - 1)?, RelativePlacement::After)
        };
        Some(ReorderIntent {
            source_key: self.source.clone(),
            anchor_key: anchor.clone(),
            placement,
        })
    }
}

fn unique_keys(keys: &[String]) -> bool {
    let mut seen = HashSet::with_capacity(keys.len());
    keys.iter()
        .all(|key| !key.is_empty() && seen.insert(key.as_str()))
}

// The pinned @dnd-kit/collision 0.5.0 default gives pointer hits priority over
// shape intersections, then scores 1/distance or IoU/distance respectively.
// Self participates in ranking and is rejected only by the directional gate.
// Equal scores keep current preview enumeration; the browser uses registry
// insertion order, which can differ after virtualization or earlier previews.
fn collision_target(
    metrics: &GridMetrics,
    pointer: Point,
    source: Bounds,
    count: usize,
) -> Option<usize> {
    let mut best: Option<(usize, bool, f64)> = None;
    for index in 0..count {
        let target = metrics.tile_bounds(index);
        if !target.valid() || target.intersection(metrics.viewport) == 0.0 {
            continue;
        }
        let pointer_hit = target.contains(pointer);
        let intersection = source.intersection(target);
        if !pointer_hit && intersection == 0.0 {
            continue;
        }
        let center = target.center();
        let distance = (f64::from(pointer.x) - f64::from(center.x))
            .hypot(f64::from(pointer.y) - f64::from(center.y));
        let score = if pointer_hit {
            1.0 / distance
        } else {
            let union = f64::from(source.width) * f64::from(source.height)
                + f64::from(target.width) * f64::from(target.height)
                - intersection;
            intersection / union / distance
        };
        if best.is_none_or(|(_, hit, value)| {
            (pointer_hit && !hit) || (pointer_hit == hit && score > value)
        }) {
            best = Some((index, pointer_hit, score));
        }
    }
    best.map(|(index, _, _)| index)
}

/// Refill only currently resolved favorite slots in the complete saved order.
/// Missing catalog IDs retain their exact slots; stale projections fail closed.
pub(crate) fn apply_reorder(
    saved: &[String],
    resolved: &[String],
    intent: &ReorderIntent,
) -> Result<Option<Vec<String>>, String> {
    if !unique_keys(saved) || !unique_keys(resolved) {
        return Err("Favorite order contains empty or duplicate identities".into());
    }
    let membership: HashSet<&str> = resolved.iter().map(String::as_str).collect();
    let current: Vec<&str> = saved
        .iter()
        .filter(|key| membership.contains(key.as_str()))
        .map(String::as_str)
        .collect();
    if current.len() != resolved.len()
        || current
            .iter()
            .copied()
            .ne(resolved.iter().map(String::as_str))
    {
        return Err("Favorite order projection is no longer current".into());
    }
    let source = resolved
        .iter()
        .position(|key| key == &intent.source_key)
        .ok_or_else(|| "Dragged favorite is no longer resolved".to_owned())?;
    if !membership.contains(intent.anchor_key.as_str()) {
        return Err("Target favorite is no longer resolved".into());
    }
    if intent.source_key == intent.anchor_key {
        return Ok(None);
    }
    let mut reordered = resolved.to_vec();
    let key = reordered.remove(source);
    let anchor = reordered
        .iter()
        .position(|key| key == &intent.anchor_key)
        .expect("validated distinct anchor remains after removal");
    reordered.insert(
        anchor + usize::from(intent.placement == RelativePlacement::After),
        key,
    );
    if reordered == resolved {
        return Ok(None);
    }
    let mut moved = reordered.into_iter();
    Ok(Some(
        saved
            .iter()
            .map(|key| {
                if membership.contains(key.as_str()) {
                    moved
                        .next()
                        .expect("one resolved identity for each saved resolved slot")
                } else {
                    key.clone()
                }
            })
            .collect(),
    ))
}

/// Signed native `content-y` adjustment for one active-gesture 10ms tick.
/// The caller must stop its timer on zero/cancel; stationary edge pointers still
/// advance. There is no idle clock, scroll owner, or pointer recognizer here.
pub(crate) fn autoscroll_delta(metrics: &GridMetrics, pointer: Point) -> f32 {
    if !metrics.valid() || !pointer.finite() {
        return 0.0;
    }
    let viewport = metrics.viewport;
    // Reference cross-axis tolerance is 10 logical pixels. Leaving the vertical
    // viewport cancels scroll rather than accelerating outside the window.
    if pointer.x < viewport.x - 10.0
        || pointer.x > viewport.x + viewport.width + 10.0
        || pointer.y < viewport.y
        || pointer.y > viewport.y + viewport.height
    {
        return 0.0;
    }
    let zone = viewport.height * 0.2;
    let top = viewport.y + zone;
    let bottom = viewport.y + viewport.height - zone;
    let delta = if pointer.y < top {
        25.0 * ((top - pointer.y) / zone)
    } else if pointer.y > bottom {
        -25.0 * ((pointer.y - bottom) / zone)
    } else {
        0.0
    };
    (metrics.content_y + delta).clamp(-metrics.extent(), 0.0) - metrics.content_y
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn metrics() -> GridMetrics {
        GridMetrics {
            viewport: Bounds {
                x: 0.0,
                y: 0.0,
                width: 360.0,
                height: 240.0,
            },
            source_width: 360.0,
            tile: 100.0,
            gap: 20.0,
            gutter: 10.0,
            columns: 3,
            content_y: 0.0,
            content_height: 600.0,
        }
    }

    fn drag(values: &[String], index: usize, press: Option<Point>) -> FavoriteDrag {
        let bounds = metrics().tile_bounds(index);
        FavoriteDrag::begin(
            values,
            &values[index],
            bounds,
            press.unwrap_or(bounds.center()),
        )
        .unwrap()
    }

    fn intent(source: &str, anchor: &str, placement: RelativePlacement) -> ReorderIntent {
        ReorderIntent {
            source_key: source.into(),
            anchor_key: anchor.into(),
            placement,
        }
    }

    #[test]
    fn translated_bounds_retain_hotspot_outside_grid_without_changing_order() {
        let values = keys(&["A", "B", "C"]);
        let moving = drag(&values, 2, Some(Point { x: 255.0, y: 23.0 }));
        assert_eq!(
            moving.translated_bounds(Point { x: -20.0, y: -30.0 }),
            Some(Bounds {
                x: -25.0,
                y: -43.0,
                width: 100.0,
                height: 100.0,
            })
        );
        for pointer in [
            Point {
                x: f32::NAN,
                y: 0.0,
            },
            Point {
                x: 0.0,
                y: f32::INFINITY,
            },
            Point {
                x: f32::NEG_INFINITY,
                y: 0.0,
            },
        ] {
            assert!(moving.translated_bounds(pointer).is_none());
        }
        assert_eq!(moving.preview_keys(), values);
        assert_eq!(moving.final_intent(), None);
    }

    #[test]
    fn backward_and_forward_previews_recover_absolute_placement() {
        let values = keys(&["A", "B", "C", "D", "E", "F"]);
        let mut backward = drag(&values, 2, None);
        assert!(backward.hover(&metrics(), Point { x: 29.0, y: 60.0 }));
        assert_eq!(
            backward.preview_keys(),
            keys(&["C", "A", "B", "D", "E", "F"])
        );
        let completed = backward.final_intent().unwrap();
        assert_eq!(completed, intent("C", "A", RelativePlacement::Before));
        assert_eq!(
            apply_reorder(&values, &values, &completed)
                .unwrap()
                .unwrap(),
            backward.preview_keys()
        );
        assert!(!backward.hover(&metrics(), Point { x: 29.0, y: 60.0 }));
        assert_eq!(backward.final_intent(), Some(completed));

        let mut forward = drag(&values, 0, None);
        assert!(forward.hover(&metrics(), Point { x: 331.0, y: 60.0 }));
        assert_eq!(
            forward.preview_keys(),
            keys(&["B", "C", "A", "D", "E", "F"])
        );
        let completed = forward.final_intent().unwrap();
        assert_eq!(completed, intent("A", "C", RelativePlacement::After));
        assert_eq!(
            apply_reorder(&values, &values, &completed)
                .unwrap()
                .unwrap(),
            forward.preview_keys()
        );
    }

    #[test]
    fn cross_row_move_preserves_other_identities_and_reversal_is_noop() {
        let values = keys(&["A", "B", "C", "D", "E", "F"]);
        let mut moving = drag(&values, 0, None);
        assert!(moving.hover(&metrics(), Point { x: 211.0, y: 180.0 }));
        assert_eq!(moving.preview_keys(), keys(&["B", "C", "D", "E", "A", "F"]));
        assert_eq!(
            moving.final_intent(),
            Some(intent("A", "E", RelativePlacement::After))
        );
        assert!(moving.hover(&metrics(), Point { x: 29.0, y: 60.0 }));
        assert_eq!(moving.preview_keys(), values);
        assert_eq!(moving.final_intent(), None);
    }

    #[test]
    fn directional_twenty_and_eighty_percent_equality_never_sorts() {
        let values = keys(&["A", "B", "C"]);
        let mut backward = drag(&values, 2, None);
        assert!(!backward.hover(&metrics(), Point { x: 30.0, y: 60.0 }));
        assert!(backward.hover(&metrics(), Point { x: 29.0, y: 60.0 }));
        let mut forward = drag(&values, 0, None);
        assert!(!forward.hover(&metrics(), Point { x: 330.0, y: 60.0 }));
        assert!(forward.hover(&metrics(), Point { x: 331.0, y: 60.0 }));
    }

    #[test]
    fn offcenter_press_follows_source_center_not_pointer_edge() {
        let values = keys(&["A", "B", "C"]);
        let mut near_left = drag(&values, 2, Some(Point { x: 255.0, y: 60.0 }));
        // Pointer is at A's leading edge, but source center is at x=65.
        assert!(!near_left.hover(&metrics(), Point { x: 20.0, y: 60.0 }));
        let mut near_right = drag(&values, 2, Some(Point { x: 345.0, y: 60.0 }));
        // Pointer is in A's middle; translated source center is x=25.
        assert!(near_right.hover(&metrics(), Point { x: 70.0, y: 60.0 }));
        assert_eq!(near_right.preview_keys(), keys(&["C", "A", "B"]));
    }

    #[test]
    fn center_and_self_winners_do_not_fall_through_to_other_collisions() {
        let values = keys(&["A", "B", "C"]);
        let mut moving = drag(&values, 2, None);
        assert!(!moving.hover(&metrics(), Point { x: 60.0, y: 60.0 }));
        assert!(!moving.hover(&metrics(), Point { x: 251.0, y: 60.0 }));
        assert_eq!(moving.final_intent(), None);
        // The translated shape overlaps B, but C contains the pointer and wins.
        assert_eq!(
            collision_target(
                &metrics(),
                Point { x: 251.0, y: 60.0 },
                Bounds {
                    x: 201.0,
                    y: 10.0,
                    width: 100.0,
                    height: 100.0
                },
                3
            ),
            Some(2)
        );
    }

    #[test]
    fn gap_overlap_uses_ranked_collision_but_empty_gap_and_tail_do_not_clamp() {
        let values = keys(&["A", "B", "C", "D", "E", "F"]);
        let mut moving = drag(&values, 2, None);
        // Pointer is between rows. Equal overlap/distance selects the first
        // preview tile, matching stable enumeration, then its strict edge gate.
        assert!(moving.hover(&metrics(), Point { x: 20.0, y: 120.0 }));
        assert_eq!(moving.preview_keys(), keys(&["C", "A", "B", "D", "E", "F"]));

        let small_source = Bounds {
            x: 295.0,
            y: 55.0,
            width: 10.0,
            height: 10.0,
        };
        let mut gap =
            FavoriteDrag::begin(&values, "C", small_source, small_source.center()).unwrap();
        assert!(!gap.hover(&metrics(), Point { x: 120.0, y: 120.0 }));
        let partial = keys(&["A", "B", "C", "D"]);
        let mut tail = drag(&partial, 0, None);
        assert!(!tail.hover(&metrics(), Point { x: 300.0, y: 180.0 }));
        assert_eq!(tail.final_intent(), None);
    }

    #[test]
    fn overlap_ranking_is_iou_over_pointer_distance_not_max_area() {
        let grid = metrics();
        let source = Bounds {
            x: 35.0,
            y: 60.0,
            width: 160.0,
            height: 100.0,
        };
        let pointer = Point { x: 129.0, y: 100.0 };
        // A has larger intersection (75*50 versus B's 65*50), but pointer is
        // nearer B; B's IoU/distance score is larger.
        assert_eq!(collision_target(&grid, pointer, source, 3), Some(1));
        // Exact ties retain preview enumeration; no universal nearest-half rule.
        assert_eq!(
            collision_target(
                &grid,
                Point { x: 120.0, y: 60.0 },
                Bounds {
                    x: 70.0,
                    y: 10.0,
                    width: 100.0,
                    height: 100.0
                },
                3
            ),
            Some(0)
        );
    }

    #[test]
    fn outside_viewport_and_protected_native_overlay_never_mutate_preview() {
        let values = keys(&["A", "B", "C"]);
        let mut moving = drag(&values, 0, None);
        let mut grid = metrics();
        grid.source_width = 340.0;
        for point in [
            Point { x: 341.0, y: 60.0 },
            Point { x: 340.0, y: 60.0 },
            Point { x: -1.0, y: 60.0 },
            Point { x: 331.0, y: -1.0 },
            Point { x: 331.0, y: 241.0 },
        ] {
            assert!(!moving.hover(&grid, point));
        }
        assert_eq!(moving.preview_keys(), values);
    }

    #[test]
    fn partial_visible_row_and_scrolled_tail_beyond_sixty_four_use_exact_keys() {
        let values: Vec<String> = (0..100).map(|index| format!("App-{index}")).collect();
        let mut moving = drag(&values, 0, None);
        let mut grid = metrics();
        grid.content_height = 4080.0;
        grid.content_y = -3840.0;
        // Last partial row has exactly one identity at index 99.
        assert!(moving.hover(&grid, Point { x: 91.0, y: 180.0 }));
        assert_eq!(moving.preview_keys().last().unwrap(), "App-0");
        assert_eq!(
            moving.final_intent(),
            Some(intent("App-0", "App-99", RelativePlacement::After))
        );
        assert_eq!(
            apply_reorder(&values, &values, &moving.final_intent().unwrap())
                .unwrap()
                .unwrap(),
            moving.preview_keys()
        );
        let mut partial_grid = metrics();
        partial_grid.viewport.height = 150.0;
        let six = keys(&["A", "B", "C", "D", "E", "F"]);
        let mut partial = drag(&six, 0, None);
        assert!(partial.hover(&partial_grid, Point { x: 211.0, y: 140.0 }));
        assert_eq!(
            partial.final_intent(),
            Some(intent("A", "E", RelativePlacement::After))
        );
    }

    #[test]
    fn missing_interleaved_slots_and_exact_case_are_preserved() {
        let saved = keys(&["missing-1", "App", "missing-2", "app", "C", "missing-3"]);
        let resolved = keys(&["App", "app", "C"]);
        assert_eq!(
            apply_reorder(
                &saved,
                &resolved,
                &intent("C", "App", RelativePlacement::Before)
            )
            .unwrap(),
            Some(keys(&[
                "missing-1",
                "C",
                "missing-2",
                "App",
                "app",
                "missing-3"
            ]))
        );
        assert_eq!(
            apply_reorder(
                &saved,
                &resolved,
                &intent("App", "C", RelativePlacement::After)
            )
            .unwrap(),
            Some(keys(&[
                "missing-1",
                "app",
                "missing-2",
                "C",
                "App",
                "missing-3"
            ]))
        );
        assert_eq!(
            apply_reorder(
                &saved,
                &resolved,
                &intent("APP", "C", RelativePlacement::After)
            ),
            Err("Dragged favorite is no longer resolved".into())
        );
    }

    #[test]
    fn merge_rejects_invalid_projection_and_returns_none_for_safe_noops() {
        let saved = keys(&["missing", "A", "B", "C"]);
        let resolved = keys(&["A", "B", "C"]);
        for invalid in [
            keys(&["A", "A", "C"]),
            keys(&["B", "A", "C"]),
            keys(&["A", "synthetic"]),
        ] {
            assert!(
                apply_reorder(
                    &saved,
                    &invalid,
                    &intent("A", "C", RelativePlacement::After)
                )
                .is_err()
            );
        }
        assert!(
            apply_reorder(
                &keys(&["A", "A"]),
                &keys(&["A"]),
                &intent("A", "A", RelativePlacement::Before)
            )
            .is_err()
        );
        assert!(
            apply_reorder(
                &saved,
                &resolved,
                &intent("A", "missing", RelativePlacement::Before)
            )
            .is_err()
        );
        assert_eq!(
            apply_reorder(
                &saved,
                &resolved,
                &intent("A", "A", RelativePlacement::Before)
            )
            .unwrap(),
            None
        );
        assert_eq!(
            apply_reorder(
                &saved,
                &resolved,
                &intent("A", "B", RelativePlacement::Before)
            )
            .unwrap(),
            None
        );
        assert_eq!(
            apply_reorder(
                &saved,
                &resolved,
                &intent("B", "A", RelativePlacement::After)
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn begin_and_hover_fail_closed_for_invalid_identity_or_nonfinite_geometry() {
        let values = keys(&["A", "B", "C"]);
        let bounds = metrics().tile_bounds(0);
        assert!(FavoriteDrag::begin(&keys(&["A", "A"]), "A", bounds, bounds.center()).is_none());
        assert!(FavoriteDrag::begin(&keys(&[""]), "", bounds, bounds.center()).is_none());
        assert!(FavoriteDrag::begin(&values, "unknown", bounds, bounds.center()).is_none());
        assert!(FavoriteDrag::begin(&values, "A", bounds, Point { x: 0.0, y: 0.0 }).is_none());
        assert!(
            FavoriteDrag::begin(
                &values,
                "A",
                Bounds {
                    width: f32::NAN,
                    ..bounds
                },
                bounds.center()
            )
            .is_none()
        );
        let mut moving = drag(&values, 0, None);
        for grid in [
            GridMetrics {
                tile: f32::NAN,
                ..metrics()
            },
            GridMetrics {
                gap: -1.0,
                ..metrics()
            },
            GridMetrics {
                gutter: f32::INFINITY,
                ..metrics()
            },
            GridMetrics {
                columns: 0,
                ..metrics()
            },
            GridMetrics {
                source_width: 361.0,
                ..metrics()
            },
            GridMetrics {
                content_y: -361.0,
                ..metrics()
            },
            GridMetrics {
                content_height: f32::INFINITY,
                ..metrics()
            },
            GridMetrics {
                viewport: Bounds {
                    width: 0.0,
                    ..metrics().viewport
                },
                ..metrics()
            },
        ] {
            assert!(!moving.hover(&grid, Point { x: 331.0, y: 60.0 }));
            assert_eq!(autoscroll_delta(&grid, Point { x: 100.0, y: 230.0 }), 0.0);
        }
        assert!(!moving.hover(
            &metrics(),
            Point {
                x: f32::NAN,
                y: 60.0
            }
        ));
        assert_eq!(moving.final_intent(), None);
    }

    #[test]
    fn autoscroll_has_twenty_percent_zones_acceleration_and_native_clamps() {
        let mut grid = metrics();
        grid.viewport.height = 100.0;
        grid.content_y = -100.0;
        for (y, expected) in [
            (0.0, 25.0),
            (10.0, 12.5),
            (20.0, 0.0),
            (50.0, 0.0),
            (80.0, 0.0),
            (90.0, -12.5),
            (100.0, -25.0),
        ] {
            assert_eq!(autoscroll_delta(&grid, Point { x: 100.0, y }), expected);
        }
        grid.content_y = -3.0;
        assert_eq!(autoscroll_delta(&grid, Point { x: 100.0, y: 0.0 }), 3.0);
        grid.content_y = -497.0;
        assert_eq!(autoscroll_delta(&grid, Point { x: 100.0, y: 100.0 }), -3.0);
        grid.content_y = -500.0;
        assert_eq!(autoscroll_delta(&grid, Point { x: 100.0, y: 100.0 }), 0.0);
        grid.content_y = 0.0;
        assert_eq!(autoscroll_delta(&grid, Point { x: 100.0, y: 0.0 }), 0.0);
    }

    #[test]
    fn stationary_autoscroll_reaches_limit_and_cross_axis_tolerance_is_bounded() {
        let mut grid = metrics();
        let pointer = Point { x: 100.0, y: 240.0 };
        for _ in 0..20 {
            grid.content_y += autoscroll_delta(&grid, pointer);
        }
        assert_eq!(grid.content_y, -360.0);
        assert_eq!(autoscroll_delta(&grid, pointer), 0.0);
        grid.content_y = -100.0;
        for x in [-10.0, 370.0] {
            assert_eq!(autoscroll_delta(&grid, Point { x, y: 240.0 }), -25.0);
        }
        for point in [
            Point { x: -10.1, y: 240.0 },
            Point { x: 370.1, y: 240.0 },
            Point { x: 100.0, y: -0.1 },
            Point { x: 100.0, y: 240.1 },
            Point {
                x: f32::INFINITY,
                y: 240.0,
            },
        ] {
            assert_eq!(autoscroll_delta(&grid, point), 0.0);
        }
        grid.content_y = 0.0;
        grid.content_height = 240.0;
        assert_eq!(autoscroll_delta(&grid, pointer), 0.0);
    }
}
