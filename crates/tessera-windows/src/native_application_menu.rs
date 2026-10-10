// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One lazy STA owns all native observations, menus and dispatch. The mailbox
//! has one flight and the issuer registry one observation (at most two targets).

use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use tessera_system::application_menu::{
    ApplicationAction, ApplicationActionCompletion, ApplicationActionOutcome,
    ApplicationActionTarget, ApplicationMenuCompletion, ApplicationMenuError, ApplicationMenuHost,
    ApplicationMenuSnapshot,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, WAIT_FAILED, WAIT_OBJECT_0};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, ResetEvent, SetEvent};
use windows::Win32::UI::Shell::{
    BHID_SFUIObject, CMF_ITEMMENU, CMF_NORMAL, CMINVOKECOMMANDINFO, GCS_VERBW, IContextMenu,
    IShellItem, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHCreateItemFromParsingName,
    SICHINT_CANONICAL, SIGDN_DESKTOPABSOLUTEPARSING,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, CreateWindowExW, DestroyMenu, DestroyWindow, DispatchMessageW, GetMenuState,
    HMENU, MF_BYCOMMAND, MF_DISABLED, MF_GRAYED, MSG, MWMO_INPUTAVAILABLE,
    MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT, SW_SHOWNORMAL,
    TranslateMessage, WINDOW_STYLE, WM_QUIT, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::{Interface, PCSTR, PCWSTR, PSTR, w};

use crate::Application;
use crate::application_menu::ApplicationLookup;
use crate::apps::{MAX_PARSING_NAME_LEN, valid_parsing_name};

pub(crate) struct NativeApplicationMenuHost {
    lookup: Arc<ApplicationLookup>,
    mailbox: Arc<Mailbox>,
}

impl NativeApplicationMenuHost {
    pub(crate) fn new(lookup: Arc<ApplicationLookup>) -> Self {
        Self {
            lookup,
            mailbox: Arc::new(Mailbox::default()),
        }
    }

    fn submit(&self, job: Job) -> Result<(), ApplicationMenuError> {
        let mut inbox = self
            .mailbox
            .inbox
            .lock()
            .map_err(|_| ApplicationMenuError::Unavailable)?;
        if inbox.closed || inbox.failed {
            return Err(ApplicationMenuError::Unavailable);
        }
        if inbox.busy {
            return Err(ApplicationMenuError::Busy);
        }
        if let Job::Request { target, .. } = &job
            && !inbox.targets.contains(target)
        {
            return Err(ApplicationMenuError::InvalidTarget);
        }
        if !inbox.started {
            let mailbox = Arc::clone(&self.mailbox);
            let lookup = Arc::clone(&self.lookup);
            if std::thread::Builder::new()
                .name("tessera-application-menu".into())
                .spawn(move || owner_loop(mailbox, lookup))
                .is_err()
            {
                inbox.failed = true;
                return Err(ApplicationMenuError::Unavailable);
            }
            inbox.started = true;
        }
        // Wake while holding the mailbox lock. The owner cannot consume the
        // wake until the job is installed; failure therefore accepts no work.
        if let Some(wake) = &inbox.wake {
            wake.signal()?;
        }
        inbox.targets.clear();
        inbox.busy = true;
        inbox.job = Some(job);
        Ok(())
    }
}

impl ApplicationMenuHost for NativeApplicationMenuHost {
    fn inspect(
        &self,
        application_key: &str,
        completion: ApplicationMenuCompletion,
    ) -> Result<(), ApplicationMenuError> {
        if !valid_parsing_name(application_key) {
            return Err(ApplicationMenuError::Unavailable);
        }
        self.submit(Job::Inspect {
            key: application_key.to_owned(),
            completion,
        })
    }

    fn request(
        &self,
        target: ApplicationActionTarget,
        completion: ApplicationActionCompletion,
    ) -> Result<(), ApplicationMenuError> {
        self.submit(Job::Request { target, completion })
    }
}

impl Drop for NativeApplicationMenuHost {
    fn drop(&mut self) {
        if let Ok(mut inbox) = self.mailbox.inbox.lock() {
            inbox.closed = true;
            if let Some(wake) = &inbox.wake {
                let _ = wake.signal();
            }
        }
        // No join: accepted work belongs to the owner, not to this handle.
    }
}

#[derive(Default)]
struct Mailbox {
    inbox: Mutex<Inbox>,
}

#[derive(Default)]
struct Inbox {
    started: bool,
    busy: bool,
    closed: bool,
    failed: bool,
    job: Option<Job>,
    wake: Option<WakeEvent>,
    targets: Vec<ApplicationActionTarget>,
}

enum Job {
    Inspect {
        key: String,
        completion: ApplicationMenuCompletion,
    },
    Request {
        target: ApplicationActionTarget,
        completion: ApplicationActionCompletion,
    },
}

impl Job {
    fn unavailable(self) {
        match self {
            Self::Inspect { completion, .. } => {
                finish(move || completion(Err(ApplicationMenuError::Unavailable)))
            }
            Self::Request { completion, .. } => {
                finish(move || completion(Err(ApplicationMenuError::Unavailable)))
            }
        }
    }
}

/// Only the synchronization handle crosses threads, never shell interfaces.
/// The integer representation avoids pretending a COM pointer is Send.
struct WakeEvent(isize);

impl WakeEvent {
    fn new() -> Result<Self, ApplicationMenuError> {
        // SAFETY: unnamed, manual-reset event with default security.
        let handle = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map_err(|_| ApplicationMenuError::Unavailable)?;
        Ok(Self(handle.0 as isize))
    }

    fn handle(&self) -> HANDLE {
        HANDLE(self.0 as *mut core::ffi::c_void)
    }

    fn signal(&self) -> Result<(), ApplicationMenuError> {
        // SAFETY: mailbox lock protects the owned event's lifetime.
        unsafe { SetEvent(self.handle()) }.map_err(|_| ApplicationMenuError::Unavailable)
    }
}

impl Drop for WakeEvent {
    fn drop(&mut self) {
        // SAFETY: event is uniquely owned and no signaling can outlive its lock.
        let _ = unsafe { CloseHandle(self.handle()) };
    }
}

struct Apartment;

impl Apartment {
    fn enter() -> Result<Self, ApplicationMenuError> {
        // SAFETY: the dedicated thread requires its own strict STA.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(|_| ApplicationMenuError::Unavailable)?;
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances initialization after native objects have dropped.
        unsafe { CoUninitialize() };
    }
}

struct OwnerWindow(HWND);

impl OwnerWindow {
    fn new() -> Result<Self, ApplicationMenuError> {
        // SAFETY: built-in class; invisible, nonactivating top-level owner.
        unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("Tessera application actions"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )
        }
        .map(Self)
        .map_err(|_| ApplicationMenuError::Unavailable)
    }
}

impl Drop for OwnerWindow {
    fn drop(&mut self) {
        // SAFETY: created and destroyed on the same owner thread.
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

fn owner_loop(mailbox: Arc<Mailbox>, lookup: Arc<ApplicationLookup>) {
    let setup = (|| {
        let apartment = Apartment::enter()?;
        let window = OwnerWindow::new()?;
        let event = WakeEvent::new()?;
        Ok::<_, ApplicationMenuError>((apartment, window, event))
    })();
    let Ok((_apartment, window, event)) = setup else {
        fail_mailbox(&mailbox);
        return;
    };
    let event_handle = event.handle();
    {
        let mut inbox = mailbox
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inbox.wake = Some(event);
    }
    let mut observation: Option<Observation> = None;
    loop {
        let (job, closed) = {
            let mut inbox = mailbox
                .inbox
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // SAFETY: reset under the same lock used to signal/install jobs.
            if unsafe { ResetEvent(event_handle) }.is_err() {
                drop(inbox);
                break;
            }
            (inbox.job.take(), inbox.closed)
        };
        if let Some(job) = job {
            match job {
                Job::Inspect { key, completion } => {
                    observation = None;
                    let result = inspect(&key, lookup.as_ref()).map(|next| {
                        let snapshot = ApplicationMenuSnapshot {
                            application_key: key,
                            targets: next.targets.clone(),
                        };
                        observation = Some(next);
                        snapshot
                    });
                    release_flight(
                        &mailbox,
                        result
                            .as_ref()
                            .ok()
                            .map(|snapshot| snapshot.targets.clone()),
                    );
                    finish(move || completion(result));
                }
                Job::Request { target, completion } => {
                    // Own the original source through fresh validation and
                    // dispatch, even when the popup/provider has retired.
                    let result = observation
                        .take()
                        .ok_or(ApplicationMenuError::InvalidTarget)
                        .and_then(|source| source.request(&target, lookup.as_ref(), window.0));
                    release_flight(&mailbox, None);
                    finish(move || completion(result));
                }
            }
            continue;
        }
        if closed {
            break;
        }
        // SAFETY: stable event plus the owner STA's message queue. Infinite
        // event-driven wait: no polling, periodic work or observer thread.
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&[event_handle]),
                INFINITE,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if wait == WAIT_FAILED {
            break;
        }
        if wait.0 == WAIT_OBJECT_0.0 + 1 {
            let mut message = MSG::default();
            // SAFETY: only this thread's messages are removed/dispatched.
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                if message.message == WM_QUIT {
                    fail_mailbox(&mailbox);
                    return;
                }
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        } else if wait != WAIT_OBJECT_0 {
            break;
        }
    }
    drop(observation);
    fail_mailbox(&mailbox);
}

fn release_flight(mailbox: &Mailbox, targets: Option<Vec<ApplicationActionTarget>>) {
    let mut inbox = mailbox
        .inbox
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    inbox.targets = targets.unwrap_or_default();
    inbox.busy = false;
}

fn fail_mailbox(mailbox: &Mailbox) {
    let job = {
        let mut inbox = mailbox
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inbox.failed = true;
        inbox.busy = false;
        inbox.targets.clear();
        inbox.wake = None;
        inbox.job.take()
    };
    if let Some(job) = job {
        job.unavailable();
    }
}

fn finish(callback: impl FnOnce()) {
    // A client panic must not destroy the native owner or strand later work.
    let _ = catch_unwind(AssertUnwindSafe(callback));
}

struct Observation {
    key: String,
    application: Application,
    native: NativeMenu,
    targets: Vec<ApplicationActionTarget>,
}

fn inspect(key: &str, lookup: &ApplicationLookup) -> Result<Observation, ApplicationMenuError> {
    let application = lookup(key)
        .filter(|app| app.id() == key)
        .ok_or(ApplicationMenuError::Unavailable)?;
    let native = NativeMenu::observe(&application)?;
    let targets = native
        .commands
        .iter()
        .map(|command| ApplicationActionTarget::new(command.action))
        .collect();
    Ok(Observation {
        key: key.to_owned(),
        application,
        native,
        targets,
    })
}

impl Observation {
    fn request(
        self,
        target: &ApplicationActionTarget,
        lookup: &ApplicationLookup,
        owner: HWND,
    ) -> Result<ApplicationActionOutcome, ApplicationMenuError> {
        if !self.targets.contains(target) {
            return Err(ApplicationMenuError::InvalidTarget);
        }
        let current = lookup(&self.key)
            .filter(|app| app == &self.application)
            .ok_or(ApplicationMenuError::Unavailable)?;
        let fresh = NativeMenu::observe(&current)?;
        // SAFETY: both items remain on this STA; canonical comparison is the
        // shell's identity check, not a label/path/PID fallback.
        if unsafe {
            self.native
                .item
                .Compare(&fresh.item, SICHINT_CANONICAL.0 as u32)
        }
        .map_err(|_| ApplicationMenuError::Unavailable)?
            != 0
        {
            return Err(ApplicationMenuError::Unavailable);
        }
        let original = self
            .native
            .commands
            .iter()
            .find(|command| command.action == target.action())
            .ok_or(ApplicationMenuError::InvalidTarget)?;
        let command = fresh
            .commands
            .iter()
            .find(|command| command.action == original.action && command.verb == original.verb)
            .ok_or(ApplicationMenuError::Unavailable)?;
        fresh.invoke(command, owner)
    }
}

struct Command {
    action: ApplicationAction,
    verb: String,
    offset: u32,
}

struct OwnedMenu(HMENU);

impl Drop for OwnedMenu {
    fn drop(&mut self) {
        // SAFETY: popup and any submenus are owned and never shown.
        let _ = unsafe { DestroyMenu(self.0) };
    }
}

struct NativeMenu {
    item: IShellItem,
    context: IContextMenu,
    _menu: OwnedMenu,
    commands: Vec<Command>,
}

const MAX_COMMANDS: u32 = 256;
const FIRST_COMMAND: u32 = 1;
const MAX_VERB_UNITS: usize = 64;

impl NativeMenu {
    fn observe(application: &Application) -> Result<Self, ApplicationMenuError> {
        if !valid_parsing_name(application.id()) {
            return Err(ApplicationMenuError::Unavailable);
        }
        let id: Vec<u16> = application.id().encode_utf16().chain(Some(0)).collect();
        // SAFETY: only an encapsulated, current enumerated Application enters
        // this parser; UI keys and arbitrary strings never do.
        let item: IShellItem =
            unsafe { SHCreateItemFromParsingName(PCWSTR(id.as_ptr()), None::<&IBindCtx>) }
                .map_err(|_| ApplicationMenuError::Unavailable)?;
        if canonical_name(&item)? != application.id() {
            return Err(ApplicationMenuError::Unavailable);
        }
        // SAFETY: documented shell UI-object binding for this exact item.
        let context: IContextMenu =
            unsafe { item.BindToHandler(None::<&IBindCtx>, &BHID_SFUIObject) }
                .map_err(|_| ApplicationMenuError::Unavailable)?;
        let menu =
            OwnedMenu(unsafe { CreatePopupMenu() }.map_err(|_| ApplicationMenuError::Unavailable)?);
        // SAFETY: owned popup and a bounded command-ID range; no async state.
        let result = unsafe {
            context.QueryContextMenu(
                menu.0,
                0,
                FIRST_COMMAND,
                MAX_COMMANDS,
                CMF_NORMAL | CMF_ITEMMENU,
            )
        };
        result.ok().map_err(|_| ApplicationMenuError::Unavailable)?;
        let count = (result.0 as u32) & 0xffff;
        if count > MAX_COMMANDS {
            return Err(ApplicationMenuError::Unavailable);
        }
        let mut commands = Vec::with_capacity(2);
        for action in [
            ApplicationAction::RunAsAdministrator,
            ApplicationAction::OpenFileLocation,
        ] {
            let mut matched = None;
            let mut ambiguous = false;
            for offset in 0..count {
                let Some(verb) = canonical_verb(&context, offset) else {
                    continue;
                };
                let supported = match action {
                    ApplicationAction::RunAsAdministrator => verb.eq_ignore_ascii_case("runas"),
                    // Not a documented universal verb. Expose only when this
                    // actual AppsFolder handler advertises it canonically.
                    ApplicationAction::OpenFileLocation => {
                        verb.eq_ignore_ascii_case("opencontaining")
                    }
                };
                if !supported {
                    continue;
                }
                // SAFETY: command belongs to the just-populated native menu.
                let state = unsafe { GetMenuState(menu.0, FIRST_COMMAND + offset, MF_BYCOMMAND) };
                if state == u32::MAX || state & (MF_DISABLED.0 | MF_GRAYED.0) != 0 {
                    continue;
                }
                if matched.is_some() {
                    ambiguous = true;
                    break;
                }
                matched = Some(Command {
                    action,
                    verb,
                    offset,
                });
            }
            if !ambiguous && let Some(command) = matched {
                commands.push(command);
            }
        }
        Ok(Self {
            item,
            context,
            _menu: menu,
            commands,
        })
    }

    fn invoke(
        &self,
        command: &Command,
        owner: HWND,
    ) -> Result<ApplicationActionOutcome, ApplicationMenuError> {
        let info = CMINVOKECOMMANDINFO {
            cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32,
            // SDK CMIC_MASK_* macros alias these emitted SEE_MASK_* constants.
            fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            hwnd: owner,
            // Documented MAKEINTRESOURCE(offset), not a caller string. The
            // offset is from this freshly queried canonical native menu.
            lpVerb: PCSTR(command.offset as usize as *const u8),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        // SAFETY: retain original and fresh native sources, menu, owner and
        // apartment until dispatch returns. Read the HRESULT directly because
        // Result<()> would erase S_FALSE and other non-S_OK success statuses.
        let result = unsafe {
            (Interface::vtable(&self.context).InvokeCommand)(self.context.as_raw(), &info)
        };
        match result.0 as u32 {
            0 => Ok(ApplicationActionOutcome::Accepted),
            0x800704c7 => Ok(ApplicationActionOutcome::Declined), // HRESULT_FROM_WIN32(ERROR_CANCELLED)
            _ => Err(ApplicationMenuError::Unconfirmed),
        }
    }
}

fn canonical_verb(context: &IContextMenu, offset: u32) -> Option<String> {
    let mut buffer = [0u16; MAX_VERB_UNITS];
    // SAFETY: GCS_VERBW writes UTF-16 despite the API's PSTR declaration;
    // buffer length is in UTF-16 characters, reserved is null.
    unsafe {
        context.GetCommandString(
            offset as usize,
            GCS_VERBW,
            None,
            PSTR(buffer.as_mut_ptr().cast()),
            buffer.len() as u32,
        )
    }
    .ok()?;
    let end = buffer.iter().position(|unit| *unit == 0)?;
    if end == 0 {
        return None;
    }
    String::from_utf16(&buffer[..end]).ok()
}

fn canonical_name(item: &IShellItem) -> Result<String, ApplicationMenuError> {
    // SAFETY: shell-owned task allocation, released exactly once below.
    let name = unsafe { item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING) }
        .map_err(|_| ApplicationMenuError::Unavailable)?;
    if name.0.is_null() {
        return Err(ApplicationMenuError::Unavailable);
    }
    let mut units = Vec::new();
    for index in 0..=MAX_PARSING_NAME_LEN {
        // SAFETY: documented NUL-terminated shell allocation; the scan is
        // bounded exactly as in the existing catalog's native conversion.
        let unit = unsafe { *name.0.add(index) };
        if unit == 0 {
            unsafe { CoTaskMemFree(Some(name.0.cast())) };
            return String::from_utf16(&units).map_err(|_| ApplicationMenuError::Unavailable);
        }
        units.push(unit);
    }
    unsafe { CoTaskMemFree(Some(name.0.cast())) };
    Err(ApplicationMenuError::Unavailable)
}
