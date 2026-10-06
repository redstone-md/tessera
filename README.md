# Tessera

A modular, native desktop environment for Windows 11, built in Rust. Tessera focuses on a deeply customizable shell with familiar floating-window behavior, replaceable modules, and restricted third-party plugins. Tiling is optional, not the default workflow.

## Status

**Public, unsigned native desktop test alpha; not a daily-driver shell.** Alpha.9 introduces Seelen UI's standard dock, toolbar, and application-menu presentation in native Rust/Slint, with a reversible temporary taskbar handoff. The older alpha.8 package is unchanged. See the [alpha tester guide](docs/alpha-testing.md) before testing in a disposable VM; sign-in-shell activation is not needed for the normal desktop session.

Ordinary launch and installation leave Explorer as the sign-in shell. Ordinary GUI launch starts a supervised temporary desktop session: Tessera owns the primary-monitor presentation and restores Explorer's prior taskbar state on exit. Only explicit `Install-Tessera.ps1 -EnableShell` may replace the sign-in shell for the current user, after backup and a real supervisor/GUI heartbeat probe; it applies at the next sign-in and never kills the current Explorer session. Tessera does not replace DWM or automatically rearrange application windows. Windows 11 x64 with MSVC is the initial target; shell activation refuses Windows Server, domain-managed hosts, conflicting policies, and unsupported security states.

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
- [Per-user installation and independent recovery](docs/adr/0006-per-user-shell-recovery.md)
- [Distribution, trust, and user comfort](docs/distribution-and-trust.md)

## Development

Install Rust 1.92 or newer. The current Slint release requires this version; earlier versions of the CLI foundation supported Rust 1.85. `rust-toolchain.toml` selects stable Rust with `rustfmt` and Clippy. The workspace contains four crates:

Linux workspace builds (including headless UI tests and cross-target builds) require `pkg-config` and Fontconfig development files. On Debian/Ubuntu, install them with `sudo apt-get install pkg-config libfontconfig1-dev`. Windows builds use the system's native font support; this is not an additional Windows runtime dependency.

| Crate | Responsibility |
| --- | --- |
| `tessera-core` | Pure geometry, window identity and modes, and the `MainStack` layout engine. |
| `tessera-windows` | Desktop observation, explicit activation/launch, native application icons/events, and isolated shell supervision/recovery. |
| `tessera-ui` | Native Slint dock, toolbar, application menu, portable models, shared asynchronous observation, and preferences; no Windows-adapter dependency. |
| `tessera` | Application composition, bounded atomic preferences, GUI session, recovery supervisor, and developer CLI. |

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

### Open the native dock or utility panel

On Windows:

```sh
cargo run -p tessera -- panel
# Or launch the GUI binary directly (no console subsystem on Windows):
cargo run -p tessera --bin tessera-desktop
```

The GUI binary starts a temporary desktop session with a centered icon dock, top toolbar, and application menu based on Seelen UI's standard theme; `panel` opens the ordinary utility/settings window. A sibling supervisor owns the GUI heartbeat and Explorer taskbar restoration. Build both Windows binaries together with `cargo build -p tessera --bins` before directly launching the development GUI. Search uses retained data, not a new observation. Native out-of-context desktop notifications coalesce into single-flight background observations, with manual Refresh fallback. Failed refreshes retain explicitly stale data and disable data-dependent actions. Captions and names are bounded plain text, never commands or markup. Installed-application and window icons are cached separately from window observations; newly installed apps may take a refresh after the cache interval or a restart to appear.

Choose a row to request foreground activation; the platform revalidates its transient HWND/PID and eligibility. Minimized targets may be restored asynchronously only on that explicit input. Windows foreground restrictions are respected and reported, not bypassed. Observation and activation cannot be made atomic; same-process handle reuse remains a race.

System/light/dark, compact spacing, and dock edge preview live. **Save preferences** atomically stores these choices and pins in `%LOCALAPPDATA%\Tessera\settings.json`; a pin click persists pins with the last saved appearance, not an unsaved preview. No captions, HWNDs, PIDs, or sign-in commands are stored there. Launch targets resolve only through the trusted current catalog, never directly from a preferences string.

The primary-monitor toolbar reserves its top work-area strip; the dock remains floating and hides for a conservative fullscreen hint. The application menu is frameless, while the recovery/settings utility keeps normal decorations. Only validated Tessera-owned windows receive native surface styling. Ordinary-session exit restores the original primary taskbar visibility and auto-hide state without changing Winlogon or killing Explorer. Other monitors retain their Windows taskbars. There is no periodic desktop polling, fixed-frame-rate idle rendering, injection, low-level input hook, telemetry, update process, or plugin execution. Supervised sessions use a two-second UI-thread heartbeat; the clock updates at most once per minute. The installer's separate diagnostic mode does not hide taskbars or reserve work area. All three Windows executables embed `asInvoker`, `uiAccess=false`, and PerMonitorV2; unsigned builds are not antivirus-approved. Installation/recovery usage and emergency steps are in the [tester guide](docs/alpha-testing.md).

### Verification and limitations

`Cargo.lock` is committed. CI runs **only when a release tag matching `v*` is pushed**, not on ordinary branch commits or pull requests. Its matrix covers Linux stable, Windows stable, and Windows Rust 1.92. Run the local checks above before pushing changes. Headless UI tests cover refresh behavior and error states. A Windows test creates a controlled window, reads it from another process, checks caption escaping and unchanged geometry, then destroys the fixture. Callback-panic handling and DPI restoration are also tested.

After all checks pass, maintainer-authorized numbered `v*-alpha.N` tags build verified unsigned test assets, complete vendored source, and SHA-256 checksums. These are public experimental prereleases, not consumer releases. Packaging verifies all three executable subsystems/resources/manifests, static CRT imports, CLI commands, deployment completeness, and the real two-heartbeat runtime probe without changing sign-in settings. Windows deployment tests use isolated registry subtrees, never real Winlogon. There is no unsigned stable/consumer publication path. See [distribution and trust](docs/distribution-and-trust.md).

These checks do not validate a complete shell. Interactive Windows 11 testing is still needed for actual sign-in/rollback, accessibility, keyboard/focus, idle resource use, mixed DPI, multiple monitors, games, Explorer reappearance, and security-product compatibility. A supervisor blocked before launch cannot perform its own fallback; the independent restore and Task Manager emergency path remain mandatory.

Home and Pro are targets, but Microsoft's [Shell Launcher](https://learn.microsoft.com/en-us/windows/configuration/shell-launcher/) is unavailable on them. The selected isolated per-user Winlogon integration is experimental, not an edition-independent Microsoft support guarantee; machine shell configuration and security policies remain untouched.

## Roadmap

1. Pure layout calculation and a side-effect-free demo — implemented.
2. Real Windows observation — implemented.
3. Native application icons/launch/pins, explicit window switching, and passive updates — implemented; the Seelen-style desktop presentation and transient taskbar handoff are under interactive Windows validation.
4. System/light/dark, compact, dock edge, and pin preferences — implemented; broader profiles, replaceable presentation, and motion remain.
5. An isolated host with one useful plugin, followed by replaceable modules.
6. Workspaces, system integrations, and optional window-placement commands with application rules and safety checks.
7. Independent install/restore and supervised opt-in shell activation — implemented experimentally; Windows 11 sign-in validation and remaining shell modules remain.

Each layer builds on a working previous layer.

## Architectural references

- [Seelen UI](https://seelen.io/apps/seelen-ui/customizable-shell): the visual reference for the default dock, toolbar, and application menu, as well as the broader customizable-shell direction. Source measurements are pinned in the [presentation decision](docs/adr/0005-native-presentation.md). Its [upstream README](https://github.com/eythaann/Seelen-UI) documents a required WebView runtime; Tessera independently implements the native presentation without copying its AGPL source or artwork.
- [komorebi](https://github.com/LGUG2Z/komorebi): window management on top of DWM, separated commands and panels, and reversible changes.
- [GlazeWM](https://github.com/glzr-io/glazewm): layouts, window rules, and independent panel integration.
- [Cairo Desktop](https://github.com/cairoshell/cairoshell): an established alternate Explorer-shell product; independent recovery and conventional desktop behavior inform the product constraints.

These projects inform separation of responsibilities; their implementations are not copied into Tessera.

## Contributing

English is the project language. Read the [contribution guide](CONTRIBUTING.md) for setup, architecture expectations, tests, and pull requests. Participation follows the [Code of Conduct](CODE_OF_CONDUCT.md). Report security concerns according to the [security policy](SECURITY.md), not the normal bug-report process.

The repository is public and open to OSS collaboration. Public test alphas remain experimental; trusted signing and interactive Windows 11 release gates are not waived by repository visibility.

## License

Copyright (C) 2026 Tessera contributors.

Tessera source code and documentation are licensed under the **GNU General Public License, version 3 only** (`GPL-3.0-only`). See [LICENSE](LICENSE) for the complete terms. This grant does not include the option to choose a later GPL version.

The software is provided without warranty. Contributors retain their copyrights; third-party dependencies retain their own licenses and notices.
