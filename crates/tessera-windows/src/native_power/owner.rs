// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Adapters replace raw calls and opaque SDK resources, not acquisition,
//! retirement, error precedence or replay policy.

use std::marker::PhantomData;
use std::ptr;
use std::rc::Rc;

use super::{
    Outcome, PRIVILEGE_ENABLED, PowerAction, PowerError, PowerRequestAccepted, PowerUpdatePolicy,
    SECURITY_IMPERSONATION, SHUTDOWN_INSTALL_UPDATES, SHUTDOWN_POWEROFF, SHUTDOWN_RESTART,
    SHUTDOWN_TOKEN_ACCESS, UPDATE_REASON, native_error, needs_privilege,
};

pub(super) trait Calls {
    type Thread;
    type Token;
    type Privilege;

    fn lock_work_station(&mut self) -> i32;
    fn exit_windows_ex(&mut self, flags: u32, reason: u32) -> i32;
    fn impersonate_self(&mut self, level: i32) -> i32;
    fn current_thread(&mut self) -> Self::Thread;
    fn open_thread_token(
        &mut self,
        thread: &Self::Thread,
        access: u32,
        open_as_self: i32,
    ) -> (i32, Option<Self::Token>);
    /// Fixed SE_SHUTDOWN_NAME, not an arbitrary caller-selected privilege.
    fn lookup_shutdown_privilege(&mut self, system: *const u16) -> (i32, Self::Privilege);
    fn adjust_token_privileges(
        &mut self,
        token: &Self::Token,
        privilege: &Self::Privilege,
        disable_all: i32,
        count: u32,
        attributes: u32,
    ) -> i32;
    fn initiate_shutdown(
        &mut self,
        machine: *const u16,
        message: *const u16,
        grace: u32,
        flags: u32,
        reason: u32,
    ) -> u32;
    fn set_suspend_state(&mut self, hibernate: bool, force: bool, disable_wake: bool) -> bool;
    fn revert_to_self(&mut self) -> i32;
    /// Real opened token only; pseudo-thread handles never enter this seam.
    fn close_token(&mut self, token: &Self::Token) -> i32;
    fn last_error(&mut self) -> u32;
}

pub(super) struct Owner<C: Calls> {
    calls: C,
    revert_pending: bool,
    token: Option<C::Token>,
    _thread_bound: PhantomData<Rc<()>>,
}

impl<C: Calls> Owner<C> {
    pub(super) fn new(calls: C) -> Self {
        Self {
            calls,
            revert_pending: false,
            token: None,
            _thread_bound: PhantomData,
        }
    }

    pub(super) fn perform(mut self, action: PowerAction) -> Outcome {
        let result = self.request(action);
        let cleanup = self.retire();
        // Preserve an earlier action/acquisition error. Once native initiation
        // succeeds, report the FIRST checked cleanup failure, never fake success.
        result.and(cleanup.map(|()| PowerRequestAccepted))
    }

    fn check_bool(&mut self, status: i32) -> Result<(), PowerError> {
        if status == 0 {
            // No native API, resource drop or callback between call and capture.
            Err(native_error(self.calls.last_error()))
        } else {
            Ok(())
        }
    }

    fn enable_shutdown_privilege(&mut self) -> Result<(), PowerError> {
        let status = self.calls.impersonate_self(SECURITY_IMPERSONATION);
        self.check_bool(status)?;
        self.revert_pending = true;

        let thread = self.calls.current_thread();
        let (status, token) = self
            .calls
            .open_thread_token(&thread, SHUTDOWN_TOKEN_ACCESS, 1);
        self.check_bool(status)?;
        self.token = Some(token.ok_or(PowerError::Unavailable)?);

        let (status, privilege) = self.calls.lookup_shutdown_privilege(ptr::null());
        self.check_bool(status)?;
        let status = self.calls.adjust_token_privileges(
            self.token.as_ref().ok_or(PowerError::Unavailable)?,
            &privilege,
            0,
            1,
            PRIVILEGE_ENABLED,
        );
        // AdjustTokenPrivileges can return nonzero while assigning NO privilege.
        // Capture immediately on BOTH BOOL outcomes, not only on false. Require
        // ERROR_SUCCESS; preserve 1300/1314 (and all other full DWORDs) unchanged.
        let code = self.calls.last_error();
        if status == 0 || code != 0 {
            Err(native_error(code))
        } else {
            Ok(())
        }
    }

    fn request(&mut self, action: PowerAction) -> Outcome {
        if needs_privilege(action) {
            self.enable_shutdown_privilege()?;
        }
        match action {
            PowerAction::LockSession => {
                let status = self.calls.lock_work_station();
                self.check_bool(status)?;
            }
            PowerAction::LogOut => {
                let status = self.calls.exit_windows_ex(0, 0);
                self.check_bool(status)?;
            }
            PowerAction::PowerOff { updates } | PowerAction::Reboot { updates } => {
                let mut flags = if matches!(action, PowerAction::PowerOff { .. }) {
                    SHUTDOWN_POWEROFF
                } else {
                    SHUTDOWN_RESTART
                };
                // Exact source reason 0 is intentional for omitted installation,
                // despite the SDK recommendation against unplanned reason 0.
                // No force, grace override, hybrid or restart-apps flags added.
                let reason = if updates == PowerUpdatePolicy::RequestInstallation {
                    flags |= SHUTDOWN_INSTALL_UPDATES;
                    UPDATE_REASON
                } else {
                    0
                };
                let code = self
                    .calls
                    .initiate_shutdown(ptr::null(), ptr::null(), 0, flags, reason);
                // This API returns the authoritative DWORD, NOT a BOOL. Never
                // replace it with a stale thread-local GetLastError value.
                if code != 0 {
                    return Err(native_error(code));
                }
            }
            PowerAction::Suspend | PowerAction::Hibernate => {
                let status =
                    self.calls
                        .set_suspend_state(action == PowerAction::Hibernate, false, false);
                if !status {
                    return Err(native_error(self.calls.last_error()));
                }
            }
        }
        Ok(PowerRequestAccepted)
    }

    fn retire(&mut self) -> Result<(), PowerError> {
        let mut error = None;
        if self.revert_pending {
            let status = self.calls.revert_to_self();
            if status == 0 {
                error = Some(native_error(self.calls.last_error()));
                // A successful best-effort second revert must not erase the
                // first CHECKED failure. Real child termination is still joined.
                let _ = self.calls.revert_to_self();
            }
            // This consumes the attempt, NOT proof that failed reversion made
            // the child neutral. Only actual joined termination proves that.
            self.revert_pending = false;
        }
        if let Some(token) = self.token.take() {
            // Take BEFORE closing: exactly one close attempt, including failure
            // or unwinding. No blind retry; failed close may leak until exit.
            let status = self.calls.close_token(&token);
            if status == 0 {
                let failure = native_error(self.calls.last_error());
                error.get_or_insert(failure);
            }
        }
        error.map_or(Ok(()), Err)
    }
}

impl<C: Calls> Drop for Owner<C> {
    fn drop(&mut self) {
        // Partial acquisition/unwind uses the same owner, not parallel cleanup.
        // Already attempted retirement consumed state and is a no-op here.
        let _ = self.retire();
    }
}
