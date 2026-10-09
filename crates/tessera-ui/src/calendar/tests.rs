// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::Panel;
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use i_slint_backend_testing::ElementHandle;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{LogicalPosition, Model, PhysicalSize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tessera_system::calendar::{CalendarReadCompletion, WeekStart};

type Hook = RefCell<Option<Box<dyn FnOnce()>>>;
thread_local! {
    static FACTORY_HOOK: Hook = RefCell::default();
    static READ_HOOK: Hook = RefCell::default();
    static FOCUS_HOOK: Hook = RefCell::default();
}

fn run_hook(hook: &'static std::thread::LocalKey<Hook>) {
    let callback = hook.with(|hook| hook.borrow_mut().take());
    if let Some(callback) = callback {
        callback();
    }
}

enum Reply {
    Pending,
    Inline(Box<Result<CalendarSnapshot, CalendarError>>),
    Reject(CalendarError),
}

#[derive(Default)]
struct RecordingCalendar {
    reads: AtomicUsize,
    pending: Mutex<Option<CalendarReadCompletion>>,
    replies: Mutex<VecDeque<Reply>>,
}

impl RecordingCalendar {
    fn finish(&self, result: Result<CalendarSnapshot, CalendarError>) {
        let completion = self.pending.lock().take().expect("accepted calendar read");
        completion(result);
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }
}

impl CalendarHost for RecordingCalendar {
    fn read(&self, completion: CalendarReadCompletion) -> Result<(), CalendarError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        assert!(
            self.pending.lock().is_none(),
            "only one accepted read may exist"
        );
        let reply = self.replies.lock().pop_front().unwrap_or(Reply::Pending);
        match reply {
            Reply::Pending => *self.pending.lock() = Some(completion),
            Reply::Inline(result) => completion(*result),
            Reply::Reject(error) => return Err(error),
        }
        run_hook(&READ_HOOK);
        Ok(())
    }
}

struct RecordingDesktop {
    provider: Mutex<Result<Option<Arc<dyn CalendarHost>>, CalendarError>>,
    provider_calls: AtomicUsize,
    root: Mutex<Option<slint::Weak<CalendarMenu>>>,
    events: Arc<Mutex<Vec<&'static str>>>,
    cancel_attachment: AtomicBool,
    deny_attachment: AtomicBool,
    deny_focus: AtomicBool,
}

struct Lease {
    root: slint::Weak<CalendarMenu>,
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(root) = self.root.upgrade() {
            assert!(
                root.window().is_visible(),
                "detach must precede native hide"
            );
        }
        self.events.lock().push("detach");
    }
}

impl DesktopHost for RecordingDesktop {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("calendar cannot observe catalog")
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("calendar cannot activate windows")
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("calendar cannot launch applications")
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("calendar is read-only")
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("calendar cannot persist")
    }
    fn subscribe(&self, _: Arc<dyn Fn() + Send + Sync>) -> Result<Option<Box<dyn Send>>, String> {
        panic!("calendar cannot subscribe to catalog changes")
    }
    fn clock_text(&self) -> Result<String, String> {
        panic!("clock text is not a calendar date")
    }
    fn shell_identity(&self) -> Result<crate::ShellIdentity, String> {
        panic!("calendar owns its independent provider")
    }

    fn calendar_host(&self) -> Result<Option<Arc<dyn CalendarHost>>, CalendarError> {
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
            assert!(
                window.is_visible(),
                "late lease must drop before cancellation hides"
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
    calendar: Arc<RecordingCalendar>,
    source: Panel,
    popup: Rc<CalendarController>,
}

impl Fixture {
    fn new() -> Self {
        i_slint_backend_testing::init_no_event_loop();
        let calendar = Arc::new(RecordingCalendar::default());
        let host = Arc::new(RecordingDesktop {
            provider: Mutex::new(Ok(Some(calendar.clone()))),
            provider_calls: AtomicUsize::new(0),
            root: Mutex::default(),
            events: Arc::new(Mutex::default()),
            cancel_attachment: AtomicBool::new(false),
            deny_attachment: AtomicBool::new(false),
            deny_focus: AtomicBool::new(false),
        });
        let source = Panel::new().unwrap();
        source
            .window()
            .set_position(PhysicalPosition::new(-1400, 100));
        source.window().set_size(PhysicalSize::new(600, 600));
        source.show().unwrap();
        let popup = CalendarController::new(host.clone()).unwrap();
        *host.root.lock() = Some(popup.component().as_weak());
        popup.apply_theme(PresentationTheme::uniform(
            slint::language::ColorScheme::Dark,
        ));
        Self {
            host,
            calendar,
            source,
            popup,
        }
    }

    fn show(&self) -> Result<(), String> {
        self.popup.show(self.source.window(), bounds(), context())
    }

    fn loaded(&self) {
        self.show().unwrap();
        self.calendar.finish(Ok(snapshot(31)));
        assert!(
            self.popup.component().get_loading(),
            "completion cannot project inline"
        );
        self.drain();
    }

    fn drain(&self) {
        // Headless delivery uses the production mailbox wake, never injected UI data.
        self.popup.component().invoke_calendar_event_ready();
    }

    fn element(&self, label: &str) -> ElementHandle {
        ElementHandle::find_by_accessible_label(self.popup.component(), label)
            .next()
            .unwrap_or_else(|| panic!("missing calendar native input: {label}"))
    }

    fn click(&self, label: &str) {
        let element = self.element(label);
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

    fn navigate(&self, forward: bool) {
        self.popup
            .component()
            .invoke_navigate_requested(forward, self.popup.component().get_action_key());
    }

    fn displayed(&self) -> CivilDate {
        self.popup
            .state
            .borrow()
            .calendar
            .as_ref()
            .unwrap()
            .displayed()
    }
    fn selected(&self) -> CivilDate {
        self.popup
            .state
            .borrow()
            .calendar
            .as_ref()
            .unwrap()
            .selected()
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

fn snapshot(day: u32) -> CalendarSnapshot {
    CalendarSnapshot::new(
        CivilDate::new(2024, 1, day).unwrap(),
        "fixture-calendar-locale".into(),
        std::array::from_fn(|i| format!("Native month {}", i + 1)),
        std::array::from_fn(|i| format!("Native weekday {}", i + 1)),
        std::array::from_fn(|i| format!("W{}", i + 1)),
        WeekStart::Monday,
    )
    .unwrap()
}

fn error(kind: CalendarErrorKind) -> CalendarError {
    CalendarError::new(kind, "secret native error path\nnever display this")
}

fn advance(milliseconds: u64) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(milliseconds));
    slint::platform::update_timers_and_animations();
}

#[test]
fn provider_identity_is_lazy_and_only_confirmed_data_becomes_actionable() {
    let f = Fixture::new();
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 0);
    assert_eq!(f.calendar.reads(), 0);
    f.show().unwrap();
    assert!(f.popup.component().get_loading());
    assert!(f.popup.component().get_action_key().is_empty());
    assert_eq!(f.popup.component().get_weeks().row_count(), 0);
    assert_eq!(f.popup.component().get_weekdays().row_count(), 0);
    f.popup
        .component()
        .invoke_today_requested("2024-01-31".into());
    f.calendar.finish(Ok(snapshot(31)));
    f.drain();
    assert_eq!(f.popup.component().get_title_text(), "Native month 1 2024");
    assert_eq!(
        f.popup.component().get_weekdays().row_data(0).unwrap(),
        "W1"
    );
    assert!(!f.popup.component().get_loading());
    assert!(!f.popup.component().get_action_key().is_empty());
    f.navigate(true);
    assert_eq!(f.displayed(), CivilDate::new(2024, 2, 29).unwrap());
    assert_eq!(f.selected(), CivilDate::new(2024, 1, 31).unwrap());
    assert_eq!(
        f.calendar.reads(),
        1,
        "browsing is pure, not another native read"
    );
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 1);
}

#[test]
fn current_private_projection_keys_not_ui_labels_dates_or_old_models_authorize_actions() {
    let f = Fixture::new();
    f.loaded();
    let root = f.popup.component();
    let stale_action = root.get_action_key();
    let stale_day = root
        .get_weeks()
        .row_data(0)
        .unwrap()
        .days
        .row_data(0)
        .unwrap()
        .key;
    let selected = f.selected();
    root.set_title_text("Forged December 2099".into());
    root.set_action_key("forged-key".into());
    root.invoke_navigate_requested(true, root.get_action_key());
    root.invoke_day_selected("2099-12-31".into());
    root.invoke_month_selected("12".into());
    assert_eq!(f.selected(), selected);
    assert_eq!(f.displayed(), selected);
    root.invoke_navigate_requested(true, stale_action.clone());
    let displayed = f.displayed();
    root.invoke_navigate_requested(true, stale_action);
    root.invoke_day_selected(stale_day);
    assert_eq!(f.displayed(), displayed);
    assert_eq!(f.selected(), selected);
    let (key, date) = f
        .popup
        .state
        .borrow()
        .keys
        .days
        .iter()
        .find(|(_, d)| d.month() != 2)
        .unwrap()
        .clone();
    root.invoke_day_selected(key);
    assert_eq!(f.selected(), date);
    assert_eq!(f.displayed(), date);
    root.invoke_toggle_view_requested(root.get_action_key());
    let month_key = f
        .popup
        .state
        .borrow()
        .keys
        .months
        .iter()
        .find(|(_, m)| *m == 7)
        .unwrap()
        .0
        .clone();
    root.invoke_month_selected(month_key.clone());
    assert_eq!(f.displayed(), CivilDate::new(2024, 7, 1).unwrap());
    assert_eq!(f.selected(), date, "month browsing does not select a date");
    root.invoke_month_selected(month_key);
    let (real_key, real_date) = f.popup.state.borrow().keys.days[0].clone();
    root.set_weeks(ModelRc::new(slint::VecModel::from(vec![CalendarWeekRow {
        days: ModelRc::new(slint::VecModel::from(vec![CalendarDayCell {
            key: real_key.clone(),
            label: "31".into(),
            description: "2099-12-31".into(),
            off_month: false,
            today: true,
            selected: true,
        }])),
    }])));
    root.invoke_day_selected("2099-12-31".into());
    assert_eq!(f.selected(), date);
    root.invoke_day_selected(real_key);
    assert_eq!(
        f.selected(),
        real_date,
        "a projected key maps only to its private date"
    );
    f.popup.projecting.set(true);
    root.invoke_today_requested(root.get_action_key());
    f.popup.projecting.set(false);
    assert_eq!(f.displayed(), CivilDate::new(2024, 7, 1).unwrap());
    f.popup.hide();
    assert!(root.get_action_key().is_empty());
    assert_eq!(root.get_weeks().row_count(), 0);
    root.invoke_toggle_view_requested("anything".into());
    assert!(!f.popup.is_open());
}

#[test]
fn factory_failure_and_rejection_are_safe_retryable_and_never_cached_as_success() {
    let f = Fixture::new();
    *f.host.provider.lock() = Err(error(CalendarErrorKind::Unavailable));
    f.show().unwrap();
    assert!(!f.popup.component().get_loading());
    assert!(f.popup.component().get_retry_enabled());
    assert!(!f.popup.component().get_notice().contains("secret"));
    assert_eq!(f.calendar.reads(), 0);
    advance(120_000);
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 1);
    *f.host.provider.lock() = Ok(None);
    f.popup.component().invoke_retry_requested();
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 2);
    assert!(f.popup.component().get_notice().contains("not supported"));
    *f.host.provider.lock() = Ok(Some(f.calendar.clone()));
    f.calendar
        .replies
        .lock()
        .push_back(Reply::Reject(error(CalendarErrorKind::Busy)));
    f.element("Retry calendar")
        .invoke_accessible_default_action();
    assert!(f.popup.component().get_loading());
    f.drain();
    assert!(f.popup.component().get_notice().contains("busy"));
    f.calendar
        .replies
        .lock()
        .push_back(Reply::Inline(Box::new(Ok(snapshot(31)))));
    f.popup.component().invoke_retry_requested();
    assert!(
        f.popup.component().get_loading(),
        "even inline success uses mailbox"
    );
    f.drain();
    assert!(!f.popup.component().get_loading());
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 3);
    assert_eq!(f.calendar.reads(), 2);
}

#[test]
fn accepted_read_retains_single_flight_across_hide_and_old_completion_cannot_project() {
    let f = Fixture::new();
    f.show().unwrap();
    f.popup.hide();
    advance(120_000);
    assert_eq!(f.calendar.reads(), 1);
    f.show().unwrap();
    f.show().unwrap();
    assert_eq!(f.calendar.reads(), 1);
    assert!(f.popup.component().get_loading());
    assert!(f.popup.component().get_action_key().is_empty());
    let calendar = f.calendar.clone();
    std::thread::spawn(move || calendar.finish(Err(error(CalendarErrorKind::InvalidData))))
        .join()
        .unwrap();
    f.drain();
    assert_eq!(f.calendar.reads(), 2);
    assert!(f.popup.component().get_loading());
    assert!(f.popup.component().get_notice().is_empty());
    f.calendar.finish(Ok(snapshot(30)));
    f.drain();
    assert_eq!(f.selected(), CivilDate::new(2024, 1, 30).unwrap());
    assert!(f.popup.mailbox.lock().completion.is_none());
    assert!(f.popup.mailbox.lock().expected.is_none());
    assert!(!f.popup.mailbox.lock().wake_queued);
}

#[test]
fn visible_minute_refresh_preserves_browse_and_stops_on_hide_or_failure() {
    let f = Fixture::new();
    f.loaded();
    f.navigate(true);
    let displayed = f.displayed();
    let selected = f.selected();
    advance(59_999);
    assert_eq!(f.calendar.reads(), 1);
    advance(1);
    assert_eq!(f.calendar.reads(), 2);
    assert!(f.popup.component().get_loading());
    advance(180_000);
    assert_eq!(f.calendar.reads(), 2, "pending reads cannot overlap");
    let mut fresh = snapshot(1);
    fresh = CalendarSnapshot::new(
        CivilDate::new(2024, 2, 1).unwrap(),
        fresh.locale_name().into(),
        fresh.months().clone(),
        fresh.weekdays_full().clone(),
        fresh.weekdays_abbreviated().clone(),
        fresh.week_start(),
    )
    .unwrap();
    f.calendar.finish(Ok(fresh));
    f.drain();
    assert_eq!(f.displayed(), displayed);
    assert_eq!(f.selected(), selected);
    let today = f
        .popup
        .component()
        .get_weeks()
        .iter()
        .flat_map(|week| week.days.iter().collect::<Vec<_>>())
        .find(|day| day.today)
        .unwrap();
    assert_eq!(today.label, "1");
    advance(60_000);
    f.calendar
        .finish(Err(error(CalendarErrorKind::Unavailable)));
    f.drain();
    assert!(!f.popup.refresh_timer.running());
    advance(180_000);
    assert_eq!(f.calendar.reads(), 3);
    f.popup.hide();
    assert!(!f.popup.refresh_timer.running());
    advance(180_000);
    assert_eq!(f.calendar.reads(), 3);
}

#[test]
fn factory_provider_and_focus_reentry_never_hold_state_borrows_or_duplicate_work() {
    let f = Fixture::new();
    let popup = f.popup.clone();
    let source = f.source.clone_strong();
    FACTORY_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            popup.hide();
            popup.show(source.window(), bounds(), context()).unwrap();
        }))
    });
    let popup = f.popup.clone();
    let source = f.source.clone_strong();
    READ_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            popup.hide();
            popup.show(source.window(), bounds(), context()).unwrap();
        }))
    });
    f.show().unwrap();
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 1);
    assert_eq!(f.calendar.reads(), 1);
    f.calendar.finish(Ok(snapshot(31)));
    f.drain();
    assert_eq!(f.calendar.reads(), 2);
    assert!(f.popup.component().get_loading());
    f.calendar.finish(Ok(snapshot(30)));
    f.drain();
    let popup = f.popup.clone();
    FOCUS_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(move || popup.hide())));
    f.show().unwrap();
    assert!(!f.popup.is_open());
    assert_eq!(f.calendar.reads(), 2);
}

#[test]
fn cancellation_focus_denial_refit_scope_and_geometry_share_native_lease_lifetime() {
    let f = Fixture::new();
    f.host.cancel_attachment.store(true, Ordering::Relaxed);
    f.show().unwrap();
    assert!(!f.popup.is_open());
    assert_eq!(&*f.host.events.lock(), &["attach", "detach"]);
    assert_eq!(f.calendar.reads(), 0);
    f.host.cancel_attachment.store(false, Ordering::Relaxed);
    f.host.deny_attachment.store(true, Ordering::Relaxed);
    assert!(f.show().is_err());
    assert!(!f.popup.is_open());
    assert_eq!(f.calendar.reads(), 0);
    f.host.deny_attachment.store(false, Ordering::Relaxed);
    f.host.deny_focus.store(true, Ordering::Relaxed);
    assert!(f.show().unwrap_err().contains("focus was not granted"));
    assert!(
        f.popup.is_open(),
        "foreground denial must not synthesize activation"
    );
    f.calendar.finish(Ok(snapshot(31)));
    f.drain();
    let events = f.host.events.lock().clone();
    let session = f.popup.state.borrow().session;
    f.popup.component().invoke_preferred_size_changed();
    f.popup.component().invoke_preferred_size_changed();
    advance(0);
    f.popup.disable_motion();
    f.popup.apply_theme(PresentationTheme::uniform(
        slint::language::ColorScheme::Light,
    ));
    f.popup.refit().unwrap();
    assert_eq!(f.popup.state.borrow().session, session);
    assert_eq!(&*f.host.events.lock(), &events);
    assert_eq!(f.calendar.reads(), 1);
    f.popup
        .close_if_geometry_changed(context(), f.source.window().scale_factor() + 1.0);
    assert!(!f.popup.is_open());
    assert!(!f.popup.fit_timer.running());
    f.host.deny_focus.store(false, Ordering::Relaxed);
    f.show().unwrap();
    let cache = Rc::new(RefCell::new(Some(f.popup.clone())));
    drop(crate::transient_window::TransientScope::new(
        cache.clone(),
        |popup| popup.hide(),
    ));
    assert!(cache.borrow().is_none());
    assert!(!f.popup.is_open());
    f.calendar.finish(Ok(snapshot(31)));
    f.drain();
    assert!(!f.popup.is_open());
}

#[test]
fn genuine_pointer_accessibility_and_keyboard_use_typed_current_calendar_callbacks() {
    let f = Fixture::new();
    f.loaded();
    f.click("Next month");
    assert_eq!(f.displayed(), CivilDate::new(2024, 2, 29).unwrap());
    let day = f
        .popup
        .component()
        .get_weeks()
        .row_data(0)
        .unwrap()
        .days
        .row_data(0)
        .unwrap();
    let expected = f
        .popup
        .state
        .borrow()
        .keys
        .days
        .iter()
        .find(|(key, _)| *key == day.key)
        .unwrap()
        .1;
    f.element(day.description.as_str())
        .invoke_accessible_default_action();
    assert_eq!(f.selected(), expected);
    f.element("Toggle calendar view")
        .invoke_accessible_default_action();
    assert_eq!(f.popup.component().get_view_mode(), CalendarMode::Year);
    f.click("Native month 7");
    assert_eq!(f.displayed(), CivilDate::new(2024, 7, 1).unwrap());
    assert_eq!(f.selected(), expected);
    f.popup.component().invoke_focus_content();
    f.key(Key::Tab);
    f.key(Key::Return);
    assert_eq!(f.popup.component().get_view_mode(), CalendarMode::Year);
    f.popup.component().invoke_focus_content();
    f.key(Key::Tab);
    f.key(Key::Space);
    assert_eq!(f.popup.component().get_view_mode(), CalendarMode::Month);
    f.popup.component().invoke_focus_content();
    f.key(Key::Tab);
    f.key(Key::Tab);
    f.key(Key::Return);
    assert_eq!(f.displayed(), CivilDate::new(2024, 6, 1).unwrap());
    let title = f.element("Toggle calendar view");
    let origin = title.absolute_position();
    let size = title.size();
    let position = LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    f.popup
        .component()
        .window()
        .dispatch_event(WindowEvent::PointerScrolled {
            position,
            delta_x: 0.0,
            delta_y: 120.0,
        });
    assert_eq!(
        f.displayed(),
        CivilDate::new(2024, 7, 1).unwrap(),
        "wheel up advances"
    );
    f.popup
        .component()
        .window()
        .dispatch_event(WindowEvent::PointerScrolled {
            position,
            delta_x: 0.0,
            delta_y: -120.0,
        });
    assert_eq!(
        f.displayed(),
        CivilDate::new(2024, 6, 1).unwrap(),
        "wheel down retreats"
    );
    f.key(Key::Escape);
    assert!(!f.popup.is_open());
    assert_eq!(f.calendar.reads(), 1);
}

#[test]
fn pure_browse_view_and_selection_survive_hide_reopen_and_fresh_session_read() {
    let f = Fixture::new();
    f.loaded();
    let anchor = f.popup.placement.get().unwrap().anchor;
    assert_eq!(
        anchor.y,
        f.source.window().position().y + i32::try_from(f.source.window().size().height).unwrap()
            - 10,
    );
    f.navigate(true);
    f.popup
        .component()
        .invoke_toggle_view_requested(f.popup.component().get_action_key());
    let displayed = f.displayed();
    let selected = f.selected();
    f.popup.hide();
    assert_eq!(f.displayed(), displayed);
    assert_eq!(f.selected(), selected);
    assert!(f.popup.state.borrow().keys.action.is_empty());
    assert!(f.popup.state.borrow().keys.days.is_empty());
    assert!(f.popup.state.borrow().keys.months.is_empty());
    f.show().unwrap();
    assert!(f.popup.component().get_loading());
    assert_eq!(f.popup.component().get_view_mode(), CalendarMode::Year);
    f.calendar.finish(Ok(snapshot(30)));
    f.drain();
    assert_eq!(f.displayed(), displayed);
    assert_eq!(f.selected(), selected);
    assert_eq!(f.popup.component().get_view_mode(), CalendarMode::Year);
    f.popup
        .component()
        .invoke_today_requested(f.popup.component().get_action_key());
    assert_eq!(f.displayed(), CivilDate::new(2024, 1, 30).unwrap());
    assert_eq!(f.selected(), f.displayed());
    assert_eq!(f.popup.component().get_view_mode(), CalendarMode::Year);
}

// TestingWindow has no native position/set_position implementation. Exercise
// extreme coordinates through the exact private helper used by show instead.
#[test]
fn checked_toolbar_anchor_uses_native_bottom_clock_center_and_shadow_at_each_scale() {
    assert_eq!(
        toolbar_anchor(
            PhysicalPosition::new(-1400, 100),
            PhysicalSize::new(600, 32),
            1.0,
            &bounds(),
        )
        .unwrap(),
        PhysicalPosition::new(-1305, 122),
    );
    assert_eq!(
        toolbar_anchor(
            PhysicalPosition::new(-1400, 100),
            PhysicalSize::new(1200, 64),
            2.0,
            &bounds(),
        )
        .unwrap(),
        PhysicalPosition::new(-1210, 144),
    );
    let size = PhysicalSize::new(600, 32);
    assert!(toolbar_anchor(PhysicalPosition::new(-1400, i32::MAX), size, 1.0, &bounds()).is_err());
    assert!(
        toolbar_anchor(
            PhysicalPosition::new(0, 0),
            PhysicalSize::new(600, u32::MAX),
            1.0,
            &bounds(),
        )
        .is_err()
    );
    assert!(toolbar_anchor(PhysicalPosition::new(i32::MAX, 100), size, 1.0, &bounds()).is_err());
    assert!(
        toolbar_anchor(
            PhysicalPosition::new(-1400, i32::MIN),
            PhysicalSize::new(600, 1),
            1.0,
            &bounds(),
        )
        .is_err(),
        "shadow cannot underflow native coordinates"
    );
}

#[test]
fn checked_toolbar_anchor_rejects_zero_geometry_invalid_scale_and_nonfinite_bounds() {
    let origin = PhysicalPosition::new(-1400, 100);
    let size = PhysicalSize::new(600, 32);
    for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(toolbar_anchor(origin, size, scale, &bounds()).is_err());
    }
    for size in [PhysicalSize::new(0, 32), PhysicalSize::new(600, 0)] {
        assert!(toolbar_anchor(origin, size, 1.0, &bounds()).is_err());
    }
    let mut invalid = bounds();
    invalid.origin.y = f32::NAN;
    assert!(toolbar_anchor(origin, size, 1.0, &invalid).is_err());
    invalid = bounds();
    invalid.width = 0.0;
    assert!(toolbar_anchor(origin, size, 1.0, &invalid).is_err());
    invalid = bounds();
    invalid.origin.x = f32::MAX;
    assert!(toolbar_anchor(origin, size, 2.0, &invalid).is_err());
}

#[test]
fn invalid_source_geometry_rejects_before_provider_and_accepted_callback_owns_no_ui() {
    let f = Fixture::new();
    f.source.hide().unwrap();
    assert!(f.show().is_err());
    f.source.show().unwrap();
    let mut invalid = bounds();
    invalid.width = f32::NAN;
    assert!(f.popup.show(f.source.window(), invalid, context()).is_err());
    assert_eq!(f.calendar.reads(), 0);
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 0);
    f.show().unwrap();
    let weak = Rc::downgrade(&f.popup);
    let Fixture {
        host,
        calendar,
        source,
        popup,
    } = f;
    drop(popup);
    assert!(
        weak.upgrade().is_none(),
        "provider callbacks and generated UI use only weak controllers"
    );
    assert_eq!(host.events.lock().last(), Some(&"detach"));
    calendar.finish(Ok(snapshot(31)));
    assert!(weak.upgrade().is_none());
    source.hide().unwrap();
}

fn native_snapshot(day: u32, start: WeekStart) -> CalendarSnapshot {
    let source = snapshot(day);
    CalendarSnapshot::new(
        source.today(),
        source.locale_name().into(),
        source.months().clone(),
        source.weekdays_full().clone(),
        source.weekdays_abbreviated().clone(),
        start,
    )
    .unwrap()
}

#[test]
fn saved_week_start_reprojects_and_refits_without_native_work_or_stale_input_authority() {
    let f = Fixture::new();
    f.loaded();
    for _ in 0..5 {
        f.navigate(true);
    }
    let (selected_key, selected) = f
        .popup
        .state
        .borrow()
        .keys
        .days
        .iter()
        .find(|(_, date)| date.month() == 6 && date.day() == 15)
        .unwrap()
        .clone();
    f.popup.component().invoke_day_selected(selected_key);
    assert_eq!(f.popup.component().get_weeks().row_count(), 5);
    let displayed = f.displayed();
    let old_action = f.popup.component().get_action_key();
    let old_day = f.popup.state.borrow().keys.days[0].0.clone();
    let old_height = f.popup.component().get_popup_content_height();
    let events = f.host.events.lock().clone();
    let session = f.popup.state.borrow().session;
    let placement = f.popup.placement.get().unwrap().anchor;
    let motion = f.popup.component().global::<PopoverMotion>();
    let motion_before = (motion.get_enabled(), motion.get_presented());
    f.popup.set_start_of_week(StartOfWeek::Sunday);
    assert_eq!(f.displayed(), displayed);
    assert_eq!(f.selected(), selected);
    assert_eq!(f.popup.component().get_view_mode(), CalendarMode::Month);
    assert_eq!(
        f.popup.component().get_weekdays().row_data(0).unwrap(),
        "W7"
    );
    assert_eq!(f.popup.component().get_weeks().row_count(), 6);
    assert!(f.popup.component().get_popup_content_height() > old_height);
    assert_eq!(
        *f.popup.rect.borrow(),
        Some(f.popup.preferred_rect().unwrap()),
        "visible geometry is refit to the new row count",
    );
    assert_ne!(f.popup.component().get_action_key(), old_action);
    f.popup
        .component()
        .invoke_today_requested(old_action.clone());
    f.popup
        .component()
        .invoke_navigate_requested(true, old_action);
    f.popup.component().invoke_day_selected(old_day);
    assert_eq!(f.displayed(), displayed);
    assert_eq!(f.selected(), selected);
    assert_eq!(f.calendar.reads(), 1);
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        *f.host.events.lock(),
        events,
        "no attach/detach/focus replay"
    );
    assert_eq!(f.popup.state.borrow().session, session);
    assert_eq!(f.popup.placement.get().unwrap().anchor, placement);
    assert_eq!(
        (motion.get_enabled(), motion.get_presented()),
        motion_before
    );
    assert!(f.popup.is_open());

    let key = f.popup.component().get_action_key();
    let sequence = f.popup.state.borrow().key_sequence;
    f.popup.set_start_of_week(StartOfWeek::Sunday);
    assert_eq!(f.popup.component().get_action_key(), key);
    assert_eq!(
        f.popup.state.borrow().key_sequence,
        sequence,
        "same policy is a no-op"
    );
}

#[test]
fn saved_week_start_preserves_year_view_and_retires_old_month_keys() {
    let f = Fixture::new();
    f.loaded();
    f.navigate(true);
    f.popup
        .component()
        .invoke_toggle_view_requested(f.popup.component().get_action_key());
    f.navigate(true);
    let displayed = f.displayed();
    let selected = f.selected();
    let old_month = f.popup.state.borrow().keys.months[0].0.clone();
    let events = f.host.events.lock().clone();
    f.popup.set_start_of_week(StartOfWeek::Saturday);
    f.popup.component().invoke_month_selected(old_month);
    assert_eq!(f.popup.component().get_view_mode(), CalendarMode::Year);
    assert_eq!(f.displayed(), displayed);
    assert_eq!(f.selected(), selected);
    assert_eq!(
        f.popup.component().get_weekdays().row_data(0).unwrap(),
        "W6"
    );
    assert_eq!(*f.host.events.lock(), events);
    assert_eq!(f.calendar.reads(), 1);
}

#[test]
fn native_sunday_snapshot_is_adapted_before_each_refresh_and_latest_inflight_policy_wins() {
    let f = Fixture::new();
    f.show().unwrap();
    f.calendar
        .finish(Ok(native_snapshot(31, WeekStart::Sunday)));
    f.drain();
    assert_eq!(
        f.popup.component().get_weekdays().row_data(0).unwrap(),
        "W1"
    );
    f.navigate(true);
    let displayed = f.displayed();
    let selected = f.selected();
    advance(60_000);
    assert_eq!(f.calendar.reads(), 2);
    f.calendar
        .finish(Ok(native_snapshot(30, WeekStart::Sunday)));
    f.drain();
    assert_eq!(
        f.displayed(),
        displayed,
        "native Sunday cannot reset Monday browsing"
    );
    assert_eq!(f.selected(), selected);
    advance(60_000);
    assert_eq!(f.calendar.reads(), 3);
    f.popup.set_start_of_week(StartOfWeek::Sunday);
    f.popup.set_start_of_week(StartOfWeek::Saturday);
    f.calendar
        .finish(Ok(native_snapshot(29, WeekStart::Sunday)));
    f.popup.set_start_of_week(StartOfWeek::Monday);
    f.popup.set_start_of_week(StartOfWeek::Saturday);
    f.drain();
    assert_eq!(f.displayed(), displayed);
    assert_eq!(f.selected(), selected);
    assert_eq!(
        f.popup.component().get_weekdays().row_data(0).unwrap(),
        "W6"
    );
    assert_eq!(f.popup.state.borrow().start_of_week, StartOfWeek::Saturday);
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        f.calendar.reads(),
        3,
        "policy changes must not request metadata"
    );
}

#[test]
fn hidden_week_start_policy_does_not_show_or_read_and_stale_session_cannot_replace_it() {
    let f = Fixture::new();
    f.popup.set_start_of_week(StartOfWeek::Sunday);
    assert!(!f.popup.is_open());
    assert_eq!(f.calendar.reads(), 0);
    assert_eq!(f.host.provider_calls.load(Ordering::Relaxed), 0);
    assert!(f.host.events.lock().is_empty());
    f.loaded();
    f.navigate(true);
    let displayed = f.displayed();
    let selected = f.selected();
    advance(60_000);
    assert_eq!(f.calendar.reads(), 2);
    f.popup.hide();
    let events = f.host.events.lock().clone();
    f.popup.set_start_of_week(StartOfWeek::Saturday);
    assert!(!f.popup.is_open());
    assert_eq!(f.popup.component().get_weeks().row_count(), 0);
    assert_eq!(*f.host.events.lock(), events);
    assert_eq!(f.calendar.reads(), 2);
    assert_eq!(f.displayed(), displayed);
    assert_eq!(f.selected(), selected);
    assert!(f.popup.state.borrow().keys.days.is_empty());
    f.show().unwrap();
    assert_eq!(
        f.calendar.reads(),
        2,
        "old accepted read still owns the single flight"
    );
    f.calendar.finish(Ok(native_snapshot(1, WeekStart::Sunday)));
    f.drain();
    assert_eq!(
        f.displayed(),
        displayed,
        "stale session cannot update retained metadata"
    );
    assert_eq!(f.selected(), selected);
    assert_eq!(f.calendar.reads(), 3);
    assert!(f.popup.component().get_loading());
    f.calendar
        .finish(Ok(native_snapshot(30, WeekStart::Sunday)));
    f.drain();
    assert_eq!(f.displayed(), displayed);
    assert_eq!(f.selected(), selected);
    assert_eq!(
        f.popup.component().get_weekdays().row_data(0).unwrap(),
        "W6"
    );
    assert_eq!(f.popup.state.borrow().start_of_week, StartOfWeek::Saturday);
}

#[test]
fn week_start_changed_under_projection_retires_authority_and_defers_latest_policy() {
    let f = Fixture::new();
    f.loaded();
    let old_day = f.popup.state.borrow().keys.days[0].0.clone();
    let selected = f.selected();
    f.popup.projecting.set(true);
    f.popup.set_start_of_week(StartOfWeek::Sunday);
    f.popup.set_start_of_week(StartOfWeek::Saturday);
    assert!(f.popup.policy_projection_pending.get());
    assert!(f.popup.state.borrow().keys.action.is_empty());
    assert!(f.popup.state.borrow().keys.days.is_empty());
    f.popup.component().invoke_day_selected(old_day.clone());
    assert_eq!(f.selected(), selected);
    f.popup.projecting.set(false);
    f.popup.project_and_fit();
    assert!(!f.popup.policy_projection_pending.get());
    assert_eq!(
        f.popup.component().get_weekdays().row_data(0).unwrap(),
        "W6"
    );
    f.popup.component().invoke_day_selected(old_day);
    assert_eq!(f.selected(), selected);
    assert_eq!(f.calendar.reads(), 1);
}
