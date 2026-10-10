// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Scoped native file selections and immediate Windows effects, outside preference drafts.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::ComponentHandle;
use tessera_system::wallpaper::{
    WallpaperApplyCompletion, WallpaperApplyOutcome, WallpaperApplyScope,
    WallpaperChooseCompletion, WallpaperError, WallpaperHost, WallpaperImageTarget,
    WallpaperMonitorTarget, collection as native_collection, slideshow as native_slideshow,
};

use crate::DesktopHost;
use crate::application_menu::WindowFrame;
use crate::generated::Panel;

mod collection;
mod colors;
mod commands;
mod image;
mod position;
mod presentation;
mod selection;
mod slideshow;

use selection::{PreparedSelection, SelectedImage};

#[derive(Clone, Copy, PartialEq)]
struct Session {
    id: u64,
    frame: WindowFrame,
}

#[derive(Clone, Copy, PartialEq)]
enum Operation {
    Choose,
    Apply,
    ChooseCollection,
    ApplyCollection,
    ReadSlideshow,
    AdvanceSlideshow(native_slideshow::Direction),
}

enum Request {
    Choose,
    Apply {
        target: WallpaperImageTarget,
        scope: WallpaperApplyScope,
        index: i32,
        requested: u32,
    },
    ChooseCollection,
    ApplyCollection {
        target: native_collection::Target,
        options: native_collection::Options,
        index: i32,
    },
    ReadSlideshow,
    AdvanceSlideshow {
        target: native_slideshow::Target,
        monitor: WallpaperMonitorTarget,
        direction: native_slideshow::Direction,
        index: i32,
    },
}

#[derive(Clone)]
struct Flight {
    ticket: u64,
    session: Session,
    operation: Operation,
    scope: Option<WallpaperApplyScope>,
    monitor_index: Option<i32>,
    requested: u32,
    collection_target: Option<native_collection::TargetWeak>,
    collection_options: Option<native_collection::Options>,
    collection_index: Option<i32>,
    slideshow: Option<slideshow::Issued>,
}

enum Reply {
    Chosen(Result<Option<PreparedSelection>, WallpaperError>),
    Applied(Result<WallpaperApplyOutcome, WallpaperError>),
    CollectionChosen(Result<Option<native_collection::Selection>, WallpaperError>),
    CollectionApplied(Result<native_collection::ApplyOutcome, WallpaperError>),
    SlideshowRead(Result<native_slideshow::Observation, WallpaperError>),
    SlideshowAdvanced(Result<native_slideshow::AdvanceOutcome, WallpaperError>),
}

struct Receipt {
    ticket: u64,
    reply: Reply,
}

#[derive(Default)]
struct State {
    sequence: u64,
    exhausted: bool,
    provider_checked: bool,
    provider: Option<Arc<dyn WallpaperHost>>,
    session: Option<Session>,
    selection: Option<SelectedImage>,
    collection: Option<collection::SelectedCollection>,
    slideshow: Option<slideshow::Current>,
    slideshow_notice: String,
    collection_notice: String,
    monitor_captions: Vec<slint::SharedString>,
    monitor_index: i32,
    notice: String,
    flight: Option<Flight>,
}

impl State {
    fn clear_root(
        &mut self,
    ) -> (
        Option<SelectedImage>,
        Option<collection::SelectedCollection>,
        Option<slideshow::Current>,
    ) {
        let (image, collection) = self.clear_files();
        self.slideshow_notice.clear();
        (image, collection, self.slideshow.take())
    }

    fn clear_selection(&mut self) -> Option<SelectedImage> {
        self.monitor_captions.clear();
        self.monitor_index = -1;
        self.selection.take()
    }

    fn clear_files(
        &mut self,
    ) -> (
        Option<SelectedImage>,
        Option<collection::SelectedCollection>,
    ) {
        let image = self.clear_selection();
        let collection = self.collection.take();
        self.collection_notice.clear();
        (image, collection)
    }

    fn next(&mut self) -> Option<u64> {
        let Some(next) = self.sequence.checked_add(1) else {
            self.exhausted = true;
            return None;
        };
        self.sequence = next;
        Some(next)
    }
}

struct ResetFlag<'a>(&'a Cell<bool>);

impl Drop for ResetFlag<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

pub(crate) struct WallpaperController {
    host: Arc<dyn DesktopHost>,
    panel: slint::Weak<Panel>,
    admission: Rc<dyn Fn() -> bool>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Option<Receipt>>>,
    projecting: Cell<bool>,
    acquiring: Cell<bool>,
    submitting: Cell<bool>,
    position: Rc<position::PositionController>,
}

impl WallpaperController {
    pub(crate) fn new(
        host: Arc<dyn DesktopHost>,
        panel: slint::Weak<Panel>,
        admission: Rc<dyn Fn() -> bool>,
    ) -> Rc<Self> {
        let position = position::PositionController::new(panel.clone(), admission.clone());
        let actor = Rc::new(Self {
            host,
            panel,
            admission,
            state: RefCell::new(State::default()),
            mailbox: Arc::default(),
            projecting: Cell::new(false),
            acquiring: Cell::new(false),
            submitting: Cell::new(false),
            position,
        });
        if let Some(panel) = actor.panel.upgrade() {
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_choose_requested(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.request(Operation::Choose);
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_apply_requested(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.request(Operation::Apply);
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_monitor_changed(move |index| {
                if let Some(actor) = weak.upgrade() {
                    actor.monitor_changed(index);
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_colors_requested(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.preview_colors();
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_event_ready(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.receive();
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_collection_choose_requested(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.request(Operation::ChooseCollection);
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_collection_apply_requested(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.request(Operation::ApplyCollection);
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_collection_interval_changed(move |index| {
                if let Some(actor) = weak.upgrade() {
                    actor.collection_options_changed(Some(index));
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_collection_shuffle_changed(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.collection_options_changed(None);
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_slideshow_read_requested(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.request(Operation::ReadSlideshow);
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_slideshow_previous_requested(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.request(Operation::AdvanceSlideshow(
                        native_slideshow::Direction::Backward,
                    ));
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_slideshow_next_requested(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.request(Operation::AdvanceSlideshow(
                        native_slideshow::Direction::Forward,
                    ));
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_slideshow_monitor_changed(move |index| {
                if let Some(actor) = weak.upgrade() {
                    actor.slideshow_monitor_changed(index);
                }
            });
        }
        actor
    }

    fn source(&self) -> Option<(Panel, WindowFrame)> {
        if !(self.admission)() {
            return None;
        }
        let panel = self.panel.upgrade()?;
        if !panel.get_wallpaper_page_visible() {
            return None;
        }
        let frame = WindowFrame::capture(panel.window())?;
        Some((panel, frame))
    }

    fn current(&self, session: Session) -> bool {
        let Some((_, frame)) = self.source() else {
            return false;
        };
        let state = self.state.borrow();
        !state.exhausted && state.session == Some(session) && frame == session.frame
    }

    fn provider_current(&self, provider: Option<&Arc<dyn WallpaperHost>>) -> bool {
        let state = self.state.borrow();
        match (state.provider.as_ref(), provider) {
            (Some(current), Some(provider)) => Arc::ptr_eq(current, provider),
            (None, None) => true,
            _ => false,
        }
    }

    /// Capability discovery is lazy and admitted; refreshing never reads Windows wallpaper.
    pub(crate) fn refresh_root(self: &Rc<Self>) {
        if self.projecting.get() || self.acquiring.get() || self.submitting.get() {
            return;
        }
        let Some((_, frame)) = self.source() else {
            self.stop_root();
            return;
        };
        let revision = self.state.borrow().sequence;
        if !self.state.borrow().provider_checked {
            self.acquiring.set(true);
            let _guard = ResetFlag(&self.acquiring);
            let provider = self.host.wallpaper_host();
            let mut state = self.state.borrow_mut();
            state.provider_checked = true;
            state.provider = provider;
        }
        if self.state.borrow().sequence != revision {
            return;
        }
        // Capability getters can reenter or retire the Root.
        let Some((_, current_frame)) = self.source() else {
            self.stop_root();
            return;
        };
        if frame != current_frame {
            self.stop_root();
            return;
        }
        let (session, retired) = {
            let mut state = self.state.borrow_mut();
            if state.exhausted {
                (None, state.clear_root())
            } else if state.session.is_none_or(|session| session.frame != frame) {
                let retired = state.clear_root();
                state.notice.clear();
                state.session = state.next().map(|id| Session { id, frame });
                (state.session, retired)
            } else {
                (state.session, (None, None, None))
            }
        };
        // Native selection retirement can enqueue work; no state borrow survives it.
        drop(retired);
        let Some(session) = session else {
            self.stop_root();
            return;
        };
        if !self.project(session, None) {
            self.stop_root();
        } else {
            let provider = self.state.borrow().provider.clone();
            self.position.refresh_root(provider);
        }
    }

    /// Retire selection and input, but never cancel or replay an accepted native effect.
    pub(crate) fn stop_root(&self) {
        self.position.stop_root();
        let (revision, retired) = {
            let mut state = self.state.borrow_mut();
            if state.session.take().is_some() || self.acquiring.get() {
                let _ = state.next();
            }
            let retired = state.clear_root();
            state.notice.clear();
            (state.sequence, retired)
        };
        let already_projecting = self.projecting.replace(true);
        drop(retired);
        if already_projecting {
            return;
        }
        let _guard = ResetFlag(&self.projecting);
        let busy = self.state.borrow().flight.is_some();
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        macro_rules! clear {
            ($setter:ident, $value:expr) => {
                if !self.stopped(revision) {
                    return;
                }
                panel.$setter($value);
                if !self.stopped(revision) {
                    return;
                }
            };
        }
        clear!(set_wallpaper_controls_enabled, false);
        clear!(set_wallpaper_apply_enabled, false);
        clear!(set_wallpaper_colors_enabled, false);
        clear!(set_wallpaper_monitor_selection_available, false);
        clear!(set_wallpaper_preview_available, false);
        clear!(set_wallpaper_preview, slint::Image::default());
        clear!(
            set_wallpaper_preview_status,
            "No current image selection.".into()
        );
        clear!(set_wallpaper_color_rgb, -1);
        clear!(
            set_wallpaper_colors_status,
            "Choose a fresh image to preview its native thumbnail colors.".into()
        );
        clear!(set_wallpaper_monitors, slint::ModelRc::default());
        clear!(set_wallpaper_monitor_index, -1);
        clear!(set_wallpaper_input_key, slint::SharedString::default());
        clear!(set_wallpaper_collection_controls_enabled, false);
        clear!(set_wallpaper_collection_apply_enabled, false);
        clear!(set_wallpaper_collection_command_available, false);
        clear!(set_wallpaper_collection_items, slint::ModelRc::default());
        clear!(set_wallpaper_collection_interval_index, -1);
        clear!(set_wallpaper_collection_shuffle, false);
        clear!(
            set_wallpaper_collection_status,
            "No current collection. Native slideshow policy has not been read.".into()
        );
        clear!(set_wallpaper_slideshow_read_enabled, false);
        clear!(set_wallpaper_slideshow_advance_enabled, false);
        clear!(set_wallpaper_slideshow_selector_available, false);
        clear!(set_wallpaper_slideshow_monitors, slint::ModelRc::default());
        clear!(set_wallpaper_slideshow_monitor_index, -1);
        clear!(
            set_wallpaper_slideshow_facts,
            "Native current slideshow policy has not been read.".into()
        );
        clear!(
            set_wallpaper_slideshow_status,
            "Read current explicitly; no monitor is assumed.".into()
        );
        clear!(set_wallpaper_available, false);
        clear!(set_wallpaper_busy, busy);
        clear!(
            set_wallpaper_status,
            "Selection is no longer current. Choose a fresh image.".into()
        );
    }

    fn stopped(&self, revision: u64) -> bool {
        let state = self.state.borrow();
        state.sequence == revision && state.session.is_none()
    }

    // A genuine TileButton callback supplies intent. Re-read actual clipping
    // independently of enabled/busy after every potentially reflowing setter.
    fn intent_current(&self, session: Session, operation: Operation) -> bool {
        if !self.current(session) {
            return false;
        }
        let Some((panel, frame)) = self.source() else {
            return false;
        };
        frame == session.frame
            && Self::focused(&panel)
            && match operation {
                Operation::Choose => {
                    panel.get_wallpaper_choose_input_active()
                        && panel.get_wallpaper_choose_control_visible()
                }
                Operation::Apply => {
                    panel.get_wallpaper_apply_input_active()
                        && panel.get_wallpaper_apply_control_visible()
                        && self.scope_current(&panel)
                }
                Operation::ChooseCollection => {
                    panel.get_wallpaper_collection_choose_input_active()
                        && panel.get_wallpaper_collection_choose_control_visible()
                }
                Operation::ApplyCollection => {
                    panel.get_wallpaper_collection_apply_input_active()
                        && panel.get_wallpaper_collection_apply_control_visible()
                        && self.collection_options_current(&panel)
                }
                Operation::ReadSlideshow => {
                    panel.get_wallpaper_slideshow_read_input_active()
                        && panel.get_wallpaper_slideshow_read_control_visible()
                }
                Operation::AdvanceSlideshow(direction) => {
                    let input = match direction {
                        native_slideshow::Direction::Backward => {
                            panel.get_wallpaper_slideshow_previous_input_active()
                                && panel.get_wallpaper_slideshow_previous_control_visible()
                        }
                        native_slideshow::Direction::Forward => {
                            panel.get_wallpaper_slideshow_next_input_active()
                                && panel.get_wallpaper_slideshow_next_control_visible()
                        }
                    };
                    input && self.slideshow_choice_current(&panel, direction)
                }
            }
    }

    fn scope_current(&self, panel: &Panel) -> bool {
        let index = panel.get_wallpaper_monitor_index();
        let state = self.state.borrow();
        if state.monitor_index != index {
            return false;
        }
        if let Some(flight) = state.flight.as_ref() {
            return state.session == Some(flight.session)
                && flight.operation == Operation::Apply
                && flight.monitor_index == Some(index)
                && flight.scope.is_some();
        }
        state
            .selection
            .as_ref()
            .is_some_and(|selection| selection.scope_at(index).as_ref() == Some(&selection.scope))
    }

    fn monitor_changed(self: &Rc<Self>, index: i32) {
        if self.projecting.get() || self.acquiring.get() || self.submitting.get() {
            return;
        }
        let Some((panel, _)) = self.source() else {
            return;
        };
        let session = self.state.borrow().session;
        let Some(session) = session else { return };
        if !self.current(session) {
            return;
        }
        let ui_index = panel.get_wallpaper_monitor_index();
        let choice = {
            let state = self.state.borrow();
            if state.flight.is_some() {
                return;
            }
            let Some(selection) = state.selection.as_ref() else {
                return;
            };
            if ui_index == index {
                selection
                    .scope_at(index)
                    .and_then(|scope| selection.notice(&scope).map(|notice| (scope, notice)))
            } else {
                None
            }
        };
        let (retired, exhausted) = {
            let mut state = self.state.borrow_mut();
            let retired = if let Some((scope, notice)) = choice {
                let Some(selection) = state.selection.as_mut() else {
                    return;
                };
                selection.scope = scope;
                state.monitor_index = index;
                state.notice = notice;
                None
            } else {
                let retired = state.clear_selection();
                state.notice = "The display scope is invalid. Choose a fresh image; no wallpaper change was requested.".into();
                retired
            };
            (retired, state.next().is_none())
        };
        drop(retired);
        // A fresh scope choice retires held Apply gestures, but never invokes Windows.
        if exhausted || !self.project(session, None) {
            self.stop_root();
        }
    }

    #[cfg(any(windows, test))]
    fn focused(panel: &Panel) -> bool {
        use slint::winit_030::WinitWindowAccessor;
        panel
            .window()
            .with_winit_window(|window| window.has_focus())
            == Some(true)
    }

    #[cfg(not(any(windows, test)))]
    fn focused(_panel: &Panel) -> bool {
        false
    }
}

fn deliver(
    mailbox: &Mutex<Option<Receipt>>,
    panel: &slint::Weak<Panel>,
    ticket: u64,
    reply: Reply,
) {
    let retired = mailbox.lock().replace(Receipt { ticket, reply });
    drop(retired);
    let _ = panel.upgrade_in_event_loop(|panel| panel.invoke_wallpaper_event_ready());
}

fn outcome_notice(outcome: WallpaperApplyOutcome, requested: u32) -> String {
    let sum = outcome
        .accepted
        .checked_add(outcome.failed)
        .and_then(|count| count.checked_add(outcome.not_submitted));
    if !(1..=32).contains(&requested)
        || outcome.requested != requested
        || outcome.accepted > 32
        || outcome.confirmed > outcome.accepted
        || outcome.failed > 32
        || outcome.not_submitted > 32
        || sum != Some(outcome.requested)
    {
        return "The provider returned invalid monitor counts. Actual Windows wallpaper is unknown; choose a fresh image.".into();
    }
    format!(
        "Requested captured displays: {}. Accepted: {}; path-confirmed: {}; unconfirmed: {}; failed: {}; not submitted: {}. Path/file readback does not confirm rendered pixels. Choose a fresh image to apply again.",
        outcome.requested,
        outcome.accepted,
        outcome.confirmed,
        outcome.accepted - outcome.confirmed,
        outcome.failed,
        outcome.not_submitted,
    )
}

fn error_notice(operation: Operation, error: WallpaperError) -> &'static str {
    match (operation, error) {
        (_, WallpaperError::Busy) => {
            "The shared Windows wallpaper provider is busy. No retry is queued. Choose fresh files before trying again."
        }
        (Operation::Choose, WallpaperError::Unavailable) => {
            "The Windows image picker or file validation failed. No image is selected. Choose again to try another image."
        }
        (Operation::Apply, WallpaperError::Unavailable) => {
            "The wallpaper request failed. Actual Windows wallpaper is unknown; choose a fresh image before trying again."
        }
        (Operation::ChooseCollection, WallpaperError::Unavailable) => {
            "The native collection picker or validation is unavailable. No group is selected; choose 2–32 images in one folder again."
        }
        (Operation::ApplyCollection, WallpaperError::Unavailable) => {
            "The native slideshow request is unavailable. Some SDK effects may already have occurred; actual Windows policy is unknown. No rollback or retry was performed."
        }
        (
            Operation::ReadSlideshow | Operation::AdvanceSlideshow(_),
            WallpaperError::Unavailable,
        ) => {
            "The native slideshow request is unavailable. Effects may already have occurred; current facts are unknown. No rollback or retry was performed."
        }
        (_, WallpaperError::InvalidTarget) => {
            "The native file selection, captured monitors, or target is no longer valid. Nothing is confirmed; choose fresh files."
        }
    }
}
