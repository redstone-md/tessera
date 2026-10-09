# Tessera

A modular, native desktop environment for Windows 11, built in Rust. Tessera focuses on a deeply customizable shell with familiar floating-window behavior, replaceable modules, and restricted third-party plugins. Tiling is optional, not the default workflow.

## Status

**Public, unsigned native desktop test alpha; not a daily-driver shell.** **Alpha.17 is published, with production CI and all downloaded assets independently verified.** [Download the latest Windows x64 test ZIP](https://github.com/redstone-md/tessera/releases/download/v0.1.0-alpha.17/tessera-0.1.0-alpha.17-windows-x64.zip) and use its shipped `START-HERE.txt`; see the [published receipt](docs/distribution-and-trust.md#published-alpha17-receipt). Alpha.16/alpha.14 remain unchanged. Main has advanced to **alpha.18 development**, not yet whole-workspace verified and not included in alpha.17; the [implementation/source boundary](#alpha17-development-source-receipt) separates these receipts. This is not full 1:1 or interactive Windows certification.

**Full-wave source Power actions are real and direct:** Lock, Log out, Power off, Reboot, Suspend and Hibernate have no extra Tessera confirmation. Save work and select each only deliberately in a safe session; never “click all” as a smoke test. Update hints/accepted requests do not prove completed shutdown or updates.

The immutable alpha.10 (`4866d3a`, [CI receipt](https://github.com/redstone-md/tessera/actions/runs/37675695227)) introduced separate dock context menus, validated window commands and read-only geometry checks after alpha.9's visual/session redesign. Alpha.8/.9/.10 packages remain unchanged; their receipts do not certify the new candidate. Work toward the complete native Seelen contract continues after the tester release.

**Separate alpha.11 tag — CI failed, no release assets:** `release/alpha.11`, full source `363fe510a0540ed7c984345a2edd3692266dcfe8`, is verified `df36b2f` plus four metadata/docs-only commits. Its local rerun passed **649 tests across 19 suites**, and `v0.1.0-alpha.11` was verified/pushed at that exact peeled SHA. [CI 37878846670](https://github.com/redstone-md/tessera/actions/runs/37878846670) **failed** on stable Rust 1.99 atomic/chunks lints and two Windows Rust 1.92 font assumptions (395 UI tests passed). Packaging did not run; no public alpha.11 assets exist. The failed tag stays immutable, never retagged.

**Baseline-only alpha.12 — code checks green, packaging failed:** branch `release/alpha.12`, source `7531f04d4270fd30403c500b099ddeaed8473d2f`, immutable `v0.1.0-alpha.12`. Local gates passed 649 tests/19 suites (397 UI/188 Windows-adapter pure), strict Rust 1.92/1.99 Linux/MSVC checks, warnings-denied docs, 35 notices and format/diff. [CI 37881361921](https://github.com/redstone-md/tessera/actions/runs/37881361921) is **terminal failed**: all Linux/Windows stable/Windows Rust 1.92 code jobs passed tests, strict lint/docs, demo, read-only inspect and deployment fixtures, but packaging's runtime supervisor exited **1**. GUI stderr was discarded and the numerical GUI exit was not retained; the cause is open, not proven “policy”. **No ZIP/checksums, public release or artifacts were produced.** The tag stays immutable.

**Baseline-only alpha.13 — code green, runtime packaging failed:** remote `release/alpha.13` and immutable `v0.1.0-alpha.13` remain at `6b31b64efd9cdc92f5edb7cffa0e64ed2b121a57`. Runtime `28b9987` plus metadata `6b31b64` added bounded stderr and an inert synthetic-context/default-preferences host while retaining the actual production GUI/two-heartbeat barrier. Local 660-test/19-suite receipt (UI 397 plus one ignored, app 16, Windows pure 196), strict Rust 1.92/1.99 Linux/MSVC/docs/35 notices/format-diff and focused 44-assertion `-RuntimeOnly` deployment checks passed. Full registry fixtures require Windows. [CI 37885358935](https://github.com/redstone-md/tessera/actions/runs/37885358935) **finished failed only at packaging**: all three code jobs, including full isolated Windows deployment fixtures, passed. No release assets; baseline first Lock excludes alpha.15's full-six/modules/General V4/live display.

Alpha.13 exited `0x00000001` before its first heartbeat, with successful owned-process cleanup; Rust captured 106 stderr bytes that PowerShell then discarded. Later source diagnosis reproduced duplicate Winit backend selection and the forbidden second event-loop creation. A single UI backend owner/passive first show corrected it: source `02fafe2112ce8faf0cd9f30d61c478e4ac3c8abd`, [NativeDebug 37888007340](https://github.com/redstone-md/tessera/actions/runs/37888007340), **passed in 3m38s with two real production UI-thread heartbeats and five-second owned-GUI cleanup**. The matching SDK error size is consistent with alpha.13, not recovery of its discarded text. This proves native debug startup, not release packaging, normal-session effects or a policy/renderer bypass.

**Published immutable baseline alpha.14:** `release/alpha.14` and `v0.1.0-alpha.14` identify source `e95dbb1e49e04a8e63d81fee6bc6f76274a36cfc`, atop corrected runtime source `02fafe2112ce8faf0cd9f30d61c478e4ac3c8abd`. Local gates passed **664 tests/19 suites** (UI 397 plus one ignored, Windows pure 196, app 20), strict Rust 1.92/1.99 Linux/MSVC all-target checks, Rust 1.92 warnings-denied docs, 35 notices, 101 `-RuntimeOnly` assertions and format/diff. [Production release CI 37889391783](https://github.com/redstone-md/tessera/actions/runs/37889391783) **passed all three code jobs, Windows packaging and publication**, including two real production GUI UI-thread pulses and five-second owned-process cleanup. The nondraft prerelease has three downloaded, checksum-verified assets; [exact hashes and source receipt](docs/distribution-and-trust.md#published-alpha14-receipt) are recorded separately. This first-Lock/runtime/font baseline does not include alpha.15's seven native layers/six actions/General V4 or certify normal Windows sessions, source pixels or full parity.

**Immutable full-wave alpha.15 — Windows documentation failed, no assets:** `v0.1.0-alpha.15` remains at source `5a9828f49f011cbb0fa410c032c2db409c494112`. [Production CI 37897841677](https://github.com/redstone-md/tessera/actions/runs/37897841677) passed all Linux gates and both Windows Rust 1.92/stable Clippy and tests, then both Windows jobs failed warnings-denied rustdoc solely on two bare Microsoft SDK URLs in `native_power_updates/sdk.rs` lines 6–7. Packaging was skipped; no alpha.15 release assets exist. The annotated tag object is `c1aa1f8bb462f1ad3f044fe3527b9211d4138b96`; the failed tag is immutable, never deleted or retagged.

## Alpha.16 release checkpoint

**Published immutable alpha.16:** verified `release/alpha.16` and `v0.1.0-alpha.16` remain at source `34ff877576e9ed68b5b2b5d5b7a987c94fe51672` (annotated object `d2a87f44f367bdb8d32336b38adcdb787a08bd26`). [Production CI 37902905003](https://github.com/redstone-md/tessera/actions/runs/37902905003) passed all three code jobs and optimized Windows packaging, including two real production GUI UI-thread pulses and five-second owned cleanup, then published a nondraft prerelease. All three downloaded assets passed independent hash/package/source verification. **Alpha.16 remains available unchanged; alpha.17 is now latest available.** See the [download and corresponding-source receipt](docs/distribution-and-trust.md#published-alpha16-receipt).

The frozen repair retains alpha.15’s seven native layers, six Power actions and schema-4 preferences, with only two SDK documentation hyperlinks and version/release metadata changed. **Final alpha.16 local gate receipt:** **1,120 workspace tests passed across 19 suites, one separately gated GL case ignored, zero failed**. All four strict workspace/all-target Clippy checks and all four warnings-denied workspace documentation checks passed for Rust **1.92/1.99 × Linux/MSVC**. The remaining documentation pass completed in **54.09s** with generated docs isolated by toolchain; no source change or lint allowance was needed. **35 notice assertions**, **101 recording-only `-RuntimeOnly` deployment assertions**, formatting and diff checks passed. The historical **116 Linux GL PPM exports (58 at each scale)** are inherited, not rerun for alpha.16 and not Windows compositor evidence. These local/MSVC cross-target gates do not certify native runtime effects or optimized release packaging.

**Published alpha.17:** global shortcuts, profile/photo, shared-image decoding, AtPoint placement and schema-5 preferences extend frozen alpha.16. Production CI, optimized packaging and downloaded-source/assets passed; see the [published receipt](docs/distribution-and-trust.md#published-alpha17-receipt) and [tester guide](docs/alpha-testing.md). Ordinary launch defaults global shortcuts enabled; eligible bare Win and Win+K behavior remains subject to Windows/interactive validation. Back up settings before the schema-5 migration. User-popup Log Out and the newer alpha.18 font-derived glyph are not included.

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
| `tessera-core` | Pure geometry, window identity and modes, and the `MainStack` layout engine. |
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

The [tester guide](docs/alpha-testing.md) targets published alpha.17 with eight focused safe checks, opt-in effect boundaries and recovery steps. The [native presentation decision](docs/adr/0005-native-presentation.md#next-alpha-native-capability-checkpoint) records current boundaries and historical tracer receipts. Prior 649-test and earlier milestone counts are **historical**, not verification of published alpha.17 or current alpha.18 main.

### Verification and limitations

#### Alpha.17 development-source receipt

**Committed public implementation checkpoint:** [`517f5c29f8f40d9c1be72c23fb12bc87ee3ad474`](https://github.com/redstone-md/tessera/commit/517f5c29f8f40d9c1be72c23fb12bc87ee3ad474) is the exact source for the completed gates below. It delivers global-shortcut/keyboard-hook implementation, profile/photo presentation, shared-image decoding, AtPoint placement and schema-5 preferences; implementation is not full native 1:1 certification.

- **Completed local gates:** Rust 1.92 and 1.99 Linux full workspace each **1,301 passed across 19 suites, one ignored, zero failed** (UI 703 passed/one ignored; system 72, Windows library 456, app 39). The actual exact tiny RTL test passed. All **four strict workspace/all-target Clippy** and **four warnings-denied workspace rustdoc** gates passed for Rust **1.92/1.99 × Linux/MSVC**; the documentation repair wraps two native-shortcut hyperlinks, without lint suppression. **35 notice** and **101 `-RuntimeOnly` deployment assertions**, formatting and diff checks passed. Genuine isolated Linux Mesa GL passed at **1× (9.79s) and 2× (14.68s)** with **58 P6 PPM exports per scale, 116 total**; these are native/source-renderer scale checks, not Windows DWM/compositor captures. Cross-target MSVC gates are not Windows execution.
- **Completed Windows diagnostic gate:** [Windows diagnostic CI 37913972345](https://github.com/redstone-md/tessera/actions/runs/37913972345) **passed** at the exact source checkpoint above; job `113765563179` completed in **10m6s**. Its authenticated full job log records Windows Rust 1.92 full-workspace **1,385 passed across 19 suites, zero ignored, zero failed** (UI 703, Windows library 536, system 72, app library 40). Both actual Debug production-bin builds with static CRT, mandatory **two real UI-thread pulses** and **five-second owned cleanup** passed. Five controlled in-memory WIC/shared-decoder fixtures also passed; these are not real account-photo or desktop-effects tests. Its global hooks/providers remain inert. This is neither optimized release packaging nor interactive VM/live-effects certification.
- **Published immutable alpha.17:** `release/alpha.17` and `v0.1.0-alpha.17` identify source `4c847f869f32fa2a795ce76956d9b9de0d70a44d`, annotated object `ae9efcb38c6bb514bf676864557ca47bf68c94fe`. It differs from implementation checkpoint `517f5c2` only in four static docs; implementation, versions, lockfile, scripts and CI are identical. Production CI 37916496887, optimized packaging/publication and exhaustive downloaded-asset/source verification **passed**. Alpha.17 is now latest available; exact asset hashes, inventory and production-job receipt are centralized in [distribution and trust](docs/distribution-and-trust.md#published-alpha17-receipt).
- **Separate alpha.18 main development:** first-party versions advanced to alpha.18 in commit `b274aeb`. In-flight User Logout work, including a teardown-counter fix after an actual failing focused test, and the independently verified OFL-font-derived glyph are **not in frozen alpha.17** and **not yet whole-workspace verified**. A licensed font-outline provenance check is not VM pixel or native-effects certification; completed alpha.17 receipts do not certify newer main.
- **Released settings/downgrade boundary:** schema 5 adds shortcut preferences and in-memory migration of valid older records; an explicit alpha.17 save writes schema 5. Shipped alpha.16 still reads/writes schema 4. Back up `%LOCALAPPDATA%\Tessera\settings.json` before switching versions: alpha.16 treats schema 5 as a future file, leaves it untouched and disables ordinary saves; older readers may discard newer fields through their own Save paths. There is no automatic downgrade conversion or backup.
- **Unverified native effects and parity:** genuine Windows Start delivery/suppression, key balance, native SID/profile-photo/OneDrive integration, focus and mixed-DPI behavior were **not exercised** by these gates. Full 1:1, interactive Windows VM/sign-in/recovery/security-product checks and existing artwork/legal SourceGap blockers remain open; earlier frozen release and failed-CI evidence is unchanged.

**Historical alpha.15 integrated local receipt — not final alpha.16 or development-source verification:** **1,120 workspace tests passed across 19 suites, with one separately gated GL test ignored in that run**. Strict Rust **1.92 and 1.99 Linux/MSVC workspace all-target checks** passed with warnings denied, alongside Rust 1.92 warnings-denied docs (seven outputs), **35 notice assertions**, **101 `-RuntimeOnly` deployment assertions**, formatting and diff checks. Genuine native Linux Mesa GL passed at **1× (9.56s) and 2× (14.84s)**, exporting **58 PPM frame families per scale, 116 total**. These are source-renderer frames on a neutral-gray synthetic backdrop, **not PNGs or Windows compositor captures**. MSVC checks are cross-target evidence, not execution of the full Windows test matrix. Windows release CI, frozen source SHA/tag/assets, actual native Windows font/geometry/effect validation and the full acceptance matrix remain pending.


`Cargo.lock` is committed. **Release CI** runs only for pushed `v*` tags; ordinary branch commits/PRs do not start its Linux stable/Windows stable/Windows Rust 1.92 matrix. The separate **NativeDebug** branch smoke diagnoses actual GUI startup and publishes no release assets. Run the local checks above before pushing changes. Headless UI tests cover refresh/error states; controlled Windows tests read another process's fixture caption/unchanged geometry, callback panic handling and DPI restoration. These are not full native-session certification.

**Alpha.17 is published and independently verified:** production CI, optimized GUI pulses/cleanup and all three downloaded-asset hash/package/source checks passed. See the [published receipt](docs/distribution-and-trust.md#published-alpha17-receipt). Alpha.16/alpha.14 and failed tags remain unchanged. Separate [alpha.18 main development](#alpha17-development-source-receipt) is not whole-workspace verified; native effects and full native parity remain uncertified.

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
