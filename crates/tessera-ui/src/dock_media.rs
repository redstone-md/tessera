// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Optional OS-current media: observation, single transport intent and readback.
//! Construction has no native effects; the parent persists enablement first.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::ComponentHandle;
use tessera_system::media::{
    MediaAction as HostAction, MediaArtwork, MediaCommand, MediaError, MediaErrorKind, MediaEvent,
    MediaHost, MediaPlayback, MediaSnapshot,
};

use crate::generated::{Dock, DockMediaView, MediaAction};
use crate::icons::IconCache;
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Flight {
    token: Token,
    kind: FlightKind,
}

#[derive(Default)]
struct State {
    generation: u64,
    sequence: u64,
    enabled: bool,
    provider: Option<Arc<dyn MediaHost>>,
    watch: Option<Token>,
    watch_guard: Option<Box<dyn Send>>,
    watch_attempted: bool,
    watch_ready: bool,
    flight: Option<Flight>,
    read_needed: bool,
    dirty: bool,
    snapshot: Option<MediaSnapshot>,
    read_error: Option<MediaError>,
    watch_error: Option<MediaError>,
    action_error: Option<MediaError>,
    action_accepted: bool,
}

impl State {
    fn token(&mut self) -> Option<Token> {
        self.sequence = self.sequence.checked_add(1)?;
        Some(Token {
            generation: self.generation,
            sequence: self.sequence,
        })
    }
}

enum Completion {
    Read(Token, Result<MediaSnapshot, MediaError>),
    Command(Token, Result<(), MediaError>),
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
    wake_queued: bool,
}

impl Mailbox {
    fn pending(&self) -> bool {
        self.completion.is_some() || self.watch_health.is_some() || self.dirty
    }
}

fn publish(
    mailbox: &Arc<Mutex<Mailbox>>,
    dock: &slint::Weak<Dock>,
    generation: u64,
    update: impl FnOnce(&mut Mailbox) -> bool,
) {
    {
        let mut mailbox = mailbox.lock();
        if !mailbox.enabled
            || mailbox.generation != generation
            || !update(&mut mailbox)
            || mailbox.wake_queued
        {
            return;
        }
        mailbox.wake_queued = true;
    }
    let weak_mailbox = Arc::downgrade(mailbox);
    if dock
        .upgrade_in_event_loop(move |dock| {
            let Some(mailbox) = weak_mailbox.upgrade() else {
                return;
            };
            let current = {
                let mut mailbox = mailbox.lock();
                let current = mailbox.enabled
                    && mailbox.generation == generation
                    && mailbox.wake_queued
                    && mailbox.pending();
                if mailbox.generation == generation {
                    mailbox.wake_queued = false;
                }
                current
            };
            if current {
                dock.invoke_media_event_ready();
            }
        })
        .is_err()
    {
        // The no-event-loop test backend drains this production mailbox explicitly.
        let mut mailbox = mailbox.lock();
        if mailbox.generation == generation {
            mailbox.wake_queued = false;
        }
    }
}

fn complete(mailbox: &Arc<Mutex<Mailbox>>, dock: &slint::Weak<Dock>, result: Completion) {
    let flight = result.flight();
    publish(mailbox, dock, flight.token.generation, |mailbox| {
        if mailbox.expected != Some(flight) || mailbox.completion.is_some() {
            return false;
        }
        mailbox.completion = Some(result);
        true
    });
}

fn watch_event(
    mailbox: &Arc<Mutex<Mailbox>>,
    dock: &slint::Weak<Dock>,
    token: Token,
    event: MediaEvent,
) {
    publish(mailbox, dock, token.generation, |mailbox| {
        if mailbox.watch != Some(token) {
            return false;
        }
        match event {
            MediaEvent::Changed => mailbox.dirty = true,
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
    state: RefCell<State>,
    mailbox: Arc<Mutex<Mailbox>>,
    icons: RefCell<IconCache>,
    application_icons: RefCell<HashMap<String, PixelIcon>>,
    driving: Cell<bool>,
    closed: Cell<bool>,
}

impl DockMediaController {
    pub(crate) fn new(host: Arc<dyn DesktopHost>, dock: &Dock) -> Rc<Self> {
        Rc::new(Self {
            host,
            dock: dock.as_weak(),
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
            icons: RefCell::default(),
            application_icons: RefCell::default(),
            driving: Cell::new(false),
            closed: Cell::new(false),
        })
    }

    /// Enablement is adopted only after the parent saves its complete preference record.
    pub(crate) fn set_enabled(&self, enabled: bool) {
        if self.closed.get() || self.state.borrow().enabled == enabled {
            return;
        }
        let retired = {
            let mut state = self.state.borrow_mut();
            let Some(generation) = state.generation.checked_add(1) else {
                return;
            };
            let retired = (state.watch_guard.take(), state.provider.take());
            *state = State {
                generation,
                sequence: state.sequence,
                enabled,
                read_needed: enabled,
                ..State::default()
            };
            *self.mailbox.lock() = Mailbox {
                generation,
                enabled,
                ..Mailbox::default()
            };
            retired
        };
        // Native guard/facade Drop only queues owner cleanup. No UI thread joins.
        drop(retired);
        self.project();
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
            if !state.enabled || state.flight.is_some() || state.dirty || state.read_error.is_some()
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
        let mailbox = self.mailbox.clone();
        let dock = self.dock.clone();
        if let Err(error) = provider.execute(
            command,
            Box::new(move |result| {
                complete(&mailbox, &dock, Completion::Command(flight.token, result));
            }),
        ) {
            complete(
                &self.mailbox,
                &self.dock,
                Completion::Command(flight.token, Err(error)),
            );
        }
    }

    /// Retry observes only. It never repeats a previous transport request.
    pub(crate) fn retry(&self) {
        if self.closed.get() {
            return;
        }
        let retired = {
            let mut state = self.state.borrow_mut();
            if !state.enabled || state.flight.is_some() {
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
            let (completion, health, dirty) = {
                let mut mailbox = self.mailbox.lock();
                mailbox.wake_queued = false;
                (
                    mailbox.completion.take(),
                    mailbox.watch_health.take(),
                    std::mem::take(&mut mailbox.dirty),
                )
            };
            let mut state = self.state.borrow_mut();
            if !state.enabled {
                return;
            }
            if let Some(health) = health {
                state.watch_ready = health.is_ok();
                state.watch_error = health.err();
            }
            if dirty {
                state.dirty = true;
                state.read_needed = true;
            }
            if let Some(completion) = completion {
                let flight = completion.flight();
                if state.flight == Some(flight) {
                    state.flight = None;
                    self.mailbox.lock().expected = None;
                    match completion {
                        Completion::Read(_, Ok(snapshot)) => {
                            state.snapshot = Some(MediaSnapshot {
                                current: snapshot.current.map(|session| session.bounded()),
                            });
                            state.read_error = None;
                            // Invalidations delivered during the read require one follow-up.
                            state.dirty = dirty || state.read_needed;
                        }
                        Completion::Read(_, Err(error)) => {
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
            state.flight = None;
            state.watch = None;
            state.snapshot = None;
            (state.watch_guard.take(), state.provider.take())
        };
        *self.mailbox.lock() = Mailbox::default();
        self.project();
        drop(retired);
    }

    fn current(&self, flight: Flight) -> bool {
        let state = self.state.borrow();
        !self.closed.get() && state.enabled && state.flight == Some(flight)
    }

    fn drive_reads(&self) {
        if self.closed.get() || self.driving.replace(true) {
            return;
        }
        let _driving = Driving(&self.driving);
        loop {
            let (flight, provider) = {
                let mut state = self.state.borrow_mut();
                if !state.enabled || !state.read_needed || state.flight.is_some() {
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
            }
            self.project();
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
                            &self.dock,
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
            let dock = self.dock.clone();
            if let Err(error) = provider.read(Box::new(move |result| {
                complete(&mailbox, &dock, Completion::Read(flight.token, result));
            })) {
                complete(
                    &self.mailbox,
                    &self.dock,
                    Completion::Read(flight.token, Err(error)),
                );
            }
            if self.current(flight) {
                break;
            }
        }
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
        let dock = self.dock.clone();
        let result = provider.subscribe(Arc::new(move |event| {
            watch_event(&mailbox, &dock, token, event);
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

    fn project(&self) {
        let Some(dock) = self.dock.upgrade() else {
            return;
        };
        let view = {
            let state = self.state.borrow();
            let current = state
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.current.as_ref());
            let busy = state.flight.is_some();
            let stale = state.dirty || state.read_error.is_some();
            let controls = !busy && !stale;
            let status = if !state.enabled {
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
                view.session_identity =
                    format!("{}:{}:{:?}", state.generation, state.sequence, session.key).into();
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
        // Slint setters may route callbacks; no RefCell borrow crosses presentation.
        dock.set_media_view(view);
    }
}

impl Drop for DockMediaController {
    fn drop(&mut self) {
        self.close();
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
