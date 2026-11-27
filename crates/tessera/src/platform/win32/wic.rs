// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pictures read by the Windows Imaging Component, for the formats Tessera
//! does not read itself: HEIC, AVIF and JPEG XR, and whatever else a codec
//! installed in Windows reads (the HEIF and AV1 extensions of the Store
//! bring the first two).

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat24bppRGB, IWICImagingFactory, IWICPalette,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::HSTRING;

/// The most pixels a picture read this way may have: 64 megapixels, a
/// little under 200 MB as it is read.
const MOST_PIXELS: u64 = 64 * 1024 * 1024;

/// The first frame of the picture in the file `path`, as its width, its
/// height and its pixels, rows from the top and each pixel red, green and
/// blue. Fails where no codec of Windows reads the file. It takes a while
/// for a large picture: not for the thread of the interface.
pub fn decode_with_wic(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    // SAFETY: the thread is given to COM and taken back in a pair, after
    // everything made on it is let go of; the objects stay on this thread.
    let entered = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let decoded = decode(path);
    if entered {
        // SAFETY: as above.
        unsafe { CoUninitialize() };
    }
    decoded.with_context(|| format!("Windows does not read {}", path.display()))
}

fn decode(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    // SAFETY: the objects are made and used on this thread with arguments
    // that outlive the calls; the buffer is of the length given.
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .context("Windows has no imaging component")?;
        let decoder = factory.CreateDecoderFromFilename(
            &HSTRING::from(path.as_os_str()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )?;
        let frame = decoder.GetFrame(0)?;
        let converter = factory.CreateFormatConverter()?;
        converter.Initialize(
            &frame,
            &GUID_WICPixelFormat24bppRGB,
            WICBitmapDitherTypeNone,
            None::<&IWICPalette>,
            0.,
            WICBitmapPaletteTypeCustom,
        )?;
        let (mut width, mut height) = (0, 0);
        converter.GetSize(&raw mut width, &raw mut height)?;
        if width == 0 || height == 0 {
            bail!("the picture is empty");
        }
        if u64::from(width) * u64::from(height) > MOST_PIXELS {
            bail!("the picture has more than {MOST_PIXELS} pixels");
        }
        let stride = width * 3;
        let mut pixels = vec![0; stride as usize * height as usize];
        converter.CopyPixels(std::ptr::null(), stride, &mut pixels)?;
        Ok((width, height, pixels))
    }
}
