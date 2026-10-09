// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::ffi::c_void;
use std::ptr::NonNull;

use tessera_system::network::NetworkError;

use super::calls::{NativeCalls, Reply, invalid, native_error};

const MAX_BUFFER: usize = 16 * 1024 * 1024;

pub(super) struct Buffer<'a, C: NativeCalls> {
    calls: &'a C,
    data: NonNull<c_void>,
    size: Option<usize>,
}

impl<'a, C: NativeCalls> Buffer<'a, C> {
    pub(super) fn acquire(
        calls: &'a C,
        reply: Reply,
        operation: &str,
    ) -> Result<Self, NetworkError> {
        let buffer = NonNull::new(reply.data).map(|data| Self {
            calls,
            data,
            size: reply.size,
        });
        if reply.status != 0 {
            // RAII also handles an unexpected allocation on an error reply.
            return Err(native_error(operation, reply.status));
        }
        let buffer = buffer.ok_or_else(|| invalid("WLAN returned a null successful allocation"))?;
        if buffer.size.is_some_and(|size| size > MAX_BUFFER) {
            return Err(invalid("WLAN allocation exceeds the bounded cache size"));
        }
        Ok(buffer)
    }

    pub(super) fn span(
        &self,
        offset: usize,
        count: usize,
        stride: usize,
    ) -> Result<(), NetworkError> {
        let end = count
            .checked_mul(stride)
            .and_then(|length| offset.checked_add(length))
            .ok_or_else(|| invalid("WLAN allocation span overflow"))?;
        if end > MAX_BUFFER || self.size.is_some_and(|size| end > size) {
            return Err(invalid("WLAN allocation span is out of bounds"));
        }
        Ok(())
    }

    pub(super) fn limit(&mut self, size: usize) -> Result<(), NetworkError> {
        self.span(0, size, 1)?;
        self.size = Some(size);
        Ok(())
    }

    /// Read only initialized SDK scalar/record types with no invalid Rust bool
    /// representations. Flexible lists never create a reference to element zero.
    pub(super) fn read<T: Copy>(&self, offset: usize) -> Result<T, NetworkError> {
        self.span(offset, 1, size_of::<T>())?;
        // SAFETY: NativeCalls guarantees readable initialized record storage.
        // Known lengths and each computed span were checked; unaligned reads
        // avoid adding a stronger alignment requirement to the seam.
        Ok(unsafe {
            self.data
                .as_ptr()
                .cast::<u8>()
                .add(offset)
                .cast::<T>()
                .read_unaligned()
        })
    }

    pub(super) fn list<T: Copy>(
        &self,
        offset: usize,
        count: u32,
        maximum: usize,
    ) -> Result<Vec<T>, NetworkError> {
        let count = count as usize;
        if count > maximum {
            return Err(invalid("WLAN list count exceeds the bounded cache size"));
        }
        self.span(offset, count, size_of::<T>())?;
        (0..count)
            .map(|index| self.read(offset + index * size_of::<T>()))
            .collect()
    }
}

impl<C: NativeCalls> Drop for Buffer<'_, C> {
    fn drop(&mut self) {
        // SAFETY: this is the only owner of a live WLAN allocation.
        unsafe { self.calls.free(self.data.as_ptr()) };
    }
}
