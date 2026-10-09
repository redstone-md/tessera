// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use crate::image_decode::{DecodedImage, ImageDecodeError};
use tessera_system::media::{MediaArtwork, MediaError, MediaErrorKind};

#[cfg(test)]
pub(super) use crate::image_decode::MAX_ENCODED_BYTES;
#[cfg(test)]
use crate::image_decode::{DecodePlan as ImagePlan, ImageDecodeLimits};

pub(super) fn error(error: ImageDecodeError) -> MediaError {
    match error.hresult() {
        Some(code) => MediaError::with_hresult(MediaErrorKind::Other, error.message(), code),
        None => MediaError::new(MediaErrorKind::Other, error.message()),
    }
}

pub(super) fn image(image: DecodedImage) -> Result<MediaArtwork, MediaError> {
    MediaArtwork::new(image.width(), image.height(), image.into_rgba())
}

// Preserve the existing media policy test seam without a second decode policy.
#[cfg(test)]
pub(super) fn read_count(size: u64) -> Result<u32, MediaError> {
    ImageDecodeLimits::media_thumbnail()
        .read_count(size)
        .map_err(error)
}

#[cfg(test)]
pub(super) fn copied_size(expected: u64, actual: u32, final_size: u64) -> Result<(), MediaError> {
    ImageDecodeLimits::media_thumbnail()
        .copied_size(expected, actual, final_size)
        .map_err(error)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DecodePlan {
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
}

#[cfg(test)]
impl DecodePlan {
    pub(super) fn new(width: u32, height: u32) -> Result<Self, MediaError> {
        let plan =
            ImagePlan::new(width, height, ImageDecodeLimits::media_thumbnail()).map_err(error)?;
        Ok(Self {
            width: plan.width,
            height: plan.height,
            bytes: plan.bytes,
        })
    }

    pub(super) fn accept(self, rgba: &[u8]) -> Result<MediaArtwork, MediaError> {
        ImagePlan {
            width: self.width,
            height: self.height,
            bytes: self.bytes,
        }
        .accept(rgba)
        .map_err(error)
        .and_then(image)
    }
}
