// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `cover` and `flip` styles of the switcher: the pictures of the
//! windows on a stage, the chosen one in front and the others around it,
//! smaller and fainter the further they are from it.
//!
//! Windows draws the picture of a window as an upright rectangle and
//! cannot tilt or turn it, so neither style turns anything in depth: the
//! pictures only move, grow, shrink and fade. Nor do two pictures overlap
//! where the choice rests, since Windows, not Tessera, decides which of
//! two pictures is drawn over the other.
//!
//! Everything is in logical pixels from the top left corner of the stage,
//! and `position` is the place of the chosen window among the windows, a
//! fraction while the choice moves from one to the next.

/// How much smaller each window is than the one nearer to the chosen one.
pub const STEP: f32 = 0.8;

/// The box the chosen window is fitted into, as shares of the width and
/// the height of the stage: in `cover` it stands in the middle, in `flip`
/// at the bottom.
const COVER_BOX: (f32, f32) = (0.24, 0.8);
const FLIP_BOX: (f32, f32) = (0.46, 0.34);

/// The space between two boxes.
const GAP: f32 = 16.;

/// How much fainter each window is than the one nearer to the chosen one.
const COVER_FADE: f32 = 0.22;
const FLIP_FADE: f32 = 0.2;

/// How far to the side each window of `flip` stands from the one in front
/// of it, as a share of the width of the box of the chosen one.
const FLIP_INWARD: f32 = 0.1;

/// Where a picture stands on the stage and how opaque it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub opacity: f32,
}

impl Placed {
    fn right(&self) -> f32 {
        self.left + self.width
    }

    fn bottom(&self) -> f32 {
        self.top + self.height
    }

    /// Whether it lies on `stage` and is to be seen at all.
    fn shown_on(self, (width, height): (f32, f32)) -> Option<Self> {
        const SLACK: f32 = 0.5;
        let within = self.left >= -SLACK
            && self.top >= -SLACK
            && self.right() <= width + SLACK
            && self.bottom() <= height + SLACK;
        (within && self.opacity > 0.01).then_some(self)
    }
}

/// The `cover` style: the chosen window large in the middle of `stage`,
/// the windows before it to the left and those after it to the right,
/// each [`STEP`] the size of the one nearer to the middle and fainter,
/// side by side, as many as the stage has room for. `sizes` are the logical sizes of the windows; a window
/// that would stand beyond the stage, or is too faint to be seen, is
/// `None`.
pub fn cover(stage: (f32, f32), sizes: &[(f32, f32)], position: f32) -> Vec<Option<Placed>> {
    let base = (stage.0 * COVER_BOX.0, stage.1 * COVER_BOX.1);
    { sizes.iter().enumerate() }
        .map(|(index, &size)| {
            let away = index as f32 - position;
            let distance = away.abs();
            let scale = STEP.powf(distance);
            let (width, height) = fit(size, (base.0 * scale, base.1 * scale));
            let middle = stage.0 / 2. + run(base.0, distance) * away.signum();
            Placed {
                left: middle - width / 2.,
                top: (stage.1 - height) / 2.,
                width,
                height,
                opacity: (1. - COVER_FADE * distance).clamp(0., 1.),
            }
            .shown_on(stage)
        })
        .collect()
}

/// The `flip` style: the chosen window at the bottom of `stage`, and the
/// windows after it stacked upward and to the side behind it, each
/// [`STEP`] the size of the one in front of it, higher and fainter. The
/// windows before the chosen one fade out where it stands.
pub fn flip(stage: (f32, f32), sizes: &[(f32, f32)], position: f32) -> Vec<Option<Placed>> {
    let base = (stage.0 * FLIP_BOX.0, stage.1 * FLIP_BOX.1);
    { sizes.iter().enumerate() }
        .map(|(index, &size)| {
            let away = index as f32 - position;
            if away <= -1. {
                return None;
            }
            let behind = away.max(0.);
            let scale = STEP.powf(behind);
            let (width, height) = fit(size, (base.0 * scale, base.1 * scale));
            let bottom = stage.1 - climb(base.1, behind);
            let middle = stage.0 / 2. + base.0 * FLIP_INWARD * behind;
            let opacity = match away < 0. {
                // Gone by the time the next one is half way to the front.
                true => (1. + away * 2.).max(0.),
                false => (1. - FLIP_FADE * behind).max(0.),
            };
            Placed {
                left: middle - width / 2.,
                top: bottom - height,
                width,
                height,
                opacity,
            }
            .shown_on(stage)
        })
        .collect()
}

/// `size` made as large as `room` allows, keeping its shape, and never
/// larger than it is.
fn fit((width, height): (f32, f32), room: (f32, f32)) -> (f32, f32) {
    let (width, height) = (width.max(1.), height.max(1.));
    let factor = (room.0 / width).min(room.1 / height).min(1.);
    (width * factor, height * factor)
}

/// How far from the middle of the stage the middle of the box `distance`
/// steps from the chosen one stands, the chosen one being `width` wide:
/// each box is [`STEP`] as wide as the one before it, and [`GAP`] apart.
fn run(width: f32, distance: f32) -> f32 {
    width * (1. + STEP) / 2. * (1. - STEP.powf(distance)) / (1. - STEP) + GAP * distance
}

/// How far above the bottom of the stage the box `behind` steps behind
/// the chosen one stands, the chosen one being `height` high: the boxes
/// in front of it stand under it, [`GAP`] apart.
fn climb(height: f32, behind: f32) -> f32 {
    height * (1. - STEP.powf(behind)) / (1. - STEP) + GAP * behind
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAGE: (f32, f32) = (1400., 560.);

    fn close(one: f32, other: f32) -> bool {
        (one - other).abs() < 0.01
    }

    fn overlap(one: &Placed, other: &Placed) -> bool {
        one.left < other.right() - 0.01
            && other.left < one.right() - 0.01
            && one.top < other.bottom() - 0.01
            && other.top < one.bottom() - 0.01
    }

    #[test]
    fn cover_has_the_chosen_window_large_in_the_middle_and_the_others_beside_it() {
        let sizes = vec![(1920., 1080.); 7];
        let placed = cover(STAGE, &sizes, 3.);
        let chosen = placed[3].unwrap();
        assert!(close(chosen.left + chosen.width / 2., STAGE.0 / 2.));
        assert!(close(chosen.opacity, 1.));
        let (before, after) = (placed[2].unwrap(), placed[4].unwrap());
        assert!(close(before.width, chosen.width * STEP));
        assert!(close(after.width, before.width));
        assert!(before.right() <= chosen.left && after.left >= chosen.right());
        assert!(before.opacity < 1. && close(before.opacity, after.opacity));
        // Further ones are smaller, fainter and further out, and those
        // that would stand beyond the stage are not shown.
        let further = placed[5].unwrap();
        assert!(further.width < after.width && further.opacity < after.opacity);
        assert!(further.left >= after.right());
        assert!(placed[1].is_some());
        assert_eq!((placed[0], placed[6]), (None, None));
    }

    #[test]
    fn pictures_never_overlap_where_the_choice_rests_and_stay_on_the_stage() {
        let sizes: Vec<(f32, f32)> = (0..12)
            .map(|index| match index % 3 {
                0 => (1920., 1080.),
                1 => (800., 1000.),
                _ => (300., 200.),
            })
            .collect();
        for style in [cover, flip] {
            for position in 0..sizes.len() {
                let placed: Vec<Placed> = style(STAGE, &sizes, position as f32)
                    .into_iter()
                    .flatten()
                    .collect();
                assert!(!placed.is_empty());
                for (index, one) in placed.iter().enumerate() {
                    assert!(one.left >= -0.5 && one.right() <= STAGE.0 + 0.5);
                    assert!(one.top >= -0.5 && one.bottom() <= STAGE.1 + 0.5);
                    for other in &placed[index + 1..] {
                        assert!(!overlap(one, other), "{one:?} on {other:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_pictures_move_smoothly_between_two_choices() {
        let sizes = vec![(1600., 900.); 5];
        let at = |position: f32| cover(STAGE, &sizes, position)[2].unwrap();
        let (from, half, to) = (at(1.), at(1.5), at(2.));
        assert!(from.left > half.left && half.left > to.left);
        assert!(from.width < half.width && half.width < to.width);
    }

    #[test]
    fn flip_stacks_the_windows_after_the_chosen_one_upward() {
        let sizes = vec![(1920., 1080.); 6];
        let placed = flip(STAGE, &sizes, 1.);
        assert_eq!(placed[0], None);
        let front = placed[1].unwrap();
        assert!(close(front.bottom(), STAGE.1));
        let next = placed[2].unwrap();
        assert!(next.bottom() <= front.top);
        assert!(close(next.width, front.width * STEP));
        assert!(next.opacity < front.opacity);
        assert!(next.left + next.width / 2. > front.left + front.width / 2.);
        // The one that was in front fades out where it stood.
        let leaving = flip(STAGE, &sizes, 1.25)[1].unwrap();
        assert!(close(leaving.opacity, 0.5));
        assert!(close(leaving.bottom(), STAGE.1));
        assert_eq!(flip(STAGE, &sizes, 1.5)[1], None);
    }

    #[test]
    fn a_small_window_is_not_enlarged() {
        let placed = cover(STAGE, &[(200., 100.)], 0.);
        let one = placed[0].unwrap();
        assert!(close(one.width, 200.) && close(one.height, 100.));
        assert!(cover(STAGE, &[], 0.).is_empty());
    }
}
