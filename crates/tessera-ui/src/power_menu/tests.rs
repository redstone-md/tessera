// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::Panel;
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use i_slint_backend_testing::ElementHandle;
use slint::LogicalPosition;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tessera_core::Rect;
use tessera_system::display_context::{DisplayContextCompletion, DisplaySelection};
use tessera_system::power::PowerCompletion;
use tessera_system::power_updates::{
    PowerUpdateHint, PowerUpdatesCompletion, PowerUpdatesError, PowerUpdatesHost,
};

mod lifetime_tests;
mod six_tests;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hook {
    DisplayFactory,
    Read,
    PowerFactory,
    UpdatesFactory,
    UpdatesRead,
    Perform,
    Configure,
    LeaseDrop,
    Focus,
    Opened,
}

type UiHook = Box<dyn FnOnce()>;
thread_local! {
    // Recording adapters alone run these on the UI thread. Send completions
    // still carry only data and the production weak generated component.
    static HOOKS: RefCell<Vec<(Hook, UiHook)>> = const { RefCell::new(Vec::new()) };
}

fn hook(point: Hook, callback: impl FnOnce() + 'static) {
    HOOKS.with(|hooks| hooks.borrow_mut().push((point, Box::new(callback))));
}

fn run_hook(point: Hook) {
    let callback = HOOKS.with(|hooks| {
        let mut hooks = hooks.borrow_mut();
        let index = hooks
            .iter()
            .position(|(candidate, _)| *candidate == point)?;
        Some(hooks.remove(index).1)
    });
    if let Some(callback) = callback {
        callback();
    }
}

enum Reply<T, E> {
    Delayed,
    Inline(Result<T, E>),
    Rejected(E),
}

type Completion<T, E> = Box<dyn FnOnce(Result<T, E>) + Send + 'static>;

struct Recording<T, E> {
    replies: VecDeque<Reply<T, E>>,
    pending: VecDeque<Completion<T, E>>,
    calls: usize,
    active: usize,
    maximum_active: usize,
}

impl<T, E> Default for Recording<T, E> {
    fn default() -> Self {
        Self {
            replies: VecDeque::new(),
            pending: VecDeque::new(),
            calls: 0,
            active: 0,
            maximum_active: 0,
        }
    }
}

// One small fixture shared by the three independent typed provider contracts.
// Consumer callbacks and reentry hooks always run outside the recording lock.
fn submit<T, E>(
    recording: &Mutex<Recording<T, E>>,
    point: Hook,
    completion: Completion<T, E>,
) -> Result<(), E> {
    let reply = {
        let mut state = recording.lock();
        state.calls += 1;
        let reply = state.replies.pop_front().unwrap_or(Reply::Delayed);
        if !matches!(reply, Reply::Rejected(_)) {
            state.active += 1;
            state.maximum_active = state.maximum_active.max(state.active);
        }
        reply
    };
    run_hook(point);
    match reply {
        Reply::Delayed => {
            recording.lock().pending.push_back(completion);
            Ok(())
        }
        Reply::Inline(result) => {
            recording.lock().active -= 1;
            completion(result);
            Ok(())
        }
        Reply::Rejected(error) => Err(error),
    }
}

fn finish<T, E>(recording: &Mutex<Recording<T, E>>, result: Result<T, E>) {
    let completion = {
        let mut state = recording.lock();
        let completion = state.pending.pop_front().expect("accepted delayed request");
        state.active -= 1;
        completion
    };
    completion(result);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    DisplayFactory,
    Read,
    Attach,
    AttachDenied,
    Opened,
    Focus,
    Detach { visible: bool },
    PowerFactory,
    Perform(PowerAction),
}

#[derive(Default)]
struct Trace {
    root: Mutex<Option<slint::Weak<PowerMenuSurface>>>,
    events: Mutex<Vec<Event>>,
    leases: AtomicUsize,
}

impl Trace {
    fn assert_retired(&self) {
        let root = self.root.lock().clone().expect("recording Power root");
        assert!(!root.upgrade().unwrap().window().is_visible());
        assert_eq!(self.leases.load(Ordering::Relaxed), 0);
    }
}

struct RecordingDisplay {
    state: Mutex<Recording<Option<DisplayLayout>, DisplayContextError>>,
    trace: Arc<Trace>,
}

impl DisplayContextHost for RecordingDisplay {
    fn read(&self, completion: DisplayContextCompletion) -> Result<(), DisplayContextError> {
        self.trace.events.lock().push(Event::Read);
        submit(&self.state, Hook::Read, completion)
    }
}

struct RecordingPower {
    state: Mutex<Recording<PowerRequestAccepted, PowerError>>,
    trace: Arc<Trace>,
}

impl PowerHost for RecordingPower {
    fn perform(&self, action: PowerAction, completion: PowerCompletion) -> Result<(), PowerError> {
        self.trace.assert_retired();
        self.trace.events.lock().push(Event::Perform(action));
        submit(&self.state, Hook::Perform, completion)
    }
}

struct RecordingUpdates {
    state: Mutex<Recording<PowerUpdateHint, PowerUpdatesError>>,
}

impl PowerUpdatesHost for RecordingUpdates {
    fn read(&self, completion: PowerUpdatesCompletion) -> Result<(), PowerUpdatesError> {
        {
            let mut state = self.state.lock();
            if state.replies.is_empty() {
                state
                    .replies
                    .push_back(Reply::Inline(Ok(PowerUpdateHint::NotDetected)));
            }
        }
        submit(&self.state, Hook::UpdatesRead, completion)
    }
}

struct RecordingDesktop {
    display: Mutex<Result<Option<Arc<dyn DisplayContextHost>>, DisplayContextError>>,
    power: Mutex<Result<Option<Arc<dyn PowerHost>>, PowerError>>,
    display_getters: AtomicUsize,
    power_getters: AtomicUsize,
    updates: Mutex<Result<Option<Arc<dyn PowerUpdatesHost>>, PowerUpdatesError>>,
    deny_attachment: AtomicBool,
    deny_focus: AtomicBool,
    updates_getters: AtomicUsize,
    animations: AtomicBool,
    trace: Arc<Trace>,
}

struct Lease(Arc<Trace>);

impl Drop for Lease {
    fn drop(&mut self) {
        let visible = self
            .0
            .root
            .lock()
            .clone()
            .unwrap()
            .upgrade()
            .unwrap()
            .window()
            .is_visible();
        self.0.leases.fetch_sub(1, Ordering::Relaxed);
        self.0.events.lock().push(Event::Detach { visible });
        run_hook(Hook::LeaseDrop);
    }
}

impl DesktopHost for RecordingDesktop {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("Power presentation must not observe the catalog")
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("Lock is not catalog activation")
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("Lock is not an application or folder launch")
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("Lock must use the typed PowerHost, never Exit or an untyped action")
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("Power presentation must not persist state")
    }
    fn shell_identity(&self) -> Result<crate::ShellIdentity, String> {
        panic!("This tracer must not invent or read an account projection")
    }
    fn display_context_host(
        &self,
    ) -> Result<Option<Arc<dyn DisplayContextHost>>, DisplayContextError> {
        self.display_getters.fetch_add(1, Ordering::Relaxed);
        self.trace.events.lock().push(Event::DisplayFactory);
        run_hook(Hook::DisplayFactory);
        self.display.lock().clone()
    }
    fn power_host(&self) -> Result<Option<Arc<dyn PowerHost>>, PowerError> {
        self.trace.assert_retired();
        self.power_getters.fetch_add(1, Ordering::Relaxed);
        self.trace.events.lock().push(Event::PowerFactory);
        run_hook(Hook::PowerFactory);
        self.power.lock().clone()
    }
    fn power_updates_host(&self) -> Result<Option<Arc<dyn PowerUpdatesHost>>, PowerUpdatesError> {
        self.updates_getters.fetch_add(1, Ordering::Relaxed);
        run_hook(Hook::UpdatesFactory);
        self.updates.lock().clone()
    }
    fn configure_surface(
        &self,
        kind: SurfaceKind,
        window: &slint::Window,
    ) -> Result<Option<Box<dyn std::any::Any>>, String> {
        assert_eq!(kind, SurfaceKind::Popup);
        assert!(window.is_visible());
        if self.deny_attachment.load(Ordering::Relaxed) {
            self.trace.events.lock().push(Event::AttachDenied);
            return Err("recording attachment failure".into());
        }
        self.trace.events.lock().push(Event::Attach);
        self.trace.leases.fetch_add(1, Ordering::Relaxed);
        let lease = Lease(self.trace.clone());
        run_hook(Hook::Configure);
        Ok(Some(Box::new(lease)))
    }
    fn request_ui_focus(&self, window: &slint::Window) -> Result<(), String> {
        assert!(window.is_visible());
        self.trace.events.lock().push(Event::Focus);
        run_hook(Hook::Focus);
        if self.deny_focus.load(Ordering::Relaxed) {
            Err("recording focus denial".into())
        } else {
            Ok(())
        }
    }
    fn ui_animations_enabled(&self) -> bool {
        self.animations.load(Ordering::Relaxed)
    }
}

struct Fixture {
    host: Arc<RecordingDesktop>,
    display: Arc<RecordingDisplay>,
    updates: Arc<RecordingUpdates>,
    power: Arc<RecordingPower>,
    sibling: Panel,
    popup: Rc<PowerMenuController>,
    initial_component: PowerMenuSurface,
    opened: Rc<Cell<usize>>,
    results: Rc<RefCell<Vec<Result<(), String>>>>,
}

impl Fixture {
    fn new() -> Self {
        i_slint_backend_testing::init_no_event_loop();
        Self::on_installed_platform()
    }

    fn on_installed_platform() -> Self {
        HOOKS.with(|hooks| hooks.borrow_mut().clear());
        let trace = Arc::new(Trace::default());
        let display = Arc::new(RecordingDisplay {
            state: Mutex::default(),
            trace: trace.clone(),
        });
        let power = Arc::new(RecordingPower {
            state: Mutex::default(),
            trace: trace.clone(),
        });
        let updates = Arc::new(RecordingUpdates {
            state: Mutex::default(),
        });
        let host = Arc::new(RecordingDesktop {
            display: Mutex::new(Ok(Some(display.clone()))),
            power: Mutex::new(Ok(Some(power.clone()))),
            updates: Mutex::new(Ok(Some(updates.clone()))),
            display_getters: AtomicUsize::new(0),
            power_getters: AtomicUsize::new(0),
            updates_getters: AtomicUsize::new(0),
            deny_attachment: AtomicBool::new(false),
            deny_focus: AtomicBool::new(false),
            animations: AtomicBool::new(false),
            trace: trace.clone(),
        });
        let sibling = Panel::new().unwrap();
        sibling.show().unwrap();
        let sibling_weak = sibling.as_weak();
        let opened = Rc::new(Cell::new(0));
        let opened_record = opened.clone();
        let results = Rc::new(RefCell::new(Vec::new()));
        let result_record = results.clone();
        let popup = PowerMenuController::new(
            host.clone(),
            Rc::new(move || {
                opened_record.set(opened_record.get() + 1);
                trace.events.lock().push(Event::Opened);
                sibling_weak.upgrade().unwrap().hide().unwrap();
                run_hook(Hook::Opened);
            }),
            Rc::new(move |result| result_record.borrow_mut().push(result)),
        )
        .unwrap();
        let initial_component = popup.component();
        *host.trace.root.lock() = Some(initial_component.as_weak());
        Self {
            host,
            display,
            power,
            updates,
            sibling,
            popup,
            opened,
            initial_component,
            results,
        }
    }

    fn component(&self) -> PowerMenuSurface {
        self.popup
            .surface_snapshot()
            .map(|(_, surface)| surface.clone_strong())
            .unwrap_or_else(|| self.initial_component.clone_strong())
    }

    fn drain(&self) {
        self.popup.process_events();
    }

    fn loaded(&self) {
        assert!(self.popup.show(Theme::Dark).unwrap());
        *self.host.trace.root.lock() = Some(self.component().as_weak());
        finish(&self.display.state, Ok(Some(layout())));
        self.drain();
        assert!(self.popup.is_visible());
    }

    fn lock(&self) {
        self.component()
            .invoke_action_requested(PowerMenuAction::LockSession);
    }

    fn events(&self) -> Vec<Event> {
        self.host.trace.events.lock().clone()
    }

    fn key(&self, key: Key) {
        let component = self.component();
        let window = component.window();
        window.dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        window.dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }

    fn click_lock(&self) {
        let component = self.component();
        let element = ElementHandle::find_by_accessible_label(&component, "Lock session")
            .next()
            .expect("genuine generated Lock tile");
        let origin = element.absolute_position();
        let size = element.size();
        assert!(size.width > 0.0 && size.height > 0.0);
        let position =
            LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        let window = component.window();
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        HOOKS.with(|hooks| hooks.borrow_mut().clear());
        self.popup.close();
        self.sibling.hide().unwrap();
    }
}

fn layout() -> DisplayLayout {
    DisplayLayout::new(
        Rect::new(-1920, -240, 3840, 1320).unwrap(),
        Rect::new(-1920, 0, 1920, 1080).unwrap(),
        1.5,
        DisplaySelection::Primary,
    )
    .unwrap()
}

#[test]
fn opening_reads_fresh_context_but_never_acquires_power_and_only_success_dismisses_sibling() {
    let fixture = Fixture::new();
    assert!(fixture.popup.show(Theme::Light).unwrap());
    assert!(!fixture.popup.is_visible());
    assert!(fixture.sibling.window().is_visible());
    assert!(!fixture.component().get_action_enabled());
    fixture.lock();
    assert_eq!(fixture.power.state.lock().calls, 0);
    finish(&fixture.display.state, Ok(Some(layout())));
    assert!(
        !fixture.popup.is_visible(),
        "delivery alone cannot project UI"
    );
    assert!(fixture.sibling.window().is_visible());
    fixture.drain();
    assert!(fixture.popup.is_visible());
    assert!(!fixture.sibling.window().is_visible());
    assert_eq!(fixture.opened.get(), 1);
    assert_eq!(
        fixture.component().window().size(),
        PhysicalSize::new(3840, 1320)
    );
    assert_eq!(fixture.component().get_selected_y(), 240.0);
    assert_eq!(fixture.component().get_metric_scale(), 1.5);
    assert!(fixture.popup.show(Theme::Dark).unwrap());
    assert!(
        !fixture.component().get_action_enabled(),
        "fresh read revokes old input authority"
    );
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert_eq!(fixture.display.state.lock().calls, 2);
    assert_eq!(fixture.host.display_getters.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.power.state.lock().calls, 0);
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|event| **event == Event::Attach)
            .count(),
        1
    );
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|event| **event == Event::Focus)
            .count(),
        1
    );
    assert_eq!(fixture.opened.get(), 1);
}

#[test]
fn empty_failed_over_budget_and_failed_native_attachment_preserve_existing_sibling() {
    let fixture = Fixture::new();
    let too_large = DisplayLayout::new(
        Rect::new(0, 0, 16385, 1).unwrap(),
        Rect::new(0, 0, 1, 1).unwrap(),
        1.0,
        DisplaySelection::FirstFallback,
    )
    .unwrap();
    for result in [
        Ok(None),
        Err(DisplayContextError::Native { code: 9876 }),
        Ok(Some(too_large)),
    ] {
        assert!(fixture.popup.show(Theme::Light).unwrap());
        finish(&fixture.display.state, result);
        fixture.drain();
        assert!(!fixture.popup.is_visible());
        assert!(!fixture.component().window().is_visible());
        assert!(fixture.sibling.window().is_visible());
        assert_eq!(fixture.opened.get(), 0);
        fixture.lock();
    }
    fixture.host.deny_attachment.store(true, Ordering::Relaxed);
    fixture.popup.show(Theme::Dark).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().window().is_visible());
    assert!(fixture.sibling.window().is_visible());
    assert_eq!(fixture.opened.get(), 0);
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.results.borrow().len(), 4);
    assert!(fixture.results.borrow().iter().all(Result::is_err));
    assert!(
        fixture
            .results
            .borrow()
            .iter()
            .all(|result| !result.as_ref().unwrap_err().contains("9876"))
    );
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.power.state.lock().calls, 0);
    assert!(!fixture.events().contains(&Event::Focus));
}

#[test]
fn display_getter_caches_only_success_and_remains_independent_of_power() {
    let fixture = Fixture::new();
    for provider in [
        Err(DisplayContextError::Unavailable),
        Err(DisplayContextError::Busy),
        Ok(None),
    ] {
        *fixture.host.display.lock() = provider;
        fixture.popup.show(Theme::Dark).unwrap();
        fixture.drain();
        assert!(!fixture.popup.is_visible());
        assert!(fixture.sibling.window().is_visible());
    }
    assert_eq!(fixture.host.display_getters.load(Ordering::Relaxed), 3);
    assert_eq!(fixture.display.state.lock().calls, 0);
    *fixture.host.display.lock() = Ok(Some(fixture.display.clone()));
    fixture.loaded();
    fixture.popup.hide();
    *fixture.host.display.lock() = Err(DisplayContextError::Unavailable);
    fixture.loaded();
    assert_eq!(fixture.host.display_getters.load(Ordering::Relaxed), 4);
    assert_eq!(fixture.display.state.lock().calls, 2);
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn power_getter_failures_retry_and_success_cache_does_not_cache_perform_failure() {
    let fixture = Fixture::new();
    for provider in [
        Err(PowerError::Unavailable),
        Err(PowerError::AccessDenied),
        Ok(None),
    ] {
        *fixture.host.power.lock() = provider;
        fixture.loaded();
        fixture.lock();
        assert!(!fixture.popup.is_visible());
        assert!(!fixture.component().get_lock_busy());
    }
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 3);
    assert_eq!(fixture.power.state.lock().calls, 0);
    *fixture.host.power.lock() = Ok(Some(fixture.power.clone()));
    fixture
        .power
        .state
        .lock()
        .replies
        .push_back(Reply::Rejected(PowerError::Busy));
    fixture.loaded();
    fixture.lock();
    assert!(
        fixture.component().get_lock_busy(),
        "immediate Err retires via mailbox"
    );
    fixture.drain();
    assert!(!fixture.component().get_lock_busy());
    *fixture.host.power.lock() = Err(PowerError::Unavailable);
    fixture
        .power
        .state
        .lock()
        .replies
        .push_back(Reply::Inline(Ok(PowerRequestAccepted)));
    fixture.loaded();
    fixture.lock();
    assert!(
        fixture.component().get_lock_busy(),
        "inline completion cannot project synchronously"
    );
    fixture.drain();
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 4);
    assert_eq!(fixture.power.state.lock().calls, 2);
    assert_eq!(fixture.power.state.lock().maximum_active, 1);
    assert_eq!(fixture.host.display_getters.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.results.borrow().last(), Some(&Ok(())));
}

#[test]
fn immediate_read_rejection_and_inline_completion_are_mailboxed_and_retryable() {
    let fixture = Fixture::new();
    fixture
        .display
        .state
        .lock()
        .replies
        .push_back(Reply::Rejected(DisplayContextError::Busy));
    assert!(fixture.popup.show(Theme::Dark).unwrap());
    assert!(fixture.results.borrow().is_empty());
    assert!(fixture.sibling.window().is_visible());
    fixture.drain();
    assert_eq!(fixture.results.borrow().len(), 1);
    fixture
        .display
        .state
        .lock()
        .replies
        .push_back(Reply::Inline(Ok(Some(layout()))));
    fixture.popup.show(Theme::Light).unwrap();
    assert!(!fixture.popup.is_visible());
    assert_eq!(fixture.opened.get(), 0);
    fixture.component().invoke_power_event_ready();
    assert!(fixture.popup.is_visible());
    assert_eq!(fixture.opened.get(), 1);
    assert_eq!(fixture.display.state.lock().active, 0);
    assert_eq!(fixture.host.display_getters.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn delayed_reads_coalesce_and_hidden_old_generation_cannot_present_or_authorize_reopen() {
    let fixture = Fixture::new();
    fixture.popup.show(Theme::Light).unwrap();
    fixture.popup.refresh_display();
    fixture.popup.refresh_display();
    fixture.popup.show(Theme::Dark).unwrap();
    fixture.popup.hide();
    fixture.popup.show(Theme::Light).unwrap();
    assert_eq!(fixture.display.state.lock().calls, 1);
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    assert!(fixture.sibling.window().is_visible());
    assert_eq!(fixture.opened.get(), 0);
    assert_eq!(fixture.display.state.lock().calls, 2);
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(fixture.popup.is_visible());
    assert_eq!(fixture.opened.get(), 1);
    fixture.popup.refresh_display();
    fixture.popup.refresh_display();
    fixture.popup.refresh_display();
    assert_eq!(fixture.display.state.lock().calls, 3);
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert_eq!(
        fixture.display.state.lock().calls,
        4,
        "one dirty follow-up, not one per notification"
    );
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    fixture.drain();
    assert_eq!(
        fixture.display.state.lock().calls,
        4,
        "quiet capability must not be polled"
    );
    assert_eq!(fixture.display.state.lock().maximum_active, 1);
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn close_discards_delayed_read_without_resurrection_or_sibling_dismissal() {
    let fixture = Fixture::new();
    fixture.popup.show(Theme::Dark).unwrap();
    fixture.popup.close();
    fixture.popup.close();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    fixture.popup.refresh_display();
    fixture.popup.set_theme(Theme::Light);
    fixture.popup.update_motion();
    fixture.lock();
    assert!(fixture.popup.show(Theme::Dark).is_err());
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().window().is_visible());
    assert!(fixture.sibling.window().is_visible());
    assert_eq!(fixture.opened.get(), 0);
    assert!(fixture.results.borrow().is_empty());
    assert_eq!(fixture.display.state.lock().calls, 1);
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn typed_lock_detaches_and_hides_before_getter_and_perform_then_survives_hide_and_reopen() {
    let fixture = Fixture::new();
    fixture.loaded();
    fixture.host.trace.events.lock().clear();
    fixture.lock();
    assert_eq!(
        fixture.events(),
        [
            Event::Detach { visible: true },
            Event::PowerFactory,
            Event::Perform(PowerAction::LockSession),
        ]
    );
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().window().is_visible());
    assert!(
        fixture.results.borrow().is_empty(),
        "initiation is not observed locked state"
    );
    fixture.loaded();
    assert!(fixture.component().get_lock_busy());
    assert!(!fixture.component().get_action_enabled());
    fixture.component().set_action_enabled(true);
    fixture.component().set_lock_busy(false);
    fixture.lock();
    assert_eq!(
        fixture.power.state.lock().calls,
        1,
        "generated properties alone cannot authorize another flight"
    );
    fixture.popup.hide();
    fixture.popup.set_theme(Theme::Light);
    fixture.popup.disable_motion();
    fixture.popup.update_motion();
    fixture.popup.refresh_display();
    fixture.loaded();
    assert!(fixture.component().get_lock_busy());
    assert_eq!(fixture.display.state.lock().maximum_active, 1);
    assert_eq!(fixture.power.state.lock().active, 1);
    finish(&fixture.power.state, Ok(PowerRequestAccepted));
    assert!(fixture.component().get_lock_busy());
    fixture.drain();
    assert!(fixture.popup.is_visible());
    assert!(!fixture.component().get_lock_busy());
    assert!(fixture.component().get_action_enabled());
    assert_eq!(fixture.results.borrow().as_slice(), [Ok(())]);
    assert_eq!(fixture.power.state.lock().calls, 1);
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 1);
}

#[test]
fn accepted_lock_survives_close_but_late_result_is_not_delivered_to_closed_root() {
    let fixture = Fixture::new();
    fixture.loaded();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::Perform, move || weak.upgrade().unwrap().close());
    fixture.lock();
    assert_eq!(fixture.power.state.lock().calls, 1);
    assert_eq!(fixture.power.state.lock().active, 1);
    finish(&fixture.power.state, Err(PowerError::Native { code: 123 }));
    fixture.drain();
    assert!(fixture.results.borrow().is_empty());
    assert!(!fixture.popup.is_visible());
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
    assert!(fixture.popup.show(Theme::Light).is_err());
}

#[test]
fn live_reopened_root_receives_one_safe_failure_without_retrying_native_request() {
    let fixture = Fixture::new();
    fixture.loaded();
    fixture.lock();
    fixture.loaded();
    finish(
        &fixture.power.state,
        Err(PowerError::Native { code: 123456 }),
    );
    fixture.drain();
    fixture.drain();
    assert!(fixture.popup.is_visible());
    assert!(fixture.component().get_action_enabled());
    assert_eq!(fixture.results.borrow().len(), 1);
    let results = fixture.results.borrow();
    let error = results[0].as_ref().unwrap_err();
    assert!(error.starts_with("Power request failed: "));
    assert!(!error.contains("123456"));
    assert_eq!(fixture.power.state.lock().calls, 1);
}

#[test]
fn lease_drop_show_then_hide_replacement_cannot_lend_hidden_authority_to_original_lock() {
    let fixture = Fixture::new();
    fixture.loaded();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::LeaseDrop, move || {
        let popup = weak.upgrade().unwrap();
        popup.show(Theme::Light).unwrap();
        popup.hide();
    });
    fixture.lock();
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.power.state.lock().calls, 0);
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().get_lock_busy());
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    assert_eq!(fixture.opened.get(), 1);
    assert!(fixture.results.borrow().is_empty());
}

#[test]
fn lease_drop_presented_replacement_owns_visibility_and_original_lock_never_enters_native() {
    let fixture = Fixture::new();
    fixture.loaded();
    let weak = Rc::downgrade(&fixture.popup);
    let display = fixture.display.clone();
    hook(Hook::LeaseDrop, move || {
        let popup = weak.upgrade().unwrap();
        popup.show(Theme::Light).unwrap();
        finish(&display.state, Ok(Some(layout())));
        popup.process_events();
    });
    fixture.lock();
    assert!(fixture.popup.is_visible());
    assert!(fixture.component().window().is_visible());
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 1);
    assert!(!fixture.component().get_lock_busy());
    assert!(fixture.component().get_action_enabled());
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.power.state.lock().calls, 0);
    assert_eq!(fixture.opened.get(), 2);
}

#[test]
fn power_factory_reentry_show_then_hide_invalidates_unsubmitted_lock_but_caches_success() {
    let fixture = Fixture::new();
    fixture.loaded();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::PowerFactory, move || {
        let popup = weak.upgrade().unwrap();
        popup.show(Theme::Light).unwrap();
        popup.hide();
    });
    fixture.lock();
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.power.state.lock().calls, 0);
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    fixture.loaded();
    fixture.lock();
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.power.state.lock().calls, 1);
}

#[test]
fn power_factory_close_reentry_never_enters_provider_or_reports_old_intent() {
    let fixture = Fixture::new();
    fixture.loaded();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::PowerFactory, move || weak.upgrade().unwrap().close());
    fixture.lock();
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.power.state.lock().calls, 0);
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().window().is_visible());
    assert!(fixture.results.borrow().is_empty());
}

#[test]
fn display_factory_close_reentry_never_submits_read_or_dismisses_sibling() {
    let fixture = Fixture::new();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::DisplayFactory, move || {
        weak.upgrade().unwrap().close()
    });
    assert!(!fixture.popup.show(Theme::Dark).unwrap());
    fixture.drain();
    assert_eq!(fixture.host.display_getters.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.display.state.lock().calls, 0);
    assert_eq!(fixture.opened.get(), 0);
    assert!(fixture.sibling.window().is_visible());
    assert!(fixture.results.borrow().is_empty());
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn display_factory_reentry_replaces_reserved_read_without_stale_provider_entry() {
    let fixture = Fixture::new();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::DisplayFactory, move || {
        let popup = weak.upgrade().unwrap();
        popup.hide();
        popup.show(Theme::Light).unwrap();
    });
    assert!(!fixture.popup.show(Theme::Dark).unwrap());
    assert_eq!(fixture.display.state.lock().calls, 0);
    fixture.drain();
    assert_eq!(fixture.display.state.lock().calls, 1);
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(fixture.popup.is_visible());
    assert_eq!(fixture.opened.get(), 1);
    assert_eq!(fixture.host.display_getters.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn submitted_read_reentry_reopen_waits_for_old_flight_and_never_overlaps() {
    let fixture = Fixture::new();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::Read, move || {
        let popup = weak.upgrade().unwrap();
        popup.hide();
        popup.show(Theme::Light).unwrap();
        popup.refresh_display();
    });
    assert!(!fixture.popup.show(Theme::Dark).unwrap());
    assert_eq!(fixture.display.state.lock().calls, 1);
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    assert_eq!(fixture.display.state.lock().calls, 2);
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(fixture.popup.is_visible());
    assert_eq!(fixture.display.state.lock().maximum_active, 1);
    assert_eq!(fixture.opened.get(), 1);
}

#[test]
fn configure_cancellation_releases_late_lease_before_hide_and_does_not_focus_or_open() {
    let fixture = Fixture::new();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::Configure, move || {
        let popup = weak.upgrade().unwrap();
        popup.hide();
        assert!(
            popup.component().window().is_visible(),
            "late lease still owns shown SDK window"
        );
    });
    fixture.popup.show(Theme::Dark).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().window().is_visible());
    assert!(fixture.events().contains(&Event::Detach { visible: true }));
    assert!(!fixture.events().contains(&Event::Focus));
    assert_eq!(fixture.opened.get(), 0);
    assert!(fixture.sibling.window().is_visible());
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
    assert!(fixture.results.borrow().is_empty());
    fixture.lock();
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn configure_reentrant_show_cancels_old_native_effect_and_only_fresh_read_presents_replacement() {
    let fixture = Fixture::new();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::Configure, move || {
        assert!(weak.upgrade().unwrap().show(Theme::Light).unwrap());
    });
    fixture.popup.show(Theme::Dark).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().window().is_visible());
    assert!(fixture.sibling.window().is_visible());
    assert_eq!(fixture.opened.get(), 0);
    assert_eq!(fixture.display.state.lock().calls, 2);
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
    assert!(fixture.events().contains(&Event::Detach { visible: true }));
    assert!(!fixture.events().contains(&Event::Focus));
    assert!(fixture.results.borrow().is_empty());
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(fixture.popup.is_visible());
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.opened.get(), 1);
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|event| **event == Event::Attach)
            .count(),
        2
    );
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|event| **event == Event::Focus)
            .count(),
        1
    );
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn genuine_viewport_property_reentry_during_presentation_cancels_stale_projection() {
    let fixture = Fixture::new();
    let weak = Rc::downgrade(&fixture.popup);
    let called = Rc::new(Cell::new(false));
    let recorded = called.clone();
    fixture.component().on_viewport_changed(move || {
        if !recorded.replace(true) {
            weak.upgrade().unwrap().hide();
        }
    });
    // The real presentation changes generated width/height. If the SDK defers
    // the changed binding, evaluate that pending property callback in configure.
    hook(Hook::Configure, || {
        i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    });
    fixture.popup.show(Theme::Dark).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(
        called.get(),
        "test must cross a genuine generated property callback"
    );
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().window().is_visible());
    assert_eq!(fixture.opened.get(), 0);
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
    assert!(!fixture.events().contains(&Event::Focus));
    fixture.lock();
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn current_root_scale_after_configure_and_viewport_refit_preserve_lease_focus_and_read_count() {
    let fixture = Fixture::new();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::Configure, move || {
        let popup = weak.upgrade().unwrap();
        let component = popup.component();
        component
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
    });
    fixture.loaded();
    let root = fixture.component();
    assert_eq!(root.window().scale_factor(), 2.0);
    assert_eq!(root.window().size(), PhysicalSize::new(3840, 1320));
    assert_eq!(root.get_metric_scale(), 0.75);
    assert_eq!(root.get_selected_y(), 120.0);
    assert_eq!(root.get_selected_width(), 960.0);
    assert_eq!(root.get_selected_height(), 540.0);
    let events = fixture.events();
    root.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.25 });
    root.invoke_viewport_changed();
    root.invoke_viewport_changed();
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    assert_eq!(root.window().size(), PhysicalSize::new(3840, 1320));
    assert_eq!(root.get_metric_scale(), 1.2);
    assert_eq!(root.get_selected_y(), 192.0);
    assert_eq!(root.get_selected_width(), 1536.0);
    assert_eq!(root.get_selected_height(), 864.0);
    fixture.popup.set_theme(Theme::Light);
    fixture.popup.disable_motion();
    fixture.popup.update_motion();
    assert_eq!(
        fixture.events(),
        events,
        "refit/theme must not attach, show, focus or reread"
    );
    assert_eq!(fixture.display.state.lock().calls, 1);
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.opened.get(), 1);
    assert_eq!(fixture.power.state.lock().calls, 0);
    assert_eq!(
        root.global::<Palette>().get_color_scheme(),
        slint::language::ColorScheme::Light
    );
    assert_eq!(
        root.global::<SeelenPalette>().get_color_scheme(),
        slint::language::ColorScheme::Light
    );
}

#[test]
fn opened_and_focus_reentry_do_not_reauthorize_hidden_or_closed_surface() {
    let fixture = Fixture::new();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::Opened, move || weak.upgrade().unwrap().hide());
    fixture.popup.show(Theme::Light).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.events().contains(&Event::Focus));
    fixture.lock();
    assert_eq!(fixture.power.state.lock().calls, 0);
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::Focus, move || weak.upgrade().unwrap().close());
    fixture.popup.show(Theme::Dark).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().window().is_visible());
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
    fixture.lock();
    assert_eq!(fixture.power.state.lock().calls, 0);
    assert!(fixture.results.borrow().is_empty());
}

#[test]
fn denied_foreground_does_not_immediately_destroy_unfocused_native_surface() {
    let fixture = Fixture::new();
    fixture.host.deny_focus.store(true, Ordering::Relaxed);
    fixture.loaded();
    assert!(fixture.popup.is_visible());
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.results.borrow().len(), 1);
    assert!(
        fixture.results.borrow()[0]
            .as_ref()
            .unwrap_err()
            .contains("focus")
    );
    // Native focus probing uses the winit HWND/window seam. The SDK testing
    // adapter has no winit window; do not mistake a synthetic active event for
    // evidence that Windows granted foreground or observed a subsequent blur.
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(300));
    assert!(fixture.popup.is_visible());
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn disabled_and_native_hidden_injected_generated_action_have_no_power_authority() {
    let fixture = Fixture::new();
    fixture.loaded();
    fixture.component().set_action_enabled(false);
    fixture.click_lock();
    fixture.lock();
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
    fixture.component().set_action_enabled(true);
    fixture.component().hide().unwrap();
    assert!(!fixture.popup.is_visible());
    fixture.lock();
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.power.state.lock().calls, 0);
    fixture.popup.hide();
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
}

#[test]
fn refresh_of_externally_native_hidden_surface_retires_lease_without_resurrecting_until_explicit_show()
 {
    let fixture = Fixture::new();
    fixture.loaded();
    fixture.component().hide().unwrap();
    assert!(!fixture.popup.is_visible());
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 1);
    fixture.popup.refresh_display();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().window().is_visible());
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
    assert!(fixture.events().contains(&Event::Detach { visible: false }));
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|event| **event == Event::Attach)
            .count(),
        1
    );
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|event| **event == Event::Focus)
            .count(),
        1
    );
    assert_eq!(fixture.opened.get(), 1);
    assert_eq!(fixture.results.borrow().len(), 1);
    fixture.lock();
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.power.state.lock().calls, 0);
    fixture.loaded();
    assert!(fixture.popup.is_visible());
    assert_eq!(fixture.opened.get(), 2);
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 1);
}

#[test]
fn genuine_pointer_tab_return_and_space_dispatch_one_typed_direct_lock_each_without_confirmation() {
    let fixture = Fixture::new();
    fixture.loaded();
    fixture.click_lock();
    assert_eq!(fixture.power.state.lock().calls, 1);
    assert!(!fixture.popup.is_visible());
    finish(&fixture.power.state, Ok(PowerRequestAccepted));
    fixture.drain();
    fixture.loaded();
    fixture.component().invoke_focus_content();
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    assert_eq!(fixture.power.state.lock().calls, 2);
    assert!(!fixture.popup.is_visible());
    finish(&fixture.power.state, Ok(PowerRequestAccepted));
    fixture.drain();
    fixture.loaded();
    fixture.component().invoke_focus_content();
    fixture.key(Key::Tab);
    let component = fixture.component();
    let window = component.window();
    window.dispatch_event(WindowEvent::KeyPressed {
        text: Key::Space.into(),
    });
    assert_eq!(
        fixture.power.state.lock().calls,
        2,
        "Space must arm, not dispatch on press"
    );
    window.dispatch_event(WindowEvent::KeyReleased {
        text: Key::Space.into(),
    });
    assert_eq!(fixture.power.state.lock().calls, 3);
    assert!(!fixture.popup.is_visible());
    assert_eq!(
        fixture.opened.get(),
        3,
        "only explicit opens, no confirmation popup"
    );
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 1);
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|event| matches!(event, Event::Perform(_)))
            .count(),
        3
    );
    assert_eq!(fixture.power.state.lock().maximum_active, 1);
}

#[test]
fn placement_uses_signed_desktop_origin_selected_full_monitor_and_independent_fractional_scales() {
    let layout = DisplayLayout::new(
        Rect::new(-2560, -1440, 4480, 2520).unwrap(),
        Rect::new(-1280, -720, 1920, 1080).unwrap(),
        1.375,
        DisplaySelection::FirstFallback,
    )
    .unwrap();
    assert_eq!(
        Frame::from_layout(layout).unwrap(),
        Frame {
            position: PhysicalPosition::new(-2560, -1440),
            size: PhysicalSize::new(4480, 2520),
        }
    );
    assert_eq!(
        LogicalFit::new(layout, 2.0).unwrap(),
        LogicalFit {
            x: 640.0,
            y: 360.0,
            width: 960.0,
            height: 540.0,
            metric: 0.6875,
        }
    );
    assert_eq!(
        LogicalFit::new(layout, 1.25).unwrap(),
        LogicalFit {
            x: 1024.0,
            y: 576.0,
            width: 1536.0,
            height: 864.0,
            metric: 1.1,
        }
    );
}

#[test]
fn tiny_monitor_is_not_anchored_or_rejected_and_invalid_metrics_never_fall_back() {
    let tiny = DisplayLayout::new(
        Rect::new(-1, -1, 2, 2).unwrap(),
        Rect::new(-1, -1, 1, 1).unwrap(),
        1.5,
        DisplaySelection::FirstFallback,
    )
    .unwrap();
    assert_eq!(
        Frame::from_layout(tiny).unwrap().size,
        PhysicalSize::new(2, 2)
    );
    assert_eq!(
        LogicalFit::new(tiny, 2.0).unwrap(),
        LogicalFit {
            x: 0.0,
            y: 0.0,
            width: 0.5,
            height: 0.5,
            metric: 0.75,
        }
    );
    for scale in [
        0.0,
        -1.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::from_bits(1),
    ] {
        assert!(LogicalFit::new(layout(), scale).is_err());
    }
    let rect = Rect::new(0, 0, 1, 1).unwrap();
    let too_large = DisplayLayout::new(rect, rect, f64::MAX, DisplaySelection::Primary).unwrap();
    assert!(LogicalFit::new(too_large, 1.0).is_err());
    let too_small =
        DisplayLayout::new(rect, rect, f64::from_bits(1), DisplaySelection::Primary).unwrap();
    assert!(LogicalFit::new(too_small, 1.0).is_err());
    let downstream_overflow = DisplayLayout::new(
        rect,
        rect,
        f64::from(f32::MAX) / 256.0,
        DisplaySelection::Primary,
    )
    .unwrap();
    assert!(
        LogicalFit::new(downstream_overflow, 1.0).is_err(),
        "metric cast alone is not enough"
    );
}

#[test]
fn frame_budget_checks_dimension_and_widened_area_at_exact_boundaries() {
    for (width, height, allowed) in [
        (16384, 1, true),
        (1, 16384, true),
        (8192, 8192, true),
        (16384, 4096, true),
        (16385, 1, false),
        (1, 16385, false),
        (8192, 8193, false),
        (16384, 16384, false),
        (u32::MAX, 1, false),
    ] {
        let rect = Rect::new(i32::MIN, 0, width, height).unwrap();
        let layout = DisplayLayout::new(rect, rect, 1.0, DisplaySelection::Primary).unwrap();
        assert_eq!(
            Frame::from_layout(layout).is_ok(),
            allowed,
            "{width}x{height}"
        );
    }
}

#[test]
fn checked_presentation_and_request_identity_exhaustion_never_wraps_or_reuses_authority() {
    let mut state = State {
        generation: u64::MAX - 1,
        sequence: u64::MAX - 1,
        desired: true,
        ..State::default()
    };
    assert_eq!(state.advance().unwrap(), u64::MAX);
    assert_eq!(
        state.token().unwrap(),
        Token {
            generation: u64::MAX,
            sequence: u64::MAX,
            epoch: 0,
        }
    );
    assert!(state.advance().is_err());
    assert!(state.token().is_err());
    assert_eq!(state.generation, u64::MAX);
    assert_eq!(state.sequence, u64::MAX);
    assert!(state.current(u64::MAX));
    assert!(!state.current(0));
    state.closed = true;
    assert!(!state.current(u64::MAX));
}

#[test]
fn three_slot_mailbox_rejects_stale_tokens_and_keeps_only_first_terminal_for_each_flight() {
    let fixture = Fixture::new();
    let read = Token {
        generation: 1,
        sequence: 1,
        epoch: 1,
    };
    let lock = Token {
        generation: 2,
        sequence: 2,
        epoch: 1,
    };
    let stale = Token {
        generation: 0,
        sequence: 0,
        epoch: 0,
    };
    let updates = Token {
        generation: 3,
        sequence: 3,
        epoch: 1,
    };
    let mailbox = Arc::new(Mutex::new(Mailbox::default()));
    mailbox
        .lock()
        .install(1, fixture.component().as_weak())
        .unwrap();
    mailbox.lock().read.expected = Some(read);
    mailbox.lock().lock.expected = Some(lock);
    mailbox.lock().updates.expected = Some(updates);
    complete_read(&mailbox, stale, Ok(Some(layout())));
    complete_lock(&mailbox, stale, Ok(PowerRequestAccepted));
    complete_updates(&mailbox, stale, Ok(PowerUpdateHint::Pending));
    assert!(mailbox.lock().read.terminal.is_none());
    assert!(mailbox.lock().lock.terminal.is_none());
    assert!(mailbox.lock().updates.terminal.is_none());
    complete_read(&mailbox, read, Ok(None));
    complete_lock(&mailbox, lock, Err(PowerError::Busy));
    complete_updates(&mailbox, updates, Ok(PowerUpdateHint::NotDetected));
    for _ in 0..128 {
        complete_read(&mailbox, read, Ok(Some(layout())));
        complete_lock(&mailbox, lock, Ok(PowerRequestAccepted));
        complete_updates(&mailbox, updates, Ok(PowerUpdateHint::Pending));
        complete_updates(&mailbox, stale, Err(PowerUpdatesError::Unavailable));
        complete_read(&mailbox, stale, Err(DisplayContextError::Unavailable));
    }
    let slots = mailbox.lock();
    assert_eq!(slots.read.terminal, Some((read, Ok(None))));
    assert_eq!(slots.lock.terminal, Some((lock, Err(PowerError::Busy))));
    assert_eq!(
        slots.updates.terminal,
        Some((updates, Ok(PowerUpdateHint::NotDetected)))
    );
    assert_eq!(slots.read.expected, Some(read));
    assert_eq!(slots.lock.expected, Some(lock));
    assert_eq!(slots.updates.expected, Some(updates));
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn exhausted_actor_request_identity_closes_visible_scope_without_native_lock_entry() {
    let fixture = Fixture::new();
    fixture.loaded();
    fixture.popup.state.borrow_mut().sequence = u64::MAX;
    fixture.lock();
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().window().is_visible());
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.power.state.lock().calls, 0);
    assert!(fixture.popup.show(Theme::Light).is_err());
    assert_eq!(fixture.results.borrow().len(), 1);
    assert!(
        fixture.results.borrow()[0]
            .as_ref()
            .unwrap_err()
            .contains("exhausted")
    );
}

#[test]
fn actual_generated_show_error_releases_presentation_and_preserves_sibling_without_native_entry() {
    use slint::platform::software_renderer::SoftwareRenderer;
    use slint::platform::{Platform, Renderer, WindowAdapter};

    struct RecordingPlatform {
        adapters: Rc<RefCell<Vec<Rc<RecordingWindow>>>>,
    }
    struct RecordingWindow {
        window: slint::Window,
        renderer: SoftwareRenderer,
        position: Cell<PhysicalPosition>,
        size: Cell<PhysicalSize>,
        reject_show: bool,
        visibility: RefCell<Vec<bool>>,
    }
    impl WindowAdapter for RecordingWindow {
        fn window(&self) -> &slint::Window {
            &self.window
        }
        fn renderer(&self) -> &dyn Renderer {
            &self.renderer
        }
        fn size(&self) -> PhysicalSize {
            self.size.get()
        }
        fn position(&self) -> Option<PhysicalPosition> {
            Some(self.position.get())
        }
        fn set_position(&self, position: slint::WindowPosition) {
            self.position
                .set(position.to_physical(self.window.scale_factor()));
        }
        fn set_size(&self, size: slint::WindowSize) {
            let physical = size.to_physical(self.window.scale_factor());
            self.size.set(physical);
            self.window.dispatch_event(WindowEvent::Resized {
                size: physical.to_logical(self.window.scale_factor()),
            });
        }
        fn set_visible(&self, visible: bool) -> Result<(), slint::PlatformError> {
            self.visibility.borrow_mut().push(visible);
            if visible && self.reject_show {
                Err("recording SDK show failure".to_string().into())
            } else {
                Ok(())
            }
        }
    }
    impl Platform for RecordingPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            // The fixture creates one existing sibling before the Power surface.
            let reject_show = !self.adapters.borrow().is_empty();
            let adapter = Rc::<RecordingWindow>::new_cyclic(|weak| RecordingWindow {
                window: slint::Window::new(weak.clone()),
                renderer: SoftwareRenderer::new(),
                position: Cell::new(PhysicalPosition::new(0, 0)),
                size: Cell::new(PhysicalSize::new(640, 480)),
                reject_show,
                visibility: RefCell::new(Vec::new()),
            });
            self.adapters.borrow_mut().push(adapter.clone());
            Ok(adapter)
        }
    }
    let adapters = Rc::new(RefCell::new(Vec::new()));
    slint::platform::set_platform(Box::new(RecordingPlatform {
        adapters: adapters.clone(),
    }))
    .unwrap();
    let fixture = Fixture::on_installed_platform();
    fixture.popup.show(Theme::Dark).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().window().is_visible());
    assert!(fixture.sibling.window().is_visible());
    assert_eq!(fixture.opened.get(), 0);
    assert_eq!(fixture.results.borrow().len(), 1);
    assert!(
        fixture.results.borrow()[0]
            .as_ref()
            .unwrap_err()
            .contains("present")
    );
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
    assert!(!fixture.events().contains(&Event::Attach));
    assert!(!fixture.events().contains(&Event::Focus));
    fixture.lock();
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.power.state.lock().calls, 0);
    let adapters = adapters.borrow();
    assert_eq!(adapters.len(), 2);
    let power = &adapters[1];
    assert_eq!(power.position.get(), PhysicalPosition::new(-1920, -240));
    assert_eq!(power.size.get(), PhysicalSize::new(3840, 1320));
    assert!(
        power.visibility.borrow().contains(&true),
        "real Slint show reached the recording adapter"
    );
    assert_eq!(power.visibility.borrow().last(), Some(&false));
}

#[test]
fn configure_motion_off_reentry_overrides_cached_permission_without_replaying_presentation() {
    let fixture = Fixture::new();
    fixture.host.animations.store(true, Ordering::Relaxed);
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::Configure, move || {
        weak.upgrade().unwrap().disable_motion()
    });
    fixture.loaded();
    let component = fixture.component();
    let motion = component.global::<PopoverMotion>();
    assert!(
        !motion.get_enabled(),
        "cached host permission must not override newer motion-off"
    );
    assert!(motion.get_presented());
    assert!(fixture.popup.is_visible());
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.opened.get(), 1);
    let events = fixture.events();
    fixture.popup.update_motion();
    fixture.popup.set_theme(Theme::Light);
    assert!(
        !motion.get_enabled(),
        "permission becoming true does not replay existing motion"
    );
    assert_eq!(fixture.events(), events);
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|event| **event == Event::Attach)
            .count(),
        1
    );
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|event| **event == Event::Focus)
            .count(),
        1
    );
    assert_eq!(fixture.display.state.lock().calls, 1);
    assert_eq!(fixture.power.state.lock().calls, 0);
}
