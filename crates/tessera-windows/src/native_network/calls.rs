// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::ffi::c_void;

use tessera_system::network::{NetworkError, NetworkErrorKind};
use windows::Win32::NetworkManagement::WiFi::{WLAN_INTF_OPCODE, WLAN_NOTIFICATION_CALLBACK};
use windows::core::GUID;

/// Out parameters are owned even if a faulty implementation also reports error.
/// `size` is authoritative for QueryInterface and recording buffers. The WLAN
/// enumeration/available APIs expose no byte length; their allocator's count
/// contract remains trusted, with checked arithmetic and bounded resource usage.
pub(super) struct Reply {
    pub status: u32,
    pub data: *mut c_void,
    pub size: Option<usize>,
}

/// # Safety
/// Returned non-null buffers must be readable, correctly initialized SDK records
/// of the requested kind, remain live until `free`, and be independently owned.
/// A successful NONE registration must quiesce all earlier callbacks. No callback
/// may use its context after that barrier; close alone is not assumed a barrier.
pub(super) unsafe trait NativeCalls: Send + 'static {
    fn open(&self, version: u32) -> (u32, u32, usize);
    fn close(&self, handle: usize) -> u32;
    fn interfaces(&self, handle: usize) -> Reply;
    fn query(&self, handle: usize, id: &GUID, opcode: WLAN_INTF_OPCODE) -> Reply;
    fn available(&self, handle: usize, id: &GUID, flags: u32) -> Reply;
    fn bss(&self, handle: usize, id: &GUID) -> Reply;
    unsafe fn free(&self, data: *mut c_void);
    fn register(
        &self,
        handle: usize,
        source: u32,
        callback: WLAN_NOTIFICATION_CALLBACK,
        context: *const c_void,
    ) -> u32;
    fn open_settings(&self) -> Result<(), NetworkError>;
    /// Retirement failures have no completion during Drop, so record a redacted
    /// diagnostic without any adapter identifiers or cached network data.
    fn retirement_error(&self, error: &NetworkError);
}

pub(super) fn native_error(operation: &str, code: u32) -> NetworkError {
    let kind = match code {
        5 => NetworkErrorKind::AccessDenied,
        50 | 120 => NetworkErrorKind::Unsupported,
        1062 | 1058 | 1722 | 1753 => NetworkErrorKind::ServiceUnavailable,
        6 | 1167 | 1168 => NetworkErrorKind::DeviceChanged,
        170 => NetworkErrorKind::Busy,
        13 | 87 => NetworkErrorKind::InvalidData,
        _ => NetworkErrorKind::Other,
    };
    NetworkError::new(kind, format!("{operation} failed (Windows error {code})"))
}

pub(super) fn invalid(message: &str) -> NetworkError {
    NetworkError::new(NetworkErrorKind::InvalidData, message)
}
