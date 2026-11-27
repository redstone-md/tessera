// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Task View and Present Windows: the windows spread over their displays
//! as live pictures, as Win+Tab shows them with Explorer and as KWin's
//! Overview and Present Windows do, so that one is found by sight or by
//! typing and brought in front.
//!
//! Each display gets a window of its own that covers it, dims what is
//! under it, and lays the windows that stand on that display out with
//! [`spread`](crate::ui::spread::spread). Windows draws the pictures and
//! keeps them up to date, over everything the window of Tessera draws, so
//! the titles, the icons, the close buttons and the marks are drawn
//! around the pictures, never over them. A minimized window, and one
//! hidden on another workspace of Tessera's own, has no picture to give
//! and shows its icon on a card in its place.
//!
//! Typing narrows the windows to those whose title or program has what
//! is typed; where no window has it, the runners of the launcher answer
//! it below the line of what is typed, and Enter does the first answer.
//! The arrows go to the picture beside the chosen one, Enter or a click
//! brings a window in front (going to its workspace first), Delete or a
//! click with the middle button asks it to close, and Esc or a click on
//! no picture closes Task View. Along the top, where Tessera keeps
//! workspaces of its own, a card for each workspace shows its windows'
//! icons: a click goes there, and a picture dropped on a card has its
//! window live on that workspace.
//!
//! The action `taskview` opens it (Win+Tab where the keys of Windows are
//! in use), `present-windows` opens the same without the workspaces;
//! both take `workspace` (the windows of the workspace each display
//! shows), `all` (those of every workspace, grouped by workspace) or
//! `app` (those of the application in front).

use std::cmp::Reverse;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AsyncApp, Context, FocusHandle, FontWeight, Global, InteractiveElement,
    IntoElement, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Render, SharedString, Styled, Subscription, Task,
    WeakEntity, Window, WindowHandle, div, img, px,
};

use crate::actions::{Action, Actions};
use crate::i18n::Language;
use crate::icons::{AppIcons, Icon};
use crate::module::WidgetContext;
use crate::motion::{InteractionState, Spring};
use crate::platform::{
    AppWindow, AppWindowId, Display, DisplayKey, NativeSurface, ScreenRect, Thumbnail, Veil,
    display_at, display_frames,
};
use crate::popup::{open_floating, retire_floating};
use crate::runners::{Answer, History, Query, RunnerOptions, Runners};
use crate::snap::gpui_display;
use crate::switcher::choice::{answers, same_application, typed_text};
use crate::switcher::{program_name, resident_switchable, switchable};
use crate::theme::Theme;
use crate::tiling::Toward;
use crate::ui::spread::{between, center, holds, nearest, scaled, shifted, spread};
use crate::ui::{icon, one_line, release_hover, state_layer};
use crate::workspaces::Workspaces;

/// How the arguments of the actions are described where one is refused.
const MODES: &str = "it takes `workspace`, `all` or `app`, or none";

/// How dark the displays are behind Task View, once it is open.
const DIM: f32 = 0.6;

/// The measures of Task View, in logical pixels: the space around what
/// it shows, the cards of the workspaces, the line of what is typed, the
/// row of a title above each picture, the space between pictures, the
/// side of the close button, the mark around the chosen picture, and the
/// line of an answer of the runners.
const PADDING: f32 = 48.;
const TOP: f32 = 20.;
const CARD: (f32, f32) = (176., 92.);
const CARD_GAP: f32 = 12.;
const FIELD: (f32, f32) = (440., 40.);
const ANSWERS_WIDTH: f32 = 560.;
const BELOW: f32 = 16.;
const TITLE: f32 = 30.;
const GAP: f32 = 20.;
const CLOSE: f32 = 24.;
const RING: f32 = 3.;
const ANSWER: f32 = 44.;
const CARD_ICON: f32 = 20.;
const CARD_ICONS: usize = 6;
const CARD_ICON_SIDE: f32 = 48.;

/// How far a press moves before it drags a picture, in logical pixels,
/// how large a dragged picture is next to its place, and how far below
/// and to the right of the pointer it hangs. It is never larger than a
/// card and never covers the cards: Windows draws it over them.
const DRAG_START: f32 = 6.;
const DRAG_SCALE: f32 = 0.45;
const DRAG_OFFSET: f32 = 16.;

/// How many answers of the runners are shown, and how often and how many
/// times a runner that is still finding out is asked again.
const ANSWERS_SHOWN: usize = 6;
const ASK_AGAIN: Duration = Duration::from_millis(300);
const ASKED_AGAIN_AT_MOST: usize = 5;

/// The runners that answer in Task View: the ones that do not offer the
/// windows again, and none that asks to be confirmed.
const ANSWERING: [&str; 8] = [
    "calculator",
    "units",
    "command",
    "web",
    "settings",
    "emoji",
    "recent",
    "search",
];

/// How often, and how many times, it is looked whether a window asked to
/// close has gone; one that is still there after that stays.
const CLOSE_CHECK: Duration = Duration::from_millis(100);
const CLOSE_CHECKS: usize = 15;

/// How long after a window of Task View lost the keyboard it is looked
/// whether another one has it, before Task View closes.
const ACTIVE_CHECK: Duration = Duration::from_millis(80);

/// Where a picture that is not to be seen is put.
const NOWHERE: ScreenRect = ScreenRect {
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
};

/// How a card under the pointer is tinted.
const HOVERED: InteractionState = InteractionState {
    hover: 1.,
    press: 0.,
};

const GLYPH_SEARCH: &str = "\u{e721}";
const GLYPH_CLOSE: &str = "\u{e8bb}";

/// Which windows Task View shows, as the `arg` of its actions says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The windows of the workspace each display shows.
    #[default]
    Workspace,
    /// The windows of every workspace, grouped by workspace.
    All,
    /// The windows of the application in front.
    App,
}

impl Mode {
    /// The mode `arg` names; `None` for one there is not.
    fn of(arg: Option<&str>) -> Option<Self> {
        Some(match arg {
            None | Some("workspace") => Self::Workspace,
            Some("all") => Self::All,
            Some("app") => Self::App,
            Some(_) => return None,
        })
    }
}

/// The open Task View: its window on each display, and what was typed.
#[derive(Default)]
pub struct TaskView {
    screens: Vec<WindowHandle<Screen>>,
    typed: String,
    /// Counts the times Task View opened, so that a check that was meant
    /// for one that closed since does nothing.
    opened: u64,
}

impl Global for TaskView {}

impl TaskView {
    /// Adds the actions `taskview` and `present-windows`.
    pub fn start(cx: &mut App) {
        cx.set_global(Self::default());
        Actions::add(
            "taskview",
            Self::action(true).by_default("win+tab", None),
            cx,
        );
        Actions::add("present-windows", Self::action(false), cx);
    }

    /// The action that opens Task View, or closes it: with the cards of
    /// the workspaces along the top where `strip` says so.
    fn action(strip: bool) -> Action {
        Action::new(move |arg, cx| {
            if let Some(mode) = Mode::of(arg) {
                Self::toggle(mode, strip, cx);
            }
        })
        .taking_reserved_keys()
        .checking(|arg| Mode::of(arg).map(|_| ()).ok_or_else(|| MODES.to_owned()))
    }

    /// Opens Task View showing the windows `mode` names, or closes it
    /// where it is open.
    pub fn toggle(mode: Mode, strip: bool, cx: &mut App) {
        if cx.default_global::<Self>().screens.is_empty() {
            Self::open(mode, strip, cx);
        } else {
            Self::close(cx);
        }
    }

    fn open(mode: Mode, strip: bool, cx: &mut App) {
        let windows = gather(mode, cx);
        let total = windows.len();
        let main = DisplayKey::under_pointer();
        let runners = Runners::of_widget(&answering(), RunnerOptions::default());
        History::read(cx);
        let this = cx.global_mut::<Self>();
        this.opened += 1;
        this.typed.clear();
        let mut screens = Vec::new();
        // The display is chosen here and the window put over it, as a
        // window that waits to be opened again stands wherever it was.
        for display in Display::all() {
            let gpui = gpui_display(&display, cx).and_then(|id| cx.find_display(id));
            let rough = gpui.as_ref().map(|gpui| gpui.bounds()).unwrap_or_default();
            let opening = Opening {
                display,
                windows: windows.clone(),
                mode,
                strip,
                main,
                total,
                runners: runners.clone(),
            };
            let opened =
                open_floating(rough, gpui.map(|gpui| gpui.id()), true, cx, |window, cx| {
                    Screen::new(opening, window, cx)
                });
            match opened {
                // One that could not be put over its display goes away
                // at once, and Task View is not open there.
                Ok(screen) => {
                    let open = |screen: &mut Screen, _: &mut Window, _: &mut Context<Screen>| {
                        !screen.closing
                    };
                    if screen.update(cx, open).unwrap_or(false) {
                        screens.push(screen);
                    }
                }
                Err(error) => report!("Task View did not open on a display: {error:#}"),
            }
        }
        cx.global_mut::<Self>().screens = screens;
        Self::give_choice(cx);
    }

    /// Has a window be chosen on the display whose window has the line of
    /// what is typed, or where that one shows none, on the first display
    /// that shows one: the keys that steer the choice work there.
    fn give_choice(cx: &mut App) {
        let screens = cx.default_global::<Self>().screens.clone();
        let shown = |screen: &mut Screen, _: &mut Window, _: &mut Context<Screen>| {
            (screen.main, !screen.order.is_empty())
        };
        let shown: Vec<(bool, bool)> = { screens.iter() }
            .map(|screen| screen.update(cx, shown).unwrap_or((false, false)))
            .collect();
        let steering = { shown.iter() }
            .position(|&(main, any)| main && any)
            .or_else(|| shown.iter().position(|&(_, any)| any));
        for (at, screen) in screens.iter().enumerate() {
            let steers = steering == Some(at);
            { screen.update(cx, |screen, _, cx| screen.offer(steers, cx)) }.ok();
        }
    }

    /// Has a key that steers the choice work on the window that offers
    /// the choice, where the one that has the keyboard shows no window.
    fn steer_elsewhere(key: &str, held: Modifiers, cx: &mut App) {
        let screens = cx.default_global::<Self>().screens.clone();
        for screen in screens {
            let steered = screen.update(cx, |screen, _, cx| {
                let offers = screen.chosen.is_some();
                if offers {
                    screen.steer(key, held, cx);
                }
                offers
            });
            if matches!(steered, Ok(true)) {
                return;
            }
        }
    }

    /// Closes Task View, bringing no window in front.
    pub fn close(cx: &mut App) {
        Self::finish(None, cx);
    }

    /// Closes Task View and brings `chosen` in front, going to its
    /// workspace first where it lives on one that is not shown.
    fn finish(chosen: Option<(AppWindowId, bool)>, cx: &mut App) {
        let this = cx.default_global::<Self>();
        let screens = std::mem::take(&mut this.screens);
        this.typed.clear();
        match chosen {
            Some((id, true)) => Workspaces::show_window(id, cx),
            Some((id, false)) => id.activate(),
            None => {}
        }
        for screen in screens {
            screen.update(cx, |screen, _, cx| screen.close(cx)).ok();
        }
    }

    /// Has `typed` be what is typed into Task View: every display shows
    /// the windows that answer it, and where none does on any display,
    /// the runners are asked.
    fn type_into(typed: String, cx: &mut App) {
        let this = cx.default_global::<Self>();
        if this.typed == typed {
            return;
        }
        this.typed.clone_from(&typed);
        let screens = this.screens.clone();
        let mut matched = 0;
        for screen in &screens {
            let shown = screen.update(cx, |screen, _, cx| screen.filter(&typed, cx));
            matched += shown.unwrap_or(0);
        }
        Self::give_choice(cx);
        for screen in &screens {
            let unmatched = matched == 0;
            { screen.update(cx, |screen, _, cx| screen.consult(unmatched, cx)) }.ok();
        }
    }

    /// Closes Task View unless one of its windows has the keyboard, once
    /// a moment has passed for another one to take it: a click on the
    /// window of another display takes it from the first.
    fn check_active(cx: &mut App) {
        let opened = cx.default_global::<Self>().opened;
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor().timer(ACTIVE_CHECK).await;
            cx.update(|cx| {
                let this = cx.global::<Self>();
                if this.opened != opened || this.screens.is_empty() {
                    return;
                }
                let screens = this.screens.clone();
                let active = screens.iter().any(|screen| {
                    let active = |_: &mut Screen, window: &mut Window, _: &mut Context<Screen>| {
                        window.is_window_active()
                    };
                    screen.update(cx, active).unwrap_or(false)
                });
                if !active {
                    Self::close(cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

/// What the runners of Task View are chosen by.
fn answering() -> WidgetContext {
    let names = ANSWERING
        .iter()
        .map(|name| toml::Value::String((*name).to_owned()));
    let mut options = toml::Table::new();
    options.insert("runners".to_owned(), toml::Value::Array(names.collect()));
    WidgetContext {
        display: None,
        options,
    }
}

/// A window as Task View shows it.
#[derive(Clone)]
struct Seen {
    id: AppWindowId,
    title: SharedString,
    /// The name of its program, which typing finds as well as the title.
    name: SharedString,
    icon: Icon,
    display: DisplayKey,
    /// Where it stands on the screen, in physical pixels, where it is
    /// seen there: neither minimized nor hidden.
    frame: Option<ScreenRect>,
    /// The workspace of Tessera's own it lives on, where Tessera keeps
    /// them.
    workspace: Option<usize>,
    /// Whether it is hidden on a workspace that is not shown.
    away: bool,
}

impl Seen {
    fn of(
        id: AppWindowId,
        program: Option<PathBuf>,
        display: DisplayKey,
        workspace: Option<usize>,
        away: bool,
        cx: &App,
    ) -> Self {
        let minimized = id.is_minimized();
        Self {
            id,
            title: id.title().into(),
            name: program_name(program.as_deref()),
            icon: { program.as_deref() }
                .and_then(|program| cx.global::<AppIcons>().of_executable(program)),
            display,
            frame: (!away && !minimized).then(|| id.frame()).flatten(),
            workspace,
            away,
        }
    }

    /// Whether Windows can draw a picture of it.
    fn may_be_pictured(&self) -> bool {
        self.frame.is_some()
    }
}

/// The windows `mode` names, of every display, in the order Windows
/// keeps them, the one in front first.
fn gather(mode: Mode, cx: &App) -> Vec<Seen> {
    let visible: Vec<AppWindow> = { AppWindow::enumerate().into_iter() }
        .filter(|window| switchable(window, cx))
        .collect();
    let own = Workspaces::residents(None, cx).is_some();
    let current = |display: DisplayKey| own.then(|| Workspaces::now(Some(display), cx).current);
    let shown = |window: &AppWindow| {
        let workspace = current(window.display);
        Seen::of(
            window.id,
            window.program(),
            window.display,
            workspace,
            false,
            cx,
        )
    };
    match mode {
        Mode::Workspace => visible.iter().map(shown).collect(),
        Mode::App => {
            let front = AppWindowId::foreground();
            let followed = { visible.iter() }
                .find(|window| Some(window.id) == front)
                .or(visible.first());
            let Some(followed) = followed else {
                return Vec::new();
            };
            let program = followed.program();
            { visible.iter() }
                .filter(|window| {
                    window.id == followed.id
                        || same_application(window.program().as_deref(), program.as_deref())
                })
                .map(shown)
                .collect()
        }
        Mode::All => {
            let mut seen: Vec<Seen> = visible.iter().map(shown).collect();
            for frame in display_frames() {
                let display = display_at(frame);
                let Some(residents) = Workspaces::residents(Some(display), cx) else {
                    continue;
                };
                let now = Workspaces::now(Some(display), cx).current;
                for (index, windows) in residents.into_iter().enumerate() {
                    for resident in windows {
                        let known = seen.iter().any(|other| other.id == resident.id);
                        if known || index == now || !resident.id.exists() {
                            continue;
                        }
                        if !resident_switchable(&resident, cx) {
                            continue;
                        }
                        // A hidden window keeps its place, which tells
                        // the display it comes back on.
                        let at = resident.id.frame().map_or(display, display_at);
                        let program = resident.program;
                        seen.push(Seen::of(resident.id, program, at, Some(index), true, cx));
                    }
                }
            }
            seen
        }
    }
}

/// What a window of Task View opens with.
struct Opening {
    /// The display it covers.
    display: Display,
    windows: Vec<Seen>,
    mode: Mode,
    strip: bool,
    /// The display the pointer is on, whose window has the line of what
    /// is typed and takes the keyboard.
    main: Option<DisplayKey>,
    /// How many windows Task View shows on all displays.
    total: usize,
    runners: Runners,
}

/// The card of a workspace along the top.
struct Card {
    index: usize,
    label: SharedString,
    icons: Vec<Icon>,
    /// Whether this is the workspace the display shows.
    current: bool,
}

/// A window on a display of Task View, its picture and where it goes.
struct Tile {
    seen: Seen,
    /// The picture of the window, with the physical size of the window.
    picture: Option<(Thumbnail, (i32, i32))>,
    /// Where its picture goes, in physical pixels of the window of Task
    /// View, and where it comes from.
    place: ScreenRect,
    from: ScreenRect,
    /// How far it has come from `from` to `place`, from 0 to 1.
    glide: Spring,
    /// Whether it answers what is typed.
    shown: bool,
    /// Whether it was asked to close and has not gone yet.
    closing: bool,
}

impl Tile {
    /// Where its picture is now.
    fn now(&mut self, window: &Window) -> ScreenRect {
        between(self.from, self.place, self.glide.value(window))
    }

    /// Has the picture go to `place` from where it is now.
    fn go(&mut self, place: ScreenRect) {
        if place == self.place {
            return;
        }
        self.from = between(self.from, self.place, self.glide.value_at(Instant::now()));
        self.place = place;
        self.glide.jump_to(0.);
        self.glide.animate_to(1.);
    }

    fn hide_picture(&self) {
        if let Some((thumbnail, _)) = &self.picture {
            thumbnail.place(NOWHERE, 0.);
        }
    }

    /// Where the picture covers its window on the screen: Windows
    /// pictures the whole window, with the borders it gives a window to
    /// be resized by, around what is seen of it.
    fn real(&self) -> Option<ScreenRect> {
        let frame = self.seen.frame?;
        let Some((_, (width, height))) = self.picture else {
            return Some(frame);
        };
        let (x, y) = center(frame);
        Some(ScreenRect {
            left: x - width / 2,
            top: y - height / 2,
            right: x - width / 2 + width,
            bottom: y - height / 2 + height,
        })
    }

    /// The shape it is spread by: that of its picture where it stands,
    /// or for one without a picture a card of the shape of a screen.
    fn shape(&self, size: (i32, i32)) -> ScreenRect {
        self.real().unwrap_or(ScreenRect {
            left: size.0 / 3,
            top: size.1 / 3,
            right: size.0 * 2 / 3,
            bottom: size.1 / 3 + size.0 / 3 * 10 / 16,
        })
    }
}

/// What a point of the window of Task View is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    /// The close button of the tile at that place.
    Close(usize),
    /// The picture or the title of the tile at that place.
    Tile(usize),
    /// The card of the workspace at that place of the cards.
    Card(usize),
    /// The answer of the runners at that place.
    Answer(usize),
    Nothing,
}

impl Hit {
    /// Whether it is on a tile, the picture, the title or the close
    /// button.
    fn is_tile(self) -> bool {
        matches!(self, Self::Tile(_) | Self::Close(_))
    }
}

/// A press of the left button and what it began on; once it has moved
/// far enough it drags the tile it began on.
#[derive(Clone, Copy, Debug)]
struct Press {
    on: Hit,
    at: (i32, i32),
    dragging: bool,
}

/// The window of Task View on one display.
pub struct Screen {
    mode: Mode,
    main: bool,
    /// How many windows Task View shows on all displays.
    total: usize,
    /// Where the window stands, the display it covers, in physical
    /// pixels, and physical pixels per logical one.
    frame: ScreenRect,
    scale: f32,
    /// The native window, which the pictures are drawn in.
    native: Option<NativeSurface>,
    tiles: Vec<Tile>,
    /// The places of the tiles in the order they are offered: by group,
    /// then as they were spread.
    order: Vec<usize>,
    /// The label over each group of `all`, and where it stands.
    bands: Vec<(ScreenRect, SharedString)>,
    /// What the workspaces are called, and their cards along the top
    /// where `strip` asks for them.
    names: Vec<SharedString>,
    strip: bool,
    cards: Vec<Card>,
    card_places: Vec<ScreenRect>,
    plan: Plan,
    /// The tile chosen with the keyboard.
    chosen: Option<usize>,
    /// What the pointer is on, and where it is, in physical pixels of
    /// the window.
    hovered: Hit,
    pointer: (i32, i32),
    press: Option<Press>,
    typed: String,
    runners: Runners,
    /// What the runners answered, best first, and the one chosen.
    answers: Vec<Answer>,
    told: Vec<Vec<Answer>>,
    answer_chosen: usize,
    /// Whether all runners have answered what is typed.
    asked: bool,
    /// Whether no window answers what is typed on any display.
    unmatched: bool,
    _asking: Option<Task<()>>,
    /// 0 while hidden, 1 when fully open.
    visibility: Spring,
    closing: bool,
    veil: Option<Veil>,
    /// Whether the window has had the keyboard; before that, not having
    /// it does not mean the user turned away.
    was_active: bool,
    focus: FocusHandle,
    _activation: Subscription,
}

impl Screen {
    fn new(opening: Opening, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let native = NativeSurface::of(window);
        let placed = native
            .as_ref()
            .map_err(|error| anyhow::anyhow!("{error:#}"));
        let Display {
            key: display,
            frame,
            scale,
            ..
        } = opening.display;
        let placed = placed.and_then(|native| {
            native.adopt_as_popup()?;
            native.place(frame)?;
            Ok(*native)
        });
        let native = match placed {
            Ok(placed) => placed,
            Err(error) => {
                report!("Task View is not where it belongs: {error:#}");
                return Self::failed(opening, window, cx);
            }
        };
        let main = opening.main.is_none_or(|main| main == display);
        let veil = Veil::open(frame, native)
            .inspect_err(|error| report!("the display behind Task View is not dimmed: {error:#}"))
            .ok();
        native.reveal(false);
        if main {
            native.bring_forward();
        }
        let tiles: Vec<Tile> = { opening.windows.into_iter() }
            .filter(|seen| seen.display == display)
            .map(|seen| {
                let picture = { seen.may_be_pictured() }
                    .then(|| Thumbnail::of(seen.id, native).ok())
                    .flatten()
                    .and_then(|thumbnail| {
                        let source = thumbnail.source_size()?;
                        Some((thumbnail, source))
                    });
                Tile {
                    seen,
                    picture,
                    place: NOWHERE,
                    from: NOWHERE,
                    glide: Spring::new(motion.spatial, 1.),
                    shown: true,
                    closing: false,
                }
            })
            .collect();
        let mut this = Self::blank(opening.mode, opening.runners, window, cx);
        this.main = main;
        this.total = opening.total;
        this.frame = frame;
        this.native = Some(native);
        // That of the display it was put over: GPUI may learn it later.
        this.scale = scale;
        this.tiles = tiles;
        this.veil = veil;
        this.strip = opening.strip;
        this.read_workspaces(display, opening.strip, cx);
        this.arrange();
        // Each picture glides from its window to its place, each card
        // grows from a little smaller.
        let origin = (frame.left, frame.top);
        for tile in &mut this.tiles {
            tile.from = match tile.real() {
                Some(real) => shifted(real, -origin.0, -origin.1),
                None => scaled(tile.place, 0.85),
            };
            tile.glide.jump_to(0.);
            tile.glide.animate_to(1.);
        }
        this.visibility = Spring::new(motion.spatial, 0.);
        this.visibility.animate_to(1.);
        this
    }

    /// The view of a window that could not be put over its display,
    /// which goes away at once.
    fn failed(opening: Opening, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::blank(opening.mode, opening.runners, window, cx);
        this.closing = true;
        this
    }

    fn blank(mode: Mode, runners: Runners, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.was_active = true;
            } else if this.was_active && !this.closing {
                cx.defer(TaskView::check_active);
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            mode,
            main: false,
            total: 0,
            frame: NOWHERE,
            scale: 1.,
            native: None,
            tiles: Vec::new(),
            order: Vec::new(),
            bands: Vec::new(),
            names: Vec::new(),
            strip: false,
            cards: Vec::new(),
            card_places: Vec::new(),
            plan: Plan::default(),
            chosen: None,
            hovered: Hit::Nothing,
            pointer: (0, 0),
            press: None,
            typed: String::new(),
            runners,
            answers: Vec::new(),
            told: Vec::new(),
            answer_chosen: 0,
            asked: false,
            unmatched: false,
            _asking: None,
            visibility: Spring::new(motion.spatial, 0.),
            closing: false,
            veil: None,
            was_active: false,
            focus,
            _activation: activation,
        }
    }

    /// Reads the names of the workspaces of `display`, where Tessera
    /// keeps workspaces of its own, and with `cards` their cards.
    fn read_workspaces(&mut self, display: DisplayKey, cards: bool, cx: &App) {
        self.cards.clear();
        let Some(residents) = Workspaces::residents(Some(display), cx) else {
            self.names.clear();
            return;
        };
        let language = Language::of(cx);
        self.names = { 0..residents.len() }
            .map(|index| workspace_label(index, language, cx))
            .collect();
        if !cards {
            return;
        }
        let current = Workspaces::now(Some(display), cx).current;
        let icons = cx.global::<AppIcons>();
        self.cards = { residents.into_iter().enumerate() }
            .map(|(index, windows)| Card {
                index,
                label: self.names[index].clone(),
                icons: { windows.iter() }
                    .take(CARD_ICONS)
                    .map(|resident| {
                        { resident.program.as_deref() }
                            .and_then(|program| icons.of_executable(program))
                    })
                    .collect(),
                current: index == current,
            })
            .collect();
    }

    /// The size of the display in physical pixels.
    fn size(&self) -> (i32, i32) {
        (
            self.frame.right - self.frame.left,
            self.frame.bottom - self.frame.top,
        )
    }

    /// `length` logical pixels in physical ones.
    fn physical(&self, length: f32) -> i32 {
        (length * self.scale).round() as i32
    }

    /// `length` physical pixels in logical ones.
    fn logical(&self, length: i32) -> Pixels {
        px(length as f32 / self.scale)
    }

    /// Lays the tiles that answer what is typed out again, the groups of
    /// `all` in bands one under the other; those that moved glide to
    /// their new places from where they are.
    fn arrange(&mut self) {
        if self.closing {
            return;
        }
        let size = self.size();
        self.plan = Plan::of(size, self.scale, !self.cards.is_empty(), self.main);
        self.card_places = card_places(self.plan.strip, self.cards.len(), self.scale);
        for tile in &mut self.tiles {
            tile.shown = answers(&tile.seen.title, &tile.seen.name, &self.typed);
        }
        let mut groups: Vec<(Option<usize>, Vec<usize>)> = Vec::new();
        for (index, tile) in self.tiles.iter().enumerate() {
            if !tile.shown {
                continue;
            }
            let key = match self.mode {
                Mode::All => tile.seen.workspace,
                _ => None,
            };
            match groups.iter_mut().find(|(group, _)| *group == key) {
                Some((_, members)) => members.push(index),
                None => groups.push((key, vec![index])),
            }
        }
        groups.sort_by_key(|(key, _)| *key);
        let labelled = groups.len() > 1 || groups.iter().any(|(key, _)| key.is_some());
        let labelled = labelled && self.mode == Mode::All;
        let label = match labelled {
            true => self.physical(TITLE),
            false => 0,
        };
        let gap = self.physical(GAP + TITLE);
        let areas = bands(self.plan.area, groups.len(), label, self.physical(GAP));
        self.bands.clear();
        self.order.clear();
        for ((key, members), (heading, area)) in groups.into_iter().zip(areas) {
            if labelled && let Some(key) = key {
                self.bands.push((heading, self.band_label(key)));
            }
            let shapes: Vec<ScreenRect> = { members.iter() }
                .map(|&index| self.tiles[index].shape(size))
                .collect();
            let places = spread(&shapes, area, gap);
            let mut placed: Vec<(usize, ScreenRect)> = members.into_iter().zip(places).collect();
            // Offered as they are read: by row, then from the left. The
            // pictures of a row stand on the same middle.
            placed.sort_by_key(|&(_, place)| (center(place).1, place.left));
            for (index, place) in placed {
                self.tiles[index].go(place);
                self.order.push(index);
            }
        }
        if self
            .chosen
            .is_some_and(|chosen| !self.order.contains(&chosen))
        {
            self.chosen = self.order.first().copied();
        }
    }

    fn band_label(&self, index: usize) -> SharedString {
        { self.names.get(index).cloned() }.unwrap_or_else(|| (index + 1).to_string().into())
    }

    /// Has `typed` be what is typed, and gives how many tiles answer it.
    fn filter(&mut self, typed: &str, cx: &mut Context<Self>) -> usize {
        self.typed = typed.to_owned();
        self.arrange();
        cx.notify();
        self.order.len()
    }

    /// Has a tile be chosen where the keys that steer the choice work
    /// here, `steers`, keeping the one chosen; none otherwise.
    fn offer(&mut self, steers: bool, cx: &mut Context<Self>) {
        self.chosen = match steers {
            true => self.chosen.or_else(|| self.order.first().copied()),
            false => None,
        };
        cx.notify();
    }

    /// Asks the runners about what is typed where this window has the
    /// line of it and no window answers it on any display, `unmatched`;
    /// otherwise forgets what they answered.
    fn consult(&mut self, unmatched: bool, cx: &mut Context<Self>) {
        self.answers.clear();
        self.told.clear();
        self.answer_chosen = 0;
        self.asked = false;
        self.unmatched = unmatched;
        self._asking = None;
        cx.notify();
        if !self.answering() {
            return;
        }
        let (text, language, runners) =
            (self.typed.clone(), Language::of(cx), self.runners.clone());
        let asking = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            for _ in 0..ASKED_AGAIN_AT_MOST {
                let query = Query::new(&text, language);
                let Ok(answers) = cx.update(|cx| runners.ask(query, cx.background_executor()))
                else {
                    return;
                };
                let mut waiting = false;
                while let Ok((runner, batch)) = answers.recv().await {
                    waiting |= batch.iter().any(|answer| answer.waiting);
                    let told = this.update(cx, |this, cx| this.answered(&text, runner, batch, cx));
                    if told.is_err() {
                        return;
                    }
                }
                if !waiting {
                    break;
                }
                cx.background_executor().timer(ASK_AGAIN).await;
            }
            this.update(cx, |this, cx| {
                this.asked = true;
                cx.notify();
            })
            .ok();
        });
        self._asking = Some(asking);
    }

    /// Takes what the runner at `runner` answered to `text`, unless
    /// something else is typed by now: the best answers of all runners
    /// are shown, those that ask to be confirmed left out.
    fn answered(&mut self, text: &str, runner: usize, batch: Vec<Answer>, cx: &mut Context<Self>) {
        if text != self.typed {
            return;
        }
        if self.told.len() <= runner {
            self.told.resize_with(runner + 1, Vec::new);
        }
        self.told[runner] = { batch.into_iter() }
            .filter(|answer| !answer.action.needs_confirmation())
            .collect();
        let history = cx.default_global::<History>();
        let mut answers = self.told.concat();
        answers
            .sort_by_key(|answer| Reverse(answer.score.saturating_add(history.boost(&answer.key))));
        answers.truncate(ANSWERS_SHOWN);
        self.answer_chosen = self.answer_chosen.min(answers.len().saturating_sub(1));
        self.answers = answers;
        cx.notify();
    }

    /// Whether the answers of the runners stand where the tiles would:
    /// on the display with the line of what is typed, where no window on
    /// any display answers it.
    fn answering(&self) -> bool {
        self.main && self.unmatched && !self.typed.trim().is_empty()
    }

    /// Does the answer at `index`, and closes.
    fn run_answer(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(answer) = self.answers.get(index) else {
            return;
        };
        let (key, action) = (answer.key.clone(), answer.action.clone());
        History::chose(&key, cx);
        action.perform(false, false, cx);
        cx.defer(TaskView::close);
    }

    /// Brings the window of the tile at `index` in front, and closes.
    fn choose(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(tile) = self.tiles.get(index) else {
            return;
        };
        let chosen = (tile.seen.id, tile.seen.away);
        cx.defer(move |cx| TaskView::finish(Some(chosen), cx));
    }

    /// Asks the window of the tile at `index` to close, and stops showing
    /// it once it is gone. The application may ask the user first, or
    /// refuse; the window then stays.
    fn close_window(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(tile) = self.tiles.get_mut(index) else {
            return;
        };
        if tile.closing {
            return;
        }
        tile.closing = true;
        let (id, shown) = (tile.seen.id, !tile.seen.away);
        id.close();
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            for _ in 0..CLOSE_CHECKS {
                cx.background_executor().timer(CLOSE_CHECK).await;
                // A window that only hides when closed, as one that goes
                // to the notification area, is gone as well.
                if !id.exists() || (shown && !id.is_shown()) {
                    this.update(cx, |this, cx| this.forget(id, cx)).ok();
                    return;
                }
            }
            let stays = |this: &mut Self, cx: &mut Context<Self>| {
                if let Some(tile) = this.tiles.iter_mut().find(|tile| tile.seen.id == id) {
                    tile.closing = false;
                    cx.notify();
                }
            };
            this.update(cx, stays).ok();
        })
        .detach();
    }

    /// Stops showing the window `id`.
    fn forget(&mut self, id: AppWindowId, cx: &mut Context<Self>) {
        let Some(index) = self.tiles.iter().position(|tile| tile.seen.id == id) else {
            return;
        };
        self.tiles[index].hide_picture();
        self.tiles.remove(index);
        let fix = |at: usize| match at.cmp(&index) {
            std::cmp::Ordering::Less => Some(at),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(at - 1),
        };
        let lost = self.chosen == Some(index);
        self.chosen = self.chosen.and_then(fix);
        // The order is told again here, as a Task View that is closing
        // does not arrange its tiles.
        self.order = self.order.iter().copied().filter_map(fix).collect();
        // What was pressed and what is under the pointer name tiles by
        // their places, which have moved; the pointer tells again when
        // it moves.
        self.press = self.press.filter(|press| !press.on.is_tile());
        self.hovered = Hit::Nothing;
        self.arrange();
        // The choice goes on to another window where the chosen one went,
        // on another display where this one shows none now.
        if lost {
            self.chosen = self.order.first().copied();
            if self.chosen.is_none() {
                cx.defer(TaskView::give_choice);
            }
        }
        cx.notify();
    }

    /// Has the window of the tile at `index` live on the workspace of the
    /// card at `card`, without going there.
    fn send(&mut self, index: usize, card: usize, cx: &mut Context<Self>) {
        let (Some(tile), Some(card)) = (self.tiles.get(index), self.cards.get(card)) else {
            return;
        };
        let (id, workspace, current) = (tile.seen.id, card.index, card.current);
        if tile.seen.workspace == Some(workspace) {
            return;
        }
        Workspaces::move_window(id, workspace, cx);
        let display = display_at(self.frame);
        self.read_workspaces(display, self.strip, cx);
        if self.mode == Mode::All || current {
            let tile = &mut self.tiles[index];
            tile.seen.workspace = Some(workspace);
            tile.seen.away = !current;
            if !current {
                tile.hide_picture();
                tile.picture = None;
                tile.seen.frame = None;
            } else if tile.picture.is_none()
                && let Some(native) = self.native
            {
                // A window brought to the workspace shown has a picture
                // again, from where it stands.
                tile.seen.frame = id.frame();
                tile.picture = { tile.seen.frame }
                    .and_then(|_| Thumbnail::of(id, native).ok())
                    .and_then(|thumbnail| {
                        let source = thumbnail.source_size()?;
                        Some((thumbnail, source))
                    });
            }
            self.arrange();
            cx.notify();
        } else {
            self.forget(id, cx);
        }
    }

    /// Starts closing: the pictures glide back to their windows and the
    /// window goes once the motion has finished.
    fn close(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.press = None;
        self.visibility.animate_to(0.);
        let origin = (self.frame.left, self.frame.top);
        for tile in &mut self.tiles {
            if let Some(real) = tile.real().filter(|_| tile.shown) {
                let now = between(tile.from, tile.place, tile.glide.value_at(Instant::now()));
                tile.from = now;
                tile.place = shifted(real, -origin.0, -origin.1);
                tile.glide.jump_to(0.);
                tile.glide.animate_to(1.);
            }
        }
        cx.notify();
    }

    /// What the point `at`, in physical pixels of the window, is on.
    fn hit(&self, at: (i32, i32)) -> Hit {
        for &index in self.order.iter().rev() {
            let place = self.tiles[index].place;
            if holds(self.close_button(place), at) {
                return Hit::Close(index);
            }
            if holds(self.title_row(place), at) || holds(place, at) {
                return Hit::Tile(index);
            }
        }
        if let Some(card) = self.card_places.iter().position(|card| holds(*card, at)) {
            return Hit::Card(card);
        }
        if self.answering() {
            let answer = { self.answer_rows().into_iter().enumerate() }
                .find(|(_, row)| holds(*row, at))
                .map(|(index, _)| index);
            if let Some(answer) = answer.filter(|answer| *answer < self.answers.len()) {
                return Hit::Answer(answer);
            }
        }
        Hit::Nothing
    }

    /// The row of the title above the picture at `place`.
    fn title_row(&self, place: ScreenRect) -> ScreenRect {
        let height = self.physical(TITLE);
        let below = self.physical(4.);
        ScreenRect {
            left: place.left,
            top: place.top - height - below,
            right: place.right,
            bottom: place.top - below,
        }
    }

    /// The close button at the end of the row of the title above the
    /// picture at `place`.
    fn close_button(&self, place: ScreenRect) -> ScreenRect {
        let row = self.title_row(place);
        let side = self.physical(CLOSE);
        let inset = (row.bottom - row.top - side) / 2;
        ScreenRect {
            left: row.right - side - inset,
            top: row.top + inset,
            right: row.right - inset,
            bottom: row.top + inset + side,
        }
    }

    /// The rows of the answers of the runners, under the line of what is
    /// typed.
    fn answer_rows(&self) -> Vec<ScreenRect> {
        let Some(field) = self.plan.field else {
            return Vec::new();
        };
        let width = self.physical(ANSWERS_WIDTH).min(self.size().0);
        let height = self.physical(ANSWER);
        let left = (self.size().0 - width) / 2;
        let top = field.bottom + self.physical(BELOW);
        (0..ANSWERS_SHOWN)
            .map(|row| ScreenRect {
                left,
                top: top + height * row as i32,
                right: left + width,
                bottom: top + height * (row as i32 + 1),
            })
            .collect()
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let at = self.at(event.position);
        self.pointer = at;
        if let Some(press) = &mut self.press
            && !press.dragging
            && matches!(press.on, Hit::Tile(_))
            && event.pressed_button == Some(MouseButton::Left)
        {
            let far = (press.at.0 - at.0).abs().max((press.at.1 - at.1).abs());
            press.dragging = far as f32 >= DRAG_START * self.scale;
        }
        let hovered = self.hit(at);
        let dragging = self.press.is_some_and(|press| press.dragging);
        if hovered != self.hovered || dragging {
            self.hovered = hovered;
            cx.notify();
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let at = self.at(event.position);
        let on = self.hit(at);
        match event.button {
            MouseButton::Left => {
                self.press = Some(Press {
                    on,
                    at,
                    dragging: false,
                })
            }
            MouseButton::Middle => {
                if let Hit::Tile(index) | Hit::Close(index) = on {
                    self.close_window(index, cx);
                }
            }
            _ => {}
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some(press) = self.press.take() else {
            return;
        };
        if self.closing {
            return;
        }
        let at = self.at(event.position);
        let on = self.hit(at);
        cx.notify();
        if press.dragging {
            if let (Hit::Tile(index), Hit::Card(card)) = (press.on, on) {
                self.send(index, card, cx);
            }
            return;
        }
        if on != press.on {
            return;
        }
        match on {
            Hit::Close(index) => self.close_window(index, cx),
            Hit::Tile(index) => self.choose(index, cx),
            Hit::Card(card) => {
                if let Some(card) = self.cards.get(card) {
                    let display = display_at(self.frame);
                    Workspaces::show(card.index, Some(display), cx);
                }
                cx.defer(TaskView::close);
            }
            Hit::Answer(answer) => self.run_answer(answer, cx),
            Hit::Nothing => cx.defer(TaskView::close),
        }
    }

    /// The physical point of the window that `position` is.
    fn at(&self, position: gpui::Point<Pixels>) -> (i32, i32) {
        (
            (f32::from(position.x) * self.scale).round() as i32,
            (f32::from(position.y) * self.scale).round() as i32,
        )
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let keystroke = &event.keystroke;
        let held = keystroke.modifiers;
        let typed = self.typed.clone();
        match keystroke.key.as_str() {
            "escape" if !typed.is_empty() => cx.defer(|cx| TaskView::type_into(String::new(), cx)),
            "escape" => cx.defer(TaskView::close),
            key if steers(key, held) => {
                if self.chosen.is_none() && !self.answering() {
                    // The choice is offered on another display.
                    let key = key.to_owned();
                    cx.defer(move |cx| TaskView::steer_elsewhere(&key, held, cx));
                } else {
                    self.steer(key, held, cx);
                }
            }
            "backspace" => {
                let mut typed = typed;
                typed.pop();
                cx.defer(move |cx| TaskView::type_into(typed, cx));
            }
            key => {
                let written = keystroke.key_char.as_deref();
                let Some(text) = typed_text(key, written, held.control, held.alt) else {
                    return;
                };
                let typed = typed + &text;
                cx.defer(move |cx| TaskView::type_into(typed, cx));
            }
        }
        cx.stop_propagation();
    }

    /// Does what `key`, one that [`steers`], does with the choice.
    fn steer(&mut self, key: &str, held: Modifiers, cx: &mut Context<Self>) {
        match key {
            "enter" => self.enter(cx),
            "left" => self.arrow(Toward::Left, cx),
            "right" => self.arrow(Toward::Right, cx),
            "up" => self.arrow(Toward::Up, cx),
            "down" => self.arrow(Toward::Down, cx),
            "tab" => self.step(held.shift, cx),
            _ => {
                if let Some(chosen) = self.chosen {
                    self.close_window(chosen, cx);
                }
            }
        }
    }

    /// Enter: does the chosen answer while answers stand in the place of
    /// the tiles, or brings the chosen window in front.
    fn enter(&mut self, cx: &mut Context<Self>) {
        if self.answering() {
            self.run_answer(self.answer_chosen, cx);
        } else if let Some(chosen) = self.chosen {
            self.choose(chosen, cx);
        }
    }

    /// Moves the choice to the tile `toward` the chosen one, or among the
    /// answers with the arrows up and down.
    fn arrow(&mut self, toward: Toward, cx: &mut Context<Self>) {
        if self.answering() {
            let count = self.answers.len();
            if count > 0 {
                self.answer_chosen = match toward {
                    Toward::Up => (self.answer_chosen + count - 1) % count,
                    Toward::Down => (self.answer_chosen + 1) % count,
                    _ => self.answer_chosen,
                };
                cx.notify();
            }
            return;
        }
        let Some(chosen) = self.chosen.or_else(|| self.order.first().copied()) else {
            return;
        };
        let places: Vec<ScreenRect> = self
            .order
            .iter()
            .map(|&index| self.tiles[index].place)
            .collect();
        let from = self
            .order
            .iter()
            .position(|&index| index == chosen)
            .unwrap_or(0);
        let to = nearest(from, &places, toward).unwrap_or(from);
        self.chosen = Some(self.order[to]);
        cx.notify();
    }

    /// Moves the choice to the next tile in order, or the one before with
    /// `back`.
    fn step(&mut self, back: bool, cx: &mut Context<Self>) {
        let count = self.order.len();
        if count == 0 {
            return;
        }
        let at =
            { self.chosen }.and_then(|chosen| self.order.iter().position(|&index| index == chosen));
        let next = match (at, back) {
            (None, _) => 0,
            (Some(at), false) => (at + 1) % count,
            (Some(at), true) => (at + count - 1) % count,
        };
        self.chosen = Some(self.order[next]);
        cx.notify();
    }
}

/// Whether `key`, with `held`, steers the choice: Enter, the arrows and
/// Tab move or take it, Delete and Ctrl+W close the chosen window.
fn steers(key: &str, held: Modifiers) -> bool {
    matches!(
        key,
        "enter" | "left" | "right" | "up" | "down" | "tab" | "delete"
    ) || (key == "w" && held.control)
}

/// What the button of the workspace at `index` says, with the word for
/// a workspace before a plain number.
fn workspace_label(index: usize, language: Language, cx: &App) -> SharedString {
    let label = Workspaces::label(index, cx);
    match label.chars().all(|c| c.is_ascii_digit()) {
        true => format!("{} {label}", language.say("Workspace")).into(),
        false => label.into(),
    }
}

/// Where the parts of a window of Task View stand, in physical pixels of
/// the window: the cards of the workspaces, the line of what is typed,
/// and the room the tiles are spread over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Plan {
    strip: Option<ScreenRect>,
    field: Option<ScreenRect>,
    area: ScreenRect,
}

impl Default for Plan {
    /// Nothing anywhere, until the window knows its size.
    fn default() -> Self {
        Self {
            strip: None,
            field: None,
            area: NOWHERE,
        }
    }
}

impl Plan {
    /// The parts of a window `size` physical pixels large at `scale`,
    /// with the cards of the workspaces where `strip` says so and the
    /// line of what is typed where `field` does.
    fn of((width, height): (i32, i32), scale: f32, strip: bool, field: bool) -> Self {
        let physical = |length: f32| (length * scale).round() as i32;
        let padding = physical(PADDING).min(width / 8);
        let mut top = physical(TOP);
        let strip = strip.then(|| {
            let rect = ScreenRect {
                left: padding,
                top,
                right: width - padding,
                bottom: top + physical(CARD.1),
            };
            top = rect.bottom + physical(BELOW);
            rect
        });
        let field = field.then(|| {
            let field_width = physical(FIELD.0).min(width - padding * 2);
            let left = (width - field_width) / 2;
            let rect = ScreenRect {
                left,
                top,
                right: left + field_width,
                bottom: top + physical(FIELD.1),
            };
            top = rect.bottom + physical(BELOW);
            rect
        });
        // The first row of tiles has its titles above it.
        let area_top = (top + physical(TITLE)).min(height - 1);
        let area = ScreenRect {
            left: padding,
            top: area_top,
            right: (width - padding).max(padding + 1),
            bottom: (height - padding).max(area_top + 1),
        };
        Self { strip, field, area }
    }
}

/// The places of `count` cards of workspaces in `strip`, side by side in
/// its middle, narrower where they do not fit.
fn card_places(strip: Option<ScreenRect>, count: usize, scale: f32) -> Vec<ScreenRect> {
    let Some(strip) = strip.filter(|_| count > 0) else {
        return Vec::new();
    };
    let physical = |length: f32| (length * scale).round() as i32;
    let gap = physical(CARD_GAP);
    let room = strip.right - strip.left;
    let fitting = (room - gap * (count as i32 - 1)) / count as i32;
    let width = physical(CARD.0).min(fitting).max(1);
    let whole = width * count as i32 + gap * (count as i32 - 1);
    let left = strip.left + (room - whole) / 2;
    (0..count as i32)
        .map(|at| ScreenRect {
            left: left + at * (width + gap),
            top: strip.top,
            right: left + at * (width + gap) + width,
            bottom: strip.bottom,
        })
        .collect()
}

/// `area` cut into `count` bands one under the other, `gap` apart, each
/// with the room of its label, `label` high, above the room of its
/// tiles: the labels and the rooms.
fn bands(area: ScreenRect, count: usize, label: i32, gap: i32) -> Vec<(ScreenRect, ScreenRect)> {
    if count == 0 {
        return Vec::new();
    }
    let height = area.bottom - area.top;
    let each = ((height - gap * (count as i32 - 1)) / count as i32).max(label + 1);
    (0..count as i32)
        .map(|at| {
            let top = area.top + at * (each + gap);
            let heading = ScreenRect {
                left: area.left,
                top,
                right: area.right,
                bottom: top + label,
            };
            let room = ScreenRect {
                left: area.left,
                // Where there are labels, the titles of the first row of
                // a band stand under them.
                top: top + label + if label > 0 { label } else { 0 },
                right: area.right,
                bottom: top + each,
            };
            let room = ScreenRect {
                bottom: room.bottom.max(room.top + 1),
                ..room
            };
            (heading, room)
        })
        .collect()
}

impl Render for Screen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let visibility = self.visibility.value(window).clamp(0., 1.);
        if self.closing && self.visibility.is_settled() {
            // The pictures and the dimming go now, not when the window
            // is opened again.
            for tile in &self.tiles {
                tile.hide_picture();
            }
            self.tiles.clear();
            self.order.clear();
            self.cards.clear();
            self.card_places.clear();
            self.bands.clear();
            self.answers.clear();
            self.veil = None;
            retire_floating(window, cx);
            return div().id("taskview");
        }
        if let Some(veil) = &self.veil {
            veil.darken(DIM * visibility);
        }
        // The labels come once the pictures are nearly in place, and go
        // first.
        let labels = match self.closing {
            true => visibility * visibility,
            false => ((visibility - 0.5) * 2.).clamp(0., 1.),
        };
        let language = Language::of(cx);
        let mut children: Vec<AnyElement> = Vec::new();
        let dragged = { self.press }
            .filter(|press| press.dragging)
            .and_then(|press| match press.on {
                Hit::Tile(index) => Some(index),
                _ => None,
            });
        let drop_card = match (dragged, self.hovered) {
            (Some(_), Hit::Card(card)) => Some(card),
            _ => None,
        };
        // The cards of the workspaces.
        for at in 0..self.cards.len().min(self.card_places.len()) {
            let target = drop_card == Some(at);
            children.push(self.card(at, target, &theme, language, labels));
        }
        // The line of what is typed.
        if let Some(field) = self.plan.field {
            children.push(self.field(field, &theme, language, labels));
        }
        if self.answering() {
            children.extend(self.answer_list(&theme, language, labels));
        } else if self.main && self.total == 0 && self.typed.is_empty() {
            children.push(self.note(language.say("No windows are open"), &theme, labels));
        }
        for (heading, label) in &self.bands {
            children.push(
                self.place(*heading)
                    .flex()
                    .items_center()
                    .font_weight(FontWeight::MEDIUM)
                    .opacity(labels)
                    .child(
                        div()
                            .px(theme.spacing.medium)
                            .py(theme.spacing.tiny)
                            .rounded(theme.shape.full)
                            .bg(theme.colors.surface_container)
                            .child(one_line(label.clone())),
                    )
                    .into_any_element(),
            );
        }
        // The tiles: the mark of the chosen one and of the one under the
        // pointer, the title, and the card of a window without a picture.
        let order = self.order.clone();
        let pointer = self.pointer;
        let mut in_order = vec![false; self.tiles.len()];
        for &index in &order {
            in_order[index] = true;
        }
        for (index, tile) in self.tiles.iter().enumerate() {
            if !in_order[index] {
                tile.hide_picture();
            }
        }
        for &index in &order {
            let rect = match dragged == Some(index) {
                true => self.dragged_place(self.tiles[index].place, pointer),
                false => self.tiles[index].now(window),
            };
            let tile = &self.tiles[index];
            let opacity = match tile.closing {
                true => 0.5,
                false => 1.,
            };
            if let Some((thumbnail, _)) = &tile.picture {
                thumbnail.place(rect, opacity);
            }
            let chosen = self.chosen == Some(index);
            let hovered = matches!(self.hovered, Hit::Tile(at) | Hit::Close(at) if at == index);
            let settled = labels * if dragged == Some(index) { 0. } else { 1. };
            if (chosen || hovered) && dragged != Some(index) {
                let ring = self.physical(RING);
                let color = match chosen {
                    true => theme.colors.primary,
                    false => theme.colors.outline,
                };
                let around = ScreenRect {
                    left: rect.left - ring,
                    top: rect.top - ring,
                    right: rect.right + ring,
                    bottom: rect.bottom + ring,
                };
                children.push(
                    self.place(around)
                        .rounded(theme.shape.medium)
                        .border_color(color)
                        .border(px(RING))
                        .opacity(settled)
                        .into_any_element(),
                );
            }
            if tile.picture.is_none() {
                children.push(self.icon_card(tile, rect, &theme, visibility * opacity));
            }
            if settled > 0. {
                let lit = chosen || hovered;
                children.push(self.title(index, rect, lit, &theme, settled * opacity));
            }
        }
        div()
            .id("taskview")
            .size_full()
            .relative()
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface)
            // Clicks on no picture reach the window everywhere.
            .bg(theme.colors.surface_container.opacity(0.01))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .on_mouse_move(cx.listener(|this, event, _, cx| this.mouse_move(event, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, _, cx| this.mouse_down(event, cx)),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, event, _, cx| this.mouse_down(event, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event, _, cx| this.mouse_up(event, cx)),
            )
            .children(children)
    }
}

impl Screen {
    /// Where the picture of a tile at `place` is drawn while it is dragged
    /// with the pointer at `pointer`: smaller, below and to the right of
    /// the pointer, and below the cards, so that the card it is dropped
    /// on and its mark stay seen.
    fn dragged_place(&self, place: ScreenRect, pointer: (i32, i32)) -> ScreenRect {
        let rect = scaled(place, DRAG_SCALE);
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        let (most_wide, most_high) = (self.physical(CARD.0), self.physical(CARD.1));
        let fit = { most_wide as f32 / width.max(1) as f32 }
            .min(most_high as f32 / height.max(1) as f32)
            .min(1.);
        let (width, height) = (
            (width as f32 * fit).round() as i32,
            (height as f32 * fit).round() as i32,
        );
        let offset = self.physical(DRAG_OFFSET);
        let below_cards = self
            .plan
            .strip
            .map_or(i32::MIN, |strip| strip.bottom + offset);
        let (left, top) = (pointer.0 + offset, (pointer.1 + offset).max(below_cards));
        ScreenRect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }

    /// A box standing at `rect`, physical pixels of the window.
    fn place(&self, rect: ScreenRect) -> gpui::Div {
        div()
            .absolute()
            .left(self.logical(rect.left))
            .top(self.logical(rect.top))
            .w(self.logical(rect.right - rect.left))
            .h(self.logical(rect.bottom - rect.top))
    }

    /// The card of the workspace at `at` of the cards, marked as where
    /// the picture dragged goes where it is the `target`.
    fn card(
        &self,
        at: usize,
        target: bool,
        theme: &Theme,
        language: Language,
        opacity: f32,
    ) -> AnyElement {
        let (card, place) = (&self.cards[at], self.card_places[at]);
        let hovered = target || self.hovered == Hit::Card(at);
        let colors = theme.colors;
        let ground = match card.current {
            true => colors.secondary_container,
            false => colors.surface_container,
        };
        let ground = match hovered {
            true => state_layer(theme, ground, HOVERED),
            false => ground,
        };
        let icons: Vec<AnyElement> = { card.icons.iter() }
            .map(|icon| small_icon(theme, icon.clone(), CARD_ICON))
            .collect();
        let body = match icons.is_empty() {
            true => div()
                .text_color(colors.on_surface_variant)
                .child(language.say("Empty")),
            false => div()
                .flex()
                .flex_wrap()
                .gap(theme.spacing.tiny)
                .children(icons),
        };
        self.place(place)
            .p(theme.spacing.medium)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .overflow_hidden()
            .rounded(theme.shape.large)
            .bg(ground)
            .border_2()
            .border_color(match target {
                true => colors.primary,
                false => colors.primary.opacity(0.),
            })
            .opacity(opacity)
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .min_w_0()
                    .child(one_line(card.label.clone())),
            )
            .child(body)
            .into_any_element()
    }

    fn field(
        &self,
        field: ScreenRect,
        theme: &Theme,
        language: Language,
        opacity: f32,
    ) -> AnyElement {
        let colors = theme.colors;
        let (text, color) = match self.typed.is_empty() {
            true => (
                language.say("Type to search").to_owned(),
                colors.on_surface_variant,
            ),
            false => (self.typed.clone(), colors.on_surface),
        };
        self.place(field)
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .rounded(theme.shape.full)
            .bg(colors.surface_container_highest)
            .opacity(opacity)
            .child(
                icon(theme, GLYPH_SEARCH)
                    .flex_none()
                    .text_color(colors.on_surface_variant),
            )
            .child(div().min_w_0().text_color(color).child(one_line(text)))
            .into_any_element()
    }

    fn answer_list(&self, theme: &Theme, language: Language, opacity: f32) -> Vec<AnyElement> {
        let colors = theme.colors;
        let rows = self.answer_rows();
        if self.answers.is_empty() {
            if !self.asked {
                return Vec::new();
            }
            let Some(&row) = rows.first() else {
                return Vec::new();
            };
            return vec![self.note_at(row, language.say("Nothing matches"), theme, opacity)];
        }
        { self.answers.iter().zip(rows).enumerate() }
            .map(|(index, (answer, row))| {
                let chosen = index == self.answer_chosen;
                let hovered = self.hovered == Hit::Answer(index);
                let ground = match (chosen, hovered) {
                    (true, _) => colors.secondary_container,
                    (false, true) => colors.surface_container_highest,
                    (false, false) => colors.surface_container,
                };
                self.place(row)
                    .px(theme.spacing.medium)
                    .flex()
                    .items_center()
                    .gap(theme.spacing.medium)
                    .rounded(theme.shape.medium)
                    .bg(ground)
                    .opacity(opacity)
                    .child(icon(theme, answer.glyph).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(one_line(answer.title.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(ANSWERS_WIDTH / 3.))
                            .text_color(colors.on_surface_variant)
                            .child(one_line(answer.detail.clone())),
                    )
                    .into_any_element()
            })
            .collect()
    }

    /// A line of text in the middle of the room of the tiles.
    fn note(&self, text: &'static str, theme: &Theme, opacity: f32) -> AnyElement {
        self.note_at(self.plan.area, text, theme, opacity)
    }

    fn note_at(
        &self,
        rect: ScreenRect,
        text: &'static str,
        theme: &Theme,
        opacity: f32,
    ) -> AnyElement {
        self.place(rect)
            .flex()
            .items_center()
            .justify_center()
            .opacity(opacity)
            .child(
                div()
                    .px(theme.spacing.medium)
                    .py(theme.spacing.small)
                    .rounded(theme.shape.full)
                    .bg(theme.colors.surface_container)
                    .text_color(theme.colors.on_surface_variant)
                    .child(text),
            )
            .into_any_element()
    }

    /// The card of a window that has no picture: its icon on a plain
    /// ground, where the picture would be.
    fn icon_card(&self, tile: &Tile, rect: ScreenRect, theme: &Theme, opacity: f32) -> AnyElement {
        self.place(rect)
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container_highest)
            .opacity(opacity)
            .child(small_icon(theme, tile.seen.icon.clone(), CARD_ICON_SIDE))
            .into_any_element()
    }

    /// The row above the picture at `rect`: the icon and the title of the
    /// window, and with `lit` the button that closes it.
    fn title(
        &self,
        index: usize,
        rect: ScreenRect,
        lit: bool,
        theme: &Theme,
        opacity: f32,
    ) -> AnyElement {
        let tile = &self.tiles[index];
        let row = self.title_row(rect);
        let colors = theme.colors;
        let close = lit.then(|| {
            let hovered = self.hovered == Hit::Close(index);
            div()
                .size(px(CLOSE))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme.shape.full)
                .bg(match hovered {
                    true => colors.surface_container_highest,
                    false => colors.surface_container,
                })
                .child(icon(theme, GLYPH_CLOSE).text_size(theme.typography.label_size))
        });
        self.place(row)
            .flex()
            .items_center()
            .justify_center()
            .opacity(opacity)
            .child(
                div()
                    .max_w_full()
                    .h_full()
                    .pl(theme.spacing.small)
                    .pr(theme.spacing.tiny)
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .rounded(theme.shape.full)
                    .bg(colors.surface_container)
                    .child(small_icon(theme, tile.seen.icon.clone(), 18.))
                    .child(div().min_w_0().child(one_line(tile.seen.title.clone())))
                    .children(close),
            )
            .into_any_element()
    }
}

/// `icon` `side` logical pixels large, or a blank square where there is
/// none.
fn small_icon(theme: &Theme, icon: Icon, side: f32) -> AnyElement {
    match icon {
        Some(icon) => img(icon).size(px(side)).flex_none().into_any_element(),
        None => div()
            .size(px(side))
            .flex_none()
            .rounded(theme.shape.small)
            .bg(theme.colors.outline)
            .into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_actions_name_the_windows_they_show() {
        assert_eq!(Mode::of(None), Some(Mode::Workspace));
        assert_eq!(Mode::of(Some("workspace")), Some(Mode::Workspace));
        assert_eq!(Mode::of(Some("all")), Some(Mode::All));
        assert_eq!(Mode::of(Some("app")), Some(Mode::App));
        assert_eq!(Mode::of(Some("desktop")), None);
        let action = TaskView::action(true).by_default("win+tab", None);
        assert!(action.takes_reserved_keys);
        assert_eq!(action.accepts("taskview", Some("all")), Ok(()));
        assert_eq!(
            action.accepts("taskview", Some("up")),
            Err(MODES.to_owned())
        );
        assert_eq!(action.default_keys, [("win+tab".to_owned(), None)]);
        assert!(TaskView::action(false).default_keys.is_empty());
    }

    #[test]
    fn the_parts_of_a_display_stand_one_under_the_other() {
        let plan = Plan::of((1920, 1080), 1., true, true);
        let strip = plan.strip.unwrap();
        let field = plan.field.unwrap();
        assert_eq!((strip.top, strip.bottom), (20, 112));
        assert_eq!((field.top, field.bottom), (128, 168));
        assert_eq!(field.left + field.right, 1920);
        assert_eq!(plan.area.top, 168 + 16 + 30);
        assert_eq!(
            (plan.area.left, plan.area.right, plan.area.bottom),
            (48, 1872, 1032)
        );
        let bare = Plan::of((1920, 1080), 2., false, false);
        assert_eq!((bare.strip, bare.field), (None, None));
        assert_eq!(bare.area.top, 40 + 60);
        // A display too small still has room for a tile.
        let tiny = Plan::of((100, 50), 2., true, true);
        assert!(tiny.area.right > tiny.area.left && tiny.area.bottom > tiny.area.top);
    }

    #[test]
    fn the_cards_of_the_workspaces_stand_in_the_middle_and_shrink_to_fit() {
        let strip = ScreenRect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 92,
        };
        let three = card_places(Some(strip), 3, 1.);
        assert_eq!(three.len(), 3);
        assert_eq!(three[0].right - three[0].left, 176);
        assert_eq!(three[0].left + three[2].right, 1000);
        assert_eq!(three[1].left - three[0].right, 12);
        let nine = card_places(Some(strip), 9, 1.);
        assert!(nine[8].right <= 1000 && nine[0].left >= 0);
        assert!(card_places(None, 3, 1.).is_empty());
        assert!(card_places(Some(strip), 0, 1.).is_empty());
    }

    #[test]
    fn the_groups_of_all_workspaces_get_bands_of_their_own() {
        let area = ScreenRect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 920,
        };
        let three = bands(area, 3, 30, 20);
        assert_eq!(three.len(), 3);
        let (heading, room) = three[0];
        assert_eq!((heading.top, heading.bottom), (0, 30));
        assert_eq!((room.top, room.bottom), (60, 293));
        assert_eq!(three[1].0.top, 313);
        // One group without a label takes the whole room.
        let [(_, room)] = bands(area, 1, 0, 20)[..] else {
            panic!("one band");
        };
        assert_eq!(room, area);
        assert!(bands(area, 0, 30, 20).is_empty());
    }
}
