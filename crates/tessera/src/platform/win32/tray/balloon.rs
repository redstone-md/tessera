// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Balloons: the short notices an application shows at its icon in the
//! notification area with `NIF_INFO`.
//!
//! While the tray window serves the icons itself, the balloons that arrive
//! are published on a channel for the notifications of the shell to show,
//! and the application is told what became of them. Beside Explorer they
//! go to Explorer, which shows them, and nothing is published but the end
//! of a balloon that was published while the window served the icons.
//! Nothing is published before a reader subscribed, and a reader that
//! falls behind loses the oldest balloons, not the newest.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use async_channel::{Receiver, Sender};
use windows::Win32::UI::Shell::{
    NIIF_ERROR, NIIF_ICON_MASK, NIIF_INFO, NIIF_LARGE_ICON, NIIF_NOSOUND, NIIF_RESPECT_QUIET_TIME,
    NIIF_USER, NIIF_WARNING, NIN_BALLOONHIDE, NIN_BALLOONSHOW, NIN_BALLOONTIMEOUT,
    NIN_BALLOONUSERCLICK,
};

use crate::platform::win32::icon::IconPixels;

/// How many balloons wait for a reader; the oldest are dropped for more.
const WAITING: usize = 32;

/// Marks a balloon that is to be dropped when it cannot be shown at once.
pub(super) const NIF_REALTIME: u32 = 0x40;

/// What a balloon says of itself by its icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BalloonKind {
    /// No icon.
    None,
    Info,
    Warning,
    Error,
    /// The icon the application gave, in [`Balloon::icon`].
    User,
}

impl BalloonKind {
    fn of(flags: u32) -> Self {
        match flags & NIIF_ICON_MASK.0 {
            flags if flags == NIIF_INFO.0 => Self::Info,
            flags if flags == NIIF_WARNING.0 => Self::Warning,
            flags if flags == NIIF_ERROR.0 => Self::Error,
            flags if flags == NIIF_USER.0 => Self::User,
            _ => Self::None,
        }
    }
}

/// A balloon an application asked to show at its icon, or the end of one.
#[derive(Clone, Debug)]
#[allow(
    dead_code,
    reason = "the banners show a balloon with its kind and picture but do not yet honour its wishes for size and sound"
)]
pub struct Balloon {
    /// The icon it belongs to, as [`super::TrayIcon::id`] names it. An icon
    /// shows one balloon at a time: a new one replaces the one before.
    pub id: u64,
    pub title: String,
    /// What it says; empty when the application took the balloon back or
    /// removed its icon, which ends the balloon shown for it.
    pub text: String,
    pub kind: BalloonKind,
    /// The application asks for its picture to be shown large.
    pub large: bool,
    /// The application asks for no sound to be played.
    pub no_sound: bool,
    /// The application asks that the balloon be dropped while the user is
    /// new to the session or does not want to be disturbed.
    pub respect_quiet: bool,
    /// The application asks that the balloon be dropped if it cannot be
    /// shown at once.
    pub realtime: bool,
    /// The picture of a balloon of the kind [`BalloonKind::User`]: the one
    /// the application gave for it, otherwise that of its icon.
    pub icon: Option<Arc<IconPixels>>,
    /// The description of the icon, which usually names the application.
    pub tip: String,
}

impl Balloon {
    /// A balloon of `id` with `text`; `flags` are the `NIIF_` flags.
    pub(super) fn new(id: u64, title: String, text: String, flags: u32, realtime: bool) -> Self {
        Self {
            id,
            title,
            text,
            kind: BalloonKind::of(flags),
            large: flags & NIIF_LARGE_ICON.0 != 0,
            no_sound: flags & NIIF_NOSOUND.0 != 0,
            respect_quiet: flags & NIIF_RESPECT_QUIET_TIME.0 != 0,
            realtime,
            icon: None,
            tip: String::new(),
        }
    }

    /// The end of the balloon of the icon `id`.
    pub(super) fn withdrawal(id: u64) -> Self {
        Self::new(id, String::new(), String::new(), 0, false)
    }

    /// Whether this ends the balloon of its icon instead of showing one.
    pub fn is_withdrawal(&self) -> bool {
        self.text.is_empty()
    }
}

/// What became of a balloon, as its application is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BalloonEvent {
    /// It is shown.
    Shown,
    /// It went for another reason than the ones below, such as its icon
    /// being removed.
    Hidden,
    /// It went by itself after a while, or the user closed it.
    TimedOut,
    /// The user clicked it.
    Clicked,
}

impl BalloonEvent {
    /// The message the application is sent for it.
    pub(super) fn message(self) -> u32 {
        match self {
            Self::Shown => NIN_BALLOONSHOW,
            Self::Hidden => NIN_BALLOONHIDE,
            Self::TimedOut => NIN_BALLOONTIMEOUT,
            Self::Clicked => NIN_BALLOONUSERCLICK,
        }
    }
}

fn channel() -> &'static (Sender<Balloon>, Receiver<Balloon>) {
    static CHANNEL: OnceLock<(Sender<Balloon>, Receiver<Balloon>)> = OnceLock::new();
    CHANNEL.get_or_init(|| async_channel::bounded(WAITING))
}

/// Whether a reader subscribed; before that nothing is kept.
static SUBSCRIBED: AtomicBool = AtomicBool::new(false);

/// The balloons as they arrive from now on. Every receiver handed out
/// takes from the same stream, so one reader is meant to take them all.
pub(super) fn subscribe() -> Receiver<Balloon> {
    SUBSCRIBED.store(true, Ordering::Relaxed);
    channel().1.clone()
}

/// Hands `balloon` to the reader, dropping the oldest that waits if too
/// many do. Without a reader it is dropped.
pub(super) fn publish(balloon: Balloon) {
    if SUBSCRIBED.load(Ordering::Relaxed) {
        let _ = channel().0.force_send(balloon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flags_of_a_balloon_are_read() {
        let flags = NIIF_WARNING.0 | NIIF_NOSOUND.0 | NIIF_LARGE_ICON.0;
        let balloon = Balloon::new(3, "Disk".into(), "Almost full".into(), flags, true);
        assert_eq!(balloon.kind, BalloonKind::Warning);
        assert!(balloon.no_sound && balloon.large && balloon.realtime);
        assert!(!balloon.respect_quiet && !balloon.is_withdrawal());
        assert_eq!(
            BalloonKind::of(NIIF_USER.0 | NIIF_RESPECT_QUIET_TIME.0),
            BalloonKind::User
        );
        assert_eq!(BalloonKind::of(9), BalloonKind::None);
        assert!(Balloon::withdrawal(3).is_withdrawal());
    }

    #[test]
    fn each_event_has_its_message() {
        let messages = [
            BalloonEvent::Shown,
            BalloonEvent::Hidden,
            BalloonEvent::TimedOut,
            BalloonEvent::Clicked,
        ]
        .map(BalloonEvent::message);
        assert_eq!(messages, [0x402, 0x403, 0x404, 0x405]);
    }
}
