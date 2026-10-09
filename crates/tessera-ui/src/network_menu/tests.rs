// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::Panel;
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use slint::{LogicalPosition, Model};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tessera_system::network::{
    InterfaceId, NetworkActionCompletion, NetworkCompletion, Observation, RadioState, Ssid,
    WifiBss, WifiInterface,
};

// All WLAN observations are recording fixtures. These tests perform no scans,
// settings dispatches, radio/connection changes or installed-OS discovery.
type Hook = RefCell<Option<Box<dyn FnOnce()>>>;
thread_local! {
    static READ_HOOK: Hook = RefCell::default();
    static WATCH_HOOK: Hook = RefCell::default();
    static DROP_HOOK: Hook = RefCell::default();
    static FACTORY_HOOK: Hook = RefCell::default();
}
fn run_hook(hook: &'static std::thread::LocalKey<Hook>) {
    if let Some(callback) = hook.with(|slot| slot.borrow_mut().take()) {
        callback();
    }
}

enum Reply {
    Pending,
    Inline(Result<NetworkSnapshot, NetworkError>),
    Reject(NetworkError),
}
enum ActionReply {
    Pending,
    Inline(Result<(), NetworkError>),
    Reject(NetworkError),
}
type Watch = Arc<dyn Fn(NetworkEvent) + Send + Sync>;
#[derive(Default)]
struct RecordingNetwork {
    reads: AtomicUsize,
    settings: AtomicUsize,
    subscriptions: AtomicUsize,
    retirements: Arc<AtomicUsize>,
    pending: Mutex<Option<NetworkCompletion>>,
    pending_settings: Mutex<Option<NetworkActionCompletion>>,
    replies: Mutex<VecDeque<Reply>>,
    action_replies: Mutex<VecDeque<ActionReply>>,
    watches: Mutex<Vec<Watch>>,
    watch_error: Mutex<Option<NetworkError>>,
    no_watch: AtomicBool,
}
struct WatchGuard {
    retirements: Arc<AtomicUsize>,
    callback: Watch,
}
impl Drop for WatchGuard {
    fn drop(&mut self) {
        self.retirements.fetch_add(1, Ordering::Relaxed);
        (self.callback)(NetworkEvent::Changed);
        run_hook(&DROP_HOOK);
    }
}
impl RecordingNetwork {
    fn finish(&self, result: Result<NetworkSnapshot, NetworkError>) {
        self.pending.lock().take().expect("accepted read")(result);
    }
    fn finish_settings(&self, result: Result<(), NetworkError>) {
        self.pending_settings
            .lock()
            .take()
            .expect("accepted settings")(result);
    }
    fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }
    fn notify(&self, event: NetworkEvent) {
        let callback = self.watches.lock().last().cloned().expect("watch startup");
        callback(event);
    }
}
impl NetworkHost for RecordingNetwork {
    fn read(&self, completion: NetworkCompletion) -> Result<(), NetworkError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        assert!(
            self.pending.lock().is_none(),
            "one accepted read across sessions"
        );
        let reply = self.replies.lock().pop_front().unwrap_or(Reply::Pending);
        match reply {
            Reply::Pending => *self.pending.lock() = Some(completion),
            Reply::Inline(result) => completion(result),
            Reply::Reject(error) => return Err(error),
        }
        run_hook(&READ_HOOK);
        Ok(())
    }
    fn open_settings(&self, completion: NetworkActionCompletion) -> Result<(), NetworkError> {
        self.settings.fetch_add(1, Ordering::Relaxed);
        assert!(self.pending_settings.lock().is_none());
        let reply = self
            .action_replies
            .lock()
            .pop_front()
            .unwrap_or(ActionReply::Pending);
        match reply {
            ActionReply::Pending => *self.pending_settings.lock() = Some(completion),
            ActionReply::Inline(result) => completion(result),
            ActionReply::Reject(error) => return Err(error),
        }
        Ok(())
    }
    fn subscribe(&self, callback: Watch) -> Result<Option<Box<dyn Send>>, NetworkError> {
        self.subscriptions.fetch_add(1, Ordering::Relaxed);
        if let Some(error) = self.watch_error.lock().clone() {
            return Err(error);
        }
        if self.no_watch.load(Ordering::Relaxed) {
            return Ok(None);
        }
        self.watches.lock().push(callback.clone());
        callback(NetworkEvent::WatchReady);
        run_hook(&WATCH_HOOK);
        Ok(Some(Box::new(WatchGuard {
            retirements: self.retirements.clone(),
            callback,
        })))
    }
}

struct Desktop {
    network: Arc<RecordingNetwork>,
    factory_calls: AtomicUsize,
    factory_error: Mutex<Option<NetworkError>>,
    events: Arc<Mutex<Vec<&'static str>>>,
    cancel_attachment: AtomicBool,
    deny_focus: AtomicBool,
}
struct Lease(Arc<Mutex<Vec<&'static str>>>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.lock().push("detach");
    }
}
impl DesktopHost for Desktop {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("no desktop catalog reads")
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("no activation")
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("no arbitrary launch")
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("no source mutation")
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("no preference writes")
    }
    fn network_host(&self) -> Result<Option<Arc<dyn NetworkHost>>, NetworkError> {
        self.factory_calls.fetch_add(1, Ordering::Relaxed);
        run_hook(&FACTORY_HOOK);
        if let Some(error) = self.factory_error.lock().clone() {
            Err(error)
        } else {
            Ok(Some(self.network.clone()))
        }
    }
    fn configure_surface(
        &self,
        kind: SurfaceKind,
        window: &slint::Window,
    ) -> Result<Option<Box<dyn std::any::Any>>, String> {
        assert_eq!(kind, SurfaceKind::Popup);
        assert!(window.is_visible());
        self.events.lock().push("attach");
        if self.cancel_attachment.load(Ordering::Relaxed) {
            window.dispatch_event(slint::platform::WindowEvent::CloseRequested);
        }
        Ok(Some(Box::new(Lease(self.events.clone()))))
    }
    fn request_ui_focus(&self, window: &slint::Window) -> Result<(), String> {
        assert!(window.is_visible());
        self.events.lock().push("focus");
        if self.deny_focus.load(Ordering::Relaxed) {
            Err("focus denied".into())
        } else {
            Ok(())
        }
    }
}
struct Fixture {
    host: Arc<Desktop>,
    network: Arc<RecordingNetwork>,
    source: Panel,
    popup: Rc<NetworkController>,
}
impl Fixture {
    fn new() -> Self {
        i_slint_backend_testing::init_no_event_loop();
        let network = Arc::new(RecordingNetwork::default());
        let host = Arc::new(Desktop {
            network: network.clone(),
            factory_calls: AtomicUsize::new(0),
            factory_error: Mutex::default(),
            events: Arc::new(Mutex::default()),
            cancel_attachment: AtomicBool::new(false),
            deny_focus: AtomicBool::new(false),
        });
        let source = Panel::new().unwrap();
        source
            .window()
            .set_position(PhysicalPosition::new(-1400, 100));
        source.window().set_size(PhysicalSize::new(600, 600));
        source.show().unwrap();
        let popup = NetworkController::new(host.clone()).unwrap();
        Self {
            host,
            network,
            source,
            popup,
        }
    }
    fn show(&self) {
        self.popup
            .show(self.source.window(), bounds(), context())
            .unwrap();
    }
    fn drain(&self) {
        self.popup.component().invoke_network_event_ready();
    }
    fn loaded(&self) {
        self.show();
        self.network.finish(Ok(snapshot()));
        self.drain();
    }
    fn refresh(&self) {
        let key = self.popup.component().get_refresh_key();
        self.popup.component().invoke_refresh_requested(key);
    }
    fn settings(&self) {
        let key = self.popup.component().get_settings_key();
        self.popup.component().invoke_settings_requested(key);
    }
}
fn bounds() -> TileBounds {
    TileBounds {
        origin: LogicalPosition::new(20.0, 500.0),
        width: 150.0,
        height: 40.0,
    }
}
fn context() -> DockContext {
    DockContext::new(-1920, 0, 1920, 1080, false).unwrap()
}
fn error(kind: NetworkErrorKind) -> NetworkError {
    NetworkError::new(kind, "private-network must not leak into UI")
}
fn bss(ssid: &[u8], ap: u8, signal: u8, known: Option<bool>, connected: Option<bool>) -> WifiBss {
    WifiBss {
        ssid: Ssid::new(ssid).unwrap(),
        bssid: [ap; 6],
        frequency_khz: 5_180_000,
        signal_percent: signal,
        rssi_dbm: -48,
        known,
        secured: None,
        connected,
        auth: None,
    }
}
fn interface(id: u8, entries: Vec<WifiBss>) -> WifiInterface {
    WifiInterface {
        id: InterfaceId::new([id; 16]),
        name: format!("Adapter {id}"),
        radio: Observation::Ready(RadioState::Enabled),
        discovery: Observation::Ready(entries),
    }
}
fn snapshot() -> NetworkSnapshot {
    NetworkSnapshot {
        interfaces: vec![interface(
            1,
            vec![
                bss(b"Connected", 1, 40, Some(true), Some(true)),
                bss(b"Connected", 2, 90, Some(true), Some(false)),
                bss(b"Saved", 3, 60, Some(true), Some(false)),
                bss(b"Available", 4, 70, Some(false), Some(false)),
                bss(&[], 5, 30, None, Some(false)),
            ],
        )],
    }
}

#[test]
fn network_constructor_and_toolbar_render_do_not_discover_until_native_user_open() {
    let f = Fixture::new();
    assert_eq!(f.network.reads(), 0);
    assert_eq!(f.host.factory_calls.load(Ordering::Relaxed), 0);
    assert_eq!(f.network.subscriptions.load(Ordering::Relaxed), 0);
    f.show();
    assert_eq!(*f.host.events.lock(), vec!["attach", "focus"]);
    assert_eq!(f.network.reads(), 1);
    assert!(f.popup.component().get_loading());
    f.popup.hide();
    assert_eq!(*f.host.events.lock(), vec!["attach", "focus", "detach"]);
    assert_eq!(f.network.retirements.load(Ordering::Relaxed), 1);
}

#[test]
fn network_inline_completion_only_projects_from_mailbox_and_uses_real_groups() {
    let f = Fixture::new();
    f.network
        .replies
        .lock()
        .push_back(Reply::Inline(Ok(snapshot())));
    f.show();
    assert!(f.popup.component().get_loading());
    assert_eq!(f.popup.component().get_connected().row_count(), 0);
    f.drain();
    let root = f.popup.component();
    assert_eq!(root.get_connected().row_count(), 1);
    assert_eq!(root.get_saved().row_count(), 1);
    assert_eq!(root.get_available().row_count(), 1);
    assert_eq!(root.get_hidden().row_count(), 1);
    assert!(
        root.get_connected()
            .row_data(0)
            .unwrap()
            .details
            .contains("Current AP 01:01:01:01:01:01")
    );
    assert!(
        !root
            .get_connected()
            .row_data(0)
            .unwrap()
            .details
            .contains("Current AP 02:02:02:02:02:02")
    );
    assert!(!root.get_loading());
    assert!(!root.get_settings_key().is_empty());
}

#[test]
fn network_read_is_single_flight_across_hide_reopen_and_stale_completion_cannot_publish() {
    let f = Fixture::new();
    f.show();
    f.popup.hide();
    f.show();
    assert_eq!(f.network.reads(), 1);
    f.network.finish(Ok(snapshot()));
    f.drain();
    assert_eq!(f.network.reads(), 2);
    assert_eq!(f.popup.component().get_connected().row_count(), 0);
    let mut fresh = snapshot();
    fresh.interfaces[0].discovery = Observation::Ready(vec![bss(b"Fresh", 9, 55, None, None)]);
    f.network.finish(Ok(fresh));
    f.drain();
    assert_eq!(
        f.popup
            .component()
            .get_available()
            .row_data(0)
            .unwrap()
            .label,
        "Fresh"
    );
}

#[test]
fn network_reentrant_accepted_read_and_factory_retire_without_refcell_borrows_or_resurrection() {
    let f = Fixture::new();
    let popup = f.popup.clone();
    READ_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(move || popup.hide())));
    f.show();
    assert!(!f.popup.is_open());
    f.network.finish(Ok(snapshot()));
    f.drain();
    assert!(!f.popup.is_open());
    assert_eq!(f.popup.component().get_connected().row_count(), 0);
    let popup = f.popup.clone();
    f.popup.state.borrow_mut().provider = None;
    FACTORY_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(move || popup.hide())));
    f.show();
    assert!(!f.popup.is_open());
    assert_eq!(f.network.reads(), 1);
}

#[test]
fn network_watch_bursts_keep_one_dirty_slot_and_readiness_failure_survive_changed() {
    let f = Fixture::new();
    f.loaded();
    f.network.notify(NetworkEvent::WatchUnavailable(error(
        NetworkErrorKind::ServiceUnavailable,
    )));
    for _ in 0..10_000 {
        f.network.notify(NetworkEvent::Changed);
    }
    assert!(f.popup.mailbox.lock().changed);
    assert!(matches!(
        f.popup.mailbox.lock().watch,
        Some(NetworkEvent::WatchUnavailable(_))
    ));
    f.drain();
    assert!(
        f.popup
            .component()
            .get_watch_status()
            .contains("unavailable")
    );
    assert_eq!(f.network.reads(), 2);
    for _ in 0..100 {
        f.network.notify(NetworkEvent::Changed);
    }
    f.drain();
    assert_eq!(f.network.reads(), 2);
    f.network.finish(Ok(snapshot()));
    f.drain();
    assert_eq!(f.network.reads(), 3, "one coalesced followup only");
}

#[test]
fn network_partial_privacy_denial_never_retries_notifications_but_manual_refresh_recovers() {
    let f = Fixture::new();
    f.show();
    let mut partial = snapshot();
    let mut denied = interface(2, vec![]);
    denied.discovery = Observation::Unavailable(error(NetworkErrorKind::AccessDenied));
    partial.interfaces.push(denied);
    f.network.finish(Ok(partial));
    f.drain();
    assert!(
        f.popup
            .component()
            .get_summary()
            .contains("Partial cache read")
    );
    assert!(
        f.popup
            .component()
            .get_summary()
            .contains("location permissions")
    );
    assert!(
        !f.popup
            .component()
            .get_summary()
            .contains("private-network")
    );
    assert_eq!(f.popup.component().get_connected().row_count(), 1);
    for _ in 0..100 {
        f.network.notify(NetworkEvent::Changed);
        f.drain();
    }
    assert_eq!(f.network.reads(), 1);
    assert!(!f.popup.component().get_settings_key().is_empty());
    f.refresh();
    assert_eq!(f.network.reads(), 2);
    f.network.finish(Ok(snapshot()));
    f.drain();
    f.network.notify(NetworkEvent::Changed);
    f.drain();
    assert_eq!(f.network.reads(), 3);
}

#[test]
fn network_top_level_denial_and_busy_rejection_show_recovery_without_completion_retry_loops() {
    let f = Fixture::new();
    f.network
        .replies
        .lock()
        .push_back(Reply::Reject(error(NetworkErrorKind::AccessDenied)));
    f.show();
    f.drain();
    assert!(
        f.popup
            .component()
            .get_notice()
            .contains("location permissions")
    );
    f.network.notify(NetworkEvent::Changed);
    f.drain();
    assert_eq!(f.network.reads(), 1);
    f.network
        .replies
        .lock()
        .push_back(Reply::Reject(error(NetworkErrorKind::Busy)));
    let previous = f.popup.component().get_refresh_key();
    f.refresh();
    f.popup.component().invoke_refresh_requested(previous);
    assert_eq!(f.network.reads(), 2);
    f.drain();
    assert!(f.popup.component().get_notice().contains("busy"));
    assert!(!f.popup.component().get_refresh_key().is_empty());
    f.refresh();
    f.network.finish(Ok(NetworkSnapshot { interfaces: vec![] }));
    f.drain();
    assert_eq!(
        f.popup.component().get_radio_text(),
        "No Wi-Fi adapter found"
    );
}

#[test]
fn network_late_watch_ready_and_failure_after_guard_retirement_cannot_overwrite_reopened_session() {
    let f = Fixture::new();
    f.loaded();
    let old = f.network.watches.lock()[0].clone();
    f.popup.hide();
    old(NetworkEvent::WatchUnavailable(error(
        NetworkErrorKind::AccessDenied,
    )));
    assert!(f.popup.mailbox.lock().watch.is_none());
    f.show();
    old(NetworkEvent::WatchReady);
    old(NetworkEvent::Changed);
    f.network.finish(Ok(snapshot()));
    f.drain();
    assert!(f.popup.component().get_watch_status().contains("active"));
    assert_eq!(f.network.reads(), 2);
    assert!(!f.popup.state.borrow().automatic_blocked);
}

#[test]
fn network_reentrant_watch_startup_and_guard_drop_preserve_replacement_and_new_watch() {
    let f = Fixture::new();
    let popup = f.popup.clone();
    let source = f.source.as_weak();
    WATCH_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            popup.hide();
            popup
                .show(source.upgrade().unwrap().window(), bounds(), context())
                .unwrap();
        }))
    });
    f.show();
    assert!(f.popup.is_open());
    assert_eq!(f.network.subscriptions.load(Ordering::Relaxed), 2);
    assert_eq!(f.network.reads(), 1);
    f.network.finish(Ok(snapshot()));
    f.drain();
    let popup = f.popup.clone();
    let source = f.source.as_weak();
    DROP_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            popup
                .show(source.upgrade().unwrap().window(), bounds(), context())
                .unwrap();
        }))
    });
    f.popup.hide();
    assert!(
        f.popup.is_open(),
        "retiring guard cannot hide a reentrant replacement"
    );
    assert_eq!(f.network.subscriptions.load(Ordering::Relaxed), 3);
}

#[test]
fn network_watch_unsupported_and_setup_error_leave_read_refresh_settings_independent() {
    let f = Fixture::new();
    f.network.no_watch.store(true, Ordering::Relaxed);
    f.loaded();
    assert!(
        f.popup
            .component()
            .get_watch_status()
            .contains("unsupported")
    );
    assert!(!f.popup.component().get_refresh_key().is_empty());
    f.popup.hide();
    f.network.no_watch.store(false, Ordering::Relaxed);
    *f.network.watch_error.lock() = Some(error(NetworkErrorKind::ServiceUnavailable));
    f.show();
    f.network.finish(Ok(snapshot()));
    f.drain();
    assert!(
        f.popup
            .component()
            .get_watch_status()
            .contains("unavailable")
    );
    assert!(!f.popup.component().get_settings_key().is_empty());
    f.refresh();
    assert_eq!(f.network.reads(), 3);
}

#[test]
fn network_settings_only_current_opaque_key_has_authority_and_dispatch_is_single_flight() {
    let f = Fixture::new();
    f.loaded();
    for fake in ["ms-settings:network", "Connected", "network-0-0", ""] {
        f.popup.component().invoke_settings_requested(fake.into());
        f.popup.component().invoke_refresh_requested(fake.into());
    }
    assert_eq!(f.network.settings.load(Ordering::Relaxed), 0);
    let old_key = f.popup.component().get_settings_key();
    f.settings();
    f.popup
        .component()
        .invoke_settings_requested(old_key.clone());
    assert_eq!(f.network.settings.load(Ordering::Relaxed), 1);
    f.network
        .finish_settings(Err(error(NetworkErrorKind::Other)));
    f.drain();
    assert!(
        f.popup
            .component()
            .get_notice()
            .contains("could not be opened")
    );
    assert!(!f.popup.component().get_notice().contains("private-network"));
    f.popup.component().invoke_settings_requested(old_key);
    assert_eq!(f.network.settings.load(Ordering::Relaxed), 1);
    f.network
        .action_replies
        .lock()
        .push_back(ActionReply::Inline(Ok(())));
    f.settings();
    assert!(f.popup.component().get_settings_loading());
    f.drain();
    assert!(
        f.popup
            .component()
            .get_notice()
            .contains("does not confirm")
    );
    f.network
        .action_replies
        .lock()
        .push_back(ActionReply::Reject(error(NetworkErrorKind::Busy)));
    f.settings();
    f.drain();
    assert!(f.popup.component().get_notice().contains("busy"));
}

#[test]
fn network_late_settings_completion_does_not_overwrite_new_read_or_reopen_hidden_surface() {
    let f = Fixture::new();
    f.loaded();
    f.settings();
    f.popup.hide();
    f.show();
    f.network
        .finish_settings(Err(error(NetworkErrorKind::Other)));
    f.network.finish(Ok(snapshot()));
    f.drain();
    assert!(f.popup.component().get_notice().is_empty());
    assert!(!f.popup.component().get_settings_loading());
    f.settings();
    f.popup.hide();
    f.network.finish_settings(Ok(()));
    f.drain();
    assert!(!f.popup.is_open());
}

#[test]
fn network_failed_factory_attachment_cancel_focus_and_geometry_use_calendar_lifetime_contract() {
    let f = Fixture::new();
    *f.host.factory_error.lock() = Some(error(NetworkErrorKind::Unsupported));
    f.show();
    assert!(f.popup.component().get_notice().contains("not supported"));
    assert_eq!(f.network.reads(), 0);
    assert!(!f.popup.component().get_refresh_key().is_empty());
    f.popup.hide();
    f.host.factory_error.lock().take();
    f.host.cancel_attachment.store(true, Ordering::Relaxed);
    f.show();
    assert!(!f.popup.is_open());
    assert_eq!(f.network.reads(), 0);
    f.host.cancel_attachment.store(false, Ordering::Relaxed);
    f.host.deny_focus.store(true, Ordering::Relaxed);
    assert!(
        f.popup
            .show(f.source.window(), bounds(), context())
            .unwrap_err()
            .contains("keyboard focus")
    );
    assert!(f.popup.is_open());
    assert_eq!(f.network.reads(), 1);
    f.popup.close_if_geometry_changed(context(), 2.0);
    assert!(!f.popup.is_open());
    assert_eq!(f.popup.component().get_connected().row_count(), 0);
}

#[test]
fn network_raw_ssid_and_interface_identity_survive_lossy_labels_and_exact_current_bssid() {
    let value = NetworkSnapshot {
        interfaces: vec![
            interface(
                1,
                vec![
                    bss(&[0xff], 1, 75, None, None),
                    bss(&[0xfe], 2, 80, None, None),
                    bss(b"same", 3, 30, Some(true), Some(true)),
                    bss(b"same", 4, 90, Some(true), Some(false)),
                ],
            ),
            interface(2, vec![bss(b"same", 5, 60, None, None)]),
        ],
    };
    let view = presentation::project(&value);
    assert_eq!(view.connected.len(), 1);
    assert_eq!(
        view.available.len(),
        3,
        "lossy text and different adapters never merge"
    );
    assert_eq!(view.available[0].label, view.available[1].label);
    let current = &view.connected[0];
    assert!(
        current
            .description
            .contains("03:03:03:03:03:03: 30%, -48 dBm, 5180000 kHz, current AP")
    );
    assert!(
        current
            .description
            .contains("04:04:04:04:04:04: 90%, -48 dBm, 5180000 kHz, not current AP")
    );
    assert!(view.available[2].details.contains("Connection unknown"));
    assert!(view.available[2].details.contains("Saved profile unknown"));
    assert!(view.available[2].details.contains("Security unknown"));
    assert!(
        view.available[2]
            .description
            .contains("Authentication unknown")
    );
}

#[test]
fn network_radio_off_unknown_absent_empty_partial_remain_distinct_without_invented_offline() {
    let absent = presentation::project(&NetworkSnapshot { interfaces: vec![] });
    assert!(absent.radio.contains("No Wi-Fi adapter"));
    let mut value = snapshot();
    value.interfaces[0].radio = Observation::Ready(RadioState::Disabled);
    let off = presentation::project(&value);
    assert!(off.radio.contains("Off"));
    assert!(off.available.is_empty() && off.connected.is_empty());
    assert!(off.summary.contains("Settings"));
    value.interfaces[0].radio = Observation::Ready(RadioState::Unknown);
    value.interfaces[0].discovery = Observation::Ready(vec![]);
    let unknown = presentation::project(&value);
    assert!(unknown.radio.contains("Unknown"));
    assert!(unknown.summary.contains("cache"));
    value.interfaces[0].discovery = Observation::Unavailable(error(NetworkErrorKind::AccessDenied));
    let denied = presentation::project(&value);
    assert!(!denied.radio.contains("Off"));
    assert!(!denied.summary.contains("No Wi-Fi networks"));
    assert!(presentation::denied(&value));
}

#[test]
fn network_current_connection_only_unknown_preserves_rows_pauses_changed_and_manual_refresh_retries()
 {
    let f = Fixture::new();
    f.show();
    let unknown = NetworkSnapshot {
        interfaces: vec![interface(
            1,
            vec![bss(b"Readable cache", 1, 72, Some(false), None)],
        )],
    };
    f.network.finish(Ok(unknown));
    f.drain();
    assert_eq!(f.popup.component().get_available().row_count(), 1);
    assert!(
        f.popup
            .component()
            .get_available()
            .row_data(0)
            .unwrap()
            .details
            .contains("Connection unknown")
    );
    assert!(
        f.popup
            .component()
            .get_summary()
            .contains("automatic cache rereads are paused")
    );
    assert!(!f.popup.component().get_summary().contains("denied"));
    for _ in 0..100 {
        f.network.notify(NetworkEvent::Changed);
        f.drain();
    }
    assert_eq!(
        f.network.reads(),
        1,
        "unknown current-connection metadata cannot cause repeated privacy queries"
    );
    f.refresh();
    assert_eq!(
        f.network.reads(),
        2,
        "explicit manual retry remains authorized"
    );
    f.network.finish(Ok(snapshot()));
    f.drain();
    assert!(!f.popup.component().get_summary().contains("paused"));
    f.network.notify(NetworkEvent::Changed);
    f.drain();
    assert_eq!(f.network.reads(), 3);
}

#[test]
fn network_settings_ack_does_not_erase_independent_privacy_read_error_or_retry_it() {
    let f = Fixture::new();
    f.network
        .replies
        .lock()
        .push_back(Reply::Inline(Err(error(NetworkErrorKind::AccessDenied))));
    f.show();
    f.drain();
    f.network
        .action_replies
        .lock()
        .push_back(ActionReply::Inline(Ok(())));
    f.settings();
    f.drain();
    assert!(
        f.popup
            .component()
            .get_notice()
            .contains("location permissions")
    );
    assert!(
        f.popup
            .component()
            .get_notice()
            .contains("does not confirm")
    );
    f.network.notify(NetworkEvent::Changed);
    f.drain();
    assert_eq!(f.network.reads(), 1);
    assert!(!f.popup.component().get_refresh_key().is_empty());
}

#[test]
fn network_band_union_and_partial_authentication_never_invent_unknown_metadata() {
    let mut entries = vec![
        bss(b"Mixed", 1, 90, Some(true), Some(false)),
        bss(b"Mixed", 2, 50, None, Some(false)),
        bss(b"Mixed", 3, 30, None, Some(false)),
        bss(b"Mixed", 4, 20, None, Some(false)),
    ];
    entries[0].frequency_khz = 2_484_000;
    entries[0].secured = Some(true);
    entries[0].auth = Some("Native WPA2".into());
    entries[1].frequency_khz = 5_850_000;
    entries[2].frequency_khz = 7_125_000;
    entries[3].frequency_khz = 0;
    let view = presentation::project(&NetworkSnapshot {
        interfaces: vec![interface(1, entries)],
    });
    assert_eq!(view.saved.len(), 1);
    assert_eq!(view.saved[0].bands, "2.4G / 5G / 6G");
    assert!(view.saved[0].details.contains("Security unknown or mixed"));
    assert!(
        view.saved[0]
            .description
            .contains("some AP authentication unknown")
    );
    assert!(view.saved[0].description.contains("0 kHz"));
}
