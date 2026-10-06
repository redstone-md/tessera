# Tessera 0.1.0-alpha.2 — public shell test alpha

This is an **unsigned, experimental test build**, not a daily-driver shell or a signed consumer release. Windows 11 x64 Home/Pro is the target. A successful hosted Windows build or the earlier alpha's server smoke test does not validate Windows 11 sign-in recovery, security-product compatibility, or every display configuration.

## Available capabilities

- Native dock and launcher, real installed-application catalog and best-effort Windows icons; no WebView.
- Search, explicit application launch, up to 32 pinned applications, and explicit window activation/restoration. Windows foreground restrictions are respected.
- System/light/dark appearance preview, compact spacing, and primary-monitor dock edge preferences. **Save** persists preferences and applies dock placement; pin clicks save pins using the last saved appearance.
- Passive out-of-context desktop notifications and coalesced background observations, with manual Refresh fallback. No low-level keyboard/mouse hooks, injection, or periodic desktop polling.
- Per-user install and independent restore scripts; an optional supervised sign-in shell with Explorer fallback.
- Separate developer CLI: `--version`, `demo`, `inspect`, and `panel`.

This does not replace DWM, automatically tile/move application windows, provide the Windows notification area/Start flyouts, run plugins, or bypass security. The dock is currently a floating primary-monitor strip, not a work-area-reserving appbar. Fullscreen hiding is a conservative geometry hint, not universal game compatibility. Explorer file-management windows may bring parts of the Explorer shell back; this is a safe side effect, not suppressed by injection.

## Download and run alongside Explorer first

1. Download `tessera-0.1.0-alpha.2-windows-x64.zip` and `SHA256SUMS.txt` from the matching **test prerelease**. Compare `Get-FileHash <zip> -Algorithm SHA256` with the checksum. A checksum detects corruption, not publisher identity.
2. Extract the entire directory. Run `Tessera.exe` as your ordinary user, without administrator privileges. The package uses a verified static CRT; system Windows DLLs are still required.
3. Leave Defender, UAC, SmartScreen, Smart App Control, and signing/execution policies enabled. **Executables and scripts are unsigned.** If protection blocks them, stop and report the exact warning. Do not disable protection, add exclusions, change execution policy, or unblock downloaded scripts. Never upload private desktop data or modified private artifacts to a scanner. A protected machine may need a signed build before testing.
4. Use the dock's launcher/settings and recovery controls. Closing the utility does not alter sign-in configuration unless shell activation was separately enabled.

Settings live in `%LOCALAPPDATA%\Tessera\settings.json`. Old alpha appearance files load with default dock/pin fields. Invalid or future files stay untouched until an explicit save. No captions, HWNDs, PIDs, or shell commands are stored in preferences. Back up this file before downgrading; the earlier alpha does not understand new fields.

## Optional per-user installation

From a permitted Windows PowerShell 5.1 or PowerShell 7 session in the extracted directory:

```powershell
.\Install-Tessera.ps1 -PackagePath $PWD.Path -WhatIf
.\Install-Tessera.ps1 -PackagePath $PWD.Path
```

Installation copies the complete package to `%LOCALAPPDATA%\Programs\Tessera\0.1.0-alpha.2`, publishes the recovery script/module to `%LOCALAPPDATA%\Tessera\Recovery`, and optionally creates a normal Start-menu shortcut. It does **not** change the sign-in shell, create a Run entry, service, scheduled task, or log off the current session. Immutable version directories reject differing-content replacement; restore an active earlier shell before switching versions. Appearance data is retained.

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

Use **Restore Explorer** in Tessera, or run the independent script:

```powershell
& "$env:LOCALAPPDATA\Tessera\Recovery\Restore-Tessera.ps1"
```

The script requires only its adjacent `Tessera.Deployment.psm1`, not Tessera executables. It restores the exact recorded missing/string/expand-string value and clears the active marker only after verification, then starts Explorer for the current session. It retains the backup, installed files, and preferences. A later unrelated shell change is refused rather than overwritten. Close the remaining utility dock after restoring.

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
- Test dock/launcher before activation: search, correct launch/pin/unpin, window switching and minimized restoration; closed windows must fail safely. Notifications should update the view without idle polling.
- Preview without Save, then save/restart: theme, compact mode, edge, and pins must persist correctly. Pinning must not accidentally save an unsaved theme preview.
- Check all dock edges, negative-origin/mixed-DPI monitors, keyboard/screen-reader navigation, overflow, and actual idle CPU/RAM.
- In a disposable VM, activate, sign in, restore independently, and sign in again. Test deleted GUI/crashed GUI and a stopped UI loop: Explorer must recover while unrelated applications stay running. Do not delete/block the supervisor without keeping emergency recovery available.
- Test fullscreen entry/exit: the dock must not overlay it or stop the heartbeat when hidden. Report geometry-hint failures rather than claiming complete game compatibility.
- Keep protection enabled and record exact policy/AV blocks. No clean scan or successful build is universal approval.

## Reporting and corresponding source

Include version, commit, Windows build, edition, session type, DPI/monitor setup, reproduction steps, and exact security warnings. Titles, paths, screenshots and `inspect` output may be private: redact them. There is no automatic log upload.

`tessera-0.1.0-alpha.2-source.zip` includes tracked source, lockfile, vendored dependencies/license files, and portable Cargo source replacement. Keep corresponding source available alongside redistributed binaries as required by GPL version 3. Dependency notices are in `THIRD-PARTY-NOTICES.txt`.

For an offline Windows build, install Rust 1.92.0 plus MSVC/Windows SDK tools, enter the source directory, set `RUSTFLAGS=-C target-feature=+crt-static` using your shell's syntax, and run `cargo +1.92.0 build -p tessera --bins --release --target x86_64-pc-windows-msvc --frozen --offline`. Native signing, interactive Windows 11 sign-in validation, and security-product approval remain separate release gates.
