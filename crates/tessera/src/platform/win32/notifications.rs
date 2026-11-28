// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The notifications Windows keeps in its notification center, the logos
//! of the applications that post them, whether the user is busy with
//! something that notifications should not interrupt, and the sounds of
//! the sound scheme notifications play.
//!
//! Every call waits for Windows to answer, so none belongs on the thread
//! that draws.
//!
//! What `UserNotificationListener` gives of a notification is its texts,
//! its application and when it was posted. The rest of what its
//! application wrote (pictures, buttons, input fields, the arguments of a
//! click) is read from Windows' notification database by `wpn.rs`, and a
//! click is done by `toast_activation.rs`.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use serde::Deserialize;
use windows::Foundation::Size;
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::DataReader;
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};
use windows::Win32::Media::Audio::{
    PlaySoundW, SND_ASYNC, SND_FILENAME, SND_FLAGS, SND_LOOP, SND_NODEFAULT,
};
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::UI::Shell::{
    QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
};
use windows::core::{HSTRING, Result as WindowsResult};

use super::execute::own_window_in_front;
use super::hdr::paths;
use super::registry::registry_text;

/// Seconds between 1601, where Windows starts counting, and 1970.
const WINDOWS_EPOCH_OFFSET: i64 = 11_644_473_600;

/// The units of a Windows timestamp in a second.
const TICKS_PER_SECOND: i64 = 10_000_000;

/// The side of the square the logos of applications are asked for in, in
/// pixels; Windows gives the nearest size it has.
const LOGO_SIDE: f32 = 64.;

/// The largest logo file that is read, in bytes.
const LOGO_LIMIT: u64 = 1 << 20;

/// The logo files of the applications, by the identifier they post under,
/// as they were read with their first notification; `None` for one that
/// has none. A logo is read once per run.
static LOGOS: LazyLock<Mutex<HashMap<String, Logo>>> = LazyLock::new(Mutex::default);

/// An image file of a logo, or none.
type Logo = Option<Arc<[u8]>>;

/// A notification as its application wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    /// Identifies the notification until it is dismissed.
    pub id: u32,
    /// The name of the application; empty if Windows does not tell.
    pub app: String,
    /// The identifier the application posts under; empty if Windows does
    /// not tell.
    pub app_id: String,
    /// The first line; may be empty.
    pub title: String,
    /// The remaining lines.
    pub body: String,
    /// When it was posted, in seconds since 1970.
    pub posted: i64,
}

/// The notification center of the signed-in user.
pub struct Notifications;

impl Notifications {
    /// The notifications waiting in the notification center, newest first.
    pub fn list() -> Result<Vec<Notification>, String> {
        let listener = listener()?;
        let read = || -> WindowsResult<Vec<Notification>> {
            let list = listener
                .GetNotificationsAsync(NotificationKinds::Toast)?
                .get()?;
            // A notification that cannot be read is left out.
            let mut notifications: Vec<Notification> = list
                .into_iter()
                .filter_map(|item| read(&item).ok())
                .collect();
            notifications.sort_by_key(|notification| std::cmp::Reverse(notification.posted));
            Ok(notifications)
        };
        read().map_err(|error| format!("the notifications could not be read: {error}"))
    }

    /// Removes the notification `id` from the notification center.
    pub fn dismiss(id: u32) -> Result<(), String> {
        listener()?
            .RemoveNotification(id)
            .map_err(|error| format!("the notification could not be dismissed: {error}"))
    }

    /// Follows the changes of the notification center: the receiver
    /// yields once each time a notification comes or goes, for as long as
    /// Tessera runs. Windows gives the event only to some programs (those
    /// with an identity of a package, among others); where it refuses,
    /// the reason is returned and the caller reads the list from time to
    /// time instead.
    pub fn watch() -> Result<async_channel::Receiver<()>, String> {
        static WATCHING: LazyLock<Mutex<Option<async_channel::Receiver<()>>>> =
            LazyLock::new(Mutex::default);
        let mut watching = WATCHING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(receiver) = watching.as_ref() {
            return Ok(receiver.clone());
        }
        let listener = listener()?;
        let (sender, receiver) = async_channel::bounded(1);
        let handler = TypedEventHandler::new(move |_, _| {
            // One waiting wake is enough: the list is read whole.
            sender.try_send(()).ok();
            Ok(())
        });
        listener.NotificationChanged(&handler).map_err(|error| {
            format!("Windows does not tell Tessera of new notifications: {error}")
        })?;
        // The listener and its handler live as long as the process.
        std::mem::forget(listener);
        *watching = Some(receiver.clone());
        Ok(receiver)
    }

    /// The image file of the logo of the application that posts under
    /// `app_id`, as Windows gave it with a notification of the application
    /// that [`Notifications::list`] read; `None` before that, and for an
    /// application Windows has no logo of. It asks Windows nothing.
    pub fn logo(app_id: &str) -> Option<Arc<[u8]>> {
        let logos = LOGOS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        logos.get(app_id).cloned().flatten()
    }
}

/// Whether a window of Tessera is in front, where the user sees what it
/// shows and needs no banner of it.
pub fn tessera_in_front() -> bool {
    own_window_in_front().is_some()
}

/// The listener, once Windows lets the shell read notifications. Windows
/// may ask the user the first time.
fn listener() -> Result<UserNotificationListener, String> {
    let open = || -> WindowsResult<(UserNotificationListener, UserNotificationListenerAccessStatus)> {
        let listener = UserNotificationListener::Current()?;
        let mut status = listener.GetAccessStatus()?;
        if status == UserNotificationListenerAccessStatus::Unspecified {
            status = listener.RequestAccessAsync()?.get()?;
        }
        Ok((listener, status))
    };
    match open() {
        Ok((listener, UserNotificationListenerAccessStatus::Allowed)) => Ok(listener),
        Ok(_) => Err("Windows does not let Tessera read notifications".to_owned()),
        Err(error) => Err(format!("the notification center is unavailable: {error}")),
    }
}

fn read(item: &UserNotification) -> WindowsResult<Notification> {
    let app = item
        .AppInfo()
        .and_then(|app| app.DisplayInfo())
        .and_then(|info| info.DisplayName())
        .map(|name| name.to_string())
        .unwrap_or_default();
    let app_id = item
        .AppInfo()
        .and_then(|app| app.AppUserModelId())
        .map(|id| id.to_string())
        .unwrap_or_default();
    let binding = item
        .Notification()?
        .Visual()?
        .GetBinding(&KnownNotificationBindings::ToastGeneric()?)?;
    let lines: Vec<String> = binding
        .GetTextElements()?
        .into_iter()
        .filter_map(|element| element.Text().ok())
        .map(|text| text.to_string())
        .collect();
    let (title, body) = split(&lines);
    remember_logo(item, &app_id);
    Ok(Notification {
        id: item.Id()?,
        app,
        app_id,
        title,
        body,
        posted: unix_seconds(item.CreationTime()?.UniversalTime),
    })
}

/// Reads the logo of the application of `item`, which posts under
/// `app_id`, unless it was read before.
fn remember_logo(item: &UserNotification, app_id: &str) {
    if app_id.is_empty() {
        return;
    }
    let known = |logos: &HashMap<String, _>| logos.contains_key(app_id);
    if known(
        &LOGOS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    ) {
        return;
    }
    let read = || -> WindowsResult<Logo> {
        let size = Size {
            Width: LOGO_SIDE,
            Height: LOGO_SIDE,
        };
        let logo = item.AppInfo()?.DisplayInfo()?.GetLogo(size)?;
        let stream = logo.OpenReadAsync()?.get()?;
        let length = stream.Size()?;
        if length == 0 || length > LOGO_LIMIT {
            return Ok(None);
        }
        let reader = DataReader::CreateDataReader(&stream)?;
        reader.LoadAsync(length as u32)?.get()?;
        let mut bytes = vec![0; length as usize];
        reader.ReadBytes(&mut bytes)?;
        Ok(Some(bytes.into()))
    };
    // An application without a logo of its own, as many programs are, is
    // drawn without one; the shell may still have an icon for it.
    let logo = read().unwrap_or(None);
    LOGOS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(app_id.to_owned(), logo);
}

/// The sounds of notifications, as the sound scheme of Windows gives them.
pub struct NotificationSound;

/// The events of the sound scheme a toast may name after
/// `ms-winsoundevent:`; the last stands for the looping alarms and calls.
const SOUND_EVENTS: [&str; 5] = [
    "Notification.Default",
    "Notification.IM",
    "Notification.Mail",
    "Notification.Reminder",
    "Notification.SMS",
];

/// What the looping sounds of alarms and calls start with.
const LOOPING_EVENTS: &str = "Notification.Looping.";

/// Whether `event` is an event of the sound scheme a toast may name: one
/// of the plain ones, or a looping alarm or call.
fn is_sound_event(event: &str) -> bool {
    if SOUND_EVENTS
        .iter()
        .any(|known| event.eq_ignore_ascii_case(known))
    {
        return true;
    }
    let Some(rest) = event.strip_prefix(LOOPING_EVENTS) else {
        return false;
    };
    let number = rest
        .strip_prefix("Alarm")
        .or_else(|| rest.strip_prefix("Call"));
    number.is_some_and(|number| number.chars().all(|c| c.is_ascii_digit()))
}

impl NotificationSound {
    /// The sound file the scheme of the user plays for `event`, as
    /// `Notification.Default`; `None` where the event is none a toast
    /// names, or the scheme plays nothing for it. It reads the registry.
    pub fn of_event(event: &str) -> Option<std::path::PathBuf> {
        if !is_sound_event(event) {
            return None;
        }
        let key = format!("AppEvents\\Schemes\\Apps\\.Default\\{event}\\.Current");
        let file = registry_text(HKEY_CURRENT_USER, &key, None, 1024)?;
        let file = file.trim();
        (!file.is_empty()).then(|| std::path::PathBuf::from(file))
    }

    /// Plays the sound file `file`, once or `looping` until [`Self::stop`]
    /// or another sound; it returns at once. Nothing plays where the file
    /// cannot be, and no other sound stands in for it.
    pub fn play(file: &std::path::Path, looping: bool) {
        let name = HSTRING::from(file.as_os_str());
        let mut flags = SND_FILENAME | SND_ASYNC | SND_NODEFAULT;
        if looping {
            flags |= SND_LOOP;
        }
        // SAFETY: the name is NUL-terminated and copied by the call, which
        // plays the sound on a thread of the system.
        let _ = unsafe { PlaySoundW(&name, None, flags) };
    }

    /// Stops the sound that plays.
    pub fn stop() {
        // SAFETY: a null name stops what plays.
        let _ = unsafe { PlaySoundW(windows::core::PCWSTR::null(), None, SND_FLAGS(0)) };
    }
}

/// What keeps the user busy, of the things during which Windows holds its
/// own notifications back where its settings ask for it. The
/// configuration names them in `quiet_when`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Occupation {
    /// An application fills the display, as a video shown full screen
    /// does.
    Fullscreen,
    /// A game draws full screen with Direct3D.
    Game,
    /// Windows is in presentation mode, which presentation programs and
    /// the mobility center switch on.
    Presenting,
    /// A display shows the same picture as another, as a projector does
    /// that duplicates the screen.
    Duplicating,
}

impl Occupation {
    /// What keeps the user busy now, if anything does; whether a display
    /// is duplicated is only asked with `duplicating`. A window of Tessera
    /// in front is no application that fills the display, whatever its
    /// size: a popup in the middle of the display, or the window of the
    /// desktop, would otherwise count as one.
    pub fn now(duplicating: bool) -> Option<Self> {
        // SAFETY: the call only fills in the state it returns.
        let state = unsafe { SHQueryUserNotificationState() }.ok();
        let busy = match state {
            Some(QUNS_RUNNING_D3D_FULL_SCREEN) => Some(Self::Game),
            Some(QUNS_PRESENTATION_MODE) => Some(Self::Presenting),
            Some(QUNS_BUSY) if own_window_in_front().is_none() => Some(Self::Fullscreen),
            _ => None,
        };
        busy.or_else(|| (duplicating && display_duplicated()).then_some(Self::Duplicating))
    }
}

/// Whether two displays in use show the picture of one source, which is
/// what duplicating a display is.
fn display_duplicated() -> bool {
    let sources: Vec<_> = paths()
        .iter()
        .map(|path| {
            let source = &path.sourceInfo;
            (
                source.adapterId.LowPart,
                source.adapterId.HighPart,
                source.id,
            )
        })
        .collect();
    shares_a_source(&sources)
}

/// Whether two of `sources` are the same.
fn shares_a_source<T: PartialEq>(sources: &[T]) -> bool {
    sources
        .iter()
        .enumerate()
        .any(|(at, source)| sources[at + 1..].contains(source))
}

/// The first line as the title and the others as the body.
fn split(lines: &[String]) -> (String, String) {
    match lines {
        [] => (String::new(), String::new()),
        [title, body @ ..] => (title.trim().to_owned(), body.join("\n").trim().to_owned()),
    }
}

/// A Windows timestamp in seconds since 1970.
fn unix_seconds(ticks: i64) -> i64 {
    ticks / TICKS_PER_SECOND - WINDOWS_EPOCH_OFFSET
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_line_is_the_title() {
        let lines = ["Ada".to_owned(), "Hello".to_owned(), "there ".to_owned()];
        assert_eq!(split(&lines), ("Ada".to_owned(), "Hello\nthere".to_owned()));
        assert_eq!(split(&lines[..1]), ("Ada".to_owned(), String::new()));
        assert_eq!(split(&[]), (String::new(), String::new()));
    }

    #[test]
    fn a_display_is_duplicated_where_two_paths_share_a_source() {
        assert!(!shares_a_source::<u32>(&[]));
        assert!(!shares_a_source(&[(1, 0, 0), (1, 0, 1)]));
        assert!(shares_a_source(&[(1, 0, 0), (2, 0, 3), (1, 0, 0)]));
    }

    #[test]
    fn only_the_sounds_of_notifications_are_played() {
        assert!(is_sound_event("Notification.Default"));
        assert!(is_sound_event("Notification.Looping.Alarm2"));
        assert!(is_sound_event("Notification.Looping.Call10"));
        assert!(!is_sound_event("Notification.Looping.Other"));
        assert!(!is_sound_event("SystemExit"));
        assert!(!is_sound_event("Notification.Looping.Alarm/x"));
    }

    #[test]
    fn windows_time_becomes_unix_time() {
        // 1 January 2026, 00:00 UTC.
        assert_eq!(unix_seconds(134_116_992_000_000_000), 1_767_225_600);
    }
}
