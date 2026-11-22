// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How many notifications of each application wait in the notification
//! center of Windows, for the badges of the `windows` widget.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use gpui::{App, AsyncApp, BorrowAppContext, Global, Task};

use crate::platform::{Notification, Notifications};

/// How often the notification center is read.
const REFRESH_INTERVAL: Duration = Duration::from_secs(3);

/// The waiting notifications, counted by the application that posted them.
#[derive(Default)]
pub struct Badges {
    /// The identifier each application posts under, in small letters, with
    /// how many of its notifications wait.
    counts: HashMap<String, usize>,
    /// Whether the last reading failed, which is reported once.
    failed: bool,
    watch: Option<Task<()>>,
}

impl Global for Badges {}

impl Badges {
    /// Starts reading the notification center, unless it is read already.
    pub fn watch(cx: &mut App) {
        if cx.default_global::<Self>().watch.is_some() {
            return;
        }
        let watch = cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                let listed = cx
                    .background_executor()
                    .spawn(async { Notifications::list() })
                    .await;
                if cx.update(|cx| Self::take(listed, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH_INTERVAL).await;
            }
        });
        cx.global_mut::<Self>().watch = Some(watch);
    }

    /// Takes in what was read; those who observe are told only of a change.
    fn take(listed: Result<Vec<Notification>, String>, cx: &mut App) {
        let this = cx.global::<Self>();
        let counts = match listed {
            Ok(listed) => count(&listed),
            Err(error) => {
                if !this.failed {
                    report!("there are no badges: {error}");
                    cx.update_global(|this: &mut Self, _| this.failed = true);
                }
                return;
            }
        };
        if counts != this.counts || this.failed {
            cx.update_global(|this: &mut Self, _| {
                this.counts = counts;
                this.failed = false;
            });
        }
    }

    /// How many notifications wait for the application `owner` tells of.
    pub fn of(&self, owner: &Owner<'_>) -> usize {
        let waiting = |(id, count): (&String, &usize)| owner.posts_as(id).then_some(*count);
        self.counts.iter().filter_map(waiting).sum()
    }
}

/// The notifications of each application that says what it posts under.
fn count(listed: &[Notification]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for notification in listed.iter().filter(|one| !one.app_id.is_empty()) {
        *counts
            .entry(notification.app_id.to_lowercase())
            .or_default() += 1;
    }
    counts
}

/// What is known of an application that may have posted notifications.
#[derive(Clone, Copy, Debug, Default)]
pub struct Owner<'a> {
    /// The identifier of a packaged application, or the one a program
    /// states for its windows.
    pub app: Option<&'a str>,
    /// The program file.
    pub program: Option<&'a Path>,
}

impl Owner<'_> {
    /// Whether notifications posted under `id`, which is in small letters,
    /// are those of the application: `id` is its identifier, or `id` is a
    /// path, as Windows makes one up for a program without an identifier,
    /// that ends with the name of its program file.
    fn posts_as(&self, id: &str) -> bool {
        if self.app.is_some_and(|app| app.eq_ignore_ascii_case(id)) {
            return true;
        }
        let Some((_, file)) = id.rsplit_once(['\\', '/']) else {
            return false;
        };
        let program = self.program.and_then(Path::file_name);
        program.is_some_and(|program| program.to_string_lossy().to_lowercase() == file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(app_id: &str) -> Notification {
        Notification {
            id: 0,
            app: String::new(),
            app_id: app_id.to_owned(),
            title: String::new(),
            body: String::new(),
            posted: 0,
        }
    }

    #[test]
    fn notifications_are_counted_for_the_application_that_posted_them() {
        let listed = [
            notification("Microsoft.WindowsStore_8wekyb3d8bbwe!App"),
            notification("{1AC14E77}\\WindowsPowerShell\\v1.0\\PowerShell.exe"),
            notification("{1AC14E77}\\WindowsPowerShell\\v1.0\\powershell.exe"),
            notification("com.example.Chat"),
            notification(""),
        ];
        let badges = Badges {
            counts: count(&listed),
            ..Badges::default()
        };
        let app = |app| Owner {
            app: Some(app),
            program: None,
        };
        let program = |program| Owner {
            app: None,
            program: Some(Path::new(program)),
        };
        assert_eq!(
            badges.of(&app("microsoft.windowsstore_8wekyb3d8bbwe!app")),
            1
        );
        assert_eq!(badges.of(&app("com.example.chat")), 1);
        let shell = "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe";
        assert_eq!(badges.of(&program(shell)), 2);
        // A program is not told by an identifier that is no path.
        assert_eq!(badges.of(&program("C:/apps/com.example.Chat")), 0);
        assert_eq!(badges.of(&program("C:/apps/shell.exe")), 0);
        assert_eq!(badges.of(&Owner::default()), 0);
    }
}
