// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::process::Command;

#[cfg(windows)]
#[path = "support/windows_fixture.rs"]
mod windows_fixture;

#[test]
fn demo_outputs_the_domain_plan_without_excluded_windows() {
    let output = Command::new(env!("CARGO_BIN_EXE_tessera"))
        .arg("demo")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(
        stdout,
        concat!(
            "Tessera layout demo (synthetic windows; no desktop changes)\n",
            "Work area: x=-1920 y=0 width=1920 height=1040\n",
            "Floating and fullscreen windows are excluded.\n",
            "window 1: x=-1920 y=0 width=1147 height=1040\n",
            "window 3: x=-765 y=0 width=765 height=516\n",
            "window 5: x=-765 y=524 width=765 height=516\n",
        )
    );
}

#[test]
fn unsupported_arguments_fail_without_running_the_demo() {
    for arguments in [
        vec!["start"],
        vec!["demo", "unexpected"],
        vec!["panel", "unexpected"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_tessera"))
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("unsupported arguments")
        );
    }
}

#[cfg(not(windows))]
#[test]
fn native_commands_report_unsupported_platform_without_a_fake_desktop() {
    for command in ["inspect", "panel"] {
        let output = Command::new(env!("CARGO_BIN_EXE_tessera"))
            .arg(command)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("Windows")
        );
    }
}

#[cfg(windows)]
#[test]
fn inspection_reads_a_foreign_window_without_changing_it() {
    let title = format!(
        "Tessera observation fixture {}\n\u{1b}[31m",
        std::process::id()
    );
    let fixture = windows_fixture::Fixture::new(&title);
    let before = fixture.bounds();
    let output = Command::new(env!("CARGO_BIN_EXE_tessera"))
        .arg("inspect")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Tessera desktop snapshot (read-only; physical pixels)"));
    assert!(
        stdout.contains(&format!(
            "window 0x{:X} pid={}",
            fixture.id(),
            std::process::id()
        )),
        "{stdout}"
    );
    assert!(stdout.contains(&format!("title: {title:?}")), "{stdout}");
    assert!(
        !stdout.contains('\u{1b}'),
        "Untrusted captions must not emit terminal controls"
    );
    assert_eq!(fixture.bounds(), before);
    assert!(fixture.visible());
}
