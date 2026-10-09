// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Bounded image decoding on the caller's initialized MTA owner thread.
//! No apartment, worker, filesystem, UI or media/profile authority is owned here.

#[cfg(windows)]
mod sdk;
#[cfg(windows)]
pub(crate) use sdk::{decode_bytes, decode_reference};
#[cfg(test)]
mod tests;

pub(crate) const MAX_ENCODED_BYTES: u64 = 8 * 1024 * 1024;

/// Closed policies keep every ingress budget valid before SDK allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ImageDecodeLimits {
    max_encoded_bytes: u64,
    max_source_axis: u32,
    max_source_bytes: u64,
    max_output_axis: u32,
}

impl ImageDecodeLimits {
    pub(crate) const fn media_thumbnail() -> Self {
        Self {
            max_encoded_bytes: MAX_ENCODED_BYTES,
            max_source_axis: 16_384,
            max_source_bytes: 64 * 1024 * 1024,
            max_output_axis: 128,
        }
    }

    pub(crate) const fn profile_photo() -> Self {
        Self {
            max_output_axis: 512,
            ..Self::media_thumbnail()
        }
    }

    /// One sentinel byte detects growth without giving the codec mutable input.
    pub(crate) fn read_count(self, size: u64) -> Result<u32, ImageDecodeError> {
        if size == 0 || size > self.max_encoded_bytes {
            return Err(ImageDecodeError::invalid(
                "Image encoded stream is empty or exceeds 8 MiB",
            ));
        }
        size.checked_add(1)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| ImageDecodeError::invalid("Image read size overflow"))
    }

    pub(crate) fn copied_size(
        self,
        expected: u64,
        actual: u32,
        final_size: u64,
    ) -> Result<(), ImageDecodeError> {
        self.read_count(expected)?;
        if u64::from(actual) != expected || final_size != expected {
            return Err(ImageDecodeError::invalid(
                "Image stream changed or returned incomplete data",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct DecodedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl DecodedImage {
    pub(crate) fn width(&self) -> u32 {
        self.width
    }

    pub(crate) fn height(&self) -> u32 {
        self.height
    }

    pub(crate) fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    pub(crate) fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }
}

/// Native status is retained without native descriptions, paths or identities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImageDecodeError {
    hresult: Option<i32>,
    message: &'static str,
}

impl ImageDecodeError {
    fn invalid(message: &'static str) -> Self {
        Self {
            hresult: None,
            message,
        }
    }

    pub(crate) fn hresult(&self) -> Option<i32> {
        self.hresult
    }

    pub(crate) fn message(&self) -> &str {
        self.message
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DecodePlan {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bytes: usize,
}

impl DecodePlan {
    pub(crate) fn new(
        width: u32,
        height: u32,
        limits: ImageDecodeLimits,
    ) -> Result<Self, ImageDecodeError> {
        let source_bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|n| n.checked_mul(4));
        if width == 0
            || height == 0
            || width > limits.max_source_axis
            || height > limits.max_source_axis
            || source_bytes.is_none_or(|n| n > limits.max_source_bytes)
        {
            return Err(ImageDecodeError::invalid(
                "Image source dimensions exceed the decode budget",
            ));
        }
        let longest = width.max(height);
        let scale = |axis: u32| -> u32 {
            if longest <= limits.max_output_axis {
                axis
            } else {
                // Source-axis admission above makes this conversion lossless.
                ((u64::from(axis) * u64::from(limits.max_output_axis)) / u64::from(longest)).max(1)
                    as u32
            }
        };
        let width = scale(width);
        let height = scale(height);
        let bytes = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(4))
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| ImageDecodeError::invalid("Image output size overflow"))?;
        Ok(Self {
            width,
            height,
            bytes,
        })
    }

    pub(crate) fn accept(self, rgba: &[u8]) -> Result<DecodedImage, ImageDecodeError> {
        let expected = Self::new(self.width, self.height, ImageDecodeLimits::profile_photo())?;
        if expected != self {
            return Err(ImageDecodeError::invalid("Image output layout is invalid"));
        }
        if rgba.len() != self.bytes {
            return Err(ImageDecodeError::invalid(
                "Image decoder returned an invalid pixel buffer length",
            ));
        }
        let image = DecodedImage {
            width: self.width,
            height: self.height,
            rgba: rgba.to_vec(),
        };
        if image
            .rgba()
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[..3].iter().any(|channel| *channel > pixel[3]))
        {
            return Err(ImageDecodeError::invalid(
                "Image decoder returned non-premultiplied RGBA8",
            ));
        }
        Ok(image)
    }
}
