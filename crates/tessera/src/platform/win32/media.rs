// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What applications tell Windows they are playing, and the transport
//! controls Windows passes on to them. Each application that plays has a
//! session of its own; one of them is the one Windows considers current,
//! which the media keys go to, and any of them can be read and controlled.
//!
//! Every call waits for Windows to answer, so none belongs on the thread
//! that draws.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Media::MediaPlaybackAutoRepeatMode as RepeatMode;
use windows::Storage::Streams::DataReader;
use windows::core::Result as WindowsResult;

/// The largest picture read; what is offered is a few hundred kilobytes
/// at most.
const COVER_LIMIT: u64 = 8 << 20;

/// What an application is playing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NowPlaying {
    /// The application, as Windows knows it: the identifier of a packaged
    /// application, or the name of a program file, as `Spotify.exe`.
    pub app: String,
    /// May be empty: applications fill in what they like.
    pub title: String,
    pub artist: String,
    /// Whether it plays right now, rather than being paused or stopped.
    pub playing: bool,
    /// Whether the application accepts each command.
    pub can_toggle: bool,
    pub can_previous: bool,
    pub can_next: bool,
    /// Whether it lets the position be set.
    pub can_seek: bool,
    /// Whether it plays in shuffled order; `None` where it does not tell.
    pub shuffle: Option<bool>,
    pub can_shuffle: bool,
    /// What it plays again once at the end; `None` where it does not tell.
    pub repeat: Option<Repeat>,
    pub can_repeat: bool,
    /// How far into the piece it is, when the application tells.
    pub position: Option<Position>,
}

/// What a player plays again once it is at the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeat {
    /// Nothing: it stops.
    Off,
    /// The piece it plays.
    Track,
    /// The whole list.
    List,
}

impl Repeat {
    /// The mode a button for it goes to next, as players go through them.
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::List,
            Self::List => Self::Track,
            Self::Track => Self::Off,
        }
    }

    fn of(mode: RepeatMode) -> Option<Self> {
        match mode {
            RepeatMode::None => Some(Self::Off),
            RepeatMode::Track => Some(Self::Track),
            RepeatMode::List => Some(Self::List),
            _ => None,
        }
    }

    fn mode(self) -> RepeatMode {
        match self {
            Self::Off => RepeatMode::None,
            Self::Track => RepeatMode::Track,
            Self::List => RepeatMode::List,
        }
    }
}

/// Tells one media session from another. An application may have several
/// sessions, as a browser has one for each tab or window that plays, all
/// under the same name: they are told apart by their order.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionKey {
    /// As [`NowPlaying::app`].
    pub app: String,
    /// How many sessions of the same application come before it, in the
    /// order of Windows.
    pub occurrence: usize,
}

/// A media session, for a choice between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub key: SessionKey,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// Whether Windows considers it current: the media keys go to it.
    pub current: bool,
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
    Play,
    Pause,
    Stop,
    Next,
    /// Go to this many seconds into the piece.
    Seek(u32),
    /// Play in shuffled order, or in order.
    Shuffle(bool),
    Repeat(Repeat),
}

/// The media applications of the signed-in user. Where a call takes a
/// session, `None` is the one Windows considers current, and so is a
/// session that is gone.
pub struct Media;

impl Media {
    /// Every session, in the order of Windows. A session that cannot be
    /// read is left out.
    pub fn sessions() -> Result<Vec<SessionInfo>, String> {
        let read = || -> WindowsResult<Vec<SessionInfo>> {
            let manager = manager()?;
            let current = manager.GetCurrentSession().ok();
            let sessions = manager.GetSessions()?;
            let mut listed = Vec::new();
            // How many sessions of each application came before.
            let mut seen = HashMap::<String, usize>::new();
            for index in 0..sessions.Size()? {
                let Ok(session) = sessions.GetAt(index) else {
                    continue;
                };
                let Ok(app) = session.SourceAppUserModelId() else {
                    continue;
                };
                let app = app.to_string();
                // Counted even if it is left out below, as `session` counts.
                let seen = seen.entry(app.clone()).or_default();
                let occurrence = *seen;
                *seen += 1;
                // An application may still be starting and tell nothing.
                let properties = session
                    .TryGetMediaPropertiesAsync()
                    .and_then(|asked| asked.get());
                let text = |read: WindowsResult<windows::core::HSTRING>| {
                    read.map(|text| text.to_string().trim().to_owned())
                        .unwrap_or_default()
                };
                let (title, artist) = match &properties {
                    Ok(properties) => (text(properties.Title()), text(properties.Artist())),
                    Err(_) => Default::default(),
                };
                let playing = session
                    .GetPlaybackInfo()
                    .and_then(|playback| playback.PlaybackStatus());
                let Ok(playing) = playing else {
                    continue;
                };
                listed.push(SessionInfo {
                    // The same object, as COM tells: an application may
                    // have more than one session.
                    current: current.as_ref() == Some(&session),
                    key: SessionKey { app, occurrence },
                    title,
                    artist,
                    playing: playing == Status::Playing,
                });
            }
            // Should Windows give another object for the current session,
            // the first session of its application stands for it.
            let current = current.and_then(|session| session.SourceAppUserModelId().ok());
            if !listed.iter().any(|session| session.current)
                && let Some(current) = current.map(|app| app.to_string())
                && let Some(first) = listed.iter_mut().find(|session| session.key.app == current)
            {
                first.current = true;
            }
            Ok(listed)
        };
        read().map_err(|error| format!("the media applications could not be listed: {error}"))
    }

    /// What `key` plays; `None` while no application offers anything.
    pub fn now_playing(key: Option<&SessionKey>) -> Result<Option<NowPlaying>, String> {
        let read = || -> WindowsResult<Option<NowPlaying>> {
            let Some(session) = session(key)? else {
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
            // Without a value Windows answers with nothing, which reads as
            // an error.
            let shuffle = playback
                .IsShuffleActive()
                .and_then(|shuffle| shuffle.Value());
            let repeat = playback.AutoRepeatMode().and_then(|repeat| repeat.Value());
            Ok(Some(NowPlaying {
                app: session.SourceAppUserModelId()?.to_string(),
                title: properties.Title()?.to_string().trim().to_owned(),
                artist: properties.Artist()?.to_string().trim().to_owned(),
                playing,
                position,
                can_toggle: controls.IsPlayPauseToggleEnabled()?,
                can_previous: controls.IsPreviousEnabled()?,
                can_next: controls.IsNextEnabled()?,
                can_seek: controls.IsPlaybackPositionEnabled()?,
                shuffle: shuffle.ok(),
                can_shuffle: controls.IsShuffleEnabled()?,
                repeat: repeat.ok().and_then(Repeat::of),
                can_repeat: controls.IsRepeatEnabled()?,
            }))
        };
        read().map_err(|error| format!("what is playing could not be read: {error}"))
    }

    /// The picture `key` shows for what it plays, as the image file it
    /// offers; `None` while there is none.
    pub fn cover(key: Option<&SessionKey>) -> Result<Option<Vec<u8>>, String> {
        let read = || -> WindowsResult<Option<Vec<u8>>> {
            let Some(session) = session(key)? else {
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

    /// Passes `command` to `key`. Whether the application followed it is
    /// not known.
    pub fn send(key: Option<&SessionKey>, command: MediaCommand) -> Result<(), String> {
        let send = || -> WindowsResult<()> {
            let Some(session) = session(key)? else {
                return Ok(());
            };
            match command {
                MediaCommand::Previous => session.TrySkipPreviousAsync()?.get()?,
                MediaCommand::Toggle => session.TryTogglePlayPauseAsync()?.get()?,
                MediaCommand::Play => session.TryPlayAsync()?.get()?,
                MediaCommand::Pause => session.TryPauseAsync()?.get()?,
                MediaCommand::Stop => session.TryStopAsync()?.get()?,
                MediaCommand::Next => session.TrySkipNextAsync()?.get()?,
                MediaCommand::Seek(seconds) => {
                    let start = session.GetTimelineProperties()?.StartTime()?.Duration;
                    let position = start + i64::from(seconds) * TICKS;
                    session.TryChangePlaybackPositionAsync(position)?.get()?
                }
                MediaCommand::Shuffle(on) => session.TryChangeShuffleActiveAsync(on)?.get()?,
                MediaCommand::Repeat(repeat) => {
                    session.TryChangeAutoRepeatModeAsync(repeat.mode())?.get()?
                }
            };
            Ok(())
        };
        send().map_err(|error| format!("the media application could not be reached: {error}"))
    }
}

/// What tells Windows of the sessions, asked for once.
fn manager() -> WindowsResult<&'static Manager> {
    static MANAGER: OnceLock<Manager> = OnceLock::new();
    match MANAGER.get() {
        Some(manager) => Ok(manager),
        None => {
            let manager = Manager::RequestAsync()?.get()?;
            Ok(MANAGER.get_or_init(|| manager))
        }
    }
}

/// The session `key` names, or the one Windows considers current, if any
/// application has one.
fn session(key: Option<&SessionKey>) -> WindowsResult<Option<Session>> {
    let manager = manager()?;
    if let Some(key) = key {
        let sessions = manager.GetSessions()?;
        let mut before = 0;
        for index in 0..sessions.Size()? {
            let Ok(session) = sessions.GetAt(index) else {
                continue;
            };
            if session
                .SourceAppUserModelId()
                .is_ok_and(|app| app == key.app.as_str())
            {
                if before == key.occurrence {
                    return Ok(Some(session));
                }
                before += 1;
            }
        }
    }
    // Windows answers with nothing, which reads as an error, when no
    // application has a session.
    Ok(manager.GetCurrentSession().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repeat_button_goes_through_the_modes() {
        assert_eq!(Repeat::Off.next(), Repeat::List);
        assert_eq!(Repeat::List.next(), Repeat::Track);
        assert_eq!(Repeat::Track.next(), Repeat::Off);
        for repeat in [Repeat::Off, Repeat::Track, Repeat::List] {
            assert_eq!(Repeat::of(repeat.mode()), Some(repeat));
        }
        assert_eq!(Repeat::of(RepeatMode(7)), None);
    }

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
