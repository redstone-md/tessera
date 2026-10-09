// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use windows::Devices::Display::Core::{
    DisplayManager, DisplayManagerChangedEventArgs, DisplayManagerDisabledEventArgs,
    DisplayManagerEnabledEventArgs, DisplayManagerOptions,
    DisplayManagerPathsFailedOrInvalidatedEventArgs,
};
use windows::Foundation::TypedEventHandler;
use windows::UI::ViewManagement::UISettings;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::IInspectable;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, HINSTANCE, HWND};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    CREATE_EVENT_MANUAL_RESET, CreateEventExW, EVENT_MODIFY_STATE, INFINITE, ResetEvent,
    SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetWindowLongPtrW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, RegisterClassW, SetWindowLongPtrW, TranslateMessage,
    UnregisterClassW, WM_DISPLAYCHANGE, WM_NCDESTROY, WM_QUIT, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_POPUP,
};

use super::{
    Calls, FAILURE, KernelCalls, MessageCalls, Notifications, Source, Wait, decode_wait,
    elapsed_ms, manager_callback, notification_callback, pump_messages,
};

pub(super) struct Kernel;
impl KernelCalls for Kernel {
    // Values are opaque under Signals' lock. Only its owner waits and closes;
    // guards/late callbacks can signal only while the same lock admits a handle.
    type Event = usize;

    fn create(&self) -> Result<usize, u32> {
        // SAFETY: unnamed manual-reset event, initially clear, not inheritable.
        let handle = unsafe {
            CreateEventExW(
                null(),
                null(),
                CREATE_EVENT_MANUAL_RESET,
                EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
            )
        };
        if handle.is_null() {
            Err(last_error())
        } else {
            Ok(handle as usize)
        }
    }

    fn set(&self, event: usize) -> Result<(), u32> {
        // SAFETY: Signals retains ownership and serializes against CloseHandle.
        if unsafe { SetEvent(event as HANDLE) } == 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }

    fn reset(&self, event: usize) -> Result<(), u32> {
        // SAFETY: the same live owned event, under the signal/close lock.
        if unsafe { ResetEvent(event as HANDLE) } == 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }

    fn close(&self, event: usize) -> Result<(), u32> {
        // SAFETY: one owned slot; only successful close removes the slot.
        if unsafe { CloseHandle(event as HANDLE) } == 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }
}

pub(super) struct Class {
    name: Vec<u16>,
    instance: HINSTANCE,
}
pub(super) struct Window(HWND);
pub(super) struct Context {
    window: usize,
    token: usize,
}

struct Attached {
    window: usize,
    token: usize,
    notifications: Arc<Notifications<Kernel>>,
}

thread_local! {
    // Exactly one dedicated watcher owns each thread. Userdata is a never-reused
    // integer, NOT a Rust pointer; an unsuccessful native detach is still safe.
    static CONTEXT: RefCell<Option<Attached>> = const { RefCell::new(None) };
}

pub(super) struct WindowsCalls {
    epoch: Instant,
}
impl WindowsCalls {
    pub(super) fn new() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }
}

impl Calls for WindowsCalls {
    type Kernel = Kernel;
    type Class = Class;
    type Window = Window;
    type Context = Context;
    type Settings = UISettings;
    type Manager = DisplayManager;
    type Token = i64;

    fn initialize(&self) -> Result<(), u32> {
        // SAFETY: strict MTA on the fresh detached owner. S_OK/S_FALSE each own
        // one balance; changed-mode/failure owns none.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(code)
    }

    fn uninitialize(&self) {
        // SAFETY: same-thread successful initialization, after every WinRT
        // object and owned callback registration resource has been released.
        unsafe { RoUninitialize() };
    }

    fn create_class(&self) -> Result<Class, u32> {
        static NEXT_CLASS: AtomicUsize = AtomicUsize::new(1);
        let sequence = next(&NEXT_CLASS)?;
        let name: Vec<_> = format!("TesseraDisplayWatch-{sequence}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: current module only; class owns its unique name.
        let instance = unsafe { GetModuleHandleW(null()) };
        if instance.is_null() {
            return Err(last_error());
        }
        let definition = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: name.as_ptr(),
            ..Default::default()
        };
        // SAFETY: static procedure, SDK layout and live UTF16 name.
        if unsafe { RegisterClassW(&definition) } == 0 {
            return Err(last_error());
        }
        Ok(Class { name, instance })
    }

    fn create_window(&self, class: &Class) -> Result<Window, u32> {
        // SAFETY: an owned hidden TOP-LEVEL 1x1 receiver, not HWND_MESSAGE.
        // No WS_VISIBLE, UI owner/parent, title, menu or activation operation.
        let window = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                class.name.as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                null_mut(),
                null_mut(),
                class.instance,
                null(),
            )
        };
        if window.is_null() {
            Err(last_error())
        } else {
            Ok(Window(window))
        }
    }

    fn attach(
        &self,
        window: &Window,
        notifications: Arc<Notifications<Kernel>>,
    ) -> Result<Context, u32> {
        static NEXT_CONTEXT: AtomicUsize = AtomicUsize::new(1);
        let token = next(&NEXT_CONTEXT)?;
        CONTEXT.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                return Err(FAILURE);
            }
            *slot = Some(Attached {
                window: window.0 as usize,
                token,
                notifications,
            });
            Ok(())
        })?;
        // SAFETY: only a checked non-reused integer goes in same-thread userdata.
        unsafe { SetWindowLongPtrW(window.0, GWLP_USERDATA, token as isize) };
        // SAFETY: compare actual installed value, avoiding ambiguous zero return.
        if unsafe { GetWindowLongPtrW(window.0, GWLP_USERDATA) } as usize != token {
            CONTEXT.with(|slot| {
                slot.borrow_mut().take();
            });
            return Err(last_error());
        }
        Ok(Context {
            window: window.0 as usize,
            token,
        })
    }

    fn retire_context(&self, context: &Context) {
        retire_context(context.window, context.token);
    }

    fn detach(&self, window: &Window) -> Result<(), u32> {
        // SAFETY: registry admission is already retired; synchronous reentry
        // cannot resolve even if native userdata clearing fails.
        unsafe { SetWindowLongPtrW(window.0, GWLP_USERDATA, 0) };
        if unsafe { GetWindowLongPtrW(window.0, GWLP_USERDATA) } == 0 {
            Ok(())
        } else {
            Err(last_error())
        }
    }

    fn destroy_window(&self, window: &Window) -> Result<(), u32> {
        // SAFETY: private same-owner HWND, context retired before native teardown.
        if unsafe { DestroyWindow(window.0) } == 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }

    fn unregister_class(&self, class: &Class) -> Result<(), u32> {
        // SAFETY: owner-thread sole window successfully destroyed first.
        if unsafe { UnregisterClassW(class.name.as_ptr(), class.instance) } == 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }

    fn create_settings(&self) -> Result<UISettings, u32> {
        UISettings::new().map_err(code)
    }
    fn create_manager(&self) -> Result<DisplayManager, u32> {
        DisplayManager::Create(DisplayManagerOptions::None).map_err(code)
    }

    fn register_text(
        &self,
        settings: &UISettings,
        notifications: Arc<Notifications<Kernel>>,
    ) -> Result<i64, u32> {
        let handler = TypedEventHandler::<UISettings, IInspectable>::new(move |_, _| {
            notification_callback(&notifications, Source::TextScale);
            Ok(())
        });
        settings.TextScaleFactorChanged(&handler).map_err(code)
    }

    fn register_manager(
        &self,
        manager: &DisplayManager,
        source: Source,
        notifications: Arc<Notifications<Kernel>>,
    ) -> Result<i64, u32> {
        // Every branch has the identical unconditional acknowledgement path;
        // returning from a closed watcher must NOT skip SetHandled(true).
        match source {
            Source::Enabled => {
                let handler =
                    TypedEventHandler::<DisplayManager, DisplayManagerEnabledEventArgs>::new(
                        move |_, args| {
                            manager_callback(&notifications, Source::Enabled, || {
                                args.ok()
                                    .and_then(|args| args.SetHandled(true))
                                    .map_err(code)
                            });
                            Ok(())
                        },
                    );
                manager.Enabled(&handler).map_err(code)
            }
            Source::Disabled => {
                let handler =
                    TypedEventHandler::<DisplayManager, DisplayManagerDisabledEventArgs>::new(
                        move |_, args| {
                            manager_callback(&notifications, Source::Disabled, || {
                                args.ok()
                                    .and_then(|args| args.SetHandled(true))
                                    .map_err(code)
                            });
                            Ok(())
                        },
                    );
                manager.Disabled(&handler).map_err(code)
            }
            Source::Changed => {
                let handler =
                    TypedEventHandler::<DisplayManager, DisplayManagerChangedEventArgs>::new(
                        move |_, args| {
                            manager_callback(&notifications, Source::Changed, || {
                                args.ok()
                                    .and_then(|args| args.SetHandled(true))
                                    .map_err(code)
                            });
                            Ok(())
                        },
                    );
                manager.Changed(&handler).map_err(code)
            }
            Source::PathsFailedOrInvalidated => {
                let handler = TypedEventHandler::<
                    DisplayManager,
                    DisplayManagerPathsFailedOrInvalidatedEventArgs,
                >::new(move |_, args| {
                    manager_callback(&notifications, Source::PathsFailedOrInvalidated, || {
                        args.ok()
                            .and_then(|args| args.SetHandled(true))
                            .map_err(code)
                    });
                    Ok(())
                });
                manager.PathsFailedOrInvalidated(&handler).map_err(code)
            }
            Source::TextScale | Source::Window => Err(FAILURE),
        }
    }

    fn start(&self, manager: &DisplayManager) -> Result<(), u32> {
        manager.Start().map_err(code)
    }
    fn stop(&self, manager: &DisplayManager) -> Result<(), u32> {
        manager.Stop().map_err(code)
    }
    fn remove_text(&self, settings: &UISettings, token: &i64) -> Result<(), u32> {
        settings.RemoveTextScaleFactorChanged(*token).map_err(code)
    }
    fn remove_manager(
        &self,
        manager: &DisplayManager,
        source: Source,
        token: &i64,
    ) -> Result<(), u32> {
        match source {
            Source::Enabled => manager.RemoveEnabled(*token),
            Source::Disabled => manager.RemoveDisabled(*token),
            Source::Changed => manager.RemoveChanged(*token),
            Source::PathsFailedOrInvalidated => manager.RemovePathsFailedOrInvalidated(*token),
            Source::TextScale | Source::Window => return Err(FAILURE),
        }
        .map_err(code)
    }
    fn close_manager(&self, manager: &DisplayManager) -> Result<(), u32> {
        manager.Close().map_err(code)
    }
    fn release_settings(&self, settings: UISettings) {
        drop(settings);
    }
    fn release_manager(&self, manager: DisplayManager) {
        drop(manager);
    }
    fn now_ms(&self) -> u64 {
        elapsed_ms(self.epoch)
    }

    fn wait(&self, stop: usize, wake: usize, timeout_ms: Option<u32>) -> Wait {
        let handles = [stop as HANDLE, wake as HANDLE];
        // SAFETY: only this owner closes, outside the wait; guards merely signal
        // under Signals' lock. Stop has index zero and wins concurrent readiness.
        // A clean watcher waits INFINITE, with no native query or safety timer.
        decode_wait(unsafe {
            MsgWaitForMultipleObjectsEx(
                2,
                handles.as_ptr(),
                timeout_ms.unwrap_or(INFINITE),
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        })
    }

    fn pump(&self, budget: usize) -> bool {
        pump_messages(&MessageQueue, budget)
    }
}

fn next(sequence: &AtomicUsize) -> Result<usize, u32> {
    // Stay positive and representable in isize userdata; never wrap or reuse.
    let mut current = sequence.load(Ordering::Relaxed);
    loop {
        let following = current
            .checked_add(1)
            .filter(|value| *value <= isize::MAX as usize)
            .ok_or(FAILURE)?;
        match sequence.compare_exchange_weak(
            current,
            following,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(previous) => return Ok(previous),
            Err(observed) => current = observed,
        }
    }
}

fn retire_context(window: usize, token: usize) {
    let _ = CONTEXT.try_with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|entry| entry.window == window && entry.token == token)
        {
            slot.take();
        }
    });
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if message == WM_DISPLAYCHANGE || message == WM_NCDESTROY {
            // SAFETY: private class, same owner, userdata is only an integer.
            let token = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as usize;
            let notifications = CONTEXT
                .try_with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .filter(|entry| entry.window == window as usize && entry.token == token)
                        .map(|entry| entry.notifications.clone())
                })
                .ok()
                .flatten();
            // RefCell lookup borrow ends before hint/native reentry.
            if message == WM_DISPLAYCHANGE {
                if let Some(notifications) = notifications {
                    notification_callback(&notifications, Source::Window);
                }
                return Some(0);
            }
            // Unexpected receiver loss is terminal; expected owner teardown has
            // already retired admission, so its lifecycle callback is inert.
            if let Some(notifications) = notifications {
                notifications.fault();
            }
            retire_context(window as usize, token);
            // SAFETY: lifecycle cleanup on this same private HWND.
            unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0) };
        }
        None
    }));
    match handled {
        Ok(Some(value)) => value,
        Err(_) => 0,
        // SAFETY: no borrowed context, owner pointer or consumer crosses FFI.
        Ok(None) => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

struct MessageQueue;
impl MessageCalls for MessageQueue {
    type Message = MSG;
    fn next(&self) -> Option<MSG> {
        let mut message = MSG::default();
        // SAFETY: SDK writable message, pump ALL owner-thread messages.
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
        // SAFETY: message obtained from this thread's own queue.
        unsafe {
            TranslateMessage(message);
            DispatchMessageW(message);
        }
    }
}

fn code(error: windows::core::Error) -> u32 {
    error.code().0 as u32
}
fn last_error() -> u32 {
    // SAFETY: immediately after a documented native failure on this thread.
    let code = unsafe { GetLastError() };
    if code == 0 {
        FAILURE
    } else {
        0x8007_0000 | (code & 0xffff)
    }
}
