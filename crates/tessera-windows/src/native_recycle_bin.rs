// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Fixed aggregate read and value-based Shell dispatch, with request-local STA.
//! Shell registration is not cryptographically authenticated. Numeric Open can
//! initialize missing Shell resources; its HRESULT acknowledges dispatch only.
//! Native activation/query/open/release/teardown can stall and are uncancellable.

use std::marker::PhantomData;
use std::rc::Rc;

use tessera_system::dock_utilities::DockUtilityError;
use tessera_system::recycle_bin::RecycleBinInfo;

use crate::recycle_bin::worker::{Driver, native_error};

pub(crate) mod watcher;

const SHELL_CLSID: u128 = 0x13709620_c279_11ce_a49e_444553540000;
#[cfg(all(windows, test))]
const SHELL_INTERFACE_IID: u128 = 0xd8f015c0_c278_11ce_a49e_444553540000;
const SHELL_CONTEXT: u32 = 0x0000_8001; // INPROC_SERVER | DISABLE_AAA
const STA_MODEL: i32 = 2;
const RECYCLE_TARGET: i32 = 10; // ssfBITBUCKET, owned VT_I4, never a UI value

#[derive(Clone, Copy)]
struct SignedInfo {
    item_count: i64,
    size_in_bytes: i64,
}

/// Only native calls vary. No Send bound on Shell, target or this adapter.
trait Calls {
    type Shell;
    type Target;

    fn initialize(&self, model: i32) -> i32;
    fn query(&self) -> Result<SignedInfo, u32>;
    fn create_shell(&self, class: u128, context: u32) -> Result<Self::Shell, u32>;
    fn make_target(&self, value: i32) -> Self::Target;
    fn open(&self, shell: &Self::Shell, target: &Self::Target) -> Result<(), u32>;
    fn uninitialize(&self);
}

struct Apartment<'a, C: Calls> {
    calls: &'a C,
    _thread_bound: PhantomData<Rc<()>>,
}

impl<'a, C: Calls> Apartment<'a, C> {
    fn enter(calls: &'a C) -> Result<Self, DockUtilityError> {
        let code = calls.initialize(STA_MODEL);
        if code < 0 {
            return Err(native_error(code as u32, "Recycle Bin COM initialization"));
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

struct RecycleBinDriver<C: Calls> {
    calls: C,
}

impl<C: Calls + 'static> Driver for RecycleBinDriver<C> {
    fn read(&mut self) -> Result<RecycleBinInfo, DockUtilityError> {
        let _apartment = Apartment::enter(&self.calls)?;
        // Query does not activate Shell dispatch. Failure ignores provider output.
        let signed = self
            .calls
            .query()
            .map_err(|code| native_error(code, "Recycle Bin aggregate query"))?;
        let invalid = || native_error(0x8000_4005, "Recycle Bin invalid aggregate");
        Ok(RecycleBinInfo {
            item_count: u64::try_from(signed.item_count).map_err(|_| invalid())?,
            size_in_bytes: u64::try_from(signed.size_in_bytes).map_err(|_| invalid())?,
        })
    }

    fn open(&mut self) -> Result<(), DockUtilityError> {
        let _apartment = Apartment::enter(&self.calls)?;
        let shell = self
            .calls
            .create_shell(SHELL_CLSID, SHELL_CONTEXT)
            .map_err(|code| native_error(code, "Recycle Bin Shell activation"))?;
        let target = self.calls.make_target(RECYCLE_TARGET);
        self.calls
            .open(&shell, &target)
            .map_err(|code| native_error(code, "Recycle Bin Shell Open"))
        // Reverse drop order: owned target/VariantClear, Shell release, balanced
        // CoUninitialize. The worker completes only after this scope is gone.
    }
}

#[cfg(windows)]
pub(crate) struct NativeRecycleBinDriver(RecycleBinDriver<sdk::WindowsCalls>);

#[cfg(windows)]
impl NativeRecycleBinDriver {
    pub(crate) fn new() -> Self {
        Self(RecycleBinDriver {
            calls: sdk::WindowsCalls,
        })
    }
}

#[cfg(windows)]
impl Driver for NativeRecycleBinDriver {
    fn read(&mut self) -> Result<RecycleBinInfo, DockUtilityError> {
        self.0.read()
    }

    fn open(&mut self) -> Result<(), DockUtilityError> {
        self.0.open()
    }
}

#[cfg(windows)]
mod sdk {
    use windows::Win32::System::Com::{
        CLSCTX, COINIT, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Shell::{
        IShellDispatch, SHQUERYRBINFO, SHQueryRecycleBinW, ssfBITBUCKET,
    };
    use windows::core::{GUID, PCWSTR};

    use super::{Calls, PhantomData, Rc, SignedInfo};

    pub(super) struct WindowsCalls;

    pub(super) struct Shell {
        dispatch: IShellDispatch,
        _thread_bound: PhantomData<Rc<()>>,
    }

    /// The SDK owns architecture-specific layout (x86 packed20, x64/aarch64 24).
    /// The closure seam permits layout/root/output recording without an OS call.
    pub(super) fn query_aggregate(
        query: impl FnOnce(PCWSTR, &mut SHQUERYRBINFO) -> Result<(), u32>,
    ) -> Result<SignedInfo, u32> {
        let mut info = SHQUERYRBINFO {
            cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
            ..Default::default()
        };
        query(PCWSTR::null(), &mut info)?;
        // Copy by value into aligned locals: never borrow x86's packed fields.
        let item_count = info.i64NumItems;
        let size_in_bytes = info.i64Size;
        Ok(SignedInfo {
            item_count,
            size_in_bytes,
        })
    }

    impl Calls for WindowsCalls {
        type Shell = Shell;
        type Target = VARIANT;

        fn initialize(&self, model: i32) -> i32 {
            // SAFETY: strict request-local STA on its owner worker, no reserved data.
            unsafe { CoInitializeEx(None, COINIT(model)).0 }
        }

        fn query(&self) -> Result<SignedInfo, u32> {
            query_aggregate(|root, info| {
                // SAFETY: initialized SDK-sized writable buffer and NULL aggregate
                // root. Success is information, not a complete real-time inventory.
                unsafe { SHQueryRecycleBinW(root, info) }.map_err(|error| error.code().0 as u32)
            })
        }

        fn create_shell(&self, class: u128, context: u32) -> Result<Shell, u32> {
            // SAFETY: live same-thread STA, fixed class/context, no aggregation.
            // Base IShellDispatch suffices; no version6 requirement or fallback.
            let dispatch: IShellDispatch =
                unsafe { CoCreateInstance(&GUID::from_u128(class), None, CLSCTX(context)) }
                    .map_err(|error| error.code().0 as u32)?;
            Ok(Shell {
                dispatch,
                _thread_bound: PhantomData,
            })
        }

        fn make_target(&self, value: i32) -> VARIANT {
            debug_assert_eq!(value, ssfBITBUCKET.0);
            VARIANT::from(value)
        }

        fn open(&self, shell: &Shell, target: &VARIANT) -> Result<(), u32> {
            // SAFETY: an owned fixed inline VT_I4 on this STA. The SDK passes the
            // VARIANT by value at the ABI, not a borrowed namespace/PIDL target.
            unsafe { shell.dispatch.Open(target) }.map_err(|error| error.code().0 as u32)
        }

        fn uninitialize(&self) {
            // SAFETY: one balance for S_OK/S_FALSE, after all request resources.
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
#[path = "native_recycle_bin/tests.rs"]
mod tests;
