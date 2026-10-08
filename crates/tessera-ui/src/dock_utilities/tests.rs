// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use tessera_system::dock_utilities::{DockUtilityCompletion, DockUtilityError};

// These exercise host reentry on the UI thread, not a presenter-only data hook.
type Hook = RefCell<Option<Box<dyn FnOnce()>>>;
thread_local! {
    static BACKEND_INITIALIZED: Cell<bool> = const { Cell::new(false) };
    static FACTORY_HOOK: Hook = RefCell::default();
    static TOGGLE_HOOK: Hook = RefCell::default();
}

fn init_backend() {
    BACKEND_INITIALIZED.with(|initialized| {
        if !initialized.get() {
            i_slint_backend_testing::init_no_event_loop();
            initialized.set(true);
        }
    });
}

fn run_hook(hook: &'static std::thread::LocalKey<Hook>) {
    let callback = hook.with(|hook| hook.borrow_mut().take());
    if let Some(callback) = callback {
        callback();
    }
}

enum Reply {
    Pending,
    Inline(Result<(), DockUtilityError>),
    Reject(DockUtilityError),
}

#[derive(Default)]
struct RecordingUtilities {
    toggles: AtomicUsize,
    pending: Mutex<Option<DockUtilityCompletion>>,
    replies: Mutex<VecDeque<Reply>>,
}

impl RecordingUtilities {
    fn finish(&self, result: Result<(), DockUtilityError>) {
        let completion = self.pending.lock().take().expect("accepted desktop toggle");
        completion(result);
    }

    fn toggles(&self) -> usize {
        self.toggles.load(Ordering::Relaxed)
    }
}

impl DockUtilitiesHost for RecordingUtilities {
    fn toggle_desktop(&self, completion: DockUtilityCompletion) -> Result<(), DockUtilityError> {
        self.toggles.fetch_add(1, Ordering::Relaxed);
        assert!(
            self.pending.lock().is_none(),
            "only one accepted toggle may exist"
        );
        let reply = self.replies.lock().pop_front().unwrap_or(Reply::Pending);
        match reply {
            Reply::Pending => *self.pending.lock() = Some(completion),
            Reply::Inline(result) => completion(result),
            Reply::Reject(error) => {
                run_hook(&TOGGLE_HOOK);
                return Err(error);
            }
        }
        run_hook(&TOGGLE_HOOK);
        Ok(())
    }
}

struct RecordingDesktop {
    provider: Mutex<Result<Option<Arc<dyn DockUtilitiesHost>>, DockUtilityError>>,
    factory_calls: AtomicUsize,
}

impl RecordingDesktop {
    fn new(utilities: Arc<RecordingUtilities>) -> Arc<Self> {
        Arc::new(Self {
            provider: Mutex::new(Ok(Some(utilities))),
            factory_calls: AtomicUsize::new(0),
        })
    }

    fn factory_calls(&self) -> usize {
        self.factory_calls.load(Ordering::Relaxed)
    }
}

impl DesktopHost for RecordingDesktop {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("desktop toggle cannot observe applications");
    }

    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("desktop toggle cannot activate a window");
    }

    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("desktop toggle cannot launch an application");
    }

    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("desktop toggle cannot dispatch recovery or system actions");
    }

    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("desktop toggle cannot save preferences");
    }

    fn subscribe(&self, _: Arc<dyn Fn() + Send + Sync>) -> Result<Option<Box<dyn Send>>, String> {
        panic!("desktop toggle cannot subscribe to application changes");
    }

    fn request_ui_focus(&self, _: &slint::Window) -> Result<(), String> {
        panic!("desktop toggle cannot request foreground");
    }

    fn dock_utilities_host(&self) -> Result<Option<Arc<dyn DockUtilitiesHost>>, DockUtilityError> {
        self.factory_calls.fetch_add(1, Ordering::Relaxed);
        run_hook(&FACTORY_HOOK);
        self.provider.lock().clone()
    }
}

struct Fixture {
    host: Arc<RecordingDesktop>,
    utilities: Arc<RecordingUtilities>,
    dock: Dock,
    presenter: Rc<DockUtilitiesController>,
    results: Rc<RefCell<Vec<Result<(), String>>>>,
}

impl Fixture {
    fn new() -> Self {
        init_backend();
        let utilities = Arc::new(RecordingUtilities::default());
        let host = RecordingDesktop::new(utilities.clone());
        let dock = Dock::new().unwrap();
        dock.window().set_size(slint::PhysicalSize::new(168, 72));
        dock.show().unwrap();
        let presenter = DockUtilitiesController::new(host.clone(), &dock);
        let results = Rc::new(RefCell::new(Vec::new()));
        let weak = Rc::downgrade(&presenter);
        let completed = results.clone();
        dock.on_utility_event_ready(move || {
            if let Some(presenter) = weak.upgrade()
                && let Some(result) = presenter.process_events()
            {
                completed.borrow_mut().push(result);
            }
        });
        let weak = Rc::downgrade(&presenter);
        dock.on_reserved_action_requested(move |action| {
            if let Some(presenter) = weak.upgrade() {
                presenter.request(action);
            }
        });
        Self {
            host,
            utilities,
            dock,
            presenter,
            results,
        }
    }

    fn request(&self) {
        self.dock
            .invoke_reserved_action_requested(DockReservedAction::ShowDesktop);
    }

    fn drain(&self) {
        // No injected data or polling: deliver the same production mailbox wake as the worker.
        self.dock.invoke_utility_event_ready();
    }

    fn assert_pending(&self) {
        assert!(self.dock.get_show_desktop_busy());
        assert_eq!(
            self.dock.get_show_desktop_notice(),
            "Desktop toggle request pending."
        );
    }

    fn take_results(&self) -> Vec<Result<(), String>> {
        std::mem::take(&mut *self.results.borrow_mut())
    }
}

fn error(kind: DockUtilityErrorKind) -> DockUtilityError {
    DockUtilityError::new(
        kind,
        "PRIVATE-provider-detail\nC:\\user\\secret\u{202e}<script>",
    )
}

#[test]
fn construction_preserves_root_callbacks_and_hidden_or_closed_intents_do_not_acquire() {
    init_backend();
    let utilities = Arc::new(RecordingUtilities::default());
    let host = RecordingDesktop::new(utilities.clone());
    let dock = Dock::new().unwrap();
    let actions = Rc::new(Cell::new(0));
    let action_count = actions.clone();
    dock.on_reserved_action_requested(move |_| action_count.set(action_count.get() + 1));
    let wakes = Rc::new(Cell::new(0));
    let wake_count = wakes.clone();
    dock.on_utility_event_ready(move || wake_count.set(wake_count.get() + 1));
    let presenter = DockUtilitiesController::new(host.clone(), &dock);
    dock.invoke_reserved_action_requested(DockReservedAction::ShowDesktop);
    dock.invoke_utility_event_ready();
    assert_eq!(
        actions.get(),
        1,
        "the constructor must not replace root action wiring"
    );
    assert_eq!(
        wakes.get(),
        1,
        "the constructor must not replace root wake wiring"
    );
    assert_eq!(host.factory_calls(), 0);
    presenter.request(DockReservedAction::ShowDesktop);
    assert_eq!(
        host.factory_calls(),
        0,
        "a hidden Dock has no input authority"
    );
    dock.show().unwrap();
    presenter.close();
    presenter.request(DockReservedAction::ShowDesktop);
    assert_eq!(
        host.factory_calls(),
        0,
        "a closed presenter cannot reacquire"
    );
    assert_eq!(utilities.toggles(), 0);
    assert!(!dock.get_show_desktop_busy());
    assert!(dock.get_show_desktop_notice().is_empty());
    assert_eq!(presenter.process_events(), None);
}

#[test]
fn missing_and_failed_factories_retry_only_for_a_new_explicit_action() {
    let fixture = Fixture::new();
    assert_eq!(fixture.host.factory_calls(), 0);
    assert_eq!(fixture.utilities.toggles(), 0);
    *fixture.host.provider.lock() = Ok(None);
    fixture.request();
    fixture.assert_pending();
    assert_eq!(fixture.host.factory_calls(), 1);
    assert!(fixture.take_results().is_empty());
    fixture.request();
    assert_eq!(
        fixture.host.factory_calls(),
        1,
        "pending missing-capability failure is still single-flight"
    );
    fixture.drain();
    assert_eq!(
        fixture.take_results(),
        vec![Err("Show desktop is not supported by this host.".into())]
    );
    assert!(!fixture.dock.get_show_desktop_busy());
    fixture.drain();
    assert_eq!(
        fixture.host.factory_calls(),
        1,
        "completion wakes never retry factories"
    );

    *fixture.host.provider.lock() = Err(error(DockUtilityErrorKind::Unavailable));
    fixture.request();
    fixture.assert_pending();
    assert_eq!(fixture.host.factory_calls(), 2);
    fixture.drain();
    assert_eq!(
        fixture.take_results(),
        vec![Err("Desktop toggle is unavailable. Try again.".into())]
    );
    fixture.drain();
    assert_eq!(fixture.host.factory_calls(), 2);

    *fixture.host.provider.lock() = Err(error(DockUtilityErrorKind::Unsupported));
    fixture.request();
    fixture.drain();
    assert_eq!(
        fixture.take_results(),
        vec![Err("Show desktop is not supported by this host.".into())]
    );
    assert_eq!(fixture.host.factory_calls(), 3);
    assert_eq!(fixture.utilities.toggles(), 0);

    *fixture.host.provider.lock() = Ok(Some(fixture.utilities.clone()));
    fixture.request();
    fixture.assert_pending();
    assert_eq!(fixture.host.factory_calls(), 4);
    fixture.utilities.finish(Ok(()));
    fixture.drain();
    assert_eq!(fixture.take_results(), vec![Ok(())]);
    fixture.request();
    assert_eq!(
        fixture.host.factory_calls(),
        4,
        "only a successfully acquired provider is cached"
    );
    assert_eq!(fixture.utilities.toggles(), 2);
    fixture.utilities.finish(Ok(()));
    fixture.drain();
}

#[test]
fn inline_worker_and_immediate_rejection_share_queued_completion_retirement() {
    for delivery in ["inline", "worker", "immediate-rejection"] {
        let fixture = Fixture::new();
        match delivery {
            "inline" => fixture
                .utilities
                .replies
                .lock()
                .push_back(Reply::Inline(Ok(()))),
            "immediate-rejection" => fixture
                .utilities
                .replies
                .lock()
                .push_back(Reply::Reject(error(DockUtilityErrorKind::Busy))),
            _ => {}
        }
        fixture.request();
        if delivery == "worker" {
            let utilities = fixture.utilities.clone();
            std::thread::spawn(move || utilities.finish(Ok(())))
                .join()
                .unwrap();
        }
        fixture.assert_pending();
        assert!(
            fixture.take_results().is_empty(),
            "{delivery}: completion cannot project inline or on a worker"
        );
        fixture.request();
        assert_eq!(
            fixture.utilities.toggles(),
            1,
            "{delivery}: unretired completion still owns the flight"
        );
        fixture.drain();
        assert!(!fixture.dock.get_show_desktop_busy());
        let expected = if delivery == "immediate-rejection" {
            Err("Desktop toggle provider is busy. Try again.".into())
        } else {
            Ok(())
        };
        assert_eq!(fixture.take_results(), vec![expected]);
        fixture.drain();
        assert!(
            fixture.take_results().is_empty(),
            "{delivery}: retirement is exactly once"
        );
        assert_eq!(
            fixture.utilities.toggles(),
            1,
            "{delivery}: completion is not a follow-up toggle"
        );
    }
}

#[test]
fn factory_and_dispatch_reentry_see_the_installed_single_flight() {
    let fixture = Fixture::new();
    let weak = Rc::downgrade(&fixture.presenter);
    let weak_dock = fixture.dock.as_weak();
    FACTORY_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let dock = weak_dock.upgrade().unwrap();
            assert!(
                dock.get_show_desktop_busy(),
                "busy must precede capability acquisition"
            );
            weak.upgrade()
                .unwrap()
                .request(DockReservedAction::ShowDesktop);
            dock.invoke_utility_event_ready();
        }));
    });
    let weak = Rc::downgrade(&fixture.presenter);
    let weak_dock = fixture.dock.as_weak();
    TOGGLE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let presenter = weak.upgrade().unwrap();
            presenter.request(DockReservedAction::ShowDesktop);
            weak_dock.upgrade().unwrap().invoke_utility_event_ready();
        }));
    });
    fixture
        .utilities
        .replies
        .lock()
        .push_back(Reply::Inline(Ok(())));
    fixture.request();
    fixture.assert_pending();
    assert_eq!(fixture.host.factory_calls(), 1);
    assert_eq!(fixture.utilities.toggles(), 1);
    assert!(
        fixture.take_results().is_empty(),
        "reentrant wake cannot retire during external dispatch"
    );
    fixture.drain();
    assert_eq!(fixture.take_results(), vec![Ok(())]);
    fixture.drain();
    assert_eq!(
        fixture.utilities.toggles(),
        1,
        "suppressed reentry has no replay"
    );
}

#[test]
fn accepted_toggle_survives_hide_stale_refit_and_reveal_without_replay() {
    let fixture = Fixture::new();
    fixture.request();
    fixture.dock.hide().unwrap();
    fixture.request();
    fixture.dock.set_compact(true);
    fixture.dock.set_edge(2);
    fixture
        .dock
        .window()
        .set_size(slint::PhysicalSize::new(58, 135));
    fixture
        .dock
        .set_surface_status(crate::generated::DockStatus {
            notice: "Observation unavailable".into(),
            status: "Refreshing".into(),
            refreshing: true,
            stale: true,
        });
    fixture.utilities.finish(Ok(()));
    fixture.assert_pending();
    fixture.drain();
    assert_eq!(fixture.take_results(), vec![Ok(())]);
    assert!(!fixture.dock.get_show_desktop_busy());
    assert_eq!(
        fixture.dock.get_show_desktop_notice(),
        "Desktop toggle requested."
    );
    fixture.request();
    assert_eq!(
        fixture.utilities.toggles(),
        1,
        "hidden new intent is rejected after retirement too"
    );
    fixture.dock.show().unwrap();
    fixture.drain();
    assert_eq!(
        fixture.utilities.toggles(),
        1,
        "reveal is never an automatic retry"
    );
    fixture.request();
    assert_eq!(
        fixture.utilities.toggles(),
        2,
        "an explicit action is independent of stale observation"
    );
    assert_eq!(fixture.host.factory_calls(), 1);
    fixture.utilities.finish(Ok(()));
    fixture.drain();
    assert_eq!(fixture.take_results(), vec![Ok(())]);
}

#[test]
fn close_retires_pending_ui_but_accepted_native_completion_still_drains() {
    let fixture = Fixture::new();
    fixture.request();
    fixture.presenter.close();
    fixture.presenter.close();
    assert!(!fixture.dock.get_show_desktop_busy());
    assert!(fixture.dock.get_show_desktop_notice().is_empty());
    assert!(
        fixture.utilities.pending.lock().is_some(),
        "close cannot cancel accepted native work"
    );
    let utilities = fixture.utilities.clone();
    std::thread::spawn(move || utilities.finish(Err(error(DockUtilityErrorKind::AccessDenied))))
        .join()
        .unwrap();
    assert!(fixture.utilities.pending.lock().is_none());
    fixture.drain();
    assert!(
        fixture.take_results().is_empty(),
        "late completion cannot restore a retired mailbox"
    );
    assert!(!fixture.dock.get_show_desktop_busy());
    assert!(fixture.dock.get_show_desktop_notice().is_empty());
    fixture.request();
    assert_eq!(fixture.host.factory_calls(), 1);
    assert_eq!(fixture.utilities.toggles(), 1);

    let completed = Fixture::new();
    completed
        .utilities
        .replies
        .lock()
        .push_back(Reply::Inline(Ok(())));
    completed.request();
    completed.assert_pending();
    completed.presenter.close();
    completed.drain();
    assert!(
        completed.take_results().is_empty(),
        "close also discards an already queued completion"
    );
    assert!(!completed.dock.get_show_desktop_busy());
    assert!(completed.dock.get_show_desktop_notice().is_empty());
}

#[test]
fn close_during_acquisition_prevents_dispatch_and_close_during_dispatch_does_not_cancel_it() {
    let acquiring = Fixture::new();
    let weak = Rc::downgrade(&acquiring.presenter);
    FACTORY_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || weak.upgrade().unwrap().close()));
    });
    acquiring.request();
    assert_eq!(acquiring.host.factory_calls(), 1);
    assert_eq!(acquiring.utilities.toggles(), 0);
    assert!(!acquiring.dock.get_show_desktop_busy());
    acquiring.drain();
    assert!(acquiring.take_results().is_empty());

    let dispatching = Fixture::new();
    let weak = Rc::downgrade(&dispatching.presenter);
    TOGGLE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || weak.upgrade().unwrap().close()));
    });
    dispatching.request();
    assert_eq!(dispatching.utilities.toggles(), 1);
    assert!(dispatching.utilities.pending.lock().is_some());
    dispatching.utilities.finish(Ok(()));
    dispatching.drain();
    assert!(dispatching.take_results().is_empty());
    assert!(!dispatching.dock.get_show_desktop_busy());
    assert!(dispatching.dock.get_show_desktop_notice().is_empty());
}

#[test]
fn weak_pending_callbacks_retain_neither_presenter_nor_dock() {
    let fixture = Fixture::new();
    fixture.request();
    let Fixture {
        host: _,
        utilities,
        dock,
        presenter,
        results: _,
    } = fixture;
    let weak_presenter = Rc::downgrade(&presenter);
    let weak_dock = dock.as_weak();
    drop(presenter);
    assert!(
        weak_presenter.upgrade().is_none(),
        "root callbacks hold only a weak presenter"
    );
    assert!(
        !dock.get_show_desktop_busy(),
        "presenter drop clears a still-live Dock"
    );
    // Slint retains shown components; hiding releases that SDK-owned strong reference.
    dock.hide().unwrap();
    drop(dock);
    assert!(
        weak_dock.upgrade().is_none(),
        "accepted completion cannot retain the UI"
    );
    assert!(
        utilities.pending.lock().is_some(),
        "accepted native job remains independent of UI lifetime"
    );
    std::thread::spawn(move || utilities.finish(Ok(())))
        .join()
        .unwrap();
    assert!(weak_presenter.upgrade().is_none());
    assert!(weak_dock.upgrade().is_none());

    let fixture = Fixture::new();
    fixture.request();
    let Fixture {
        host: _,
        utilities,
        dock,
        presenter,
        results: _,
    } = fixture;
    let weak_dock = dock.as_weak();
    dock.hide().unwrap();
    drop(dock);
    assert!(
        weak_dock.upgrade().is_none(),
        "a live presenter also owns only a weak Dock"
    );
    std::thread::spawn(move || utilities.finish(Ok(())))
        .join()
        .unwrap();
    presenter.close();
    assert_eq!(presenter.process_events(), None);
    assert!(weak_dock.upgrade().is_none());
}

#[test]
fn every_provider_error_is_bounded_kind_only_and_completion_never_claims_desktop_state() {
    let cases = [
        (
            DockUtilityErrorKind::Unsupported,
            "Show desktop is not supported by this host.",
        ),
        (
            DockUtilityErrorKind::AccessDenied,
            "Desktop toggle was denied.",
        ),
        (
            DockUtilityErrorKind::Unavailable,
            "Desktop toggle is unavailable. Try again.",
        ),
        (
            DockUtilityErrorKind::Busy,
            "Desktop toggle provider is busy. Try again.",
        ),
        (
            DockUtilityErrorKind::Stopped,
            "Desktop toggle provider has stopped. Try again.",
        ),
        (
            DockUtilityErrorKind::Other,
            "Desktop toggle could not be requested.",
        ),
    ];
    for (kind, expected) in cases {
        let fixture = Fixture::new();
        fixture
            .utilities
            .replies
            .lock()
            .push_back(Reply::Inline(Err(error(kind))));
        fixture.request();
        fixture.assert_pending();
        fixture.drain();
        assert_eq!(fixture.take_results(), vec![Err(expected.into())]);
        assert_eq!(fixture.dock.get_show_desktop_notice(), expected);
        assert!(fixture.dock.get_show_desktop_notice().len() <= 64);
        assert!(!fixture.dock.get_show_desktop_notice().contains("PRIVATE"));
        fixture.drain();
        assert_eq!(
            fixture.utilities.toggles(),
            1,
            "provider errors do not auto-retry non-idempotent toggles"
        );
        fixture.request();
        assert_eq!(
            fixture.utilities.toggles(),
            2,
            "only a new explicit intent may retry"
        );
        fixture.utilities.finish(Ok(()));
        fixture.drain();
        assert_eq!(fixture.take_results(), vec![Ok(())]);
        assert_eq!(
            fixture.dock.get_show_desktop_notice(),
            "Desktop toggle requested."
        );
        assert_eq!(fixture.host.factory_calls(), 1);
    }
}
