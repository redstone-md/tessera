// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A QR code, drawn as squares: black on white whatever the theme, since
//! that is what cameras read best, with the quiet zone readers need
//! around it.

use std::rc::Rc;

use gpui::{IntoElement, Pixels, Styled, black, canvas, fill, point, px, size, white};
use qrcodegen::{QrCode, QrCodeEcc};

/// How many modules of white the code is surrounded by, as the standard
/// asks for.
const QUIET_ZONE: usize = 4;

/// The modules of a QR code, encoded once and drawn as often as needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrPicture {
    /// How many modules a side has, without the quiet zone.
    side: usize,
    /// Whether each module is dark, row by row.
    dark: Vec<bool>,
}

impl QrPicture {
    /// The code of `text`, with medium error correction; `None` for a
    /// text too long for any code.
    pub fn of(text: &str) -> Option<Self> {
        let code = QrCode::encode_text(text, QrCodeEcc::Medium).ok()?;
        let side = code.size();
        let dark = (0..side)
            .flat_map(|y| (0..side).map(move |x| (x, y)))
            .map(|(x, y)| code.get_module(x, y))
            .collect();
        Some(Self {
            side: side as usize,
            dark,
        })
    }

    /// How many modules a side has, without the quiet zone.
    pub fn side(&self) -> usize {
        self.side
    }

    /// The dark stretches of each row, as the row, the first module and
    /// how many follow: one square is drawn for each instead of one for
    /// every module.
    fn runs(&self) -> Vec<(usize, usize, usize)> {
        let mut runs = Vec::new();
        for (y, row) in self.dark.chunks(self.side.max(1)).enumerate() {
            let mut x = 0;
            while x < row.len() {
                if !row[x] {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < row.len() && row[x] {
                    x += 1;
                }
                runs.push((y, start, x - start));
            }
        }
        runs
    }
}

/// How large a module is drawn when the code with its quiet zone is to
/// fit in `side` pixels: a whole number of pixels, so that the edges of
/// the modules are sharp, and at least one.
fn module_size(side: f32, modules: usize) -> f32 {
    let across = (modules + QUIET_ZONE * 2) as f32;
    (side / across).floor().max(1.)
}

/// `picture` drawn `side` pixels wide and high, on white.
pub fn qr_code(picture: Rc<QrPicture>, side: Pixels) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            window.paint_quad(fill(bounds, white()));
            let module = module_size(f32::from(bounds.size.width), picture.side());
            let drawn = module * picture.side() as f32;
            // The code stands in the middle of the white.
            let left = (f32::from(bounds.size.width) - drawn) / 2.;
            let top = (f32::from(bounds.size.height) - drawn) / 2.;
            let origin = bounds.origin + point(px(left.round()), px(top.round()));
            for (y, x, length) in picture.runs() {
                let at = origin + point(px(x as f32 * module), px(y as f32 * module));
                let extent = size(px(length as f32 * module), px(module));
                window.paint_quad(fill(gpui::Bounds::new(at, extent), black()));
            }
        },
    )
    .flex_none()
    .size(side)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_becomes_a_square_code_with_its_finder_patterns() {
        let picture = QrPicture::of("WIFI:T:WPA;S:Home;P:secret12;;").unwrap();
        let side = picture.side();
        assert!(side >= 21 && (side - 17).is_multiple_of(4));
        assert_eq!(picture.dark.len(), side * side);
        // The corner of each finder pattern is dark, the module inside its
        // ring light.
        let dark = |x: usize, y: usize| picture.dark[y * side + x];
        for (x, y) in [(0, 0), (side - 1, 0), (0, side - 1)] {
            assert!(dark(x, y));
        }
        assert!(!dark(1, 1));
        assert!(dark(3, 3));
    }

    #[test]
    fn the_dark_modules_are_drawn_as_runs() {
        let picture = QrPicture {
            side: 4,
            dark: [
                [true, true, false, true],
                [false, false, false, false],
                [true, true, true, true],
                [false, true, true, false],
            ]
            .concat(),
        };
        assert_eq!(picture.runs(), [(0, 0, 2), (0, 3, 1), (2, 0, 4), (3, 1, 2)]);
    }

    #[test]
    fn a_module_is_a_whole_number_of_pixels() {
        // 25 modules and 8 of quiet zone in 200 pixels: 6 each.
        assert_eq!(module_size(200., 25), 6.);
        assert_eq!(module_size(10., 25), 1.);
    }

    #[test]
    fn a_text_too_long_has_no_code() {
        assert!(QrPicture::of(&"x".repeat(8000)).is_none());
    }
}
