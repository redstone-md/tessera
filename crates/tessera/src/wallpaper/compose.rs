// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How the picture of one display is made: a picture fitted to it in one
//! of the ways Windows has, over a color or a gradient.

use std::collections::VecDeque;

use image::RgbImage;
use serde::{Deserialize, Serialize};

use super::adjust::{Adjustments, Anchor};
use crate::platform::{BackdropPicture, ScreenRect, WallpaperPosition};

/// A part of a picture or of a display: its left, top, width and height.
pub type Part = (u32, u32, u32, u32);

/// How a picture that has not the shape of its display is fitted to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    /// Scaled until it covers the display; what sticks out is cut off.
    #[default]
    #[serde(alias = "fill")]
    Cover,
    /// Scaled until all of it is seen; the rest of the display has the
    /// color.
    #[serde(alias = "fit")]
    Contain,
    /// Scaled to the display in both directions, which changes its shape.
    Stretch,
    /// In its own size in the middle of the display, the color around it
    /// and what is larger than the display cut off.
    Center,
    /// In its own size, repeated from the top left corner of the display.
    Tile,
    /// One picture over all displays together, scaled until it covers
    /// the rectangle around them; each display shows its part of it.
    Span,
}

impl Fit {
    /// The way Windows names as `position`.
    pub fn of_windows(position: WallpaperPosition) -> Self {
        match position {
            WallpaperPosition::Center => Self::Center,
            WallpaperPosition::Tile => Self::Tile,
            WallpaperPosition::Stretch => Self::Stretch,
            WallpaperPosition::Fit => Self::Contain,
            WallpaperPosition::Fill => Self::Cover,
            WallpaperPosition::Span => Self::Span,
        }
    }

    /// The position of the wallpaper of Windows that fits as this does.
    pub fn to_windows(self) -> WallpaperPosition {
        match self {
            Self::Cover => WallpaperPosition::Fill,
            Self::Contain => WallpaperPosition::Fit,
            Self::Stretch => WallpaperPosition::Stretch,
            Self::Center => WallpaperPosition::Center,
            Self::Tile => WallpaperPosition::Tile,
            Self::Span => WallpaperPosition::Span,
        }
    }

    /// The part of a picture of `width` by `height` pixels that is shown
    /// on a display of `over_width` by `over_height`, and the part of the
    /// display it is shown in, each as left, top, width and height. Only
    /// for the ways that scale: covering, containing and stretching. What
    /// is cut off, or left of the display, is shared out as `anchor` says.
    pub fn parts(
        self,
        (width, height): (u32, u32),
        (over_width, over_height): (u32, u32),
        anchor: Anchor,
    ) -> (Part, Part) {
        let whole = (0, 0, width, height);
        let all = (0, 0, over_width, over_height);
        match self {
            Self::Stretch => (whole, all),
            // The part of the display with the shape of the picture is
            // found as the part of a picture with the shape of a display.
            Self::Contain => (
                whole,
                covering_part_at(over_width, over_height, width, height, anchor),
            ),
            _ => (
                covering_part_at(width, height, over_width, over_height, anchor),
                all,
            ),
        }
    }

    /// Every way, in the order the settings offer them.
    pub const ALL: [Self; 6] = [
        Self::Cover,
        Self::Contain,
        Self::Stretch,
        Self::Center,
        Self::Tile,
        Self::Span,
    ];

    /// The name the configuration gives it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Cover => "cover",
            Self::Contain => "contain",
            Self::Stretch => "stretch",
            Self::Center => "center",
            Self::Tile => "tile",
            Self::Span => "span",
        }
    }

    /// Whether a picture fitted this way can lie elsewhere than in the
    /// middle: where it is cut, or does not fill its display.
    pub fn takes_position(self) -> bool {
        matches!(self, Self::Cover | Self::Contain | Self::Center)
    }
}

/// What lies under a picture, or is the whole wallpaper without one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ground {
    /// One color, as red, green and blue.
    Solid([u8; 3]),
    /// From the first color at the top to the second at the bottom.
    Gradient([u8; 3], [u8; 3]),
}

impl Default for Ground {
    fn default() -> Self {
        Self::Solid([0; 3])
    }
}

impl Ground {
    /// The color of the row `row` of `rows`, as red, green and blue.
    fn at(self, row: u32, rows: u32) -> [u8; 3] {
        match self {
            Self::Solid(color) => color,
            Self::Gradient(top, bottom) => {
                let share = row as f32 / rows.saturating_sub(1).max(1) as f32;
                let mix = |from: u8, to: u8| {
                    (f32::from(from) + (f32::from(to) - f32::from(from)) * share).round() as u8
                };
                [
                    mix(top[0], bottom[0]),
                    mix(top[1], bottom[1]),
                    mix(top[2], bottom[2]),
                ]
            }
        }
    }

    /// The color between the two of a gradient, or the one color.
    pub fn middle(self) -> [u8; 3] {
        self.at(1, 3)
    }
}

/// The picture of a display at `display`, on a desktop whose displays all
/// lie within `desktop`: `image` fitted as `fit` says over `ground`, or
/// `ground` alone without an image. Rows from the top, a pixel being
/// blue, green and red, as Windows expects.
///
/// The scaling is done here, a few rows at a time: that of the `image`
/// crate first makes a copy of the whole picture that is four times its
/// size in memory.
///
/// A picture that is cut to cover its display, or does not fill it, lies
/// where `anchor` says; then `adjust`ments are made to the whole.
pub fn compose(
    image: Option<&RgbImage>,
    display: ScreenRect,
    desktop: ScreenRect,
    (fit, ground): (Fit, Ground),
    adjust: &Adjustments,
) -> BackdropPicture {
    let mut picture = fitted(image, display, desktop, fit, ground, adjust.anchor);
    if adjust.changes_pixels() {
        adjust.apply(&mut picture);
    }
    picture
}

/// The picture of a display before the adjustments: see [`compose`].
fn fitted(
    image: Option<&RgbImage>,
    display: ScreenRect,
    desktop: ScreenRect,
    fit: Fit,
    ground: Ground,
    anchor: Anchor,
) -> BackdropPicture {
    let (width, height) = size(display);
    let stride = BackdropPicture::stride(width);
    let mut pixels = vec![0; stride * height as usize];
    for (row, line) in pixels.chunks_exact_mut(stride).enumerate() {
        let [red, green, blue] = ground.at(row as u32, height);
        for pixel in line[..width as usize * 3].as_chunks_mut::<3>().0 {
            *pixel = [blue, green, red];
        }
    }
    let Some(image) = image.filter(|image| image.width() > 0 && image.height() > 0) else {
        return BackdropPicture::new(width, height, pixels);
    };
    let picture = (image.width(), image.height());
    match fit {
        Fit::Center => center(image, (width, height), &mut pixels, stride, anchor),
        Fit::Tile => tile(image, (width, height), &mut pixels, stride),
        Fit::Span => {
            let part = span_part(picture, display, desktop);
            scale_into(image, part, (0, 0, width, height), &mut pixels, stride);
        }
        Fit::Cover | Fit::Contain | Fit::Stretch => {
            let (part, place) = fit.parts(picture, (width, height), anchor);
            scale_into(image, part, place, &mut pixels, stride);
        }
    }
    BackdropPicture::new(width, height, pixels)
}

/// The width and height of `frame`.
fn size(frame: ScreenRect) -> (u32, u32) {
    (
        (frame.right - frame.left).unsigned_abs(),
        (frame.bottom - frame.top).unsigned_abs(),
    )
}

/// The rectangle around all of `frames`.
pub fn around(frames: &[ScreenRect]) -> Option<ScreenRect> {
    frames.iter().copied().reduce(|all, frame| ScreenRect {
        left: all.left.min(frame.left),
        top: all.top.min(frame.top),
        right: all.right.max(frame.right),
        bottom: all.bottom.max(frame.bottom),
    })
}

/// Copies the row `from` of a picture, red, green and blue, into `to`,
/// blue, green and red.
fn put_row(from: &[u8], to: &mut [u8]) {
    let (from, _) = from.as_chunks::<3>();
    let (to, _) = to.as_chunks_mut::<3>();
    for ([red, green, blue], pixel) in from.iter().zip(to) {
        *pixel = [*blue, *green, *red];
    }
}

/// Puts `image` in its own size in a display of `width` by `height`, in
/// the middle or where `anchor` says, cutting off what is larger.
fn center(
    image: &RgbImage,
    (width, height): (u32, u32),
    pixels: &mut [u8],
    stride: usize,
    anchor: Anchor,
) {
    let (wide, high) = (i64::from(image.width()), i64::from(image.height()));
    let place = |share: f32, left_over: i64| {
        if share == 0.5 {
            left_over / 2
        } else {
            Anchor::offset(share, left_over as f32).round() as i64
        }
    };
    let left = place(anchor.x, i64::from(width) - wide);
    let top = place(anchor.y, i64::from(height) - high);
    let columns = left.max(0)..(left + wide).min(i64::from(width));
    let rows = top.max(0)..(top + high).min(i64::from(height));
    if columns.is_empty() {
        return;
    }
    let source_row = image.width() as usize * 3;
    for row in rows {
        let from_row = (row - top) as usize;
        let from_column = (columns.start - left) as usize;
        let start = from_row * source_row + from_column * 3;
        let length = (columns.end - columns.start) as usize * 3;
        let to = row as usize * stride + columns.start as usize * 3;
        put_row(
            &image.as_raw()[start..start + length],
            &mut pixels[to..to + length],
        );
    }
}

/// Repeats `image` in its own size over a display of `width` by `height`,
/// from the top left corner.
fn tile(image: &RgbImage, (width, height): (u32, u32), pixels: &mut [u8], stride: usize) {
    let source_row = image.width() as usize * 3;
    let row = width as usize * 3;
    // Each row of the picture is repeated across once, and copied to every
    // row of the display it falls on.
    let mut across = vec![0; row];
    for from in 0..image.height().min(height) as usize {
        let line = &image.as_raw()[from * source_row..(from + 1) * source_row];
        for chunk in across.chunks_mut(source_row) {
            put_row(&line[..chunk.len()], chunk);
        }
        for to in (from..height as usize).step_by(image.height() as usize) {
            pixels[to * stride..to * stride + row].copy_from_slice(&across);
        }
    }
}

/// The part of a picture of `width` by `height` pixels that the display at
/// `display` shows, when the picture is scaled to cover all of `desktop`.
pub fn span_part((width, height): (u32, u32), display: ScreenRect, desktop: ScreenRect) -> Part {
    let (desktop_wide, desktop_high) = size(desktop);
    let (display_wide, display_high) = size(display);
    let (left, top, wide, _) = covering_part(width, height, desktop_wide, desktop_high);
    // Pixels of the picture for one of the desktop.
    let scale = f64::from(wide) / f64::from(desktop_wide.max(1));
    let from_left = f64::from((display.left - desktop.left).max(0)) * scale;
    let from_top = f64::from((display.top - desktop.top).max(0)) * scale;
    let part_left = (f64::from(left) + from_left).round() as u32;
    let part_top = (f64::from(top) + from_top).round() as u32;
    let part_left = part_left.min(width - 1);
    let part_top = part_top.min(height - 1);
    let part_wide = ((f64::from(display_wide) * scale).round() as u32).clamp(1, width - part_left);
    let part_high = ((f64::from(display_high) * scale).round() as u32).clamp(1, height - part_top);
    (part_left, part_top, part_wide, part_high)
}

/// For each of `to` pixels in a row that `from` pixels are scaled to: the
/// first of the pixels it is mixed from, and the share of that one and of
/// each after it. The shares fall off evenly with the distance and sum
/// to 1.
pub fn shares(from: u32, to: u32) -> Vec<(usize, Vec<f32>)> {
    let ratio = from as f32 / to as f32;
    // How far a pixel reaches: over its neighbours, and when the row gets
    // shorter over all the pixels that go into one.
    let reach = ratio.max(1.);
    (0..to)
        .map(|index| {
            let centre = (index as f32 + 0.5) * ratio;
            let first = (centre - reach).floor().max(0.) as usize;
            let end = ((centre + reach).ceil() as usize).clamp(first + 1, from as usize);
            let mut shares: Vec<f32> = (first..end)
                .map(|source| (1. - ((source as f32 + 0.5 - centre) / reach).abs()).max(0.))
                .collect();
            let sum: f32 = shares.iter().sum();
            if sum > 0. {
                shares.iter_mut().for_each(|share| *share /= sum);
            } else {
                let even = 1. / shares.len() as f32;
                shares.fill(even);
            }
            (first, shares)
        })
        .collect()
}

/// Scales the `part` of `image` into the `place` of a picture whose rows
/// of `stride` bytes are `pixels`.
fn scale_into(image: &RgbImage, part: Part, place: Part, pixels: &mut [u8], stride: usize) {
    let (place_left, place_top, place_wide, place_high) = place;
    let row = place_wide as usize * 3;
    let mut rows = pixels.chunks_exact_mut(stride).skip(place_top as usize);
    let source = Source {
        bytes: image.as_raw(),
        stride: image.width() as usize * 3,
        // Windows expects blue first.
        swapped: true,
    };
    source.scale(part, (place_wide, place_high), |mixed| {
        if let Some(line) = rows.next() {
            let out = &mut line[place_left as usize * 3..][..row];
            for (to, from) in out.iter_mut().zip(mixed) {
                *to = from.round().clamp(0., 255.) as u8;
            }
        }
    });
}

/// Pixels of three bytes each to scale from: rows of `stride` bytes, and
/// whether the first and the third byte of a pixel change places on the
/// way.
struct Source<'a> {
    bytes: &'a [u8],
    stride: usize,
    swapped: bool,
}

impl Source<'_> {
    /// Scales the `part` of the pixels to `wide_to` by `high_to`, and
    /// gives each row made to `put`, from the top, three numbers a pixel.
    fn scale(&self, part: Part, (wide_to, high_to): (u32, u32), mut put: impl FnMut(&[f32])) {
        let (left, top, wide, high) = part;
        if wide == 0 || high == 0 || wide_to == 0 || high_to == 0 {
            return;
        }
        let across = shares(wide, wide_to);
        let down = shares(high, high_to);
        let row = wide_to as usize * 3;
        let order = if self.swapped { [2, 1, 0] } else { [0, 1, 2] };
        // Rows of the source scaled across, with their number, kept as
        // long as a row of the picture is still mixed from them.
        let mut scaled: VecDeque<(usize, Vec<f32>)> = VecDeque::new();
        let mut mixed = vec![0.; row];
        for (first, shares) in &down {
            while scaled.front().is_some_and(|(number, _)| number < first) {
                scaled.pop_front();
            }
            let mut next = scaled.back().map_or(*first, |(number, _)| number + 1);
            while next < first + shares.len() {
                let start = (top as usize + next) * self.stride + left as usize * 3;
                let line = &self.bytes[start..start + wide as usize * 3];
                let mut wide_row = vec![0.; row];
                for ((first, shares), to) in across.iter().zip(wide_row.as_chunks_mut::<3>().0) {
                    for (share, from) in shares.iter().zip(line[first * 3..].as_chunks::<3>().0) {
                        for (to, &at) in to.iter_mut().zip(&order) {
                            *to += share * f32::from(from[at]);
                        }
                    }
                }
                scaled.push_back((next, wide_row));
                next += 1;
            }
            mixed.fill(0.);
            for (share, (_, line)) in shares.iter().zip(&scaled) {
                for (to, from) in mixed.iter_mut().zip(line) {
                    *to += share * from;
                }
            }
            put(&mixed);
        }
    }
}

/// The size of a picture of `width` by `height` made no larger than
/// `longest` on its longer side, its shape kept.
pub fn shrunk_size(width: u32, height: u32, longest: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= longest || long == 0 {
        return (width.max(1), height.max(1));
    }
    let scale = f64::from(longest) / f64::from(long);
    let side = |length: u32| ((f64::from(length) * scale).round() as u32).max(1);
    (side(width), side(height))
}

/// `picture` as a still for the interface to draw: no larger than
/// `longest` on its longer side, a pixel being blue, green, red and alpha
/// as GPUI takes it.
pub fn still(picture: &BackdropPicture, longest: u32) -> image::RgbaImage {
    let (width, height) = picture.size();
    let (wide, high) = shrunk_size(width, height, longest);
    let mut out = Vec::with_capacity(wide as usize * high as usize * 4);
    let source = Source {
        bytes: picture.pixels(),
        stride: BackdropPicture::stride(width),
        swapped: false,
    };
    source.scale((0, 0, width, height), (wide, high), |mixed| {
        for pixel in mixed.as_chunks::<3>().0 {
            let byte = |value: f32| value.round().clamp(0., 255.) as u8;
            out.extend([byte(pixel[0]), byte(pixel[1]), byte(pixel[2]), 255]);
        }
    });
    out.resize(wide as usize * high as usize * 4, 255);
    image::RgbaImage::from_raw(wide, high, out).expect("the still has all its pixels")
}

/// The part of a picture of `width` by `height` pixels that has the shape
/// of `over_width` by `over_height` and is as large as can be, in the
/// middle of the picture: its left, top, width and height.
pub fn covering_part(width: u32, height: u32, over_width: u32, over_height: u32) -> Part {
    covering_part_at(width, height, over_width, over_height, Anchor::default())
}

/// The part [`covering_part`] gives, with what is cut off shared out on
/// the two sides as `anchor` says instead of evenly.
pub fn covering_part_at(
    width: u32,
    height: u32,
    over_width: u32,
    over_height: u32,
    anchor: Anchor,
) -> Part {
    let (wide, high) = (u64::from(width), u64::from(height));
    let (over_wide, over_high) = (u64::from(over_width.max(1)), u64::from(over_height.max(1)));
    let start = |share: f32, left_over: u32| {
        (Anchor::offset(share, left_over as f32).round() as u32).min(left_over)
    };
    if wide * over_high > high * over_wide {
        // Wider in shape: the sides go.
        let kept = ((high * over_wide / over_high) as u32).clamp(1, width);
        let left = if anchor.x == 0.5 {
            (width - kept) / 2
        } else {
            start(anchor.x, width - kept)
        };
        (left, 0, kept, height)
    } else {
        let kept = ((wide * over_high / over_wide) as u32).clamp(1, height);
        let top = if anchor.y == 0.5 {
            (height - kept) / 2
        } else {
            start(anchor.y, height - kept)
        };
        (0, top, width, kept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The picture of a display without adjustments.
    fn compose(
        image: Option<&RgbImage>,
        display: ScreenRect,
        desktop: ScreenRect,
        fit: Fit,
        ground: Ground,
    ) -> BackdropPicture {
        super::compose(
            image,
            display,
            desktop,
            (fit, ground),
            &Adjustments::default(),
        )
    }

    #[test]
    fn a_cut_picture_keeps_to_its_place() {
        // A wide picture on a square display: what is cut goes from the
        // right only, or from the left only.
        let left = Anchor { x: 0., y: 0.5 };
        let right = Anchor { x: 1., y: 0.5 };
        assert_eq!(
            covering_part_at(8000, 2000, 1000, 1000, left),
            (0, 0, 2000, 2000)
        );
        assert_eq!(
            covering_part_at(8000, 2000, 1000, 1000, right),
            (6000, 0, 2000, 2000)
        );
        let top = Anchor { x: 0.5, y: 0. };
        assert_eq!(
            covering_part_at(4000, 6000, 2000, 1000, top),
            (0, 0, 4000, 2000)
        );
        // Contained, the room left beside it lies on one side.
        let (_, place) = Fit::Contain.parts((8, 4), (4, 4), Anchor { x: 0.5, y: 1. });
        assert_eq!(place, (0, 2, 4, 2));
        // Centered in its own size, it keeps to a corner.
        let image = RgbImage::from_pixel(2, 2, image::Rgb([9, 9, 9]));
        let display = frame(0, 0, 4, 4);
        let adjust = Adjustments {
            anchor: Anchor { x: 1., y: 1. },
            ..Adjustments::default()
        };
        let red = [200, 0, 0];
        let shown = super::compose(
            Some(&image),
            display,
            display,
            (Fit::Center, Ground::Solid(red)),
            &adjust,
        );
        assert_eq!(row(&shown, 3), [red, red, [9; 3], [9; 3]]);
        assert_eq!(row(&shown, 1), [red; 4]);
    }

    #[test]
    fn a_still_is_no_larger_than_asked_and_keeps_its_shape() {
        assert_eq!(shrunk_size(3840, 2160, 1920), (1920, 1080));
        assert_eq!(shrunk_size(1000, 500, 1920), (1000, 500));
        assert_eq!(shrunk_size(500, 4000, 1000), (125, 1000));
        let display = frame(0, 0, 8, 4);
        let picture = compose(
            None,
            display,
            display,
            Fit::Cover,
            Ground::Solid([10, 20, 30]),
        );
        let small = still(&picture, 4);
        assert_eq!(small.dimensions(), (4, 2));
        // Blue first, and wholly opaque.
        assert!(small.pixels().all(|pixel| pixel.0 == [30, 20, 10, 255]));
    }

    fn frame(left: i32, top: i32, width: i32, height: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }

    /// The pixels of row `row` of `picture`, as red, green and blue.
    fn row(picture: &BackdropPicture, row: usize) -> Vec<[u8; 3]> {
        let (width, _) = picture.size();
        let stride = BackdropPicture::stride(width);
        let line = &picture.pixels()[row * stride..row * stride + width as usize * 3];
        line.as_chunks::<3>()
            .0
            .iter()
            .map(|[blue, green, red]| [*red, *green, *blue])
            .collect()
    }

    #[test]
    fn the_part_of_a_picture_that_covers_has_the_shape_of_the_display() {
        // Wider in shape: the sides go.
        assert_eq!(covering_part(8000, 2000, 2000, 1000), (2000, 0, 4000, 2000));
        // Higher in shape: top and bottom go.
        assert_eq!(covering_part(4000, 6000, 2000, 1000), (0, 2000, 4000, 2000));
        // The same shape: all of it, larger or smaller.
        assert_eq!(covering_part(1000, 500, 2000, 1000), (0, 0, 1000, 500));
        assert_eq!(covering_part(1, 900, 3440, 1440), (0, 449, 1, 1));
    }

    #[test]
    fn the_shares_of_the_pixels_mixed_into_one_sum_to_one() {
        for (from, to) in [(3840, 2560), (1000, 3440), (7, 7), (1, 5), (5, 1)] {
            let shares = shares(from, to);
            assert_eq!(shares.len(), to as usize);
            for (first, shares) in shares {
                assert!(first + shares.len() <= from as usize);
                assert!((shares.iter().sum::<f32>() - 1.).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn a_picture_is_cut_shown_whole_or_stretched() {
        // Twice as wide as high, on a square display.
        let image = RgbImage::from_fn(8, 4, |x, _| image::Rgb([if x < 4 { 40 } else { 200 }; 3]));
        assert_eq!(
            Fit::Cover.parts((8, 4), (4, 4), Anchor::default()),
            ((2, 0, 4, 4), (0, 0, 4, 4))
        );
        assert_eq!(
            Fit::Contain.parts((8, 4), (4, 4), Anchor::default()),
            ((0, 0, 8, 4), (0, 1, 4, 2))
        );
        assert_eq!(
            Fit::Stretch.parts((8, 4), (4, 4), Anchor::default()),
            ((0, 0, 8, 4), (0, 0, 4, 4))
        );

        let display = frame(0, 0, 4, 4);
        let whole = compose(
            Some(&image),
            display,
            display,
            Fit::Contain,
            Ground::default(),
        );
        assert_eq!(row(&whole, 0), [[0; 3]; 4]);
        assert_eq!(row(&whole, 1)[0], [40; 3]);
        assert_eq!(row(&whole, 1)[3], [200; 3]);
        assert_eq!(row(&whole, 2), row(&whole, 1));
        assert_eq!(row(&whole, 3), [[0; 3]; 4]);

        let stretched = compose(
            Some(&image),
            display,
            display,
            Fit::Stretch,
            Ground::default(),
        );
        assert!((0..4).all(|line| row(&stretched, line) == row(&whole, 1)));
    }

    #[test]
    fn a_picture_scaled_down_keeps_its_halves() {
        // Left half dark, right half bright.
        let image = RgbImage::from_fn(8, 4, |x, _| image::Rgb([if x < 4 { 0 } else { 200 }; 3]));
        let display = frame(0, 0, 4, 2);
        let picture = compose(
            Some(&image),
            display,
            display,
            Fit::Cover,
            Ground::default(),
        );
        assert_eq!(row(&picture, 0)[0], [0, 0, 0]);
        assert_eq!(row(&picture, 0)[3], [200, 200, 200]);
    }

    #[test]
    fn a_picture_is_fitted_to_the_display_with_blue_first() {
        let mut image = RgbImage::new(4, 2);
        for pixel in image.pixels_mut() {
            pixel.0 = [10, 20, 30];
        }
        let display = frame(0, 0, 3, 3);
        let picture = compose(
            Some(&image),
            display,
            display,
            Fit::Cover,
            Ground::default(),
        );
        assert_eq!(picture.size(), (3, 3));
        // Three pixels take nine bytes, a row twelve.
        assert_eq!(BackdropPicture::stride(3), 12);
        assert_eq!(
            picture.pixels()[..12],
            [30, 20, 10, 30, 20, 10, 30, 20, 10, 0, 0, 0]
        );
    }

    #[test]
    fn a_centered_picture_keeps_its_size_and_has_the_color_around() {
        let red = [200, 0, 0];
        let image = RgbImage::from_pixel(2, 2, image::Rgb([9, 9, 9]));
        let display = frame(0, 0, 4, 4);
        let picture = compose(
            Some(&image),
            display,
            display,
            Fit::Center,
            Ground::Solid(red),
        );
        assert_eq!(row(&picture, 0), [red; 4]);
        assert_eq!(row(&picture, 1), [red, [9; 3], [9; 3], red]);
        assert_eq!(row(&picture, 2), row(&picture, 1));
        assert_eq!(row(&picture, 3), [red; 4]);

        // Larger than the display: its middle.
        let large = RgbImage::from_fn(6, 1, |x, _| image::Rgb([x as u8; 3]));
        let display = frame(0, 0, 2, 3);
        let picture = compose(
            Some(&large),
            display,
            display,
            Fit::Center,
            Ground::Solid(red),
        );
        assert_eq!(row(&picture, 0), [red, red]);
        assert_eq!(row(&picture, 1), [[2; 3], [3; 3]]);
        assert_eq!(row(&picture, 2), [red, red]);
    }

    #[test]
    fn a_tiled_picture_repeats_from_the_top_left() {
        let image = RgbImage::from_fn(2, 2, |x, y| image::Rgb([(x + 2 * y) as u8; 3]));
        let display = frame(0, 0, 5, 3);
        let picture = compose(Some(&image), display, display, Fit::Tile, Ground::default());
        let first = [[0; 3], [1; 3], [0; 3], [1; 3], [0; 3]];
        let second = [[2; 3], [3; 3], [2; 3], [3; 3], [2; 3]];
        assert_eq!(row(&picture, 0), first);
        assert_eq!(row(&picture, 1), second);
        assert_eq!(row(&picture, 2), first);
    }

    #[test]
    fn a_spanned_picture_gives_each_display_its_part() {
        // Two displays side by side, 4 by 2 each; a picture of 8 by 2 with
        // a dark left and a bright right half.
        let image = RgbImage::from_fn(8, 2, |x, _| image::Rgb([if x < 4 { 10 } else { 250 }; 3]));
        let left = frame(-4, 0, 4, 2);
        let right = frame(0, 0, 4, 2);
        let desktop = around(&[left, right]).unwrap();
        assert_eq!(desktop, frame(-4, 0, 8, 2));
        assert_eq!(span_part((8, 2), left, desktop), (0, 0, 4, 2));
        assert_eq!(span_part((8, 2), right, desktop), (4, 0, 4, 2));
        let shown = compose(Some(&image), right, desktop, Fit::Span, Ground::default());
        assert_eq!(row(&shown, 0), [[250; 3]; 4]);
        let shown = compose(Some(&image), left, desktop, Fit::Span, Ground::default());
        assert_eq!(row(&shown, 1), [[10; 3]; 4]);

        // A picture of another shape is cut as for covering the whole.
        assert_eq!(span_part((16, 16), right, desktop), (8, 6, 8, 4));
    }

    #[test]
    fn a_gradient_goes_from_the_top_color_to_the_bottom_one() {
        let display = frame(0, 0, 2, 3);
        let ground = Ground::Gradient([0, 0, 0], [200, 100, 0]);
        let picture = compose(None, display, display, Fit::Cover, ground);
        assert_eq!(row(&picture, 0), [[0, 0, 0]; 2]);
        assert_eq!(row(&picture, 1), [[100, 50, 0]; 2]);
        assert_eq!(row(&picture, 2), [[200, 100, 0]; 2]);
        assert_eq!(ground.middle(), [100, 50, 0]);
        assert_eq!(Ground::Solid([1, 2, 3]).middle(), [1, 2, 3]);
    }

    #[test]
    fn the_ways_to_fit_have_the_names_of_windows_too() {
        #[derive(Deserialize)]
        struct Named {
            fit: Fit,
        }
        for (name, fit) in [
            ("fill", Fit::Cover),
            ("cover", Fit::Cover),
            ("fit", Fit::Contain),
            ("tile", Fit::Tile),
            ("span", Fit::Span),
            ("center", Fit::Center),
        ] {
            let named: Named = toml::from_str(&format!("fit = \"{name}\"")).unwrap();
            assert_eq!(named.fit, fit);
            assert_eq!(Fit::of_windows(fit.to_windows()), fit);
        }
    }
}
