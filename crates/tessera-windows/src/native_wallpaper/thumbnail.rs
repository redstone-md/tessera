// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Actual Shell thumbnail handler only: no generic-icon or path/URI fallback.
//! All COM and owned bitmap resources remain on the wallpaper owner STA.

use tessera_system::wallpaper::{WallpaperError, WallpaperPreview};
use windows::Win32::Graphics::Gdi::HBITMAP;
use windows::Win32::System::Com::IBindCtx;
use windows::Win32::UI::Shell::{
    BHID_ThumbnailHandler, IShellItem, IThumbnailProvider, WTSAT_ARGB, WTSAT_RGB, WTSAT_UNKNOWN,
};
use windows_sys::Win32::Graphics::Gdi::DeleteObject;

use crate::native_bitmap::{BitmapAlpha, copy_bitmap};

const THUMBNAIL_EDGE: u32 = 128;

/// The caller has validated this original item and retains an exact-file lease
/// across the entire operation. Revalidation errors invalidate the whole choose;
/// unsupported or malformed thumbnail metadata merely leaves preview absent.
pub(super) fn capture(
    item: &IShellItem,
    validate_source: impl Fn() -> Result<(), WallpaperError>,
) -> Result<Option<WallpaperPreview>, WallpaperError> {
    // SAFETY: original protected SDK item and initialized owner STA; the handler
    // itself supplies a real thumbnail, never an ICONONLY generic shell image.
    let handler: windows::core::Result<IThumbnailProvider> =
        unsafe { item.BindToHandler(None::<&IBindCtx>, &BHID_ThumbnailHandler) };
    validate_source()?;
    let Ok(handler) = handler else {
        return Ok(None);
    };
    // Install ownership before calling: even an error may have set the output
    // bitmap, which must still be deleted on this same native owner thread.
    let mut bitmap = OwnedBitmap(HBITMAP::default());
    let mut alpha = WTSAT_UNKNOWN;
    let receipt = unsafe { handler.GetThumbnail(THUMBNAIL_EDGE, &mut bitmap.0, &mut alpha) };
    validate_source()?;
    let alpha = match alpha {
        WTSAT_ARGB => Some(BitmapAlpha::Premultiplied),
        WTSAT_RGB => Some(BitmapAlpha::Opaque),
        _ => None,
    };
    let preview = receipt
        .ok()
        .and(alpha)
        // Typed windows HBITMAP -> windows-sys handle; the shared copier bounds
        // actual bitmap dimensions before allocation and owns no bitmap lifetime.
        .and_then(|alpha| copy_bitmap(bitmap.0.0, alpha))
        .and_then(|pixels| WallpaperPreview::new(pixels.width, pixels.height, pixels.rgba).ok());
    validate_source()?;
    Ok(preview)
}

struct OwnedBitmap(HBITMAP);

impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            // SAFETY: GetThumbnail transfers ownership; no handle leaves this
            // STA and the shared copier restores any GDI selections before drop.
            let _ = unsafe { DeleteObject(self.0.0) };
        }
    }
}
