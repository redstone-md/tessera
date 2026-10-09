// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pinned windows 0.62.2 calls; all objects remain on the caller's MTA thread.

use super::{DecodePlan, DecodedImage, ImageDecodeError, ImageDecodeLimits};
use windows::{
    Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
        ExifOrientationMode,
    },
    Storage::Streams::{
        Buffer, DataWriter, IBuffer, IRandomAccessStreamReference,
        IRandomAccessStreamWithContentType, InMemoryRandomAccessStream, InputStreamOptions,
    },
};

pub(crate) fn decode_reference(
    reference: &IRandomAccessStreamReference,
    limits: ImageDecodeLimits,
) -> Result<DecodedImage, ImageDecodeError> {
    let source = SourceStream(
        reference
            .OpenReadAsync()
            .and_then(|operation| operation.join())
            .map_err(native_error)?,
    );
    let size = source.0.Size().map_err(native_error)?;
    let count = limits.read_count(size)?;
    source.0.Seek(0).map_err(native_error)?;
    let buffer = Buffer::Create(count).map_err(native_error)?;
    let buffer = source
        .0
        .ReadAsync(&buffer, count, InputStreamOptions::None)
        .and_then(|operation| operation.join())
        .map_err(native_error)?;
    limits.copied_size(
        size,
        buffer.Length().map_err(native_error)?,
        source.0.Size().map_err(native_error)?,
    )?;
    let copy = copy_buffer(&buffer, size, limits)?;
    decode_copy(&copy, limits)
}

pub(crate) fn decode_bytes(
    bytes: &[u8],
    limits: ImageDecodeLimits,
) -> Result<DecodedImage, ImageDecodeError> {
    let size = u64::try_from(bytes.len())
        .map_err(|_| ImageDecodeError::invalid("Image encoded length overflow"))?;
    // Check before DataWriter allocates; its generated WriteBytes also needs u32.
    limits.read_count(size)?;
    let writer = BufferWriter(DataWriter::new().map_err(native_error)?);
    writer.0.WriteBytes(bytes).map_err(native_error)?;
    let buffer = writer.0.DetachBuffer().map_err(native_error)?;
    limits.copied_size(size, buffer.Length().map_err(native_error)?, size)?;
    let copy = copy_buffer(&buffer, size, limits)?;
    decode_copy(&copy, limits)
}

fn copy_buffer(
    buffer: &IBuffer,
    size: u64,
    limits: ImageDecodeLimits,
) -> Result<MemoryStream, ImageDecodeError> {
    let copy = MemoryStream(InMemoryRandomAccessStream::new().map_err(native_error)?);
    let written = copy
        .0
        .WriteAsync(buffer)
        .and_then(|operation| operation.join())
        .map_err(native_error)?;
    limits.copied_size(size, written, copy.0.Size().map_err(native_error)?)?;
    copy.0.Seek(0).map_err(native_error)?;
    Ok(copy)
}

/// The codec only sees our private immutable copy, never the source reference.
fn decode_copy(
    copy: &MemoryStream,
    limits: ImageDecodeLimits,
) -> Result<DecodedImage, ImageDecodeError> {
    let decoder = BitmapDecoder::CreateAsync(&copy.0)
        .and_then(|operation| operation.join())
        .map_err(native_error)?;
    // Admission precedes pixel decode, transform/output allocation and copying.
    let plan = DecodePlan::new(
        decoder.PixelWidth().map_err(native_error)?,
        decoder.PixelHeight().map_err(native_error)?,
        limits,
    )?;
    let transform = BitmapTransform::new().map_err(native_error)?;
    transform.SetScaledWidth(plan.width).map_err(native_error)?;
    transform
        .SetScaledHeight(plan.height)
        .map_err(native_error)?;
    let pixels = decoder
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Rgba8,
            BitmapAlphaMode::Premultiplied,
            &transform,
            ExifOrientationMode::IgnoreExifOrientation,
            ColorManagementMode::ColorManageToSRgb,
        )
        .and_then(|operation| operation.join())
        .map_err(native_error)?;
    let rgba = pixels.DetachPixelData().map_err(native_error)?;
    plan.accept(&rgba)
}

// Every early return/unwind closes before releasing on the owning MTA thread.
// Drop remains best effort, preserving the production decoder's return policy.
struct SourceStream(IRandomAccessStreamWithContentType);
impl Drop for SourceStream {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}

struct MemoryStream(InMemoryRandomAccessStream);
impl Drop for MemoryStream {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}

struct BufferWriter(DataWriter);
impl Drop for BufferWriter {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}

fn native_error(error: windows::core::Error) -> ImageDecodeError {
    ImageDecodeError {
        hresult: Some(error.code().0),
        message: "Windows image decoding failed",
    }
}
