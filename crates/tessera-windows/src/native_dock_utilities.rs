// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One Shell toggle per request, with no COM resources retained during idle waits.

use std::marker::PhantomData;
use std::rc::Rc;

use tessera_system::dock_utilities::DockUtilityError;

use crate::dock_utilities::worker::{Driver, native_error};

// Pinned windows 0.62.2 / Shldisp bindings. No local/remote activation fallback.
const SHELL_CLSID: u128 = 0x13709620_c279_11ce_a49e_444553540000;
#[cfg(test)]
const SHELL_INTERFACE_IID: u128 = 0x286e6f1b_7113_4355_9562_96b7e9d64c54;
const SHELL_CONTEXT: u32 = 0x0000_8001; // INPROC_SERVER | DISABLE_AAA
const STA_MODEL: i32 = 2; // COINIT_APARTMENTTHREADED, never accept CHANGED_MODE

/// The recording variation uses this same operation and RAII lifetime, replacing
/// only the four native calls. Its Shell type, like the real one, need not be Send.
trait Calls {
    type Shell;

    fn initialize(&self, model: i32) -> i32;
    fn create_shell(&self, class: u128, context: u32) -> Result<Self::Shell, u32>;
    fn toggle_desktop(&self, shell: &Self::Shell) -> Result<(), u32>;
    fn uninitialize(&self);
}

/// S_OK and S_FALSE both own one balance; failed initialization owns none.
/// Explicitly thread-bound even when a recording/native call adapter is Send.
struct Apartment<'a, C: Calls> {
    calls: &'a C,
    _thread_bound: PhantomData<Rc<()>>,
}

impl<'a, C: Calls> Apartment<'a, C> {
    fn enter(calls: &'a C) -> Result<Self, DockUtilityError> {
        let code = calls.initialize(STA_MODEL);
        if code < 0 {
            return Err(native_error(code as u32, "Dock utility COM initialization"));
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

struct DockUtilitiesDriver<C: Calls> {
    calls: C,
}

impl<C: Calls + 'static> Driver for DockUtilitiesDriver<C> {
    fn toggle_desktop(&mut self) -> Result<(), DockUtilityError> {
        let _apartment = Apartment::enter(&self.calls)?;
        // Reverse local drop order releases Shell before uninitializing, including
        // native errors or unwinding. No resource survives this request scope.
        let shell = self
            .calls
            .create_shell(SHELL_CLSID, SHELL_CONTEXT)
            .map_err(|code| native_error(code, "Dock utility Shell activation"))?;
        self.calls
            .toggle_desktop(&shell)
            .map_err(|code| native_error(code, "Dock utility ToggleDesktop"))
    }
}

#[cfg(windows)]
use sdk::WindowsCalls;

#[cfg(windows)]
pub(crate) struct NativeDockUtilitiesDriver(DockUtilitiesDriver<WindowsCalls>);

#[cfg(windows)]
impl NativeDockUtilitiesDriver {
    pub(crate) fn new() -> Self {
        Self(DockUtilitiesDriver {
            calls: WindowsCalls,
        })
    }
}

#[cfg(windows)]
impl Driver for NativeDockUtilitiesDriver {
    fn toggle_desktop(&mut self) -> Result<(), DockUtilityError> {
        self.0.toggle_desktop()
    }
}

#[cfg(windows)]
mod sdk {
    use windows::Win32::System::Com::{
        CLSCTX, COINIT, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::UI::Shell::IShellDispatch6;
    use windows::core::GUID;

    use super::{Calls, PhantomData, Rc};

    pub(super) struct WindowsCalls;

    pub(super) struct Shell {
        dispatch: IShellDispatch6,
        _thread_bound: PhantomData<Rc<()>>,
    }

    impl Calls for WindowsCalls {
        type Shell = Shell;

        fn initialize(&self, model: i32) -> i32 {
            // SAFETY: request-local initialization on the dedicated worker;
            // null reserved parameter, strict STA selected by the common operation.
            unsafe { CoInitializeEx(None, COINIT(model)).0 }
        }

        fn create_shell(&self, class: u128, context: u32) -> Result<Shell, u32> {
            // SAFETY: apartment is live on this thread; fixed Shell class and
            // INPROC_SERVER | DISABLE_AAA context, no aggregation. The typed call
            // requests IShellDispatch6's exact IID, not an arbitrary interface.
            let dispatch: IShellDispatch6 =
                unsafe { CoCreateInstance(&GUID::from_u128(class), None, CLSCTX(context)) }
                    .map_err(|error| error.code().0 as u32)?;
            Ok(Shell {
                dispatch,
                _thread_bound: PhantomData,
            })
        }

        fn toggle_desktop(&self, shell: &Shell) -> Result<(), u32> {
            // SAFETY: Shell and its strict apartment remain on this worker; this
            // inherited IShellDispatch4 method is called exactly once, no retries.
            unsafe { shell.dispatch.ToggleDesktop() }.map_err(|error| error.code().0 as u32)
        }

        fn uninitialize(&self) {
            // SAFETY: guard balances a successful initialization on the same
            // thread, after releasing all request-local Shell interfaces.
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
#[path = "native_dock_utilities/tests.rs"]
mod tests;
