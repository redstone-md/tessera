// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::marker::PhantomData;
use std::ptr;
use std::rc::Rc;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, LUID};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, ImpersonateSelf, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
    RevertToSelf, SE_SHUTDOWN_NAME, TOKEN_PRIVILEGES,
};
use windows_sys::Win32::System::Power::SetSuspendState;
use windows_sys::Win32::System::Shutdown::{ExitWindowsEx, InitiateShutdownW, LockWorkStation};
use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

use super::Calls;

/// Neither token nor pseudo-thread adapter implements Drop: the ONE Owner
/// protocol controls checked retirement and never closes a pseudo handle.
pub(super) struct Thread(HANDLE);
pub(super) struct Token(HANDLE);
pub(super) struct WindowsCalls(PhantomData<Rc<()>>);

impl WindowsCalls {
    pub(super) fn new() -> Self {
        Self(PhantomData)
    }
}

impl Calls for WindowsCalls {
    type Thread = Thread;
    type Token = Token;
    type Privilege = LUID;

    fn lock_work_station(&mut self) -> i32 {
        // SAFETY: argument-free explicit request, initiation-only raw BOOL.
        unsafe { LockWorkStation() }
    }

    fn exit_windows_ex(&mut self, flags: u32, reason: u32) -> i32 {
        // SAFETY: Owner supplies only EWX_LOGOFF=0 and source reason=0.
        unsafe { ExitWindowsEx(flags, reason) }
    }

    fn impersonate_self(&mut self, level: i32) -> i32 {
        // SAFETY: SecurityImpersonation affects only this private child thread,
        // never the process token. Owner retains and retires the acquired scope.
        unsafe { ImpersonateSelf(level) }
    }

    fn current_thread(&mut self) -> Thread {
        // SAFETY: pseudo-handle for THIS child, valid only for immediate open.
        // It has no native close/drop; only Token can enter close_token.
        Thread(unsafe { GetCurrentThread() })
    }

    fn open_thread_token(
        &mut self,
        thread: &Thread,
        access: u32,
        open_as_self: i32,
    ) -> (i32, Option<Token>) {
        let mut handle = ptr::null_mut();
        // SAFETY: current pseudo-thread handle; initialized writable output;
        // fixed adjust/query rights and TRUE OpenAsSelf. No process-token open.
        let status = unsafe { OpenThreadToken(thread.0, access, open_as_self, &mut handle) };
        (
            status,
            if status != 0 && !handle.is_null() {
                Some(Token(handle))
            } else {
                None
            },
        )
    }

    fn lookup_shutdown_privilege(&mut self, system: *const u16) -> (i32, LUID) {
        let mut privilege = LUID::default();
        // SAFETY: Owner supplies NULL local system; SDK constant is a terminated
        // immutable UTF-16 SE_SHUTDOWN_NAME. Output points to initialized LUID.
        let status = unsafe { LookupPrivilegeValueW(system, SE_SHUTDOWN_NAME, &mut privilege) };
        (status, privilege)
    }

    fn adjust_token_privileges(
        &mut self,
        token: &Token,
        privilege: &LUID,
        disable_all: i32,
        count: u32,
        attributes: u32,
    ) -> i32 {
        let state = TOKEN_PRIVILEGES {
            PrivilegeCount: count,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: *privilege,
                Attributes: attributes,
            }],
        };
        // SAFETY: a real child token with adjust/query rights, one stack-stored
        // LUID with enabled attribute, FALSE DisableAllPrivileges. No previous
        // state requested: impersonation token retires rather than becoming
        // process-global state. Return RAW BOOL; Owner captures GetLastError on
        // BOTH outcomes immediately (nonzero + ERROR_NOT_ALL_ASSIGNED can fail).
        unsafe {
            AdjustTokenPrivileges(
                token.0,
                disable_all,
                &state,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        }
    }

    fn initiate_shutdown(
        &mut self,
        machine: *const u16,
        message: *const u16,
        grace: u32,
        flags: u32,
        reason: u32,
    ) -> u32 {
        // SAFETY: Owner supplies NULL local machine/message, zero grace and the
        // closed exact no-force source flags/reason. DWORD itself is the result.
        unsafe { InitiateShutdownW(machine, message, grace, flags, reason) }
    }

    fn set_suspend_state(&mut self, hibernate: bool, force: bool, disable_wake: bool) -> bool {
        // SAFETY: shutdown privilege was enabled on this child's thread token;
        // fixed false force/wake-disable. Installed BOOLEAN projection is bool,
        // NOT a four-byte BOOL or error DWORD. This call may block until resume.
        unsafe { SetSuspendState(hibernate, force, disable_wake) }
    }

    fn revert_to_self(&mut self) -> i32 {
        // SAFETY: only this child's self-impersonation scope is retired.
        unsafe { RevertToSelf() }
    }

    fn close_token(&mut self, token: &Token) -> i32 {
        // SAFETY: only the real successfully opened token, consumed by Owner
        // before exactly one close attempt. Never a pseudo-thread handle/retry.
        unsafe { CloseHandle(token.0) }
    }

    fn last_error(&mut self) -> u32 {
        // SAFETY: immediately reads this native owner's thread-local error;
        // Owner never substitutes it for an authoritative shutdown DWORD.
        unsafe { GetLastError() }
    }
}
