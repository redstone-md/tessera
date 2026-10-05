# Tessera

A modular desktop environment for Windows 11, built in Rust. Tessera aims to combine hybrid window management, a replaceable shell, native UI, and restricted third-party plugins.

## Status

**Early development; not a daily-driver shell.** The current implementation provides a pure layout engine, a synthetic layout demo, and read-only observation of real Windows windows and monitors through `inspect`.

Tessera does **not** move windows or modify Explorer. The panel, shell replacement, and plugin runtime are not implemented. Windows 11 x64 is the initial platform target; other architectures need separate validation.

## Product direction

- Hybrid window management: automatic tiling and floating windows.
- Main-and-stack layout: one main window on the left, other tiled windows stacked on the right.
- Mouse-driven reordering with a placement preview.
- Independent workspaces per monitor, with linked switching for monitor groups.
- Replaceable taskbar and notification area, launcher, desktop, system panels, and widgets.
- Fully replaceable presentation, while permissions and recovery remain under trusted control.
- Restricted plugins for themes, layouts, commands, integrations, and shell modules, without arbitrary system access.
- GUI-based configuration for a broad audience, rather than mandatory configuration-file editing.
- Explicit opt-in shell replacement with a recovery path; a separate choice of file manager.
- Fullscreen applications excluded from automatic placement by default.

These are planned capabilities, not a list of completed features.

## Architecture

The domain model calculates rules and placement plans without OS calls. Platform integration observes windows and eventually applies validated commands. Native presentation displays state. The plugin host will expose a restricted interface without handing third-party code control over safety.

Start with working modules, then add layers. Do not create empty crates for hypothetical features or freeze a plugin interface before reviewing its threat model.

- [Domain glossary](CONTEXT.md)
- [Domain, platform, and presentation](docs/adr/0001-domain-and-platform.md)
- [Restricted plugins](docs/adr/0002-restricted-plugins.md)
- [Shell activation and recovery](docs/adr/0003-shell-activation-and-recovery.md)

## Development

Install Rust 1.85 or newer. `rust-toolchain.toml` selects stable Rust with `rustfmt` and Clippy. The workspace contains three crates:

| Crate | Responsibility |
| --- | --- |
| `tessera-core` | Pure geometry, window identity and modes, and the `MainStack` layout engine. |
| `tessera-windows` | Desktop observation through official `windows-sys` bindings; Win32 and production `unsafe` are isolated in the platform module. |
| `tessera` | A CLI that exercises the domain and platform interfaces. |

```sh
cargo run -p tessera -- demo
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo doc --workspace --no-deps --locked
```

`demo` uses a work area with a negative origin, three tiled windows, one floating window, and one fullscreen window. Only tiled windows appear in the plan. By default, the main column receives 60% of the width after subtracting an eight-physical-pixel gap. There is no outer gap; remaining stack-height pixels are distributed from top to bottom.

Invalid geometry, duplicate identities, and insufficient space return a layout error without a partial plan. Real application size constraints and applying plans are not implemented yet.

### Inspect a Windows desktop

On Windows:

```sh
cargo run -p tessera -- inspect
```

`inspect` reports monitor bounds and work areas, visible top-level desktop-app windows, PIDs, captions, classes, rectangles, and observed window flags. On other operating systems, it reports an unsupported-platform error instead of returning a fake empty desktop.

- Observation is sequential, not atomic. Windows can close and monitors can disconnect during collection.
- Invisible windows and windows belonging to the calling process are excluded. Tool, owned, and system windows can appear: observation is not permission to manage them.
- Coordinates use physical pixels under a temporary thread-local DPI context, restored on exit. `GetWindowRect` includes invisible resize borders.
- `covers-monitor` is a geometric hint, not a reliable fullscreen classification. `cloaked=unknown` means the DWM query failed.
- Per-window read failures appear in `Warnings`; some invalid or disappearing records are skipped. Enumeration or monitor-geometry errors abort the observation.
- Identities contain transient system-handle values. Do not persist them as application or monitor identities.
- Captions and classes are escaped for terminal output. Fixed buffers limit captions to 1,023 UTF-16 code units and classes to 255; longer values may be truncated.

The adapter uses documented [EnumWindows](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-enumwindows), [GetWindowRect](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowrect), and [SetThreadDpiAwarenessContext](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setthreaddpiawarenesscontext) APIs. Observation does not write settings, save captions to files, or launch applications.

### Verification and limitations

`Cargo.lock` is committed. CI checks Linux stable, Windows stable, and Windows Rust 1.85 so the minimum supported version also covers native code. A Windows test creates a controlled window, reads it from another process, checks caption escaping and unchanged geometry, then destroys the fixture. Callback-panic handling and DPI restoration are also tested.

These checks do not validate a complete shell. Manual Windows 11 testing is still needed for mixed DPI, multiple monitors, windows closing during commands, privilege boundaries, games, the notification area, and recovery.

Home and Pro are target editions, but Microsoft's [Shell Launcher](https://learn.microsoft.com/en-us/windows/configuration/shell-launcher/) is unavailable on them. Shell activation mechanisms have not been selected.

## Roadmap

1. Pure layout calculation and a side-effect-free demo — implemented.
2. Real Windows observation — implemented; safe plan application alongside Explorer is next.
3. A minimal native panel, hotkeys, and tiled/floating mode switching.
4. Workspaces, settings, application rules, and mouse interaction.
5. An isolated host with one useful plugin, followed by replaceable modules.
6. Remaining shell modules and opt-in shell replacement after recovery has been verified.

Each layer builds on a working previous layer.

## Architectural references

- [komorebi](https://github.com/LGUG2Z/komorebi): window management on top of DWM, separated commands and panels, and reversible changes.
- [GlazeWM](https://github.com/glzr-io/glazewm): layouts, window rules, and independent panel integration.

These projects inform separation of responsibilities; their implementations are not copied into Tessera.

## Contributing

English is the project language. Read the [contribution guide](CONTRIBUTING.md) for setup, architecture expectations, tests, and pull requests. Participation follows the [Code of Conduct](CODE_OF_CONDUCT.md). Report security concerns according to the [security policy](SECURITY.md), not the normal bug-report process.

The repository remains private during early development. These files prepare it for public OSS collaboration; they do not announce a public release.

## License

Copyright (C) 2026 Tessera contributors.

Tessera source code and documentation are licensed under the **GNU General Public License, version 3 only** (`GPL-3.0-only`). See [LICENSE](LICENSE) for the complete terms. This grant does not include the option to choose a later GPL version.

The software is provided without warranty. Contributors retain their copyrights; third-party dependencies retain their own licenses and notices.
