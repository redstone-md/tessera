// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::Dock;
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::{Key, WindowEvent};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tessera_system::audio::AudioCompletion;
use tessera_system::media::{
    MediaCapabilities, MediaCommandCompletion, MediaError, MediaErrorKind, MediaEvent, MediaHost,
    MediaPlayback, MediaReadCompletion, MediaRequest, MediaSession, MediaSessionKey, MediaSnapshot,
    MediaTimeline,
};

thread_local! {
    static MEDIA_FACTORY_HOOK: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    static MEDIA_RETIRE_HOOK: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    static FOCUS_HOOK: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

#[derive(Default)]
struct RecordingPopupMedia {
    reads: AtomicUsize,
    events: Arc<Mutex<Vec<&'static str>>>,
    observation: Mutex<MediaSnapshot>,
    commands: Mutex<Vec<MediaRequest>>,
}

struct PopupMediaLease(Arc<Mutex<Vec<&'static str>>>);

impl Drop for PopupMediaLease {
    fn drop(&mut self) {
        self.0.lock().push("media-unwatch");
        let hook = MEDIA_RETIRE_HOOK.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }
}

impl MediaHost for RecordingPopupMedia {
    fn read(&self, completion: MediaReadCompletion) -> Result<(), MediaError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.events.lock().push("media-read");
        let snapshot = self.observation.lock().clone();
        completion(Ok(snapshot));
        Ok(())
    }

    fn execute(
        &self,
        request: MediaRequest,
        completion: MediaCommandCompletion,
    ) -> Result<(), MediaError> {
        self.commands.lock().push(request);
        completion(Ok(()));
        Ok(())
    }

    fn subscribe(
        &self,
        changed: Arc<dyn Fn(MediaEvent) + Send + Sync>,
    ) -> Result<Option<Box<dyn Send>>, MediaError> {
        self.events.lock().push("media-watch");
        changed(MediaEvent::WatchReady);
        Ok(Some(Box::new(PopupMediaLease(self.events.clone()))))
    }
}

type ResultSnapshot = Result<AudioSnapshot, AudioError>;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RecordedRequest {
    Read,
    Command(AudioCommand),
}

enum Reply {
    Queued,
    Inline(ResultSnapshot),
    Rejected(AudioError),
}

#[derive(Clone, Default)]
enum WatchMode {
    #[default]
    Pending,
    ReadyInline,
    UnavailableInline(AudioError),
    Unsupported,
    Rejected(AudioError),
}

type Notification = Arc<dyn Fn(AudioEvent) + Send + Sync>;

#[derive(Default)]
struct RecordingState {
    requests: Vec<RecordedRequest>,
    replies: VecDeque<Reply>,
    completions: VecDeque<AudioCompletion>,
    watchers: Vec<Notification>,
    watch_mode: WatchMode,
    active: usize,
    maximum_active: usize,
    completed: usize,
}

#[derive(Default)]
pub(crate) struct RecordingAudio {
    state: Mutex<RecordingState>,
    events: Arc<Mutex<Vec<&'static str>>>,
}

struct WatchLease(Arc<Mutex<Vec<&'static str>>>);
impl Drop for WatchLease {
    fn drop(&mut self) {
        self.0.lock().push("unwatch");
    }
}

impl RecordingAudio {
    fn accept(
        &self,
        request: RecordedRequest,
        completion: AudioCompletion,
    ) -> Result<(), AudioError> {
        let reply = {
            let mut state = self.state.lock();
            state.requests.push(request);
            let reply = state.replies.pop_front().unwrap_or(Reply::Queued);
            if !matches!(reply, Reply::Rejected(_)) {
                state.active += 1;
                state.maximum_active = state.maximum_active.max(state.active);
            }
            if matches!(reply, Reply::Queued) {
                state.completions.push_back(completion);
                return Ok(());
            }
            if matches!(reply, Reply::Inline(_)) {
                state.active -= 1;
                state.completed += 1;
            }
            reply
        };
        match reply {
            Reply::Inline(result) => {
                completion(result);
                Ok(())
            }
            Reply::Rejected(error) => Err(error),
            Reply::Queued => unreachable!(),
        }
    }

    pub(crate) fn requests(&self) -> Vec<RecordedRequest> {
        self.state.lock().requests.clone()
    }

    pub(crate) fn watch_count(&self) -> usize {
        self.state.lock().watchers.len()
    }

    pub(crate) fn maximum_active(&self) -> usize {
        self.state.lock().maximum_active
    }

    fn reply(&self, reply: Reply) {
        self.state.lock().replies.push_back(reply);
    }

    fn take_completion(&self) -> AudioCompletion {
        let mut state = self.state.lock();
        let completion = state
            .completions
            .pop_front()
            .expect("one accepted queued request");
        state.active -= 1;
        state.completed += 1;
        completion
    }

    pub(crate) fn finish(&self, result: ResultSnapshot) {
        self.take_completion()(result);
    }

    /// The native actor invalidates watchers before handing back its confirmed
    /// command snapshot. Both deliveries may share a single UI wakeup.
    fn finish_after_changed(&self, result: ResultSnapshot) {
        self.event(AudioEvent::Changed);
        self.finish(result);
    }

    pub(crate) fn event(&self, event: AudioEvent) {
        let watcher = self
            .state
            .lock()
            .watchers
            .last()
            .cloned()
            .expect("watch setup accepted");
        watcher(event);
    }
}

impl AudioHost for RecordingAudio {
    fn read(&self, completion: AudioCompletion) -> Result<(), AudioError> {
        self.accept(RecordedRequest::Read, completion)
    }

    fn execute(
        &self,
        command: AudioCommand,
        completion: AudioCompletion,
    ) -> Result<(), AudioError> {
        self.accept(RecordedRequest::Command(command), completion)
    }

    fn subscribe(&self, notification: Notification) -> Result<Option<Box<dyn Send>>, AudioError> {
        let mode = {
            let mut state = self.state.lock();
            state.watchers.push(notification.clone());
            state.watch_mode.clone()
        };
        match mode {
            WatchMode::Unsupported => return Ok(None),
            WatchMode::Rejected(error) => return Err(error),
            WatchMode::ReadyInline => notification(AudioEvent::WatchReady),
            WatchMode::UnavailableInline(error) => {
                notification(AudioEvent::WatchUnavailable(error))
            }
            WatchMode::Pending => {}
        }
        self.events.lock().push("watch");
        Ok(Some(Box::new(WatchLease(Arc::clone(&self.events)))))
    }
}

struct RecordingDesktop {
    provider: Mutex<Result<Option<Arc<dyn AudioHost>>, AudioError>>,
    provider_calls: AtomicUsize,
    events: Arc<Mutex<Vec<&'static str>>>,
    deny_focus: AtomicBool,
    deny_attachment: AtomicBool,
    close_during_attachment: AtomicBool,
    media: Arc<RecordingPopupMedia>,
    media_provider_calls: AtomicUsize,
}

struct SurfaceLease(Arc<Mutex<Vec<&'static str>>>);
impl Drop for SurfaceLease {
    fn drop(&mut self) {
        self.0.lock().push("detach");
    }
}

impl DesktopHost for RecordingDesktop {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("Quick Settings must not observe the desktop")
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("Unexpected desktop activation")
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("Unexpected application launch")
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("Unexpected system command")
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("Audio must not save preferences")
    }

    fn audio_host(&self) -> Result<Option<Arc<dyn AudioHost>>, AudioError> {
        self.provider_calls.fetch_add(1, Ordering::Relaxed);
        self.provider.lock().clone()
    }

    fn media_host(&self) -> Result<Option<Arc<dyn MediaHost>>, MediaError> {
        self.media_provider_calls.fetch_add(1, Ordering::Relaxed);
        let hook = MEDIA_FACTORY_HOOK.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
        Ok(Some(self.media.clone()))
    }

    fn configure_surface(
        &self,
        kind: SurfaceKind,
        window: &slint::Window,
    ) -> Result<Option<Box<dyn std::any::Any>>, String> {
        assert_eq!(kind, SurfaceKind::Popup);
        assert!(window.is_visible());
        if self.deny_attachment.load(Ordering::Relaxed) {
            self.events.lock().push("attach-denied");
            return Err("Native popup attachment denied".into());
        }
        self.events.lock().push("attach");
        if self.close_during_attachment.load(Ordering::Relaxed) {
            window.dispatch_event(WindowEvent::CloseRequested);
            assert!(
                window.is_visible(),
                "attachment lease must drop before native hide"
            );
        }
        Ok(Some(Box::new(SurfaceLease(Arc::clone(&self.events)))))
    }

    fn request_ui_focus(&self, window: &slint::Window) -> Result<(), String> {
        assert!(window.is_visible());
        self.events.lock().push("focus");
        let hook = FOCUS_HOOK.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
        if self.deny_focus.load(Ordering::Relaxed) {
            Err("OS denied foreground".into())
        } else {
            Ok(())
        }
    }
}

fn setup() -> (
    Arc<RecordingDesktop>,
    Arc<RecordingAudio>,
    Panel,
    Rc<QuickSettingsController>,
) {
    i_slint_backend_testing::init_no_event_loop();
    let audio = Arc::new(RecordingAudio::default());
    let host = Arc::new(RecordingDesktop {
        provider: Mutex::new(Ok(Some(audio.clone()))),
        provider_calls: AtomicUsize::new(0),
        events: Arc::clone(&audio.events),
        deny_focus: AtomicBool::new(false),
        deny_attachment: AtomicBool::new(false),
        close_during_attachment: AtomicBool::new(false),
        media: Arc::new(RecordingPopupMedia {
            reads: AtomicUsize::new(0),
            events: Arc::clone(&audio.events),
            observation: Mutex::default(),
            commands: Mutex::default(),
        }),
        media_provider_calls: AtomicUsize::new(0),
    });
    let panel = Panel::new().unwrap();
    let dock = Dock::new().unwrap();
    let media = DockMediaController::new(host.clone(), &dock);
    let controller = QuickSettingsController::new(host.clone(), &panel, media).unwrap();
    (host, audio, panel, controller)
}

fn show(controller: &Rc<QuickSettingsController>) -> Result<(), String> {
    controller.show(
        PresentationTheme::uniform(slint::language::ColorScheme::Dark),
        PhysicalPosition::new(-960, 60),
        DockContext::new(-1920, 0, 1920, 1080, false).unwrap(),
        1.5,
    )
}

// This backend intentionally has no event loop. Invoke the same generated
// mailbox wakeup that the production proxy posts, after proving inline/worker
// delivery did not project synchronously. Timer advancement remains native.
fn drain(controller: &QuickSettingsController) {
    controller.surface.invoke_audio_event_ready();
}

fn advance(milliseconds: u64) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(milliseconds));
    slint::platform::update_timers_and_animations();
}

fn error(kind: AudioErrorKind, message: &str) -> AudioError {
    AudioError::new(kind, message)
}

fn ready(id: &str, percent: f32, muted: bool) -> EndpointState {
    EndpointState::Ready(AudioEndpoint {
        id: EndpointId::new(id.into()).unwrap(),
        volume: Volume::from_scalar(percent / 100.0).unwrap(),
        muted,
    })
}

pub(crate) fn snapshot(output: f32, input: f32) -> AudioSnapshot {
    AudioSnapshot {
        output: ready("output-A", output, false),
        input: ready("input-I", input, false),
    }
}

fn loaded(
    controller: &Rc<QuickSettingsController>,
    audio: &RecordingAudio,
    snapshot: AudioSnapshot,
) {
    show(controller).unwrap();
    audio.finish(Ok(snapshot));
    assert!(
        controller.surface.get_loading(),
        "queued completion must not touch UI on delivery"
    );
    drain(controller);
    assert!(!controller.surface.get_loading());
}

pub(crate) fn volume_command(flow: AudioFlow, id: &str, percent: f32) -> RecordedRequest {
    RecordedRequest::Command(AudioCommand::SetVolume {
        flow,
        expected_id: EndpointId::new(id.into()).unwrap(),
        volume: Volume::from_scalar(percent / 100.0).unwrap(),
    })
}

#[test]
fn provider_is_lazy_and_none_failure_and_each_flow_remain_truthful() {
    let (host, audio, _panel, controller) = setup();
    assert_eq!(host.provider_calls.load(Ordering::Relaxed), 0);
    *host.provider.lock() = Ok(None);
    show(&controller).unwrap();
    assert!(!controller.surface.get_loading());
    assert!(controller.surface.get_status().contains("not supported"));
    assert!(!controller.surface.get_output_ready());
    assert!(!controller.surface.get_input_ready());
    assert!(audio.requests().is_empty());
    advance(1000);
    assert_eq!(host.provider_calls.load(Ordering::Relaxed), 1);

    *host.provider.lock() = Err(error(
        AudioErrorKind::AccessDenied,
        "permission\n denied\u{202e}",
    ));
    controller.surface.invoke_refresh_requested();
    assert!(
        controller
            .surface
            .get_status()
            .contains("permission denied")
    );
    assert!(!controller.surface.get_status().contains('\u{202e}'));
    assert!(audio.requests().is_empty());

    *host.provider.lock() = Ok(Some(audio.clone()));
    controller.surface.invoke_refresh_requested();
    assert!(controller.surface.get_loading());
    assert_eq!(audio.requests(), [RecordedRequest::Read]);
    audio.finish(Ok(AudioSnapshot {
        output: ready("real-output", 37.4, false),
        input: EndpointState::Unavailable(error(
            AudioErrorKind::AccessDenied,
            "Microphone access denied",
        )),
    }));
    drain(&controller);
    assert!(controller.surface.get_output_ready());
    assert!((controller.surface.get_output_percent() - 37.4).abs() < 0.001);
    assert!(!controller.surface.get_input_ready());
    assert!(
        controller
            .surface
            .get_input_status()
            .contains("Microphone access denied")
    );
    assert!(
        !controller.surface.get_watch_live(),
        "an accepted guard is not watch readiness"
    );
    audio.event(AudioEvent::WatchReady);
    assert!(!controller.surface.get_watch_live());
    drain(&controller);
    assert!(controller.surface.get_watch_live());
    advance(10_000);
    assert_eq!(
        audio.requests(),
        [RecordedRequest::Read],
        "no idle audio or desktop polling"
    );
}

#[test]
fn inline_completion_and_immediate_rejection_cross_the_mailbox_and_restore_confirmed_feedback() {
    let (_host, audio, _panel, controller) = setup();
    audio.state.lock().watch_mode = WatchMode::ReadyInline;
    audio.reply(Reply::Inline(Ok(snapshot(40.0, 30.0))));
    show(&controller).unwrap();
    assert!(controller.surface.get_loading());
    assert!(!controller.surface.get_output_ready());
    assert!(!controller.surface.get_watch_live());
    drain(&controller);
    assert!(controller.surface.get_watch_live());
    assert_eq!(controller.surface.get_output_percent(), 40.0);

    audio.reply(Reply::Rejected(error(
        AudioErrorKind::AccessDenied,
        "write denied\n\u{202e}",
    )));
    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 85.0);
    assert_eq!(controller.surface.get_output_percent(), 85.0);
    assert!(controller.state.borrow().flight.is_some());
    assert_eq!(
        audio.state.lock().completed,
        1,
        "immediate Err never invokes its callback"
    );
    drain(&controller);
    assert_eq!(controller.surface.get_output_percent(), 40.0);
    assert!(controller.surface.get_status().contains("write denied"));
    assert!(controller.state.borrow().flight.is_none());
    assert_eq!(audio.requests().len(), 2);

    audio.reply(Reply::Inline(Err(error(
        AudioErrorKind::Other,
        &format!("read failed {}", "x".repeat(600)),
    ))));
    controller.surface.invoke_refresh_requested();
    assert!(
        !controller.surface.get_loading(),
        "confirmed controls stay interactive during background retry"
    );
    assert!(
        controller.state.borrow().flight.is_some(),
        "inline completion still crosses the mailbox"
    );
    assert!(
        !controller.surface.get_status().contains("read failed"),
        "no synchronous completion projection"
    );
    drain(&controller);
    assert!(!controller.surface.get_loading());
    assert!(controller.surface.get_status().contains("read failed"));
    assert!(controller.surface.get_status().chars().count() <= 321);
    assert_eq!(
        controller.surface.get_output_percent(),
        40.0,
        "retain genuine confirmed feedback"
    );
    assert_eq!(audio.requests().len(), 3);
}

#[test]
fn native_timer_coalesces_latest_percent_and_preserves_both_flows_and_absolute_mute_while_busy() {
    let (_host, audio, _panel, controller) = setup();
    loaded(&controller, &audio, snapshot(20.0, 30.0));
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 30.2);
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 40.6);
    advance(99);
    assert_eq!(audio.requests().len(), 1);
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 60.4);
    advance(1);
    assert_eq!(
        audio.requests().last(),
        Some(&volume_command(AudioFlow::Output, "output-A", 60.0))
    );
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 70.0);
    controller
        .surface
        .invoke_mute_requested(AudioRoute::Output, true);
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Input, 80.0);
    advance(100);
    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 77.6);
    assert_eq!(
        audio.requests().len(),
        2,
        "all later intentions stay bounded behind one flight"
    );
    assert_eq!(controller.surface.get_output_percent(), 78.0);
    assert!(controller.surface.get_output_muted());

    audio.finish(Ok(snapshot(60.0, 30.0)));
    drain(&controller);
    assert_eq!(
        audio.requests().last(),
        Some(&volume_command(AudioFlow::Input, "input-I", 80.0))
    );
    assert_eq!(
        controller.surface.get_output_percent(),
        78.0,
        "confirmed older volume cannot erase the final release"
    );
    audio.finish(Ok(snapshot(60.0, 80.0)));
    drain(&controller);
    assert_eq!(
        audio.requests().last(),
        Some(&RecordedRequest::Command(AudioCommand::SetMuted {
            flow: AudioFlow::Output,
            expected_id: EndpointId::new("output-A".into()).unwrap(),
            muted: true,
        }))
    );
    audio.finish(Ok(AudioSnapshot {
        output: ready("output-A", 60.0, true),
        input: ready("input-I", 80.0, false),
    }));
    drain(&controller);
    assert_eq!(
        audio.requests().last(),
        Some(&volume_command(AudioFlow::Output, "output-A", 78.0))
    );
    audio.finish(Ok(AudioSnapshot {
        output: ready("output-A", 78.0, true),
        input: ready("input-I", 80.0, false),
    }));
    drain(&controller);
    advance(1000);
    assert_eq!(audio.requests().len(), 5);
    assert_eq!(audio.state.lock().maximum_active, 1);
    assert!(!controller.volume_timer.running());
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 90.0);
    controller.hide();
    advance(1000);
    assert_eq!(
        audio.requests().len(),
        5,
        "closing cancels unsent intent, not a new hardware command"
    );
}

#[test]
fn release_flushes_explicit_quantized_intent_without_waiting_for_the_timer() {
    let (_host, audio, _panel, controller) = setup();
    loaded(&controller, &audio, snapshot(20.0, 30.0));
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 30.2);
    advance(20);
    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 31.6);
    assert_eq!(
        audio.requests(),
        [
            RecordedRequest::Read,
            volume_command(AudioFlow::Output, "output-A", 32.0)
        ]
    );
    assert!(!controller.volume_timer.running());
    audio.finish(Ok(snapshot(32.0, 30.0)));
    drain(&controller);
    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 31.6);
    advance(1000);
    assert_eq!(
        audio.requests().len(),
        2,
        "duplicate release is a confirmed no-op, not a command loop"
    );
}

#[test]
fn hidden_loading_missing_flow_and_nonfinite_or_out_of_range_input_never_reach_the_host() {
    let (_host, audio, _panel, controller) = setup();
    show(&controller).unwrap();
    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 50.0);
    controller
        .surface
        .invoke_mute_requested(AudioRoute::Output, true);
    assert_eq!(audio.requests(), [RecordedRequest::Read]);
    audio.finish(Ok(AudioSnapshot {
        output: ready("output-A", 40.0, false),
        input: EndpointState::Absent,
    }));
    drain(&controller);
    for percent in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 100.1] {
        controller
            .surface
            .invoke_volume_requested(AudioRoute::Output, percent);
        controller
            .surface
            .invoke_volume_released(AudioRoute::Output, percent);
    }
    controller
        .surface
        .invoke_volume_released(AudioRoute::Input, 50.0);
    controller
        .surface
        .invoke_mute_requested(AudioRoute::Input, true);
    assert_eq!(controller.surface.get_output_percent(), 40.0);
    // Generated property projection is silent; only genuine user callbacks
    // create intentions (renderer fixtures independently exercise native input).
    controller.surface.set_output_percent(72.0);
    controller.surface.set_output_muted(true);
    assert_eq!(audio.requests().len(), 1);
    controller.project();
    assert_eq!(controller.surface.get_output_percent(), 40.0);
    controller.hide();
    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 80.0);
    controller
        .surface
        .invoke_mute_requested(AudioRoute::Output, true);
    controller.surface.invoke_refresh_requested();
    advance(1000);
    assert_eq!(audio.requests(), [RecordedRequest::Read]);
}

#[test]
fn device_changed_discards_stale_flow_intentions_and_reads_before_any_remaining_command() {
    let (_host, audio, _panel, controller) = setup();
    loaded(&controller, &audio, snapshot(20.0, 30.0));
    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 50.0);
    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 60.0);
    controller
        .surface
        .invoke_mute_requested(AudioRoute::Output, true);
    controller
        .surface
        .invoke_volume_released(AudioRoute::Input, 80.0);
    assert_eq!(
        audio.requests().last(),
        Some(&volume_command(AudioFlow::Output, "output-A", 50.0))
    );
    audio.finish(Err(error(
        AudioErrorKind::DeviceChanged,
        "Default output changed",
    )));
    drain(&controller);
    assert_eq!(
        audio.requests(),
        [
            RecordedRequest::Read,
            volume_command(AudioFlow::Output, "output-A", 50.0),
            RecordedRequest::Read
        ]
    );
    assert!(
        !controller.surface.get_loading(),
        "the independent confirmed input remains usable during refresh"
    );
    assert!(controller.surface.get_input_ready());
    assert_eq!(
        controller.surface.get_output_percent(),
        20.0,
        "failed command restores confirmed feedback"
    );
    audio.finish(Ok(AudioSnapshot {
        output: ready("output-B", 25.0, false),
        input: ready("input-I", 30.0, false),
    }));
    drain(&controller);
    assert_eq!(controller.surface.get_output_percent(), 25.0);
    assert!(!controller.surface.get_output_muted());
    assert_eq!(
        audio.requests().last(),
        Some(&volume_command(AudioFlow::Input, "input-I", 80.0))
    );
    audio.finish(Ok(AudioSnapshot {
        output: ready("output-B", 25.0, false),
        input: ready("input-I", 80.0, false),
    }));
    drain(&controller);
    advance(1000);
    assert_eq!(
        audio.requests().len(),
        4,
        "no stale intention was retargeted to output-B"
    );
    assert_eq!(audio.state.lock().maximum_active, 1);
}

#[test]
fn change_bursts_during_commands_and_reads_coalesce_into_one_fresh_read_each() {
    let (_host, audio, _panel, controller) = setup();
    loaded(&controller, &audio, snapshot(20.0, 30.0));
    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 50.0);
    for _ in 0..100 {
        audio.event(AudioEvent::Changed);
    }
    drain(&controller);
    assert_eq!(audio.requests().len(), 2);
    assert!(
        !controller.surface.get_loading(),
        "a queued invalidation must not erase the active drag's release"
    );
    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 60.0);
    audio.finish(Ok(snapshot(50.0, 30.0)));
    drain(&controller);
    assert_eq!(audio.requests().last(), Some(&RecordedRequest::Read));
    for _ in 0..100 {
        audio.event(AudioEvent::Changed);
    }
    drain(&controller);
    assert_eq!(audio.requests().len(), 3);
    audio.finish(Ok(snapshot(50.0, 30.0)));
    drain(&controller);
    assert_eq!(audio.requests().len(), 4);
    assert_eq!(audio.requests().last(), Some(&RecordedRequest::Read));
    audio.finish(Ok(snapshot(50.0, 30.0)));
    drain(&controller);
    assert_eq!(
        audio.requests().last(),
        Some(&volume_command(AudioFlow::Output, "output-A", 60.0))
    );
    audio.finish(Ok(snapshot(60.0, 30.0)));
    drain(&controller);
    advance(10_000);
    assert_eq!(
        audio.requests().len(),
        5,
        "reads cannot turn into an unbounded read-command cycle"
    );
    assert_eq!(audio.state.lock().maximum_active, 1);
}

#[test]
fn hide_and_reopen_retire_old_flights_without_projecting_or_overlapping_them() {
    let (host, audio, _panel, controller) = setup();
    show(&controller).unwrap();
    let old_watcher = audio.state.lock().watchers[0].clone();
    let generation = controller.state.borrow().generation;
    controller.hide();
    assert_eq!(
        &*host.events.lock(),
        &["attach", "focus", "watch", "unwatch", "detach"]
    );
    old_watcher(AudioEvent::Changed);
    old_watcher(AudioEvent::WatchReady);
    drain(&controller);
    advance(1000);
    assert_eq!(audio.requests().len(), 1);
    show(&controller).unwrap();
    assert_ne!(controller.state.borrow().generation, generation);
    assert_eq!(
        audio.requests().len(),
        1,
        "reopen waits for the already accepted request"
    );
    audio.finish(Ok(snapshot(91.0, 92.0)));
    drain(&controller);
    assert_eq!(audio.requests().len(), 2);
    assert!(
        !controller.surface.get_output_ready(),
        "old generation cannot populate a reopened popup"
    );
    old_watcher(AudioEvent::Changed);
    old_watcher(AudioEvent::WatchReady);
    drain(&controller);
    assert!(!controller.surface.get_watch_live());
    let completion = audio.take_completion();
    std::thread::spawn(move || completion(Ok(snapshot(41.0, 42.0))))
        .join()
        .unwrap();
    assert!(
        !controller.surface.get_output_ready(),
        "worker completion must wait for UI mailbox processing"
    );
    drain(&controller);
    assert_eq!(controller.surface.get_output_percent(), 41.0);
    assert_eq!(audio.requests().len(), 2);
    assert_eq!(audio.state.lock().maximum_active, 1);
    assert_eq!(
        host.provider_calls.load(Ordering::Relaxed),
        1,
        "one cached audio provider, no second worker"
    );
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 60.0);
    controller.hide();
    advance(10_000);
    assert_eq!(audio.requests().len(), 2);
}

#[test]
fn watch_failure_clears_live_status_and_refresh_retries_without_inventing_endpoints() {
    let (host, audio, _panel, controller) = setup();
    audio.state.lock().watch_mode = WatchMode::ReadyInline;
    loaded(&controller, &audio, snapshot(20.0, 30.0));
    assert!(controller.surface.get_watch_live());
    let failed_watcher = audio.state.lock().watchers.last().cloned().unwrap();
    audio.event(AudioEvent::WatchUnavailable(error(
        AudioErrorKind::Other,
        &format!("registration failed\n\u{202e} {}", "x".repeat(600)),
    )));
    drain(&controller);
    assert!(!controller.surface.get_watch_live());
    assert!(controller.surface.get_status().contains("use Refresh"));
    assert!(!controller.surface.get_status().contains('\u{202e}'));
    assert!(controller.surface.get_status().chars().count() <= 321);
    assert_eq!(host.events.lock().last(), Some(&"unwatch"));
    assert_eq!(audio.requests().len(), 1);
    controller.surface.invoke_refresh_requested();
    assert!(
        !controller.surface.get_watch_live(),
        "inline WatchReady still waits for a UI turn"
    );
    drain(&controller);
    assert!(controller.surface.get_watch_live());
    audio.finish(Ok(snapshot(21.0, 31.0)));
    drain(&controller);
    assert!(controller.surface.get_status().is_empty());
    assert_eq!(controller.surface.get_output_percent(), 21.0);
    failed_watcher(AudioEvent::WatchUnavailable(error(
        AudioErrorKind::Other,
        "late old-watch failure",
    )));
    failed_watcher(AudioEvent::Changed);
    drain(&controller);
    assert!(
        controller.surface.get_watch_live(),
        "retry owns a new watch epoch, not the failed guard"
    );
    assert!(controller.surface.get_status().is_empty());
    assert_eq!(audio.requests().len(), 2);

    controller.hide();
    audio.state.lock().watch_mode =
        WatchMode::UnavailableInline(error(AudioErrorKind::Other, "setup failed"));
    show(&controller).unwrap();
    drain(&controller);
    assert!(!controller.surface.get_watch_live());
    assert!(controller.watch.borrow().is_none());
    audio.finish(Ok(snapshot(22.0, 32.0)));
    drain(&controller);
    assert!(controller.surface.get_status().contains("setup failed"));
    assert!(controller.surface.get_output_ready());
}

#[test]
fn unsupported_and_rejected_watch_setup_leave_manual_refresh_and_real_read_available() {
    let (_host, audio, _panel, controller) = setup();
    audio.state.lock().watch_mode = WatchMode::Unsupported;
    loaded(&controller, &audio, snapshot(20.0, 30.0));
    assert!(!controller.surface.get_watch_live());
    assert!(controller.watch.borrow().is_none());
    assert!(controller.surface.get_status().is_empty());
    controller.hide();
    audio.state.lock().watch_mode =
        WatchMode::Rejected(error(AudioErrorKind::AccessDenied, "callbacks denied"));
    show(&controller).unwrap();
    assert!(controller.surface.get_status().contains("callbacks denied"));
    audio.finish(Ok(snapshot(21.0, 31.0)));
    drain(&controller);
    assert!(controller.surface.get_output_ready());
    assert!(!controller.surface.get_watch_live());
    controller.surface.invoke_refresh_requested();
    audio.finish(Ok(snapshot(22.0, 32.0)));
    drain(&controller);
    assert_eq!(audio.requests().len(), 3);
}

#[test]
fn async_height_refit_preserves_native_lease_focus_generation_and_pending_command() {
    let (host, audio, _panel, controller) = setup();
    show(&controller).unwrap();
    let initial_height = controller.surface.window().size().height;
    let generation = controller.state.borrow().generation;
    audio.finish(Ok(snapshot(40.0, 30.0)));
    drain(&controller);
    let loaded_height = controller.surface.window().size().height;
    assert!(
        loaded_height > initial_height,
        "real rows must grow and refit the loading-only popup"
    );
    assert_eq!(controller.state.borrow().generation, generation);
    assert_eq!(
        &*host.events.lock(),
        &["attach", "focus", "watch"],
        "refitting must not reattach or steal focus"
    );
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 50.0);
    audio.event(AudioEvent::Changed);
    drain(&controller);
    advance(100);
    audio.finish(Ok(AudioSnapshot {
        output: ready("output-A", 40.0, false),
        input: EndpointState::Absent,
    }));
    drain(&controller);
    assert!(
        controller.surface.window().size().height < loaded_height,
        "replacing the input slider with its truthful absence status must refit while output intent survives"
    );
    assert_eq!(controller.state.borrow().generation, generation);
    assert_eq!(controller.surface.get_output_percent(), 50.0);
    assert_eq!(
        audio.requests().last(),
        Some(&volume_command(AudioFlow::Output, "output-A", 50.0))
    );
    assert_eq!(&*host.events.lock(), &["attach", "focus", "watch"]);
    assert!(controller.surface.global::<PopoverMotion>().get_presented());
    assert_eq!(
        controller.surface.window().size(),
        controller.rect.borrow().as_ref().unwrap().size
    );
    audio.finish(Ok(AudioSnapshot {
        output: ready("output-A", 50.0, false),
        input: EndpointState::Absent,
    }));
    drain(&controller);
    assert_eq!(audio.requests().len(), 3);
}

#[test]
fn footer_uses_real_weak_panel_and_drops_watch_and_native_lease_before_focus() {
    let (host, audio, panel, controller) = setup();
    loaded(&controller, &audio, snapshot(40.0, 30.0));
    assert!(!panel.window().is_visible());
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 80.0);
    host.deny_focus.store(true, Ordering::Relaxed);
    controller.surface.invoke_app_settings_requested();
    assert!(!controller.is_open());
    assert!(panel.window().is_visible());
    assert!(panel.get_status().contains("OS denied foreground"));
    assert_eq!(
        &*host.events.lock(),
        &["attach", "focus", "watch", "unwatch", "detach", "focus"]
    );
    advance(1000);
    assert_eq!(audio.requests(), [RecordedRequest::Read]);
    let weak = Rc::downgrade(&controller);
    let root = controller.surface.as_weak();
    drop(controller);
    assert!(weak.upgrade().is_none());
    assert!(
        root.upgrade().is_none(),
        "callbacks and panel handle must not retain the presenter"
    );
}

#[test]
fn native_close_escape_and_attachment_cancellation_hide_without_quitting_or_starting_audio() {
    let (host, audio, _panel, controller) = setup();
    host.close_during_attachment.store(true, Ordering::Relaxed);
    show(&controller).unwrap();
    assert!(!controller.is_open());
    assert!(!controller.surface.window().is_visible());
    assert_eq!(&*host.events.lock(), &["attach", "detach"]);
    assert_eq!(host.provider_calls.load(Ordering::Relaxed), 0);
    assert!(audio.requests().is_empty());
    host.close_during_attachment.store(false, Ordering::Relaxed);
    host.deny_attachment.store(true, Ordering::Relaxed);
    assert!(show(&controller).unwrap_err().contains("attachment denied"));
    assert!(!controller.is_open());
    assert!(audio.requests().is_empty());
    host.deny_attachment.store(false, Ordering::Relaxed);
    show(&controller).unwrap();
    let key = Key::Escape.into();
    controller
        .surface
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: key });
    assert!(!controller.is_open());
    assert!(!controller.surface.window().is_visible());
    audio.finish(Ok(snapshot(40.0, 30.0)));
    drain(&controller);
    assert_eq!(
        audio.requests().len(),
        1,
        "hidden completion retires its flight without another read"
    );
    show(&controller).unwrap();
    controller
        .surface
        .window()
        .dispatch_event(WindowEvent::CloseRequested);
    assert!(!controller.is_open());
    assert!(!controller.surface.window().is_visible());
    assert!(controller.watch.borrow().is_none());
    assert!(!controller.volume_timer.running());
    assert!(!controller.focus_watch.running());
    audio.finish(Ok(snapshot(41.0, 31.0)));
    drain(&controller);
    assert_eq!(audio.requests().len(), 2);
}

#[test]
fn a_fresh_read_drops_unissued_intent_when_the_displayed_default_identity_changes() {
    let (_host, audio, _panel, controller) = setup();
    loaded(&controller, &audio, snapshot(20.0, 30.0));
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 60.0);
    audio.event(AudioEvent::Changed);
    drain(&controller);
    advance(100);
    audio.finish(Ok(AudioSnapshot {
        output: ready("output-B", 25.0, false),
        input: ready("input-I", 30.0, false),
    }));
    drain(&controller);
    advance(1000);
    assert_eq!(controller.surface.get_output_percent(), 25.0);
    assert_eq!(
        audio.requests(),
        [RecordedRequest::Read, RecordedRequest::Read]
    );
    assert!(!controller.volume_timer.running());
}

#[test]
fn read_device_changed_failure_does_not_retry_forever_without_new_input() {
    let (_host, audio, _panel, controller) = setup();
    audio.reply(Reply::Inline(Err(error(
        AudioErrorKind::DeviceChanged,
        "Endpoint churn during read",
    ))));
    show(&controller).unwrap();
    drain(&controller);
    advance(10_000);
    assert_eq!(audio.requests(), [RecordedRequest::Read]);
    assert!(!controller.surface.get_loading());
    assert!(controller.surface.get_status().contains("Endpoint churn"));
    controller.surface.invoke_refresh_requested();
    audio.finish(Ok(snapshot(20.0, 30.0)));
    drain(&controller);
    assert!(controller.surface.get_output_ready());
    assert_eq!(audio.requests().len(), 2);
}

#[test]
fn ordinary_geometry_observations_preserve_intent_and_changed_native_geometry_cancels_it() {
    let (host, audio, _panel, controller) = setup();
    loaded(&controller, &audio, snapshot(20.0, 30.0));
    let generation = controller.state.borrow().generation;
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 60.0);
    controller
        .close_if_geometry_changed(DockContext::new(-1920, 0, 1920, 1080, false).unwrap(), 1.5);
    assert!(controller.is_open());
    assert_eq!(controller.state.borrow().generation, generation);
    assert!(controller.volume_timer.running());
    assert_eq!(&*host.events.lock(), &["attach", "focus", "watch"]);
    controller
        .close_if_geometry_changed(DockContext::new(-1280, 0, 1280, 720, false).unwrap(), 1.5);
    assert!(!controller.is_open());
    assert!(!controller.volume_timer.running());
    advance(1000);
    assert_eq!(audio.requests(), [RecordedRequest::Read]);

    loaded(&controller, &audio, snapshot(21.0, 31.0));
    controller
        .close_if_geometry_changed(DockContext::new(-1920, 0, 1920, 1080, false).unwrap(), 2.0);
    assert!(
        !controller.is_open(),
        "DPI change invalidates the stored physical tile anchor"
    );
    loaded(&controller, &audio, snapshot(22.0, 32.0));
    controller
        .close_if_geometry_changed(DockContext::new(-1920, 0, 1920, 1080, true).unwrap(), 1.5);
    assert!(
        !controller.is_open(),
        "fullscreen must dismiss the popup without audio work"
    );
    assert_eq!(audio.requests().len(), 3);
}

#[test]
fn os_foreground_denial_leaves_the_owned_popup_and_mouse_audio_commands_usable() {
    let (host, audio, _panel, controller) = setup();
    host.deny_focus.store(true, Ordering::Relaxed);
    assert!(show(&controller).unwrap_err().contains("popup opened"));
    assert!(controller.is_open());
    audio.finish(Ok(snapshot(20.0, 30.0)));
    drain(&controller);
    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 50.0);
    assert_eq!(
        audio.requests().last(),
        Some(&volume_command(AudioFlow::Output, "output-A", 50.0))
    );
    controller.hide();
    assert_eq!(host.events.lock().last(), Some(&"detach"));
    audio.finish(Ok(snapshot(50.0, 30.0)));
    drain(&controller);
    assert_eq!(audio.requests().len(), 2);
}

#[test]
fn native_change_before_completion_preserves_drag_and_final_release_during_background_read() {
    let (_host, audio, _panel, controller) = setup();
    loaded(&controller, &audio, snapshot(20.0, 30.0));
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 40.0);
    advance(100);
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 50.0);
    audio.finish_after_changed(Ok(snapshot(40.0, 30.0)));
    drain(&controller);
    assert_eq!(audio.requests().last(), Some(&RecordedRequest::Read));
    assert!(
        !controller.surface.get_loading(),
        "background confirmation read must not disable an already ready native Slider"
    );
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 65.0);
    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 70.6);
    assert_eq!(controller.surface.get_output_percent(), 71.0);
    assert_eq!(
        audio.requests().len(),
        3,
        "continued input remains serialized behind the read"
    );
    audio.finish(Ok(snapshot(40.0, 30.0)));
    drain(&controller);
    assert_eq!(
        audio.requests().last(),
        Some(&volume_command(AudioFlow::Output, "output-A", 71.0))
    );
    audio.finish_after_changed(Ok(snapshot(71.0, 30.0)));
    drain(&controller);
    audio.finish(Ok(snapshot(71.0, 30.0)));
    drain(&controller);
    advance(1000);
    assert_eq!(
        audio.requests().len(),
        5,
        "confirmed final release must not create a command loop"
    );

    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 85.0);
    audio.finish_after_changed(Ok(snapshot(85.0, 30.0)));
    drain(&controller);
    controller
        .surface
        .invoke_volume_released(AudioRoute::Output, 90.0);
    audio.finish(Ok(AudioSnapshot {
        output: ready("output-B", 25.0, false),
        input: ready("input-I", 30.0, false),
    }));
    drain(&controller);
    advance(1000);
    assert_eq!(controller.surface.get_output_percent(), 25.0);
    assert_eq!(
        audio.requests().len(),
        7,
        "release during background read is still bound to output-A and cannot retarget output-B"
    );
    assert_eq!(audio.state.lock().maximum_active, 1);
}

#[test]
fn preferred_size_notifications_coalesce_refit_and_cannot_revive_a_closed_session() {
    let (host, audio, _panel, controller) = setup();
    loaded(&controller, &audio, snapshot(40.0, 30.0));
    let height = controller.surface.window().size().height;
    let generation = controller.state.borrow().generation;
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 50.0);
    controller.surface.set_status(
        "Feedback requiring additional wrapped lines. "
            .repeat(5)
            .into(),
    );
    controller.surface.invoke_preferred_size_changed();
    controller.surface.invoke_preferred_size_changed();
    assert!(controller.fit_timer.running());
    assert_eq!(
        controller.surface.window().size().height,
        height,
        "notification refit is deferred, not layout-recursive"
    );
    advance(0);
    assert!(!controller.fit_timer.running());
    assert!(
        controller.surface.window().size().height > height,
        "wrapped feedback must actually refit native height"
    );
    assert_eq!(controller.state.borrow().generation, generation);
    assert_eq!(controller.surface.get_output_percent(), 50.0);
    assert_eq!(&*host.events.lock(), &["attach", "focus", "watch"]);
    assert_eq!(audio.requests(), [RecordedRequest::Read]);

    controller.surface.set_status("Short feedback".into());
    controller.surface.invoke_preferred_size_changed();
    assert!(controller.fit_timer.running());
    controller.hide();
    assert!(!controller.fit_timer.running());
    let hidden_size = controller.surface.window().size();
    controller.surface.invoke_preferred_size_changed();
    advance(0);
    assert!(!controller.is_open());
    assert_eq!(controller.surface.window().size(), hidden_size);
    assert_eq!(host.events.lock().last(), Some(&"detach"));
    assert_eq!(
        audio.requests(),
        [RecordedRequest::Read],
        "geometry notifications never start audio work"
    );
}

#[test]
fn media_attachment_requires_authorized_native_show_and_retires_before_audio_and_surface() {
    let (host, audio, _panel, controller) = setup();
    controller.attach_media();
    controller.surface.set_timeline_available(true);
    controller.surface.set_timeline_time("0:30 / 3:00".into());
    controller
        .surface
        .invoke_media_action_requested(MediaAction::Toggle, "untrusted".into());
    assert!(!controller.media_input_ready());
    assert_eq!(host.media_provider_calls.load(Ordering::Relaxed), 0);

    host.deny_attachment.store(true, Ordering::Relaxed);
    assert!(show(&controller).is_err());
    controller.attach_media();
    assert!(!controller.media_input_ready());
    assert_eq!(host.media_provider_calls.load(Ordering::Relaxed), 0);
    host.deny_attachment.store(false, Ordering::Relaxed);
    host.close_during_attachment.store(true, Ordering::Relaxed);
    show(&controller).unwrap();
    controller.attach_media();
    assert!(!controller.media_input_ready());
    assert_eq!(host.media_provider_calls.load(Ordering::Relaxed), 0);
    host.close_during_attachment.store(false, Ordering::Relaxed);

    // OS focus denial is distinct from a genuinely visible native popup.
    host.deny_focus.store(true, Ordering::Relaxed);
    assert!(show(&controller).is_err());
    assert!(controller.is_open());
    assert!(
        !controller.media_input_ready(),
        "show itself never attaches media"
    );
    assert_eq!(host.media_provider_calls.load(Ordering::Relaxed), 0);
    controller.attach_media();
    controller.attach_media();
    assert!(controller.media_input_ready());
    assert_eq!(host.media_provider_calls.load(Ordering::Relaxed), 1);
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 1);
    controller.media.process_events();
    controller.request_media(MediaAction::Toggle, "not-current");
    assert!(controller.surface.get_media_view().enabled);

    host.events.lock().clear();
    controller.hide();
    assert!(!controller.media_input_ready());
    assert!(!controller.surface.get_media_view().enabled);
    assert!(!controller.surface.get_timeline_available());
    assert!(controller.surface.get_timeline_time().is_empty());
    assert_eq!(
        &*host.events.lock(),
        &["media-unwatch", "unwatch", "detach"],
        "shared media demand must retire before audio watch and native lease"
    );
    controller.request_media(MediaAction::Next, "not-current");
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 1);
    audio.finish(Ok(snapshot(30.0, 20.0)));
    drain(&controller);
}

#[test]
fn pending_media_attachment_reentry_cannot_detach_the_newer_same_window_token() {
    let (host, _audio, _panel, controller) = setup();
    show(&controller).unwrap();
    let reopened = controller.clone();
    MEDIA_FACTORY_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            assert!(
                reopened.media_input_ready(),
                "token is installed before provider effects"
            );
            reopened.hide();
            show(&reopened).unwrap();
            reopened.attach_media();
            assert!(reopened.media_input_ready());
        }));
    });
    controller.attach_media();
    assert!(controller.is_open());
    assert!(controller.media_input_ready());
    assert!(controller.surface.get_media_view().enabled);
    assert_eq!(host.media_provider_calls.load(Ordering::Relaxed), 2);
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 1);
    controller.media.process_events();
    assert_eq!(controller.surface.get_media_view().status, "Not playing");
    controller.hide();
    assert!(!controller.media_input_ready());
}

#[test]
fn media_watch_retirement_reentry_preserves_new_attachment_audio_watch_and_native_lease() {
    let (host, audio, _panel, controller) = setup();
    loaded(&controller, &audio, snapshot(30.0, 20.0));
    controller.attach_media();
    controller.media.process_events();
    let old_token = controller.media_attachment.get().unwrap();
    let reopened = controller.clone();
    MEDIA_RETIRE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            show(&reopened).unwrap();
            reopened.attach_media();
        }));
    });
    controller.hide();
    assert!(controller.is_open());
    assert!(controller.media_input_ready());
    assert!(controller.surface.get_media_view().enabled);
    assert!(controller.watch.borrow().is_some());
    assert_ne!(controller.media_attachment.get(), Some(old_token));
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 2);
    controller.media.detach_popup(old_token);
    controller.media.process_events();
    assert!(controller.media_input_ready());
    assert_eq!(controller.surface.get_media_view().status, "Not playing");
    host.events.lock().clear();
    controller.hide();
    assert_eq!(
        &*host.events.lock(),
        &["media-unwatch", "unwatch", "detach"]
    );
}

#[test]
fn scoped_show_stops_after_focus_callback_replaces_the_root_popup_operation() {
    let (host, audio, _panel, controller) = setup();
    let current = Rc::new(Cell::new(true));
    let replaced = current.clone();
    let reopened = controller.clone();
    FOCUS_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            replaced.set(false);
            reopened.hide();
            show(&reopened).unwrap();
            reopened.attach_media();
        }));
    });
    controller
        .show_scoped(
            PresentationTheme::uniform(slint::language::ColorScheme::Dark),
            PhysicalPosition::new(-960, 60),
            DockContext::new(-1920, 0, 1920, 1080, false).unwrap(),
            1.5,
            || current.get(),
        )
        .unwrap();
    assert!(controller.is_open());
    assert!(controller.media_input_ready());
    assert_eq!(audio.requests(), [RecordedRequest::Read]);
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 1);
    let token = controller.media_attachment.get();
    controller
        .show_scoped(
            PresentationTheme::uniform(slint::language::ColorScheme::Light),
            PhysicalPosition::new(0, 0),
            DockContext::new(0, 0, 1920, 1080, false).unwrap(),
            1.0,
            || false,
        )
        .unwrap();
    assert!(controller.is_open());
    assert_eq!(controller.media_attachment.get(), token);
    controller.hide();
}

#[test]
fn late_media_and_timeline_refit_preserves_audio_intent_attachment_and_native_lease() {
    let (host, audio, _panel, controller) = setup();
    loaded(&controller, &audio, snapshot(40.0, 30.0));
    controller.attach_media();
    controller.media.process_events();
    let initial_height = controller.surface.window().size().height;
    let generation = controller.state.borrow().generation;
    let token = controller.media_attachment.get();
    controller
        .surface
        .invoke_volume_requested(AudioRoute::Output, 55.0);
    *host.media.observation.lock() = MediaSnapshot {
        current: Some(MediaSession {
            key: MediaSessionKey::issue().unwrap(),
            source_app_id: "actual.player".into(),
            title: "Actual title".into(),
            author: "Actual artist".into(),
            playback: MediaPlayback::Paused,
            capabilities: MediaCapabilities {
                previous: true,
                toggle: true,
                next: true,
            },
            timeline: Ok(MediaTimeline {
                start_ticks: 0,
                end_ticks: 1_800_000_000,
                position_ticks: 300_000_000,
                min_seek_ticks: 0,
                max_seek_ticks: 1_800_000_000,
                last_updated_utc_ticks: Some(123),
            }),
            seek: Err(MediaError::new(
                MediaErrorKind::Unsupported,
                "Seek not exposed by lifecycle fixture",
            )),
            artwork: None,
            artwork_notice: None,
        }),
    };
    controller.retry_media();
    host.events.lock().clear();
    controller.media.process_events();
    let size = controller.surface.window().size();
    assert!(
        size.height > initial_height,
        "completed domain batch must immediately refit the native viewport"
    );
    assert!(controller.surface.get_timeline_available());
    assert_eq!(
        controller.surface.get_timeline_time(),
        "0:30 / 3:00 · observed"
    );
    for label in ["Previous track", "Play", "Next track"] {
        let node = ElementHandle::find_by_accessible_label(&controller.component(), label)
            .next()
            .unwrap_or_else(|| {
                panic!("completed projection must immediately expose real AX transport: {label}")
            });
        assert_eq!(node.accessible_role(), Some(AccessibleRole::Button));
        assert_eq!(node.accessible_enabled(), Some(true));
        let origin = node.absolute_position();
        let bounds = node.size();
        assert!(bounds.width > 0.0 && bounds.height > 0.0);
        assert!(origin.x >= 0.0 && origin.y >= 0.0);
        assert!(origin.x + bounds.width <= size.width as f32);
        assert!(
            origin.y + bounds.height <= size.height as f32,
            "real {label} must not remain clipped by the old native viewport"
        );
    }
    assert_eq!(controller.state.borrow().generation, generation);
    assert_eq!(controller.media_attachment.get(), token);
    assert!(controller.media_input_ready());
    assert_eq!(controller.surface.get_output_percent(), 55.0);
    assert!(
        host.events.lock().is_empty(),
        "late fit cannot reattach or steal focus"
    );
    assert_eq!(audio.requests(), [RecordedRequest::Read]);
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 2);
    controller.hide();
    let hidden_size = controller.surface.window().size();
    controller.surface.invoke_media_projection_complete();
    assert_eq!(controller.surface.window().size(), hidden_size);
    controller.surface.invoke_preferred_size_changed();
    advance(0);
    assert!(!controller.is_open());
    assert_eq!(controller.surface.window().size(), hidden_size);
    assert!(!controller.media_input_ready());
}

#[test]
fn existing_refresh_requires_root_admission_before_attached_media_observation() {
    let (host, audio, _panel, controller) = setup();
    loaded(&controller, &audio, snapshot(40.0, 30.0));
    controller.attach_media();
    controller.media.process_events();
    controller.surface.invoke_refresh_requested();
    assert_eq!(
        host.media.reads.load(Ordering::Relaxed),
        1,
        "constructor cannot admit media refresh"
    );
    let admitted = Rc::new(Cell::new(false));
    let root_admission = admitted.clone();
    let weak = Rc::downgrade(&controller);
    controller.surface.on_media_refresh_requested(move || {
        if root_admission.get()
            && let Some(controller) = weak.upgrade()
        {
            controller.retry_media();
        }
    });
    controller.surface.invoke_refresh_requested();
    assert_eq!(
        host.media.reads.load(Ordering::Relaxed),
        1,
        "retired Root/Toolbar rejects media refresh"
    );
    admitted.set(true);
    controller.surface.invoke_refresh_requested();
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 2);
    assert_eq!(
        audio.requests(),
        [RecordedRequest::Read, RecordedRequest::Read]
    );
    assert_eq!(host.media_provider_calls.load(Ordering::Relaxed), 1);
    controller.media.process_events();
    controller.hide();
    controller.surface.invoke_refresh_requested();
    controller.surface.invoke_media_refresh_requested();
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 2);
    assert_eq!(
        audio.requests(),
        [RecordedRequest::Read, RecordedRequest::Read]
    );
    assert!(!controller.media_input_ready());
}

#[test]
fn scoped_media_attachment_rechecks_root_admission_after_factory_before_watch_and_read() {
    let (host, _audio, _panel, controller) = setup();
    show(&controller).unwrap();
    let admitted = Rc::new(Cell::new(false));
    let current_scope = admitted.clone();
    let admission: Rc<dyn Fn() -> bool> = Rc::new(move || current_scope.get());
    controller.attach_media_scoped(admission.clone());
    assert!(!controller.media_input_ready());
    assert_eq!(host.media_provider_calls.load(Ordering::Relaxed), 0);
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 0);

    admitted.set(true);
    let retired_scope = admitted.clone();
    MEDIA_FACTORY_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || retired_scope.set(false)));
    });
    controller.attach_media_scoped(admission.clone());
    assert!(
        controller.is_open(),
        "local native visibility is not Root source authority"
    );
    assert!(!controller.media_input_ready());
    assert_eq!(host.media_provider_calls.load(Ordering::Relaxed), 1);
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 0);
    assert!(
        host.events
            .lock()
            .iter()
            .all(|event| *event != "media-watch" && *event != "media-read"),
        "factory retirement must reject all subsequent native subscription/read effects"
    );
    controller.retry_media();
    assert_eq!(host.media_provider_calls.load(Ordering::Relaxed), 1);

    admitted.set(true);
    controller.attach_media_scoped(admission);
    assert!(controller.media_input_ready());
    assert_eq!(host.media_provider_calls.load(Ordering::Relaxed), 2);
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 1);
    controller.media.process_events();
    assert_eq!(controller.surface.get_media_view().status, "Not playing");
    controller.hide();
}

#[test]
fn scoped_media_admission_callback_reentry_preserves_new_popup_attachment() {
    let (host, _audio, _panel, controller) = setup();
    show(&controller).unwrap();
    let first_check = Cell::new(true);
    let weak = Rc::downgrade(&controller);
    let admission: Rc<dyn Fn() -> bool> = Rc::new(move || {
        if first_check.replace(false) {
            let reopened = weak.upgrade().unwrap();
            reopened.hide();
            show(&reopened).unwrap();
            reopened.attach_media();
        }
        true
    });
    controller.attach_media_scoped(admission);
    assert!(controller.is_open());
    assert!(controller.media_input_ready());
    assert_eq!(host.media_provider_calls.load(Ordering::Relaxed), 1);
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 1);
    controller.media.process_events();
    assert_eq!(controller.surface.get_media_view().status, "Not playing");
    controller.hide();
}

#[test]
fn durable_media_admission_retirement_disables_input_without_replaying_accepted_read() {
    let (host, _audio, _panel, controller) = setup();
    show(&controller).unwrap();
    let admitted = Rc::new(Cell::new(true));
    let current_scope = admitted.clone();
    let admission: Rc<dyn Fn() -> bool> = Rc::new(move || current_scope.get());
    controller.attach_media_scoped(admission.clone());
    assert!(controller.media_input_ready());
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 1);
    controller.surface.set_timeline_available(true);
    controller.surface.set_timeline_time("0:30 / 3:00".into());
    controller
        .surface
        .set_timeline_notice("Recorded timeline notice".into());
    admitted.set(false);
    assert!(!controller.media_input_ready());
    assert!(
        controller.is_open(),
        "actual native visibility alone cannot revive retired Root authority"
    );
    assert!(!controller.surface.get_media_view().enabled);
    assert!(!controller.surface.get_timeline_available());
    assert!(controller.surface.get_timeline_time().is_empty());
    assert!(controller.surface.get_timeline_notice().is_empty());
    controller.media.process_events();
    assert_eq!(
        host.media.reads.load(Ordering::Relaxed),
        1,
        "accepted read retires without replay"
    );
    admitted.set(true);
    controller.retry_media();
    controller.request_media(MediaAction::Toggle, "retired-source");
    assert!(!controller.media_input_ready());
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 1);

    show(&controller).unwrap();
    controller.attach_media_scoped(admission);
    assert!(controller.media_input_ready());
    assert_eq!(host.media.reads.load(Ordering::Relaxed), 2);
    controller.media.process_events();
    assert_eq!(controller.surface.get_media_view().status, "Not playing");
    controller.hide();
}
