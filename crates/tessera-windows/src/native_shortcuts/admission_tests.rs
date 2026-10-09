// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Controlled internal admission intervals; no worker, SDK, input or hook calls.

use super::*;
use std::sync::atomic::AtomicUsize;
use tessera_system::shortcuts::{KeyChord, ShortcutAction, TriggerPoint};

// Reuse the real host's mutex types and explicit poisoning recovery policy.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct CursorRecording(usize);

impl Backend for CursorRecording {
    fn wake(&self) -> Wake {
        Arc::new(|| {})
    }
    fn install_launcher(&mut self, _: u64) -> Result<(), ShortcutError> {
        Ok(())
    }
    fn register_settings(&mut self, _: &KeyChord, _: u64) -> Result<(), ShortcutError> {
        Ok(())
    }
    fn cleanup(&mut self) -> Result<(), ShortcutError> {
        Ok(())
    }
    fn wait(&mut self) -> Result<Option<BackendEvent>, ShortcutError> {
        Ok(None)
    }
    fn cursor(&mut self) -> Option<TriggerPoint> {
        self.0 += 1;
        None
    }
}

#[test]
fn dead_occupied_slot_cannot_inherit_a_retiring_listeners_generation_or_gesture() {
    let (shared, commands, deliveries) = channel_state();
    let host = Host {
        shared: shared.clone(),
    };
    let old_events = Arc::new(AtomicUsize::new(0));
    let old_counter = old_events.clone();
    let old_guard = host
        .subscribe(Arc::new(move |_| {
            old_counter.fetch_add(1, Ordering::AcqRel);
        }))
        .expect("first guard");
    *lock(&shared.admission) = 5;
    shared.gate.generation.store(5, Ordering::Release);
    let old_listener = lock(&shared.listener).as_ref().unwrap().clone();

    // Exact reviewed interval: listener death was published, but its destructor
    // has not acquired the slot mutex or published the retired watermark yet.
    old_listener.alive.store(false, Ordering::Release);
    let rejected_events = Arc::new(AtomicUsize::new(0));
    let rejected_counter = rejected_events.clone();
    let rejected = host.subscribe(Arc::new(move |_| {
        rejected_counter.fetch_add(1, Ordering::AcqRel);
    }));
    assert!(matches!(rejected, Err(error) if error.kind() == ShortcutErrorKind::Busy));
    assert!(Arc::ptr_eq(
        lock(&shared.listener).as_ref().unwrap(),
        &old_listener
    ));
    assert_eq!(shared.gate.generation.load(Ordering::Acquire), 5);
    assert_eq!(shared.gate.retirement.load(Ordering::Acquire), 0);

    drop(old_guard);
    assert!(!shared.gate.listening.load(Ordering::Acquire));
    assert_eq!(shared.gate.generation.load(Ordering::Acquire), 0);
    assert_eq!(shared.gate.retirement.load(Ordering::Acquire), 5);
    assert!(lock(&shared.listener).is_none());

    let new_events = Arc::new(AtomicUsize::new(0));
    let new_counter = new_events.clone();
    let new_guard = host
        .subscribe(Arc::new(move |_| {
            new_counter.fetch_add(1, Ordering::AcqRel);
        }))
        .expect("replacement after retirement");
    let (completed, result) = mpsc::sync_channel(1);
    host.configure(
        ShortcutConfig::default(),
        Box::new(move |snapshot| {
            completed.try_send(snapshot).unwrap();
        }),
    )
    .expect("replacement accepted");
    let command = loop {
        match commands.try_recv().expect("admitted command or wake") {
            Command::Configure(command) => break command,
            Command::Wake => {}
        }
    };
    assert_eq!(command.generation, 6);
    let snapshot = ShortcutSnapshot::new(&command.config, command.generation, Ok(()), Ok(()));
    shared.complete(command, Ok(snapshot));
    match deliveries.try_recv().expect("one accepted completion") {
        Delivery::Completed(completion, snapshot) => completion(snapshot),
        _ => panic!("unexpected event before registration completion"),
    }
    assert_eq!(result.try_recv().unwrap().unwrap().generation, 6);
    assert!(matches!(
        result.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));

    let mut backend = CursorRecording(0);
    shared.trigger(
        &mut backend,
        RawTrigger {
            generation: 5,
            action: ShortcutAction::ToggleLauncher,
            reserved: false,
        },
    );
    assert_eq!(
        backend.0, 0,
        "old gesture cannot even query cursor for new listener"
    );
    assert!(matches!(
        deliveries.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(!retirement_due(
        6,
        shared.gate.retirement.load(Ordering::Acquire)
    ));
    assert_eq!(shared.gate.generation.load(Ordering::Acquire), 6);

    shared.trigger(
        &mut backend,
        RawTrigger {
            generation: 6,
            action: ShortcutAction::ToggleLauncher,
            reserved: false,
        },
    );
    match deliveries.try_recv().expect("fresh owned event") {
        Delivery::Event(listener, event) => {
            shared.gate.events.fetch_sub(1, Ordering::AcqRel);
            assert!(listener.alive.load(Ordering::Acquire));
            (listener.sink)(event);
        }
        _ => panic!("unexpected completion/fault for fresh gesture"),
    }
    assert_eq!(backend.0, 1);
    assert_eq!(old_events.load(Ordering::Acquire), 0);
    assert_eq!(rejected_events.load(Ordering::Acquire), 0);
    assert_eq!(new_events.load(Ordering::Acquire), 1);
    drop(new_guard);
}

#[test]
fn generation_exhaustion_rejects_without_callback_and_revokes_native_authority() {
    let (shared, commands, deliveries) = channel_state();
    let host = Host {
        shared: shared.clone(),
    };
    *lock(&shared.admission) = u64::MAX;
    shared.gate.generation.store(u64::MAX, Ordering::Release);
    let callbacks = Arc::new(AtomicUsize::new(0));
    let callback_counter = callbacks.clone();
    let result = host.configure(
        ShortcutConfig::default(),
        Box::new(move |_| {
            callback_counter.fetch_add(1, Ordering::AcqRel);
        }),
    );
    assert!(matches!(result, Err(error) if error.kind() == ShortcutErrorKind::Other));
    assert!(shared.gate.closed.load(Ordering::Acquire));
    assert_eq!(shared.gate.generation.load(Ordering::Acquire), 0);
    assert_eq!(*lock(&shared.admission), u64::MAX);
    assert_eq!(shared.pending.load(Ordering::Acquire), 0);
    assert_eq!(callbacks.load(Ordering::Acquire), 0);
    assert!(matches!(commands.try_recv(), Ok(Command::Wake)));
    assert!(matches!(
        deliveries.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}
