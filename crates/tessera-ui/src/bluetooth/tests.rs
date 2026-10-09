// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::{BluetoothRadioStatus, Panel};
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{LogicalPosition, Model};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tessera_system::bluetooth::{
    BluetoothConnectionState, BluetoothDeviceId, BluetoothPairedDevice, BluetoothRadioObservation,
    BluetoothRadioState, BluetoothReadCompletion,
};

enum Reply {
    Pending,
    Inline(Result<BluetoothSnapshot, BluetoothError>),
    Reject(BluetoothError),
}

#[derive(Default)]
struct RecordingBluetooth {
    reads: AtomicUsize,
    pending: Mutex<Option<BluetoothReadCompletion>>,
    replies: Mutex<VecDeque<Reply>>,
}

impl BluetoothHost for RecordingBluetooth {
    fn read(&self, completion: BluetoothReadCompletion) -> Result<(), BluetoothError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        assert!(
            self.pending.lock().is_none(),
            "one native flight, even across sessions"
        );
        let reply = self.replies.lock().pop_front().unwrap_or(Reply::Pending);
        match reply {
            Reply::Pending => *self.pending.lock() = Some(completion),
            Reply::Inline(result) => completion(result),
            Reply::Reject(error) => return Err(error),
        }
        Ok(())
    }
}

impl RecordingBluetooth {
    fn finish(&self, result: Result<BluetoothSnapshot, BluetoothError>) {
        let completion = self
            .pending
            .lock()
            .take()
            .expect("an accepted Bluetooth read");
        completion(result);
    }
    fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }
}

struct RecordingDesktop {
    provider: Mutex<Result<Option<Arc<dyn BluetoothHost>>, BluetoothError>>,
    provider_calls: AtomicUsize,
    root: Mutex<Option<slint::Weak<BluetoothMenu>>>,
    events: Arc<Mutex<Vec<&'static str>>>,
    deny_attachment: AtomicBool,
    cancel_attachment: AtomicBool,
    deny_focus: AtomicBool,
}

struct Lease {
    root: slint::Weak<BluetoothMenu>,
    events: Arc<Mutex<Vec<&'static str>>>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(root) = self.root.upgrade() {
            assert!(
                root.window().is_visible(),
                "lease retires before native hide"
            );
        }
        self.events.lock().push("detach");
    }
}

impl DesktopHost for RecordingDesktop {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("no desktop observation")
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("no window effects")
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("no launch effects")
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("read-only Bluetooth")
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("no persistence")
    }
    fn bluetooth_host(&self) -> Result<Option<Arc<dyn BluetoothHost>>, BluetoothError> {
        self.provider_calls.fetch_add(1, Ordering::Relaxed);
        self.provider.lock().clone()
    }
    fn configure_surface(
        &self,
        kind: SurfaceKind,
        window: &slint::Window,
    ) -> Result<Option<Box<dyn std::any::Any>>, String> {
        assert_eq!(kind, SurfaceKind::Popup);
        assert!(window.is_visible());
        if self.deny_attachment.load(Ordering::Relaxed) {
            return Err("attachment denied".into());
        }
        self.events.lock().push("attach");
        if self.cancel_attachment.load(Ordering::Relaxed) {
            window.dispatch_event(WindowEvent::CloseRequested);
            assert!(
                window.is_visible(),
                "the late lease must retire before hide"
            );
        }
        Ok(Some(Box::new(Lease {
            root: self.root.lock().clone().unwrap(),
            events: self.events.clone(),
        })))
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

// The stock testing adapter deliberately omits native screen position. This
// geometry adapter records the actual Window allocation instead of asserting
// monitor containment against that adapter's default (0, 0) position.
struct GeometryWindow {
    window: slint::Window,
    renderer: slint::platform::software_renderer::SoftwareRenderer,
    position: Cell<PhysicalPosition>,
    size: Cell<PhysicalSize>,
}

impl slint::platform::WindowAdapter for GeometryWindow {
    fn window(&self) -> &slint::Window {
        &self.window
    }
    fn size(&self) -> PhysicalSize {
        self.size.get()
    }
    fn renderer(&self) -> &dyn slint::platform::Renderer {
        &self.renderer
    }
    fn position(&self) -> Option<PhysicalPosition> {
        Some(self.position.get())
    }
    fn set_position(&self, position: slint::WindowPosition) {
        self.position
            .set(position.to_physical(self.window.scale_factor()));
    }
    fn set_size(&self, size: slint::WindowSize) {
        self.size.set(size.to_physical(self.window.scale_factor()));
        self.window.dispatch_event(WindowEvent::Resized {
            size: size.to_logical(self.window.scale_factor()),
        });
    }
}

struct GeometryPlatform;

impl slint::platform::Platform for GeometryPlatform {
    fn create_window_adapter(
        &self,
    ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(Rc::<GeometryWindow>::new_cyclic(|weak| GeometryWindow {
            window: slint::Window::new(weak.clone()),
            renderer: slint::platform::software_renderer::SoftwareRenderer::new(),
            position: Cell::new(PhysicalPosition::new(0, 0)),
            size: Cell::new(PhysicalSize::new(1, 1)),
        }))
    }
}

struct Fixture {
    host: Arc<RecordingDesktop>,
    bluetooth: Arc<RecordingBluetooth>,
    source: Panel,
    popup: Rc<BluetoothController>,
}
impl Fixture {
    fn new() -> Self {
        Self::with_backend(i_slint_backend_testing::init_no_event_loop)
    }
    fn with_backend(initialize: impl FnOnce()) -> Self {
        initialize();
        let bluetooth = Arc::new(RecordingBluetooth::default());
        let host = Arc::new(RecordingDesktop {
            provider: Mutex::new(Ok(Some(bluetooth.clone()))),
            provider_calls: AtomicUsize::new(0),
            root: Mutex::default(),
            events: Arc::new(Mutex::default()),
            deny_attachment: AtomicBool::new(false),
            cancel_attachment: AtomicBool::new(false),
            deny_focus: AtomicBool::new(false),
        });
        let source = Panel::new().unwrap();
        source
            .window()
            .set_position(PhysicalPosition::new(-1400, 0));
        source.window().set_size(PhysicalSize::new(600, 40));
        source.show().unwrap();
        let popup = BluetoothController::new(host.clone()).unwrap();
        *host.root.lock() = Some(popup.component().as_weak());
        Self {
            host,
            bluetooth,
            source,
            popup,
        }
    }
    fn show(&self) -> Result<(), String> {
        self.popup.show(self.source.window(), bounds(), context())
    }
    fn drain(&self) {
        self.popup.component().invoke_bluetooth_event_ready();
    }
    fn loaded(&self, snapshot: BluetoothSnapshot) {
        self.show().unwrap();
        self.bluetooth.finish(Ok(snapshot));
        assert!(
            self.popup.component().get_loading(),
            "worker completion cannot project inline"
        );
        self.drain();
    }
    fn element(&self, label: &str) -> ElementHandle {
        ElementHandle::find_by_accessible_label(self.popup.component(), label)
            .find(|element| element.accessible_role() != Some(AccessibleRole::None))
            .unwrap_or_else(|| panic!("missing semantic element {label}"))
    }
    fn click_refresh(&self) {
        let element = self.element("Refresh Bluetooth");
        let origin = element.absolute_position();
        let size = element.size();
        assert!(size.width > 0.0 && size.height > 0.0);
        let position =
            LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        let window = self.popup.component().window();
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    }
    fn key(&self, key: Key) {
        let window = self.popup.component().window();
        window.dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        window.dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }
}
fn bounds() -> TileBounds {
    TileBounds {
        origin: LogicalPosition::new(20.0, 0.0),
        width: 40.0,
        height: 40.0,
    }
}
fn context() -> DockContext {
    DockContext::new(-1920, 0, 1920, 1080, false).unwrap()
}
fn device(id: &str, name: &str, connection: BluetoothConnectionState) -> BluetoothPairedDevice {
    BluetoothPairedDevice {
        id: BluetoothDeviceId::new(id).unwrap(),
        name: name.into(),
        connection,
    }
}
fn ready() -> BluetoothSnapshot {
    BluetoothSnapshot::new(
        Ok(vec![BluetoothRadioObservation {
            name: "Actual radio".into(),
            state: BluetoothRadioState::On,
        }]),
        Ok(vec![
            device(
                "classic/opaque/1",
                "Headset",
                BluetoothConnectionState::Connected,
            ),
            device(
                "classic/opaque/2",
                "Keyboard",
                BluetoothConnectionState::Disconnected,
            ),
        ]),
        Ok(vec![device(
            "le/opaque/1",
            "Sensor",
            BluetoothConnectionState::Unknown,
        )]),
    )
}
fn empty() -> BluetoothSnapshot {
    BluetoothSnapshot::new(Ok(vec![]), Ok(vec![]), Ok(vec![]))
}
fn error(kind: BluetoothErrorKind) -> BluetoothError {
    BluetoothError::new(kind, "native\n detail\u{202e}\u{0007}")
}

#[test]
fn bluetooth_inline_and_rejected_reads_project_only_through_posted_wake() {
    let fixture = Fixture::new();
    fixture
        .bluetooth
        .replies
        .lock()
        .push_back(Reply::Inline(Ok(ready())));
    assert_eq!(
        fixture.host.provider_calls.load(Ordering::Relaxed),
        0,
        "inert construction"
    );
    fixture.show().unwrap();
    assert_eq!(fixture.bluetooth.reads(), 1);
    assert!(fixture.popup.component().get_loading());
    assert!(!fixture.popup.component().get_has_snapshot());
    assert!(fixture.popup.mailbox.lock().completion.is_some());
    fixture.drain();
    let root = fixture.popup.component();
    assert!(!root.get_loading());
    assert!(root.get_has_snapshot());
    assert_eq!(root.get_connected().row_count(), 1);
    assert_eq!(root.get_paired().row_count(), 1);
    assert_eq!(root.get_unknown().row_count(), 1);
    assert_eq!(
        fixture.element("Sensor").accessible_description().unwrap(),
        "Connection unknown, Low Energy"
    );
    fixture
        .bluetooth
        .replies
        .lock()
        .push_back(Reply::Reject(error(BluetoothErrorKind::AccessDenied)));
    fixture.click_refresh();
    assert!(root.get_loading());
    fixture.drain();
    assert!(!root.get_has_snapshot());
    assert!(root.get_notice().contains("access was denied"));
    assert!(root.get_notice().contains("native detail"));
    assert!(!root.get_notice().contains('\u{202e}'));
    assert_eq!(
        fixture.host.provider_calls.load(Ordering::Relaxed),
        1,
        "reuse independent capability"
    );
}

#[test]
fn bluetooth_close_reopen_rejects_stale_projection_and_keeps_one_flight() {
    let fixture = Fixture::new();
    fixture.show().unwrap();
    fixture.popup.hide();
    fixture.show().unwrap();
    assert_eq!(
        fixture.bluetooth.reads(),
        1,
        "reopen does not abandon accepted slot"
    );
    for _ in 0..12 {
        fixture.click_refresh();
    }
    assert_eq!(fixture.bluetooth.reads(), 1);
    fixture.bluetooth.finish(Ok(ready()));
    fixture.drain();
    assert!(
        !fixture.popup.component().get_has_snapshot(),
        "retired session cannot paint the next one"
    );
    assert!(fixture.popup.component().get_loading());
    assert_eq!(fixture.bluetooth.reads(), 2, "one coalesced fresh read");
    fixture.bluetooth.finish(Ok(empty()));
    fixture.drain();
    assert!(fixture.popup.component().get_no_radios());
    assert!(fixture.popup.component().get_empty_inventory());
    assert!(fixture.popup.component().get_connected().row_count() == 0);
    assert_eq!(fixture.bluetooth.reads(), 2);
}

#[test]
fn bluetooth_repeated_manual_refresh_coalesces_exactly_one_subsequent_observation() {
    let fixture = Fixture::new();
    fixture.loaded(ready());
    fixture.click_refresh();
    assert_eq!(fixture.bluetooth.reads(), 2);
    for _ in 0..10 {
        fixture.click_refresh();
    }
    assert_eq!(fixture.bluetooth.reads(), 2);
    fixture.bluetooth.finish(Ok(empty()));
    fixture.drain();
    assert_eq!(fixture.bluetooth.reads(), 3);
    fixture.bluetooth.finish(Ok(ready()));
    fixture.drain();
    assert!(!fixture.popup.component().get_loading());
    assert_eq!(fixture.bluetooth.reads(), 3);
    fixture.popup.hide();
    fixture.popup.component().invoke_refresh_requested();
    assert_eq!(fixture.bluetooth.reads(), 3, "hidden callbacks cannot read");
}

#[test]
fn bluetooth_partial_errors_do_not_turn_unknown_or_denied_inventory_into_empty_success() {
    let fixture = Fixture::new();
    fixture.loaded(BluetoothSnapshot::new(
        Err(error(BluetoothErrorKind::AccessDenied)),
        Ok(vec![
            device("a", "Same", BluetoothConnectionState::Unknown),
            device("b", "Same", BluetoothConnectionState::Unknown),
        ]),
        Err(error(BluetoothErrorKind::Unavailable)),
    ));
    let root = fixture.popup.component();
    assert!(!root.get_no_radios(), "radio failure is not absence");
    assert!(!root.get_empty_inventory());
    assert!(root.get_partial_inventory());
    assert_eq!(
        root.get_unknown().row_count(),
        2,
        "names are not identities"
    );
    assert_eq!(
        root.get_paired().row_count(),
        0,
        "unknown is not disconnected"
    );
    assert!(
        root.get_notice()
            .contains("Radios: Bluetooth access was denied")
    );
    assert!(
        root.get_notice()
            .contains("Low Energy: Bluetooth observations are unavailable")
    );
    fixture.click_refresh();
    fixture.bluetooth.finish(Ok(BluetoothSnapshot::new(
        Ok(vec![]),
        Ok(vec![]),
        Err(error(BluetoothErrorKind::AccessDenied)),
    )));
    fixture.drain();
    assert!(root.get_empty_inventory());
    assert!(root.get_partial_inventory());
    fixture.element("No paired devices in the inventories read.");
    fixture.click_refresh();
    fixture.bluetooth.finish(Ok(BluetoothSnapshot::new(
        Ok(vec![]),
        Err(error(BluetoothErrorKind::AccessDenied)),
        Err(error(BluetoothErrorKind::InvalidData)),
    )));
    fixture.drain();
    assert!(
        !root.get_empty_inventory(),
        "two failures cannot claim empty inventory"
    );
}

#[test]
fn bluetooth_unsupported_capability_is_honest_and_refresh_retries_acquisition() {
    let fixture = Fixture::new();
    *fixture.host.provider.lock() = Ok(None);
    fixture.show().unwrap();
    assert!(!fixture.popup.component().get_loading());
    assert!(!fixture.popup.component().get_has_snapshot());
    assert!(
        fixture
            .popup
            .component()
            .get_notice()
            .contains("not supported")
    );
    assert!(!fixture.popup.component().get_no_radios());
    assert_eq!(fixture.bluetooth.reads(), 0);
    *fixture.host.provider.lock() = Err(error(BluetoothErrorKind::AccessDenied));
    fixture.click_refresh();
    assert!(fixture.popup.component().get_notice().contains("denied"));
    *fixture.host.provider.lock() = Ok(Some(fixture.bluetooth.clone()));
    fixture.click_refresh();
    assert_eq!(fixture.bluetooth.reads(), 1);
    assert!(fixture.popup.component().get_loading());
}

#[test]
fn bluetooth_bounded_names_radios_and_genuine_sections_have_no_effect_buttons() {
    let fixture = Fixture::new();
    let long = format!("\u{202e}\u{0007}  אוזניות\n{}", "界".repeat(300));
    fixture.loaded(BluetoothSnapshot::new(
        Ok(vec![
            BluetoothRadioObservation {
                name: "Radio disabled".into(),
                state: BluetoothRadioState::Disabled,
            },
            BluetoothRadioObservation {
                name: "Radio unknown".into(),
                state: BluetoothRadioState::Unknown,
            },
            BluetoothRadioObservation {
                name: "Radio off".into(),
                state: BluetoothRadioState::Off,
            },
        ]),
        Ok(vec![device("id", &long, BluetoothConnectionState::Unknown)]),
        Ok(vec![]),
    ));
    let root = fixture.popup.component();
    let name = root.get_unknown().row_data(0).unwrap().name;
    assert!(name.chars().count() <= 129);
    assert!(!name.contains('\u{202e}'));
    assert!(!name.contains('\u{0007}'));
    assert!(name.starts_with("אוזניות "));
    assert_eq!(
        root.get_radios().row_data(0).unwrap().state,
        BluetoothRadioStatus::Disabled
    );
    assert_eq!(
        fixture
            .element("Radio unknown")
            .accessible_description()
            .unwrap(),
        "Bluetooth radio, Unknown"
    );
    let buttons = ElementQuery::from_root(root)
        .match_accessible_role(AccessibleRole::Button)
        .find_all();
    assert_eq!(buttons.len(), 1);
    assert_eq!(buttons[0].accessible_label().unwrap(), "Refresh Bluetooth");
    assert!(
        ElementQuery::from_root(root)
            .match_accessible_role(AccessibleRole::Checkbox)
            .find_all()
            .is_empty()
    );
}

#[test]
fn bluetooth_attach_failure_focus_denial_visibility_escape_and_focus_loss_follow_lease() {
    let fixture = Fixture::new();
    fixture.host.deny_attachment.store(true, Ordering::Relaxed);
    assert!(fixture.show().unwrap_err().contains("attachment denied"));
    assert!(!fixture.popup.is_open());
    assert!(!fixture.popup.component().window().is_visible());
    assert_eq!(fixture.bluetooth.reads(), 0);
    fixture.host.deny_attachment.store(false, Ordering::Relaxed);
    fixture
        .host
        .cancel_attachment
        .store(true, Ordering::Relaxed);
    fixture.show().unwrap();
    assert!(!fixture.popup.is_open());
    assert_eq!(&*fixture.host.events.lock(), &["attach", "detach"]);
    fixture
        .host
        .cancel_attachment
        .store(false, Ordering::Relaxed);
    fixture.host.deny_focus.store(true, Ordering::Relaxed);
    assert!(fixture.show().unwrap_err().contains("focus"));
    assert!(
        fixture.popup.is_open(),
        "accepted native visibility survives focus denial"
    );
    assert!(fixture.popup.component().window().is_visible());
    assert_eq!(
        fixture.host.events.lock().last(),
        Some(&"focus"),
        "keep the native lease"
    );
    assert_eq!(
        fixture.bluetooth.reads(),
        1,
        "visible observations still start"
    );
    fixture.host.deny_focus.store(false, Ordering::Relaxed);
    fixture.show().unwrap();
    fixture.key(Key::Escape);
    assert!(!fixture.popup.is_open());
    fixture.show().unwrap();
    fixture.popup.focus_observed(Some(false));
    assert!(
        fixture.popup.is_open(),
        "do not dismiss before first observed focus"
    );
    fixture.popup.focus_observed(Some(true));
    fixture.popup.focus_observed(Some(false));
    assert!(!fixture.popup.is_open());
    assert_eq!(fixture.host.events.lock().last(), Some(&"detach"));
    fixture.bluetooth.finish(Ok(ready()));
    fixture.drain();
    assert!(fixture.popup.state.borrow().flight.is_none());
    assert!(fixture.popup.state.borrow().snapshot.is_none());
}

#[test]
fn bluetooth_native_monitor_placement_and_geometry_retirement_use_real_bounds() {
    let fixture = Fixture::with_backend(|| {
        slint::platform::set_platform(Box::new(GeometryPlatform)).unwrap();
    });
    fixture.show().unwrap();
    let window = fixture.popup.component().window();
    let position = window.position();
    let size = window.size();
    assert!(
        position.x >= -1920 && position.y >= 0,
        "{position:?} {size:?}"
    );
    assert!(
        i64::from(position.x) + i64::from(size.width) <= 0,
        "{position:?} {size:?}"
    );
    assert!(
        i64::from(position.y) + i64::from(size.height) <= 1080,
        "{position:?} {size:?}"
    );
    let bottom = fixture.source.window().position().y
        + i32::try_from(fixture.source.window().size().height).unwrap();
    assert_eq!(
        position.y,
        bottom - 10,
        "native source bottom owns toolbar anchoring"
    );
    fixture
        .popup
        .close_if_geometry_changed(DockContext::new(-1920, 0, 1920, 900, false).unwrap(), 1.0);
    assert!(!fixture.popup.is_open());
    let tiny = DockContext::new(-80, -60, 80, 60, false).unwrap();
    fixture
        .popup
        .show(fixture.source.window(), bounds(), tiny)
        .unwrap();
    let actual = fixture.popup.component().window();
    assert_eq!(actual.position(), PhysicalPosition::new(-80, -60));
    assert_eq!(actual.size(), PhysicalSize::new(80, 60));
    fixture.popup.hide();
    let mut invalid = bounds();
    invalid.width = f32::NAN;
    assert!(
        fixture
            .popup
            .show(fixture.source.window(), invalid, context())
            .is_err()
    );
    assert_eq!(
        fixture.bluetooth.reads(),
        1,
        "invalid geometry cannot acquire another read"
    );
}

#[test]
fn bluetooth_controller_drop_does_not_join_or_cancel_accepted_native_work() {
    let fixture = Fixture::new();
    fixture.show().unwrap();
    let bluetooth = fixture.bluetooth.clone();
    let weak = Rc::downgrade(&fixture.popup);
    drop(fixture);
    assert!(weak.upgrade().is_none());
    assert!(bluetooth.pending.lock().is_some());
    bluetooth.finish(Ok(ready()));
    assert!(bluetooth.pending.lock().is_none());
}
