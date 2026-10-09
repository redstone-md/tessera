// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

#[test]
fn dock_media_reentrant_factory_retirement_cannot_adopt_the_old_scope() {
    let fixture = Fixture::new();
    let weak = Rc::downgrade(&fixture.controller);
    FACTORY_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            let controller = weak.upgrade().unwrap();
            controller.set_enabled(false);
            controller.set_enabled(true);
        }))
    });
    fixture.controller.set_enabled(true);
    assert_eq!(fixture.host.acquisitions.load(Ordering::Relaxed), 2);
    assert_eq!(fixture.media.reads(), 1);
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(session(MediaPlayback::Paused)),
    }));
    fixture.drain();
    assert_eq!(fixture.view().playback, "Paused");
    assert_eq!(
        fixture.media.changed.lock().len(),
        1,
        "old factory result never registers a watch"
    );
}

#[test]
fn dock_media_close_retires_accepted_transport_without_cancel_join_or_readback() {
    let fixture = Fixture::new();
    fixture.enable(session(MediaPlayback::Paused));
    fixture.controller.request(MediaAction::Toggle);
    fixture.controller.close();
    fixture.media.finish_command(Ok(()));
    fixture.media.event(MediaEvent::Changed);
    fixture.drain();
    fixture.controller.retry();
    fixture.controller.request(MediaAction::Toggle);
    assert_eq!(fixture.media.commands.lock().len(), 1);
    assert_eq!(fixture.media.reads(), 1);
    assert_eq!(fixture.media.watch_drops.load(Ordering::Relaxed), 1);
    assert!(!fixture.view().enabled);
}

#[test]
fn dock_media_gesture_identity_uses_session_incarnation_and_enabled_generation_not_source() {
    let fixture = Fixture::new();
    let initial = session(MediaPlayback::Paused);
    fixture.enable(initial.clone());
    let first = fixture.view().session_identity;
    fixture.controller.retry();
    let replacement = session(MediaPlayback::Paused);
    assert_eq!(replacement.source_app_id, initial.source_app_id);
    assert_ne!(replacement.key, initial.key);
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(replacement.clone()),
    }));
    fixture.drain();
    let second = fixture.view().session_identity;
    assert_ne!(
        first, second,
        "same source cannot preserve a replaced session's gesture"
    );
    fixture.controller.set_enabled(false);
    assert!(fixture.view().session_identity.is_empty());
    fixture.controller.set_enabled(true);
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(replacement),
    }));
    fixture.drain();
    assert_ne!(
        second,
        fixture.view().session_identity,
        "new enabled scope retires old gestures even for the same native session"
    );
    fixture.controller.retry();
    fixture
        .media
        .finish_read(Ok(MediaSnapshot { current: None }));
    fixture.drain();
    assert!(fixture.view().session_identity.is_empty());
    assert!(fixture.media.commands.lock().is_empty());
}

#[test]
fn popup_pending_attach_reentry_cannot_detach_newer_same_window_presentation() {
    let fixture = Fixture::new();
    let popup = QuickSettings::new().unwrap();
    popup.show().unwrap();
    let weak = Rc::downgrade(&fixture.controller);
    let view = popup.as_weak();
    let newer = Rc::new(Cell::new(None));
    let recorded = Rc::clone(&newer);
    FACTORY_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            let controller = weak.upgrade().unwrap();
            let old = controller.state.borrow().popup.as_ref().unwrap().token;
            controller.detach_popup(old);
            let popup = view.upgrade().unwrap();
            recorded.set(controller.attach_popup(&popup));
        }));
    });
    let old = fixture.controller.attach_popup(&popup).unwrap();
    let new = newer.get().unwrap();
    assert_ne!(old, new);
    fixture.controller.detach_popup(old);
    assert_eq!(
        fixture
            .controller
            .state
            .borrow()
            .popup
            .as_ref()
            .unwrap()
            .token,
        new
    );
    assert_eq!(fixture.host.acquisitions.load(Ordering::Relaxed), 2);
    assert_eq!(
        fixture.media.reads(),
        1,
        "retired unsubmitted factory result never reads"
    );
    assert_eq!(fixture.media.changed.lock().len(), 1);
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(session(MediaPlayback::Paused)),
    }));
    fixture.drain();
    assert_eq!(popup.get_media_view().playback, "Paused");
    fixture.controller.detach_popup(new);
    popup.hide().unwrap();
}
