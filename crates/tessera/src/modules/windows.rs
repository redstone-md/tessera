// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One button per pinned application and per application with open
//! windows; the windows of one application share a button, unless the
//! dock is told to give each its own, and a button before them may open
//! a popup, the way a Start button does. A button shows the progress, the
//! overlay icon and the buttons under its picture that its application
//! gives the taskbar (as the shell of the session), marks a window that
//! asks for attention, and has Windows fly a window that is minimized to
//! it. A dock taller than wide lays its buttons out in a column.
//! Widget: `windows`.

mod entries;
mod keys;
mod marks;
mod options;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Context, Entity, FocusHandle,
    Global, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Pixels,
    Render, ScrollDelta, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, canvas, div, img, px,
};

use self::entries::{StartAnother, is_files};
use self::marks::Edge;
use self::options::{Candidate, DockOptions, Grouping, Layout, Middle, Reveal, Side, SortKey};
use crate::actions::Actions;
use crate::badges::{Badges, Owner};
use crate::config::edit::ConfigFile;
use crate::icons::{AppIcons, Icon, icon_of_pixels, is_link};
use crate::menu::Menus;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{
    AppWindow, AppWindowId, DisplayKey, NativeSurface, Progress, ScreenEdge, ScreenRect,
    ShellEvent, ShellHost, Shortcut, TaskbarList, WindowEvents, app_id_in, app_path, shell_mode,
};
use crate::popup::{PopupAnchor, Popups};
use crate::preview::{PreviewOwner, Previews};
use crate::rules::Rules;
use crate::shell::Shell;
use crate::surface::EdgeSurface;
use crate::theme::Theme;
use crate::ui::{Pick, icon, logo, one_line, state_layer};
use crate::workspaces::Workspaces;

/// How often it is checked whether something happened to a window.
const CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// For how long a window that asks for attention keeps the surfaces that
/// hide shown, unless it stops asking first: told again at each change of
/// the dock, it lasts until the window is activated.
const ATTENTION_REVEAL: Duration = Duration::from_secs(60 * 60);

/// How many checks pass before the window list is read again although
/// nothing was reported: every half second when Windows reports nothing at
/// all, and every five seconds otherwise, for what goes unreported.
const CHECKS_UNWATCHED: u32 = 5;
const CHECKS_WATCHED: u32 = 50;

/// How much the icon under the pointer grows, as a share of its size.
const HOVER_GROWTH: f32 = 0.12;

/// How much a pressed icon shrinks, as a share of its size.
const PRESS_SHRINK: f32 = 0.16;

/// How large the glyph of the button that opens a popup is, as a part of
/// the icon of an application, and the mark of Tessera it shows otherwise, and how strong the line after it is.
const START_GLYPH: f32 = 0.8;
const START_LOGO: f32 = 0.62;
const START_LINE: f32 = 0.4;

/// How many dots a button shows besides the first, one for each further
/// window of its application, where it does not count them.
const MORE_DOTS: u8 = 2;

/// The most a badge counts to; beyond it the badge says so with a plus.
const BADGE_MOST: usize = 9;

/// How large a badge is, as a part of the button it sits on.
const BADGE_SIZE: f32 = 0.36;

/// How wide a button with the title of its window is, as many buttons
/// without one, as the taskbar of Windows has them.
const LABELLED_WIDTH: f32 = 4.;

/// How wide a column has to be, as many buttons, for its buttons to show
/// the titles of their windows.
const LABELLED_COLUMN: f32 = 2.5;

/// How far the wheel turns, in pixels, before it brings the next window.
const WHEEL_STEP: f32 = 40.;

/// The application the windows of the file manager of Tessera belong to.
pub const FILES_APP: &str = "Tessera.Files";

pub struct WindowsModule;

impl Module for WindowsModule {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("windows", |context, cx| {
            let options = DockOptions::of(context);
            let display = context.display;
            let pins = Pin::find(&context.texts("pinned").unwrap_or_default());
            let start = context.text("start").map(|popup| {
                let glyph = context.text("start-icon");
                let glyph = glyph.map(|glyph| SharedString::from(glyph.to_owned()));
                (popup.to_owned(), glyph)
            });
            cx.new(|cx| WindowList::new(display, options, pins, start, cx))
                .into()
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        keys::register(actions)
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
        !window.is_own() && same_file(program, executable)
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

/// The group of the button of `window` where the dock groups as
/// `grouping`: that of its application, or one of its own.
fn button_group(grouping: Grouping, window: &AppWindow) -> Option<String> {
    match grouping {
        Grouping::Always => group_of(window),
        Grouping::Never => Some(format!("window:{}", window.id.raw())),
    }
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

/// What the taskbar was told of the windows of a button, as it is shown.
#[derive(Default)]
struct Told {
    /// The most urgent progress of the windows.
    progress: Option<Progress>,
    /// The overlay icon of the window used last that has one, with the
    /// number of that icon.
    overlay: Option<(u64, Icon)>,
}

/// One button and the motion it is in.
struct DockItem {
    /// Identifies the button for as long as it is shown.
    key: usize,
    /// The pinned application the button stands for. Such a button stays
    /// when its window closes.
    pin: Option<Pin>,
    /// The place of the pinned application among the pins.
    pinned: Option<usize>,
    /// The application of the windows, as [`button_group`] tells it;
    /// `None` for a pinned application and for a window nothing is known
    /// of.
    group: Option<String>,
    /// The windows of the application the button stands for, the one used
    /// last first; a button without one is that of a pinned application
    /// that does not run on this display.
    windows: Vec<AppWindow>,
    /// When the button got its first window, counted up.
    opened: Option<u64>,
    /// What the button is called: the title of the window in front, or
    /// the name of the pinned application.
    title: SharedString,
    /// 1 while there is a window, 0 while there is none.
    running: Spring,
    /// How many windows there are besides the first, up to [`MORE_DOTS`].
    more: Spring,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    /// 1 while the window is the foreground window.
    active: Spring,
    interaction: Interaction,
    /// Where the button is in its surface, once it has been drawn: where
    /// its preview opens and where its windows minimize to.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The window is gone and its button is shrinking away.
    leaving: bool,
    /// How many notifications of the application wait; what it was last
    /// while the badge shrinks away.
    waiting: usize,
    /// 1 while notifications wait, 0 while none does.
    badge: Spring,
    /// Whether the badge is shown or on its way there.
    badged: bool,
    /// 1 while a window of the button asks for attention.
    attention: Spring,
    /// Whether one does.
    asking: bool,
    told: Told,
    /// How full the progress is shown, following what is told.
    progress: Spring,
    /// Where Windows was told the windows minimize to, and which windows.
    minimizes_to: Option<(ScreenRect, Vec<AppWindowId>)>,
    /// How far the wheel turned that has not brought a window yet.
    wheel: f32,
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
                "window" => return None,
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

    fn ids(&self) -> Vec<AppWindowId> {
        self.windows.iter().map(|window| window.id).collect()
    }

    /// Whether the button stands for the file manager of Tessera.
    fn is_files(&self) -> bool {
        self.front().is_some_and(is_files)
    }

    /// What the button is ordered by.
    fn sort_key(&self) -> SortKey {
        let name = match (&self.pin, self.front()) {
            (Some(pin), _) => pin.shortcut.name.clone(),
            (None, Some(window)) => match (&window.app_id, &window.executable) {
                (Some(app), _) => app.clone(),
                (None, Some(executable)) => executable
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                (None, None) => self.title.to_string(),
            },
            (None, None) => String::new(),
        };
        SortKey {
            pinned: self.pinned,
            opened: self.opened,
            name: name.to_lowercase(),
        }
    }

    /// Whose media a preview of the button offers to control.
    fn preview_owner(&self) -> PreviewOwner {
        let front = self.front();
        let app = { front.and_then(|window| window.app_id.clone()) }
            .or_else(|| self.pin.as_ref().and_then(|pin| pin.app.clone()));
        let program = { front.and_then(|window| window.executable.clone()) }
            .or_else(|| self.pin.as_ref().and_then(|pin| pin.program.clone()));
        PreviewOwner { app, program }
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

/// What a key of the dock does to the button of its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Deed {
    /// What a click does.
    Press,
    /// Starts another window.
    New,
    /// Brings the next of its windows forward.
    Cycle,
    /// Opens its menu.
    Menu,
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
        Self::act(number, Deed::Press, cx);
    }

    /// The list on the display the pointer is on, or else the one that
    /// was made first.
    fn chosen(cx: &mut App) -> Option<Entity<WindowList>> {
        let docks = cx.default_global::<Self>();
        docks.lists.retain(|list| list.upgrade().is_some());
        let lists: Vec<_> = docks.lists.iter().filter_map(WeakEntity::upgrade).collect();
        let pointed = DisplayKey::under_pointer();
        let here =
            |list: &&Entity<WindowList>| pointed.is_some() && list.read(cx).display == pointed;
        lists.iter().find(here).or(lists.first()).cloned()
    }

    /// Does `deed` to the button `number`, counted from 1, of the chosen
    /// list.
    fn act(number: usize, deed: Deed, cx: &mut App) {
        let Some(list) = Self::chosen(cx) else {
            return report!("no application has the number {number}: no dock is shown");
        };
        Previews::close(cx);
        Menus::close(cx);
        let key = list.read(cx).numbered(number);
        let Some(key) = key else {
            return;
        };
        let surface = list.read(cx).surface;
        match deed {
            Deed::Press => list.update(cx, |list, cx| list.press(key, cx)),
            Deed::New => list.update(cx, |list, cx| list.start_another(key, cx)),
            Deed::Cycle => list.update(cx, |list, _| list.cycle(key, true)),
            Deed::Menu => {
                let Some(surface) = surface else {
                    return;
                };
                surface
                    .update(cx, |_, window, cx| {
                        list.update(cx, |list, cx| list.menu(key, window, cx));
                    })
                    .ok();
            }
        }
    }

    /// Puts the keyboard on the chosen list, as Win+T does.
    fn focus(cx: &mut App) {
        let Some(list) = Self::chosen(cx) else {
            return report!("no dock is shown to take the keyboard");
        };
        Previews::close(cx);
        Menus::close(cx);
        let Some(surface) = list.read(cx).surface else {
            return;
        };
        let before = AppWindowId::foreground();
        surface
            .update(cx, |_, window, cx| {
                if let Some(id) = AppWindowId::of(window) {
                    id.activate();
                }
                list.update(cx, |list, cx| list.take_keyboard(before, window, cx));
            })
            .ok();
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
    /// The character it shows; the mark of Tessera without one.
    glyph: Option<SharedString>,
    interaction: Interaction,
    /// Where the button is in its surface, once it has been drawn.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// How a button is drawn, the same for every button of a list.
#[derive(Clone, Copy)]
struct Look {
    theme: Theme,
    layout: Layout,
    edge: Edge,
    /// Whether the buttons show the titles of their windows, and how wide
    /// they are then.
    labelled: Option<Pixels>,
    counter: bool,
    middle: Middle,
    wheel: bool,
    /// How far the segments of progress and the pulses of attention are.
    travel: f32,
    pulse: f32,
}

/// Lists the pinned applications and the applications with windows on one
/// display, starts the former and switches between the windows of each.
struct WindowList {
    start: Option<Start>,
    /// The display whose windows are listed; every window when unknown.
    display: Option<DisplayKey>,
    options: DockOptions,
    /// In display order, including buttons that are still leaving.
    items: Vec<DockItem>,
    foreground: Option<AppWindowId>,
    next_key: usize,
    /// Counts the buttons that got their first window.
    next_opened: u64,
    /// The windows that ask for attention, each with whether it flashes
    /// still.
    attention: HashMap<AppWindowId, bool>,
    /// Whether Tessera is the shell of the session: only then does it
    /// hear the taskbar calls of applications and answer where windows
    /// minimize to.
    shell: bool,
    /// How the buttons were laid out when they were last drawn.
    layout: Layout,
    /// The window of the surface the list is drawn in.
    surface: Option<AnyWindowHandle>,
    focus: FocusHandle,
    /// The button the keyboard is on, while the dock has it.
    keyboard: Option<usize>,
    /// The window that had the keyboard before the dock took it.
    given_back: Option<AppWindowId>,
    blur: Option<Subscription>,
    /// When the list was made, from which the motion of marks counts.
    born: Instant,
    /// Whether the list has the surfaces that hide on its display shown,
    /// for a window that asks for attention.
    revealing: bool,
}

impl WindowList {
    fn new(
        display: Option<DisplayKey>,
        options: DockOptions,
        pins: Vec<Pin>,
        start: Option<(String, Option<SharedString>)>,
        cx: &mut Context<Self>,
    ) -> Self {
        if options.badges {
            Badges::watch(cx);
            cx.observe_global::<Badges>(Self::refresh).detach();
        }
        let shell = shell_mode();
        TaskbarList::start(shell);
        let patience = if WindowEvents::watch() {
            CHECKS_WATCHED
        } else {
            report!("Windows does not report window changes, so they are polled");
            CHECKS_UNWATCHED
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            // Which windows there are changes with what happens to them
            // and with the buttons applications add and remove.
            let seen_now = || (WindowEvents::generation(), TaskbarList::tabs());
            let mut seen = seen_now();
            let mut told = TaskbarList::generation();
            let mut quiet = 0;
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                let (now, told_now) = (seen_now(), TaskbarList::generation());
                quiet += 1;
                if now == seen && quiet < patience {
                    // An application that reports its progress tells
                    // often: only what it told is read again then.
                    let retold = told_now != told;
                    told = told_now;
                    let kept = match retold {
                        true => this.update(cx, Self::retell).is_ok(),
                        false => this.upgrade().is_some(),
                    };
                    if !kept {
                        break;
                    }
                    continue;
                }
                (seen, told, quiet) = (now, told_now, 0);
                if this.update(cx, Self::refresh).is_err() {
                    break;
                }
            }
        })
        .detach();
        let events = ShellHost::events();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(event) = events.recv().await {
                let heard = this.update(cx, |this, cx| this.hear(event, cx));
                if heard.is_err() {
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
                bounds: Rc::default(),
            }),
            display,
            options,
            items: Vec::new(),
            foreground: None,
            next_key: 0,
            next_opened: 0,
            attention: HashMap::new(),
            shell,
            layout: Layout::Row,
            surface: None,
            focus: cx.focus_handle(),
            keyboard: None,
            given_back: None,
            blur: None,
            born: Instant::now(),
            revealing: false,
        };
        for (index, pin) in pins.into_iter().enumerate() {
            list.add(Some(pin), Some(index), None, Vec::new(), cx);
        }
        list.refresh(cx);
        list
    }

    /// Takes in what the shell hook tells: a window that flashes asks for
    /// attention until it is activated.
    fn hear(&mut self, event: ShellEvent, cx: &mut Context<Self>) {
        self.hear_event(event, cx);
        self.reveal_for_attention(cx);
    }

    /// Has the surfaces that hide on the display of the list shown while
    /// a window of the list asks for attention as `reveal_on_attention`
    /// says, and hide again as they do once none does.
    fn reveal_for_attention(&mut self, cx: &mut Context<Self>) {
        let Some(display) = self.display else {
            return;
        };
        let wanted = self.wants_reveal();
        if wanted {
            // Told again at each change, so that it lasts until then.
            EdgeSurface::reveal(display, ATTENTION_REVEAL, cx);
        } else if self.revealing {
            EdgeSurface::reveal(display, Duration::ZERO, cx);
        }
        self.revealing = wanted;
    }

    fn hear_event(&mut self, event: ShellEvent, cx: &mut Context<Self>) {
        match event {
            ShellEvent::Flash(id) => {
                if self.attention.insert(id, true) != Some(true) {
                    self.refresh(cx);
                }
            }
            // A window that stopped flashing is redrawn without the flag.
            ShellEvent::Redraw(id) => {
                if let Some(flashing) = self.attention.get_mut(&id).filter(|flashing| **flashing) {
                    *flashing = false;
                    cx.notify();
                }
            }
            ShellEvent::Activated { window, .. } | ShellEvent::Destroyed(window)
                if self.attention.remove(&window).is_some() =>
            {
                self.refresh(cx);
            }
            _ => {}
        }
    }

    /// Whether a window of the list asks for attention in the way
    /// `reveal_on_attention` wants the dock shown for. Only the windows
    /// this dock shows count: every flashing window of the session is
    /// heard, those of other displays and workspaces too.
    fn wants_reveal(&self) -> bool {
        let wanted = |flashing: bool| match self.options.reveal {
            Reveal::Never => false,
            Reveal::WhileFlashing => flashing,
            Reveal::UntilActivated => true,
        };
        { self.items.iter() }
            .filter(|item| !item.leaving)
            .flat_map(|item| &item.windows)
            .filter_map(|window| self.attention.get(&window.id))
            .any(|&flashing| wanted(flashing))
    }

    /// The key of the button `number`, counted from 1 among those that
    /// are shown.
    fn numbered(&self, number: usize) -> Option<usize> {
        let leaving = self.items.iter().map(|item| item.leaving);
        numbered(leaving, number).map(|index| self.items[index].key)
    }

    fn item(&self, key: usize) -> Option<&DockItem> {
        let shown = |item: &&DockItem| item.key == key && !item.leaving;
        self.items.iter().find(shown)
    }

    /// Does what a click on the button `key` does: starts a pinned
    /// application without a window, and goes through the windows of the
    /// others.
    fn press(&self, key: usize, cx: &mut App) {
        let Some(item) = self.item(key) else {
            return;
        };
        let ids = item.ids();
        match &item.pin {
            Some(pin) if ids.is_empty() => pin.shortcut.launch(),
            _ if ids.is_empty() => StartAnother::of(item).start(cx),
            _ => switch_to(&ids),
        }
    }

    /// Starts another window of the application of the button `key`.
    fn start_another(&self, key: usize, cx: &mut App) {
        if let Some(item) = self.item(key) {
            StartAnother::of(item).start(cx);
        }
    }

    /// Brings the next of the windows of the button `key` forward, or the
    /// one before.
    fn cycle(&mut self, key: usize, forward: bool) {
        let Some(item) = self.item(key) else {
            return;
        };
        let ids = item.ids();
        let front = AppWindowId::foreground();
        if let Some(index) = options::cycled(&ids, front.as_ref(), forward) {
            ids[index].activate();
        }
    }

    /// The wheel turned over the button `key`.
    fn wheel(&mut self, key: usize, event: &ScrollWheelEvent) {
        let turned = match event.delta {
            ScrollDelta::Pixels(delta) => delta.y / px(1.),
            ScrollDelta::Lines(delta) => delta.y * WHEEL_STEP,
        };
        let Some(item) = self.items.iter_mut().find(|item| item.key == key) else {
            return;
        };
        item.wheel += turned;
        if item.wheel.abs() < WHEEL_STEP {
            return;
        }
        // Turned toward the user goes on to the windows used longer ago.
        let forward = item.wheel < 0.;
        item.wheel = 0.;
        self.cycle(key, forward);
    }

    /// Appends a button, which grows in.
    fn add(
        &mut self,
        pin: Option<Pin>,
        pinned: Option<usize>,
        group: Option<String>,
        windows: Vec<AppWindow>,
        cx: &Context<Self>,
    ) {
        let motion = cx.global::<Theme>().motion;
        let mut presence = Spring::new(motion.spatial, 0.);
        presence.animate_to(1.);
        let title = pin.as_ref().map(|pin| pin.shortcut.name.clone());
        self.items.push(DockItem {
            key: self.next_key,
            pin,
            pinned,
            group,
            windows,
            opened: None,
            title: title.unwrap_or_default().into(),
            running: Spring::new(motion.spatial, 0.),
            more: Spring::new(motion.spatial, 0.),
            presence,
            active: Spring::new(motion.spatial, 0.),
            interaction: Interaction::new(&motion),
            bounds: Rc::default(),
            leaving: false,
            waiting: 0,
            badge: Spring::new(motion.spatial_fast, 0.),
            badged: false,
            attention: Spring::new(motion.spatial, 0.),
            asking: false,
            told: Told::default(),
            progress: Spring::new(motion.spatial, 0.),
            minimizes_to: None,
            wheel: 0.,
        });
        self.next_key += 1;
    }

    /// Where a preview or a menu of the button `key` opens: over or under
    /// it in a row, beside the dock in a column.
    fn anchor(&self, key: usize, window: &Window, cx: &App) -> Option<PopupAnchor> {
        let bounds = self.item(key)?.bounds.get()?;
        self.anchor_at(bounds, window, cx)
    }

    /// Where what opens for the button at `bounds` opens: away from the
    /// edge of the dock's surface, centered on the button along it.
    fn anchor_at(&self, bounds: Bounds<Pixels>, window: &Window, cx: &App) -> Option<PopupAnchor> {
        match PopupAnchor::at_point(window, bounds.center(), cx) {
            Ok(anchor) => Some(anchor),
            Err(error) => {
                report!("a preview or a menu of the dock has nowhere to open: {error:#}");
                None
            }
        }
    }

    /// The pointer came onto the button `key`, or left it: the previews of
    /// its windows open after a moment, or close.
    fn preview(&self, key: usize, hovered: bool, window: &Window, cx: &mut Context<Self>) {
        let item = self.item(key).filter(|item| !item.windows.is_empty());
        let Some(item) = item.filter(|_| hovered) else {
            let left = item.and_then(DockItem::front).map(|window| window.id);
            Previews::leave(left, cx);
            return;
        };
        let (ids, owner) = (item.ids(), item.preview_owner());
        // The jump list is read while the pointer rests, so that the menu
        // has it when it opens.
        if let Some((app, _)) = entries::jump_app(item) {
            crate::jump_menu::prefetch(&app, cx);
        }
        if let Some(anchor) = self.anchor(key, window, cx) {
            Previews::request_group(ids, owner, anchor, cx);
        }
    }

    /// Opens the menu of the button `key` beside it.
    fn menu(&self, key: usize, window: &Window, cx: &mut Context<Self>) {
        Previews::close(cx);
        let Some(item) = self.item(key) else {
            return;
        };
        let entries = entries::entries(item, cx);
        if let Some(anchor) = self.anchor(key, window, cx) {
            Menus::open(entries, anchor, cx);
        }
    }

    /// Puts the keyboard on the first button, until it leaves the dock.
    fn take_keyboard(
        &mut self,
        before: Option<AppWindowId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let own = AppWindowId::of(window);
        self.given_back = before.filter(|before| Some(*before) != own);
        self.keyboard = self.numbered(1);
        window.focus(&self.focus);
        if self.blur.is_none() {
            let focus = self.focus.clone();
            self.blur = Some(cx.on_blur(&focus, window, |this, _, cx| {
                this.keyboard = None;
                cx.notify();
            }));
        }
        cx.notify();
    }

    /// Gives the keyboard back to the window that had it.
    fn give_keyboard_back(&mut self, cx: &mut Context<Self>) {
        self.keyboard = None;
        if let Some(window) = self.given_back.take() {
            window.activate();
        }
        cx.notify();
    }

    /// Answers a key while the dock has the keyboard.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.keyboard else {
            return;
        };
        let shown: Vec<usize> = { self.items.iter() }
            .filter(|item| !item.leaving)
            .map(|item| item.key)
            .collect();
        let index = shown.iter().position(|&key| key == at).unwrap_or(0);
        let held = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        let menu = key == "menu" || (key == "f10" && held.shift);
        let pick = match key {
            "left" => Some(Pick::Move(-1)),
            "right" => Some(Pick::Move(1)),
            _ => Pick::of(event),
        };
        if menu {
            self.menu(at, window, cx);
            return;
        }
        match pick {
            Some(Pick::Take) => {
                self.press(at, cx);
                self.keyboard = None;
            }
            Some(Pick::Leave) => return self.give_keyboard_back(cx),
            Some(Pick::Letter(letter)) => {
                let number = letter.to_digit(10).map(|digit| digit as usize);
                let key = number.and_then(|number| self.numbered(number));
                if let Some(key) = key {
                    self.press(key, cx);
                    self.keyboard = None;
                }
            }
            Some(pick) => {
                self.keyboard = shown
                    .get(options::stepped(index, shown.len(), pick))
                    .copied()
            }
            None => return,
        }
        cx.notify();
    }

    /// Re-reads the window list and repaints only when it changed. Known
    /// applications keep their place, new ones are appended, and those
    /// whose last window closed start leaving. A pinned application takes
    /// its windows; the buttons of pinned applications come first, so they
    /// choose first. The buttons are then put in the order the dock is
    /// sorted by.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let windows = self.shown_windows(cx);
        let grouping = self.options.group;
        let mut windows: Vec<(Option<String>, AppWindow)> = windows
            .into_iter()
            .map(|window| (button_group(grouping, &window), window))
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
            // Where each window has a button of its own, a pinned
            // application has one window: the one it had, while it lasts.
            let single = grouping == Grouping::Never;
            let kept = { item.windows.iter().map(|window| window.id) }
                .find(|&id| windows.iter().any(|(_, window)| window.id == id))
                .filter(|_| single && item.pin.is_some());
            let mut mine = Vec::new();
            windows.retain(|(group, window)| {
                let owned = match (&item.pin, &item.group, group) {
                    (Some(_), _, _) if kept.is_some() => kept == Some(window.id),
                    (Some(pin), _, _) => item.holds(window.id) || pin.owns(window),
                    (None, Some(one), Some(other)) => one == other,
                    (None, _, _) => item.holds(window.id),
                };
                let owned = owned && !(single && !mine.is_empty());
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
                None => self.add(None, None, group, vec![window], cx),
            }
            changed = true;
        }
        // A preview does not outlive its window.
        if previewed.is_some_and(|id| !shows(&self.items, id)) {
            Previews::close(cx);
        }
        // A window in front asks for nothing any more.
        self.attention
            .retain(|id, _| id.exists() && Some(*id) != foreground);
        for item in &mut self.items {
            changed |= Self::follow(item, foreground, &self.attention, &mut self.next_opened);
        }
        if self.shell {
            for item in &mut self.items {
                changed |= Self::take_told(item);
            }
        }
        if let Some(badges) = cx.try_global::<Badges>().filter(|_| self.options.badges) {
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
        changed |= self.sort();
        if changed {
            cx.notify();
            self.reveal_for_attention(cx);
        }
    }

    /// Takes in only what the applications told the taskbar, for the
    /// windows the buttons already have.
    fn retell(&mut self, cx: &mut Context<Self>) {
        if !self.shell {
            return;
        }
        let mut changed = false;
        for item in &mut self.items {
            changed |= Self::take_told(item);
        }
        if changed {
            cx.notify();
        }
    }

    /// The windows the dock shows buttons for, as its options and the
    /// rules say.
    fn shown_windows(&self, cx: &App) -> Vec<AppWindow> {
        let show = self.options.show;
        let workspace: Option<Vec<AppWindowId>> = match show {
            options::Showing::Workspace => cx.try_global::<Workspaces>().and_then(|_| {
                let residents = Workspaces::residents(self.display, cx)?;
                let current = Workspaces::now(self.display, cx).current;
                let shown = residents.into_iter().nth(current)?;
                Some(shown.into_iter().map(|resident| resident.id).collect())
            }),
            _ => None,
        };
        let mut windows = AppWindow::enumerate();
        windows.retain(|window| {
            let removed =
                self.shell && TaskbarList::read(window.id, |told| told.removed).unwrap_or(false);
            let candidate = Candidate {
                here: self.display.is_none_or(|display| window.display == display),
                minimized: show == options::Showing::Minimized && window.id.is_minimized(),
                in_workspace: workspace.as_ref().map(|ids| ids.contains(&window.id)),
                skipped: Rules::of(window, cx).skip_dock == Some(true),
                removed,
            };
            options::is_shown(show, candidate)
        });
        windows
    }

    /// Moves the marks of `item` toward what its windows are now, and
    /// says whether it changed.
    fn follow(
        item: &mut DockItem,
        foreground: Option<AppWindowId>,
        attention: &HashMap<AppWindowId, bool>,
        next_opened: &mut u64,
    ) -> bool {
        let mut changed = false;
        let runs = !item.windows.is_empty();
        if runs && item.opened.is_none() {
            item.opened = Some(*next_opened);
            *next_opened += 1;
        } else if !runs && item.pin.is_some() {
            item.opened = None;
        }
        let is_foreground = !item.leaving && foreground.is_some_and(|id| item.holds(id));
        item.active.animate_to(if is_foreground { 1. } else { 0. });
        item.running.animate_to(if runs { 1. } else { 0. });
        let more = item.windows.len().saturating_sub(1);
        let more = u8::try_from(more).unwrap_or(u8::MAX).min(MORE_DOTS);
        item.more.animate_to(f32::from(more));
        let title = match (item.front(), &item.pin) {
            (Some(window), _) => window.id.title(),
            (None, Some(pin)) => pin.shortcut.name.clone(),
            (None, None) => String::new(),
        };
        if item.title != title {
            item.title = title.into();
            changed = true;
        }
        let asking = !item.leaving && item.windows.iter().any(|w| attention.contains_key(&w.id));
        if asking != item.asking {
            item.asking = asking;
            item.attention.animate_to(if asking { 1. } else { 0. });
            changed = true;
        }
        changed
    }

    /// Takes in what the applications of the windows of `item` told the
    /// taskbar, and says whether it changed.
    fn take_told(item: &mut DockItem) -> bool {
        let told: Vec<_> = { item.windows.iter() }
            .filter_map(|window| {
                TaskbarList::read(window.id, |told| (told.progress, told.overlay.clone()))
            })
            .collect();
        let progress = { told.iter().filter_map(|(progress, _)| *progress) }
            .max_by_key(|progress| options::urgency(progress.state));
        let overlay = told.into_iter().find_map(|(_, overlay)| overlay);
        let mut changed = false;
        if progress != item.told.progress {
            item.told.progress = progress;
            item.progress
                .animate_to(progress.map_or(0., |progress| progress.value));
            changed = true;
        }
        let stamp = overlay.as_ref().map(|overlay| overlay.stamp);
        if stamp != item.told.overlay.as_ref().map(|(stamp, _)| *stamp) {
            item.told.overlay =
                overlay.map(|overlay| (overlay.stamp, icon_of_pixels(&overlay.pixels)));
            changed = true;
        }
        changed
    }

    /// Puts the buttons in the order the dock is sorted by, and says
    /// whether it changed.
    fn sort(&mut self) -> bool {
        let keys: Vec<SortKey> = self.items.iter().map(DockItem::sort_key).collect();
        let order = options::order(&keys, self.options.sort);
        if order
            .iter()
            .enumerate()
            .all(|(place, &index)| place == index)
        {
            return false;
        }
        let mut items: Vec<Option<DockItem>> = self.items.drain(..).map(Some).collect();
        self.items = order
            .into_iter()
            .filter_map(|index| items[index].take())
            .collect();
        true
    }

    /// Tells Windows where the windows of each button minimize to: the
    /// button, on the screen. `frame` is where the surface is, `scale` its
    /// physical pixels per logical one. Windows keeps one place for each
    /// window, so a dock that shows the windows of every display answers
    /// only for those of its own, and each window minimizes to the dock
    /// beside it.
    fn tell_minimize_targets(&mut self, frame: ScreenRect, scale: f32) {
        let physical = |length: Pixels| (length / px(1.) * scale).round() as i32;
        let display = self.display;
        for item in &mut self.items {
            let target = item.bounds.get().filter(|_| !item.leaving).map(|bounds| {
                let left = frame.left + physical(bounds.origin.x);
                let top = frame.top + physical(bounds.origin.y);
                ScreenRect {
                    left,
                    top,
                    right: left + physical(bounds.size.width),
                    bottom: top + physical(bounds.size.height),
                }
            });
            let here: Vec<AppWindowId> = { item.windows.iter() }
                .filter(|window| display.is_none_or(|display| window.display == display))
                .map(|window| window.id)
                .collect();
            let wanted = target.map(|rect| (rect, here));
            if wanted == item.minimizes_to {
                continue;
            }
            if let Some((_, before)) = &item.minimizes_to {
                let now = wanted
                    .as_ref()
                    .map(|(_, ids)| ids.as_slice())
                    .unwrap_or(&[]);
                // A window that went to another display is left to the
                // dock there, which may have answered for it already.
                let moved = |id: &AppWindowId| target.is_some() && item.holds(*id);
                for id in before.iter().filter(|id| !now.contains(id) && !moved(id)) {
                    ShellHost::set_min_rect(*id, None);
                }
            }
            if let Some((rect, ids)) = &wanted {
                for id in ids {
                    ShellHost::set_min_rect(*id, Some(*rect));
                }
            }
            item.minimizes_to = wanted;
        }
    }

    fn render_item(
        item: &mut DockItem,
        icon: Icon,
        look: Look,
        focused: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = look.theme;
        let presence = item.presence.value(window);
        let active = item.active.value(window);
        let running = item.running.value(window);
        let more = item.more.value(window);
        let badge = item.badge.value(window).clamp(0., 1.2);
        let attention = item.attention.value(window);
        let filled = item.progress.value(window);
        let waiting = item.waiting;
        let state = item.interaction.state(window);
        let key = item.key;
        let ids = item.ids();
        let shortcut = item.pin.as_ref().map(|pin| pin.shortcut.clone());
        // What the middle button starts: the pinned application, or the
        // program of the window in front, as `New window` of the menu does.
        let another = StartAnother::of(item);

        let icon_size =
            theme.dock.icon_size * (1. + HOVER_GROWTH * state.hover - PRESS_SHRINK * state.press);
        let background = mix(
            theme.colors.surface_container,
            theme.colors.secondary_container,
            active,
        );
        let background = marks::attention_background(&theme, background, attention, look.pulse);
        let width = look.labelled.unwrap_or(theme.dock.item_size);
        let length = match look.layout {
            Layout::Row => width,
            Layout::Column { .. } => theme.dock.item_size,
        };
        let closing = ids.clone();
        let button = div()
            .id(("dock-item", key))
            .relative()
            .w(width)
            .h(theme.dock.item_size)
            .flex_none()
            .flex()
            .items_center()
            .rounded(theme.shape.medium)
            .bg(state_layer(&theme, background, state))
            .focus_ring(focused, theme)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| this.menu(key, window, cx)),
            )
            .on_mouse_down(MouseButton::Middle, move |_, _, cx| {
                Previews::close(cx);
                Menus::close(cx);
                match look.middle {
                    Middle::New => another.start(cx),
                    Middle::Close => {
                        for id in &closing {
                            id.close();
                        }
                    }
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                Previews::close(cx);
                Menus::close(cx);
                match &shortcut {
                    Some(shortcut) if ids.is_empty() => shortcut.launch(),
                    _ => switch_to(&ids),
                }
                this.keyboard = None;
            }))
            .child({
                let bounds = item.bounds.clone();
                let measure = move |seen: Bounds<Pixels>, _: &mut Window, _: &mut App| {
                    bounds.set(Some(seen));
                };
                canvas(measure, |_, (), _, _| {}).absolute().size_full()
            });
        let button = match look.wheel {
            true => button.on_scroll_wheel(cx.listener(move |this, event, _, _| {
                this.wheel(key, event);
            })),
            false => button,
        };
        let shown_icon = match icon {
            Some(icon) => img(icon).size(icon_size).flex_none().into_any_element(),
            None => div()
                .size(icon_size)
                .flex_none()
                .rounded(theme.shape.small)
                .bg(theme.colors.outline)
                .into_any_element(),
        };
        let button = match look.labelled {
            Some(_) => button
                .px(theme.spacing.small)
                .gap(theme.spacing.small)
                .child(shown_icon)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(theme.typography.label_size)
                        .text_color(theme.colors.on_surface)
                        .child(one_line(item.title.clone())),
                ),
            None => button.justify_center().child(shown_icon),
        };
        let count = look.counter.then_some(item.windows.len());
        let dots = if look.counter { 0 } else { MORE_DOTS };
        let mut button = button.child(marks::indicator(
            &theme, look.edge, active, running, more, dots, count,
        ));
        if let Some(progress) = item.told.progress {
            let size = match look.layout {
                Layout::Row => width,
                Layout::Column { .. } => theme.dock.item_size,
            };
            button = button.child(marks::progress(
                &theme,
                look.edge,
                size,
                progress.state,
                filled,
                look.travel,
            ));
        }
        if attention > 0.01 {
            button = button.child(marks::attention_dot(&theme, look.edge, attention));
        }
        // An overlay the application gives takes the place of the badge.
        let overlay = item
            .told
            .overlay
            .as_ref()
            .and_then(|(_, icon)| icon.clone());
        let button = match marks::overlay(&theme, overlay) {
            Some(overlay) => button.child(overlay),
            None => {
                button.children((badge > 0.01).then(|| Self::render_badge(waiting, badge, theme)))
            }
        };
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
        let slot = length + theme.spacing.tiny;
        let slot_div = div()
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .opacity(presence.clamp(0., 1.));
        match look.layout {
            Layout::Row => slot_div.w(slot * presence.max(0.)).h(theme.dock.item_size),
            Layout::Column { .. } => slot_div.h(slot * presence.max(0.)).w(width),
        }
        .child(button)
        .into_any_element()
    }

    /// The button that opens the popup of `start`, drawn like the buttons
    /// of the applications, and the line that parts it from them when
    /// there are `others`. A click with the right button opens the menu
    /// of Win+X.
    fn render_start(
        &mut self,
        others: bool,
        look: Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = look.theme;
        let layout = self.layout;
        let start = self.start.as_mut()?;
        let state = start.interaction.state(window);
        let size =
            theme.dock.icon_size * (1. + HOVER_GROWTH * state.hover - PRESS_SHRINK * state.press);
        let (popup, bounds) = (start.popup.clone(), start.bounds.clone());
        let shown = match &start.glyph {
            Some(glyph) => icon(&theme, glyph.clone())
                .text_size(size * START_GLYPH)
                .text_color(theme.colors.primary),
            None => logo(size * START_LOGO, theme.colors.primary),
        };
        let (name, seen) = (popup.clone(), bounds.clone());
        let measure = move |measured: Bounds<Pixels>, window: &mut Window, cx: &mut App| {
            seen.set(Some(measured));
            let x = measured.origin.x + measured.size.width / 2.;
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
            .on_click(cx.listener(move |this, _, window, cx| {
                Previews::close(cx);
                Menus::close(cx);
                let measured = bounds.get();
                let anchor = measured.and_then(|measured| this.anchor_at(measured, window, cx));
                let anchor =
                    anchor.or_else(|| PopupAnchor::at(window, window.mouse_position().x, cx).ok());
                match anchor {
                    Some(anchor) => Popups::toggle(&popup, anchor, cx),
                    None => report!("the popup `{popup}` has nowhere to open"),
                }
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| {
                Previews::close(cx);
                Menus::close(cx);
                Actions::run("winx", None, cx);
            })
            .child(canvas(measure, |_, (), _, _| {}).absolute().size_full())
            .child(shown);
        let button = Interaction::track(button, cx, |this: &mut Self| {
            Some(&mut this.start.as_mut()?.interaction)
        });
        let line = others.then(|| {
            let line = div()
                .flex_none()
                .bg(theme.colors.outline.opacity(START_LINE));
            match layout {
                Layout::Row => line
                    .w(px(1.))
                    .h(theme.dock.item_size / 2.)
                    .mx(theme.spacing.tiny),
                Layout::Column { .. } => line
                    .h(px(1.))
                    .w(theme.dock.item_size / 2.)
                    .my(theme.spacing.tiny),
            }
        });
        let wrap = div().flex().items_center().flex_none();
        let wrap = match layout {
            Layout::Row => wrap,
            Layout::Column { .. } => wrap.flex_col(),
        };
        Some(wrap.child(button).children(line).into_any_element())
    }

    /// The number of notifications that wait, `shown` of the way grown.
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

    /// How the buttons are laid out in `window`: a row on a surface at
    /// the top or the bottom, a column on one at the left or the right
    /// with the marks on the side away from that edge. Where the dock is
    /// on no surface, as in a popup, a column where it is taller than
    /// wide, with the marks toward the larger room of the display.
    fn layout_of(window: &Window, cx: &App) -> Layout {
        match crate::surface::edge_of(window, cx) {
            Some(ScreenEdge::Top | ScreenEdge::Bottom) => return Layout::Row,
            Some(ScreenEdge::Left) => {
                return Layout::Column {
                    toward: Side::Right,
                };
            }
            Some(ScreenEdge::Right) => {
                return Layout::Column { toward: Side::Left };
            }
            None => {}
        }
        let bounds = window.bounds();
        let display = window.display(cx).map(|display| display.bounds());
        let (left, right) = display.map_or((0., 0.), |display| {
            let left = bounds.origin.x - display.origin.x;
            let right =
                display.origin.x + display.size.width - (bounds.origin.x + bounds.size.width);
            (left / px(1.), right / px(1.))
        });
        Layout::of(
            bounds.size.width / px(1.),
            bounds.size.height / px(1.),
            left,
            right,
        )
    }
}

/// A ring around the button the keyboard is on.
trait FocusRing {
    fn focus_ring(self, focused: bool, theme: Theme) -> Self;
}

impl FocusRing for gpui::Stateful<gpui::Div> {
    fn focus_ring(self, focused: bool, theme: Theme) -> Self {
        match focused {
            true => self.border_2().border_color(theme.colors.primary),
            false => self,
        }
    }
}

impl Render for WindowList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        self.surface = Some(window.window_handle());
        self.layout = Self::layout_of(window, cx);
        let since = self.born.elapsed();
        let moving = self.items.iter().any(|item| {
            !item.leaving
                && (item.asking
                    || item.told.progress.is_some_and(|progress| {
                        progress.state == crate::platform::ProgressState::Indeterminate
                    }))
        });
        if moving {
            window.request_animation_frame();
        }
        let wide = window.viewport_size().width;
        let labelled = match (self.options.labels, self.layout) {
            (false, _) => None,
            (true, Layout::Row) => Some(theme.dock.item_size * LABELLED_WIDTH),
            (true, Layout::Column { .. }) => {
                let room = wide - theme.spacing.small * 2.;
                (wide >= theme.dock.item_size * LABELLED_COLUMN).then_some(room)
            }
        };
        let look = Look {
            theme,
            layout: self.layout,
            edge: Edge::of(self.layout),
            labelled,
            counter: self.options.counter,
            middle: self.options.middle,
            wheel: self.options.wheel,
            travel: marks::phase(since, marks::TRAVEL),
            pulse: marks::pulse(marks::phase(since, marks::PULSE)),
        };
        if self.shell
            && let Ok(frame) = NativeSurface::of(window).and_then(|native| native.frame())
        {
            self.tell_minimize_targets(frame, window.scale_factor());
        }
        let keyboard = self.keyboard;
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
                let focused = keyboard == Some(item.key);
                Self::render_item(item, icon, look, focused, window, cx)
            })
            .collect();
        // A button that has finished leaving was just drawn at no width;
        // its windows no longer minimize to it.
        items.retain(|item| {
            let gone = item.leaving && item.presence.is_settled();
            if gone && let Some((_, ids)) = &item.minimizes_to {
                for id in ids {
                    ShellHost::set_min_rect(*id, None);
                }
            }
            !gone
        });

        let start = self.render_start(!elements.is_empty(), look, window, cx);
        if elements.is_empty() && start.is_none() {
            return div().id("dock");
        }
        let padding = theme.spacing.small - theme.spacing.tiny / 2.;
        let list = div()
            .id("dock")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, window, cx| this.key_down(event, window, cx)))
            .flex()
            .items_center()
            .rounded(theme.shape.large)
            .bg(theme.colors.surface_container);
        let list = match self.layout {
            Layout::Row => list.h(theme.dock.container_height).px(padding),
            Layout::Column { .. } => list
                .w(labelled.map_or(theme.dock.container_height, |room| room + padding * 2.))
                .flex_col()
                .py(padding),
        };
        list.children(start).children(elements)
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
