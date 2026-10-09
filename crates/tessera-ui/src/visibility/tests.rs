// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tessera_system::visibility::{
    DEFAULT_HIDE_DELAY, DEFAULT_SHOW_DELAY, PointerWatchCallback, PointerWatchCompletion,
    PointerWatchEvent, VisibilityDecision,
};

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

#[test]
fn visibility_deadline_repeated_target_does_not_restart_and_fires_at_800ms() {
    let mut state = BarState::default();
    state.apply(VisibilityDecision::HideAfter(DEFAULT_HIDE_DELAY), 7, ms(0));
    let pending = state.pending.unwrap();
    for now in [20, 100, 300, 799] {
        state.apply(
            VisibilityDecision::HideAfter(DEFAULT_HIDE_DELAY),
            7,
            ms(now),
        );
        assert_eq!(state.pending, Some(pending));
    }
    assert!(!state.expire(pending.token, 7, ms(799)));
    assert!(state.visible());
    assert!(state.expire(pending.token, 7, ms(800)));
    assert!(!state.visible());
    assert!(!state.expire(pending.token, 7, ms(900)));
}

#[test]
fn visibility_deadline_reveal_is_100ms_and_opposite_target_cancels_old_hide() {
    let mut state = BarState::default();
    state.apply(VisibilityDecision::HideAfter(DEFAULT_HIDE_DELAY), 1, ms(0));
    let hide = state.pending.unwrap();
    state.apply(
        VisibilityDecision::ShowAfter(DEFAULT_SHOW_DELAY),
        1,
        ms(799),
    );
    assert!(state.pending.is_none());
    assert!(!state.expire(hide.token, 1, ms(800)));
    state.apply(
        VisibilityDecision::HideAfter(DEFAULT_HIDE_DELAY),
        1,
        ms(900),
    );
    let hide = state.pending.unwrap();
    assert!(state.expire(hide.token, 1, ms(1700)));
    state.apply(
        VisibilityDecision::ShowAfter(DEFAULT_SHOW_DELAY),
        1,
        ms(1800),
    );
    let show = state.pending.unwrap();
    state.apply(
        VisibilityDecision::ShowAfter(DEFAULT_SHOW_DELAY),
        1,
        ms(1850),
    );
    assert_eq!(state.pending, Some(show));
    assert!(!state.expire(show.token, 1, ms(1899)));
    assert!(state.expire(show.token, 1, ms(1900)));
    assert!(state.visible());
}

#[test]
fn visibility_deadline_geometry_replacement_and_workspace_freeze_deny_stale_timer() {
    let mut state = BarState::default();
    state.apply(VisibilityDecision::HideAfter(DEFAULT_HIDE_DELAY), 4, ms(0));
    let old = state.pending.unwrap();
    state.invalidate();
    state.apply(
        VisibilityDecision::HideAfter(DEFAULT_HIDE_DELAY),
        5,
        ms(300),
    );
    let replacement = state.pending.unwrap();
    assert!(!state.expire(old.token, 5, ms(900)));
    assert!(state.visible());
    assert_eq!(replacement.at, ms(1100));
    state.apply(VisibilityDecision::Keep, 5, ms(1000));
    assert!(state.pending.is_none());
    assert!(!state.expire(replacement.token, 5, ms(1100)));
    assert!(state.visible());
}

#[test]
fn visibility_fullscreen_is_separate_from_workspace_frozen_autohide_result() {
    let mut state = BarState::default();
    state.apply(VisibilityDecision::HideAfter(DEFAULT_HIDE_DELAY), 8, ms(0));
    let pending = state.pending.unwrap();
    state.apply(VisibilityDecision::HideNow, 8, ms(200));
    assert!(!state.visible());
    assert!(state.pending.is_none());
    assert!(!state.expire(pending.token, 8, ms(800)));
    state.apply(VisibilityDecision::Keep, 8, ms(900));
    assert!(state.visible()); // outer suppression did not mutate autohide
    state.apply(
        VisibilityDecision::HideAfter(DEFAULT_HIDE_DELAY),
        8,
        ms(1000),
    );
    let pending = state.pending.unwrap();
    assert!(state.expire(pending.token, 8, ms(1800)));
    state.apply(VisibilityDecision::Keep, 8, ms(1900));
    assert!(!state.visible()); // genuine autohide state remains frozen
    state.apply(VisibilityDecision::ShowNow, 8, ms(2000));
    assert!(state.visible());
}

#[test]
fn visibility_immediate_unknown_recovery_cancels_pending_hide() {
    let mut state = BarState::default();
    state.apply(VisibilityDecision::HideAfter(DEFAULT_HIDE_DELAY), 6, ms(0));
    let pending = state.pending.unwrap();
    state.apply(VisibilityDecision::ShowNow, 6, ms(400));
    assert!(!state.expire(pending.token, 6, ms(800)));
    assert!(state.visible());
}

#[test]
fn visibility_environment_absence_is_explicit_and_any_touch_is_conservative() {
    let unknown = BarFacts::default();
    assert_eq!(
        model::protected_facts(unknown, PointerEnvironment::default()).touch_primary,
        None
    );
    assert_eq!(
        model::protected_facts(
            unknown,
            PointerEnvironment {
                touch_capable: Some(false)
            }
        )
        .touch_primary,
        Some(false)
    );
    for fact in [None, Some(false), Some(true)] {
        assert_eq!(
            model::protected_facts(
                BarFacts {
                    touch_primary: fact,
                    ..unknown
                },
                PointerEnvironment {
                    touch_capable: Some(true)
                }
            )
            .touch_primary,
            Some(true)
        );
    }
    assert_eq!(
        model::protected_facts(
            BarFacts {
                touch_primary: Some(true),
                ..unknown
            },
            PointerEnvironment {
                touch_capable: Some(false)
            }
        )
        .touch_primary,
        Some(true)
    );
}

#[test]
fn visibility_checked_no_touch_snapshot_enables_default_overlap_policy_without_fake_fact() {
    let facts = BarFacts {
        overlap: Some(true),
        own_focus: Some(false),
        ..BarFacts::default()
    };
    for (touch_capable, expected) in [
        (None, VisibilityDecision::ShowNow),
        (Some(true), VisibilityDecision::ShowNow),
        (
            Some(false),
            VisibilityDecision::HideAfter(DEFAULT_HIDE_DELAY),
        ),
    ] {
        assert_eq!(
            decide_visibility(VisibilityInputs {
                config: VisibilityConfig::default().dock,
                facts: model::protected_facts(facts, PointerEnvironment { touch_capable }),
                pointer_ready: true,
                selected_edge: Some(false),
            }),
            expected
        );
    }
    let mut dock = BarState::default();
    let mut toolbar = BarState::default();
    dock.apply(VisibilityDecision::HideAfter(DEFAULT_HIDE_DELAY), 1, ms(0));
    toolbar.apply(VisibilityDecision::ShowNow, 1, ms(0));
    assert!(dock.expire(dock.pending.unwrap().token, 1, ms(800)));
    assert!(!dock.visible());
    assert!(toolbar.visible());
    assert!(toolbar.pending.is_none());
}

struct MockGuard(Arc<AtomicUsize>);
impl PointerWatchGuard for MockGuard {}
impl Drop for MockGuard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct MockHost {
    watch_calls: AtomicUsize,
    retired: Arc<AtomicUsize>,
    events: Mutex<Option<PointerWatchCallback>>,
    completion: Mutex<Option<PointerWatchCompletion>>,
    immediate: Mutex<Option<PointerWatchError>>,
}

impl PointerHost for MockHost {
    fn watch(
        &self,
        events: PointerWatchCallback,
        ready: PointerWatchCompletion,
    ) -> Result<Box<dyn PointerWatchGuard>, PointerWatchError> {
        self.watch_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = *self.immediate.lock() {
            return Err(error);
        }
        *self.events.lock() = Some(events);
        *self.completion.lock() = Some(ready);
        Ok(Box::new(MockGuard(self.retired.clone())))
    }
}

impl MockHost {
    fn ready(&self, result: Result<(), PointerWatchError>) {
        let completion = self.completion.lock().take().unwrap();
        completion(result);
    }
    fn event(&self, event: PointerWatchEvent) {
        let events = self.events.lock().clone().unwrap();
        events(event);
    }
}

type Effects = Rc<RefCell<Vec<(BarVisibility, u64)>>>;

fn actor() -> (
    Rc<VisibilityController>,
    Arc<MockHost>,
    Effects,
    Arc<AtomicUsize>,
) {
    let host = Arc::new(MockHost::default());
    let effects = Rc::new(RefCell::new(Vec::new()));
    let recorded = effects.clone();
    let actor = VisibilityController::new(host.clone(), move |visibility, revision| {
        recorded.borrow_mut().push((visibility, revision));
    });
    let wakes = Arc::new(AtomicUsize::new(0));
    let counted = wakes.clone();
    // Test adapter at the real weak-dispatch seam: count enqueues only, never
    // invoke GUI processing/effects from a provider callback or register a hook.
    actor.mailbox.lock().wake = Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    (actor, host, effects, wakes)
}

fn geometry(revision: u64) -> VisibilityGeometry {
    VisibilityGeometry {
        revision,
        monitor: Rect::new(-500, -100, 500, 400).unwrap(),
        dock_edge: Edge::Bottom,
        toolbar_edge: Edge::Top,
    }
}

#[test]
fn visibility_controller_construction_and_callbacks_are_inert_until_weak_gui_drain() {
    let host = Arc::new(MockHost::default());
    let count = Rc::new(Cell::new(0));
    let recorded = count.clone();
    let inert =
        VisibilityController::new(host.clone(), move |_, _| recorded.set(recorded.get() + 1));
    assert_eq!(host.watch_calls.load(Ordering::SeqCst), 0);
    assert_eq!(inert.start(), Err(PointerWatchError::Unavailable));
    assert_eq!(count.get(), 0);
    assert_eq!(host.watch_calls.load(Ordering::SeqCst), 0);
    let (actor, host, effects, wakes) = actor();
    actor.update(
        VisibilityConfig::default(),
        Some(geometry(1)),
        VisibilityObservations::default(),
    );
    assert_eq!(effects.borrow().len(), 1);
    actor.start().unwrap();
    host.ready(Ok(()));
    host.event(PointerWatchEvent::Environment(PointerEnvironment {
        touch_capable: Some(false),
    }));
    host.event(PointerWatchEvent::Position(PhysicalPoint {
        x: -200,
        y: 299,
    }));
    assert_eq!(actor.readiness(), PointerReadiness::Starting);
    assert_eq!(effects.borrow().len(), 1);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    actor.process_events();
    assert_eq!(actor.readiness(), PointerReadiness::Ready);
    assert_eq!(
        actor.state.borrow().pointer,
        Some(PhysicalPoint { x: -200, y: 299 })
    );
    assert_eq!(actor.current(), BarVisibility::default()); // focus is still unknown
    assert_eq!(effects.borrow().len(), 1);
}

#[test]
fn visibility_controller_pointer_bursts_coalesce_without_catalog_or_effect_calls() {
    let (actor, host, effects, wakes) = actor();
    actor.update(
        VisibilityConfig::default(),
        Some(geometry(2)),
        VisibilityObservations::default(),
    );
    actor.start().unwrap();
    host.ready(Ok(()));
    for value in 0..1000 {
        host.event(PointerWatchEvent::Position(PhysicalPoint {
            x: value,
            y: 50,
        }));
    }
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert_eq!(
        actor.mailbox.lock().position,
        Some((Some(2), PhysicalPoint { x: 999, y: 50 }))
    );
    actor.process_events();
    assert_eq!(effects.borrow().len(), 1);
    assert_eq!(host.watch_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        actor.state.borrow().pointer,
        Some(PhysicalPoint { x: 999, y: 50 })
    );
}

#[test]
fn visibility_controller_failure_never_invents_position_and_preserves_fullscreen() {
    let (actor, host, effects, _) = actor();
    let facts = VisibilityObservations {
        dock: BarFacts {
            confirmed_fullscreen: true,
            ..BarFacts::default()
        },
        toolbar: BarFacts {
            confirmed_fullscreen: true,
            ..BarFacts::default()
        },
    };
    actor.update(VisibilityConfig::default(), Some(geometry(3)), facts);
    assert_eq!(
        actor.current(),
        BarVisibility {
            dock: false,
            toolbar: false
        }
    );
    actor.start().unwrap();
    host.ready(Ok(()));
    host.event(PointerWatchEvent::Position(PhysicalPoint {
        x: -200,
        y: 299,
    }));
    host.event(PointerWatchEvent::Failed(PointerWatchError::Unavailable));
    actor.process_events();
    assert_eq!(
        actor.readiness(),
        PointerReadiness::Failed(PointerWatchError::Unavailable)
    );
    assert!(actor.state.borrow().pointer.is_none());
    assert_eq!(host.retired.load(Ordering::SeqCst), 1);
    assert_eq!(
        effects.borrow().as_slice(),
        &[(
            BarVisibility {
                dock: false,
                toolbar: false
            },
            3
        )]
    );
    actor.update(
        VisibilityConfig::default(),
        Some(geometry(3)),
        VisibilityObservations::default(),
    );
    assert_eq!(actor.current(), BarVisibility::default());
    assert_eq!(
        effects.borrow().last(),
        Some(&(BarVisibility::default(), 3))
    );
}

#[test]
fn visibility_controller_geometry_revision_discards_queued_pointer_and_old_surfaces() {
    let (actor, host, effects, _) = actor();
    actor.update(
        VisibilityConfig::default(),
        Some(geometry(5)),
        VisibilityObservations::default(),
    );
    actor.start().unwrap();
    host.ready(Ok(()));
    host.event(PointerWatchEvent::Position(PhysicalPoint {
        x: -200,
        y: 299,
    }));
    actor.update(
        VisibilityConfig::default(),
        Some(geometry(6)),
        VisibilityObservations::default(),
    );
    actor.process_events();
    assert!(actor.state.borrow().pointer.is_none());
    assert_eq!(effects.borrow().last().unwrap().1, 6);
    actor.update(
        VisibilityConfig::default(),
        Some(geometry(5)),
        VisibilityObservations {
            dock: BarFacts {
                confirmed_fullscreen: true,
                ..BarFacts::default()
            },
            ..VisibilityObservations::default()
        },
    );
    assert_eq!(actor.current(), BarVisibility::default());
    actor.update(
        VisibilityConfig::default(),
        None,
        VisibilityObservations::default(),
    );
    let effects_before = effects.borrow().len();
    actor.update(
        VisibilityConfig::default(),
        Some(geometry(6)),
        VisibilityObservations::default(),
    );
    assert!(actor.state.borrow().geometry.is_none()); // retired revision cannot resurrect
    assert_eq!(effects.borrow().len(), effects_before);
    actor.update(
        VisibilityConfig::default(),
        Some(geometry(7)),
        VisibilityObservations::default(),
    );
    assert_eq!(effects.borrow().last().unwrap().1, 7);
}

#[test]
fn visibility_controller_same_revision_geometry_mutation_and_stale_timer_are_denied() {
    let (actor, _, effects, _) = actor();
    actor.update(
        VisibilityConfig::default(),
        Some(geometry(10)),
        VisibilityObservations::default(),
    );
    let changed = VisibilityGeometry {
        dock_edge: Edge::Left,
        ..geometry(10)
    };
    actor.update(
        VisibilityConfig::default(),
        Some(changed),
        VisibilityObservations::default(),
    );
    assert_eq!(actor.state.borrow().geometry, Some(geometry(10)));
    actor.timer_expired(
        0,
        DeadlineToken {
            revision: 9,
            sequence: 1,
        },
    );
    assert_eq!(effects.borrow().len(), 1);
}

#[test]
fn visibility_controller_close_rejects_late_ready_pointer_and_failure_without_join() {
    let (actor, host, effects, wakes) = actor();
    actor.update(
        VisibilityConfig::default(),
        Some(geometry(11)),
        VisibilityObservations::default(),
    );
    actor.start().unwrap();
    actor.close();
    actor.close();
    host.ready(Ok(()));
    host.event(PointerWatchEvent::Position(PhysicalPoint { x: 1, y: 2 }));
    host.event(PointerWatchEvent::Failed(PointerWatchError::Unavailable));
    actor.process_events();
    assert_eq!(actor.readiness(), PointerReadiness::Closed);
    assert_eq!(host.retired.load(Ordering::SeqCst), 1);
    assert_eq!(wakes.load(Ordering::SeqCst), 0);
    assert_eq!(effects.borrow().len(), 1);
    assert_eq!(actor.start(), Err(PointerWatchError::Stopped));
}

#[test]
fn visibility_controller_immediate_host_failure_uses_gui_mailbox_without_direct_fallback() {
    let (actor, host, effects, _) = actor();
    *host.immediate.lock() = Some(PointerWatchError::Busy);
    actor.update(
        VisibilityConfig::default(),
        Some(geometry(12)),
        VisibilityObservations::default(),
    );
    assert_eq!(actor.start(), Err(PointerWatchError::Busy));
    assert_eq!(actor.readiness(), PointerReadiness::Starting);
    actor.process_events();
    assert_eq!(
        actor.readiness(),
        PointerReadiness::Failed(PointerWatchError::Busy)
    );
    assert!(host.events.lock().is_none());
    assert!(host.completion.lock().is_none());
    assert_eq!(effects.borrow().len(), 1);
}

#[test]
fn visibility_mailbox_first_terminal_wins_and_dispatch_failure_never_executes_effect() {
    let mailbox = Arc::new(Mutex::new(mailbox::Mailbox::default()));
    mailbox.lock().activate(1);
    mailbox::ready(&mailbox, 1, Ok(())); // no weak route, intentionally inert
    assert!(mailbox.lock().ready.is_none());
    let attempts = Arc::new(AtomicUsize::new(0));
    let counted = attempts.clone();
    mailbox.lock().wake = Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        Err(())
    }));
    mailbox::ready(&mailbox, 1, Err(PointerWatchError::Stopped));
    mailbox::ready(&mailbox, 1, Ok(()));
    mailbox::event(
        &mailbox,
        1,
        PointerWatchEvent::Failed(PointerWatchError::Unavailable),
    );
    mailbox::event(
        &mailbox,
        1,
        PointerWatchEvent::Position(PhysicalPoint { x: 0, y: 0 }),
    );
    assert_eq!(mailbox.lock().ready, Some(Err(PointerWatchError::Stopped)));
    assert_eq!(mailbox.lock().failure, Some(PointerWatchError::Unavailable));
    assert!(mailbox.lock().position.is_none());
    assert!(!mailbox.lock().queued);
    assert!(attempts.load(Ordering::SeqCst) > 0);
    mailbox.lock().activate(2);
    mailbox::event(
        &mailbox,
        1,
        PointerWatchEvent::Position(PhysicalPoint { x: 99, y: 99 }),
    );
    assert!(mailbox.lock().position.is_none());
}

#[test]
fn visibility_controller_effect_can_reentrantly_retire_without_outstanding_borrows() {
    let host = Arc::new(MockHost::default());
    let actor_slot: Rc<RefCell<Option<std::rc::Weak<VisibilityController>>>> =
        Rc::new(RefCell::new(None));
    let effect_slot = actor_slot.clone();
    let actor = VisibilityController::new(host, move |_, _| {
        if let Some(actor) = effect_slot
            .borrow()
            .as_ref()
            .and_then(std::rc::Weak::upgrade)
        {
            actor.close();
        }
    });
    *actor_slot.borrow_mut() = Some(Rc::downgrade(&actor));
    actor.update(
        VisibilityConfig::default(),
        Some(geometry(13)),
        VisibilityObservations::default(),
    );
    assert_eq!(actor.readiness(), PointerReadiness::Closed);
}
