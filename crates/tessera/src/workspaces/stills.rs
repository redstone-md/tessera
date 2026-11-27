// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The still pictures of hidden windows, made into images GPUI draws for
//! the views that show the windows of other workspaces: the overview, the
//! task view and the window switcher. Each picture is the one the passage
//! keeps of a window it hid ([`Passage::still`]); it is made into an image
//! once and kept until its window is shown or gone, the images least
//! lately asked for going first once together they are larger than
//! [`MOST_BYTES`]. GPUI keeps what it drew of an image until it is told
//! to let go, so the images let go of are handed back
//! ([`Stills::take_dropped`]) for that.

use std::sync::Arc;

use gpui::RenderImage;

use crate::platform::{AppWindowId, Passage};

/// How many bytes the images kept may have together: 64 MB.
const MOST_BYTES: usize = 64 << 20;

/// The images kept, the one least lately asked for first.
#[derive(Default)]
pub struct Stills {
    kept: Vec<(AppWindowId, Arc<RenderImage>, usize)>,
    /// The images let go of, which GPUI is still to drop.
    dropped: Vec<Arc<RenderImage>>,
}

impl Stills {
    /// The picture of the hidden window `id`, if the passage keeps one.
    pub fn of(&mut self, id: AppWindowId) -> Option<Arc<RenderImage>> {
        if let Some(place) = self.kept.iter().position(|(kept, ..)| *kept == id) {
            let entry = self.kept.remove(place);
            let image = entry.1.clone();
            self.kept.push(entry);
            return Some(image);
        }
        let still = Passage::still(id)?;
        let bytes = still.bgra.len();
        // GPUI takes the bytes in the order of the picture, blue first.
        let buffer = image::RgbaImage::from_raw(still.width, still.height, still.bgra.to_vec())?;
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        self.kept.push((id, image.clone(), bytes));
        let sizes: Vec<usize> = self.kept.iter().map(|(.., bytes)| *bytes).collect();
        let gone = self.kept.drain(..first_to_go(&sizes, MOST_BYTES));
        self.dropped.extend(gone.map(|(_, image, _)| image));
        Some(image)
    }

    /// Lets go of the images of the windows `keep` says no to.
    pub fn retain(&mut self, keep: impl Fn(AppWindowId) -> bool) {
        let (kept, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.kept)
            .into_iter()
            .partition(|(id, ..)| keep(*id));
        self.kept = kept;
        self.dropped
            .extend(gone.into_iter().map(|(_, image, _)| image));
    }

    /// Whether there are images let go of that GPUI is still to drop.
    pub fn dropped(&self) -> bool {
        !self.dropped.is_empty()
    }

    /// The images let go of since this was last asked, for GPUI to drop
    /// what it drew of them.
    pub fn take_dropped(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.dropped)
    }
}

/// How many of the first of `sizes` are to go for the rest to sum to
/// `most` or less; the last one stays, however large it is.
fn first_to_go(sizes: &[usize], most: usize) -> usize {
    let mut sum: usize = sizes.iter().sum();
    let mut gone = 0;
    while sum > most && gone + 1 < sizes.len() {
        sum -= sizes[gone];
        gone += 1;
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_images_least_lately_asked_for_go_first() {
        assert_eq!(first_to_go(&[], 10), 0);
        assert_eq!(first_to_go(&[4, 5], 10), 0);
        assert_eq!(first_to_go(&[4, 5, 6], 10), 2);
        assert_eq!(first_to_go(&[4, 5, 6], 11), 1);
    }

    #[test]
    fn the_image_just_made_stays() {
        assert_eq!(first_to_go(&[40], 10), 0);
        assert_eq!(first_to_go(&[3, 40], 10), 1);
    }
}
