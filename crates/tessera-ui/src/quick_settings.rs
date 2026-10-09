// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Identity-bound audio intentions and shared current-player attachment for one owned popup.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use slint::{ComponentHandle, PhysicalPosition};
use tessera_system::audio::{
    AudioCommand, AudioEndpoint, AudioError, AudioErrorKind, AudioEvent, AudioFlow, AudioHost,
    AudioSnapshot, EndpointId, EndpointState, Volume,
};

use crate::dock_media::{DockMediaController, PopupMediaToken};
use crate::generated::{AudioRoute, MediaAction, Panel, PopoverMotion, QuickSettings};
use crate::popup_placement::{self as placement, PopupRect};
use crate::sanitize::bounded_text;
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::transient_window::{TransientComponent, TransientWindow};
use crate::{DesktopHost, DockContext, SurfaceKind};

#[cfg(test)]
mod tests;

impl TransientComponent for QuickSettings {
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

#[derive(Clone)]
enum Request {
    Read,
    Command(AudioCommand),
}

struct Flight {
    token: RequestToken,
    request: Request,
}

struct Completion {
    token: RequestToken,
    result: Result<AudioSnapshot, AudioError>,
}

/// Only owned Send data crosses the host seam. One slot per event kind bounds
/// both storage and event-loop wakeups, including a burst during a native write.
#[derive(Default)]
struct Mailbox {
    generation: Option<u64>,
    watch_epoch: u64,
    expected: Option<RequestToken>,
    completion: Option<Completion>,
    changed: bool,
    watch: Option<AudioEvent>,
    wake_queued: bool,
}

fn wake(mailbox: &Arc<Mutex<Mailbox>>, root: &slint::Weak<QuickSettings>) {
    {
        let mut mailbox = mailbox.lock();
        if mailbox.wake_queued {
            return;
        }
        mailbox.wake_queued = true;
    }
    if root
        .upgrade_in_event_loop(|root| root.invoke_audio_event_ready())
        .is_err()
    {
        mailbox.lock().wake_queued = false;
    }
}

fn complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<QuickSettings>,
    token: RequestToken,
    result: Result<AudioSnapshot, AudioError>,
) {
    {
        let mut mailbox = mailbox.lock();
        if mailbox.expected != Some(token) {
            return;
        }
        mailbox.completion = Some(Completion { token, result });
    }
    // Even an inline host completion reaches the presenter only through the
    // generated UI callback; it never captures Rc, a timer, or a native lease.
    wake(mailbox, root);
}

struct Desired<T> {
    id: EndpointId,
    value: T,
}

struct PendingVolume {
    desired: Desired<Volume>,
    ready: bool,
}

#[derive(Default)]
struct PendingFlow {
    volume: Option<PendingVolume>,
    muted: Option<Desired<bool>>,
}

#[derive(Default)]
struct AudioState {
    generation: u64,
    sequence: u64,
    audio: Option<Arc<dyn AudioHost>>,
    confirmed: Option<AudioSnapshot>,
    pending: [PendingFlow; 2],
    flight: Option<Flight>,
    read_requested: bool,
    watch_live: bool,
    status: String,
    watch_status: String,
    next_flow: usize,
}

fn flow_index(flow: AudioFlow) -> usize {
    match flow {
        AudioFlow::Output => 0,
        AudioFlow::Input => 1,
    }
}

fn route_flow(route: AudioRoute) -> AudioFlow {
    match route {
        AudioRoute::Output => AudioFlow::Output,
        AudioRoute::Input => AudioFlow::Input,
    }
}

fn endpoint_state(snapshot: &AudioSnapshot, flow: AudioFlow) -> &EndpointState {
    match flow {
        AudioFlow::Output => &snapshot.output,
        AudioFlow::Input => &snapshot.input,
    }
}

fn endpoint(snapshot: &AudioSnapshot, flow: AudioFlow) -> Option<&AudioEndpoint> {
    match endpoint_state(snapshot, flow) {
        EndpointState::Ready(endpoint) => Some(endpoint),
        EndpointState::Absent | EndpointState::Unavailable(_) => None,
    }
}

fn command_flow(command: &AudioCommand) -> AudioFlow {
    match command {
        AudioCommand::SetVolume { flow, .. } | AudioCommand::SetMuted { flow, .. } => *flow,
    }
}

fn failure(context: &str, error: &AudioError) -> String {
    let detail = bounded_text(&error.message, 160);
    bounded_text(
        &format!(
            "{context}: {}",
            if detail.is_empty() {
                "Audio is unavailable"
            } else {
                &detail
            }
        ),
        240,
    )
}

impl AudioState {
    fn loading(&self) -> bool {
        // A watch invalidation commonly follows our own write while a native
        // drag is still active. Keep confirmed endpoints interactive during
        // background reads; pending identities are checked against their result.
        self.confirmed.is_none()
            && (self.read_requested
                || self.flight.as_ref().is_some_and(|flight| {
                    flight.token.generation == self.generation
                        && matches!(flight.request, Request::Read)
                }))
    }

    fn displayed_endpoint(&self, flow: AudioFlow) -> Option<&AudioEndpoint> {
        self.confirmed
            .as_ref()
            .and_then(|snapshot| endpoint(snapshot, flow))
    }

    fn accept_snapshot(&mut self, snapshot: AudioSnapshot) {
        for flow in [AudioFlow::Output, AudioFlow::Input] {
            let id = endpoint(&snapshot, flow).map(|endpoint| &endpoint.id);
            let pending = &mut self.pending[flow_index(flow)];
            if pending
                .volume
                .as_ref()
                .is_some_and(|volume| Some(&volume.desired.id) != id)
            {
                pending.volume = None;
            }
            if pending
                .muted
                .as_ref()
                .is_some_and(|muted| Some(&muted.id) != id)
            {
                pending.muted = None;
            }
        }
        self.confirmed = Some(snapshot);
        self.status.clear();
    }

    /// Every queued intention retains its displayed identity. A newer snapshot
    /// may discard it, but can never silently redirect it to another endpoint.
    fn next_command(&mut self) -> Option<AudioCommand> {
        for offset in 0..2 {
            let index = (self.next_flow + offset) % 2;
            let flow = if index == 0 {
                AudioFlow::Output
            } else {
                AudioFlow::Input
            };
            let Some(endpoint) = self.displayed_endpoint(flow).cloned() else {
                self.pending[index] = PendingFlow::default();
                continue;
            };
            if let Some(desired) = self.pending[index].muted.take()
                && desired.id == endpoint.id
                && desired.value != endpoint.muted
            {
                self.next_flow = 1 - index;
                return Some(AudioCommand::SetMuted {
                    flow,
                    expected_id: desired.id,
                    muted: desired.value,
                });
            }
            if self.pending[index]
                .volume
                .as_ref()
                .is_some_and(|volume| volume.ready)
            {
                let pending = self.pending[index]
                    .volume
                    .take()
                    .expect("ready volume exists");
                if pending.desired.id == endpoint.id && pending.desired.value != endpoint.volume {
                    self.next_flow = 1 - index;
                    return Some(AudioCommand::SetVolume {
                        flow,
                        expected_id: pending.desired.id,
                        volume: pending.desired.value,
                    });
                }
            }
        }
        None
    }

    fn has_unready_volume(&self) -> bool {
        self.pending
            .iter()
            .any(|flow| flow.volume.as_ref().is_some_and(|volume| !volume.ready))
    }
}

#[derive(Default)]
struct FlowProjection {
    ready: bool,
    muted: bool,
    percent: f32,
    status: String,
}

impl FlowProjection {
    fn from_state(state: &AudioState, flow: AudioFlow) -> Self {
        let Some(snapshot) = &state.confirmed else {
            return Self::default();
        };
        let endpoint = match endpoint_state(snapshot, flow) {
            EndpointState::Ready(endpoint) => endpoint,
            EndpointState::Absent => {
                return Self {
                    status: "No default device".into(),
                    ..Self::default()
                };
            }
            EndpointState::Unavailable(error) => {
                return Self {
                    status: failure("Unavailable", error),
                    ..Self::default()
                };
            }
        };
        let mut projection = Self {
            ready: true,
            muted: endpoint.muted,
            percent: endpoint.volume.scalar() * 100.0,
            status: String::new(),
        };
        if let Some(Flight {
            request: Request::Command(command),
            token,
        }) = &state.flight
            && token.generation == state.generation
        {
            match command {
                AudioCommand::SetVolume {
                    flow: target,
                    expected_id,
                    volume,
                } if *target == flow && *expected_id == endpoint.id => {
                    projection.percent = volume.scalar() * 100.0;
                }
                AudioCommand::SetMuted {
                    flow: target,
                    expected_id,
                    muted,
                } if *target == flow && *expected_id == endpoint.id => projection.muted = *muted,
                _ => {}
            }
        }
        let pending = &state.pending[flow_index(flow)];
        if let Some(volume) = &pending.volume
            && volume.desired.id == endpoint.id
        {
            projection.percent = volume.desired.value.scalar() * 100.0;
        }
        if let Some(muted) = &pending.muted
            && muted.id == endpoint.id
        {
            projection.muted = muted.value;
        }
        projection
    }
}

#[derive(Clone, Copy)]
struct Placement {
    anchor: PhysicalPosition,
    context: DockContext,
    scale: f32,
}

pub(crate) struct QuickSettingsController {
    surface: TransientWindow<QuickSettings>,
    host: Arc<dyn DesktopHost>,
    panel: slint::Weak<Panel>,
    media: Rc<DockMediaController>,
    media_attachment: Cell<Option<PopupMediaToken>>,
    media_attaching: Cell<bool>,
    presentation_epoch: RefCell<Rc<()>>,
    state: RefCell<AudioState>,
    mailbox: Arc<Mutex<Mailbox>>,
    watch: RefCell<Option<Box<dyn Send>>>,
    volume_timer: slint::Timer,
    fit_timer: slint::Timer,
    focus_watch: slint::Timer,
    focus_seen: Cell<bool>,
    placement: Cell<Option<Placement>>,
    rect: RefCell<Option<PopupRect>>,
}

impl QuickSettingsController {
    pub(crate) fn new(
        host: Arc<dyn DesktopHost>,
        panel: &Panel,
        media: Rc<DockMediaController>,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            surface: TransientWindow::new(host.clone(), QuickSettings::new()?, SurfaceKind::Popup),
            host,
            panel: panel.as_weak(),
            media,
            media_attachment: Cell::new(None),
            media_attaching: Cell::new(false),
            presentation_epoch: RefCell::new(Rc::new(())),
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
            watch: RefCell::default(),
            volume_timer: slint::Timer::default(),
            fit_timer: slint::Timer::default(),
            focus_watch: slint::Timer::default(),
            focus_seen: Cell::new(false),
            placement: Cell::new(None),
            rect: RefCell::default(),
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_audio_event_ready(move || {
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
        controller.surface.on_media_projection_complete(move || {
            if let Some(controller) = weak.upgrade() {
                let epoch = controller.presentation_epoch.borrow().clone();
                if controller.media_input_ready() && controller.presentation_is_current(&epoch) {
                    controller.fit();
                }
            }
        });
        let weak = Rc::downgrade(&controller);
        controller
            .surface
            .on_volume_requested(move |route, percent| {
                if let Some(controller) = weak.upgrade() {
                    controller.volume(route, percent, false);
                }
            });
        let weak = Rc::downgrade(&controller);
        controller
            .surface
            .on_volume_released(move |route, percent| {
                if let Some(controller) = weak.upgrade() {
                    controller.volume(route, percent, true);
                }
            });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_mute_requested(move |route, muted| {
            if let Some(controller) = weak.upgrade() {
                controller.mute(route, muted);
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_refresh_requested(move || {
            if let Some(controller) = weak.upgrade() {
                let epoch = controller.presentation_epoch.borrow().clone();
                controller.start_audio();
                if controller.presentation_is_current(&epoch) && controller.media_input_ready() {
                    controller.surface.invoke_media_refresh_requested();
                }
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_app_settings_requested(move || {
            if let Some(controller) = weak.upgrade() {
                controller.open_settings();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_dismiss_requested(move || {
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

    pub(crate) fn is_open(&self) -> bool {
        self.surface.is_visible()
    }

    pub(crate) fn component(&self) -> QuickSettings {
        self.surface.clone_strong()
    }

    fn presentation_is_current(&self, epoch: &Rc<()>) -> bool {
        Rc::ptr_eq(&self.presentation_epoch.borrow(), epoch)
    }

    #[cfg(test)]
    pub(crate) fn attach_media(&self) {
        self.attach_media_scoped(Rc::new(|| true));
    }

    /// Root's durable admission follows the attachment into shared projection
    /// and native submission; local visibility alone is not source authority.
    pub(crate) fn attach_media_scoped(&self, admission: Rc<dyn Fn() -> bool>) {
        let epoch = self.presentation_epoch.borrow().clone();
        if !admission()
            || !self.presentation_is_current(&epoch)
            || !self.is_open()
            || !self.surface.window().is_visible()
            || self.media_attachment.get().is_some()
            || self.media_attaching.get()
        {
            return;
        }
        self.media_attaching.set(true);
        let installed_token = Cell::new(None);
        let returned_token =
            self.media
                .attach_popup_scoped(&self.surface, admission.clone(), |token| {
                    if !admission()
                        || !self.presentation_is_current(&epoch)
                        || !self.is_open()
                        || !self.surface.window().is_visible()
                        || self.media_attachment.get().is_some()
                    {
                        return false;
                    }
                    self.media_attachment.set(Some(token));
                    installed_token.set(Some(token));
                    true
                });
        if self.presentation_is_current(&epoch) {
            self.media_attaching.set(false);
        }
        let Some(token) = returned_token.or(installed_token.get()) else {
            return;
        };
        if returned_token.is_none()
            || !admission()
            || !self.presentation_is_current(&epoch)
            || !self.is_open()
            || !self.surface.window().is_visible()
        {
            if self.media_attachment.get() == Some(token) {
                self.media_attachment.set(None);
            }
            self.media.detach_popup(token);
        }
    }

    pub(crate) fn media_input_ready(&self) -> bool {
        let Some(token) = self.media_attachment.get() else {
            return false;
        };
        let epoch = self.presentation_epoch.borrow().clone();
        self.is_open()
            && self.surface.window().is_visible()
            && self.media.popup_input_ready(token)
            && self.presentation_is_current(&epoch)
            && self.media_attachment.get() == Some(token)
            && self.is_open()
            && self.surface.window().is_visible()
    }

    /// Root owns the source/visibility admission callback, not this constructor.
    pub(crate) fn request_media(&self, action: MediaAction, identity: &str) {
        if self.media_input_ready()
            && let Some(token) = self.media_attachment.get()
        {
            self.media.request_from_popup(token, action, identity);
        }
    }

    pub(crate) fn retry_media(&self) {
        if self.media_input_ready() {
            self.media.retry();
        }
    }

    #[cfg(test)]
    pub(crate) fn show(
        self: &Rc<Self>,
        theme: PresentationTheme,
        anchor: PhysicalPosition,
        context: DockContext,
        scale: f32,
    ) -> Result<(), String> {
        self.show_scoped(theme, anchor, context, scale, || true)
    }

    pub(crate) fn show_scoped(
        self: &Rc<Self>,
        theme: PresentationTheme,
        anchor: PhysicalPosition,
        context: DockContext,
        scale: f32,
        current: impl Fn() -> bool,
    ) -> Result<(), String> {
        if !current() {
            return Ok(());
        }
        let epoch = Rc::new(());
        self.hide_in(epoch.clone());
        let is_current = || current() && self.presentation_is_current(&epoch);
        if !is_current() {
            return Ok(());
        }
        // Align the body, not its shadow footprint, immediately below the tile.
        let anchor = placement::physical_anchor(anchor, scale, (0.0, -10.0))?;
        self.placement.set(Some(Placement {
            anchor,
            context,
            scale,
        }));
        self.state.borrow_mut().read_requested = true;
        self.mailbox.lock().generation = Some(self.state.borrow().generation);
        self.surface.apply_presentation_theme(theme);
        if !is_current() {
            return Ok(());
        }
        self.project();
        if !is_current() {
            return Ok(());
        }
        let presentation = self.preferred_rect().and_then(|rect| {
            if !is_current() {
                return Ok((rect, false));
            }
            self.surface
                .present(rect.position, rect.size)
                .map(|shown| (rect, shown))
        });
        if !is_current() {
            return Ok(());
        }
        let rect = match presentation {
            Ok((rect, true)) => rect,
            Ok((_, false)) => {
                self.hide();
                return Ok(());
            }
            Err(error) => {
                self.hide();
                return Err(error);
            }
        };
        *self.rect.borrow_mut() = Some(rect);
        self.surface.invoke_focus_content();
        if !is_current() {
            return Ok(());
        }
        let focus = self.surface.request_focus();
        if !is_current() {
            return Ok(());
        }
        self.watch_focus();
        if !is_current() {
            return Ok(());
        }
        self.start_audio();
        if !is_current() {
            return Ok(());
        }
        focus.map_err(|error| {
            bounded_text(
                &format!("Audio popup opened, but keyboard focus was not granted: {error}"),
                240,
            )
        })
    }

    pub(crate) fn hide(&self) {
        self.hide_in(Rc::new(()));
    }

    fn hide_in(&self, epoch: Rc<()>) {
        self.presentation_epoch.replace(epoch.clone());
        self.media_attaching.set(false);
        if let Some(token) = self.media_attachment.take() {
            self.media.detach_popup(token);
        }
        // Detachment setters can reenter show/attach. Old cleanup must not
        // cancel the new source, stop its timers or release its native lease.
        if !self.presentation_is_current(&epoch) {
            return;
        }
        self.surface.invoke_cancel_media_input();
        if !self.presentation_is_current(&epoch) {
            return;
        }
        self.surface.set_media_view(Default::default());
        if !self.presentation_is_current(&epoch) {
            return;
        }
        self.surface.set_timeline_available(false);
        if !self.presentation_is_current(&epoch) {
            return;
        }
        self.surface.set_timeline_notice(Default::default());
        if !self.presentation_is_current(&epoch) {
            return;
        }
        self.surface.set_timeline_time(Default::default());
        if !self.presentation_is_current(&epoch) {
            return;
        }
        self.surface.set_timeline_progress(0.0);
        if !self.presentation_is_current(&epoch) {
            return;
        }
        self.focus_watch.stop();
        self.volume_timer.stop();
        self.fit_timer.stop();
        self.focus_seen.set(false);
        {
            let mut state = self.state.borrow_mut();
            state.generation = state.generation.wrapping_add(1);
            state.confirmed = None;
            state.pending = Default::default();
            state.read_requested = false;
            state.watch_live = false;
            state.status.clear();
            state.watch_status.clear();
            // An accepted operation cannot be cancelled through this port.
            // Retain only its flight until completion, so reopening cannot
            // create overlapping hardware requests. Its data cannot project.
        }
        {
            let mut mailbox = self.mailbox.lock();
            mailbox.generation = None;
            mailbox.changed = false;
            mailbox.watch = None;
        }
        let watch = self.watch.borrow_mut().take();
        drop(watch);
        if !self.presentation_is_current(&epoch) {
            return;
        }
        self.placement.set(None);
        self.rect.borrow_mut().take();
        self.surface.hide();
    }

    /// Ordinary observations do not close the popup. Changed native geometry
    /// conservatively invalidates its old tile anchor instead of retargeting it.
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

    pub(crate) fn disable_motion(&self) {
        self.surface.disable_motion();
    }

    pub(crate) fn apply_theme(&self, theme: PresentationTheme) {
        self.surface.apply_presentation_theme(theme);
        self.fit();
    }

    fn start_audio(self: &Rc<Self>) {
        if !self.is_open() {
            return;
        }
        if self.state.borrow().audio.is_none() {
            match self.host.audio_host() {
                Ok(Some(audio)) => self.state.borrow_mut().audio = Some(audio),
                result => {
                    let mut state = self.state.borrow_mut();
                    state.read_requested = false;
                    state.status = match result {
                        Err(error) => failure("Audio is unavailable", &error),
                        Ok(None) => "Audio controls are not supported on this platform.".into(),
                        Ok(Some(_)) => unreachable!(),
                    };
                    drop(state);
                    self.project_and_fit();
                    return;
                }
            }
        }
        self.subscribe();
        {
            let mut state = self.state.borrow_mut();
            state.status.clear();
            state.read_requested = true;
        }
        self.project_and_fit();
        self.pump();
    }

    fn subscribe(&self) {
        if self.watch.borrow().is_some() {
            return;
        }
        let (audio, generation) = {
            let state = self.state.borrow();
            (state.audio.clone(), state.generation)
        };
        let Some(audio) = audio else { return };
        let watch_epoch = {
            let mut mailbox = self.mailbox.lock();
            mailbox.watch_epoch = mailbox.watch_epoch.wrapping_add(1);
            mailbox.watch = None;
            mailbox.watch_epoch
        };
        let mailbox = Arc::clone(&self.mailbox);
        let root = self.surface.as_weak();
        let notification = Arc::new(move |event| {
            {
                let mut mailbox = mailbox.lock();
                if mailbox.generation != Some(generation) || mailbox.watch_epoch != watch_epoch {
                    return;
                }
                match event {
                    AudioEvent::Changed => mailbox.changed = true,
                    AudioEvent::WatchReady => {
                        if !matches!(mailbox.watch, Some(AudioEvent::WatchUnavailable(_))) {
                            mailbox.watch = Some(AudioEvent::WatchReady);
                        }
                    }
                    event => mailbox.watch = Some(event),
                }
            }
            wake(&mailbox, &root);
        });
        match audio.subscribe(notification) {
            Ok(guard) => *self.watch.borrow_mut() = guard,
            Err(error) => {
                self.state.borrow_mut().watch_status =
                    failure("Live audio updates unavailable; use Refresh", &error);
            }
        }
    }

    fn volume(self: &Rc<Self>, route: AudioRoute, percent: f32, released: bool) {
        if !self.is_open() {
            return;
        }
        if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
            self.project();
            return;
        }
        let Ok(value) = Volume::from_scalar(percent.round() / 100.0) else {
            return;
        };
        let flow = route_flow(route);
        {
            let mut state = self.state.borrow_mut();
            if state.loading() {
                drop(state);
                self.project();
                return;
            }
            let Some(id) = state
                .displayed_endpoint(flow)
                .map(|endpoint| endpoint.id.clone())
            else {
                drop(state);
                self.project();
                return;
            };
            let pending = &mut state.pending[flow_index(flow)].volume;
            let ready = released || pending.as_ref().is_some_and(|volume| volume.ready);
            *pending = Some(PendingVolume {
                desired: Desired { id, value },
                ready,
            });
        }
        self.project();
        if !self.state.borrow().has_unready_volume() {
            self.volume_timer.stop();
        } else if !self.volume_timer.running() {
            let weak = Rc::downgrade(self);
            let generation = self.state.borrow().generation;
            self.volume_timer.start(
                slint::TimerMode::SingleShot,
                Duration::from_millis(100),
                move || {
                    let Some(controller) = weak.upgrade() else {
                        return;
                    };
                    if !controller.is_open() || controller.state.borrow().generation != generation {
                        return;
                    }
                    for flow in &mut controller.state.borrow_mut().pending {
                        if let Some(volume) = &mut flow.volume {
                            volume.ready = true;
                        }
                    }
                    controller.pump();
                },
            );
        }
        self.pump();
    }

    fn mute(self: &Rc<Self>, route: AudioRoute, value: bool) {
        if !self.is_open() {
            return;
        }
        let flow = route_flow(route);
        {
            let mut state = self.state.borrow_mut();
            if state.loading() {
                return;
            }
            let Some(id) = state
                .displayed_endpoint(flow)
                .map(|endpoint| endpoint.id.clone())
            else {
                return;
            };
            state.pending[flow_index(flow)].muted = Some(Desired { id, value });
        }
        self.project();
        self.pump();
    }

    fn pump(self: &Rc<Self>) {
        if !self.is_open() {
            return;
        }
        let (audio, token, request) = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() {
                return;
            }
            let Some(audio) = state.audio.clone() else {
                return;
            };
            let request = if state.read_requested {
                state.read_requested = false;
                Request::Read
            } else if let Some(command) = state.next_command() {
                Request::Command(command)
            } else {
                return;
            };
            state.sequence = state.sequence.wrapping_add(1);
            let token = RequestToken {
                generation: state.generation,
                sequence: state.sequence,
            };
            state.flight = Some(Flight {
                token,
                request: request.clone(),
            });
            (audio, token, request)
        };
        self.mailbox.lock().expected = Some(token);
        self.project_and_fit();
        if !self.is_open() || self.state.borrow().generation != token.generation {
            self.state.borrow_mut().flight = None;
            self.mailbox.lock().expected = None;
            return;
        }
        let mailbox = Arc::clone(&self.mailbox);
        let root = self.surface.as_weak();
        let completion = Box::new(move |result| complete(&mailbox, &root, token, result));
        let accepted = match request {
            Request::Read => audio.read(completion),
            Request::Command(command) => audio.execute(command, completion),
        };
        if let Err(error) = accepted {
            complete(&self.mailbox, &self.surface.as_weak(), token, Err(error));
        }
    }

    fn drain(self: &Rc<Self>) {
        let (generation, changed, watch, completion) = {
            let mut mailbox = self.mailbox.lock();
            mailbox.wake_queued = false;
            let completion = mailbox.completion.take();
            if completion.is_some() {
                mailbox.expected = None;
            }
            (
                mailbox.generation,
                std::mem::take(&mut mailbox.changed),
                mailbox.watch.take(),
                completion,
            )
        };
        let visible = self.is_open();
        let mut drop_watch = false;
        {
            let mut state = self.state.borrow_mut();
            if visible && generation == Some(state.generation) {
                if changed {
                    state.read_requested = true;
                }
                match watch {
                    Some(AudioEvent::WatchReady) if self.watch.borrow().is_some() => {
                        state.watch_live = true;
                        state.watch_status.clear();
                    }
                    Some(AudioEvent::WatchUnavailable(error)) => {
                        state.watch_live = false;
                        state.watch_status =
                            failure("Live audio updates unavailable; use Refresh", &error);
                        drop_watch = true;
                    }
                    _ => {}
                }
            }
            if let Some(completion) = completion
                && state
                    .flight
                    .as_ref()
                    .is_some_and(|flight| flight.token == completion.token)
            {
                let flight = state.flight.take().expect("matching flight exists");
                if visible && completion.token.generation == state.generation {
                    match completion.result {
                        Ok(snapshot) => state.accept_snapshot(snapshot),
                        Err(error) => {
                            let context = match &flight.request {
                                Request::Read => {
                                    state.pending = Default::default();
                                    "Could not read audio"
                                }
                                Request::Command(command) => {
                                    state.pending[flow_index(command_flow(command))] =
                                        PendingFlow::default();
                                    "Could not change audio"
                                }
                            };
                            state.status = failure(context, &error);
                            if error.kind == AudioErrorKind::DeviceChanged
                                && matches!(flight.request, Request::Command(_))
                            {
                                state.read_requested = true;
                            }
                        }
                    }
                }
            }
        }
        if drop_watch {
            {
                let mut mailbox = self.mailbox.lock();
                mailbox.watch_epoch = mailbox.watch_epoch.wrapping_add(1);
                mailbox.watch = None;
            }
            drop(self.watch.borrow_mut().take());
        }
        if !self.state.borrow().has_unready_volume() {
            self.volume_timer.stop();
        }
        if visible {
            self.project_and_fit();
            self.pump();
        }
    }

    fn project(&self) {
        let (output, input, loading, live, status) = {
            let state = self.state.borrow();
            let status = [state.status.as_str(), state.watch_status.as_str()]
                .into_iter()
                .filter(|status| !status.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            (
                FlowProjection::from_state(&state, AudioFlow::Output),
                FlowProjection::from_state(&state, AudioFlow::Input),
                state.loading(),
                state.watch_live,
                bounded_text(&status, 320),
            )
        };
        // Standard Slider changed/released are input callbacks, not property
        // observers: this projection cannot generate a native audio request.
        self.surface.set_loading(loading);
        self.surface.set_watch_live(live);
        self.surface.set_status(status.into());
        self.surface.set_output_ready(output.ready);
        self.surface.set_output_muted(output.muted);
        self.surface.set_output_percent(output.percent);
        self.surface.set_output_status(output.status.into());
        self.surface.set_input_ready(input.ready);
        self.surface.set_input_muted(input.muted);
        self.surface.set_input_percent(input.percent);
        self.surface.set_input_status(input.status.into());
    }

    fn preferred_rect(&self) -> Result<PopupRect, String> {
        let placement = self
            .placement
            .get()
            .ok_or("Audio popup placement is unavailable.")?;
        placement::place_centered(
            placement.context,
            placement.anchor,
            (
                self.surface.get_preferred_popup_width(),
                self.surface.get_preferred_popup_height(),
            ),
            placement.scale,
        )
    }

    fn project_and_fit(&self) {
        self.project();
        self.fit();
    }

    /// Preferred metrics may settle after projection. Coalesce those UI-only
    /// notifications without re-entering layout or reviving a closed session.
    fn schedule_fit(self: &Rc<Self>) {
        if !self.is_open() || self.fit_timer.running() {
            return;
        }
        let weak = Rc::downgrade(self);
        let generation = self.state.borrow().generation;
        self.fit_timer
            .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                let Some(controller) = weak.upgrade() else {
                    return;
                };
                if controller.is_open() && controller.state.borrow().generation == generation {
                    controller.fit();
                }
            });
    }

    fn fit(&self) {
        if !self.is_open() {
            return;
        }
        match self.preferred_rect() {
            Ok(rect) => {
                let changed = self.rect.borrow().as_ref() != Some(&rect);
                if changed && self.surface.reposition(rect.position, rect.size) {
                    *self.rect.borrow_mut() = Some(rect);
                }
            }
            Err(error) => {
                self.hide();
                if let Some(panel) = self.panel.upgrade() {
                    panel.set_status(bounded_text(&error, 240).into());
                }
            }
        }
    }

    fn open_settings(&self) {
        self.hide();
        let Some(panel) = self.panel.upgrade() else {
            eprintln!("App Settings could not open: the settings panel is no longer available.");
            return;
        };
        let result = panel
            .show()
            .map_err(|error| error.to_string())
            .and_then(|()| self.host.request_ui_focus(panel.window()));
        if let Err(error) = result {
            panel.set_status(
                bounded_text(
                    &format!("App Settings could not receive focus: {error}"),
                    240,
                )
                .into(),
            );
        }
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

impl Drop for QuickSettingsController {
    fn drop(&mut self) {
        self.hide();
    }
}
