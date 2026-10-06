// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Takeover ownership policy, without reading registry or starting a process.
use super::*;

fn backup() -> ShellRecoveryBackup {
    ShellRecoveryBackup::new(
        "\"C:\\Tessera\\alpha.2\\tessera-shell.exe\"".into(),
        OriginalShellState::Absent,
        "C:\\Tessera\\alpha.2".into(),
    )
    .unwrap()
}

#[test]
fn only_recorded_version_and_exact_plain_live_command_can_supervise() {
    let backup = backup();
    let executable = std::path::Path::new("C:\\Tessera\\alpha.2\\tessera-shell.exe");
    let live = OriginalShellState::RawString(backup.shell_command().into());
    assert!(validate_takeover_record(executable, &backup, Some(&live)).is_ok());
    assert!(
        validate_takeover_record(
            std::path::Path::new("C:\\Other\\tessera-shell.exe"),
            &backup,
            Some(&live)
        )
        .is_err()
    );
    for state in [
        None,
        Some(OriginalShellState::Absent),
        Some(OriginalShellState::RawString("explorer.exe".into())),
        Some(OriginalShellState::RawExpandString(
            backup.shell_command().into(),
        )),
    ] {
        assert!(validate_takeover_record(executable, &backup, state.as_ref()).is_err());
    }
}
