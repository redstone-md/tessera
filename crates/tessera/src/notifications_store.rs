// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The notifications Tessera has seen: those the notification center of
//! Windows holds, read every two seconds, those of Tessera itself, and,
//! as a history, those that left the notification center without being
//! dismissed here, as when Windows let them expire. The history is kept
//! in `notifications.toml` of the `tessera` directory of
//! `%LOCALAPPDATA%`, written two seconds after a change, together with
//! when the list was last looked at and until when do not disturb holds.
//!
//! The banners, the list of notifications, the button of the bar and the
//! badges of the dock all take what they show from here, so the
//! notification center is read once for all of them. The `[[notification_rule]]`
//! entries of the configuration decide what is kept of which application.
//!
//! For each notification of Windows the toast its application wrote is
//! read from Windows' notification database, on a thread of its own, a
//! moment after the list changed ([`crate::toast`]): with it come the
//! pictures, the progress bars, the buttons, the input fields and what a
//! click starts, which this part does ([`NotificationStore::activate`]).
//! Where the database cannot be read, the texts of the list are all
//! there is, and a click opens the application. The jobs on files that
//! publish their progress (`platform::FileJobs`) are kept here too, under
//! `[notifications] jobs = true`.

mod jobs;
mod pictures;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui::{App, AsyncApp, Global, Task};
use serde::{Deserialize, Serialize};

use crate::badges::Badges;
use crate::banners::{Banners, NotificationsConfig};
use crate::icons::{Icon, icon_of_pixels, image_of_bytes};
use crate::platform::{
    Activated, AppNotificationSettings, IconPixels, Notification, NotificationDatabase,
    Notifications, Occupation, QuietProfile, ShellThread, ToastActivation, ToastRoute,
    WindowsQuiet,
};
use crate::toast::{Activation, Toast, input_values};

pub use jobs::Job;
pub use pictures::sound_file;

/// How often the notification center is read.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// How long after a change of the list the toasts are read from the
/// database, so that a burst of changes is read once.
const PAYLOAD_DELAY: Duration = Duration::from_millis(300);

/// How many readings of the list pass before the settings of the
/// applications in Windows are read again.
const SETTINGS_EVERY: u32 = 15;

/// How long after a change the history is written, so that a burst of
/// changes is written once.
const SAVE_DELAY: Duration = Duration::from_secs(2);

/// The file of the history, in the `tessera` directory of the local
/// application data.
const HISTORY_FILE: &str = "notifications.toml";

/// The side of the square the logos of applications are prepared for, in
/// pixels; enough for a logo of 32 logical pixels at 200 %.
const LOGO_PIXELS: u32 = 64;

fn yes() -> bool {
    true
}

/// What a `[[notification_rule]]` of the configuration says of the
/// notifications of one application. The first rule that names the
/// application decides; an application no rule names gets everything.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationRule {
    /// The application: the name its notifications give, or the
    /// identifier it posts under. `*` stands for any letters and `?` for
    /// one; small and capital letters are alike.
    pub app: String,
    /// `false`: its notifications come as no banner, but are listed.
    #[serde(default = "yes")]
    pub banners: bool,
    /// `false`: its notifications are shown nowhere, not even listed or
    /// counted. They stay in the notification center of Windows.
    #[serde(default = "yes")]
    pub show: bool,
    /// `true`: its banners come even while do not disturb holds.
    #[serde(default)]
    pub priority: bool,
    /// `false`: the buttons of the dock do not count its notifications.
    #[serde(default = "yes")]
    pub badge: bool,
}

impl NotificationRule {
    /// Whether the rule names the application called `app` that posts
    /// under `app_id`.
    fn names(&self, app: &str, app_id: &str) -> bool {
        let pattern = self.app.trim();
        !pattern.is_empty()
            && [app, app_id]
                .iter()
                .any(|name| !name.is_empty() && matches_pattern(pattern, name))
    }
}

/// The rule of `rules` that decides for the application `app`, posting
/// under `app_id`: the first that names it.
pub fn rule_for<'a>(
    rules: &'a [NotificationRule],
    app: &str,
    app_id: &str,
) -> Option<&'a NotificationRule> {
    rules.iter().find(|rule| rule.names(app, app_id))
}

/// Whether `text` is what `pattern` describes, where `*` stands for any
/// letters, `?` for one, and small and capital letters are alike.
fn matches_pattern(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    // The last star and where in the text it stands for letters up to,
    // to go back to when a later part does not fit.
    let (mut at, mut on) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while on < text.len() {
        match pattern.get(at) {
            Some('*') => {
                star = Some((at, on));
                at += 1;
            }
            Some(&wanted) if wanted == '?' || wanted == text[on] => {
                at += 1;
                on += 1;
            }
            _ => match star {
                Some((star_at, star_on)) => {
                    at = star_at + 1;
                    on = star_on + 1;
                    star = Some((star_at, star_on + 1));
                }
                None => return false,
            },
        }
    }
    pattern[at..].iter().all(|&rest| rest == '*')
}

/// A notification Tessera keeps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Tells it from the others while Tessera runs.
    #[serde(skip)]
    pub key: u64,
    /// The identifier the notification center gave it; none for one of
    /// Tessera's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// When it was posted, in seconds since 1970.
    pub posted: i64,
    /// Whether the notification center still holds it.
    #[serde(skip)]
    pub present: bool,
    /// The toast its application wrote, once it was read from the
    /// database; it is not kept in the history file.
    #[serde(skip)]
    pub toast: Option<Arc<Toast>>,
    /// Until when it was snoozed, in seconds since 1970: it is not listed
    /// until then, and then comes again.
    #[serde(skip)]
    pub snoozed_until: Option<i64>,
    /// For a job on files that ended: the folder its items went to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<PathBuf>,
}

impl Entry {
    fn of(notification: Notification, key: u64) -> Self {
        Self {
            key,
            id: Some(notification.id),
            app: notification.app,
            app_id: notification.app_id,
            title: notification.title,
            body: notification.body,
            posted: notification.posted,
            present: true,
            toast: None,
            snoozed_until: None,
            destination: None,
        }
    }

    /// Whether it is listed now: not while it is snoozed.
    pub fn listed(&self, now: i64) -> bool {
        self.snoozed_until.is_none_or(|until| until <= now)
    }

    /// Whether it says what `notification` says.
    fn says(&self, notification: &Notification) -> bool {
        self.app == notification.app
            && self.app_id == notification.app_id
            && self.title == notification.title
            && self.body == notification.body
    }

    /// What the entries of one application are grouped under.
    pub fn group(&self) -> String {
        if self.app_id.is_empty() {
            self.app.to_lowercase()
        } else {
            self.app_id.to_lowercase()
        }
    }
}

/// The notifications of one application, by their place in the list they
/// were grouped from, the newest first.
pub struct Group {
    /// What [`Entry::group`] gives for each of them.
    pub key: String,
    pub members: Vec<usize>,
}

/// The places of `items`, which are newest first and each have the entry
/// `entry` gives, by application: the application with the newest
/// notification first.
pub fn groups<T>(items: &[T], entry: impl Fn(&T) -> &Entry) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    let mut found: HashMap<String, usize> = HashMap::new();
    for (at, item) in items.iter().enumerate() {
        let key = entry(item).group();
        match found.get(&key) {
            Some(&group) => groups[group].members.push(at),
            None => {
                found.insert(key.clone(), groups.len());
                groups.push(Group {
                    key,
                    members: vec![at],
                });
            }
        }
    }
    groups
}

/// Takes into `entries` (newest first) what the notification center now
/// holds, `listed`. Known notifications get their new texts, those that
/// left are marked so and stay, and new ones are added at their place;
/// `next_key` gives each of them its key. Those `hidden` says are not
/// wanted, and those in `dismissed` (taken away here, which Windows may
/// not have done yet), are passed over; an identifier of `dismissed` that
/// is no longer listed is forgotten. Returns the keys of the new ones,
/// oldest first, unless `first`: what the notification center holds when
/// it is first read has arrived before and is not news. The second answer
/// tells whether anything changed.
fn merge(
    entries: &mut Vec<Entry>,
    listed: Vec<Notification>,
    first: bool,
    next_key: &mut u64,
    dismissed: &mut HashSet<u32>,
    hidden: impl Fn(&Notification) -> bool,
) -> (Vec<u64>, bool) {
    let ids: HashSet<u32> = listed.iter().map(|notification| notification.id).collect();
    dismissed.retain(|id| ids.contains(id));
    let mut changed = false;
    for entry in entries.iter_mut() {
        if entry.present && !entry.id.is_some_and(|id| ids.contains(&id)) {
            entry.present = false;
            changed = true;
        }
    }
    let mut arrived = Vec::new();
    for notification in listed {
        if dismissed.contains(&notification.id) || hidden(&notification) {
            continue;
        }
        let id = Some(notification.id);
        // The same notification while it is held, or one of an earlier
        // run of Tessera, which Windows held all along.
        let known = entries
            .iter_mut()
            .find(|entry| entry.id == id && (entry.present || entry.posted == notification.posted));
        if let Some(entry) = known {
            if !entry.present || !entry.says(&notification) {
                // The toast stays until it is read again.
                let (key, toast) = (entry.key, entry.toast.take());
                *entry = Entry {
                    toast,
                    ..Entry::of(notification, key)
                };
                changed = true;
            }
            continue;
        }
        let key = *next_key;
        *next_key += 1;
        let at = entries
            .iter()
            .position(|entry| entry.posted < notification.posted)
            .unwrap_or(entries.len());
        arrived.push((notification.posted, key));
        entries.insert(at, Entry::of(notification, key));
        changed = true;
    }
    if first {
        arrived.clear();
    }
    arrived.sort();
    (arrived.into_iter().map(|(_, key)| key).collect(), changed)
}

/// Removes from `entries` the oldest of those the notification center no
/// longer holds, until no more than `keep` of them are left. Those it
/// holds stay whatever their number. Returns whether any was removed.
fn trim(entries: &mut Vec<Entry>, keep: usize) -> bool {
    let mut gone = 0;
    let before = entries.len();
    entries.retain(|entry| {
        if entry.present {
            return true;
        }
        gone += 1;
        gone <= keep
    });
    entries.len() != before
}

/// What on a notification was pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// The notification itself.
    Body,
    /// The header it is grouped under.
    Header,
    /// The action of its toast at this place, a button or an entry of its
    /// menu.
    Action(usize),
}

/// What the history file holds.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    /// When the newest notification that was listed was posted.
    seen: i64,
    /// Until when do not disturb holds, in seconds since 1970.
    #[serde(skip_serializing_if = "Option::is_none")]
    quiet_until: Option<i64>,
    #[serde(rename = "notification")]
    entries: Vec<Entry>,
}

/// Where the history is kept: the `tessera` directory of the local
/// application data, which does not roam with the profile.
fn history_path() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").filter(|local| !local.is_empty())?;
    Some(PathBuf::from(local).join("tessera").join(HISTORY_FILE))
}

/// Reads the history; a missing file is an empty one. One that cannot be
/// used is reported and replaced at the next change.
fn load() -> Saved {
    let Some(path) = history_path() else {
        return Saved::default();
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Saved::default(),
        Err(error) => {
            report!(
                "the notification history in {} is not read: {error}",
                path.display()
            );
            return Saved::default();
        }
    };
    toml::from_str(&text).unwrap_or_else(|error| {
        report!(
            "the notification history in {} is started anew: {error}",
            path.display()
        );
        Saved::default()
    })
}

/// Writes the history, by way of a file beside it, so that a history that
/// was being written when Tessera ended is not lost.
fn store(saved: &Saved) -> Result<(), String> {
    let path = history_path().ok_or("there is no folder of local application data")?;
    let text = toml::to_string(saved).map_err(|error| error.to_string())?;
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    }
    let written = path.with_extension("toml.new");
    fs::write(&written, text).map_err(|error| error.to_string())?;
    fs::rename(&written, &path).map_err(|error| error.to_string())
}

/// Seconds since 1970 now.
pub fn unix_now() -> i64 {
    let since = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH);
    since.map_or(0, |since| since.as_secs() as i64)
}

/// The notifications Tessera keeps, and what goes with them. Those who
/// observe it are told of every change, and only of a change.
#[derive(Default)]
pub struct NotificationStore {
    /// Newest first.
    entries: Vec<Entry>,
    rules: Vec<NotificationRule>,
    /// How many notifications that left the notification center are kept.
    keep: usize,
    next_key: u64,
    /// Why the notification center cannot be read, while it cannot.
    error: Option<String>,
    /// When the newest notification that was listed was posted.
    seen: i64,
    /// Until when do not disturb holds, in seconds since 1970.
    quiet_until: Option<i64>,
    /// What keeps the user busy, as last asked.
    occupation: Option<Occupation>,
    /// The logos of the applications, by the identifier they post under.
    logos: HashMap<String, Icon>,
    /// The pictures of the toasts, by the application and the name the
    /// toast gives them.
    pictures: HashMap<(String, String), Icon>,
    /// Why the toasts cannot be read from the database, while they cannot.
    payload_error: Option<String>,
    /// What Windows' own do not disturb lets through, while it is
    /// followed.
    windows_quiet: Option<QuietProfile>,
    /// What the settings of Windows say of each application, while they
    /// are followed, by the identifier it posts under.
    app_settings: HashMap<String, AppNotificationSettings>,
    /// The jobs on files that run, oldest first.
    jobs: Vec<Job>,
}

impl Global for NotificationStore {}

/// The name the self test of Windows knows the store by.
#[allow(
    dead_code,
    reason = "read by the report of windows-selftest after merge"
)]
pub type NotificationsStore = NotificationStore;

/// What reads the notification center for the store, kept apart from it
/// so that its own changes tell no one.
#[derive(Default)]
struct Watch {
    /// What the notification center held when it was last read.
    last: Option<Vec<Notification>>,
    /// Whether a notification that is new to the store is news to show.
    /// It is not at the first reading, nor at the one after the rules
    /// changed, as what they no longer hide arrived before.
    news: bool,
    /// Notifications dismissed here that the notification center may
    /// still hold.
    dismissed: HashSet<u32>,
    /// Whether what keeps the user busy is asked, and whether a display
    /// that is duplicated counts.
    occupation: Option<bool>,
    /// The applications whose logos were looked for.
    logos: HashSet<String>,
    /// The pictures that were looked for.
    pictures: HashSet<(String, String)>,
    /// The notifications whose toast was looked for in the database, with
    /// the texts they had then; one whose texts change is read again.
    tried: HashMap<u32, (String, String)>,
    /// The new notifications whose banners wait for their toasts.
    waiting_banners: Vec<u64>,
    /// Whether Windows' do not disturb and the settings of the
    /// applications are followed.
    follow_windows: bool,
    /// Whether pictures of the web are fetched.
    web_images: bool,
    /// How many readings of the list there were.
    readings: u32,
    reading: Option<Task<()>>,
    saving: Option<Task<()>>,
    payloads: Option<Task<()>>,
    following: Option<Task<()>>,
    jobs: Option<Task<()>>,
}

impl Global for Watch {}

impl NotificationStore {
    /// Takes over what the configuration says, and starts reading the
    /// notification center unless it is read already.
    pub fn configure(config: &NotificationsConfig, rules: &[NotificationRule], cx: &mut App) {
        let keep = config.history();
        let this = cx.default_global::<Self>();
        let rules_changed = this.rules != rules;
        this.rules = rules.to_vec();
        this.keep = keep;
        if rules_changed {
            let rules = &this.rules;
            this.entries.retain(|entry| {
                rule_for(rules, &entry.app, &entry.app_id).is_none_or(|rule| rule.show)
            });
            // What the notification center holds is read anew, so that
            // notifications no longer hidden are taken in.
            let watch = cx.default_global::<Watch>();
            watch.last = None;
            watch.news = false;
        }
        if trim(&mut cx.global_mut::<Self>().entries, keep) {
            Self::save_later(cx);
        }
        let watch = cx.default_global::<Watch>();
        watch.occupation = config.occupations();
        watch.web_images = config.web_images();
        let follow = config.follow_windows();
        if watch.follow_windows != follow {
            watch.follow_windows = follow;
            watch.readings = 0;
            let following = follow.then(|| cx.spawn(Self::follow_windows));
            cx.global_mut::<Watch>().following = following;
            if !follow {
                let this = cx.global_mut::<Self>();
                this.windows_quiet = None;
                this.app_settings.clear();
            }
        }
        if cx.global::<Watch>().reading.is_none() {
            let reading = cx.spawn(Self::read);
            cx.global_mut::<Watch>().reading = Some(reading);
        }
        jobs::configure(config.jobs(), cx);
        Badges::count(cx);
    }

    /// Follows Windows' own do not disturb for as long as it is asked to.
    async fn follow_windows(cx: &mut AsyncApp) {
        let read = cx
            .background_executor()
            .spawn(async { WindowsQuiet::now().and_then(|now| Ok((now, WindowsQuiet::follow()?))) })
            .await;
        let changes = match read {
            Ok((now, changes)) => {
                let set = cx.update(|cx| Self::take_windows_quiet(Some(now), cx));
                if set.is_err() {
                    return;
                }
                changes
            }
            Err(error) => {
                report!("{error}; Tessera's do not disturb does not follow Windows'");
                return;
            }
        };
        while let Ok(profile) = changes.recv().await {
            if cx
                .update(|cx| Self::take_windows_quiet(Some(profile), cx))
                .is_err()
            {
                break;
            }
        }
    }

    fn take_windows_quiet(profile: Option<QuietProfile>, cx: &mut App) {
        if cx.global::<Self>().windows_quiet != profile {
            cx.global_mut::<Self>().windows_quiet = profile;
        }
    }

    /// Reads the history, then the notification center every two seconds
    /// and each time Windows tells of a change, for as long as Tessera
    /// runs.
    async fn read(cx: &mut AsyncApp) {
        let saved = cx.background_executor().spawn(async { load() }).await;
        if cx.update(|cx| Self::restore(saved, cx)).is_err() {
            return;
        }
        // The ticks of the clock and the changes Windows tells of wake the
        // reading alike; one waiting wake is enough.
        let (wake, woken) = async_channel::bounded::<()>(1);
        let ticks = wake.clone();
        cx.background_executor()
            .spawn(async move {
                let changes = Notifications::watch();
                if let Ok(changes) = changes {
                    while changes.recv().await.is_ok() {
                        if wake.try_send(()).is_err() && wake.is_closed() {
                            break;
                        }
                    }
                }
            })
            .detach();
        let clock = cx.background_executor().clone();
        cx.background_executor()
            .spawn(async move {
                loop {
                    clock.timer(REFRESH_INTERVAL).await;
                    if ticks.try_send(()).is_err() && ticks.is_closed() {
                        break;
                    }
                }
            })
            .detach();
        loop {
            let Ok((asked, settings_of)) = cx.update(Self::what_to_ask) else {
                break;
            };
            let read = cx.background_executor().spawn(async move {
                let occupation = asked.and_then(Occupation::now);
                let settings: Option<HashMap<String, AppNotificationSettings>> =
                    settings_of.map(|ids| {
                        { ids.into_iter() }
                            .map(|id| {
                                let settings = AppNotificationSettings::of(&id);
                                (id, settings)
                            })
                            .collect()
                    });
                (Notifications::list(), occupation, settings)
            });
            let (listed, occupation, settings) = read.await;
            if cx
                .update(|cx| Self::take(listed, occupation, settings, cx))
                .is_err()
            {
                break;
            }
            if woken.recv().await.is_err() {
                break;
            }
        }
    }

    /// What the next reading asks besides the list: whether the user is
    /// busy (and whether duplicating counts), and the applications whose
    /// settings in Windows are read again, now and then while they are
    /// followed.
    fn what_to_ask(cx: &mut App) -> (Option<bool>, Option<Vec<String>>) {
        let ids: Vec<String> = {
            let mut ids: Vec<String> = { cx.global::<Self>().entries.iter() }
                .map(|entry| entry.app_id.clone())
                .filter(|id| !id.is_empty())
                .collect();
            ids.sort();
            ids.dedup();
            ids
        };
        let watch = cx.global_mut::<Watch>();
        let again = watch.follow_windows && watch.readings.is_multiple_of(SETTINGS_EVERY);
        watch.readings = watch.readings.wrapping_add(1);
        (watch.occupation, again.then_some(ids))
    }

    /// Takes in the history that was read.
    fn restore(saved: Saved, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        let rules = &this.rules;
        let mut entries: Vec<Entry> = { saved.entries.into_iter() }
            .filter(|entry| rule_for(rules, &entry.app, &entry.app_id).is_none_or(|rule| rule.show))
            .collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.posted));
        for entry in &mut entries {
            entry.key = this.next_key;
            this.next_key += 1;
        }
        // Tessera's own notifications that came before the history was
        // read stay.
        entries.append(&mut this.entries);
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.posted));
        this.entries = entries;
        this.seen = this.seen.max(saved.seen);
        this.quiet_until = this.quiet_until.or(saved.quiet_until);
        trim(&mut this.entries, this.keep);
    }

    /// Takes in what the notification center holds and what keeps the user
    /// busy; the banners are told of new notifications.
    fn take(
        listed: Result<Vec<Notification>, String>,
        occupation: Option<Occupation>,
        settings: Option<HashMap<String, AppNotificationSettings>>,
        cx: &mut App,
    ) {
        if cx.global::<Self>().occupation != occupation {
            cx.global_mut::<Self>().occupation = occupation;
            Banners::occupation_changed(occupation, cx);
        }
        if let Some(settings) = settings
            && cx.global::<Self>().app_settings != settings
        {
            cx.global_mut::<Self>().app_settings = settings;
        }
        let listed = match listed {
            Ok(listed) => listed,
            Err(error) => {
                if cx.global::<Self>().error.as_ref() != Some(&error) {
                    report!("{error}");
                    cx.global_mut::<Self>().error = Some(error);
                }
                return;
            }
        };
        if cx.global::<Self>().error.is_some() {
            cx.global_mut::<Self>().error = None;
        }
        let watch = cx.global_mut::<Watch>();
        if watch.last.as_ref() == Some(&listed) {
            // A progress bar may have moved without a text changing.
            Self::read_payloads_later(Vec::new(), cx);
            return;
        }
        let first = !watch.news;
        watch.news = true;
        watch.last = Some(listed.clone());
        let mut dismissed = std::mem::take(&mut watch.dismissed);
        let store = cx.global::<Self>();
        let rules = store.rules.clone();
        let mut entries = store.entries.clone();
        let mut next_key = store.next_key;
        let silenced: HashSet<String> = { store.app_settings.iter() }
            .filter(|(_, settings)| !settings.enabled)
            .map(|(id, _)| id.clone())
            .collect();
        let hidden = |notification: &Notification| {
            let rule = rule_for(&rules, &notification.app, &notification.app_id);
            rule.is_some_and(|rule| !rule.show) || silenced.contains(&notification.app_id)
        };
        let (arrived, changed) = merge(
            &mut entries,
            listed,
            first,
            &mut next_key,
            &mut dismissed,
            hidden,
        );
        cx.global_mut::<Watch>().dismissed = dismissed;
        if changed {
            let keep = cx.global::<Self>().keep;
            trim(&mut entries, keep);
            let this = cx.global_mut::<Self>();
            this.entries = entries;
            this.next_key = next_key;
            Self::save_later(cx);
            Badges::count(cx);
        }
        Self::look_for_logos(cx);
        Self::read_payloads_later(arrived, cx);
    }

    /// The notifications whose toasts are to be read: those held whose
    /// texts changed since their toast was looked for, or that were never
    /// looked for, and those with a progress bar, which may move without
    /// a text changing.
    fn payloads_wanted(cx: &App) -> Vec<u32> {
        let tried = &cx.global::<Watch>().tried;
        { cx.global::<Self>().entries.iter() }
            .filter(|entry| entry.present)
            .filter_map(|entry| {
                let id = entry.id?;
                let texts = (entry.title.clone(), entry.body.clone());
                let moving =
                    { entry.toast.as_ref() }.is_some_and(|toast| !toast.progress.is_empty());
                (moving || tried.get(&id) != Some(&texts)).then_some(id)
            })
            .collect()
    }

    /// Reads the toasts of the notifications that want it a moment from
    /// now, unless that is under way; the banners of `arrived` come once
    /// their toasts were read, or could not be.
    fn read_payloads_later(arrived: Vec<u64>, cx: &mut App) {
        cx.global_mut::<Watch>().waiting_banners.extend(arrived);
        if cx.global::<Watch>().payloads.is_some() {
            return;
        }
        if Self::payloads_wanted(cx).is_empty() {
            Self::release_banners(cx);
            return;
        }
        let reading = cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor().timer(PAYLOAD_DELAY).await;
            let Ok(wanted) = cx.update(|cx| Self::payloads_wanted(cx)) else {
                return;
            };
            let answer = NotificationDatabase::read(wanted.clone()).recv().await;
            let answer =
                answer.unwrap_or_else(|_| Err("the notification database was not read".to_owned()));
            let parsed = cx
                .background_executor()
                .spawn(async move {
                    answer.map(|payloads| {
                        { payloads.into_iter() }
                            .map(|payload| (payload.id, Toast::parse(&payload.xml, &payload.data)))
                            .collect::<Vec<_>>()
                    })
                })
                .await;
            cx.update(|cx| {
                cx.global_mut::<Watch>().payloads = None;
                Self::take_payloads(wanted, parsed, cx);
                Self::release_banners(cx);
            })
            .ok();
        });
        cx.global_mut::<Watch>().payloads = Some(reading);
    }

    /// Shows the banners that waited for their toasts.
    fn release_banners(cx: &mut App) {
        let waiting = std::mem::take(&mut cx.global_mut::<Watch>().waiting_banners);
        if !waiting.is_empty() {
            Banners::arrive(&waiting, cx);
        }
    }

    /// Takes in the toasts read for the notifications `wanted`.
    fn take_payloads(
        wanted: Vec<u32>,
        parsed: Result<Vec<(u32, Option<Toast>)>, String>,
        cx: &mut App,
    ) {
        let texts: HashMap<u32, (String, String)> = { cx.global::<Self>().entries.iter() }
            .filter(|entry| entry.present)
            .filter_map(|entry| Some((entry.id?, (entry.title.clone(), entry.body.clone()))))
            .collect();
        let watch = cx.global_mut::<Watch>();
        for id in &wanted {
            if let Some(texts) = texts.get(id) {
                watch.tried.insert(*id, texts.clone());
            }
        }
        watch.tried.retain(|id, _| texts.contains_key(id));
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                if cx.global::<Self>().payload_error.as_ref() != Some(&error) {
                    report!("{error}; notifications show their texts only");
                    cx.global_mut::<Self>().payload_error = Some(error);
                }
                return;
            }
        };
        if cx.global::<Self>().payload_error.is_some() {
            cx.global_mut::<Self>().payload_error = None;
        }
        let mut found = Vec::new();
        {
            let this = cx.global_mut::<Self>();
            for (id, toast) in parsed {
                let Some(toast) = toast else {
                    continue;
                };
                let held =
                    { this.entries.iter_mut() }.find(|entry| entry.present && entry.id == Some(id));
                let Some(entry) = held else {
                    continue;
                };
                if entry.toast.as_deref() == Some(&toast) {
                    continue;
                }
                NotificationDatabase::saw(&entry.title);
                let toast = Arc::new(toast);
                found.push((entry.app_id.clone(), toast.clone()));
                entry.toast = Some(toast);
            }
        }
        for (app_id, toast) in found {
            pictures::look_for(&app_id, &toast, cx);
        }
    }

    /// Looks for the logos of the applications that have none yet: the
    /// one Windows gave with their notifications, or else the icon the
    /// shell draws for them.
    fn look_for_logos(cx: &mut App) {
        let wanted: Vec<String> = {
            let asked = &cx.global::<Watch>().logos;
            let ids = cx
                .global::<Self>()
                .entries
                .iter()
                .map(|entry| &entry.app_id);
            let mut wanted: Vec<String> = ids
                .filter(|id| !id.is_empty() && !asked.contains(*id))
                .cloned()
                .collect();
            wanted.sort();
            wanted.dedup();
            wanted
        };
        if wanted.is_empty() {
            return;
        }
        cx.global_mut::<Watch>()
            .logos
            .extend(wanted.iter().cloned());
        cx.spawn(async move |cx: &mut AsyncApp| {
            for app_id in wanted {
                let id = app_id.clone();
                let logo = cx.background_executor().spawn(async move {
                    let bytes = Notifications::logo(&id)?;
                    image_of_bytes(&bytes, LOGO_PIXELS).ok().flatten()
                });
                let mut logo = logo.await;
                if logo.is_none() {
                    let id = app_id.clone();
                    let icon = ShellThread::call(move || IconPixels::of_app(&id, LOGO_PIXELS));
                    logo = icon
                        .recv()
                        .await
                        .ok()
                        .flatten()
                        .and_then(|pixels| icon_of_pixels(&pixels));
                }
                let Some(logo) = logo else {
                    continue;
                };
                let stored = cx.update(|cx| {
                    cx.global_mut::<Self>().logos.insert(app_id, Some(logo));
                });
                if stored.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Writes the history once it has stayed the same for a moment.
    fn save_later(cx: &mut App) {
        if cx.default_global::<Watch>().saving.is_some() {
            return;
        }
        let saving = cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let Ok(saved) = cx.update(|cx| {
                cx.global_mut::<Watch>().saving = None;
                let this = cx.global::<Self>();
                Saved {
                    seen: this.seen,
                    quiet_until: this.quiet_until,
                    entries: this.entries.clone(),
                }
            }) else {
                return;
            };
            let written = cx.background_executor().spawn(async move { store(&saved) });
            if let Err(error) = written.await {
                report!("the notification history is not written: {error}");
            }
        });
        cx.global_mut::<Watch>().saving = Some(saving);
    }

    /// The notifications, newest first.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The notification `key`.
    pub fn entry(&self, key: u64) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.key == key)
    }

    /// The `[[notification_rule]]` entries of the configuration.
    pub fn rules(&self) -> &[NotificationRule] {
        &self.rules
    }

    /// The rule that decides for the application of `entry`.
    pub fn rule(&self, entry: &Entry) -> Option<&NotificationRule> {
        rule_for(&self.rules, &entry.app, &entry.app_id)
    }

    /// Why the notification center cannot be read, while it cannot.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The logo of the application that posts under `app_id`, once found.
    pub fn logo(&self, app_id: &str) -> Icon {
        self.logos.get(app_id).cloned().flatten()
    }

    /// How many notifications came since the list was last looked at.
    pub fn unseen(&self) -> usize {
        let seen = self.seen;
        self.entries
            .iter()
            .filter(|entry| entry.posted > seen)
            .count()
    }

    /// What keeps the user busy, as last asked; nothing while it is not
    /// asked.
    pub fn occupation(&self) -> Option<Occupation> {
        self.occupation
    }

    /// Until when do not disturb was asked to hold, in seconds since
    /// 1970; it may have passed.
    pub fn quiet_until(&self) -> Option<i64> {
        self.quiet_until
    }

    /// Has do not disturb hold until `until`, or no longer by time.
    pub fn set_quiet_until(until: Option<i64>, cx: &mut App) {
        let now = cx.try_global::<Self>().and_then(|this| this.quiet_until);
        if now != until {
            cx.default_global::<Self>().quiet_until = until;
            Self::save_later(cx);
        }
    }

    /// Counts every notification there is as looked at.
    pub fn see(cx: &mut App) {
        // Reading tells no one; only a change does.
        let Some(this) = cx.try_global::<Self>() else {
            return;
        };
        let newest = this.entries.iter().map(|entry| entry.posted).max();
        if let Some(newest) = newest.filter(|&newest| newest > this.seen) {
            cx.global_mut::<Self>().seen = newest;
            Self::save_later(cx);
        }
    }

    /// Adds a notification of Tessera's own, posted now, and returns its
    /// key.
    pub fn add_own(app: String, title: String, body: String, cx: &mut App) -> u64 {
        let this = cx.default_global::<Self>();
        let key = this.next_key;
        this.next_key += 1;
        this.entries.insert(
            0,
            Entry {
                key,
                id: None,
                app,
                app_id: String::new(),
                title,
                body,
                posted: unix_now(),
                present: false,
                toast: None,
                snoozed_until: None,
                destination: None,
            },
        );
        let keep = this.keep;
        trim(&mut this.entries, keep);
        Self::save_later(cx);
        key
    }

    /// Does what `press` on the notification `key` asks, with `typed` the
    /// values the user gave its input fields, by their identifiers. A
    /// snooze shows it again after the minutes chosen; anything else takes
    /// it away afterwards, as Windows does.
    pub fn activate(key: u64, press: Press, typed: &HashMap<String, String>, cx: &mut App) {
        let Some(entry) = cx.default_global::<Self>().entry(key).cloned() else {
            return;
        };
        let toast = entry.toast.clone();
        let asked = match (&toast, press) {
            (Some(toast), Press::Body) => {
                Some((toast.launch.clone().unwrap_or_default(), toast.activation))
            }
            (Some(toast), Press::Header) => toast
                .header
                .as_ref()
                .map(|header| (header.arguments.clone(), header.activation)),
            (Some(toast), Press::Action(index)) => {
                let Some(action) = toast.actions.get(index) else {
                    return;
                };
                if action.is_snooze() {
                    let chosen = { action.input_id.as_ref() }.and_then(|id| {
                        typed.get(id).cloned().or_else(|| {
                            input_values(toast, typed)
                                .into_iter()
                                .find(|(input, _)| input == id)
                                .map(|(_, value)| value)
                        })
                    });
                    let minutes = toast.snooze_minutes(chosen.as_deref());
                    Self::snooze(key, minutes, cx);
                    return;
                }
                if action.is_dismiss() {
                    Self::dismiss(&[key], cx);
                    return;
                }
                Some((action.arguments.clone(), action.activation))
            }
            (None, _) => None,
        };
        let route = |activation: Activation| match activation {
            Activation::Background => ToastRoute::Background,
            Activation::Protocol => ToastRoute::Protocol,
            Activation::Foreground | Activation::System => ToastRoute::Foreground,
        };
        match asked {
            Some((arguments, activation))
                if !entry.app_id.is_empty() || activation == Activation::Protocol =>
            {
                let inputs = { toast.as_deref() }
                    .map(|toast| input_values(toast, typed))
                    .unwrap_or_default();
                let started = ToastActivation {
                    app_id: entry.app_id.clone(),
                    arguments,
                    route: route(activation),
                    inputs,
                }
                .start();
                let (app_id, app) = (entry.app_id.clone(), entry.app.clone());
                cx.spawn(async move |_: &mut AsyncApp| match started.recv().await {
                    Ok(Ok(
                        Activated::AsToast
                        | Activated::AsLaunch
                        | Activated::Address
                        | Activated::Opened,
                    )) => {}
                    Ok(Err(error)) => {
                        report!("{error}");
                        // The application is opened at least, as before.
                        if press == Press::Body && !app_id.is_empty() {
                            crate::platform::Launch::app(&app_id)
                                .named(app)
                                .quiet()
                                .start();
                        }
                    }
                    Err(_) => {}
                })
                .detach();
            }
            _ if press == Press::Body && !entry.app_id.is_empty() => {
                crate::platform::Launch::app(&entry.app_id)
                    .named(entry.app.clone())
                    .quiet()
                    .start();
            }
            _ => {}
        }
        Self::dismiss(&[key], cx);
    }

    /// Takes the notification `key` out of the notification center of
    /// Windows and out of the list for `minutes`, and then shows it again
    /// as a banner, as Windows' snooze does.
    pub fn snooze(key: u64, minutes: u32, cx: &mut App) {
        let until = unix_now() + i64::from(minutes) * 60;
        let this = cx.default_global::<Self>();
        let Some(entry) = this.entries.iter_mut().find(|entry| entry.key == key) else {
            return;
        };
        entry.snoozed_until = Some(until);
        let id = entry.id.filter(|_| entry.present);
        Banners::withdraw(key, cx);
        if let Some(id) = id {
            Self::forget_in_windows(vec![id], cx);
        }
        let back = cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(Duration::from_secs(u64::from(minutes) * 60))
                .await;
            cx.update(|cx| {
                let this = cx.global_mut::<Self>();
                let Some(entry) = this.entries.iter_mut().find(|entry| entry.key == key) else {
                    return;
                };
                entry.snoozed_until = None;
                entry.posted = unix_now();
                this.entries
                    .sort_by_key(|entry| std::cmp::Reverse(entry.posted));
                Banners::arrive(&[key], cx);
                Badges::count(cx);
            })
            .ok();
        });
        back.detach();
        Badges::count(cx);
    }

    /// The picture of a toast of the application `app_id` that the toast
    /// names `src`, once it was read.
    pub fn picture(&self, app_id: &str, src: &str) -> Icon {
        { self.pictures.get(&(app_id.to_owned(), src.to_owned())) }
            .cloned()
            .flatten()
    }

    /// Whether the toast of a notification titled `title` was read from
    /// Windows' notification database while Tessera runs; the self test
    /// asks it after it sent a toast.
    #[allow(
        dead_code,
        reason = "read by the report of windows-selftest after merge"
    )]
    pub fn payload_seen(title: &str) -> bool {
        NotificationDatabase::seen(title)
    }

    /// What Windows' own do not disturb lets through, while it is
    /// followed.
    pub fn windows_quiet(&self) -> Option<QuietProfile> {
        self.windows_quiet
    }

    /// What the settings of Windows say of the application `app_id`,
    /// while they are followed; everything on otherwise.
    pub fn app_settings(&self, app_id: &str) -> AppNotificationSettings {
        self.app_settings.get(app_id).copied().unwrap_or_default()
    }

    /// The jobs on files that run, oldest first.
    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    /// Takes the notifications `keys` away, here and from the notification
    /// center of Windows.
    pub fn dismiss(keys: &[u64], cx: &mut App) {
        let this = cx.default_global::<Self>();
        let ids: Vec<u32> = { this.entries.iter() }
            .filter(|entry| keys.contains(&entry.key) && entry.present)
            .filter_map(|entry| entry.id)
            .collect();
        let before = this.entries.len();
        this.entries.retain(|entry| !keys.contains(&entry.key));
        if this.entries.len() == before {
            return;
        }
        Self::forget_in_windows(ids, cx);
        Self::save_later(cx);
        Badges::count(cx);
    }

    /// Takes every notification listed here away, here and from the
    /// notification center of Windows. What a rule keeps out of the list,
    /// and what arrived since the list was last read, stays there.
    pub fn clear(cx: &mut App) {
        let this = cx.default_global::<Self>();
        let ids: Vec<u32> = { this.entries.iter() }
            .filter(|entry| entry.present)
            .filter_map(|entry| entry.id)
            .collect();
        this.entries.clear();
        Self::forget_in_windows(ids, cx);
        Self::save_later(cx);
        Badges::count(cx);
    }

    /// Has the notification center of Windows let go of `ids`. Until it
    /// has, they are not taken in again.
    fn forget_in_windows(ids: Vec<u32>, cx: &mut App) {
        if ids.is_empty() {
            return;
        }
        cx.default_global::<Watch>()
            .dismissed
            .extend(ids.iter().copied());
        let dismissing = cx.background_executor().spawn(async move {
            ids.into_iter()
                .map(Notifications::dismiss)
                .find_map(Result::err)
        });
        cx.spawn(async move |_: &mut AsyncApp| {
            if let Some(error) = dismissing.await {
                report!("{error}");
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(id: u32, app_id: &str, posted: i64) -> Notification {
        Notification {
            id,
            app: format!("App {app_id}"),
            app_id: app_id.to_owned(),
            title: format!("Title {id}"),
            body: String::new(),
            posted,
        }
    }

    fn keys(entries: &[Entry]) -> Vec<Option<u32>> {
        entries.iter().map(|entry| entry.id).collect()
    }

    #[test]
    fn a_pattern_stands_for_names_in_any_case() {
        assert!(matches_pattern("Teams", "teams"));
        assert!(matches_pattern("micro*", "Microsoft.Teams"));
        assert!(matches_pattern("*teams*", "MSTeams_8wekyb3d8bbwe!MSTeams"));
        assert!(matches_pattern("ch?t", "chat"));
        assert!(matches_pattern("*", ""));
        assert!(matches_pattern("a*b*c", "aXbYbZc"));
        assert!(!matches_pattern("ch?t", "chaat"));
        assert!(!matches_pattern("teams", "teams2"));
        assert!(!matches_pattern("a*b*c", "aXbYbZ"));
    }

    #[test]
    fn the_first_rule_that_names_an_application_decides() {
        let rule = |app: &str, banners| NotificationRule {
            app: app.to_owned(),
            banners,
            show: true,
            priority: false,
            badge: true,
        };
        let rules = [rule("Mail", false), rule("*", true)];
        assert!(
            !rule_for(&rules, "Mail", "microsoft.windowscommunicationsapps")
                .unwrap()
                .banners
        );
        assert!(
            rule_for(&rules, "Chat", "com.example.chat")
                .unwrap()
                .banners
        );
        assert!(rule_for(&rules[..1], "", "").is_none());
        // A rule names the identifier as well as the name.
        let by_id = [rule("com.example.*", false)];
        assert!(rule_for(&by_id, "Chat", "com.example.Chat").is_some());
        assert!(rule_for(&[rule(" ", false)], "Chat", "chat").is_none());
    }

    #[test]
    fn what_was_there_at_first_is_no_news_and_what_left_stays() {
        let mut entries = Vec::new();
        let (mut next, mut dismissed) = (0, HashSet::new());
        let first = vec![notification(1, "a", 10), notification(2, "b", 20)];
        let (arrived, changed) =
            merge(&mut entries, first, true, &mut next, &mut dismissed, |_| {
                false
            });
        assert!(arrived.is_empty() && changed);
        assert_eq!(keys(&entries), [Some(2), Some(1)]);

        let next_listed = vec![
            notification(4, "a", 40),
            notification(3, "b", 30),
            notification(2, "b", 20),
        ];
        let (arrived, _) = merge(
            &mut entries,
            next_listed,
            false,
            &mut next,
            &mut dismissed,
            |_| false,
        );
        // Oldest first, so that the newest banner ends up on top.
        let arrived: Vec<_> = arrived
            .iter()
            .map(|key| entries.iter().find(|entry| entry.key == *key).unwrap().id)
            .collect();
        assert_eq!(arrived, [Some(3), Some(4)]);
        assert_eq!(keys(&entries), [Some(4), Some(3), Some(2), Some(1)]);
        // The first one left the notification center but is kept.
        assert!(!entries[3].present);
        assert!(entries[..3].iter().all(|entry| entry.present));

        // The same reading changes nothing.
        let again = vec![
            notification(4, "a", 40),
            notification(3, "b", 30),
            notification(2, "b", 20),
        ];
        let (arrived, changed) = merge(
            &mut entries,
            again,
            false,
            &mut next,
            &mut dismissed,
            |_| false,
        );
        assert!(arrived.is_empty() && !changed);
    }

    #[test]
    fn a_notification_that_changes_its_text_is_the_same_one() {
        let mut entries = Vec::new();
        let (mut next, mut dismissed) = (0, HashSet::new());
        merge(
            &mut entries,
            vec![notification(7, "a", 5)],
            false,
            &mut next,
            &mut dismissed,
            |_| false,
        );
        let key = entries[0].key;
        let mut changed = notification(7, "a", 5);
        changed.body = "50 %".to_owned();
        let (arrived, was_changed) = merge(
            &mut entries,
            vec![changed],
            false,
            &mut next,
            &mut dismissed,
            |_| false,
        );
        assert!(arrived.is_empty() && was_changed);
        assert_eq!((entries.len(), entries[0].key), (1, key));
        assert_eq!(entries[0].body, "50 %");
    }

    #[test]
    fn a_notification_of_an_earlier_run_is_found_again() {
        let mut old = Entry::of(notification(3, "a", 30), 0);
        old.present = false;
        let mut entries = vec![old];
        let (mut next, mut dismissed) = (1, HashSet::new());
        let listed = vec![notification(3, "a", 30), notification(3 + 1, "a", 31)];
        let (arrived, _) = merge(
            &mut entries,
            listed,
            true,
            &mut next,
            &mut dismissed,
            |_| false,
        );
        assert!(arrived.is_empty());
        assert_eq!(keys(&entries), [Some(4), Some(3)]);
        assert!(entries.iter().all(|entry| entry.present));
        // An identifier Windows gave again to another notification is
        // another one.
        let mut entries = vec![Entry {
            present: false,
            ..Entry::of(notification(3, "a", 30), 0)
        }];
        merge(
            &mut entries,
            vec![notification(3, "b", 99)],
            false,
            &mut next,
            &mut dismissed,
            |_| false,
        );
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn dismissed_and_hidden_notifications_are_not_taken_in() {
        let mut entries = Vec::new();
        let mut next = 0;
        let mut dismissed = HashSet::from([1, 9]);
        let listed = vec![notification(1, "a", 10), notification(2, "secret", 20)];
        let hidden = |notification: &Notification| notification.app_id == "secret";
        let (arrived, _) = merge(
            &mut entries,
            listed,
            false,
            &mut next,
            &mut dismissed,
            hidden,
        );
        assert!(arrived.is_empty() && entries.is_empty());
        // 9 is no longer listed, so it is forgotten; 1 still is.
        assert_eq!(dismissed, HashSet::from([1]));
    }

    #[test]
    fn the_history_keeps_what_is_held_and_the_newest_of_the_rest() {
        let entry = |id, present| Entry {
            present,
            ..Entry::of(notification(id, "a", i64::from(100 - id)), u64::from(id))
        };
        let mut entries = vec![
            entry(1, true),
            entry(2, false),
            entry(3, true),
            entry(4, false),
            entry(5, false),
        ];
        assert!(trim(&mut entries, 1));
        assert_eq!(keys(&entries), [Some(1), Some(2), Some(3)]);
        assert!(!trim(&mut entries, 1));
        assert!(trim(&mut entries, 0));
        assert_eq!(keys(&entries), [Some(1), Some(3)]);
    }

    #[test]
    fn notifications_are_grouped_by_application_newest_first() {
        let mut entries = Vec::new();
        let (mut next, mut dismissed) = (0, HashSet::new());
        let listed = vec![
            notification(1, "Mail", 10),
            notification(2, "chat", 20),
            notification(3, "mail", 30),
        ];
        merge(
            &mut entries,
            listed,
            true,
            &mut next,
            &mut dismissed,
            |_| false,
        );
        let groups = groups(&entries, |entry| entry);
        let shape: Vec<(String, Vec<Option<u32>>)> = groups
            .iter()
            .map(|group| {
                let ids = group.members.iter().map(|&at| entries[at].id);
                (group.key.clone(), ids.collect())
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("mail".to_owned(), vec![Some(3), Some(1)]),
                ("chat".to_owned(), vec![Some(2)]),
            ]
        );
    }

    #[test]
    fn the_history_file_says_what_it_needs_to() {
        let saved = Saved {
            seen: 20,
            quiet_until: Some(99),
            entries: vec![Entry::of(notification(1, "a", 10), 5)],
        };
        let text = toml::to_string(&saved).unwrap();
        let read: Saved = toml::from_str(&text).unwrap();
        assert_eq!((read.seen, read.quiet_until), (20, Some(99)));
        assert_eq!(read.entries.len(), 1);
        // The key and whether it is held are no matter of the file.
        assert_eq!((read.entries[0].key, read.entries[0].present), (0, false));
        assert_eq!(read.entries[0].title, "Title 1");
        let empty: Saved = toml::from_str("").unwrap();
        assert!(empty.entries.is_empty() && empty.quiet_until.is_none());
    }
}
