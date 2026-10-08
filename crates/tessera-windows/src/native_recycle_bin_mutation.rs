// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Fixed confirmed aggregate Empty, with a request-local strict STA and private
//! top-level popup owner. No GUI HWND, message-only window or Rust WndProc data.
//! Dialog visibility/activation and cancellation mappings require Windows runtime
//! qualification. Native calls and cleanup can stall; cleanup failures are not
//! proof of native release. None of these limits changes the recorded HRESULT.

use std::marker::PhantomData;
use std::rc::Rc;

use tessera_system::dock_utilities::DockUtilityError;
use tessera_system::recycle_bin_mutation::RecycleBinEmptyOutcome;

use crate::recycle_bin_mutation::worker::{Driver, native_error};

const STA_MODEL: i32 = 2;
const EMPTY_FLAGS: u32 = 4; // SHERB_NOSOUND only; retain confirmation and progress.
const OWNER_STYLE: u32 = 0x8000_0000; // WS_POPUP, not CHILD/VISIBLE/message-only.
const CLOSE_MESSAGE: u32 = 0x0010;
const SYSTEM_COMMAND_MESSAGE: u32 = 0x0112;
const CLOSE_COMMAND: usize = 0xF060;

/// Shared by the native WndProc and pure recording tests. Other messages default.
fn suppress_owner_close(message: u32, wparam: usize) -> bool {
    message == CLOSE_MESSAGE
        || (message == SYSTEM_COMMAND_MESSAGE && wparam & 0xFFF0 == CLOSE_COMMAND)
}

/// Only platform calls vary; owner/class authority never escapes this module.
trait Calls {
    type Class;
    type Owner;

    fn initialize(&self, model: i32) -> i32;
    fn uninitialize(&self);
    fn register_class(&self) -> Result<Self::Class, DockUtilityError>;
    fn unregister_class(&self, class: &Self::Class);
    fn create_owner(
        &self,
        class: &Self::Class,
        style: u32,
    ) -> Result<Self::Owner, DockUtilityError>;
    fn destroy_owner(&self, owner: &Self::Owner);
    fn empty(&self, owner: &Self::Owner, root: *const u16, flags: u32) -> i32;
}

struct Apartment<'a, C: Calls> {
    calls: &'a C,
    _thread_bound: PhantomData<Rc<()>>,
}

impl<'a, C: Calls> Apartment<'a, C> {
    fn enter(calls: &'a C) -> Result<Self, DockUtilityError> {
        let status = calls.initialize(STA_MODEL);
        if status < 0 {
            return Err(native_error(
                status as u32,
                "Recycle Bin mutation COM initialization",
            ));
        }
        Ok(Self {
            calls,
            _thread_bound: PhantomData,
        })
    }
}

impl<C: Calls> Drop for Apartment<'_, C> {
    fn drop(&mut self) {
        self.calls.uninitialize();
    }
}

struct Class<'a, C: Calls> {
    calls: &'a C,
    value: C::Class,
}

impl<C: Calls> Drop for Class<'_, C> {
    fn drop(&mut self) {
        self.calls.unregister_class(&self.value);
    }
}

struct Owner<'a, C: Calls> {
    calls: &'a C,
    value: C::Owner,
}

impl<C: Calls> Drop for Owner<'_, C> {
    fn drop(&mut self) {
        self.calls.destroy_owner(&self.value);
    }
}

struct RecycleBinMutationDriver<C> {
    calls: C,
    _thread_bound: PhantomData<Rc<()>>,
}

impl<C: Calls + 'static> Driver for RecycleBinMutationDriver<C> {
    fn empty(&mut self) -> Result<RecycleBinEmptyOutcome, DockUtilityError> {
        let _apartment = Apartment::enter(&self.calls)?;
        let class = Class {
            calls: &self.calls,
            value: self.calls.register_class()?,
        };
        let owner = Owner {
            calls: &self.calls,
            value: self.calls.create_owner(&class.value, OWNER_STYLE)?,
        };
        let native_hresult = self
            .calls
            .empty(&owner.value, std::ptr::null(), EMPTY_FLAGS);
        Ok(RecycleBinEmptyOutcome { native_hresult })
        // Exactly one call; reverse cleanup: owner, class, balanced STA. No query,
        // retry, status collapsing or inference of consent/cancel/current count.
    }
}

#[cfg(windows)]
pub(crate) struct NativeRecycleBinMutationDriver(RecycleBinMutationDriver<sdk::WindowsCalls>);

#[cfg(windows)]
impl NativeRecycleBinMutationDriver {
    pub(crate) fn new() -> Self {
        Self(RecycleBinMutationDriver {
            calls: sdk::WindowsCalls,
            _thread_bound: PhantomData,
        })
    }
}

#[cfg(windows)]
impl Driver for NativeRecycleBinMutationDriver {
    fn empty(&mut self) -> Result<RecycleBinEmptyOutcome, DockUtilityError> {
        self.0.empty()
    }
}

#[cfg(windows)]
#[path = "native_recycle_bin_mutation/sdk.rs"]
mod sdk;

#[cfg(test)]
#[path = "native_recycle_bin_mutation/tests.rs"]
mod tests;
