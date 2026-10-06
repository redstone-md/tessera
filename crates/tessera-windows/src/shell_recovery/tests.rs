// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native deployment tests. Registry tests run against a unique private
//! subtree under HKCU\Software\Tessera\Tests\ created per test; real Winlogon
//! values and real per-user Tessera state are never touched.

use super::{
    DeploymentError, OwnedKey, apply_restore_at, delete_value, read_backup_from, read_dword,
    read_live_shell_at, read_string, set_raw, to_wide,
};
use crate::shell_recovery::{
    BackupRejection, FIELD_ACTIVE, FIELD_INSTALL_DIRECTORY, FIELD_ORIGINAL_KIND,
    FIELD_SCHEMA_VERSION, FIELD_SHELL_COMMAND, MAX_VALUE_CHARS, OriginalShellState,
    RECOVERY_SUBKEY, RegistryType, RestoreDecision, SHELL_VALUE_NAME, ShellRecoveryBackup,
    WINLOGON_SUBKEY,
};
use windows_sys::Win32::Foundation::{ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_DWORD};

/// A unique per-test registry subtree, deleted on drop.
struct TestTree(String);

impl TestTree {
    fn new(tag: &str) -> Self {
        let path = format!("Software\\Tessera\\Tests\\shell_recovery_{tag}");
        let _ = Self::delete_tree(&path);
        Self(path)
    }

    fn path(&self) -> &str {
        &self.0
    }

    fn delete_tree(path: &str) -> Result<(), u32> {
        let wide = to_wide(path);
        // SAFETY: wide is NUL-terminated UTF-16; the subtree is test-owned.
        let status = unsafe {
            windows_sys::Win32::System::Registry::RegDeleteTreeW(HKEY_CURRENT_USER, wide.as_ptr())
        };
        if status == ERROR_SUCCESS || status == ERROR_PATH_NOT_FOUND {
            Ok(())
        } else {
            Err(status)
        }
    }
}

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = Self::delete_tree(&self.0);
    }
}

/// One Winlogon-like subkey plus one recovery subkey under a private tree,
/// so the exact production restore path runs without real Winlogon access.
struct Fixture {
    _tree: TestTree,
    winlogon: String,
    recovery: String,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let tree = TestTree::new(tag);
        let winlogon = format!("{}\\{WINLOGON_SUBKEY}", tree.path());
        let recovery = format!("{}\\{RECOVERY_SUBKEY}", tree.path());
        Self {
            _tree: tree,
            winlogon,
            recovery,
        }
    }

    fn seed_shell(&self, value: &str) {
        let key = OwnedKey::create(HKEY_CURRENT_USER, &self.winlogon).expect("create winlogon");
        set_raw(
            key.0,
            SHELL_VALUE_NAME,
            windows_sys::Win32::System::Registry::REG_SZ,
            &crate::shell_recovery::utf16_bytes(value),
        )
        .expect("seed live shell");
    }
}

const SUPERVISOR: &str = r#""C:\Program Files\Tessera\0.1.0\tessera-shell.exe""#;
const INSTALL_DIR: &str = "C:\\Program Files\\Tessera\\0.1.0";

fn valid_backup() -> ShellRecoveryBackup {
    ShellRecoveryBackup::new(
        SUPERVISOR.to_string(),
        OriginalShellState::RawString("explorer.exe".to_string()),
        INSTALL_DIR.to_string(),
    )
    .expect("valid backup")
}

/// Writes fixture fields at an explicit subkey path (a tree root for the
/// pure read/write tests, the recovery subkey for restore-path tests).
fn write_all(subkey: &str, fields: &[(&'static str, RegistryType, Vec<u8>)]) {
    let key = OwnedKey::create(HKEY_CURRENT_USER, subkey).expect("create");
    for (name, kind, payload) in fields {
        set_raw(key.0, name, kind.as_raw(), payload).expect("write");
    }
}

fn restore(fixture: &Fixture, backup: &ShellRecoveryBackup, decision: &RestoreDecision) {
    apply_restore_at(
        HKEY_CURRENT_USER,
        &fixture.winlogon,
        &fixture.recovery,
        backup,
        decision,
    )
    .expect("apply_restore_at");
}

fn live_shell(fixture: &Fixture) -> Option<OriginalShellState> {
    read_live_shell_at(HKEY_CURRENT_USER, &fixture.winlogon).expect("read live")
}

#[test]
fn utf16_roundtrip_through_registry() {
    let tree = TestTree::new("utf16");
    let key = OwnedKey::create(HKEY_CURRENT_USER, tree.path()).expect("create");
    let payload = crate::shell_recovery::utf16_bytes("hello");
    set_raw(
        key.0,
        "probe",
        windows_sys::Win32::System::Registry::REG_SZ,
        &payload,
    )
    .expect("write");
    let read = read_string(&key, "probe").expect("read");
    assert_eq!(read.as_deref(), Some("hello"));
    assert!(read_string(&key, "missing").expect("missing").is_none());
}

#[test]
fn dword_roundtrip_and_type_mismatch() {
    let tree = TestTree::new("dword");
    let key = OwnedKey::create(HKEY_CURRENT_USER, tree.path()).expect("create");
    set_raw(key.0, "count", REG_DWORD, &7u32.to_le_bytes()).expect("write");
    assert_eq!(read_dword(&key, "count").expect("read"), Some(7));
    // A string in a DWORD slot is rejected, not coerced.
    set_raw(
        key.0,
        "bad",
        windows_sys::Win32::System::Registry::REG_SZ,
        &crate::shell_recovery::utf16_bytes("nope"),
    )
    .expect("write string");
    assert!(matches!(
        read_dword(&key, "bad"),
        Err(DeploymentError::Rejected(BackupRejection::Malformed { .. }))
    ));
}

#[test]
fn backup_roundtrip_preserves_expand_string_kind() {
    let tree = TestTree::new("roundtrip");
    let original = OriginalShellState::RawExpandString("%SystemRoot%\\explorer.exe".to_string());
    let backup =
        ShellRecoveryBackup::new(SUPERVISOR.to_string(), original, INSTALL_DIR.to_string())
            .expect("valid");
    let mut fields = backup.encode();
    // The fixture starts inactive so the read gate is exercised: a complete
    // record is only reported as a backup while Active is 1.
    for (name, _, payload) in fields.iter_mut() {
        if *name == FIELD_ACTIVE {
            *payload = 0u32.to_le_bytes().to_vec();
        }
    }
    write_all(tree.path(), &fields);
    let key = OwnedKey::open(
        HKEY_CURRENT_USER,
        tree.path(),
        windows_sys::Win32::System::Registry::KEY_READ
            | windows_sys::Win32::System::Registry::KEY_SET_VALUE,
    )
    .expect("open")
    .expect("key exists");
    assert!(read_backup_from(&key).expect("read inactive").is_none());
    set_raw(key.0, FIELD_ACTIVE, REG_DWORD, &1u32.to_le_bytes()).expect("activate");
    let record = read_backup_from(&key).expect("read activated");
    assert_eq!(record.as_ref(), Some(&backup));
}

#[test]
fn backup_with_wrong_schema_is_refused() {
    let tree = TestTree::new("schema");
    let backup = valid_backup();
    let mut fields = backup.encode();
    for (name, _, payload) in fields.iter_mut() {
        if *name == FIELD_SCHEMA_VERSION {
            *payload = 99u32.to_le_bytes().to_vec();
        }
    }
    write_all(tree.path(), &fields);
    let key = OwnedKey::open(
        HKEY_CURRENT_USER,
        tree.path(),
        windows_sys::Win32::System::Registry::KEY_READ,
    )
    .expect("open")
    .expect("key exists");
    assert!(matches!(
        read_backup_from(&key),
        Err(DeploymentError::Rejected(
            BackupRejection::UnsupportedSchema { version: 99 }
        ))
    ));
}

#[test]
fn backup_with_inconsistent_kind_is_refused() {
    let tree = TestTree::new("kind");
    let mut fields = valid_backup().encode();
    for (name, _, payload) in fields.iter_mut() {
        if *name == FIELD_ORIGINAL_KIND {
            *payload = 42u32.to_le_bytes().to_vec();
        }
    }
    write_all(tree.path(), &fields);
    let key = OwnedKey::open(
        HKEY_CURRENT_USER,
        tree.path(),
        windows_sys::Win32::System::Registry::KEY_READ,
    )
    .expect("open")
    .expect("key exists");
    assert!(matches!(
        read_backup_from(&key),
        Err(DeploymentError::Rejected(BackupRejection::InconsistentKind))
    ));
}

#[test]
fn backup_with_missing_field_is_refused() {
    let tree = TestTree::new("missing");
    let fields: Vec<_> = valid_backup()
        .encode()
        .into_iter()
        .filter(|(name, _, _)| *name != FIELD_INSTALL_DIRECTORY)
        .collect();
    write_all(tree.path(), &fields);
    let key = OwnedKey::open(
        HKEY_CURRENT_USER,
        tree.path(),
        windows_sys::Win32::System::Registry::KEY_READ,
    )
    .expect("open")
    .expect("key exists");
    assert!(matches!(
        read_backup_from(&key),
        Err(DeploymentError::Rejected(BackupRejection::Malformed { field }))
            if field == FIELD_INSTALL_DIRECTORY
    ));
}

#[test]
fn missing_keys_read_as_none_not_error() {
    let tree = TestTree::new("absent_keys");
    // Neither subtree exists yet under the private tree root.
    let winlogon = format!("{}\\{WINLOGON_SUBKEY}", tree.path());
    let recovery = format!("{}\\{RECOVERY_SUBKEY}", tree.path());
    assert!(
        read_live_shell_at(HKEY_CURRENT_USER, &winlogon)
            .expect("live")
            .is_none()
    );
    assert!(
        super::read_backup_at(HKEY_CURRENT_USER, &recovery)
            .expect("backup")
            .is_none()
    );
}

#[test]
fn restore_writes_exact_string_and_reads_back() {
    let fixture = Fixture::new("restore_string");
    write_all(&fixture.recovery, &valid_backup().encode());
    // Live value is the owned supervisor command: a clean takeover restore.
    fixture.seed_shell(SUPERVISOR);
    restore(&fixture, &valid_backup(), &RestoreDecision::RestoreOriginal);
    assert_eq!(
        live_shell(&fixture),
        Some(OriginalShellState::RawString("explorer.exe".to_string()))
    );
    // Active is cleared; the rest of the record stays for diagnosis.
    let key = OwnedKey::open(
        HKEY_CURRENT_USER,
        &fixture.recovery,
        windows_sys::Win32::System::Registry::KEY_READ,
    )
    .expect("open recovery")
    .expect("recovery key");
    assert_eq!(read_dword(&key, FIELD_ACTIVE).expect("active"), Some(0));
    assert_eq!(
        read_string(&key, FIELD_SHELL_COMMAND)
            .expect("command")
            .as_deref(),
        Some(SUPERVISOR)
    );
}

#[test]
fn restore_writes_exact_expand_string_kind() {
    let fixture = Fixture::new("restore_expand");
    let original = OriginalShellState::RawExpandString("%SystemRoot%\\explorer.exe".to_string());
    let backup = ShellRecoveryBackup::new(
        SUPERVISOR.to_string(),
        original.clone(),
        INSTALL_DIR.to_string(),
    )
    .expect("valid");
    write_all(&fixture.recovery, &backup.encode());
    // Live value is the owned supervisor command: a clean takeover restore.
    fixture.seed_shell(SUPERVISOR);
    restore(&fixture, &backup, &RestoreDecision::RestoreOriginal);
    assert_eq!(live_shell(&fixture), Some(original));
}

#[test]
fn restore_absent_deletes_value_and_reads_back_gone() {
    let fixture = Fixture::new("restore_absent");
    let backup = ShellRecoveryBackup::new(
        SUPERVISOR.to_string(),
        OriginalShellState::Absent,
        INSTALL_DIR.to_string(),
    )
    .expect("valid");
    write_all(&fixture.recovery, &backup.encode());
    fixture.seed_shell(SUPERVISOR);
    restore(&fixture, &backup, &RestoreDecision::RestoreOriginal);
    assert!(live_shell(&fixture).is_none());
}

#[test]
fn restore_refuses_to_clobber_foreign_value() {
    let fixture = Fixture::new("restore_refuse");
    let backup = valid_backup();
    write_all(&fixture.recovery, &backup.encode());
    fixture.seed_shell("other-shell.exe");
    // A refusal must leave the live value untouched.
    restore(&fixture, &backup, &RestoreDecision::RefuseClobber);
    // A stale caller decision must not bypass the native ownership recheck.
    assert!(matches!(
        apply_restore_at(
            HKEY_CURRENT_USER,
            &fixture.winlogon,
            &fixture.recovery,
            &backup,
            &RestoreDecision::RestoreOriginal
        ),
        Err(DeploymentError::ShellOwnershipChanged)
    ));
    assert_eq!(
        live_shell(&fixture),
        Some(OriginalShellState::RawString("other-shell.exe".to_string()))
    );
    // And must not clear the active flag either.
    let recovery = OwnedKey::open(
        HKEY_CURRENT_USER,
        &fixture.recovery,
        windows_sys::Win32::System::Registry::KEY_READ,
    )
    .expect("open recovery")
    .expect("recovery key");
    assert_eq!(
        read_dword(&recovery, FIELD_ACTIVE).expect("active"),
        Some(1)
    );
}

#[test]
fn already_restored_clears_active_only() {
    let fixture = Fixture::new("already_restored");
    let backup = valid_backup();
    write_all(&fixture.recovery, &backup.encode());
    fixture.seed_shell("explorer.exe");
    restore(&fixture, &backup, &RestoreDecision::AlreadyRestored);
    let recovery = OwnedKey::open(
        HKEY_CURRENT_USER,
        &fixture.recovery,
        windows_sys::Win32::System::Registry::KEY_READ,
    )
    .expect("open recovery")
    .expect("recovery key");
    assert_eq!(
        read_dword(&recovery, FIELD_ACTIVE).expect("active"),
        Some(0)
    );
    // The live value was never rewritten on this path.
    assert_eq!(
        live_shell(&fixture),
        Some(OriginalShellState::RawString("explorer.exe".to_string()))
    );
}

#[test]
fn clear_active_never_clobbers_foreign_flag() {
    let fixture = Fixture::new("foreign_flag");
    let backup = valid_backup();
    write_all(&fixture.recovery, &backup.encode());
    fixture.seed_shell("explorer.exe");
    // Replace the Active flag with a foreign DWORD payload (7): not ours.
    let key = OwnedKey::open(
        HKEY_CURRENT_USER,
        &fixture.recovery,
        windows_sys::Win32::System::Registry::KEY_READ
            | windows_sys::Win32::System::Registry::KEY_WRITE,
    )
    .expect("open recovery")
    .expect("recovery key");
    set_raw(key.0, FIELD_ACTIVE, REG_DWORD, &7u32.to_le_bytes()).expect("seed foreign flag");
    drop(key);
    restore(&fixture, &backup, &RestoreDecision::AlreadyRestored);
    let key = OwnedKey::open(
        HKEY_CURRENT_USER,
        &fixture.recovery,
        windows_sys::Win32::System::Registry::KEY_READ,
    )
    .expect("open recovery")
    .expect("recovery key");
    assert_eq!(read_dword(&key, FIELD_ACTIVE).expect("active"), Some(7));
}

#[test]
fn missing_recovery_key_during_clear_is_a_no_op() {
    let fixture = Fixture::new("clear_no_key");
    // No recovery subtree was created: clearing must not fail and must not
    // implicitly create it. The live value already matches the backup's
    // original, so the fresh recheck decides AlreadyRestored.
    fixture.seed_shell("explorer.exe");
    restore(&fixture, &valid_backup(), &RestoreDecision::AlreadyRestored);
    assert!(
        OwnedKey::open(
            HKEY_CURRENT_USER,
            &fixture.recovery,
            windows_sys::Win32::System::Registry::KEY_READ,
        )
        .expect("open")
        .is_none()
    );
}

#[test]
fn missing_winlogon_key_during_restore_is_an_error() {
    let fixture = Fixture::new("restore_no_winlogon");
    write_all(&fixture.recovery, &valid_backup().encode());
    // Missing live state cannot authorize a write, so the fresh ownership
    // check must refuse the stale RestoreOriginal decision without creating
    // a Winlogon-like subtree.
    assert!(matches!(
        apply_restore_at(
            HKEY_CURRENT_USER,
            &fixture.winlogon,
            &fixture.recovery,
            &valid_backup(),
            &RestoreDecision::RestoreOriginal,
        ),
        Err(DeploymentError::ShellOwnershipChanged)
    ));
    assert!(
        OwnedKey::open(
            HKEY_CURRENT_USER,
            &fixture.winlogon,
            windows_sys::Win32::System::Registry::KEY_READ,
        )
        .expect("open")
        .is_none()
    );
}

/// Writes through delete_value must actually remove the value.
#[test]
fn delete_value_roundtrip() {
    let tree = TestTree::new("delete");
    let key = OwnedKey::create(HKEY_CURRENT_USER, tree.path()).expect("create");
    set_raw(
        key.0,
        "gone",
        windows_sys::Win32::System::Registry::REG_SZ,
        &crate::shell_recovery::utf16_bytes("soon"),
    )
    .expect("write");
    delete_value(key.0, "gone").expect("delete");
    // Deleting a missing value is a success, not an error.
    delete_value(key.0, "gone").expect("delete missing");
    assert!(read_string(&key, "gone").expect("read").is_none());
}

/// The lock is serialized against a fixture file, not real per-user state:
/// a second acquire while the guard lives fails, and after Drop it succeeds.
#[test]
fn deployment_lock_is_exclusive_within_process() {
    let base = std::env::temp_dir().join(format!(
        "tessera_shell_recovery_lock_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    let first = super::DeploymentLock::acquire_in(&base).expect("first lock");
    assert!(matches!(
        super::DeploymentLock::acquire_in(&base),
        Err(DeploymentError::LockBusy)
    ));
    drop(first);
    let retry = super::DeploymentLock::acquire_in(&base).expect("lock after release");
    drop(retry);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn oversized_value_is_out_of_bounds() {
    let tree = TestTree::new("oversized");
    let key = OwnedKey::create(HKEY_CURRENT_USER, tree.path()).expect("create");
    let oversized: Vec<u8> = vec![0x41; (MAX_VALUE_CHARS + 1) * 2];
    set_raw(
        key.0,
        "big",
        windows_sys::Win32::System::Registry::REG_SZ,
        &oversized,
    )
    .expect("write oversized");
    // The read path rejects a payload that cannot decode within bounds.
    assert!(matches!(
        read_string(&key, "big"),
        Err(DeploymentError::InvalidUtf16 { .. })
    ));
}
