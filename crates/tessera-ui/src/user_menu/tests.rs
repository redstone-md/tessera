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
use tessera_system::folders::{FolderOpenCompletion, FolderReadCompletion};

type SnapshotResult = Result<FolderSnapshot, FolderError>;
type ReentryHook = RefCell<Option<Box<dyn FnOnce()>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TestTarget {
    id: FolderId,
    revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordedRequest {
    Read,
    Open(TestTarget),
}

enum Reply {
    Queued,
    ReadInline(SnapshotResult),
    OpenInline(Result<(), FolderError>),
    Rejected(FolderError),
}

enum ProviderCompletion {
    Read(FolderReadCompletion),
    Open(FolderOpenCompletion),
}

#[derive(Default)]
struct RecordingState {
    requests: Vec<RecordedRequest>,
    replies: VecDeque<Reply>,
    completions: VecDeque<ProviderCompletion>,
    active: usize,
    maximum_active: usize,
}

#[derive(Default)]
struct RecordingFolders(Mutex<RecordingState>);

thread_local! {
    // Only recording providers invoke these on the test UI thread. Production
    // completions still contain Send data and the weak generated component.
    static REQUEST_HOOK: ReentryHook = const { RefCell::new(None) };
    static FACTORY_HOOK: ReentryHook = const { RefCell::new(None) };
    static FOCUS_HOOK: ReentryHook = const { RefCell::new(None) };
}

fn run_hook(hook: &'static std::thread::LocalKey<ReentryHook>) {
    let callback = hook.with(|hook| hook.borrow_mut().take());
    if let Some(callback) = callback {
        callback();
    }
}

impl RecordingFolders {
    fn accept(
        &self,
        request: RecordedRequest,
        completion: ProviderCompletion,
    ) -> Result<(), FolderError> {
        let reply = {
            let mut state = self.0.lock();
            state.requests.push(request);
            let reply = state.replies.pop_front().unwrap_or(Reply::Queued);
            if !matches!(reply, Reply::Rejected(_)) {
                state.active += 1;
                state.maximum_active = state.maximum_active.max(state.active);
            }
            reply
        };
        run_hook(&REQUEST_HOOK);
        match (reply, completion) {
            (Reply::Queued, completion) => {
                self.0.lock().completions.push_back(completion);
                Ok(())
            }
            (Reply::ReadInline(result), ProviderCompletion::Read(completion)) => {
                self.0.lock().active -= 1;
                completion(result);
                Ok(())
            }
            (Reply::OpenInline(result), ProviderCompletion::Open(completion)) => {
                self.0.lock().active -= 1;
                completion(result);
                Ok(())
            }
            (Reply::Rejected(error), _) => Err(error),
            _ => panic!("recording reply type must match request"),
        }
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.0.lock().requests.clone()
    }

    fn reply(&self, reply: Reply) {
        self.0.lock().replies.push_back(reply);
    }

    fn take_completion(&self) -> ProviderCompletion {
        let mut state = self.0.lock();
        let completion = state
            .completions
            .pop_front()
            .expect("accepted queued request");
        state.active -= 1;
        completion
    }

    fn finish_read(&self, result: SnapshotResult) {
        let ProviderCompletion::Read(completion) = self.take_completion() else {
            panic!("expected a folder read");
        };
        completion(result);
    }

    fn finish_open(&self, result: Result<(), FolderError>) {
        let ProviderCompletion::Open(completion) = self.take_completion() else {
            panic!("expected a folder open");
        };
        completion(result);
    }
}

impl FolderHost for RecordingFolders {
    fn read(&self, completion: FolderReadCompletion) -> Result<(), FolderError> {
        self.accept(RecordedRequest::Read, ProviderCompletion::Read(completion))
    }

    fn open(
        &self,
        id: FolderId,
        target: FolderTarget,
        completion: FolderOpenCompletion,
    ) -> Result<(), FolderError> {
        let target = *target
            .get::<TestTarget>()
            .expect("provider-issued private target");
        assert_eq!(id, target.id, "UI must preserve provider-issued category");
        self.accept(
            RecordedRequest::Open(target),
            ProviderCompletion::Open(completion),
        )
    }
}

struct RecordingDesktop {
    provider: Mutex<Result<Option<Arc<dyn FolderHost>>, FolderError>>,
    provider_calls: AtomicUsize,
    events: Arc<Mutex<Vec<&'static str>>>,
    root: Mutex<Option<slint::Weak<UserMenu>>>,
    deny_focus: AtomicBool,
    deny_attachment: AtomicBool,
    close_during_attachment: AtomicBool,
}

struct SurfaceLease {
    events: Arc<Mutex<Vec<&'static str>>>,
    root: slint::Weak<UserMenu>,
}

impl Drop for SurfaceLease {
    fn drop(&mut self) {
        if let Some(root) = self.root.upgrade() {
            assert!(
                root.window().is_visible(),
                "native lease must release before hide"
            );
        }
        self.events.lock().push("detach");
    }
}

impl DesktopHost for RecordingDesktop {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("Folder access must not observe the desktop/catalog")
    }

    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("Folder access must not activate catalog windows")
    }

    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("A folder is not an application launch")
    }

    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("A folder is not an untyped system command")
    }

    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("Folder projections must not be persisted")
    }

    fn shell_identity(&self) -> Result<crate::ShellIdentity, String> {
        panic!("Identity is supplied by the caller, not reread here")
    }

    fn folder_host(&self) -> Result<Option<Arc<dyn FolderHost>>, FolderError> {
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
            self.events.lock().push("attach-denied");
            return Err("Native popup attachment denied".into());
        }
        self.events.lock().push("attach");
        if self.close_during_attachment.load(Ordering::Relaxed) {
            window.dispatch_event(WindowEvent::CloseRequested);
            assert!(
                window.is_visible(),
                "late lease must drop before cancellation hides"
            );
        }
        Ok(Some(Box::new(SurfaceLease {
            events: self.events.clone(),
            root: self.root.lock().clone().expect("recording popup root"),
        })))
    }

    fn request_ui_focus(&self, window: &slint::Window) -> Result<(), String> {
        assert!(window.is_visible());
        self.events.lock().push("focus");
        run_hook(&FOCUS_HOOK);
        if self.deny_focus.load(Ordering::Relaxed) {
            Err("OS denied foreground".into())
        } else {
            Ok(())
        }
    }
}

struct Fixture {
    host: Arc<RecordingDesktop>,
    folders: Arc<RecordingFolders>,
    source: Panel,
    popup: Rc<UserMenuController>,
}

impl Fixture {
    fn new() -> Self {
        i_slint_backend_testing::init_no_event_loop();
        let folders = Arc::new(RecordingFolders::default());
        let host = Arc::new(RecordingDesktop {
            provider: Mutex::new(Ok(Some(folders.clone()))),
            provider_calls: AtomicUsize::new(0),
            events: Arc::new(Mutex::default()),
            root: Mutex::default(),
            deny_focus: AtomicBool::new(false),
            deny_attachment: AtomicBool::new(false),
            close_during_attachment: AtomicBool::new(false),
        });
        let source = Panel::new().unwrap();
        source
            .window()
            .set_position(PhysicalPosition::new(-1400, 100));
        source.window().set_size(PhysicalSize::new(600, 600));
        source.show().unwrap();
        let popup = UserMenuController::new(host.clone()).unwrap();
        *host.root.lock() = Some(popup.component().as_weak());
        popup.apply_theme(PresentationTheme::uniform(
            slint::language::ColorScheme::Dark,
        ));
        Self {
            host,
            folders,
            source,
            popup,
        }
    }

    fn show(&self) -> Result<(), String> {
        self.popup.show(
            self.source.window(),
            bounds(),
            context(),
            "  genuine\naccount  ",
        )
    }

    fn loaded(&self, snapshot: FolderSnapshot) {
        self.show().unwrap();
        self.folders.finish_read(Ok(snapshot));
        assert!(
            self.popup.component().get_loading(),
            "delivery must not project inline"
        );
        self.drain();
        assert!(!self.popup.component().get_loading());
    }

    fn drain(&self) {
        // The no-event-loop backend cannot deliver posts. Exercise the same
        // production-generated wakeup after asserting delivery stayed async.
        self.popup.component().invoke_folder_event_ready();
    }

    fn row(&self, id: FolderId) -> UserFolderRow {
        self.popup
            .component()
            .get_rows()
            .row_data(folder_index(id))
            .unwrap()
    }

    fn open(&self, id: FolderId) {
        let row = self.row(id);
        self.popup
            .component()
            .invoke_folder_open_requested(row.kind, row.key);
    }

    fn element(&self, label: &str) -> ElementHandle {
        ElementHandle::find_by_accessible_label(self.popup.component(), label)
            .next()
            .unwrap_or_else(|| panic!("missing generated element: {label}"))
    }

    fn key(&self, key: Key) {
        let window = self.popup.component().window();
        window.dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        window.dispatch_event(WindowEvent::KeyReleased { text: key.into() });
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
}

fn context() -> DockContext {
    DockContext::new(-1920, 0, 1920, 1080, false).unwrap()
}

fn bounds() -> TileBounds {
    TileBounds {
        origin: LogicalPosition::new(20.0, 500.0),
        width: 150.0,
        height: 40.0,
    }
}

fn snapshot(revision: u64) -> FolderSnapshot {
    FolderSnapshot::new(
        FolderId::ALL
            .map(|id| FolderAvailability::Ready(FolderTarget::new(TestTarget { id, revision }))),
    )
}

fn denied_snapshot() -> FolderSnapshot {
    FolderSnapshot::new(FolderId::ALL.map(|id| match id {
        FolderId::Music => FolderAvailability::Unavailable(error(FolderErrorKind::AccessDenied)),
        FolderId::Pictures => FolderAvailability::Unavailable(error(FolderErrorKind::Unavailable)),
        _ => FolderAvailability::Ready(FolderTarget::new(TestTarget { id, revision: 7 })),
    }))
}

fn error(kind: FolderErrorKind) -> FolderError {
    FolderError::new(
        kind,
        "private C:\\redirected\\user-secret\u{202e}\n must not leak",
    )
}

fn advance(milliseconds: u64) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(milliseconds));
    slint::platform::update_timers_and_animations();
}

#[test]
fn real_identity_lazy_provider_and_ordered_availability_are_truthful_and_silent() {
    let fixture = Fixture::new();
    assert_eq!(fixture.host.provider_calls.load(Ordering::Relaxed), 0);
    assert!(fixture.folders.requests().is_empty());
    fixture.show().unwrap();
    assert_eq!(fixture.popup.component().get_user_name(), "genuine account");
    assert!(fixture.popup.component().get_loading());
    assert!(!fixture.popup.component().get_retry_enabled());
    for id in FolderId::ALL {
        let row = fixture.row(id);
        assert_eq!(row.kind, folder_kind(id));
        assert!(!row.ready);
        assert!(row.key.is_empty());
        fixture.open(id);
    }
    assert_eq!(fixture.folders.requests(), [RecordedRequest::Read]);
    fixture.folders.finish_read(Ok(denied_snapshot()));
    fixture.drain();
    let mut keys = std::collections::HashSet::new();
    for id in FolderId::ALL {
        let row = fixture.row(id);
        assert_eq!(row.kind, folder_kind(id));
        assert!(!row.key.is_empty());
        assert!(keys.insert(row.key.to_string()));
    }
    assert!(fixture.row(FolderId::Desktop).ready);
    assert!(!fixture.row(FolderId::Music).ready);
    assert_eq!(fixture.row(FolderId::Music).status, "Access denied");
    assert_eq!(fixture.row(FolderId::Pictures).status, "Folder unavailable");
    fixture.open(FolderId::Music);
    fixture.open(FolderId::Pictures);
    assert_eq!(fixture.folders.requests(), [RecordedRequest::Read]);
    assert!(fixture.popup.component().get_retry_enabled());
    assert_eq!(fixture.host.provider_calls.load(Ordering::Relaxed), 1);
}

#[test]
fn exact_closed_id_current_key_and_ready_private_target_authorize_one_open() {
    let fixture = Fixture::new();
    fixture.loaded(snapshot(1));
    let row = fixture.row(FolderId::Downloads);
    for key in ["", "2", "Downloads", "C:\\Users\\me\\Downloads", "user-1-1"] {
        fixture
            .popup
            .component()
            .invoke_folder_open_requested(row.kind, key.into());
    }
    fixture
        .popup
        .component()
        .invoke_folder_open_requested(UserFolderKind::Desktop, row.key.clone());
    assert_eq!(fixture.folders.requests(), [RecordedRequest::Read]);
    fixture.open(FolderId::Downloads);
    fixture.open(FolderId::Downloads);
    fixture.popup.component().invoke_retry_requested();
    assert!(fixture.popup.component().get_opening());
    assert!(!fixture.popup.component().get_retry_enabled());
    assert_eq!(
        fixture.folders.requests(),
        [
            RecordedRequest::Read,
            RecordedRequest::Open(TestTarget {
                id: FolderId::Downloads,
                revision: 1
            }),
        ]
    );
    fixture.folders.finish_open(Ok(()));
    fixture.drain();
    assert_eq!(
        fixture.popup.component().get_notice(),
        "Downloads open request accepted."
    );
    fixture.popup.component().invoke_retry_requested();
    fixture.folders.finish_read(Ok(snapshot(2)));
    fixture.drain();
    assert_ne!(fixture.row(FolderId::Downloads).key, row.key);
    fixture
        .popup
        .component()
        .invoke_folder_open_requested(row.kind, row.key);
    assert_eq!(
        fixture.folders.requests().len(),
        3,
        "old projection must not dispatch"
    );
    let current = fixture.row(FolderId::Downloads);
    fixture.popup.hide();
    fixture
        .popup
        .component()
        .invoke_folder_open_requested(current.kind, current.key);
    fixture.popup.component().invoke_retry_requested();
    assert_eq!(
        fixture.folders.requests().len(),
        3,
        "hidden input must not dispatch"
    );
}

#[test]
fn immediate_rejection_inline_and_worker_completions_only_project_through_mailbox() {
    let fixture = Fixture::new();
    fixture
        .folders
        .reply(Reply::Rejected(error(FolderErrorKind::AccessDenied)));
    fixture.show().unwrap();
    assert!(fixture.popup.component().get_loading());
    fixture.drain();
    assert!(!fixture.popup.component().get_loading());
    assert!(
        fixture
            .popup
            .component()
            .get_notice()
            .contains("Access denied")
    );
    assert!(
        !fixture
            .popup
            .component()
            .get_notice()
            .contains("user-secret")
    );
    assert!(fixture.popup.component().get_retry_enabled());
    fixture.folders.reply(Reply::ReadInline(Ok(snapshot(3))));
    fixture.popup.component().invoke_retry_requested();
    assert!(
        fixture.popup.component().get_loading(),
        "inline callback is still async to UI"
    );
    fixture.drain();
    assert!(fixture.row(FolderId::Documents).ready);
    fixture
        .folders
        .reply(Reply::Rejected(error(FolderErrorKind::Busy)));
    fixture.open(FolderId::Documents);
    assert!(fixture.popup.component().get_opening());
    fixture.drain();
    assert!(!fixture.popup.component().get_opening());
    assert!(!fixture.row(FolderId::Documents).ready);
    assert!(fixture.popup.component().get_notice().contains("busy"));
    fixture.folders.reply(Reply::ReadInline(Ok(snapshot(4))));
    fixture.popup.component().invoke_retry_requested();
    fixture.drain();
    fixture.folders.reply(Reply::OpenInline(Ok(())));
    fixture.open(FolderId::Documents);
    assert!(fixture.popup.component().get_opening());
    fixture.drain();
    assert!(!fixture.popup.component().get_opening());
    fixture.open(FolderId::Videos);
    let ProviderCompletion::Open(completion) = fixture.folders.take_completion() else {
        panic!("queued open expected");
    };
    std::thread::spawn(move || completion(Ok(())))
        .join()
        .unwrap();
    assert!(
        fixture.popup.component().get_opening(),
        "worker never touches GUI directly"
    );
    fixture.drain();
    assert_eq!(
        fixture.popup.component().get_notice(),
        "Videos open request accepted."
    );
    assert_eq!(fixture.folders.0.lock().maximum_active, 1);
}

#[test]
fn unsupported_and_factory_failures_retry_without_permanent_error_cache() {
    let fixture = Fixture::new();
    *fixture.host.provider.lock() = Ok(None);
    fixture.show().unwrap();
    assert!(!fixture.popup.component().get_loading());
    assert!(
        fixture
            .popup
            .component()
            .get_notice()
            .contains("not supported")
    );
    assert!(fixture.folders.requests().is_empty());
    *fixture.host.provider.lock() = Err(error(FolderErrorKind::Stopped));
    fixture.popup.component().invoke_retry_requested();
    assert!(fixture.popup.component().get_notice().contains("stopped"));
    *fixture.host.provider.lock() = Ok(Some(fixture.folders.clone()));
    fixture.popup.component().invoke_retry_requested();
    assert_eq!(fixture.host.provider_calls.load(Ordering::Relaxed), 3);
    assert_eq!(fixture.folders.requests(), [RecordedRequest::Read]);
    fixture.folders.finish_read(Ok(snapshot(1)));
    fixture.drain();
    assert!(fixture.row(FolderId::Recent).ready);
    fixture.popup.hide();
    fixture.show().unwrap();
    assert!(fixture.popup.component().get_notice().is_empty());
    assert_eq!(fixture.host.provider_calls.load(Ordering::Relaxed), 3);
    assert_eq!(
        fixture.folders.requests(),
        [RecordedRequest::Read, RecordedRequest::Read]
    );
}

#[test]
fn hidden_read_retires_without_projecting_and_reopen_reads_only_after_its_completion() {
    let fixture = Fixture::new();
    fixture.show().unwrap();
    fixture.popup.hide();
    fixture.show().unwrap();
    assert_eq!(fixture.folders.requests(), [RecordedRequest::Read]);
    fixture.folders.finish_read(Ok(snapshot(99)));
    fixture.drain();
    assert!(fixture.popup.component().get_loading());
    assert!(!fixture.row(FolderId::Desktop).ready);
    assert_eq!(
        fixture.folders.requests(),
        [RecordedRequest::Read, RecordedRequest::Read]
    );
    fixture.folders.finish_read(Ok(snapshot(2)));
    fixture.drain();
    fixture.open(FolderId::Desktop);
    assert_eq!(
        fixture.folders.requests().last(),
        Some(&RecordedRequest::Open(TestTarget {
            id: FolderId::Desktop,
            revision: 2,
        }))
    );
    assert_eq!(fixture.folders.0.lock().maximum_active, 1);
    fixture.popup.hide();
    fixture
        .folders
        .finish_open(Err(error(FolderErrorKind::Unavailable)));
    fixture.drain();
    assert!(!fixture.popup.is_open());
    assert!(fixture.popup.state.borrow().flight.is_none());
    assert_eq!(fixture.folders.requests().len(), 3);
}

#[test]
fn accepted_open_survives_hide_reopen_without_duplicate_or_old_error_feedback() {
    let fixture = Fixture::new();
    fixture.loaded(snapshot(1));
    let old = fixture.row(FolderId::Recent);
    fixture.open(FolderId::Recent);
    fixture.popup.hide();
    fixture.show().unwrap();
    fixture
        .popup
        .component()
        .invoke_folder_open_requested(old.kind, old.key);
    fixture.popup.component().invoke_retry_requested();
    assert!(fixture.popup.component().get_opening());
    assert!(fixture.popup.component().get_loading());
    assert_eq!(fixture.folders.requests().len(), 2);
    fixture
        .folders
        .finish_open(Err(error(FolderErrorKind::TargetChanged)));
    fixture.drain();
    assert!(!fixture.popup.component().get_opening());
    assert!(
        fixture.popup.component().get_notice().is_empty(),
        "old session cannot publish errors"
    );
    assert_eq!(
        fixture.folders.requests().len(),
        3,
        "only a fresh read follows old completion"
    );
    fixture.folders.finish_read(Ok(snapshot(2)));
    fixture.drain();
    assert_eq!(
        fixture.folders.requests().len(),
        3,
        "opening is never retried automatically"
    );
    fixture.open(FolderId::Recent);
    assert_eq!(
        fixture.folders.requests().last(),
        Some(&RecordedRequest::Open(TestTarget {
            id: FolderId::Recent,
            revision: 2,
        }))
    );
    assert_eq!(fixture.folders.0.lock().maximum_active, 1);
}

#[test]
fn changed_target_invalidates_row_and_only_explicit_retry_reads_a_new_target() {
    let fixture = Fixture::new();
    fixture.loaded(snapshot(1));
    let old = fixture.row(FolderId::Downloads);
    fixture.folders.reply(Reply::OpenInline(Err(error(
        FolderErrorKind::TargetChanged,
    ))));
    fixture.open(FolderId::Downloads);
    fixture.drain();
    assert!(!fixture.row(FolderId::Downloads).ready);
    assert!(fixture.row(FolderId::Downloads).status.contains("changed"));
    assert!(
        fixture.row(FolderId::Desktop).ready,
        "failure remains per-folder"
    );
    fixture.open(FolderId::Downloads);
    fixture
        .popup
        .component()
        .invoke_folder_open_requested(old.kind, old.key);
    assert_eq!(fixture.folders.requests().len(), 2);
    fixture.popup.component().invoke_retry_requested();
    fixture.folders.finish_read(Ok(snapshot(2)));
    fixture.drain();
    assert!(fixture.row(FolderId::Downloads).ready);
    assert_eq!(fixture.folders.requests().len(), 3);
}

#[test]
fn native_cancellation_and_attachment_failure_drop_late_lease_without_read_or_focus() {
    let fixture = Fixture::new();
    fixture
        .host
        .close_during_attachment
        .store(true, Ordering::Relaxed);
    fixture.show().unwrap();
    assert!(!fixture.popup.is_open());
    assert!(!fixture.popup.component().window().is_visible());
    assert_eq!(&*fixture.host.events.lock(), &["attach", "detach"]);
    assert_eq!(fixture.host.provider_calls.load(Ordering::Relaxed), 0);
    assert!(fixture.folders.requests().is_empty());
    fixture
        .host
        .close_during_attachment
        .store(false, Ordering::Relaxed);
    fixture.host.deny_attachment.store(true, Ordering::Relaxed);
    assert!(fixture.show().unwrap_err().contains("attachment denied"));
    assert!(!fixture.popup.is_open());
    assert!(fixture.folders.requests().is_empty());
    fixture.host.deny_attachment.store(false, Ordering::Relaxed);
    fixture.host.deny_focus.store(true, Ordering::Relaxed);
    assert!(
        fixture
            .show()
            .unwrap_err()
            .contains("focus was not granted")
    );
    assert!(
        fixture.popup.is_open(),
        "foreground refusal leaves mouse access usable"
    );
    fixture.folders.finish_read(Ok(snapshot(1)));
    fixture.drain();
    fixture.open(FolderId::Desktop);
    assert_eq!(fixture.folders.requests().len(), 2);
}

#[test]
fn factory_and_provider_reentry_do_not_hold_refcell_borrows_or_overlap_requests() {
    let fixture = Fixture::new();
    let popup = fixture.popup.clone();
    let source = fixture.source.clone_strong();
    FACTORY_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            popup.hide();
            popup
                .show(source.window(), bounds(), context(), "new identity")
                .unwrap();
        }));
    });
    fixture.show().unwrap();
    assert_eq!(fixture.host.provider_calls.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.popup.component().get_user_name(), "new identity");
    assert_eq!(fixture.folders.requests(), [RecordedRequest::Read]);
    fixture.folders.finish_read(Ok(snapshot(1)));
    fixture.drain();
    let popup = fixture.popup.clone();
    let source = fixture.source.clone_strong();
    REQUEST_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            popup.hide();
            popup
                .show(source.window(), bounds(), context(), "reopened identity")
                .unwrap();
        }));
    });
    fixture.open(FolderId::Pictures);
    assert!(fixture.popup.component().get_opening());
    assert_eq!(fixture.folders.requests().len(), 2);
    fixture.folders.finish_open(Ok(()));
    fixture.drain();
    assert_eq!(
        fixture.popup.component().get_user_name(),
        "reopened identity"
    );
    assert!(fixture.popup.component().get_notice().is_empty());
    assert_eq!(fixture.folders.requests().len(), 3);
    assert_eq!(fixture.folders.0.lock().maximum_active, 1);
}

#[test]
fn focus_reentry_close_escape_and_scope_teardown_release_before_hide_and_never_resurrect() {
    let fixture = Fixture::new();
    let popup = fixture.popup.clone();
    FOCUS_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || popup.hide()));
    });
    fixture.show().unwrap();
    assert!(!fixture.popup.is_open());
    assert!(
        fixture.folders.requests().is_empty(),
        "cancelled focus cannot start a read"
    );
    assert_eq!(&*fixture.host.events.lock(), &["attach", "focus", "detach"]);
    fixture.show().unwrap();
    fixture.key(Key::Escape);
    assert!(!fixture.popup.is_open());
    assert!(!fixture.popup.fit_timer.running());
    assert!(!fixture.popup.focus_watch.running());
    fixture.folders.finish_read(Ok(snapshot(1)));
    fixture.drain();
    assert_eq!(fixture.folders.requests().len(), 1);
    fixture.show().unwrap();
    fixture
        .popup
        .component()
        .window()
        .dispatch_event(WindowEvent::CloseRequested);
    assert!(!fixture.popup.is_open());
    fixture.folders.finish_read(Ok(snapshot(2)));
    fixture.drain();
    fixture.show().unwrap();
    let cache = Rc::new(RefCell::new(Some(fixture.popup.clone())));
    let scope = crate::transient_window::TransientScope::new(cache.clone(), |popup| popup.hide());
    drop(scope);
    assert!(cache.borrow().is_none());
    assert!(!fixture.popup.is_open());
    fixture.folders.finish_read(Ok(snapshot(3)));
    fixture.drain();
    assert!(!fixture.popup.is_open());
    assert_eq!(fixture.host.events.lock().last(), Some(&"detach"));
}

#[test]
fn preferred_size_refit_and_theme_changes_preserve_native_lease_and_accepted_flight() {
    let fixture = Fixture::new();
    fixture.loaded(snapshot(1));
    fixture.open(FolderId::Desktop);
    let generation = fixture.popup.state.borrow().generation;
    let events = fixture.host.events.lock().clone();
    let height = fixture.popup.component().window().size().height;
    fixture
        .popup
        .component()
        .set_notice("Additional bounded explanatory text. ".repeat(6).into());
    fixture.popup.component().invoke_preferred_size_changed();
    fixture.popup.component().invoke_preferred_size_changed();
    assert!(fixture.popup.fit_timer.running());
    advance(0);
    assert!(!fixture.popup.fit_timer.running());
    assert!(fixture.popup.component().window().size().height >= height);
    fixture.popup.apply_theme(PresentationTheme::uniform(
        slint::language::ColorScheme::Light,
    ));
    fixture.popup.refit().unwrap();
    fixture
        .popup
        .close_if_geometry_changed(context(), fixture.source.window().scale_factor());
    assert!(fixture.popup.is_open());
    assert_eq!(fixture.popup.state.borrow().generation, generation);
    assert_eq!(&*fixture.host.events.lock(), &events);
    assert_eq!(fixture.folders.requests().len(), 2);
    fixture.popup.component().invoke_preferred_size_changed();
    fixture.popup.hide();
    let hidden_size = fixture.popup.component().window().size();
    fixture.popup.component().invoke_preferred_size_changed();
    advance(0);
    assert!(!fixture.popup.is_open());
    assert!(!fixture.popup.fit_timer.running());
    assert_eq!(fixture.popup.component().window().size(), hidden_size);
}

#[test]
fn real_pointer_tab_enter_space_and_accessibility_dispatch_only_current_typed_ready_rows() {
    let fixture = Fixture::new();
    fixture.loaded(denied_snapshot());
    fixture.click("Open Downloads");
    assert_eq!(
        fixture.folders.requests().last(),
        Some(&RecordedRequest::Open(TestTarget {
            id: FolderId::Downloads,
            revision: 7,
        }))
    );
    fixture.click("Open Downloads");
    fixture
        .element("Open Desktop")
        .invoke_accessible_default_action();
    assert_eq!(
        fixture.folders.requests().len(),
        2,
        "busy popup rejects real duplicate input"
    );
    fixture.folders.finish_open(Ok(()));
    fixture.drain();
    fixture.popup.component().invoke_focus_content();
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    assert_eq!(
        fixture.folders.requests().last(),
        Some(&RecordedRequest::Open(TestTarget {
            id: FolderId::Recent,
            revision: 7,
        }))
    );
    fixture.folders.finish_open(Ok(()));
    fixture.drain();
    fixture.popup.component().invoke_focus_content();
    fixture.key(Key::Tab);
    fixture.key(Key::Tab);
    fixture.key(Key::Space);
    assert_eq!(
        fixture.folders.requests().last(),
        Some(&RecordedRequest::Open(TestTarget {
            id: FolderId::Desktop,
            revision: 7,
        }))
    );
    fixture.folders.finish_open(Ok(()));
    fixture.drain();
    let before = fixture.folders.requests().len();
    fixture.click("Open Music");
    fixture
        .element("Open Music")
        .invoke_accessible_default_action();
    assert_eq!(
        fixture.folders.requests().len(),
        before,
        "denied row is readable, not actionable"
    );
    fixture
        .element("Open Videos")
        .invoke_accessible_default_action();
    assert_eq!(
        fixture.folders.requests().last(),
        Some(&RecordedRequest::Open(TestTarget {
            id: FolderId::Videos,
            revision: 7,
        }))
    );
    fixture.key(Key::Escape);
    assert!(
        !fixture.popup.is_open(),
        "Escape bubbles from genuine focused folder input"
    );
    assert_eq!(fixture.folders.0.lock().maximum_active, 1);
}

#[test]
fn invalid_source_geometry_and_changed_monitor_close_without_mutating_host_state() {
    let fixture = Fixture::new();
    let mut invalid = bounds();
    invalid.width = f32::NAN;
    assert!(
        fixture
            .popup
            .show(fixture.source.window(), invalid, context(), "identity")
            .is_err()
    );
    fixture.source.hide().unwrap();
    assert!(fixture.show().is_err());
    assert!(fixture.folders.requests().is_empty());
    assert!(fixture.host.events.lock().is_empty());
    fixture.source.show().unwrap();
    fixture.loaded(snapshot(1));
    fixture
        .popup
        .close_if_geometry_changed(DockContext::new(-1900, 0, 1920, 1080, false).unwrap(), 1.0);
    assert!(!fixture.popup.is_open());
    assert_eq!(fixture.host.events.lock().last(), Some(&"detach"));
}
