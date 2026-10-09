// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::power_menu::PowerMenuController;
use tessera_core::Rect;
use tessera_system::display_context::{
    DisplayContextCompletion, DisplayContextError, DisplayContextHost, DisplayContextWatchCallback,
    DisplayContextWatchEvent, DisplayContextWatchGuard, DisplayContextWatchReady, DisplayLayout,
    DisplaySelection,
};

struct WatchEntry {
    events: DisplayContextWatchCallback,
    ready: Mutex<Option<DisplayContextWatchReady>>,
}

struct RecordingGuard {
    entry: Arc<WatchEntry>,
    drops: Arc<AtomicUsize>,
}
impl DisplayContextWatchGuard for RecordingGuard {}
impl Drop for RecordingGuard {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        let ready = self.entry.ready.lock().take();
        if let Some(ready) = ready {
            ready(Err(DisplayContextError::Stopped));
        }
        // An already admitted event may finish during detached retirement.
        (self.entry.events)(DisplayContextWatchEvent::Changed);
    }
}

struct RecordingDisplay {
    reads: AtomicUsize,
    watches: AtomicUsize,
    deferred: bool,
    unsupported: bool,
    read: Mutex<Option<DisplayContextCompletion>>,
    entries: Mutex<Vec<Arc<WatchEntry>>>,
    drops: Arc<AtomicUsize>,
}

fn layout() -> DisplayLayout {
    let bounds = Rect::new(-1600, -120, 3520, 1200).unwrap();
    DisplayLayout::new(
        bounds,
        Rect::new(0, 0, 1920, 1080).unwrap(),
        1.25,
        DisplaySelection::Primary,
    )
    .unwrap()
}

impl RecordingDisplay {
    fn new(deferred: bool, unsupported: bool) -> Arc<Self> {
        Arc::new(Self {
            reads: AtomicUsize::new(0),
            watches: AtomicUsize::new(0),
            deferred,
            unsupported,
            read: Mutex::default(),
            entries: Mutex::default(),
            drops: Arc::default(),
        })
    }
    fn ready(&self, index: usize) {
        let entry = self.entries.lock()[index].clone();
        let ready = entry
            .ready
            .lock()
            .take()
            .expect("one accepted readiness callback");
        ready(Ok(()));
    }
    fn event(&self, index: usize, event: DisplayContextWatchEvent) {
        let entry = self.entries.lock()[index].clone();
        (entry.events)(event);
    }
    fn complete_read(&self) {
        let completion = self.read.lock().take().expect("one accepted display read");
        completion(Ok(Some(layout())));
    }
}

impl DisplayContextHost for RecordingDisplay {
    fn read(&self, completion: DisplayContextCompletion) -> Result<(), DisplayContextError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.deferred {
            assert!(self.read.lock().replace(completion).is_none());
        } else {
            completion(Ok(Some(layout())));
        }
        Ok(())
    }
    fn watch(
        &self,
        events: DisplayContextWatchCallback,
        ready: DisplayContextWatchReady,
    ) -> Result<Box<dyn DisplayContextWatchGuard>, DisplayContextError> {
        self.watches.fetch_add(1, Ordering::SeqCst);
        if self.unsupported {
            return Err(DisplayContextError::Unsupported);
        }
        let entry = Arc::new(WatchEntry {
            events,
            ready: Mutex::new(Some(ready)),
        });
        self.entries.lock().push(entry.clone());
        Ok(Box::new(RecordingGuard {
            entry,
            drops: self.drops.clone(),
        }))
    }
}

fn with_display(display: &Arc<RecordingDisplay>) -> LauncherFixture {
    let fixture = LauncherFixture::new();
    *fixture.host.display_provider.lock() = Some(display.clone());
    fixture.controller.open_launcher();
    fixture
}

fn open(fixture: &LauncherFixture) -> Rc<PowerMenuController> {
    fixture.click_launcher("Open power menu");
    let actor = fixture
        .controller
        .power_menu
        .borrow()
        .clone()
        .expect("real footer routed to Power");
    let surface = actor.component();
    POWER_WINDOW.with(|window| *window.borrow_mut() = Some(surface.as_weak()));
    actor.process_events();
    actor
}

#[test]
fn ready_changed_coalesces_one_actual_followup_without_show_focus_or_second_factory() {
    let display = RecordingDisplay::new(true, false);
    let fixture = with_display(&display);
    let actor = open(&fixture);
    assert_eq!(display.reads.load(Ordering::SeqCst), 1);
    assert_eq!(display.watches.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.host.display_provider_calls.load(Ordering::SeqCst),
        1
    );
    display.ready(0);
    for _ in 0..32 {
        display.event(0, DisplayContextWatchEvent::Changed);
    }
    fixture.panel.invoke_power_display_event_ready();
    assert_eq!(
        display.reads.load(Ordering::SeqCst),
        1,
        "initial read remains single flight"
    );
    display.complete_read();
    actor.process_events();
    assert_eq!(
        display.reads.load(Ordering::SeqCst),
        2,
        "dirty watch caused a genuine followup"
    );
    display.complete_read();
    actor.process_events();
    assert!(actor.is_visible());
    let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    let activity = fixture.host.unrelated_activity();
    let leases = fixture.host.power_lease_drops.load(Ordering::SeqCst);
    for _ in 0..16 {
        display.event(0, DisplayContextWatchEvent::Changed);
    }
    fixture.panel.invoke_power_display_event_ready();
    assert_eq!(display.reads.load(Ordering::SeqCst), 3);
    display.complete_read();
    actor.process_events();
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), focus);
    assert_eq!(
        fixture.host.power_lease_drops.load(Ordering::SeqCst),
        leases
    );
    assert_eq!(fixture.host.unrelated_activity(), activity);
    assert_eq!(
        fixture.host.display_provider_calls.load(Ordering::SeqCst),
        1
    );
}

#[test]
fn weak_panel_relay_survives_hidden_presentation_retirement_but_does_not_recreate_it() {
    let display = RecordingDisplay::new(false, false);
    let fixture = with_display(&display);
    let actor = open(&fixture);
    display.ready(0);
    display.event(0, DisplayContextWatchEvent::Changed);
    fixture.panel.invoke_power_display_event_ready();
    actor.process_events();
    actor.hide();
    let retired_surface = actor.component();
    i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(31));
    slint::platform::update_timers_and_animations();
    let reads = display.reads.load(Ordering::SeqCst);
    let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    display.event(0, DisplayContextWatchEvent::Changed);
    fixture.panel.invoke_power_display_event_ready();
    assert_eq!(display.reads.load(Ordering::SeqCst), reads);
    assert!(!retired_surface.window().is_visible());
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), focus);
    assert_eq!(display.drops.load(Ordering::SeqCst), 0);
    let reopened = open(&fixture);
    assert!(Rc::ptr_eq(&actor, &reopened));
    assert!(reopened.is_visible());
    assert_eq!(display.watches.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.host.display_provider_calls.load(Ordering::SeqCst),
        1
    );
}

#[test]
fn terminal_watch_failure_preserves_reads_and_restarts_only_on_explicit_open() {
    let display = RecordingDisplay::new(false, false);
    let fixture = with_display(&display);
    let actor = open(&fixture);
    display.ready(0);
    let reads = display.reads.load(Ordering::SeqCst);
    display.event(
        0,
        DisplayContextWatchEvent::Unavailable(DisplayContextError::Unavailable),
    );
    fixture.panel.invoke_power_display_event_ready();
    assert_eq!(display.drops.load(Ordering::SeqCst), 1);
    assert_eq!(display.reads.load(Ordering::SeqCst), reads);
    assert!(actor.is_visible());
    fixture.panel.invoke_power_display_event_ready();
    assert_eq!(display.watches.load(Ordering::SeqCst), 1);
    open(&fixture);
    assert_eq!(display.watches.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture.host.display_provider_calls.load(Ordering::SeqCst),
        1
    );

    let unsupported = RecordingDisplay::new(false, true);
    let other = with_display(&unsupported);
    let actor = open(&other);
    assert!(actor.is_visible());
    assert_eq!(unsupported.watches.load(Ordering::SeqCst), 1);
    actor.refresh_display();
    actor.process_events();
    assert!(unsupported.reads.load(Ordering::SeqCst) >= 2);
    assert!(!other.panel.get_status().contains("watch"));
}

#[test]
fn stale_ready_events_and_guard_drop_cannot_refresh_replacement_or_closed_root() {
    let display = RecordingDisplay::new(false, false);
    let fixture = with_display(&display);
    let old = open(&fixture);
    let entry = display.entries.lock()[0].clone();
    let late_ready = entry.ready.lock().take().unwrap();
    let removed = fixture.controller.power_menu.borrow_mut().take().unwrap();
    removed.close();
    let replacement = open(&fixture);
    assert!(!Rc::ptr_eq(&old, &replacement));
    assert_eq!(display.drops.load(Ordering::SeqCst), 1);
    let reads = display.reads.load(Ordering::SeqCst);
    late_ready(Ok(()));
    (entry.events)(DisplayContextWatchEvent::Changed);
    fixture.panel.invoke_power_display_event_ready();
    assert_eq!(display.reads.load(Ordering::SeqCst), reads);
    assert!(replacement.is_visible());
    let scope = power_menu::PowerAdmissionScope::new(&fixture.controller);
    drop(scope);
    assert!(fixture.controller.power_admission_closed.get());
    display.event(1, DisplayContextWatchEvent::Changed);
    fixture.panel.invoke_power_display_event_ready();
    assert_eq!(display.reads.load(Ordering::SeqCst), reads);
    assert_eq!(display.drops.load(Ordering::SeqCst), 2);
    assert!(!replacement.is_visible());
}
