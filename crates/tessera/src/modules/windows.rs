// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One button per pinned application and per application with open
//! windows; the windows of one application share a button, and a button
//! before them may open a popup, the way a Start button does.
//! Widget: `windows`.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Context, Global, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    WeakEntity, Window, canvas, div, img, px,
};

use crate::badges::{Badges, Owner};
use crate::config::edit::ConfigFile;
use crate::icons::{AppIcons, Icon};
use crate::menu::{MenuEntry, Menus};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{
    AppWindow, AppWindowId, DisplayKey, JumpTask, RecentDocument, Shortcut, WindowEvents,
    app_id_in, app_id_of_program, app_path,
};
use crate::popup::{PopupAnchor, Popups};
use crate::preview::Previews;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, state_layer};

/// How often it is checked whether something happened to a window.
const CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// How many checks pass before the window list is read again although
/// nothing was reported: every half second when Windows reports nothing at
/// all, and every five seconds otherwise, for what goes unreported.
const CHECKS_UNWATCHED: u32 = 5;
const CHECKS_WATCHED: u32 = 50;

/// How much the icon under the pointer grows, as a share of its size.
const HOVER_GROWTH: f32 = 0.12;

/// How much a pressed icon shrinks, as a share of its size.
const PRESS_SHRINK: f32 = 0.16;

/// How many of the documents an application opened lately its menu
/// offers.
const RECENT_SHOWN: u32 = 5;

/// How large the glyph of the button that opens a popup is, as a part of
/// the icon of an application, and how strong the line after it is.
const START_GLYPH: f32 = 0.8;
const START_LINE: f32 = 0.4;

/// What the button that opens a popup shows when the layout names no icon.
const ICON_START: &str = "\u{e71d}";
const ICON_CLOSE: &str = "\u{e8bb}";
const ICON_DOCUMENT: &str = "\u{e8a5}";
const ICON_TASK: &str = "\u{e945}";
const ICON_NEW_WINDOW: &str = "\u{e78b}";
const ICON_PIN: &str = "\u{e718}";
const ICON_UNPIN: &str = "\u{e77a}";
const ICON_WINDOW: &str = "\u{e7c4}";

/// How many dots a button shows besides the first, one for each further
/// window of its application.
const MORE_DOTS: u8 = 2;

/// The most a badge counts to; beyond it the badge says so with a plus.
const BADGE_MOST: usize = 9;

/// How large a badge is, as a part of the button it sits on.
const BADGE_SIZE: f32 = 0.36;

pub struct WindowsModule;

impl Module for WindowsModule {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("windows", |context, cx| {
            let display = context.display;
            let pins = Pin::find(&context.texts("pinned").unwrap_or_default());
            let start = context.text("start").map(|popup| {
                let glyph = context.text("start-icon").unwrap_or(ICON_START);
                (popup.to_owned(), SharedString::from(glyph.to_owned()))
            });
            let badges = context.flag("badges").unwrap_or(true);
            cx.new(|cx| WindowList::new(display, pins, start, badges, cx))
                .into()
        })
    }
}

/// An application that keeps a button whether it runs or not.
struct Pin {
    /// What a click opens while the application has no window.
    shortcut: Shortcut,
    /// The program file its windows belong to, when that is known.
    program: Option<PathBuf>,
    /// The packaged application its windows belong to, if it is one.
    app: Option<String>,
}

impl Pin {
    /// The applications `names` stand for. A name is the path of a file,
    /// the path that names a packaged application, or what the Start menu
    /// calls an application; one that is none of these is reported and
    /// skipped.
    fn find(names: &[String]) -> Vec<Self> {
        let is_path = |name: &str| {
            let path = Path::new(name);
            path.is_absolute() || app_id_in(path).is_some()
        };
        let by_name = names.iter().any(|name| !is_path(name));
        let start_menu = if by_name {
            Shortcut::start_menu()
        } else {
            Vec::new()
        };
        let pins = names.iter().filter_map(|name| {
            let path = Path::new(name);
            let shortcut = if let Some(app) = app_id_in(path) {
                Some(Shortcut {
                    name: name.clone(),
                    path: app_path(app),
                })
            } else if path.is_absolute() {
                path.is_file().then(|| Shortcut {
                    name: name.clone(),
                    path: path.to_owned(),
                })
            } else {
                let known = |shortcut: &&Shortcut| shortcut.name.eq_ignore_ascii_case(name);
                start_menu.iter().find(known).cloned()
            };
            if shortcut.is_none() {
                report!("nothing to pin is called `{name}`");
            }
            shortcut.map(Self::of)
        });
        pins.collect()
    }

    fn of(shortcut: Shortcut) -> Self {
        let app = app_id_in(&shortcut.path).map(str::to_owned);
        let program = if app.is_some() {
            None
        } else if is_link(&shortcut.path) {
            shortcut.link().target
        } else {
            Some(shortcut.path.clone())
        };
        Self {
            shortcut,
            program,
            app,
        }
    }

    /// Whether `window` belongs to the pinned application.
    fn owns(&self, window: &AppWindow) -> bool {
        if let (Some(app), Some(shown)) = (&self.app, &window.app_id) {
            return app.eq_ignore_ascii_case(shown);
        }
        let (Some(program), Some(executable)) = (&self.program, &window.executable) else {
            return false;
        };
        same_file(program, executable)
    }

    fn icon(&self, icons: &AppIcons) -> Icon {
        icons.of_shortcut(&self.shortcut)
    }
}

/// What tells the application of a window from other applications, the
/// way the taskbar of Windows groups windows: the packaged application,
/// or the identifier the application states for the window, or else its
/// program file. `None` when nothing of it is known.
fn group(
    packaged: Option<&str>,
    stated: Option<&str>,
    executable: Option<&Path>,
) -> Option<String> {
    if let Some(app) = packaged {
        return Some(format!("app:{}", app.to_lowercase()));
    }
    if let Some(id) = stated {
        return Some(format!("id:{}", id.to_lowercase()));
    }
    let executable = executable?.to_string_lossy().to_lowercase();
    Some(format!("program:{executable}"))
}

fn group_of(window: &AppWindow) -> Option<String> {
    let stated = match window.app_id {
        Some(_) => None,
        None => window.id.stated_app_id(),
    };
    group(
        window.app_id.as_deref(),
        stated.as_deref(),
        window.executable.as_deref(),
    )
}

fn is_link(path: &Path) -> bool {
    let extension = path.extension();
    extension.is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
}

/// Whether two paths as Windows writes them name one file: it ignores case
/// and accepts both kinds of slashes.
fn same_file(one: &Path, other: &Path) -> bool {
    let plain = |path: &Path| path.to_string_lossy().replace('/', "\\").to_lowercase();
    plain(one) == plain(other)
}

/// How the configuration file names the program at `path` when it is
/// pinned.
fn pin_name(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// `pins` without those called `name`, whatever their case.
fn without(pins: &[String], name: &str) -> Vec<String> {
    let kept = pins.iter().filter(|pin| !pin.eq_ignore_ascii_case(name));
    kept.cloned().collect()
}

/// `pins` and `name` after them, unless it is among them already.
fn with(pins: &[String], name: &str) -> Vec<String> {
    let mut pins = without(pins, name);
    pins.push(name.to_owned());
    pins
}

/// Makes `change` of the pinned applications in the configuration file,
/// which the shell then takes over. A failure is reported on the error
/// output.
fn repin(cx: &mut App, change: impl FnOnce(&[String]) -> Vec<String>) {
    let changed = ConfigFile::open().and_then(|mut file| {
        let pins = file.config()?.pinned();
        let pins = pins.ok_or("the layout has no `windows` widget to pin to")?;
        if !file.set_pinned(&change(&pins)) {
            return Err("the layout has no `windows` widget to pin to".to_owned());
        }
        file.save()
    });
    match changed {
        Ok(()) => cx.defer(Shell::config_changed),
        Err(error) => report!("{error}"),
    }
}

/// The documents the application of `item` opened lately, each to be
/// opened again by it, and then the tasks it put in its jump list.
fn recent_entries(item: &DockItem) -> Vec<MenuEntry> {
    let window = item.front();
    let pin = item.pin.as_ref();
    let packaged = window
        .and_then(|window| window.app_id.clone())
        .or_else(|| pin.and_then(|pin| pin.app.clone()));
    // A packaged application has no program file to hand a document to.
    let program = match packaged {
        Some(_) => None,
        None => window
            .and_then(|window| window.executable.clone())
            .or_else(|| pin.and_then(|pin| pin.program.clone())),
    };
    let app = packaged
        .or_else(|| window.and_then(|window| window.id.stated_app_id()))
        .or_else(|| program.as_deref().map(app_id_of_program));
    let Some(app) = app else {
        return Vec::new();
    };
    let documents = RecentDocument::of_app(&app, RECENT_SHOWN);
    let documents = documents.into_iter().map(|document| {
        let program = program.clone();
        let label = document.name.clone();
        MenuEntry::new(ICON_DOCUMENT, label, move |_| match &program {
            Some(program) => document.open_with(program),
            None => Shortcut {
                name: document.name.clone(),
                path: document.path.clone(),
            }
            .launch(),
        })
    });
    let tasks = JumpTask::of_app(&app).into_iter().map(|task| {
        let label = task.name.clone();
        MenuEntry::new(ICON_TASK, label, move |_| task.start())
    });
    documents.chain(tasks).collect()
}

/// One button and the motion it is in.
struct DockItem {
    /// Identifies the button for as long as it is shown.
    key: usize,
    /// The pinned application the button stands for. Such a button stays
    /// when its window closes.
    pin: Option<Pin>,
    /// The application of the windows, as [`group_of`] tells it; `None`
    /// for a pinned application and for a window nothing is known of.
    group: Option<String>,
    /// The windows of the application the button stands for, the one used
    /// last first; a button without one is that of a pinned application
    /// that does not run on this display.
    windows: Vec<AppWindow>,
    /// 1 while there is a window, 0 while there is none.
    running: Spring,
    /// How many windows there are besides the first, up to [`MORE_DOTS`].
    more: Spring,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    /// 1 while the window is the foreground window.
    active: Spring,
    interaction: Interaction,
    /// The middle of the button in its surface, once it has been drawn:
    /// where the preview of its window opens.
    middle: Rc<Cell<Option<Pixels>>>,
    /// The window is gone and its button is shrinking away.
    leaving: bool,
    /// How many notifications of the application wait; what it was last
    /// while the badge shrinks away.
    waiting: usize,
    /// 1 while notifications wait, 0 while none does.
    badge: Spring,
    /// Whether the badge is shown or on its way there.
    badged: bool,
}

impl DockItem {
    /// How many notifications `badges` counts for the application of the
    /// button: for the pinned application, or for the application that
    /// has the windows.
    fn waiting(&self, badges: &Badges) -> usize {
        let pinned = self.pin.as_ref().map(|pin| Owner {
            app: pin.app.as_deref(),
            program: pin.program.as_deref(),
        });
        let grouped = self.group.as_deref().and_then(|group| {
            let (kind, name) = group.split_once(':')?;
            Some(match kind {
                "program" => Owner {
                    app: None,
                    program: Some(Path::new(name)),
                },
                _ => Owner {
                    app: Some(name),
                    program: None,
                },
            })
        });
        // The program of a window tells of it where nothing else does.
        let front = self.front().map(|window| Owner {
            app: window.app_id.as_deref(),
            program: window.executable.as_deref(),
        });
        let owners = [pinned, grouped, front];
        let counts = owners.iter().flatten().map(|owner| badges.of(owner));
        counts.max().unwrap_or(0)
    }

    /// The window of the application that was used last.
    fn front(&self) -> Option<&AppWindow> {
        self.windows.first()
    }

    fn holds(&self, id: AppWindowId) -> bool {
        self.windows.iter().any(|window| window.id == id)
    }
}

/// Brings a window of an application forward: its only window, or
/// minimizes that one if it is in front already; of several the one used
/// last, or, when that one is in front, the one used longest ago, so that
/// click after click goes through them all.
fn switch_to(windows: &[AppWindowId]) {
    match windows {
        [] => {}
        [only] => only.activate_or_minimize(),
        [front, .., last] => {
            if AppWindowId::foreground() == Some(*front) {
                last.activate();
            } else {
                front.activate();
            }
        }
    }
}

/// The lists that are shown, for the keys that press their buttons.
#[derive(Default)]
pub struct Docks {
    lists: Vec<WeakEntity<WindowList>>,
}

impl Global for Docks {}

impl Docks {
    /// Does what a click on the button `number` does, counted from 1
    /// among the applications of the list on the display the pointer is
    /// on, or else of the list that was made first.
    pub fn press(number: usize, cx: &mut App) {
        let docks = cx.default_global::<Self>();
        docks.lists.retain(|list| list.upgrade().is_some());
        let lists: Vec<_> = docks.lists.iter().filter_map(WeakEntity::upgrade).collect();
        let pointed = DisplayKey::under_pointer();
        let here = |list: &&gpui::Entity<WindowList>| {
            pointed.is_some() && list.read(cx).display == pointed
        };
        let Some(list) = lists.iter().find(here).or(lists.first()) else {
            return report!("no application has the number {number}: no dock is shown");
        };
        Previews::close(cx);
        Menus::close(cx);
        list.read(cx).press(number);
    }
}

/// Which of the buttons is the one of `number`, counted from 1 among
/// those that are not `leaving`.
fn numbered(leaving: impl Iterator<Item = bool>, number: usize) -> Option<usize> {
    let mut shown = leaving.enumerate().filter(|(_, leaving)| !leaving);
    shown.nth(number.checked_sub(1)?).map(|(index, _)| index)
}

/// The button before the applications, which opens a popup.
struct Start {
    popup: String,
    glyph: SharedString,
    interaction: Interaction,
    /// The middle of the button in its surface, once it has been drawn.
    middle: Rc<Cell<Option<Pixels>>>,
}

/// Lists the pinned applications and the applications with windows on one
/// display, starts the former and switches between the windows of each.
struct WindowList {
    start: Option<Start>,
    /// The display whose windows are listed; every window when unknown.
    display: Option<DisplayKey>,
    /// In display order, including buttons that are still leaving.
    items: Vec<DockItem>,
    foreground: Option<AppWindowId>,
    next_key: usize,
    /// Whether the buttons count the waiting notifications.
    badges: bool,
}

impl WindowList {
    fn new(
        display: Option<DisplayKey>,
        pins: Vec<Pin>,
        start: Option<(String, SharedString)>,
        badges: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        if badges {
            Badges::watch(cx);
            cx.observe_global::<Badges>(Self::refresh).detach();
        }
        let patience = if WindowEvents::watch() {
            CHECKS_WATCHED
        } else {
            report!("Windows does not report window changes, so they are polled");
            CHECKS_UNWATCHED
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut seen = WindowEvents::generation();
            let mut quiet = 0;
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                let now = WindowEvents::generation();
                quiet += 1;
                if now == seen && quiet < patience {
                    if this.upgrade().is_none() {
                        break;
                    }
                    continue;
                }
                (seen, quiet) = (now, 0);
                if this.update(cx, Self::refresh).is_err() {
                    break;
                }
            }
        })
        .detach();

        let this = cx.weak_entity();
        cx.default_global::<Docks>().lists.push(this);
        let motion = cx.global::<Theme>().motion;
        let mut list = Self {
            start: start.map(|(popup, glyph)| Start {
                popup,
                glyph,
                interaction: Interaction::new(&motion),
                middle: Rc::default(),
            }),
            display,
            items: Vec::new(),
            foreground: None,
            next_key: 0,
            badges,
        };
        for pin in pins {
            list.add(Some(pin), None, Vec::new(), cx);
        }
        list.refresh(cx);
        list
    }

    /// Does what a click on the button `number` of the applications
    /// does: starts a pinned one without a window, and goes through the
    /// windows of the others.
    fn press(&self, number: usize) {
        let leaving = self.items.iter().map(|item| item.leaving);
        let Some(item) = numbered(leaving, number).map(|index| &self.items[index]) else {
            return;
        };
        let ids: Vec<AppWindowId> = item.windows.iter().map(|window| window.id).collect();
        match &item.pin {
            Some(pin) if ids.is_empty() => pin.shortcut.launch(),
            _ => switch_to(&ids),
        }
    }

    /// Appends a button, which grows in.
    fn add(
        &mut self,
        pin: Option<Pin>,
        group: Option<String>,
        windows: Vec<AppWindow>,
        cx: &Context<Self>,
    ) {
        let motion = cx.global::<Theme>().motion;
        let mut presence = Spring::new(motion.spatial, 0.);
        presence.animate_to(1.);
        self.items.push(DockItem {
            key: self.next_key,
            pin,
            group,
            windows,
            running: Spring::new(motion.spatial, 0.),
            more: Spring::new(motion.spatial, 0.),
            presence,
            active: Spring::new(motion.spatial, 0.),
            interaction: Interaction::new(&motion),
            middle: Rc::default(),
            leaving: false,
            waiting: 0,
            badge: Spring::new(motion.spatial_fast, 0.),
            badged: false,
        });
        self.next_key += 1;
    }

    /// The pointer came onto the button `key`, or left it: the preview of
    /// its window opens after a moment, or closes.
    fn preview(&self, key: usize, hovered: bool, window: &Window, cx: &mut Context<Self>) {
        let item = self.items.iter().find(|item| item.key == key);
        let target = item
            .filter(|item| !item.leaving)
            .and_then(|item| Some((item.front()?.id, item.middle.get()?)));
        let Some((id, middle)) = target.filter(|_| hovered) else {
            let left = target.filter(|_| !hovered).map(|(id, _)| id);
            Previews::leave(left, cx);
            return;
        };
        match PopupAnchor::at(window, middle, cx) {
            Ok(anchor) => Previews::request(id, anchor, cx),
            Err(error) => report!("a preview has nowhere to open: {error:#}"),
        }
    }

    /// What the menu of the button `key` offers: the documents its
    /// application opened lately, its windows if it has several, another
    /// window of its program, pinning or unpinning it, and closing its
    /// windows.
    fn menu_entries(&self, key: usize) -> Vec<MenuEntry> {
        let shown = |item: &&DockItem| item.key == key && !item.leaving;
        let Some(item) = self.items.iter().find(shown) else {
            return Vec::new();
        };
        let mut entries = recent_entries(item);
        if item.windows.len() > 1 {
            entries.extend(item.windows.iter().map(|window| {
                let id = window.id;
                MenuEntry::new(ICON_WINDOW, id.title(), move |_| id.activate())
            }));
        }
        let executable = item.front().and_then(AppWindow::program);
        let launched = match (&item.pin, &executable) {
            (Some(pin), _) => Some(pin.shortcut.clone()),
            (None, Some(executable)) => Some(Shortcut {
                name: pin_name(executable),
                path: executable.clone(),
            }),
            (None, None) => None,
        };
        if let Some(shortcut) = launched {
            let label = if item.front().is_some() {
                "New window"
            } else {
                "Open"
            };
            entries.push(MenuEntry::new(ICON_NEW_WINDOW, label, move |_| {
                shortcut.launch();
            }));
        }
        match (&item.pin, executable) {
            (Some(pin), _) => {
                let name = pin.shortcut.name.clone();
                entries.push(MenuEntry::new(ICON_UNPIN, "Unpin", move |cx| {
                    repin(cx, |pins| without(pins, &name));
                }));
            }
            (None, Some(executable)) => {
                let name = pin_name(&executable);
                entries.push(MenuEntry::new(ICON_PIN, "Pin to the dock", move |cx| {
                    repin(cx, |pins| with(pins, &name));
                }));
            }
            (None, None) => {}
        }
        let ids: Vec<AppWindowId> = item.windows.iter().map(|window| window.id).collect();
        match ids.as_slice() {
            [] => {}
            &[id] => entries.push(MenuEntry::new(ICON_CLOSE, "Close window", move |_| {
                id.close();
            })),
            _ => entries.push(MenuEntry::new(ICON_CLOSE, "Close all windows", move |_| {
                for id in &ids {
                    id.close();
                }
            })),
        }
        entries
    }

    /// Opens the menu of the button `key` above it.
    fn menu(&self, key: usize, window: &Window, cx: &mut Context<Self>) {
        Previews::close(cx);
        let item = self.items.iter().find(|item| item.key == key);
        let Some(middle) = item.and_then(|item| item.middle.get()) else {
            return;
        };
        match PopupAnchor::at(window, middle, cx) {
            Ok(anchor) => Menus::open(self.menu_entries(key), anchor, cx),
            Err(error) => report!("a menu has nowhere to open: {error:#}"),
        }
    }

    /// Re-reads the window list and repaints only when it changed. Known
    /// applications keep their place, new ones are appended, and those
    /// whose last window closed start leaving. A pinned application takes
    /// its windows; the buttons of pinned applications come first, so they
    /// choose first.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let mut windows = AppWindow::enumerate();
        windows.retain(|window| self.display.is_none_or(|display| window.display == display));
        let mut windows: Vec<(Option<String>, AppWindow)> = windows
            .into_iter()
            .map(|window| (group_of(&window), window))
            .collect();
        let foreground = AppWindowId::foreground();
        let mut changed = foreground != self.foreground;
        self.foreground = foreground;
        let shows = |items: &[DockItem], id| {
            let shown = |item: &DockItem| !item.leaving && item.holds(id);
            items.iter().any(shown)
        };
        let previewed = Previews::shown(cx).filter(|&id| shows(&self.items, id));

        // An application may say what it is only after its window opened:
        // the button of the window then goes on as that application.
        for item in &mut self.items {
            let held = windows.iter().find(|(_, window)| item.holds(window.id));
            if let (None, Some((group, _))) = (&item.pin, held) {
                item.group.clone_from(group);
            }
        }
        for item in &mut self.items {
            let mut mine = Vec::new();
            windows.retain(|(group, window)| {
                let owned = match (&item.pin, &item.group, group) {
                    (Some(pin), _, _) => pin.owns(window) || item.holds(window.id),
                    (None, Some(one), Some(other)) => one == other,
                    (None, _, _) => item.holds(window.id),
                };
                if owned {
                    mine.push(window.clone());
                }
                !owned
            });
            let present = !mine.is_empty();
            // The button of an application that left keeps its windows
            // for the icon it shrinks away with.
            if (present || item.pin.is_some()) && mine != item.windows {
                item.windows = mine;
                changed = true;
            }
            if item.pin.is_none() && item.leaving == present {
                item.leaving = !present;
                item.presence.animate_to(if present { 1. } else { 0. });
                changed = true;
            }
        }
        for (group, window) in windows {
            let same = |item: &&mut DockItem| {
                item.pin.is_none() && !item.leaving && group.is_some() && item.group == group
            };
            match self.items.iter_mut().find(same) {
                Some(item) => item.windows.push(window),
                None => self.add(None, group, vec![window], cx),
            }
            changed = true;
        }
        // A preview does not outlive its window.
        if previewed.is_some_and(|id| !shows(&self.items, id)) {
            Previews::close(cx);
        }
        for item in &mut self.items {
            let is_foreground = !item.leaving && foreground.is_some_and(|id| item.holds(id));
            item.active.animate_to(if is_foreground { 1. } else { 0. });
            let runs = !item.windows.is_empty();
            item.running.animate_to(if runs { 1. } else { 0. });
            let more = item.windows.len().saturating_sub(1);
            let more = u8::try_from(more).unwrap_or(u8::MAX).min(MORE_DOTS);
            item.more.animate_to(f32::from(more));
        }
        if let Some(badges) = cx.try_global::<Badges>().filter(|_| self.badges) {
            for item in &mut self.items {
                let waiting = item.waiting(badges);
                if waiting > 0 && waiting != item.waiting {
                    item.waiting = waiting;
                    changed = true;
                }
                if (waiting > 0) != item.badged {
                    item.badged = waiting > 0;
                    item.badge.animate_to(if item.badged { 1. } else { 0. });
                    changed = true;
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn render_item(
        item: &mut DockItem,
        icon: Icon,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let presence = item.presence.value(window);
        let active = item.active.value(window);
        let running = item.running.value(window);
        let more = item.more.value(window);
        let badge = item.badge.value(window).clamp(0., 1.2);
        let waiting = item.waiting;
        let state = item.interaction.state(window);
        let key = item.key;
        let ids: Vec<AppWindowId> = item.windows.iter().map(|window| window.id).collect();
        let shortcut = item.pin.as_ref().map(|pin| pin.shortcut.clone());
        // What the middle button starts: the pinned application, or the
        // program of the window in front, as `New window` of the menu does.
        let another = shortcut.clone().or_else(|| {
            let executable = item.front().and_then(AppWindow::program)?;
            Some(Shortcut {
                name: pin_name(&executable),
                path: executable,
            })
        });

        let icon_size =
            theme.dock.icon_size * (1. + HOVER_GROWTH * state.hover - PRESS_SHRINK * state.press);
        let background = mix(
            theme.colors.surface_container,
            theme.colors.secondary_container,
            active,
        );
        let button = div()
            .id(("dock-item", key))
            .size(theme.dock.item_size)
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(theme.spacing.tiny)
            .rounded(theme.shape.medium)
            .bg(state_layer(&theme, background, state))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| this.menu(key, window, cx)),
            )
            .on_mouse_down(MouseButton::Middle, move |_, _, cx| {
                Previews::close(cx);
                Menus::close(cx);
                if let Some(another) = &another {
                    another.launch();
                }
            })
            .on_click(move |_, _, cx| {
                Previews::close(cx);
                Menus::close(cx);
                match &shortcut {
                    Some(shortcut) if ids.is_empty() => shortcut.launch(),
                    _ => switch_to(&ids),
                }
            })
            .child({
                let middle = item.middle.clone();
                let measure =
                    move |bounds: gpui::Bounds<Pixels>, _: &mut Window, _: &mut gpui::App| {
                        middle.set(Some(bounds.origin.x + bounds.size.width / 2.));
                    };
                canvas(measure, |_, (), _, _| {}).absolute().size_full()
            })
            .child(match icon {
                Some(icon) => img(icon).size(icon_size).into_any_element(),
                None => div()
                    .size(icon_size)
                    .rounded(theme.shape.small)
                    .bg(theme.colors.outline)
                    .into_any_element(),
            })
            .child(Self::render_indicator(active, running, more, theme))
            .children((badge > 0.01).then(|| Self::render_badge(waiting, badge, theme)));
        let button = Interaction::track_hover(
            button,
            cx,
            move |this: &mut Self| {
                let item = this.items.iter_mut().find(|item| item.key == key)?;
                Some(&mut item.interaction)
            },
            move |this, hovered, window, cx| this.preview(key, hovered, window, cx),
        );

        // The slot includes the spacing between buttons, so that a button
        // that comes or goes moves its neighbors smoothly.
        let slot = theme.dock.item_size + theme.spacing.tiny;
        div()
            .w(slot * presence.max(0.))
            .h(theme.dock.item_size)
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .opacity(presence.clamp(0., 1.))
            .child(button)
            .into_any_element()
    }

    /// The button that opens the popup of `start`, drawn like the buttons
    /// of the applications, and the line that parts it from them when
    /// there are `others`.
    fn render_start(
        start: &mut Start,
        others: bool,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = start.interaction.state(window);
        let size =
            theme.dock.icon_size * (1. + HOVER_GROWTH * state.hover - PRESS_SHRINK * state.press);
        let (popup, middle) = (start.popup.clone(), start.middle.clone());
        let (name, seen) = (popup.clone(), middle.clone());
        let measure = move |bounds: gpui::Bounds<Pixels>, window: &mut Window, cx: &mut App| {
            let x = bounds.origin.x + bounds.size.width / 2.;
            seen.set(Some(x));
            Popups::stand(&name, x, window, cx);
        };
        let button = div()
            .id("dock-start")
            .relative()
            .size(theme.dock.item_size)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.medium)
            .bg(state_layer(&theme, theme.colors.surface_container, state))
            .on_click(move |_, window, cx| {
                Previews::close(cx);
                Menus::close(cx);
                let x = middle.get().unwrap_or(window.mouse_position().x);
                match PopupAnchor::at(window, x, cx) {
                    Ok(anchor) => Popups::toggle(&popup, anchor, cx),
                    Err(error) => report!("the popup `{popup}` has nowhere to open: {error:#}"),
                }
            })
            .child(canvas(measure, |_, (), _, _| {}).absolute().size_full())
            .child(
                icon(&theme, start.glyph.clone())
                    .text_size(size * START_GLYPH)
                    .text_color(theme.colors.primary),
            );
        let button = Interaction::track(button, cx, |this: &mut Self| {
            Some(&mut this.start.as_mut()?.interaction)
        });
        let line = others.then(|| {
            div()
                .w(px(1.))
                .h(theme.dock.item_size / 2.)
                .mx(theme.spacing.tiny)
                .flex_none()
                .bg(theme.colors.outline.opacity(START_LINE))
        });
        div()
            .flex()
            .items_center()
            .flex_none()
            .child(button)
            .children(line)
            .into_any_element()
    }

    /// A dot under every application with a window, which grows into a
    /// pill while one of its windows is in front, and up to [`MORE_DOTS`]
    /// dots beside it, `more` of them, for its further windows. `running`
    /// is 0 under an application without a window, which has no dot.
    fn render_badge(waiting: usize, shown: f32, theme: Theme) -> AnyElement {
        let size = theme.dock.item_size * BADGE_SIZE;
        let text = if waiting > BADGE_MOST {
            format!("{BADGE_MOST}+")
        } else {
            waiting.to_string()
        };
        // The badge grows out of its middle.
        let inset = size * (1. - shown) / 2.;
        div()
            .absolute()
            .top(inset)
            .right(inset)
            .h(size * shown)
            .min_w(size * shown)
            .px(size * shown / 4.)
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .rounded(theme.shape.full)
            .bg(theme.colors.primary)
            .text_color(theme.colors.surface_container)
            .text_size(size * 0.62 * shown.min(1.))
            .font_weight(gpui::FontWeight::BOLD)
            .opacity(shown.min(1.))
            .child(SharedString::from(text))
            .into_any_element()
    }

    fn render_indicator(active: f32, running: f32, more: f32, theme: Theme) -> AnyElement {
        let dot = theme.dock.indicator_height;
        let growth = theme.dock.indicator_active_width - dot;
        let color = mix(theme.colors.outline, theme.colors.primary, active);
        let further = (0..MORE_DOTS).map(|index| {
            let shown = (more - f32::from(index)).clamp(0., 1.);
            div()
                .h(dot)
                .w(dot * shown)
                .ml(dot / 2. * shown)
                .rounded(theme.shape.full)
                .opacity(shown)
                .bg(color)
        });
        div()
            .flex()
            .items_center()
            .opacity(running.clamp(0., 1.))
            .child(
                div()
                    .h(dot)
                    .w(dot + growth * active.max(0.))
                    .rounded(theme.shape.full)
                    .bg(color),
            )
            .children(further)
            .into_any_element()
    }
}

impl Render for WindowList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let items = &mut self.items;
        let elements: Vec<AnyElement> = items
            .iter_mut()
            .map(|item| {
                let icons = cx.global::<AppIcons>();
                let icon = match (&item.pin, item.front()) {
                    (Some(pin), _) => pin.icon(icons),
                    (None, Some(window)) => {
                        let program = window.program();
                        program.and_then(|program| icons.of_executable(&program))
                    }
                    (None, None) => None,
                };
                Self::render_item(item, icon, theme, window, cx)
            })
            .collect();
        // A button that has finished leaving was just drawn at no width.
        items.retain(|item| !(item.leaving && item.presence.is_settled()));

        let start = self
            .start
            .as_mut()
            .map(|start| Self::render_start(start, !elements.is_empty(), theme, window, cx));
        if elements.is_empty() && start.is_none() {
            return div();
        }
        div()
            .h(theme.dock.container_height)
            .px(theme.spacing.small - theme.spacing.tiny / 2.)
            .flex()
            .items_center()
            .rounded(theme.shape.large)
            .bg(theme.colors.surface_container)
            .children(start)
            .children(elements)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_of_one_file_are_told_whatever_their_spelling() {
        let one = Path::new(r"C:\Program Files\App\app.exe");
        assert!(same_file(one, Path::new("c:/program files/APP/App.exe")));
        assert!(!same_file(
            one,
            Path::new(r"C:\Program Files\App\other.exe")
        ));
    }

    #[test]
    fn a_program_is_pinned_once_and_unpinned_whatever_its_case() {
        let name = pin_name(Path::new(r"C:\Apps\app.exe"));
        assert_eq!(name, "C:/Apps/app.exe");
        let pins = with(&["Notepad".to_owned()], &name);
        assert_eq!(pins, ["Notepad", "C:/Apps/app.exe"]);
        assert_eq!(
            with(&pins, "c:/apps/APP.exe"),
            ["Notepad", "c:/apps/APP.exe"]
        );
        assert_eq!(without(&pins, "notepad"), ["C:/Apps/app.exe"]);
    }

    #[test]
    fn windows_are_grouped_by_what_the_application_states_first() {
        let program = Path::new("C:/Programs/Editor.exe");
        assert_eq!(
            group(Some("Vendor.App!Main"), Some("stated"), Some(program)).as_deref(),
            Some("app:vendor.app!main")
        );
        assert_eq!(
            group(None, Some("Vendor.Profile1"), Some(program)).as_deref(),
            Some("id:vendor.profile1")
        );
        assert_eq!(
            group(None, None, Some(program)),
            group(None, None, Some(Path::new("C:/programs/editor.EXE")))
        );
        assert_eq!(group(None, None, None), None);
    }

    #[test]
    fn buttons_are_numbered_without_the_ones_that_leave() {
        let leaving = [false, true, false, false];
        assert_eq!(numbered(leaving.into_iter(), 1), Some(0));
        assert_eq!(numbered(leaving.into_iter(), 2), Some(2));
        assert_eq!(numbered(leaving.into_iter(), 3), Some(3));
        assert_eq!(numbered(leaving.into_iter(), 4), None);
        assert_eq!(numbered(leaving.into_iter(), 0), None);
    }

    #[test]
    fn only_shortcut_files_are_read_as_links() {
        assert!(is_link(Path::new(r"C:\Menu\App.LNK")));
        assert!(!is_link(Path::new(r"C:\Apps\app.exe")));
    }
}
