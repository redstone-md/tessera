// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The devices of the machine, as extensions are told of them.
//!
//! Extensions are called on the thread that draws, and Windows takes its
//! time to list devices, so a thread of its own reads them now and then
//! and extensions are given what it read last.

use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::platform::{Bluetooth, PairedDevice};

/// How often the devices are read while extensions ask for them.
const READ_INTERVAL: Duration = Duration::from_secs(20);

/// How long after the last question the reading goes on.
const KEPT_UP: Duration = Duration::from_secs(120);

type Reading = Result<Vec<PairedDevice>, String>;

struct Watch {
    /// What was read last; `None` before the first reading.
    reading: Option<Reading>,
    /// When an extension last asked.
    asked: Option<Instant>,
    /// Whether a thread is reading.
    running: bool,
}

static WATCH: Mutex<Watch> = Mutex::new(Watch {
    reading: None,
    asked: None,
    running: false,
});

fn watch() -> std::sync::MutexGuard<'static, Watch> {
    WATCH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The paired Bluetooth devices as they were read last, at most
/// [`READ_INTERVAL`] ago while extensions keep asking. The first question
/// starts the reading and is answered with an error that says so.
pub fn bluetooth() -> Reading {
    let mut watch = watch();
    watch.asked = Some(Instant::now());
    if !watch.running {
        watch.running = true;
        // What was read before the reading stopped is old by now.
        watch.reading = None;
        thread::spawn(read_on);
    }
    let reading = watch.reading.clone();
    reading.unwrap_or_else(|| Err("the Bluetooth devices are still being read".to_owned()))
}

/// Reads the devices until no extension has asked for a while.
fn read_on() {
    loop {
        let reading = Bluetooth::devices();
        let mut watch = watch();
        watch.reading = Some(reading);
        if watch.asked.is_none_or(|asked| asked.elapsed() > KEPT_UP) {
            watch.running = false;
            return;
        }
        drop(watch);
        thread::sleep(READ_INTERVAL);
    }
}
