// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::Toolbar;
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{LogicalPosition, Model};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tessera_system::input_language::{
    InputLanguage, InputLanguageActionCompletion, InputLanguageReadCompletion, InputProfile,
};

type Hook = RefCell<Option<Box<dyn FnOnce()>>>;
thread_local! {
    static FACTORY_HOOK: Hook = RefCell::default();
    static READ_HOOK: Hook = RefCell::default();
    static ACTION_HOOK: Hook = RefCell::default();
    static FOCUS_HOOK: Hook = RefCell::default();
    static DETACH_HOOK: Hook = RefCell::default();
}
fn run_hook(hook: &'static std::thread::LocalKey<Hook>) {
    if let Some(callback) = hook.with(|hook| hook.borrow_mut().take()) {
        callback();
    }
}

enum Reply<T> {
    Pending,
    Inline(T),
    Reject(InputLanguageError),
}
#[derive(Default)]
struct RecordingInput {
    reads: AtomicUsize,
    actions: Mutex<Vec<InputLanguageAction>>,
    read_completion: Mutex<Option<InputLanguageReadCompletion>>,
    action_completion: Mutex<Option<InputLanguageActionCompletion>>,
    read_replies: Mutex<VecDeque<Reply<Result<InputLanguageSnapshot, InputLanguageError>>>>,
    action_replies: Mutex<VecDeque<Reply<Result<InputLanguageOutcome, InputLanguageError>>>>,
}
impl RecordingInput {
    fn finish_read(&self, result: Result<InputLanguageSnapshot, InputLanguageError>) {
        let completion = self.read_completion.lock().take().expect("accepted read");
        completion(result);
    }
    fn finish_action(&self, result: Result<InputLanguageOutcome, InputLanguageError>) {
        let completion = self
            .action_completion
            .lock()
            .take()
            .expect("accepted action");
        completion(result);
    }
    fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }
    fn assert_idle(&self) {
        assert!(self.read_completion.lock().is_none());
        assert!(self.action_completion.lock().is_none());
    }
}
impl InputLanguageHost for RecordingInput {
    fn read(&self, completion: InputLanguageReadCompletion) -> Result<(), InputLanguageError> {
        self.assert_idle();
        self.reads.fetch_add(1, Ordering::Relaxed);
        match self
            .read_replies
            .lock()
            .pop_front()
            .unwrap_or(Reply::Pending)
        {
            Reply::Pending => *self.read_completion.lock() = Some(completion),
            Reply::Inline(result) => completion(result),
            Reply::Reject(error) => return Err(error),
        }
        run_hook(&READ_HOOK);
        Ok(())
    }
    fn execute(
        &self,
        action: InputLanguageAction,
        completion: InputLanguageActionCompletion,
    ) -> Result<(), InputLanguageError> {
        self.assert_idle();
        self.actions.lock().push(action);
        match self
            .action_replies
            .lock()
            .pop_front()
            .unwrap_or(Reply::Pending)
        {
            Reply::Pending => *self.action_completion.lock() = Some(completion),
            Reply::Inline(result) => completion(result),
            Reply::Reject(error) => return Err(error),
        }
        run_hook(&ACTION_HOOK);
        Ok(())
    }
}

struct RecordingDesktop {
    provider: Mutex<Result<Option<Arc<dyn InputLanguageHost>>, InputLanguageError>>,
    provider_calls: AtomicUsize,
    root: Mutex<Option<slint::Weak<InputLanguageMenu>>>,
    events: Arc<Mutex<Vec<&'static str>>>,
    deny_focus: AtomicBool,
    cancel_attachment: AtomicBool,
    deny_attachment: AtomicBool,
}
struct Lease {
    root: slint::Weak<InputLanguageMenu>,
    events: Arc<Mutex<Vec<&'static str>>>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(root) = self.root.upgrade() {
            assert!(
                root.window().is_visible(),
                "native lease must release before hide"
            );
        }
        self.events.lock().push("detach");
        run_hook(&DETACH_HOOK);
    }
}
impl DesktopHost for RecordingDesktop {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("input selector cannot observe desktop/catalog");
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("input selector cannot activate windows");
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("input selector cannot launch arbitrary apps");
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("input selector cannot invoke cross-domain actions");
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("input selector cannot save preferences");
    }
    fn subscribe(&self, _: Arc<dyn Fn() + Send + Sync>) -> Result<Option<Box<dyn Send>>, String> {
        panic!("input selector cannot subscribe to desktop/catalog");
    }
    fn shell_identity(&self) -> Result<crate::ShellIdentity, String> {
        panic!("toolbar identity is not selector metadata");
    }
    fn clock_text(&self) -> Result<String, String> {
        panic!("input selector cannot read clock");
    }
    fn input_language_host(
        &self,
    ) -> Result<Option<Arc<dyn InputLanguageHost>>, InputLanguageError> {
        self.provider_calls.fetch_add(1, Ordering::Relaxed);
        run_hook(&FACTORY_HOOK);
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
        }
        Ok(Some(Box::new(Lease {
            root: self.root.lock().clone().unwrap(),
            events: self.events.clone(),
        })))
    }
    fn request_ui_focus(&self, window: &slint::Window) -> Result<(), String> {
        assert!(window.is_visible());
        self.events.lock().push("focus");
        run_hook(&FOCUS_HOOK);
        if self.deny_focus.load(Ordering::Relaxed) {
            Err("foreground denied".into())
        } else {
            Ok(())
        }
    }
}

struct Fixture {
    host: Arc<RecordingDesktop>,
    input: Arc<RecordingInput>,
    source: Toolbar,
    popup: Rc<InputLanguageController>,
    tooltips: Rc<RefCell<Vec<String>>>,
}
impl Fixture {
    fn new() -> Self {
        i_slint_backend_testing::init_no_event_loop();
        let input = Arc::new(RecordingInput::default());
        let host = Arc::new(RecordingDesktop {
            provider: Mutex::new(Ok(Some(input.clone()))),
            provider_calls: AtomicUsize::new(0),
            root: Mutex::default(),
            events: Arc::new(Mutex::default()),
            deny_focus: AtomicBool::new(false),
            cancel_attachment: AtomicBool::new(false),
            deny_attachment: AtomicBool::new(false),
        });
        let source = Toolbar::new().unwrap();
        source.set_language("日本語".into());
        source.set_clock("Observed clock".into());
        source
            .window()
            .set_position(PhysicalPosition::new(-1400, -900));
        source.window().set_size(PhysicalSize::new(600, 32));
        source.show().unwrap();
        let popup = InputLanguageController::new(host.clone()).unwrap();
        *host.root.lock() = Some(popup.component().as_weak());
        let weak = Rc::downgrade(&popup);
        let source_weak = source.as_weak();
        source.on_keyboard_requested(move |bounds| {
            if let (Some(popup), Some(source)) = (weak.upgrade(), source_weak.upgrade()) {
                popup.show(source.window(), bounds, context()).unwrap();
            }
        });
        let tooltips = Rc::new(RefCell::new(Vec::new()));
        let recorded = tooltips.clone();
        source.on_tooltip_requested(move |text, _| recorded.borrow_mut().push(text.to_string()));
        Self {
            host,
            input,
            source,
            popup,
            tooltips,
        }
    }
    fn show(&self) -> Result<(), String> {
        self.popup.show(self.source.window(), bounds(), context())
    }
    fn loaded(&self) {
        self.show().unwrap();
        self.input.finish_read(Ok(snapshot(&[true, false, true])));
        assert!(
            self.popup.component().get_loading(),
            "provider callback cannot project inline"
        );
        self.drain();
    }
    fn drain(&self) {
        self.popup.component().invoke_input_event_ready();
    }
    fn row(&self, index: usize) -> InputProfileRow {
        self.popup.component().get_rows().row_data(index).unwrap()
    }
    fn element(&self, label: &str) -> ElementHandle {
        ElementHandle::find_by_accessible_label(self.popup.component(), label)
            .find(|element| element.accessible_role() == Some(AccessibleRole::Button))
            .unwrap_or_else(|| panic!("missing input-language button: {label}"))
    }
    fn click(&self, label: &str) {
        dispatch_click(self.popup.component().window(), &self.element(label));
    }
    fn key(&self, key: Key) {
        self.popup
            .component()
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        self.popup
            .component()
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }
}
fn dispatch_click(window: &slint::Window, element: &ElementHandle) {
    let p = element.absolute_position();
    let size = element.size();
    assert!(size.width > 0.0 && size.height > 0.0);
    let position = LogicalPosition::new(p.x + size.width / 2.0, p.y + size.height / 2.0);
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}
fn bounds() -> TileBounds {
    TileBounds {
        origin: LogicalPosition::new(450.0, 8.0),
        width: 50.0,
        height: 16.0,
    }
}
fn context() -> DockContext {
    DockContext::new(-1920, -1080, 1920, 1080, false).unwrap()
}
fn profile_id(index: usize) -> ProfileId {
    ProfileId::new(format!("native-issued/opaque-β/{index}")).unwrap()
}
fn snapshot(active: &[bool]) -> InputLanguageSnapshot {
    InputLanguageSnapshot {
        languages: vec![
            InputLanguage {
                id: "native-language/one".into(),
                code: "ja-JP".into(),
                name: "日本語（日本）".into(),
                native_name: "日本語".into(),
                profiles: active
                    .iter()
                    .take(2)
                    .enumerate()
                    .map(|(index, active)| InputProfile {
                        id: profile_id(index),
                        display_name: ["Observed keyboard", "Observed TIP 日本語"][index].into(),
                        active: *active,
                    })
                    .collect(),
            },
            InputLanguage {
                id: "native-language/two".into(),
                code: "".into(),
                name: "العربية".into(),
                native_name: "".into(),
                profiles: active
                    .iter()
                    .skip(2)
                    .enumerate()
                    .map(|(index, active)| InputProfile {
                        id: profile_id(index + 2),
                        display_name: format!("Observed layout {}", index + 2),
                        active: *active,
                    })
                    .collect(),
            },
        ],
    }
}
fn label(index: usize) -> &'static str {
    [
        "日本語（日本） - Observed keyboard",
        "日本語（日本） - Observed TIP 日本語",
        "العربية - Observed layout 2",
    ][index]
}
fn error(kind: InputLanguageErrorKind) -> InputLanguageError {
    InputLanguageError::new(kind, "secret native path\nC:\\private\\provider.exe")
}
fn advance(ms: u64) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(ms));
    slint::platform::update_timers_and_animations();
}

#[test]
fn genuine_toolbar_pointer_opens_lazy_selector_and_genuine_row_dispatches_opaque_profile() {
    let f = Fixture::new();
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 0);
    assert_eq!(f.input.reads(), 0);
    let keyboard = ElementHandle::find_by_accessible_label(&f.source, "Open keyboard selector")
        .find(|element| element.accessible_role() == Some(AccessibleRole::Button))
        .unwrap();
    dispatch_click(f.source.window(), &keyboard);
    assert!(f.popup.is_open());
    assert!(
        !f.tooltips.borrow().is_empty(),
        "real toolbar hover publishes its tooltip"
    );
    assert_eq!(f.input.reads(), 1);
    assert_eq!(f.popup.component().get_rows().row_count(), 0);
    f.popup
        .component()
        .invoke_select_profile(profile_id(0).as_str().into());
    assert!(f.input.actions.lock().is_empty());
    f.input.finish_read(Ok(snapshot(&[true, false, true])));
    f.drain();
    assert_eq!(f.row(0).language_name.as_str(), "日本語（日本）");
    assert_eq!(f.row(1).layout_name.as_str(), "Observed TIP 日本語");
    assert!(
        f.row(0).active && f.row(2).active,
        "multiple genuine observed flags survive"
    );
    assert_ne!(f.row(1).key.as_str(), profile_id(1).as_str());
    f.click(label(1));
    assert_eq!(
        *f.input.actions.lock(),
        vec![InputLanguageAction::Activate {
            profile: profile_id(1)
        }]
    );
    assert!(
        f.popup.is_open(),
        "source selector stays open after selection"
    );
    assert!(
        f.row(0).active && !f.row(1).active,
        "pending action cannot optimistically highlight"
    );
    assert_eq!(f.element(label(1)).accessible_enabled(), Some(false));
}

#[test]
fn forged_old_profile_and_footer_keys_never_regain_authority() {
    let f = Fixture::new();
    f.loaded();
    let row_key = f.row(1).key;
    let footer_key = f.popup.component().get_settings_key();
    for key in ["1", "Observed TIP 日本語", profile_id(1).as_str()] {
        f.popup.component().invoke_select_profile(key.into());
        f.popup.component().invoke_open_settings(key.into());
    }
    assert!(f.input.actions.lock().is_empty());
    f.click(label(1));
    f.element(label(1)).invoke_accessible_default_action();
    f.popup.component().invoke_select_profile(row_key.clone());
    f.popup.component().invoke_open_settings(footer_key.clone());
    assert_eq!(f.input.actions.lock().len(), 1);
    f.input
        .finish_action(Err(error(InputLanguageErrorKind::ProfileChanged)));
    f.drain();
    assert_eq!(
        f.input.reads(),
        2,
        "action failure still observes partial native effects"
    );
    f.input
        .finish_read(Err(error(InputLanguageErrorKind::Unavailable)));
    f.drain();
    assert_eq!(f.popup.component().get_rows().row_count(), 0);
    f.popup.component().invoke_select_profile(row_key);
    f.popup.component().invoke_open_settings(footer_key);
    assert_eq!(f.input.actions.lock().len(), 1);
    assert!(
        !f.popup.component().get_settings_key().is_empty(),
        "unavailable enumeration does not remove fixed Settings"
    );
}

#[test]
fn postactivation_snapshot_is_observed_and_followed_by_exactly_one_read_not_another_action() {
    let f = Fixture::new();
    f.loaded();
    f.click(label(1));
    f.input
        .finish_action(Ok(InputLanguageOutcome::Snapshot(snapshot(&[
            false, true, false,
        ]))));
    assert!(!f.row(1).active, "delivery waits in mailbox");
    f.drain();
    assert!(f.row(1).active, "fresh native outcome alone owns selection");
    assert!(
        f.row(1).key.is_empty(),
        "post-action observation read disables profile actions"
    );
    assert_eq!(f.input.reads(), 2);
    assert_eq!(f.input.actions.lock().len(), 1);
    f.input.finish_read(Ok(snapshot(&[false, true, false])));
    f.drain();
    assert!(!f.row(1).key.is_empty());
    assert!(f.popup.refresh_timer.running());
}

#[test]
fn footer_is_reachable_during_read_but_serializes_fixed_settings_intent() {
    let f = Fixture::new();
    f.show().unwrap();
    assert_eq!(
        f.element("More keyboard settings").accessible_enabled(),
        Some(true)
    );
    f.click("More keyboard settings");
    assert!(
        f.input.actions.lock().is_empty(),
        "accepted read completes before Settings execute"
    );
    assert!(f.popup.component().get_action_pending());
    f.click("More keyboard settings");
    f.input
        .finish_read(Err(error(InputLanguageErrorKind::Unavailable)));
    f.drain();
    assert_eq!(
        *f.input.actions.lock(),
        vec![InputLanguageAction::OpenKeyboardSettings]
    );
    f.input
        .finish_action(Ok(InputLanguageOutcome::SettingsDispatched));
    f.drain();
    assert_eq!(f.input.reads(), 2);
    f.input.finish_read(Ok(InputLanguageSnapshot::default()));
    f.drain();
    assert!(f.popup.component().get_notice().contains("launch accepted"));
    assert!(!f.popup.component().get_notice().contains("displayed"));
    assert_eq!(
        f.element("More keyboard settings").accessible_enabled(),
        Some(true)
    );
}

#[test]
fn action_failure_notice_survives_fresh_read_and_never_exposes_provider_message() {
    let f = Fixture::new();
    f.loaded();
    f.click(label(1));
    f.input
        .finish_action(Err(error(InputLanguageErrorKind::Unavailable)));
    f.drain();
    assert!(
        f.popup
            .component()
            .get_notice()
            .contains("could not be confirmed")
    );
    f.input.finish_read(Ok(snapshot(&[false, true, false])));
    f.drain();
    assert!(f.row(1).active);
    assert!(
        f.popup
            .component()
            .get_notice()
            .contains("could not be confirmed")
    );
    assert!(!f.popup.component().get_notice().contains("secret"));
    assert_eq!(f.input.actions.lock().len(), 1);
}

#[test]
fn hide_reopen_serializes_old_read_and_ignores_old_delivery_and_mailbox_replay() {
    let f = Fixture::new();
    f.show().unwrap();
    let old = f.popup.state.borrow().flight.as_ref().unwrap().token;
    f.popup.hide();
    f.show().unwrap();
    assert_eq!(f.input.reads(), 1);
    f.input.finish_read(Ok(snapshot(&[true, false, true])));
    f.drain();
    assert_eq!(f.input.reads(), 2);
    assert_eq!(f.popup.component().get_rows().row_count(), 0);
    complete(
        &f.popup.mailbox,
        &f.popup.surface.as_weak(),
        old,
        Outcome::Read(Ok(snapshot(&[true, true, true]))),
    );
    f.drain();
    assert_eq!(f.popup.component().get_rows().row_count(), 0);
    f.input.finish_read(Ok(snapshot(&[false, true, false])));
    f.drain();
    assert!(!f.row(0).active && f.row(1).active);
}

#[test]
fn accepted_action_completion_after_hide_neither_reopens_nor_reads_until_real_open() {
    let f = Fixture::new();
    f.loaded();
    f.click(label(1));
    f.popup.hide();
    f.input
        .finish_action(Ok(InputLanguageOutcome::Snapshot(snapshot(&[
            false, true, false,
        ]))));
    f.drain();
    assert!(!f.popup.is_open());
    assert_eq!(f.input.reads(), 1);
    assert_eq!(f.popup.component().get_rows().row_count(), 0);
    assert!(!f.popup.refresh_timer.running());
    f.show().unwrap();
    assert_eq!(f.input.reads(), 2);
}

#[test]
fn terminal_close_revokes_mailbox_and_all_timers_without_cancelling_accepted_action() {
    let f = Fixture::new();
    f.loaded();
    f.click(label(1));
    f.popup.close();
    assert!(
        !f.popup.fit_timer.running()
            && !f.popup.refresh_timer.running()
            && !f.popup.focus_watch.running()
    );
    assert!(f.popup.mailbox.lock().expected.is_none());
    f.input
        .finish_action(Ok(InputLanguageOutcome::Snapshot(snapshot(&[
            false, true, false,
        ]))));
    f.drain();
    assert!(!f.popup.is_open());
    assert_eq!(f.input.reads(), 1);
    assert!(f.show().is_err());
    assert!(f.popup.state.borrow().snapshot.is_none());
}

#[test]
fn visible_only_one_second_timer_is_read_only_and_does_not_overlap_pending_work() {
    let f = Fixture::new();
    f.loaded();
    advance(999);
    assert_eq!(f.input.reads(), 1);
    advance(1);
    assert_eq!(f.input.reads(), 2);
    assert!(f.row(0).key.is_empty());
    advance(5000);
    assert_eq!(f.input.reads(), 2);
    f.input.finish_read(Ok(snapshot(&[false, true, false])));
    f.drain();
    assert!(f.row(1).active);
    advance(1000);
    assert_eq!(f.input.reads(), 3);
    assert!(f.input.actions.lock().is_empty());
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 1);
    f.popup.hide();
    advance(5000);
    f.input.finish_read(Ok(snapshot(&[true, false, false])));
    f.drain();
    advance(5000);
    assert_eq!(f.input.reads(), 3);
    assert!(!f.popup.refresh_timer.running());
}

#[test]
fn unavailable_empty_and_unknown_active_are_distinct_and_all_retain_settings() {
    let f = Fixture::new();
    f.show().unwrap();
    f.input
        .finish_read(Err(error(InputLanguageErrorKind::Unavailable)));
    f.drain();
    assert!(!f.popup.component().get_observed());
    assert!(f.popup.component().get_retry_enabled());
    assert_eq!(
        f.element("More keyboard settings").accessible_enabled(),
        Some(true)
    );
    f.popup.component().invoke_retry_requested();
    f.input.finish_read(Ok(InputLanguageSnapshot::default()));
    f.drain();
    assert!(f.popup.component().get_observed());
    assert_eq!(f.popup.component().get_rows().row_count(), 0);
    assert!(f.popup.component().get_notice().is_empty());
    advance(1000);
    f.input.finish_read(Ok(snapshot(&[false, false, false])));
    f.drain();
    assert!(f.popup.component().get_unknown_active());
    assert!((0..3).all(|index| !f.row(index).active));
    assert_eq!(
        f.element("More keyboard settings").accessible_enabled(),
        Some(true)
    );
}

#[test]
fn provider_none_and_factory_errors_are_honest_retryable_and_lazy() {
    let f = Fixture::new();
    *f.host.provider.lock() = Ok(None);
    f.show().unwrap();
    assert_eq!(f.input.reads(), 0);
    assert!(f.popup.component().get_notice().contains("not supported"));
    assert!(f.popup.component().get_retry_enabled());
    assert_eq!(
        f.element("More keyboard settings").accessible_enabled(),
        Some(false)
    );
    *f.host.provider.lock() = Err(error(InputLanguageErrorKind::AccessDenied));
    f.click("Retry keyboard profiles");
    assert!(f.popup.component().get_notice().contains("denied"));
    assert!(!f.popup.component().get_notice().contains("private"));
    *f.host.provider.lock() = Ok(Some(f.input.clone()));
    f.click("Retry keyboard profiles");
    assert_eq!(f.input.reads(), 1);
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 3);
}

#[test]
fn inline_and_immediate_rejected_calls_use_same_posted_completion_path() {
    let f = Fixture::new();
    f.input
        .read_replies
        .lock()
        .push_back(Reply::Inline(Ok(snapshot(&[true, false, false]))));
    f.show().unwrap();
    assert!(f.popup.component().get_loading());
    assert_eq!(f.popup.component().get_rows().row_count(), 0);
    f.drain();
    f.input
        .action_replies
        .lock()
        .push_back(Reply::Reject(error(InputLanguageErrorKind::Busy)));
    f.click(label(1));
    assert!(f.popup.component().get_action_pending());
    assert!(f.popup.component().get_notice().is_empty());
    f.drain();
    assert_eq!(f.input.reads(), 2);
    f.input.finish_read(Ok(snapshot(&[true, false, false])));
    f.drain();
    assert!(f.popup.component().get_notice().contains("busy"));
    assert!(!f.row(1).active);
}

#[test]
fn every_open_refreshes_and_reissues_per_session_keys_without_label_normalization() {
    let f = Fixture::new();
    f.loaded();
    let old = f.row(0).key;
    f.popup.hide();
    f.show().unwrap();
    let mut observed = snapshot(&[false, false, false]);
    observed.languages[0].name = "  Unicode\n名前  ".into();
    observed.languages[0].profiles[0].display_name = "raw (layout) — αβ".into();
    f.input.finish_read(Ok(observed));
    f.drain();
    assert_eq!(f.input.reads(), 2);
    assert_ne!(old, f.row(0).key);
    assert_eq!(f.row(0).language_name.as_str(), "  Unicode\n名前  ");
    assert_eq!(f.row(0).layout_name.as_str(), "raw (layout) — αβ");
    f.popup.component().invoke_select_profile(old);
    assert!(f.input.actions.lock().is_empty());
}

#[test]
fn provider_factory_and_action_callbacks_may_reentrantly_hide_and_reopen() {
    let f = Fixture::new();
    let weak = Rc::downgrade(&f.popup);
    FACTORY_HOOK
        .with(|hook| *hook.borrow_mut() = Some(Box::new(move || weak.upgrade().unwrap().hide())));
    f.show().unwrap();
    assert!(!f.popup.is_open());
    assert_eq!(f.input.reads(), 0);
    f.show().unwrap();
    f.input.finish_read(Ok(snapshot(&[true, false, false])));
    f.drain();
    let popup = f.popup.clone();
    let source = f.source.as_weak();
    ACTION_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            popup.hide();
            popup
                .show(source.upgrade().unwrap().window(), bounds(), context())
                .unwrap();
        }))
    });
    f.click(label(1));
    assert_eq!(
        f.input.reads(),
        1,
        "new read waits behind old accepted action"
    );
    f.input
        .finish_action(Ok(InputLanguageOutcome::Snapshot(snapshot(&[
            false, true, false,
        ]))));
    f.drain();
    assert_eq!(f.input.reads(), 2);
    assert_eq!(f.popup.component().get_rows().row_count(), 0);
    assert!(f.popup.is_open());
}

#[test]
fn focus_denial_is_reported_without_fake_focus_or_selector_rollback() {
    let f = Fixture::new();
    f.host.deny_focus.store(true, Ordering::Relaxed);
    let result = f.show().unwrap_err();
    assert!(result.contains("focus was not granted"));
    assert!(f.popup.is_open());
    assert_eq!(f.input.reads(), 1);
    assert_eq!(*f.host.events.lock(), vec!["attach", "focus"]);
}

#[test]
fn native_attachment_cancellation_and_failure_do_not_acquire_provider_or_resurrect() {
    let f = Fixture::new();
    f.host.cancel_attachment.store(true, Ordering::Relaxed);
    f.show().unwrap();
    assert!(!f.popup.is_open());
    assert_eq!(f.input.reads(), 0);
    assert_eq!(*f.host.events.lock(), vec!["attach", "detach"]);
    f.host.cancel_attachment.store(false, Ordering::Relaxed);
    f.host.deny_attachment.store(true, Ordering::Relaxed);
    assert!(f.show().is_err());
    assert!(!f.popup.is_open());
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 0);
}

#[test]
fn geometry_fullscreen_native_close_and_genuine_escape_release_lease_before_hide() {
    let f = Fixture::new();
    f.loaded();
    {
        let rect = f.popup.rect.borrow();
        let rect = rect.as_ref().unwrap();
        assert!(rect.position.x >= -1920 && rect.position.y >= -1080);
        assert!(i64::from(rect.position.x) + i64::from(rect.size.width) <= 0);
    }
    f.popup.component().invoke_focus_content();
    f.key(Key::Tab);
    f.key(Key::Escape);
    assert!(!f.popup.is_open());
    assert_eq!(f.host.events.lock().last(), Some(&"detach"));
    f.show().unwrap();
    f.popup.close_if_geometry_changed(context(), 1.25);
    assert!(!f.popup.is_open());
    f.input.finish_read(Ok(snapshot(&[true, false, false])));
    f.drain();
    f.show().unwrap();
    f.popup.close_if_geometry_changed(
        DockContext::new(-1920, -1080, 1920, 1080, true).unwrap(),
        1.0,
    );
    assert!(!f.popup.is_open());
    f.input.finish_read(Ok(snapshot(&[true, false, false])));
    f.drain();
    f.show().unwrap();
    f.popup
        .component()
        .window()
        .dispatch_event(WindowEvent::CloseRequested);
    assert!(!f.popup.is_open());
}

#[test]
fn lease_drop_can_present_competing_session_without_outer_hide_destroying_it() {
    let f = Fixture::new();
    f.loaded();
    let popup = f.popup.clone();
    let source = f.source.as_weak();
    DETACH_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            popup
                .show(source.upgrade().unwrap().window(), bounds(), context())
                .unwrap();
        }))
    });
    f.popup.hide();
    assert!(f.popup.is_open());
    assert_eq!(f.input.reads(), 2);
    assert_eq!(f.popup.component().get_rows().row_count(), 0);
}

#[test]
fn queued_settings_intention_is_revoked_on_hide_instead_of_becoming_a_late_effect() {
    let f = Fixture::new();
    f.show().unwrap();
    f.click("More keyboard settings");
    assert!(f.popup.state.borrow().pending_action.is_some());
    f.popup.hide();
    f.input.finish_read(Ok(snapshot(&[true, false, false])));
    f.drain();
    assert!(f.input.actions.lock().is_empty());
    assert!(!f.popup.is_open());
    f.show().unwrap();
    assert_eq!(f.input.reads(), 2);
    assert!(f.input.actions.lock().is_empty());
}

#[test]
fn dropping_selector_does_not_join_or_cancel_an_already_accepted_activation() {
    let f = Fixture::new();
    f.loaded();
    f.click(label(1));
    let native = f.input.clone();
    let weak = Rc::downgrade(&f.popup);
    drop(f);
    assert!(weak.upgrade().is_none());
    native.finish_action(Ok(InputLanguageOutcome::Snapshot(snapshot(&[
        false, true, false,
    ]))));
    assert_eq!(native.actions.lock().len(), 1);
    assert_eq!(native.reads(), 1);
}

#[test]
fn genuine_keyboard_repeat_cannot_start_second_action_after_postaction_refresh() {
    let f = Fixture::new();
    f.loaded();
    f.popup.component().invoke_focus_content();
    f.key(Key::Tab);
    let window = f.popup.component().window();
    window.dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    assert_eq!(f.input.actions.lock().len(), 1);
    window.dispatch_event(WindowEvent::KeyPressRepeated {
        text: Key::Return.into(),
    });
    f.input
        .finish_action(Ok(InputLanguageOutcome::Snapshot(snapshot(&[
            true, false, false,
        ]))));
    f.drain();
    f.input.finish_read(Ok(snapshot(&[true, false, false])));
    f.drain();
    f.popup.component().invoke_focus_content();
    f.key(Key::Tab);
    window.dispatch_event(WindowEvent::KeyPressRepeated {
        text: Key::Return.into(),
    });
    window.dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    assert_eq!(
        f.input.actions.lock().len(),
        1,
        "held Return is never a new native intention"
    );
    f.key(Key::Space);
    assert_eq!(
        f.input.actions.lock().len(),
        2,
        "a new genuine Space gesture may act"
    );
}

#[test]
fn known_read_error_kinds_show_fixed_safe_notices_and_keep_fixed_settings_action() {
    let f = Fixture::new();
    for kind in [
        InputLanguageErrorKind::Unsupported,
        InputLanguageErrorKind::AccessDenied,
        InputLanguageErrorKind::Busy,
        InputLanguageErrorKind::Stopped,
        InputLanguageErrorKind::ProfileChanged,
        InputLanguageErrorKind::Unavailable,
        InputLanguageErrorKind::InvalidValue,
        InputLanguageErrorKind::Other,
    ] {
        f.show().unwrap();
        f.input.finish_read(Err(error(kind)));
        f.drain();
        assert_eq!(f.popup.component().get_notice().as_str(), failure(kind));
        assert_eq!(f.popup.component().get_rows().row_count(), 0);
        assert!(f.popup.component().get_retry_enabled());
        assert_eq!(
            f.element("More keyboard settings").accessible_enabled(),
            Some(true)
        );
    }
}

#[test]
fn negative_monitor_native_fit_respects_actual_scale_for_every_real_open() {
    let f = Fixture::new();
    for scale in [1.0_f32, 1.25, 1.5, 2.0] {
        f.source
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        f.show().unwrap();
        f.input.finish_read(Ok(snapshot(&[true, false, true])));
        f.drain();
        {
            let rect = f.popup.rect.borrow();
            let rect = rect.as_ref().unwrap();
            assert_eq!(rect.size.width, (320.0 * scale).ceil() as u32);
            assert!(rect.position.x >= -1920 && rect.position.y >= -1080);
            assert!(i64::from(rect.position.x) + i64::from(rect.size.width) <= 0);
            assert!(i64::from(rect.position.y) + i64::from(rect.size.height) <= 0);
        }
        f.popup.hide();
    }
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 1);
    assert_eq!(f.input.reads(), 4);
}

#[test]
fn invalid_source_bounds_hidden_source_and_fullscreen_never_acquire_input_provider() {
    let f = Fixture::new();
    let mut invalid = bounds();
    invalid.width = f32::NAN;
    assert!(f.popup.show(f.source.window(), invalid, context()).is_err());
    assert!(
        f.popup
            .show(
                f.source.window(),
                bounds(),
                DockContext::new(-1920, -1080, 1920, 1080, true).unwrap()
            )
            .is_err()
    );
    f.source.hide().unwrap();
    assert!(f.show().is_err());
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 0);
    assert_eq!(f.input.reads(), 0);
    assert!(!f.popup.is_open());
}
