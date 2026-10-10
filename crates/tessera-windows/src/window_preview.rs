// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Read-only DWM relationships, authorized by the existing window target seam.

use std::marker::PhantomData;
use std::rc::Rc;

use tessera_core::Rect;
use windows_sys::Win32::Foundation::{HWND, RECT, SIZE};
use windows_sys::Win32::Graphics::Dwm::{
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION,
    DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE, DwmQueryThumbnailSourceSize,
    DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetClientRect, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
};

use crate::ActivationTarget;

/// UI-thread-owned relationship. Drop before hiding/destroying its destination.
/// No raw source handle is accepted; activation admission is reused unchanged.
pub struct OwnedWindowPreview {
    thumbnail: isize,
    _ui_thread: PhantomData<Rc<()>>,
}

impl OwnedWindowPreview {
    pub fn attach(
        destination: isize,
        target: ActivationTarget,
        bounds: Rect,
    ) -> Result<Self, String> {
        let destination = destination as HWND;
        validate_destination(destination, bounds)?;
        let source =
            crate::native_activation::validate_target(target).map_err(|error| error.to_string())?;
        let mut thumbnail = 0;
        // SAFETY: live admitted top-level source and same-process destination;
        // DWM validates transient HWNDs, and the output has the documented type.
        check("Register window preview", unsafe {
            DwmRegisterThumbnail(destination, source, &mut thumbnail)
        })?;
        if thumbnail == 0 {
            return Err("Windows did not provide a window preview relationship.".into());
        }
        let lease = Self {
            thumbnail,
            _ui_thread: PhantomData,
        };
        let mut size = SIZE::default();
        // SAFETY: successful relationship owned by this lease and writable SIZE.
        check("Read window preview size", unsafe {
            DwmQueryThumbnailSourceSize(lease.thumbnail, &mut size)
        })?;
        if size.cx <= 0 || size.cy <= 0 {
            return Err("The window preview source size is unavailable.".into());
        }
        let destination_rect = contained(bounds, size)?;
        // Revalidate after registration/query, immediately before publishing pixels.
        let current_source =
            crate::native_activation::validate_target(target).map_err(|error| error.to_string())?;
        if current_source != source {
            return Err("The window preview source changed.".into());
        }
        validate_destination(destination, bounds)?;
        // Use the SDK's packed struct directly; never borrow its packed fields.
        let properties = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION
                | DWM_TNP_VISIBLE
                | DWM_TNP_OPACITY
                | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: destination_rect,
            rcSource: RECT::default(),
            opacity: 255,
            fVisible: 1,
            fSourceClientAreaOnly: 0,
        };
        // SAFETY: live owned registration and correctly typed SDK properties.
        check("Display window preview", unsafe {
            DwmUpdateThumbnailProperties(lease.thumbnail, &properties)
        })?;
        Ok(lease)
    }
}

impl Drop for OwnedWindowPreview {
    fn drop(&mut self) {
        // SAFETY: exactly one successful relationship, retained on its UI thread.
        unsafe { DwmUnregisterThumbnail(self.thumbnail) };
    }
}

fn validate_destination(destination: HWND, bounds: Rect) -> Result<(), String> {
    let mut process = 0;
    let mut client = RECT::default();
    // SAFETY: user32 validates transient handles; outputs are correctly sized.
    let valid = unsafe {
        IsWindow(destination) != 0
            && IsWindowVisible(destination) != 0
            && GetAncestor(destination, GA_ROOT) == destination
            && GetWindowThreadProcessId(destination, &mut process) != 0
            && process == GetCurrentProcessId()
            && GetClientRect(destination, &mut client) != 0
    };
    if !valid
        || bounds.x() < client.left
        || bounds.y() < client.top
        || bounds.right() > client.right
        || bounds.bottom() > client.bottom
    {
        return Err("The owned window preview viewport is unavailable.".into());
    }
    Ok(())
}

/// Integer aspect fit, never cropping or painting beyond the checked viewport.
fn contained(bounds: Rect, source: SIZE) -> Result<RECT, String> {
    let (sw, sh) = (source.cx as u64, source.cy as u64);
    let (bw, bh) = (u64::from(bounds.width()), u64::from(bounds.height()));
    let (width, height) = if bw * sh <= bh * sw {
        (bw, bw * sh / sw)
    } else {
        (bh * sw / sh, bh)
    };
    if width == 0 || height == 0 {
        return Err("The window preview does not fit this viewport.".into());
    }
    let left = i64::from(bounds.x()) + ((bw - width) / 2) as i64;
    let top = i64::from(bounds.y()) + ((bh - height) / 2) as i64;
    Ok(RECT {
        left: left as i32,
        top: top as i32,
        right: (left + width as i64) as i32,
        bottom: (top + height as i64) as i32,
    })
}

fn check(operation: &str, result: i32) -> Result<(), String> {
    if result < 0 {
        Err(format!(
            "{operation} is unavailable (0x{:08x}).",
            result as u32
        ))
    } else {
        Ok(())
    }
}
