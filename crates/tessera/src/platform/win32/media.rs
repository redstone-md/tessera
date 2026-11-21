// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What applications tell Windows they are playing, and the transport
//! controls Windows passes on to them.
//!
//! Every call waits for Windows to answer, so none belongs on the thread
//! that draws.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

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
    /// How far into the piece it is, when the application tells.
    pub position: Option<Position>,
}

/// How far into a piece the player is, in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub elapsed: u32,
    /// How long the piece is; never 0.
    pub length: u32,
}

/// How many of the units Windows counts time in make a second.
const TICKS: i64 = 10_000_000;

/// The seconds between the year 1601, from which Windows counts, and 1970.
const EPOCH_GAP: i64 = 11_644_473_600;

impl Position {
    /// The position an application reported: the piece runs from `start`
    /// to `end`, and was at `at` when the clock showed `reported`, all in
    /// the units of Windows. While it plays, the time passed until `now`
    /// counts too; a speed other than the usual one is not accounted for.
    /// `None` when the piece has no length.
    fn at(start: i64, end: i64, at: i64, reported: i64, now: i64, playing: bool) -> Option<Self> {
        let length = (end - start) / TICKS;
        if length <= 0 {
            return None;
        }
        // Some applications never say when they reported.
        let passed = if playing && reported > 0 {
            (now - reported).max(0)
        } else {
            0
        };
        let elapsed = ((at - start + passed) / TICKS).clamp(0, length);
        Some(Self {
            elapsed: u32::try_from(elapsed).ok()?,
            length: u32::try_from(length).ok()?,
        })
    }
}

/// The time now, in the units of Windows since 1601.
fn now_ticks() -> i64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH);
    let since = since.unwrap_or_default();
    (since.as_secs() as i64 + EPOCH_GAP) * TICKS + i64::from(since.subsec_nanos() / 100)
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
            let playing = playback.PlaybackStatus()? == Status::Playing;
            let position = session.GetTimelineProperties().ok().and_then(|timeline| {
                Position::at(
                    timeline.StartTime().ok()?.Duration,
                    timeline.EndTime().ok()?.Duration,
                    timeline.Position().ok()?.Duration,
                    timeline.LastUpdatedTime().ok()?.UniversalTime,
                    now_ticks(),
                    playing,
                )
            });
            Ok(Some(NowPlaying {
                title: properties.Title()?.to_string().trim().to_owned(),
                artist: properties.Artist()?.to_string().trim().to_owned(),
                playing,
                position,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_position_counts_the_time_passed_only_while_it_plays() {
        let second = TICKS;
        let paused = Position::at(0, 200 * second, 30 * second, 1000, 1000 + 5 * second, false);
        assert_eq!(
            paused,
            Some(Position {
                elapsed: 30,
                length: 200
            })
        );
        let playing = Position::at(0, 200 * second, 30 * second, 1000, 1000 + 5 * second, true);
        assert_eq!(playing.map(|position| position.elapsed), Some(35));
    }

    #[test]
    fn a_position_stays_within_the_piece_and_needs_a_length() {
        let second = TICKS;
        let late = Position::at(0, 10 * second, 9 * second, 1000, 1000 + 60 * second, true);
        assert_eq!(late.map(|position| position.elapsed), Some(10));
        // No time of the report: nothing is added.
        let unknown = Position::at(0, 10 * second, 4 * second, 0, 1000 + 60 * second, true);
        assert_eq!(unknown.map(|position| position.elapsed), Some(4));
        assert_eq!(Position::at(0, 0, 0, 0, 0, true), None);
    }
}
