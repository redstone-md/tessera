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
    // Complete the replacement first, then the old accepted read.
    let replacement = fixture.media.pending_reads.lock().remove(1).unwrap();
    replacement(Ok(MediaSnapshot {
        current: Some(session(MediaPlayback::Playing)),
    }));
    fixture.drain();
    fixture
        .media
        .finish_read(Ok(MediaSnapshot { current: None }));
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
    assert_eq!(fixture.media.reads(), 2);
    fixture.media.finish_command(Ok(()));
    fixture.drain();
    assert_eq!(
        fixture.media.reads(),
        2,
        "retired command cannot schedule readback in new scope"
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
