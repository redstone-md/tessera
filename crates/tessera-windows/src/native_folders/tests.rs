// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread::{self, ThreadId};
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

use tessera_system::folders::{
    FolderAvailability, FolderError, FolderErrorKind, FolderHost, FolderId, FolderSnapshot,
    FolderTarget,
};

use super::worker::{Driver, QUEUE_CAPACITY, Service, known_folder_guid, native_error, start};

const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Record {
    identities: HashMap<FolderId, u64>,
    failures: HashMap<FolderId, FolderErrorKind>,
    resolutions: Vec<FolderId>,
    dispatches: Vec<(FolderId, u64)>,
    dispatched_allocations: Vec<usize>,
    owner: Option<ThreadId>,
    comparisons: usize,
    comparison_error: Option<FolderErrorKind>,
    dispatch_error: Option<FolderErrorKind>,
    allocated: usize,
    freed: usize,
    stopped: bool,
    pause: bool,
    entered: bool,
    panic_once: bool,
}

type Recording = Arc<(Mutex<Record>, Condvar)>;

struct RecordingTarget {
    folder: FolderId,
    identity: u64,
    allocation: usize,
    recording: Recording,
    _thread_bound: PhantomData<Rc<()>>,
}

impl Drop for RecordingTarget {
    fn drop(&mut self) {
        let (record, changed) = &*self.recording;
        let mut record = record.lock();
        assert_eq!(record.owner, Some(thread::current().id()));
        record.freed += 1;
        changed.notify_all();
    }
}

struct RecordingDriver {
    recording: Recording,
    _thread_bound: PhantomData<Rc<()>>,
}

impl Drop for RecordingDriver {
    fn drop(&mut self) {
        let (record, changed) = &*self.recording;
        let mut record = record.lock();
        assert_eq!(record.owner, Some(thread::current().id()));
        assert_eq!(
            record.allocated, record.freed,
            "targets must free before STA teardown"
        );
        record.stopped = true;
        changed.notify_all();
    }
}

impl Driver for RecordingDriver {
    type Target = RecordingTarget;

    fn resolve(&mut self, folder: FolderId) -> Result<Self::Target, FolderError> {
        let (record, changed) = &*self.recording;
        let mut record = record.lock();
        assert_eq!(record.owner, Some(thread::current().id()));
        record.resolutions.push(folder);
        if record.pause {
            record.entered = true;
            changed.notify_all();
            let timeout = changed.wait_while_for(&mut record, |record| record.pause, DEADLINE);
            assert!(
                !timeout.timed_out(),
                "test must release its recording worker"
            );
        }
        if std::mem::take(&mut record.panic_once) {
            drop(record);
            panic!("recorded native failure");
        }
        if let Some(kind) = record.failures.get(&folder) {
            return Err(FolderError::new(*kind, "recorded resolution failure"));
        }
        let identity = *record.identities.get(&folder).unwrap_or(&1);
        record.allocated += 1;
        Ok(RecordingTarget {
            folder,
            identity,
            allocation: record.allocated,
            recording: Arc::clone(&self.recording),
            _thread_bound: PhantomData,
        })
    }

    fn same_target(
        &mut self,
        expected: &Self::Target,
        fresh: &Self::Target,
    ) -> Result<bool, FolderError> {
        let mut record = self.recording.0.lock();
        assert_eq!(record.owner, Some(thread::current().id()));
        record.comparisons += 1;
        if let Some(kind) = record.comparison_error {
            return Err(FolderError::new(kind, "recorded comparison failure"));
        }
        Ok(expected.folder == fresh.folder && expected.identity == fresh.identity)
    }

    fn dispatch(&mut self, fresh: &Self::Target) -> Result<(), FolderError> {
        let mut record = self.recording.0.lock();
        assert_eq!(record.owner, Some(thread::current().id()));
        if let Some(kind) = record.dispatch_error {
            return Err(FolderError::new(kind, "recorded dispatch failure"));
        }
        record.dispatches.push((fresh.folder, fresh.identity));
        record.dispatched_allocations.push(fresh.allocation);
        Ok(())
    }
}

fn recording() -> Recording {
    Arc::new((Mutex::new(Record::default()), Condvar::new()))
}

fn driver(recording: &Recording) -> RecordingDriver {
    recording.0.lock().owner = Some(thread::current().id());
    RecordingDriver {
        recording: Arc::clone(recording),
        _thread_bound: PhantomData,
    }
}

fn service(recording: &Recording) -> Service<RecordingDriver> {
    Service::new(driver(recording), Arc::new(()))
}

fn host(recording: &Recording) -> Arc<dyn FolderHost> {
    let recording = Arc::clone(recording);
    start(move || Ok(driver(&recording))).unwrap()
}

fn ready(snapshot: &FolderSnapshot, folder: FolderId) -> FolderTarget {
    let FolderAvailability::Ready(target) = snapshot.get(folder) else {
        panic!("recorded folder must be ready");
    };
    target.clone()
}

fn read(host: &dyn FolderHost) -> Result<FolderSnapshot, FolderError> {
    let (send, receive) = channel();
    host.read(Box::new(move |result| send.send(result).unwrap()))
        .unwrap();
    receive.recv_timeout(DEADLINE).unwrap()
}

fn wait_for(recording: &Recording, predicate: impl FnMut(&Record) -> bool) {
    let (record, changed) = &**recording;
    let mut predicate = predicate;
    let mut record = record.lock();
    let timeout = changed.wait_while_for(&mut record, |record| !predicate(record), DEADLINE);
    assert!(
        !timeout.timed_out() || predicate(&record),
        "recording worker deadline"
    );
}

#[test]
fn all_seven_guids_match_the_pinned_shell_values() {
    assert_eq!(
        FolderId::ALL.map(known_folder_guid),
        [
            0xae50c081_ebd2_438a_8655_8a092e34987a,
            0xb4bfcc3a_db2c_424c_b029_7fe99a87c641,
            0x374de290_123f_4565_9164_39c4925e467b,
            0xfdd39ad0_238f_46af_adb4_6c85480369c7,
            0x4bd8d571_6d19_48d3_be97_422220080e43,
            0x33e28130_4e1e_4676_835a_98395c3bc3bb,
            0x18989b1d_99b5_455b_841c_ab7c74e4ddfc,
        ]
    );
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::{
            FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music,
            FOLDERID_Pictures, FOLDERID_Recent, FOLDERID_Videos,
        };
        assert_eq!(
            FolderId::ALL.map(|folder| windows::core::GUID::from_u128(known_folder_guid(folder))),
            [
                FOLDERID_Recent,
                FOLDERID_Desktop,
                FOLDERID_Downloads,
                FOLDERID_Documents,
                FOLDERID_Music,
                FOLDERID_Pictures,
                FOLDERID_Videos
            ]
        );
    }
}

#[test]
fn read_only_resolves_ordered_rows_and_keeps_each_error_independent() {
    let recording = recording();
    recording
        .0
        .lock()
        .failures
        .insert(FolderId::Documents, FolderErrorKind::AccessDenied);
    let mut service = service(&recording);
    let snapshot = service.read();
    assert!(
        matches!(snapshot.get(FolderId::Documents), FolderAvailability::Unavailable(error)
        if error.kind == FolderErrorKind::AccessDenied)
    );
    for folder in FolderId::ALL
        .into_iter()
        .filter(|folder| *folder != FolderId::Documents)
    {
        ready(&snapshot, folder);
    }
    let record = recording.0.lock();
    assert_eq!(record.resolutions, FolderId::ALL);
    assert!(record.dispatches.is_empty());
    assert_eq!(record.allocated, 6);
    drop(record);
    drop(service);
    assert_eq!(recording.0.lock().freed, 6);
}

#[test]
fn wrong_provider_type_and_category_reject_before_any_fresh_resolution() {
    let first_recording = recording();
    let second_recording = recording();
    let mut first = service(&first_recording);
    let mut second = service(&second_recording);
    let target = ready(&first.read(), FolderId::Downloads);
    second.read();
    let first_count = first_recording.0.lock().resolutions.len();
    let second_count = second_recording.0.lock().resolutions.len();
    for result in [
        first.open(FolderId::Desktop, &target),
        first.open(FolderId::Downloads, &FolderTarget::new("forged path")),
        second.open(FolderId::Downloads, &target),
    ] {
        assert_eq!(result.unwrap_err().kind, FolderErrorKind::TargetChanged);
    }
    assert_eq!(first_recording.0.lock().resolutions.len(), first_count);
    assert_eq!(second_recording.0.lock().resolutions.len(), second_count);
    assert!(first_recording.0.lock().dispatches.is_empty());
    assert!(second_recording.0.lock().dispatches.is_empty());
}

#[test]
fn redirected_missing_or_denied_fresh_target_never_dispatches_and_frees_temporary_identity() {
    let recording = recording();
    let mut service = service(&recording);
    let target = ready(&service.read(), FolderId::Downloads);
    recording.0.lock().identities.insert(FolderId::Downloads, 2);
    assert_eq!(
        service.open(FolderId::Downloads, &target).unwrap_err().kind,
        FolderErrorKind::TargetChanged
    );
    assert_eq!(recording.0.lock().freed, 1);
    for kind in [FolderErrorKind::Unavailable, FolderErrorKind::AccessDenied] {
        recording
            .0
            .lock()
            .failures
            .insert(FolderId::Downloads, kind);
        assert_eq!(
            service.open(FolderId::Downloads, &target).unwrap_err().kind,
            kind
        );
    }
    assert!(recording.0.lock().dispatches.is_empty());
    drop(service);
    let record = recording.0.lock();
    assert_eq!(record.allocated, record.freed);
}

#[test]
fn unchanged_reread_preserves_expectation_but_redirected_reread_replaces_it() {
    let recording = recording();
    let mut service = service(&recording);
    let old = ready(&service.read(), FolderId::Desktop);
    service.read();
    service.open(FolderId::Desktop, &old).unwrap();
    recording.0.lock().identities.insert(FolderId::Desktop, 3);
    let new = ready(&service.read(), FolderId::Desktop);
    let resolutions = recording.0.lock().resolutions.len();
    assert_eq!(
        service.open(FolderId::Desktop, &old).unwrap_err().kind,
        FolderErrorKind::TargetChanged
    );
    assert_eq!(recording.0.lock().resolutions.len(), resolutions);
    service.open(FolderId::Desktop, &new).unwrap();
    assert_eq!(
        recording.0.lock().dispatches,
        [(FolderId::Desktop, 1), (FolderId::Desktop, 3)]
    );
}

#[test]
fn comparison_denial_and_dispatch_failure_release_fresh_allocations() {
    let recording = recording();
    let mut service = service(&recording);
    let target = ready(&service.read(), FolderId::Pictures);
    recording.0.lock().comparison_error = Some(FolderErrorKind::AccessDenied);
    assert_eq!(
        service.open(FolderId::Pictures, &target).unwrap_err().kind,
        FolderErrorKind::AccessDenied
    );
    {
        let mut record = recording.0.lock();
        record.comparison_error = None;
        record.dispatch_error = Some(FolderErrorKind::Other);
    }
    assert_eq!(
        service.open(FolderId::Pictures, &target).unwrap_err().kind,
        FolderErrorKind::Other
    );
    assert_eq!(recording.0.lock().freed, 2);
    assert!(recording.0.lock().dispatches.is_empty());
    drop(service);
    let record = recording.0.lock();
    assert_eq!(record.allocated, record.freed);
}

#[test]
fn native_error_mapping_is_structured_and_does_not_disclose_native_details() {
    for (code, expected) in [
        (0x8007_0005_u32, FolderErrorKind::AccessDenied),
        (0x8007_0002, FolderErrorKind::Unavailable),
        (0x8007_0003, FolderErrorKind::Unavailable),
        (0x8007_0035, FolderErrorKind::Unavailable),
        (0x8007_0057, FolderErrorKind::Unsupported),
        (0x8004_0154, FolderErrorKind::Unsupported),
        (0x8007_00aa, FolderErrorKind::Busy),
        (0x8001_010a, FolderErrorKind::Busy),
        (0x8000_4005, FolderErrorKind::Other),
    ] {
        let error = native_error(code as i32, "Folder resolution");
        assert_eq!(error.kind, expected);
        assert_eq!(
            error.message,
            format!("Folder resolution failed (0x{code:08X}).")
        );
    }
}

#[test]
fn accepted_worker_open_completes_once_and_drains_without_ui_join() {
    let recording = recording();
    let host = host(&recording);
    let target = ready(&read(&*host).unwrap(), FolderId::Videos);
    let (send, receive) = channel();
    let count = Arc::new(AtomicUsize::new(0));
    let callback_count = Arc::clone(&count);
    host.open(
        FolderId::Videos,
        target,
        Box::new(move |result| {
            callback_count.fetch_add(1, Ordering::SeqCst);
            send.send(result).unwrap();
        }),
    )
    .unwrap();
    drop(host);
    receive.recv_timeout(DEADLINE).unwrap().unwrap();
    wait_for(&recording, |record| record.stopped);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(recording.0.lock().dispatches, [(FolderId::Videos, 1)]);
    assert_eq!(recording.0.lock().dispatched_allocations, [8]);
    assert_ne!(recording.0.lock().owner, Some(thread::current().id()));
}

#[test]
fn bounded_queue_rejects_promptly_without_completion_and_preserves_accepted_work() {
    let recording = recording();
    recording.0.lock().pause = true;
    let host = host(&recording);
    let (send, receive) = channel();
    let first_send = send.clone();
    host.read(Box::new(move |result| {
        first_send.send(result.is_ok()).unwrap()
    }))
    .unwrap();
    wait_for(&recording, |record| record.entered);
    for _ in 0..QUEUE_CAPACITY {
        let send = send.clone();
        host.read(Box::new(move |result| send.send(result.is_ok()).unwrap()))
            .unwrap();
    }
    let rejected = Arc::new(AtomicUsize::new(0));
    let callback_rejected = Arc::clone(&rejected);
    let error = host
        .read(Box::new(move |_| {
            callback_rejected.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap_err();
    assert_eq!(error.kind, FolderErrorKind::Busy);
    drop(host); // Drop cannot wait for the paused worker.
    {
        let (record, changed) = &*recording;
        record.lock().pause = false;
        changed.notify_all();
    }
    for _ in 0..=QUEUE_CAPACITY {
        assert!(receive.recv_timeout(DEADLINE).unwrap());
    }
    wait_for(&recording, |record| record.stopped);
    assert_eq!(rejected.load(Ordering::SeqCst), 0);
}

#[test]
fn apartment_initialization_failure_completes_once_and_can_retry() {
    let recording = recording();
    let attempts = Arc::new(AtomicUsize::new(0));
    let factory_attempts = Arc::clone(&attempts);
    let factory_recording = Arc::clone(&recording);
    let host = start(move || {
        if factory_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(FolderError::new(
                FolderErrorKind::Other,
                "recorded apartment failure",
            ))
        } else {
            Ok(driver(&factory_recording))
        }
    })
    .unwrap();
    assert_eq!(read(&*host).unwrap_err().kind, FolderErrorKind::Other);
    let snapshot = read(&*host).unwrap();
    ready(&snapshot, FolderId::Music);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    drop(host);
    wait_for(&recording, |record| record.stopped);
}

#[test]
fn driver_or_consumer_panic_does_not_strand_later_accepted_completions() {
    let recording = recording();
    recording.0.lock().panic_once = true;
    let host = host(&recording);
    assert_eq!(read(&*host).unwrap_err().kind, FolderErrorKind::Other);
    host.read(Box::new(|_| panic!("recorded consumer failure")))
        .unwrap();
    let snapshot = read(&*host).unwrap();
    ready(&snapshot, FolderId::Recent);
    drop(host);
    wait_for(&recording, |record| record.stopped);
}

#[test]
fn worker_completion_can_reenter_host_without_lock_or_duplicate_dispatch() {
    let recording = recording();
    let host = host(&recording);
    let reentrant_host = Arc::clone(&host);
    let (send, receive): (_, Receiver<Result<(), FolderError>>) = channel();
    host.read(Box::new(move |result| {
        let target = ready(&result.unwrap(), FolderId::Documents);
        reentrant_host
            .open(
                FolderId::Documents,
                target,
                Box::new(move |result| {
                    send.send(result).unwrap();
                }),
            )
            .unwrap();
    }))
    .unwrap();
    receive.recv_timeout(DEADLINE).unwrap().unwrap();
    assert_eq!(recording.0.lock().dispatches, [(FolderId::Documents, 1)]);
    drop(host);
    wait_for(&recording, |record| record.stopped);
}

#[cfg(not(windows))]
#[test]
fn non_windows_factory_is_explicitly_unsupported() {
    let error = super::native_folder_host().err().unwrap();
    assert_eq!(error.kind, FolderErrorKind::Unsupported);
}
