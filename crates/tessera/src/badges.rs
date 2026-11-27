// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How many notifications of each application wait in the notification
//! center of Windows, for the badges of the `windows` widget. They are
//! counted from what the notification store read, leaving out the
//! applications a `[[notification_rule]]` gives `badge = false`.

use std::collections::HashMap;
use std::path::Path;

use gpui::{App, BorrowAppContext, Global};

use crate::notifications_store::{Entry, NotificationRule, NotificationStore, rule_for};

/// The waiting notifications, counted by the application that posted them.
#[derive(Default)]
pub struct Badges {
    /// The identifier each application posts under, in small letters, with
    /// how many of its notifications wait.
    counts: HashMap<String, usize>,
}

impl Global for Badges {}

impl Badges {
    /// Starts counting, unless it is counted already. The notification
    /// center is read by the store, which the shell starts.
    pub fn watch(cx: &mut App) {
        if cx.has_global::<Self>() {
            return;
        }
        cx.set_global(Self::default());
        Self::count(cx);
    }

    /// Counts again what the store holds, where the badges are counted;
    /// those who observe are told only of a change.
    pub fn count(cx: &mut App) {
        let Some(this) = cx.try_global::<Self>() else {
            return;
        };
        let counts = match cx.try_global::<NotificationStore>() {
            Some(store) => count(store.entries(), store.rules()),
            None => HashMap::new(),
        };
        if counts != this.counts {
            cx.update_global(|this: &mut Self, _| this.counts = counts);
        }
    }

    /// How many notifications wait for the application `owner` tells of.
    pub fn of(&self, owner: &Owner<'_>) -> usize {
        let waiting = |(id, count): (&String, &usize)| owner.posts_as(id).then_some(*count);
        self.counts.iter().filter_map(waiting).sum()
    }
}

/// The notifications of each application that says what it posts under,
/// of those the notification center holds, but for the applications
/// `rules` keep from being counted.
fn count(entries: &[Entry], rules: &[NotificationRule]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    let counted = |entry: &&Entry| {
        entry.present
            && !entry.app_id.is_empty()
            && rule_for(rules, &entry.app, &entry.app_id).is_none_or(|rule| rule.badge)
    };
    for entry in entries.iter().filter(counted) {
        *counts.entry(entry.app_id.to_lowercase()).or_default() += 1;
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

    fn entry(app_id: &str) -> Entry {
        Entry {
            key: 0,
            id: Some(0),
            app: String::new(),
            app_id: app_id.to_owned(),
            title: String::new(),
            body: String::new(),
            posted: 0,
            present: true,
        }
    }

    #[test]
    fn notifications_are_counted_for_the_application_that_posted_them() {
        let gone = Entry {
            present: false,
            ..entry("com.example.Chat")
        };
        let listed = [
            entry("Microsoft.WindowsStore_8wekyb3d8bbwe!App"),
            entry("{1AC14E77}\\WindowsPowerShell\\v1.0\\PowerShell.exe"),
            entry("{1AC14E77}\\WindowsPowerShell\\v1.0\\powershell.exe"),
            entry("com.example.Chat"),
            entry(""),
            gone,
        ];
        let badges = Badges {
            counts: count(&listed, &[]),
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

    #[test]
    fn a_rule_keeps_an_application_from_being_counted() {
        let rules = [NotificationRule {
            app: "com.example.*".to_owned(),
            banners: true,
            show: true,
            priority: false,
            badge: false,
        }];
        let counts = count(&[entry("com.example.Chat"), entry("other")], &rules);
        assert_eq!(counts, HashMap::from([("other".to_owned(), 1)]));
    }
}
