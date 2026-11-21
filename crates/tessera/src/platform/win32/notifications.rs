// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The notifications Windows keeps in its notification center.
//!
//! Every call waits for Windows to answer, so none belongs on the thread
//! that draws.

use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};
use windows::core::Result as WindowsResult;

/// Seconds between 1601, where Windows starts counting, and 1970.
const WINDOWS_EPOCH_OFFSET: i64 = 11_644_473_600;

/// The units of a Windows timestamp in a second.
const TICKS_PER_SECOND: i64 = 10_000_000;

/// A notification as its application wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    /// Identifies the notification until it is dismissed.
    pub id: u32,
    /// The name of the application; empty if Windows does not tell.
    pub app: String,
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

    /// Removes every notification from the notification center.
    pub fn clear() -> Result<(), String> {
        listener()?
            .ClearNotifications()
            .map_err(|error| format!("the notifications could not be cleared: {error}"))
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
    Ok(Notification {
        id: item.Id()?,
        app,
        title,
        body,
        posted: unix_seconds(item.CreationTime()?.UniversalTime),
    })
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
    fn windows_time_becomes_unix_time() {
        // 1 January 2026, 00:00 UTC.
        assert_eq!(unix_seconds(134_116_992_000_000_000), 1_767_225_600);
    }
}
