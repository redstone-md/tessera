// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Event {
    Initialize(i32),
    Uninitialize,
    Pump,
    Create(&'static str),
    Release(&'static str),
    Languages,
    Names(u16),
    Enumerate(u16),
    Next,
    Description(Identity),
    ManagerActivate,
    ManagerDeactivate,
    Change(u16),
    Activate(Identity, u32),
    Settings(&'static str, Option<String>),
}

struct State {
    events: Vec<Event>,
    profiles: Vec<NativeProfile>,
    languages: Vec<u16>,
    failures: HashMap<&'static str, u32>,
    panic_at: Option<&'static str>,
    initialization: i32,
    change_status: i32,
    activation_status: i32,
    end_status: i32,
    end_fetched: u32,
    activation: Option<Identity>,
    confirm_active: bool,
    acquisitions: usize,
}

#[derive(Clone)]
struct Recording(Rc<RefCell<State>>);
impl Recording {
    fn new(profiles: Vec<NativeProfile>) -> Self {
        let mut languages = Vec::new();
        for row in &profiles {
            if !languages.contains(&row.identity.language) {
                languages.push(row.identity.language)
            }
        }
        Self(Rc::new(RefCell::new(State {
            events: Vec::new(),
            profiles,
            languages,
            failures: HashMap::new(),
            panic_at: None,
            initialization: 0,
            change_status: 0,
            activation_status: 0,
            end_status: 1,
            end_fetched: 0,
            activation: None,
            confirm_active: true,
            acquisitions: 0,
        })))
    }
    fn event(&self, event: Event) {
        self.0.borrow_mut().events.push(event)
    }
    fn hit(&self, point: &'static str) -> Result<(), u32> {
        let state = self.0.borrow();
        let panic = state.panic_at == Some(point);
        let failure = state.failures.get(point).copied();
        drop(state);
        assert!(!panic, "recorded native panic at {point}");
        failure.map_or(Ok(()), Err)
    }
    fn resource(&self, name: &'static str) -> Resource {
        self.event(Event::Create(name));
        Resource {
            recording: self.clone(),
            name,
        }
    }
    fn events(&self) -> Vec<Event> {
        self.0.borrow().events.clone()
    }
    fn switches(&self) -> Vec<Event> {
        self.events()
            .into_iter()
            .filter(|event| matches!(event, Event::Change(_) | Event::Activate(..)))
            .collect()
    }
    fn fail(&self, point: &'static str) {
        self.0.borrow_mut().failures.insert(point, 0x8000_4005);
    }
}

struct Resource {
    recording: Recording,
    name: &'static str,
}
impl Drop for Resource {
    fn drop(&mut self) {
        self.recording.event(Event::Release(self.name))
    }
}
struct Languages {
    values: Vec<u16>,
    _allocation: Resource,
}
impl AsRef<[u16]> for Languages {
    fn as_ref(&self) -> &[u16] {
        &self.values
    }
}
struct Enumerator {
    rows: Vec<NativeProfile>,
    cursor: Cell<usize>,
    _interface: Resource,
}

impl NativeCalls for Recording {
    type Session = Resource;
    type Languages = Languages;
    type Enumerator = Enumerator;
    type ThreadManager = Resource;
    fn initialize(&self, model: i32) -> i32 {
        self.event(Event::Initialize(model));
        self.0.borrow().initialization
    }
    fn uninitialize(&self) {
        self.event(Event::Uninitialize)
    }
    fn pump(&self) -> Result<(), u32> {
        self.event(Event::Pump);
        self.hit("pump")
    }
    fn session(&self) -> Result<Resource, u32> {
        self.hit("session")?;
        Ok(self.resource("session"))
    }
    fn languages(&self, _: &Resource) -> Result<Languages, u32> {
        self.event(Event::Languages);
        let allocation = self.resource("language allocation");
        // Model the SDK's immediate ownership even if GetLanguageList fails.
        self.hit("languages")?;
        let acquisition = {
            let mut state = self.0.borrow_mut();
            state.acquisitions += 1;
            state.acquisitions
        };
        if acquisition > 1 {
            self.hit("confirmation")?
        }
        Ok(Languages {
            values: self.0.borrow().languages.clone(),
            _allocation: allocation,
        })
    }
    fn names(&self, language: u16) -> LanguageNames {
        self.event(Event::Names(language));
        if self.hit("names").is_err() {
            return LanguageNames::default();
        }
        LanguageNames {
            code: "ja-JP".into(),
            display: "日本語 — 日本".into(),
            native: "日本語".into(),
        }
    }
    fn enumerate(&self, _: &Resource, language: u16) -> Result<Enumerator, u32> {
        self.event(Event::Enumerate(language));
        self.hit("enumerate")?;
        let state = self.0.borrow();
        let rows = state
            .profiles
            .iter()
            .filter(|row| row.identity.language == language)
            .map(|row| {
                let mut row = *row;
                if state.confirm_active && row.enabled_identity() == state.activation {
                    row.flags |= ACTIVE;
                }
                row
            })
            .collect();
        drop(state);
        Ok(Enumerator {
            rows,
            cursor: Cell::new(0),
            _interface: self.resource("enumerator"),
        })
    }
    fn next(&self, enumerator: &Enumerator) -> Fetch {
        self.event(Event::Next);
        let failure = self.hit("next").err();
        let index = enumerator.cursor.get();
        enumerator.cursor.set(index + 1);
        let state = self.0.borrow();
        if let Some(row) = enumerator.rows.get(index) {
            Fetch {
                status: failure.map_or(0, |code| code as i32),
                fetched: 1,
                profile: *row,
            }
        } else {
            Fetch {
                status: failure.map_or(state.end_status, |code| code as i32),
                fetched: state.end_fetched,
                profile: ordinary(0, 0, 0),
            }
        }
    }
    fn description(&self, _: &Resource, identity: Identity) -> String {
        self.event(Event::Description(identity));
        if self.hit("description").is_err() {
            return String::new();
        }
        "キーボード — Écriture".into()
    }
    fn thread_manager(&self) -> Result<Resource, u32> {
        self.hit("manager")?;
        Ok(self.resource("manager"))
    }
    fn activate_manager(&self, _: &Resource) -> Result<(), u32> {
        self.event(Event::ManagerActivate);
        self.hit("manager activate")
    }
    fn deactivate_manager(&self, _: &Resource) -> Result<(), u32> {
        self.event(Event::ManagerDeactivate);
        self.hit("manager deactivate")
    }
    fn change_language(&self, _: &Resource, language: u16) -> i32 {
        self.event(Event::Change(language));
        self.hit("change")
            .err()
            .map_or(self.0.borrow().change_status, |code| code as i32)
    }
    fn activate(&self, _: &Resource, identity: Identity, flags: u32) -> i32 {
        self.event(Event::Activate(identity, flags));
        let status = self
            .hit("activate")
            .err()
            .map_or(self.0.borrow().activation_status, |code| code as i32);
        if status == 0 {
            self.0.borrow_mut().activation = Some(identity)
        }
        status
    }
    fn settings(&self, uri: &'static str, parameters: Option<&str>) -> Result<(), u32> {
        self.event(Event::Settings(uri, parameters.map(str::to_owned)));
        self.hit("settings")
    }
}

fn ordinary(language: u16, layout: usize, flags: u32) -> NativeProfile {
    NativeProfile {
        identity: Identity {
            kind: KEYBOARD_LAYOUT,
            language,
            class: 0,
            profile: 0,
            layout,
        },
        category: 0,
        flags,
    }
}
fn tip(language: u16, class: u128, profile: u128, flags: u32) -> NativeProfile {
    NativeProfile {
        identity: Identity {
            kind: INPUT_PROCESSOR,
            language,
            class,
            profile,
            layout: 0,
        },
        category: KEYBOARD_CATEGORY,
        flags,
    }
}
fn action(row: NativeProfile) -> InputLanguageAction {
    InputLanguageAction::Activate {
        profile: row.enabled_identity().unwrap().id().unwrap(),
    }
}
fn count(events: &[Event], event: Event) -> usize {
    events.iter().filter(|item| **item == event).count()
}
fn assert_released_before_uninitialize(recording: &Recording) {
    let events = recording.events();
    assert_eq!(events.last(), Some(&Event::Uninitialize));
    for name in ["session", "language allocation", "enumerator", "manager"] {
        assert_eq!(
            count(&events, Event::Create(name)),
            count(&events, Event::Release(name)),
            "{name}"
        );
    }
}

#[test]
fn enabled_ordinary_and_keyboard_tip_preserve_native_order_unicode_and_active_flags() {
    let a = ordinary(0x411, 0x04110411, ENABLED | ACTIVE);
    let b = ordinary(0x411, 0xf0010411, ENABLED);
    let c = tip(0x409, 17, 23, ENABLED | ACTIVE);
    let mut speech = tip(0x409, 17, 24, ENABLED);
    speech.category = 0xb5a73cd1_8355_426b_a161_259808f26b14;
    let mut handwriting = speech;
    handwriting.category = 0x246ecb87_c2f2_4abe_905b_c8b38add2c43;
    let recording = Recording::new(vec![
        a,
        b,
        a,
        ordinary(0x411, 44, 0),
        c,
        speech,
        handwriting,
        tip(0x409, 17, 25, 0),
    ]);
    let snapshot = read_with(&recording).unwrap();
    assert_eq!(snapshot.languages.len(), 2);
    assert_eq!(snapshot.languages[0].profiles.len(), 2);
    assert_eq!(snapshot.languages[1].profiles.len(), 1);
    assert_eq!(snapshot.languages[0].name, "日本語 — 日本");
    assert_eq!(snapshot.languages[0].native_name, "日本語");
    assert_eq!(
        snapshot.languages[0].profiles[0].display_name,
        "キーボード — Écriture"
    );
    assert!(snapshot.languages[0].profiles[0].active);
    assert!(!snapshot.languages[0].profiles[1].active);
    assert!(snapshot.languages[1].profiles[0].active);
    assert_eq!(
        snapshot.languages[0].profiles[0].id,
        a.identity.id().unwrap()
    );
    assert_eq!(
        snapshot.languages[0].profiles[1].id,
        b.identity.id().unwrap()
    );
    assert!(recording.switches().is_empty());
    assert!(!recording.events().contains(&Event::ManagerActivate));
    assert_released_before_uninitialize(&recording);
}

#[test]
fn empty_and_filtered_catalogs_are_successful_empty_not_invented_data() {
    for profiles in [vec![], vec![ordinary(0x409, 99, 0)]] {
        let recording = Recording::new(profiles);
        assert!(read_with(&recording).unwrap().languages.is_empty());
        assert!(recording.switches().is_empty());
        assert_released_before_uninitialize(&recording);
    }
}

#[test]
fn metadata_failures_are_empty_observations_not_locale_fallbacks() {
    let recording = Recording::new(vec![ordinary(0x409, 99, ENABLED)]);
    recording.fail("names");
    recording.fail("description");
    let snapshot = read_with(&recording).unwrap();
    let language = &snapshot.languages[0];
    assert!(
        language.code.is_empty() && language.name.is_empty() && language.native_name.is_empty()
    );
    assert!(language.profiles[0].display_name.is_empty());
}

#[test]
fn exact_identity_is_stable_not_label_index_or_langid_only() {
    let a = ordinary(0x409, 0xf0010409, ENABLED);
    let b = ordinary(0x409, 0xf0020409, ENABLED);
    let c = tip(0x409, 17, 23, ENABLED);
    let d = tip(0x409, 17, 24, ENABLED);
    assert_ne!(a.identity.id().unwrap(), b.identity.id().unwrap());
    assert_ne!(c.identity.id().unwrap(), d.identity.id().unwrap());
    let first = read_with(&Recording::new(vec![a, b])).unwrap();
    let second = read_with(&Recording::new(vec![b, a])).unwrap();
    assert_eq!(
        first.languages[0].profiles[0].id,
        second.languages[0].profiles[1].id
    );
}

#[test]
fn native_unused_identity_fields_are_canonicalized_to_exact_switch_tuple() {
    let mut ordinary_row = ordinary(0x409, 55, ENABLED);
    ordinary_row.identity.class = 123;
    ordinary_row.identity.profile = 456;
    let mut tip_row = tip(0x409, 17, 23, ENABLED);
    tip_row.identity.layout = 999;
    for row in [ordinary_row, tip_row] {
        let recording = Recording::new(vec![row]);
        execute_with(&recording, action(row)).unwrap();
        let identity = row.enabled_identity().unwrap();
        assert_eq!(
            recording.switches(),
            vec![
                Event::Change(0x409),
                Event::Activate(identity, DESKTOP_SCOPE)
            ]
        );
        if identity.kind == KEYBOARD_LAYOUT {
            assert_eq!((identity.class, identity.profile), (0, 0))
        } else {
            assert_eq!(identity.layout, 0)
        }
    }
}

#[test]
fn forged_removed_disabled_and_category_changed_ids_have_zero_switch_calls() {
    let wanted = tip(0x409, 17, 23, ENABLED);
    let mut disabled = wanted;
    disabled.flags = 0;
    let mut speech = wanted;
    speech.category = 77;
    for rows in [
        vec![],
        vec![disabled],
        vec![speech],
        vec![tip(0x409, 17, 24, ENABLED)],
    ] {
        let recording = Recording::new(rows);
        assert_eq!(
            execute_with(&recording, action(wanted)).unwrap_err().kind,
            InputLanguageErrorKind::ProfileChanged
        );
        assert!(recording.switches().is_empty());
        assert!(!recording.events().contains(&Event::ManagerActivate));
    }
    let recording = Recording::new(vec![wanted]);
    let forged = InputLanguageAction::Activate {
        profile: ProfileId::new("not-issued").unwrap(),
    };
    assert_eq!(
        execute_with(&recording, forged).unwrap_err().kind,
        InputLanguageErrorKind::ProfileChanged
    );
    assert!(recording.switches().is_empty());
}

#[test]
fn ordinary_and_tip_switch_once_then_return_observed_confirmation() {
    for row in [
        ordinary(0x409, 0x04090409, ENABLED),
        tip(0x411, 17, 23, ENABLED),
    ] {
        let recording = Recording::new(vec![row]);
        let InputLanguageOutcome::Snapshot(snapshot) =
            execute_with(&recording, action(row)).unwrap()
        else {
            panic!("expected observation")
        };
        assert!(snapshot.languages[0].profiles[0].active);
        assert_eq!(
            recording.switches(),
            vec![
                Event::Change(row.identity.language),
                Event::Activate(row.identity, 0x3000_0000)
            ]
        );
        let events = recording.events();
        assert_eq!(count(&events, Event::ManagerActivate), 1);
        assert_eq!(count(&events, Event::ManagerDeactivate), 1);
        assert_eq!(count(&events, Event::Languages), 2);
        let revalidated = events
            .iter()
            .position(|event| *event == Event::Release("language allocation"))
            .unwrap();
        let switched = events
            .iter()
            .position(|event| *event == Event::Change(row.identity.language))
            .unwrap();
        assert!(revalidated < switched);
        assert_released_before_uninitialize(&recording);
    }
}

#[test]
fn activation_s_false_and_negative_hresult_report_partial_effect_without_retry_or_rollback() {
    for status in [1, 0x8000_4005u32 as i32, 0x8007_0005u32 as i32] {
        let row = ordinary(0x409, 55, ENABLED);
        let recording = Recording::new(vec![row]);
        recording.0.borrow_mut().activation_status = status;
        let error = execute_with(&recording, action(row)).unwrap_err();
        assert!(error.message.contains("language changed"));
        assert!(error.message.contains("no rollback or retry"));
        if status == 1 {
            assert_eq!(error.kind, InputLanguageErrorKind::ProfileChanged)
        }
        assert_eq!(
            recording.switches(),
            vec![
                Event::Change(0x409),
                Event::Activate(row.identity, DESKTOP_SCOPE)
            ]
        );
        assert_eq!(count(&recording.events(), Event::Languages), 1);
        assert_eq!(count(&recording.events(), Event::ManagerDeactivate), 1);
        assert_released_before_uninitialize(&recording);
    }
}

#[test]
fn language_change_failure_including_s_false_never_activates_profile() {
    for status in [1, 0x8000_ffffu32 as i32] {
        let row = ordinary(0x409, 55, ENABLED);
        let recording = Recording::new(vec![row]);
        recording.0.borrow_mut().change_status = status;
        assert!(execute_with(&recording, action(row)).is_err());
        assert_eq!(recording.switches(), vec![Event::Change(0x409)]);
        assert_eq!(count(&recording.events(), Event::ManagerDeactivate), 1);
        assert_released_before_uninitialize(&recording);
    }
}

#[test]
fn successful_activation_with_failed_or_inactive_confirmation_is_truthful_error() {
    for confirmation_failure in [true, false] {
        let row = ordinary(0x409, 55, ENABLED);
        let recording = Recording::new(vec![row]);
        if confirmation_failure {
            recording.fail("confirmation")
        } else {
            recording.0.borrow_mut().confirm_active = false
        }
        let error = execute_with(&recording, action(row)).unwrap_err();
        assert!(
            error
                .message
                .contains("activation succeeded, but confirmation is unavailable")
        );
        assert_eq!(recording.switches().len(), 2);
        assert_eq!(count(&recording.events(), Event::ManagerDeactivate), 1);
        assert_released_before_uninitialize(&recording);
    }
}

#[test]
fn com_s_ok_s_false_each_balance_once_and_changed_mode_balances_nothing() {
    for status in [0, 1] {
        let recording = Recording::new(vec![]);
        recording.0.borrow_mut().initialization = status;
        read_with(&recording).unwrap();
        assert_eq!(count(&recording.events(), Event::Initialize(STA)), 1);
        assert_eq!(count(&recording.events(), Event::Uninitialize), 1);
    }
    let recording = Recording::new(vec![]);
    recording.0.borrow_mut().initialization = 0x8001_0106u32 as i32;
    assert!(read_with(&recording).is_err());
    assert_eq!(recording.events(), vec![Event::Initialize(STA)]);
}

#[test]
fn enumeration_zero_fetched_terminates_with_s_ok_or_s_false() {
    for status in [0, 1] {
        let recording = Recording::new(vec![]);
        recording.0.borrow_mut().languages = vec![0x409];
        recording.0.borrow_mut().end_status = status;
        assert!(read_with(&recording).unwrap().languages.is_empty());
        assert_eq!(count(&recording.events(), Event::Next), 1);
        assert_released_before_uninitialize(&recording);
    }
}

#[test]
fn malformed_native_fetched_count_is_rejected_and_releases_all_resources() {
    let recording = Recording::new(vec![]);
    recording.0.borrow_mut().languages = vec![0x409];
    recording.0.borrow_mut().end_fetched = 2;
    assert_eq!(
        read_with(&recording).unwrap_err().kind,
        InputLanguageErrorKind::InvalidValue
    );
    assert_released_before_uninitialize(&recording);
}

#[test]
fn allocation_and_interfaces_release_on_every_read_failure_and_panic() {
    for point in ["session", "languages", "pump", "enumerate", "next"] {
        let recording = Recording::new(vec![ordinary(0x409, 55, ENABLED)]);
        recording.fail(point);
        assert!(read_with(&recording).is_err(), "{point}");
        assert_released_before_uninitialize(&recording);
    }
    for point in [
        "languages",
        "pump",
        "names",
        "enumerate",
        "next",
        "description",
    ] {
        let recording = Recording::new(vec![ordinary(0x409, 55, ENABLED)]);
        recording.0.borrow_mut().panic_at = Some(point);
        assert!(
            catch_unwind(AssertUnwindSafe(|| read_with(&recording))).is_err(),
            "{point}"
        );
        assert_released_before_uninitialize(&recording);
    }
}

#[test]
fn manager_activation_failures_do_not_own_a_deactivation_and_other_failures_do() {
    for point in [
        "manager",
        "manager activate",
        "change",
        "activate",
        "confirmation",
        "manager deactivate",
    ] {
        let row = ordinary(0x409, 55, ENABLED);
        let recording = Recording::new(vec![row]);
        recording.fail(point);
        assert!(execute_with(&recording, action(row)).is_err(), "{point}");
        let expected = usize::from(point != "manager" && point != "manager activate");
        assert_eq!(
            count(&recording.events(), Event::ManagerDeactivate),
            expected,
            "{point}"
        );
        if point == "manager" || point == "manager activate" {
            assert!(recording.switches().is_empty())
        }
        assert_released_before_uninitialize(&recording);
    }
}

#[test]
fn active_manager_and_interfaces_balance_during_action_unwinding() {
    for point in ["change", "activate", "confirmation"] {
        let row = ordinary(0x409, 55, ENABLED);
        let recording = Recording::new(vec![row]);
        recording.0.borrow_mut().panic_at = Some(point);
        assert!(catch_unwind(AssertUnwindSafe(|| execute_with(&recording, action(row)))).is_err());
        assert_eq!(count(&recording.events(), Event::ManagerDeactivate), 1);
        assert_released_before_uninitialize(&recording);
    }
}

#[test]
fn settings_is_fixed_parameterless_dispatch_without_profile_or_switch_calls() {
    for fail in [false, true] {
        let recording = Recording::new(vec![]);
        if fail {
            recording.fail("settings")
        }
        let result = execute_with(&recording, InputLanguageAction::OpenKeyboardSettings);
        if fail {
            assert!(result.is_err())
        } else {
            assert_eq!(result.unwrap(), InputLanguageOutcome::SettingsDispatched)
        }
        assert_eq!(
            recording.events(),
            vec![
                Event::Initialize(STA),
                Event::Settings("ms-settings:keyboard", None),
                Event::Uninitialize
            ]
        );
    }
}

#[test]
fn duplicate_languages_unknown_types_null_hkls_and_no_active_flag_stay_honest() {
    let valid = ordinary(0x409, 55, ENABLED);
    let mut unknown = valid;
    unknown.identity.kind = 77;
    let recording = Recording::new(vec![valid, unknown, ordinary(0x409, 0, ENABLED)]);
    recording.0.borrow_mut().languages = vec![0x409, 0x409];
    let snapshot = read_with(&recording).unwrap();
    assert_eq!(snapshot.languages.len(), 1);
    assert_eq!(snapshot.languages[0].profiles.len(), 1);
    assert!(!snapshot.languages[0].profiles[0].active);
    assert_eq!(count(&recording.events(), Event::Enumerate(0x409)), 1);
    assert!(recording.switches().is_empty());
}

#[test]
fn previously_observed_ordinary_profile_is_revalidated_after_removal_or_disable() {
    let row = ordinary(0x409, 55, ENABLED);
    for disable in [false, true] {
        let recording = Recording::new(vec![row]);
        let id = read_with(&recording).unwrap().languages[0].profiles[0]
            .id
            .clone();
        if disable {
            recording.0.borrow_mut().profiles[0].flags = 0;
        } else {
            recording.0.borrow_mut().profiles.clear();
        }
        let result = execute_with(&recording, InputLanguageAction::Activate { profile: id });
        assert_eq!(
            result.unwrap_err().kind,
            InputLanguageErrorKind::ProfileChanged
        );
        assert!(recording.switches().is_empty());
        assert!(!recording.events().contains(&Event::ManagerActivate));
        assert_released_before_uninitialize(&recording);
    }
}
