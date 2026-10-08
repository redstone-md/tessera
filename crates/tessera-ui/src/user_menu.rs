// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Session-bound known-folder intentions in one independently owned native popup.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, PhysicalPosition, SharedString};
use tessera_system::folders::{
    FolderAvailability, FolderError, FolderErrorKind, FolderHost, FolderId, FolderSnapshot,
    FolderTarget,
};

use crate::generated::{PopoverMotion, TileBounds, UserFolderKind, UserFolderRow, UserMenu};
use crate::popup_placement::{self as placement, PopupRect};
use crate::sanitize::bounded_text;
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::transient_window::{TransientComponent, TransientWindow};
use crate::{DesktopHost, DockContext, SurfaceKind};

#[cfg(test)]
mod tests;

impl TransientComponent for UserMenu {
    fn motion(&self) -> PopoverMotion<'_> {
        self.global::<PopoverMotion>()
    }

    fn set_presentation_opacity(&self, opacity: f32) {
        self.invoke_set_presentation_opacity(opacity);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RequestToken {
    generation: u64,
    sequence: u64,
}

#[derive(Clone, Copy)]
enum Request {
    Read,
    Open(FolderId),
}

struct Flight {
    token: RequestToken,
    request: Request,
}

enum Outcome {
    Read(Result<FolderSnapshot, FolderError>),
    Open(Result<(), FolderError>),
}

struct Completion {
    token: RequestToken,
    outcome: Outcome,
}

/// An opaque projection key is compared verbatim, never parsed into authority.
struct RowKey(SharedString);

/// The provider snapshot and its currently issued row keys never leave Rust.
struct FolderProjection {
    snapshot: FolderSnapshot,
    keys: [RowKey; 7],
}

#[derive(Default)]
struct FolderState {
    generation: u64,
    sequence: u64,
    key_sequence: u64,
    provider: Option<Arc<dyn FolderHost>>,
    acquiring_provider: bool,
    read_requested: bool,
    confirmed: Option<FolderProjection>,
    flight: Option<Flight>,
    notice: String,
}

impl FolderState {
    fn token(&mut self) -> RequestToken {
        self.sequence = self.sequence.wrapping_add(1);
        RequestToken {
            generation: self.generation,
            sequence: self.sequence,
        }
    }

    fn accept_snapshot(&mut self, snapshot: FolderSnapshot) {
        let keys = std::array::from_fn(|_| {
            self.key_sequence = self.key_sequence.wrapping_add(1);
            RowKey(format!("user-{:x}-{:x}", self.generation, self.key_sequence).into())
        });
        self.confirmed = Some(FolderProjection { snapshot, keys });
    }

    fn loading(&self) -> bool {
        self.read_requested
            || self.flight.as_ref().is_some_and(|flight| {
                flight.token.generation == self.generation
                    && matches!(flight.request, Request::Read)
            })
    }

    fn opening(&self) -> bool {
        self.flight
            .as_ref()
            .is_some_and(|flight| matches!(flight.request, Request::Open(_)))
    }

    fn target(&self, id: FolderId, key: &SharedString) -> Option<FolderTarget> {
        if self.loading() || self.flight.is_some() || self.acquiring_provider {
            return None;
        }
        let projection = self.confirmed.as_ref()?;
        if projection.keys[folder_index(id)].0 != *key {
            return None;
        }
        match projection.snapshot.get(id) {
            FolderAvailability::Ready(target) => Some(target.clone()),
            FolderAvailability::Unavailable(_) => None,
        }
    }

    fn invalidate_folder(&mut self, id: FolderId, error: FolderError) {
        let Some(projection) = &self.confirmed else {
            return;
        };
        let entries = FolderId::ALL.map(|candidate| {
            if candidate == id {
                FolderAvailability::Unavailable(error.clone())
            } else {
                projection.snapshot.get(candidate).clone()
            }
        });
        self.accept_snapshot(FolderSnapshot::new(entries));
    }
}

/// There is only one accepted flight, including when its popup session closes.
/// A bounded mailbox carries Send data; even inline completions post to Slint.
#[derive(Default)]
struct Mailbox {
    expected: Option<RequestToken>,
    completion: Option<Completion>,
    wake_queued: bool,
}

fn complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<UserMenu>,
    token: RequestToken,
    outcome: Outcome,
) {
    {
        let mut mailbox = mailbox.lock();
        if mailbox.expected != Some(token) || mailbox.completion.is_some() {
            return;
        }
        mailbox.completion = Some(Completion { token, outcome });
        if mailbox.wake_queued {
            return;
        }
        mailbox.wake_queued = true;
    }
    if root
        .upgrade_in_event_loop(|root| root.invoke_folder_event_ready())
        .is_err()
    {
        mailbox.lock().wake_queued = false;
    }
}

#[derive(Clone, Copy)]
struct Placement {
    anchor: PhysicalPosition,
    context: DockContext,
    scale: f32,
}

pub(crate) struct UserMenuController {
    surface: TransientWindow<UserMenu>,
    host: Arc<dyn DesktopHost>,
    state: RefCell<FolderState>,
    mailbox: Arc<Mutex<Mailbox>>,
    placement: Cell<Option<Placement>>,
    rect: RefCell<Option<PopupRect>>,
    fit_timer: slint::Timer,
    focus_watch: slint::Timer,
    focus_seen: Cell<bool>,
}

impl UserMenuController {
    pub(crate) fn new(host: Arc<dyn DesktopHost>) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            surface: TransientWindow::new(host.clone(), UserMenu::new()?, SurfaceKind::Popup),
            host,
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
            placement: Cell::new(None),
            rect: RefCell::default(),
            fit_timer: slint::Timer::default(),
            focus_watch: slint::Timer::default(),
            focus_seen: Cell::new(false),
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_folder_event_ready(move || {
            if let Some(controller) = weak.upgrade() {
                controller.drain();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_preferred_size_changed(move || {
            if let Some(controller) = weak.upgrade() {
                controller.schedule_fit();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller
            .surface
            .on_folder_open_requested(move |kind, key| {
                if let Some(controller) = weak.upgrade() {
                    controller.open(folder_id(kind), key);
                }
            });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_retry_requested(move || {
            if let Some(controller) = weak.upgrade() {
                controller.retry();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_hide_requested(move || {
            if let Some(controller) = weak.upgrade() {
                controller.hide();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.window().on_close_requested(move || {
            if let Some(controller) = weak.upgrade() {
                controller.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        Ok(controller)
    }

    pub(crate) fn component(&self) -> &UserMenu {
        &self.surface
    }

    pub(crate) fn is_open(&self) -> bool {
        self.surface.is_visible()
    }

    /// The caller supplies genuine identity and applies its current pure theme.
    /// Bounds are logical input geometry on the source window, not magic pixels.
    pub(crate) fn show(
        self: &Rc<Self>,
        source: &slint::Window,
        bounds: TileBounds,
        context: DockContext,
        user_name: &str,
    ) -> Result<(), String> {
        self.hide();
        if !source.is_visible()
            || context.fullscreen_active()
            || !bounds.origin.x.is_finite()
            || !bounds.origin.y.is_finite()
            || !bounds.width.is_finite()
            || !bounds.height.is_finite()
            || bounds.width <= 0.0
            || bounds.height <= 0.0
        {
            return Err("User popup needs a visible source and valid input bounds.".into());
        }
        let scale = source.scale_factor();
        let anchor = placement::physical_anchor(
            source.position(),
            scale,
            (
                bounds.origin.x + bounds.width / 2.0,
                bounds.origin.y + bounds.height - 10.0,
            ),
        )?;
        self.placement.set(Some(Placement {
            anchor,
            context,
            scale,
        }));
        let generation = {
            let mut state = self.state.borrow_mut();
            state.read_requested = true;
            state.generation
        };
        let name = bounded_text(user_name, 128);
        self.surface.set_user_name(
            if name.is_empty() {
                "User identity unavailable".into()
            } else {
                name
            }
            .into(),
        );
        self.project();
        let presentation = self.preferred_rect().and_then(|rect| {
            self.surface
                .present(rect.position, rect.size)
                .map(|shown| (rect, shown))
        });
        match presentation {
            Ok((rect, true)) if self.current(generation) => {
                *self.rect.borrow_mut() = Some(rect);
            }
            Ok((_, _)) => {
                self.hide();
                return Ok(());
            }
            Err(error) => {
                self.hide();
                return Err(error);
            }
        }
        self.surface.invoke_focus_content();
        if !self.current(generation) {
            return Ok(());
        }
        let focus = self.surface.request_focus();
        if !self.current(generation) {
            return Ok(());
        }
        self.watch_focus();
        self.pump();
        focus.map_err(|error| {
            bounded_text(
                &format!("User popup opened, but keyboard focus was not granted: {error}"),
                240,
            )
        })
    }

    pub(crate) fn hide(&self) {
        self.focus_watch.stop();
        self.fit_timer.stop();
        self.focus_seen.set(false);
        {
            let mut state = self.state.borrow_mut();
            state.generation = state.generation.wrapping_add(1);
            state.read_requested = false;
            state.confirmed = None;
            state.notice.clear();
            // Accepted work cannot be cancelled. Keep its flight/mailbox until
            // completion, but retire all visible identities immediately.
        }
        self.placement.set(None);
        self.rect.borrow_mut().take();
        self.surface.hide();
    }

    pub(crate) fn disable_motion(&self) {
        self.surface.disable_motion();
    }

    pub(crate) fn apply_theme(&self, theme: PresentationTheme) {
        self.component().apply_presentation_theme(theme);
        let _ = self.refit();
    }

    pub(crate) fn close_if_geometry_changed(&self, context: DockContext, scale: f32) {
        let Some(previous) = self.placement.get() else {
            return;
        };
        let old = previous.context;
        if context.fullscreen_active()
            || previous.scale != scale
            || (old.x(), old.y(), old.width(), old.height())
                != (context.x(), context.y(), context.width(), context.height())
        {
            self.hide();
        }
    }

    fn current(&self, generation: u64) -> bool {
        self.is_open() && self.state.borrow().generation == generation
    }

    fn retry(self: &Rc<Self>) {
        if !self.is_open() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || state.acquiring_provider || state.read_requested {
                return;
            }
            state.confirmed = None;
            state.notice.clear();
            state.read_requested = true;
        }
        self.project_and_fit();
        self.pump();
    }

    fn open(self: &Rc<Self>, id: FolderId, key: SharedString) {
        if !self.is_open() {
            return;
        }
        let (provider, target, token) = {
            let mut state = self.state.borrow_mut();
            let Some(target) = state.target(id, &key) else {
                return;
            };
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let token = state.token();
            state.notice.clear();
            state.flight = Some(Flight {
                token,
                request: Request::Open(id),
            });
            (provider, target, token)
        };
        self.mailbox.lock().expected = Some(token);
        self.project_and_fit();
        if !self.current(token.generation) {
            self.cancel_unissued(token);
            return;
        }
        let mailbox = Arc::clone(&self.mailbox);
        let root = self.surface.as_weak();
        let accepted = provider.open(
            id,
            target,
            Box::new(move |result| complete(&mailbox, &root, token, Outcome::Open(result))),
        );
        if let Err(error) = accepted {
            complete(
                &self.mailbox,
                &self.surface.as_weak(),
                token,
                Outcome::Open(Err(error)),
            );
        }
    }

    /// A reopen queues only a fresh read behind any old accepted operation.
    fn pump(self: &Rc<Self>) {
        if !self.is_open() {
            return;
        }
        let acquire = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || state.acquiring_provider || !state.read_requested {
                return;
            }
            if state.provider.is_none() {
                state.acquiring_provider = true;
                Some(state.generation)
            } else {
                None
            }
        };
        if let Some(generation) = acquire {
            // A capability factory may re-enter hide/show; no RefCell borrow
            // crosses it, and a failure must not become a permanent cache.
            let result = self.host.folder_host();
            {
                let mut state = self.state.borrow_mut();
                state.acquiring_provider = false;
                match result {
                    Ok(Some(provider)) => state.provider = Some(provider),
                    result if state.generation == generation => {
                        state.read_requested = false;
                        state.notice = match result {
                            Err(error) => failure("Folder access unavailable", &error),
                            Ok(None) => "Known folders are not supported on this platform.".into(),
                            Ok(Some(_)) => unreachable!(),
                        };
                    }
                    _ => {}
                }
            }
            if !self.is_open() {
                return;
            }
            self.project_and_fit();
            if self.state.borrow().generation != generation {
                self.pump();
                return;
            }
        }
        let (provider, token) = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || !state.read_requested {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            state.read_requested = false;
            let token = state.token();
            state.flight = Some(Flight {
                token,
                request: Request::Read,
            });
            (provider, token)
        };
        self.mailbox.lock().expected = Some(token);
        self.project_and_fit();
        if !self.current(token.generation) {
            self.cancel_unissued(token);
            return;
        }
        let mailbox = Arc::clone(&self.mailbox);
        let root = self.surface.as_weak();
        let accepted = provider.read(Box::new(move |result| {
            complete(&mailbox, &root, token, Outcome::Read(result));
        }));
        if let Err(error) = accepted {
            complete(
                &self.mailbox,
                &self.surface.as_weak(),
                token,
                Outcome::Read(Err(error)),
            );
        }
    }

    fn cancel_unissued(self: &Rc<Self>, token: RequestToken) {
        {
            let mut state = self.state.borrow_mut();
            if state
                .flight
                .as_ref()
                .is_some_and(|flight| flight.token == token)
            {
                state.flight = None;
            }
        }
        {
            let mut mailbox = self.mailbox.lock();
            if mailbox.expected == Some(token) {
                mailbox.expected = None;
            }
        }
        if self.is_open() {
            self.project_and_fit();
            self.pump();
        }
    }

    fn drain(self: &Rc<Self>) {
        let completion = {
            let mut mailbox = self.mailbox.lock();
            mailbox.wake_queued = false;
            let completion = mailbox.completion.take();
            if completion.is_some() {
                mailbox.expected = None;
            }
            completion
        };
        let visible = self.is_open();
        {
            let mut state = self.state.borrow_mut();
            if let Some(completion) = completion
                && state
                    .flight
                    .as_ref()
                    .is_some_and(|flight| flight.token == completion.token)
            {
                let flight = state.flight.take().expect("matching flight exists");
                if visible && completion.token.generation == state.generation {
                    match (flight.request, completion.outcome) {
                        (Request::Read, Outcome::Read(Ok(snapshot))) => {
                            state.notice.clear();
                            state.accept_snapshot(snapshot);
                        }
                        (Request::Read, Outcome::Read(Err(error))) => {
                            state.confirmed = None;
                            state.notice = failure("Could not read known folders", &error);
                        }
                        (Request::Open(id), Outcome::Open(Ok(()))) => {
                            state.notice = format!("{} open request accepted.", folder_label(id));
                        }
                        (Request::Open(id), Outcome::Open(Err(error))) => {
                            state.notice = failure("Could not open folder", &error);
                            state.invalidate_folder(id, error);
                        }
                        _ => unreachable!("typed completion must match its request"),
                    }
                }
            }
        }
        if visible {
            self.project_and_fit();
            self.pump();
        }
    }

    fn project(&self) {
        let (rows, loading, opening, notice, retry_enabled) = {
            let state = self.state.borrow();
            let loading = state.loading();
            let opening = state.opening();
            let rows: Vec<UserFolderRow> = FolderId::ALL
                .into_iter()
                .map(|id| {
                    let (key, ready, status) = if loading {
                        (
                            SharedString::default(),
                            false,
                            "Checking availability…".into(),
                        )
                    } else if let Some(projection) = &state.confirmed {
                        let (ready, status) = match projection.snapshot.get(id) {
                            FolderAvailability::Ready(_) => (true, String::new()),
                            FolderAvailability::Unavailable(error) => {
                                (false, folder_status(error.kind).into())
                            }
                        };
                        (projection.keys[folder_index(id)].0.clone(), ready, status)
                    } else {
                        (SharedString::default(), false, "Unavailable".into())
                    };
                    UserFolderRow {
                        kind: folder_kind(id),
                        key,
                        ready,
                        status: status.into(),
                    }
                })
                .collect();
            (
                rows,
                loading,
                opening,
                bounded_text(&state.notice, 240),
                !loading && !opening && !state.acquiring_provider && state.flight.is_none(),
            )
        };
        self.surface
            .set_rows(ModelRc::new(slint::VecModel::from(rows)));
        self.surface.set_loading(loading);
        self.surface.set_opening(opening);
        self.surface.set_notice(notice.into());
        self.surface.set_retry_enabled(retry_enabled);
    }

    fn preferred_rect(&self) -> Result<PopupRect, String> {
        let placement = self
            .placement
            .get()
            .ok_or("User popup placement is unavailable.")?;
        placement::place_centered(
            placement.context,
            placement.anchor,
            (
                self.surface.get_popup_content_width(),
                self.surface.get_popup_content_height(),
            ),
            placement.scale,
        )
    }

    fn project_and_fit(&self) {
        self.project();
        let _ = self.refit();
    }

    pub(crate) fn refit(&self) -> Result<(), String> {
        if !self.is_open() {
            return Ok(());
        }
        match self.preferred_rect() {
            Ok(rect) => {
                let changed = self.rect.borrow().as_ref() != Some(&rect);
                if changed && self.surface.reposition(rect.position, rect.size) {
                    *self.rect.borrow_mut() = Some(rect);
                }
                Ok(())
            }
            Err(error) => {
                self.hide();
                Err(error)
            }
        }
    }

    fn schedule_fit(self: &Rc<Self>) {
        if !self.is_open() || self.fit_timer.running() {
            return;
        }
        let weak = Rc::downgrade(self);
        let generation = self.state.borrow().generation;
        self.fit_timer
            .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                if let Some(controller) = weak.upgrade()
                    && controller.current(generation)
                {
                    let _ = controller.refit();
                }
            });
    }

    fn watch_focus(self: &Rc<Self>) {
        if !self.is_open() {
            return;
        }
        self.focus_seen.set(self.is_focused() == Some(true));
        if self.is_focused().is_none() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.focus_watch.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || {
                if let Some(controller) = weak.upgrade() {
                    if !controller.is_open() {
                        controller.focus_watch.stop();
                        return;
                    }
                    match controller.is_focused() {
                        Some(true) => controller.focus_seen.set(true),
                        Some(false) if controller.focus_seen.get() => controller.hide(),
                        _ => {}
                    }
                }
            },
        );
    }

    #[cfg(any(windows, test))]
    fn is_focused(&self) -> Option<bool> {
        use slint::winit_030::WinitWindowAccessor;
        self.surface
            .window()
            .with_winit_window(|window| window.has_focus())
    }

    #[cfg(not(any(windows, test)))]
    fn is_focused(&self) -> Option<bool> {
        None
    }
}

impl Drop for UserMenuController {
    fn drop(&mut self) {
        self.hide();
    }
}

fn folder_index(id: FolderId) -> usize {
    match id {
        FolderId::Recent => 0,
        FolderId::Desktop => 1,
        FolderId::Downloads => 2,
        FolderId::Documents => 3,
        FolderId::Music => 4,
        FolderId::Pictures => 5,
        FolderId::Videos => 6,
    }
}

fn folder_kind(id: FolderId) -> UserFolderKind {
    match id {
        FolderId::Recent => UserFolderKind::Recent,
        FolderId::Desktop => UserFolderKind::Desktop,
        FolderId::Downloads => UserFolderKind::Downloads,
        FolderId::Documents => UserFolderKind::Documents,
        FolderId::Music => UserFolderKind::Music,
        FolderId::Pictures => UserFolderKind::Pictures,
        FolderId::Videos => UserFolderKind::Videos,
    }
}

fn folder_id(kind: UserFolderKind) -> FolderId {
    match kind {
        UserFolderKind::Recent => FolderId::Recent,
        UserFolderKind::Desktop => FolderId::Desktop,
        UserFolderKind::Downloads => FolderId::Downloads,
        UserFolderKind::Documents => FolderId::Documents,
        UserFolderKind::Music => FolderId::Music,
        UserFolderKind::Pictures => FolderId::Pictures,
        UserFolderKind::Videos => FolderId::Videos,
    }
}

fn folder_label(id: FolderId) -> &'static str {
    match id {
        FolderId::Recent => "Recent",
        FolderId::Desktop => "Desktop",
        FolderId::Downloads => "Downloads",
        FolderId::Documents => "Documents",
        FolderId::Music => "Music",
        FolderId::Pictures => "Pictures",
        FolderId::Videos => "Videos",
    }
}

fn folder_status(kind: FolderErrorKind) -> &'static str {
    match kind {
        FolderErrorKind::Unsupported => "Not supported",
        FolderErrorKind::AccessDenied => "Access denied",
        FolderErrorKind::Unavailable => "Folder unavailable",
        FolderErrorKind::TargetChanged => "Folder changed; retry availability",
        FolderErrorKind::Busy => "Folder service is busy",
        FolderErrorKind::Stopped => "Folder service stopped",
        FolderErrorKind::Other => "Folder unavailable",
    }
}

fn failure(context: &str, error: &FolderError) -> String {
    // Provider messages may contain redirected/native paths. Kind is the safe
    // presentation vocabulary; neither raw messages nor Debug reach the GUI.
    bounded_text(&format!("{context}: {}.", folder_status(error.kind)), 240)
}
