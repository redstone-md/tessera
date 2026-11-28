// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What is drawn on a screenshot in the editor, and how it is put on the
//! pixels: shapes, lines, strokes of a pen or a highlighter, texts and
//! numbered stamps, as marks that stay apart from the picture until it is
//! saved or copied; blurring and pixelating a part, and cropping, which
//! change the picture itself. The edges of what is drawn are smoothed.

use image::{GrayImage, Rgba, RgbaImage};

/// A colour with its opacity, red, green, blue and alpha.
pub type Color = [u8; 4];

/// A point on the picture, in its pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spot {
    pub x: f32,
    pub y: f32,
}

impl Spot {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn minus(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }

    fn length(self) -> f32 {
        self.x.hypot(self.y)
    }
}

/// How much of the colour of a highlighter covers what is under it.
pub const HIGHLIGHT: u8 = 102;

/// Something drawn on the picture.
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    Rectangle {
        from: Spot,
        to: Spot,
        color: Color,
        width: f32,
    },
    Ellipse {
        from: Spot,
        to: Spot,
        color: Color,
        width: f32,
    },
    Arrow {
        from: Spot,
        to: Spot,
        color: Color,
        width: f32,
    },
    Line {
        from: Spot,
        to: Spot,
        color: Color,
        width: f32,
    },
    /// A stroke of the pen, through its points.
    Pen {
        points: Vec<Spot>,
        color: Color,
        width: f32,
    },
    /// A stroke of the highlighter: wide and see-through.
    Highlighter {
        points: Vec<Spot>,
        color: Color,
        width: f32,
    },
    /// Lines of text, the top left corner of the first at `at`.
    Text {
        at: Spot,
        text: String,
        color: Color,
        size: f32,
    },
    /// A round mark with a number, its middle at `at`.
    Stamp {
        at: Spot,
        number: u32,
        color: Color,
        size: f32,
    },
}

impl Mark {
    /// The number of the next stamp after `marks`.
    pub fn next_stamp(marks: &[Self]) -> u32 {
        let highest = { marks.iter() }
            .filter_map(|mark| match mark {
                Self::Stamp { number, .. } => Some(*number),
                _ => None,
            })
            .max();
        highest.map_or(1, |number| number + 1)
    }

    /// The points of the head of an arrow from `from` to `to` of `width`:
    /// its tip and the two corners behind it.
    pub fn arrow_head(from: Spot, to: Spot, width: f32) -> [Spot; 3] {
        let along = to.minus(from);
        let length = along.length().max(0.001);
        let (dx, dy) = (along.x / length, along.y / length);
        let head = (width * 4.).max(12.).min(length);
        let half = head * 0.55;
        let base = Spot::new(to.x - dx * head, to.y - dy * head);
        [
            to,
            Spot::new(base.x - dy * half, base.y + dx * half),
            Spot::new(base.x + dy * half, base.y - dx * half),
        ]
    }

    /// Where the shaft of an arrow ends, under its head.
    pub fn arrow_shaft_end(from: Spot, to: Spot, width: f32) -> Spot {
        let along = to.minus(from);
        let length = along.length().max(0.001);
        let back = ((width * 4.).max(12.) * 0.6).min(length);
        Spot::new(
            to.x - along.x / length * back,
            to.y - along.y / length * back,
        )
    }
}

/// A rectangle on the picture, in whole pixels, its sides ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Area {
    /// The pixels between `from` and `to`, within a picture of `size`;
    /// `None` where that is less than a pixel wide or high.
    pub fn between(from: Spot, to: Spot, size: (u32, u32)) -> Option<Self> {
        let clamp = |value: f32, most: u32| value.round().clamp(0., most as f32) as u32;
        let (left, right) = (from.x.min(to.x), from.x.max(to.x));
        let (top, bottom) = (from.y.min(to.y), from.y.max(to.y));
        let (x, y) = (clamp(left, size.0), clamp(top, size.1));
        let (right, bottom) = (clamp(right, size.0), clamp(bottom, size.1));
        (right > x && bottom > y).then(|| Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        })
    }
}

/// How much of each pixel of a part of the picture a shape covers, from
/// 0 to 1, put together from several parts that each cover what the most
/// covering of them covers, so that a see-through stroke is as dark where
/// its parts meet as elsewhere.
struct Cover {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    cover: Vec<f32>,
}

impl Cover {
    /// A cover of the part of a picture of `size` that `spots` with a
    /// margin of `margin` lie in; `None` where that is off the picture.
    fn around(spots: &[Spot], margin: f32, size: (u32, u32)) -> Option<Self> {
        let first = spots.first()?;
        let (mut left, mut top, mut right, mut bottom) = (first.x, first.y, first.x, first.y);
        for spot in spots {
            left = left.min(spot.x);
            top = top.min(spot.y);
            right = right.max(spot.x);
            bottom = bottom.max(spot.y);
        }
        let left = ((left - margin).floor() as i32).max(0);
        let top = ((top - margin).floor() as i32).max(0);
        let right = ((right + margin).ceil() as i32).min(size.0 as i32);
        let bottom = ((bottom + margin).ceil() as i32).min(size.1 as i32);
        if right <= left || bottom <= top {
            return None;
        }
        let (width, height) = ((right - left) as usize, (bottom - top) as usize);
        Some(Self {
            left,
            top,
            width,
            height,
            cover: vec![0.; width * height],
        })
    }

    /// Covers each pixel as much as `covered` says for its middle, where
    /// that is more than it is covered already.
    fn add(&mut self, covered: impl Fn(Spot) -> f32) {
        for row in 0..self.height {
            for column in 0..self.width {
                let middle = Spot::new(
                    (self.left + column as i32) as f32 + 0.5,
                    (self.top + row as i32) as f32 + 0.5,
                );
                let cell = &mut self.cover[row * self.width + column];
                *cell = cell.max(covered(middle).clamp(0., 1.));
            }
        }
    }

    /// Puts `color` on `picture` as much as each pixel is covered.
    fn paint(&self, picture: &mut RgbaImage, color: Color) {
        for row in 0..self.height {
            for column in 0..self.width {
                let cover = self.cover[row * self.width + column];
                if cover <= 0. {
                    continue;
                }
                let (x, y) = (
                    (self.left + column as i32) as u32,
                    (self.top + row as i32) as u32,
                );
                blend(picture.get_pixel_mut(x, y), color, cover);
            }
        }
    }
}

/// `color` put over `pixel` as much as `cover` says, from 0 to 1.
fn blend(pixel: &mut Rgba<u8>, color: Color, cover: f32) {
    let alpha = color[3] as f32 / 255. * cover.clamp(0., 1.);
    if alpha <= 0. {
        return;
    }
    let under = pixel[3] as f32 / 255.;
    let out = alpha + under * (1. - alpha);
    for channel in 0..3 {
        let mixed = (color[channel] as f32 * alpha + pixel[channel] as f32 * under * (1. - alpha))
            / out.max(f32::EPSILON);
        pixel[channel] = mixed.round().clamp(0., 255.) as u8;
    }
    pixel[3] = (out * 255.).round().clamp(0., 255.) as u8;
}

/// How far `spot` is from the segment from `from` to `to`.
fn distance_to_segment(spot: Spot, from: Spot, to: Spot) -> f32 {
    let along = to.minus(from);
    let squared = along.x * along.x + along.y * along.y;
    let share = match squared > 0. {
        true => ((spot.x - from.x) * along.x + (spot.y - from.y) * along.y) / squared,
        false => 0.,
    };
    let share = share.clamp(0., 1.);
    let nearest = Spot::new(from.x + along.x * share, from.y + along.y * share);
    spot.minus(nearest).length()
}

/// How much a pixel whose middle is `distance` from the middle of a line
/// `width` wide is covered by it.
fn line_cover(distance: f32, width: f32) -> f32 {
    width.max(1.) / 2. + 0.5 - distance
}

/// Strokes the line through `points`, `width` wide, in `color`.
fn stroke(picture: &mut RgbaImage, points: &[Spot], width: f32, color: Color) {
    let Some(mut cover) = Cover::around(points, width + 2., picture.dimensions()) else {
        return;
    };
    match points {
        [one] => cover.add(|spot| line_cover(spot.minus(*one).length(), width)),
        _ => {
            for pair in points.windows(2) {
                let (from, to) = (pair[0], pair[1]);
                cover.add(|spot| line_cover(distance_to_segment(spot, from, to), width));
            }
        }
    }
    cover.paint(picture, color);
}

/// Fills the triangle `corners` in `color`.
fn fill_triangle(picture: &mut RgbaImage, corners: [Spot; 3], color: Color) {
    let Some(mut cover) = Cover::around(&corners, 2., picture.dimensions()) else {
        return;
    };
    // Which way round the corners go, so that inside is positive.
    let [a, b, c] = corners;
    let turn = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let sign = if turn < 0. { -1. } else { 1. };
    let edge = move |from: Spot, to: Spot, spot: Spot| {
        let along = to.minus(from);
        let length = along.length().max(0.001);
        sign * (along.x * (spot.y - from.y) - along.y * (spot.x - from.x)) / length
    };
    cover.add(|spot| {
        let inside = edge(a, b, spot).min(edge(b, c, spot)).min(edge(c, a, spot));
        inside + 0.5
    });
    cover.paint(picture, color);
}

/// The outline of the ellipse in the rectangle from `from` to `to`.
fn ellipse(picture: &mut RgbaImage, from: Spot, to: Spot, width: f32, color: Color) {
    let middle = Spot::new((from.x + to.x) / 2., (from.y + to.y) / 2.);
    let (rx, ry) = (
        ((to.x - from.x) / 2.).abs().max(0.5),
        ((to.y - from.y) / 2.).abs().max(0.5),
    );
    let Some(mut cover) = Cover::around(&[from, to], width + 2., picture.dimensions()) else {
        return;
    };
    cover.add(|spot| {
        let (dx, dy) = (spot.x - middle.x, spot.y - middle.y);
        let level = (dx / rx).powi(2) + (dy / ry).powi(2);
        let slope = (2. * dx / (rx * rx)).hypot(2. * dy / (ry * ry)).max(0.001);
        line_cover(((level - 1.) / slope).abs(), width)
    });
    cover.paint(picture, color);
}

/// A filled circle of `radius` around `middle`.
fn disc(picture: &mut RgbaImage, middle: Spot, radius: f32, color: Color) {
    let Some(mut cover) = Cover::around(&[middle], radius + 2., picture.dimensions()) else {
        return;
    };
    cover.add(|spot| radius + 0.5 - spot.minus(middle).length());
    cover.paint(picture, color);
}

/// Puts `color` on `picture` as much as `mask`, laid with its top left
/// corner at `at`, covers each pixel.
fn paint_mask(picture: &mut RgbaImage, mask: &GrayImage, at: (i64, i64), color: Color) {
    let (width, height) = (picture.width() as i64, picture.height() as i64);
    for (x, y, cover) in mask.enumerate_pixels() {
        let (px, py) = (at.0 + x as i64, at.1 + y as i64);
        if cover[0] == 0 || px < 0 || py < 0 || px >= width || py >= height {
            continue;
        }
        blend(
            picture.get_pixel_mut(px as u32, py as u32),
            color,
            cover[0] as f32 / 255.,
        );
    }
}

/// White or black, whichever reads better on `color`.
pub fn contrast(color: Color) -> Color {
    let light = 0.299 * color[0] as f32 + 0.587 * color[1] as f32 + 0.114 * color[2] as f32;
    match light > 150. {
        true => [0, 0, 0, 255],
        false => [255, 255, 255, 255],
    }
}

/// What draws a text: its cover for `text`, `size` pixels high, bold or
/// not, or `None` where it cannot.
pub type TextDrawer<'a> = &'a dyn Fn(&str, u32, bool) -> Option<GrayImage>;

/// Draws `mark` on `picture`, texts with `text`.
pub fn draw(picture: &mut RgbaImage, mark: &Mark, text: TextDrawer<'_>) {
    match mark {
        Mark::Rectangle {
            from,
            to,
            color,
            width,
        } => {
            let corners = [
                *from,
                Spot::new(to.x, from.y),
                *to,
                Spot::new(from.x, to.y),
                *from,
            ];
            stroke(picture, &corners, *width, *color);
        }
        Mark::Ellipse {
            from,
            to,
            color,
            width,
        } => ellipse(picture, *from, *to, *width, *color),
        Mark::Line {
            from,
            to,
            color,
            width,
        } => stroke(picture, &[*from, *to], *width, *color),
        Mark::Arrow {
            from,
            to,
            color,
            width,
        } => {
            let end = Mark::arrow_shaft_end(*from, *to, *width);
            stroke(picture, &[*from, end], *width, *color);
            fill_triangle(picture, Mark::arrow_head(*from, *to, *width), *color);
        }
        Mark::Pen {
            points,
            color,
            width,
        } => stroke(picture, points, *width, *color),
        Mark::Highlighter {
            points,
            color,
            width,
        } => {
            let color = [color[0], color[1], color[2], HIGHLIGHT];
            stroke(picture, points, *width, color);
        }
        Mark::Text {
            at,
            text: written,
            color,
            size,
        } => {
            if let Some(mask) = text(written, size.round() as u32, false) {
                paint_mask(
                    picture,
                    &mask,
                    (at.x.round() as i64, at.y.round() as i64),
                    *color,
                );
            }
        }
        Mark::Stamp {
            at,
            number,
            color,
            size,
        } => {
            disc(picture, *at, size / 2., *color);
            if let Some(mask) = text(&number.to_string(), (size * 0.6).round() as u32, true) {
                let left = at.x - mask.width() as f32 / 2.;
                let top = at.y - mask.height() as f32 / 2.;
                paint_mask(
                    picture,
                    &mask,
                    (left.round() as i64, top.round() as i64),
                    contrast(*color),
                );
            }
        }
    }
}

/// `picture` with every one of `marks` drawn on it, in order.
pub fn flatten(picture: &RgbaImage, marks: &[Mark], text: TextDrawer<'_>) -> RgbaImage {
    let mut drawn = picture.clone();
    for mark in marks {
        draw(&mut drawn, mark, text);
    }
    drawn
}

/// `picture` with `area` blurred so that it cannot be read, by `strength`
/// pixels.
pub fn blur(picture: &RgbaImage, area: Area, strength: f32) -> RgbaImage {
    let mut changed = picture.clone();
    let part =
        image::imageops::crop_imm(picture, area.x, area.y, area.width, area.height).to_image();
    let blurred = image::imageops::blur(&part, strength.max(1.));
    image::imageops::replace(&mut changed, &blurred, area.x as i64, area.y as i64);
    changed
}

/// `picture` with `area` made of squares `block` pixels wide, each of the
/// mean colour of what it covers.
pub fn pixelate(picture: &RgbaImage, area: Area, block: u32) -> RgbaImage {
    let block = block.max(2);
    let mut changed = picture.clone();
    let (right, bottom) = (area.x + area.width, area.y + area.height);
    let mut top = area.y;
    while top < bottom {
        let mut left = area.x;
        let rows = block.min(bottom - top);
        while left < right {
            let columns = block.min(right - left);
            let mut sum = [0u64; 4];
            for y in top..top + rows {
                for x in left..left + columns {
                    let pixel = picture.get_pixel(x, y);
                    for channel in 0..4 {
                        sum[channel] += u64::from(pixel[channel]);
                    }
                }
            }
            let count = u64::from(rows * columns);
            let mean = Rgba(sum.map(|total| (total / count) as u8));
            for y in top..top + rows {
                for x in left..left + columns {
                    changed.put_pixel(x, y, mean);
                }
            }
            left += columns;
        }
        top += rows;
    }
    changed
}

/// `picture` cut to `area`.
pub fn crop(picture: &RgbaImage, area: Area) -> RgbaImage {
    image::imageops::crop_imm(picture, area.x, area.y, area.width, area.height).to_image()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Color = [255, 0, 0, 255];

    fn white(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_pixel(width, height, Rgba([255, 255, 255, 255]))
    }

    fn no_text(_: &str, _: u32, _: bool) -> Option<GrayImage> {
        None
    }

    #[test]
    fn a_line_covers_its_width_and_leaves_the_rest() {
        let mut picture = white(20, 20);
        let line = Mark::Line {
            from: Spot::new(2., 10.),
            to: Spot::new(18., 10.),
            color: RED,
            width: 4.,
        };
        draw(&mut picture, &line, &no_text);
        assert_eq!(picture.get_pixel(10, 10).0, RED);
        assert_eq!(picture.get_pixel(10, 9).0, RED);
        assert_eq!(picture.get_pixel(10, 2).0, [255, 255, 255, 255]);
    }

    #[test]
    fn a_rectangle_is_an_outline() {
        let mut picture = white(30, 30);
        let rectangle = Mark::Rectangle {
            from: Spot::new(5., 5.),
            to: Spot::new(25., 25.),
            color: RED,
            width: 2.,
        };
        draw(&mut picture, &rectangle, &no_text);
        assert_eq!(picture.get_pixel(5, 15).0, RED);
        assert_eq!(picture.get_pixel(15, 15).0, [255, 255, 255, 255]);
    }

    #[test]
    fn a_highlighter_is_as_dark_where_its_parts_meet() {
        let mut picture = white(40, 20);
        let stroke = Mark::Highlighter {
            points: vec![Spot::new(5., 10.), Spot::new(20., 10.), Spot::new(35., 10.)],
            color: [255, 255, 0, 255],
            width: 8.,
        };
        draw(&mut picture, &stroke, &no_text);
        assert_eq!(picture.get_pixel(20, 10), picture.get_pixel(12, 10));
        assert!(picture.get_pixel(20, 10)[2] < 255);
        assert!(picture.get_pixel(20, 10)[2] > 100);
    }

    #[test]
    fn an_arrow_has_a_head_at_its_end() {
        let [tip, one, other] = Mark::arrow_head(Spot::new(0., 0.), Spot::new(100., 0.), 4.);
        assert_eq!(tip, Spot::new(100., 0.));
        assert!(one.x < 100. && other.x < 100.);
        assert!((one.y + other.y).abs() < 0.001);
        let mut picture = white(120, 40);
        let arrow = Mark::Arrow {
            from: Spot::new(5., 20.),
            to: Spot::new(110., 20.),
            color: RED,
            width: 3.,
        };
        draw(&mut picture, &arrow, &no_text);
        assert_eq!(picture.get_pixel(100, 20).0, RED);
        assert_eq!(picture.get_pixel(100, 24).0, RED);
    }

    #[test]
    fn stamps_count_on_from_the_highest() {
        assert_eq!(Mark::next_stamp(&[]), 1);
        let stamp = |number| Mark::Stamp {
            at: Spot::default(),
            number,
            color: RED,
            size: 24.,
        };
        assert_eq!(Mark::next_stamp(&[stamp(1), stamp(4), stamp(2)]), 5);
    }

    #[test]
    fn a_text_is_put_where_its_cover_says() {
        let mut picture = white(10, 10);
        let mask = |_: &str, _: u32, _: bool| Some(GrayImage::from_pixel(2, 2, image::Luma([255])));
        let text = Mark::Text {
            at: Spot::new(3., 3.),
            text: "x".to_owned(),
            color: RED,
            size: 12.,
        };
        draw(&mut picture, &text, &mask);
        assert_eq!(picture.get_pixel(3, 3).0, RED);
        assert_eq!(picture.get_pixel(4, 4).0, RED);
        assert_eq!(picture.get_pixel(5, 5).0, [255, 255, 255, 255]);
    }

    #[test]
    fn pixelating_gives_each_square_its_mean() {
        let picture = RgbaImage::from_fn(4, 2, |x, _| match x < 2 {
            true => Rgba([0, 0, 0, 255]),
            false => Rgba([200, 100, 50, 255]),
        });
        let area = Area::between(Spot::new(0., 0.), Spot::new(4., 2.), (4, 2)).unwrap();
        let squares = pixelate(&picture, area, 4);
        assert!(squares.pixels().all(|pixel| pixel.0 == [100, 50, 25, 255]));
        let part = Area::between(Spot::new(9., 9.), Spot::new(1., 1.), (4, 2)).unwrap();
        assert_eq!(
            part,
            Area {
                x: 1,
                y: 1,
                width: 3,
                height: 1
            }
        );
        assert!(Area::between(Spot::new(1., 1.), Spot::new(1.2, 3.), (4, 2)).is_none());
    }

    #[test]
    fn blurring_and_cropping_keep_the_size_they_should() {
        let picture =
            RgbaImage::from_fn(8, 8, |x, y| Rgba([(x * 30) as u8, (y * 30) as u8, 0, 255]));
        let area = Area {
            x: 2,
            y: 2,
            width: 4,
            height: 4,
        };
        let blurred = blur(&picture, area, 3.);
        assert_eq!(blurred.dimensions(), (8, 8));
        assert_eq!(blurred.get_pixel(0, 0), picture.get_pixel(0, 0));
        assert_eq!(crop(&picture, area).dimensions(), (4, 4));
    }

    #[test]
    fn a_stamp_writes_in_the_colour_that_reads() {
        assert_eq!(contrast([255, 255, 0, 255]), [0, 0, 0, 255]);
        assert_eq!(contrast([0, 0, 200, 255]), [255, 255, 255, 255]);
    }
}
