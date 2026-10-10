// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Legacy current-session facts plus optional all-session inventory and selection.
//!
//! Session keys are transient command authority, never application identities.
//! A successful empty snapshot is distinct from an unavailable observation.

use std::fmt;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SESSION_KEY: AtomicU64 = AtomicU64::new(1);
static NEXT_OBSERVATION_REVISION: AtomicU64 = AtomicU64::new(1);

/// Opaque identity for one live native session incarnation. Never persist it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MediaSessionKey(NonZeroU64);

impl MediaSessionKey {
    /// Allocates a globally unique key for a host or recording adapter only.
    /// Native hosts retain it for the same live session and rotate on reconnect,
    /// including when the replacement reports the same source application ID.
    pub fn issue() -> Result<Self, MediaError> {
        Self::issue_from(&NEXT_SESSION_KEY)
    }

    fn issue_from(counter: &AtomicU64) -> Result<Self, MediaError> {
        issue_nonce(counter, "Media session keys exhausted").map(Self)
    }
}

/// Opaque native observation invalidation key; never derive it from metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaObservationRevision(NonZeroU64);

impl MediaObservationRevision {
    /// Allocates authority for native owners and recording adapters only.
    /// Exhaustion is an error, never a wrap or a reused observation.
    pub fn issue() -> Result<Self, MediaError> {
        Self::issue_from(&NEXT_OBSERVATION_REVISION)
    }

    fn issue_from(counter: &AtomicU64) -> Result<Self, MediaError> {
        issue_nonce(counter, "Media observation revisions exhausted").map(Self)
    }
}

fn issue_nonce(counter: &AtomicU64, exhausted: &str) -> Result<NonZeroU64, MediaError> {
    let mut value = counter.load(Ordering::Relaxed);
    let value = loop {
        let next = value
            .checked_add(1)
            .filter(|_| value != 0)
            .ok_or_else(|| MediaError::new(MediaErrorKind::Other, exhausted))?;
        match counter.compare_exchange_weak(value, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(previous) => break previous,
            Err(observed) => value = observed,
        }
    };
    NonZeroU64::new(value).ok_or_else(|| MediaError::new(MediaErrorKind::Other, exhausted))
}

/// Confirmed native seek capability and exact signed 100 ns range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaSeekObservation {
    pub revision: MediaObservationRevision,
    pub min_ticks: i64,
    pub max_ticks: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaPlayback {
    Closed,
    Opened,
    Changing,
    Stopped,
    Playing,
    Paused,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaAction {
    Previous,
    Toggle,
    Next,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MediaCapabilities {
    pub previous: bool,
    pub toggle: bool,
    pub next: bool,
}

impl MediaCapabilities {
    pub fn allows(self, action: MediaAction) -> bool {
        match action {
            MediaAction::Previous => self.previous,
            MediaAction::Toggle => self.toggle,
            MediaAction::Next => self.next,
        }
    }
}

/// Owned, tightly packed premultiplied RGBA8, at most 128 pixels per axis.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaArtwork {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl MediaArtwork {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, MediaError> {
        if width == 0 || height == 0 || width > 128 || height > 128 {
            return Err(invalid_artwork(
                "Media artwork dimensions must be between 1 and 128",
            ));
        }
        let expected = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| invalid_artwork("Media artwork dimensions overflow"))?;
        if rgba.len() != expected {
            return Err(invalid_artwork(
                "Media artwork must contain exactly width × height × 4 bytes",
            ));
        }
        if rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[..3].iter().any(|channel| *channel > pixel[3]))
        {
            return Err(invalid_artwork("Media artwork must use premultiplied RGBA"));
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

fn invalid_artwork(message: &str) -> MediaError {
    MediaError::new(MediaErrorKind::Other, message)
}

/// Owned OS timeline facts, in signed 100 ns ticks without normalization.
///
/// Bounds may be inverted, degenerate, or outside the position; consumers must
/// validate them before deriving duration/progress. Observed zero is not absence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaTimeline {
    pub start_ticks: i64,
    pub end_ticks: i64,
    pub position_ticks: i64,
    pub min_seek_ticks: i64,
    pub max_seek_ticks: i64,
    /// UTC wall-clock ticks, not an elapsed time or a monotonic observation age.
    /// A failed timestamp read does not discard the five timeline fields.
    pub last_updated_utc_ticks: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaSession {
    pub key: MediaSessionKey,
    /// Presentation metadata only; never pass this value to launch or transport.
    pub source_app_id: String,
    pub title: String,
    pub author: String,
    pub playback: MediaPlayback,
    pub capabilities: MediaCapabilities,
    /// Independent timeline read failure; metadata/artwork/transports survive.
    pub timeline: Result<MediaTimeline, MediaError>,
    /// Independent capability/range failure; other session facts survive.
    pub seek: Result<MediaSeekObservation, MediaError>,
    pub artwork: Option<MediaArtwork>,
    /// Independent thumbnail failure; usable metadata/controls remain available.
    pub artwork_notice: Option<MediaError>,
}

impl MediaSession {
    /// Bounds Unicode scalar counts at ingress without splitting UTF-8.
    /// Overlong source IDs are cleared, not shortened into another app identity.
    pub fn bounded(mut self) -> Self {
        if self.source_app_id.chars().nth(256).is_some() {
            self.source_app_id.clear();
        }
        truncate_chars(&mut self.title, 512);
        truncate_chars(&mut self.author, 512);
        self
    }
}

fn truncate_chars(value: &mut String, maximum: usize) {
    if let Some((end, _)) = value.char_indices().nth(maximum) {
        value.truncate(end);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MediaSnapshot {
    /// `None` means the OS positively reported no current session.
    pub current: Option<MediaSession>,
}

/// Tessera's target only. Following Windows current does not change its default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MediaSelection {
    #[default]
    FollowCurrent,
    Session(MediaSessionKey),
}

/// Each enumerated incarnation survives independent metadata/playback failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaInventoryEntry {
    pub key: MediaSessionKey,
    pub observation: Result<MediaSession, MediaError>,
    /// Readiness of this source's metadata, playback and timeline registrations.
    /// Inventory hosts report manager delivery readiness through `MediaEvent`;
    /// a healthy manager never implies that every source registered successfully.
    pub watch: Result<(), MediaError>,
}

/// A complete bounded enumeration, never a silently truncated session list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaInventory {
    pub revision: MediaObservationRevision,
    pub sessions: Vec<MediaInventoryEntry>,
    pub native_current: Result<Option<MediaSessionKey>, MediaError>,
    pub selection: MediaSelection,
    /// Missing explicit selection is an error, never a redirect to OS current.
    pub selected: Result<MediaSnapshot, MediaError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaSelectionCommand {
    pub expected_inventory: MediaObservationRevision,
    pub selection: MediaSelection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaCommand {
    pub expected_session: MediaSessionKey,
    pub action: MediaAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaSeekCommand {
    pub expected_session: MediaSessionKey,
    pub expected_revision: MediaObservationRevision,
    pub observed_min_ticks: i64,
    pub observed_max_ticks: i64,
    pub position_ticks: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaRequest {
    Transport(MediaCommand),
    Seek(MediaSeekCommand),
}

impl From<MediaCommand> for MediaRequest {
    fn from(command: MediaCommand) -> Self {
        Self::Transport(command)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaErrorKind {
    Unsupported,
    Unavailable,
    SessionChanged,
    CommandUnavailable,
    Rejected,
    WatchUnavailable,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaError {
    pub kind: MediaErrorKind,
    pub message: String,
    pub hresult: Option<i32>,
}

impl MediaError {
    pub fn new(kind: MediaErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            hresult: None,
        }
    }

    pub fn with_hresult(kind: MediaErrorKind, message: impl Into<String>, code: i32) -> Self {
        Self {
            kind,
            message: message.into(),
            hresult: Some(code),
        }
    }
}

impl fmt::Display for MediaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for MediaError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaEvent {
    Changed,
    /// Current-session/metadata authority was revoked before dirty coalescing.
    /// Position-only timeline notifications do not emit this event.
    SeekInvalidated,
    WatchReady,
    WatchUnavailable(MediaError),
}

pub type MediaReadCompletion = Box<dyn FnOnce(Result<MediaSnapshot, MediaError>) + Send + 'static>;
pub type MediaCommandCompletion = Box<dyn FnOnce(Result<(), MediaError>) + Send + 'static>;
pub type MediaInventoryCompletion =
    Box<dyn FnOnce(Result<MediaInventory, MediaError>) + Send + 'static>;

/// Prompt, nonblocking seam for native media and recording adapters.
///
/// `Ok` from read/execute transfers exactly one completion; immediate `Err`
/// transfers none. Callbacks may run inline or on a worker. Media intents must
/// never be retried automatically, including after initialization failure.
pub trait MediaHost: Send + Sync + 'static {
    fn read(&self, completion: MediaReadCompletion) -> Result<(), MediaError>;

    /// Separate optional capability preserves current-only hosts and literals.
    fn read_inventory(&self, _completion: MediaInventoryCompletion) -> Result<(), MediaError> {
        Err(MediaError::new(
            MediaErrorKind::Unsupported,
            "Media session inventory is not supported by this host",
        ))
    }

    /// Selects Tessera's session, not the read-only GSMTC manager's current one.
    /// Shares read/transport admission; rejected selections are never replayed.
    fn select(
        &self,
        _command: MediaSelectionCommand,
        _completion: MediaCommandCompletion,
    ) -> Result<(), MediaError> {
        Err(MediaError::new(
            MediaErrorKind::Unsupported,
            "Media session selection is not supported by this host",
        ))
    }

    /// Acknowledges OS acceptance, not a confirmed new playback/title snapshot.
    /// Revalidate native identity/capability and (for seek) revision/exact range
    /// immediately before dispatch. Explicitly reread after success; never
    /// mutate confirmed observations optimistically or replay rejected work.
    fn execute(
        &self,
        command: MediaRequest,
        completion: MediaCommandCompletion,
    ) -> Result<(), MediaError>;

    /// `Changed` invalidates facts; `SeekInvalidated` additionally revokes any
    /// unsubmitted seek captured from them. Readiness follows native registration;
    /// a returned guard only accepts asynchronous setup, not live readiness.
    /// `None` means watching is unsupported. Immediate `Err` emits no events.
    /// Guard drop suppresses new delivery and queues cleanup without joining;
    /// a callback already admitted may finish, so consumers generation-gate.
    fn subscribe(
        &self,
        _changed: Arc<dyn Fn(MediaEvent) + Send + Sync>,
    ) -> Result<Option<Box<dyn Send>>, MediaError> {
        Ok(None)
    }
}

#[cfg(test)]
#[path = "media/tests.rs"]
mod tests;
