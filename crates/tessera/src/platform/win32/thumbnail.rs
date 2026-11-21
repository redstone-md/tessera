// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Live pictures of other windows, drawn by Windows inside a surface.

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Dwm::{
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_VISIBLE,
    DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
    DwmUpdateThumbnailProperties,
};

use super::{AppWindowId, NativeSurface, ScreenRect};

/// A picture of a window that Windows keeps up to date and draws over the
/// content of a surface, until dropped.
#[derive(Debug)]
pub struct Thumbnail(isize);

impl Thumbnail {
    /// Starts picturing `window` on `surface`. Nothing is drawn before
    /// [`Self::place`] says where.
    pub fn of(window: AppWindowId, surface: NativeSurface) -> Result<Self> {
        // SAFETY: the call tolerates handles whose windows are gone, and
        // fails for them.
        let handle = unsafe { DwmRegisterThumbnail(surface.hwnd, window.hwnd()) }
            .context("Windows gave no picture of the window")?;
        Ok(Self(handle))
    }

    /// The width and height of the pictured window, in physical pixels.
    pub fn source_size(&self) -> Option<(i32, i32)> {
        // SAFETY: the handle is a registered thumbnail.
        let size = unsafe { DwmQueryThumbnailSourceSize(self.0) }.ok()?;
        (size.cx > 0 && size.cy > 0).then_some((size.cx, size.cy))
    }

    /// Draws the picture into `area`, in physical pixels from the top left
    /// corner of the surface, as opaque as `opacity`, from 0 to 1.
    pub fn place(&self, area: ScreenRect, opacity: f32) {
        let properties = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_OPACITY | DWM_TNP_VISIBLE,
            rcDestination: RECT {
                left: area.left,
                top: area.top,
                right: area.right,
                bottom: area.bottom,
            },
            opacity: (opacity.clamp(0., 1.) * 255.).round() as u8,
            fVisible: true.into(),
            ..Default::default()
        };
        // SAFETY: `properties` outlives the call and the handle is a
        // registered thumbnail. A failure leaves the picture where it was.
        unsafe {
            let _ = DwmUpdateThumbnailProperties(self.0, &raw const properties);
        }
    }
}

impl Drop for Thumbnail {
    fn drop(&mut self) {
        // SAFETY: the handle was registered by `of` and is not used again.
        unsafe {
            let _ = DwmUnregisterThumbnail(self.0);
        }
    }
}
