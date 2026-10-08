// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Folder-local wake/pump adapter. No folder data, HWND or callback authority.

use std::marker::PhantomData;
use std::rc::Rc;
use tessera_system::folders::{FolderError, FolderErrorKind};

pub(super) trait Wake: Send + Sync + 'static {
    fn signal(&self) -> Result<(), FolderError>;
}

pub(super) trait Idle: 'static {
    fn pump(&mut self) -> Result<(), FolderError>;
    fn wait(&mut self) -> Result<(), FolderError>;
}

pub(super) const MESSAGE_BATCH: usize = 32;
// Normal requests are event-driven. This bounded LOCAL safety wake checks only
// queue/fault/disconnect state; it never reads folder or desktop state.
pub(super) const SAFETY_INTERVAL_MS: u32 = 250;

// Pinned Win32 values, also checked against the native bindings below. Keeping
// the call arguments at this seam lets recording tests run without Windows.
const QS_ALLINPUT: u32 = 1279;
const MWMO_INPUTAVAILABLE: u32 = 4;
const PM_REMOVE: u32 = 1;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
pub(super) const WM_QUIT: u32 = 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WaitOptions {
    pub handles: u32,
    pub timeout_ms: u32,
    pub wake_mask: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PeekOptions {
    pub hwnd: isize,
    pub min: u32,
    pub max: u32,
    pub flags: u32,
}

pub(super) trait MessageCalls: 'static {
    type Message;
    // Actual failure captures GetLastError inside the native call adapter.
    fn wait(&mut self, options: WaitOptions) -> Result<u32, FolderError>;
    fn peek(&mut self, options: PeekOptions) -> Option<Self::Message>;
    fn message_id(message: &Self::Message) -> u32;
    fn translate(&mut self, message: &Self::Message) -> bool;
    fn dispatch(&mut self, message: &Self::Message) -> isize;
}

pub(super) struct Pump<C> {
    calls: C,
    _thread_bound: PhantomData<Rc<()>>,
}

impl<C: MessageCalls> Pump<C> {
    pub(super) fn new(calls: C) -> Self {
        Self {
            calls,
            _thread_bound: PhantomData,
        }
    }
}

impl<C: MessageCalls> Idle for Pump<C> {
    fn pump(&mut self) -> Result<(), FolderError> {
        for _ in 0..MESSAGE_BATCH {
            // NULL includes both thread messages and COM's hidden windows.
            // Peek itself can dispatch nonqueued messages, even when empty.
            let Some(message) = self.calls.peek(PeekOptions {
                hwnd: 0,
                min: 0,
                max: 0,
                flags: PM_REMOVE,
            }) else {
                break;
            };
            if C::message_id(&message) == WM_QUIT {
                return Err(stopped(
                    "The folder worker received an unexpected quit message.",
                ));
            }
            // False translation and a zero procedure result are ordinary.
            // We install no Rust WndProc/TIMERPROC. Owned Rust completions are
            // caught outside FFI, never invoked from a native message callback.
            let _ = self.calls.translate(&message);
            let _ = self.calls.dispatch(&message);
        }
        Ok(())
    }

    fn wait(&mut self) -> Result<(), FolderError> {
        let result = self.calls.wait(WaitOptions {
            handles: 1,
            timeout_ms: SAFETY_INTERVAL_MS,
            wake_mask: QS_ALLINPUT,
            flags: MWMO_INPUTAVAILABLE,
        })?;
        // Event state is a coalesced hint, not a request count. A message/system
        // wake (including no retrieved message) also just returns to the loop.
        if matches!(result, WAIT_OBJECT_0 | 1 | WAIT_TIMEOUT) {
            Ok(())
        } else {
            Err(stopped(
                "The folder worker received an unexpected wait result.",
            ))
        }
    }
}

fn stopped(message: &str) -> FolderError {
    FolderError::new(FolderErrorKind::Stopped, message)
}

#[cfg(all(windows, not(test)))]
#[allow(unsafe_code)]
pub(super) mod native {
    use super::{FolderError, MessageCalls, PeekOptions, WaitOptions, Wake};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::Arc;
    use windows_sys::Win32::Foundation::{GetLastError, WAIT_FAILED};
    use windows_sys::Win32::System::Threading::{
        CreateEventExW, EVENT_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, MsgWaitForMultipleObjectsEx, PeekMessageW, TranslateMessage,
    };

    const _: () = {
        use windows_sys::Win32::UI::WindowsAndMessaging as win;
        assert!(super::QS_ALLINPUT == win::QS_ALLINPUT);
        assert!(super::MWMO_INPUTAVAILABLE == win::MWMO_INPUTAVAILABLE);
        assert!(super::PM_REMOVE == win::PM_REMOVE);
        assert!(super::WM_QUIT == win::WM_QUIT);
        assert!(super::WAIT_OBJECT_0 == windows_sys::Win32::Foundation::WAIT_OBJECT_0);
        assert!(super::WAIT_TIMEOUT == windows_sys::Win32::Foundation::WAIT_TIMEOUT);
    };

    pub(in crate::folders) fn event() -> Result<Arc<OwnedHandle>, FolderError> {
        // Unnamed, noninheritable, initially nonsignaled AUTO-reset event, with
        // only signal/wait access. Each wait retains its own owning Arc.
        let handle = unsafe {
            CreateEventExW(
                std::ptr::null(),
                std::ptr::null(),
                0,
                EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
            )
        };
        if handle.is_null() {
            return Err(last_error("Folder wake creation"));
        }
        Ok(Arc::new(unsafe { OwnedHandle::from_raw_handle(handle) }))
    }

    impl Wake for OwnedHandle {
        fn signal(&self) -> Result<(), FolderError> {
            if unsafe { SetEvent(self.as_raw_handle()) } == 0 {
                Err(last_error("Folder wake signal"))
            } else {
                Ok(())
            }
        }
    }

    pub(in crate::folders) struct Calls {
        event: Arc<OwnedHandle>,
    }

    impl Calls {
        pub(in crate::folders) fn new(event: Arc<OwnedHandle>) -> Self {
            Self { event }
        }
    }

    impl MessageCalls for Calls {
        type Message = MSG;

        fn wait(&mut self, options: WaitOptions) -> Result<u32, FolderError> {
            let handles = [self.event.as_raw_handle()];
            let result = unsafe {
                MsgWaitForMultipleObjectsEx(
                    options.handles,
                    handles.as_ptr(),
                    options.timeout_ms,
                    options.wake_mask,
                    options.flags,
                )
            };
            if result == WAIT_FAILED {
                Err(last_error("Folder message wait"))
            } else {
                Ok(result)
            }
        }

        fn peek(&mut self, options: PeekOptions) -> Option<MSG> {
            let mut message = MSG::default();
            (unsafe {
                PeekMessageW(
                    &mut message,
                    options.hwnd as _,
                    options.min,
                    options.max,
                    options.flags,
                )
            } != 0)
                .then_some(message)
        }

        fn message_id(message: &MSG) -> u32 {
            message.message
        }
        fn translate(&mut self, message: &MSG) -> bool {
            unsafe { TranslateMessage(message) != 0 }
        }
        fn dispatch(&mut self, message: &MSG) -> isize {
            unsafe { DispatchMessageW(message) }
        }
    }

    fn last_error(operation: &str) -> FolderError {
        // Capture before allocation/locking/any other native call. Raw Win32
        // codes must become HRESULTs to retain the existing safe error mapping.
        let code = unsafe { GetLastError() };
        super::super::worker::native_error(windows::core::HRESULT::from_win32(code).0, operation)
    }
}

#[cfg(test)]
#[path = "idle/recording.rs"]
pub(super) mod recording;
