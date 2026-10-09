// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pinned windows 0.62.2 calls, confined to the MTA owner thread.
#![allow(unsafe_code)]

use super::{
    artwork::{DecodePlan, copied_size, read_count},
    owner::{Calls, Event, nullable_result},
};
use std::{marker::PhantomData, ptr::null_mut, rc::Rc, sync::Arc};
use tessera_system::media::{
    MediaAction, MediaArtwork, MediaCapabilities, MediaError, MediaErrorKind, MediaSession,
    MediaSessionKey,
};
use windows::{
    Foundation::TypedEventHandler,
    Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
        ExifOrientationMode,
    },
    Media::Control::{
        CurrentSessionChangedEventArgs, GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionMediaProperties as Properties,
        MediaPropertiesChangedEventArgs, PlaybackInfoChangedEventArgs, SessionsChangedEventArgs,
    },
    Storage::Streams::{
        Buffer, IRandomAccessStreamReference, IRandomAccessStreamWithContentType,
        InMemoryRandomAccessStream, InputStreamOptions,
    },
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    core::{IUnknown, Interface},
};

const MAX_SESSIONS: u32 = 256;

// !Send/!Sync even though some generated WinRT wrappers are agile. Apartment
// initialization, event removal, releases and uninitialization stay on owner.
pub(super) struct Apartment(PhantomData<Rc<()>>);

impl Apartment {
    pub(super) fn new() -> Result<Self, MediaError> {
        // SAFETY: invoked only by the media actor owner or in-memory SDK tests.
        // Both S_OK and S_FALSE acquire one balance; failure acquires none.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map_err(|e| error(MediaErrorKind::Unavailable, "Initialize media MTA", e))?;
        Ok(Self(PhantomData))
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: successful matching RoInitialize on this same owner thread.
        unsafe { RoUninitialize() };
    }
}

pub(super) struct WindowsCalls {
    // Field order matters: release manager BEFORE the apartment guard.
    manager: Manager,
    _apartment: Apartment,
}

#[derive(Clone)]
pub(super) struct NativeSession {
    object: Session,
    identity: IUnknown,
}

impl NativeSession {
    fn new(object: Session) -> Result<Self, MediaError> {
        let identity = object.cast::<IUnknown>().map_err(|e| {
            error(
                MediaErrorKind::Unavailable,
                "Resolve canonical media session identity",
                e,
            )
        })?;
        Ok(Self { object, identity })
    }
}

impl PartialEq for NativeSession {
    fn eq(&self, other: &Self) -> bool {
        // Both interfaces were queried for IID_IUnknown, whose pointer is the
        // canonical identity. Held references prevent recycled-address aliases.
        self.identity.as_raw() == other.identity.as_raw()
    }
}
impl Eq for NativeSession {}

pub(super) enum Token {
    Sessions(Manager, i64),
    Current(Manager, i64),
    Properties(Session, i64),
    Playback(Session, i64),
}

impl WindowsCalls {
    pub(super) fn new() -> Result<Self, MediaError> {
        let apartment = Apartment::new()?;
        // windows-future 0.3.2 uses join(), not the older get(). This waits only
        // on the actor owner; service construction/UI/drop never waits or joins.
        let manager = Manager::RequestAsync()
            .and_then(|operation| operation.join())
            .map_err(|e| error(MediaErrorKind::Unavailable, "Request GSMTC manager", e))?;
        Ok(Self {
            manager,
            _apartment: apartment,
        })
    }
}

impl Calls for WindowsCalls {
    type Session = NativeSession;
    type Token = Token;

    fn sessions(&mut self) -> Result<Vec<NativeSession>, MediaError> {
        let sessions = self.manager.GetSessions().map_err(|e| {
            error(
                MediaErrorKind::Unavailable,
                "Enumerate live media sessions",
                e,
            )
        })?;
        let count = sessions
            .Size()
            .map_err(|e| error(MediaErrorKind::Unavailable, "Read media session count", e))?;
        if count > MAX_SESSIONS {
            return Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "Media session count exceeds the identity bookkeeping budget",
            ));
        }
        (0..count)
            .map(|index| {
                NativeSession::new(sessions.GetAt(index).map_err(|e| {
                    error(MediaErrorKind::Unavailable, "Read live media session", e)
                })?)
            })
            .collect()
    }

    fn current(&mut self) -> Result<Option<NativeSession>, MediaError> {
        let mut raw = null_mut();
        // SAFETY: the exact installed manager vtable writes an owned default
        // session interface. The generated convenience method uses from_abi,
        // which loses successful-null vs a real E_POINTER failure; use the ABI
        // once here to retain that distinction. No failure HRESULT becomes None.
        let status = unsafe {
            (Interface::vtable(&self.manager).GetCurrentSession)(self.manager.as_raw(), &mut raw)
        };
        if !nullable_result(status.0, !raw.is_null(), "GetCurrentSession")? {
            return Ok(None);
        }
        // SAFETY: successful nonnull out pointer owns exactly one reference to
        // the default Session interface; from_raw transfers that reference.
        NativeSession::new(unsafe { Session::from_raw(raw) }).map(Some)
    }

    fn snapshot(
        &mut self,
        session: &NativeSession,
        key: MediaSessionKey,
    ) -> Result<MediaSession, MediaError> {
        let properties = session
            .object
            .TryGetMediaPropertiesAsync()
            .and_then(|operation| operation.join())
            .map_err(|e| {
                error(
                    MediaErrorKind::Unavailable,
                    "Read current media properties",
                    e,
                )
            })?;
        let source_app_id = session
            .object
            .SourceAppUserModelId()
            .map_err(|e| error(MediaErrorKind::Unavailable, "Read media source metadata", e))?
            .to_string_lossy();
        let title = properties
            .Title()
            .map_err(|e| error(MediaErrorKind::Unavailable, "Read media title", e))?
            .to_string_lossy();
        let author = properties
            .Artist()
            .map_err(|e| error(MediaErrorKind::Unavailable, "Read media artist", e))?
            .to_string_lossy();
        let info = session
            .object
            .GetPlaybackInfo()
            .map_err(|e| error(MediaErrorKind::Unavailable, "Read media playback", e))?;
        let status = info
            .PlaybackStatus()
            .map_err(|e| error(MediaErrorKind::Unavailable, "Read media playback status", e))?;
        let playback = super::owner::playback(status.0)?;
        let capabilities = self.capabilities(session)?;
        let (artwork, artwork_notice) = match thumbnail(&properties) {
            Ok(artwork) => (artwork, None),
            Err(error) => (None, Some(error)),
        };
        Ok(MediaSession {
            key,
            source_app_id,
            title,
            author,
            playback,
            capabilities,
            artwork,
            artwork_notice,
        })
    }

    fn capabilities(&mut self, session: &NativeSession) -> Result<MediaCapabilities, MediaError> {
        let read = || -> windows::core::Result<MediaCapabilities> {
            let controls = session.object.GetPlaybackInfo()?.Controls()?;
            Ok(MediaCapabilities {
                previous: controls.IsPreviousEnabled()?,
                toggle: controls.IsPlayPauseToggleEnabled()?,
                next: controls.IsNextEnabled()?,
            })
        };
        read().map_err(|e| {
            error(
                MediaErrorKind::Unavailable,
                "Read current media transport capabilities",
                e,
            )
        })
    }

    fn transport(
        &mut self,
        session: &NativeSession,
        action: MediaAction,
    ) -> Result<bool, MediaError> {
        let operation = match action {
            MediaAction::Previous => session.object.TrySkipPreviousAsync(),
            MediaAction::Toggle => session.object.TryTogglePlayPauseAsync(),
            MediaAction::Next => session.object.TrySkipNextAsync(),
        };
        operation
            .and_then(|operation| operation.join())
            .map_err(|e| {
                error(
                    MediaErrorKind::Rejected,
                    "Request current media transport",
                    e,
                )
            })
    }

    fn register(
        &mut self,
        event: Event,
        session: Option<&NativeSession>,
        dirty: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Token, MediaError> {
        // Delegates contain only a generation-gated dirty callback. No manager,
        // session, owner borrow, decode, or transport work occurs in a callback.
        let result = match event {
            Event::Sessions => self
                .manager
                .SessionsChanged(
                    &TypedEventHandler::<Manager, SessionsChangedEventArgs>::new(move |_, _| {
                        dirty();
                        Ok(())
                    }),
                )
                .map(|token| Token::Sessions(self.manager.clone(), token)),
            Event::Current => {
                self.manager
                    .CurrentSessionChanged(&TypedEventHandler::<
                        Manager,
                        CurrentSessionChangedEventArgs,
                    >::new(move |_, _| {
                        dirty();
                        Ok(())
                    }))
                    .map(|token| Token::Current(self.manager.clone(), token))
            }
            Event::Properties => {
                let session = session.ok_or_else(|| {
                    MediaError::new(
                        MediaErrorKind::WatchUnavailable,
                        "Missing media event source",
                    )
                })?;
                session
                    .object
                    .MediaPropertiesChanged(&TypedEventHandler::<
                        Session,
                        MediaPropertiesChangedEventArgs,
                    >::new(move |_, _| {
                        dirty();
                        Ok(())
                    }))
                    .map(|token| Token::Properties(session.object.clone(), token))
            }
            Event::Playback => {
                let session = session.ok_or_else(|| {
                    MediaError::new(
                        MediaErrorKind::WatchUnavailable,
                        "Missing media event source",
                    )
                })?;
                session
                    .object
                    .PlaybackInfoChanged(
                        &TypedEventHandler::<Session, PlaybackInfoChangedEventArgs>::new(
                            move |_, _| {
                                dirty();
                                Ok(())
                            },
                        ),
                    )
                    .map(|token| Token::Playback(session.object.clone(), token))
            }
        };
        result.map_err(|e| {
            error(
                MediaErrorKind::WatchUnavailable,
                "Register media invalidation",
                e,
            )
        })
    }

    fn remove(&mut self, token: Token) {
        // Removal is best-effort because frozen stop_watch has no result. The
        // callback generation is already disabled even if Windows removal fails.
        let _ = match token {
            Token::Sessions(source, token) => source.RemoveSessionsChanged(token),
            Token::Current(source, token) => source.RemoveCurrentSessionChanged(token),
            Token::Properties(source, token) => source.RemoveMediaPropertiesChanged(token),
            Token::Playback(source, token) => source.RemovePlaybackInfoChanged(token),
        };
    }
}

fn thumbnail(properties: &Properties) -> Result<Option<MediaArtwork>, MediaError> {
    let mut raw = null_mut();
    // SAFETY: installed media-properties vtable writes an owned stream reference;
    // distinguish confirmed absence from failures, as with GetCurrentSession.
    let status =
        unsafe { (Interface::vtable(properties).Thumbnail)(properties.as_raw(), &mut raw) };
    if !nullable_result(status.0, !raw.is_null(), "Read media thumbnail")? {
        return Ok(None);
    }
    // SAFETY: successful nonnull out pointer transfers one owned reference.
    let reference = unsafe { IRandomAccessStreamReference::from_raw(raw) };
    decode_reference(&reference).map(Some)
}

/// Shared production decode seam; tests supply only an in-memory reference.
pub(super) fn decode_reference(
    reference: &IRandomAccessStreamReference,
) -> Result<MediaArtwork, MediaError> {
    let source = SourceStream(
        reference
            .OpenReadAsync()
            .and_then(|operation| operation.join())
            .map_err(|e| error(MediaErrorKind::Other, "Open media thumbnail", e))?,
    );
    let decode = || -> Result<MediaArtwork, MediaError> {
        let size = source.0.Size().map_err(art_error)?;
        let count = read_count(size)?;
        source.0.Seek(0).map_err(art_error)?;
        let buffer = Buffer::Create(count).map_err(art_error)?;
        let buffer = source
            .0
            .ReadAsync(&buffer, count, InputStreamOptions::None)
            .and_then(|operation| operation.join())
            .map_err(art_error)?;
        copied_size(
            size,
            buffer.Length().map_err(art_error)?,
            source.0.Size().map_err(art_error)?,
        )?;
        let copy = MemoryStream(InMemoryRandomAccessStream::new().map_err(art_error)?);
        let written = copy
            .0
            .WriteAsync(&buffer)
            .and_then(|operation| operation.join())
            .map_err(art_error)?;
        copied_size(size, written, copy.0.Size().map_err(art_error)?)?;
        copy.0.Seek(0).map_err(art_error)?;
        let decoder = BitmapDecoder::CreateAsync(&copy.0)
            .and_then(|operation| operation.join())
            .map_err(art_error)?;
        let plan = DecodePlan::new(
            decoder.PixelWidth().map_err(art_error)?,
            decoder.PixelHeight().map_err(art_error)?,
        )?;
        let transform = BitmapTransform::new().map_err(art_error)?;
        transform.SetScaledWidth(plan.width).map_err(art_error)?;
        transform.SetScaledHeight(plan.height).map_err(art_error)?;
        let pixels = decoder
            .GetPixelDataTransformedAsync(
                BitmapPixelFormat::Rgba8,
                BitmapAlphaMode::Premultiplied,
                &transform,
                ExifOrientationMode::IgnoreExifOrientation,
                ColorManagementMode::ColorManageToSRgb,
            )
            .and_then(|operation| operation.join())
            .map_err(art_error)?;
        let rgba = pixels.DetachPixelData().map_err(art_error)?;
        plan.accept(&rgba)
    };
    decode()
}

// RAII covers every early-return/unwind after an opened stream. Close and
// interface release occur on owner, before manager/apartment retirement.
struct SourceStream(IRandomAccessStreamWithContentType);
impl Drop for SourceStream {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}
struct MemoryStream(InMemoryRandomAccessStream);
impl Drop for MemoryStream {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}

fn art_error(error: windows::core::Error) -> MediaError {
    self::error(MediaErrorKind::Other, "Decode media thumbnail", error)
}

fn error(kind: MediaErrorKind, operation: &str, error: windows::core::Error) -> MediaError {
    MediaError::with_hresult(kind, format!("{operation}: {error}"), error.code().0)
}
