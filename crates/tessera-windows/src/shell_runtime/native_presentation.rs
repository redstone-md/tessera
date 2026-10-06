// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Reversible primary Explorer taskbar presentation. The watcher only coalesces
//! create/show notifications; the supervisor alone captures, hides and restores.
//! No lock is held over a cross-window call. Diagnostics never construct this guard.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, SetLastError,
};
use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW;
use windows_sys::Win32::System::Threading::{
    CreateEventExW, CreateMutexExW, EVENT_MODIFY_STATE, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, SYNCHRONIZATION_DELETE,
    SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
};
use windows_sys::Win32::UI::Shell::{ABM_GETSTATE, ABM_SETSTATE, APPBARDATA, SHAppBarMessage};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GA_ROOT, GW_OWNER, GetAncestor, GetClassNameW, GetWindow, GetWindowPlacement,
    GetWindowTextW, GetWindowThreadProcessId, IsWindow, IsWindowVisible, MONITORINFOF_PRIMARY,
    ShowWindowAsync, WINDOWPLACEMENT,
};

use super::error::ShellRuntimeError;
use super::placement::{CapturedAppbarState, CapturedPlacement, TaskbarFacts};
use super::session_guard::{SessionPresentation, TaskbarBackend, TaskbarHide};

fn windows_error(operation: &'static str) -> ShellRuntimeError {
    ShellRuntimeError::Windows {
        operation,
        code: unsafe { GetLastError() },
    }
}

pub(crate) struct PresentationOwner {
    _handle: OwnedHandle,
}

impl PresentationOwner {
    pub(crate) fn acquire() -> Result<Self, ShellRuntimeError> {
        let mut session = 0;
        // SAFETY: writable session output for our current process.
        if unsafe { ProcessIdToSessionId(std::process::id(), &mut session) } == 0 {
            return Err(windows_error("ProcessIdToSessionId(owner)"));
        }
        let name: Vec<u16> = format!("Local\\Tessera.Presentation.{session}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // Object lifetime, not mutex thread ownership, is the lease. A second
        // creation observes ALREADY_EXISTS before any child or native mutation.
        let handle =
            unsafe { CreateMutexExW(std::ptr::null(), name.as_ptr(), 0, SYNCHRONIZATION_DELETE) };
        if handle.is_null() {
            return Err(windows_error("CreateMutexExW"));
        }
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        // SAFETY: this call transferred one handle, including the busy case.
        let owner = Self {
            _handle: unsafe { OwnedHandle::from_raw_handle(handle) },
        };
        if already_exists {
            return Err(ShellRuntimeError::PresentationOwnerBusy);
        }
        Ok(owner)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TaskbarWindow {
    handle: isize,
    process: u32,
}

impl TaskbarWindow {
    fn hwnd(self) -> HWND {
        self.handle as HWND
    }
}

fn exact_explorer(process: u32) -> Result<Option<(u32, u32)>, ShellRuntimeError> {
    let mut own_session = 0;
    if unsafe { ProcessIdToSessionId(std::process::id(), &mut own_session) } == 0 {
        return Err(windows_error("ProcessIdToSessionId(current)"));
    }
    let mut session = 0;
    if process == 0
        || unsafe { ProcessIdToSessionId(process, &mut session) } == 0
        || session != own_session
    {
        return Ok(None);
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process) };
    if handle.is_null() {
        return Ok(None);
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut image = [0u16; 32768];
    let mut size = image.len() as u32;
    if unsafe {
        QueryFullProcessImageNameW(
            handle.as_raw_handle(),
            PROCESS_NAME_WIN32,
            image.as_mut_ptr(),
            &mut size,
        )
    } == 0
    {
        return Ok(None);
    }
    let mut directory = [0u16; 32768];
    let length =
        unsafe { GetWindowsDirectoryW(directory.as_mut_ptr(), directory.len() as u32) } as usize;
    if length == 0 || length >= directory.len() {
        return Err(windows_error("GetWindowsDirectoryW"));
    }
    let expected =
        std::path::PathBuf::from(OsString::from_wide(&directory[..length])).join("explorer.exe");
    let expected: Vec<u16> = expected.as_os_str().encode_wide().collect();
    // Windows paths compare ordinally ignoring case, not with Rust's case-sensitive Path equality.
    if unsafe {
        CompareStringOrdinal(
            image.as_ptr(),
            size as i32,
            expected.as_ptr(),
            expected.len() as i32,
            1,
        )
    } != CSTR_EQUAL
    {
        return Ok(None);
    }
    Ok(Some((session, own_session)))
}

pub(crate) fn validate_taskbar(handle: isize) -> Result<Option<TaskbarWindow>, ShellRuntimeError> {
    let hwnd = handle as HWND;
    if hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
        return Ok(None);
    }
    let mut class = [0u16; 64];
    let length = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    let class_matches = length > 0
        && class[..length as usize] == "Shell_TrayWnd".encode_utf16().collect::<Vec<_>>();
    if !class_matches {
        return Ok(None);
    }
    let mut title = [0u16; 2];
    unsafe { SetLastError(0) };
    let title_length = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
    if title_length != 0 || unsafe { GetLastError() } != 0 {
        return Ok(None);
    }
    let root_unowned = unsafe { GetAncestor(hwnd, GA_ROOT) } == hwnd
        && unsafe { GetWindow(hwnd, GW_OWNER) }.is_null();
    if !root_unowned {
        return Ok(None);
    }
    let mut bounds = windows_sys::Win32::Foundation::RECT::default();
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut bounds) } == 0
        || bounds.right <= bounds.left
        || bounds.bottom <= bounds.top
    {
        return Ok(None);
    }
    // Hidden/tool/noactivate are legitimate Explorer taskbar states, not exclusions.
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if monitor.is_null()
        || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0
        || info.dwFlags & MONITORINFOF_PRIMARY == 0
    {
        return Ok(None);
    }
    let mut process = 0;
    if unsafe { GetWindowThreadProcessId(hwnd, &mut process) } == 0 {
        return Ok(None);
    }
    let Some((process_session, own_session)) = exact_explorer(process)? else {
        return Ok(None);
    };
    let facts = TaskbarFacts {
        class_matches,
        title_empty: true,
        root_unowned,
        primary_monitor: true,
        width: i64::from(bounds.right) - i64::from(bounds.left),
        height: i64::from(bounds.bottom) - i64::from(bounds.top),
        process_id: process,
        explorer_image: true,
        process_session,
        own_session,
    };
    let window = TaskbarWindow { handle, process };
    // CREATE may expose the handle before its placement is initialized.
    // It is not a mutation failure: a later SHOW will be reconsidered.
    if placement(window).is_err() {
        return Ok(None);
    }
    Ok(facts.eligible().then_some(window))
}

#[derive(Default)]
struct FindContext {
    window: Option<TaskbarWindow>,
    error: Option<ShellRuntimeError>,
}

unsafe extern "system" fn find_callback(hwnd: HWND, parameter: LPARAM) -> windows_sys::core::BOOL {
    // Enumeration callbacks cannot unwind through user32 either.
    let context = unsafe { &mut *(parameter as *mut FindContext) };
    let result = std::panic::catch_unwind(|| validate_taskbar(hwnd as isize));
    match result {
        Ok(Ok(Some(window))) => {
            context.window = Some(window);
            0
        }
        Ok(Ok(None)) => 1,
        Ok(Err(error)) => {
            context.error = Some(error);
            0
        }
        Err(_) => {
            context.error = Some(ShellRuntimeError::HeartbeatViolation {
                reason: "taskbar enumeration panicked",
            });
            0
        }
    }
}

pub(crate) fn find_taskbar() -> Result<Option<TaskbarWindow>, ShellRuntimeError> {
    let mut context = FindContext::default();
    unsafe { SetLastError(0) };
    let result = unsafe {
        EnumWindows(
            Some(find_callback),
            (&mut context as *mut FindContext) as LPARAM,
        )
    };
    if let Some(error) = context.error {
        return Err(error);
    }
    if result == 0 && context.window.is_none() && unsafe { GetLastError() } != 0 {
        return Err(windows_error("EnumWindows(taskbar)"));
    }
    Ok(context.window)
}

fn revalidate(window: TaskbarWindow) -> Result<(), ShellRuntimeError> {
    if validate_taskbar(window.handle)? != Some(window) {
        return Err(ShellRuntimeError::Windows {
            operation: "taskbar identity changed before mutation",
            code: 1400,
        });
    }
    Ok(())
}

fn read_appbar_state(window: TaskbarWindow) -> Result<CapturedAppbarState, ShellRuntimeError> {
    let mut data = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        hWnd: window.hwnd(),
        ..Default::default()
    };
    let flags = unsafe { SHAppBarMessage(ABM_GETSTATE, &mut data) } as u32;
    CapturedAppbarState::new(flags).ok_or(ShellRuntimeError::Windows {
        operation: "ABM_GETSTATE unsupported flags",
        code: flags,
    })
}

fn placement(window: TaskbarWindow) -> Result<CapturedPlacement, ShellRuntimeError> {
    let mut data = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    if unsafe { GetWindowPlacement(window.hwnd(), &mut data) } == 0 {
        return Err(windows_error("GetWindowPlacement(taskbar)"));
    }
    CapturedPlacement::new(data.showCmd).ok_or(ShellRuntimeError::Windows {
        operation: "unexpected taskbar showCmd",
        code: data.showCmd,
    })
}

/// Bounded settling after a requested state change, never idle desktop polling.
fn wait_readback(
    window: TaskbarWindow,
    flags: u32,
    visible: bool,
    show: Option<u32>,
) -> Result<(), ShellRuntimeError> {
    let deadline = Instant::now() + Duration::from_millis(750);
    loop {
        revalidate(window)?;
        let state = read_appbar_state(window)?;
        let visibility = unsafe { IsWindowVisible(window.hwnd()) } != 0;
        let command_matches = match show {
            Some(show) => placement(window)?.show_command == show,
            None => true,
        };
        if state.flags == flags && visibility == visible && command_matches {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(ShellRuntimeError::Windows {
                operation: "taskbar state readback timed out",
                code: 1460,
            });
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn set_appbar_flags(window: TaskbarWindow, flags: u32) -> Result<(), ShellRuntimeError> {
    revalidate(window)?;
    let mut data = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        hWnd: window.hwnd(),
        lParam: flags as isize,
        ..Default::default()
    };
    // ABM_SETSTATE always returns TRUE; success is proved by readback instead.
    unsafe { SHAppBarMessage(ABM_SETSTATE, &mut data) };
    Ok(())
}

fn request_visibility(
    window: TaskbarWindow,
    command: u32,
    flags: u32,
    visible: bool,
    show: Option<u32>,
) -> Result<(), ShellRuntimeError> {
    revalidate(window)?;
    let accepted = unsafe { ShowWindowAsync(window.hwnd(), command as i32) } != 0;
    let request_error = (!accepted).then(|| windows_error("ShowWindowAsync(taskbar)"));
    // Readback is authoritative, including an already-hidden no-op. A zero
    // return never excuses a mismatch, lost HWND, wrong PID or failed appbar state.
    match wait_readback(window, flags, visible, show) {
        Ok(()) => Ok(()),
        Err(error) => Err(request_error.unwrap_or(error)),
    }
}

struct NativeTaskbar;
impl TaskbarBackend for NativeTaskbar {
    type Window = TaskbarWindow;
    type Error = ShellRuntimeError;

    fn find(&mut self) -> Result<Option<TaskbarWindow>, ShellRuntimeError> {
        find_taskbar()
    }
    fn capture(&mut self, window: TaskbarWindow) -> Result<Option<TaskbarHide>, ShellRuntimeError> {
        if validate_taskbar(window.handle)? != Some(window) {
            return Ok(None);
        }
        let captured_placement = match placement(window) {
            Ok(placement) => placement,
            Err(error) => {
                if validate_taskbar(window.handle)? != Some(window) {
                    return Ok(None);
                }
                return Err(error);
            }
        };
        let original = TaskbarHide {
            placement: captured_placement,
            appbar: read_appbar_state(window)?,
            visible: unsafe { IsWindowVisible(window.hwnd()) } != 0,
        };
        if validate_taskbar(window.handle)? != Some(window) {
            return Ok(None);
        }
        Ok(Some(original))
    }
    fn hide(
        &mut self,
        window: TaskbarWindow,
        original: TaskbarHide,
    ) -> Result<(), ShellRuntimeError> {
        revalidate(window)?;
        if read_appbar_state(window)?.flags == original.appbar.hidden_flags()
            && unsafe { IsWindowVisible(window.hwnd()) } == 0
        {
            return Ok(());
        }
        set_appbar_flags(window, original.appbar.hidden_flags())?;
        request_visibility(
            window,
            CapturedPlacement::SW_HIDE,
            original.appbar.hidden_flags(),
            false,
            None,
        )
    }
    fn restore(
        &mut self,
        window: Option<TaskbarWindow>,
        original: TaskbarHide,
    ) -> Result<(), ShellRuntimeError> {
        let window = window.ok_or(ShellRuntimeError::Windows {
            operation: "taskbar disappeared before original state could be restored",
            code: 1400,
        })?;
        // Attempt both sides of restoration even if the appbar call fails.
        let appbar = set_appbar_flags(window, original.appbar.flags);
        let visibility = request_visibility(
            window,
            original.restore_command(),
            original.appbar.flags,
            original.visible,
            original.visible.then_some(original.placement.show_command),
        );
        appbar.and(visibility)
    }
}

#[derive(Default)]
struct Pending {
    candidate: Option<isize>,
    failure: Option<ShellRuntimeError>,
}

/// One kernel event and one coalesced candidate/failure slot per session.
/// This bounded signal replaces the process-global state and unbounded queue.
struct SessionSignal {
    event: OwnedHandle,
    pending: Mutex<Pending>,
}

impl SessionSignal {
    fn create() -> Result<Self, ShellRuntimeError> {
        let handle = unsafe {
            CreateEventExW(
                std::ptr::null(),
                std::ptr::null(),
                0,
                EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
            )
        };
        if handle.is_null() {
            return Err(windows_error("CreateEventExW(presentation)"));
        }
        Ok(Self {
            event: unsafe { OwnedHandle::from_raw_handle(handle) },
            pending: Mutex::new(Pending::default()),
        })
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Pending> {
        // Poisoned state must still permit bounded failure delivery and cleanup.
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn candidate(&self, handle: isize) {
        self.lock().candidate = Some(handle);
        self.wake();
    }
    fn fail(&self, failure: ShellRuntimeError) {
        self.lock().failure.get_or_insert(failure);
        self.wake();
    }
    fn wake(&self) {
        if unsafe { SetEvent(self.event.as_raw_handle()) } == 0 {
            self.lock()
                .failure
                .get_or_insert_with(|| windows_error("SetEvent(presentation)"));
        }
    }
    fn take(&self) -> Pending {
        std::mem::take(&mut *self.lock())
    }
}

pub(crate) struct PresentationGuard {
    presentation: SessionPresentation<NativeTaskbar>,
    signal: Arc<SessionSignal>,
    watcher: Option<crate::native_events::TaskbarWatcher>,
}

impl PresentationGuard {
    pub(crate) fn new() -> Result<Self, ShellRuntimeError> {
        Ok(Self {
            presentation: SessionPresentation::new(NativeTaskbar),
            signal: Arc::new(SessionSignal::create()?),
            watcher: None,
        })
    }
    pub(crate) fn prepare(&mut self) -> Result<(), ShellRuntimeError> {
        self.presentation.prepare()
    }
    pub(crate) fn hide_after_ready(&mut self) -> Result<(), ShellRuntimeError> {
        // Register before the final discovery/hide to close the startup event gap.
        let signal = Arc::clone(&self.signal);
        self.watcher = Some(
            crate::native_events::start_taskbar_watcher(Arc::new(move |notification| {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match notification {
                    Ok(candidate) => match validate_taskbar(candidate.hwnd) {
                        Ok(Some(_)) => signal.candidate(candidate.hwnd),
                        Ok(None) => {}
                        Err(error) => signal.fail(error),
                    },
                    Err(error) => signal.fail(watcher_error(error)),
                }))
                .is_err()
                {
                    signal.fail(ShellRuntimeError::HeartbeatViolation {
                        reason: "taskbar notification panicked",
                    });
                }
            }))
            .map_err(watcher_error)?,
        );
        self.presentation.hide_after_ready()
    }
    pub(crate) fn signal_handle(&self) -> HANDLE {
        self.signal.event.as_raw_handle()
    }
    pub(crate) fn process_signal(&mut self) -> Result<(), ShellRuntimeError> {
        let pending = self.signal.take();
        if let Some(error) = pending.failure {
            return Err(error);
        }
        let Some(handle) = pending.candidate else {
            return Ok(());
        };
        // CREATE can precede usable geometry; stale/ineligible notifications are
        // ignored, awaiting SHOW, not treated as failed native mutations.
        if let Some(window) = validate_taskbar(handle)? {
            self.presentation.rehide(window)?;
        }
        Ok(())
    }
    pub(crate) fn stop_watcher(&mut self) {
        drop(self.watcher.take());
    }
    pub(crate) fn restore(&mut self) -> Result<(), ShellRuntimeError> {
        self.stop_watcher();
        self.presentation.restore()
    }
}

impl Drop for PresentationGuard {
    fn drop(&mut self) {
        // Child is declared after the guard and explicitly reaped before restore.
        // Failed explicit restoration leaves dirty set, allowing this final retry.
        let _ = self.restore();
    }
}

fn watcher_error(error: crate::apps::ApplicationError) -> ShellRuntimeError {
    match error {
        crate::apps::ApplicationError::Windows { code, .. } => ShellRuntimeError::Windows {
            operation: "taskbar watcher",
            code,
        },
        _ => ShellRuntimeError::Windows {
            operation: "taskbar watcher",
            code: 0,
        },
    }
}
