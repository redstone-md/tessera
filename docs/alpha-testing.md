# Tessera 0.1.0-alpha.10 — native desktop test alpha

This is an **unsigned, experimental test build**, not a daily-driver shell or a signed consumer release. Windows 11 x64 Home/Pro is the target. A successful hosted Windows build or the earlier alpha's server smoke test does not validate Windows 11 sign-in recovery, security-product compatibility, or every display configuration.

**New in alpha.10:** separate native dock context menus, pinned-item middle-click launch, explicit activate/minimize/ordinary-close requests, and a read-only toolbar geometry diagnostic. Pending appearance callbacks no longer recursively borrow window leases during geometry updates. The Seelen-style dock/toolbar/application menu and supervised temporary primary-taskbar handoff introduced in alpha.9 remain native, with no WebView and no sign-in-shell replacement on ordinary launch. Alpha.8 and alpha.9 remain unchanged. Take a disposable Windows 11 VM snapshot; do not enable sign-in-shell replacement for the first UI test.

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

1. Download `tessera-0.1.0-alpha.10-windows-x64.zip` and `SHA256SUMS.txt` from the matching **test prerelease**. Compare `Get-FileHash <zip> -Algorithm SHA256` with the checksum. A checksum detects corruption, not publisher identity.
2. Extract the entire directory and keep `Tessera.exe`, `tessera-shell.exe`, `tessera-cli.exe` and all accompanying files together. Run `Tessera.exe` as your ordinary user, without administrator privileges. The package uses a verified static CRT; system Windows DLLs are still required.
3. Leave Defender, UAC, SmartScreen, Smart App Control, and signing/execution policies enabled. **Executables and scripts are unsigned.** If protection blocks them, stop and report the exact warning. Do not disable protection, add exclusions, change execution policy, or unblock downloaded scripts. Never upload private desktop data or modified private artifacts to a scanner. A protected machine may need a signed build before testing.
4. The native dock and toolbar should appear, then the Explorer taskbar should hide. Open applications with the start tile. Right-click Start for settings/recovery/exit, a pinned tile for Open/Unpin, or a running window for Activate/Minimize/Close; middle-click a pinned tile to launch another instance. Test search, pins and window switching. Choose **Exit** or **Restore Explorer** in Tessera: the supervisor should restore the original taskbar visibility and auto-hide state. Ordinary launch never changes sign-in configuration.

In released **alpha.10**, settings live in `%LOCALAPPDATA%\Tessera\settings.json`. Old alpha appearance files load with default dock/pin fields. Invalid or future files stay untouched until an explicit save, which can replace them. No captions, HWNDs, PIDs, or shell commands are stored in preferences. Back up this file before running/downgrading to an earlier alpha; it does not understand new fields.

**Unpublished development settings — not in alpha.10:** independent ordered launcher favorites are stored in strict schema 2. Valid schema 1 loads without changing bytes and starts with empty favorites, not dock pins; only successful explicit persistence writes schema 2. Appearance, pins and favorites share one 16-KiB record budget, with visible save errors rather than truncation. Invalid/future startup files disable ordinary Appearance/Pin/Favorite writes for the session; preview remains usable. Manually back up and rename the settings file, then restart to recover. There is no automatic settings backup. Alpha.10 rejects schema 2, and its old explicit Save/pin path can replace newer preferences and lose favorites, so back up before testing an older binary.

**Unpublished development inventory checks — not alpha.10:** with a genuine catalog containing more than 64 applications, use All and raw-title substring search to reach results beyond the former cutoff; save more than 64 resolvable favorites within the shared record budget and check their complete saved order. An unavailable favorite should disappear from executable results while its saved identity remains, then resolve in the same order after a genuine catalog refresh. View switches/reopen clear query, selection and scroll; same-query refresh retains a still-matching selected identity. Launch and favorite actions must work beyond the first viewport without adding desktop observations. Native discovery is bounded to 1,024 entries before deduplication, so do not treat absent installed applications as proof of pagination or promise complete OS inventory. The recovery Panel still projects at most 64 applications.

Exercise native Arrow then Return and Arrow then Space across virtual row boundaries without an intervening draw/property query, and check the actual 1,024-item tail and its two-item final row when using the controlled full-size fixture. Offscreen Tab/accessibility semantics are not fully certified. Seven positive one-pixel squares, six eight-pixel gaps and two four-pixel gutters cannot fit a viewport narrower than 63 pixels; there is no all-columns-at-any-width guarantee. Focused source-core unit tests are separate from application fixtures. Software pixel tests and passing Mesa GL checks at 1x/2x do not certify native Windows/VM focus, IME, screen readers, mixed DPI or multi-monitor behavior. This verifies the retained-inventory layer, not full Seelen applications-menu parity.

## Optional per-user installation

From a permitted Windows PowerShell 5.1 or PowerShell 7 session in the extracted directory:

```powershell
.\Install-Tessera.ps1 -PackagePath $PWD.Path -WhatIf
.\Install-Tessera.ps1 -PackagePath $PWD.Path
```

Installation copies the complete package to `%LOCALAPPDATA%\Programs\Tessera\0.1.0-alpha.10`, publishes the recovery script/module to `%LOCALAPPDATA%\Tessera\Recovery`, and optionally creates a normal Start-menu shortcut. It does **not** change the sign-in shell, create a Run entry, service, scheduled task, or log off the current session. Immutable version directories reject differing-content replacement; restore an active earlier shell before switching versions. Appearance data is retained.

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

`tessera-0.1.0-alpha.10-source.zip` includes tracked source, lockfile, vendored dependencies/license files, and portable Cargo source replacement. Keep corresponding source available alongside redistributed binaries as required by GPL version 3. Dependency and bundled-icon notices are in `THIRD-PARTY-NOTICES.txt`.

For an offline Windows build, install Rust 1.92.0 plus MSVC/Windows SDK tools, enter the source directory, set `RUSTFLAGS=-C target-feature=+crt-static` using your shell's syntax, and run `cargo +1.92.0 build -p tessera --bins --release --target x86_64-pc-windows-msvc --frozen --offline`. Native signing, interactive Windows 11 sign-in validation, and security-product approval remain separate release gates.
