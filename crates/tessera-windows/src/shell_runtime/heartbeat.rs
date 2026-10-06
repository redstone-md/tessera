// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Auto-reset event protocol: creator waits, UI opens signal-only access.
#![allow(unsafe_code)]

use crate::shell_runtime::error::ShellRuntimeError;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{
    GetLastError, HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};
use windows_sys::Win32::System::Threading::{
    CreateEventExW, EVENT_MODIFY_STATE, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
    WaitForMultipleObjects,
};

pub const EVENT_NAME_PREFIX: &str = "Local\\Tessera.Shell.";

#[derive(Debug)]
pub struct ShellHeartbeat {
    handle: OwnedHandle,
}

impl ShellHeartbeat {
    /// Opens only a supervisor-shaped name, never creates an object or inherits it.
    pub fn connect(name: &str) -> Result<Self, ShellRuntimeError> {
        let valid = name
            .strip_prefix(EVENT_NAME_PREFIX)
            .and_then(|suffix| suffix.split_once('.'))
            .is_some_and(|(pid, random)| {
                pid.parse::<u32>()
                    .is_ok_and(|number| number != 0 && number.to_string() == pid)
                    && random.len() == 32
                    && random
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            });
        if !valid {
            return Err(ShellRuntimeError::HeartbeatViolation {
                reason: "invalid supervisor event name",
            });
        }
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: validated NUL-free name and live buffer; default DACL, no inheritance.
        let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, wide.as_ptr()) };
        if handle.is_null() {
            return Err(windows_error("OpenEventW"));
        }
        // SAFETY: OpenEventW transferred one owned handle.
        Ok(Self {
            handle: unsafe { OwnedHandle::from_raw_handle(handle) },
        })
    }

    /// One pending pulse is retained; repeated pulses coalesce until a wait consumes it.
    pub fn pulse(&self) -> Result<(), ShellRuntimeError> {
        // SAFETY: this handle is valid for self's lifetime and grants signal access.
        if unsafe { SetEvent(self.handle.as_raw_handle()) } == 0 {
            Err(windows_error("SetEvent"))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug)]
pub(crate) struct SupervisorEvent {
    name: String,
    handle: OwnedHandle,
}

impl SupervisorEvent {
    pub(crate) fn create(pid: u32) -> Result<Self, ShellRuntimeError> {
        let mut random = [0u8; 16];
        // SAFETY: valid writable array; system RNG requires no algorithm handle.
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                random.as_mut_ptr(),
                random.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return Err(ShellRuntimeError::Windows {
                operation: "BCryptGenRandom",
                code: status as u32,
            });
        }
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let name = format!("{EVENT_NAME_PREFIX}{pid}.{suffix}");
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: no flags means auto-reset/initially clear. Creator needs SYNCHRONIZE
        // as well as signal access; signal-only handles cannot be waited on.
        let handle = unsafe {
            CreateEventExW(
                std::ptr::null(),
                wide.as_ptr(),
                0,
                EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
            )
        };
        if handle.is_null() {
            return Err(windows_error("CreateEventExW"));
        }
        // SAFETY: CreateEventExW transferred one owned handle.
        Ok(Self {
            name,
            handle: unsafe { OwnedHandle::from_raw_handle(handle) },
        })
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn raw_handle(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }
}

fn windows_error(operation: &'static str) -> ShellRuntimeError {
    // SAFETY: thread-local error query has no preconditions.
    ShellRuntimeError::Windows {
        operation,
        code: unsafe { GetLastError() },
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WaitOutcome {
    Signalled(usize),
    TimedOut,
}

pub(crate) fn wait_any(
    handles: &[HANDLE],
    timeout_ms: u32,
) -> Result<WaitOutcome, ShellRuntimeError> {
    debug_assert!(!handles.is_empty() && handles.len() <= 64);
    // SAFETY: callers pass only live owned child/event handles, never duplicates.
    let result =
        unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout_ms) };
    let index = result.wrapping_sub(WAIT_OBJECT_0);
    if index < handles.len() as u32 {
        Ok(WaitOutcome::Signalled(index as usize))
    } else if result == WAIT_TIMEOUT {
        Ok(WaitOutcome::TimedOut)
    } else if result == WAIT_FAILED {
        Err(windows_error("WaitForMultipleObjects"))
    } else {
        Err(ShellRuntimeError::Windows {
            operation: "unexpected wait result",
            code: result,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creator_can_wait_and_signal_only_peer_delivers_fresh_pulses() {
        let event = SupervisorEvent::create(std::process::id()).unwrap();
        let peer = ShellHeartbeat::connect(event.name()).unwrap();
        assert_eq!(
            wait_any(&[event.raw_handle()], 0).unwrap(),
            WaitOutcome::TimedOut
        );
        for _ in 0..2 {
            peer.pulse().unwrap();
            assert_eq!(
                wait_any(&[event.raw_handle()], 1000).unwrap(),
                WaitOutcome::Signalled(0)
            );
            assert_eq!(
                wait_any(&[event.raw_handle()], 0).unwrap(),
                WaitOutcome::TimedOut
            );
        }
    }
    #[test]
    fn malformed_names_are_refused_before_open() {
        for name in [
            "Local\\Other.1.abcdef",
            "Local\\Tessera.Shell.0.00000000000000000000000000000000",
            "Local\\Tessera.Shell.1.abc",
            "Local\\Tessera.Shell.1.0000000000000000000000000000000\0",
        ] {
            assert!(matches!(
                ShellHeartbeat::connect(name),
                Err(ShellRuntimeError::HeartbeatViolation { .. })
            ));
        }
    }
}
