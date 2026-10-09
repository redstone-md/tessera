# Tessera 0.1.0-alpha.11 candidate — native desktop test alpha

This is an **unsigned, experimental test build**, not a daily-driver shell or a signed consumer release. Windows 11 x64 Home/Pro is the target. A successful hosted Windows build or the earlier alpha's server smoke test does not validate Windows 11 sign-in recovery, security-product compatibility, or every display configuration.

**New in alpha.11:** full native Favorites/All retained catalog and search without a UI 64-result cap (native discovery remains bounded to 1,024 entries before deduplication), saved Expand/Contract, Favorites drag ordering and whole-tile feedback, real User known folders, native Calendar, Show Desktop, Recycle Bin read/watch/Open/native-confirmed Empty, and the first genuine Power popup with **Lock only**. This is a verified source checkpoint, not the unverified next implementation wave. Publication is gated by the new tag pipeline; no alpha.11 Windows CI/runtime pass or published binaries are claimed yet. Alpha.10 remains immutable at `4866d3a` (CI run `37675695227`), as do all earlier releases. Take a disposable Windows 11 VM snapshot; do not enable sign-in-shell replacement for the first UI test.

## Available capabilities

- Native dock, top toolbar and application menu, real installed-application catalog and best-effort Windows icons; no WebView, Tauri, or Electron.
- Search, explicit application launch, up to 32 pinned applications, and explicit window activation/restoration. Dock menus add minimize and ordinary close requests; success means a request was accepted, not that an application finished saving or closing. Windows foreground restrictions are respected.
- Live system/light/dark appearance, compact spacing, and primary-monitor dock edge previews. **Save** persists preferences; pin clicks save pins using the last saved appearance, not an unsaved preview.
- Passive out-of-context desktop notifications and coalesced background observations, with manual Refresh fallback. No low-level keyboard/mouse hooks, injection, or periodic desktop polling.
- Per-user install and independent restore scripts; an optional supervised sign-in shell with Explorer fallback.
- Separate developer CLI: `--version`, `demo`, `inspect`, `inspect --check-surfaces`, and `panel`.

This does not replace DWM, automatically tile/move application windows, provide the Windows notification area/Start flyouts, run plugins, or bypass security. The dock is floating; the 32-logical-pixel top toolbar reserves work area only when an Explorer appbar host is available. Fullscreen hiding covers both bars and is a conservative foreground geometry hint, not universal game compatibility. Taskbar visibility calls target the primary monitor, but Windows appbar auto-hide state may also affect secondary taskbars.

This is not a complete or pixel-certified 1:1 Seelen replacement. Application grouping/counters, overlap-triggered auto-hide/reveal, Windows-key toggling, tray/media modules, folders, and plugins remain absent. Native transparency depends on the VM's graphics driver; software fallback may be opaque, and native backdrop blur is not promised. Test startup focus, fullscreen return, DPI changes and competing appbars explicitly. A hard-killed supervisor cannot restore its in-memory taskbar backup.

## Download and test a temporary desktop session

1. Once the matching **test prerelease** passes its publication gate, download `tessera-0.1.0-alpha.11-windows-x64.zip` and `SHA256SUMS.txt`. Compare `Get-FileHash <zip> -Algorithm SHA256` with the checksum. A checksum detects corruption, not publisher identity.
2. Extract the entire directory and keep `Tessera.exe`, `tessera-shell.exe`, `tessera-cli.exe` and all accompanying files together. Run `Tessera.exe` as your ordinary user, without administrator privileges. The package uses a verified static CRT; system Windows DLLs are still required.
3. Leave Defender, UAC, SmartScreen, Smart App Control, and signing/execution policies enabled. **Executables and scripts are unsigned.** If protection blocks them, stop and report the exact warning. Do not disable protection, add exclusions, change execution policy, or unblock downloaded scripts. Never upload private desktop data or modified private artifacts to a scanner. A protected machine may need a signed build before testing.
4. The native dock and toolbar should appear, then the Explorer taskbar should hide. Open applications with the start tile. Right-click Start for settings/recovery/exit, a pinned tile for Open/Unpin, or a running window for Activate/Minimize/Close; middle-click a pinned tile to launch another instance. Test search, pins and window switching. Choose **Exit** or **Restore Explorer** in Tessera: the supervisor should restore the original taskbar visibility and auto-hide state. Ordinary launch never changes sign-in configuration.

## Alpha.11 checkpoint checks and limits

Settings live in `%LOCALAPPDATA%\Tessera\settings.json`. Strict schema 3 groups `launcher: { "display_mode": "windowed" | "fullscreen", "favorites": [...] }` with appearance and dock pins. Strict legacy schema 1/2 readers load without rewriting bytes and default to Windowed; schema 1 starts with empty Favorites, schema 2 preserves their exact order. Only successful explicit persistence migrates. The entire atomic record shares a 16-KiB budget including newline; oversized saves fail without truncation. Invalid/future/corrupt startup files disable ordinary saves for the session while preview remains usable. Back up and rename the file, then restart to recover. Back up before downgrading to alpha.10. No captions, HWNDs, PIDs, or shell commands are stored.

Eight focused capability checks:

1. **Inventory:** exercise Favorites, All and raw-title substring search beyond 64 results and the first viewport. Unavailable Favorites retain saved identities/order but cannot launch. View changes/reopen reset query/selection/scroll; same-query refresh retains a still-matching selection. Native seven-column rows and lazy `ListView` do not guarantee all columns in arbitrarily narrow windows or certified offscreen accessibility.
2. **Mode and drag:** Expand/Contract saves independently of unsaved appearance previews and dock pins, without resetting current query/view/selection. Windowed is centered at 55% of monitor dimensions, capped at 1,200 logical pixels each; Fullscreen is an opaque topmost primary-monitor overlay, not exclusive fullscreen or cursor-monitor targeting. Drag only resolved Favorites with empty query; changed completion saves once, preview/no-op/cancel never saves. Check cross-row/tail movement, native scrollbar, gesture-only edge autoscroll, passive whole-tile feedback, Escape/hide/refit cancellation and truthful save-failure rollback. Missing Favorites retain their saved slots.
3. **User:** the actual User footer opens its own popup with real shell name or honest unavailable state, generic profile fallback, and Recent/Desktop/Downloads/Documents/Music/Pictures/Videos. Only current Ready rows open; Retry reads, never repeats an open. On an explicitly chosen Windows machine check redirected/changed folders, pending hide/reopen, denial and STA responsiveness. Shell acceptance does not prove Explorer appeared; do not share private paths.
4. **Calendar:** the actual toolbar clock opens Month/Year, four-to-six-week grids, Previous/Next and Today. Test off-month selection, twelve-month year view, wheel UP advancing/DOWN retreating, keyboard/Escape and hide/reopen. Real local Gregorian date and locale-based names/week start come from date metadata, not clock-text parsing or persisted week-start/language settings. Browsing cannot change OS time or save preferences; Today refreshes only while visible, at most once per minute.
5. **Show Desktop:** the Dock begins Start, Show desktop, applications. Test pointer/Tab/Enter/Space, stale inventory and all edges/densities. One explicit Shell toggle has one accepted flight; held input, hide/refit and completion must not duplicate or retry it. On an explicitly chosen Windows session observe two actual hide/restore outcomes; accepted HRESULT is not visible-state proof. Automated recording tests never toggle the real desktop.
6. **Recycle Bin read/watch/Open:** one fixed trailing Bin leaves complete application models and pin capacity intact. Unknown/loading/unavailable/stale are honest; zero confirms Empty only at read time, and zero-byte items still count as Full. Test native namespace Open, context Menu/Shift+F10, duplicate suppression and independent Retry. Compare genuine aggregate state across controlled volumes/removable media; watcher registration does not certify coverage. Generic MIT Trash artwork is unchanged across states, not exact reference empty/full artwork.
7. **Empty — separately opt in:** use only an explicitly authorized disposable Windows VM with snapshot and disposable contents on **every affected volume**. Empty targets the aggregate Bin, not a selected file/drive. Native confirmation/progress are not suppressed, but Windows may omit a dialog in some conditions; inspect real owner activation, GUI responsiveness, cancellation and subsequent fresh aggregate state. A returned HRESULT proves neither consent nor deletion nor Empty. There is no automatic native deletion/retry; automated recording tests invoke no native deletes.
8. **Power/Lock — separately opt in:** opening the real Power footer performs a fresh read-only raw-monitor query, never Lock. Geometry uses the selected full primary monitor/raw-order fallback and actual SDK scale; failed/empty queries retain the prior popup. Before genuine **Lock session**, explicitly choose a safe Windows session, save work and verify the unlock method. Current pointer/Return/Space/accessibility input initiates Lock after popup retirement, **without an extra application confirmation**. Native acceptance is initiation, not completed/observed locking. Held input, hide/reopen and accepted-flight completion must never duplicate/replay/retry. Other five source Power actions are unsupported, not fake buttons.

Pre-metadata evidence for this checkpoint: **649 workspace tests across 19 suites**, strict Linux/MSVC all-target checks, warnings-denied docs, 35 packaging-notice assertions, and owned Mesa GL at genuine native/renderer 1x/2x across 46 frame families. The new-version checks and tag pipeline remain pending; Linux recordings/software/GL do not certify native Windows session effects, COM, native confirmation, focus, IME, accessibility, mixed DPI, screen-edge placement or all-volume watch coverage. The read-only geometry diagnostic still cannot resolve the reported VM toolbar offset by itself.

The full native Seelen 1:1 goal remains ongoing, unchanged and incomplete. This checkpoint excludes the next unverified wave: full six-action Power, Network, Bluetooth, TSF/input language, Media toggle, overlap visibility, General schema 4 and stable CCD/live Power topology watch. Partial rights/icon-source behavior, account/avatar enrichment, source cursor/desired-position monitor targeting, persisted Calendar locale preferences and exact source artwork/layout parity remain gaps. Existing controlled launcher layout checks are not a new full-native-runtime certification.


## Optional per-user installation

From a permitted Windows PowerShell 5.1 or PowerShell 7 session in the extracted directory:

```powershell
.\Install-Tessera.ps1 -PackagePath $PWD.Path -WhatIf
.\Install-Tessera.ps1 -PackagePath $PWD.Path
```

Installation copies the complete package to `%LOCALAPPDATA%\Programs\Tessera\0.1.0-alpha.11`, publishes the recovery script/module to `%LOCALAPPDATA%\Tessera\Recovery`, and optionally creates a normal Start-menu shortcut. It does **not** change the sign-in shell, create a Run entry, service, scheduled task, or log off the current session. Immutable version directories reject differing-content replacement; restore an active earlier shell before switching versions. Appearance data is retained.

`-WhatIf` is read-only: no file lock, copied files, shortcut, registry value, or process launch. If the script cannot run under the current execution policy, stop rather than weakening that policy.

## Experimental shell activation — disposable VM first

Keep a VM snapshot and know the emergency steps below **before signing out**. Activation is refused on unsupported Windows builds, Windows Server, domain-managed hosts, conflicting shell policies, non-Explorer shells, and enforcing or unverifiable Smart App Control for this unsigned build. Evaluation/off states still require the real runtime probe. This isolated per-user integration is experimental, not Microsoft's Enterprise-only Shell Launcher and not a promise that every Home/Pro build supports identical behavior.

```powershell
.\Install-Tessera.ps1 -PackagePath $PWD.Path -EnableShell -WhatIf
.\Install-Tessera.ps1 -PackagePath $PWD.Path -EnableShell
```

Before activation, the installer publishes independent recovery and invokes the installed supervisor's `--verify-runtime`. The real GUI must produce **two UI-thread heartbeats**, then its owned diagnostic process is stopped. A blocked executable, missing GUI, failed startup, or failed heartbeat prevents activation. The explicit confirmation defaults to **No**.

Only after these checks does it record the original per-user `Shell` presence, raw value, and type under `HKCU\Software\Tessera\ShellRecovery`, verify the backup, and set the per-user Winlogon `Shell` to the quoted supervisor path. Machine Winlogon, Userinit, UAC, and security policies are not modified. The change takes effect at the **next sign-in**; there is no automatic sign-out or killing Explorer.

The supervisor starts one GUI child and uses kernel waits on that child and its local heartbeat event. Child exit, startup failure, or a 30-second heartbeat loss triggers owned rollback and starts system Explorer. Only its own failed child can be terminated; other applications are untouched. Closing the supervised Tessera shell also restores Explorer. There is no restart loop or installed watchdog service.

**Important limit:** passing the probe proves launchability under current policy, not future policy. If Windows blocks or deletes the supervisor itself at the next sign-in, it cannot execute fallback. Keep the independent emergency recovery route; unsigned shell activation is not consumer-ready.

## Return to Explorer

For an ordinary temporary desktop session, choose **Exit** in the application menu or **Restore Explorer** in settings. The independent supervisor restores the captured taskbar state after the owned GUI exits. If the GUI is unresponsive, Task Manager can stop **Tessera.exe only**; leave `tessera-shell.exe` running so it can restore the taskbar. Do not deliberately stop the supervisor without a VM snapshot.

For separately enabled persistent sign-in-shell activation, use **Restore Explorer** in Tessera or the independent script:

```powershell
& "$env:LOCALAPPDATA\Tessera\Recovery\Restore-Tessera.ps1"
```

The script requires only its adjacent `Tessera.Deployment.psm1`, not Tessera executables. It restores the exact recorded missing/string/expand-string value and clears the active marker only after verification, then starts Explorer for the current session. It retains the backup, installed files, and preferences. A later unrelated shell change is refused rather than overwritten. Close the remaining utility dock after restoring.

The independent script restores persistent Winlogon configuration, not the transient supervisor's in-memory taskbar backup. Do not mistake it for recovery from a hard-killed temporary-session supervisor.

### Emergency recovery without Tessera or PowerShell scripts

1. Use **Ctrl+Alt+Del → Task Manager → Run new task** and run `cmd.exe` without the administrator checkbox.
2. Inspect the override:

```cmd
reg.exe query "HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon" /v Shell
```

3. **Only if it points to the Tessera supervisor**, remove that per-user override and start system Explorer:

```cmd
reg.exe delete "HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon" /v Shell /f
%windir%\explorer.exe
```

This emergency route restores the default Explorer fallback, not necessarily the exact original raw value; the backup remains for inspection. Do not delete machine Winlogon values, Userinit, an unrelated shell override, or the whole Winlogon key. Do not activate again until the original backup/state has been reconciled.

## Focused tester checklist

- Confirm version/commit with `tessera-cli.exe --version` and `BUILD-INFO.txt`.
- Test the portable temporary session before activation: native toolbar/dock/menu, search, correct launch/pin/unpin, window switching and minimized restoration; closed windows must fail safely. Notifications should update the view without idle polling.
- Keep a dock menu open for at least 40 seconds: it must not freeze the UI or cause a heartbeat timeout. Check mouse and arrows/Home/End/Enter/Escape, dismissal, focus-denial behavior and 1x/2x sizing; menus must not clip to the dock's height.
- If the toolbar is displaced, run `.\tessera-cli.exe inspect --check-surfaces` from the extracted directory. Capture redacted output, a full screenshot, Windows display scaling, and whether it occurs at startup or after fullscreen return. The command only reads native geometry and metadata; it neither repairs placement nor records Slint scale/AppBar history. No toolbar, ambiguous titles, incomplete data or a mismatch intentionally returns failure.
- Record the original taskbar visibility and auto-hide setting before launch. Verify exact restoration after Exit, GUI crash and heartbeat loss while the supervisor remains alive. Confirm no ordinary-launch Winlogon change and no startup/fullscreen-return focus steal.
- Preview without Save, then save/restart: theme, compact mode, edge, and pins must persist correctly. Pinning must not accidentally save an unsaved theme preview.
- Check all dock edges, negative-origin/mixed-DPI monitors, keyboard/screen-reader navigation, overflow, and actual idle CPU/RAM.
- In a disposable VM, activate, sign in, restore independently, and sign in again. Test deleted GUI/crashed GUI and a stopped UI loop: Explorer must recover while unrelated applications stay running. Do not delete/block the supervisor without keeping emergency recovery available.
- Test fullscreen entry/exit: neither bar should overlay the foreground fullscreen window or stop the heartbeat when hidden. Report conservative geometry-hint failures rather than claiming complete game compatibility.
- Keep protection enabled and record exact policy/AV blocks. No clean scan or successful build is universal approval.

## Reporting and corresponding source

Include version, commit, Windows build, edition, session type, DPI/monitor setup, reproduction steps, and exact security warnings. Titles, paths, screenshots and `inspect` output may be private: redact them. There is no automatic log upload.

`tessera-0.1.0-alpha.11-source.zip` includes tracked source, lockfile, vendored dependencies/license files, and portable Cargo source replacement, including the maintained GPL Slint core patch. Keep corresponding source available alongside redistributed binaries as required by GPL version 3. Dependency and bundled-icon notices are in `THIRD-PARTY-NOTICES.txt`. The guide, release body and packaged `START-HERE` must correspond to the same final version/commit; use packaged `BUILD-INFO.txt` and `SOURCE-COMMIT.txt` for that commit rather than embedding a document's own commit hash.

For an offline Windows build, install Rust 1.92.0 plus MSVC/Windows SDK tools, enter the source directory, set `RUSTFLAGS=-C target-feature=+crt-static` using your shell's syntax, and run `cargo +1.92.0 build -p tessera --bins --release --target x86_64-pc-windows-msvc --frozen --offline`. Native signing, interactive Windows 11 sign-in validation, and security-product approval remain separate release gates.
