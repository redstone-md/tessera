// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tessera_system::media::{
    MediaAction, MediaCapabilities, MediaCommand, MediaError, MediaErrorKind, MediaEvent,
    MediaObservationRevision, MediaRequest, MediaSeekCommand, MediaSeekObservation, MediaSelection,
    MediaSession, MediaSessionKey, MediaSnapshot,
};
#[cfg(windows)]
use tessera_system::media::{MediaInventory, MediaInventoryEntry, MediaSelectionCommand};

type Dirty = Arc<dyn Fn(MediaEvent) + Send + Sync>;
type NativeCallback = Arc<dyn Fn() + Send + Sync>;

/// Callback-time revocation, independent of the bounded actor dirty queue.
/// Zero is permanent exhaustion; it is never another valid generation.
struct RevisionState(AtomicU64);

impl RevisionState {
    fn new() -> Self {
        Self(AtomicU64::new(1))
    }

    fn stamp(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    fn invalidate(&self) {
        let mut value = self.0.load(Ordering::Acquire);
        while value != 0 {
            match self.0.compare_exchange_weak(
                value,
                value.checked_add(1).unwrap_or(0),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(observed) => value = observed,
            }
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct ObservationStamp {
    manager: u64,
    session: Option<u64>,
}

impl ObservationStamp {
    fn exhausted(self) -> bool {
        self.manager == 0 || self.session == Some(0)
    }
}

struct Observed<S> {
    session: S,
    key: MediaSessionKey,
    stamp: ObservationStamp,
    revision: MediaObservationRevision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Event {
    Sessions,
    Current,
    Properties,
    Playback,
    Timeline,
}

/// All methods run on the actor owner. Session equality is canonical COM
/// identity, never source metadata. Tokens own the corresponding event source.
pub(super) trait Calls {
    type Session: Clone + Eq;
    type Token;
    fn sessions(&mut self) -> Result<Vec<Self::Session>, MediaError>;
    fn current(&mut self) -> Result<Option<Self::Session>, MediaError>;
    fn snapshot(
        &mut self,
        session: &Self::Session,
        key: MediaSessionKey,
    ) -> Result<MediaSession, MediaError>;
    fn capabilities(&mut self, session: &Self::Session) -> Result<MediaCapabilities, MediaError>;
    fn transport(
        &mut self,
        session: &Self::Session,
        action: MediaAction,
    ) -> Result<bool, MediaError>;
    /// Fresh playback-position control and exact native seek bounds.
    fn seek_range(&mut self, session: &Self::Session) -> Result<(i64, i64), MediaError>;
    fn seek(&mut self, session: &Self::Session, position_ticks: i64) -> Result<bool, MediaError>;
    fn register(
        &mut self,
        event: Event,
        session: Option<&Self::Session>,
        dirty: NativeCallback,
    ) -> Result<Self::Token, MediaError>;
    fn remove(&mut self, token: Self::Token);
}

struct Registrations<T> {
    active: Arc<AtomicBool>,
    // A retired callback can only revoke its own registration group, even
    // when it was admitted immediately before replacement/retirement.
    revision: Arc<RevisionState>,
    tokens: Vec<T>,
}

impl<T> Registrations<T> {
    fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(true)),
            revision: Arc::new(RevisionState::new()),
            tokens: Vec::new(),
        }
    }

    fn callback(&self, dirty: &Dirty, event: Event) -> NativeCallback {
        self.scoped_callback(dirty, event, false)
    }

    fn scoped_callback(&self, dirty: &Dirty, event: Event, inventory: bool) -> NativeCallback {
        let active = self.active.clone();
        let dirty = dirty.clone();
        let revision = Arc::downgrade(&self.revision);
        Arc::new(move || {
            if active.load(Ordering::Acquire) {
                let Some(revision) = revision.upgrade() else {
                    return;
                };
                if matches!(event, Event::Sessions | Event::Current | Event::Properties)
                    || (inventory && event == Event::Playback)
                {
                    revision.invalidate();
                    dirty(MediaEvent::SeekInvalidated);
                } else {
                    dirty(MediaEvent::Changed);
                }
            }
        })
    }
}

struct Watch<S, T> {
    dirty: Dirty,
    manager: Registrations<T>,
    session: Option<(S, Registrations<T>)>,
    sources: Vec<(S, Registrations<T>)>,
    failures: Vec<(S, MediaError)>,
}

pub(super) struct Owner<C: Calls> {
    calls: C,
    // Owning canonical interfaces prevent recycled pointer addresses from
    // aliasing a still-recorded session. Retire entries absent from GetSessions.
    keys: Vec<(C::Session, MediaSessionKey)>,
    watch: Option<Watch<C::Session, C::Token>>,
    observed: Option<Observed<C::Session>>,
    read_watch_failure: Option<MediaError>,
    inventory_mode: bool,
    selection: MediaSelection,
    native_current: Option<MediaSessionKey>,
    current_error: Option<MediaError>,
    transport_observation: Option<(C::Session, MediaSessionKey, Option<ObservationStamp>)>,
    inventory_authority: Option<(
        MediaObservationRevision,
        Vec<MediaSessionKey>,
        ObservationStamp,
    )>,
}

impl<C: Calls> Owner<C> {
    pub(super) fn new(calls: C) -> Self {
        Self {
            calls,
            keys: Vec::new(),
            watch: None,
            observed: None,
            read_watch_failure: None,
            inventory_mode: false,
            selection: MediaSelection::FollowCurrent,
            native_current: None,
            current_error: None,
            transport_observation: None,
            inventory_authority: None,
        }
    }

    pub(super) fn take_watch_failure(&mut self) -> Option<MediaError> {
        self.read_watch_failure.take()
    }

    #[cfg(test)]
    pub(super) fn exhaust_observation_revisions(&self) {
        if let Some(watch) = &self.watch {
            watch.manager.revision.0.store(u64::MAX, Ordering::Release);
            watch.manager.revision.invalidate();
        }
    }

    fn current(&mut self) -> Result<Option<(C::Session, MediaSessionKey)>, MediaError> {
        if self.inventory_mode {
            let native_current = self.reconcile_inventory()?;
            if self.selection == MediaSelection::FollowCurrent
                && let Some(error) = &self.current_error
            {
                return Err(error.clone());
            }
            let key = match self.selection {
                MediaSelection::FollowCurrent => native_current,
                MediaSelection::Session(key) => Some(key),
            };
            return match key {
                None => Ok(None),
                Some(key) => self
                    .keys
                    .iter()
                    .find(|(_, known)| *known == key)
                    .map(|(session, key)| Some((session.clone(), *key)))
                    .ok_or_else(changed),
            };
        }
        let live = self.calls.sessions()?;
        // Read current AFTER enumeration: do not accidentally recommend a
        // listed session when the authoritative current session is absent.
        let current = self.calls.current()?;
        self.keys
            .retain(|(session, _)| live.contains(session) || current.as_ref() == Some(session));
        let Some(session) = current else {
            return Ok(None);
        };
        let key = match self.keys.iter().find(|(known, _)| known == &session) {
            Some((_, key)) => *key,
            None => {
                let key = MediaSessionKey::issue()?;
                self.keys.push((session.clone(), key));
                key
            }
        };
        Ok(Some((session, key)))
    }

    fn still_current(
        &mut self,
        session: &C::Session,
        key: MediaSessionKey,
    ) -> Result<bool, MediaError> {
        Ok(self
            .current()?
            .as_ref()
            .map(|(current, key)| (current, *key))
            == Some((session, key)))
    }

    fn has_seek_watch(&self, session: &C::Session) -> bool {
        self.watch.as_ref().is_some_and(|watch| {
            watch
                .session
                .as_ref()
                .is_some_and(|(bound, _)| bound == session)
                || watch.sources.iter().any(|(bound, _)| bound == session)
        })
    }

    fn observation_stamp(&self) -> Option<ObservationStamp> {
        let key = match self.selection {
            MediaSelection::Session(key) => Some(key),
            MediaSelection::FollowCurrent => self.native_current,
        };
        let selected = key.and_then(|key| self.keys.iter().find(|(_, known)| *known == key));
        self.watch.as_ref().map(|watch| ObservationStamp {
            manager: watch.manager.revision.stamp(),
            session: if self.inventory_mode {
                let session = selected.map(|(session, _)| session);
                session.and_then(|session| {
                    watch
                        .sources
                        .iter()
                        .find(|(bound, _)| bound == session)
                        .map(|(_, registrations)| registrations.revision.stamp())
                })
            } else {
                watch
                    .session
                    .as_ref()
                    .map(|(_, registrations)| registrations.revision.stamp())
            },
        })
    }

    fn reconcile_inventory(&mut self) -> Result<Option<MediaSessionKey>, MediaError> {
        let live = match self.calls.sessions() {
            Ok(live) => live,
            Err(error) => {
                // An unknowable enumeration retires authority, including reconnect.
                self.keys.clear();
                self.observed = None;
                self.transport_observation = None;
                self.inventory_authority = None;
                self.native_current = None;
                self.read_watch_failure = Some(watch_error(error.clone()));
                self.stop_watch();
                return Err(error);
            }
        };
        if live.len() > 256 {
            self.keys.clear();
            self.observed = None;
            self.transport_observation = None;
            self.inventory_authority = None;
            self.native_current = None;
            let error = MediaError::new(
                MediaErrorKind::Unavailable,
                "Media inventory exceeds 256 sessions",
            );
            self.read_watch_failure = Some(watch_error(error.clone()));
            self.stop_watch();
            return Err(error);
        }
        self.keys.retain(|(session, _)| live.contains(session));
        for session in live {
            if !self.keys.iter().any(|(known, _)| known == &session) {
                self.keys.push((session, MediaSessionKey::issue()?));
            }
        }
        if self.observed.as_ref().is_some_and(|observed| {
            !self
                .keys
                .iter()
                .any(|(session, key)| session == &observed.session && *key == observed.key)
        }) {
            self.observed = None;
        }
        if self
            .transport_observation
            .as_ref()
            .is_some_and(|(session, key, _)| {
                !self
                    .keys
                    .iter()
                    .any(|(known, live_key)| known == session && live_key == key)
            })
        {
            self.transport_observation = None;
        }
        let current = self.calls.current().and_then(|current| match current {
            None => Ok(None),
            Some(current) => self
                .keys
                .iter()
                .find(|(session, _)| *session == current)
                .map(|(_, key)| Some(*key))
                .ok_or_else(changed),
        });
        self.current_error = current.as_ref().err().cloned();
        self.native_current = current.unwrap_or(None);
        Ok(self.native_current)
    }

    #[cfg(windows)]
    fn manager_stamp(&self) -> ObservationStamp {
        ObservationStamp {
            manager: self
                .watch
                .as_ref()
                .map_or(1, |watch| watch.manager.revision.stamp()),
            session: None,
        }
    }

    fn source_stamp(&self, session: &C::Session) -> Option<ObservationStamp> {
        self.watch.as_ref().map(|watch| ObservationStamp {
            manager: watch.manager.revision.stamp(),
            session: watch
                .sources
                .iter()
                .find(|(bound, _)| bound == session)
                .map(|(_, registrations)| registrations.revision.stamp()),
        })
    }

    #[cfg(windows)]
    pub(super) fn read_inventory(&mut self) -> Result<MediaInventory, MediaError> {
        self.inventory_mode = true;
        let native_current = self.reconcile_inventory()?;
        let current_error = self.current_error.clone();
        let registration_error = self.refresh_watch().err();
        if let Some(error) = &registration_error {
            self.read_watch_failure = Some(error.clone());
        }
        let manager_stamp = self.manager_stamp();
        let selected_key = match self.selection {
            MediaSelection::FollowCurrent => native_current,
            MediaSelection::Session(key) => Some(key),
        };
        let live = self.keys.clone();
        let mut sessions = Vec::with_capacity(live.len());
        let mut stamps = Vec::with_capacity(live.len());
        for (session, key) in &live {
            let watch = registration_error.clone().map_or_else(
                || {
                    self.watch.as_ref().map_or_else(
                        || {
                            Err(MediaError::new(
                                MediaErrorKind::WatchUnavailable,
                                "Live media updates are unavailable",
                            ))
                        },
                        |watch| {
                            if watch.sources.iter().any(|(bound, _)| bound == session) {
                                Ok(())
                            } else {
                                Err(watch
                                    .failures
                                    .iter()
                                    .find(|(bound, _)| bound == session)
                                    .map(|(_, error)| error.clone())
                                    .unwrap_or_else(|| {
                                        MediaError::new(
                                            MediaErrorKind::WatchUnavailable,
                                            "Session watch unavailable",
                                        )
                                    }))
                            }
                        },
                    )
                },
                Err,
            );
            let stamp = self.source_stamp(session);
            stamps.push(stamp);
            let mut observation = self.calls.snapshot(session, *key);
            if let Ok(facts) = &mut observation {
                facts.seek = if Some(*key) == selected_key {
                    let range = self.calls.seek_range(session);
                    watch
                        .clone()
                        .and_then(|()| self.observe_seek(session, *key, stamp, range))
                } else {
                    Err(MediaError::new(
                        MediaErrorKind::CommandUnavailable,
                        "Select this player to seek",
                    ))
                };
            }
            if self.source_stamp(session) != stamp {
                observation = Err(changed());
            }
            sessions.push(MediaInventoryEntry {
                key: *key,
                observation: observation.map(MediaSession::bounded),
                watch,
            });
        }
        // Blocking WinRT reads cannot publish an enumeration that changed meanwhile.
        self.reconcile_inventory()?;
        if self.keys != live
            || self.manager_stamp() != manager_stamp
            || self.native_current != native_current
            || self.current_error != current_error
        {
            self.inventory_authority = None;
            return Err(changed());
        }
        for ((session, _), (entry, stamp)) in live.iter().zip(sessions.iter_mut().zip(stamps)) {
            if self.source_stamp(session) != stamp {
                entry.observation = Err(changed());
            }
        }
        let selected = match (self.selection, selected_key, &current_error) {
            (MediaSelection::FollowCurrent, _, Some(error)) => Err(error.clone()),
            (_, None, _) => Ok(MediaSnapshot { current: None }),
            (_, Some(key), _) => sessions
                .iter()
                .find(|entry| entry.key == key)
                .ok_or_else(changed)
                .and_then(|entry| entry.observation.clone())
                .map(|current| MediaSnapshot {
                    current: Some(current),
                }),
        };
        self.transport_observation = selected
            .as_ref()
            .ok()
            .and_then(|snapshot| snapshot.current.as_ref())
            .and_then(|facts| self.keys.iter().find(|(_, key)| *key == facts.key))
            .map(|(session, key)| (session.clone(), *key, self.source_stamp(session)));
        if selected.is_err() {
            self.observed = None;
        }
        let revision = MediaObservationRevision::issue()?;
        self.inventory_authority = Some((
            revision,
            live.iter().map(|(_, key)| *key).collect(),
            manager_stamp,
        ));
        Ok(MediaInventory {
            revision,
            sessions,
            native_current: current_error.map_or(Ok(native_current), Err),
            selection: self.selection,
            selected,
        })
    }

    #[cfg(windows)]
    pub(super) fn select(&mut self, command: MediaSelectionCommand) -> Result<(), MediaError> {
        let (revision, keys, stamp) = self.inventory_authority.clone().ok_or_else(changed)?;
        self.reconcile_inventory()?;
        if revision != command.expected_inventory
            || self.keys.iter().map(|(_, key)| *key).collect::<Vec<_>>() != keys
            || self.manager_stamp() != stamp
            || stamp.exhausted()
        {
            self.inventory_authority = None;
            return Err(changed());
        }
        if let MediaSelection::Session(key) = command.selection
            && !keys.contains(&key)
        {
            return Err(changed());
        }
        self.selection = command.selection;
        self.observed = None;
        self.transport_observation = None;
        self.inventory_authority = None;
        Ok(())
    }

    fn rebind_inventory(&mut self) -> Result<(), MediaError> {
        self.reconcile_inventory()?;
        let watch = self
            .watch
            .as_mut()
            .expect("watch checked before inventory rebind");
        if let Some((_, registrations)) = watch.session.take() {
            remove_all(&mut self.calls, registrations);
            self.observed = None;
        }
        let mut index = 0;
        while index < watch.sources.len() {
            if self
                .keys
                .iter()
                .any(|(session, _)| *session == watch.sources[index].0)
            {
                index += 1;
            } else {
                let (_, registrations) = watch.sources.remove(index);
                remove_all(&mut self.calls, registrations);
            }
        }
        watch
            .failures
            .retain(|(session, _)| self.keys.iter().any(|(known, _)| known == session));
        for (session, _) in &self.keys {
            if watch.sources.iter().any(|(bound, _)| bound == session) {
                continue;
            }
            watch.failures.retain(|(bound, _)| bound != session);
            watch.sources.push((session.clone(), Registrations::new()));
            let mut failure = None;
            for event in [Event::Properties, Event::Playback, Event::Timeline] {
                let (bound, registrations) = watch
                    .sources
                    .last_mut()
                    .expect("source installed before register");
                let callback = registrations.scoped_callback(&watch.dirty, event, true);
                match self.calls.register(event, Some(bound), callback) {
                    Ok(token) => registrations.tokens.push(token),
                    Err(error) => {
                        failure = Some(watch_error(error));
                        break;
                    }
                }
            }
            if let Some(error) = failure {
                let (_, registrations) = watch.sources.pop().expect("source installed");
                remove_all(&mut self.calls, registrations);
                watch.failures.push((session.clone(), error));
            }
        }
        Ok(())
    }

    pub(super) fn read(&mut self) -> Result<MediaSnapshot, MediaError> {
        if self.inventory_mode {
            self.transport_observation = None;
        }
        let Some((session, key)) = self.current()? else {
            self.observed = None;
            return Ok(MediaSnapshot { current: None });
        };
        // Bind event authority before capturing the metadata observation.
        let registration_error = self.refresh_watch().err();
        if let Some(error) = &registration_error {
            // Preserve the independent read result while informing the actor
            // that its retained readiness/dirty generation has been retired.
            self.read_watch_failure = Some(error.clone());
        }
        let stamp = self.observation_stamp();
        let result = self.calls.snapshot(&session, key);
        let range = self.calls.seek_range(&session);
        // Metadata/artwork can outlive a track on the same COM object. Both
        // callback revocation and canonical current identity must still match.
        if !self.still_current(&session, key)? || self.observation_stamp() != stamp {
            return Err(changed());
        }
        let mut result = result?;
        result.seek = match registration_error {
            Some(error) => Err(error),
            None => self.observe_seek(&session, key, stamp, range),
        };
        if self.observation_stamp() != stamp {
            return Err(changed());
        }
        if self.inventory_mode {
            self.transport_observation = Some((session.clone(), key, self.source_stamp(&session)));
        }
        Ok(MediaSnapshot {
            current: Some(result.bounded()),
        })
    }

    fn observe_seek(
        &mut self,
        session: &C::Session,
        key: MediaSessionKey,
        stamp: Option<ObservationStamp>,
        range: Result<(i64, i64), MediaError>,
    ) -> Result<MediaSeekObservation, MediaError> {
        if !self.has_seek_watch(session) {
            return Err(MediaError::new(
                MediaErrorKind::CommandUnavailable,
                "Seek observation requires live native invalidation registration",
            ));
        }
        let stamp = stamp.ok_or_else(changed)?;
        if stamp.exhausted() {
            return Err(revision_exhausted());
        }
        let revision = match &self.observed {
            Some(observed)
                if observed.session == *session
                    && observed.key == key
                    && observed.stamp == stamp =>
            {
                observed.revision
            }
            _ => {
                let revision = MediaObservationRevision::issue()?;
                self.observed = Some(Observed {
                    session: session.clone(),
                    key,
                    stamp,
                    revision,
                });
                revision
            }
        };
        let (min_ticks, max_ticks) = range?;
        validate_range(min_ticks, max_ticks)?;
        Ok(MediaSeekObservation {
            revision,
            min_ticks,
            max_ticks,
        })
    }

    pub(super) fn execute(&mut self, request: MediaRequest) -> Result<(), MediaError> {
        match request {
            MediaRequest::Transport(command) => self.transport(command),
            MediaRequest::Seek(command) => self.seek(command),
        }
    }

    fn transport(&mut self, command: MediaCommand) -> Result<(), MediaError> {
        let Some((session, key)) = self.current()? else {
            return Err(changed());
        };
        if key != command.expected_session {
            return Err(changed());
        }
        let stamp = self.source_stamp(&session);
        if self.inventory_mode
            && !self.transport_observation.as_ref().is_some_and(
                |(observed, known, observed_stamp)| {
                    *known == key
                        && observed == &session
                        && *observed_stamp == stamp
                        && stamp.is_some_and(|stamp| stamp.session.is_some() && !stamp.exhausted())
                },
            )
        {
            return Err(changed());
        }
        let capabilities = self.calls.capabilities(&session)?;
        // There is no atomic GSMTC compare-and-command primitive. This final
        // identity query follows the capability query, immediately before Try*.
        if !self.still_current(&session, key)?
            || (self.inventory_mode && self.source_stamp(&session) != stamp)
        {
            return Err(changed());
        }
        if !capabilities.allows(command.action) {
            return Err(MediaError::new(
                MediaErrorKind::CommandUnavailable,
                "The current session does not enable this media command",
            ));
        }
        if self.calls.transport(&session, command.action)? {
            Ok(())
        } else {
            Err(MediaError::new(
                MediaErrorKind::Rejected,
                "The media session rejected the transport request",
            ))
        }
    }

    fn seek(&mut self, command: MediaSeekCommand) -> Result<(), MediaError> {
        if self
            .observation_stamp()
            .is_some_and(ObservationStamp::exhausted)
        {
            return Err(revision_exhausted());
        }
        let Some((session, key)) = self.current()? else {
            return Err(changed());
        };
        let observed = self.observed.as_ref().ok_or_else(changed)?;
        let stamp = observed.stamp;
        if key != command.expected_session
            || observed.key != key
            || observed.session != session
            || observed.revision != command.expected_revision
            || self.observation_stamp() != Some(stamp)
            || !self.has_seek_watch(&session)
        {
            return Err(changed());
        }
        let (min, max) = self.calls.seek_range(&session)?;
        validate_range(min, max)?;
        if (min, max) != (command.observed_min_ticks, command.observed_max_ticks) {
            return Err(changed());
        }
        if command.position_ticks < min || command.position_ticks > max {
            return Err(MediaError::new(
                MediaErrorKind::Rejected,
                "Seek position is outside the observed native range",
            ));
        }
        // Sequential final recheck, not an atomic OS compare-and-seek promise.
        if !self.still_current(&session, key)? || self.observation_stamp() != Some(stamp) {
            return Err(changed());
        }
        if self.calls.seek(&session, command.position_ticks)? {
            Ok(())
        } else {
            Err(MediaError::new(
                MediaErrorKind::Rejected,
                "The media session rejected the seek request",
            ))
        }
    }

    pub(super) fn start_watch(&mut self, dirty: Dirty) -> Result<(), MediaError> {
        self.stop_watch();
        self.read_watch_failure = None;
        self.watch = Some(Watch {
            dirty,
            manager: Registrations::new(),
            session: None,
            sources: Vec::new(),
            failures: Vec::new(),
        });
        let result = self
            .start_manager_watch()
            .and_then(|()| self.refresh_watch());
        if result.is_err() {
            self.stop_watch();
        }
        result.map_err(watch_error)
    }

    fn start_manager_watch(&mut self) -> Result<(), MediaError> {
        for event in [Event::Sessions, Event::Current] {
            let watch = self
                .watch
                .as_ref()
                .expect("watch installed before registration");
            let callback = watch.manager.callback(&watch.dirty, event);
            let token = self.calls.register(event, None, callback)?;
            self.watch
                .as_mut()
                .expect("watch remains installed")
                .manager
                .tokens
                .push(token);
        }
        Ok(())
    }

    pub(super) fn refresh_watch(&mut self) -> Result<(), MediaError> {
        if self.watch.is_none() {
            return Ok(());
        }
        let result = if self.inventory_mode {
            self.rebind_inventory()
        } else {
            self.rebind_session()
        };
        if result.is_err() {
            self.stop_watch();
        }
        result.map_err(watch_error)
    }

    fn rebind_session(&mut self) -> Result<(), MediaError> {
        let current = self.current()?.map(|(session, _)| session);
        let watch = self.watch.as_mut().expect("watch checked before rebind");
        if watch.session.as_ref().map(|(session, _)| session) == current.as_ref() {
            return Ok(());
        }
        if watch.session.is_some() || self.observed.is_some() {
            (watch.dirty)(MediaEvent::SeekInvalidated);
        }
        // Detached properties are unknowable: A -> B -> A must never reuse A's
        // seek observation, even before delayed manager callbacks are delivered.
        self.observed = None;
        if let Some((_, registrations)) = watch.session.take() {
            remove_all(&mut self.calls, registrations);
        }
        let Some(session) = current else {
            return Ok(());
        };
        // Install state before the first register so an error or unwind leaves
        // every acquired token visible to stop_watch/Drop on this same owner.
        watch.session = Some((session, Registrations::new()));
        for event in [Event::Properties, Event::Playback, Event::Timeline] {
            let (session, registrations) = watch.session.as_mut().expect("session installed");
            let callback = registrations.callback(&watch.dirty, event);
            let token = self.calls.register(event, Some(session), callback)?;
            registrations.tokens.push(token);
        }
        Ok(())
    }

    pub(super) fn stop_watch(&mut self) {
        if let Some(mut watch) = self.watch.take() {
            self.observed = None;
            self.transport_observation = None;
            self.inventory_authority = None;
            // Disable both callback generations before any removal can deliver
            // a reentrant or already-queued callback. Callbacks borrow no owner.
            watch.manager.active.store(false, Ordering::Release);
            for (_, registrations) in &watch.sources {
                registrations.active.store(false, Ordering::Release);
            }
            for (_, registrations) in watch.sources.drain(..) {
                remove_all(&mut self.calls, registrations);
            }
            if let Some((_, registrations)) = &watch.session {
                registrations.active.store(false, Ordering::Release);
            }
            if let Some((_, registrations)) = watch.session.take() {
                remove_all(&mut self.calls, registrations);
            }
            remove_all(&mut self.calls, watch.manager);
        }
    }
}

impl<C: Calls> Drop for Owner<C> {
    fn drop(&mut self) {
        self.stop_watch();
        self.keys.clear();
        // Calls drops its manager before its apartment guard. No join here.
    }
}

fn remove_all<C: Calls>(calls: &mut C, registrations: Registrations<C::Token>) {
    registrations.active.store(false, Ordering::Release);
    for token in registrations.tokens.into_iter().rev() {
        calls.remove(token);
    }
}

fn changed() -> MediaError {
    MediaError::new(
        MediaErrorKind::SessionChanged,
        "The media session authority changed; refresh before commanding it",
    )
}

fn revision_exhausted() -> MediaError {
    MediaError::new(
        MediaErrorKind::Other,
        "Media observation revisions exhausted",
    )
}

fn validate_range(min: i64, max: i64) -> Result<(), MediaError> {
    if min < max {
        Ok(())
    } else {
        Err(MediaError::new(
            MediaErrorKind::CommandUnavailable,
            "Native seek range is empty or inverted",
        ))
    }
}

fn watch_error(mut error: MediaError) -> MediaError {
    error.kind = MediaErrorKind::WatchUnavailable;
    error
}

/// Preserve a successful null WinRT out pointer without treating a failed
/// E_POINTER (or any other HRESULT) as confirmed absence.
pub(super) fn nullable_result(
    code: i32,
    present: bool,
    operation: &str,
) -> Result<bool, MediaError> {
    if code < 0 {
        Err(MediaError::with_hresult(
            MediaErrorKind::Unavailable,
            format!("{operation} failed (HRESULT 0x{:08X})", code as u32),
            code,
        ))
    } else {
        Ok(present)
    }
}

pub(super) fn playback(status: i32) -> Result<tessera_system::media::MediaPlayback, MediaError> {
    use tessera_system::media::MediaPlayback;
    match status {
        0 => Ok(MediaPlayback::Closed),
        1 => Ok(MediaPlayback::Opened),
        2 => Ok(MediaPlayback::Changing),
        3 => Ok(MediaPlayback::Stopped),
        4 => Ok(MediaPlayback::Playing),
        5 => Ok(MediaPlayback::Paused),
        _ => Err(MediaError::new(
            MediaErrorKind::Other,
            "The SDK returned an unknown media playback status",
        )),
    }
}

#[cfg(test)]
mod revision_tests {
    use super::*;

    #[test]
    fn admitted_retired_revision_cannot_revoke_a_replacement_registration() {
        let old = Registrations::<()>::new();
        // Model a callback that already passed admission and upgraded its weak
        // revision before the owner retires the native registration group.
        let admitted = Arc::downgrade(&old.revision).upgrade().unwrap();
        old.active.store(false, Ordering::Release);
        drop(old);
        let replacement = Registrations::<()>::new();
        admitted.invalidate();
        assert_eq!(admitted.stamp(), 2);
        assert_eq!(replacement.revision.stamp(), 1);
    }

    #[test]
    fn native_epoch_exhaustion_is_permanent_without_wrapping() {
        let revision = RevisionState(AtomicU64::new(u64::MAX - 1));
        revision.invalidate();
        assert_eq!(revision.stamp(), u64::MAX);
        revision.invalidate();
        assert_eq!(revision.stamp(), 0);
        revision.invalidate();
        assert_eq!(revision.stamp(), 0);
        assert!(
            ObservationStamp {
                manager: 1,
                session: Some(0),
            }
            .exhausted()
        );
    }
}
