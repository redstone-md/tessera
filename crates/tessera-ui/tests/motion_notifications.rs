// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

#[path = "../src/motion.rs"]
mod motion;

use motion::MotionSubscription;
use parking_lot::Mutex;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tessera_ui::{DesktopHost, PanelPreferences, PanelSnapshot, SystemAction};

slint::slint! {
    export component MotionTarget inherits Window {
        callback motion-policy-changed(bool);
    }
}

type Callback = Arc<dyn Fn(bool) + Send + Sync>;

#[derive(Default)]
struct Host {
    callback: Mutex<Option<Callback>>,
    deny: bool,
    dropped: Arc<AtomicUsize>,
}

struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl Host {
    fn emit_on_worker(&self, enabled: bool) {
        let callback = self.callback.lock().clone().unwrap();
        std::thread::spawn(move || callback(enabled))
            .join()
            .unwrap();
    }
}

impl DesktopHost for Host {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("Motion must not observe the desktop")
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("Motion must not activate windows")
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("Motion must not launch applications")
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("Motion must not dispatch system actions")
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("Motion must not save preferences")
    }
    fn subscribe_ui_motion(&self, callback: Callback) -> Result<Option<Box<dyn Send>>, String> {
        *self.callback.lock() = Some(callback);
        if self.deny {
            self.emit_on_worker(false);
            return Err("Motion subscription denied".into());
        }
        Ok(Some(Box::new(Guard(Arc::clone(&self.dropped)))))
    }
}

#[test]
fn worker_delivery_is_motion_only_and_queued_events_stop_on_drop_or_failed_registration() {
    i_slint_backend_testing::init_integration_test_with_mock_time();
    let panel = MotionTarget::new().unwrap();
    panel.show().unwrap();
    let host = Arc::new(Host::default());
    let scope = Rc::new(RefCell::new(None::<MotionSubscription>));
    let deliveries = Arc::new(Mutex::new(Vec::new()));
    panel.on_motion_policy_changed({
        let host = Arc::clone(&host);
        let scope = Rc::clone(&scope);
        let deliveries = Arc::clone(&deliveries);
        move |enabled| {
            deliveries.lock().push(enabled);
            // This event is queued, then its scope is dropped before delivery.
            host.emit_on_worker(false);
            scope.borrow_mut().take();
            slint::invoke_from_event_loop(|| slint::quit_event_loop().unwrap()).unwrap();
        }
    });

    let denied = Host {
        deny: true,
        ..Host::default()
    };
    assert!(
        MotionSubscription::new(&denied, &panel, MotionTarget::invoke_motion_policy_changed)
            .is_err()
    );
    slint::invoke_from_event_loop({
        let deliveries = Arc::clone(&deliveries);
        move || {
            assert!(
                deliveries.lock().is_empty(),
                "Failed registration must not deliver"
            )
        }
    })
    .unwrap();
    *scope.borrow_mut() = Some(
        MotionSubscription::new(
            host.as_ref(),
            &panel,
            MotionTarget::invoke_motion_policy_changed,
        )
        .unwrap(),
    );
    host.emit_on_worker(true); // A new allowance must not replay a current transition.
    host.emit_on_worker(false);
    slint::run_event_loop().unwrap();

    assert_eq!(&*deliveries.lock(), &[false]);
    assert!(scope.borrow().is_none());
    assert_eq!(host.dropped.load(Ordering::Relaxed), 1);
    // Retained native callbacks no longer even enqueue work after shutdown.
    host.emit_on_worker(false);
    denied.emit_on_worker(false);
}
