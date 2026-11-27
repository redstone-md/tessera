// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Which applications play sound, for the speaker on their buttons: the
//! streams of the default output are read away from the thread of GPUI,
//! every second while none plays and four times a second while one does,
//! for as long as a dock shows speakers. The docks are told only when
//! what they show changes: which streams play, which are muted and how
//! loud they are set.

use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{App, AsyncApp, Global};

use crate::platform::{AppSound, StreamSound};

/// How often the streams are read while none plays, and while one does.
const QUIET_EVERY: Duration = Duration::from_secs(1);
const PLAYING_EVERY: Duration = Duration::from_millis(250);

/// The peak below which a stream counts as silent.
const SILENT: f32 = 0.002;

/// For how long a stream counts as playing after it was last heard, so
/// that the speaker does not blink between words or notes.
const HELD: Duration = Duration::from_millis(1500);

/// A stream as the docks show it.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    /// The process that plays and those above it.
    pub lineage: Vec<u32>,
    pub program: String,
    /// Whether it was heard lately.
    pub playing: bool,
    pub muted: bool,
    /// The volume of the stream, in hundredths.
    pub volume: u8,
}

/// What the streams of the default output do, as it was read last.
#[derive(Default)]
pub struct Speakers {
    pub heard: Vec<Heard>,
    /// Held by every dock that shows speakers; the streams are read while
    /// one is.
    watched: Weak<()>,
}

impl Global for Speakers {}

/// The sound of one button: the programs of the streams of its
/// applications, whether any of them plays, and whether all are muted.
#[derive(Clone, Debug, PartialEq)]
pub struct ButtonSound {
    pub programs: Vec<String>,
    pub playing: bool,
    pub muted: bool,
    /// The volume of the loudest of them, from 0 to 1.
    pub volume: f32,
}

impl Speakers {
    /// Has the streams read while what is returned is held.
    pub fn watch(cx: &mut App) -> Rc<()> {
        let speakers = cx.default_global::<Self>();
        if let Some(token) = speakers.watched.upgrade() {
            return token;
        }
        let token = Rc::new(());
        speakers.watched = Rc::downgrade(&token);
        speakers.heard.clear();
        cx.spawn(async move |cx: &mut AsyncApp| Self::read_while_watched(cx).await)
            .detach();
        token
    }

    async fn read_while_watched(cx: &mut AsyncApp) {
        let mut heard_last: HashMap<(Vec<u32>, String), Instant> = HashMap::new();
        loop {
            let streams = cx
                .background_executor()
                .spawn(async { AppSound::playing() })
                .await
                .unwrap_or_default();
            let now = Instant::now();
            let heard = hear(&streams, &mut heard_last, now);
            let playing = heard.iter().any(|stream| stream.playing);
            let kept = cx.update(|cx| {
                // Read first, so that the docks hear only of a change.
                let Some(speakers) = cx.try_global::<Self>() else {
                    return false;
                };
                if speakers.watched.upgrade().is_none() {
                    return false;
                }
                if speakers.heard != heard {
                    cx.global_mut::<Self>().heard = heard;
                }
                true
            });
            if !matches!(kept, Ok(true)) {
                // Nobody shows speakers: the next dock that does starts
                // reading again.
                break;
            }
            let wait = match playing || streams.iter().any(|stream| stream.active) {
                true => PLAYING_EVERY,
                false => QUIET_EVERY,
            };
            cx.background_executor().timer(wait).await;
        }
    }

    /// The sound of a button whose windows belong to `processes`.
    pub fn of_processes(&self, processes: &[u32]) -> Option<ButtonSound> {
        let mine: Vec<&Heard> = { self.heard.iter() }
            .filter(|stream| stream.lineage.iter().any(|id| processes.contains(id)))
            .collect();
        button_sound(&mine)
    }
}

/// What `streams` sound like now, `heard_last` holding when each was last
/// heard, which is brought up to date.
fn hear(
    streams: &[StreamSound],
    heard_last: &mut HashMap<(Vec<u32>, String), Instant>,
    now: Instant,
) -> Vec<Heard> {
    let mut heard = Vec::new();
    for stream in streams {
        let key = (stream.lineage.clone(), stream.program.clone());
        if stream.active && stream.peak > SILENT {
            heard_last.insert(key.clone(), now);
        }
        let playing = { heard_last.get(&key) }.is_some_and(|last| now - *last < HELD);
        heard.push(Heard {
            lineage: stream.lineage.clone(),
            program: stream.program.clone(),
            playing,
            muted: stream.muted,
            volume: (stream.volume.clamp(0., 1.) * 100.).round() as u8,
        });
    }
    heard_last.retain(|_, last| now - *last < HELD);
    heard
}

/// The sound of a button with the streams `mine`: `None` where none of
/// them plays and none is muted.
fn button_sound(mine: &[&Heard]) -> Option<ButtonSound> {
    let playing = mine.iter().any(|stream| stream.playing);
    let muted = !mine.is_empty() && mine.iter().all(|stream| stream.muted);
    if !playing && !muted {
        return None;
    }
    let mut programs: Vec<String> = mine.iter().map(|stream| stream.program.clone()).collect();
    programs.sort();
    programs.dedup();
    let volume = { mine.iter().map(|stream| stream.volume) }
        .max()
        .unwrap_or(100);
    Some(ButtonSound {
        programs,
        playing,
        muted,
        volume: f32::from(volume) / 100.,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(lineage: &[u32], active: bool, peak: f32, muted: bool) -> StreamSound {
        StreamSound {
            lineage: lineage.to_vec(),
            program: "browser".to_owned(),
            active,
            peak,
            volume: 0.5,
            muted,
        }
    }

    #[test]
    fn a_stream_plays_for_a_while_after_it_was_heard() {
        let mut last = HashMap::new();
        let start = Instant::now();
        let loud = [stream(&[30, 20], true, 0.4, false)];
        assert!(hear(&loud, &mut last, start)[0].playing);
        let silent = [stream(&[30, 20], true, 0., false)];
        let soon = start + Duration::from_millis(500);
        assert!(hear(&silent, &mut last, soon)[0].playing);
        let later = start + Duration::from_secs(3);
        let quiet = hear(&silent, &mut last, later);
        assert!(!quiet[0].playing);
        assert_eq!(quiet[0].volume, 50);
        assert!(last.is_empty());
    }

    #[test]
    fn a_button_hears_the_streams_its_processes_started() {
        let playing = Heard {
            lineage: vec![30, 20],
            program: "browser".to_owned(),
            playing: true,
            muted: false,
            volume: 80,
        };
        let speakers = Speakers {
            heard: vec![playing.clone()],
            watched: Weak::new(),
        };
        let sound = speakers.of_processes(&[20]).unwrap();
        assert!(sound.playing && !sound.muted);
        assert_eq!(sound.programs, ["browser"]);
        assert!((sound.volume - 0.8).abs() < 1e-6);
        assert_eq!(speakers.of_processes(&[99]), None);
        let muted = Heard {
            playing: false,
            muted: true,
            ..playing
        };
        let sound = button_sound(&[&muted]).unwrap();
        assert!(sound.muted && !sound.playing);
        let idle = Heard {
            muted: false,
            ..muted
        };
        assert_eq!(button_sound(&[&idle]), None);
    }
}
