// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::cell::Cell;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use std::sync::Arc;

use tessera_system::visibility::{
    PhysicalPoint, PointerEnvironment, PointerNativeOperation, PointerWatchError,
};
use windows_sys::Win32::Foundation::{
    GetLastError, POINT, SetLastError, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    CREATE_EVENT_MANUAL_RESET, CreateEventExW, EVENT_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE,
    SetEvent,
};
use windows_sys::Win32::UI::Controls::POINTER_DEVICE_INFO;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetSystemMetricsForDpi,
    SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::Pointer::GetPointerDevices;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetCursorPos, HHOOK, MSG, MSLLHOOKSTRUCT,
    MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
    SM_DIGITIZER, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL, WM_QUIT,
};

use super::environment::{DEVICE_CAP, EnvironmentCalls, pointer_environment};
use super::{Calls, Hints, HookCalls, StopSignal, Wait, hook_callback, native_error};

pub(super) struct Factory;

impl super::Factory for Factory {
    type Calls = WindowsCalls;

    fn create_stop(&self) -> Result<Arc<StopEvent>, PointerWatchError> {
        // SAFETY: unnamed, noninherited, initially nonsignaled manual-reset
        // event, with only the signal/synchronize rights used by the owner.
        let raw = unsafe {
            CreateEventExW(
                null(),
                null(),
                CREATE_EVENT_MANUAL_RESET,
                EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
            )
        };
        if raw.is_null() {
            return Err(native_error(
                PointerNativeOperation::CreateStopEvent,
                last_error(),
            ));
        }
        // SAFETY: exactly one valid ownership returned by CreateEventExW.
        Ok(Arc::new(StopEvent(unsafe {
            OwnedHandle::from_raw_handle(raw)
        })))
    }

    fn create_calls(&self) -> Result<WindowsCalls, PointerWatchError> {
        Ok(WindowsCalls)
    }
}

/// No handwritten Send/Sync or raw cross-thread ownership. The guard and owner
/// share this OwnedHandle through Arc, so guard Drop cannot close a pending wait.
/// Last-reference closure is handled once by OwnedHandle, after owner cleanup;
/// a guard retained after worker exit may hold this inert handle until its Drop.
pub(super) struct StopEvent(OwnedHandle);

impl StopSignal for StopEvent {
    fn signal(&self) -> Result<(), u32> {
        // SAFETY: a shared reference keeps the owned event open across the call.
        if unsafe { SetEvent(self.0.as_raw_handle()) } == 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }
}

struct Context {
    active: Cell<bool>,
    latest: Cell<Option<PhysicalPoint>>,
    failed: Cell<bool>,
}

impl Context {
    const fn new() -> Self {
        Self {
            active: Cell::new(false),
            latest: Cell::new(None),
            failed: Cell::new(false),
        }
    }
}

thread_local! {
    // Static thread-owned Cells, not a pointer into an Owner allocation. Late
    // native reentry cannot dereference freed Rust storage, even if unhook fails.
    static CONTEXT: Context = const { Context::new() };
}

struct NativeHookCalls;

impl HookCalls for NativeHookCalls {
    fn copy_move(&self, lparam: isize) {
        let _ = CONTEXT.try_with(|context| {
            if !context.active.get() {
                return;
            }
            if lparam == 0 {
                context.failed.set(true);
                return;
            }
            // SAFETY: Windows supplies MSLLHOOKSTRUCT for HC_ACTION. This reads
            // only its physical, per-monitor-aware point while the FFI frame is
            // alive, with no allocation, mutex, consumer or OS observation.
            let point = unsafe { (*(lparam as *const MSLLHOOKSTRUCT)).pt };
            context.latest.set(Some(PhysicalPoint {
                x: point.x,
                y: point.y,
            }));
        });
    }

    fn callback_failed(&self) {
        let _ = CONTEXT.try_with(|context| {
            if context.active.get() {
                context.failed.set(true);
            }
        });
    }

    fn chain(&self, code: i32, wparam: usize, lparam: isize) -> isize {
        // SAFETY: exact original hook arguments; the unused HHOOK may be NULL.
        // Never suppress input, including negative codes and local copy errors.
        unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    hook_callback(&NativeHookCalls, code, wparam, lparam)
}

pub(super) struct WindowsCalls;

impl Calls for WindowsCalls {
    type Stop = StopEvent;
    type Hook = HHOOK;
    type DpiContext = DPI_AWARENESS_CONTEXT;
    type Message = MSG;

    fn begin_context(&self) -> Result<(), u32> {
        CONTEXT
            .try_with(|context| {
                if context.active.replace(true) {
                    return Err(super::UNEXPECTED_PUMP);
                }
                context.latest.set(None);
                context.failed.set(false);
                Ok(())
            })
            .unwrap_or(Err(super::LOCAL_FAILURE))
    }

    fn retire_context(&self) {
        let _ = CONTEXT.try_with(|context| context.active.set(false));
    }

    fn end_context(&self) {
        let _ = CONTEXT.try_with(|context| {
            context.active.set(false);
            context.latest.set(None);
            context.failed.set(false);
        });
    }

    fn install_hook(&self) -> Result<HHOOK, u32> {
        // SAFETY: current executable module is kept loaded for this process.
        let module = unsafe { GetModuleHandleW(null()) };
        if module.is_null() {
            return Err(last_error());
        }
        // SAFETY: fixed passive LL hook/static procedure, same-desktop global
        // scope. WH_MOUSE_LL runs on this installing thread, not in a target
        // process; Owner keeps its TLS admission alive until teardown finishes.
        let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0) };
        if hook.is_null() {
            Err(last_error())
        } else {
            Ok(hook)
        }
    }

    fn remove_hook(&self, hook: HHOOK) -> Result<(), u32> {
        // SAFETY: one owned hook token, retired and consumed once on its owner.
        if unsafe { UnhookWindowsHookEx(hook) } == 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }

    fn set_dpi_context(&self) -> Result<DPI_AWARENESS_CONTEXT, u32> {
        // SetThreadDpiAwarenessContext documents NULL failure but not last error.
        // Clear stale error first; if no native code is supplied use local 1.
        unsafe { SetLastError(0) };
        // SAFETY: explicit temporary per-monitor-aware V2 context on this owner
        // only; the returned previous context is immediately owned by Owner.
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.is_null() {
            Err(last_error())
        } else {
            Ok(previous)
        }
    }

    fn restore_dpi_context(&self, previous: DPI_AWARENESS_CONTEXT) -> Result<(), u32> {
        unsafe { SetLastError(0) };
        // SAFETY: exact prior valid context from this same thread; restoration is
        // checked before startup readiness/seed delivery, even after read failure.
        let replaced = unsafe { SetThreadDpiAwarenessContext(previous) };
        if replaced.is_null() {
            Err(last_error())
        } else {
            Ok(())
        }
    }

    fn read_cursor(&self) -> Result<PhysicalPoint, u32> {
        let mut point = POINT::default();
        // SAFETY: writable output under explicit per-monitor DPI scope. No
        // desktop switching or secure-desktop access is attempted on failure.
        if unsafe { GetCursorPos(&mut point) } == 0 {
            Err(last_error())
        } else {
            Ok(PhysicalPoint {
                x: point.x,
                y: point.y,
            })
        }
    }

    fn environment(&self) -> PointerEnvironment {
        pointer_environment(self)
    }

    fn take_hints(&self) -> Hints {
        CONTEXT
            .try_with(|context| Hints {
                latest: context.latest.replace(None),
                failed: context.failed.replace(false),
            })
            .unwrap_or(Hints {
                latest: None,
                failed: true,
            })
    }

    fn wait(&self, stop: &StopEvent, safety_ms: u32) -> Result<Wait, u32> {
        let handles = [stop.0.as_raw_handle()];
        // SAFETY: borrowed shared OwnedHandle outlives the entire native wait.
        // INPUTAVAILABLE prevents unread-message starvation. The 250ms timeout
        // only examines local retirement/hints, never cursor/window/desktop data.
        let code = unsafe {
            MsgWaitForMultipleObjectsEx(
                1,
                handles.as_ptr(),
                safety_ms,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        match code {
            WAIT_OBJECT_0 => Ok(Wait::Stop),
            1 => Ok(Wait::Messages),
            WAIT_TIMEOUT => Ok(Wait::Safety),
            WAIT_FAILED => Err(last_error()),
            _ => Err(super::UNEXPECTED_PUMP),
        }
    }

    fn next_message(&self) -> Option<MSG> {
        let mut message = MSG::default();
        // SAFETY: writable copy; NULL HWND and no filters service the whole
        // installing thread queue. PeekMessage itself may run the tiny hook.
        if unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } == 0 {
            None
        } else {
            Some(message)
        }
    }

    fn is_quit(&self, message: &MSG) -> bool {
        message.message == WM_QUIT
    }

    fn dispatch(&self, message: &MSG) {
        // SAFETY: copied message from this owner's own queue, not a GUI HWND
        // supplied by the caller. No Slint objects or windows enter the owner.
        unsafe {
            TranslateMessage(message);
            DispatchMessageW(message);
        }
    }
}

impl EnvironmentCalls for WindowsCalls {
    fn digitizer_bits(&self) -> u32 {
        // SAFETY: DPI-aware variant accepts the same metric indices, avoiding
        // inherited per-monitor context restrictions. Capability bits are not
        // pixel dimensions; 96 DPI leaves this configuration setting unchanged.
        // An ambiguous zero never establishes absence of touch.
        unsafe { GetSystemMetricsForDpi(SM_DIGITIZER, 96) as u32 }
    }

    fn device_count(&self) -> Result<u32, u32> {
        let mut count = 0;
        // SAFETY: NULL list requests only the attached pointer device count.
        if unsafe { GetPointerDevices(&mut count, null_mut()) } == 0 {
            Err(last_error())
        } else {
            Ok(count)
        }
    }

    fn device_types(&self, kinds: &mut [i32]) -> Result<u32, u32> {
        if kinds.len() > DEVICE_CAP {
            return Err(super::UNEXPECTED_PUMP);
        }
        let mut count = kinds.len() as u32;
        let mut devices = [POINTER_DEVICE_INFO::default(); DEVICE_CAP];
        let output = if kinds.is_empty() {
            null_mut()
        } else {
            devices.as_mut_ptr()
        };
        // SAFETY: nonempty output has at least `count` writable structures;
        // empty output performs the documented count query. Nothing is retained,
        // registered, opened, or polled; only device kinds escape this frame.
        if unsafe { GetPointerDevices(&mut count, output) } == 0 {
            return Err(last_error());
        }
        if count as usize != kinds.len() {
            // Hotplug/count race: don't retry, allocate unboundedly, or certify
            // a partial list as no touch. The portable environment stays unknown.
            return Err(super::UNEXPECTED_PUMP);
        }
        for (kind, device) in kinds.iter_mut().zip(devices.iter()) {
            *kind = device.pointerDeviceType;
        }
        Ok(count)
    }
}

fn last_error() -> u32 {
    // SAFETY: read on the failing native call's thread, before another OS call.
    let code = unsafe { GetLastError() };
    if code == 0 {
        super::LOCAL_FAILURE
    } else {
        code
    }
}
