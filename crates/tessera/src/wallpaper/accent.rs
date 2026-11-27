// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The color a wallpaper stands out with, for a theme whose seed is
//! `wallpaper`.

use image::RgbImage;

/// How many hues are told apart.
const HUES: usize = 64;

/// How many pixels across and down are looked at, at most.
const SAMPLES: u32 = 64;

/// The share of the pixels looked at that must be colorful, weighed by
/// how colorful they are, for the picture to have a color of its own.
const COLORFUL: f32 = 0.02;

/// The color `image` stands out with, as red, green and blue: the hue its
/// colorful and bright pixels have most, weighed by how saturated and how
/// bright each is, and the mean of the pixels of that hue and the two next
/// to it. A picture with hardly any color gives the mean of its pixels.
/// It looks at 64 by 64 pixels at most, spread evenly.
pub fn dominant_color(image: &RgbImage) -> Option<[u8; 3]> {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let step_x = (width / SAMPLES).max(1);
    let step_y = (height / SAMPLES).max(1);
    let mut weights = [0_f32; HUES];
    let mut sums = [[0_f32; 3]; HUES];
    let mut mean = [0_f32; 3];
    let mut count = 0_f32;
    for y in (step_y / 2..height).step_by(step_y as usize) {
        for x in (step_x / 2..width).step_by(step_x as usize) {
            let [red, green, blue] = image.get_pixel(x, y).0.map(f32::from);
            mean[0] += red;
            mean[1] += green;
            mean[2] += blue;
            count += 1.;
            let (hue, saturation, value) = hsv(red, green, blue);
            let weight = saturation * value;
            if weight <= 0. {
                continue;
            }
            let bin = ((hue / 360. * HUES as f32) as usize).min(HUES - 1);
            weights[bin] += weight;
            sums[bin][0] += red * weight;
            sums[bin][1] += green * weight;
            sums[bin][2] += blue * weight;
        }
    }
    let neighbours = |bin: usize| [(bin + HUES - 1) % HUES, bin, (bin + 1) % HUES];
    let around = |bin: usize| neighbours(bin).iter().map(|&at| weights[at]).sum::<f32>();
    let best = (0..HUES).max_by(|a, b| around(*a).total_cmp(&around(*b)))?;
    let weight = around(best);
    let color = if weight >= count * COLORFUL && weight > 0. {
        let mut color = [0_f32; 3];
        for at in neighbours(best) {
            for (to, from) in color.iter_mut().zip(sums[at]) {
                *to += from;
            }
        }
        color.map(|channel| channel / weight)
    } else {
        mean.map(|channel| channel / count.max(1.))
    };
    Some(color.map(|channel| channel.round().clamp(0., 255.) as u8))
}

/// The hue in degrees, the saturation and the value, each from 0 to 1, of
/// a color whose channels go from 0 to 255.
fn hsv(red: f32, green: f32, blue: f32) -> (f32, f32, f32) {
    let most = red.max(green).max(blue);
    let least = red.min(green).min(blue);
    let range = most - least;
    let value = most / 255.;
    let saturation = if most > 0. { range / most } else { 0. };
    let hue = if range <= 0. {
        0.
    } else if most == red {
        60. * ((green - blue) / range).rem_euclid(6.)
    } else if most == green {
        60. * ((blue - red) / range + 2.)
    } else {
        60. * ((red - green) / range + 4.)
    };
    (hue, saturation, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_of_one_color_has_that_color() {
        let image = RgbImage::from_pixel(300, 200, image::Rgb([20, 90, 200]));
        assert_eq!(dominant_color(&image), Some([20, 90, 200]));
    }

    #[test]
    fn a_small_colorful_part_wins_over_much_grey() {
        // A grey picture with an orange tenth.
        let image = RgbImage::from_fn(100, 100, |x, _| {
            image::Rgb(if x < 10 {
                [240, 120, 20]
            } else {
                [128, 128, 128]
            })
        });
        assert_eq!(dominant_color(&image), Some([240, 120, 20]));
    }

    #[test]
    fn the_hue_most_seen_wins_over_another() {
        let image = RgbImage::from_fn(90, 30, |x, _| {
            image::Rgb(if x < 60 { [30, 160, 60] } else { [200, 30, 40] })
        });
        assert_eq!(dominant_color(&image), Some([30, 160, 60]));
    }

    #[test]
    fn a_grey_picture_gives_its_mean() {
        let image = RgbImage::from_fn(10, 10, |x, _| {
            image::Rgb([if x < 5 { 100 } else { 200 }; 3])
        });
        assert_eq!(dominant_color(&image), Some([150, 150, 150]));
        assert_eq!(dominant_color(&RgbImage::new(0, 0)), None);
    }

    #[test]
    fn hues_are_in_degrees() {
        assert_eq!(hsv(255., 0., 0.).0, 0.);
        assert_eq!(hsv(0., 255., 0.).0, 120.);
        assert_eq!(hsv(0., 0., 255.).0, 240.);
        assert_eq!(hsv(255., 0., 255.).0, 300.);
        assert_eq!(hsv(0., 0., 0.), (0., 0., 0.));
    }
}
