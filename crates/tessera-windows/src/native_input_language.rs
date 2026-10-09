// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Request-local TSF objects; only copied observations leave the STA worker.

use std::{collections::HashSet, marker::PhantomData, rc::Rc};
use tessera_system::input_language::{
    InputLanguage, InputLanguageAction, InputLanguageError, InputLanguageErrorKind,
    InputLanguageOutcome, InputLanguageSnapshot, InputProfile, ProfileId,
};

const STA: i32 = 2;
const KEYBOARD_LAYOUT: u32 = 2;
const INPUT_PROCESSOR: u32 = 1;
const KEYBOARD_CATEGORY: u128 = 0x34745c63_b2f0_4784_8b67_5e12c8701a31;
const ENABLED: u32 = 2;
const ACTIVE: u32 = 1;
const DESKTOP_SCOPE: u32 = 0x3000_0000; // FORPROCESS | FORSESSION only.
const SETTINGS_URI: &str = "ms-settings:keyboard";
const MAX_PROFILES: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Identity {
    kind: u32,
    language: u16,
    class: u128,
    profile: u128,
    layout: usize,
}

impl Identity {
    fn id(self) -> Result<ProfileId, InputLanguageError> {
        // Stable, native-issued identity. Never parsed to obtain switch operands;
        // activation resolves it exclusively against a freshly enumerated table.
        ProfileId::new(format!(
            "tsf-v1:{:08x}:{:04x}:{:032x}:{:032x}:{:016x}",
            self.kind, self.language, self.class, self.profile, self.layout
        ))
    }
}

#[derive(Clone, Copy, Debug)]
struct NativeProfile {
    identity: Identity,
    category: u128,
    flags: u32,
}

impl NativeProfile {
    fn enabled_identity(self) -> Option<Identity> {
        if self.flags & ENABLED == 0 {
            return None;
        }
        match self.identity.kind {
            KEYBOARD_LAYOUT if self.identity.layout != 0 => Some(Identity {
                class: 0,
                profile: 0,
                ..self.identity
            }),
            INPUT_PROCESSOR if self.category == KEYBOARD_CATEGORY => Some(Identity {
                layout: 0,
                ..self.identity
            }),
            _ => None,
        }
    }
}

#[derive(Default)]
struct LanguageNames {
    code: String,
    display: String,
    native: String,
}

struct Fetch {
    status: i32,
    fetched: u32,
    profile: NativeProfile,
}

/// Private seam retains raw HRESULTs and owned allocation/interface lifetimes.
trait NativeCalls {
    type Session;
    type Languages: AsRef<[u16]>;
    type Enumerator;
    type ThreadManager;

    fn initialize(&self, model: i32) -> i32;
    fn uninitialize(&self);
    fn pump(&self) -> Result<(), u32>;
    fn session(&self) -> Result<Self::Session, u32>;
    fn languages(&self, session: &Self::Session) -> Result<Self::Languages, u32>;
    fn names(&self, language: u16) -> LanguageNames;
    fn enumerate(&self, session: &Self::Session, language: u16) -> Result<Self::Enumerator, u32>;
    fn next(&self, enumerator: &Self::Enumerator) -> Fetch;
    fn description(&self, session: &Self::Session, identity: Identity) -> String;
    fn thread_manager(&self) -> Result<Self::ThreadManager, u32>;
    fn activate_manager(&self, manager: &Self::ThreadManager) -> Result<(), u32>;
    fn deactivate_manager(&self, manager: &Self::ThreadManager) -> Result<(), u32>;
    fn change_language(&self, session: &Self::Session, language: u16) -> i32;
    fn activate(&self, session: &Self::Session, identity: Identity, flags: u32) -> i32;
    fn settings(&self, uri: &'static str, parameters: Option<&str>) -> Result<(), u32>;
}

fn native_error(operation: &str, code: u32) -> InputLanguageError {
    let kind = match code {
        5 | 0x8007_0005 => InputLanguageErrorKind::AccessDenied,
        0x8001_0106 => InputLanguageErrorKind::Unavailable, // RPC_E_CHANGED_MODE
        _ => InputLanguageErrorKind::Unavailable,
    };
    InputLanguageError::new(kind, format!("{operation} failed (native 0x{code:08X})."))
}

struct Apartment<'a, C: NativeCalls> {
    calls: &'a C,
    _thread_bound: PhantomData<Rc<()>>,
}

impl<'a, C: NativeCalls> Apartment<'a, C> {
    fn enter(calls: &'a C) -> Result<Self, InputLanguageError> {
        let result = calls.initialize(STA);
        if result < 0 {
            return Err(native_error(
                "Input language STA initialization",
                result as u32,
            ));
        }
        Ok(Self {
            calls,
            _thread_bound: PhantomData,
        })
    }
}

impl<C: NativeCalls> Drop for Apartment<'_, C> {
    fn drop(&mut self) {
        self.calls.uninitialize();
    }
}

struct ActiveManager<'a, C: NativeCalls> {
    calls: &'a C,
    manager: C::ThreadManager,
    active: bool,
}

impl<'a, C: NativeCalls> ActiveManager<'a, C> {
    fn enter(calls: &'a C) -> Result<Self, InputLanguageError> {
        let manager = calls
            .thread_manager()
            .map_err(|code| native_error("Input language thread manager creation", code))?;
        calls
            .activate_manager(&manager)
            .map_err(|code| native_error("Input language thread manager activation", code))?;
        Ok(Self {
            calls,
            manager,
            active: true,
        })
    }

    fn finish(&mut self) -> Result<(), InputLanguageError> {
        self.active = false; // Even failure is not automatically retried.
        self.calls
            .deactivate_manager(&self.manager)
            .map_err(|code| native_error("Input language thread manager deactivation", code))
    }
}

impl<C: NativeCalls> Drop for ActiveManager<'_, C> {
    fn drop(&mut self) {
        if self.active {
            let _ = self.calls.deactivate_manager(&self.manager);
        }
    }
}

struct Catalog {
    snapshot: InputLanguageSnapshot,
    identities: Vec<(ProfileId, Identity)>,
}

fn acquire<C: NativeCalls>(calls: &C, session: &C::Session) -> Result<Catalog, InputLanguageError> {
    // The SDK wraps GetLanguageList's allocation before checking HRESULT. This
    // owning value outlives all fallible locale/enumerator/description work.
    let languages = calls
        .languages(session)
        .map_err(|code| native_error("Input language list", code))?;
    let mut result = Catalog {
        snapshot: InputLanguageSnapshot::default(),
        identities: Vec::new(),
    };
    let mut seen_languages = HashSet::new();
    let mut seen_profiles = HashSet::new();
    let mut visited = 0;
    for &language in languages.as_ref() {
        if !seen_languages.insert(language) {
            continue;
        }
        calls
            .pump()
            .map_err(|code| native_error("Input language message pump", code))?;
        let names = calls.names(language);
        let mut group = InputLanguage {
            id: format!("tsf-language-v1:{language:04x}"),
            code: names.code,
            name: names.display,
            native_name: names.native,
            profiles: Vec::new(),
        };
        let enumerator = calls
            .enumerate(session, language)
            .map_err(|code| native_error("Input language profile enumeration", code))?;
        loop {
            calls
                .pump()
                .map_err(|code| native_error("Input language message pump", code))?;
            let fetched = calls.next(&enumerator);
            if fetched.status < 0 {
                return Err(native_error(
                    "Input language profile Next",
                    fetched.status as u32,
                ));
            }
            if fetched.fetched == 0 {
                break; // S_FALSE / no fetched row is termination, never a zeroed row.
            }
            if fetched.fetched != 1 || fetched.profile.identity.language != language {
                return Err(InputLanguageError::new(
                    InputLanguageErrorKind::InvalidValue,
                    "Invalid native input profile enumeration.",
                ));
            }
            visited += 1;
            if visited > MAX_PROFILES {
                return Err(InputLanguageError::new(
                    InputLanguageErrorKind::Unavailable,
                    "Input profile enumeration exceeded its bounded capacity.",
                ));
            }
            let Some(identity) = fetched.profile.enabled_identity() else {
                continue;
            };
            if !seen_profiles.insert(identity) {
                continue;
            }
            let id = identity.id()?;
            group.profiles.push(InputProfile {
                id: id.clone(),
                display_name: calls.description(session, identity),
                active: fetched.profile.flags & ACTIVE != 0,
            });
            result.identities.push((id, identity));
        }
        if !group.profiles.is_empty() {
            result.snapshot.languages.push(group);
        }
    }
    Ok(result)
}

fn read_with<C: NativeCalls>(calls: &C) -> Result<InputLanguageSnapshot, InputLanguageError> {
    let _apartment = Apartment::enter(calls)?;
    let session = calls
        .session()
        .map_err(|code| native_error("Input language TSF creation", code))?;
    Ok(acquire(calls, &session)?.snapshot)
}

fn execute_with<C: NativeCalls>(
    calls: &C,
    action: InputLanguageAction,
) -> Result<InputLanguageOutcome, InputLanguageError> {
    let _apartment = Apartment::enter(calls)?;
    if matches!(action, InputLanguageAction::OpenKeyboardSettings) {
        calls
            .settings(SETTINGS_URI, None)
            .map_err(|code| native_error("Keyboard Settings dispatch", code))?;
        return Ok(InputLanguageOutcome::SettingsDispatched);
    }
    let InputLanguageAction::Activate { profile } = action else {
        unreachable!()
    };
    let session = calls
        .session()
        .map_err(|code| native_error("Input language TSF creation", code))?;
    let catalog = acquire(calls, &session)?;
    let identity = catalog
        .identities
        .iter()
        .find(|(id, _)| *id == profile)
        .map(|(_, identity)| *identity)
        .ok_or_else(|| {
            InputLanguageError::new(
                InputLanguageErrorKind::ProfileChanged,
                "The requested input profile is no longer enabled or available.",
            )
        })?;
    // No language-changing calls before the exact enabled native table lookup.
    let mut manager = ActiveManager::enter(calls)?;
    calls
        .pump()
        .map_err(|code| native_error("Input language message pump", code))?;
    let changed = calls.change_language(&session, identity.language);
    if changed != 0 {
        return Err(native_error(
            "Input language ChangeCurrentLanguage",
            changed as u32,
        ));
    }
    let status = calls.activate(&session, identity, DESKTOP_SCOPE);
    if status != 0 {
        let kind = if status == 1 {
            InputLanguageErrorKind::ProfileChanged
        } else {
            native_error("Input language ActivateProfile", status as u32).kind
        };
        return Err(InputLanguageError::new(
            kind,
            format!(
                "Input language changed, but profile activation failed (HRESULT 0x{:08X}); no rollback or retry was attempted.",
                status as u32
            ),
        ));
    }
    // Success is confirmed by a fresh enumeration, never an inferred active row.
    let confirmation = (|| {
        calls
            .pump()
            .map_err(|code| native_error("Input language message pump", code))?;
        let observed = acquire(calls, &session)?;
        let active = observed
            .snapshot
            .languages
            .iter()
            .flat_map(|group| &group.profiles)
            .any(|row| row.id == profile && row.active);
        if !active {
            return Err(InputLanguageError::new(
                InputLanguageErrorKind::Unavailable,
                "The activated input profile is not observed active.",
            ));
        }
        Ok(observed.snapshot)
    })();
    let cleanup = manager.finish();
    let snapshot = confirmation.map_err(|error| InputLanguageError::new(error.kind,
        format!("Input profile activation succeeded, but confirmation is unavailable: {} No retry was attempted.", error.message)))?;
    cleanup.map_err(|error| {
        InputLanguageError::new(
            error.kind,
            format!(
                "Input profile activation succeeded, but TSF cleanup failed: {}",
                error.message
            ),
        )
    })?;
    Ok(InputLanguageOutcome::Snapshot(snapshot))
}

#[cfg(windows)]
pub(crate) fn read_snapshot() -> Result<InputLanguageSnapshot, InputLanguageError> {
    read_with(&sdk::WindowsCalls)
}

#[cfg(windows)]
pub(crate) fn execute(
    action: InputLanguageAction,
) -> Result<InputLanguageOutcome, InputLanguageError> {
    execute_with(&sdk::WindowsCalls, action)
}

#[cfg(any(windows, test))]
#[path = "native_input_language/layout.rs"]
mod layout;
#[cfg(windows)]
#[path = "native_input_language/sdk.rs"]
mod sdk;
#[cfg(test)]
#[path = "native_input_language/tests.rs"]
mod tests;
