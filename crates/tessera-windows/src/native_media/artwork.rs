// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use tessera_system::media::{MediaArtwork, MediaError, MediaErrorKind};

pub(super) const MAX_ENCODED_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SOURCE_AXIS: u32 = 16_384;
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OUTPUT_AXIS: u32 = 128;

/// A bounded read includes one sentinel byte to detect source growth. Never
/// hand the decoder an externally mutable or unbounded thumbnail stream.
pub(super) fn read_count(size: u64) -> Result<u32, MediaError> {
    if size == 0 || size > MAX_ENCODED_BYTES {
        return Err(invalid(
            "Thumbnail encoded stream is empty or exceeds 8 MiB",
        ));
    }
    let count = size.checked_add(1).and_then(|n| u32::try_from(n).ok());
    count.ok_or_else(|| invalid("Thumbnail read size overflow"))
}

pub(super) fn copied_size(expected: u64, actual: u32, final_size: u64) -> Result<(), MediaError> {
    read_count(expected)?;
    if u64::from(actual) != expected || final_size != expected {
        return Err(invalid(
            "Thumbnail stream changed or returned incomplete data",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DecodePlan {
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
}

impl DecodePlan {
    pub(super) fn new(width: u32, height: u32) -> Result<Self, MediaError> {
        let source_bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|n| n.checked_mul(4));
        if width == 0
            || height == 0
            || width > MAX_SOURCE_AXIS
            || height > MAX_SOURCE_AXIS
            || source_bytes.is_none_or(|n| n > MAX_SOURCE_BYTES)
        {
            return Err(invalid(
                "Thumbnail source dimensions exceed the decode budget",
            ));
        }
        let longest = width.max(height);
        let scale = |axis: u32| -> u32 {
            if longest <= MAX_OUTPUT_AXIS {
                axis
            } else {
                // Source-axis admission above makes this conversion lossless.
                ((u64::from(axis) * u64::from(MAX_OUTPUT_AXIS)) / u64::from(longest)).max(1) as u32
            }
        };
        let width = scale(width);
        let height = scale(height);
        let bytes = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(4))
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| invalid("Thumbnail output size overflow"))?;
        Ok(Self {
            width,
            height,
            bytes,
        })
    }

    pub(super) fn accept(self, rgba: &[u8]) -> Result<MediaArtwork, MediaError> {
        if rgba.len() != self.bytes {
            return Err(invalid(
                "Thumbnail decoder returned an invalid pixel buffer length",
            ));
        }
        // The SDK output is explicitly RGBA8 premultiplied; portable validation
        // additionally rejects malformed alpha channels and incorrect lengths.
        MediaArtwork::new(self.width, self.height, rgba.to_vec())
    }
}

fn invalid(message: &str) -> MediaError {
    MediaError::new(MediaErrorKind::Other, message)
}
