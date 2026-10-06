// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.
use super::*;

const SUPERVISOR: &str = r#""C:\Program Files\Tessera\alpha.2\tessera-shell.exe""#;
const DIRECTORY: &str = r"C:\Program Files\Tessera\alpha.2";
fn backup(original: OriginalShellState) -> ShellRecoveryBackup {
    ShellRecoveryBackup::new(SUPERVISOR.into(), original, DIRECTORY.into()).unwrap()
}

#[test]
fn valid_backup_preserves_each_original_kind() {
    for original in [
        OriginalShellState::Absent,
        OriginalShellState::RawString("explorer.exe".into()),
        OriginalShellState::RawExpandString(r"%windir%\explorer.exe".into()),
    ] {
        assert_eq!(backup(original.clone()).original(), &original);
    }
}

#[test]
fn supervisor_must_be_one_quoted_executable_in_the_recorded_directory() {
    for command in [
        "tessera-shell.exe",
        r"%ProgramFiles%\x.exe",
        "",
        r"C:\Program Files\Tessera\alpha.2\tessera-shell.exe",
        r#""C:\Other\tessera-shell.exe""#,
        r#""C:\Program Files\Tessera\alpha.2\other.exe""#,
        r#""C:\Program Files\Tessera\alpha.2\tessera-shell.exe" --extra"#,
    ] {
        assert!(
            ShellRecoveryBackup::new(command.into(), OriginalShellState::Absent, DIRECTORY.into())
                .is_err(),
            "accepted {command}"
        );
    }
    assert!(
        ShellRecoveryBackup::new(
            SUPERVISOR.into(),
            OriginalShellState::Absent,
            "relative".into()
        )
        .is_err()
    );
    assert!(
        ShellRecoveryBackup::new(
            SUPERVISOR.into(),
            OriginalShellState::RawString("x".repeat(MAX_VALUE_CHARS)),
            DIRECTORY.into()
        )
        .is_err()
    );
}

#[test]
fn encoded_record_distinguishes_registry_type_from_original_kind() {
    let record = backup(OriginalShellState::RawExpandString(
        r"%windir%\explorer.exe".into(),
    ));
    let encoded = record.encode();
    let field = |name| encoded.iter().find(|(key, _, _)| *key == name).unwrap();
    assert_eq!(field(FIELD_ORIGINAL_KIND).1.as_raw(), 4); // REG_DWORD, not original-kind 2
    assert_eq!(field(FIELD_ORIGINAL_KIND).2, 2u32.to_le_bytes());
    assert_eq!(field(FIELD_ORIGINAL_VALUE).1.as_raw(), 1); // raw payload remains REG_SZ
    assert_eq!(
        decode_utf16_bytes(&field(FIELD_ORIGINAL_VALUE).2).as_deref(),
        Some(r"%windir%\explorer.exe")
    );
    let absent = backup(OriginalShellState::Absent).encode();
    assert_eq!(
        absent
            .iter()
            .find(|(key, _, _)| *key == FIELD_ORIGINAL_VALUE)
            .unwrap()
            .2,
        [0, 0]
    );
    assert_eq!(
        absent
            .iter()
            .find(|(key, _, _)| *key == FIELD_ORIGINAL_PRESENT)
            .unwrap()
            .2,
        0u32.to_le_bytes()
    );
}

#[test]
fn utf16_roundtrip_is_bounded_and_never_lossy() {
    assert_eq!(
        decode_utf16_bytes(&utf16_bytes("路径 😀")).as_deref(),
        Some("路径 😀")
    );
    assert!(decode_utf16_bytes(&[0]).is_none());
    assert!(decode_utf16_bytes(&[]).is_none());
    assert!(decode_utf16_bytes(&[0, 0xd8, 0, 0]).is_none());
    assert!(decode_utf16_bytes(&vec![0; MAX_VALUE_CHARS * 2 + 2]).is_none());
}

#[test]
fn restore_ownership_requires_exact_plain_command() {
    let record = backup(OriginalShellState::RawString("explorer.exe".into()));
    let owned = OriginalShellState::RawString(SUPERVISOR.into());
    assert_eq!(
        decide_restore(Some(&record), Some(&owned)),
        RestoreDecision::RestoreOriginal
    );
    for state in [
        None,
        Some(OriginalShellState::RawString("other.exe".into())),
        Some(OriginalShellState::RawExpandString(SUPERVISOR.into())),
    ] {
        assert_eq!(
            decide_restore(Some(&record), state.as_ref()),
            RestoreDecision::RefuseClobber
        );
    }
    assert!(matches!(
        decide_restore(None, None),
        RestoreDecision::RefuseMalformed(_)
    ));
}

#[test]
fn already_restored_is_presence_and_kind_aware() {
    let original = OriginalShellState::RawString("explorer.exe".into());
    let record = backup(original.clone());
    assert_eq!(
        decide_restore(Some(&record), Some(&original)),
        RestoreDecision::AlreadyRestored
    );
    let different_kind = OriginalShellState::RawExpandString("explorer.exe".into());
    assert_eq!(
        decide_restore(Some(&record), Some(&different_kind)),
        RestoreDecision::RefuseClobber
    );
    assert_eq!(
        decide_restore(Some(&backup(OriginalShellState::Absent)), None),
        RestoreDecision::AlreadyRestored
    );
}

#[test]
fn absolute_paths_support_unicode_spaces_and_literal_percent_without_expansion() {
    for path in [
        SUPERVISOR,
        r"\\server\share\tessera.exe",
        r"C:\Users\路径\Tessera",
        r"C:\Users\Percent%name\Tessera",
    ] {
        assert!(is_absolute_windows_path(path), "rejected {path}");
    }
    for path in [
        "explorer.exe",
        r"C:relative.exe",
        r"%SystemRoot%\explorer.exe",
        r"\\server",
        "C:\\a\0b",
    ] {
        assert!(!is_absolute_windows_path(path), "accepted {path}");
    }
}
