// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Actual SDK decoder tests, no GSMTC manager, player, filesystem, GUI or command.
//! Fixtures are independently generated solid-color PNGs, not imported artwork.

use super::{
    artwork::MAX_ENCODED_BYTES,
    sdk::{Apartment, decode_reference},
};
use crate::image_decode::{ImageDecodeLimits, decode_bytes};
use tessera_system::media::{MediaArtwork, MediaError};
use windows::Storage::Streams::{
    DataWriter, IRandomAccessStreamReference, InMemoryRandomAccessStream,
    RandomAccessStreamReference,
};

struct FixtureStream(InMemoryRandomAccessStream);
impl Drop for FixtureStream {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}

struct FixtureWriter(DataWriter);
impl Drop for FixtureWriter {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}

fn decode_fixture(bytes: &[u8], advertised_size: Option<u64>) -> Result<MediaArtwork, MediaError> {
    with_fixture_reference(bytes, advertised_size, decode_reference)
}

fn with_fixture_reference<T>(
    bytes: &[u8],
    advertised_size: Option<u64>,
    decode: impl FnOnce(&IRandomAccessStreamReference) -> Result<T, MediaError>,
) -> Result<T, MediaError> {
    let _apartment = Apartment::new()?;
    let memory = FixtureStream(InMemoryRandomAccessStream::new().unwrap());
    let writer = FixtureWriter(DataWriter::new().unwrap());
    writer.0.WriteBytes(bytes).unwrap();
    let buffer = writer.0.DetachBuffer().unwrap();
    memory.0.WriteAsync(&buffer).unwrap().join().unwrap();
    if let Some(size) = advertised_size {
        memory.0.SetSize(size).unwrap();
    }
    memory.0.Seek(0).unwrap();
    let reference = RandomAccessStreamReference::CreateFromStream(&memory.0).unwrap();
    decode(&reference.into())
}

#[test]
fn sdk_png_decodes_explicit_rgba8_and_premultiplies_alpha() {
    let png = solid_png(1, 1, [255, 0, 0, 128]);
    let artwork = decode_fixture(&png, None).unwrap();
    assert_eq!((artwork.width(), artwork.height()), (1, 1));
    // BGRA would produce a blue channel here; straight alpha would yield 255.
    assert_eq!(artwork.rgba(), &[128, 0, 0, 128]);
}

#[test]
fn sdk_png_scales_to_the_same_bounded_aspect_plan_used_by_production() {
    let png = solid_png(256, 2, [255, 0, 0, 128]);
    let artwork = decode_fixture(&png, None).unwrap();
    assert_eq!((artwork.width(), artwork.height()), (128, 1));
    assert_eq!(artwork.rgba().len(), 128 * 4);
    assert!(
        artwork
            .rgba()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [128, 0, 0, 128])
    );
}

#[test]
fn sdk_decode_rejects_malformed_empty_and_oversized_encoded_streams() {
    let malformed = decode_fixture(b"not an encoded image", None).unwrap_err();
    assert!(malformed.hresult.is_some());
    let empty = decode_fixture(&[], None).unwrap_err();
    assert!(empty.message.contains("empty"));
    let oversized = decode_fixture(&[], Some(MAX_ENCODED_BYTES + 1)).unwrap_err();
    assert!(oversized.message.contains("8 MiB"));
}

#[test]
fn sdk_shared_bytes_and_reference_use_the_same_profile512_premultiplied_decode() {
    let png = solid_png(1024, 2, [255, 0, 0, 128]);
    let from_reference = with_fixture_reference(&png, None, |reference| {
        crate::image_decode::decode_reference(reference, ImageDecodeLimits::profile_photo())
            .map_err(super::artwork::error)
    })
    .unwrap();
    let _apartment = Apartment::new().unwrap();
    let from_bytes = decode_bytes(&png, ImageDecodeLimits::profile_photo()).unwrap();
    assert_eq!(from_bytes, from_reference);
    assert_eq!((from_bytes.width(), from_bytes.height()), (512, 1));
    assert_eq!(from_bytes.rgba().len(), 512 * 4);
    assert!(
        from_bytes
            .rgba()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [128, 0, 0, 128])
    );
    let media = decode_bytes(&png, ImageDecodeLimits::media_thumbnail()).unwrap();
    assert_eq!((media.width(), media.height()), (128, 1));
    assert_eq!(media.rgba().len(), 128 * 4);
    assert!(
        media
            .rgba()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [128, 0, 0, 128])
    );
}

#[test]
fn sdk_shared_bytes_reject_empty_oversized_and_malformed_without_native_error_text() {
    let _apartment = Apartment::new().unwrap();
    for limits in [
        ImageDecodeLimits::media_thumbnail(),
        ImageDecodeLimits::profile_photo(),
    ] {
        let empty = decode_bytes(&[], limits).unwrap_err();
        assert!(empty.message().contains("empty"));
        assert_eq!(empty.hresult(), None);
        let oversized = decode_bytes(&vec![0; MAX_ENCODED_BYTES as usize + 1], limits).unwrap_err();
        assert!(oversized.message().contains("8 MiB"));
        assert_eq!(oversized.hresult(), None);
        let malformed = decode_bytes(b"not an encoded image", limits).unwrap_err();
        assert!(malformed.hresult().is_some());
        assert_eq!(malformed.message(), "Windows image decoding failed");
    }
}

#[test]
fn sdk_both_policies_admit_dimensions_before_requesting_huge_pixel_data() {
    let _apartment = Apartment::new().unwrap();
    for (width, height) in [(16_385_u32, 1_u32), (8192, 8192)] {
        let mut header = Vec::new();
        header.extend_from_slice(&width.to_be_bytes());
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(&mut png, b"IHDR", &header);
        // A valid zlib stream containing one RGBA pixel, not huge raster data.
        chunk(
            &mut png,
            b"IDAT",
            &[
                0x78, 0x01, 0x01, 0x05, 0x00, 0xFA, 0xFF, 0, 255, 0, 0, 128, 0x04, 0x81, 0x01, 0x80,
            ],
        );
        chunk(&mut png, b"IEND", &[]);
        for limits in [
            ImageDecodeLimits::media_thumbnail(),
            ImageDecodeLimits::profile_photo(),
        ] {
            let error = decode_bytes(&png, limits).unwrap_err();
            assert_eq!(error.hresult(), None);
            assert!(error.message().contains("source dimensions"));
        }
    }
}

// Minimal fixture writer, deliberately not a production image codec. One
// uncompressed DEFLATE block keeps the generated bytes auditable and requires
// no image/base64/compression dependency or copied external asset.
fn solid_png(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
    let mut filtered = Vec::new();
    for _ in 0..height {
        filtered.extend([0]); // PNG filter: None.
        for _ in 0..width {
            filtered.extend_from_slice(&pixel);
        }
    }
    let length = u16::try_from(filtered.len()).expect("fixture fits one stored block");
    let mut zlib = vec![0x78, 0x01, 0x01]; // zlib + final stored DEFLATE block.
    zlib.extend_from_slice(&length.to_le_bytes());
    zlib.extend_from_slice(&(!length).to_le_bytes());
    zlib.extend_from_slice(&filtered);
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in &filtered {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]); // RGBA8, standard PNG methods.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut crc = u32::MAX;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 0 { 0 } else { 0xEDB8_8320 };
        }
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
}
