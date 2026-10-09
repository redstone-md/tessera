// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::cell::Cell;

use super::*;

struct Fixture {
    root: UserMenu,
    actor: Rc<UserLogoutController>,
    admitted: Rc<Cell<bool>>,
    received: Rc<RefCell<Vec<UserLogoutIntent>>>,
}

impl Fixture {
    fn new(bound: bool) -> Self {
        i_slint_backend_testing::init_no_event_loop();
        let root = UserMenu::new().unwrap();
        let actor = UserLogoutController::new(&root);
        let admitted = Rc::new(Cell::new(true));
        let received = Rc::new(RefCell::new(Vec::new()));
        if bound {
            let current = admitted.clone();
            let sink = received.clone();
            assert!(actor.bind_root(
                move || current.get(),
                move |intent| {
                    sink.borrow_mut().push(intent);
                }
            ));
        }
        Self {
            root,
            actor,
            admitted,
            received,
        }
    }

    fn show(&self) -> UserLogoutSession {
        let session = self.actor.prepare().unwrap();
        self.root.show().unwrap();
        self.actor.activate(session);
        session
    }

    fn input(&self) -> UserLogoutIntent {
        self.root
            .invoke_logout_requested(self.root.get_logout_key());
        self.received
            .borrow_mut()
            .pop()
            .expect("genuine current input receipt")
    }
}

#[test]
fn standalone_and_closed_admission_never_issue_or_project_fake_logout() {
    let fixture = Fixture::new(false);
    let session = fixture.show();
    assert!(fixture.actor.session_current(session));
    assert!(!fixture.root.get_logout_ready());
    assert!(fixture.root.get_logout_key().is_empty());
    fixture.root.set_logout_ready(true);
    fixture.root.set_logout_key("forged label or key".into());
    fixture
        .root
        .invoke_logout_requested("forged label or key".into());
    assert!(fixture.received.borrow().is_empty());
}

#[test]
fn bind_once_before_activate_missing_native_or_source_admission_fails_closed() {
    let fixture = Fixture::new(true);
    assert!(
        !fixture
            .actor
            .bind_root(|| true, |_| panic!("replacement binding"))
    );
    let session = fixture.actor.prepare().unwrap();
    assert!(fixture.actor.session_current(session));
    assert!(
        !fixture.actor.activate(session),
        "native surface not presented"
    );
    fixture.root.show().unwrap();
    fixture.admitted.set(false);
    assert!(!fixture.actor.activate(session));
    assert!(fixture.root.get_logout_key().is_empty());
    fixture.admitted.set(true);
    assert!(fixture.actor.activate(session));
    let intent = fixture.input();
    assert!(fixture.actor.is_current(&intent));
    fixture.admitted.set(false);
    assert!(!fixture.actor.is_current(&intent));
    fixture
        .root
        .invoke_logout_requested(fixture.root.get_logout_key());
    assert!(fixture.received.borrow().is_empty());
}

#[test]
fn issued_receipt_requires_same_session_and_exact_native_retirement_not_all_hidden() {
    let fixture = Fixture::new(true);
    let session = fixture.show();
    let stale_key = fixture.root.get_logout_key();
    let intent = fixture.input();
    assert!(fixture.actor.is_current(&intent));
    fixture.root.hide().unwrap();
    assert!(
        !fixture.actor.is_current(&intent),
        "native visibility mismatch rejects current input"
    );
    assert!(
        !fixture.actor.is_exact_retired(&intent),
        "all hidden without exact actor retirement is insufficient"
    );
    fixture.root.show().unwrap();
    assert_eq!(fixture.actor.retire(), Some(session));
    assert!(
        fixture.actor.session_retired(session),
        "pure lifecycle guard precedes native hide"
    );
    assert!(!fixture.actor.is_current(&intent));
    assert!(
        !fixture.actor.is_exact_retired(&intent),
        "visible native surface rejects authority"
    );
    fixture.root.hide().unwrap();
    assert!(fixture.actor.is_exact_retired(&intent));
    assert_eq!(fixture.actor.retire(), Some(session));
    assert!(
        fixture.actor.is_exact_retired(&intent),
        "repeat hide preserves exact receipt"
    );
    let replacement = fixture.actor.prepare().unwrap();
    assert_ne!(session, replacement);
    assert!(!fixture.actor.session_retired(session));
    assert!(
        !fixture.actor.is_exact_retired(&intent),
        "hidden replacement invalidates old receipt"
    );
    fixture.root.show().unwrap();
    assert!(fixture.actor.activate(replacement));
    fixture.root.invoke_logout_requested(stale_key);
    assert!(fixture.received.borrow().is_empty());
    assert!(!fixture.actor.is_current(&intent));
    let current = fixture.input();
    assert!(fixture.actor.is_current(&current));
    assert!(!fixture.actor.is_current(&intent));
}

#[test]
fn shared_busy_blocks_fresh_input_but_preserves_reserved_typed_receipt() {
    let fixture = Fixture::new(true);
    fixture.show();
    let intent = fixture.input();
    let original_key = fixture.root.get_logout_key();
    fixture.actor.set_busy(true);
    assert!(fixture.root.get_logout_busy());
    assert!(!fixture.root.get_logout_ready());
    assert!(fixture.actor.is_current(&intent));
    fixture
        .root
        .invoke_logout_requested(fixture.root.get_logout_key());
    assert!(fixture.received.borrow().is_empty());
    fixture.actor.set_busy(false);
    assert!(fixture.root.get_logout_ready());
    assert_ne!(fixture.root.get_logout_key(), original_key);
    fixture.root.invoke_logout_requested(original_key);
    assert!(fixture.received.borrow().is_empty());
    let next = fixture.input();
    assert!(fixture.actor.is_current(&next));
    assert!(
        !fixture.actor.is_current(&intent),
        "new genuine issuance supersedes old receipt"
    );
}

#[test]
fn actor_and_component_are_weakly_bound_and_cross_actor_intents_fail() {
    let fixture = Fixture::new(true);
    fixture.show();
    let intent = fixture.input();
    let other_root = UserMenu::new().unwrap();
    let other = UserLogoutController::new(&other_root);
    other.bind_root(|| true, |_| {});
    let session = other.prepare().unwrap();
    other_root.show().unwrap();
    other.activate(session);
    other_root.invoke_logout_requested(other_root.get_logout_key());
    assert!(!other.is_current(&intent));
    other.retire();
    other_root.hide().unwrap();
    assert!(!other.is_exact_retired(&intent));
    let weak_actor = Rc::downgrade(&fixture.actor);
    let Fixture { root, actor, .. } = fixture;
    drop(actor);
    assert!(
        weak_actor.upgrade().is_none(),
        "generated callback must not retain actor"
    );
    root.invoke_logout_requested(root.get_logout_key());
    let weak_root = other_root.as_weak();
    drop(other_root);
    assert!(
        weak_root.upgrade().is_none(),
        "actor must not retain component/window"
    );
}

#[test]
fn predicate_reentry_replacing_session_rejects_outer_activation_and_input_without_borrows() {
    let fixture = Fixture::new(false);
    let replace = Rc::new(Cell::new(false));
    let replacement = Rc::new(Cell::new(None));
    let weak = Rc::downgrade(&fixture.actor);
    let flag = replace.clone();
    let captured = replacement.clone();
    let sink = fixture.received.clone();
    fixture.actor.bind_root(
        move || {
            if flag.replace(false) {
                captured.set(weak.upgrade().unwrap().prepare());
            }
            true
        },
        move |intent| sink.borrow_mut().push(intent),
    );
    let original = fixture.actor.prepare().unwrap();
    fixture.root.show().unwrap();
    replace.set(true);
    assert!(!fixture.actor.activate(original));
    let current = replacement.get().unwrap();
    assert!(!fixture.actor.session_current(original));
    assert!(fixture.actor.session_current(current));
    assert!(fixture.actor.activate(current));
    replace.set(true);
    fixture
        .root
        .invoke_logout_requested(fixture.root.get_logout_key());
    assert!(fixture.received.borrow().is_empty());
    assert!(!fixture.root.get_logout_ready());
}

#[test]
fn each_checked_counter_exhausts_closed_without_wrap_or_receipt_replay() {
    let fixture = Fixture::new(true);
    fixture.actor.state.borrow_mut().sessions = u64::MAX;
    assert!(fixture.actor.prepare().is_none());
    assert!(fixture.root.get_logout_key().is_empty());
    assert!(!fixture.root.get_logout_ready());
    // New actors, same pure testing backend; no native executor exists here.
    for issuance_counter in [false, true] {
        let root = UserMenu::new().unwrap();
        let actor = UserLogoutController::new(&root);
        actor.bind_root(|| true, |_| panic!("exhausted actor cannot issue"));
        let session = actor.prepare().unwrap();
        root.show().unwrap();
        if issuance_counter {
            assert!(actor.activate(session));
            actor.state.borrow_mut().issuances = u64::MAX;
            root.invoke_logout_requested(root.get_logout_key());
        } else {
            actor.state.borrow_mut().keys = u64::MAX;
            assert!(!actor.activate(session));
        }
        assert!(!root.get_logout_ready());
        assert!(root.get_logout_key().is_empty());
        assert!(!actor.session_current(session));
        assert!(actor.prepare().is_none());
    }
}

#[test]
fn typed_sink_can_reenter_retirement_and_replacement_without_callback_borrow_or_outer_projection() {
    let fixture = Fixture::new(false);
    let weak = Rc::downgrade(&fixture.actor);
    let replacement = Rc::new(Cell::new(None));
    let captured = replacement.clone();
    let sink = fixture.received.clone();
    fixture.actor.bind_root(
        || true,
        move |intent| {
            let actor = weak.upgrade().unwrap();
            assert!(actor.is_current(&intent));
            actor.retire();
            captured.set(actor.prepare());
            sink.borrow_mut().push(intent);
        },
    );
    let original = fixture.show();
    let intent = fixture.input();
    assert!(!fixture.actor.is_current(&intent));
    assert!(!fixture.actor.is_exact_retired(&intent));
    assert!(!fixture.actor.session_current(original));
    assert!(fixture.actor.session_current(replacement.get().unwrap()));
    assert!(
        fixture.root.get_logout_key().is_empty(),
        "outer request never overwrites replacement projection"
    );
    assert!(!fixture.root.get_logout_ready());
}

#[test]
fn exhausted_input_keeps_exact_cleanup_identity_but_never_authorizes_an_intent() {
    i_slint_backend_testing::init_no_event_loop();
    for counter in ["issuance", "keys", "sessions"] {
        let root = UserMenu::new().unwrap();
        let actor = UserLogoutController::new(&root);
        let received = Rc::new(RefCell::new(Vec::new()));
        let sink = received.clone();
        assert!(actor.bind_root(|| true, move |intent| sink.borrow_mut().push(intent)));
        let session = actor.prepare().unwrap();
        root.show().unwrap();
        assert!(actor.activate(session));
        root.invoke_logout_requested(root.get_logout_key());
        let intent = received.borrow_mut().pop().unwrap();
        actor.exhaust_counter_for_test(counter);
        match counter {
            "issuance" => root.invoke_logout_requested(root.get_logout_key()),
            "keys" => actor.set_busy(true),
            "sessions" => assert!(actor.prepare().is_none()),
            _ => unreachable!(),
        }
        assert!(
            received.borrow().is_empty(),
            "exhaustion never reaches the typed sink"
        );
        assert!(!root.get_logout_ready());
        assert!(root.get_logout_key().is_empty());
        assert!(!actor.is_current(&intent));
        assert_eq!(actor.retire(), Some(session));
        assert!(
            actor.session_retired(session),
            "exhausted {counter} input must not revoke the same session's cleanup identity"
        );
        root.hide().unwrap();
        assert!(!root.window().is_visible());
        assert!(
            !actor.is_exact_retired(&intent),
            "cleanup permission is not trusted command permission after exhaustion"
        );
        assert!(
            actor.prepare().is_none(),
            "numeric input authority remains closed"
        );
    }
}
