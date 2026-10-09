// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tessera_system::media::{
    MediaAction, MediaCapabilities, MediaCommand, MediaError, MediaErrorKind, MediaSession,
    MediaSessionKey, MediaSnapshot,
};

type Dirty = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Event {
    Sessions,
    Current,
    Properties,
    Playback,
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
    fn register(
        &mut self,
        event: Event,
        session: Option<&Self::Session>,
        dirty: Dirty,
    ) -> Result<Self::Token, MediaError>;
    fn remove(&mut self, token: Self::Token);
}

struct Registrations<T> {
    active: Arc<AtomicBool>,
    tokens: Vec<T>,
}

impl<T> Registrations<T> {
    fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(true)),
            tokens: Vec::new(),
        }
    }

    fn callback(&self, dirty: &Dirty) -> Dirty {
        let active = self.active.clone();
        let dirty = dirty.clone();
        Arc::new(move || {
            if active.load(Ordering::Acquire) {
                dirty();
            }
        })
    }
}

struct Watch<S, T> {
    dirty: Dirty,
    manager: Registrations<T>,
    session: Option<(S, Registrations<T>)>,
}

pub(super) struct Owner<C: Calls> {
    calls: C,
    // Owning canonical interfaces prevent recycled pointer addresses from
    // aliasing a still-recorded session. Retire entries absent from GetSessions.
    keys: Vec<(C::Session, MediaSessionKey)>,
    watch: Option<Watch<C::Session, C::Token>>,
}

impl<C: Calls> Owner<C> {
    pub(super) fn new(calls: C) -> Self {
        Self {
            calls,
            keys: Vec::new(),
            watch: None,
        }
    }

    fn current(&mut self) -> Result<Option<(C::Session, MediaSessionKey)>, MediaError> {
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

    pub(super) fn read(&mut self) -> Result<MediaSnapshot, MediaError> {
        let Some((session, key)) = self.current()? else {
            return Ok(MediaSnapshot { current: None });
        };
        let result = self.calls.snapshot(&session, key);
        // Metadata and thumbnail operations can outlive their session. Recheck
        // even when a property failed so replacement never masquerades as A.
        if self
            .current()?
            .as_ref()
            .map(|(current, key)| (current, *key))
            != Some((&session, key))
        {
            return Err(changed());
        }
        Ok(MediaSnapshot {
            current: Some(result?.bounded()),
        })
    }

    pub(super) fn execute(&mut self, command: MediaCommand) -> Result<(), MediaError> {
        let Some((session, key)) = self.current()? else {
            return Err(changed());
        };
        if key != command.expected_session {
            return Err(changed());
        }
        let capabilities = self.calls.capabilities(&session)?;
        // There is no atomic GSMTC compare-and-command primitive. This final
        // identity query follows the capability query, immediately before Try*.
        if self
            .current()?
            .as_ref()
            .map(|(current, key)| (current, *key))
            != Some((&session, key))
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

    pub(super) fn start_watch(&mut self, dirty: Dirty) -> Result<(), MediaError> {
        self.stop_watch();
        self.watch = Some(Watch {
            dirty,
            manager: Registrations::new(),
            session: None,
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
            let callback = watch.manager.callback(&watch.dirty);
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
        let result = self.rebind_session();
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
        if let Some((_, registrations)) = watch.session.take() {
            remove_all(&mut self.calls, registrations);
        }
        let Some(session) = current else {
            return Ok(());
        };
        // Install state before the first register so an error or unwind leaves
        // every acquired token visible to stop_watch/Drop on this same owner.
        watch.session = Some((session, Registrations::new()));
        for event in [Event::Properties, Event::Playback] {
            let (session, registrations) = watch.session.as_mut().expect("session installed");
            let token =
                self.calls
                    .register(event, Some(session), registrations.callback(&watch.dirty))?;
            registrations.tokens.push(token);
        }
        Ok(())
    }

    pub(super) fn stop_watch(&mut self) {
        if let Some(mut watch) = self.watch.take() {
            // Disable both callback generations before any removal can deliver
            // a reentrant or already-queued callback. Callbacks borrow no owner.
            watch.manager.active.store(false, Ordering::Release);
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
        "The OS-current media session changed; refresh before commanding it",
    )
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
