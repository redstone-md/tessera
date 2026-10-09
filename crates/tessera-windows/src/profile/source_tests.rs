// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::super::actor::Driver;
use super::super::source::{self, Backend, DirectoryIdentity, Reader, Value};
use parking_lot::Mutex;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use tessera_system::profile::{
    ProfileCommand, ProfileError, ProfileErrorKind, ProfilePhoto, ProfilePhotoState, ProfileTarget,
};

const SELF_SID: &str = "S-1-5-21-100-200-300-1001";
const OTHER_SID: &str = "S-1-5-21-999-888-777-1002";

struct State {
    sid: String,
    sid_sequence: VecDeque<String>,
    values: BTreeMap<String, Result<Option<String>, ProfileError>>,
    photos: BTreeMap<String, Result<ProfilePhoto, ProfileError>>,
    identity: DirectoryIdentity,
    directory_failure: Option<ProfileError>,
    calls: Vec<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            sid: SELF_SID.into(),
            sid_sequence: VecDeque::new(),
            values: BTreeMap::new(),
            photos: BTreeMap::new(),
            identity: DirectoryIdentity {
                volume: 1,
                file_id: [42; 16],
            },
            directory_failure: None,
            calls: vec![],
        }
    }
}

#[derive(Clone)]
struct Recording(Arc<Mutex<State>>);

impl Backend for Recording {
    fn current_sid(&mut self) -> Result<String, ProfileError> {
        let mut state = self.0.lock();
        state.calls.push("CurrentProcessSid".into());
        Ok(state
            .sid_sequence
            .pop_front()
            .unwrap_or_else(|| state.sid.clone()))
    }
    fn display_name(&mut self) -> Result<String, ProfileError> {
        self.0.lock().calls.push("NameDisplayOrSam".into());
        Ok("Genuine 名字".into())
    }
    fn value(&mut self, value: Value<'_>) -> Result<Option<String>, ProfileError> {
        let key = match value {
            Value::Photo { sid, quality } => format!("HKLM/{sid}/{quality}"),
            Value::PersonalEmail => "HKCU/Personal/UserEmail".into(),
            Value::PersonalFolder => "HKCU/Personal/UserFolder".into(),
        };
        let mut state = self.0.lock();
        state.calls.push(key.clone());
        state.values.get(&key).cloned().unwrap_or(Ok(None))
    }
    fn photo(&mut self, path: &str) -> Result<ProfilePhoto, ProfileError> {
        let mut state = self.0.lock();
        state.calls.push(format!("Photo/{path}"));
        state
            .photos
            .get(path)
            .cloned()
            .unwrap_or_else(|| Err(source::invalid()))
    }
    fn directory(&mut self, path: &str) -> Result<DirectoryIdentity, ProfileError> {
        let mut state = self.0.lock();
        state.calls.push(format!("Directory/{path}"));
        state
            .directory_failure
            .clone()
            .map_or(Ok(state.identity), Err)
    }
    fn open_directory(
        &mut self,
        path: &str,
        identity: DirectoryIdentity,
    ) -> Result<(), ProfileError> {
        let mut state = self.0.lock();
        if state.identity != identity {
            return Err(source::invalid());
        }
        state.calls.push(format!("OpenDirectory/{path}"));
        Ok(())
    }
    fn open_home(&mut self) -> Result<(), ProfileError> {
        self.0.lock().calls.push("OpenFixedHome".into());
        Ok(())
    }
    fn open_accounts(&mut self) -> Result<(), ProfileError> {
        self.0.lock().calls.push("OpenFixedAccounts".into());
        Ok(())
    }
}

fn fixture() -> (Reader<Recording>, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State::default()));
    (Reader::new(Recording(Arc::clone(&state))), state)
}

fn picture(
    state: &mut State,
    sid: &str,
    key: &str,
    path: &str,
    photo: Result<ProfilePhoto, ProfileError>,
) {
    state
        .values
        .insert(format!("HKLM/{sid}/{key}"), Ok(Some(path.into())));
    state.photos.insert(path.into(), photo);
}

#[test]
fn only_process_self_sid_is_used_and_highest_actually_valid_picture_wins() {
    let (mut reader, state) = fixture();
    {
        let mut state = state.lock();
        picture(
            &mut state,
            OTHER_SID,
            "Image1080",
            "C:\\photos\\unrelated.png",
            Ok(ProfilePhoto::new(1, 1, vec![0, 255, 0, 255]).unwrap()),
        );
        picture(
            &mut state,
            SELF_SID,
            "Image1080",
            "C:\\photos\\bad.png",
            Err(source::invalid()),
        );
        picture(
            &mut state,
            SELF_SID,
            "Image448",
            "C:\\photos\\valid.png",
            Ok(ProfilePhoto::new(1, 1, vec![128, 0, 0, 128]).unwrap()),
        );
    }
    let snapshot = reader.read().unwrap();
    let ProfilePhotoState::Ready(photo) = snapshot.photo() else {
        panic!("valid fallback expected")
    };
    assert_eq!(photo.rgba(), [128, 0, 0, 128]);
    let calls = &state.lock().calls;
    assert!(
        !calls
            .iter()
            .any(|call| call.contains(OTHER_SID) || call.contains("LastLogged"))
    );
    assert!(!calls.iter().any(|call| call.contains("Image424")));
}

#[test]
fn absent_or_denied_or_malformed_photo_preserves_name_and_optional_personal_email() {
    for error in [
        None,
        Some(ProfileErrorKind::AccessDenied),
        Some(ProfileErrorKind::InvalidData),
    ] {
        let (mut reader, state) = fixture();
        {
            let mut state = state.lock();
            state.values.insert(
                "HKCU/Personal/UserEmail".into(),
                Ok(Some("personal@example.test".into())),
            );
            if let Some(kind) = error {
                state.values.insert(
                    format!("HKLM/{SELF_SID}/Image1080"),
                    Err(ProfileError::new(
                        kind,
                        "The source is unavailable.",
                        Some(5),
                    )),
                );
            }
        }
        let snapshot = reader.read().unwrap();
        assert_eq!(snapshot.display_name(), "Genuine 名字");
        assert_eq!(snapshot.personal_email(), Some("personal@example.test"));
        assert_eq!(
            snapshot.photo(),
            &error.map_or(ProfilePhotoState::Absent, ProfilePhotoState::Unavailable)
        );
        assert!(snapshot.onedrive().is_none());
    }
}

#[test]
fn absent_denied_malformed_personal_sources_are_not_windows_account_claims() {
    for value in [
        Ok(None),
        Err(source::invalid()),
        Ok(Some("bad\0email".into())),
        Ok(Some("x".repeat(1_025))),
    ] {
        let (mut reader, state) = fixture();
        state
            .lock()
            .values
            .insert("HKCU/Personal/UserEmail".into(), value);
        let snapshot = reader.read().unwrap();
        assert_eq!(snapshot.personal_email(), None);
        assert!(snapshot.onedrive().is_none());
        assert_eq!(snapshot.display_name(), "Genuine 名字");
    }
}

#[test]
fn changed_self_sid_during_read_prevents_snapshot_publication() {
    let (mut reader, state) = fixture();
    state.lock().sid_sequence = VecDeque::from([SELF_SID.into(), OTHER_SID.into()]);
    let error = reader.read().unwrap_err();
    assert_eq!(error.kind(), ProfileErrorKind::InvalidData);
    assert!(!format!("{error:?}").contains(SELF_SID));
}

#[test]
fn onedrive_is_provider_owned_and_fresh_sid_registry_directory_revalidated() {
    for change in [
        "none",
        "sid",
        "path",
        "directory",
        "missing",
        "foreign",
        "forged",
        "denied",
    ] {
        let (mut reader, state) = fixture();
        state.lock().values.insert(
            "HKCU/Personal/UserFolder".into(),
            Ok(Some("C:\\Users\\Self\\OneDrive".into())),
        );
        let snapshot = reader.read().unwrap();
        let mut target = snapshot.onedrive().unwrap().clone();
        match change {
            "sid" => state.lock().sid = OTHER_SID.into(),
            "path" => {
                state.lock().values.insert(
                    "HKCU/Personal/UserFolder".into(),
                    Ok(Some("C:\\different\\folder".into())),
                );
            }
            "directory" => state.lock().identity.file_id[0] += 1,
            "missing" => {
                state.lock().values.remove("HKCU/Personal/UserFolder");
            }
            "foreign" => {
                let mut other = Reader::new(Recording(Arc::clone(&state)));
                target = other.read().unwrap().onedrive().unwrap().clone();
            }
            "forged" => target = ProfileTarget::new("C:\\arbitrary\\path".to_owned()),
            "denied" => {
                state.lock().directory_failure = Some(ProfileError::new(
                    ProfileErrorKind::AccessDenied,
                    "The directory is unavailable.",
                    Some(5),
                ))
            }
            _ => {}
        }
        let result = reader.execute(ProfileCommand::OpenOneDrive { expected: target });
        let opened = state
            .lock()
            .calls
            .iter()
            .filter(|call| call.starts_with("OpenDirectory/"))
            .count();
        assert_eq!(result.is_ok(), change == "none", "{change}");
        assert_eq!(opened, usize::from(change == "none"));
    }
}

#[test]
fn closed_home_and_accounts_intents_do_not_accept_paths() {
    let (mut reader, state) = fixture();
    reader.execute(ProfileCommand::OpenHome).unwrap();
    reader
        .execute(ProfileCommand::OpenAccountsSettings)
        .unwrap();
    assert_eq!(state.lock().calls, ["OpenFixedHome", "OpenFixedAccounts"]);
    assert_eq!(source::ACCOUNTS_URI, "ms-settings:accounts");
    assert_eq!(
        source::PERSONAL_KEY,
        "Software\\Microsoft\\OneDrive\\Accounts\\Personal"
    );
}

#[test]
fn local_path_policy_rejects_remote_device_special_aliases_and_executables() {
    for path in [
        "\\\\server\\share\\image.png",
        "\\\\?\\UNC\\server\\share\\image.png",
        "\\\\?\\C:\\photo.png",
        "\\\\.\\C:\\photo.png",
        "relative.png",
        "C:relative.png",
        "C:\\photo.png:stream",
        "C:\\..\\photo.png",
        "C:\\CON.png",
        "C:\\COM1\\photo.png",
        "C:\\LPT²\\photo.png",
        "C:\\CON .png",
        "C:\\folder \\photo.png",
        "C:\\folder.\\photo.png",
        "C:\\photo.exe",
        "C:\\photo.lnk",
        "C:\\bad\0name.png",
        "C:\\double\\\\photo.png",
        "C:/photo.png",
        "C:\\image.svg",
    ] {
        assert!(source::validate_local_path(path, true).is_err(), "{path:?}");
    }
    assert!(source::validate_local_path("C:\\照片\\actual.PNG", true).is_ok());
    assert!(source::validate_local_path("C:\\Users\\Self\\OneDrive", false).is_ok());
    assert!(source::validate_local_path("C:\\program.exe", false).is_err());
    assert!(
        source::validate_local_path(&format!("C:\\{}photo.png", "x\\".repeat(129)), true).is_err()
    );
}

#[test]
fn strict_utf16_rejects_invalid_surrogates_embedded_nul_and_missing_termination() {
    for units in [
        vec![],
        vec![0],
        vec![0xd800, 0],
        vec![b'a' as u16, 0, b'b' as u16, 0],
        vec![b'a' as u16],
    ] {
        assert!(source::strict_utf16(&units).is_err());
    }
    let units: Vec<u16> = "名字".encode_utf16().chain(Some(0)).collect();
    assert_eq!(source::strict_utf16(&units).unwrap(), "名字");
    assert!(source::strict_utf16(&vec![1; 32_769]).is_err());
}

#[test]
fn encoded_copy_detects_growth_truncation_executable_bytes_and_exact_cap_sentinel() {
    assert_eq!(
        source::bounded_bytes(&mut &b"abcd"[..], 4).unwrap(),
        b"abcd"
    );
    for (payload, advertised) in [
        (b"abcde".as_slice(), 4),
        (b"abc".as_slice(), 4),
        (b"MZxx".as_slice(), 4),
        (b"".as_slice(), 0),
    ] {
        assert!(source::bounded_bytes(&mut &payload[..], advertised).is_err());
    }
    let exact = vec![42; source::ENCODED_LIMIT];
    assert_eq!(
        source::bounded_bytes(&mut exact.as_slice(), exact.len() as u64)
            .unwrap()
            .len(),
        source::ENCODED_LIMIT
    );
    assert!(source::bounded_bytes(&mut std::io::repeat(42), source::ENCODED_LIMIT as u64).is_err());
    assert!(source::bounded_bytes(&mut &b"x"[..], source::ENCODED_LIMIT as u64 + 1).is_err());
}
