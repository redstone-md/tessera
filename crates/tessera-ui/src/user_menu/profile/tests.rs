// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::{Panel, TileBounds};
use crate::{DockContext, PanelPreferences, PanelSnapshot, SystemAction};
use slint::{ComponentHandle, LogicalPosition, Model, PhysicalSize};
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use tessera_system::folders::{
    FolderAvailability, FolderHost, FolderId, FolderOpenCompletion, FolderReadCompletion,
    FolderSnapshot, FolderTarget,
};
use tessera_system::profile::{
    ProfileOpenCompletion, ProfilePhoto, ProfileReadCompletion, ProfileTarget,
};

type Hook = RefCell<Option<Box<dyn FnOnce()>>>;
thread_local! {
    static FACTORY: Hook = const { RefCell::new(None) };
    static REQUEST: Hook = const { RefCell::new(None) };
    static ADMISSION: Hook = const { RefCell::new(None) };
}

fn hook(slot: &'static std::thread::LocalKey<Hook>) {
    let callback = slot.with(|slot| slot.borrow_mut().take());
    if let Some(callback) = callback {
        callback();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Recorded {
    Read,
    Home,
    Accounts,
    OneDrive(u64),
}

enum Reply {
    Queued,
    ReadInline(Result<ProfileSnapshot, ProfileError>),
    OpenInline(Result<(), ProfileError>),
    Reject(ProfileError),
}

enum Callback {
    Read(ProfileReadCompletion),
    Open(ProfileOpenCompletion),
}

#[derive(Default)]
struct Recording {
    requests: Vec<Recorded>,
    replies: VecDeque<Reply>,
    callbacks: VecDeque<Callback>,
    active: usize,
    maximum: usize,
}

#[derive(Default)]
struct Provider(Mutex<Recording>);

impl Provider {
    fn accept(&self, request: Recorded, callback: Callback) -> Result<(), ProfileError> {
        let reply = {
            let mut state = self.0.lock();
            state.requests.push(request);
            let reply = state.replies.pop_front().unwrap_or(Reply::Queued);
            if !matches!(reply, Reply::Reject(_)) {
                state.active += 1;
                state.maximum = state.maximum.max(state.active);
            }
            reply
        };
        hook(&REQUEST);
        match (reply, callback) {
            (Reply::Queued, callback) => self.0.lock().callbacks.push_back(callback),
            (Reply::ReadInline(result), Callback::Read(callback)) => {
                self.0.lock().active -= 1;
                callback(result);
            }
            (Reply::OpenInline(result), Callback::Open(callback)) => {
                self.0.lock().active -= 1;
                callback(result);
            }
            (Reply::Reject(error), _) => return Err(error),
            _ => panic!("recording reply must match typed request"),
        }
        Ok(())
    }

    fn take(&self) -> Callback {
        let mut state = self.0.lock();
        let callback = state.callbacks.pop_front().expect("accepted operation");
        state.active -= 1;
        callback
    }

    fn read(&self, snapshot: Result<ProfileSnapshot, ProfileError>) {
        let Callback::Read(callback) = self.take() else {
            panic!("read expected")
        };
        callback(snapshot);
    }

    fn open(&self, result: Result<(), ProfileError>) {
        let Callback::Open(callback) = self.take() else {
            panic!("open expected")
        };
        callback(result);
    }

    fn requests(&self) -> Vec<Recorded> {
        self.0.lock().requests.clone()
    }
}

impl ProfileHost for Provider {
    fn read(&self, callback: ProfileReadCompletion) -> Result<(), ProfileError> {
        self.accept(Recorded::Read, Callback::Read(callback))
    }

    fn execute(
        &self,
        command: ProfileCommand,
        callback: ProfileOpenCompletion,
    ) -> Result<(), ProfileError> {
        let request = match command {
            ProfileCommand::OpenHome => Recorded::Home,
            ProfileCommand::OpenAccountsSettings => Recorded::Accounts,
            ProfileCommand::OpenOneDrive { expected } => Recorded::OneDrive(
                *expected
                    .downcast_ref::<u64>()
                    .expect("opaque recording target"),
            ),
        };
        self.accept(request, Callback::Open(callback))
    }
}

#[derive(Default)]
struct Folders(Mutex<Vec<FolderId>>);
impl FolderHost for Folders {
    fn read(
        &self,
        completion: FolderReadCompletion,
    ) -> Result<(), tessera_system::folders::FolderError> {
        completion(Ok(FolderSnapshot::new(
            FolderId::ALL.map(|id| FolderAvailability::Ready(FolderTarget::new(id))),
        )));
        Ok(())
    }
    fn open(
        &self,
        id: FolderId,
        _: FolderTarget,
        completion: FolderOpenCompletion,
    ) -> Result<(), tessera_system::folders::FolderError> {
        self.0.lock().push(id);
        completion(Ok(()));
        Ok(())
    }
}

struct Desktop {
    provider: Mutex<Result<Option<Arc<dyn ProfileHost>>, ProfileError>>,
    calls: AtomicUsize,
    folders: Arc<Folders>,
}
impl DesktopHost for Desktop {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("no inventory")
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("no activation")
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("no launches")
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("no untyped command or second power executor")
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("profile is memory-only")
    }
    fn shell_identity(&self) -> Result<crate::ShellIdentity, String> {
        panic!("no identity inventory")
    }
    fn folder_host(
        &self,
    ) -> Result<Option<Arc<dyn FolderHost>>, tessera_system::folders::FolderError> {
        Ok(Some(self.folders.clone()))
    }
    fn profile_host(&self) -> Result<Option<Arc<dyn ProfileHost>>, ProfileError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        hook(&FACTORY);
        self.provider.lock().clone()
    }
    fn request_ui_focus(&self, _: &slint::Window) -> Result<(), String> {
        Ok(())
    }
}

struct Fixture {
    provider: Arc<Provider>,
    desktop: Arc<Desktop>,
    source: Panel,
    popup: Rc<UserMenuController>,
    admitted: Rc<Cell<bool>>,
}

impl Fixture {
    fn new(bound: bool) -> Self {
        i_slint_backend_testing::init_no_event_loop();
        let provider = Arc::new(Provider::default());
        let desktop = Arc::new(Desktop {
            provider: Mutex::new(Ok(Some(provider.clone()))),
            calls: AtomicUsize::new(0),
            folders: Arc::new(Folders::default()),
        });
        let source = Panel::new().unwrap();
        source.window().set_size(PhysicalSize::new(600, 600));
        source.show().unwrap();
        let popup = UserMenuController::new(desktop.clone()).unwrap();
        let admitted = Rc::new(Cell::new(true));
        if bound {
            let flag = admitted.clone();
            let weak_source = source.as_weak();
            popup.bind_profile_admission(move || {
                hook(&ADMISSION);
                flag.get()
                    && weak_source
                        .upgrade()
                        .is_some_and(|source| source.window().is_visible())
            });
        }
        Self {
            provider,
            desktop,
            source,
            popup,
            admitted,
        }
    }

    fn show(&self) {
        self.popup
            .show(self.source.window(), bounds(), context(), "Caller identity")
            .unwrap();
        self.popup.component().invoke_folder_event_ready();
    }

    fn drain(&self) {
        self.popup.component().invoke_profile_event_ready();
    }
    fn load(&self, value: ProfileSnapshot) {
        self.show();
        self.provider.read(Ok(value));
        assert!(self.popup.component().get_profile_loading());
        self.drain();
    }
    fn home(&self) {
        self.popup.component().invoke_profile_open_requested(
            UserProfileAction::Home,
            self.popup.component().get_profile_key(),
        );
    }
}

fn context() -> DockContext {
    DockContext::new(0, 0, 1920, 1080, false).unwrap()
}
fn bounds() -> TileBounds {
    TileBounds {
        origin: LogicalPosition::new(20.0, 300.0),
        width: 150.0,
        height: 40.0,
    }
}
fn error(kind: ProfileErrorKind) -> ProfileError {
    ProfileError::new(kind, "Private profile data must not reach UI", Some(5))
}
fn snapshot(revision: u64, photo: ProfilePhotoState) -> ProfileSnapshot {
    ProfileSnapshot::new(
        format!("Genuine user {revision}"),
        photo,
        Some(format!("personal-{revision}@fixture.invalid")),
        Some(ProfileTarget::new(revision)),
    )
    .unwrap()
}
fn photo() -> ProfilePhotoState {
    ProfilePhotoState::Ready(
        ProfilePhoto::new(
            2,
            2,
            vec![
                128, 0, 0, 128, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        )
        .unwrap(),
    )
}

#[test]
fn unbound_profile_is_fail_closed_without_replacing_folder_projection() {
    let fixture = Fixture::new(false);
    fixture.show();
    assert_eq!(fixture.desktop.calls.load(Ordering::Relaxed), 0);
    assert!(fixture.provider.requests().is_empty());
    assert!(fixture.popup.component().get_profile_status().is_empty());
    assert!(fixture.popup.component().get_personal_email().is_empty());
    assert!(!fixture.popup.component().get_profile_loading());
    let row = fixture.popup.component().get_rows().row_data(0).unwrap();
    fixture
        .popup
        .component()
        .invoke_folder_open_requested(row.kind, row.key);
    assert_eq!(&*fixture.desktop.folders.0.lock(), &[FolderId::Recent]);
}

#[test]
fn bound_profile_factory_is_lazy_and_premultiplied_photo_and_email_are_genuine() {
    let fixture = Fixture::new(true);
    assert_eq!(fixture.desktop.calls.load(Ordering::Relaxed), 0);
    fixture.load(snapshot(1, photo()));
    assert_eq!(fixture.desktop.calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        fixture.popup.component().get_profile_name(),
        "Genuine user 1"
    );
    assert!(fixture.popup.component().get_has_photo());
    assert!(!fixture.popup.component().get_profile_fallback());
    let size = fixture.popup.component().get_profile_photo().size();
    assert_eq!(
        (size.width, size.height),
        (512, 512),
        "cached renderer-independent circular presentation"
    );
    let state = fixture.popup.profile.state.borrow();
    let ProfilePhotoState::Ready(canonical) = state.confirmed.as_ref().unwrap().snapshot.photo()
    else {
        panic!("canonical genuine photo");
    };
    assert_eq!((canonical.width(), canonical.height()), (2, 2));
    assert_eq!(
        canonical.rgba(),
        &[
            128, 0, 0, 128, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255
        ]
    );
    assert_eq!(
        fixture.popup.component().get_personal_email(),
        "personal-1@fixture.invalid"
    );
}

#[test]
fn profile_inline_rejections_and_worker_results_publish_only_through_bounded_mailbox() {
    let fixture = Fixture::new(true);
    fixture
        .provider
        .0
        .lock()
        .replies
        .push_back(Reply::Reject(error(ProfileErrorKind::AccessDenied)));
    fixture.show();
    assert!(fixture.popup.component().get_profile_loading());
    fixture.drain();
    assert!(fixture.popup.component().get_profile_retry_enabled());
    assert!(
        !fixture
            .popup
            .component()
            .get_profile_status()
            .contains("Private")
    );
    fixture
        .provider
        .0
        .lock()
        .replies
        .push_back(Reply::ReadInline(Ok(snapshot(2, photo()))));
    let key = fixture.popup.component().get_profile_retry_key();
    fixture
        .popup
        .component()
        .invoke_profile_retry_requested(key);
    assert!(fixture.popup.component().get_profile_loading());
    assert!(!fixture.popup.component().get_has_photo());
    fixture.drain();
    fixture
        .provider
        .0
        .lock()
        .replies
        .push_back(Reply::OpenInline(Ok(())));
    fixture.home();
    assert!(fixture.popup.component().get_profile_opening());
    fixture.drain();
    assert!(!fixture.popup.component().get_profile_opening());
    fixture.home();
    let Callback::Open(callback) = fixture.provider.take() else {
        panic!("open")
    };
    std::thread::spawn(move || callback(Ok(()))).join().unwrap();
    assert!(fixture.popup.component().get_profile_opening());
    fixture.drain();
    assert_eq!(fixture.provider.0.lock().maximum, 1);
}

#[test]
fn hidden_profile_flight_survives_reopen_stale_photo_is_discarded_and_folder_keys_are_unchanged_by_profile()
 {
    let fixture = Fixture::new(true);
    fixture.show();
    fixture.popup.hide();
    fixture.show();
    assert_eq!(fixture.provider.requests(), [Recorded::Read]);
    let row = fixture.popup.component().get_rows().row_data(0).unwrap();
    fixture.provider.read(Ok(snapshot(99, photo())));
    fixture.drain();
    assert_eq!(
        fixture.provider.requests(),
        [Recorded::Read, Recorded::Read]
    );
    assert!(fixture.popup.component().get_profile_name().is_empty());
    assert!(!fixture.popup.component().get_has_photo());
    fixture
        .provider
        .read(Ok(snapshot(2, ProfilePhotoState::Absent)));
    fixture.drain();
    assert_eq!(
        fixture
            .popup
            .component()
            .get_rows()
            .row_data(0)
            .unwrap()
            .key,
        row.key
    );
    assert_eq!(
        fixture.popup.component().get_profile_name(),
        "Genuine user 2"
    );
    assert!(fixture.popup.component().get_profile_fallback());
    assert_eq!(fixture.provider.0.lock().maximum, 1);
}

#[test]
fn persistent_profile_open_busy_does_not_disable_or_retoken_working_folders_and_never_replays_open()
{
    let fixture = Fixture::new(true);
    fixture.load(snapshot(1, photo()));
    let row = fixture.popup.component().get_rows().row_data(0).unwrap();
    fixture.home();
    fixture.home();
    fixture
        .popup
        .component()
        .invoke_folder_open_requested(row.kind, row.key.clone());
    fixture.popup.component().invoke_folder_event_ready();
    assert_eq!(&*fixture.desktop.folders.0.lock(), &[FolderId::Recent]);
    assert_eq!(
        fixture
            .popup
            .component()
            .get_rows()
            .row_data(0)
            .unwrap()
            .key,
        row.key
    );
    fixture.popup.hide();
    fixture.show();
    assert_eq!(
        fixture.provider.requests(),
        [Recorded::Read, Recorded::Home]
    );
    fixture
        .provider
        .open(Err(error(ProfileErrorKind::NotFound)));
    fixture.drain();
    assert_eq!(
        fixture.provider.requests(),
        [Recorded::Read, Recorded::Home, Recorded::Read]
    );
    assert!(fixture.popup.component().get_profile_status().is_empty());
    fixture
        .provider
        .read(Ok(snapshot(2, ProfilePhotoState::Absent)));
    fixture.drain();
    assert_eq!(fixture.provider.requests().len(), 3);
}

#[test]
fn current_closed_actions_and_opaque_onedrive_keys_only_no_hidden_stale_or_display_authority() {
    let fixture = Fixture::new(true);
    fixture.load(snapshot(1, ProfilePhotoState::Absent));
    let old = fixture.popup.component().get_onedrive_key();
    for forged in [
        "",
        "OneDrive",
        "C:\\Users\\person\\OneDrive",
        "1",
        "Genuine user 1",
    ] {
        fixture
            .popup
            .component()
            .invoke_onedrive_open_requested(forged.into());
        fixture
            .popup
            .component()
            .invoke_profile_open_requested(UserProfileAction::Home, forged.into());
    }
    assert_eq!(fixture.provider.requests(), [Recorded::Read]);
    fixture
        .popup
        .component()
        .invoke_onedrive_open_requested(old.clone());
    assert_eq!(
        fixture.provider.requests().last(),
        Some(&Recorded::OneDrive(1))
    );
    fixture.provider.open(Ok(()));
    fixture.drain();
    fixture.popup.component().invoke_profile_open_requested(
        UserProfileAction::Accounts,
        fixture.popup.component().get_profile_key(),
    );
    assert_eq!(
        fixture.provider.requests().last(),
        Some(&Recorded::Accounts)
    );
    fixture.provider.open(Ok(()));
    fixture.drain();
    fixture.popup.hide();
    fixture.show();
    fixture.provider.read(Ok(snapshot(2, photo())));
    fixture.drain();
    fixture
        .popup
        .component()
        .invoke_onedrive_open_requested(old);
    assert_eq!(fixture.provider.requests().len(), 4);
    let current = fixture.popup.component().get_onedrive_key();
    fixture.popup.hide();
    fixture
        .popup
        .component()
        .invoke_onedrive_open_requested(current);
    fixture.home();
    assert_eq!(fixture.provider.requests().len(), 4);
}

#[test]
fn factory_and_request_hide_reopen_reentry_never_cache_or_apply_old_private_results() {
    let fixture = Fixture::new(true);
    let popup = fixture.popup.clone();
    let source = fixture.source.clone_strong();
    FACTORY.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            popup.hide();
            popup
                .show(source.window(), bounds(), context(), "Replacement identity")
                .unwrap();
        }))
    });
    fixture.show();
    assert_eq!(
        fixture.desktop.calls.load(Ordering::Relaxed),
        2,
        "stale factory result discarded"
    );
    assert_eq!(fixture.provider.requests(), [Recorded::Read]);
    fixture.provider.read(Ok(snapshot(1, photo())));
    fixture.drain();
    let popup = fixture.popup.clone();
    let source = fixture.source.clone_strong();
    REQUEST.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            popup.hide();
            popup
                .show(source.window(), bounds(), context(), "Reopened identity")
                .unwrap();
        }))
    });
    fixture.home();
    fixture.provider.open(Ok(()));
    fixture.drain();
    assert_eq!(
        fixture.provider.requests(),
        [Recorded::Read, Recorded::Home, Recorded::Read]
    );
    assert!(fixture.popup.component().get_profile_name().is_empty());
    assert!(fixture.popup.component().get_personal_email().is_empty());
    assert_eq!(fixture.provider.0.lock().maximum, 1);
}

#[test]
fn admission_reentry_and_revoked_or_hidden_source_scope_fail_closed_after_return() {
    let fixture = Fixture::new(true);
    let popup = fixture.popup.clone();
    ADMISSION.with(|slot| *slot.borrow_mut() = Some(Box::new(move || popup.hide())));
    fixture.show();
    assert_eq!(fixture.desktop.calls.load(Ordering::Relaxed), 0);
    assert!(!fixture.popup.is_open());
    fixture.show();
    fixture.admitted.set(false);
    fixture.provider.read(Ok(snapshot(7, photo())));
    fixture.drain();
    assert!(fixture.popup.component().get_profile_name().is_empty());
    assert!(fixture.popup.component().get_personal_email().is_empty());
    fixture.home();
    assert_eq!(fixture.provider.requests(), [Recorded::Read]);
    fixture.popup.hide();
    fixture.admitted.set(true);
    fixture.show();
    fixture.source.hide().unwrap();
    fixture.provider.read(Ok(snapshot(8, photo())));
    fixture.drain();
    assert!(fixture.popup.component().get_personal_email().is_empty());
    assert!(!fixture.popup.component().get_has_photo());
}

#[test]
fn unavailable_and_absent_photos_preserve_genuine_identity_optional_personal_email_is_not_account_claim()
 {
    let fixture = Fixture::new(true);
    fixture.load(snapshot(
        1,
        ProfilePhotoState::Unavailable(ProfileErrorKind::AccessDenied),
    ));
    assert!(!fixture.popup.component().get_has_photo());
    assert!(fixture.popup.component().get_profile_fallback());
    assert_eq!(
        fixture.popup.component().get_profile_name(),
        "Genuine user 1"
    );
    assert_eq!(
        fixture.popup.component().get_personal_email(),
        "personal-1@fixture.invalid"
    );
    assert!(
        fixture
            .popup
            .component()
            .get_profile_status()
            .contains("photo unavailable")
    );
    fixture.popup.hide();
    fixture.show();
    fixture.provider.read(Ok(ProfileSnapshot::new(
        "語 שלום مستخدم".repeat(20),
        ProfilePhotoState::Absent,
        None,
        None,
    )
    .unwrap()));
    fixture.drain();
    assert!(fixture.popup.component().get_profile_name().chars().count() <= 129);
    assert!(fixture.popup.component().get_personal_email().is_empty());
    assert!(fixture.popup.component().get_onedrive_key().is_empty());
    assert!(fixture.popup.component().get_profile_fallback());
}

#[test]
fn provider_none_and_factory_failures_are_honest_explicit_retry_does_not_inventory_or_persist() {
    let fixture = Fixture::new(true);
    *fixture.desktop.provider.lock() = Ok(None);
    fixture.show();
    assert!(fixture.provider.requests().is_empty());
    assert!(
        fixture
            .popup
            .component()
            .get_profile_status()
            .contains("not supported")
    );
    assert!(!fixture.popup.component().get_profile_retry_enabled());
    fixture.popup.hide();
    *fixture.desktop.provider.lock() = Err(error(ProfileErrorKind::AccessDenied));
    fixture.show();
    assert!(fixture.popup.component().get_profile_retry_enabled());
    let key = fixture.popup.component().get_profile_retry_key();
    fixture
        .popup
        .component()
        .invoke_profile_retry_requested("Retry profile".into());
    assert_eq!(fixture.desktop.calls.load(Ordering::Relaxed), 2);
    *fixture.desktop.provider.lock() = Ok(Some(fixture.provider.clone()));
    fixture
        .popup
        .component()
        .invoke_profile_retry_requested(key);
    assert_eq!(fixture.provider.requests(), [Recorded::Read]);
}

#[test]
fn weak_drop_worker_completion_does_not_retain_controller_source_or_resurrect_private_projection() {
    let fixture = Fixture::new(true);
    fixture.show();
    let weak = Rc::downgrade(&fixture.popup);
    let root = fixture.popup.component().as_weak();
    let Callback::Read(callback) = fixture.provider.take() else {
        panic!("read")
    };
    drop(fixture);
    assert!(weak.upgrade().is_none());
    assert!(root.upgrade().is_none());
    std::thread::spawn(move || callback(Ok(snapshot(9, photo()))))
        .join()
        .unwrap();
    assert!(weak.upgrade().is_none());
}

#[test]
fn profile_identity_counters_exhaust_fail_closed_without_reusing_intent_or_calling_provider() {
    let fixture = Fixture::new(true);
    fixture.popup.profile.state.borrow_mut().session = u64::MAX;
    fixture.show();
    assert_eq!(fixture.desktop.calls.load(Ordering::Relaxed), 0);
    assert!(fixture.provider.requests().is_empty());
    fixture.home();
    assert!(fixture.provider.requests().is_empty());
}

#[test]
fn failed_onedrive_retires_only_target_preserves_genuine_photo_identity_and_requires_current_explicit_retry()
 {
    let fixture = Fixture::new(true);
    fixture.load(snapshot(1, photo()));
    let old = fixture.popup.component().get_onedrive_key();
    fixture
        .popup
        .component()
        .invoke_onedrive_open_requested(old.clone());
    fixture
        .provider
        .open(Err(error(ProfileErrorKind::NotFound)));
    fixture.drain();
    assert!(fixture.popup.component().get_has_photo());
    assert_eq!(
        fixture.popup.component().get_profile_name(),
        "Genuine user 1"
    );
    assert_eq!(
        fixture.popup.component().get_personal_email(),
        "personal-1@fixture.invalid"
    );
    assert!(!fixture.popup.component().get_onedrive_ready());
    assert!(fixture.popup.component().get_onedrive_key().is_empty());
    fixture
        .popup
        .component()
        .invoke_onedrive_open_requested(old);
    assert_eq!(
        fixture.provider.requests(),
        [Recorded::Read, Recorded::OneDrive(1)]
    );
    let retry = fixture.popup.component().get_profile_retry_key();
    fixture
        .popup
        .component()
        .invoke_profile_retry_requested(retry);
    assert_eq!(
        fixture.provider.requests(),
        [Recorded::Read, Recorded::OneDrive(1), Recorded::Read]
    );
    fixture
        .provider
        .read(Ok(snapshot(2, ProfilePhotoState::Absent)));
    fixture.drain();
    fixture
        .popup
        .component()
        .invoke_onedrive_open_requested(fixture.popup.component().get_onedrive_key());
    assert_eq!(
        fixture.provider.requests().last(),
        Some(&Recorded::OneDrive(2))
    );
}

#[test]
fn profile_setter_reentry_retires_old_photo_and_never_refits_or_closes_replacement_session() {
    let fixture = Fixture::new(true);
    fixture.show();
    let popup = Rc::downgrade(&fixture.popup);
    let source = fixture.source.as_weak();
    let fired = Rc::new(Cell::new(false));
    let callback_fired = fired.clone();
    fixture
        .popup
        .component()
        .on_preferred_size_changed(move || {
            if !callback_fired.replace(true)
                && let (Some(popup), Some(source)) = (popup.upgrade(), source.upgrade())
            {
                popup.hide();
                popup
                    .show(source.window(), bounds(), context(), "Setter replacement")
                    .unwrap();
            }
        });
    fixture.provider.read(Ok(snapshot(77, photo())));
    fixture.drain();
    // A property-induced size callback may be deferred by the toolkit. Force
    // its genuine generated callback, never call a production native effect.
    if !fired.get() {
        fixture.popup.component().invoke_preferred_size_changed();
    }
    assert!(fixture.popup.is_open());
    assert!(fixture.popup.component().get_profile_name().is_empty());
    assert!(fixture.popup.component().get_personal_email().is_empty());
    assert!(!fixture.popup.component().get_has_photo());
    assert_eq!(
        fixture.provider.requests(),
        [Recorded::Read, Recorded::Read]
    );
}
