// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Keeps one extension running for all of its widgets, its answers to the
//! launcher and its shortcuts.
//!
//! An extension runs on a thread of its own ([`Worker`]), so that what it
//! computes and what it asks of Windows never holds up the thread that
//! draws. Calls are sent there in order and their outcome comes back with
//! the requests the extension started and what it asked to be done, which
//! the thread that draws then sends and does ([`Host::finish`]).

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use gpui::{AnyWindowHandle, App, AppContext, AsyncApp, Context, Entity, Pixels, WeakEntity};

use super::access::{SystemKind, WatchKind};
use super::http::{self, HttpRequest};
use super::nodes::Content;
use super::runtime::{Effect, Instance, NumberedRequest};
use super::system::{self, ShellReader};
use super::widget::ExtensionWidget;
use crate::actions::Actions;
use crate::banners::{Banners, Notice, OnGone};
use crate::platform::DisplayKey;
use crate::popup::{PopupAnchor, Popups};
use crate::shell::Shell;

/// How often what widgets watch is looked at: changes within this time
/// are drawn together, at most four times a second.
const WATCH_TICK: Duration = Duration::from_millis(250);

/// The icon of notices that extensions post.
const ICON_EXTENSION: &str = "\u{ea86}";

/// What a call into an extension came to: its value, and what the
/// extension started meanwhile.
pub struct Called<T> {
    pub value: T,
    pub requests: Vec<NumberedRequest>,
    pub effects: Vec<Effect>,
}

/// Why a call fails that is made after one ended the extension.
pub const STOPPED: &str = "it stopped before";

/// A call that was made, or why the extension stopped.
pub type Outcome<T> = Result<Called<T>, String>;

type Job = Box<dyn FnOnce(&mut Option<Instance>) + Send>;

/// The thread an extension runs on. Clones send to the same thread, which
/// ends once every clone is gone.
#[derive(Clone)]
pub struct Worker {
    jobs: mpsc::Sender<Job>,
}

impl Worker {
    /// Starts the thread of the extension `id`, which runs `instance`.
    pub fn start(id: &str, instance: Instance) -> anyhow::Result<Self> {
        let (jobs, received) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name(format!("extension {id}"))
            .spawn(move || {
                let mut instance = Some(instance);
                while let Ok(job) = received.recv() {
                    job(&mut instance);
                }
            })?;
        Ok(Self { jobs })
    }

    /// Has the thread do `call` and hands what it came to to `reply`, on
    /// that thread. A call that fails ends the extension: the calls after
    /// it fail too, with [`STOPPED`].
    pub fn send<T: Send + 'static>(
        &self,
        call: impl FnOnce(&mut Instance) -> anyhow::Result<T> + Send + 'static,
        reply: impl FnOnce(Outcome<T>) + Send + 'static,
    ) {
        let job: Job = Box::new(move |slot| {
            let outcome = match slot.as_mut() {
                None => Err(STOPPED.to_owned()),
                Some(instance) => match call(instance) {
                    Ok(value) => Ok(Called {
                        value,
                        requests: instance.take_requests(),
                        effects: instance.take_effects(),
                    }),
                    Err(error) => {
                        *slot = None;
                        Err(format!("{error:#}"))
                    }
                },
            };
            reply(outcome);
        });
        // A thread that is gone drops the job, and with it the reply.
        self.jobs.send(job).ok();
    }

    /// Does `call` on the thread; the future gives what it came to.
    pub fn call<T, F>(&self, call: F) -> impl Future<Output = Outcome<T>> + use<T, F>
    where
        T: Send + 'static,
        F: FnOnce(&mut Instance) -> anyhow::Result<T> + Send + 'static,
    {
        let (sender, receiver) = async_channel::bounded(1);
        self.send(call, move |outcome| {
            sender.try_send(outcome).ok();
        });
        async move {
            let ended = || "it has ended".to_owned();
            receiver.recv().await.unwrap_or_else(|_| Err(ended()))
        }
    }
}

/// An extension as it was loaded: its thread, and the host that the first
/// of its widgets, shortcuts or chosen answers makes on the thread that
/// draws.
pub struct Loaded {
    id: String,
    worker: Worker,
    /// Whether it may read what only the thread that draws knows.
    reads_shell: bool,
    host: OnceCell<Entity<Host>>,
}

thread_local! {
    /// The extensions loaded last, by identifier. They are loaded and used
    /// on the thread that draws.
    static LOADED: RefCell<HashMap<String, Rc<Loaded>>> = RefCell::default();
}

impl Loaded {
    pub fn new(id: &str, worker: Worker, permits: &[SystemKind]) -> Rc<Self> {
        let reads = [
            SystemKind::Windows,
            SystemKind::WindowControl,
            SystemKind::Workspaces,
            SystemKind::Theme,
        ];
        Rc::new(Self {
            id: id.to_owned(),
            worker,
            reads_shell: reads.iter().any(|kind| permits.contains(kind)),
            host: OnceCell::new(),
        })
    }

    /// The host of the extension, made the first time it is asked for.
    pub fn host(&self, cx: &mut App) -> Entity<Host> {
        self.host
            .get_or_init(|| {
                cx.new(|cx| Host::new(self.id.clone(), self.worker.clone(), self.reads_shell, cx))
            })
            .clone()
    }

    /// Forgets the extensions that were loaded, before they are loaded
    /// again; they end once nothing shows them any more.
    pub fn clear() {
        LOADED.with_borrow_mut(HashMap::clear);
    }

    /// Makes `loaded` one of the running extensions.
    pub fn add(loaded: Rc<Self>) {
        LOADED.with_borrow_mut(|running| running.insert(loaded.id.clone(), loaded));
    }

    /// The running extension `id`.
    pub fn get(id: &str) -> Option<Rc<Self>> {
        LOADED.with_borrow(|running| running.get(id).cloned())
    }
}

/// The host of the running extension `id`, made where it was not yet; for
/// the settings, which hand a changed table to
/// [`Host::update_settings`].
pub fn running(id: &str, cx: &mut App) -> Option<Entity<Host>> {
    Some(Loaded::get(id)?.host(cx))
}

/// Where what an extension opens is anchored.
#[derive(Clone, Copy)]
pub enum Place {
    /// At this position of the window of a widget, which was acted on.
    Widget(AnyWindowHandle, Pixels),
    /// At the first widget of the extension on the display the pointer is
    /// on, or else in the middle of a bar of that display.
    Pointer,
}

/// A widget of the extension that is placed somewhere.
struct Placed {
    widget: WeakEntity<ExtensionWidget>,
    watch: Vec<WatchKind>,
}

/// A running extension, as the thread that draws sees it. It notifies its
/// observers whenever the content of its widgets may have changed.
pub struct Host {
    /// The extension's identifier, for messages and popups.
    id: String,
    worker: Worker,
    /// Whether a call into the extension has failed, which ended it.
    failed: bool,
    widgets: Vec<Placed>,
    /// The changes of each watched kind seen last.
    seen: HashMap<WatchKind, u64>,
    /// Has the media and the meters read while widgets watch them.
    watch: Option<system::Watch>,
    /// Whether what widgets watch is being looked at.
    ticking: bool,
    /// Has what only the thread that draws knows written down, for an
    /// extension that may read it.
    shell: Option<ShellReader>,
    /// What was said already about the content it described.
    reported: HashSet<String>,
}

impl Host {
    fn new(id: String, worker: Worker, reads_shell: bool, cx: &mut Context<Self>) -> Self {
        Self {
            id,
            worker,
            failed: false,
            widgets: Vec::new(),
            seen: HashMap::new(),
            watch: None,
            ticking: false,
            shell: reads_shell.then(|| ShellReader::new(cx)),
            reported: HashSet::new(),
        }
    }

    /// Whether the extension has failed and shows nothing any more.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// Keeps `widget` among those of the extension, drawn again whenever
    /// what `watch` names changes.
    pub fn add_widget(
        &mut self,
        widget: WeakEntity<ExtensionWidget>,
        watch: &[WatchKind],
        cx: &mut Context<Self>,
    ) {
        self.widgets.push(Placed {
            widget,
            watch: watch.to_vec(),
        });
        if watch.is_empty() {
            return;
        }
        for &kind in watch {
            self.seen
                .entry(kind)
                .or_insert_with(|| system::generation(kind));
        }
        system::start_watching(watch);
        self.watch_now();
        self.tick_on(cx);
    }

    /// Has what the widgets watch now read and written down.
    fn watch_now(&mut self) {
        let watched = self.watched();
        let shell = [WatchKind::Windows, WatchKind::Workspaces, WatchKind::Theme];
        if let Some(reader) = &mut self.shell {
            reader.watch(shell.iter().any(|kind| watched.contains(kind)));
        }
        self.watch = (!watched.is_empty()).then(|| system::Watch::new(&watched));
    }

    /// Every kind a widget watches.
    fn watched(&self) -> Vec<WatchKind> {
        let all = self.widgets.iter().flat_map(|placed| placed.watch.iter());
        let mut kinds: Vec<WatchKind> = all.copied().collect();
        kinds.sort_by_key(|kind| WatchKind::ALL.iter().position(|known| known == kind));
        kinds.dedup();
        kinds
    }

    /// Looks at what is watched every [`WATCH_TICK`] for as long as a
    /// widget watches something, and draws the widgets
    /// that watch what changed.
    fn tick_on(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(WATCH_TICK).await;
                let Ok(Some(due)) = this.update(cx, |host, _| host.tick()) else {
                    break;
                };
                for widget in due {
                    widget.update(cx, |widget, cx| widget.refresh(cx)).ok();
                }
            }
        })
        .detach();
    }

    /// Says which widgets watch what changed since the last look, or
    /// `None` once no widget watches anything, which ends the looks.
    fn tick(&mut self) -> Option<Vec<WeakEntity<ExtensionWidget>>> {
        let before = self.widgets.len();
        self.widgets
            .retain(|placed| placed.widget.upgrade().is_some());
        if self.widgets.len() != before {
            self.watch_now();
        }
        if self.watch.is_none() {
            self.ticking = false;
            return None;
        }
        let mut changed = Vec::new();
        for kind in self.watched() {
            let now = system::generation(kind);
            if self.seen.insert(kind, now) != Some(now) {
                changed.push(kind);
            }
        }
        if changed.is_empty() || self.failed {
            return Some(Vec::new());
        }
        let due = self
            .widgets
            .iter()
            .filter(|placed| placed.watch.iter().any(|kind| changed.contains(kind)));
        Some(due.map(|placed| placed.widget.clone()).collect())
    }

    /// Does `call` on the thread of the extension.
    pub fn call<T, F>(&self, call: F) -> impl Future<Output = Outcome<T>> + use<T, F>
    where
        T: Send + 'static,
        F: FnOnce(&mut Instance) -> anyhow::Result<T> + Send + 'static,
    {
        self.worker.call(call)
    }

    /// The content of `widget` now, checked, and what is wrong with it.
    pub fn render(
        &self,
        widget: &str,
    ) -> impl Future<Output = Outcome<(Content, Vec<String>)>> + use<> {
        let widget = widget.to_owned();
        self.worker.call(move |instance| {
            let nodes = instance.render(&widget)?;
            Ok(Content::checked(nodes, &instance.access().permissions))
        })
    }

    /// Says what is wrong with the content the extension described, once
    /// each.
    pub fn complain(&mut self, widget: &str, problems: Vec<String>) {
        for problem in problems {
            let problem = format!("the widget `{}.{widget}` shows less: {problem}", self.id);
            if self.reported.insert(problem.clone()) {
                report!("{problem}");
            }
        }
    }

    /// Takes in what a call came to: sends the requests the extension
    /// started and does what it asked to be done, anchored at `place`.
    /// Its value is given back, or `None` when the call ended the
    /// extension.
    pub fn finish<T>(
        host: &Entity<Self>,
        outcome: Outcome<T>,
        place: Place,
        cx: &mut App,
    ) -> Option<T> {
        let called = match outcome {
            Ok(called) => called,
            Err(error) => {
                host.update(cx, |host, cx| host.stop(&error, cx));
                return None;
            }
        };
        for (number, request) in called.requests {
            Self::send_request(host.downgrade(), number, request, cx);
        }
        for effect in called.effects {
            Self::apply(host, effect, place, cx);
        }
        Some(called.value)
    }

    /// Reports why the extension stopped, once, and has its widgets say
    /// so.
    fn stop(&mut self, error: &str, cx: &mut Context<Self>) {
        if !self.failed {
            report!("the extension `{}` stopped: {error}", self.id);
        }
        self.failed = true;
        cx.notify();
    }

    /// Does `call` on the thread of the extension, takes in what it came
    /// to and draws the widgets again.
    fn call_and_draw(
        &mut self,
        call: impl FnOnce(&mut Instance) -> anyhow::Result<()> + Send + 'static,
        place: Place,
        cx: &mut Context<Self>,
    ) {
        let call = self.worker.call(call);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let outcome = call.await;
            cx.update(|cx| {
                let Some(host) = this.upgrade() else {
                    return;
                };
                Self::finish(&host, outcome, place, cx);
                host.update(cx, |_, cx| cx.notify());
            })
            .ok();
        })
        .detach();
    }

    /// Takes over `settings` as the extension's table of the
    /// configuration: `setting` answers with it from now on, the
    /// extension is told where it listens, and its widgets are drawn
    /// again.
    pub fn update_settings(&mut self, settings: HashMap<String, String>, cx: &mut Context<Self>) {
        self.call_and_draw(
            move |instance| instance.settings_changed(settings).map(drop),
            Place::Pointer,
            cx,
        );
    }

    /// Tells the extension that the keys of its shortcut `id` were
    /// pressed.
    pub fn shortcut(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_owned();
        self.call_and_draw(move |instance| instance.shortcut(&id), Place::Pointer, cx);
    }

    /// Tells the extension that its answer `id` was chosen in the
    /// launcher.
    pub fn run_answer(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_owned();
        self.call_and_draw(move |instance| instance.run_answer(&id), Place::Pointer, cx);
    }

    /// Sends `request` in the background and gives the extension its
    /// outcome.
    fn send_request(host: WeakEntity<Self>, number: u32, request: HttpRequest, cx: &mut App) {
        cx.spawn(async move |cx: &mut AsyncApp| {
            let outcome = cx
                .background_executor()
                .spawn(async move { http::send(&request) })
                .await;
            let fetched = host.read_with(cx, |host, _| {
                host.worker
                    .call(move |instance| instance.fetched(number, outcome))
            });
            let Ok(fetched) = fetched else {
                return;
            };
            let outcome = fetched.await;
            cx.update(|cx| {
                let Some(host) = host.upgrade() else {
                    return;
                };
                Self::finish(&host, outcome, Place::Pointer, cx);
                host.update(cx, |_, cx| cx.notify());
            })
            .ok();
        })
        .detach();
    }

    /// Does what the extension asked to be done. Nothing of the host is
    /// borrowed meanwhile, as an action may well ask the extension again.
    fn apply(host: &Entity<Self>, effect: Effect, place: Place, cx: &mut App) {
        let id = host.read(cx).id.clone();
        match effect {
            Effect::OpenPopup(name) => Self::open_popup(host, &format!("{id}.{name}"), place, cx),
            Effect::ClosePopup(name) => {
                if is_open(&format!("{id}.{name}"), cx) {
                    Popups::close(cx);
                }
            }
            Effect::RunAction { name, arg } => {
                let checked = match cx.try_global::<Actions>() {
                    Some(actions) => actions.check(&name, arg.as_deref()),
                    None => Err("there are no actions yet".to_owned()),
                };
                match checked {
                    Ok(()) => Actions::run(&name, arg.as_deref(), cx),
                    Err(error) => report!("the extension `{id}` cannot do `{name}`: {error}"),
                }
            }
            Effect::Notify { title, body } => {
                let notice = Notice {
                    app: id,
                    title,
                    body,
                    glyph: Some(ICON_EXTENSION),
                    actions: Vec::new(),
                    urgent: false,
                    on_gone: OnGone::default(),
                };
                Banners::post(notice, cx);
            }
            Effect::Window(window, deed) => deed.perform(window, cx),
        }
    }

    /// Opens the popup `name`, unless it is open, whether the extension
    /// or a click opened it.
    fn open_popup(host: &Entity<Self>, name: &str, place: Place, cx: &mut App) {
        if is_open(name, cx) {
            return;
        }
        let anchor = match place {
            Place::Widget(window, x) => anchor_in(window, x, cx),
            Place::Pointer => Self::pointer_anchor(host, cx),
        };
        match anchor {
            Some(anchor) => Popups::toggle(name, anchor, cx),
            None => Shell::toggle_popup(name, cx),
        }
    }

    /// The anchor at the first widget of the extension on the display the
    /// pointer is on.
    fn pointer_anchor(host: &Entity<Self>, cx: &mut App) -> Option<PopupAnchor> {
        let display = DisplayKey::under_pointer();
        let widgets = host.read(cx).widgets.iter();
        let widgets: Vec<Entity<ExtensionWidget>> = widgets
            .filter_map(|placed| placed.widget.upgrade())
            .collect();
        let places: Vec<(AnyWindowHandle, Pixels)> = { widgets.iter() }
            .filter_map(|widget| widget.read(cx).place())
            .collect();
        places.into_iter().find_map(|(window, x)| {
            let anchor = anchor_in(window, x, cx)?;
            (display.is_none() || anchor.display == display).then_some(anchor)
        })
    }
}

/// Whether the popup `name` is the one that is open.
fn is_open(name: &str, cx: &App) -> bool {
    Popups::open_name(cx) == Some(name)
}

/// The anchor at `x` of `window`.
fn anchor_in(window: AnyWindowHandle, x: Pixels, cx: &mut App) -> Option<PopupAnchor> {
    let anchor = cx.update_window(window, |_, window, cx| PopupAnchor::at(window, x, cx));
    anchor.ok()?.ok()
}
