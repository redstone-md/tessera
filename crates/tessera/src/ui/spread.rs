// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Where the pictures of windows go when they are spread over an area,
//! as KWin's Present Windows spreads them: in rows, as large as the area
//! gives room for, each keeping the shape of its window, in roughly the
//! order the windows stand on the screen. Task View and the overview
//! place their pictures with it.
//!
//! Everything here is in pixels, physical or logical as the caller
//! chooses, and does no drawing.

use crate::platform::ScreenRect;
use crate::tiling::Toward;

/// The places of the pictures of windows whose frames are `frames`,
/// spread over `area` with `gap` between them, in the order of `frames`.
///
/// The rows are chosen so that the pictures are as large as they can be:
/// every picture of a row has the same height, and every row too, and a
/// picture keeps the shape of its window. A picture is never larger than
/// its window; a small window gets its own size in the middle of its
/// place. The windows go into rows from the top of the screen down, and
/// within a row from the left, as they stand.
pub fn spread(frames: &[ScreenRect], area: ScreenRect, gap: i32) -> Vec<ScreenRect> {
    let count = frames.len();
    if count == 0 {
        return Vec::new();
    }
    let gap = f64::from(gap.max(0));
    let (width, height) = (
        f64::from((area.right - area.left).max(1)),
        f64::from((area.bottom - area.top).max(1)),
    );
    let sizes: Vec<(f64, f64)> = { frames.iter() }
        .map(|frame| {
            (
                f64::from((frame.right - frame.left).max(1)),
                f64::from((frame.bottom - frame.top).max(1)),
            )
        })
        .collect();
    let aspects: Vec<f64> = sizes.iter().map(|(w, h)| w / h).collect();
    // From the top down, and from the left where two stand as high.
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by_key(|&index| {
        let (x, y) = center(frames[index]);
        (y, x, index)
    });
    let ordered: Vec<f64> = order.iter().map(|&index| aspects[index]).collect();
    let splits = Splits::of(&ordered);
    let mut best: Option<(f64, Vec<std::ops::Range<usize>>)> = None;
    for rows in 1..=count {
        let groups = splits.rows(rows);
        let along = |group: &std::ops::Range<usize>| {
            let sum: f64 = ordered[group.clone()].iter().sum();
            (width - gap * (group.len() - 1) as f64).max(1.) / sum
        };
        let across = (height - gap * (rows - 1) as f64).max(1.) / rows as f64;
        let line = groups.iter().map(along).fold(across, f64::min);
        if best.as_ref().is_none_or(|(most, _)| line > *most) {
            best = Some((line, groups));
        }
    }
    let Some((line, groups)) = best else {
        return Vec::new();
    };
    let used = line * groups.len() as f64 + gap * (groups.len() - 1) as f64;
    let mut top = f64::from(area.top) + (height - used) / 2.;
    let mut placed = vec![area; count];
    for group in groups {
        let mut row: Vec<usize> = order[group].to_vec();
        row.sort_by_key(|&index| {
            let (x, y) = center(frames[index]);
            (x, y, index)
        });
        let length: f64 = row.iter().map(|&index| aspects[index] * line).sum::<f64>()
            + gap * (row.len() - 1) as f64;
        let mut left = f64::from(area.left) + (width - length) / 2.;
        for index in row {
            let slot = aspects[index] * line;
            // Never larger than the window itself.
            let shrink = (sizes[index].1 / line).min(1.);
            let (w, h) = (slot * shrink, line * shrink);
            let (x, y) = (left + (slot - w) / 2., top + (line - h) / 2.);
            placed[index] = ScreenRect {
                left: x.round() as i32,
                top: y.round() as i32,
                right: (x + w).round().max(x.round() + 1.) as i32,
                bottom: (y + h).round().max(y.round() + 1.) as i32,
            };
            left += slot + gap;
        }
        top += line + gap;
    }
    placed
}

/// The ways to split windows that follow each other into rows that
/// follow each other, each row as short as can be: for every number of
/// rows, the split whose longest row is shortest.
struct Splits {
    /// `cut[rows][end]`: where the last of `rows` rows over the first
    /// `end` windows begins.
    cut: Vec<Vec<usize>>,
    count: usize,
}

impl Splits {
    /// Works out the splits of windows whose shapes, width over height,
    /// are `aspects`: how long a row of them is at a height of one.
    fn of(aspects: &[f64]) -> Self {
        let count = aspects.len();
        let mut sums = vec![0.; count + 1];
        for (index, aspect) in aspects.iter().enumerate() {
            sums[index + 1] = sums[index] + aspect;
        }
        let span = |from: usize, to: usize| sums[to] - sums[from];
        // `longest[rows][end]`: the longest row of the best split of the
        // first `end` windows into `rows` rows.
        let mut longest = vec![vec![f64::INFINITY; count + 1]; count + 1];
        let mut cut = vec![vec![0; count + 1]; count + 1];
        for (end, longest) in longest[1].iter_mut().enumerate().skip(1) {
            *longest = span(0, end);
        }
        for rows in 2..=count {
            for end in rows..=count {
                for start in rows - 1..end {
                    let longer = longest[rows - 1][start].max(span(start, end));
                    if longer < longest[rows][end] {
                        longest[rows][end] = longer;
                        cut[rows][end] = start;
                    }
                }
            }
        }
        Self { cut, count }
    }

    /// The windows of each of `rows` rows, from the first row.
    fn rows(&self, rows: usize) -> Vec<std::ops::Range<usize>> {
        let mut groups = Vec::with_capacity(rows);
        let mut end = self.count;
        for left in (1..=rows).rev() {
            let start = if left == 1 { 0 } else { self.cut[left][end] };
            groups.push(start..end);
            end = start;
        }
        groups.reverse();
        groups
    }
}

/// Of `rects`, the one that lies `toward` the one at `from`: the nearest
/// one whose middle is on that side, a step aside counting twice as far
/// as a step ahead. `None` where none lies that way.
pub fn nearest(from: usize, rects: &[ScreenRect], toward: Toward) -> Option<usize> {
    let start = *rects.get(from)?;
    { rects.iter().enumerate() }
        .filter(|&(index, _)| index != from)
        .filter_map(|(index, &other)| Some((reach(start, other, toward)?, index)))
        .min()
        .map(|(_, index)| index)
}

/// How far `other` lies `toward` from `from`, by their middles, a step
/// aside counting twice as far as a step ahead; `None` where its middle
/// is not on that side.
pub fn reach(from: ScreenRect, other: ScreenRect, toward: Toward) -> Option<i32> {
    let ((x, y), (other_x, other_y)) = (center(from), center(other));
    let (ahead, aside) = match toward {
        Toward::Left => (x - other_x, other_y - y),
        Toward::Right => (other_x - x, other_y - y),
        Toward::Up => (y - other_y, other_x - x),
        Toward::Down => (other_y - y, other_x - x),
    };
    (ahead > 0).then(|| ahead + 2 * aside.abs())
}

/// The rectangle `amount` of the way from `from` to `to`, kept at least
/// one pixel wide and high, as a window can be drawn; an amount above 1
/// lies beyond `to`.
pub fn between(from: ScreenRect, to: ScreenRect, amount: f32) -> ScreenRect {
    let mix = |from: i32, to: i32| from + ((to - from) as f32 * amount).round() as i32;
    let (left, top) = (mix(from.left, to.left), mix(from.top, to.top));
    ScreenRect {
        left,
        top,
        right: mix(from.right, to.right).max(left + 1),
        bottom: mix(from.bottom, to.bottom).max(top + 1),
    }
}

/// `rect` moved by `dx` and `dy`.
pub fn shifted(rect: ScreenRect, dx: i32, dy: i32) -> ScreenRect {
    ScreenRect {
        left: rect.left + dx,
        top: rect.top + dy,
        right: rect.right + dx,
        bottom: rect.bottom + dy,
    }
}

/// `rect` made `scale` times as large around its middle.
pub fn scaled(rect: ScreenRect, scale: f32) -> ScreenRect {
    let (x, y) = center(rect);
    let half = |length: i32| (length as f32 * scale / 2.).round() as i32;
    let (w, h) = (half(rect.right - rect.left), half(rect.bottom - rect.top));
    ScreenRect {
        left: x - w,
        top: y - h,
        right: x + w.max(1),
        bottom: y + h.max(1),
    }
}

/// Whether the point `x`, `y` lies in `rect`.
pub fn holds(rect: ScreenRect, (x, y): (i32, i32)) -> bool {
    (rect.left..rect.right).contains(&x) && (rect.top..rect.bottom).contains(&y)
}

/// The middle of `rect`.
pub fn center(rect: ScreenRect) -> (i32, i32) {
    ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: ScreenRect = ScreenRect {
        left: 0,
        top: 100,
        right: 1920,
        bottom: 1080,
    };

    fn rect(left: i32, top: i32, width: i32, height: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }

    fn overlap(one: ScreenRect, other: ScreenRect) -> bool {
        one.left < other.right
            && other.left < one.right
            && one.top < other.bottom
            && other.top < one.bottom
    }

    /// Windows of a few shapes and sizes, spread over a screen.
    fn windows(count: usize) -> Vec<ScreenRect> {
        (0..count)
            .map(|index| {
                let index = index as i32;
                let (width, height) = match index % 4 {
                    0 => (1280, 720),
                    1 => (800, 900),
                    2 => (1600, 1000),
                    _ => (400, 300),
                };
                rect((index * 173) % 1500, (index * 97) % 700, width, height)
            })
            .collect()
    }

    #[test]
    fn from_one_to_thirty_windows_every_picture_fits_and_none_overlaps() {
        for count in 1..=30 {
            let frames = windows(count);
            let placed = spread(&frames, AREA, 16);
            assert_eq!(placed.len(), count);
            for (index, (&place, &frame)) in placed.iter().zip(&frames).enumerate() {
                assert!(AREA.covers(place), "{count} windows: {index} is outside");
                let (width, height) = (place.right - place.left, place.bottom - place.top);
                let (whole_width, whole_height) =
                    (frame.right - frame.left, frame.bottom - frame.top);
                // Never larger than the window, and of its shape.
                assert!(width <= whole_width + 1 && height <= whole_height + 1);
                let shape = width as f32 / height as f32;
                let whole = whole_width as f32 / whole_height as f32;
                assert!(
                    (shape - whole).abs() / whole < 0.05 || height < 40,
                    "{count} windows: {index} lost its shape"
                );
                for (other_index, &other) in placed.iter().enumerate() {
                    if other_index != index {
                        assert!(!overlap(place, other), "{count}: {index} on {other_index}");
                    }
                }
            }
        }
    }

    #[test]
    fn one_window_stands_in_the_middle_and_is_not_enlarged() {
        let small = rect(10, 10, 400, 300);
        assert_eq!(spread(&[small], AREA, 16), [rect(760, 440, 400, 300)]);
        // A large one fills the height.
        let large = rect(0, 0, 3840, 2160);
        let [placed] = spread(&[large], AREA, 16)[..] else {
            panic!("one window, one place");
        };
        assert_eq!((placed.top, placed.bottom), (AREA.top, AREA.bottom));
        assert_eq!(placed.left + placed.right, AREA.right);
    }

    #[test]
    fn windows_keep_their_order_on_the_screen() {
        let left = rect(0, 0, 900, 1000);
        let right = rect(1000, 0, 900, 1000);
        let placed = spread(&[right, left], AREA, 16);
        assert!(placed[1].right <= placed[0].left);
        // A row of many narrow windows goes into several rows, the upper
        // windows first.
        let frames: Vec<ScreenRect> = (0..9)
            .map(|index| rect(index * 100, index * 50, 1600, 1000))
            .collect();
        let placed = spread(&frames, AREA, 16);
        let tops: Vec<i32> = placed.iter().map(|place| place.top).collect();
        assert!(tops.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(tops[0] < tops[8]);
    }

    #[test]
    fn rows_are_chosen_for_the_largest_pictures() {
        // Four wide windows on a wide area: two rows of two give them more
        // than one row of four or four rows.
        let frames = vec![rect(0, 0, 1920, 1080); 4];
        let area = rect(0, 0, 1920, 1080);
        let placed = spread(&frames, area, 0);
        let tops: std::collections::BTreeSet<i32> = placed.iter().map(|place| place.top).collect();
        assert_eq!(tops.len(), 2);
        assert_eq!(placed[0].right - placed[0].left, 960);
    }

    #[test]
    fn windows_without_a_size_are_placed_all_the_same() {
        let frames = [rect(0, 0, 0, 0), rect(50, 50, -10, 20)];
        let placed = spread(&frames, AREA, 8);
        assert_eq!(placed.len(), 2);
        assert!(placed.iter().all(|place| AREA.covers(*place)));
        assert!(spread(&[], AREA, 8).is_empty());
    }

    #[test]
    fn the_nearest_picture_lies_on_the_side_the_arrow_points_to() {
        // 0 1 2
        //  3 4
        let rects = [
            rect(0, 0, 100, 100),
            rect(200, 0, 100, 100),
            rect(400, 0, 100, 100),
            rect(100, 200, 100, 100),
            rect(300, 200, 100, 100),
        ];
        assert_eq!(nearest(0, &rects, Toward::Right), Some(1));
        assert_eq!(nearest(1, &rects, Toward::Left), Some(0));
        assert_eq!(nearest(0, &rects, Toward::Left), None);
        assert_eq!(nearest(2, &rects, Toward::Down), Some(4));
        assert_eq!(nearest(3, &rects, Toward::Up), Some(0));
        assert_eq!(nearest(3, &rects, Toward::Right), Some(4));
        assert_eq!(nearest(4, &rects, Toward::Down), None);
        assert_eq!(nearest(9, &rects, Toward::Down), None);
    }

    #[test]
    fn a_rectangle_moves_between_two() {
        let from = rect(0, 0, 100, 100);
        let to = rect(100, 200, 300, 100);
        assert_eq!(between(from, to, 0.), from);
        assert_eq!(between(from, to, 1.), to);
        assert_eq!(between(from, to, 0.5), rect(50, 100, 200, 100));
        assert_eq!(scaled(rect(0, 0, 100, 50), 0.5), rect(25, 12, 50, 26));
        assert_eq!(shifted(from, 5, -5), rect(5, -5, 100, 100));
        assert!(holds(from, (0, 99)) && !holds(from, (100, 0)));
    }
}
