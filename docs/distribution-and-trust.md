# Distribution, trust, and user comfort

Tessera must behave like an ordinary, well-identified desktop application. Reducing security friction means minimizing unnecessary privileges and disruptive behavior, maintaining a trustworthy release process, and fixing false detections through vendor channels. It does not mean hiding from security software. No implementation or signature can guarantee that every antivirus product or enterprise policy will accept every build.

## Runtime baseline

- Run at the caller's normal permission level. The Windows MSVC executable embeds `requestedExecutionLevel="asInvoker"` and `uiAccess="false"`; it does not request elevation or bypass protected UI. Starting it from an elevated process can still inherit that process's privileges.
- Use documented Windows interfaces and a native toolkit. No drivers, process injection, remote process-memory manipulation, low-level global keyboard/mouse hooks, executable packers, or security-setting changes in the baseline.
- Ordinary launch/installation remain alongside Explorer. The floating dock stays above normal windows but hides for a conservative fullscreen hint; the utility panel remains a normal closable window. Neither automatically positions foreign windows. Activation/launch/system actions require explicit input and respect Windows restrictions.
- Observe off the UI thread with one request in flight, passive out-of-context desktop notifications, coalescing, and manual Refresh fallback. No continuous desktop polling or fixed-frame-rate idle rendering. Only the supervised shell uses a two-second UI-thread heartbeat and kernel waits on its own child/event; hiding the dock must not stop that heartbeat.
- No implicit autostart, services, scheduled tasks, telemetry, downloads, updates, or plugin execution. Explicit Save/pin changes store bounded per-user preferences. A separate experimental `-EnableShell` action requires preflight, backup, independent restore, and a real supervisor/GUI launchability probe before writing only the current-user Winlogon Shell. DWM, machine Winlogon, Userinit, and security settings are untouched.
- Keep captions in memory for the visible panel; treat them as untrusted text, not commands, paths, or markup. Do not log or upload them. Disable toolkit inspection servers and live-debug features in production.

Future integrations must justify any additional rights and stay separate from appearance settings. Prefer scoped documented commands and events over techniques that alter other processes. A theme must not silently enable a system integration.

## Distinguish the protections

[Microsoft's SmartScreen guidance](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation) distinguishes publisher reputation from each file's reputation. A valid signature identifies a publisher but does not guarantee the absence of an unrecognized-app warning. Self-signed certificates are not a consumer trust solution. Windows Smart App Control can also block unsigned or untrusted executables; enterprise policies may prevent continuation entirely.

[Microsoft Defender's developer FAQ](https://learn.microsoft.com/en-us/defender-xdr/developer-faq) distinguishes malware/PUA detections from SmartScreen reputation and firewall policy. Microsoft offers no general developer pre-approval list that guarantees no false positives. A malware false-positive submission is not a mechanism for manufacturing SmartScreen download reputation.

## Consumer-release gate

Before distributing a consumer binary or installer:

1. Pass release-tag CI and interactive Windows 11 testing with Defender, SmartScreen, and applicable Smart App Control settings left enabled. Verify standard-user launch, clean close, mixed DPI, keyboard/screen-reader behavior, and no focus stealing or sustained idle CPU usage. Record tested Windows builds and security settings; do not call one clean scan universal approval.
2. Build from the tagged source and committed lockfile with a documented toolchain. Review the runtime dependency closure on a clean machine, including CRT requirements. No opaque post-build payloads, bundlers, or unreviewed executables downloaded at runtime.
3. Include accurate executable/installer identity, version, license, and dependency notices. Provide corresponding source required by GPL version 3. Finalize resources before signing.
4. Sign distributed executable code and installer/package artifacts with a real trusted publisher identity and appropriate timestamping. Verify the final signatures and compute checksums after signing. Never modify signed artifacts afterward. Signing credentials must stay outside the repository and untrusted pull-request jobs.
5. Use a consistent publisher identity and an authenticated distribution channel. Evaluate Microsoft Store distribution where its policies fit the shell capabilities; it is not an assumed deployment mechanism. Microsoft-managed signing services or a trusted certificate issuer require real identity validation and eligibility checks.
6. Make installation and any startup/shell integration explicit, reversible, and removable without disabling Windows protection. Do not add application directories to Defender exclusions or instruct users to turn off UAC, Defender, SmartScreen, or Smart App Control.

Signing, a trusted publisher identity, consumer installation, and clean-machine security validation are **not implemented yet**. Test-alpha packaging is implemented. Ordinary source commits do not run GitHub Actions; release tags matching `v*` start checks. Maintainer-authorized numbered alpha tags package and publish explicitly unsigned test assets only after checks pass. Alpha.2 is explicitly approved as a public experimental prerelease. There is no unsigned stable/consumer publication path.

## Informed tester alpha exception

A maintainer-approved test prerelease may distribute an explicitly unsigned build to informed testers before consumer gates are complete. Public alpha.2 publication is an explicit development decision, not evidence of consumer readiness. It includes optional per-user install/restore scripts and separately consented, experimental shell activation: ordinary installation adds no sign-in persistence, and activation requires tested launchability plus independent/emergency recovery. Use disposable test environments. The package includes version/commit, checksums, complete vendored source/licenses, and the [tester limitations/checklist](alpha-testing.md), and verifies native resources, subsystems, privilege/DPI manifests, static CRT, and real GUI heartbeat.

This exception does not imply security-product approval. Smart App Control, SmartScreen, antivirus, or enterprise policy may block it; report the block rather than weakening protection. Machines requiring trusted signing must wait for a signed build. Stable/consumer distribution still requires every gate above; a public repository or successful CI does not waive them.


## Handling a detection

Capture the exact artifact hash, release/source revision, signature status, Windows/security-product version, and detection name. Investigate the code and dependencies before assuming a false positive. Reproduce against the exact final signed artifact and current definitions; avoid exposing unrelated user data.

Maintainers may submit confirmed incorrect malware detections through the affected vendor's official software-developer channel, such as [Microsoft Security Intelligence](https://www.microsoft.com/en-us/wdsi/filesubmission). Obtain approval before uploading private binaries to a vendor or third-party service; submissions can disclose the software. Never change the binary merely to defeat a scanner, repeatedly re-sign to chase heuristics, or ask users for blanket exclusions. Resolve the underlying issue or wait for the vendor's reviewed determination.
