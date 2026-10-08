# Tessera

A modular, native desktop environment for Windows 11, built in Rust. Tessera focuses on a deeply customizable shell with familiar floating-window behavior, replaceable modules, and restricted third-party plugins. Tiling is optional, not the default workflow.

## Status

**Public, unsigned native desktop test alpha; not a daily-driver shell.** Alpha.10 adds separate native dock context menus, validated window commands and read-only geometry checks to the Seelen-style native Rust/Slint presentation introduced in alpha.9. The older alpha.8 and alpha.9 packages are unchanged. See the [alpha tester guide](docs/alpha-testing.md) before testing in a disposable VM; sign-in-shell activation is not needed for the normal desktop session.

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

The geometry domain calculates rules and placement plans without OS calls. Portable capability contracts keep audio, known-folder intent, Gregorian calendar state and reserved Dock effects independent from both layout and native UI. Platform adapters validate explicit effects and acquire real system metadata; native presentation displays confirmed state and pending user intent. The plugin host will expose a restricted interface without handing third-party code control over safety.

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

Install Rust 1.92 or newer. The current Slint release requires this version; earlier versions of the CLI foundation supported Rust 1.85. `rust-toolchain.toml` selects stable Rust with `rustfmt` and Clippy. The workspace contains five crates:

Linux workspace builds (including headless UI tests and cross-target builds) require `pkg-config` and Fontconfig development files. On Debian/Ubuntu, install them with `sudo apt-get install pkg-config libfontconfig1-dev`. Windows builds use the system's native font support; this is not an additional Windows runtime dependency.

| Crate | Responsibility |
| --- | --- |
| `tessera-core` | Pure geometry, window identity and modes, and the `MainStack` layout engine. |
| `tessera-system` | Portable typed system-capability contracts; currently asynchronous default-multimedia audio, with no OS/UI dependencies. |
| `tessera-windows` | Desktop observation, explicit activation/launch, native application icons/events, worker-owned audio, and isolated shell supervision/recovery. |
| `tessera-ui` | Native Slint dock, toolbar, application menu, independent audio popup, portable models, asynchronous observation, and preferences; no Windows-adapter dependency. |
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

### Maintained Slint core patch

All coupled Slint SDK pins remain exactly **1.18.1**. The root [`[patch.crates-io]`](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html#the-patch-section) selects the checked-in `third-party/i-slint-core` source for that one crate; it is excluded from the five-member first-party workspace. This maintained library patch corrects native `ListView` coordinate/height bookkeeping and offscreen retention, without replacing the application's native list, ensure-visible or reset architecture with manual viewport logic, polling or timers.

**Third-party core modified on 2026-10-08.** The published crate's 100 baseline files are retained; only `Cargo.toml`, `model/repeater.rs` and `item_tree.rs` differ. The manifest records the immutable upstream revision, archive SHA-256 and changed-file receipt. Original upstream attribution, licenses and sidecars remain intact; Tessera selects the original `GPL-3.0-only` option. See the [patch provenance and focused test recipe](docs/adr/0005-native-presentation.md#maintained-slint-core-source-patch) and [source/notice packaging requirements](docs/distribution-and-trust.md#patched-sdk-source-and-notices). This unpublished patch does not change alpha.10 or certify Windows composition or complete launcher parity.

### Inspect a Windows desktop

On Windows:

```sh
cargo run -p tessera -- inspect
```

`inspect` reports monitor bounds and work areas, visible top-level desktop-app windows, PIDs, captions, classes, rectangles, and observed window flags. On other operating systems, it reports an unsupported-platform error instead of returning a fake empty desktop.

Current development builds also accept `cargo run -p tessera -- inspect --check-surfaces`. This read-only diagnostic prints the named Tessera surfaces' physical rectangles, monitor bounds/work areas, DPI, styles and show state, and checks the toolbar against the full monitor origin/width and `round(32 * DPI / 96)` height. Missing, ambiguous, stale or incomplete toolbar data fails instead of producing a vacuous pass. Captions are diagnostic hints, never authorization to control a window.

The check does not capture Slint's internal scale or AppBar query/approved-position history and cannot by itself establish the cause of a toolbar offset. The controlled Windows fixture exercises conformant → deliberately offset → restored geometry without changing Explorer, AppBars or foreign windows; cross-compiling that fixture is not evidence that it ran on Windows. Alpha.10 includes this diagnostic; the unchanged alpha.9 package predates it.

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

System/light/dark, compact spacing, and dock edge preview live. **Save preferences** atomically stores the complete appearance, dock pins, and grouped launcher preferences (display mode and independent ordered favorites) in `%LOCALAPPDATA%\Tessera\settings.json`. Pin/favorite/display-mode clicks preserve the last saved appearance rather than an unsaved preview; appearance saves retain both application collections and the saved launcher mode. No captions, HWNDs, PIDs, or sign-in commands are stored there. Launch targets resolve only through the trusted current catalog, never directly from a preferences string.

The primary-monitor toolbar reserves its top work-area strip; the dock remains floating and hides for a conservative fullscreen hint. The application menu is frameless, while the recovery/settings utility keeps normal decorations. Only validated Tessera-owned windows receive native surface styling. Ordinary-session exit restores the original primary taskbar visibility and auto-hide state without changing Winlogon or killing Explorer. Other monitors retain their Windows taskbars. There is no periodic desktop polling, fixed-frame-rate idle rendering, injection, low-level input hook, telemetry, update process, or plugin execution. Supervised sessions use a two-second UI-thread heartbeat; the clock updates at most once per minute. The installer's separate diagnostic mode does not hide taskbars or reserve work area. All three Windows executables embed `asInvoker`, `uiAccess=false`, and PerMonitorV2; unsigned builds are not antivirus-approved. Installation/recovery usage and emergency steps are in the [tester guide](docs/alpha-testing.md).

Unpublished development work adds an independently owned toolbar quick-settings popup with real default-multimedia output/input volume and mute. Typed commands revalidate the expected endpoint, and a single native COM worker performs hardware I/O and confirmed readback without desktop observation. Watching reports actual readiness/failure, rather than treating accepted registration as live updates. This is **not included in alpha.10** and is not complete quick-settings parity: native hardware/driver behavior, exact flat-slider styling, radios, brightness, OS theme/nightlight, power and the remaining system modules still need validation or implementation.

The unpublished launcher layer opens in Favorites, offers real All Apps/Back navigation, routes search to All, and resets query/selection/scroll on view changes and reopen rather than inventing per-view history. All and raw-title substring search expose the complete current matching retained trusted catalog; Favorites resolves the full saved order, skipping unavailable entries without dropping their saved identities. There is no launcher 64-result cap or continuation pagination. Native discovery remains bounded to 1,024 source catalog entries **before deduplication**, not a guarantee of every installed application; the recovery Panel's separate 64-application projection is unchanged. Seven-tile rows use native `ListView` and a lazy model with the existing icon cache, converting requested rows rather than every logical image. Favorite actions neither launch nor change dock pins.

Native footer **Expand/Contract** saves an absolute Windowed/Fullscreen choice and refits the owned launcher without reopening or resetting its current query/view/selection. Windowed is centered at 55% of each monitor dimension, each capped at 1,200 logical pixels; Fullscreen is an activatable topmost overlay filling physical monitor bounds, not exclusive/toolkit fullscreen. Only the current primary-monitor context is supported, not source cursor/trigger-monitor selection. Fullscreen removes this launcher's shadow gutter/radius/shadow, not those of other popovers. A failed disk save leaves the previous geometry and applied state unchanged; a successful save followed by presentation failure may hide the launcher and reports that the mode was saved but could not be presented. Reopen uses the saved mode and the existing Favorites/query-reset policy.

Native Favorites drag ordering is implemented in unpublished development and passes controlled native input, persistence and GL verification. Only a completed changed reorder saves the whole schema-3 record; preview does not save, and failure restores displayed applied order. Resolved favorites refill their existing saved slots, preserving unavailable identities in place along with display mode, dock pins and saved appearance. The native drag recognizer and existing lazy `ListView` remain; a gesture-only 10ms edge-autoscroll timer stops on completion/cancellation, with no idle desktop polling. The pinned Fluent 1.18.1 scrollbar adapter protects the actual 14-logical-pixel native input overlay only when present, without shrinking the full grid or replacing the scrollbar. A stable native focus scope keeps Escape working through source-delegate eviction and restores ordinary tile keyboard input after completion. Exact floating drag visuals, reference activation/touch timing, collision tie-registry equivalence and Windows/OLE behavior remain gaps; this does not add folders or complete favorite context actions.

The unpublished drag-feedback layer passes controlled native and genuine GL verification: one passive whole-tile overlay (icon and name) moves in the existing launcher window, shares normal tile appearance and leaves an invisible retained source slot in the native list. One validated source image and source-local pressed scale survive virtual-row eviction and icon-cache clearing; the native Down appearance is captured once before image conversion, not reread during hover. A rejecting full-window hover observer follows into header/footer/margins without adding drop/save/autoscroll authority. Genuine and fallback icons share a centered square frame; the genuine image is explicitly contained, including both dock callers. Fixed 8px padding/gap and 35.2px label space are not traded for a larger narrow-window icon. Its source-scale gate passed **307 workspace tests**, strict Linux/MSVC all-target checks, warning-free docs and 35 notice assertions; owned GL separately passes at matched native/Slint 1x/2x and widths 560/840, comparing the ghost against the ordinary **pressed** source frame. This adds no native window, renderer, screenshot loop, schema, asset or SDK patch and is **not in alpha.10**. Feedback starts with the first immutable typed DragMove after SDK activation, not the payload-free dragging flag. Native Down-local scale is not literal browser activation-time computed-transform capture. Exact activation timing/threshold, cursor, general ancestor transforms, drop/displacement motion, browser narrow-size behavior and Windows capture/AX remain open.

The unpublished launcher **User** footer now opens a genuine separately owned native popup, not Settings. It displays the existing real shell user name and an original schematic profile fallback, and requests availability of **Recent, Desktop, Downloads, Documents, Music, Pictures and Videos** through a lazy typed Windows capability. Only current Ready rows can request an open; a bounded STA worker freshly resolves and checks the expected folder before Shell dispatch. Loading, denied, changed, unsupported and busy states stay honest, with explicit read retry and no automatic open retry. These OS known folders are not Favorite application-group folders. Settings remains a separate footer action. Footer anchoring, account/photo enrichment, file previews, directory watching, power and the remaining full user menu are still incomplete; accepted dispatch does not prove visible Explorer. Controlled production routing, 345 workspace tests, strict Linux/MSVC checks and native GL dark/light body/shadow frames at 1x/2x pass. This is **not included in alpha.10**, and actual Windows folder redirection, foreground and mixed-DPI behavior remain unverified.

The unpublished toolbar **clock** now opens a genuine native Calendar with variable four-to-six-week month grids, a three-column year view, Previous/Next, Today and projected day/month selection. Dates come from real Windows system-local Gregorian time and user locale/week-start, never parsed clock text or UTC/sample-date fallback. A bounded worker reads localized names; documented empty calendar names inherit the corresponding genuine native locale names. Checked Chrono arithmetic is pure, while selection/browsing cannot change OS time or preferences. Browse/view/selection survives hide/reopen; real Today refreshes on open and each visible minute, with no hidden timer or desktop observation. A shared post-show popup boundary retains existing popups after invalid/cancelled presentation and handles visible foreground-denial errors honestly. The Calendar gate passes **394 workspace tests**, strict Linux/MSVC checks, warning-free docs, 35 notices and owned native GL dark/light body/shadow frames at actual 1x/2x. This is **not in alpha.10**: real Windows focus/locale/DPI behavior, persisted Seelen language/week-start/date-format, exact Moment abbreviations, exact navigator artwork/accent shades and multi-monitor targeting remain uncompleted.

The unpublished Dock now has **Start → Show desktop → applications**, with a separately typed native Shell toggle rather than a fabricated application row, minimize-all loop, saved desktop boolean or Explorer-recovery command. A lazy bounded worker enters strict STA only for one request, activates the fixed Shell interface with narrow in-process/disable-AAA flags, invokes `ToggleDesktop` once, then releases Shell/COM before idle waiting or completion. The utility stays independent of stale app inventory; one flight survives hide/refit and suppresses duplicates, including held Return after completion, without automatic retry or preference/catalog effects. Its exact Phosphor Desktop Duotone artwork is imported unmodified from independent immutable MIT upstream with the complete grant/provenance. **425 workspace tests**, strict Linux/MSVC checks, docs, 35 notice assertions, actual asset notice collection and owned GL dark/light Dock body/glyph frames at 1x/2x pass. This is **not in alpha.10**: genuine Windows Shell success/Explorer absence/registration, source monitor/module preferences, Trash and the rest of Dock parity remain incomplete. The separate known-folder message-aware wait correction is documented below; request-local toggle tests alone do not certify persistent STA behavior.

The unpublished known-folder worker now pumps its own Windows messages while retaining native targets, rather than holding a live STA across a plain blocking channel receive. It keeps the same sixteen-slot queue, seven IDs and strong folder identity checks. An unnamed owned auto-reset event wakes work; message batches are bounded, callbacks run outside service borrows, and final host drop disconnects before signaling without joining. A 250ms safety wait checks only local queue/fault/disconnect state, not folders or the desktop. Native wait/quit/signal failures stop admission, release targets/COM on the worker and fulfill accepted callbacks exactly once. Twenty-five focused recording tests pass, including twelve new pump/race cases. Real Windows dispatch, native handler liveness and measured idle behavior remain unverified; this does not add a native execution timeout or make `NOASYNC` a namespace-completion guarantee.

Current strict schema 3 requires `launcher: { "display_mode": "windowed" | "fullscreen", "favorites": [...] }`. Valid schema 1/2 load without rewriting bytes and default to Windowed; schema 1 starts with empty favorites, while schema 2 preserves their exact saved order. Only a successful explicit Appearance/Pin/Favorite/Display save writes schema 3; completed changed Favorites reorder uses that same full-record format, not a new schema. The complete record shares a 16-KiB budget including its newline; oversized writes fail without truncation or changing applied state. Invalid/future/damaged startup files disable all ordinary preference saves for the session. Preview still works: back up and rename the settings file, then restart to recover. Alpha.10 and old unpublished schema-2 readers reject schema 3, and their explicit Save paths may replace it and lose favorites/mode: back up before downgrading. There is no automatic settings backup or multi-instance merge. These changes are **not included in alpha.10**; full Settings, scopes/prefixes, files/web, ranking, folders, favorite context actions and exact remaining drag behavior, global triggers, native user/power footer, caret-end Right transfer, Windows IME/focus/accessibility, offscreen Tab semantics and target/multi-monitor placement remain required. Controlled display-mode and Favorites-order cases and actual Mesa GL frames at 1x/2x pass; native Windows/VM validation remains separate.

### Verification and limitations

`Cargo.lock` is committed. CI runs **only when a release tag matching `v*` is pushed**, not on ordinary branch commits or pull requests. Its matrix covers Linux stable, Windows stable, and Windows Rust 1.92. Run the local checks above before pushing changes. Headless UI tests cover refresh behavior and error states. A Windows test creates a controlled window, reads it from another process, checks caption escaping and unchanged geometry, then destroys the fixture. Callback-panic handling and DPI restoration are also tested.

After all checks pass, maintainer-authorized numbered `v*-alpha.N` tags build verified unsigned test assets, complete vendored source, and SHA-256 checksums. These are public experimental prereleases, not consumer releases. Packaging verifies all three executable subsystems/resources/manifests, static CRT imports, CLI commands, deployment completeness, and the real two-heartbeat runtime probe without changing sign-in settings. Windows deployment tests use isolated registry subtrees, never real Winlogon. There is no unsigned stable/consumer publication path. See [distribution and trust](docs/distribution-and-trust.md).

These checks do not validate a complete shell. Interactive Windows 11 testing is still needed for actual sign-in/rollback, accessibility, keyboard/focus, idle resource use, mixed DPI, multiple monitors, games, Explorer reappearance, and security-product compatibility. A supervisor blocked before launch cannot perform its own fallback; the independent restore and Task Manager emergency path remain mandatory.

Home and Pro are targets, but Microsoft's [Shell Launcher](https://learn.microsoft.com/en-us/windows/configuration/shell-launcher/) is unavailable on them. The selected isolated per-user Winlogon integration is experimental, not an edition-independent Microsoft support guarantee; machine shell configuration and security policies remain untouched.

## Roadmap

1. Pure layout calculation and a side-effect-free demo — implemented.
2. Real Windows observation — implemented.
3. Native application icons/launch/dock pins, explicit window switching, and passive updates — implemented; the Seelen-style desktop presentation and transient taskbar handoff are under interactive Windows validation.
4. System/light/dark, compact, dock edge, dock-pin and independent launcher-favorite preferences — implemented in development; broader profiles, replaceable presentation, and motion remain.
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
