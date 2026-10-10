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
    AudioCommand, AudioDefaultTarget, AudioDeviceCommand, AudioDevicesResult, AudioEndpoint,
    AudioError, AudioErrorKind, AudioEvent, AudioFlow, AudioHost, AudioSnapshot, EndpointId,
    EndpointState, Volume,
};

use crate::dock_media::{CapturedMediaSeek, DockMediaController, PopupMediaToken};
use crate::generated::{AudioRoute, MediaAction, Panel, PopoverMotion, QuickSettings, TileBounds};
use crate::popup_placement::{self as placement, PopupRect};
use crate::sanitize::bounded_text;
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::transient_window::{TransientComponent, TransientWindow};
use crate::{DesktopHost, DockContext, SurfaceKind};

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use tests::{
    RecordedRequest as RecordedAudioRequest, RecordingAudio as RecordingAudioHost,
    snapshot as recorded_audio_snapshot, volume_command as recorded_audio_volume,
};

mod seek;
use seek::{InputScope, SeekInput};

mod devices;
use devices::DevicesState;

#[derive(Clone, PartialEq)]
struct MediaInputFrame {
    position: PhysicalPosition,
    size: slint::PhysicalSize,
    scale: f32,
    slider: TileBounds,
    clip: TileBounds,
}

impl MediaInputFrame {
    fn capture(popup: &QuickSettings) -> Option<Self> {
        let window = popup.window();
        let scale = window.scale_factor();
        let size = window.size();
        let slider = popup.get_seek_bounds();
        let clip = popup.get_seek_clip_bounds();
        let valid_bounds = |bounds: &TileBounds| {
            [
                bounds.origin.x,
                bounds.origin.y,
                bounds.width,
                bounds.height,
            ]
            .into_iter()
            .all(f32::is_finite)
                && bounds.width > 0.0
                && bounds.height > 0.0
        };
        (window.is_visible()
            && popup.get_seek_visible()
            && scale.is_finite()
            && scale > 0.0
            && size.width > 0
            && size.height > 0
            && valid_bounds(&slider)
            && valid_bounds(&clip))
        .then(|| Self {
            position: window.position(),
            size,
            scale,
            slider,
            clip,
        })
    }
}

#[derive(Clone)]
struct CapturedSeekInput {
    presentation: Rc<()>,
    authority: CapturedMediaSeek,
    frame: MediaInputFrame,
}

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
    ReadDevices,
    DeviceCommand(AudioDeviceCommand),
}

struct Flight {
    token: RequestToken,
    request: Request,
}

enum AudioResult {
    Default(AudioSnapshot),
    Devices(Box<AudioDevicesResult>),
}

struct Completion {
    token: RequestToken,
    result: Result<AudioResult, AudioError>,
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
    result: Result<AudioResult, AudioError>,
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
    devices: DevicesState,
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

/// Presentation admission is independent of either provider's availability.
/// Awaiting Root has no input authority; a revoked source stays revoked until
/// an explicit new presentation, even if its old predicate later returns true.
enum PopupSource {
    AwaitingRoot,
    Standalone,
    Scoped(Rc<dyn Fn() -> bool>),
    Retired,
}

pub(crate) struct QuickSettingsController {
    surface: TransientWindow<QuickSettings>,
    host: Arc<dyn DesktopHost>,
    panel: slint::Weak<Panel>,
    media: Rc<DockMediaController>,
    media_attachment: Cell<Option<PopupMediaToken>>,
    media_attaching: Cell<bool>,
    presentation_epoch: RefCell<Rc<()>>,
    source: RefCell<PopupSource>,
    // AX has no physical hold. Its latest frame still scopes trailing input
    // after the leading command completes and clears the visual preview.
    latest_seek_input: RefCell<Option<CapturedSeekInput>>,
    #[cfg(any(windows, test))]
    seek_input:
        RefCell<SeekInput<CapturedSeekInput, (slint::winit_030::winit::event::DeviceId, u64)>>,
    #[cfg(not(any(windows, test)))]
    seek_input: RefCell<SeekInput<CapturedSeekInput>>,
    #[cfg(any(windows, test))]
    input_release_timer: slint::Timer,
    state: RefCell<AudioState>,
    device_input: RefCell<devices::PhysicalAudioInput>,
    audio_focus: RefCell<Option<devices::AudioFocusFrame>>,
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
            source: RefCell::new(PopupSource::Retired),
            latest_seek_input: RefCell::default(),
            seek_input: RefCell::default(),
            #[cfg(any(windows, test))]
            input_release_timer: slint::Timer::default(),
            state: RefCell::default(),
            device_input: RefCell::default(),
            audio_focus: RefCell::default(),
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
        controller.surface.on_media_seek_invalidated(move || {
            if let Some(controller) = weak.upgrade() {
                controller.cancel_seek_input();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller
            .surface
            .on_media_selection_requested(move |identity| {
                if let Some(controller) = weak.upgrade() {
                    let epoch = controller.presentation_epoch.borrow().clone();
                    if !controller.media_input_ready()
                        || !controller.presentation_is_current(&epoch)
                    {
                        return;
                    }
                    controller.cancel_seek_input();
                    if !controller.media_input_ready()
                        || !controller.presentation_is_current(&epoch)
                    {
                        return;
                    }
                    if let Some(token) = controller.media_attachment.get() {
                        controller.media.select_from_popup(token, identity.as_str());
                    }
                }
            });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_media_seek_geometry_changed(move || {
            if let Some(controller) = weak.upgrade() {
                controller.cancel_seek_if_geometry_changed(None, None);
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_media_seek_finished(move || {
            if let Some(controller) = weak.upgrade() {
                controller.clear_seek_preview();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_media_seek_released(move || {
            if let Some(controller) = weak.upgrade() {
                controller.clear_seek_preview();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_media_seek_key_pressed(move |key| {
            if let Some(controller) = weak.upgrade() {
                let fresh = matches!(controller.seek_input.borrow().scope(), InputScope::Fresh);
                let capture = fresh.then(|| controller.capture_seek_input()).flatten();
                controller
                    .seek_input
                    .borrow_mut()
                    .key_down(key.as_str(), capture);
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_media_seek_key_released(move |key| {
            if let Some(controller) = weak.upgrade() {
                controller.seek_input.borrow_mut().key_up(key.as_str());
                controller.clear_seek_preview();
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
        Self::install_media_input_observer(&controller);
        controller.install_devices_callbacks();
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

    fn source_input_ready(&self) -> bool {
        let epoch = self.presentation_epoch.borrow().clone();
        self.source_is_current(&epoch)
    }

    fn source_is_current(&self, epoch: &Rc<()>) -> bool {
        if !self.presentation_is_current(epoch) {
            return false;
        }
        let admission = {
            let source = self.source.borrow();
            match &*source {
                PopupSource::AwaitingRoot | PopupSource::Retired => return false,
                PopupSource::Standalone => None,
                PopupSource::Scoped(admission) => Some(admission.clone()),
            }
        };
        // Never run Root or host callbacks while holding presentation/state
        // borrows. Reentry may install a different presentation or source.
        let admitted = admission.as_ref().is_none_or(|admission| admission());
        let same_source = match (&*self.source.borrow(), &admission) {
            (PopupSource::Standalone, None) => true,
            (PopupSource::Scoped(current), Some(admission)) => Rc::ptr_eq(current, admission),
            _ => false,
        };
        if !self.presentation_is_current(epoch) || !same_source {
            return false;
        }
        if admitted && self.is_open() && self.surface.window().is_visible() {
            return true;
        }
        self.retire_source(epoch);
        false
    }

    /// Root calls this after successful presentation/exclusive-popup admission,
    /// before any provider acquisition. Media gets the same revocable source,
    /// not a second authority predicate or an audio dependency on media readiness.
    pub(crate) fn attach_source_scoped(self: &Rc<Self>, admission: Rc<dyn Fn() -> bool>) {
        let epoch = self.presentation_epoch.borrow().clone();
        if !matches!(*self.source.borrow(), PopupSource::AwaitingRoot) || !self.is_open() {
            return;
        }
        self.source.replace(PopupSource::Scoped(admission));
        self.start_audio();
        if !self.source_is_current(&epoch) {
            return;
        }
        let weak = Rc::downgrade(self);
        self.attach_media_scoped(Rc::new(move || {
            weak.upgrade()
                .is_some_and(|controller| controller.source_is_current(&epoch))
        }));
    }

    fn retire_source(&self, epoch: &Rc<()>) {
        self.source.replace(PopupSource::Retired);
        self.cancel_device_input();
        self.volume_timer.stop();
        self.fit_timer.stop();
        {
            let mut state = self.state.borrow_mut();
            state.generation = state.generation.wrapping_add(1);
            state.confirmed = None;
            state.devices.retire();
            state.pending = Default::default();
            state.read_requested = false;
            state.watch_live = false;
            state.status = "Quick settings source expired; reopen from the toolbar.".into();
            state.watch_status.clear();
            // Keep the accepted flight/expected completion until terminal.
        }
        {
            let mut mailbox = self.mailbox.lock();
            mailbox.generation = None;
            mailbox.watch_epoch = mailbox.watch_epoch.wrapping_add(1);
            mailbox.changed = false;
            mailbox.watch = None;
        }
        let watch = self.watch.borrow_mut().take();
        drop(watch);
        if !self.presentation_is_current(epoch) {
            return;
        }
        self.cancel_seek_input();
        if !self.presentation_is_current(epoch) {
            return;
        }
        if let Some(token) = self.media_attachment.take() {
            self.media.detach_popup(token);
        }
        if self.presentation_is_current(epoch) {
            self.project_and_fit();
        }
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
        if !self.source_input_ready() {
            return false;
        }
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
        let presentation = self.presentation_epoch.borrow().clone();
        let attachment = self.media_attachment.get();
        self.cancel_seek_input();
        if self.presentation_is_current(&presentation)
            && self.media_attachment.get() == attachment
            && self.media_input_ready()
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

    fn capture_seek_input(&self) -> Option<CapturedSeekInput> {
        let frame = self.current_seek_frame()?;
        let presentation = self.presentation_epoch.borrow().clone();
        if !self.media_input_ready()
            || !self.surface.get_seek_enabled()
            || !self.surface.get_seek_visible()
        {
            return None;
        }
        let authority = self.media.capture_seek(self.media_attachment.get()?)?;
        (self.presentation_is_current(&presentation)
            && self.current_seek_frame().as_ref() == Some(&frame))
        .then_some(CapturedSeekInput {
            presentation,
            authority,
            frame,
        })
    }

    fn seek_input_current(&self, scope: &CapturedSeekInput) -> bool {
        self.presentation_is_current(&scope.presentation)
            && self.current_seek_frame().as_ref() == Some(&scope.frame)
            && self.media_input_ready()
            && self.media.seek_scope_current(&scope.authority)
            && self.presentation_is_current(&scope.presentation)
            && self.current_seek_frame().as_ref() == Some(&scope.frame)
    }

    fn current_seek_frame(&self) -> Option<MediaInputFrame> {
        self.is_open()
            .then(|| MediaInputFrame::capture(&self.surface))
            .flatten()
    }

    /// A reactive notification may describe a frame already read at capture.
    /// Only differing actual facts retire input; this never mints a new scope.
    fn cancel_seek_if_geometry_changed(
        &self,
        native_size: Option<slint::PhysicalSize>,
        native_scale: Option<f32>,
    ) {
        let captured = match self.seek_input.borrow().scope() {
            InputScope::Held(scope) => scope,
            InputScope::Fresh => self.latest_seek_input.borrow().clone(),
        };
        let unchanged = captured.is_some_and(|scope| {
            self.seek_input_current(&scope)
                && native_size.is_none_or(|size| size == scope.frame.size)
                && native_scale.is_none_or(|scale| scale == scope.frame.scale)
        });
        if !unchanged {
            self.cancel_seek_input();
        }
    }

    #[cfg(test)]
    pub(crate) fn observe_media_geometry_changed(&self) {
        self.cancel_seek_if_geometry_changed(None, None);
    }

    pub(crate) fn request_seek(&self, fraction: f32) {
        if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
            return;
        }
        let input = self.seek_input.borrow().scope();
        let presentation = self.presentation_epoch.borrow().clone();
        let scope = match input {
            InputScope::Held(scope) => scope,
            // Standard Slider changed is a genuine action, not a property
            // observer. AX has no raw WindowEvent and needs no release.
            InputScope::Fresh => self.capture_seek_input(),
        };
        let Some(scope) = scope.filter(|scope| self.seek_input_current(scope)) else {
            if self.presentation_is_current(&presentation) {
                self.cancel_seek_input();
            }
            return;
        };
        let Some(preview_time) = scope.authority.preview_time(fraction) else {
            return;
        };
        self.latest_seek_input.replace(Some(scope.clone()));
        self.surface.set_seek_preview_progress(fraction);
        if !self.seek_input_current(&scope) {
            if self.presentation_is_current(&scope.presentation) {
                self.cancel_seek_input();
            }
            return;
        }
        self.surface.set_seek_preview_time(preview_time.into());
        if !self.seek_input_current(&scope) {
            if self.presentation_is_current(&scope.presentation) {
                self.cancel_seek_input();
            }
            return;
        }
        self.surface.set_seek_preview_active(true);
        if self.seek_input_current(&scope) {
            self.surface.invoke_project_seek();
        }
        if self.seek_input_current(&scope) {
            self.media.seek_from_popup(&scope.authority, fraction);
        }
        if !self.seek_input_current(&scope) && self.presentation_is_current(&scope.presentation) {
            self.cancel_seek_input();
        }
    }

    fn clear_seek_preview(&self) {
        let presentation = self.presentation_epoch.borrow().clone();
        self.surface.set_seek_preview_active(false);
        if self.presentation_is_current(&presentation) {
            self.surface.invoke_project_seek();
        }
    }

    fn cancel_seek_input(&self) {
        let presentation = self.presentation_epoch.borrow().clone();
        self.seek_input.borrow_mut().cancel();
        self.latest_seek_input.borrow_mut().take();
        if let Some(token) = self.media_attachment.get() {
            self.media.cancel_pending_seek(token);
        }
        if self.presentation_is_current(&presentation) {
            self.clear_seek_preview();
        }
    }

    #[cfg(any(windows, test))]
    fn media_pointer(&self, position: Option<slint::LogicalPosition>, pressed: bool) {
        if !pressed {
            self.seek_input.borrow_mut().pointer_up();
            self.clear_seek_preview();
            return;
        }
        let bounds = self.surface.get_seek_bounds();
        let inside = self.seek_input.borrow().can_capture_pointer()
            && self.surface.get_seek_visible()
            && position.is_some_and(|position| {
                position.x.is_finite()
                    && position.y.is_finite()
                    && bounds.origin.x.is_finite()
                    && bounds.origin.y.is_finite()
                    && bounds.width.is_finite()
                    && bounds.height.is_finite()
                    && bounds.width > 0.0
                    && bounds.height > 0.0
                    && position.x >= bounds.origin.x
                    && position.x < bounds.origin.x + bounds.width
                    && position.y >= bounds.origin.y
                    && position.y < bounds.origin.y + bounds.height
            });
        let captured = inside.then(|| self.capture_seek_input()).flatten();
        self.seek_input.borrow_mut().pointer_down(captured);
    }

    #[cfg(any(windows, test))]
    fn media_touch(
        &self,
        device: slint::winit_030::winit::event::DeviceId,
        id: u64,
        phase: slint::winit_030::winit::event::TouchPhase,
    ) -> bool {
        use slint::winit_030::winit::event::TouchPhase;

        let contact = (device, id);
        let needs_finish = {
            let mut input = self.seek_input.borrow_mut();
            match phase {
                TouchPhase::Started => {
                    input.touch_start(contact);
                    false
                }
                TouchPhase::Moved => {
                    input.touch_move(contact);
                    false
                }
                TouchPhase::Ended | TouchPhase::Cancelled => input.touch_end(contact),
            }
        };
        self.cancel_seek_input();
        needs_finish
    }

    #[cfg(any(windows, test))]
    fn finish_input_release(&self) {
        let finished = self.seek_input.borrow_mut().finish_releases();
        if finished {
            self.clear_seek_preview();
        }
        self.finish_device_releases();
    }

    #[cfg(any(windows, test))]
    fn schedule_input_release(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        // Pinned winit processes its filter, buffered move and native release
        // synchronously. Timers run at the following new_events boundary.
        // Pending flags are revoked by fresh presses; an old timer cannot end
        // their new hold. This timer only retires media/audio input, never observes or commands.
        self.input_release_timer
            .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                if let Some(controller) = weak.upgrade() {
                    controller.finish_input_release();
                }
            });
    }

    /// Software-backend fixture bridge: the same pre-dispatch classifier runs
    /// immediately before a genuine native Slider pointer event.
    #[cfg(test)]
    pub(crate) fn observe_media_pointer(&self, position: slint::LogicalPosition, pressed: bool) {
        self.media_pointer(Some(position), pressed);
    }

    /// Post-dispatch fixture marker for the production weak timer fallback.
    #[cfg(test)]
    pub(crate) fn finish_media_pointer_release(&self) {
        self.finish_input_release();
    }

    #[cfg(test)]
    pub(crate) fn observe_media_touch(
        &self,
        id: u64,
        phase: slint::winit_030::winit::event::TouchPhase,
    ) {
        self.media_touch(slint::winit_030::winit::event::DeviceId::dummy(), id, phase);
    }

    #[cfg(test)]
    pub(crate) fn elapse_seek_throttle(&self, elapsed: Duration) {
        self.media.elapse_seek_throttle(elapsed);
    }

    #[cfg(any(windows, test))]
    fn install_media_input_observer(controller: &Rc<Self>) {
        use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
        use winit::event::{ElementState, MouseButton, WindowEvent};

        let weak = Rc::downgrade(controller);
        let mut cursor = None;
        // This popup owns the backend's single filter slot. Always propagate:
        // the standard widget remains the sole input/value implementation.
        controller
            .surface
            .window()
            .on_winit_window_event(move |window, event| {
                if let Some(controller) = weak.upgrade() {
                    match event {
                        WindowEvent::CursorMoved { position, .. } => cursor = Some(*position),
                        WindowEvent::MouseInput {
                            state,
                            button: MouseButton::Left,
                            ..
                        } => {
                            let scale = f64::from(window.scale_factor());
                            let logical = cursor.filter(|_| scale.is_finite() && scale > 0.0).map(
                                |position| {
                                    slint::LogicalPosition::new(
                                        (position.x / scale) as f32,
                                        (position.y / scale) as f32,
                                    )
                                },
                            );
                            controller.device_pointer(logical, *state == ElementState::Pressed);
                            controller.media_pointer(logical, *state == ElementState::Pressed);
                            if *state == ElementState::Released {
                                controller.schedule_input_release();
                            }
                        }
                        WindowEvent::KeyboardInput { event, .. } => {
                            use slint::platform::Key;
                            use winit::keyboard::{Key as NativeKey, NamedKey};
                            let audio_key = match &event.logical_key {
                                NativeKey::Named(NamedKey::ArrowLeft) => Some("Left"),
                                NativeKey::Named(NamedKey::ArrowRight) => Some("Right"),
                                NativeKey::Named(NamedKey::ArrowUp) => Some("Up"),
                                NativeKey::Named(NamedKey::ArrowDown) => Some("Down"),
                                NativeKey::Named(NamedKey::Home) => Some("Home"),
                                NativeKey::Named(NamedKey::End) => Some("End"),
                                NativeKey::Named(NamedKey::PageUp) => Some("PageUp"),
                                NativeKey::Named(NamedKey::PageDown) => Some("PageDown"),
                                _ => None,
                            };
                            if let Some(key) = audio_key {
                                controller.device_key(
                                    key,
                                    event.state == ElementState::Pressed,
                                    event.repeat,
                                );
                            }
                            if event.state == ElementState::Released {
                                let key = match &event.logical_key {
                                    NativeKey::Named(NamedKey::ArrowLeft) => Some(Key::LeftArrow),
                                    NativeKey::Named(NamedKey::ArrowRight) => Some(Key::RightArrow),
                                    NativeKey::Named(NamedKey::Home) => Some(Key::Home),
                                    NativeKey::Named(NamedKey::End) => Some(Key::End),
                                    _ => None,
                                };
                                if let Some(key) = key {
                                    let key = slint::SharedString::from(key);
                                    controller.seek_input.borrow_mut().key_up(key.as_str());
                                    controller.clear_seek_preview();
                                }
                            }
                        }
                        WindowEvent::Focused(false) => {
                            controller.cancel_device_input();
                            controller.cancel_seek_input();
                        }
                        WindowEvent::Moved(_) => {
                            // Position queries live native geometry, so an old
                            // moved notification alone cannot revoke new input.
                            controller.cancel_seek_if_geometry_changed(None, None);
                        }
                        WindowEvent::Resized(size) => {
                            cursor = None;
                            // Slint's cached size updates after this filter.
                            controller.cancel_seek_if_geometry_changed(
                                Some(slint::PhysicalSize::new(size.width, size.height)),
                                None,
                            );
                        }
                        WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                            cursor = None;
                            // Runtime scale also updates after this filter.
                            controller
                                .cancel_seek_if_geometry_changed(None, Some(*scale_factor as f32));
                        }
                        WindowEvent::Touch(touch) => {
                            let scale = f64::from(window.scale_factor());
                            let position = (scale.is_finite() && scale > 0.0).then(|| {
                                slint::LogicalPosition::new(
                                    (touch.location.x / scale) as f32,
                                    (touch.location.y / scale) as f32,
                                )
                            });
                            let audio_released = controller.device_touch(
                                touch.device_id,
                                touch.id,
                                touch.phase,
                                position,
                            );
                            let media_released =
                                controller.media_touch(touch.device_id, touch.id, touch.phase);
                            if audio_released || media_released {
                                controller.schedule_input_release();
                            }
                        }
                        _ => {}
                    }
                }
                EventResult::Propagate
            });
    }

    #[cfg(not(any(windows, test)))]
    fn install_media_input_observer(_controller: &Rc<Self>) {}

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

    #[cfg(test)]
    pub(crate) fn show_scoped(
        self: &Rc<Self>,
        theme: PresentationTheme,
        anchor: PhysicalPosition,
        context: DockContext,
        scale: f32,
        current: impl Fn() -> bool,
    ) -> Result<(), String> {
        self.present_scoped_inner(theme, anchor, context, scale, current, true)
    }

    pub(crate) fn present_scoped(
        self: &Rc<Self>,
        theme: PresentationTheme,
        anchor: PhysicalPosition,
        context: DockContext,
        scale: f32,
        current: impl Fn() -> bool,
    ) -> Result<(), String> {
        self.present_scoped_inner(theme, anchor, context, scale, current, false)
    }

    fn present_scoped_inner(
        self: &Rc<Self>,
        theme: PresentationTheme,
        anchor: PhysicalPosition,
        context: DockContext,
        scale: f32,
        current: impl Fn() -> bool,
        standalone: bool,
    ) -> Result<(), String> {
        if !current() {
            return Ok(());
        }
        let epoch = Rc::new(());
        self.hide_in(epoch.clone());
        if !self.presentation_is_current(&epoch) {
            return Ok(());
        }
        self.source.replace(PopupSource::AwaitingRoot);
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
        if standalone {
            self.source.replace(PopupSource::Standalone);
            self.start_audio();
            if !is_current() {
                return Ok(());
            }
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
        self.source.replace(PopupSource::Retired);
        self.cancel_device_input();
        self.cancel_seek_input();
        if !self.presentation_is_current(&epoch) {
            return;
        }
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
            state.devices.retire();
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
        let epoch = self.presentation_epoch.borrow().clone();
        if !self.source_is_current(&epoch) {
            return;
        }
        let needs_provider = self.state.borrow().audio.is_none();
        if needs_provider {
            let provider = self.host.audio_host();
            if !self.source_is_current(&epoch) {
                return;
            }
            // A reentrant valid acquisition may already have cached the one
            // provider. Never replace it or overwrite its successful status.
            if self.state.borrow().audio.is_none() {
                match provider {
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
        }
        let audio = self.state.borrow().audio.clone();
        if let Some(audio) = audio {
            let supported = audio.supports_devices();
            if !self.source_is_current(&epoch) {
                return;
            }
            self.state.borrow_mut().devices.supported = supported;
        }
        self.subscribe(&epoch);
        if !self.source_is_current(&epoch) {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.status.clear();
            state.read_requested = true;
            if state.devices.supported {
                state.devices.read_requested = true;
            }
        }
        self.project_and_fit();
        if self.source_is_current(&epoch) {
            self.pump();
        }
    }

    fn subscribe(&self, epoch: &Rc<()>) {
        if !self.source_is_current(epoch) {
            return;
        }
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
        let subscribed = audio.subscribe(notification);
        let source_current = self.source_is_current(epoch);
        let same_watch = self.mailbox.lock().watch_epoch == watch_epoch;
        if !source_current || !same_watch {
            // Drop only the returned old lease, never a reentrant new watch.
            drop(subscribed);
            return;
        }
        match subscribed {
            Ok(guard) => *self.watch.borrow_mut() = guard,
            Err(error) => {
                self.state.borrow_mut().watch_status =
                    failure("Live audio updates unavailable; use Refresh", &error);
            }
        }
    }

    fn volume(self: &Rc<Self>, route: AudioRoute, percent: f32, released: bool) {
        let epoch = self.presentation_epoch.borrow().clone();
        if !self.source_is_current(&epoch) {
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
        if !self.source_is_current(&epoch) {
            return;
        }
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
                    if !controller.source_is_current(&epoch)
                        || controller.state.borrow().generation != generation
                    {
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
        let epoch = self.presentation_epoch.borrow().clone();
        if !self.source_is_current(&epoch) {
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
        if self.source_is_current(&epoch) {
            self.pump();
        }
    }

    fn pump(self: &Rc<Self>) {
        let epoch = self.presentation_epoch.borrow().clone();
        if !self.source_is_current(&epoch) {
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
            } else if let Some(command) = state.devices.next_command() {
                Request::DeviceCommand(command)
            } else if state.devices.supported && state.devices.read_requested {
                state.devices.read_requested = false;
                Request::ReadDevices
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
        if !self.source_is_current(&epoch) || self.state.borrow().generation != token.generation {
            let mut state = self.state.borrow_mut();
            if state
                .flight
                .as_ref()
                .is_some_and(|flight| flight.token == token)
            {
                state.flight = None;
            }
            drop(state);
            let mut mailbox = self.mailbox.lock();
            if mailbox.expected == Some(token) {
                mailbox.expected = None;
            }
            drop(mailbox);
            if !self.presentation_is_current(&epoch) {
                self.pump();
            }
            return;
        }
        let mailbox = Arc::clone(&self.mailbox);
        let root = self.surface.as_weak();
        let completion = move |result| complete(&mailbox, &root, token, result);
        let accepted = match request {
            Request::Read => audio.read(Box::new(move |result| {
                completion(result.map(AudioResult::Default))
            })),
            Request::Command(command) => audio.execute(
                command,
                Box::new(move |result| completion(result.map(AudioResult::Default))),
            ),
            Request::ReadDevices => audio.read_devices(Box::new(move |result| {
                completion(result.map(|result| AudioResult::Devices(Box::new(result))))
            })),
            Request::DeviceCommand(command) => audio.execute_device(
                command,
                Box::new(move |result| {
                    completion(result.map(|result| AudioResult::Devices(Box::new(result))))
                }),
            ),
        };
        if let Err(error) = accepted {
            complete(&self.mailbox, &self.surface.as_weak(), token, Err(error));
        }
        // Acceptance is not cancellation or confirmation. A source lost inside
        // the host call retires pending work but keeps its accepted flight.
        self.source_is_current(&epoch);
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
        let visible = self.source_input_ready();
        let mut drop_watch = false;
        {
            let mut state = self.state.borrow_mut();
            if visible && generation == Some(state.generation) {
                if changed {
                    state.read_requested = true;
                    if state.devices.supported {
                        state.devices.read_requested = true;
                        // A dirty read is not identity retirement. Native writes
                        // still freshly validate the captured incarnation.
                    }
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
                        // Keep the same native device callback lease when only
                        // default volume watching failed. Inventory/session
                        // controls remain independently available; legacy hosts
                        // retain their existing failed-watch retirement.
                        drop_watch = !state.devices.supported;
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
                        Ok(AudioResult::Default(snapshot)) => state.accept_snapshot(snapshot),
                        Ok(AudioResult::Devices(result)) => {
                            let command = match &flight.request {
                                Request::DeviceCommand(command) => Some(command),
                                _ => None,
                            };
                            state.devices.accept(*result, command);
                        }
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
                                Request::ReadDevices | Request::DeviceCommand(_) => {
                                    state.devices.failed(&error);
                                    if error.kind == AudioErrorKind::DeviceChanged
                                        && matches!(flight.request, Request::DeviceCommand(_))
                                    {
                                        state.devices.read_requested = true;
                                    }
                                    "Could not read or change audio devices"
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
            let watch = self.watch.borrow_mut().take();
            drop(watch);
        }
        if !self.state.borrow().has_unready_volume() {
            self.volume_timer.stop();
        }
        if self.is_open() {
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
        self.project_devices();
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
