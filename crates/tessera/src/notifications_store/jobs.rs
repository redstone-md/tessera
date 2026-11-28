// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The jobs on files (copying, moving, deleting, packing archives) as the
//! notification center shows them, from what they publish through
//! [`FileJobs`]: while one runs, its progress, speed and time left; once
//! it ends, a notification in the history that opens where its items went.
//! A job that took longer than a few seconds also ends with a banner,
//! unless a window of Tessera is in front, where the user saw it end.
//!
//! Subscribing is what tells the file operations that their progress is
//! shown here, so that the Shell need not show its own window of it; it
//! is done only under `[notifications] jobs = true`.

use std::time::{Duration, Instant};

use gpui::{App, AsyncApp};

use super::{NotificationStore, Watch};
use crate::banners::{Banners, Notice, NoticeAction};
use crate::i18n::Language;
use crate::platform::{FileJobEvent, FileJobState, FileJobs, Launch, reveal, tessera_in_front};

/// How often the progress of the jobs is taken in, at most.
const PACE: Duration = Duration::from_millis(250);

/// A job that ran at least this long ends with a banner.
const LONG_JOB: Duration = Duration::from_secs(5);

/// How much of a new measure of the speed counts against the speed until
/// then, so that the time left does not jump.
const SMOOTHING: f64 = 0.3;

/// The glyph of the folder on the banner of a job.
const FOLDER_GLYPH: &str = "\u{E8B7}";

/// A job on files and how fast it goes.
#[derive(Clone, Debug)]
pub struct Job {
    /// What it published last.
    pub event: FileJobEvent,
    started: Instant,
    /// Bytes a second, smoothed; none until measured.
    speed: Option<f64>,
    /// When the bytes were last measured, and how many were done then.
    measured: (Instant, u64),
}

impl Job {
    fn new(event: FileJobEvent, now: Instant) -> Self {
        let done = event.bytes.0;
        Self {
            event,
            started: now,
            speed: None,
            measured: (now, done),
        }
    }

    /// Takes in what it published now.
    fn update(&mut self, event: FileJobEvent, now: Instant) {
        let (then, done_then) = self.measured;
        let seconds = now.duration_since(then).as_secs_f64();
        if seconds >= PACE.as_secs_f64() && event.bytes.0 >= done_then {
            let measure = (event.bytes.0 - done_then) as f64 / seconds;
            self.speed = Some(smoothed(self.speed, measure));
            self.measured = (now, event.bytes.0);
        }
        self.event = event;
    }

    /// Bytes a second, once measured.
    pub fn speed(&self) -> Option<f64> {
        self.speed.filter(|speed| *speed > 0.)
    }

    /// How long it still takes at the speed it goes.
    pub fn time_left(&self) -> Option<Duration> {
        time_left(self.event.bytes, self.speed()?)
    }

    /// The share done, from 0 to 1, by the bytes or else the items; none
    /// while neither is known.
    pub fn share(&self) -> Option<f32> {
        let share = |(done, all): (u64, u64)| {
            (all > 0).then(|| (done as f64 / all as f64).clamp(0., 1.) as f32)
        };
        share(self.event.bytes).or_else(|| share(self.event.items))
    }

    /// What its line under the bar says: items, bytes, speed and time left.
    pub fn progress_text(&self, language: Language) -> String {
        let mut parts = Vec::new();
        let (done, all) = self.event.items;
        if all > 0 {
            parts.push(format!("{done} / {all} {}", language.say("items")));
        }
        let (done, all) = self.event.bytes;
        if all > 0 {
            parts.push(format!("{} / {}", size_text(done), size_text(all)));
        }
        if let Some(speed) = self.speed() {
            parts.push(format!("{}/s", size_text(speed as u64)));
        }
        if let Some(left) = self.time_left() {
            parts.push(format!(
                "{} {}",
                duration_text(left, language),
                language.say("left")
            ));
        }
        parts.join(" \u{b7} ")
    }
}

/// The speed after `measure`, given the speed until then.
fn smoothed(speed: Option<f64>, measure: f64) -> f64 {
    match speed {
        Some(speed) => speed + (measure - speed) * SMOOTHING,
        None => measure,
    }
}

/// How long the bytes `(done, all)` still take at `speed` bytes a second.
fn time_left((done, all): (u64, u64), speed: f64) -> Option<Duration> {
    (all > done && speed > 0.).then(|| Duration::from_secs_f64((all - done) as f64 / speed))
}

/// A number of bytes as people read it.
fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    match unit {
        0 => format!("{bytes} B"),
        _ if value < 10. => format!("{value:.1} {}", UNITS[unit]),
        _ => format!("{value:.0} {}", UNITS[unit]),
    }
}

/// A span of time in its largest unit, rounded up.
fn duration_text(span: Duration, language: Language) -> String {
    let seconds = span.as_secs().max(1);
    match seconds {
        ..60 => format!("{seconds} {}", language.say("s")),
        60..3600 => format!("{} {}", seconds.div_ceil(60), language.say("min")),
        _ => format!("{} {}", seconds.div_ceil(3600), language.say("h")),
    }
}

/// Subscribes to the jobs on files where `on`, and lets go of them
/// otherwise.
pub(super) fn configure(on: bool, cx: &mut App) {
    let watch = cx.default_global::<Watch>();
    match (on, watch.jobs.is_some()) {
        (true, false) => {
            let listening = cx.spawn(listen);
            cx.global_mut::<Watch>().jobs = Some(listening);
        }
        (false, true) => {
            // Dropping the task drops the receiver, which unsubscribes.
            watch.jobs = None;
            cx.global_mut::<NotificationStore>().jobs.clear();
        }
        _ => {}
    }
}

/// Takes in the events of the jobs, a batch at a time.
async fn listen(cx: &mut AsyncApp) {
    let events = FileJobs::subscribe();
    while let Ok(first) = events.recv().await {
        let mut batch = vec![first];
        while let Ok(more) = events.try_recv() {
            batch.push(more);
        }
        if cx.update(|cx| take(batch, cx)).is_err() {
            break;
        }
        cx.background_executor().timer(PACE).await;
    }
}

/// Takes in what the jobs published.
fn take(batch: Vec<FileJobEvent>, cx: &mut App) {
    let now = Instant::now();
    let mut ended = Vec::new();
    let jobs = &mut cx.global_mut::<NotificationStore>().jobs;
    for event in batch {
        let at = jobs.iter().position(|job| job.event.id == event.id);
        match (at, &event.state) {
            (Some(at), FileJobState::Running) => jobs[at].update(event, now),
            (None, FileJobState::Running) => jobs.push(Job::new(event, now)),
            (Some(at), _) => {
                let mut job = jobs.remove(at);
                job.update(event, now);
                ended.push(job);
            }
            // A job that ended before it was heard of ran too briefly to
            // tell of.
            (None, _) => {}
        }
    }
    for job in ended {
        end(job, now, cx);
    }
}

/// Keeps a job that ended in the history, and tells of it with a banner
/// where it took long and no window of Tessera is in front.
fn end(job: Job, now: Instant, cx: &mut App) {
    let language = Language::of(cx);
    let event = job.event;
    let body = match &event.state {
        FileJobState::Done => language.say("Done").to_owned(),
        FileJobState::Cancelled => language.say("Cancelled").to_owned(),
        FileJobState::Failed(reason) => format!("{}: {reason}", language.say("Failed")),
        FileJobState::Running => return,
    };
    let app = language.say("Files").to_owned();
    let long = now.duration_since(job.started) >= LONG_JOB;
    let destination = event
        .destination
        .clone()
        .filter(|_| event.state == FileJobState::Done);
    let key = if long && !tessera_in_front() {
        let mut actions = Vec::new();
        if let Some(folder) = destination.clone() {
            let shown = folder.clone();
            actions.push(NoticeAction::new(
                language.say("Open destination"),
                move |_| {
                    Launch::new(&folder).start();
                },
            ));
            actions.push(NoticeAction::new(
                language.say("Show in folder"),
                move |_| {
                    if let Err(error) = reveal(&shown) {
                        report!("{error:#}");
                    }
                },
            ));
        }
        let notice = Notice {
            app,
            title: event.title,
            body,
            glyph: Some(FOLDER_GLYPH),
            actions,
            ..Notice::default()
        };
        Banners::post(notice, cx).key
    } else {
        NotificationStore::add_own(app, event.title, body, cx)
    };
    if let Some(folder) = destination {
        let this = cx.global_mut::<NotificationStore>();
        if let Some(entry) = this.entries.iter_mut().find(|entry| entry.key == key) {
            entry.destination = Some(folder);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn event(bytes: (u64, u64), state: FileJobState) -> FileJobEvent {
        FileJobEvent {
            id: 1,
            title: "Copying 3 items to Downloads".to_owned(),
            items: (1, 3),
            bytes,
            state,
            destination: Some(PathBuf::from(r"C:\Users\Ada\Downloads")),
            can_cancel: true,
        }
    }

    #[test]
    fn the_speed_is_smoothed_and_gives_the_time_left() {
        let start = Instant::now();
        let mut job = Job::new(event((0, 1000), FileJobState::Running), start);
        assert_eq!(job.speed(), None);
        job.update(
            event((100, 1000), FileJobState::Running),
            start + Duration::from_secs(1),
        );
        assert_eq!(job.speed(), Some(100.));
        assert_eq!(job.time_left(), Some(Duration::from_secs(9)));
        job.update(
            event((400, 1000), FileJobState::Running),
            start + Duration::from_secs(2),
        );
        // 300 bytes in the last second, of which 30 % count.
        assert_eq!(job.speed(), Some(160.));
        assert_eq!(job.share(), Some(0.4));
        assert_eq!(time_left((10, 10), 5.), None);
    }

    #[test]
    fn sizes_and_spans_read_as_people_say_them() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(1536), "1.5 KB");
        assert_eq!(size_text(25 * 1024 * 1024), "25 MB");
        let english = Language::English;
        assert_eq!(duration_text(Duration::from_millis(300), english), "1 s");
        assert_eq!(duration_text(Duration::from_secs(61), english), "2 min");
        assert_eq!(duration_text(Duration::from_secs(3 * 3600), english), "3 h");
    }

    #[test]
    fn the_share_comes_from_the_bytes_or_else_the_items() {
        let now = Instant::now();
        let job = Job::new(event((0, 0), FileJobState::Running), now);
        assert!((job.share().unwrap() - 1. / 3.).abs() < 1e-6);
        let mut unknown = event((0, 0), FileJobState::Running);
        unknown.items = (0, 0);
        assert_eq!(Job::new(unknown, now).share(), None);
    }
}
