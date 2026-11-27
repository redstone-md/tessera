// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What is done to the picture of a display once it was fitted to it, as
//! the wallpapers of Seelen UI have it: a blur, the brightness, contrast
//! and saturation, a mirror image, and a color laid over it with one of
//! the ways CSS blends colors; and where in the display a picture that is
//! cut or does not fill it lies.
//!
//! Everything here works on the pixels of a [`BackdropPicture`], rows from
//! the top and a pixel blue, green and red, and is for a thread other than
//! that of the interface: a blur of a display of 4K takes a good part of a
//! second.

use serde::Deserialize;

use crate::platform::BackdropPicture;
use crate::theme::parse_rgb;

/// The largest blur, in pixels of the display.
const MOST_BLUR: f32 = 200.;

/// The largest brightness, contrast and saturation, as a factor.
const MOST_FACTOR: f32 = 4.;

/// How many box blurs one after the other stand for a Gaussian one.
const BOX_PASSES: usize = 3;

/// Which way the wallpaper is mirrored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flip {
    /// As it is.
    #[default]
    None,
    /// Left and right change places.
    Horizontal,
    /// Top and bottom change places.
    Vertical,
    /// Both, which turns it round by half a turn.
    Both,
}

impl Flip {
    /// Every way, in the order the settings offer them.
    pub const ALL: [Self; 4] = [Self::None, Self::Horizontal, Self::Vertical, Self::Both];

    /// Whether left and right change places.
    pub fn across(self) -> bool {
        matches!(self, Self::Horizontal | Self::Both)
    }

    /// Whether top and bottom change places.
    pub fn down(self) -> bool {
        matches!(self, Self::Vertical | Self::Both)
    }

    /// The name the configuration gives it.
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
            Self::Both => "both",
        }
    }
}

/// How the color laid over the wallpaper mixes with it, as the blend modes
/// of CSS (`mix-blend-mode`) do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Blend {
    /// The color over the picture, as much as its alpha says.
    #[default]
    Normal,
    /// Darkens: the two multiplied.
    Multiply,
    /// Lightens: the opposite of multiplying the opposites.
    Screen,
    /// Multiplies the dark parts and screens the light ones.
    Overlay,
    /// The darker of the two.
    Darken,
    /// The lighter of the two.
    Lighten,
    /// A gentler overlay.
    SoftLight,
}

impl Blend {
    /// Every way, in the order the settings offer them.
    pub const ALL: [Self; 7] = [
        Self::Normal,
        Self::Multiply,
        Self::Screen,
        Self::Overlay,
        Self::Darken,
        Self::Lighten,
        Self::SoftLight,
    ];

    /// The name the configuration gives it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Multiply => "multiply",
            Self::Screen => "screen",
            Self::Overlay => "overlay",
            Self::Darken => "darken",
            Self::Lighten => "lighten",
            Self::SoftLight => "soft-light",
        }
    }

    /// The share of light that `over`, the color laid on, makes of
    /// `under`, the picture; each from 0 to 1.
    pub fn mix(self, under: f32, over: f32) -> f32 {
        match self {
            Self::Normal => over,
            Self::Multiply => under * over,
            Self::Screen => 1. - (1. - under) * (1. - over),
            Self::Overlay => Self::hard(over, under),
            Self::Darken => under.min(over),
            Self::Lighten => under.max(over),
            Self::SoftLight => {
                if over <= 0.5 {
                    under - (1. - 2. * over) * under * (1. - under)
                } else {
                    let deep = if under <= 0.25 {
                        ((16. * under - 12.) * under + 4.) * under
                    } else {
                        under.sqrt()
                    };
                    under + (2. * over - 1.) * (deep - under)
                }
            }
        }
    }

    /// The hard light of CSS: multiplies where `over` is dark, screens
    /// where it is light. Overlay is hard light with the two swapped.
    fn hard(over: f32, under: f32) -> f32 {
        if under <= 0.5 {
            2. * under * over
        } else {
            1. - 2. * (1. - under) * (1. - over)
        }
    }
}

/// The color laid over the wallpaper, as the configuration gives it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayConfig {
    /// As `#rrggbbaa`, or `#rrggbb` for a color that covers all.
    pub color: String,
    #[serde(default)]
    pub blend: Blend,
}

/// The color laid over the wallpaper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overlay {
    /// Red, green, blue and how much of it is laid on, of 255.
    pub color: [u8; 4],
    pub blend: Blend,
}

/// A place in the display a picture keeps to, by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Place {
    Center,
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Place {
    /// Every place, row by row from the top left, as the settings lay
    /// them out.
    pub const GRID: [Self; 9] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Left,
        Self::Center,
        Self::Right,
        Self::BottomLeft,
        Self::Bottom,
        Self::BottomRight,
    ];

    /// The name the configuration gives it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Center => "center",
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Left => "left",
            Self::Right => "right",
            Self::TopLeft => "top-left",
            Self::TopRight => "top-right",
            Self::BottomLeft => "bottom-left",
            Self::BottomRight => "bottom-right",
        }
    }

    pub fn anchor(self) -> Anchor {
        let (x, y) = match self {
            Self::Center => (0.5, 0.5),
            Self::Top => (0.5, 0.),
            Self::Bottom => (0.5, 1.),
            Self::Left => (0., 0.5),
            Self::Right => (1., 0.5),
            Self::TopLeft => (0., 0.),
            Self::TopRight => (1., 0.),
            Self::BottomLeft => (0., 1.),
            Self::BottomRight => (1., 1.),
        };
        Anchor { x, y }
    }
}

/// Where a picture lies that is cut to cover its display or does not fill
/// it: a place by name, or as two percentages, across and down, as
/// `object-position` of CSS has it.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Position {
    Named(Place),
    At([f32; 2]),
}

impl Position {
    pub fn anchor(self) -> Anchor {
        match self {
            Self::Named(place) => place.anchor(),
            Self::At([x, y]) => Anchor {
                x: (x / 100.).clamp(0., 1.),
                y: (y / 100.).clamp(0., 1.),
            },
        }
    }
}

/// Where a picture lies in the room it has, from 0 at the left or top to
/// 1 at the right or bottom: what is cut off, or left over, is shared out
/// on the two sides in that proportion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub x: f32,
    pub y: f32,
}

impl Default for Anchor {
    fn default() -> Self {
        Self { x: 0.5, y: 0.5 }
    }
}

impl Anchor {
    /// Where something `left_over` long begins when it is shared out as
    /// `share` says, `share` being `x` or `y`.
    pub fn offset(share: f32, left_over: f32) -> f32 {
        left_over * share.clamp(0., 1.)
    }
}

/// The adjustments as one place of the configuration gives them, each key
/// left out where it says nothing: a display or a workspace takes what it
/// leaves out from the wallpaper of all displays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdjustKeys {
    pub blur: Option<f32>,
    pub saturation: Option<f32>,
    pub contrast: Option<f32>,
    pub brightness: Option<f32>,
    pub flip: Option<Flip>,
    pub overlay: Option<OverlayConfig>,
    pub position: Option<Position>,
}

impl AdjustKeys {
    /// These keys, and those of `under` where these say nothing.
    pub fn over(&self, under: &Self) -> Self {
        Self {
            blur: self.blur.or(under.blur),
            saturation: self.saturation.or(under.saturation),
            contrast: self.contrast.or(under.contrast),
            brightness: self.brightness.or(under.brightness),
            flip: self.flip.or(under.flip),
            overlay: self.overlay.clone().or_else(|| under.overlay.clone()),
            position: self.position.or(under.position),
        }
    }

    /// What the keys come to. A value out of its range is reported and
    /// taken as the nearest one in it, a color that is not one leaves the
    /// overlay out; `of` names the place for the report.
    pub fn resolve(&self, of: &str) -> Adjustments {
        let within = |key: &str, value: Option<f32>, most: f32, unset: f32| {
            let Some(value) = value else {
                return unset;
            };
            if !(0. ..=most).contains(&value) {
                report!("`{of}.{key}` goes from 0 to {most}, {value} is taken as the nearest");
            }
            if value.is_nan() {
                unset
            } else {
                value.clamp(0., most)
            }
        };
        let overlay = self.overlay.as_ref().and_then(|overlay| {
            parse_rgba(&overlay.color)
                .inspect_err(|error| report!("`{of}.overlay`: {error:#}, it is left out"))
                .ok()
                .map(|color| Overlay {
                    color,
                    blend: overlay.blend,
                })
        });
        Adjustments {
            blur: within("blur", self.blur, MOST_BLUR, 0.),
            saturation: within("saturation", self.saturation, MOST_FACTOR, 1.),
            contrast: within("contrast", self.contrast, MOST_FACTOR, 1.),
            brightness: within("brightness", self.brightness, MOST_FACTOR, 1.),
            flip: self.flip.unwrap_or_default(),
            overlay: overlay.filter(|overlay| overlay.color[3] > 0),
            anchor: self.position.map(Position::anchor).unwrap_or_default(),
        }
    }
}

/// A color as `#rrggbbaa`, or as `#rrggbb` with all of it laid on: red,
/// green, blue and alpha.
pub fn parse_rgba(text: &str) -> anyhow::Result<[u8; 4]> {
    let trimmed = text.trim();
    let wrong = || anyhow::anyhow!("`{text}` is not a color of the form #rrggbbaa");
    // Split by bytes only where every one is a character.
    if !trimmed.is_ascii() {
        return Err(wrong());
    }
    let (color, alpha) = match trimmed.len() {
        9 => trimmed.split_at(7),
        _ => (trimmed, "ff"),
    };
    let [red, green, blue] = parse_rgb(color).map_err(|_| wrong())?;
    if !alpha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(wrong());
    }
    let alpha = u8::from_str_radix(alpha, 16).map_err(|_| wrong())?;
    Ok([red, green, blue, alpha])
}

/// What is done to the picture of a display, as it came out of the
/// configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Adjustments {
    /// The blur, as the standard deviation in pixels, as `blur()` of CSS.
    pub blur: f32,
    /// Factors, 1 for as it is, as the filters of CSS take them.
    pub saturation: f32,
    pub contrast: f32,
    pub brightness: f32,
    pub flip: Flip,
    pub overlay: Option<Overlay>,
    /// Where a picture lies in its display.
    pub anchor: Anchor,
}

impl Default for Adjustments {
    fn default() -> Self {
        Self {
            blur: 0.,
            saturation: 1.,
            contrast: 1.,
            brightness: 1.,
            flip: Flip::None,
            overlay: None,
            anchor: Anchor::default(),
        }
    }
}

impl Adjustments {
    /// Whether the pixels of the picture are changed once it was fitted.
    pub fn changes_pixels(&self) -> bool {
        self.blur > 0. || self.changes_tone() || self.flip != Flip::None || self.overlay.is_some()
    }

    /// Whether brightness, contrast or saturation change anything.
    fn changes_tone(&self) -> bool {
        self.brightness != 1. || self.contrast != 1. || self.saturation != 1.
    }

    /// Whether the picture is shown just as it is fitted in the middle.
    pub fn is_plain(&self) -> bool {
        !self.changes_pixels() && self.anchor == Anchor::default()
    }

    /// What of these a video gets: a mirror image, the color over it and
    /// its place. Blur, brightness, contrast and saturation are left out.
    pub fn for_video(&self) -> Self {
        Self {
            flip: self.flip,
            overlay: self.overlay,
            anchor: self.anchor,
            ..Self::default()
        }
    }

    /// These as a video shows them: the compositor lays a color over a
    /// video only as `normal` or `darken`, so another blend is `normal`.
    pub fn as_video_shows(&self) -> Self {
        let overlay = self.overlay.map(|overlay| Overlay {
            blend: match overlay.blend {
                Blend::Darken => Blend::Darken,
                _ => Blend::Normal,
            },
            ..overlay
        });
        Self { overlay, ..*self }
    }

    /// Does all of it to `picture`: brightness and contrast, then the
    /// saturation, the blur, the color laid over and the mirror image.
    pub fn apply(&self, picture: &mut BackdropPicture) {
        let (width, height) = picture.size();
        let mut pixels = Pixels {
            width: width as usize,
            height: height as usize,
            stride: BackdropPicture::stride(width),
            bytes: picture.pixels_mut(),
        };
        if self.changes_tone() {
            pixels.tone(self.brightness, self.contrast, self.saturation);
        }
        if self.blur > 0. {
            pixels.blur(self.blur);
        }
        if let Some(overlay) = self.overlay {
            pixels.lay_over(overlay);
        }
        pixels.flip(self.flip);
    }
}

/// The pixels of a picture to change: rows of `stride` bytes, a pixel
/// being blue, green and red.
pub struct Pixels<'a> {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes: &'a mut [u8],
}

impl Pixels<'_> {
    /// The bytes of the pixels of each row, without what pads it.
    fn rows(&mut self) -> impl Iterator<Item = &mut [u8]> {
        let used = self.width * 3;
        self.bytes
            .chunks_exact_mut(self.stride)
            .take(self.height)
            .map(move |row| &mut row[..used])
    }

    /// Brightness and contrast, then saturation, as the filters of CSS
    /// `brightness()`, `contrast()` and `saturate()` do one after the
    /// other.
    fn tone(&mut self, brightness: f32, contrast: f32, saturation: f32) {
        let levels: [f32; 256] = std::array::from_fn(|value| {
            let value = value as f32 / 255. * brightness;
            ((value - 0.5) * contrast + 0.5).clamp(0., 1.)
        });
        let saturate = saturation != 1.;
        // The matrix of `saturate()`, rows for red, green and blue.
        let s = saturation;
        let matrix = [
            [
                0.2126 + 0.7874 * s,
                0.7152 - 0.7152 * s,
                0.0722 - 0.0722 * s,
            ],
            [
                0.2126 - 0.2126 * s,
                0.7152 + 0.2848 * s,
                0.0722 - 0.0722 * s,
            ],
            [
                0.2126 - 0.2126 * s,
                0.7152 - 0.7152 * s,
                0.0722 + 0.9278 * s,
            ],
        ];
        let byte = |share: f32| (share * 255.).round().clamp(0., 255.) as u8;
        for row in self.rows() {
            for pixel in row.as_chunks_mut::<3>().0 {
                let [blue, green, red] = *pixel;
                let (red, green, blue) = (
                    levels[usize::from(red)],
                    levels[usize::from(green)],
                    levels[usize::from(blue)],
                );
                *pixel = if saturate {
                    let mixed = matrix.map(|[r, g, b]| r * red + g * green + b * blue);
                    [byte(mixed[2]), byte(mixed[1]), byte(mixed[0])]
                } else {
                    [byte(blue), byte(green), byte(red)]
                };
            }
        }
    }

    /// Blurs by `sigma` pixels: three box blurs across and down, which
    /// come close to a Gaussian blur. The edges are taken to go on as the
    /// last pixel, so that they do not darken.
    fn blur(&mut self, sigma: f32) {
        let (width, height, stride) = (self.width, self.height, self.stride);
        if width == 0 || height == 0 {
            return;
        }
        let mut spare = vec![0_u8; self.bytes.len()];
        for size in box_sizes(sigma, BOX_PASSES) {
            let reach = (size - 1) / 2;
            if reach == 0 {
                continue;
            }
            box_across(self.bytes, &mut spare, width, height, stride, reach);
            box_down(&spare, self.bytes, width, height, stride, reach);
        }
    }

    /// Lays `overlay` over every pixel.
    fn lay_over(&mut self, overlay: Overlay) {
        let [red, green, blue, alpha] = overlay.color;
        let share = f32::from(alpha) / 255.;
        // What each value of each color becomes, blue first.
        let table = |over: u8| -> [u8; 256] {
            let over = f32::from(over) / 255.;
            std::array::from_fn(|under| {
                let under = under as f32 / 255.;
                let mixed = overlay.blend.mix(under, over).clamp(0., 1.);
                let out = under + (mixed - under) * share;
                (out * 255.).round().clamp(0., 255.) as u8
            })
        };
        let tables = [table(blue), table(green), table(red)];
        for row in self.rows() {
            for pixel in row.as_chunks_mut::<3>().0 {
                for (value, table) in pixel.iter_mut().zip(&tables) {
                    *value = table[usize::from(*value)];
                }
            }
        }
    }

    /// Mirrors the picture as `flip` says.
    fn flip(&mut self, flip: Flip) {
        if flip.across() {
            for row in self.rows() {
                row.as_chunks_mut::<3>().0.reverse();
            }
        }
        if flip.down() {
            let (stride, height, used) = (self.stride, self.height, self.width * 3);
            for top in 0..height / 2 {
                let bottom = height - 1 - top;
                let (upper, lower) = self.bytes.split_at_mut(bottom * stride);
                upper[top * stride..top * stride + used].swap_with_slice(&mut lower[..used]);
            }
        }
    }
}

/// The widths of `passes` box blurs one after the other that blur about
/// as a Gaussian blur of `sigma` does, each an odd number of pixels.
pub fn box_sizes(sigma: f32, passes: usize) -> Vec<usize> {
    let passes_f = passes as f32;
    let ideal = (12. * sigma * sigma / passes_f + 1.).sqrt();
    let mut lower = ideal.floor() as usize;
    if lower.is_multiple_of(2) {
        lower = lower.saturating_sub(1);
    }
    let lower = lower.max(1);
    let upper = lower + 2;
    let l = lower as f32;
    let wanted = (12. * sigma * sigma - passes_f * l * l - 4. * passes_f * l - 3. * passes_f)
        / (-4. * l - 4.);
    let smaller = (wanted.round().max(0.) as usize).min(passes);
    (0..passes)
        .map(|pass| if pass < smaller { lower } else { upper })
        .collect()
}

/// Averages each pixel of `from` with the `reach` pixels on either side
/// of it in its row, into `to`.
fn box_across(
    from: &[u8],
    to: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    reach: usize,
) {
    let window = (2 * reach + 1) as u32;
    let last = width - 1;
    for row in 0..height {
        let line = &from[row * stride..row * stride + width * 3];
        let out = &mut to[row * stride..row * stride + width * 3];
        for channel in 0..3 {
            let at = |column: usize| u32::from(line[column.min(last) * 3 + channel]);
            let mut sum = at(0) * (reach as u32 + 1);
            for column in 1..=reach {
                sum += at(column);
            }
            for column in 0..width {
                out[column * 3 + channel] = ((sum + window / 2) / window) as u8;
                sum += at(column + reach + 1);
                sum -= at(column.saturating_sub(reach));
            }
        }
    }
}

/// Averages each pixel of `from` with the `reach` pixels above and below
/// it, into `to`. The rows are gone through once, a sum kept for every
/// column, which reads the memory in its order.
fn box_down(from: &[u8], to: &mut [u8], width: usize, height: usize, stride: usize, reach: usize) {
    let window = (2 * reach + 1) as u32;
    let used = width * 3;
    let last = height - 1;
    let row = |index: usize| &from[index.min(last) * stride..index.min(last) * stride + used];
    let mut sums: Vec<u32> = row(0)
        .iter()
        .map(|&value| u32::from(value) * (reach as u32 + 1))
        .collect();
    for index in 1..=reach {
        for (sum, &value) in sums.iter_mut().zip(row(index)) {
            *sum += u32::from(value);
        }
    }
    for index in 0..height {
        let out = &mut to[index * stride..index * stride + used];
        for (value, sum) in out.iter_mut().zip(&sums) {
            *value = ((sum + window / 2) / window) as u8;
        }
        let (coming, going) = (row(index + reach + 1), row(index.saturating_sub(reach)));
        for ((sum, &add), &take) in sums.iter_mut().zip(coming).zip(going) {
            *sum = *sum + u32::from(add) - u32::from(take);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture of `width` by `height` whose pixels `color` gives as red,
    /// green and blue.
    fn picture(width: u32, height: u32, color: impl Fn(u32, u32) -> [u8; 3]) -> BackdropPicture {
        let stride = BackdropPicture::stride(width);
        let mut pixels = vec![0; stride * height as usize];
        for y in 0..height {
            for x in 0..width {
                let [red, green, blue] = color(x, y);
                let at = y as usize * stride + x as usize * 3;
                pixels[at..at + 3].copy_from_slice(&[blue, green, red]);
            }
        }
        BackdropPicture::new(width, height, pixels)
    }

    /// The pixel at `x`, `y` as red, green and blue.
    fn pixel(picture: &BackdropPicture, x: u32, y: u32) -> [u8; 3] {
        let stride = BackdropPicture::stride(picture.size().0);
        let at = y as usize * stride + x as usize * 3;
        let [blue, green, red] = picture.pixels()[at..at + 3] else {
            unreachable!()
        };
        [red, green, blue]
    }

    fn adjusted(adjust: Adjustments, picture: &mut BackdropPicture) {
        adjust.apply(picture);
    }

    #[test]
    fn colors_with_an_alpha_are_read() {
        assert_eq!(parse_rgba("#10203040").unwrap(), [0x10, 0x20, 0x30, 0x40]);
        assert_eq!(parse_rgba("#102030").unwrap(), [0x10, 0x20, 0x30, 0xff]);
        for wrong in [
            "#1020304",
            "102030",
            "#10203g40",
            "#102030+1",
            "",
            "#abcde\u{e9}f",
        ] {
            assert!(parse_rgba(wrong).is_err(), "{wrong:?} was taken");
        }
    }

    #[test]
    fn a_video_lays_a_color_on_as_normal_or_darken() {
        let laid = |blend| Adjustments {
            blur: 3.,
            overlay: Some(Overlay {
                color: [1, 2, 3, 4],
                blend,
            }),
            ..Adjustments::default()
        };
        let blend_of = |adjust: Adjustments| adjust.as_video_shows().overlay.map(|o| o.blend);
        assert_eq!(blend_of(laid(Blend::Screen)), Some(Blend::Normal));
        assert_eq!(blend_of(laid(Blend::Darken)), Some(Blend::Darken));
        assert_eq!(laid(Blend::Multiply).as_video_shows().blur, 3.);
    }

    #[test]
    fn the_keys_of_a_place_win_over_those_of_all() {
        let all = AdjustKeys {
            blur: Some(4.),
            contrast: Some(1.5),
            ..AdjustKeys::default()
        };
        let place = AdjustKeys {
            blur: Some(0.),
            flip: Some(Flip::Both),
            ..AdjustKeys::default()
        };
        let both = place.over(&all).resolve("wallpaper");
        assert_eq!(both.blur, 0.);
        assert_eq!(both.contrast, 1.5);
        assert_eq!(both.flip, Flip::Both);
        assert!(both.changes_pixels());
        assert!(AdjustKeys::default().resolve("wallpaper").is_plain());
        let far = AdjustKeys {
            brightness: Some(9.),
            ..AdjustKeys::default()
        };
        assert_eq!(far.resolve("wallpaper").brightness, MOST_FACTOR);
    }

    #[test]
    fn positions_are_named_or_given_in_percent() {
        #[derive(Deserialize)]
        struct Given {
            position: Position,
        }
        let read = |text: &str| toml::from_str::<Given>(text).unwrap().position.anchor();
        assert_eq!(read("position = \"top-right\""), Anchor { x: 1., y: 0. });
        assert_eq!(read("position = \"center\""), Anchor::default());
        assert_eq!(read("position = [25, 100]"), Anchor { x: 0.25, y: 1. });
        assert_eq!(read("position = [-5, 300.0]"), Anchor { x: 0., y: 1. });
        assert!(toml::from_str::<Given>("position = \"middle\"").is_err());
        for place in Place::GRID {
            let text = format!("position = \"{}\"", place.name());
            assert_eq!(read(&text), place.anchor());
        }
    }

    #[test]
    fn the_blend_modes_mix_as_css_does() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(close(Blend::Multiply.mix(0.5, 0.5), 0.25));
        assert!(close(Blend::Screen.mix(0.5, 0.5), 0.75));
        assert!(close(Blend::Overlay.mix(0.25, 0.5), 0.25));
        assert!(close(Blend::Overlay.mix(0.75, 1.), 1.));
        assert!(close(Blend::Darken.mix(0.3, 0.6), 0.3));
        assert!(close(Blend::Lighten.mix(0.3, 0.6), 0.6));
        assert!(close(Blend::SoftLight.mix(0.5, 0.5), 0.5));
        assert!(close(Blend::SoftLight.mix(0.25, 0.), 0.25 - 0.25 * 0.75));
        // The edges stay where they are for every mode.
        for blend in Blend::ALL {
            for value in [0., 1.] {
                let mixed = blend.mix(value, value);
                assert!(close(mixed, value), "{blend:?} moves {value} to {mixed}");
            }
        }
    }

    #[test]
    fn a_color_laid_over_is_mixed_by_its_alpha() {
        let mut shown = picture(2, 1, |_, _| [100, 100, 100]);
        let overlay = Overlay {
            color: [200, 0, 100, 128],
            blend: Blend::Normal,
        };
        adjusted(
            Adjustments {
                overlay: Some(overlay),
                ..Adjustments::default()
            },
            &mut shown,
        );
        assert_eq!(pixel(&shown, 0, 0), [150, 50, 100]);
        let mut darkened = picture(1, 1, |_, _| [200, 200, 200]);
        let black = Overlay {
            color: [0, 0, 0, 255],
            blend: Blend::Multiply,
        };
        adjusted(
            Adjustments {
                overlay: Some(black),
                ..Adjustments::default()
            },
            &mut darkened,
        );
        assert_eq!(pixel(&darkened, 0, 0), [0, 0, 0]);
    }

    #[test]
    fn brightness_contrast_and_saturation_change_the_tone() {
        let mut darker = picture(1, 1, |_, _| [200, 100, 50]);
        adjusted(
            Adjustments {
                brightness: 0.5,
                ..Adjustments::default()
            },
            &mut darker,
        );
        assert_eq!(pixel(&darker, 0, 0), [100, 50, 25]);
        let mut flat = picture(1, 1, |_, _| [255, 0, 128]);
        adjusted(
            Adjustments {
                contrast: 0.,
                ..Adjustments::default()
            },
            &mut flat,
        );
        assert_eq!(pixel(&flat, 0, 0), [128, 128, 128]);
        let mut gray = picture(1, 1, |_, _| [255, 0, 0]);
        adjusted(
            Adjustments {
                saturation: 0.,
                ..Adjustments::default()
            },
            &mut gray,
        );
        let [red, green, blue] = pixel(&gray, 0, 0);
        assert_eq!((red, green, blue), (54, 54, 54));
    }

    #[test]
    fn the_box_blurs_come_close_to_the_gaussian() {
        for sigma in [1., 3., 10., 40.] {
            let sizes = box_sizes(sigma, 3);
            assert_eq!(sizes.len(), 3);
            assert!(sizes.iter().all(|size| size % 2 == 1));
            // Three boxes have the variance of the sum of theirs.
            let variance: f32 = sizes
                .iter()
                .map(|&size| ((size * size) as f32 - 1.) / 12.)
                .sum();
            assert!(
                (variance.sqrt() - sigma).abs() < 1.,
                "{sigma} became {variance}"
            );
        }
    }

    #[test]
    fn a_blur_spreads_a_line_and_keeps_an_even_color() {
        let mut even = picture(9, 7, |_, _| [90, 40, 10]);
        adjusted(
            Adjustments {
                blur: 3.,
                ..Adjustments::default()
            },
            &mut even,
        );
        for (x, y) in [(0, 0), (4, 3), (8, 6)] {
            assert_eq!(pixel(&even, x, y), [90, 40, 10]);
        }
        let mut line = picture(11, 3, |x, _| if x == 5 { [255; 3] } else { [0; 3] });
        adjusted(
            Adjustments {
                blur: 1.5,
                ..Adjustments::default()
            },
            &mut line,
        );
        let middle = pixel(&line, 5, 1)[0];
        let near = pixel(&line, 4, 1)[0];
        let far = pixel(&line, 0, 1)[0];
        assert!(
            middle < 255 && middle > near && near > far,
            "{middle} {near} {far}"
        );
        assert_eq!(pixel(&line, 4, 1), pixel(&line, 6, 1));
    }

    #[test]
    fn a_picture_is_mirrored_either_way() {
        let corner = |x, y| if (x, y) == (0, 0) { [255; 3] } else { [0; 3] };
        let mut across = picture(3, 2, corner);
        adjusted(
            Adjustments {
                flip: Flip::Horizontal,
                ..Adjustments::default()
            },
            &mut across,
        );
        assert_eq!(pixel(&across, 2, 0), [255; 3]);
        assert_eq!(pixel(&across, 0, 0), [0; 3]);
        let mut both = picture(3, 3, corner);
        adjusted(
            Adjustments {
                flip: Flip::Both,
                ..Adjustments::default()
            },
            &mut both,
        );
        assert_eq!(pixel(&both, 2, 2), [255; 3]);
        assert_eq!(pixel(&both, 0, 0), [0; 3]);
        assert!(Flip::Both.across() && Flip::Both.down());
        assert!(!Flip::Vertical.across());
    }
}
