// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Thumbnails of files and folders as the Shell draws them: the pictures
//! of photos and videos, the first page of a PDF or an Office document,
//! and whatever a thumbnail provider of another program gives, read with
//! `IShellItemImageFactory::GetImage` and copied out as pixels.
//!
//! Providers can take long (a video is decoded, a document is opened)
//! and some hang, so they never run on the thread of GPUI, nor on the
//! [`ShellThread`](super::ShellThread), which menus need: two threads of
//! their own, each in a single-threaded apartment as providers expect,
//! take the requests from one queue. The request with the lowest
//! priority number goes first, the newest among equals, so that the rows
//! in sight are pictured before those just scrolled past. A request whose
//! receiver was dropped is skipped, which is how the file manager calls
//! off the rows that scrolled away. A provider that hangs keeps one
//! thread; the other goes on.
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::DeleteObject;
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF, SIIGBF_BIGGERSIZEOK,
    SIIGBF_RESIZETOFIT, SIIGBF_THUMBNAILONLY,
};
use windows::core::HSTRING;

use super::{IconPixels, ShellThread};

/// How many threads picture files at once.
const WORKERS: usize = 2;

/// The largest side a thumbnail is asked for at, in pixels, as Explorer's
/// largest icons are.
pub const LARGEST_THUMBNAIL: u32 = 256;

/// What a thumbnail may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbnailKind {
    /// Only a picture of what the file holds; nothing where its kind has
    /// no thumbnail provider, so that its icon is drawn instead.
    Picture,
    /// A picture of what it holds, or else the icon of its kind at the
    /// size asked for.
    PictureOrIcon,
}

impl ThumbnailKind {
    fn flags(self) -> SIIGBF {
        let fit = SIIGBF_RESIZETOFIT | SIIGBF_BIGGERSIZEOK;
        match self {
            Self::Picture => fit | SIIGBF_THUMBNAILONLY,
            Self::PictureOrIcon => fit,
        }
    }
}

/// A request for one thumbnail, waiting in the queue.
struct Request {
    path: PathBuf,
    size: u32,
    kind: ThumbnailKind,
    priority: u32,
    /// The order requests came in, to take the newest among equals.
    sequence: u64,
    answer: Sender<Option<IconPixels>>,
}

/// The requests waiting, and the signal that one came.
#[derive(Default)]
struct Queue {
    waiting: Mutex<Waiting>,
    came: Condvar,
}

#[derive(Default)]
struct Waiting {
    requests: Vec<Request>,
    sequence: u64,
}

impl Waiting {
    /// Takes the request to do next: the lowest priority number, the
    /// newest among equals. Requests nobody waits for any longer are
    /// dropped on the way.
    fn next(&mut self) -> Option<Request> {
        self.requests.retain(|request| !request.answer.is_closed());
        let best = { self.requests.iter().enumerate() }
            .min_by_key(|(_, request)| (request.priority, std::cmp::Reverse(request.sequence)))
            .map(|(index, _)| index)?;
        Some(self.requests.swap_remove(best))
    }
}

/// The queue of the process, with its threads started the first time it
/// is used.
fn queue() -> &'static Queue {
    static QUEUE: OnceLock<Queue> = OnceLock::new();
    static STARTED: OnceLock<()> = OnceLock::new();
    let queue = QUEUE.get_or_init(Queue::default);
    STARTED.get_or_init(|| {
        for _ in 0..WORKERS {
            ShellThread::run_alone("thumbnails", || work(queue));
        }
    });
    queue
}

/// The body of a thread that pictures files, for as long as the process
/// lives.
fn work(queue: &'static Queue) {
    loop {
        let request = {
            let mut waiting = queue.waiting.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if let Some(request) = waiting.next() {
                    break request;
                }
                waiting = { queue.came.wait(waiting) }.unwrap_or_else(PoisonError::into_inner);
            }
        };
        let picture = thumbnail(&request.path, request.size, request.kind);
        request.answer.try_send(picture).ok();
    }
}

/// The thumbnails of the Shell.
pub struct Thumbnails;

impl Thumbnails {
    /// Asks for the thumbnail of the file or folder at `path`, fitting a
    /// square `size` pixels wide (at most [`LARGEST_THUMBNAIL`]), as
    /// `kind` allows; a lower `priority` is done sooner. It returns at
    /// once; the receiver yields once, `None` where there is no picture.
    /// Dropping the receiver calls the request off, if it has not begun.
    pub fn request(
        path: PathBuf,
        size: u32,
        kind: ThumbnailKind,
        priority: u32,
    ) -> Receiver<Option<IconPixels>> {
        let (answer, answered) = async_channel::bounded(1);
        let queue = queue();
        let mut waiting = queue.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        waiting.sequence += 1;
        let sequence = waiting.sequence;
        waiting.requests.push(Request {
            path,
            size: size.clamp(16, LARGEST_THUMBNAIL),
            kind,
            priority,
            sequence,
            answer,
        });
        drop(waiting);
        queue.came.notify_one();
        answered
    }
}

/// The thumbnail of `path`, read on this thread, which has COM.
fn thumbnail(path: &Path, size: u32, kind: ThumbnailKind) -> Option<IconPixels> {
    // The Shell takes no path written with forward slashes.
    let name = HSTRING::from(path.to_string_lossy().replace('/', "\\"));
    let side = size as i32;
    // SAFETY: COM is initialized on this thread (`run_alone`), and the
    // bitmap is deleted once its pixels are copied.
    unsafe {
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&name, None).ok()?;
        let bitmap = { factory.GetImage(SIZE { cx: side, cy: side }, kind.flags()) }.ok()?;
        let pixels = IconPixels::of_bitmap(bitmap);
        let _ = DeleteObject(bitmap.into());
        pixels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(priority: u32, sequence: u64) -> (Request, Receiver<Option<IconPixels>>) {
        let (answer, answered) = async_channel::bounded(1);
        let request = Request {
            path: PathBuf::from(format!("{priority}-{sequence}")),
            size: 64,
            kind: ThumbnailKind::Picture,
            priority,
            sequence,
            answer,
        };
        (request, answered)
    }

    #[test]
    fn the_lowest_priority_goes_first_and_the_newest_among_equals() {
        let mut waiting = Waiting::default();
        let mut kept = Vec::new();
        for (priority, sequence) in [(3, 1), (1, 2), (1, 3), (2, 4)] {
            let (request, answered) = request(priority, sequence);
            waiting.requests.push(request);
            kept.push(answered);
        }
        let order: Vec<u64> = std::iter::from_fn(|| waiting.next())
            .map(|request| request.sequence)
            .collect();
        assert_eq!(order, [3, 2, 4, 1]);
    }

    #[test]
    fn a_request_nobody_waits_for_is_skipped() {
        let mut waiting = Waiting::default();
        let (gone, answered) = request(0, 1);
        drop(answered);
        let (kept, _answered) = request(5, 2);
        waiting.requests.extend([gone, kept]);
        assert_eq!(waiting.next().map(|request| request.sequence), Some(2));
        assert!(waiting.next().is_none());
    }
}
