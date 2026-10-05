# Tessera 0.1.0-alpha.1 — private test alpha

This is the first native Windows test build, **not a daily-driver shell or a signed consumer release**. It is an ordinary window alongside Explorer, not yet a dock or taskbar replacement. Windows 11 x64 Home/Pro is the test target; hosted Windows CI is not a substitute for testing that desktop.

## What is available

- Native GUI launcher without a console window; native Slint widgets, no WebView.
- A manually refreshed list of ordinary application windows, title search, and explicit foreground activation. Minimized windows may be restored only when you choose them. Windows can refuse foreground activation; Tessera reports the refusal rather than bypassing it.
- System/light/dark appearance and compact spacing. Changes preview immediately; **Save preferences** persists only these choices.
- A separate developer CLI for `--version`, `demo`, and read-only `inspect`.

No automatic window layout, Explorer replacement, taskbar hiding, autostart, global input hooks, process injection, telemetry, updates, or plugin execution. Observation and search do not activate windows. A failed observation retains visibly stale data and disables activation until a successful refresh.

## Download and run

1. Download `tessera-0.1.0-alpha.1-windows-x64.zip` and `SHA256SUMS.txt` from the matching **private prerelease**. Check the ZIP's SHA-256, for example with `Get-FileHash <zip> -Algorithm SHA256`. Checksums detect corruption; they do not replace a publisher signature.
2. Extract the complete directory to a location writable by your normal user. Run `Tessera.exe` without administrator privileges. No installer or extra VC++ redistributable is intended: the package uses a static CRT and verifies its imports. System Windows DLLs are still required.
3. Leave Defender, UAC, SmartScreen, and Smart App Control enabled. **These executables are unsigned.** SmartScreen, Smart App Control, enterprise policy, or antivirus software may block them. If blocked, stop and report the exact warning; do not disable protection, add exclusions, or upload private artifacts to a scanning service. Some machines cannot test this alpha until trusted signing is available.
4. Close the panel normally to stop it. It does not install startup entries, a service, or a shell. Delete the extracted directory to remove the binaries.

Settings are read from `%LOCALAPPDATA%\Tessera\settings.json` and written only by Save preferences. Missing settings use defaults; invalid/future settings remain unchanged until an explicit save. To reset appearance, close Tessera and remove that file. No captions, system handles, or startup configuration are stored there.

## Test checklist

- Confirm the exact version in the panel and with `tessera-cli.exe --version`.
- Move, resize, minimize, and close the panel. Check mixed-DPI monitors and keyboard/screen-reader navigation. Explorer and its taskbar must remain available.
- Open normal apps such as Notepad, a browser, and File Explorer. Refresh, search by part of their titles, and choose the intended result. A filtered result must still target the correct window. Verify minimized restoration; foreground refusal is an expected reported outcome, not a reason to elevate.
- Close a listed app before choosing it: the action must fail safely and allow Refresh. Handles are transient and revalidated; observation and activation are not atomic.
- Switch themes and compact spacing, save, close, reopen, and check persistence. Preview without Save must not change stored preferences. A settings failure must leave the panel usable with a visible error.
- Leave the panel idle: no periodic observation or fixed-frame-rate rendering is requested by Tessera. Record CPU/memory usage rather than assuming it is zero.
- Exercise empty lists and stale/error states if reproducible. A failed refresh must not silently claim current data or allow stale actions.

The alpha does not manage games or fullscreen layouts. Switching foreground can itself cause an application/game to react to focus loss; do not use it during an important session without testing that application's behavior first.

## Reporting and source

Include version, `BUILD-INFO.txt` commit, Windows build, monitor/DPI setup, reproduction steps, and the exact security warning if applicable. Window titles, screenshots, paths, and `inspect` output can be private; redact them before posting. There is no automatic log collection or upload.

The matching `tessera-0.1.0-alpha.1-source.zip` contains the tracked source, lockfile, vendored dependencies and their license files, and Cargo source-replacement configuration. GPL-3.0-only applies to Tessera; dependency licenses are listed in `THIRD-PARTY-NOTICES.txt`. Supply the source ZIP alongside any shared binary when recipients cannot access this private repository.

To build the source archive on Windows, install Rust 1.92.0 and the MSVC C++/Windows SDK build tools, enter the extracted source directory, set `RUSTFLAGS=-C target-feature=+crt-static`, and run `cargo +1.92.0 build -p tessera --bins --release --target x86_64-pc-windows-msvc --frozen --offline`. Set the environment variable using your shell's normal syntax; the explicit target keeps static-CRT flags away from host build tools.

Signing, installer identity validation, and interactive security-product compatibility are still pending. A passing build or one clean scan is not universal antivirus approval.
