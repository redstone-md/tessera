// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use std::sync::Arc;
use tessera_system::web_search::{
    WebSearchHost, WebSearchOutcome, WebSearchProvider, WebSearchQuery, WebSearchRequest,
};

pub(super) fn app_filter(query: &str) -> Option<&str> {
    if query.starts_with("web:") || query.starts_with("files:") {
        None
    } else {
        Some(query.strip_prefix("apps:").unwrap_or(query))
    }
}

#[derive(Default)]
pub(super) struct WebState {
    serial: Cell<u64>,
    exhausted: Cell<bool>,
    intent: RefCell<Option<Rc<Intent>>>,
    flight: RefCell<Option<Flight>>,
    notice: RefCell<Option<Notice>>,
    publication: RefCell<Rc<()>>,
    source_epoch: RefCell<Rc<()>>,
}

#[derive(Clone)]
struct Flight {
    ticket: Arc<()>,
    intent: Rc<Intent>,
}

#[derive(Clone)]
struct Notice {
    intent: Rc<Intent>,
    message: String,
}

/// Identity is owner-issued Rc, never query text, indices or metadata. Native
/// input retains the opaque key; dispatch also requires this exact payload.
struct Intent {
    request: WebSearchRequest,
    raw: String,
    inventory: Rc<LauncherInventory>,
    host: Arc<dyn WebSearchHost>,
    view: LauncherView,
    mode: UiDisplayMode,
    session: Rc<LauncherSession>,
    frame: Frame,
    source_epoch: Rc<()>,
    source: slint::Weak<Launcher>,
    session_generation: u64,
}

#[derive(Clone, PartialEq)]
struct Frame {
    bounds: TileBounds,
    clip: TileBounds,
    scale: f32,
    position: slint::PhysicalPosition,
    size: slint::PhysicalSize,
}

impl Frame {
    fn capture(launcher: &Launcher) -> Self {
        Self {
            bounds: launcher.get_web_action_bounds(),
            clip: launcher.get_web_clip_bounds(),
            scale: launcher.window().scale_factor(),
            position: launcher.window().position(),
            size: launcher.window().size(),
        }
    }

    fn valid(&self) -> bool {
        let b = &self.bounds;
        let c = &self.clip;
        self.scale.is_finite()
            && self.scale > 0.0
            && [
                b.origin.x, b.origin.y, b.width, b.height, c.origin.x, c.origin.y, c.width,
                c.height,
            ]
            .iter()
            .all(|v| v.is_finite())
            && b.width > 0.0
            && b.height > 0.0
            && c.width > 0.0
            && c.height > 0.0
    }

    fn visible(&self) -> bool {
        let b = &self.bounds;
        let c = &self.clip;
        self.valid()
            && b.origin.x >= c.origin.x
            && b.origin.y >= c.origin.y
            && b.origin.x + b.width <= c.origin.x + c.width
            && b.origin.y + b.height <= c.origin.y + c.height
    }
}

impl PanelController {
    fn web_state(&self) -> Rc<WebState> {
        Rc::clone(&self.launcher_state.borrow().web)
    }

    pub(super) fn cancel_launcher_web(&self, launcher: &Launcher) {
        let web = self.web_state();
        let publication = Rc::new(());
        web.publication.replace(Rc::clone(&publication));
        web.source_epoch.replace(Rc::new(()));
        let retired = web.notice.borrow_mut().take();
        drop(retired);
        self.retire_web_input(launcher);
        if Rc::ptr_eq(&publication, &web.publication.borrow()) {
            launcher.set_web_notice("".into());
        }
    }

    fn retire_web_input(&self, launcher: &Launcher) {
        let web = self.web_state();
        let publication = Rc::clone(&web.publication.borrow());
        let current = || Rc::ptr_eq(&publication, &web.publication.borrow());
        let retired = web.intent.borrow_mut().take();
        drop(retired);
        launcher.invoke_cancel_web_input();
        if !current() {
            return;
        }
        launcher.set_web_available(false);
        if !current() {
            return;
        }
        launcher.set_web_action_data(DataTransfer::default());
        if !current() {
            return;
        }
        launcher.set_web_action_key("".into());
    }

    fn web_current(&self, launcher: &Launcher, intent: &Rc<Intent>) -> bool {
        let web = self.web_state();
        let idle = web.flight.borrow().is_none();
        let owned = web
            .intent
            .borrow()
            .as_ref()
            .is_some_and(|current| Rc::ptr_eq(current, intent));
        !web.exhausted.get()
            && idle
            && owned
            && launcher.get_input_available()
            && self.web_attributed(launcher, intent)
    }

    /// Outcome attribution is independent of flight/gesture admission. Source
    /// retirement invalidates projection only, never accepted native work.
    fn web_attributed(&self, launcher: &Launcher, intent: &Rc<Intent>) -> bool {
        let same_epoch = Rc::ptr_eq(
            &intent.source_epoch,
            &self.web_state().source_epoch.borrow(),
        );
        if !self.root_current()
            || !self.launcher_popup_ready()
            || !launcher.window().is_visible()
            || launcher.get_reorder_dragging()
            || intent.session.generation.get() != intent.session_generation
            || !same_epoch
            || intent
                .source
                .upgrade()
                .is_none_or(|source| !std::ptr::eq(source.window(), launcher.window()))
            || self
                .launcher_and_upgrade()
                .is_none_or(|source| !std::ptr::eq(source.window(), launcher.window()))
        {
            return false;
        }
        let frame = Frame::capture(launcher);
        if frame != intent.frame
            || !frame.valid()
            || launcher.get_search().as_str() != intent.raw
            || launcher.get_view() != intent.view
            || launcher.get_display_mode() != intent.mode
        {
            return false;
        }
        let state = self.launcher_state.borrow();
        Rc::ptr_eq(&state.inventory, &intent.inventory)
            && Rc::ptr_eq(&state.session, &intent.session)
            && state.query == intent.raw
            && state.view == intent.view
    }

    pub(super) fn sync_launcher_web(&self) {
        if !self.root_current() {
            return;
        }
        let Some(launcher) = self.launcher_and_upgrade() else {
            return;
        };
        let web = self.web_state();
        let publication = Rc::new(());
        web.publication.replace(Rc::clone(&publication));
        let raw = launcher.get_search().to_string();
        let is_web = raw.starts_with("web:");
        let publishing = || {
            let owned = Rc::ptr_eq(&publication, &web.publication.borrow());
            self.root_current() && owned && launcher.get_search().as_str() == raw
        };
        launcher.set_web_scope(is_web);
        if !publishing() {
            return;
        }
        launcher.set_files_scope(raw.starts_with("files:"));
        if !publishing() {
            return;
        }
        let flight = web.flight.borrow().clone();
        let scoped_notice = web.notice.borrow().clone();
        let busy = flight
            .as_ref()
            .is_some_and(|flight| self.web_attributed(&launcher, &flight.intent));
        let notice = match scoped_notice {
            Some(notice) if self.web_attributed(&launcher, &notice.intent) => notice.message,
            _ => {
                let retired = web.notice.borrow_mut().take();
                drop(retired);
                String::new()
            }
        };
        let notice = if flight.is_some() && !busy {
            "A previous browser dispatch is still pending.".to_owned()
        } else {
            notice
        };
        launcher.set_web_busy(busy);
        if !publishing() {
            return;
        }
        launcher.set_web_notice(notice.into());
        if !publishing() {
            return;
        }
        let request = raw
            .strip_prefix("web:")
            .and_then(WebSearchQuery::new)
            .map(|query| WebSearchRequest {
                provider: WebSearchProvider::DuckDuckGo,
                query,
            });
        if request.is_none() || web.flight.borrow().is_some() || web.exhausted.get() {
            self.retire_web_input(&launcher);
            return;
        }
        let intent = web.intent.borrow().clone();
        if intent
            .as_ref()
            .is_some_and(|intent| self.web_current(&launcher, intent))
        {
            return;
        }
        self.retire_web_input(&launcher);
        if !publishing() {
            return;
        }
        let source_current = self.launcher_source_guard();
        if !source_current() || !launcher.get_input_available() {
            return;
        }
        let frame = Frame::capture(&launcher);
        if !frame.valid() {
            return;
        }
        let Some(host) = self.core.host().web_search_host() else {
            return;
        };
        // Provider acquisition can synchronously replace/hide the source.
        if !publishing()
            || !source_current()
            || launcher.get_search().as_str() != raw
            || Frame::capture(&launcher) != frame
        {
            return;
        }
        let Some(serial) = web.serial.get().checked_add(1) else {
            web.exhausted.set(true);
            return;
        };
        web.serial.set(serial);
        let view = launcher.get_view();
        let (inventory, session) = {
            let state = self.launcher_state.borrow();
            if state.query != raw || state.view != view {
                return;
            }
            (Rc::clone(&state.inventory), Rc::clone(&state.session))
        };
        let source_epoch = Rc::clone(&web.source_epoch.borrow());
        let session_generation = session.generation.get();
        let intent = Rc::new(Intent {
            request: request.expect("validated web scope"),
            raw: raw.clone(),
            inventory,
            host,
            view,
            mode: launcher.get_display_mode(),
            session,
            frame,
            source_epoch,
            source: launcher.as_weak(),
            session_generation,
        });
        *web.intent.borrow_mut() = Some(Rc::clone(&intent));
        launcher.set_web_action_key(format!("web/{serial}").into());
        if !publishing() || !source_current() || !self.web_current(&launcher, &intent) {
            return;
        }
        launcher.set_web_action_data(transfer(Rc::clone(&intent)));
        if !publishing() || !source_current() || !self.web_current(&launcher, &intent) {
            return;
        }
        launcher.set_web_available(true);
        if publishing() && (!source_current() || !self.web_current(&launcher, &intent)) {
            self.retire_web_input(&launcher);
        }
    }

    fn dispatch_launcher_web(&self, launcher: &Launcher, payload: DataTransfer) {
        let Some(intent) = payload
            .user_data()
            .and_then(|p| p.downcast::<Intent>().ok())
        else {
            return;
        };
        if !self.web_current(launcher, &intent) || !intent.frame.visible() {
            return;
        }
        let source_current = self.launcher_source_guard();
        let Some(host) = self.core.host().web_search_host() else {
            return;
        };
        if !Arc::ptr_eq(&host, &intent.host)
            || !source_current()
            || !self.web_current(launcher, &intent)
        {
            return;
        }
        // Native setters may reenter. Keep the payload eligible until their
        // completion, then consume immediately before the host admission call.
        launcher.invoke_cancel_web_input();
        launcher.set_web_available(false);
        launcher.set_web_busy(true);
        launcher.set_web_notice("Requesting browser dispatch…".into());
        if !source_current() || !self.web_current(launcher, &intent) {
            self.sync_launcher_web();
            return;
        }
        let web = self.web_state();
        let flight = Arc::new(());
        *web.flight.borrow_mut() = Some(Flight {
            ticket: Arc::clone(&flight),
            intent: Rc::clone(&intent),
        });
        let retired = web.intent.borrow_mut().take();
        drop(retired);
        let signal = launcher.as_weak();
        let ticket = Arc::clone(&flight);
        let result = host.search(
            intent.request.clone(),
            Box::new(move |result| {
                let message = match result {
                    Ok(WebSearchOutcome::Accepted) => {
                        "Browser dispatch accepted; page visibility is not confirmed.".to_owned()
                    }
                    Ok(WebSearchOutcome::Declined) => "Browser dispatch declined.".to_owned(),
                    Err(error) => error.to_string(),
                };
                let _ = signal.upgrade_in_event_loop(move |launcher| {
                    // A completion owns its accepted flight, not a new query/session.
                    launcher.invoke_web_search_completed(transfer_completion(ticket, message));
                });
            }),
        );
        if let Err(error) = result {
            self.complete_launcher_web(launcher, &flight, error.to_string());
        }
    }

    fn complete_launcher_web(&self, launcher: &Launcher, ticket: &Arc<()>, message: String) {
        let web = self.web_state();
        let captured = web.flight.borrow().clone();
        let Some(captured) = captured.filter(|flight| Arc::ptr_eq(&flight.ticket, ticket)) else {
            return;
        };
        let retired = web.flight.borrow_mut().take();
        drop(retired);
        let notice = self
            .web_attributed(launcher, &captured.intent)
            .then(|| Notice {
                intent: Rc::clone(&captured.intent),
                message,
            });
        let retired = web.notice.replace(notice);
        drop(retired);
        if self.root_current() {
            launcher.set_web_busy(false);
            self.sync_launcher_web();
        }
    }

    pub(super) fn wire_launcher_web(&self, launcher: &Launcher) {
        let controller = self.clone();
        let owner = launcher.as_weak();
        launcher.on_web_search_requested(move |payload| {
            if let Some(launcher) = owner.upgrade() {
                controller.dispatch_launcher_web(&launcher, payload);
            }
        });
        let controller = self.clone();
        let owner = launcher.as_weak();
        launcher.on_web_input_cancelled(move || {
            if let Some(launcher) = owner.upgrade() {
                controller.cancel_launcher_web(&launcher);
            }
        });
        let controller = self.clone();
        launcher.on_web_geometry_changed(move || controller.sync_launcher_web());
        let controller = self.clone();
        let owner = launcher.as_weak();
        launcher.on_web_search_completed(move |payload| {
            let Some(completion) = payload
                .user_data()
                .and_then(|p| p.downcast::<Completion>().ok())
            else {
                return;
            };
            if let Some(launcher) = owner.upgrade() {
                controller.complete_launcher_web(
                    &launcher,
                    &completion.ticket,
                    completion.message.clone(),
                );
            }
        });
    }
}

struct Completion {
    ticket: Arc<()>,
    message: String,
}

fn transfer_completion(ticket: Arc<()>, message: String) -> DataTransfer {
    transfer(Rc::new(Completion { ticket, message }))
}
