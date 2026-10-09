// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shared OS-current media observation and single transport flight for two views.
//! Construction is inert; saved Dock enablement or an admitted visible popup starts observation.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use slint::ComponentHandle;
use tessera_system::media::{
    MediaAction as HostAction, MediaArtwork, MediaCommand, MediaError, MediaErrorKind, MediaEvent,
    MediaHost, MediaObservationRevision, MediaPlayback, MediaRequest, MediaSeekCommand,
    MediaSeekObservation, MediaSessionKey, MediaSnapshot, MediaTimeline,
};

use crate::generated::{Dock, DockMediaView, MediaAction, QuickSettings};
use crate::icons::IconCache;
use crate::image_mask::{Mask, cover_pixels};
use crate::{DesktopHost, PanelApplication, PixelIcon, sanitize};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Token {
    generation: u64,
    sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlightKind {
    Read,
    Command,
    Seek(PopupMediaToken),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Flight {
    token: Token,
    kind: FlightKind,
}

/// Presentation authority, never a native session or a persisted preference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PopupMediaToken(u64);

/// Immutable native authority for one admitted popup gesture; position is not authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CapturedMediaSeek {
    popup: PopupMediaToken,
    generation: u64,
    invalidation: u64,
    observation: u64,
    session: MediaSessionKey,
    revision: MediaObservationRevision,
    min_ticks: i64,
    max_ticks: i64,
}

impl CapturedMediaSeek {
    fn target_ticks(&self, fraction: f32) -> Option<i64> {
        if !fraction.is_finite() || self.min_ticks >= self.max_ticks {
            return None;
        }
        let span = i128::from(self.max_ticks) - i128::from(self.min_ticks);
        let offset = (span as f64 * f64::from(fraction.clamp(0.0, 1.0))).round() as i128;
        let target = (i128::from(self.min_ticks) + offset)
            .clamp(i128::from(self.min_ticks), i128::from(self.max_ticks));
        i64::try_from(target).ok()
    }

    /// Local preview relative to the captured seek range, never an observed position.
    pub(crate) fn preview_time(&self, fraction: f32) -> Option<String> {
        let target = self.target_ticks(fraction)?;
        Some(format!(
            "{} / {} · seek preview",
            ticks_label(i128::from(target) - i128::from(self.min_ticks)),
            ticks_label(i128::from(self.max_ticks) - i128::from(self.min_ticks)),
        ))
    }
}

struct PendingSeek {
    scope: CapturedMediaSeek,
    position_ticks: i64,
}

const SEEK_INTERVAL: Duration = Duration::from_millis(200);

struct PopupProjection {
    token: PopupMediaToken,
    view: slint::Weak<QuickSettings>,
    admission: Rc<dyn Fn() -> bool>,
}

#[derive(Default)]
struct State {
    generation: u64,
    sequence: u64,
    enabled: bool,
    popup_sequence: u64,
    popup: Option<PopupProjection>,
    provider: Option<Arc<dyn MediaHost>>,
    watch: Option<Token>,
    watch_guard: Option<Box<dyn Send>>,
    watch_attempted: bool,
    watch_ready: bool,
    flight: Option<Flight>,
    flight_accepted: bool,
    read_needed: bool,
    dirty: bool,
    snapshot: Option<MediaSnapshot>,
    read_error: Option<MediaError>,
    watch_error: Option<MediaError>,
    action_error: Option<MediaError>,
    action_accepted: bool,
    seek_epoch: u64,
    read_seek_epoch: u64,
    seek_observation: u64,
    seek_observation_exhausted: bool,
    pending_seek: Option<PendingSeek>,
    last_seek_accepted: Option<Instant>,
    seek_error: Option<MediaError>,
}

impl State {
    fn observation_active(&self) -> bool {
        self.enabled || self.popup.is_some()
    }

    fn token(&mut self) -> Option<Token> {
        self.sequence = self.sequence.checked_add(1)?;
        Some(Token {
            generation: self.generation,
            sequence: self.sequence,
        })
    }

    fn invalidate_seek_observation(&mut self) {
        if let Some(next) = self.seek_observation.checked_add(1) {
            self.seek_observation = next;
        } else {
            self.seek_observation_exhausted = true;
        }
    }
}

fn seek_authority(
    snapshot: Option<&MediaSnapshot>,
) -> Option<(MediaSessionKey, MediaSeekObservation)> {
    let session = snapshot?.current.as_ref()?;
    Some((session.key, *session.seek.as_ref().ok()?))
}

enum Completion {
    Read(Token, Result<Box<MediaSnapshot>, MediaError>),
    Command(Token, Result<(), MediaError>),
    Seek(Token, PopupMediaToken, Result<(), MediaError>),
}

impl Completion {
    fn flight(&self) -> Flight {
        match self {
            Self::Read(token, _) => Flight {
                token: *token,
                kind: FlightKind::Read,
            },
            Self::Command(token, _) => Flight {
                token: *token,
                kind: FlightKind::Command,
            },
            Self::Seek(token, popup, _) => Flight {
                token: *token,
                kind: FlightKind::Seek(*popup),
            },
        }
    }
}

/// Bounded Send-only slots. Neither an Rc, native interface nor Slint Image crosses threads.
#[derive(Default)]
struct Mailbox {
    generation: u64,
    enabled: bool,
    expected: Option<Flight>,
    completion: Option<Completion>,
    watch: Option<Token>,
    watch_health: Option<Result<(), MediaError>>,
    dirty: bool,
    seek_epoch: u64,
    seek_exhausted: bool,
    seek_invalidated: bool,
    wake_queued: bool,
}

impl Mailbox {
    fn pending(&self) -> bool {
        self.completion.is_some()
            || self.watch_health.is_some()
            || self.dirty
            || self.seek_invalidated
    }
}

/// Event-loop delivery seam, independent of either presentation's admission.
/// This desktop composition retains its Dock window even when its tile is off.
#[derive(Clone)]
struct MediaWake {
    destination: slint::Weak<Dock>,
}

impl MediaWake {
    fn enqueue(&self, pending: impl FnOnce() -> bool + Send + 'static) -> bool {
        self.destination
            .upgrade_in_event_loop(move |dock| {
                if pending() {
                    dock.invoke_media_event_ready();
                }
            })
            .is_ok()
    }
}

fn publish(
    mailbox: &Arc<Mutex<Mailbox>>,
    wake: &MediaWake,
    generation: u64,
    update: impl FnOnce(&mut Mailbox) -> bool,
) {
    {
        let mut mailbox = mailbox.lock();
        if !mailbox.enabled
            || (mailbox.generation != generation
                && mailbox
                    .expected
                    .is_none_or(|flight| flight.token.generation != generation))
            || !update(&mut mailbox)
            || mailbox.wake_queued
        {
            return;
        }
        mailbox.wake_queued = true;
    }
    let weak_mailbox = Arc::downgrade(mailbox);
    if !wake.enqueue(move || {
        let Some(mailbox) = weak_mailbox.upgrade() else {
            return false;
        };
        let mut mailbox = mailbox.lock();
        let current = mailbox.enabled && mailbox.wake_queued && mailbox.pending();
        mailbox.wake_queued = false;
        current
    }) {
        // The no-event-loop test backend drains this production mailbox explicitly.
        let mut mailbox = mailbox.lock();
        if mailbox.generation == generation {
            mailbox.wake_queued = false;
        }
    }
}

fn complete(mailbox: &Arc<Mutex<Mailbox>>, wake: &MediaWake, result: Completion) {
    let flight = result.flight();
    publish(mailbox, wake, flight.token.generation, |mailbox| {
        if mailbox.expected != Some(flight) || mailbox.completion.is_some() {
            return false;
        }
        mailbox.completion = Some(result);
        true
    });
}

fn watch_event(mailbox: &Arc<Mutex<Mailbox>>, wake: &MediaWake, token: Token, event: MediaEvent) {
    publish(mailbox, wake, token.generation, |mailbox| {
        if mailbox.watch != Some(token) {
            return false;
        }
        match event {
            MediaEvent::Changed => mailbox.dirty = true,
            MediaEvent::SeekInvalidated => {
                mailbox.seek_invalidated = true;
                mailbox.dirty = true;
                if let Some(epoch) = mailbox.seek_epoch.checked_add(1) {
                    mailbox.seek_epoch = epoch;
                } else {
                    mailbox.seek_exhausted = true;
                }
            }
            MediaEvent::WatchReady => mailbox.watch_health = Some(Ok(())),
            MediaEvent::WatchUnavailable(error) => mailbox.watch_health = Some(Err(error)),
        }
        true
    });
}

struct Driving<'a>(&'a Cell<bool>);
impl Drop for Driving<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

pub(crate) struct DockMediaController {
    host: Arc<dyn DesktopHost>,
    dock: slint::Weak<Dock>,
    wake: MediaWake,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Mailbox>>,
    icons: RefCell<IconCache>,
    application_icons: RefCell<HashMap<String, PixelIcon>>,
    projection: RefCell<Rc<()>>,
    popup_artwork: RefCell<Option<(MediaArtwork, slint::Image)>>,
    driving: Cell<bool>,
    seek_timer: slint::Timer,
    self_weak: std::rc::Weak<Self>,
    projected_seek: RefCell<Option<CapturedMediaSeek>>,
    #[cfg(test)]
    seek_clock: Cell<Instant>,
    closed: Cell<bool>,
}

impl DockMediaController {
    pub(crate) fn new(host: Arc<dyn DesktopHost>, dock: &Dock) -> Rc<Self> {
        Rc::new_cyclic(|weak| Self {
            host,
            dock: dock.as_weak(),
            wake: MediaWake {
                destination: dock.as_weak(),
            },
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
            icons: RefCell::default(),
            application_icons: RefCell::default(),
            projection: RefCell::default(),
            popup_artwork: RefCell::default(),
            seek_timer: slint::Timer::default(),
            self_weak: weak.clone(),
            projected_seek: RefCell::default(),
            #[cfg(test)]
            seek_clock: Cell::new(Instant::now()),
            driving: Cell::new(false),
            closed: Cell::new(false),
        })
    }

    /// Saved preference adoption affects only Dock presentation, not popup demand.
    pub(crate) fn set_enabled(&self, enabled: bool) {
        if self.closed.get()
            || self.state.borrow().enabled == enabled
            || (enabled
                && !self.state.borrow().observation_active()
                && self.state.borrow().generation == u64::MAX)
        {
            return;
        }
        let was_active = self.state.borrow().observation_active();
        self.state.borrow_mut().enabled = enabled;
        self.demand_changed(was_active);
    }

    #[cfg(test)]
    pub(crate) fn attach_popup(&self, popup: &QuickSettings) -> Option<PopupMediaToken> {
        self.attach_popup_scoped(popup, Rc::new(|| true), |_| true)
    }

    /// Install exact presentation authority before any callback-capable projection
    /// or provider call, so central hide can retire a still-pending attachment.
    pub(crate) fn attach_popup_scoped(
        &self,
        popup: &QuickSettings,
        admission: Rc<dyn Fn() -> bool>,
        installed: impl FnOnce(PopupMediaToken) -> bool,
    ) -> Option<PopupMediaToken> {
        let prior_attachment = self.state.borrow().popup_sequence;
        if self.closed.get() || !popup.window().is_visible() || !admission() {
            return None;
        }
        if self.closed.get() || self.state.borrow().popup_sequence != prior_attachment {
            return None;
        }
        let (was_active, token, retired) = {
            let mut state = self.state.borrow_mut();
            if !state.observation_active() && state.generation == u64::MAX {
                return None;
            }
            let sequence = state.popup_sequence.checked_add(1)?;
            let was_active = state.observation_active();
            let token = PopupMediaToken(sequence);
            state.popup_sequence = sequence;
            state.pending_seek = None;
            state.seek_error = None;
            let retired = state.popup.replace(PopupProjection {
                token,
                view: popup.as_weak(),
                admission: Rc::clone(&admission),
            });
            (was_active, token, retired)
        };
        self.seek_timer.stop();
        drop(retired);
        if !installed(token)
            || self.closed.get()
            || !admission()
            || !self
                .state
                .borrow()
                .popup
                .as_ref()
                .is_some_and(|popup| popup.token == token)
        {
            self.detach_popup(token);
            return None;
        }
        self.demand_changed(was_active);
        Some(token)
    }

    pub(crate) fn detach_popup(&self, token: PopupMediaToken) {
        let (was_active, retired) = {
            let mut state = self.state.borrow_mut();
            if state.popup.as_ref().map(|popup| popup.token) != Some(token) {
                return;
            }
            state.pending_seek = None;
            state.seek_error = None;
            let was_active = state.observation_active();
            (was_active, state.popup.take())
        };
        self.seek_timer.stop();
        if let Some(retired) = &retired {
            self.clear_retired_popup(retired);
        }
        self.demand_changed(was_active);
        drop(retired);
    }

    /// A source guard is UI-thread-only; backend completions retain neither it
    /// nor Root. Revocation retires this popup only, never saved Dock demand.
    fn retire_invalid_popup(&self) {
        let captured = self
            .state
            .borrow()
            .popup
            .as_ref()
            .map(|popup| (popup.token, popup.view.clone(), Rc::clone(&popup.admission)));
        if let Some((token, view, admission)) = captured
            && (!view
                .upgrade()
                .is_some_and(|view| view.window().is_visible())
                || !admission())
        {
            self.detach_popup(token);
        }
    }

    pub(crate) fn popup_input_ready(&self, token: PopupMediaToken) -> bool {
        self.retire_invalid_popup();
        !self.closed.get()
            && self
                .state
                .borrow()
                .popup
                .as_ref()
                .is_some_and(|popup| popup.token == token)
    }

    pub(crate) fn capture_seek(&self, token: PopupMediaToken) -> Option<CapturedMediaSeek> {
        if !self.popup_input_ready(token) {
            return None;
        }
        let state = self.state.borrow();
        let mailbox = self.mailbox.lock();
        if state.read_error.is_some()
            || mailbox.seek_exhausted
            || state.seek_observation_exhausted
            || state.seek_epoch != mailbox.seek_epoch
        {
            return None;
        }
        let session = state.snapshot.as_ref()?.current.as_ref()?;
        let observation = session.seek.as_ref().ok()?;
        if observation.min_ticks >= observation.max_ticks {
            return None;
        }
        Some(CapturedMediaSeek {
            popup: token,
            generation: state.generation,
            invalidation: state.seek_epoch,
            observation: state.seek_observation,
            session: session.key,
            revision: observation.revision,
            min_ticks: observation.min_ticks,
            max_ticks: observation.max_ticks,
        })
    }

    pub(crate) fn seek_scope_current(&self, scope: &CapturedMediaSeek) -> bool {
        self.capture_seek(scope.popup).as_ref() == Some(scope)
    }

    pub(crate) fn cancel_pending_seek(&self, token: PopupMediaToken) {
        let mut state = self.state.borrow_mut();
        if state
            .pending_seek
            .as_ref()
            .is_some_and(|pending| pending.scope.popup == token)
        {
            state.pending_seek = None;
            self.seek_timer.stop();
        }
    }

    fn seek_now(&self) -> Instant {
        #[cfg(test)]
        {
            self.seek_clock.get()
        }
        #[cfg(not(test))]
        {
            Instant::now()
        }
    }

    #[cfg(test)]
    pub(crate) fn elapse_seek_throttle(&self, elapsed: Duration) {
        if let Some(now) = self.seek_clock.get().checked_add(elapsed) {
            self.seek_clock.set(now);
        }
    }

    pub(crate) fn seek_from_popup(&self, scope: &CapturedMediaSeek, fraction: f32) {
        if !fraction.is_finite() || !self.seek_scope_current(scope) {
            let same = self
                .state
                .borrow()
                .pending_seek
                .as_ref()
                .is_some_and(|pending| pending.scope == *scope);
            if same {
                self.cancel_pending_seek(scope.popup);
            }
            return;
        }
        let Some(position_ticks) = scope.target_ticks(fraction) else {
            return;
        };
        self.state.borrow_mut().pending_seek = Some(PendingSeek {
            scope: scope.clone(),
            position_ticks,
        });
        self.drive_reads();
    }

    /// One latest intention shares the observation/transport flight. A trailing
    /// timer is armed only while idle; busy completion resumes the same scheduler.
    fn drive_seek_at(&self, now: Instant) {
        self.seek_timer.stop();
        let scope = self
            .state
            .borrow()
            .pending_seek
            .as_ref()
            .map(|pending| pending.scope.clone());
        let Some(scope) = scope else {
            return;
        };
        if !self.seek_scope_current(&scope) {
            self.cancel_pending_seek(scope.popup);
            return;
        }
        let delay = {
            let state = self.state.borrow();
            if state.flight.is_some() || state.read_needed {
                return;
            }
            state
                .last_seek_accepted
                .and_then(|last| SEEK_INTERVAL.checked_sub(now.saturating_duration_since(last)))
        };
        if let Some(delay) = delay.filter(|delay| !delay.is_zero()) {
            let weak = self.self_weak.clone();
            self.seek_timer
                .start(slint::TimerMode::SingleShot, delay, move || {
                    if let Some(controller) = weak.upgrade() {
                        controller.drive_reads();
                    }
                });
            return;
        }
        let (flight, command, provider) = {
            let mut state = self.state.borrow_mut();
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let Some(token) = state.token() else {
                state.pending_seek = None;
                return;
            };
            let Some(pending) = state.pending_seek.take() else {
                return;
            };
            let flight = Flight {
                token,
                kind: FlightKind::Seek(scope.popup),
            };
            state.flight = Some(flight);
            state.seek_error = None;
            let command = MediaSeekCommand {
                expected_session: scope.session,
                expected_revision: scope.revision,
                observed_min_ticks: scope.min_ticks,
                observed_max_ticks: scope.max_ticks,
                position_ticks: pending.position_ticks,
            };
            (flight, command, provider)
        };
        {
            let mut mailbox = self.mailbox.lock();
            mailbox.expected = Some(flight);
            mailbox.completion = None;
        }
        self.project();
        if !self.current(flight) || !self.seek_scope_current(&scope) {
            self.cancel_unsubmitted(flight);
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.flight_accepted = true;
        }
        let mailbox = self.mailbox.clone();
        let wake = self.wake.clone();
        let popup = scope.popup;
        let result = provider.execute(
            MediaRequest::Seek(command),
            Box::new(move |result| {
                complete(
                    &mailbox,
                    &wake,
                    Completion::Seek(flight.token, popup, result),
                );
            }),
        );
        match result {
            Ok(()) => self.state.borrow_mut().last_seek_accepted = Some(self.seek_now()),
            Err(error) => complete(
                &self.mailbox,
                &self.wake,
                Completion::Seek(flight.token, popup, Err(error)),
            ),
        }
    }

    fn clear_retired_popup(&self, retired: &PopupProjection) {
        let Some(view) = retired.view.upgrade() else {
            return;
        };
        let current = || {
            !self.state.borrow().popup.as_ref().is_some_and(|popup| {
                popup
                    .view
                    .upgrade()
                    .is_some_and(|new| std::ptr::eq(new.window(), view.window()))
            })
        };
        if !current() {
            return;
        }
        view.invoke_cancel_media_input();
        if !current() {
            return;
        }
        view.set_media_view(DockMediaView::default());
        if !current() {
            return;
        }
        view.set_timeline_available(false);
        if !current() {
            return;
        }
        view.set_timeline_time(Default::default());
        if !current() {
            return;
        }
        view.set_timeline_notice(Default::default());
        if !current() {
            return;
        }
        view.set_timeline_progress(0.0);
        if !current() {
            return;
        }
        view.set_seek_enabled(false);
        if !current() {
            return;
        }
        view.set_seek_progress(0.0);
        if !current() {
            return;
        }
        view.set_seek_notice(Default::default());
        if current() {
            view.invoke_project_seek();
        }
    }

    fn demand_changed(&self, was_active: bool) {
        let retired = {
            let mut state = self.state.borrow_mut();
            let active = state.observation_active();
            if active == was_active {
                None
            } else {
                // Exhaustion refuses future activation; retirement itself still succeeds.
                let generation = state.generation.checked_add(1).unwrap_or(state.generation);
                state.generation = generation;
                state.snapshot = None;
                state.read_error = None;
                state.watch_error = None;
                state.action_error = None;
                state.action_accepted = false;
                state.pending_seek = None;
                state.seek_error = None;
                state.watch = None;
                state.watch_attempted = false;
                state.watch_ready = false;
                state.read_needed = active;
                state.dirty = active;
                if !state.flight_accepted {
                    state.flight = None;
                }
                let provider = if state.flight.is_none() {
                    state.provider.take()
                } else {
                    None
                };
                let mut mailbox = self.mailbox.lock();
                mailbox.generation = generation;
                mailbox.enabled = active || state.flight.is_some();
                mailbox.expected = state.flight;
                if state.flight.is_none() {
                    mailbox.completion = None;
                }
                mailbox.watch = None;
                mailbox.watch_health = None;
                mailbox.dirty = false;
                Some((state.watch_guard.take(), provider))
            }
        };
        drop(retired);
        self.project();
        self.drive_reads();
    }

    pub(crate) fn request_from_popup(
        &self,
        token: PopupMediaToken,
        action: MediaAction,
        identity: &str,
    ) {
        self.retire_invalid_popup();
        let admitted = {
            let state = self.state.borrow();
            state.popup.as_ref().is_some_and(|popup| {
                popup.token == token
                    && popup
                        .view
                        .upgrade()
                        .is_some_and(|view| view.window().is_visible())
            }) && state
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.current.as_ref())
                .is_some_and(|session| identity == session_identity(&state, session.key))
        };
        if !admitted || self.closed.get() {
            return;
        }
        self.cancel_pending_seek(token);
        if self.driving.replace(true) {
            return;
        }
        {
            let _driving = Driving(&self.driving);
            self.dispatch_transport(action, Some(token));
        }
        self.drive_reads();
    }

    pub(crate) fn request(&self, action: MediaAction) {
        if self.closed.get() || self.driving.replace(true) {
            return;
        }
        {
            let _driving = Driving(&self.driving);
            self.dispatch(action);
        }
        // A host may synchronously disable/re-enable the module while dispatching.
        self.drive_reads();
    }

    fn dispatch(&self, action: MediaAction) {
        let Some(dock) = self.dock.upgrade() else {
            return;
        };
        if !dock.window().is_visible() {
            return;
        }
        if !self.state.borrow().enabled {
            return;
        }
        self.dispatch_transport(action, None);
    }

    fn dispatch_transport(&self, action: MediaAction, popup: Option<PopupMediaToken>) {
        self.retire_invalid_popup();
        // Transport intentions supersede unsubmitted seek input even while busy.
        self.state.borrow_mut().pending_seek = None;
        self.seek_timer.stop();
        // A native invalidation already in the mailbox also blocks stale GUI input.
        if self.mailbox.lock().dirty {
            return;
        }
        let action = match action {
            MediaAction::Previous => HostAction::Previous,
            MediaAction::Toggle => HostAction::Toggle,
            MediaAction::Next => HostAction::Next,
        };
        let (flight, command, provider) = {
            let mut state = self.state.borrow_mut();
            if !state.observation_active()
                || state.flight.is_some()
                || state.dirty
                || state.read_error.is_some()
            {
                return;
            }
            let Some(session) = state
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.current.as_ref())
            else {
                return;
            };
            if !session.capabilities.allows(action) {
                return;
            }
            let key = session.key;
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let Some(token) = state.token() else {
                return;
            };
            let flight = Flight {
                token,
                kind: FlightKind::Command,
            };
            state.flight = Some(flight);
            state.action_error = None;
            state.action_accepted = false;
            (
                flight,
                MediaCommand {
                    expected_session: key,
                    action,
                },
                provider,
            )
        };
        {
            let mut mailbox = self.mailbox.lock();
            mailbox.expected = Some(flight);
            mailbox.completion = None;
        }
        self.project();
        if !self.current(flight) {
            self.cancel_unsubmitted(flight);
            return;
        }
        let source_current = match popup {
            Some(token) => self.popup_input_ready(token),
            None => {
                self.state.borrow().enabled
                    && self
                        .dock
                        .upgrade()
                        .is_some_and(|dock| dock.window().is_visible())
            }
        };
        if !source_current || self.mailbox.lock().dirty {
            self.cancel_unsubmitted(flight);
            return;
        }
        self.state.borrow_mut().flight_accepted = true;
        let mailbox = self.mailbox.clone();
        let wake = self.wake.clone();
        if let Err(error) = provider.execute(
            MediaRequest::Transport(command),
            Box::new(move |result| {
                complete(&mailbox, &wake, Completion::Command(flight.token, result));
            }),
        ) {
            complete(
                &self.mailbox,
                &self.wake,
                Completion::Command(flight.token, Err(error)),
            );
        }
    }

    fn cancel_unsubmitted(&self, flight: Flight) {
        {
            let mut state = self.state.borrow_mut();
            if state.flight != Some(flight) || state.flight_accepted {
                return;
            }
            state.flight = None;
            let mut mailbox = self.mailbox.lock();
            mailbox.expected = None;
            mailbox.completion = None;
        }
        self.project();
    }

    /// Retry observes only. It never repeats a previous transport request.
    pub(crate) fn retry(&self) {
        self.retire_invalid_popup();
        if self.closed.get() {
            return;
        }
        let retired = {
            let mut state = self.state.borrow_mut();
            if !state.observation_active() || state.flight.is_some() {
                return;
            }
            state.read_needed = true;
            state.dirty = true;
            if state.watch_error.is_some() {
                state.watch_attempted = false;
                state.watch = None;
                let mut mailbox = self.mailbox.lock();
                mailbox.watch = None;
                mailbox.watch_health = None;
                state.watch_guard.take()
            } else {
                None
            }
        };
        drop(retired);
        self.project();
        self.drive_reads();
    }

    pub(crate) fn process_events(&self) {
        if self.closed.get() || self.driving.replace(true) {
            return;
        }
        {
            let _driving = Driving(&self.driving);
            self.retire_invalid_popup();
            let (completion, health, dirty, seek_invalidated) = {
                let mut mailbox = self.mailbox.lock();
                mailbox.wake_queued = false;
                (
                    mailbox.completion.take(),
                    mailbox.watch_health.take(),
                    std::mem::take(&mut mailbox.dirty),
                    std::mem::take(&mut mailbox.seek_invalidated),
                )
            };
            let mut state = self.state.borrow_mut();
            let active = state.observation_active();
            if let Some(health) = health {
                state.watch_ready = health.is_ok();
                state.watch_error = health.err();
            }
            if dirty {
                state.dirty = true;
                state.read_needed = true;
            }
            if seek_invalidated {
                state.pending_seek = None;
                self.seek_timer.stop();
            }
            if let Some(completion) = completion {
                let flight = completion.flight();
                if state.flight == Some(flight) {
                    state.flight = None;
                    state.flight_accepted = false;
                    self.mailbox.lock().expected = None;
                    if !active || flight.token.generation != state.generation {
                        let provider = if !active {
                            self.mailbox.lock().enabled = false;
                            state.provider.take()
                        } else {
                            None
                        };
                        drop(state);
                        drop(provider);
                        self.project();
                        drop(_driving);
                        self.drive_reads();
                        return;
                    }
                    match completion {
                        Completion::Read(_, Ok(snapshot)) => {
                            let snapshot = *snapshot;
                            let snapshot = MediaSnapshot {
                                current: snapshot.current.map(|session| session.bounded()),
                            };
                            if seek_authority(state.snapshot.as_ref())
                                != seek_authority(Some(&snapshot))
                            {
                                state.invalidate_seek_observation();
                            }
                            state.snapshot = Some(snapshot);
                            state.read_error = None;
                            state.seek_epoch = state.read_seek_epoch;
                            // Invalidations delivered during the read require one follow-up.
                            state.dirty = dirty || state.read_needed;
                        }
                        Completion::Read(_, Err(error)) => {
                            state.invalidate_seek_observation();
                            state.read_error = Some(error);
                            state.dirty = true;
                            // Failure is not an automatic idle retry loop.
                            state.read_needed = false;
                        }
                        Completion::Command(_, result) => {
                            state.action_accepted = result.is_ok();
                            state.action_error = result.err();
                            state.dirty = true;
                            state.read_needed = true;
                        }
                        Completion::Seek(_, popup, result) => {
                            let accepted = result.is_ok();
                            if state
                                .popup
                                .as_ref()
                                .is_some_and(|active| active.token == popup)
                            {
                                state.seek_error = result.err();
                            }
                            if accepted {
                                state.dirty = true;
                                state.read_needed = true;
                            }
                            drop(state);
                            if self.popup_input_ready(popup) {
                                let view = self
                                    .state
                                    .borrow()
                                    .popup
                                    .as_ref()
                                    .and_then(|popup| popup.view.upgrade());
                                if let Some(view) = view {
                                    view.invoke_media_seek_finished();
                                }
                            }
                            self.project();
                            drop(_driving);
                            self.drive_reads();
                            return;
                        }
                    }
                }
            }
        }
        self.project();
        self.drive_reads();
    }

    /// Only exact trusted catalog identities supply an overlay. Never a launch request.
    pub(crate) fn update_applications(&self, applications: &[PanelApplication]) {
        if self.closed.get() {
            return;
        }
        *self.application_icons.borrow_mut() = applications
            .iter()
            .take(256)
            .filter_map(|application| {
                application
                    .icon()
                    .map(|icon| (application.key().to_owned(), icon.clone()))
            })
            .collect();
        self.project();
    }

    pub(crate) fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        let retired = {
            let mut state = self.state.borrow_mut();
            state.enabled = false;
            // Take callback-owning presentation state outside the borrow.
            let popup = state.popup.take();
            state.pending_seek = None;
            state.flight = None;
            state.watch = None;
            state.snapshot = None;
            (state.watch_guard.take(), state.provider.take(), popup)
        };
        *self.mailbox.lock() = Mailbox::default();
        self.seek_timer.stop();
        self.project();
        drop(retired);
    }

    fn current(&self, flight: Flight) -> bool {
        self.retire_invalid_popup();
        let state = self.state.borrow();
        !self.closed.get()
            && state.observation_active()
            && state.generation == flight.token.generation
            && state.flight == Some(flight)
    }

    fn drive_reads(&self) {
        if self.closed.get() || self.driving.replace(true) {
            return;
        }
        let _driving = Driving(&self.driving);
        loop {
            self.retire_invalid_popup();
            let (flight, provider) = {
                let mut state = self.state.borrow_mut();
                if !state.observation_active() || !state.read_needed || state.flight.is_some() {
                    break;
                }
                let Some(token) = state.token() else {
                    break;
                };
                let flight = Flight {
                    token,
                    kind: FlightKind::Read,
                };
                state.flight = Some(flight);
                state.read_needed = false;
                (flight, state.provider.clone())
            };
            {
                let mut mailbox = self.mailbox.lock();
                mailbox.expected = Some(flight);
                mailbox.completion = None;
                // Earlier invalidation is covered by this read, not by a later completion.
                mailbox.dirty = false;
                self.state.borrow_mut().read_seek_epoch = mailbox.seek_epoch;
            }
            self.project();
            if !self.current(flight) {
                continue;
            }
            let provider = match provider {
                Some(provider) => provider,
                None => match self.host.media_host().and_then(|provider| {
                    provider.ok_or_else(|| {
                        MediaError::new(
                            MediaErrorKind::Unsupported,
                            "Media playback is not supported by this host.",
                        )
                    })
                }) {
                    Ok(provider) => {
                        if !self.current(flight) {
                            continue;
                        }
                        self.state.borrow_mut().provider = Some(provider.clone());
                        provider
                    }
                    Err(error) => {
                        if !self.current(flight) {
                            continue;
                        }
                        complete(
                            &self.mailbox,
                            &self.wake,
                            Completion::Read(flight.token, Err(error)),
                        );
                        break;
                    }
                },
            };
            if !self.current(flight) {
                continue;
            }
            self.ensure_watch(flight, &provider);
            if !self.current(flight) {
                continue;
            }
            let mailbox = self.mailbox.clone();
            self.state.borrow_mut().flight_accepted = true;
            let wake = self.wake.clone();
            if let Err(error) = provider.read(Box::new(move |result| {
                complete(
                    &mailbox,
                    &wake,
                    Completion::Read(flight.token, result.map(Box::new)),
                );
            })) {
                complete(
                    &self.mailbox,
                    &self.wake,
                    Completion::Read(flight.token, Err(error)),
                );
            }
            if self.current(flight) {
                break;
            }
        }
        self.drive_seek_at(self.seek_now());
    }

    fn ensure_watch(&self, flight: Flight, provider: &Arc<dyn MediaHost>) {
        let token = {
            let mut state = self.state.borrow_mut();
            if state.watch_attempted {
                return;
            }
            let Some(token) = state.token() else {
                return;
            };
            state.watch_attempted = true;
            state.watch = Some(token);
            state.watch_ready = false;
            state.watch_error = None;
            self.mailbox.lock().watch = Some(token);
            token
        };
        let mailbox = self.mailbox.clone();
        let wake = self.wake.clone();
        let result = provider.subscribe(Arc::new(move |event| {
            watch_event(&mailbox, &wake, token, event);
        }));
        if !self.current(flight) {
            drop(result);
            return;
        }
        match result {
            Ok(Some(guard)) => self.state.borrow_mut().watch_guard = Some(guard),
            Ok(None) => {
                self.state.borrow_mut().watch = None;
                let mut mailbox = self.mailbox.lock();
                mailbox.watch = None;
                mailbox.watch_health = None;
                self.state.borrow_mut().watch_error = Some(MediaError::new(
                    MediaErrorKind::WatchUnavailable,
                    "Live media updates are unavailable; use Retry to refresh.",
                ));
            }
            Err(error) => {
                self.state.borrow_mut().watch = None;
                let mut mailbox = self.mailbox.lock();
                mailbox.watch = None;
                mailbox.watch_health = None;
                self.state.borrow_mut().watch_error = Some(error);
            }
        }
    }

    /// One accepted artwork content owns one prepared popup image. Raw Dock
    /// pixels remain unchanged; refit/theme/timeline observations reuse this slot.
    fn popup_artwork_image(&self, artwork: &MediaArtwork) -> slint::Image {
        let mut cached = self.popup_artwork.borrow_mut();
        if let Some((source, image)) = cached.as_ref()
            && source == artwork
        {
            return image.clone();
        }
        let image = slint::Image::from_rgba8_premultiplied(cover_pixels(
            artwork.width(),
            artwork.height(),
            artwork.rgba(),
            512,
            Mask::RoundedSquare {
                radius_fraction: 6.0 / 40.0,
            },
        ));
        *cached = Some((artwork.clone(), image.clone()));
        image
    }

    fn project(&self) {
        self.retire_invalid_popup();
        let revision = Rc::new(());
        self.projection.replace(Rc::clone(&revision));
        let dock = self.dock.upgrade();
        let view = {
            let state = self.state.borrow();
            let current = state
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.current.as_ref());
            let busy = state.flight.is_some();
            let stale = state.dirty || state.read_error.is_some();
            let controls = !busy && !stale;
            let status = if !state.observation_active() {
                ""
            } else if state.read_error.is_some() {
                "Media unavailable"
            } else if current.is_some() {
                ""
            } else if busy {
                "Loading media"
            } else if state.snapshot.is_some() {
                "Not playing"
            } else {
                "Media not yet observed"
            };
            let mut view = DockMediaView {
                enabled: state.enabled,
                current_present: current.is_some(),
                busy,
                stale,
                status: status.into(),
                read_notice: notice(state.read_error.as_ref()).into(),
                watch_notice: if state.watch.is_some()
                    && !state.watch_ready
                    && state.watch_error.is_none()
                {
                    "Live media updates are starting.".into()
                } else {
                    notice(state.watch_error.as_ref()).into()
                },
                action_notice: if matches!(
                    state.flight,
                    Some(Flight {
                        kind: FlightKind::Command,
                        ..
                    })
                ) {
                    "Playback request pending.".into()
                } else if state.action_accepted {
                    "Playback request accepted.".into()
                } else {
                    notice(state.action_error.as_ref()).into()
                },
                ..DockMediaView::default()
            };
            if let Some(session) = current {
                // Checked operation epochs cancel held input even if busy clears before
                // the binding is observed. Commands still use only the typed session key.
                view.session_identity = session_identity(&state, session.key).into();
                view.title = sanitize::bounded_text(&session.title, 512).into();
                view.author = sanitize::bounded_text(&session.author, 512).into();
                view.playback = playback_label(session.playback).into();
                view.playing = session.playback == MediaPlayback::Playing;
                view.previous_enabled = controls && session.capabilities.previous;
                view.toggle_enabled = controls && session.capabilities.toggle;
                view.next_enabled = controls && session.capabilities.next;
                view.artwork_notice = notice(session.artwork_notice.as_ref()).into();
                if let Some(artwork) = &session.artwork
                    && let Some(icon) =
                        PixelIcon::new(artwork.width(), artwork.height(), artwork.rgba().to_vec())
                {
                    view.artwork = self.icons.borrow_mut().image(&icon);
                    view.has_artwork = true;
                    view.dark_foreground = artwork_dark_foreground(artwork);
                }
                if let Some(icon) = self.application_icons.borrow().get(&session.source_app_id) {
                    view.app_icon = self.icons.borrow_mut().image(icon);
                    view.has_app_icon = true;
                }
            }
            view
        };
        // Capture attachment before callback-capable setters; an old projection
        // must not overwrite a popup opened synchronously by a Dock setter.
        let popup = self
            .state
            .borrow()
            .popup
            .as_ref()
            .map(|popup| (popup.token, popup.view.clone()));
        let seek_scope = popup
            .as_ref()
            .and_then(|(token, _)| self.capture_seek(*token));
        if !Rc::ptr_eq(&revision, &self.projection.borrow()) {
            return;
        }
        let old_seek = self.projected_seek.replace(seek_scope.clone());
        let seek_invalidated = old_seek.as_ref().is_some_and(|old| {
            old_seek != seek_scope && seek_scope.as_ref().is_none_or(|new| new.popup == old.popup)
        });
        if seek_invalidated && let Some(old) = old_seek {
            self.cancel_pending_seek(old.popup);
        }
        let (seek_progress, seek_notice) = {
            let state = self.state.borrow();
            let session = state
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.current.as_ref());
            let progress = seek_scope
                .as_ref()
                .zip(session.and_then(|session| session.timeline.as_ref().ok()))
                .map(|(scope, timeline)| {
                    let span = i128::from(scope.max_ticks) - i128::from(scope.min_ticks);
                    let elapsed = i128::from(timeline.position_ticks) - i128::from(scope.min_ticks);
                    (elapsed as f64 / span as f64).clamp(0.0, 1.0) as f32
                })
                .unwrap_or(0.0);
            let observed_error = session.and_then(|session| session.seek.as_ref().err());
            let message = if let Some(error) = state.seek_error.as_ref().or(observed_error) {
                notice(Some(error))
            } else if session.is_some() && seek_scope.is_none() {
                "Seeking unavailable for the observed range.".into()
            } else {
                String::new()
            };
            (progress, message)
        };
        let timeline = self
            .state
            .borrow()
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.current.as_ref())
            .map(|session| session.timeline.clone());
        let popup_artwork = if popup.is_some() {
            self.state
                .borrow()
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.current.as_ref())
                .and_then(|session| session.artwork.as_ref())
                .map(|artwork| self.popup_artwork_image(artwork))
        } else {
            None
        };
        if let Some(dock) = dock {
            dock.set_media_view(view.clone());
        }
        if let Some((token, popup)) = popup {
            let current = || {
                self.retire_invalid_popup();
                Rc::ptr_eq(&revision, &self.projection.borrow())
                    && self
                        .state
                        .borrow()
                        .popup
                        .as_ref()
                        .is_some_and(|popup| popup.token == token)
            };
            if let Some(popup) = popup.upgrade()
                && current()
            {
                let mut view = view;
                view.enabled = true;
                // Dirty marks an ordinary refresh, not revoked seek authority.
                // Keep its transient notice from moving and cancelling valid input;
                // raw Dock staleness and precomputed transport gates stay unchanged.
                if seek_scope.is_some() {
                    view.stale = false;
                }
                if let Some(artwork) = popup_artwork {
                    view.artwork = artwork;
                }
                popup.set_media_view(view);
                if !current() {
                    return;
                }
                if seek_invalidated {
                    popup.invoke_media_seek_invalidated();
                    if !current() {
                        return;
                    }
                }
                popup.set_seek_enabled(
                    seek_scope
                        .as_ref()
                        .is_some_and(|scope| self.seek_scope_current(scope)),
                );
                if !current() {
                    return;
                }
                popup.set_seek_progress(seek_progress);
                if !current() {
                    return;
                }
                popup.set_seek_notice(seek_notice.into());
                if !current() {
                    return;
                }
                popup.invoke_project_seek();
                if !current() {
                    return;
                }
                let projection = timeline_projection(timeline.as_ref());
                popup.set_timeline_available(projection.available);
                if !current() {
                    return;
                }
                popup.set_timeline_time(projection.time.into());
                if !current() {
                    return;
                }
                popup.set_timeline_notice(projection.notice.into());
                if !current() {
                    return;
                }
                popup.set_timeline_progress(projection.progress);
                if current() {
                    popup.invoke_media_projection_complete();
                    // Native fitting may synchronously retire or replace the source.
                    self.retire_invalid_popup();
                }
            }
        }
    }
}

impl Drop for DockMediaController {
    fn drop(&mut self) {
        self.close();
    }
}

fn session_identity(state: &State, key: tessera_system::media::MediaSessionKey) -> String {
    format!("{}:{}:{key:?}", state.generation, state.sequence)
}

struct TimelineProjection {
    available: bool,
    time: String,
    notice: String,
    progress: f32,
}

/// Raw 100ns observations only: UTC LastUpdatedTime is not an elapsed clock.
fn timeline_projection(timeline: Option<&Result<MediaTimeline, MediaError>>) -> TimelineProjection {
    let unavailable = |notice: String| TimelineProjection {
        available: false,
        time: String::new(),
        notice,
        progress: 0.0,
    };
    let Some(timeline) = timeline else {
        return unavailable(String::new());
    };
    let timeline = match timeline {
        Ok(timeline) => timeline,
        Err(error) => return unavailable(format!("Timeline unavailable: {}", notice(Some(error)))),
    };
    if timeline.end_ticks < timeline.start_ticks
        || timeline.min_seek_ticks > timeline.max_seek_ticks
        || timeline.position_ticks < timeline.start_ticks
        || timeline.position_ticks > timeline.end_ticks
    {
        return unavailable("Timeline unavailable: inconsistent observed bounds.".into());
    }
    let elapsed = i128::from(timeline.position_ticks) - i128::from(timeline.start_ticks);
    let duration = i128::from(timeline.end_ticks) - i128::from(timeline.start_ticks);
    TimelineProjection {
        available: true,
        time: format!(
            "{} / {} · observed",
            ticks_label(elapsed),
            ticks_label(duration)
        ),
        notice: if duration == 0 {
            "Observed zero-length timeline.".into()
        } else {
            String::new()
        },
        progress: if duration > 0 {
            (elapsed as f64 / duration as f64) as f32
        } else {
            0.0
        },
    }
}

fn ticks_label(ticks: i128) -> String {
    let seconds = ticks / 10_000_000;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

fn notice(error: Option<&MediaError>) -> String {
    error
        .map(|error| sanitize::bounded_text(&error.message, 256))
        .unwrap_or_default()
}

fn playback_label(playback: MediaPlayback) -> &'static str {
    match playback {
        MediaPlayback::Closed => "Closed",
        MediaPlayback::Opened => "Opened",
        MediaPlayback::Changing => "Changing",
        MediaPlayback::Stopped => "Stopped",
        MediaPlayback::Playing => "Playing",
        MediaPlayback::Paused => "Paused",
    }
}

/// Contrast comes only from actual decoded pixels, never a fabricated album color.
fn artwork_dark_foreground(artwork: &MediaArtwork) -> bool {
    let mut luminance = 0_u64;
    let mut alpha = 0_u64;
    for rgba in artwork.rgba().as_chunks::<4>().0 {
        luminance +=
            2126 * u64::from(rgba[0]) + 7152 * u64::from(rgba[1]) + 722 * u64::from(rgba[2]);
        alpha += u64::from(rgba[3]);
    }
    alpha != 0 && luminance > alpha * 5000
}
