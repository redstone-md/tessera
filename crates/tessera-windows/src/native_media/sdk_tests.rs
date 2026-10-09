// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Actual SDK decode/timeline projection tests, no GSMTC manager or player.
//! Fixtures are independently generated solid-color PNGs, not imported artwork.

use super::{
    artwork::MAX_ENCODED_BYTES,
    sdk::{Apartment, decode_reference, timeline_facts},
};
use crate::image_decode::{ImageDecodeLimits, decode_bytes};
use tessera_system::media::{MediaArtwork, MediaError, MediaErrorKind, MediaTimeline};
use windows::{
    Foundation::{DateTime, TimeSpan},
    Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionPlaybackControls as Controls,
        GlobalSystemMediaTransportControlsSessionTimelineProperties as TimelineProperties,
        IGlobalSystemMediaTransportControlsSession_Vtbl as SessionVtable,
    },
    Storage::Streams::{
        DataWriter, IRandomAccessStreamReference, InMemoryRandomAccessStream,
        RandomAccessStreamReference,
    },
    core::{Error, HRESULT},
};

#[test]
fn sdk_timeline_method_shapes_match_the_pinned_sdk_without_invoking_a_player() {
    let _: fn(&Session) -> windows::core::Result<TimelineProperties> =
        Session::GetTimelineProperties;
    let _: fn(&TimelineProperties) -> windows::core::Result<TimeSpan> =
        TimelineProperties::StartTime;
    let _: fn(&TimelineProperties) -> windows::core::Result<TimeSpan> = TimelineProperties::EndTime;
    let _: fn(&TimelineProperties) -> windows::core::Result<TimeSpan> =
        TimelineProperties::Position;
    let _: fn(&TimelineProperties) -> windows::core::Result<TimeSpan> =
        TimelineProperties::MinSeekTime;
    let _: fn(&TimelineProperties) -> windows::core::Result<TimeSpan> =
        TimelineProperties::MaxSeekTime;
    let _: fn(&TimelineProperties) -> windows::core::Result<DateTime> =
        TimelineProperties::LastUpdatedTime;
    let _: fn(&Session, i64) -> windows::core::Result<()> =
        Session::RemoveTimelinePropertiesChanged;
    // Typecheck registration's exact generated ABI without constructing an
    // interface or calling the registration function.
    let _registration_shape = |vtable: &SessionVtable| {
        let _: unsafe extern "system" fn(
            *mut core::ffi::c_void,
            *mut core::ffi::c_void,
            *mut i64,
        ) -> HRESULT = vtable.TimelinePropertiesChanged;
    };
}

#[test]
fn sdk_seek_method_shapes_use_signed_ticks_and_native_capability_without_a_player() {
    let _: fn(&Session, i64) -> _ = Session::TryChangePlaybackPositionAsync;
    let _: fn(&Controls) -> windows::core::Result<bool> = Controls::IsPlaybackPositionEnabled;
    let _seek_shape = |vtable: &SessionVtable| {
        let _: unsafe extern "system" fn(
            *mut core::ffi::c_void,
            i64,
            *mut *mut core::ffi::c_void,
        ) -> HRESULT = vtable.TryChangePlaybackPositionAsync;
    };
}

#[test]
fn sdk_timeline_projection_preserves_signed_zero_invalid_extreme_and_utc_ticks() {
    for (values, utc) in [
        ([0, 0, 0, 0, 0], 0),
        ([-50, 50, -25, -40, 40], -1),
        ([10, -10, 20, 15, -15], 133_000_000_000_000_000),
        ([i64::MIN, i64::MAX, i64::MIN, i64::MIN, i64::MAX], i64::MAX),
        ([i64::MAX, i64::MIN, i64::MAX, i64::MAX, i64::MIN], i64::MIN),
    ] {
        let actual = timeline_facts(
            values.map(|ticks| Ok(TimeSpan { Duration: ticks })),
            Ok(DateTime { UniversalTime: utc }),
        )
        .unwrap();
        assert_eq!(
            actual,
            MediaTimeline {
                start_ticks: values[0],
                end_ticks: values[1],
                position_ticks: values[2],
                min_seek_ticks: values[3],
                max_seek_ticks: values[4],
                last_updated_utc_ticks: Some(utc),
            }
        );
    }
}

#[test]
fn sdk_each_required_timeline_field_failure_is_not_synthesized_as_zero() {
    let operations = [
        "Read media timeline start",
        "Read media timeline end",
        "Read media timeline position",
        "Read media minimum seek time",
        "Read media maximum seek time",
    ];
    for (failed, operation) in operations.into_iter().enumerate() {
        let mut fields = [0, 90, 30, 0, 90].map(|ticks| Ok(TimeSpan { Duration: ticks }));
        fields[failed] = Err(Error::from_hresult(HRESULT(-2147024891)));
        let failure = timeline_facts(fields, Ok(DateTime { UniversalTime: 123 })).unwrap_err();
        assert_eq!(failure.kind, MediaErrorKind::Unavailable);
        assert_eq!(failure.hresult, Some(-2147024891));
        assert!(failure.message.starts_with(operation));
    }
}

#[test]
fn sdk_optional_utc_failure_does_not_discard_five_usable_timeline_fields() {
    let actual = timeline_facts(
        [-50, 90, -30, -40, 80].map(|ticks| Ok(TimeSpan { Duration: ticks })),
        Err(Error::from_hresult(HRESULT(-2147024891))),
    )
    .unwrap();
    assert_eq!(
        actual,
        MediaTimeline {
            start_ticks: -50,
            end_ticks: 90,
            position_ticks: -30,
            min_seek_ticks: -40,
            max_seek_ticks: 80,
            last_updated_utc_ticks: None,
        }
    );
}

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
