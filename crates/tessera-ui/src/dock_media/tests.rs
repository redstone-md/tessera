// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use tessera_system::media::{
    MediaCapabilities, MediaCommandCompletion, MediaReadCompletion, MediaSession, MediaSessionKey,
};

#[path = "retirement_tests.rs"]
mod retirement;

type Hook = RefCell<Option<Box<dyn FnOnce()>>>;
thread_local! {
    static BACKEND: Cell<bool> = const { Cell::new(false) };
    static FACTORY_HOOK: Hook = RefCell::default();
    static READ_HOOK: Hook = RefCell::default();
    static COMMAND_HOOK: Hook = RefCell::default();
    static WATCH_HOOK: Hook = RefCell::default();
}

fn hook(slot: &'static std::thread::LocalKey<Hook>) {
    let callback = slot.with(|slot| slot.borrow_mut().take());
    if let Some(callback) = callback {
        callback();
    }
}

enum ReadReply {
    Pending,
    Inline(Result<MediaSnapshot, MediaError>),
    Reject(MediaError),
}

enum CommandReply {
    Pending,
    Inline(Result<(), MediaError>),
    Reject(MediaError),
}

type Changed = Arc<dyn Fn(MediaEvent) + Send + Sync>;

#[derive(Default)]
struct RecordingMedia {
    reads: AtomicUsize,
    commands: Mutex<Vec<MediaCommand>>,
    pending_reads: Mutex<VecDeque<MediaReadCompletion>>,
    pending_commands: Mutex<VecDeque<MediaCommandCompletion>>,
    read_replies: Mutex<VecDeque<ReadReply>>,
    command_replies: Mutex<VecDeque<CommandReply>>,
    changed: Mutex<Vec<Changed>>,
    watch_drops: Arc<AtomicUsize>,
    watch_error: Mutex<Option<MediaError>>,
}

struct Watch(Arc<AtomicUsize>);
impl Drop for Watch {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl RecordingMedia {
    fn finish_read(&self, result: Result<MediaSnapshot, MediaError>) {
        let completion = self
            .pending_reads
            .lock()
            .pop_front()
            .expect("accepted read");
        completion(result);
    }
    fn finish_command(&self, result: Result<(), MediaError>) {
        let completion = self
            .pending_commands
            .lock()
            .pop_front()
            .expect("accepted command");
        completion(result);
    }
    fn event(&self, event: MediaEvent) {
        let callback = self.changed.lock().last().cloned().expect("watch");
        callback(event);
    }
    fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }
}

impl MediaHost for RecordingMedia {
    fn read(&self, completion: MediaReadCompletion) -> Result<(), MediaError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let reply = self
            .read_replies
            .lock()
            .pop_front()
            .unwrap_or(ReadReply::Pending);
        match reply {
            ReadReply::Pending => self.pending_reads.lock().push_back(completion),
            ReadReply::Inline(result) => completion(result),
            ReadReply::Reject(error) => {
                hook(&READ_HOOK);
                return Err(error);
            }
        }
        hook(&READ_HOOK);
        Ok(())
    }
    fn execute(
        &self,
        command: MediaCommand,
        completion: MediaCommandCompletion,
    ) -> Result<(), MediaError> {
        self.commands.lock().push(command);
        let reply = self
            .command_replies
            .lock()
            .pop_front()
            .unwrap_or(CommandReply::Pending);
        match reply {
            CommandReply::Pending => self.pending_commands.lock().push_back(completion),
            CommandReply::Inline(result) => completion(result),
            CommandReply::Reject(error) => {
                hook(&COMMAND_HOOK);
                return Err(error);
            }
        }
        hook(&COMMAND_HOOK);
        Ok(())
    }
    fn subscribe(&self, changed: Changed) -> Result<Option<Box<dyn Send>>, MediaError> {
        if let Some(error) = self.watch_error.lock().clone() {
            return Err(error);
        }
        self.changed.lock().push(changed.clone());
        changed(MediaEvent::WatchReady);
        hook(&WATCH_HOOK);
        Ok(Some(Box::new(Watch(self.watch_drops.clone()))))
    }
}

struct RecordingDesktop {
    media: Mutex<Result<Option<Arc<dyn MediaHost>>, MediaError>>,
    acquisitions: AtomicUsize,
}

impl DesktopHost for RecordingDesktop {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("media cannot observe desktop");
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("media cannot activate");
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("source id is not launch authority");
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("media cannot dispatch system action");
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("parent owns persistence");
    }
    fn subscribe(&self, _: Arc<dyn Fn() + Send + Sync>) -> Result<Option<Box<dyn Send>>, String> {
        panic!("media cannot watch desktop");
    }
    fn media_host(&self) -> Result<Option<Arc<dyn MediaHost>>, MediaError> {
        self.acquisitions.fetch_add(1, Ordering::Relaxed);
        hook(&FACTORY_HOOK);
        self.media.lock().clone()
    }
}

struct Fixture {
    dock: Dock,
    controller: Rc<DockMediaController>,
    host: Arc<RecordingDesktop>,
    media: Arc<RecordingMedia>,
}

impl Fixture {
    fn new() -> Self {
        BACKEND.with(|initialized| {
            if !initialized.replace(true) {
                i_slint_backend_testing::init_no_event_loop();
            }
        });
        let dock = Dock::new().unwrap();
        dock.show().unwrap();
        let media = Arc::new(RecordingMedia::default());
        let host = Arc::new(RecordingDesktop {
            media: Mutex::new(Ok(Some(media.clone()))),
            acquisitions: AtomicUsize::new(0),
        });
        let controller = DockMediaController::new(host.clone(), &dock);
        Self {
            dock,
            controller,
            host,
            media,
        }
    }
    fn drain(&self) {
        self.controller.process_events();
    }
    fn enable(&self, session: MediaSession) {
        self.controller.set_enabled(true);
        self.media.finish_read(Ok(MediaSnapshot {
            current: Some(session),
        }));
        self.drain();
    }
    fn view(&self) -> DockMediaView {
        self.dock.get_media_view()
    }
}

fn error(kind: MediaErrorKind) -> MediaError {
    MediaError::new(kind, "Recorded native failure")
}
fn session(playback: MediaPlayback) -> MediaSession {
    MediaSession {
        key: MediaSessionKey::issue().unwrap(),
        source_app_id: "actual.player".into(),
        title: "Actual title 後".into(),
        author: "Actual author".into(),
        playback,
        capabilities: MediaCapabilities {
            previous: true,
            toggle: true,
            next: true,
        },
        artwork: None,
        artwork_notice: None,
        timeline: Err(error(MediaErrorKind::Unavailable)),
    }
}

#[test]
fn dock_media_lazy_enable_and_none_is_not_a_failed_observation() {
    let fixture = Fixture::new();
    assert_eq!(fixture.host.acquisitions.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.media.reads(), 0);
    fixture.controller.set_enabled(false);
    fixture.controller.retry();
    fixture.controller.request(MediaAction::Toggle);
    assert_eq!(fixture.media.reads(), 0);
    fixture.controller.set_enabled(true);
    assert_eq!(fixture.view().status, "Loading media");
    fixture
        .media
        .finish_read(Ok(MediaSnapshot { current: None }));
    fixture.drain();
    assert_eq!(fixture.view().status, "Not playing");
    assert!(!fixture.view().current_present);
    fixture.controller.retry();
    fixture
        .media
        .finish_read(Err(error(MediaErrorKind::Unavailable)));
    fixture.drain();
    assert_eq!(fixture.view().status, "Media unavailable");
    assert!(fixture.view().stale);
    assert!(!fixture.view().read_notice.is_empty());
    assert_eq!(fixture.media.reads(), 2);
    fixture.drain();
    assert_eq!(
        fixture.media.reads(),
        2,
        "failed read cannot create idle retries"
    );
}

#[test]
fn dock_media_command_exact_incarnation_single_flight_and_authoritative_readback() {
    let fixture = Fixture::new();
    let initial = session(MediaPlayback::Paused);
    let key = initial.key;
    fixture.enable(initial);
    let observed_epoch = fixture.view().session_identity;
    fixture.controller.request(MediaAction::Toggle);
    fixture.controller.request(MediaAction::Toggle);
    assert_eq!(
        &*fixture.media.commands.lock(),
        &[MediaCommand {
            expected_session: key,
            action: HostAction::Toggle
        }]
    );
    assert!(fixture.view().busy);
    let command_epoch = fixture.view().session_identity;
    assert_ne!(
        observed_epoch, command_epoch,
        "transport admission cancels prior held input"
    );
    assert_eq!(fixture.view().playback, "Paused");
    assert!(
        !fixture.view().playing,
        "request is not an observed playback change"
    );
    fixture.media.finish_command(Ok(()));
    fixture.drain();
    assert_eq!(fixture.media.reads(), 2);
    assert!(fixture.view().busy);
    assert_eq!(fixture.view().title, "Actual title 後");
    assert!(!fixture.view().playing);
    let observed = MediaSession {
        key,
        playback: MediaPlayback::Playing,
        ..session(MediaPlayback::Paused)
    };
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(observed),
    }));
    fixture.drain();
    assert!(fixture.view().playing);
    assert!(!fixture.view().busy);
    assert_ne!(
        command_epoch,
        fixture.view().session_identity,
        "same-session readback keeps a new input epoch even if busy was coalesced"
    );
    assert_eq!(fixture.media.commands.lock().len(), 1);
}

#[test]
fn dock_media_capabilities_and_already_queued_dirty_suppress_actions() {
    let fixture = Fixture::new();
    let observed = MediaSession {
        capabilities: MediaCapabilities {
            previous: false,
            toggle: true,
            next: false,
        },
        ..session(MediaPlayback::Playing)
    };
    fixture.enable(observed);
    fixture.controller.request(MediaAction::Previous);
    fixture.controller.request(MediaAction::Next);
    assert!(fixture.media.commands.lock().is_empty());
    fixture.media.event(MediaEvent::Changed);
    fixture.controller.request(MediaAction::Toggle);
    assert!(
        fixture.media.commands.lock().is_empty(),
        "unprocessed invalidation is still known stale input"
    );
    fixture.drain();
    assert!(fixture.view().busy);
    assert!(fixture.view().stale);
    assert_eq!(fixture.media.reads(), 2);
}

#[test]
fn dock_media_dirty_storm_queues_one_read_after_read_or_command() {
    let fixture = Fixture::new();
    fixture.controller.set_enabled(true);
    for _ in 0..1000 {
        fixture.media.event(MediaEvent::Changed);
    }
    fixture.drain();
    assert_eq!(fixture.media.reads(), 1);
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(session(MediaPlayback::Opened)),
    }));
    fixture.drain();
    assert_eq!(fixture.media.reads(), 2);
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(session(MediaPlayback::Opened)),
    }));
    fixture.drain();
    assert!(!fixture.view().stale);
    fixture.controller.request(MediaAction::Next);
    for _ in 0..1000 {
        fixture.media.event(MediaEvent::Changed);
    }
    fixture.drain();
    assert_eq!(
        fixture.media.reads(),
        2,
        "read cannot run alongside transport"
    );
    fixture.media.finish_command(Ok(()));
    fixture.drain();
    assert_eq!(fixture.media.reads(), 3);
    assert_eq!(fixture.media.commands.lock().len(), 1);
}

#[test]
fn dock_media_disable_reenable_out_of_order_and_late_watch_are_generation_gated() {
    let fixture = Fixture::new();
    fixture.controller.set_enabled(true);
    let old_watch = fixture.media.changed.lock()[0].clone();
    fixture.controller.set_enabled(false);
    assert_eq!(fixture.media.watch_drops.load(Ordering::Relaxed), 1);
    fixture.controller.set_enabled(true);
    assert_eq!(
        fixture.media.reads(),
        1,
        "reopen waits for the accepted old read"
    );
    fixture
        .media
        .finish_read(Ok(MediaSnapshot { current: None }));
    fixture.drain();
    assert_eq!(fixture.media.reads(), 2);
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(session(MediaPlayback::Playing)),
    }));
    fixture.drain();
    old_watch(MediaEvent::Changed);
    old_watch(MediaEvent::WatchUnavailable(error(
        MediaErrorKind::WatchUnavailable,
    )));
    fixture.drain();
    assert!(fixture.view().playing);
    assert!(fixture.view().watch_notice.is_empty());
    assert_eq!(fixture.media.reads(), 2);
    fixture.controller.close();
    fixture.media.event(MediaEvent::Changed);
    fixture.controller.retry();
    fixture.controller.set_enabled(true);
    fixture.drain();
    assert!(!fixture.view().enabled);
    assert_eq!(fixture.media.reads(), 2);
    assert_eq!(fixture.media.watch_drops.load(Ordering::Relaxed), 2);
}

#[test]
fn dock_media_inline_reentrant_hosts_do_not_borrow_state_across_calls() {
    let fixture = Fixture::new();
    fixture
        .media
        .read_replies
        .lock()
        .push_back(ReadReply::Inline(Ok(MediaSnapshot {
            current: Some(session(MediaPlayback::Paused)),
        })));
    let weak = Rc::downgrade(&fixture.controller);
    FACTORY_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            let controller = weak.upgrade().unwrap();
            controller.process_events();
            controller.request(MediaAction::Toggle);
        }))
    });
    let weak = Rc::downgrade(&fixture.controller);
    WATCH_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || weak.upgrade().unwrap().process_events()))
    });
    let weak = Rc::downgrade(&fixture.controller);
    READ_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || weak.upgrade().unwrap().process_events()))
    });
    fixture.controller.set_enabled(true);
    fixture.drain();
    assert_eq!(fixture.view().playback, "Paused");
    fixture
        .media
        .command_replies
        .lock()
        .push_back(CommandReply::Inline(Ok(())));
    let weak = Rc::downgrade(&fixture.controller);
    COMMAND_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            let controller = weak.upgrade().unwrap();
            controller.request(MediaAction::Toggle);
            controller.process_events();
        }))
    });
    fixture.controller.request(MediaAction::Toggle);
    fixture.drain();
    assert_eq!(fixture.media.commands.lock().len(), 1);
    assert_eq!(fixture.media.reads(), 2);
}

#[test]
fn dock_media_reentrant_disable_reenable_during_command_starts_new_generation_read() {
    let fixture = Fixture::new();
    fixture.enable(session(MediaPlayback::Paused));
    let weak = Rc::downgrade(&fixture.controller);
    COMMAND_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            let controller = weak.upgrade().unwrap();
            controller.set_enabled(false);
            controller.set_enabled(true);
        }))
    });
    fixture.controller.request(MediaAction::Toggle);
    assert_eq!(
        fixture.media.reads(),
        1,
        "accepted command prevents a reopen read"
    );
    fixture.media.finish_command(Ok(()));
    fixture.drain();
    assert_eq!(
        fixture.media.reads(),
        2,
        "fresh observation starts only after the retired command completes"
    );
}

#[test]
fn dock_media_rejections_preserve_facts_and_retry_reads_without_replaying_intent() {
    let fixture = Fixture::new();
    fixture.enable(session(MediaPlayback::Playing));
    fixture
        .media
        .command_replies
        .lock()
        .push_back(CommandReply::Reject(error(MediaErrorKind::Rejected)));
    fixture.controller.request(MediaAction::Previous);
    fixture.drain();
    assert_eq!(fixture.media.commands.lock().len(), 1);
    assert!(fixture.view().playing);
    assert!(!fixture.view().action_notice.is_empty());
    fixture
        .media
        .finish_read(Err(error(MediaErrorKind::Unavailable)));
    fixture.drain();
    fixture.controller.retry();
    assert_eq!(fixture.media.commands.lock().len(), 1);
    assert_eq!(fixture.media.reads(), 3);
    fixture
        .media
        .finish_read(Ok(MediaSnapshot { current: None }));
    fixture.drain();
    assert_eq!(fixture.view().status, "Not playing");
    assert!(
        !fixture.view().action_notice.is_empty(),
        "action health is not overwritten by read health"
    );
}

#[test]
fn dock_media_watch_and_art_errors_are_independent_and_actual_pixels_determine_contrast() {
    let fixture = Fixture::new();
    *fixture.media.watch_error.lock() = Some(error(MediaErrorKind::WatchUnavailable));
    let mut observed = session(MediaPlayback::Stopped);
    observed.artwork_notice = Some(error(MediaErrorKind::Other));
    fixture.enable(observed);
    assert!(fixture.view().toggle_enabled);
    assert!(!fixture.view().watch_notice.is_empty());
    assert!(!fixture.view().artwork_notice.is_empty());
    assert!(fixture.view().read_notice.is_empty());
    assert!(!fixture.view().has_artwork);
    let white = MediaArtwork::new(1, 1, vec![255, 255, 255, 255]).unwrap();
    let black = MediaArtwork::new(1, 1, vec![0, 0, 0, 255]).unwrap();
    let clear = MediaArtwork::new(1, 1, vec![0, 0, 0, 0]).unwrap();
    assert!(artwork_dark_foreground(&white));
    assert!(!artwork_dark_foreground(&black));
    assert!(!artwork_dark_foreground(&clear));
    *fixture.media.watch_error.lock() = None;
    fixture.controller.retry();
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(MediaSession {
            artwork: Some(white),
            ..session(MediaPlayback::Changing)
        }),
    }));
    fixture.drain();
    assert!(fixture.view().watch_notice.is_empty());
    assert!(fixture.view().artwork_notice.is_empty());
    assert!(fixture.view().has_artwork);
    assert!(fixture.view().dark_foreground);
}

#[test]
fn dock_media_all_playback_states_metadata_bound_and_trusted_icon_exact_match() {
    let fixture = Fixture::new();
    let icon = PixelIcon::new(1, 1, vec![0, 0, 0, 255]).unwrap();
    let application =
        PanelApplication::new("actual.player".into(), "Catalog player".into(), Some(icon)).unwrap();
    fixture.controller.update_applications(&[application]);
    for (index, playback) in [
        MediaPlayback::Closed,
        MediaPlayback::Opened,
        MediaPlayback::Changing,
        MediaPlayback::Stopped,
        MediaPlayback::Playing,
        MediaPlayback::Paused,
    ]
    .into_iter()
    .enumerate()
    {
        if index == 0 {
            fixture.controller.set_enabled(true);
        } else {
            fixture.controller.retry();
        }
        fixture.media.finish_read(Ok(MediaSnapshot {
            current: Some(MediaSession {
                title: format!("{}\u{202e}", "後".repeat(1024)),
                author: "  Real\nAuthor  ".into(),
                ..session(playback)
            }),
        }));
        fixture.drain();
        let view = fixture.view();
        assert_eq!(view.playback.as_str(), playback_label(playback));
        assert_eq!(view.playing, playback == MediaPlayback::Playing);
        assert!(view.title.chars().count() <= 513);
        assert!(!view.title.contains('\u{202e}'));
        assert_eq!(view.author, "Real Author");
        assert!(view.has_app_icon);
    }
    fixture.controller.update_applications(&[]);
    assert!(!fixture.view().has_app_icon);
}

#[test]
fn dock_media_factory_failure_is_manual_retryable_and_immediate_read_error_has_one_result() {
    let fixture = Fixture::new();
    *fixture.host.media.lock() = Err(error(MediaErrorKind::Unavailable));
    fixture.controller.set_enabled(true);
    fixture.drain();
    assert_eq!(fixture.media.reads(), 0);
    assert_eq!(fixture.view().status, "Media unavailable");
    *fixture.host.media.lock() = Ok(Some(fixture.media.clone()));
    fixture
        .media
        .read_replies
        .lock()
        .push_back(ReadReply::Reject(error(MediaErrorKind::Unavailable)));
    fixture.controller.retry();
    fixture.drain();
    assert_eq!(fixture.media.reads(), 1);
    assert!(fixture.media.pending_reads.lock().is_empty());
    assert!(!fixture.view().busy);
    fixture.controller.retry();
    fixture
        .media
        .finish_read(Ok(MediaSnapshot { current: None }));
    fixture.drain();
    assert_eq!(fixture.view().status, "Not playing");
    assert_eq!(fixture.host.acquisitions.load(Ordering::Relaxed), 2);
}

#[test]
fn popup_media_is_lazy_and_saved_dock_off_is_not_observation_off() {
    let fixture = Fixture::new();
    let popup = QuickSettings::new().unwrap();
    assert!(fixture.controller.attach_popup(&popup).is_none());
    assert_eq!(fixture.host.acquisitions.load(Ordering::Relaxed), 0);
    popup.show().unwrap();
    let token = fixture.controller.attach_popup(&popup).unwrap();
    assert_eq!(fixture.host.acquisitions.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.media.reads(), 1);
    assert_eq!(fixture.media.changed.lock().len(), 1);
    let current = session(MediaPlayback::Paused);
    let key = current.key;
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(current),
    }));
    fixture.drain();
    assert!(
        !fixture.view().enabled,
        "popup observation does not create a Dock slot"
    );
    assert!(popup.get_media_view().enabled);
    assert_eq!(popup.get_media_view().title, "Actual title 後");
    assert!(!popup.get_timeline_available());
    assert!(popup.get_timeline_notice().contains("Timeline unavailable"));
    assert!(popup.get_media_view().previous_enabled);
    assert!(popup.get_media_view().toggle_enabled);
    assert!(popup.get_media_view().next_enabled);
    fixture.controller.request(MediaAction::Toggle);
    assert!(
        fixture.media.commands.lock().is_empty(),
        "Dock admission still needs the preference"
    );
    let identity = popup.get_media_view().session_identity;
    fixture
        .controller
        .request_from_popup(token, MediaAction::Toggle, identity.as_str());
    assert_eq!(fixture.media.commands.lock()[0].expected_session, key);
    assert_eq!(fixture.media.commands.lock()[0].action, HostAction::Toggle);
    fixture.controller.detach_popup(token);
    popup.hide().unwrap();
    fixture.media.finish_command(Ok(()));
    fixture.drain();
    assert_eq!(
        fixture.media.reads(),
        1,
        "closed popup does not request command readback"
    );
    assert_eq!(fixture.media.watch_drops.load(Ordering::Relaxed), 1);
}

#[test]
fn popup_and_dock_share_one_provider_watch_and_flight_across_one_to_one_demand() {
    let fixture = Fixture::new();
    fixture.enable(session(MediaPlayback::Playing));
    let generation = fixture.controller.state.borrow().generation;
    let popup = QuickSettings::new().unwrap();
    popup.show().unwrap();
    let first = fixture.controller.attach_popup(&popup).unwrap();
    let replacement = fixture.controller.attach_popup(&popup).unwrap();
    assert_ne!(
        first, replacement,
        "exact attachment replacement cannot reuse authority"
    );
    fixture.controller.detach_popup(first);
    assert!(fixture.controller.state.borrow().popup.is_some());
    assert_eq!(fixture.controller.state.borrow().generation, generation);
    fixture.controller.set_enabled(false);
    assert!(!fixture.view().enabled);
    assert!(popup.get_media_view().playing);
    assert_eq!(fixture.host.acquisitions.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.media.changed.lock().len(), 1);
    assert_eq!(fixture.media.reads(), 1);
    assert_eq!(fixture.media.watch_drops.load(Ordering::Relaxed), 0);
    fixture.controller.set_enabled(true);
    fixture.controller.detach_popup(replacement);
    assert!(fixture.view().enabled);
    assert_eq!(fixture.media.watch_drops.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.controller.state.borrow().generation, generation);
    popup.hide().unwrap();
}

#[test]
fn popup_reopen_waits_for_accepted_read_and_retires_old_facts_and_watch() {
    let fixture = Fixture::new();
    let popup = QuickSettings::new().unwrap();
    popup.show().unwrap();
    let old = fixture.controller.attach_popup(&popup).unwrap();
    let late_watch = fixture.media.changed.lock()[0].clone();
    fixture.controller.detach_popup(old);
    let new = fixture.controller.attach_popup(&popup).unwrap();
    assert_ne!(old, new);
    assert_eq!(fixture.media.reads(), 1);
    assert_eq!(fixture.host.acquisitions.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.media.changed.lock().len(), 1);
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(session(MediaPlayback::Playing)),
    }));
    fixture.drain();
    assert!(
        !popup.get_media_view().current_present,
        "retired read cannot project into reopen"
    );
    assert_eq!(fixture.media.reads(), 2);
    assert_eq!(fixture.media.changed.lock().len(), 2);
    late_watch(MediaEvent::Changed);
    late_watch(MediaEvent::WatchUnavailable(error(
        MediaErrorKind::WatchUnavailable,
    )));
    let mut current = session(MediaPlayback::Paused);
    current.timeline = Ok(MediaTimeline {
        start_ticks: 10_000_000,
        end_ticks: 610_000_000,
        position_ticks: 310_000_000,
        min_seek_ticks: 10_000_000,
        max_seek_ticks: 610_000_000,
        last_updated_utc_ticks: None,
    });
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(current),
    }));
    fixture.drain();
    assert_eq!(popup.get_media_view().playback, "Paused");
    assert!(popup.get_media_view().watch_notice.is_empty());
    assert!(popup.get_timeline_available());
    assert_eq!(popup.get_timeline_time(), "0:30 / 1:00 · observed");
    assert!((popup.get_timeline_progress() - 0.5).abs() < f32::EPSILON);
    assert_eq!(
        fixture.media.reads(),
        2,
        "late retired watch cannot schedule a third read"
    );
    fixture.controller.detach_popup(new);
    popup.hide().unwrap();
}

#[test]
fn popup_accepted_command_reopen_never_overlaps_or_replays_and_requires_new_identity() {
    let fixture = Fixture::new();
    let popup = QuickSettings::new().unwrap();
    popup.show().unwrap();
    let old = fixture.controller.attach_popup(&popup).unwrap();
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(session(MediaPlayback::Paused)),
    }));
    fixture.drain();
    let old_identity = popup.get_media_view().session_identity;
    fixture
        .controller
        .request_from_popup(old, MediaAction::Next, old_identity.as_str());
    fixture.controller.detach_popup(old);
    let new = fixture.controller.attach_popup(&popup).unwrap();
    fixture.controller.retry();
    fixture
        .controller
        .request_from_popup(old, MediaAction::Next, old_identity.as_str());
    fixture
        .controller
        .request_from_popup(new, MediaAction::Next, old_identity.as_str());
    assert_eq!(fixture.media.commands.lock().len(), 1);
    assert_eq!(fixture.media.reads(), 1);
    fixture.media.finish_command(Ok(()));
    fixture.drain();
    assert_eq!(fixture.media.reads(), 2);
    assert!(
        popup.get_media_view().action_notice.is_empty(),
        "old accepted result is not new presentation feedback"
    );
    let replacement = session(MediaPlayback::Playing);
    let key = replacement.key;
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(replacement),
    }));
    fixture.drain();
    fixture
        .controller
        .request_from_popup(new, MediaAction::Next, old_identity.as_str());
    assert_eq!(fixture.media.commands.lock().len(), 1);
    fixture.controller.request_from_popup(
        new,
        MediaAction::Previous,
        popup.get_media_view().session_identity.as_str(),
    );
    assert_eq!(fixture.media.commands.lock()[1].expected_session, key);
    fixture.controller.detach_popup(new);
    popup.hide().unwrap();
    fixture
        .media
        .finish_command(Err(error(MediaErrorKind::Rejected)));
    fixture.drain();
    assert_eq!(
        fixture.media.commands.lock().len(),
        2,
        "errors never retry a transport"
    );
}

#[test]
fn popup_session_switch_and_watch_failure_preserve_independent_metadata_and_transport() {
    let fixture = Fixture::new();
    let popup = QuickSettings::new().unwrap();
    popup.show().unwrap();
    let token = fixture.controller.attach_popup(&popup).unwrap();
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(session(MediaPlayback::Paused)),
    }));
    fixture.drain();
    let previous = popup.get_media_view().session_identity;
    fixture.media.event(MediaEvent::Changed);
    fixture
        .controller
        .request_from_popup(token, MediaAction::Toggle, previous.as_str());
    assert!(
        fixture.media.commands.lock().is_empty(),
        "mailbox invalidation precedes projection"
    );
    fixture.drain();
    let mut replacement = session(MediaPlayback::Playing);
    replacement.title = "Untrusted **ordinary** text https://not-authority.invalid".into();
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(replacement),
    }));
    fixture.media.event(MediaEvent::WatchUnavailable(error(
        MediaErrorKind::WatchUnavailable,
    )));
    fixture.drain();
    assert!(popup.get_media_view().title.contains("**ordinary**"));
    assert!(
        popup
            .get_media_view()
            .watch_notice
            .contains("Recorded native failure")
    );
    assert!(popup.get_media_view().toggle_enabled);
    assert!(!popup.get_timeline_available());
    fixture
        .controller
        .request_from_popup(token, MediaAction::Toggle, previous.as_str());
    assert!(
        fixture.media.commands.lock().is_empty(),
        "same source app is not session authority"
    );
    fixture.controller.detach_popup(token);
    popup.hide().unwrap();
}

#[test]
fn timeline_projection_is_checked_observation_not_fabricated_duration_or_utc_age() {
    let observed = MediaTimeline {
        start_ticks: 0,
        end_ticks: 0,
        position_ticks: 0,
        min_seek_ticks: 0,
        max_seek_ticks: 0,
        last_updated_utc_ticks: Some(i64::MIN),
    };
    let zero = timeline_projection(Some(&Ok(observed)));
    assert!(zero.available);
    assert_eq!(zero.time, "0:00 / 0:00 · observed");
    assert_eq!(zero.progress, 0.0);
    assert_eq!(zero.notice, "Observed zero-length timeline.");
    let extreme = timeline_projection(Some(&Ok(MediaTimeline {
        start_ticks: i64::MIN,
        end_ticks: i64::MAX,
        position_ticks: 0,
        min_seek_ticks: i64::MIN,
        max_seek_ticks: i64::MAX,
        last_updated_utc_ticks: None,
    })));
    assert!(extreme.available);
    assert!((extreme.progress - 0.5).abs() < f32::EPSILON);
    assert_ne!(extreme.time, zero.time);
    for invalid in [
        MediaTimeline {
            start_ticks: 1,
            ..observed
        },
        MediaTimeline {
            position_ticks: 1,
            ..observed
        },
        MediaTimeline {
            min_seek_ticks: 1,
            ..observed
        },
    ] {
        let projection = timeline_projection(Some(&Ok(invalid)));
        assert!(!projection.available);
        assert!(
            projection.time.is_empty(),
            "invalid raw facts do not become 0:00"
        );
        assert!(projection.notice.contains("inconsistent observed bounds"));
    }
    let unavailable = timeline_projection(Some(&Err(error(MediaErrorKind::Unavailable))));
    assert!(!unavailable.available);
    assert!(unavailable.time.is_empty());
}

#[test]
fn popup_attachment_and_observation_epochs_never_wrap_authority() {
    let fixture = Fixture::new();
    let popup = QuickSettings::new().unwrap();
    popup.show().unwrap();
    fixture.controller.state.borrow_mut().popup_sequence = u64::MAX;
    assert!(fixture.controller.attach_popup(&popup).is_none());
    assert_eq!(fixture.host.acquisitions.load(Ordering::Relaxed), 0);
    fixture.controller.state.borrow_mut().popup_sequence = 0;
    fixture.controller.state.borrow_mut().generation = u64::MAX;
    assert!(fixture.controller.attach_popup(&popup).is_none());
    fixture.controller.set_enabled(true);
    assert!(!fixture.view().enabled);
    assert_eq!(fixture.host.acquisitions.load(Ordering::Relaxed), 0);
    popup.hide().unwrap();
}

#[test]
fn popup_rounded_cover_reuses_shared_mask_without_changing_raw_dock_artwork() {
    let fixture = Fixture::new();
    let artwork = MediaArtwork::new(2, 4, [128, 0, 0, 128].repeat(8)).unwrap();
    let mut current = session(MediaPlayback::Paused);
    current.artwork = Some(artwork.clone());
    fixture.enable(current.clone());
    let raw = fixture.view().artwork.size();
    assert_eq!((raw.width, raw.height), (2, 4));
    assert!(
        fixture.controller.popup_artwork.borrow().is_none(),
        "hidden preview does not prepare popup art"
    );
    let popup = QuickSettings::new().unwrap();
    popup.show().unwrap();
    let token = fixture.controller.attach_popup(&popup).unwrap();
    let prepared = popup.get_media_view().artwork.size();
    assert_eq!((prepared.width, prepared.height), (512, 512));
    assert_eq!(
        fixture.view().artwork.size(),
        raw,
        "existing Dock pixels and dimensions are untouched"
    );
    assert_eq!(
        fixture
            .controller
            .popup_artwork
            .borrow()
            .as_ref()
            .unwrap()
            .0,
        artwork
    );
    fixture.controller.update_applications(&[]);
    fixture.media.event(MediaEvent::Changed);
    fixture.drain();
    current.timeline = Ok(MediaTimeline {
        start_ticks: 0,
        end_ticks: 0,
        position_ticks: 0,
        min_seek_ticks: 0,
        max_seek_ticks: 0,
        last_updated_utc_ticks: None,
    });
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(current),
    }));
    fixture.drain();
    assert_eq!(popup.get_media_view().artwork.size(), prepared);
    assert_eq!(
        fixture
            .controller
            .popup_artwork
            .borrow()
            .as_ref()
            .unwrap()
            .0,
        artwork
    );
    assert_eq!(artwork.rgba(), [128, 0, 0, 128].repeat(8));
    assert_eq!(fixture.host.acquisitions.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.media.changed.lock().len(), 1);
    fixture.controller.detach_popup(token);
    popup.hide().unwrap();
}
