// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The progress of the work on files that runs away from the user
//! interface (copying, moving, deleting, packing archives), told to every
//! part of Tessera that listens, and the asking of such work to stop.
//!
//! A job takes a number with [`FileJobs::next_id`], publishes an event each
//! time it gets on and once when it ends, and asks [`FileJobs::is_cancelled`]
//! between its items. Whoever shows the jobs subscribes and may cancel one.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use async_channel::{Receiver, Sender};

/// Where a job is.
#[derive(Clone, Debug, PartialEq)]
#[allow(
    dead_code,
    reason = "published by the jobs of files-namespace and files-search-actions"
)]
pub enum FileJobState {
    Running,
    Done,
    /// It ended without being done, for this reason.
    Failed(String),
    Cancelled,
}

/// How far a job got.
#[derive(Clone, Debug)]
#[allow(
    dead_code,
    reason = "read by the notifications of notifications-rich, which show the jobs"
    reason = "published by the jobs of files-namespace and files-search-actions"
)]
pub struct FileJobEvent {
    /// The number of the job, from [`FileJobs::next_id`].
    pub id: u64,
    /// What it does, as "Copying 12 items to Downloads", in the language
    /// shown.
    pub title: String,
    /// The items done and the items in all; 0 in all while that is not
    /// known.
    pub items: (u64, u64),
    /// The bytes done and the bytes in all, as the items.
    pub bytes: (u64, u64),
    pub state: FileJobState,
    /// The folder the items go to, where they go to one.
    pub destination: Option<PathBuf>,
    /// Whether the job stops when it is asked to.
    pub can_cancel: bool,
}

/// The jobs on files, as those who show them see them.
pub struct FileJobs;

static SUBSCRIBERS: Mutex<Vec<Sender<FileJobEvent>>> = Mutex::new(Vec::new());
static CANCELLED: Mutex<Option<HashSet<u64>>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The value a mutex guards, also after a panic while it was held.
fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[allow(
    dead_code,
    reason = "used by files-namespace, files-search-actions and notifications-rich"
)]
impl FileJobs {
    /// A number no other job of this process has.
    pub fn next_id() -> u64 {
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    }

    /// Gives `event` to every receiver there still is; one that was
    /// dropped is forgotten. A job that ended is forgotten too, also where
    /// it was asked to stop.
    pub fn publish(event: FileJobEvent) {
        if event.state != FileJobState::Running
            && let Some(cancelled) = locked(&CANCELLED).as_mut()
        {
            cancelled.remove(&event.id);
        }
        locked(&SUBSCRIBERS).retain(|subscriber| subscriber.try_send(event.clone()).is_ok());
    }

    /// A receiver of every event from now on, in the order they are
    /// published; each caller gets its own.
    #[allow(dead_code, reason = "subscribed by notifications-rich")]
    pub fn subscribe() -> Receiver<FileJobEvent> {
        let (sender, receiver) = async_channel::unbounded();
        locked(&SUBSCRIBERS).push(sender);
        receiver
    }

    /// Whether anybody listens, so that a job need not make events nobody
    /// reads.
    pub fn has_subscribers() -> bool {
        let mut subscribers = locked(&SUBSCRIBERS);
        subscribers.retain(|subscriber| !subscriber.is_closed());
        !subscribers.is_empty()
    }

    /// Asks the job `id` to stop; it stops between two items.
    #[allow(
        dead_code,
        reason = "cancelled from the notifications of notifications-rich"
    )]
    pub fn cancel(id: u64) {
        locked(&CANCELLED).get_or_insert_default().insert(id);
    }

    /// Whether the job `id` was asked to stop, which its worker asks
    /// between items.
    pub fn is_cancelled(id: u64) -> bool {
        locked(&CANCELLED)
            .as_ref()
            .is_some_and(|cancelled| cancelled.contains(&id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: u64, state: FileJobState) -> FileJobEvent {
        FileJobEvent {
            id,
            title: "Copying 2 items to Downloads".to_owned(),
            items: (1, 2),
            bytes: (10, 20),
            state,
            destination: None,
            can_cancel: true,
        }
    }

    #[test]
    fn every_subscriber_hears_every_event_and_a_cancel_lasts_until_the_end() {
        let id = FileJobs::next_id();
        assert_ne!(FileJobs::next_id(), id);
        let first = FileJobs::subscribe();
        let second = FileJobs::subscribe();
        assert!(FileJobs::has_subscribers());
        FileJobs::publish(event(id, FileJobState::Running));
        for receiver in [&first, &second] {
            let heard = receiver.try_recv().expect("it was published");
            assert_eq!((heard.id, heard.state), (id, FileJobState::Running));
        }
        assert!(!FileJobs::is_cancelled(id));
        FileJobs::cancel(id);
        assert!(FileJobs::is_cancelled(id));
        drop(second);
        FileJobs::publish(event(id, FileJobState::Running));
        assert!(FileJobs::is_cancelled(id));
        FileJobs::publish(event(id, FileJobState::Cancelled));
        assert!(!FileJobs::is_cancelled(id));
        assert_eq!(first.try_recv().map(|heard| heard.id), Ok(id));
    }
}
