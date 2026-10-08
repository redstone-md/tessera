# Security policy

## Supported versions

Tessera is pre-release software with no supported stable release. Security work currently targets the latest `main`. Older snapshots are not maintained, and the project is not ready for use as a primary desktop shell.

## Confidential vulnerability reporting

GitHub private vulnerability reporting is enabled for this public repository. Use [Report a vulnerability](https://github.com/redstone-md/tessera/security/advisories/new), also available under **Security → Advisories**, instead of a public issue for exploit details or sensitive information. Reports are shared privately with maintainers; there is no response-time commitment yet.

Include the affected commit/release, Windows version/build, expected impact, and a minimal reproduction with personal data removed. Do not include credentials or unrelated private desktop output, and do not send reports to GitHub `noreply` addresses.

If GitHub's confidential form is unavailable, open only a high-level public request to arrange a safe follow-up; do not publish exploit details or secrets there. Allow coordinated assessment and disclosure before sharing vulnerability details publicly.

## Security scope

Relevant areas include Win32 memory safety, FFI callback boundaries, DPI-context restoration, untrusted desktop text, dependency vulnerabilities, and future window-management or plugin permissions.

The implementation observes desktop state, calculates synthetic layouts, and presents a native dock/launcher. Observation remains read-only; scoped out-of-context WinEvent notifications trigger coalesced refreshes, not input interception. Explicit activation revalidates HWND/PID and respects foreground restrictions; launch resolves a trusted catalog item. Explicit Appearance Save/dock-pin/launcher-favorite actions persist a complete per-user preference record bounded to 16 KiB; invalid/future startup files disable ordinary writes for that session, never implicit recovery or replacement. See the [settings recovery and downgrade notes](docs/alpha-testing.md). Experimental shell activation requires a separate installer action, verified backup, and a production two-heartbeat runtime probe; its current-user override is restored by the native supervisor or independent scripts. No DWM replacement, automatic layout, plugin execution, implicit autostart, or security-setting changes are implemented. Activation is non-atomic; same-process handle reuse remains a risk.

In unpublished development, launcher launch/favorite authorization checks current logical result keys and the full retained trusted catalog, not instantiated Slint rows or keys displayed only on another surface. All/search inventory and saved-order Favorites resolution are no longer capped at 64; unresolved saved favorites cannot authorize execution. Native catalog discovery still stops at 1,024 entries before deduplication, and the recovery Panel's independent 64-application projection remains bounded. Virtualization changes presentation demand, not the trusted launch boundary, and adds no desktop observations, polling, timer or input hook.

Avoid publishing desktop captions, file paths, screenshots, or tokens as part of a report. Please allow maintainers to assess a report and coordinate disclosure before making exploit details public.

## Security-product compatibility

All Windows executables request `asInvoker` with `uiAccess=false`; normal use does not require elevation. There are no low-level global input hooks, injection, desktop polling, network operations, or toolkit inspection server. Only the supervisor's own failed/diagnostic GUI child may be terminated. Registry recovery refuses unrelated subsequent shell changes and retains the backup; a blocked supervisor cannot execute fallback, so independent Task Manager recovery is required. Read the [distribution and trust policy](docs/distribution-and-trust.md).

Public test alphas are explicitly unsigned and not certified as antivirus-safe. The prerelease includes corresponding source and checksums; some protected machines cannot run it until trusted signing is available. Never disable Windows protection or add exclusions for a test. Investigate detections before calling them false positives, and obtain approval before uploading modified private artifacts or private desktop data to a vendor or third-party scanner.
