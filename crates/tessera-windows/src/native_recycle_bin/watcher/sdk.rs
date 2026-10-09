// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::marker::PhantomData;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{NonNull, null, null_mut};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use tessera_system::dock_utilities::DockUtilityError;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::Com::{COINIT, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    ILFree, SHCNRF_SOURCE, SHChangeNotification_Lock, SHChangeNotification_Unlock,
    SHChangeNotifyDeregister, SHChangeNotifyEntry, SHChangeNotifyRegister, SHGetKnownFolderIDList,
};
use windows::core::{BOOL, GUID};
use windows_sys::Win32::Foundation::{GetLastError, HWND as RawHwnd};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    CREATE_EVENT_MANUAL_RESET, CreateEventExW, EVENT_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE,
    SetEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetWindowLongPtrW, HWND_MESSAGE, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx,
    PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassW, SetWindowLongPtrW, TranslateMessage,
    UnregisterClassW, WM_NCDESTROY, WM_QUIT, WNDCLASSW,
};

use super::{
    Calls, Contexts, Hints, MessageCalls, NOTIFY_MESSAGE, NotificationCalls, Registration,
    StopSignal, Wait, WindowContext, WindowTeardown, decode_wait, native_error, notification,
    pump_messages, teardown_window,
};

/// Standard OwnedHandle provides the only cross-thread native ownership. No
/// custom Send/Sync implementation is needed, and every pending wait borrows it.
pub(super) struct StopEvent(OwnedHandle);

impl StopEvent {
    pub(super) fn new() -> Result<Self, DockUtilityError> {
        // SAFETY: unnamed, initially nonsignaled manual-reset event, no inherited
        // security attributes. Only signal and synchronize access are requested.
        let raw = unsafe {
            CreateEventExW(
                null(),
                null(),
                CREATE_EVENT_MANUAL_RESET,
                EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
            )
        };
        if raw.is_null() {
            return Err(native_error(last_error(), "Recycle Bin watch stop event"));
        }
        // SAFETY: successful CreateEventExW returned one owned, valid handle.
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(raw) }))
    }
}

impl StopSignal for StopEvent {
    fn signal(&self) {
        // SAFETY: shared OwnedHandle remains alive across this signal and waits.
        // Failure is covered by the local cancellation-only safety wake below.
        let _ = unsafe { SetEvent(self.0.as_raw_handle()) };
    }
}

pub(super) struct WindowsCalls;

pub(super) struct Pidl {
    pointer: NonNull<ITEMIDLIST>,
    _thread_bound: PhantomData<Rc<()>>,
}

impl Drop for Pidl {
    fn drop(&mut self) {
        // SAFETY: this unique fixed SHGetKnownFolderIDList allocation is released
        // exactly once with its documented allocator, on its same STA owner.
        unsafe { ILFree(Some(self.pointer.as_ptr())) };
    }
}

thread_local! {
    static CONTEXTS: Contexts = Contexts::default();
}

struct Class {
    name: Vec<u16>,
    instance: windows_sys::Win32::Foundation::HINSTANCE,
}

impl Class {
    fn register() -> Result<Self, u32> {
        static NEXT_CLASS: AtomicU64 = AtomicU64::new(1);
        let mut value = NEXT_CLASS.load(Ordering::Relaxed);
        let sequence = loop {
            let next = value.checked_add(1).ok_or(0x8000_4005_u32)?;
            match NEXT_CLASS.compare_exchange_weak(
                value,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(old) => break old,
                Err(actual) => value = actual,
            }
        };
        let name: Vec<u16> = format!("TesseraRecycleBinWatch-{sequence}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: NULL names the current module, not a caller-supplied library.
        let instance = unsafe { GetModuleHandleW(null()) };
        if instance.is_null() {
            return Err(last_error());
        }
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: name.as_ptr(),
            ..Default::default()
        };
        // SAFETY: initialized class with static callback and valid UTF16 name.
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(last_error());
        }
        Ok(Self { name, instance })
    }
}

impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: same owner thread, after its sole window teardown. If anomalous
        // native destruction failed, unregister may fail too; no pointer is freed
        // from a still-attached window (see Window Drop).
        let _ = unsafe { UnregisterClassW(self.name.as_ptr(), self.instance) };
    }
}

pub(super) struct Window {
    hwnd: RawHwnd,
    context: Option<Rc<WindowContext>>,
    _class: Class,
    _thread_bound: PhantomData<Rc<()>>,
}

impl Window {
    fn create() -> Result<Self, u32> {
        let class = Class::register()?;
        // Context installation follows HWND creation; creation has no token.
        // SAFETY: private class and message-only parent. No UI owner, title,
        // style, menu, desktop observation or externally supplied parameter.
        // Creation messages use DefWindowProc until context is installed below.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.name.as_ptr(),
                null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                class.instance,
                null(),
            )
        };
        if hwnd.is_null() {
            return Err(last_error());
        }
        let mut window = Self {
            hwnd,
            context: None,
            _class: class,
            _thread_bound: PhantomData,
        };
        let context = CONTEXTS.with(|contexts| contexts.install(hwnd as usize))?;
        let token = context.token;
        window.context = Some(context);
        // SAFETY: userdata is a non-reused opaque token, never a Rust pointer.
        // Registry owns context; no Shell event exists before registration.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, token as _) };
        // SAFETY: our live private HWND, same owner thread. Verify installation
        // rather than infer success from SetWindowLongPtrW's ambiguous zero.
        if unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as usize != token {
            return Err(last_error());
        }
        Ok(window)
    }

    fn retire_context(&self) {
        if let Some(context) = self.context.as_ref() {
            let _ =
                CONTEXTS.try_with(|contexts| contexts.retire(self.hwnd as usize, context.token));
        }
    }
}

struct NativeTeardown(RawHwnd);

impl WindowTeardown for NativeTeardown {
    fn detach(&self) -> bool {
        // SAFETY: same-thread private window; context owner remains alive across
        // detach, verification, DestroyWindow and any synchronous native reentry.
        unsafe { SetWindowLongPtrW(self.0, GWLP_USERDATA, 0) };
        // SAFETY: same private HWND; zero proves no attached context token.
        unsafe { GetWindowLongPtrW(self.0, GWLP_USERDATA) == 0 }
    }

    fn destroy(&self) -> bool {
        // SAFETY: same owner thread, context remains alive until this returns.
        unsafe { DestroyWindow(self.0) != 0 }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.retire_context();
        let _ = teardown_window(&NativeTeardown(self.hwnd));
        // Context frees only after native teardown. Even if both native calls
        // failed, userdata is a retired numeric token, never a dangling pointer.
    }
}

impl NotificationCalls for WindowsCalls {
    type Lock = HANDLE;

    fn lock(&self, shared_handle: usize, process_id: u32) -> Option<(HANDLE, i32)> {
        let mut borrowed_pidls = null_mut();
        let mut event = 0;
        // SAFETY: the NewDelivery message's wParam HANDLE and lParam sender PID,
        // not our registration cookie. Borrowed PIDLs are never dereferenced,
        // owned, freed or carried beyond the matching Unlock.
        let lock = unsafe {
            SHChangeNotification_Lock(
                HANDLE(shared_handle as *mut _),
                process_id,
                Some(&mut borrowed_pidls),
                Some(&mut event),
            )
        };
        if lock.0.is_null() {
            None
        } else {
            Some((lock, event))
        }
    }

    fn unlock(&self, lock: &HANDLE) -> bool {
        // SAFETY: exactly one call for each successful Lock, same delivery thread.
        unsafe { SHChangeNotification_Unlock(*lock).as_bool() }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: RawHwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let handled = catch_unwind(AssertUnwindSafe(|| {
        if message == NOTIFY_MESSAGE {
            // SAFETY: only our private class uses this numeric userdata slot.
            // The checked token resolves only its own live owner-thread context.
            let token = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as usize;
            let context = CONTEXTS
                .try_with(|contexts| contexts.lookup(hwnd as usize, token))
                .ok()
                .flatten();
            if let Some(context) = context {
                // Lookup's RefCell borrow is gone before any native/reentrant
                // call. This owned clone keeps context alive through delivery.
                notification(&WindowsCalls, &context.flags, wparam, lparam as u32);
            }
            return Some(0);
        }
        if message == WM_NCDESTROY {
            // SAFETY: retire admission and detach on native destruction too.
            let token = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as usize;
            let _ = CONTEXTS.try_with(|contexts| contexts.retire(hwnd as usize, token));
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
        }
        None
    }));
    match handled {
        Ok(Some(result)) => result,
        Err(_) => 0,
        Ok(None) => {
            // SAFETY: forward ordinary window lifecycle messages to the default
            // procedure; no consumer callback or Rust borrow crosses this call.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

struct MessageQueue;

impl MessageCalls for MessageQueue {
    type Message = MSG;

    fn next(&self) -> Option<MSG> {
        let mut message = MSG::default();
        // SAFETY: writable message, NULL HWND and zero filters pump ALL thread
        // messages (including thread/COM traffic), not only our private window.
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
        // SAFETY: owned copied message obtained from this thread's own queue.
        unsafe {
            TranslateMessage(message);
            DispatchMessageW(message);
        }
    }
}

impl Calls<StopEvent> for WindowsCalls {
    type Pidl = Pidl;
    type Window = Window;

    fn initialize(&self, model: i32) -> i32 {
        // SAFETY: dedicated owner, strict STA. S_OK/S_FALSE each balanced once.
        unsafe { CoInitializeEx(None, COINIT(model)).0 }
    }

    fn resolve(&self, folder: u128) -> Result<Option<Pidl>, u32> {
        // SAFETY: fixed Recycle Bin KNOWNFOLDERID, zero flags/current-user token;
        // never null-root, desktop-root, CREATE or a caller-supplied namespace.
        let pointer = unsafe { SHGetKnownFolderIDList(&GUID::from_u128(folder), 0, None) }
            .map_err(|error| error.code().0 as u32)?;
        Ok(NonNull::new(pointer).map(|pointer| Pidl {
            pointer,
            _thread_bound: PhantomData,
        }))
    }

    fn create_window(&self, message: u32) -> Result<Window, u32> {
        debug_assert_eq!(message, NOTIFY_MESSAGE);
        Window::create()
    }

    fn register(&self, window: &Window, pidl: &Pidl, registration: Registration) -> u32 {
        let entry = SHChangeNotifyEntry {
            pidl: pidl.pointer.as_ptr(),
            fRecursive: BOOL(i32::from(registration.recursive)),
        };
        // SAFETY: one nonnull fixed-PIDL recursive entry, our message-only HWND,
        // ShellLevel/NewDelivery item mask. PIDL remains owned until deregister.
        unsafe {
            SHChangeNotifyRegister(
                HWND(window.hwnd),
                SHCNRF_SOURCE(registration.sources),
                registration.mask as i32,
                registration.message,
                registration.entries,
                &entry,
            )
        }
    }

    fn hints(&self, window: &Window) -> Hints {
        window.context.as_deref().unwrap().flags.take()
    }

    fn wait(&self, stop: &StopEvent, safety_ms: u32) -> Wait {
        let handles = [stop.0.as_raw_handle()];
        // SAFETY: OwnedHandle is borrowed across the whole wait, never closed by
        // guard drop. QS_ALLINPUT/INPUTAVAILABLE combines event and message wait.
        // The 250ms local timeout ONLY rechecks retirement if SetEvent failed;
        // it never polls data, sends a query, retries registration or emits dirt.
        let code = unsafe {
            MsgWaitForMultipleObjectsEx(
                1,
                handles.as_ptr(),
                safety_ms,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        decode_wait(code)
    }

    fn pump(&self, _: &Window, budget: usize) -> bool {
        pump_messages(&MessageQueue, budget)
    }

    fn retire(&self, window: &Window) {
        window.retire_context();
    }

    fn deregister(&self, cookie: u32) {
        // SAFETY: exactly one positive cookie owned by this same-thread session.
        // FALSE means removed/not found; no reliable GetLastError contract.
        let _ = unsafe { SHChangeNotifyDeregister(cookie) };
    }

    fn uninitialize(&self) {
        // SAFETY: same owner balance, after deregister/window/PIDL teardown.
        unsafe { CoUninitialize() };
    }
}

fn last_error() -> u32 {
    // SAFETY: read this thread's last error immediately after documented failure.
    let code = unsafe { GetLastError() };
    if code == 0 {
        0x8000_4005
    } else {
        0x8007_0000 | (code & 0xFFFF)
    }
}
