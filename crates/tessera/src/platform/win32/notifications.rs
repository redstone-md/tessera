// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The notifications Windows keeps in its notification center, the logos
//! of the applications that post them, and whether the user is busy with
//! something that notifications should not interrupt.
//!
//! Every call waits for Windows to answer, so none belongs on the thread
//! that draws.
//!
//! What Windows lets another program read of a notification is its texts,
//! its application and when it was posted. The buttons, pictures, input
//! fields and the arguments the application would be started with are not
//! given out (`UserNotificationListener` has no such properties), so a
//! notification can only open its application, not do what its buttons
//! would.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use serde::Deserialize;
use windows::Foundation::Size;
use windows::Storage::Streams::DataReader;
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::{
    QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::core::Result as WindowsResult;

use super::hdr::paths;

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
            Some(QUNS_BUSY) if !own_window_in_front() => Some(Self::Fullscreen),
            _ => None,
        };
        busy.or_else(|| (duplicating && display_duplicated()).then_some(Self::Duplicating))
    }
}

/// Whether the window in front is one of this program.
fn own_window_in_front() -> bool {
    let mut process = 0;
    // SAFETY: the calls only read which window is in front and whose it
    // is; `process` outlives the call that fills it.
    unsafe {
        let window = GetForegroundWindow();
        if window.is_invalid() {
            return false;
        }
        GetWindowThreadProcessId(window, Some(&raw mut process));
        process == GetCurrentProcessId()
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
    fn windows_time_becomes_unix_time() {
        // 1 January 2026, 00:00 UTC.
        assert_eq!(unix_seconds(134_116_992_000_000_000), 1_767_225_600);
    }
}
