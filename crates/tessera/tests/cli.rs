// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::process::Command;

#[cfg(windows)]
#[path = "support/windows_fixture.rs"]
mod windows_fixture;

#[test]
fn version_identifies_the_exact_alpha_build() {
    let output = Command::new(env!("CARGO_BIN_EXE_tessera"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("Tessera {}", env!("CARGO_PKG_VERSION")),
    );
}

#[cfg(not(windows))]
#[test]
fn desktop_launcher_reports_unsupported_platform() {
    let output = Command::new(env!("CARGO_BIN_EXE_tessera-desktop"))
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
        vec!["inspect", "--unknown"],
        vec!["inspect", "--check-surfaces", "unexpected"],
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
    for arguments in [
        vec!["inspect"],
        vec!["inspect", "--check-surfaces"],
        vec!["panel"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_tessera"))
            .args(arguments)
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

#[cfg(windows)]
mod check_surfaces {
    use std::process::Command;

    use super::windows_fixture::{Fixture, ScopedDpiContext};

    fn run_check() -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_tessera"))
            .args(["inspect", "--check-surfaces"])
            .output()
            .unwrap()
    }

    fn stdout_of(output: &std::process::Output) -> String {
        String::from_utf8(output.stdout.clone())
            .unwrap()
            .replace('\r', "")
    }

    #[test]
    fn toolbar_green_then_offset_red_then_restored_green_with_unchanged_fixture() {
        // Start elsewhere: leaving observe() in PMv2 must make this red.
        {
            let system_scope = ScopedDpiContext::system_aware();
            tessera_windows::observe().expect("observation must succeed");
            system_scope.assert_current_unchanged();
        }
        let _dpi_scope = ScopedDpiContext::enter();

        let fixture = Fixture::toolbar("Tessera toolbar");
        let (monitor_x, monitor_y, monitor_width, _) = fixture.monitor_bounds();
        let dpi = fixture.dpi();
        assert_ne!(dpi, 0, "fixture window must report a real DPI");
        // Expected contract height: round(32 logical * dpi / 96).
        let expected_height = (32u64 * u64::from(dpi) + 48) / 96;
        let (style_before, ex_style_before) = fixture.styles();
        let assert_fixture_untouched = |fixture: &Fixture, styles: (isize, isize)| {
            assert!(fixture.visible());
            assert_eq!(fixture.styles(), styles);
        };

        // Green run: fixture sized to the exact contract at its real monitor
        // bounds (no resolution or DPI assumptions).
        fixture.set_rect(
            monitor_x,
            monitor_y,
            monitor_width as i32,
            expected_height as i32,
        );
        let output = run_check();
        assert!(
            output.status.success(),
            "contract-conformant toolbar must pass: {}",
            stdout_of(&output)
        );
        let stdout = stdout_of(&output);
        assert!(stdout.contains("toolbar contract PASS"), "{stdout}");
        assert!(stdout.contains("surface kind=toolbar"), "{stdout}");
        assert!(stdout.contains("dpi=Some("), "{stdout}");
        assert!(stdout.contains("style=Some("), "{stdout}");
        assert!(stdout.contains("ex-style=Some("), "{stdout}");
        assert!(stdout.contains("show-cmd=Some("), "{stdout}");
        assert!(!stdout.contains("diagnostic unavailable"), "{stdout}");
        assert!(!stdout.contains('\u{1b}'), "no terminal escapes");
        assert_eq!(
            fixture.bounds(),
            (
                monitor_x,
                monitor_y,
                monitor_x + monitor_width as i32,
                monitor_y + expected_height as i32
            )
        );
        assert_fixture_untouched(&fixture, (style_before, ex_style_before));

        // Red run: deliberately offset below the monitor-bounds top; the CLI
        // must exit nonzero and print the exact top delta.
        fixture.set_rect(
            monitor_x,
            monitor_y + 40,
            monitor_width as i32,
            expected_height as i32,
        );
        let output = run_check();
        assert_eq!(output.status.code(), Some(1), "offset toolbar must fail");
        let stdout = stdout_of(&output);
        assert!(stdout.contains("toolbar contract FAIL"), "{stdout}");
        assert!(stdout.contains("top delta +40"), "{stdout}");
        assert!(stdout.contains("surface kind=toolbar pid="), "{stdout}");
        assert!(!stdout.contains('\u{1b}'), "no terminal escapes");
        assert_eq!(
            fixture.bounds(),
            (
                monitor_x,
                monitor_y + 40,
                monitor_x + monitor_width as i32,
                monitor_y + 40 + expected_height as i32
            )
        );
        assert_fixture_untouched(&fixture, (style_before, ex_style_before));

        // Restored green run: back at the contract position, success again.
        fixture.set_rect(
            monitor_x,
            monitor_y,
            monitor_width as i32,
            expected_height as i32,
        );
        let output = run_check();
        assert!(
            output.status.success(),
            "restored toolbar must pass: {}",
            stdout_of(&output)
        );
        assert!(stdout_of(&output).contains("toolbar contract PASS"));
        assert_fixture_untouched(&fixture, (style_before, ex_style_before));
    }
}
