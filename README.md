# Tessera

A modular, native desktop environment for Windows 11, built in Rust. Tessera focuses on a deeply customizable shell with familiar floating-window behavior, replaceable modules, and restricted third-party plugins. Tiling is optional, not the default workflow.

## Status

**Private test alpha; not a daily-driver shell.** The first alpha adds a native window switcher with title search, explicit activation, and saved appearance preferences to the layout/inspection foundation. See the [alpha tester guide](docs/alpha-testing.md) for the portable package, security limitations, and checklist.

Tessera does not automatically rearrange application windows or modify Explorer. The panel is a normal utility window, not a dock or taskbar replacement. Only choosing an application requests foreground activation and, if minimized, asynchronous restoration. Shell replacement and the plugin runtime are not implemented. Windows 11 x64 with the MSVC toolchain is the initial platform target; other architectures need separate validation.

## Product direction

- Conventional desktop behavior by default: floating windows, mouse dragging and resizing, minimize, maximize, and application fullscreen, without automatic rearrangement.
- Replaceable dock/taskbar, notification area, launcher, desktop, system panels, notification center, and widgets.
- Profiles combining module selection, layouts, themes, icons, and motion settings, including macOS-inspired dock-and-top-panel arrangements.
- Native Rust presentation: no WebView, Electron, or HTML/CSS/JavaScript UI stack, including for custom shell modules.
- Fully replaceable presentation, while permissions and recovery remain under trusted control.
- Restricted plugins for themes, layouts, commands, integrations, and shell modules, without arbitrary system access.
- GUI-based configuration for a broad audience, rather than mandatory configuration-file editing.
- Independent workspaces per monitor, with linked switching for monitor groups.
- Optional tiling: main-and-stack layout and mouse-driven reordering with a placement preview, enabled explicitly.
- Explicit opt-in shell replacement with a recovery path; a separate choice of file manager, with Explorer as the default.
- Games and fullscreen applications left alone by default.

These are planned capabilities, not a list of completed features.

Customization applies first to Tessera-owned surfaces. Animating foreign application windows, integrating Windows notifications, and replacing system flyouts are separate compatibility-sensitive platform features, not automatic consequences of theming. Tessera does not aim to replace DWM or promise arbitrary restyling of other applications.

## Architecture

The domain model calculates rules and placement plans without OS calls. Platform integration observes windows and eventually applies validated commands. Native presentation displays state. The plugin host will expose a restricted interface without handing third-party code control over safety.

Start with working modules, then add layers. Do not create empty crates for hypothetical features or freeze a plugin interface before reviewing its threat model.

- [Domain glossary](CONTEXT.md)
- [Domain, platform, and presentation](docs/adr/0001-domain-and-platform.md)
- [Restricted plugins](docs/adr/0002-restricted-plugins.md)
- [Shell activation and recovery](docs/adr/0003-shell-activation-and-recovery.md)
- [Conventional desktop and native customization](docs/adr/0004-conventional-native-desktop.md)
- [Native presentation toolkit](docs/adr/0005-native-presentation.md)
- [Distribution, trust, and user comfort](docs/distribution-and-trust.md)

## Development

Install Rust 1.92 or newer. The current Slint release requires this version; earlier versions of the CLI foundation supported Rust 1.85. `rust-toolchain.toml` selects stable Rust with `rustfmt` and Clippy. The workspace contains four crates:

Linux workspace builds (including headless UI tests and cross-target builds) require `pkg-config` and Fontconfig development files. On Debian/Ubuntu, install them with `sudo apt-get install pkg-config libfontconfig1-dev`. Windows builds use the system's native font support; this is not an additional Windows runtime dependency.

| Crate | Responsibility |
| --- | --- |
| `tessera-core` | Pure geometry, window identity and modes, and the `MainStack` layout engine. |
| `tessera-windows` | Read-only desktop observation and separately scoped explicit foreground activation; handwritten Win32 FFI stays here. |
| `tessera-ui` | Native Slint presentation, portable window rows, asynchronous observation, search, and preference preview; no Windows-adapter dependency. |
| `tessera` | Shared application host, bounded atomic preferences, GUI launcher, and developer CLI. |

```sh
cargo run -p tessera -- demo
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo doc --workspace --no-deps --locked
```

`demo` deliberately exercises the **optional tiling engine**, not the default desktop experience. It uses a synthetic work area with a negative origin, three explicitly tiled windows, one floating window, and one fullscreen window. Only tiled windows appear in the plan; `WindowMode::default()` is `Floating`. The default **layout parameters** give the main column 60% of the width after subtracting an eight-physical-pixel gap. There is no outer gap; remaining stack-height pixels are distributed from top to bottom.

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

### Open the native panel

On Windows:

```sh
cargo run -p tessera -- panel
# Or launch the GUI binary directly (no console subsystem on Windows):
cargo run -p tessera --bin tessera-desktop
```

The native Slint window shows ordinary application candidates and monitor/application/warning counts. It observes at startup and on **Refresh**, off the UI thread with one request in flight. Search uses retained data, not a new observation. Failed refreshes retain explicitly stale data and disable activation until a successful refresh. Captions are bounded plain text, never commands or markup.

Choose a row to request foreground activation; the platform revalidates its transient HWND/PID and eligibility. Minimized targets may be restored asynchronously only on that explicit input. Windows foreground restrictions are respected and reported, not bypassed. Observation and activation cannot be made atomic; same-process handle reuse remains a race.

System/light/dark and compact spacing preview live. **Save preferences** writes only versioned appearance data to `%LOCALAPPDATA%\Tessera\settings.json`, atomically; invalid or future files stay untouched until an explicit save. No captions or system identities are stored.

The panel has native decorations and normal move/resize/minimize/close behavior. It is not always-on-top and does not hide Explorer, reserve work area, install autostart, run plugins, or perform periodic background polling. Linux rejects native launch rather than showing a fake desktop. Qt, WebView, GPU renderers, and inspection servers remain disabled. Both Windows launchers embed `asInvoker`, `uiAccess=false`, and PerMonitorV2; unsigned alpha artifacts are not antivirus-approved.

### Verification and limitations

`Cargo.lock` is committed. CI runs **only when a release tag matching `v*` is pushed**, not on ordinary branch commits or pull requests. Its matrix covers Linux stable, Windows stable, and Windows Rust 1.92. Run the local checks above before pushing changes. Headless UI tests cover refresh behavior and error states. A Windows test creates a controlled window, reads it from another process, checks caption escaping and unchanged geometry, then destroys the fixture. Callback-panic handling and DPI restoration are also tested.

After all checks pass, numbered `v*-alpha.N` tags in the private repository also build verified unsigned portable assets, complete corresponding source with vendored dependencies, and SHA-256 checksums. The packaging script checks x64 GUI/console PE subsystems, product version, embedded privilege/DPI manifests, static CRT imports, and packaged CLI commands. This private-alpha path does not publish unsigned consumer/stable releases. See [distribution and trust](docs/distribution-and-trust.md).

These checks do not validate a complete shell. Interactive Windows 11 testing is still needed for panel rendering, native accessibility, keyboard/focus behavior, idle resource use, mixed DPI, multiple monitors, privilege boundaries, games, the notification area, recovery, and security-product compatibility. A Linux cross-target check is not a Windows runtime test.

Home and Pro are target editions, but Microsoft's [Shell Launcher](https://learn.microsoft.com/en-us/windows/configuration/shell-launcher/) is unavailable on them. Shell activation mechanisms have not been selected.

## Roadmap

1. Pure layout calculation and a side-effect-free demo — implemented.
2. Real Windows observation — implemented.
3. Native panel alongside Explorer with explicit window switching and title search — implemented; a dock and application launching are next.
4. System/light/dark and compact preferences — implemented; broader profiles, replaceable presentation, themes, and motion remain.
5. An isolated host with one useful plugin, followed by replaceable modules.
6. Workspaces, system integrations, and optional window-placement commands with application rules and safety checks.
7. Remaining shell modules and opt-in shell replacement after recovery has been verified.

Each layer builds on a working previous layer.

## Architectural references

- [Seelen UI](https://seelen.io/apps/seelen-ui): functional reference for customizable docks, toolbars, launchers, widgets, themes, and notification panels. Its [upstream README](https://github.com/eythaann/Seelen-UI) documents a required WebView runtime; Tessera adopts the product direction, not its web UI stack.
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
