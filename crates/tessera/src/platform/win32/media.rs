// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What applications tell Windows they are playing, and the transport
//! controls Windows passes on to them.
//!
//! Every call waits for Windows to answer, so none belongs on the thread
//! that draws.

use std::sync::OnceLock;

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::core::Result as WindowsResult;

/// The largest picture read; what is offered is a few hundred kilobytes
/// at most.
const COVER_LIMIT: u64 = 8 << 20;

/// What the application Windows considers current is playing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NowPlaying {
    /// May be empty: applications fill in what they like.
    pub title: String,
    pub artist: String,
    /// Whether it plays right now, rather than being paused or stopped.
    pub playing: bool,
    /// Whether the application accepts each command.
    pub can_toggle: bool,
    pub can_previous: bool,
    pub can_next: bool,
}

/// What a media application can be asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCommand {
    Previous,
    /// Pause if playing, play otherwise.
    Toggle,
    Next,
}

/// The media applications of the signed-in user.
pub struct Media;

impl Media {
    /// What the current media application plays; `None` while no
    /// application offers anything.
    pub fn now_playing() -> Result<Option<NowPlaying>, String> {
        let read = || -> WindowsResult<Option<NowPlaying>> {
            let Some(session) = session()? else {
                return Ok(None);
            };
            let properties = session.TryGetMediaPropertiesAsync()?.get()?;
            let playback = session.GetPlaybackInfo()?;
            let controls = playback.Controls()?;
            Ok(Some(NowPlaying {
                title: properties.Title()?.to_string().trim().to_owned(),
                artist: properties.Artist()?.to_string().trim().to_owned(),
                playing: playback.PlaybackStatus()? == Status::Playing,
                can_toggle: controls.IsPlayPauseToggleEnabled()?,
                can_previous: controls.IsPreviousEnabled()?,
                can_next: controls.IsNextEnabled()?,
            }))
        };
        read().map_err(|error| format!("what is playing could not be read: {error}"))
    }

    /// The picture the current media application shows for what it plays,
    /// as the image file it offers; `None` while there is none.
    pub fn cover() -> Result<Option<Vec<u8>>, String> {
        let read = || -> WindowsResult<Option<Vec<u8>>> {
            let Some(session) = session()? else {
                return Ok(None);
            };
            let properties = session.TryGetMediaPropertiesAsync()?.get()?;
            // Without a picture Windows answers with nothing, which reads
            // as an error.
            let Ok(picture) = properties.Thumbnail() else {
                return Ok(None);
            };
            let stream = picture.OpenReadAsync()?.get()?;
            let size = stream.Size()?;
            if size == 0 || size > COVER_LIMIT {
                return Ok(None);
            }
            let reader = DataReader::CreateDataReader(&stream)?;
            reader.LoadAsync(size as u32)?.get()?;
            let mut bytes = vec![0; size as usize];
            reader.ReadBytes(&mut bytes)?;
            Ok(Some(bytes))
        };
        read().map_err(|error| format!("the picture of what is playing could not be read: {error}"))
    }

    /// Passes `command` to the current media application. Whether the
    /// application followed it is not known.
    pub fn send(command: MediaCommand) -> Result<(), String> {
        let send = || -> WindowsResult<()> {
            let Some(session) = session()? else {
                return Ok(());
            };
            match command {
                MediaCommand::Previous => session.TrySkipPreviousAsync()?.get()?,
                MediaCommand::Toggle => session.TryTogglePlayPauseAsync()?.get()?,
                MediaCommand::Next => session.TrySkipNextAsync()?.get()?,
            };
            Ok(())
        };
        send().map_err(|error| format!("the media application could not be reached: {error}"))
    }
}

/// The session Windows considers current, if any application has one.
fn session() -> WindowsResult<Option<Session>> {
    static MANAGER: OnceLock<Manager> = OnceLock::new();
    let manager = match MANAGER.get() {
        Some(manager) => manager,
        None => {
            let manager = Manager::RequestAsync()?.get()?;
            MANAGER.get_or_init(|| manager)
        }
    };
    // Windows answers with nothing, which reads as an error, when no
    // application has a session.
    Ok(manager.GetCurrentSession().ok())
}
