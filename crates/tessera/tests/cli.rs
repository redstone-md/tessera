use std::process::Command;

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
    for arguments in [vec!["start"], vec!["demo", "unexpected"]] {
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
