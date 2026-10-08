// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::folders::idle::{self, Pump};
use crate::folders::idle::recording::{Calls, Event, Recording, Trace};
use parking_lot::Mutex;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::channel;
use std::thread::{self, ThreadId};
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Life {
    targets: AtomicUsize,
    depth: AtomicUsize,
    maximum_depth: AtomicUsize,
}

struct Depth<'a>(&'a Life);
impl Drop for Depth<'_> {
    fn drop(&mut self) { self.0.depth.fetch_sub(1, Ordering::SeqCst); }
}
impl Life {
    fn enter(&self) -> Depth<'_> {
        let depth = self.depth.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum_depth.fetch_max(depth, Ordering::SeqCst);
        Depth(self)
    }
}

struct Target {
    folder: FolderId,
    owner: ThreadId,
    life: Arc<Life>,
    trace: Trace,
    _thread_bound: PhantomData<Rc<()>>,
}
impl Drop for Target {
    fn drop(&mut self) {
        assert_eq!(self.owner, thread::current().id());
        self.life.targets.fetch_sub(1, Ordering::SeqCst);
        self.trace.push(Event::TargetDrop(self.owner));
    }
}

struct RecordedDriver {
    owner: ThreadId,
    life: Arc<Life>,
    trace: Trace,
    _thread_bound: PhantomData<Rc<()>>,
}
impl Drop for RecordedDriver {
    fn drop(&mut self) {
        assert_eq!(self.owner, thread::current().id());
        assert_eq!(self.life.targets.load(Ordering::SeqCst), 0);
        assert_eq!(self.life.depth.load(Ordering::SeqCst), 0);
        self.trace.push(Event::Uninitialize(self.owner));
    }
}
impl Driver for RecordedDriver {
    type Target = Target;
    fn resolve(&mut self, folder: FolderId) -> Result<Target, FolderError> {
        let _depth = self.life.enter();
        assert_eq!(self.owner, thread::current().id());
        self.trace.push(Event::Resolve(self.owner));
        self.life.targets.fetch_add(1, Ordering::SeqCst);
        Ok(Target { folder, owner: self.owner, life: Arc::clone(&self.life),
            trace: self.trace.clone(), _thread_bound: PhantomData })
    }
    fn same_target(&mut self, expected: &Target, fresh: &Target) -> Result<bool, FolderError> {
        let _depth = self.life.enter();
        Ok(expected.folder == fresh.folder)
    }
    fn dispatch(&mut self, _: &Target) -> Result<(), FolderError> {
        let _depth = self.life.enter();
        Ok(())
    }
}

fn factory(life: Arc<Life>, trace: Trace) -> impl FnMut() -> Result<RecordedDriver, FolderError> + Send {
    move || {
        let owner = thread::current().id();
        trace.push(Event::Initialized(owner));
        Ok(RecordedDriver { owner, life: Arc::clone(&life), trace: trace.clone(),
            _thread_bound: PhantomData })
    }
}

fn host(recording: &Arc<Recording>) -> (Arc<dyn FolderHost>, Arc<Life>) {
    let life = Arc::new(Life::default());
    let host = start_with_idle(factory(Arc::clone(&life), recording.trace.clone()),
        Arc::clone(recording), |wake| Pump::new(Calls::new(wake))).unwrap();
    (host, life)
}

fn read(host: &dyn FolderHost) -> FolderSnapshot {
    let (send, receive) = channel();
    host.read(Box::new(move |result| send.send(result).unwrap())).unwrap();
    receive.recv_timeout(DEADLINE).unwrap().unwrap()
}

fn ready(snapshot: &FolderSnapshot) -> FolderTarget {
    let FolderAvailability::Ready(target) = snapshot.get(FolderId::Desktop) else {
        panic!("recording target must be ready");
    };
    target.clone()
}

fn shutdown(host: Arc<dyn FolderHost>, recording: Arc<Recording>) -> Vec<Event> {
    let trace = recording.trace.clone();
    drop(host);
    drop(recording);
    trace.until(|events| events.contains(&Event::Close));
    trace.events()
}

fn assert_teardown(events: &[Event]) {
    let initialized = events.iter().find_map(|event| match event {
        Event::Initialized(owner) => Some(*owner), _ => None,
    }).unwrap();
    assert_ne!(initialized, thread::current().id());
    let uninitialize = events.iter().position(|event| matches!(event, Event::Uninitialize(_))).unwrap();
    for (index, event) in events.iter().enumerate() {
        if let Event::TargetDrop(owner) = event {
            assert_eq!(*owner, initialized);
            assert!(index < uninitialize);
        }
        if let Event::Resolve(owner) | Event::Uninitialize(owner) = event {
            assert_eq!(*owner, initialized);
        }
    }
    assert!(uninitialize < events.iter().position(|event| *event == Event::Close).unwrap());
}

#[test]
fn final_sender_disconnect_precedes_shutdown_signal() {
    let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
    let wake = Arc::new(Recording::default());
    let trace = wake.trace.clone();
    wake.during_signal(move || {
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Disconnected)));
        trace.push(Event::Disconnected);
    });
    let host = WorkerHost { sender: Some(sender), wake: Arc::clone(&wake),
        status: Arc::new(Status::default()) };
    drop(host);
    assert_eq!(wake.trace.events(), [Event::Disconnected, Event::Signal]);
}

#[test]
fn idle_live_service_final_host_drop_retains_wait_owner_and_releases_targets_first() {
    let recording = Arc::new(Recording::default());
    let trace = recording.trace.clone();
    let weak_event = Arc::downgrade(&recording);
    let (host, _) = host(&recording);
    read(&*host);
    recording.parked();
    drop(recording); // Only host and worker own the event now.
    assert!(weak_event.upgrade().is_some());
    drop(host); // Must return without waiting for any worker teardown.
    trace.until(|events| events.contains(&Event::Close));
    assert!(weak_event.upgrade().is_none());
    let events = trace.events();
    assert_eq!(events.iter().filter(|event| matches!(event, Event::TargetDrop(_))).count(), 7);
    assert_teardown(&events);
}

#[test]
fn wake_between_empty_and_wait_is_latched_and_coalesced_requests_drain_fifo() {
    let recording = Arc::new(Recording::default());
    let slot = Arc::new(Mutex::new(None::<std::sync::Weak<dyn FolderHost>>));
    let callback_slot = Arc::clone(&slot);
    let (entered, enter) = channel();
    let (release, released) = channel();
    let (send, receive) = channel();
    recording.before_wait(move || {
        entered.send(()).unwrap();
        released.recv_timeout(DEADLINE).unwrap();
        let host = callback_slot.lock().as_ref().unwrap().upgrade().unwrap();
        for index in 0..3 {
            let send = send.clone();
            host.read(Box::new(move |result| {
                assert!(result.is_ok());
                send.send(index).unwrap();
            })).unwrap();
        }
    });
    let (host, life) = host(&recording);
    *slot.lock() = Some(Arc::downgrade(&host));
    enter.recv_timeout(DEADLINE).unwrap();
    release.send(()).unwrap();
    for index in 0..3 { assert_eq!(receive.recv_timeout(DEADLINE).unwrap(), index); }
    recording.parked();
    assert_eq!(life.maximum_depth.load(Ordering::SeqCst), 1);
    assert_teardown(&shutdown(host, recording));
}

#[test]
fn completion_before_delayed_producer_signal_does_not_duplicate_a_request() {
    let recording = Arc::new(Recording::default());
    let (host, _) = host(&recording);
    read(&*host);
    recording.parked();
    let (entered, enter) = channel();
    let (release, released) = channel();
    recording.during_signal(move || {
        entered.send(()).unwrap();
        released.recv_timeout(DEADLINE).unwrap();
    });
    let producer_host = Arc::clone(&host);
    let (send, receive) = channel();
    let (submitted, submission) = channel();
    let count = Arc::new(AtomicUsize::new(0));
    let callback_count = Arc::clone(&count);
    thread::spawn(move || {
        submitted.send(producer_host.read(Box::new(move |result| {
            callback_count.fetch_add(1, Ordering::SeqCst);
            send.send(result.is_ok()).unwrap();
        }))).unwrap();
    });
    enter.recv_timeout(DEADLINE).unwrap(); // Accepted, but SetEvent is not called yet.
    recording.safety_timeout();
    assert!(receive.recv_timeout(DEADLINE).unwrap());
    release.send(()).unwrap();
    submission.recv_timeout(DEADLINE).unwrap().unwrap();
    recording.parked(); // Stale signal has been harmlessly consumed.
    assert_teardown(&shutdown(host, recording));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn pump_is_bounded_uses_null_all_input_filters_and_ignores_false_zero_results() {
    let recording = Arc::new(Recording::default());
    recording.messages(std::iter::repeat_n(42, idle::MESSAGE_BATCH + 3));
    let mut pump = Pump::new(Calls::new(Arc::clone(&recording)));
    pump.pump().unwrap();
    assert_eq!(recording.trace.events().iter().filter(|event| matches!(event, Event::Dispatch(_, _))).count(), 32);
    pump.wait().unwrap(); // Already inspected, unread input remains available.
    pump.pump().unwrap();
    let events = recording.trace.events();
    assert!(events.contains(&Event::Wait(idle::WaitOptions {
        handles: 1, timeout_ms: 250, wake_mask: 1279, flags: 4,
    })));
    for event in &events {
        match event {
            Event::Peek(options) => assert_eq!(*options, idle::PeekOptions { hwnd: 0, min: 0, max: 0, flags: 1 }),
            Event::Translate(_, result) => assert!(!result),
            Event::Dispatch(_, result) => assert_eq!(*result, 0),
            _ => {}
        }
    }
    assert_eq!(events.iter().filter(|event| matches!(event, Event::Dispatch(_, _))).count(), 35);
}

#[test]
fn empty_system_message_wake_and_local_safety_timeouts_do_not_resolve_os_data() {
    let recording = Arc::new(Recording::default());
    let (host, _) = host(&recording);
    read(&*host);
    recording.parked();
    for outcome in [Some(1), None, None] {
        let waits = recording.trace.events().iter().filter(|event| matches!(event, Event::Wait(_))).count();
        if let Some(value) = outcome { recording.wait_result(Ok(value)); }
        else { recording.safety_timeout(); }
        recording.trace.until(|events| events.iter().filter(|event| matches!(event, Event::Wait(_))).count() > waits);
        recording.parked();
    }
    assert_eq!(recording.trace.events().iter().filter(|event| matches!(event, Event::Resolve(_))).count(), 7);
    assert_teardown(&shutdown(host, recording));
}

#[test]
fn nonqueued_peek_and_completion_reentry_enqueue_with_driver_depth_one() {
    let recording = Arc::new(Recording::default());
    let (host, life) = host(&recording);
    read(&*host);
    recording.hold();
    recording.parked();
    let weak = Arc::downgrade(&host);
    let callback_life = Arc::clone(&life);
    let trace = recording.trace.clone();
    let (send, receive) = channel();
    recording.during_peek(move || {
        assert_eq!(callback_life.depth.load(Ordering::SeqCst), 0);
        let host = weak.upgrade().unwrap();
        let weak = Arc::downgrade(&host);
        host.read(Box::new(move |result| {
            assert_eq!(callback_life.depth.load(Ordering::SeqCst), 0);
            trace.push(Event::Completion);
            let target = ready(&result.unwrap());
            weak.upgrade().unwrap().open(FolderId::Desktop, target, Box::new(move |result| {
                assert_eq!(callback_life.depth.load(Ordering::SeqCst), 0);
                trace.push(Event::Completion);
                send.send(result).unwrap();
            })).unwrap();
        })).unwrap();
    });
    recording.wait_result(Ok(1));
    recording.resume();
    receive.recv_timeout(DEADLINE).unwrap().unwrap();
    assert_eq!(life.maximum_depth.load(Ordering::SeqCst), 1);
    assert_eq!(recording.trace.events().iter().filter(|event| **event == Event::Completion).count(), 2);
    assert_teardown(&shutdown(host, recording));
}

#[test]
fn message_flood_and_buffered_requests_get_bounded_turns_at_the_same_loop_boundary() {
    let recording = Arc::new(Recording::default());
    let (host, _) = host(&recording);
    read(&*host);
    recording.hold();
    recording.parked();
    let start = recording.trace.events().len();
    recording.messages(std::iter::repeat_n(42, idle::MESSAGE_BATCH * 3));
    let (send, receive) = channel();
    for index in 1..=3 {
        let send = send.clone();
        let trace = recording.trace.clone();
        host.read(Box::new(move |result| {
            assert!(result.is_ok());
            trace.push(Event::Completion);
            send.send(index).unwrap();
        })).unwrap();
    }
    recording.resume();
    for index in 1..=3 { assert_eq!(receive.recv_timeout(DEADLINE).unwrap(), index); }
    let events = recording.trace.events();
    let mut dispatched = 0;
    let mut completed = 0;
    for event in &events[start..] {
        match event {
            Event::Dispatch(_, _) => dispatched += 1,
            Event::Completion => {
                completed += 1;
                assert_eq!(dispatched, completed * idle::MESSAGE_BATCH);
            }
            _ => {}
        }
    }
    assert_eq!((completed, dispatched), (3, 96));
    assert_teardown(&shutdown(host, recording));
}

#[test]
fn accepted_signal_failure_returns_ok_and_safety_wake_completes_once_before_later_stopped() {
    let recording = Arc::new(Recording::default());
    let (host, _) = host(&recording);
    read(&*host);
    recording.hold();
    recording.parked();
    // The native adapter converts failing Win32 5 to this HRESULT immediately.
    recording.fail_signal(native_error(0x8007_0005_u32 as i32, "Recorded signal"));
    let (send, receive) = channel();
    let trace = recording.trace.clone();
    let count = Arc::new(AtomicUsize::new(0));
    let callback_count = Arc::clone(&count);
    assert!(host.read(Box::new(move |result| {
        callback_count.fetch_add(1, Ordering::SeqCst);
        trace.push(Event::Completion);
        send.send(result.err().unwrap()).unwrap();
    })).is_ok());
    let rejected = Arc::new(AtomicUsize::new(0));
    let rejected_callback = Arc::clone(&rejected);
    assert_eq!(host.read(Box::new(move |_| {
        rejected_callback.fetch_add(1, Ordering::SeqCst);
    })).unwrap_err().kind, FolderErrorKind::Stopped);
    recording.safety_timeout();
    recording.resume();
    assert_eq!(receive.recv_timeout(DEADLINE).unwrap().kind, FolderErrorKind::AccessDenied);
    let events = shutdown(host, recording);
    let release = events.iter().position(|event| matches!(event, Event::Uninitialize(_))).unwrap();
    assert!(release < events.iter().position(|event| *event == Event::Completion).unwrap());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(rejected.load(Ordering::SeqCst), 0);
    assert_teardown(&events);
}

#[test]
fn failed_final_drop_signal_is_recovered_without_a_live_sta_blocking_receive() {
    let recording = Arc::new(Recording::default());
    let (host, _) = host(&recording);
    read(&*host);
    recording.hold();
    recording.parked();
    recording.fail_signal(native_error(0x8007_0005_u32 as i32, "Recorded shutdown signal"));
    drop(host);
    recording.safety_timeout();
    recording.resume();
    let trace = recording.trace.clone();
    drop(recording);
    trace.until(|events| events.contains(&Event::Close));
    assert_teardown(&trace.events());
}

#[test]
fn quit_wait_failure_and_unexpected_wait_drain_buffered_and_racing_acceptances_despite_panic() {
    for fault in 0..3 {
        let recording = Arc::new(Recording::default());
        let trace = recording.trace.clone();
        let life = Arc::new(Life::default());
        let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
        let racing_sender = sender.clone(); // Models a submitter past its stopped check.
        let status = Arc::new(Status::default());
        let host: Arc<dyn FolderHost> = Arc::new(WorkerHost { sender: Some(sender),
            wake: Arc::clone(&recording), status: Arc::clone(&status) });
        let worker_status = Arc::clone(&status);
        let wake = Arc::clone(&recording);
        let create = factory(Arc::clone(&life), trace.clone());
        thread::spawn(move || run(receiver, create, Pump::new(Calls::new(wake)), worker_status));
        let target = ready(&read(&*host));
        recording.hold();
        recording.parked();
        let (send, receive) = channel();
        for index in 0..3 {
            let send = send.clone();
            let trace = trace.clone();
            if index == 1 {
                host.open(FolderId::Desktop, target.clone(), Box::new(move |result| {
                    trace.push(Event::Completion);
                    send.send(result.err().unwrap().kind).unwrap();
                })).unwrap();
            } else {
                host.read(Box::new(move |result| {
                    trace.push(Event::Completion);
                    send.send(result.err().unwrap().kind).unwrap();
                    if index == 0 { panic!("recorded terminal consumer panic"); }
                })).unwrap();
            }
        }
        let expected = if fault == 0 { FolderErrorKind::AccessDenied } else { FolderErrorKind::Stopped };
        match fault {
            0 => recording.wait_result(Err(native_error(0x8007_0005_u32 as i32, "Recorded wait"))),
            1 => recording.messages([idle::WM_QUIT]),
            _ => recording.wait_result(Ok(7)),
        }
        recording.resume();
        trace.until(|events| events.iter().any(|event| matches!(event, Event::Uninitialize(_))));
        assert!(status.stopped.load(Ordering::Acquire));
        let callback_trace = trace.clone();
        let racing_send = send.clone();
        assert!(racing_sender.try_send(Request::Read(Box::new(move |result| {
            callback_trace.push(Event::Completion);
            racing_send.send(result.err().unwrap().kind).unwrap();
        }))).is_ok());
        for _ in 0..4 { assert_eq!(receive.recv_timeout(DEADLINE).unwrap(), expected); }
        let rejected = Arc::new(AtomicUsize::new(0));
        let callback_rejected = Arc::clone(&rejected);
        assert_eq!(host.read(Box::new(move |_| {
            callback_rejected.fetch_add(1, Ordering::SeqCst);
        })).unwrap_err().kind, FolderErrorKind::Stopped);
        drop(racing_sender);
        let events = shutdown(host, recording);
        assert_eq!(rejected.load(Ordering::SeqCst), 0);
        assert_eq!(events.iter().filter(|event| **event == Event::Completion).count(), 4);
        assert_eq!(events.iter().filter(|event| matches!(event, Event::Resolve(_))).count(), 7);
        let uninitialize = events.iter().position(|event| matches!(event, Event::Uninitialize(_))).unwrap();
        assert!(uninitialize < events.iter().position(|event| *event == Event::Completion).unwrap());
        if fault == 1 {
            assert!(!events.iter().any(|event| matches!(event, Event::Translate(id, _) | Event::Dispatch(id, _) if *id == idle::WM_QUIT)));
        }
        assert_teardown(&events);
    }
}

#[test]
fn completion_can_release_final_host_without_self_join_and_pending_native_work_stays_fifo() {
    let recording = Arc::new(Recording::default());
    let (host, _) = host(&recording);
    read(&*host);
    recording.hold();
    recording.parked();
    let last_host = Arc::clone(&host);
    let (send, receive) = channel();
    let first_send = send.clone();
    host.read(Box::new(move |result| {
        assert!(result.is_ok());
        drop(last_host);
        first_send.send(1).unwrap();
    })).unwrap();
    host.read(Box::new(move |result| {
        assert!(result.is_ok());
        send.send(2).unwrap();
    })).unwrap();
    drop(host);
    recording.resume();
    assert_eq!(receive.recv_timeout(DEADLINE).unwrap(), 1);
    assert_eq!(receive.recv_timeout(DEADLINE).unwrap(), 2);
    let trace = recording.trace.clone();
    drop(recording);
    trace.until(|events| events.contains(&Event::Close));
    assert_teardown(&trace.events());
}
