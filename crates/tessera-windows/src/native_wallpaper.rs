// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One lazy STA owns the dialog, protected file selection, global position and
//! current slideshow policy ledgers. One event-backed mailbox; idle means wait.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, TryLockError};

use tessera_system::wallpaper::{
    WallpaperApplyCompletion, WallpaperApplyOutcome, WallpaperApplyScope,
    WallpaperChooseCompletion, WallpaperError, WallpaperHost, WallpaperImageTarget,
    WallpaperImageTargetWeak, WallpaperMonitorTarget, WallpaperSelection,
    collection as collection_contract, position as position_contract,
    slideshow as slideshow_contract,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, WAIT_FAILED, WAIT_OBJECT_0};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, ResetEvent, SetEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, MWMO_INPUTAVAILABLE,
    MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT, TranslateMessage,
    WINDOW_STYLE, WM_QUIT, WS_EX_TOOLWINDOW,
};
use windows::core::{PCWSTR, w};

use crate::single_flight::{Flight, FlightGate};

mod collection;
mod displays;
mod position;
mod slideshow;
mod source;
mod thumbnail;

#[derive(Default)]
pub(crate) struct NativeWallpaperHost {
    gate: FlightGate,
    mailbox: Arc<Mailbox>,
}

impl NativeWallpaperHost {
    fn submit(&self, work: Work) -> Result<(), WallpaperError> {
        let flight = self.gate.try_enter().ok_or(WallpaperError::Busy)?;
        let mut inbox = self.mailbox.inbox.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => WallpaperError::Busy,
            TryLockError::Poisoned(_) => WallpaperError::Unavailable,
        })?;
        if inbox.closed || inbox.failed {
            return Err(WallpaperError::Unavailable);
        }
        if let Work::Apply { target, scope, .. } = &work
            && !inbox
                .issued
                .as_ref()
                .is_some_and(|issued| issued.accepts(target, scope))
        {
            // A forged/retired token cannot start an owner or reach the SDK.
            return Err(WallpaperError::InvalidTarget);
        }
        if let Work::ApplyCollection { target, .. } = &work
            && !inbox
                .issued
                .as_ref()
                .is_some_and(|issued| issued.accepts_collection(target))
        {
            return Err(WallpaperError::InvalidTarget);
        }
        if let Work::SetPosition { target, .. } = &work
            && inbox.position_issued.as_ref() != Some(target)
        {
            // Position tickets are independent of image/monitor authority.
            return Err(WallpaperError::InvalidTarget);
        }
        if let Work::AdvanceSlideshow {
            target, monitor, ..
        } = &work
            && !inbox
                .slideshow_issued
                .as_ref()
                .is_some_and(|issued| issued.accepts(target, monitor))
        {
            // Both the exact policy and its exact native cohort member are
            // required before starting/signaling an owner or touching the SDK.
            return Err(WallpaperError::InvalidTarget);
        }
        if inbox.job.is_some() {
            return Err(WallpaperError::Busy);
        }
        if !inbox.started {
            let mailbox = Arc::clone(&self.mailbox);
            if std::thread::Builder::new()
                .name("tessera-wallpaper".into())
                .spawn(move || owner_loop(mailbox))
                .is_err()
            {
                inbox.failed = true;
                return Err(WallpaperError::Unavailable);
            }
            inbox.started = true;
        }
        // Signal/install under one lock: signaling failure accepts no work.
        // The initial job needs no signal; setup inspects the installed inbox.
        if let Some(wake) = &inbox.wake
            && wake.signal().is_err()
        {
            inbox.failed = true;
            inbox.issued = None;
            inbox.position_issued = None;
            inbox.slideshow_issued = None;
            return Err(WallpaperError::Unavailable);
        }
        match &work {
            Work::Choose { .. } | Work::ChooseCollection { .. } => inbox.issued = None,
            Work::Apply { .. } | Work::ApplyCollection { .. } => {
                inbox.issued = None;
                // Accepted file application changes OS policy; draft choice
                // never revokes independently captured current-policy authority.
                inbox.slideshow_issued = None;
            }
            Work::ReadPosition { .. } | Work::SetPosition { .. } => {
                inbox.position_issued = None;
            }
            Work::ReadSlideshow { .. } | Work::AdvanceSlideshow { .. } => {
                inbox.slideshow_issued = None;
            }
        }
        inbox.job = Some(Job { work, flight });
        Ok(())
    }
}

impl WallpaperHost for NativeWallpaperHost {
    fn choose(&self, completion: WallpaperChooseCompletion) -> Result<(), WallpaperError> {
        self.submit(Work::Choose { completion })
    }

    fn apply(
        &self,
        target: WallpaperImageTarget,
        scope: WallpaperApplyScope,
        completion: WallpaperApplyCompletion,
    ) -> Result<(), WallpaperError> {
        self.submit(Work::Apply {
            target,
            scope,
            completion,
        })
    }

    fn choose_collection(
        &self,
        completion: collection_contract::ChooseCompletion,
    ) -> Result<(), WallpaperError> {
        self.submit(Work::ChooseCollection { completion })
    }

    fn apply_collection(
        &self,
        target: collection_contract::Target,
        options: collection_contract::Options,
        completion: collection_contract::ApplyCompletion,
    ) -> Result<(), WallpaperError> {
        self.submit(Work::ApplyCollection {
            target,
            options,
            completion,
        })
    }

    fn read_position(
        &self,
        completion: position_contract::ReadCompletion,
    ) -> Result<(), WallpaperError> {
        self.submit(Work::ReadPosition { completion })
    }

    fn set_position(
        &self,
        target: position_contract::Target,
        desired: position_contract::Position,
        completion: position_contract::WriteCompletion,
    ) -> Result<(), WallpaperError> {
        self.submit(Work::SetPosition {
            target,
            desired,
            completion,
        })
    }

    fn read_slideshow(
        &self,
        completion: slideshow_contract::ReadCompletion,
    ) -> Result<(), WallpaperError> {
        self.submit(Work::ReadSlideshow { completion })
    }

    fn advance_slideshow(
        &self,
        target: slideshow_contract::Target,
        monitor: WallpaperMonitorTarget,
        direction: slideshow_contract::Direction,
        completion: slideshow_contract::AdvanceCompletion,
    ) -> Result<(), WallpaperError> {
        self.submit(Work::AdvanceSlideshow {
            target,
            monitor,
            direction,
            completion,
        })
    }
}

impl Drop for NativeWallpaperHost {
    fn drop(&mut self) {
        if let Ok(mut inbox) = self.mailbox.inbox.lock() {
            inbox.closed = true;
            if let Some(wake) = &inbox.wake {
                let _ = wake.signal();
            }
        }
        // No join or cancellation: accepted work remains owned by the STA.
    }
}

#[derive(Default)]
struct Mailbox {
    inbox: Mutex<Inbox>,
}

#[derive(Default)]
struct Inbox {
    started: bool,
    closed: bool,
    failed: bool,
    job: Option<Job>,
    wake: Option<WakeEvent>,
    issued: Option<Issuer>,
    position_issued: Option<position_contract::Target>,
    slideshow_issued: Option<slideshow::Issuer>,
    retire_pending: bool,
}

enum Issuer {
    Image(ImageIssuer),
    Collection(collection_contract::TargetWeak),
}

struct ImageIssuer {
    image: WallpaperImageTargetWeak,
    monitors: Vec<WallpaperMonitorTarget>,
}

impl Issuer {
    fn accepts(&self, image: &WallpaperImageTarget, scope: &WallpaperApplyScope) -> bool {
        matches!(self, Self::Image(issuer) if issuer.accepts(image, scope))
    }

    fn accepts_collection(&self, target: &collection_contract::Target) -> bool {
        matches!(self, Self::Collection(issuer) if issuer.matches(target))
    }
}

impl ImageIssuer {
    fn from_selection(selection: &WallpaperSelection) -> Self {
        Self {
            image: selection.target.downgrade(),
            monitors: selection
                .monitors
                .iter()
                .map(|monitor| monitor.target.clone())
                .collect(),
        }
    }

    fn accepts(&self, image: &WallpaperImageTarget, scope: &WallpaperApplyScope) -> bool {
        self.image.matches(image)
            && match scope {
                WallpaperApplyScope::AllCaptured => true,
                WallpaperApplyScope::Monitor(target) => self.monitors.contains(target),
            }
    }
}

enum Work {
    Choose {
        completion: WallpaperChooseCompletion,
    },
    Apply {
        target: WallpaperImageTarget,
        scope: WallpaperApplyScope,
        completion: WallpaperApplyCompletion,
    },
    ChooseCollection {
        completion: collection_contract::ChooseCompletion,
    },
    ApplyCollection {
        target: collection_contract::Target,
        options: collection_contract::Options,
        completion: collection_contract::ApplyCompletion,
    },
    ReadPosition {
        completion: position_contract::ReadCompletion,
    },
    SetPosition {
        target: position_contract::Target,
        desired: position_contract::Position,
        completion: position_contract::WriteCompletion,
    },
    ReadSlideshow {
        completion: slideshow_contract::ReadCompletion,
    },
    AdvanceSlideshow {
        target: slideshow_contract::Target,
        monitor: WallpaperMonitorTarget,
        direction: slideshow_contract::Direction,
        completion: slideshow_contract::AdvanceCompletion,
    },
}

struct Job {
    work: Work,
    flight: Flight,
}

impl Job {
    fn unavailable(self) {
        drop(self.flight);
        match self.work {
            Work::Choose { completion } => {
                finish(move || completion(Err(WallpaperError::Unavailable)))
            }
            Work::Apply { completion, .. } => {
                finish(move || completion(Err(WallpaperError::Unavailable)))
            }
            Work::ChooseCollection { completion } => {
                finish(move || completion(Err(WallpaperError::Unavailable)))
            }
            Work::ApplyCollection { completion, .. } => {
                finish(move || completion(Err(WallpaperError::Unavailable)))
            }
            Work::ReadPosition { completion } => {
                finish(move || completion(Err(WallpaperError::Unavailable)))
            }
            Work::SetPosition { completion, .. } => {
                finish(move || completion(Err(WallpaperError::Unavailable)))
            }
            Work::ReadSlideshow { completion } => {
                finish(move || completion(Err(WallpaperError::Unavailable)))
            }
            Work::AdvanceSlideshow { completion, .. } => {
                finish(move || completion(Err(WallpaperError::Unavailable)))
            }
        }
    }
}

/// Only this synchronization handle crosses threads, never COM/file resources.
struct WakeEvent(isize);

impl WakeEvent {
    fn new() -> Result<Self, WallpaperError> {
        let handle = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map_err(|_| WallpaperError::Unavailable)?;
        Ok(Self(handle.0 as isize))
    }

    fn handle(&self) -> HANDLE {
        HANDLE(self.0 as *mut core::ffi::c_void)
    }

    fn signal(&self) -> Result<(), WallpaperError> {
        // SAFETY: the inbox lock protects the event's lifetime during signaling.
        unsafe { SetEvent(self.handle()) }.map_err(|_| WallpaperError::Unavailable)
    }
}

impl Drop for WakeEvent {
    fn drop(&mut self) {
        // SAFETY: unique ownership; teardown runs on the native owner thread.
        let _ = unsafe { CloseHandle(self.handle()) };
    }
}

struct Apartment;

impl Apartment {
    fn enter() -> Result<Self, WallpaperError> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(|_| WallpaperError::Unavailable)?;
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances initialization after all owner COM resources drop.
        unsafe { CoUninitialize() };
    }
}

struct OwnerWindow(HWND);

impl OwnerWindow {
    fn new() -> Result<Self, WallpaperError> {
        // SAFETY: built-in class, dedicated thread's own invisible top-level
        // window. The dialog itself is normally activated, never suppressed.
        unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("Tessera wallpaper selection"),
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
        .map_err(|_| WallpaperError::Unavailable)
    }
}

impl Drop for OwnerWindow {
    fn drop(&mut self) {
        // SAFETY: the HWND is created and destroyed on this same owner thread.
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

fn owner_loop(mailbox: Arc<Mailbox>) {
    let setup = (|| {
        let apartment = Apartment::enter()?;
        let window = OwnerWindow::new()?;
        let event = WakeEvent::new()?;
        Ok::<_, WallpaperError>((apartment, window, event))
    })();
    let Ok((_apartment, window, event)) = setup else {
        fail_mailbox(&mailbox);
        return;
    };
    let event_handle = event.handle();
    mailbox
        .inbox
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .wake = Some(event);
    let mut current: Option<FileSelection> = None;
    let mut current_position = position::Ledger::default();
    let mut current_slideshow = slideshow::Ledger::default();
    loop {
        let (job, closed, retired) = {
            let mut inbox = mailbox
                .inbox
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // SAFETY: reset shares the lock used to signal/install the next job.
            if unsafe { ResetEvent(event_handle) }.is_err() {
                drop(inbox);
                break;
            }
            let retired = std::mem::take(&mut inbox.retire_pending);
            (inbox.job.take(), inbox.closed || inbox.failed, retired)
        };
        if retired
            && current
                .as_ref()
                .is_some_and(|selection| !selection.is_alive())
        {
            current = None;
            publish_issuer(&mailbox, None);
        }
        if retired && current_slideshow.retire() {
            publish_slideshow_issuer(&mailbox, None);
        }
        if let Some(Job { work, flight }) = job {
            match work {
                Work::Choose { completion } => {
                    // Replacement retires resources on their native owner.
                    current = None;
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        Selection::choose(window.0, &mailbox).map(|selection| {
                            selection.map(|(selection, snapshot)| {
                                current = Some(FileSelection::Image(selection));
                                snapshot
                            })
                        })
                    }))
                    .unwrap_or_else(|_| {
                        current = None;
                        Err(WallpaperError::Unavailable)
                    });
                    publish_issuer(
                        &mailbox,
                        result.as_ref().ok().and_then(|value| {
                            value.as_ref().map(|snapshot| {
                                Issuer::Image(ImageIssuer::from_selection(snapshot))
                            })
                        }),
                    );
                    drop(flight);
                    finish(move || completion(result));
                }
                Work::Apply {
                    target,
                    scope,
                    completion,
                } => {
                    current_slideshow.clear();
                    let result = current
                        .take()
                        .and_then(|selection| match selection {
                            FileSelection::Image(selection)
                                if selection.target.matches(&target) =>
                            {
                                Some(selection)
                            }
                            _ => None,
                        })
                        .ok_or(WallpaperError::InvalidTarget)
                        .and_then(|selection| selection.apply(scope));
                    // Accepted apply owns its public ticket through every SDK
                    // write/readback, even if all UI tickets were retired.
                    drop(target);
                    publish_issuer(&mailbox, None);
                    drop(flight);
                    finish(move || completion(result));
                }
                Work::ChooseCollection { completion } => {
                    current = None;
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        collection::Collection::choose(window.0, &mailbox).map(|selection| {
                            selection.map(|(selection, snapshot)| {
                                current = Some(FileSelection::Collection(selection));
                                snapshot
                            })
                        })
                    }))
                    .unwrap_or_else(|_| {
                        current = None;
                        Err(WallpaperError::Unavailable)
                    });
                    publish_issuer(
                        &mailbox,
                        result.as_ref().ok().and_then(|value| {
                            value
                                .as_ref()
                                .map(|snapshot| Issuer::Collection(snapshot.target.downgrade()))
                        }),
                    );
                    drop(flight);
                    finish(move || completion(result));
                }
                Work::ApplyCollection {
                    target,
                    options,
                    completion,
                } => {
                    current_slideshow.clear();
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        current
                            .take()
                            .and_then(|selection| match selection {
                                FileSelection::Collection(selection)
                                    if selection.target.matches(&target) =>
                                {
                                    Some(selection)
                                }
                                _ => None,
                            })
                            .ok_or(WallpaperError::InvalidTarget)
                            .and_then(|selection| selection.apply(options))
                    }))
                    .unwrap_or(Err(WallpaperError::Unavailable));
                    // Strong exact group ticket spans every SDK step/readback.
                    // A panic is unavailable with unknown effects, not rollback.
                    drop(target);
                    publish_issuer(&mailbox, None);
                    drop(flight);
                    finish(move || completion(result));
                }
                Work::ReadPosition { completion } => {
                    let result = catch_unwind(AssertUnwindSafe(|| current_position.read()))
                        .unwrap_or_else(|_| {
                            current_position.clear();
                            Err(WallpaperError::Unavailable)
                        });
                    publish_position_issuer(&mailbox, current_position.issuer());
                    drop(flight);
                    finish(move || completion(result));
                }
                Work::SetPosition {
                    target,
                    desired,
                    completion,
                } => {
                    let result =
                        catch_unwind(AssertUnwindSafe(|| current_position.set(target, desired)))
                            .unwrap_or_else(|_| {
                                current_position.clear();
                                Err(WallpaperError::Unavailable)
                            });
                    publish_position_issuer(&mailbox, current_position.issuer());
                    drop(flight);
                    finish(move || completion(result));
                }
                Work::ReadSlideshow { completion } => {
                    let result =
                        catch_unwind(AssertUnwindSafe(|| current_slideshow.read(&mailbox)))
                            .unwrap_or_else(|_| {
                                current_slideshow.clear();
                                Err(WallpaperError::Unavailable)
                            });
                    publish_slideshow_issuer(&mailbox, current_slideshow.issuer());
                    drop(flight);
                    finish(move || completion(result));
                }
                Work::AdvanceSlideshow {
                    target,
                    monitor,
                    direction,
                    completion,
                } => {
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        current_slideshow.advance(target, monitor, direction, &mailbox)
                    }))
                    .unwrap_or_else(|_| {
                        // A panic may follow a native effect. Unavailable never
                        // promises that no effect occurred or that it rolled back.
                        current_slideshow.clear();
                        Err(WallpaperError::Unavailable)
                    });
                    publish_slideshow_issuer(&mailbox, current_slideshow.issuer());
                    drop(flight);
                    finish(move || completion(result));
                }
            }
            continue;
        }
        if closed {
            break;
        }
        // SAFETY: stable owned event and this STA's message queue. No polling,
        // periodic observations, extra observer, respawn or GUI-thread SDK work.
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
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                if message.message == WM_QUIT {
                    drop(current);
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
    drop(current);
    fail_mailbox(&mailbox);
}

fn publish_issuer(mailbox: &Mailbox, target: Option<Issuer>) {
    let mut inbox = mailbox
        .inbox
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    inbox.issued = if inbox.closed || inbox.failed {
        None
    } else {
        target
    };
}

fn publish_position_issuer(mailbox: &Mailbox, target: Option<position_contract::Target>) {
    let mut inbox = mailbox
        .inbox
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    inbox.position_issued = if inbox.closed || inbox.failed {
        None
    } else {
        target
    };
}

fn publish_slideshow_issuer(mailbox: &Mailbox, target: Option<slideshow::Issuer>) {
    let mut inbox = mailbox
        .inbox
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    inbox.slideshow_issued = if inbox.closed || inbox.failed {
        None
    } else {
        target
    };
}

fn retirement_target(mailbox: &Arc<Mailbox>) -> WallpaperImageTarget {
    WallpaperImageTarget::with_retirement(retirement_callback(mailbox))
}

fn retirement_collection_target(mailbox: &Arc<Mailbox>) -> collection_contract::Target {
    collection_contract::Target::with_retirement(retirement_callback(mailbox))
}

fn retirement_callback(mailbox: &Arc<Mailbox>) -> impl FnOnce() + Send + Sync + 'static {
    let mailbox = Arc::downgrade(mailbox);
    move || {
        let Some(mailbox) = mailbox.upgrade() else {
            return;
        };
        let mut inbox = mailbox
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Bounded notification only. Native handles/COM drop on the STA when
        // it consumes this flag, independently of the request-slot capacity.
        inbox.retire_pending = true;
        if let Some(wake) = &inbox.wake {
            let _ = wake.signal();
        }
    }
}

fn fail_mailbox(mailbox: &Mailbox) {
    let job = {
        let mut inbox = mailbox
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inbox.failed = true;
        inbox.issued = None;
        inbox.position_issued = None;
        inbox.slideshow_issued = None;
        inbox.wake = None;
        inbox.job.take()
    };
    if let Some(job) = job {
        job.unavailable();
    }
}

fn finish(callback: impl FnOnce()) {
    // Callback reentry holds no issuer/mailbox lock; panic cannot strand work.
    let _ = catch_unwind(AssertUnwindSafe(callback));
}

/// One typed file slot: either choice replaces the other, never control state.
enum FileSelection {
    Image(Selection),
    Collection(collection::Collection),
}

impl FileSelection {
    fn is_alive(&self) -> bool {
        match self {
            Self::Image(selection) => selection.target.is_alive(),
            Self::Collection(selection) => selection.target.is_alive(),
        }
    }
}

struct Selection {
    target: WallpaperImageTargetWeak,
    image: source::Image,
    topology: displays::Topology,
    bindings: Vec<displays::Binding>,
}

impl Selection {
    fn choose(
        owner: HWND,
        mailbox: &Arc<Mailbox>,
    ) -> Result<Option<(Self, WallpaperSelection)>, WallpaperError> {
        let Some(image) = source::Image::choose(owner)? else {
            return Ok(None);
        };
        let desktop = displays::desktop()?;
        let topology = displays::Topology::capture(&desktop)?;
        let preview = image.preview()?;
        let _fresh_file = image.validate()?;
        // Tokens bind only to these original descriptors. Fresh topology
        // equality remains descriptor-only, never newly minted ticket equality.
        let (bindings, monitors) = topology.issue_members()?;
        let target = retirement_target(mailbox);
        let snapshot = WallpaperSelection {
            target: target.clone(),
            caption: image.caption.clone(),
            monitors,
            preview,
        };
        Ok(Some((
            Self {
                target: target.downgrade(),
                image,
                topology,
                bindings,
            },
            snapshot,
        )))
    }

    fn apply(self, scope: WallpaperApplyScope) -> Result<WallpaperApplyOutcome, WallpaperError> {
        // Resolve exact issued authority before even creating a fresh SDK
        // object. Scope never supplies a path, enumeration index or fallback.
        let bindings: Vec<_> = match scope {
            WallpaperApplyScope::AllCaptured => self.bindings.iter().collect(),
            WallpaperApplyScope::Monitor(target) => vec![
                self.bindings
                    .iter()
                    .find(|binding| binding.target == target)
                    .ok_or(WallpaperError::InvalidTarget)?,
            ],
        };
        let requested = bindings.len() as u32;
        let mut outcome = WallpaperApplyOutcome {
            requested,
            accepted: 0,
            confirmed: 0,
            failed: 0,
            not_submitted: requested,
        };
        // Keep partial SDK receipts even if a later native step becomes
        // unavailable or panics. No rollback, retry or rediscovered target.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let Ok(desktop) = displays::desktop() else {
                return;
            };
            for binding in bindings {
                let monitor = &binding.monitor;
                let Ok(_fresh_file) = self.image.validate() else {
                    break;
                };
                if !self.topology.matches(&desktop) {
                    break;
                }
                // SAFETY: original, nonempty native device ID and original
                // protected file path. Never null/global/all-monitor dispatch.
                let receipt = unsafe { desktop.SetWallpaper(monitor.id(), self.image.path()) };
                outcome.not_submitted -= 1;
                if receipt.is_err() {
                    outcome.failed += 1;
                    continue;
                }
                outcome.accepted += 1;
                let confirmed = unsafe { desktop.GetWallpaper(monitor.id()) }
                    .is_ok_and(|path| self.image.confirms(path));
                if confirmed && self.topology.matches(&desktop) && self.image.validate().is_ok() {
                    outcome.confirmed += 1;
                }
            }
        }));
        Ok(outcome)
    }
}
