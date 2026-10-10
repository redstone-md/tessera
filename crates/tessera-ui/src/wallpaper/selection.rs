// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native selection authority and its retained, optional presentation facts.

use tessera_system::wallpaper::{WallpaperApplyScope, WallpaperSelection};

use crate::SourceSeed;

use super::image;

/// Send-only choose preparation; no Slint image or UI/SDK calls belong here.
pub(super) struct PreparedSelection {
    image: WallpaperSelection,
    seed: Option<SourceSeed>,
}

impl PreparedSelection {
    pub(super) fn new(image: WallpaperSelection) -> Self {
        let seed = image.preview.as_ref().and_then(|preview| {
            // A maintained quantizer failure must not swallow the native
            // completion or discard an otherwise real thumbnail/selection.
            std::panic::catch_unwind(|| image::source_seed(preview))
                .ok()
                .flatten()
        });
        Self { image, seed }
    }
}

/// Display indices resolve only inside this validated, native-issued selection.
pub(super) struct SelectedImage {
    pub(super) image: WallpaperSelection,
    pub(super) scope: WallpaperApplyScope,
    preview: Option<slint::Image>,
    seed: Option<SourceSeed>,
    colors_previewed: bool,
}

impl SelectedImage {
    pub(super) fn new(prepared: PreparedSelection) -> Option<Self> {
        let PreparedSelection { mut image, seed } = prepared;
        if !(1..=32).contains(&image.monitors.len())
            || image.monitors.iter().enumerate().any(|(index, monitor)| {
                image.monitors[..index]
                    .iter()
                    .any(|previous| previous.target == monitor.target)
            })
        {
            return None;
        }
        image.caption = bounded_caption(&image.caption, 96)?;
        for monitor in &mut image.monitors {
            // Bound malformed external metadata inspection as well as display.
            if monitor.caption.chars().take(257).count() > 256 {
                return None;
            }
            monitor.caption = bounded_caption(&monitor.caption, 256)?;
        }
        let preview = image.preview.as_ref().map(image::slint_image);
        Some(Self {
            image,
            scope: WallpaperApplyScope::AllCaptured,
            preview,
            seed,
            colors_previewed: false,
        })
    }

    pub(super) fn scope_at(&self, index: i32) -> Option<WallpaperApplyScope> {
        if index == 0 {
            return Some(WallpaperApplyScope::AllCaptured);
        }
        let index = usize::try_from(index).ok()?.checked_sub(1)?;
        self.image
            .monitors
            .get(index)
            .map(|monitor| WallpaperApplyScope::Monitor(monitor.target.clone()))
    }

    pub(super) fn captions(&self) -> Vec<slint::SharedString> {
        std::iter::once("All captured displays".into())
            .chain(
                self.image
                    .monitors
                    .iter()
                    .map(|monitor| monitor.caption.as_str().into()),
            )
            .collect()
    }

    pub(super) fn requested(&self) -> Option<u32> {
        match &self.scope {
            WallpaperApplyScope::AllCaptured => u32::try_from(self.image.monitors.len()).ok(),
            WallpaperApplyScope::Monitor(target) => self
                .image
                .monitors
                .iter()
                .any(|monitor| &monitor.target == target)
                .then_some(1),
        }
    }

    pub(super) fn notice(&self, scope: &WallpaperApplyScope) -> Option<String> {
        let scope = match scope {
            WallpaperApplyScope::AllCaptured => {
                format!("all {} captured displays", self.image.monitors.len())
            }
            WallpaperApplyScope::Monitor(target) => {
                let monitor = self
                    .image
                    .monitors
                    .iter()
                    .find(|monitor| &monitor.target == target)?;
                format!("captured display {}", monitor.caption)
            }
        };
        Some(format!(
            "Selected: {}. Scope: {}. Image usability is checked by Windows.",
            self.image.caption, scope
        ))
    }

    pub(super) fn seed(&self) -> Option<SourceSeed> {
        self.seed
    }

    pub(super) fn mark_colors_previewed(&mut self) {
        self.colors_previewed = true;
    }

    pub(super) fn presentation(&self) -> SelectionPresentation {
        let preview_available = self.preview.is_some();
        let preview_status = if preview_available {
            "Captured native Windows Shell thumbnail of the selected file only. This approximation is not current Windows wallpaper or desktop-rendered pixels."
        } else {
            "Windows did not provide a usable native Shell thumbnail for this selection. No preview image is shown."
        };
        let colors_status = match self.seed {
            Some(seed) if self.colors_previewed => format!(
                "Thumbnail source #{:06x} was previewed in Material. Save to keep; Cancel restores the saved source. This action did not change Windows wallpaper.",
                seed.rgb()
            ),
            Some(seed) => format!(
                "Source #{:06x} is derived from opaque pixels in the selected native Shell thumbnail, an approximation of the image. Preview changes Material only; Save to keep, Cancel to restore.",
                seed.rgb()
            ),
            None if preview_available => {
                "No usable opaque image color could be derived from this native thumbnail. No synthetic color is offered.".into()
            }
            None => "Image colors are unavailable without a usable native Shell thumbnail.".into(),
        };
        SelectionPresentation {
            preview: self.preview.clone().unwrap_or_default(),
            preview_available,
            preview_status: preview_status.into(),
            seed_rgb: self
                .seed
                .and_then(|seed| i32::try_from(seed.rgb()).ok())
                .unwrap_or(-1),
            colors_available: self.seed.is_some(),
            colors_status: colors_status.into(),
        }
    }
}

pub(super) struct SelectionPresentation {
    pub(super) preview: slint::Image,
    pub(super) preview_available: bool,
    pub(super) preview_status: slint::SharedString,
    pub(super) seed_rgb: i32,
    pub(super) colors_available: bool,
    pub(super) colors_status: slint::SharedString,
}

impl Default for SelectionPresentation {
    fn default() -> Self {
        Self {
            preview: slint::Image::default(),
            preview_available: false,
            preview_status: "No current image selection.".into(),
            seed_rgb: -1,
            colors_available: false,
            colors_status: "Choose a fresh image to preview its native thumbnail colors.".into(),
        }
    }
}

pub(super) fn bounded_caption(value: &str, limit: usize) -> Option<String> {
    let caption: String = value
        .chars()
        .take(limit)
        .filter(|character| {
            !character.is_control()
                && !matches!(*character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    let caption = caption.trim();
    (!caption.is_empty()).then(|| caption.to_owned())
}
