// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! No native API is invoked: only raw Calls/KernelCalls operations are replaced.
//! All lifecycle, acknowledgement, admission, pump and debounce code is real.

use parking_lot::Mutex;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Barrier;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::thread::{self, ThreadId};

use super::*;

#[derive(Default)]
struct Record {
    trace: Mutex<Vec<(String, ThreadId)>>,
    failures: Mutex<BTreeMap<String, usize>>,
    kernel_live: Mutex<BTreeMap<usize, bool>>,
    next_event: AtomicUsize,
    now: AtomicU64,
    notifications: Mutex<Option<Arc<Notifications<RecordingKernel>>>>,
    actions: Mutex<VecDeque<Action>>,
    callback_frame: AtomicBool,
    flood: AtomicBool,
    dispatches: AtomicUsize,
    stop_on_dispatch: AtomicBool,
    silent_start: AtomicBool,
}

impl Record {
    fn log(&self, value: impl Into<String>) {
        self.trace
            .lock()
            .push((value.into(), thread::current().id()));
    }
    fn names(&self) -> Vec<String> {
        self.trace
            .lock()
            .iter()
            .map(|entry| entry.0.clone())
            .collect()
    }
    fn fail(&self, operation: impl Into<String>, count: usize) {
        self.failures.lock().insert(operation.into(), count);
    }
    fn step(&self, operation: impl Into<String>) -> Result<(), u32> {
        let operation = operation.into();
        self.log(&operation);
        let mut failures = self.failures.lock();
        if let Some(count) = failures.get_mut(&operation)
            && *count != 0
        {
            *count -= 1;
            return Err(FAILURE);
        }
        Ok(())
    }
    fn notifications(&self) -> Arc<Notifications<RecordingKernel>> {
        self.notifications.lock().as_ref().unwrap().clone()
    }
    fn fire(&self, source: Source) {
        let notifications = self.notifications();
        self.callback_frame.store(true, Ordering::Release);
        if MANAGER_SOURCES.contains(&source) {
            manager_callback(&notifications, source, || {
                self.step(format!("ack:{source:?}"))
            });
        } else {
            notification_callback(&notifications, source);
        }
        self.callback_frame.store(false, Ordering::Release);
    }
    fn consumer(&self, name: &str) {
        assert!(
            !self.callback_frame.load(Ordering::Acquire),
            "consumer in native callback frame"
        );
        assert!(
            self.notifications.try_lock().is_some(),
            "consumer under recording state lock"
        );
        self.log(name);
    }
}

#[derive(Clone)]
struct RecordingKernel(Arc<Record>);
impl KernelCalls for RecordingKernel {
    type Event = usize;
    fn create(&self) -> Result<usize, u32> {
        let event = self.0.next_event.fetch_add(1, Ordering::Relaxed) + 1;
        self.0.step(format!("create-event:{event}"))?;
        self.0.kernel_live.lock().insert(event, false);
        Ok(event)
    }
    fn set(&self, event: usize) -> Result<(), u32> {
        self.0.step(format!("set-event:{event}"))?;
        let mut live = self.0.kernel_live.lock();
        *live
            .get_mut(&event)
            .expect("signal used a closed/reused handle") = true;
        Ok(())
    }
    fn reset(&self, event: usize) -> Result<(), u32> {
        self.0.step(format!("reset-event:{event}"))?;
        let mut live = self.0.kernel_live.lock();
        *live.get_mut(&event).expect("reset used a closed handle") = false;
        Ok(())
    }
    fn close(&self, event: usize) -> Result<(), u32> {
        self.0.step(format!("close-event:{event}"))?;
        assert!(
            self.0.kernel_live.lock().remove(&event).is_some(),
            "double close"
        );
        Ok(())
    }
}

struct Resource {
    name: String,
    record: Arc<Record>,
    thread: ThreadId,
}
impl Resource {
    fn new(record: &Arc<Record>, name: impl Into<String>) -> Self {
        let name = name.into();
        record.log(format!("acquire:{name}"));
        Self {
            name,
            record: record.clone(),
            thread: thread::current().id(),
        }
    }
}
impl Drop for Resource {
    fn drop(&mut self) {
        assert_eq!(
            self.thread,
            thread::current().id(),
            "resource left its MTA owner"
        );
        self.record.log(format!("release:{}", self.name));
    }
}

#[derive(Clone, Copy)]
enum Action {
    Hint(Source, u64),
    Messages,
    Fail,
    Stop,
}

struct RecordingCalls(Arc<Record>);
impl RecordingCalls {
    fn acquire(&self, operation: &str, resource: &str) -> Result<Resource, u32> {
        self.0.step(operation)?;
        Ok(Resource::new(&self.0, resource))
    }
}

impl Calls for RecordingCalls {
    type Kernel = RecordingKernel;
    type Class = Resource;
    type Window = Resource;
    type Context = Resource;
    type Settings = Resource;
    type Manager = Resource;
    type Token = Resource;
    fn initialize(&self) -> Result<(), u32> {
        self.0.step("mta-init")
    }
    fn uninitialize(&self) {
        self.0.log("mta-uninit");
    }
    fn create_class(&self) -> Result<Resource, u32> {
        self.acquire("class", "class")
    }
    fn create_window(&self, _: &Resource) -> Result<Resource, u32> {
        self.acquire("window", "window")
    }
    fn attach(
        &self,
        _: &Resource,
        notifications: Arc<Notifications<RecordingKernel>>,
    ) -> Result<Resource, u32> {
        *self.0.notifications.lock() = Some(notifications);
        self.acquire("attach", "context")
    }
    fn retire_context(&self, _: &Resource) {
        self.0.log("retire-context");
    }
    fn detach(&self, _: &Resource) -> Result<(), u32> {
        self.0.step("detach")
    }
    fn destroy_window(&self, _: &Resource) -> Result<(), u32> {
        self.0.step("destroy-window")
    }
    fn unregister_class(&self, _: &Resource) -> Result<(), u32> {
        self.0.step("unregister-class")
    }
    fn create_settings(&self) -> Result<Resource, u32> {
        self.acquire("settings", "settings")
    }
    fn create_manager(&self) -> Result<Resource, u32> {
        self.acquire("manager", "manager")
    }
    fn register_text(
        &self,
        _: &Resource,
        _: Arc<Notifications<RecordingKernel>>,
    ) -> Result<Resource, u32> {
        self.acquire("register:TextScale", "token:TextScale")
    }
    fn register_manager(
        &self,
        _: &Resource,
        source: Source,
        _: Arc<Notifications<RecordingKernel>>,
    ) -> Result<Resource, u32> {
        self.acquire(
            &format!("register:{source:?}"),
            &format!("token:{source:?}"),
        )
    }
    fn start(&self, _: &Resource) -> Result<(), u32> {
        // Simulates Start synchronously calling every installed SDK handler.
        self.0.log("start-enter");
        if !self.0.silent_start.load(Ordering::Acquire) {
            for source in MANAGER_SOURCES {
                self.0.fire(source);
            }
        }
        self.0.step("start")
    }
    fn stop(&self, _: &Resource) -> Result<(), u32> {
        self.0.log("stop-enter");
        // Stop is allowed to synchronously reenter Disabled, AFTER admission
        // retirement. This must still acknowledge, without needing owner locks.
        self.0.fire(Source::Disabled);
        self.0.step("stop")
    }
    fn remove_text(&self, _: &Resource, _: &Resource) -> Result<(), u32> {
        self.0.step("remove:TextScale")
    }
    fn remove_manager(&self, _: &Resource, source: Source, _: &Resource) -> Result<(), u32> {
        self.0.step(format!("remove:{source:?}"))
    }
    fn close_manager(&self, _: &Resource) -> Result<(), u32> {
        self.0.step("close-manager")
    }
    fn release_settings(&self, settings: Resource) {
        drop(settings);
    }
    fn release_manager(&self, manager: Resource) {
        drop(manager);
    }
    fn now_ms(&self) -> u64 {
        self.0.now.load(Ordering::Acquire)
    }
    fn wait(&self, stop: usize, wake: usize, timeout: Option<u32>) -> Wait {
        self.0.log(format!("wait:{timeout:?}"));
        {
            let live = self.0.kernel_live.lock();
            if live.get(&stop).copied() == Some(true) {
                return Wait::Stop;
            }
            if live.get(&wake).copied() == Some(true) {
                return Wait::Wake;
            }
        }
        let action = self.0.actions.lock().pop_front();
        match action {
            Some(Action::Hint(source, delta)) => {
                self.0.now.fetch_add(delta, Ordering::AcqRel);
                self.0.fire(source);
                Wait::Wake
            }
            Some(Action::Messages) => Wait::Messages,
            Some(Action::Fail) => Wait::Failed,
            Some(Action::Stop) => {
                let notifications = self.0.notifications();
                drop(Guard(notifications));
                Wait::Stop
            }
            None => match timeout {
                Some(timeout) => {
                    self.0.now.fetch_add(u64::from(timeout), Ordering::AcqRel);
                    Wait::Timeout
                }
                None => Wait::Failed,
            },
        }
    }
    fn pump(&self, budget: usize) -> bool {
        pump_messages(&RecordingQueue(self.0.clone()), budget)
    }
}

struct RecordingQueue(Arc<Record>);
impl MessageCalls for RecordingQueue {
    type Message = ();
    fn next(&self) -> Option<()> {
        self.0.flood.load(Ordering::Acquire).then_some(())
    }
    fn is_quit(&self, _: &()) -> bool {
        false
    }
    fn dispatch(&self, _: &()) {
        self.0.dispatches.fetch_add(1, Ordering::AcqRel);
        self.0.fire(Source::Window);
        if self.0.stop_on_dispatch.swap(false, Ordering::AcqRel) {
            drop(Guard(self.0.notifications()));
        }
    }
}

fn fixture() -> (Arc<Record>, Arc<Notifications<RecordingKernel>>) {
    let record = Arc::new(Record::default());
    let signals = Signals::create(RecordingKernel(record.clone())).unwrap();
    (record, Notifications::new(signals))
}

fn no_events(record: &Arc<Record>) -> DisplayContextWatchCallback {
    let record = record.clone();
    Arc::new(move |_| record.log("unexpected-event"))
}

fn drive(
    record: Arc<Record>,
    notifications: Arc<Notifications<RecordingKernel>>,
    events: DisplayContextWatchCallback,
    ready: DisplayContextWatchReady,
) {
    let observed = record.clone();
    run(move || RecordingCalls(record), notifications, events, ready);
    assert_eq!(count(&observed.names(), "unexpected-event"), 0);
}

fn index(names: &[String], name: &str) -> usize {
    names
        .iter()
        .position(|entry| entry == name)
        .unwrap_or_else(|| panic!("missing {name}: {names:?}"))
}
fn count(names: &[String], name: &str) -> usize {
    names.iter().filter(|entry| *entry == name).count()
}

#[test]
fn display_watch_every_registration_failure_retires_only_acquired_tokens_before_ready() {
    let registrations = [
        Source::TextScale,
        Source::Enabled,
        Source::Disabled,
        Source::Changed,
        Source::PathsFailedOrInvalidated,
    ];
    for (failed, source) in registrations.into_iter().enumerate() {
        let (record, notifications) = fixture();
        record.fail(format!("register:{source:?}"), 1);
        let events = no_events(&record);
        let seen = record.clone();
        drive(
            record.clone(),
            notifications,
            events,
            Box::new(move |result| {
                assert_eq!(result, Err(native(FAILURE)));
                seen.consumer("ready-error");
                assert!(seen.kernel_live.lock().is_empty());
            }),
        );
        let names = record.names();
        assert_eq!(count(&names, "ready-error"), 1);
        assert_eq!(count(&names, "start-enter"), 0);
        assert_eq!(count(&names, "stop-enter"), 0);
        for (position, registered) in registrations.iter().enumerate() {
            assert_eq!(
                count(&names, &format!("remove:{registered:?}")),
                usize::from(position < failed)
            );
        }
        assert!(index(&names, "release:settings") < index(&names, "mta-uninit"));
        assert!(index(&names, "destroy-window") < index(&names, "unregister-class"));
        assert!(index(&names, "mta-uninit") < index(&names, "ready-error"));
    }
}

#[test]
fn display_watch_each_partial_startup_failure_uses_real_owner_retirement() {
    for operation in [
        "mta-init", "class", "window", "attach", "settings", "manager",
    ] {
        let (record, notifications) = fixture();
        record.fail(operation, 1);
        let seen = record.clone();
        drive(
            record.clone(),
            notifications,
            no_events(&record),
            Box::new(move |result| {
                assert_eq!(result, Err(native(FAILURE)));
                seen.consumer("ready-error");
            }),
        );
        let names = record.names();
        assert_eq!(count(&names, "ready-error"), 1);
        assert_eq!(
            count(&names, "mta-uninit"),
            usize::from(operation != "mta-init")
        );
        assert_eq!(count(&names, "stop-enter"), 0);
        assert!(record.kernel_live.lock().is_empty());
    }
}

#[test]
fn display_watch_failed_start_with_sync_reentry_stops_and_acknowledges_disabled() {
    let (record, notifications) = fixture();
    record.fail("start", 1);
    let seen = record.clone();
    drive(
        record.clone(),
        notifications.clone(),
        no_events(&record),
        Box::new(move |result| {
            assert_eq!(result, Err(native(FAILURE)));
            seen.consumer("ready-error");
        }),
    );
    let names = record.names();
    assert_eq!(count(&names, "ready-error"), 1);
    assert_eq!(count(&names, "stop-enter"), 1);
    assert_eq!(count(&names, "ack:Disabled"), 2);
    for source in MANAGER_SOURCES {
        assert!(index(&names, &format!("register:{source:?}")) < index(&names, "start-enter"));
        assert_eq!(count(&names, &format!("remove:{source:?}")), 1);
    }
    assert!(!notifications.admits());
}

#[test]
fn display_watch_initial_dirty_ready_order_and_mta_resource_release() {
    let (record, notifications) = fixture();
    record.silent_start.store(true, Ordering::Release);
    let ready_record = record.clone();
    let event_record = record.clone();
    let close = notifications.clone();
    drive(
        record.clone(),
        notifications,
        Arc::new(move |event| {
            assert_eq!(event, DisplayContextWatchEvent::Changed);
            event_record.consumer("changed");
            drop(Guard(close.clone()));
        }),
        Box::new(move |result| {
            assert_eq!(result, Ok(()));
            ready_record.consumer("ready");
        }),
    );
    let names = record.names();
    assert_eq!(count(&names, "changed"), 1);
    assert!(index(&names, "ready") < index(&names, "changed"));
    assert!(index(&names, "retire-context") < index(&names, "stop-enter"));
    assert!(index(&names, "stop-enter") < index(&names, "remove:Enabled"));
    assert!(index(&names, "release:manager") < index(&names, "mta-uninit"));
    assert!(index(&names, "release:settings") < index(&names, "mta-uninit"));
    assert!(index(&names, "release:window") < index(&names, "unregister-class"));
    assert!(index(&names, "release:context") < index(&names, "mta-uninit"));
    assert!(record.kernel_live.lock().is_empty());
}

#[test]
fn display_watch_early_drop_ready_stopped_and_drop_inside_ready_suppresses_dirty() {
    for early in [true, false] {
        let (record, notifications) = fixture();
        if early {
            drop(Guard(notifications.clone()));
        }
        let close = notifications.clone();
        let seen = record.clone();
        drive(
            record.clone(),
            notifications,
            no_events(&record),
            Box::new(move |result| {
                assert_eq!(
                    result,
                    if early {
                        Err(DisplayContextError::Stopped)
                    } else {
                        Ok(())
                    }
                );
                seen.consumer("ready");
                drop(Guard(close));
            }),
        );
        assert_eq!(count(&record.names(), "ready"), 1);
        assert_eq!(count(&record.names(), "mta-init"), usize::from(!early));
        assert!(record.kernel_live.lock().is_empty());
    }
}

#[test]
fn display_watch_late_sdk_callbacks_ack_all_four_after_stop_without_signal_or_consumer() {
    let (record, notifications) = fixture();
    let mut owner = Owner::new(RecordingCalls(record.clone()), notifications.clone());
    owner.install().unwrap();
    owner.retire().unwrap();
    let before = record.names().len();
    for source in MANAGER_SOURCES {
        record.fire(source);
    }
    record.fire(Source::TextScale);
    record.fire(Source::Window);
    let names = record.names();
    assert_eq!(
        &names[before..],
        [
            "ack:Enabled",
            "ack:Disabled",
            "ack:Changed",
            "ack:PathsFailedOrInvalidated"
        ]
    );
    assert!(!notifications.admits());
    assert!(record.kernel_live.lock().is_empty());
}

#[test]
fn display_watch_ack_failure_is_terminal_after_retirement_and_ack_panics_are_contained() {
    for panics in [true, false] {
        let (record, notifications) = fixture();
        let seen = record.clone();
        let fault = notifications.clone();
        drive(
            record.clone(),
            notifications,
            Arc::new(move |event| {
                assert_eq!(
                    event,
                    DisplayContextWatchEvent::Unavailable(DisplayContextError::Unavailable)
                );
                assert!(seen.kernel_live.lock().is_empty());
                assert!(seen.names().contains(&"mta-uninit".to_string()));
                seen.consumer("unavailable");
            }),
            Box::new(move |result| {
                assert_eq!(result, Ok(()));
                manager_callback(&fault, Source::Changed, || {
                    if panics {
                        panic!("SDK projection panic");
                    } else {
                        Err(FAILURE)
                    }
                });
            }),
        );
        assert_eq!(count(&record.names(), "unavailable"), 1);
    }
}

#[test]
fn display_watch_token_remove_stop_and_close_failures_still_release_all_winrt_before_balance() {
    for operation in [
        "remove:TextScale",
        "remove:Enabled",
        "remove:Disabled",
        "remove:Changed",
        "remove:PathsFailedOrInvalidated",
        "stop",
        "close-manager",
    ] {
        let (record, notifications) = fixture();
        record.fail(operation, 1);
        let mut owner = Owner::new(RecordingCalls(record.clone()), notifications);
        owner.install().unwrap();
        assert_eq!(owner.retire(), Err(native(FAILURE)));
        drop(owner);
        let names = record.names();
        assert_eq!(count(&names, "stop-enter"), 1);
        for source in MANAGER_SOURCES {
            assert_eq!(count(&names, &format!("remove:{source:?}")), 1);
        }
        assert_eq!(count(&names, "remove:TextScale"), 1);
        assert_eq!(count(&names, "mta-uninit"), 1);
        assert!(index(&names, "release:manager") < index(&names, "mta-uninit"));
        assert!(index(&names, "release:settings") < index(&names, "mta-uninit"));
        assert!(record.kernel_live.lock().is_empty());
    }
}

#[test]
fn display_watch_context_detach_failure_is_safe_and_window_failure_does_not_fake_closure() {
    for (operation, failures) in [
        ("detach", 1),
        ("destroy-window", 1),
        ("destroy-window", 2),
        ("unregister-class", 1),
    ] {
        let (record, notifications) = fixture();
        record.fail(operation, failures);
        let mut owner = Owner::new(RecordingCalls(record.clone()), notifications);
        owner.install().unwrap();
        assert!(owner.retire().is_err());
        let names = record.names();
        assert!(index(&names, "retire-context") < index(&names, "detach"));
        assert!(index(&names, "retire-context") < index(&names, "destroy-window"));
        if operation == "destroy-window" && failures == 2 {
            assert_eq!(count(&names, "release:window"), 0);
            assert_eq!(count(&names, "unregister-class"), 0);
            assert_eq!(count(&names, "release:class"), 0);
        } else {
            assert!(index(&names, "release:window") < index(&names, "unregister-class"));
        }
        assert!(index(&names, "release:context") < index(&names, "mta-uninit"));
        record.fire(Source::Changed); // late WinRT callback remains harmless
    }
}

#[test]
fn display_watch_detached_owner_keeps_class_window_winrt_and_balance_on_one_thread() {
    let (record, notifications) = fixture();
    let caller = thread::current().id();
    let worker_record = record.clone();
    let close = notifications.clone();
    let worker = thread::spawn(move || {
        let forbidden = no_events(&worker_record);
        drive(
            worker_record,
            notifications,
            forbidden,
            Box::new(move |result| {
                assert_eq!(result, Ok(()));
                drop(Guard(close));
            }),
        );
    });
    worker.join().unwrap();
    let trace = record.trace.lock();
    let owner_thread = trace.iter().find(|entry| entry.0 == "mta-init").unwrap().1;
    assert_ne!(owner_thread, caller);
    for (operation, thread) in trace.iter() {
        if !operation.contains("event:") {
            assert_eq!(*thread, owner_thread, "{operation}");
        }
    }
}

#[test]
fn display_watch_guard_drop_signal_close_race_never_uses_reused_handle() {
    for _ in 0..32 {
        let (record, notifications) = fixture();
        let barrier = Arc::new(Barrier::new(2));
        let signals = notifications.signals.clone();
        let other = barrier.clone();
        let worker = thread::spawn(move || {
            other.wait();
            drop(Guard(notifications));
        });
        barrier.wait();
        signals.close().unwrap();
        worker.join().unwrap();
        assert!(record.kernel_live.lock().is_empty());
        // A very late drop sees empty ownership slots, not recycled integers.
        signals.stop().unwrap();
    }
}

#[test]
fn display_watch_failed_close_retains_only_failed_slot_for_retry() {
    let (record, notifications) = fixture();
    record.fail("close-event:1", 1);
    assert_eq!(notifications.signals.close(), Err(FAILURE));
    assert!(record.kernel_live.lock().contains_key(&1));
    assert!(!record.kernel_live.lock().contains_key(&2));
    notifications.signals.stop().unwrap();
    notifications.signals.close().unwrap();
    let names = record.names();
    assert_eq!(count(&names, "close-event:1"), 2);
    assert_eq!(count(&names, "close-event:2"), 1);
}

#[test]
fn display_watch_message_flood_budget_does_not_starve_stop() {
    let (record, notifications) = fixture();
    record.flood.store(true, Ordering::Release);
    record.stop_on_dispatch.store(true, Ordering::Release);
    record.actions.lock().push_back(Action::Messages);
    drive(
        record.clone(),
        notifications,
        no_events(&record),
        Box::new(|result| assert_eq!(result, Ok(()))),
    );
    assert_eq!(record.dispatches.load(Ordering::Acquire), PUMP_BUDGET);
    assert_eq!(count(&record.names(), "stop-enter"), 1);
}

#[test]
fn display_watch_consumer_and_ready_panics_are_contained_outside_ffi() {
    let (record, notifications) = fixture();
    record
        .actions
        .lock()
        .push_back(Action::Hint(Source::TextScale, QUIET_MS));
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let close = notifications.clone();
    let consumer_record = record.clone();
    drive(
        record.clone(),
        notifications,
        Arc::new(move |event| {
            assert_eq!(event, DisplayContextWatchEvent::Changed);
            consumer_record.consumer("changed");
            if seen.fetch_add(1, Ordering::AcqRel) == 0 {
                close.hint(Source::TextScale);
                panic!("consumer panic");
            }
            drop(Guard(close.clone()));
        }),
        Box::new(|result| assert_eq!(result, Ok(()))),
    );
    assert_eq!(calls.load(Ordering::Acquire), 2);
    assert!(record.kernel_live.lock().is_empty());

    let (record, notifications) = fixture();
    drive(
        record.clone(),
        notifications,
        no_events(&record),
        Box::new(|_| panic!("ready panic")),
    );
    assert_eq!(count(&record.names(), "stop-enter"), 1);
    assert!(record.kernel_live.lock().is_empty());
}

#[test]
fn display_watch_burst_has_one_hint_and_trailing_400ms_with_finite_postponement() {
    let (record, notifications) = fixture();
    let mut owner = Owner::new(RecordingCalls(record.clone()), notifications.clone());
    owner.install().unwrap();
    notifications.take().unwrap();
    let baseline = count(&record.names(), "set-event:2");
    for _ in 0..10_000 {
        record.fire(Source::TextScale);
    }
    assert_eq!(count(&record.names(), "set-event:2") - baseline, 1);
    let hints = notifications.take().unwrap();
    assert!(hints.dirty);
    assert!(!notifications.take().unwrap().dirty);
    owner.retire().unwrap();

    let mut burst = Burst::default();
    assert_eq!(burst.timeout(0), None);
    burst.hint(0);
    burst.hint(250);
    assert_eq!(burst.timeout(250), Some(400));
    assert!(!burst.due(649));
    assert!(burst.due(650));
    for now in (300..2_000).step_by(100) {
        burst.hint(now);
    }
    assert!(!burst.due(1_999));
    assert!(burst.due(2_000));

    let (record, notifications) = fixture();
    record
        .actions
        .lock()
        .extend((0..24).map(|_| Action::Hint(Source::Changed, 100)));
    let close = notifications.clone();
    let event_record = record.clone();
    drive(
        record.clone(),
        notifications,
        Arc::new(move |event| {
            assert_eq!(event, DisplayContextWatchEvent::Changed);
            assert_eq!(event_record.now.load(Ordering::Acquire), BURST_CAP_MS);
            event_record.consumer("changed-at-cap");
            drop(Guard(close.clone()));
        }),
        Box::new(|result| assert_eq!(result, Ok(()))),
    );
    assert_eq!(count(&record.names(), "changed-at-cap"), 1);
}

#[test]
fn display_watch_clean_owner_has_indefinite_wait_not_an_idle_loop() {
    let (record, notifications) = fixture();
    let events_record = record.clone();
    drive(
        record.clone(),
        notifications,
        Arc::new(move |event| events_record.log(format!("event:{event:?}"))),
        Box::new(|result| assert_eq!(result, Ok(()))),
    );
    let names = record.names();
    assert_eq!(count(&names, "wait:Some(400)"), 1);
    assert_eq!(count(&names, "wait:None"), 1);
    assert_eq!(
        names
            .iter()
            .filter(|entry| entry.starts_with("wait:"))
            .count(),
        2
    );
    assert_eq!(count(&names, "event:Changed"), 1);
    assert!(
        names
            .iter()
            .all(|entry| !entry.contains("query") && !entry.contains("Some(250)"))
    );
}

#[test]
fn display_watch_terminal_failure_delivers_once_only_after_retirement_no_restart() {
    let (record, notifications) = fixture();
    record.actions.lock().push_back(Action::Fail);
    let seen = record.clone();
    drive(
        record.clone(),
        notifications,
        Arc::new(move |event| {
            assert_eq!(
                event,
                DisplayContextWatchEvent::Unavailable(DisplayContextError::Unavailable)
            );
            assert!(seen.kernel_live.lock().is_empty());
            let names = seen.names();
            assert!(names.iter().any(|entry| entry == "mta-uninit"));
            assert!(names.iter().any(|entry| entry == "unregister-class"));
            seen.consumer("unavailable");
        }),
        Box::new(|result| assert_eq!(result, Ok(()))),
    );
    let names = record.names();
    assert_eq!(count(&names, "unavailable"), 1);
    assert_eq!(count(&names, "start-enter"), 1);
    assert_eq!(count(&names, "stop-enter"), 1);
}

#[test]
fn display_watch_disabled_is_unavailability_hint_not_empty_and_enabled_recovers_dirty() {
    let (record, notifications) = fixture();
    let mut owner = Owner::new(RecordingCalls(record.clone()), notifications.clone());
    owner.install().unwrap();
    notifications.take().unwrap();
    record.fire(Source::Disabled);
    let hints = notifications.take().unwrap();
    assert!(hints.disabled && hints.dirty);
    assert!(!hints.failed);
    record.fire(Source::Enabled);
    let hints = notifications.take().unwrap();
    assert!(!hints.disabled && hints.dirty);
    owner.retire().unwrap();
}

#[test]
fn display_watch_wait_decode_and_message_quit_are_software_only() {
    assert_eq!(decode_wait(0), Wait::Stop);
    assert_eq!(decode_wait(1), Wait::Wake);
    assert_eq!(decode_wait(2), Wait::Messages);
    assert_eq!(decode_wait(258), Wait::Timeout);
    assert_eq!(decode_wait(u32::MAX), Wait::Failed);
    assert_eq!(decode_wait(17), Wait::Failed);
    struct Quit;
    impl MessageCalls for Quit {
        type Message = ();
        fn next(&self) -> Option<()> {
            Some(())
        }
        fn is_quit(&self, _: &()) -> bool {
            true
        }
        fn dispatch(&self, _: &()) {
            panic!("WM_QUIT must not dispatch");
        }
    }
    assert!(!pump_messages(&Quit, PUMP_BUDGET));
}

#[test]
fn display_watch_stop_during_pump_is_not_a_terminal_failure() {
    let (record, notifications) = fixture();
    record.actions.lock().push_back(Action::Stop);
    drive(
        record.clone(),
        notifications,
        no_events(&record),
        Box::new(|result| assert_eq!(result, Ok(()))),
    );
    assert!(record.kernel_live.lock().is_empty());
}

#[test]
fn display_watch_accepted_early_guard_drop_finishes_stopped_without_joining_owner() {
    use std::sync::mpsc;
    use std::time::Duration;
    let (record, notifications) = fixture();
    let signals = notifications.signals.clone();
    let (release, blocked) = mpsc::channel();
    let (completed, results) = mpsc::channel();
    let worker_record = record.clone();
    let guard = start(
        signals,
        move || {
            blocked.recv().unwrap();
            RecordingCalls(worker_record)
        },
        no_events(&record),
        Box::new(move |result| {
            completed.send(result).unwrap();
        }),
    )
    .unwrap();
    // This returns while creation is still blocked; a joining Drop would deadlock.
    drop(guard);
    release.send(()).unwrap();
    assert_eq!(
        results.recv_timeout(Duration::from_secs(5)).unwrap(),
        Err(DisplayContextError::Stopped)
    );
    assert!(results.recv_timeout(Duration::from_millis(20)).is_err());
    assert_eq!(count(&record.names(), "mta-init"), 0);
    assert!(record.kernel_live.lock().is_empty());
    assert_eq!(count(&record.names(), "unexpected-event"), 0);
}

#[test]
fn display_watch_immediate_kernel_failure_accepts_no_callbacks_and_retires_partial_event() {
    for failed in [1, 2] {
        let record = Arc::new(Record::default());
        record.fail(format!("create-event:{failed}"), 1);
        assert!(Signals::create(RecordingKernel(record.clone())).is_err());
        assert!(record.kernel_live.lock().is_empty());
        assert_eq!(
            count(&record.names(), "close-event:1"),
            usize::from(failed == 2)
        );
    }
}

fn recorded_terminal_watch(
    events: DisplayContextWatchCallback,
    ready: DisplayContextWatchReady,
) -> Result<Box<dyn DisplayContextWatchGuard>, DisplayContextError> {
    let (record, notifications) = fixture();
    record.actions.lock().push_back(Action::Fail);
    start(
        notifications.signals.clone(),
        move || RecordingCalls(record),
        events,
        ready,
    )
}

fn recorded_startup_failure_watch(
    events: DisplayContextWatchCallback,
    ready: DisplayContextWatchReady,
) -> Result<Box<dyn DisplayContextWatchGuard>, DisplayContextError> {
    let (record, notifications) = fixture();
    record.fail("register:Changed", 1);
    start(
        notifications.signals.clone(),
        move || RecordingCalls(record),
        events,
        ready,
    )
}

#[test]
fn display_watch_real_composite_read_delegation_survives_accepted_watch_failure() {
    use std::sync::mpsc;
    use std::time::Duration;
    use tessera_system::display_context::{DisplayContextCompletion, DisplayContextHost};

    struct RecordedRead(Arc<AtomicUsize>);
    impl DisplayContextHost for RecordedRead {
        fn read(&self, complete: DisplayContextCompletion) -> Result<(), DisplayContextError> {
            self.0.fetch_add(1, Ordering::AcqRel);
            complete(Ok(None));
            Ok(())
        }
    }
    for terminal in [false, true] {
        let reads = Arc::new(AtomicUsize::new(0));
        let host = crate::display_context::CompositeDisplayContextHost {
            read: RecordedRead(reads.clone()),
            watch: if terminal {
                recorded_terminal_watch
            } else {
                recorded_startup_failure_watch
            },
        };
        let (ready_send, ready_recv) = mpsc::channel();
        let (event_send, event_recv) = mpsc::channel();
        let guard = host
            .watch(
                Arc::new(move |event| event_send.send(event).unwrap()),
                Box::new(move |result| ready_send.send(result).unwrap()),
            )
            .unwrap();
        let ready = ready_recv.recv_timeout(Duration::from_secs(5)).unwrap();
        if terminal {
            assert_eq!(ready, Ok(()));
            assert_eq!(
                event_recv.recv_timeout(Duration::from_secs(5)).unwrap(),
                DisplayContextWatchEvent::Unavailable(DisplayContextError::Unavailable)
            );
        } else {
            assert_eq!(ready, Err(native(FAILURE)));
            assert!(event_recv.recv_timeout(Duration::from_millis(20)).is_err());
        }
        let (read_send, read_recv) = mpsc::channel();
        host.read(Box::new(move |result| read_send.send(result).unwrap()))
            .unwrap();
        assert_eq!(
            read_recv.recv_timeout(Duration::from_secs(5)).unwrap(),
            Ok(None)
        );
        assert_eq!(reads.load(Ordering::Acquire), 1);
        drop(guard);
    }
}

#[test]
fn display_watch_already_admitted_consumer_can_finish_after_nonjoining_guard_drop() {
    use std::sync::mpsc;
    use std::time::Duration;
    let (record, notifications) = fixture();
    let worker_record = record.clone();
    let (entered, enter_recv) = mpsc::channel();
    let (release, release_recv) = mpsc::channel();
    let release_recv = Mutex::new(release_recv);
    let (finished, finish_recv) = mpsc::channel();
    let (ready, ready_recv) = mpsc::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let delivered = calls.clone();
    let events: DisplayContextWatchCallback = Arc::new(move |event| {
        assert_eq!(event, DisplayContextWatchEvent::Changed);
        delivered.fetch_add(1, Ordering::AcqRel);
        entered.send(()).unwrap();
        release_recv.lock().recv().unwrap();
        finished.send(()).unwrap();
    });
    let guard = start(
        notifications.signals.clone(),
        move || RecordingCalls(worker_record),
        events,
        Box::new(move |result| ready.send(result).unwrap()),
    )
    .unwrap();
    assert_eq!(
        ready_recv.recv_timeout(Duration::from_secs(5)).unwrap(),
        Ok(())
    );
    enter_recv.recv_timeout(Duration::from_secs(5)).unwrap();
    // The callback is still blocked. Drop retracts later admission, not this
    // already admitted invocation, and cannot join the owner executing it.
    drop(guard);
    release.send(()).unwrap();
    finish_recv.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(calls.load(Ordering::Acquire), 1);
}
