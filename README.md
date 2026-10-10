# Tessera

A modular, native desktop environment for Windows 11, built in Rust. Tessera focuses on a deeply customizable shell with familiar floating-window behavior, replaceable modules, and restricted third-party plugins. Tiling is optional, not the default workflow.

## Status

**Public, unsigned native desktop test alpha; not a daily-driver shell.** **Alpha.20 is latest available, with production CI and all three actual downloads independently verified.** [Download the Windows x64 test ZIP](https://github.com/redstone-md/tessera/releases/download/v0.1.0-alpha.20/tessera-0.1.0-alpha.20-windows-x64.zip) and read its frozen `START-HERE.txt` plus the [maintained tester guide](docs/alpha-testing.md); see the [central publication receipt](docs/distribution-and-trust.md#published-alpha20-receipt). Alpha.20 includes scoped current-player seek and repaired audio admission; seek changes the real player position. It has no 250ms interpolation/absolute-label layer. Main separately advances to alpha.21 metadata and display work in progress, not covered by these receipts or claimed completed gates. Older releases remain unchanged; publication does not complete full native 1:1 or interactive Windows certification.

**Published Power and User Log Out actions are real and direct:** Lock, Log out, Power off, Reboot, Suspend, Hibernate and the User-popup/avatar-corner Log Out command have no extra Tessera confirmation. Save work and select each only deliberately in a safe session; never “click all” as a smoke test. Update hints/accepted requests do not prove completed shutdown, logoff or updates.

The immutable alpha.10 (`4866d3a`, [CI receipt](https://github.com/redstone-md/tessera/actions/runs/37675695227)) introduced separate dock context menus, validated window commands and read-only geometry checks after alpha.9's visual/session redesign. Alpha.8/.9/.10 packages remain unchanged; their receipts do not certify the new candidate. Work toward the complete native Seelen contract continues after the tester release.

**Separate alpha.11 tag — CI failed, no release assets:** `release/alpha.11`, full source `363fe510a0540ed7c984345a2edd3692266dcfe8`, is verified `df36b2f` plus four metadata/docs-only commits. Its local rerun passed **649 tests across 19 suites**, and `v0.1.0-alpha.11` was verified/pushed at that exact peeled SHA. [CI 37878846670](https://github.com/redstone-md/tessera/actions/runs/37878846670) **failed** on stable Rust 1.99 atomic/chunks lints and two Windows Rust 1.92 font assumptions (395 UI tests passed). Packaging did not run; no public alpha.11 assets exist. The failed tag stays immutable, never retagged.

**Baseline-only alpha.12 — code checks green, packaging failed:** branch `release/alpha.12`, source `7531f04d4270fd30403c500b099ddeaed8473d2f`, immutable `v0.1.0-alpha.12`. Local gates passed 649 tests/19 suites (397 UI/188 Windows-adapter pure), strict Rust 1.92/1.99 Linux/MSVC checks, warnings-denied docs, 35 notices and format/diff. [CI 37881361921](https://github.com/redstone-md/tessera/actions/runs/37881361921) is **terminal failed**: all Linux/Windows stable/Windows Rust 1.92 code jobs passed tests, strict lint/docs, demo, read-only inspect and deployment fixtures, but packaging's runtime supervisor exited **1**. GUI stderr was discarded and the numerical GUI exit was not retained; the cause is open, not proven “policy”. **No ZIP/checksums, public release or artifacts were produced.** The tag stays immutable.

**Baseline-only alpha.13 — code green, runtime packaging failed:** remote `release/alpha.13` and immutable `v0.1.0-alpha.13` remain at `6b31b64efd9cdc92f5edb7cffa0e64ed2b121a57`. Runtime `28b9987` plus metadata `6b31b64` added bounded stderr and an inert synthetic-context/default-preferences host while retaining the actual production GUI/two-heartbeat barrier. Local 660-test/19-suite receipt (UI 397 plus one ignored, app 16, Windows pure 196), strict Rust 1.92/1.99 Linux/MSVC/docs/35 notices/format-diff and focused 44-assertion `-RuntimeOnly` deployment checks passed. Full registry fixtures require Windows. [CI 37885358935](https://github.com/redstone-md/tessera/actions/runs/37885358935) **finished failed only at packaging**: all three code jobs, including full isolated Windows deployment fixtures, passed. No release assets; baseline first Lock excludes alpha.15's full-six/modules/General V4/live display.

Alpha.13 exited `0x00000001` before its first heartbeat, with successful owned-process cleanup; Rust captured 106 stderr bytes that PowerShell then discarded. Later source diagnosis reproduced duplicate Winit backend selection and the forbidden second event-loop creation. A single UI backend owner/passive first show corrected it: source `02fafe2112ce8faf0cd9f30d61c478e4ac3c8abd`, [NativeDebug 37888007340](https://github.com/redstone-md/tessera/actions/runs/37888007340), **passed in 3m38s with two real production UI-thread heartbeats and five-second owned-GUI cleanup**. The matching SDK error size is consistent with alpha.13, not recovery of its discarded text. This proves native debug startup, not release packaging, normal-session effects or a policy/renderer bypass.

**Published immutable baseline alpha.14:** `release/alpha.14` and `v0.1.0-alpha.14` identify source `e95dbb1e49e04a8e63d81fee6bc6f76274a36cfc`, atop corrected runtime source `02fafe2112ce8faf0cd9f30d61c478e4ac3c8abd`. Local gates passed **664 tests/19 suites** (UI 397 plus one ignored, Windows pure 196, app 20), strict Rust 1.92/1.99 Linux/MSVC all-target checks, Rust 1.92 warnings-denied docs, 35 notices, 101 `-RuntimeOnly` assertions and format/diff. [Production release CI 37889391783](https://github.com/redstone-md/tessera/actions/runs/37889391783) **passed all three code jobs, Windows packaging and publication**, including two real production GUI UI-thread pulses and five-second owned-process cleanup. The nondraft prerelease has three downloaded, checksum-verified assets; [exact hashes and source receipt](docs/distribution-and-trust.md#published-alpha14-receipt) are recorded separately. This first-Lock/runtime/font baseline does not include alpha.15's seven native layers/six actions/General V4 or certify normal Windows sessions, source pixels or full parity.

**Immutable full-wave alpha.15 — Windows documentation failed, no assets:** `v0.1.0-alpha.15` remains at source `5a9828f49f011cbb0fa410c032c2db409c494112`. [Production CI 37897841677](https://github.com/redstone-md/tessera/actions/runs/37897841677) passed all Linux gates and both Windows Rust 1.92/stable Clippy and tests, then both Windows jobs failed warnings-denied rustdoc solely on two bare Microsoft SDK URLs in `native_power_updates/sdk.rs` lines 6–7. Packaging was skipped; no alpha.15 release assets exist. The annotated tag object is `c1aa1f8bb462f1ad3f044fe3527b9211d4138b96`; the failed tag is immutable, never deleted or retagged.

## Alpha.16 release checkpoint

**Published immutable alpha.16:** verified `release/alpha.16` and `v0.1.0-alpha.16` remain at source `34ff877576e9ed68b5b2b5d5b7a987c94fe51672` (annotated object `d2a87f44f367bdb8d32336b38adcdb787a08bd26`). [Production CI 37902905003](https://github.com/redstone-md/tessera/actions/runs/37902905003) passed all three code jobs and optimized Windows packaging, including two real production GUI UI-thread pulses and five-second owned cleanup, then published a nondraft prerelease. All three downloaded assets passed independent hash/package/source verification. **Alpha.16 remains available unchanged; alpha.20 is now latest available.** See the [download and corresponding-source receipt](docs/distribution-and-trust.md#published-alpha16-receipt).

The frozen repair retains alpha.15’s seven native layers, six Power actions and schema-4 preferences, with only two SDK documentation hyperlinks and version/release metadata changed. **Final alpha.16 local gate receipt:** **1,120 workspace tests passed across 19 suites, one separately gated GL case ignored, zero failed**. All four strict workspace/all-target Clippy checks and all four warnings-denied workspace documentation checks passed for Rust **1.92/1.99 × Linux/MSVC**. The remaining documentation pass completed in **54.09s** with generated docs isolated by toolchain; no source change or lint allowance was needed. **35 notice assertions**, **101 recording-only `-RuntimeOnly` deployment assertions**, formatting and diff checks passed. The historical **116 Linux GL PPM exports (58 at each scale)** are inherited, not rerun for alpha.16 and not Windows compositor evidence. These local/MSVC cross-target gates do not certify native runtime effects or optimized release packaging.

**Published alpha.17:** global shortcuts, profile/photo, shared-image decoding, AtPoint placement and schema-5 preferences extend frozen alpha.16. Production CI, optimized packaging and downloaded-source/assets passed; see the [published receipt](docs/distribution-and-trust.md#published-alpha17-receipt) and [tester guide](docs/alpha-testing.md). Ordinary launch defaults global shortcuts enabled; eligible bare Win and Win+K behavior remains subject to Windows/interactive validation. Back up settings before the schema-5 migration. User-popup Log Out and the newer alpha.18 font-derived glyph are not included.

**Published alpha.18:** adds source-scoped User-popup/avatar-corner Log Out and its independently derived OFL-font glyph. Production CI, optimized runtime verification and exhaustive downloaded package/source verification passed; its [publication receipt](docs/distribution-and-trust.md#published-alpha18-receipt) remains unchanged. The direct logoff action is not live-Windows outcome certification.

**Published historical alpha.19:** adds native observed current-player timeline and the shared media card in existing Toolbar Quick Settings, including read-only time/progress and the three real transports. It observes with saved Dock media off without saving or allocating Dock slots. Production CI, optimized runtime verification and exhaustive downloaded-asset/source verification passed; see the [actual publication receipt](docs/distribution-and-trust.md#published-alpha19-receipt). No seek/interpolation or full sessions/devices/mixer parity is claimed for this frozen release. **Latest published alpha.20** adds [seek](#alpha20-private-seek-source-receipt) and [audio-admission repair](#alpha20-audio-admission-source-receipt), with separate [production/download evidence](docs/distribution-and-trust.md#published-alpha20-receipt).

**Historical full-wave Windows Debug preflight:** [CI 37893888958](https://github.com/redstone-md/tessera/actions/runs/37893888958) passed in **8m6s** at source `8eeb17d567e17c8aaa90476feb8ec2ae7955ff05`: **1,186 tests across 19 suites, zero failed or ignored**, Rust 1.92/static CRT, followed by two real production GUI UI-thread pulses and five-second owned-GUI cleanup. This was **Debug** diagnostic startup, skipping normal providers/preferences/hooks, not optimized alpha.16 release packaging or certification of native effects, focus, privacy, pixels, multiple monitors or sign-in/recovery.

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

The geometry domain calculates rules and placement plans without OS calls. Portable capability contracts keep audio, folders, calendar, radios, input profiles, media, visibility and Power independent from geometry and native UI. Platform adapters validate explicit effects and acquire real system metadata; native presentation displays confirmed state and pending user intent. Observation never grants activation authority. The plugin host will expose a restricted interface without handing third-party code control over safety.

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
| `tessera-core` | Pure geometry, window identity/modes, the `MainStack` layout engine, and shared validated source-color intent; no OS/UI dependencies. |
| `tessera-system` | Portable typed system-capability contracts and pure calendar/visibility state; no OS/UI dependencies. |
| `tessera-windows` | Desktop observation, explicit native effects, worker-owned system providers/events, application icons, and isolated shell supervision/recovery. |
| `tessera-ui` | Native Slint desktop/popups, scoped input, portable presenters, asynchronous state and preference drafts; no Windows-adapter dependency. |
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

**Third-party core modified on 2026-10-08.** The published crate's 100 baseline files are retained; only `Cargo.toml`, `model/repeater.rs` and `item_tree.rs` differ. The manifest records the immutable upstream revision, archive SHA-256 and changed-file receipt. Original upstream attribution, licenses and sidecars remain intact; Tessera selects the original `GPL-3.0-only` option. See the [patch provenance and focused test recipe](docs/adr/0005-native-presentation.md#maintained-slint-core-source-patch) and [source/notice packaging requirements](docs/distribution-and-trust.md#patched-sdk-source-and-notices). The tagged alpha.11 source includes this patch without changing alpha.10; Windows composition and complete launcher parity remain uncertified.

### Inspect a Windows desktop

On Windows:

```sh
cargo run -p tessera -- inspect
```

`inspect` reports monitor bounds and work areas, visible top-level desktop-app windows, PIDs, captions, classes, rectangles, and observed window flags. On other operating systems, it reports an unsupported-platform error instead of returning a fake empty desktop.

Current development builds also accept `cargo run -p tessera -- inspect --check-surfaces`. This read-only diagnostic prints the named Tessera surfaces' physical rectangles, monitor bounds/work areas, DPI, styles and show state, and checks the toolbar against the full monitor origin/width and `round(40 * DPI / 96)` height. The Material toolbar, native AppBar reservation and inspector share that logical-height contract; older immutable neutral releases retain 32px. Missing, ambiguous, stale or incomplete toolbar data fails instead of producing a vacuous pass. Captions are diagnostic hints, never authorization to control a window.

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

System/light/dark, compact spacing and dock edge preview independently from persistence. The General week-start selector is a draft; Calendar adopts it only after successful explicit Save. **In shipped alpha.16, Save preferences** atomically stores one complete schema-4 record in `%LOCALAPPDATA%\Tessera\settings.json`: appearance, dock pins, launcher mode/ordered Favorites, General week start and optional dock-media enable. Pin/Favorite/reorder/mode/media saves preserve applied General/appearance rather than an unsaved draft. No captions, HWNDs, PIDs or sign-in commands are stored. Launch targets resolve only through the trusted current catalog, never directly from a preferences string.

The primary-monitor toolbar reserves its top work-area strip; the dock floats, defaults to OnOverlap and supports delayed edge reveal/hide. Only validated Tessera-owned windows receive styling; visibility uses uncapped eligible-window bounds and conservative unknown focus/touch facts without widening activation eligibility. Ordinary exit restores the captured primary taskbar visibility/auto-hide state without changing Winlogon or killing Explorer; Windows appbar auto-hide state can affect secondary taskbars. Frozen alpha.16 adds no periodic desktop polling, fixed-frame-rate idle rendering, injection, global keyboard hook, telemetry, update process or plugin execution. Alpha.17 implements a separate keyboard-hook/shortcut layer; its completed source gates do not certify native effects (see the [source/candidate receipt](#alpha17-development-source-receipt)). The passive mouse watcher observes movement without consuming input and remains while bars are hidden. Supervised sessions use two-second UI heartbeats; the clock updates at most once per minute. Diagnostic startup skips native desktop attachments/focus and pointer watching. Executables embed `asInvoker`, `uiAccess=false` and PerMonitorV2; unsigned builds are not antivirus-approved.

### Frozen alpha.16 capability scope — not alpha.17 source verification

- **Launcher:** native Favorites/All and retained search expose the full matching catalog, with no launcher 64-result cap; native discovery remains bounded to 1,024 entries before deduplication. Existing lazy seven-column `ListView`, Favorites reorder/whole-tile feedback and saved Expand/Contract remain. Windowed is centered at 55% of each monitor dimension, capped at 1,200 logical pixels; Fullscreen fills the primary monitor as an activatable overlay, not exclusive fullscreen. Scoped **Pin/Unpin** changes launcher Favorites through the existing one-save path, not dock pins; query/view/catalog replacement retires its input.
- **User/Calendar/General:** real shell identity with generic profile artwork, seven trusted OS known-folder actions and an OS-local Gregorian Calendar. General offers saved Monday/Sunday/Saturday week start, default Monday. Browsing does not change OS dates/locale. Full account/photo, application-group folders and date/formatter parity remain open.
- **Dock utilities:** genuine Shell Show desktop and aggregate Recycle Bin read/Open/watch, with separate confirmed Empty and read-only Retry. Empty is destructive and needs deliberately disposable contents; OS confirmation/progress remain unsuppressed, not guaranteed visible. Generic licensed Trash artwork is identical in every state; exact reference Empty/Full rights remain blocked.
- **Audio/network/Bluetooth/input:** real default-multimedia output/input volume/mute with endpoint revalidation/readback; OS-cached WLAN Refresh and real Settings, without Connect/scan/hotspot; passive paired Classic/LE Bluetooth/radio observations, without Pair/toggle. The TSF selector changes the actual desktop-session profile on explicit input; full IME behavior remains incomplete.
- **Media:** optional, saved and default off. Real GSMTC current-session transport and artwork readback use the source-sized 136×40 logical tile at all four edges (compact ×.8). Clicking supported transport controls the actual current player; missing sessions/artwork remain honest, not sample data.
- **Power/display:** six genuine direct actions, real user/status, a captured optional update-install choice only for pending Power off/Reboot, stable display selection and live display/text-scale watch. The update diagnostic reads only two known HKLM keys, not all Windows Update state. Hidden Power presentation actually retires after 30 seconds; the domain preserves accepted work without replay. Suspend deliberately has an empty decorative image while the exact BiMoon grant is unestablished.
- **Visibility:** passive actual mouse watch, OnOverlap and 100ms/800ms default reveal/hide delays use genuine uncapped window bounds. Unknown touch/focus/watch facts protect visibility; touch hotplug, native classification differences and mixed-monitor behavior remain validation gaps.

Schema **4** requires General and Dock groups alongside grouped launcher preferences. Valid schema 1/2/3 loads migrate only in memory with Monday week start and media off; no original bytes change until an explicit successful save. Schema 1 uses empty Favorites, schema 2 preserves order, and schema 3 preserves mode/order. The complete record retains the 16-KiB budget including newline and atomic replacement. Invalid/future/damaged startup files remain untouched and disable ordinary saves for that session; back up/rename/restart to recover. Back up before downgrading: old readers may replace unsupported fields on Save. No automatic backup or multi-instance merge is provided.

The [tester guide](docs/alpha-testing.md) targets published alpha.20 with eight focused safe checks, opt-in effect boundaries and recovery steps. The [native presentation decision](docs/adr/0005-native-presentation.md#next-alpha-native-capability-checkpoint) records current boundaries and historical tracer receipts. Earlier milestone counts and alpha.19/alpha.20 implementation gates are **historical**, not verification of private alpha.21 display work.

### Verification and limitations

#### Alpha.17 development-source receipt

**Committed public implementation checkpoint:** [`517f5c29f8f40d9c1be72c23fb12bc87ee3ad474`](https://github.com/redstone-md/tessera/commit/517f5c29f8f40d9c1be72c23fb12bc87ee3ad474) is the exact source for the completed gates below. It delivers global-shortcut/keyboard-hook implementation, profile/photo presentation, shared-image decoding, AtPoint placement and schema-5 preferences; implementation is not full native 1:1 certification.

- **Completed local gates:** Rust 1.92 and 1.99 Linux full workspace each **1,301 passed across 19 suites, one ignored, zero failed** (UI 703 passed/one ignored; system 72, Windows library 456, app 39). The actual exact tiny RTL test passed. All **four strict workspace/all-target Clippy** and **four warnings-denied workspace rustdoc** gates passed for Rust **1.92/1.99 × Linux/MSVC**; the documentation repair wraps two native-shortcut hyperlinks, without lint suppression. **35 notice** and **101 `-RuntimeOnly` deployment assertions**, formatting and diff checks passed. Genuine isolated Linux Mesa GL passed at **1× (9.79s) and 2× (14.68s)** with **58 P6 PPM exports per scale, 116 total**; these are native/source-renderer scale checks, not Windows DWM/compositor captures. Cross-target MSVC gates are not Windows execution.
- **Completed Windows diagnostic gate:** [Windows diagnostic CI 37913972345](https://github.com/redstone-md/tessera/actions/runs/37913972345) **passed** at the exact source checkpoint above; job `113765563179` completed in **10m6s**. Its authenticated full job log records Windows Rust 1.92 full-workspace **1,385 passed across 19 suites, zero ignored, zero failed** (UI 703, Windows library 536, system 72, app library 40). Both actual Debug production-bin builds with static CRT, mandatory **two real UI-thread pulses** and **five-second owned cleanup** passed. Five controlled in-memory WIC/shared-decoder fixtures also passed; these are not real account-photo or desktop-effects tests. Its global hooks/providers remain inert. This is neither optimized release packaging nor interactive VM/live-effects certification.
- **Published immutable alpha.17:** `release/alpha.17` and `v0.1.0-alpha.17` identify source `4c847f869f32fa2a795ce76956d9b9de0d70a44d`, annotated object `ae9efcb38c6bb514bf676864557ca47bf68c94fe`. It differs from implementation checkpoint `517f5c2` only in four static docs; implementation, versions, lockfile, scripts and CI are identical. Production CI 37916496887, optimized packaging/publication and exhaustive downloaded-asset/source verification **passed**. Its historical asset hashes, inventory and production-job receipt remain centralized in [distribution and trust](docs/distribution-and-trust.md#published-alpha17-receipt); alpha.19 is now latest available.
- **Released settings/downgrade boundary:** schema 5 adds shortcut preferences and in-memory migration of valid older records; an explicit alpha.17 save writes schema 5. Shipped alpha.16 still reads/writes schema 4. Back up `%LOCALAPPDATA%\Tessera\settings.json` before switching versions: alpha.16 treats schema 5 as a future file, leaves it untouched and disables ordinary saves; older readers may discard newer fields through their own Save paths. There is no automatic downgrade conversion or backup.
- **Unverified native effects and parity:** genuine Windows Start delivery/suppression, key balance, native SID/profile-photo/OneDrive integration, focus and mixed-DPI behavior were **not exercised** by these gates. Full 1:1, interactive Windows VM/sign-in/recovery/security-product checks and existing artwork/legal SourceGap blockers remain open; earlier frozen release and failed-CI evidence is unchanged.

#### Alpha.18 development-source receipt

**Historical verified alpha.18 implementation source:** [`b37080b2e2c48bd40dda402deadaa21803575170`](https://github.com/redstone-md/tessera/commit/b37080b2e2c48bd40dda402deadaa21803575170) includes the source-scoped User-popup/avatar-corner **Log Out** command and its independently derived OFL-font glyph. The 28px native tile/16px glyph is at (46, 50), beside the 70px avatar at (0, 4) within the User header; these are logical source dimensions, not certified native pixels. Explicit Log Out requests immediate session logoff with **no extra Tessera confirmation**. Save work and know sign-in/recovery first. This is absent from frozen alpha.17; the completed gates below belong to this implementation, not newer alpha.19 main.

- **Completed local gates:** Rust **1.92 and 1.99 Linux**, each **1,334 workspace tests passed across 19 suites, one separately gated GL case ignored, zero failed** (UI 736 passed/one ignored; Windows library 456, system 72, app 39). All **four strict workspace/all-target Clippy** and **four warnings-denied workspace rustdoc** gates passed for **1.92/1.99 × Linux/MSVC**, with compiler-isolated documentation outputs and no source workaround or lint weakening. **35 notice** and **101 recording-only `-RuntimeOnly` deployment assertions** passed; expected negative-fixture panic traces are not GUI crashes.
- **Repaired admission/lifetime regressions:** actual failing User counter-exhaustion/teardown and eight existing Power regressions were reproduced and repaired. Final focused gates passed two minimized reentry cases, 49 Root/Power cases, 20 Power lifetime cases and 13 exhaustion cases; the old eight assertions remain unchanged. Power `show` is the sole first event pump, and successful factory caching survives stale command receipts without admitting stale work.
- **Completed owned Linux Mesa GL:** **1× (9.67s) and 2× (15.06s)** passed. All **116 P6 PPM exports (58 per scale)** were parsed with complete dimensions and byte counts. These source-renderer frames are not Windows compositor captures or native-effects/focus/mixed-DPI certification.
- **Completed Windows Debug preflight:** [NativeDebug CI 37923052607](https://github.com/redstone-md/tessera/actions/runs/37923052607) **passed** at the exact checkpoint above; job `113795357632` completed in **10m30s**. The full job log's 19 suite summaries were independently recounted: **1,418 passed, zero failed or ignored** (UI 736, Windows library 536, system 72, app library 40). Both actual Debug production-bin builds with static CRT, mandatory **two real production UI-thread pulses** and **five-second owned-GUI cleanup** passed. This hosted Windows Server 2025/build 26100 diagnostic startup and controlled native fixtures leave ordinary hooks/providers inert; it is **not optimized release packaging, a Windows 11 VM or native-effects/focus/pixel certification**. Fresh `diagnostics/native-user-logout` adds only the fourth branch to the existing diagnostic workflow; prior diagnostic branches remain unchanged.
- **Artwork/full-parity boundaries:** the licensed User glyph derives from font U+EB4F with 26 canonical commands/two contours; provenance is not pixel certification. A separately researched U+EB94 moon fails the pinned standalone geometry comparison: no approximation is shipped and Suspend remains decoratively blank. Original Bin/Remix SourceGap, interactive Windows VM, security, recovery and full-matrix/full native 1:1 gates remain open. Historical alpha.17 local/Debug counts and its actual production receipt above are separate, not reassigned to this source.
- **Published immutable alpha.18:** all **505 non-document tracked files** match the gated implementation above byte-for-byte; only four static docs differ (**509 tracked files total**), with first-party versions and all **546 dependency** records unchanged. Exact frozen source/tag identity, successful production CI, optimized runtime barrier and exhaustive three-asset/source verification remain centralized in the [published alpha.18 receipt](docs/distribution-and-trust.md#published-alpha18-receipt). Old release/tag refs remain immutable. Alpha.19 is now latest available; these completed alpha.18 receipts do not certify newer source.


#### Alpha.19 development-source receipt

**Historical committed media implementation:** [`2683c31941a37d505a534fec67965236fe8dc52f`](https://github.com/redstone-md/tessera/commit/2683c31941a37d505a534fec67965236fe8dc52f) adds an independent native signed **100ns** timeline observation and `TimelinePropertiesChanged` event, plus a current-only player card/read-only time and progress in the existing Toolbar Quick Settings popup. Optional UTC observation is not an age estimate; timeline failure preserves metadata, artwork and the three existing transports. This implementation first shipped in **published immutable alpha.19**, not alpha.18, and is retained by alpha.20; its completed gates do not certify newer private alpha.21 display work.

- **Shared authority/lifetime:** Toolbar popup and optional Dock use one existing MediaHost/provider/watch/flight/mailbox, not parallel media engines. With saved Dock media off, an actually admitted/shown popup observes without saving or allocating Dock slots; zero-to-one demand starts only on that show or explicit saved enable. Accepted late work remains tracked without overlap/replay. Checked opaque `PopupMediaToken`, durable weak-Root predicate and current Toolbar geometry context reject stale input; exact popup retirement cannot replace another Root scope, and rejected finite-coordinate overflow preserves the previous popup scope.
- **Presentation boundary:** source-sized 40px cover/6px corners reuse a pure shared image mask derived from the old avatar loop; 512px avatar behavior remains unchanged. The app icon is a separate 1rem sibling, raw Dock presentation stays unchanged and no new decoder is added. Actual missing-controls failures from the old viewport were repaired through guarded batch content-fit with the existing deferred fallback, retaining genuine accessibility/pointer/key paths rather than fake callbacks or weakened guards.
- **Completed local gates:** Rust **1.92.0 and 1.99.0 Linux**, each **1,374 workspace tests passed across 19 suites, one separately gated GL case ignored, zero failed**. All **four strict workspace/all-target Clippy** and **four warnings-denied isolated rustdoc** gates passed for **1.92/1.99 × Linux/MSVC**. **35 notice** and **101 recording-only `-RuntimeOnly` deployment assertions** passed; expected negative-fixture traces are not native GUI effects. Focused gates passed **9 Root, 38 Quick Settings, 31 Dock Media, two mask and 16 profile cases**.
- **Completed owned Linux Mesa GL:** **1× (10.07s) and 2× (15.39s)** passed; all **116 P6 exports (58 per scale)** were parsed with complete headers, dimensions and RGB byte counts (1× **110,704,006 bytes**, 2× **183,611,354 bytes**). Existing fixture exports do not certify the integrated current-player rounded cover, Windows VM pixels, focus/DWM or native media effects.
- **Completed Windows Debug preflight:** [NativeDebug CI 37936511655](https://github.com/redstone-md/tessera/actions/runs/37936511655) **passed** at exact trigger-only source [`e4521e841170c904706612aeafab801af6fd4c71`](https://github.com/redstone-md/tessera/commit/e4521e841170c904706612aeafab801af6fd4c71); job `113839796636` completed in **10m58s**. The completed 2,392-line raw job log's **19 suite summaries** were independently recounted: **1,462 passed, zero failed or ignored** (UI 770, Windows library 543, system 75, app library 40). Actual Rust 1.92/static-CRT Debug production-bin startup passed **two real production UI-thread pulses and owned-GUI cleanup**. The hosted Windows Server 2025/build 26100 diagnostic and controlled fixtures are **not Windows 11 VM, actual media-effects/focus/pixel or optimized-release certification**; no assets were published. Fresh `diagnostics/native-media-popup` leaves prior diagnostic/release refs unchanged. Local **1,374** and Windows Debug **1,462** totals are separate receipts, not production counts.
- **Historical alpha.19 media/1:1 boundary:** this published current-only tracer has no seek slider or interpolated progress. Source-bounded 200ms leading/trailing seek input, captured track revision, fresh bounds/readback, all sessions, devices and mixer were not waived; separately gated seek source is recorded below and published in alpha.20, not added to frozen alpha.19. Published alpha.18 local **1,334**/Windows Debug **1,418** counts and its production assets remain separate historical receipts. Windows 11 VM/security/recovery/native effects/pixels and the complete reference matrix remain open.
- **Published immutable alpha.19:** all **507 non-document tracked files** match the Windows-Debug-verified checkpoint above byte-for-byte; only four static docs differ (**511 tracked files total**). First-party alpha.19 versions, **546 dependency** records, licensed glyph bytes and full reference-target columns remain unchanged. Exact source/tag identities, successful production CI, optimized runtime barrier and exhaustive three-asset/source verification are centralized in the [actual publication receipt](docs/distribution-and-trust.md#published-alpha19-receipt). Frozen tagged docs remain immutable prepublication snapshots. **Alpha.19 was latest available at publication**; alpha.20 is now latest, not native/full-parity certification.
- **Historical alpha.20 metadata:** commit `22d847b4fef97792dce073cabe1a3b8df5ce1698` changes six version strings: root manifest plus five first-party lockfile records. Offline locked no-dependency metadata verified all five packages and unchanged **546 dependency** pins/bytes. Metadata verification alone is not feature/native/production proof; seek implementation has its own historical receipt below and never changes frozen alpha.19. **Main alpha.21 metadata** at `6e4e126` separately changes those six version strings; manifest/lockfile parsing and source-byte comparison confirmed that metadata-only change, but offline Cargo metadata and display-work gates are not claimed complete while display work is active.

#### Alpha.20 private seek-source receipt

**Historical verified seek source, now included in published alpha.20:** [`116373e88583c558df1a2d2d418b720ae6be4c6e`](https://github.com/redstone-md/tessera/commit/116373e88583c558df1a2d2d418b720ae6be4c6e) adds scoped current-player seeking to the existing Toolbar Quick Settings player. Frozen alpha.19 has no seek slider. Its exact native-preflight checkpoint is [`96d629b237a85aaadf0ceb8b85d518beb8a42687`](https://github.com/redstone-md/tessera/commit/96d629b237a85aaadf0ceb8b85d518beb8a42687): functional source is unchanged, with only two workflows and five maintained static documents changed after the gated implementation. The [subsequent audio-admission repair](#alpha20-audio-admission-source-receipt) is not certified by this checkpoint; the [alpha.20 publication receipt](docs/distribution-and-trust.md#published-alpha20-receipt) separately records optimized packaging and actual downloads. The historical heading/anchor is retained for existing links.

- **One authority/actor:** typed transport/seek requests reuse the existing bounded MediaHost/provider/watch/mailbox and single accepted flight. Opaque observation revisions include callback-time revocation; the native owner freshly checks current-session identity, seek support and exact signed range before dispatch. False native `Try*` results reject; accepted work is not automatically retried or replayed. Read-time watch-rebind failure retires readiness independently of metadata/timeline/transport and recovers only on an explicit subsequent read.
- **Genuine input:** the shared standard Slider uses writable two-way local state with silent observed-value projection, not synthetic input callbacks. Pointer, keyboard and accessibility ONINPUT use source-bounded **200ms leading/latest-trailing** submission with exact popup/Root/session/track and measured frame/clip authority. Popup/session/Root retirement, native release/touch cancellation and actual geometry changes cancel only unsubmitted intent; old buffered moves, deferred releases and same-frame queued layout notices cannot acquire replacement authority or cancel valid trailing work. Saved Dock behavior stays unchanged, with no Dock seek or implicit preference write.
- **Completed final local gates:** Rust **1.92.0 and 1.99.0 Linux**, each **1,435 passed across 19 suites, zero failed, one separately gated GL case ignored**; the original full log's 38 summaries were independently recounted. All **four strict workspace/all-target Clippy** and **four warnings-denied rustdoc** combinations passed for **1.92/1.99 × Linux/MSVC**. Generated documentation/search indices were isolated before and between compiler versions, without source workarounds or lint suppression.
- **Focused verification:** **65 backend media**, **40 UI seek**, **11 portable media DTO** and two actual measured popup/layout-change cases passed. The genuine Root 200ms accessibility failure was reproduced and fixed with its original ordering/assertions retained. A queue-pressure fixture now acknowledges the existing FIFO owner queue before its next read, not sleep/retry; **100/100 repetitions passed**, preserving immediate queue-full rejection/no transferred completion and 1,000 nonblocking callback-union emissions. Separately, fixture-only shortcut retirement synchronization (`6fe9d0d`) passed a deterministic red→green and **100/100** repetitions without changing shortcut production code.
- **Owned Linux GL and scripts:** actual Mesa **1× (9.77s) and 2× (14.75s)** passed; all **116 P6 exports (58 per scale)** have complete headers, dimensions and RGB payloads (**110,704,006 / 183,611,354 bytes**). **35 notice** and **101 recording-only deployment assertions** passed. These existing renderer/recording fixtures neither certify integrated native seek pixels nor execute player, keyboard, audio or Power effects.
- **Completed actual source-only native matrix:** [CI 37955069678](https://github.com/redstone-md/tessera/actions/runs/37955069678) passed at exact **96d629b**. Windows **1.92.0** job `113903392547` (**14m44s**) and **stable** job `113903392632` (**9m29s**) each passed **1,524 tests across 19 suites, zero failed or ignored**; Linux stable job `113903392274` (**6m29s**) passed **1,435/19 suites, zero failed, one separately gated GL case ignored**. All **57 raw suite summaries** were independently recounted from authenticated completed-job logs; unchanged formatting, strict Clippy, warnings-denied docs, layout demo, Windows read-only inspection and isolated deployment gates passed. Packaging job `113909577137` was **skipped with zero steps**, proving this diagnostic branch did not package/publish assets.
- **Completed actual Windows Debug runtime:** [NativeDebug CI 37955069662](https://github.com/redstone-md/tessera/actions/runs/37955069662), job `113903393483`, passed at the same **96d629b** in **7m13s**. Its complete **2,453-line** job log independently confirms **1,524/19 suites, zero failed or ignored**, native SDK/controlled fixtures, actual Rust 1.92/static-CRT Debug production GUI/supervisor linking and **two real production UI-thread pulses with owned-GUI cleanup**. This is hosted diagnostic startup, not optimized release, Windows 11 VM pixels/focus/accessibility or actual player-seek effects. No release assets were published; older diagnostic/release refs remain unchanged.
- **Remaining layers:** retain the complete reference matrix. The separately [verified audio/media source-admission repair](#alpha20-audio-admission-source-receipt) is outside **96d629b**; media attachment alone is not proof for audio. Next are **250ms receipt-local interpolation/absolute labels**, all-session inventory, all active input/output devices, explicit default roles and per-output session mixer. Repaired-source native preflight passed at **746f472**, and alpha.20 optimized publication/download verification has its [separate receipt](docs/distribution-and-trust.md#published-alpha20-receipt), without certifying subsequent main alpha.21 display work. Interactive Windows 11 effects/focus/accessibility/mixed-DPI/recovery/security and rights-blocked artwork remain distinct open gates.

#### Alpha.20 audio-admission source receipt

**Historical independently verified audio source, now included in published alpha.20:** [`245da5bbdb348fb5da8af13a01ce42633819c5c1`](https://github.com/redstone-md/tessera/commit/245da5bbdb348fb5da8af13a01ce42633819c5c1) fixes stale audio input in Toolbar Quick Settings. Earlier native seek checkpoint **96d629b** does not certify this repair. Its own exact native-preflight checkpoint [`746f4727cb533f46a09fec4a3c4eba5d32271d40`](https://github.com/redstone-md/tessera/commit/746f4727cb533f46a09fec4a3c4eba5d32271d40) passed the full matrix and Debug runtime below; functional source is unchanged from **245da5b**, with only two workflows and five maintained static docs changed. Alpha.20 is now latest available; optimized packaging/publication and actual download evidence are centralized in the [separate publication receipt](docs/distribution-and-trust.md#published-alpha20-receipt).

- **Reproduced, not speculative:** four original recording/genuine-input Root fixtures first failed on actual stale **100ms volume**, mute, post-factory watch/read and accepted-completion **90% trailing replay**. They then passed with the original source/command/one-flight/no-replay ordering and assertions retained. The final fresh 30% display assertion uses the existing **0.001 f32 presentation tolerance**, not weakened command/authority comparisons; tests execute no real audio/player/Power/keyboard effect.
- **One retained source boundary:** Root presents and establishes exclusive-popup admission before installing the existing durable weak Root/Toolbar predicate, independently of media availability and before lazy audio acquisition. Private awaiting/standalone/scoped/retired admission is shared with media through a weak wrapper. Capture, 100ms submission, Refresh, factory/watch/read/execute and completion projection check exact captured presentation outside `RefCell` borrows, including post-call reentry checks.
- **Retirement, not replay:** observed source rejection latches retirement, removes unsubmitted intent/current confirmation, invalidates watch delivery and releases old leases outside borrows while leaving the actual popup open. Accepted-flight completion ownership remains until terminal; reopened presentation waits without overlap, and an old completion cannot confirm new controls or replay pending volume. Existing provider caching, endpoint IDs, generation/watch epoch, mailbox and single-flight pump remain; no new provider, executor, dependency or authority counter is added.
- **Completed final local gates:** Rust **1.92.0 and 1.99.0 Linux**, each **1,439 passed across 19 suites, zero failed, one separately gated GL case ignored**; all 38 original summaries were independently recounted. All **four strict workspace/all-target Clippy** and **four warnings-denied compiler-isolated rustdoc** combinations passed for **1.92/1.99 × Linux/MSVC**. Focused **four Root audio**, **53 Quick Settings** and **18 Root current-player** cases passed. Independent source review found no concrete P1/P2, without implying runtime certification.
- **Completed actual repaired-source native matrix:** [CI 37961265825](https://github.com/redstone-md/tessera/actions/runs/37961265825) passed at exact **746f472**. Windows **1.92.0** job `113924397102` (**14m51s**) and **stable** job `113924397229` (**12m47s**) each passed **1,528 tests across 19 suites, zero failed or ignored**; Linux stable job `113924396811` (**6m44s**) passed **1,439/19 suites, zero failed, one separately gated GL case ignored**. All **57 original suite summaries** were independently recounted from authenticated complete job logs. Unchanged formatting, strict Clippy, warnings-denied docs, layout demo and Windows read-only inspection/isolated deployment gates passed. Packaging job `113930367890` was **skipped with zero steps**; this source-only run published no assets.
- **Completed actual repaired-source Windows Debug runtime:** [NativeDebug CI 37961265878](https://github.com/redstone-md/tessera/actions/runs/37961265878), job `113924397044`, passed at the same **746f472** in **8m37s**. Its complete **2,457-line** raw log independently confirms **1,528/19 suites, zero failed or ignored**, actual Rust 1.92/static-CRT production GUI/supervisor linking and **two real production UI-thread pulses with owned-GUI cleanup**. Hosted diagnostic startup is not optimized packaging, Windows 11 VM pixels/focus/accessibility or actual audio/player effects. Subsequent timeline interpolation code is excluded from this immutable checkpoint and these receipts.
- **Owned GL/remaining risk:** actual Mesa **1× (9.78s) and 2× (15.61s)** passed; all **116 P6 exports (58/scale)** have complete headers/dimensions/RGB payloads (**110,704,006 / 183,611,354 bytes**). Renderer fixtures are not integrated native audio/seek pixels. Source-callback, subscription-return/watch-drop and execute-time hide/reopen reentry deserve further focused native/recording validation; actual effects, Windows 11 VM/DPI/focus/accessibility/recovery/security, full media inventories/settings and artwork rights remain open.

**Historical alpha.15 integrated local receipt — not final alpha.16 or development-source verification:** **1,120 workspace tests passed across 19 suites, with one separately gated GL test ignored in that run**. Strict Rust **1.92 and 1.99 Linux/MSVC workspace all-target checks** passed with warnings denied, alongside Rust 1.92 warnings-denied docs (seven outputs), **35 notice assertions**, **101 `-RuntimeOnly` deployment assertions**, formatting and diff checks. Genuine native Linux Mesa GL passed at **1× (9.56s) and 2× (14.84s)**, exporting **58 PPM frame families per scale, 116 total**. These are source-renderer frames on a neutral-gray synthetic backdrop, **not PNGs or Windows compositor captures**. MSVC checks are cross-target evidence, not execution of the full Windows test matrix. Windows release CI, frozen source SHA/tag/assets, actual native Windows font/geometry/effect validation and the full acceptance matrix remain pending.


`Cargo.lock` is committed. **CI** runs for pushed `v*` tags and the exact source-only `diagnostics/native-media-seek`, `diagnostics/native-audio-admission` and `diagnostics/native-visual-parity` branches; ordinary branch commits/PRs do not start its Linux stable/Windows stable/Windows Rust 1.92 matrix. Packaging/publication explicitly requires a tag ref plus an alpha tag name, never a diagnostic branch. The separate **NativeDebug** branch smoke diagnoses actual GUI startup and publishes no release assets. Run the local checks above before pushing changes. Headless UI tests cover refresh/error states; controlled Windows tests read another process's fixture caption/unchanged geometry, callback panic handling and DPI restoration. These are not full native-session certification.

**Alpha.20 is latest available and independently verified:** production CI, optimized runtime verification and all three actual downloaded-asset hash/package/source checks passed; see the [published receipt](docs/distribution-and-trust.md#published-alpha20-receipt). Historical seek and independently repaired audio-admission source proofs above are included in alpha.20, not retroactively in alpha.19. Alpha.21 development work is outside these receipts; parked media-display changes have no completed gates. Native effects and full native parity remain uncertified.

**Alpha.21 visual development:** source-backed toolbar/dock geometry and paint,
native Settings shell, and launcher search/footer/empty states are in main.
Both shell and User/Calendar checkpoints passed actual Linux/Windows CI and
native Debug GUI two-pulse/owned-cleanup gates. Keyboard/Network/Bluetooth's
repaired selector checkpoint `8dbd385` also passed exact-source Windows
1.92/stable (1,537 tests each) and Debug GUI; the earlier `384e005` remains failed.
The user's [Material 3 / end4-pC video reference](https://www.youtube.com/watch?v=fcK0vem1RtI)
sets the appearance direction; Seelen remains the functional contract.
The isolated merged Material/selector candidate passed 1,448 tests/19 suites,
strict Linux/MSVC checks, rustdoc, 35 notices and both actual GL scenarios at
1×/2× (180 complete frames). Its exact merged checkpoint `8f7844b` then passed
[native CI](https://github.com/redstone-md/tessera/actions/runs/38005383181)
(Windows 1.92/stable: 1,537 tests each) and
[Debug GUI](https://github.com/redstone-md/tessera/actions/runs/38005383217)
(two actual UI pulses and owned cleanup). Complete logs were authenticated/
recounted; packaging was skipped. Those gates cover the earlier static-green
layer. The subsequent RGB/HCT/schema-6 checkpoint `4f28b52` passed its own
[CI](https://github.com/redstone-md/tessera/actions/runs/38009841918)
(Windows 1.92/stable: 1,543 tests each; Linux: 1,454) and
[Debug GUI](https://github.com/redstone-md/tessera/actions/runs/38009841951)
(two actual UI pulses and owned cleanup), with complete logs authenticated/
recounted and packaging skipped. Presets reuse explicit preview/Save/Cancel and
complete-record storage. A later test-only correction captures settled stock
control colors; it changes no production palette or controls. Ordinary-session
effects remain uncertified. See the
[exact source/gate boundaries and remaining gaps](docs/adr/0005-native-presentation.md#alpha21-source-visual-layer-development);
this is not a published release, complete 1:1 parity or a fix claim for the VM toolbar offset.

Material windows embed the unmodified OFL-1.1 Google Sans Flex 5.000 font and
share its family through the existing captured theme, including lazy popups
and Power recreation. Original licenses, trademark text and immutable hashes
are retained in `crates/tessera-ui/assets/fonts/google-sans-flex`.
Neutral reference fixtures retain the system font and old typography. Material
body/title tokens use 15px/450 and 22px/550 through the same captured theme.
Missing Cyrillic/Arabic/Hebrew glyphs rely on the SDK's platform fallback, not
a claim of font coverage. This family choice does not certify the video's
exact font binary, custom variable axes or Windows rendering.

Material Suspend uses the unmodified Apache-2.0 Google Material `bedtime` SVG
from the existing pinned icon family, with its original source hash and license.
This is an independent Material glyph, not the rights-blocked Seelen BiMoon;
neutral reference mode still keeps its own supplied or empty decoration.

Development Dock groups only OS-observed application identities: an explicit
per-window AppID takes precedence over process AppID/executable, including apps
sharing a host process. Full member counters and an owned selectable window
menu remain. Running pins match only exact AUMID; executable grouping never
becomes a launch command. Missing identity keeps separate windows. The selected
member has a real, scoped DWM thumbnail; protected/unavailable previews remain honest and
do not disable window actions.

Pinned applications support native drag to measured Dock slots and discrete
Move earlier/later menu actions. The existing complete-record saver commits
order only on success; cancellation clears passive feedback without replay.
Scrolling/layout changes cancel drag, and feedback stays in the owned Dock
window rather than a new desktop overlay. Full monitor/action preferences and
Windows drag/runtime outcomes still require completion and verification.

Development Quick Settings adds active input/output devices, selected-device
volume/mute, each output's app/System Sounds mixer and independent default-role
observations. All mutations capture complete endpoint/session incarnations and
return native readback through the existing audio owner. Multimedia→Console
results stay independent; Communications is separate. Default-role writes use
an isolated undocumented Windows PolicyConfig adapter and can be unavailable.
Sound Settings dispatch is fixed and source-scoped. These development controls
are not native Windows outcome certification or a published release.

Development media enumerates all GSMTC sessions through the existing owner and
shared card. Follow-current tracks Windows; choosing a player changes only
Tessera's target, not the OS default. Removed players stay unavailable instead
of redirecting commands. Canonical COM incarnations, per-source readiness and
fresh native checks protect transport/seek; selector input remains scoped to
the current popup and visible frame. Toolbar observation still works with
saved Dock media off, without an implicit Save. Native effects and the separate
parked timeline-interpolation work are not certified by this source layer.

Development Wi-Fi adds explicit Connect/Disconnect for exact native
interface/network observations: supported saved profiles, open networks and
WPA2-Personal AES temporary profiles. Credentials are scoped, cleared and never
saved by Tessera; WPA/WPA3/enterprise/hidden or ambiguous sources remain
unavailable. Native acceptance is separate from association/readback; after
20 seconds without a conclusive result it stays unconfirmed, never replayed.
No scan or hotspot is added. Windows cached queries may prompt for
location consent or deny access under Privacy & security > Location.
Software radio On/Off targets exact enumerated interfaces and native PHY IDs,
independently of location-denied network discovery. Hardware switches remain
read-only; each PHY records actual acceptance/error and software/hardware
readback. Partial results never trigger rollback, replay or fake all-success.
Saved profiles remain observable independently of denied discovery. Explicit
Forget requires current inline confirmation and fresh exact GUID/native-profile
identity, descriptor and policy checks; readback distinguishes absent, present,
replaced and unavailable. No plaintext keys or fake successful removal.

Development Bluetooth adds explicit On/Off for each available Bluetooth
radio, with exact native device IDs, retained COM incarnations and source/state
invalidation. Consent begins only from deliberate GUI input; native acceptance
does not fabricate the requested state. Refresh reads a later transition.
Legacy paired-device reads keep their scoped resource lifetime; opt-in controls
retain one owned radio/event apartment without idle polling. Discovery, pairing
and device connection are not implemented.

Development battery adds actual cached presence, charging/supply, percent,
remaining-runtime and energy-saver facts through one event-driven native owner
shared by Toolbar and popup. Unknown/error stays visible; only confirmed
absence or unsupported capability hides the indicator. Refresh does not poll,
and Power & battery Settings acknowledges SDK initiation, not a visible window.
These development layers need actual Windows outcome verification and are
outside the published release.

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

- [Seelen UI](https://seelen.io/apps/seelen-ui/customizable-shell): the functional reference and original geometry baseline for the customizable shell. The user's [Material 3 / end4-pC video](https://www.youtube.com/watch?v=fcK0vem1RtI) sets the current appearance target; both boundaries are recorded in the [presentation decision](docs/adr/0005-native-presentation.md). Seelen's [upstream README](https://github.com/eythaann/Seelen-UI) documents a required WebView runtime; Tessera independently implements native presentation without copying its AGPL source or artwork.
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
