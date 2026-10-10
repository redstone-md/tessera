# Tessera — native desktop test alpha and source capability guide

**Unsigned, experimental Windows 11 x64 Home/Pro build; not a daily-driver shell or a complete 1:1 Seelen replacement.** Start with a disposable VM snapshot and an ordinary temporary desktop session, not sign-in-shell activation. Keep Windows protection enabled.

**Alpha.21 release candidate:** this guide describes the accumulated Material/native-capability source being packaged as `v0.1.0-alpha.21`. The existing release workflow must pass before its Windows ZIP, vendored GPL source and checksums are available. Alpha.20 remains the independently verified previous release; older releases/tags are unchanged. The parked, unverified 250ms media interpolation/absolute-label work is excluded. A successful release is not complete Seelen parity or interactive Windows 11 certification.

> **FULL-WAVE SOURCE POWER WARNING — six buttons perform real OS actions directly, with no extra Tessera confirmation.** Lock session, Log out, Power off, Reboot, Suspend and Hibernate are genuine lock/logoff/shutdown/restart/sleep/hibernate requests, not previews. Save work, verify your sign-in/recovery method, and choose each action explicitly only in a safe VM/session. Never automate a “click all buttons” smoke test. A native initiation receipt is not proof of the eventual power state or completed Windows updates.

## Separate baselines — alpha.11 code failure; alpha.12/.13 runtime packaging failures

Branch `release/alpha.11`, full source `363fe510a0540ed7c984345a2edd3692266dcfe8`, is verified `df36b2f` plus four metadata/docs-only commits. Its local rerun passed **649 tests across 19 suites**, and fresh `v0.1.0-alpha.11` was verified/pushed at that peeled SHA. [CI 37878846670](https://github.com/redstone-md/tessera/actions/runs/37878846670) **failed**: stable Rust 1.99 atomic/chunks lint failures, and Windows Rust 1.92 font-assumption failures (395 UI tests passed, two failed). **No packaging ran and no public alpha.11 release assets exist.** The failed tag is immutable and will not be retagged.

**Immutable alpha.12 passed code checks but failed packaging:** branch `release/alpha.12`, full source `7531f04d4270fd30403c500b099ddeaed8473d2f`, tag `v0.1.0-alpha.12`. Local gates passed 649 tests/19 suites (397 UI/188 Windows-adapter pure), strict Rust 1.92/1.99 Linux/MSVC, Rust 1.92 warnings-denied docs, 35 notices and format/diff. [CI 37881361921](https://github.com/redstone-md/tessera/actions/runs/37881361921) **finished failed**: Linux stable, Windows stable and Windows Rust 1.92 code jobs all passed tests/lint/docs/demo/read-only inspect/deployment fixtures; packaging's runtime supervisor exited **1**. GUI stderr/numerical GUI exit were not retained, so the cause remains open—generic “policy” text is not evidence of policy blocking. **No ZIP, checksums, public release or artifacts exist for this tag.**

**Immutable baseline alpha.13 — code green, packaging failed:** remote branch `release/alpha.13`, full source `6b31b64efd9cdc92f5edb7cffa0e64ed2b121a57`, tag `v0.1.0-alpha.13` stay unchanged. Runtime `28b9987` plus metadata `6b31b64` added bounded stderr and an inert synthetic-context/default-preferences host, rejecting native providers/saves/subscriptions while retaining the actual production GUI/two real UI-thread heartbeats. Local 660-test/19-suite receipt (UI 397 plus one ignored, app 16, Windows pure 196), strict Rust 1.92/1.99 Linux/MSVC/docs/35 notices/format-diff and focused 44-assertion `-RuntimeOnly` checks passed; full registry tests require Windows. [CI 37885358935](https://github.com/redstone-md/tessera/actions/runs/37885358935) **finished failed only at packaging**: all three code jobs, including full isolated Windows deployment fixtures, were green. No new release assets. First **Lock only** remains; full-six Power, network/Bluetooth/TSF/media/visibility, General V4 and live display belong to alpha.15.

Alpha.13's record proves GUI exit `0x00000001` before the first heartbeat, zero pulses and successful cleanup; all 106 stderr bytes were captured in Rust but discarded by PowerShell. Later source diagnosis reproduced duplicate Winit backend selection/forbidden event-loop recreation. The single-owner correction/default backend/passive first show at source `02fafe2112ce8faf0cd9f30d61c478e4ac3c8abd` passed [NativeDebug 37888007340](https://github.com/redstone-md/tessera/actions/runs/37888007340) in **3m38s: two real production UI-thread pulses plus five-second owned-GUI cleanup**. A matching SDK error length is consistent, not recovered historical stderr. This is genuine native debug startup proof, **not** production-release packaging, normal-session effects or a policy/renderer workaround.

**Published immutable baseline alpha.14:** remote `release/alpha.14`, source `e95dbb1e49e04a8e63d81fee6bc6f76274a36cfc`, immutable `v0.1.0-alpha.14`, annotated object `801ef9ae37add50b87170f90164d23ca0a83db65`; metadata-only release commit atop runtime correction `02fafe2112ce8faf0cd9f30d61c478e4ac3c8abd`. Local gates passed **664 tests/19 suites** (UI 397 plus one ignored, Windows pure 196, app 20), strict Rust 1.92/1.99 Linux/MSVC all-target checking, Rust 1.92 warnings-denied docs, 35 notices, 101 `-RuntimeOnly` assertions and format/diff. [Production CI 37889391783](https://github.com/redstone-md/tessera/actions/runs/37889391783) **passed** Linux stable, Windows stable, Windows Rust 1.92 and packaging/publication, including full isolated Windows deployment fixtures, static-CRT/PE/unsigned manifests, offline source/notices and **two actual production GUI UI-thread pulses plus five-second owned-GUI cleanup**. This does not certify ordinary desktop attachment, Windows 11 mixed-DPI/focus/accessibility, native effects or source-pixel parity.


**Immutable full-wave alpha.15 — Windows documentation failed, no assets:** `v0.1.0-alpha.15` remains at source `5a9828f49f011cbb0fa410c032c2db409c494112`. [Production CI 37897841677](https://github.com/redstone-md/tessera/actions/runs/37897841677) passed all Linux gates and both Windows Rust 1.92/stable Clippy and tests, then both Windows jobs failed warnings-denied rustdoc solely on two bare Microsoft SDK URLs in `native_power_updates/sdk.rs` lines 6–7. Packaging was skipped; no alpha.15 release assets exist. The annotated tag object is `c1aa1f8bb462f1ad3f044fe3527b9211d4138b96`; the failed tag is immutable, never deleted or retagged.

**Published immutable alpha.16:** verified `release/alpha.16` and `v0.1.0-alpha.16` remain at source `34ff877576e9ed68b5b2b5d5b7a987c94fe51672` (annotated object `d2a87f44f367bdb8d32336b38adcdb787a08bd26`). [Production CI 37902905003](https://github.com/redstone-md/tessera/actions/runs/37902905003) passed all three code jobs and optimized Windows packaging, including two real production GUI UI-thread pulses and five-second owned cleanup, then published a nondraft prerelease. All three downloaded assets passed independent hash/package/source verification. **Alpha.16 remains available unchanged; alpha.20 is now latest available.** See the [download and corresponding-source receipt](distribution-and-trust.md#published-alpha16-receipt). Alpha.17 has a separate published receipt; this alpha.16 evidence remains historical and immutable.

**Historical full-wave Windows Debug preflight:** [CI 37893888958](https://github.com/redstone-md/tessera/actions/runs/37893888958) passed in **8m6s** at source `8eeb17d567e17c8aaa90476feb8ec2ae7955ff05`: **1,186 tests across 19 suites, zero failed or ignored**, Rust 1.92/static CRT, followed by two real production GUI UI-thread pulses and five-second owned-GUI cleanup. This was **Debug** diagnostic startup, skipping normal providers/preferences/hooks, not optimized alpha.16 release packaging or certification of native effects, focus, privacy, pixels, multiple monitors or sign-in/recovery.

## Alpha.21 accumulated additions

- **Material appearance:** semantic HCT source colors, custom `#RRGGBB` Preview/Save/Cancel, rounded toolbar/dock surfaces and original OFL Google Sans Flex. Exact-video font, native blur and rights-blocked reference artwork are not certified.
- **Media/audio:** actual all-player inventory and selected-player controls; active input/output devices, selected-device masters, independent default-role requests and output app/System Sounds mixers. Seek still changes the real player; there is no parked 250ms interpolation layer.
- **Network/Bluetooth/battery:** source-scoped Wi-Fi Connect/Disconnect, Forget and software radio power with hardware-state honesty; Bluetooth radio On/Off with consent/readback; real event-driven battery facts. No active WLAN scan, hotspot, Bluetooth discovery or pairing is added.
- **Launcher/dock:** explicit escaped `web:` browser search and `files:` Windows Search dispatch without fake result inventories; saved dock lock and middle-click policy; native-supported Run as administrator/Open file location for trusted catalog targets.
- **Settings:** saved independent Dock/Toolbar visibility and optional CPU/RAM indicators; explicit current-user startup registration with foreign-entry protection. Startup changes the actual current-user Run value and is separate from preference Save/Cancel.
- **Wallpaper:** real static image/monitor selection, bounded Shell thumbnail and explicit image-color Preview; global Center/Tile/Stretch/Fit/Fill/Span; native-selected 2–32 same-folder images or one folder for Windows-owned slideshow timing/shuffle; explicit current-policy Read, genuine selected-monitor Previous/Next and options-only global Apply. Folder contents are not enumerated locally and there is no fabricated image count/gallery. These buttons change real Windows wallpaper policy outside preference Save/Cancel; SDK receipts/readbacks are not desktop-pixel proof.

Read-only inspection must not click native mutation buttons. Opt in separately
to each network/radio/profile/audio/startup/wallpaper effect in a safe session;
there is no automatic “click everything” effects check. No wallpaper video,
automatic live accent, workspace manager, tiling runtime, tray/notification
parity or claimed fix for the reported VM toolbar offset is included.

## Published alpha.20 capabilities and boundaries

Alpha.20 retains alpha.19's full-wave shortcuts/profile/placement/schema-5, direct User Log Out and shared Quick Settings current-player card, adding scoped seek and independently repaired audio source admission. The [central publication receipt](distribution-and-trust.md#published-alpha20-receipt) separates optimized production/download evidence from historical source/Debug proofs; none certifies interactive Windows effects or full native 1:1. **Seek changes the real player position:** inspect without touching the slider during read-only checks. No 250ms interpolation, absolute-label display layer, all-player inventory, device picker, explicit default-role settings or per-app mixer is included.

- **Native desktop:** Rust/Slint dock, toolbar, launcher and separately owned popups; no WebView. Ordinary launch leaves Explorer as the sign-in shell, supervises the GUI, temporarily hands off the primary taskbar and restores its captured state on exit. Tessera does not replace DWM or automatically move foreign windows.
- **Launcher:** independent Favorites/All, retained-title search, native keyboard selection, full retained results beyond 64, Favorites drag ordering/whole-tile feedback and saved Expand/Contract. Native discovery is bounded to 1,024 entries before deduplication, not every installed application. A scoped application **Pin/Unpin** menu changes launcher Favorites, not dock pins, through one complete-record save; query/view/catalog changes retire its authority.
- **User and Calendar:** real shell user name with a generic profile fallback; seven real OS known folders (Recent, Desktop, Downloads, Documents, Music, Pictures, Videos), opened only by explicit Ready-row input. Calendar uses real local Gregorian dates/locale names and transient month/year navigation. Saved General week start offers Monday, Sunday or Saturday, default Monday; navigation never changes OS time/locale.
- **Dock utilities:** Start → Show desktop → applications → Recycle Bin, plus optional media. Show desktop invokes the real Shell toggle. Recycle Bin reads actual aggregate counts, opens its fixed Shell namespace and has separate read/watch Retry and genuine Empty. Unknown/stale/error never means zero; Empty is destructive across the aggregate bin and belongs only in a separately authorized disposable-content test. Native confirmation/progress are not suppressed, but a dialog is not guaranteed in every OS condition.
- **Visibility:** default Dock OnOverlap, passive native mouse watching, edge reveal and delayed show/hide (100ms/800ms defaults). Overlap uses genuine uncapped eligible-window bounds, not the capped visible list. Unknown focus/touch/watch facts conservatively protect visibility; touch hotplug coverage, fullscreen classification and Windows behavior still need testing. Alpha.17 adds the global shortcut layer described below; native Start suppression, balanced input and focus/mixed-DPI effects remain interactive test gaps.
- **Audio and radios:** quick settings reads/controls genuine default-multimedia output/input volume and mute, with endpoint revalidation/readback. Network Refresh reads **OS-cached WLAN results**, not an active scan; its Settings action opens real Windows Settings. No Connect/Forget/hotspot controls. Bluetooth passively reads paired Classic/LE devices and radios; no discovery, Pair/Connect or radio toggle. Unavailable/denied/partial observations stay explicit.
- **Input language:** a genuine TSF desktop-session profile selector with real Windows Settings. Choosing a profile changes the actual desktop-session input profile, not just Tessera text; test switching only by deliberate input on a chosen session. Full IME toolbar/mode/candidate behavior is not implemented.
- **Optional media, default off:** explicitly saved enable/remove uses real GSMTC current-session data, supported Previous/Play-Pause/Next commands and bounded artwork decode/readback. Commands control the actual current player and revalidate the expected session. The source-sized tile is 136 logical pixels along the dock and 40 across (compact scales by .8), at all four edges. Missing session/artwork/control support stays honest; no sample album or invented player.
- **Alpha.20 current-player popup:** existing Toolbar Quick Settings shares the same MediaHost/provider/watch/one accepted flight as optional Dock media. An admitted/shown standalone popup observes even with saved Dock media off, without saving preferences or allocating Dock slots. Current-only metadata/artwork/transports retain honest unavailable states. Time/progress displays native observed signed 100ns timeline data, without interpolation. Pointer, keyboard and accessibility seek input uses **200ms leading/latest-trailing** submission with fresh session/range validation; there is no Dock seek. Timeline failure preserves metadata/artwork/transports. Popup/Root/session retirement discards unsubmitted seek/audio intent; accepted commands are never replayed or redirected to a replacement player. Audio rejection retires old controls/watch/leases without losing accepted-flight ownership, even if the popup remains open. Transport/seek/volume/mute are real effects requiring deliberate opt-in.
- **Power:** six explicit actions, real user/status presentation, optional captured update-install choice, checked stable display selection and display/text-scale watch. Hide releases the lease; after 30 seconds hidden, the actual optional Power window retires. Reopening builds fresh presentation without replaying accepted work. Display/watch failure does not invent placement or an empty monitor set. Alpha.15's Power typography follows a **16px browser-default tracer**, not the standard popover's 12.8px rule; the actual source WebView initial font and native selected system face remain uncaptured. The complete update prompt must wrap naturally: **18px is the switch height, not a fixed row height**, and no fixed 475px pending-state total is established. Inspect full text/wrapping without treating local font/frame evidence as Windows/source pixel parity.

The update label is only a diagnostic existence hint from two HKLM keys: `SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired` and `SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending`. It is **not** exhaustive Windows Update state. Explicit install intent is captured only when the hint is Pending and the choice is enabled, and applies only to Power off/Reboot. Omitting that intent does not guarantee Windows installs nothing; requesting it does not guarantee installation completes.

## Alpha.17 additions — published and gated, not live-VM certified

- Ordinary launch defaults global shortcuts enabled; diagnostic `false` is inert. Bare Win uses eligible hardware bare-key-up suppression in a global hook; it does not inject keys or bypass Windows-reserved shortcuts. Win+K uses real `RegisterHotKey`; typed registration/conflict status must remain visible rather than claiming success when Windows refuses registration.
- Shortcut capture pauses registration; cancel/reset and explicit Save separate drafts from applied behavior. Placement samples the current physical cursor point and its monitor. Optional cursor failure reports `placementUnavailable`, never fake primary-monitor placement.
- User/Home/Accounts use the current-process profile and bounded photo decoding, optional OneDrive Personal email and an opaque trusted OneDrive navigation target. The neutral shared-image decoder is not a sample identity/photo. Alpha.17 did not include User-popup Log Out; alpha.18 adds it as described below. Power **Log out** remains a real action.
- Actual Windows Start suppression/delivery, balanced key input, focus/mixed DPI, SID/photo/OneDrive integration and complete account parity remain pending interactive gates. These additions do not certify full native 1:1 behavior.

**Retained alpha.18 addition:** User-popup/avatar-corner **Log Out** requests immediate session logoff with **no extra Tessera confirmation**. It is shipped in alpha.18–alpha.20, absent from alpha.17, and not live-Windows certified. Do not click it during the ordinary read-only checklist; effects tests require a deliberate safe session, saved work and known sign-in/recovery. Its independently derived OFL glyph does not certify native pixels. See the [historical implementation receipt](../README.md#alpha18-development-source-receipt).

## Download and start safely

**Alpha.21 download locations (available only after successful packaging):** [Windows x64 ZIP](https://github.com/redstone-md/tessera/releases/download/v0.1.0-alpha.21/tessera-0.1.0-alpha.21-windows-x64.zip), [vendored source ZIP](https://github.com/redstone-md/tessera/releases/download/v0.1.0-alpha.21/tessera-0.1.0-alpha.21-source.zip), and [SHA256SUMS.txt](https://github.com/redstone-md/tessera/releases/download/v0.1.0-alpha.21/SHA256SUMS.txt). Keep all ten extracted files together. Older releases remain unchanged.


1. Download `tessera-0.1.0-alpha.21-windows-x64.zip`, `tessera-0.1.0-alpha.21-source.zip` and `SHA256SUMS.txt` from the successful alpha.21 prerelease. Compare both ZIPs with its two SHA-256 lines using `Get-FileHash <zip> -Algorithm SHA256`. Integrity is not publisher authentication; automatic GitHub source downloads are not the vendored corresponding-source ZIP.
2. Extract the whole `tessera-0.1.0-alpha.21-windows-x64` directory. Keep its ten files together: `Tessera.exe`, `tessera-shell.exe`, `tessera-cli.exe`, `LICENSE.txt`, `START-HERE.txt`, `THIRD-PARTY-NOTICES.txt`, `BUILD-INFO.txt`, `Install-Tessera.ps1`, `Restore-Tessera.ps1`, `Tessera.Deployment.psm1`. There is no separate portable legal-notices folder.
3. Read `BUILD-INFO.txt` and verify `tessera-cli.exe --version` reports `0.1.0-alpha.21`; compare its source commit with the immutable release tag and source ZIP's `SOURCE-COMMIT.txt`. Run `Tessera.exe` as an ordinary user, not administrator. Native packaging checks x64 resources/subsystems, `asInvoker`, `uiAccess=false`, PerMonitorV2 and static CRT. Windows system DLLs remain required; executables/scripts are unsigned.
4. Leave Defender, UAC, SmartScreen, Smart App Control and signing/execution policies enabled. If blocked, stop and report the exact warning. Do not disable protections, add exclusions, change execution policy or unblock downloaded scripts. A protected machine may need a signed build before testing. Do not upload private desktop data or modified private artifacts to a scanner.
5. Record the original taskbar visibility/auto-hide state. Open apps through Start; **Exit** or **Restore Explorer** should return the captured taskbar state. Ordinary launch never changes Winlogon or kills Explorer. A hard-killed supervisor cannot restore its in-memory taskbar backup; leave it running when stopping an unresponsive GUI.

## Settings and downgrade safety

**Shipped alpha.17–alpha.20 use strict schema 5; alpha.16 uses schema 4.** Back up `%LOCALAPPDATA%\Tessera\settings.json` before testing or downgrading. Schema 5 adds grouped shortcut preferences alongside General week start, Dock media, appearance, dock pins and grouped launcher mode/Favorites. Default global shortcuts are enabled and media is off. Valid schema **1–4** records migrate **only in memory**; no original bytes change until an explicit successful Save. Existing week start/media/mode/Favorites survive where present; older missing fields receive defaults (Monday/media off, schema 1 empty Favorites, schema 1/2 Windowed).

All explicit writes use one atomic complete-record save with a **16-KiB budget including the newline**. Explicit **Settings Save commits the current settings draft, including shortcuts**. Independent Pin/Favorite/reorder/mode/media saves preserve applied General/appearance/shortcuts and do not commit an unsaved settings draft; previews do not save. Failed writes preserve applied state. Invalid/future/damaged startup files stay untouched and disable ordinary saves for that session. Back up and rename the file manually, then restart to recover. **Alpha.16 treats schema 5 as future, leaves its bytes unchanged and disables ordinary Save**; there is no automatic downgrade conversion. Earlier readers may discard unsupported fields through their own Save paths. No automatic backup or multi-instance merge is provided; no captions, HWNDs, PIDs or shell commands are stored.

**Alpha.21 source-color/storage layer:** schema 6 adds required 24-bit RGB source
intent, default `#7ca45c`, with exact read-only schema-5 migration. Appearance's
Green/Amber/Blue/Coral/Pink choices preview real retained surfaces; Save persists
the source and Cancel restores the applied value. Valid non-preset RGB remains
visible as its actual hex with no falsely selected preset. Independent
Pin/Favorite/order/mode/media writes preserve applied source, not an unsaved
preview. Alpha.21 also retains saved dock lock/middle-click, CPU/RAM and visibility
choices. Alpha.20's schema 5 does not understand schema 6: back up before downgrading.

## Eight focused checks — ordinary session first

1. **Launch/recovery:** confirm version/commit, normal-user startup, native toolbar/dock/menu, no initial focus steal, and exact taskbar restoration after Exit. Check GUI-crash/heartbeat recovery only in a disposable VM with the supervisor still alive.
2. **Launcher:** All/search/Favorites beyond the first viewport, arrows then Return/Space, correct launch, independent dock pins and launcher Pin/Unpin. Drag a Favorite, cancel with Escape, then complete one reorder and restart. Check no stale menu action after query/view changes, no preview writes and no lost unavailable Favorite identities.
3. **Preferences/Calendar:** preview theme/compact/edge and change the General selector without Save; Calendar must keep the applied week start, and a separate favorite/mode/media save must not commit that draft. Explicit Save/restart should retain the chosen week start. Browse Month/Year/Today and verify genuine local dates, long/RTL labels and saved Monday/Sunday/Saturday grids without changing the system clock.
4. **Read-only popups:** open Network/Bluetooth and use Refresh; compare genuine cached/paired data and honest unavailable states. Open Network Settings and input-language Settings without switching profiles. No radio/network mutation should occur from opening/refreshing.
5. **Power presentation only:** open, inspect six labels/status/update choice, dismiss without selecting an action, wait over 30 seconds and reopen. Check focus/dismissal, negative origins, effective DPI/text scaling and live display changes; do not infer OS-action correctness from these read-only checks.
6. **Visibility/geometry:** check OnOverlap reveal/hide delay, each dock edge, fullscreen return, touch/unknown-state conservative visibility and heartbeat while hidden. If the toolbar is displaced, run `.\tessera-cli.exe inspect --check-surfaces`; retain redacted output/screenshot, scaling and startup/fullscreen-return timing. It reads geometry only, fails missing/ambiguous/incomplete/mismatched data, and does not repair the offset or record Slint scale/AppBar negotiation.
7. **Optional media/audio:** first open Quick Settings with Dock media disabled and observe the chosen current player/timeline without touching transport or seek input. If explicitly wanted, separately save Dock media enable, observe artwork/all four edges, then disable/restart to check persistence. Transport and seek really control the player; volume/mute really affect the selected default endpoint. Exercise each only deliberately at safe volume. Hide/reopen or retire the originating Toolbar: pending unsubmitted audio/seek intent must not replay, accepted work must not redirect to a replacement source, and reopened controls must wait for the old accepted flight. There is no automatic native seek-test harness.
8. **Input/accessibility/resources:** hold an ordinary non-destructive context menu open for 40 seconds; test Tab/arrows/Home/End/Enter/Space/Escape, overflow, clipping and idle CPU/RAM. Record mixed-DPI/screen-reader/focus/protection failures rather than treating Linux renderer evidence as Windows approval.

For alpha.21 retain the existing bare Win/Win+K, shortcut draft/Save, current-process profile/photo/OneDrive and source-scoped seek checks. The all-player/device/mixer views must show genuine inventories or explicit unavailable states; do not operate their native controls during read-only inspection. New wallpaper/startup/network/radio controls are real effects and require separate deliberate consent. No interpolated media progress or VM toolbar-offset fix is promised.

Separately opt in, action by action, to actual input-profile switching, known-folder opening, Show desktop, Recycle Bin Empty, six Power actions, User-popup Log Out and sign-in-shell tests. Use disposable contents/sessions, save work and know recovery first. Power and User Log Out have no extra Tessera confirmation. Recording/SDK-shape tests do not execute these effects; there is no automatic environment-variable native-effects test harness.

## Optional per-user installation

From a permitted Windows PowerShell 5.1 or PowerShell 7 session in the extracted directory:

```powershell
.\Install-Tessera.ps1 -PackagePath $PWD.Path -WhatIf
.\Install-Tessera.ps1 -PackagePath $PWD.Path
```

This copies the complete matching alpha.21 package to `%LOCALAPPDATA%\Programs\Tessera\0.1.0-alpha.21`, publishes independent recovery to `%LOCALAPPDATA%\Tessera\Recovery` and optionally adds a normal Start-menu shortcut. Installation itself does not change the sign-in shell, add a Run entry/service/scheduled task or log off; the separate Settings startup action can change the current-user Run registration. Version directories reject differing-content replacement; restore an active earlier shell before switching versions. Preferences are retained. `-WhatIf` is read-only; if scripts are blocked, stop rather than weakening policy.

## Experimental sign-in shell — disposable VM only

Keep a snapshot and know emergency recovery before signing out. This is isolated per-user Winlogon integration, not Microsoft's Enterprise-only Shell Launcher or a Home/Pro support guarantee. Activation refuses unsupported Windows builds, Windows Server, domain-managed hosts, conflicting policies, non-Explorer shells and enforcing/unverifiable Smart App Control for this unsigned build.

```powershell
.\Install-Tessera.ps1 -PackagePath $PWD.Path -EnableShell -WhatIf
.\Install-Tessera.ps1 -PackagePath $PWD.Path -EnableShell
```

The installer publishes independent recovery and runs the installed supervisor's genuine diagnostic GUI probe: two UI-thread heartbeats, then owned-process cleanup. Diagnostic mode skips normal native desktop attachments/focus and the mouse watcher; it proves launchability, not normal-session behavior or power/input/media effects. Blocked executable/missing GUI/startup/heartbeat failure prevents activation; explicit activation confirmation defaults to **No** (unlike direct Power actions).

Only then does it back up the original per-user Shell presence/raw value/type under `HKCU\Software\Tessera\ShellRecovery`, verify it and set the quoted supervisor path. Machine Winlogon, Userinit, UAC and security policies are untouched. Change applies at next sign-in, with no automatic sign-out. Child exit/startup failure/30-second heartbeat loss rolls back and starts system Explorer; only the owned failed GUI may be terminated, never unrelated apps. No restart loop exists. If the supervisor itself is blocked/deleted at next sign-in, it cannot execute fallback.

## Return to Explorer and emergency recovery

For a temporary session, use Tessera **Exit** or **Restore Explorer**. Task Manager may stop **Tessera.exe only** if unresponsive; keep `tessera-shell.exe` alive to restore the captured taskbar state.

For explicitly enabled persistent shell activation, use Restore Explorer or:

```powershell
& "$env:LOCALAPPDATA\Tessera\Recovery\Restore-Tessera.ps1"
```

The script needs only its adjacent `Tessera.Deployment.psm1`. It restores the exact recorded missing/string/expand-string Shell value, verifies before clearing the active marker and starts Explorer; backup/files/preferences remain. It refuses to overwrite a later unrelated Shell change. Close any remaining utility dock. This restores persistent configuration, not a hard-killed temporary supervisor's in-memory taskbar lease.

Without Tessera or permitted PowerShell scripts:

1. **Ctrl+Alt+Del → Task Manager → Run new task**, run `cmd.exe` without administrator privileges.
2. Inspect the per-user override:

```cmd
reg.exe query "HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon" /v Shell
```

3. **Only if it points to Tessera's supervisor**, remove that override and start system Explorer:

```cmd
reg.exe delete "HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon" /v Shell /f
%windir%\explorer.exe
```

This restores the default fallback, not necessarily the exact original raw value; reconcile the retained backup before activation again. Do not delete machine Winlogon, Userinit, another shell override or the whole key.

## Known problems and full-parity work remaining

- The reported VM toolbar offset remains causally unresolved; captured Windows evidence is needed. Linux software/Mesa GL at native/renderer 1x/2x is not native Windows DPI, foreground, accessibility or compositor certification. VM software fallback may be opaque; native backdrop blur is not promised. AppBar competition, focus/fullscreen return, games, touch hotplug, mixed DPI and multiple monitors remain interactive gates. Taskbar auto-hide state may also affect secondary taskbars.
- Tray/notification area, notifications/DND/actions, global bare-Windows-key toggle/shortcuts, workspaces, multi-monitor instances/targeting/inheritance, grouping/counters/previews/attention, launcher app-group folders/file/web scopes/ranking, full IME and full profile/account/photo remain incomplete. Full Settings pages/profiles/resources/plugins, wallpaper, update/service UI, brightness/nightlight/theme integrations, complete devices/media/session UI and optional tiling runtime remain in scope, not waived by this alpha.
- Exact source glyph parity is not certified. New independent Ionicons logout (MIT), Material restart (Apache-2.0) and Tabler Hibernate (MIT) artwork retains complete grants/notices. Suspend is enabled with a deliberately **empty decorative image**: the exact BiMoon grant is unestablished, not replaced by an OFL moon approximation. Reference `bin::empty/full` state art and exact-current Remix Settings/Unpin remain rights-blocked SourceGap items; the licensed generic Trash mark does not change by state. Bundled OFL/ISC/MIT/Apache and patched Slint notices retain their original terms.
- Native requests can fail or stall; admission/registration/return values are not proof of observed outcomes. Power/updates/display/watch teardown, real WLAN/paired-device availability, TSF/session effects, GSMTC player/artwork behavior, bin aggregate/watch/dialog coverage and sign-in/rollback still need actual Windows evidence. No automatic retry/replay of native mutations, update-completion promise or antivirus approval is claimed.

## Reporting, source and release evidence

Include version, full source commit, Windows build/edition, session type, monitor/DPI setup, reproduction steps and exact warnings. Captions, paths, artwork and screenshots can be private: redact `inspect` output and screenshots before sharing. No automatic log upload exists.

The corresponding-source ZIP contains tracked source/lockfile, registry vendor dependencies, `.cargo/config.toml`, `SOURCE-COMMIT.txt`, the actual patched `third-party/i-slint-core` and preserved licenses/sidecars/asset notices. Keep it available alongside redistributed binaries as GPL version 3 requires. Portable `THIRD-PARTY-NOTICES.txt` aggregates those notices; it is not a substitute for source. For an offline Windows build, install Rust 1.92.0 plus MSVC/Windows SDK, set `RUSTFLAGS=-C target-feature=+crt-static` using your shell's syntax, then run `cargo +1.92.0 build -p tessera --bins --release --target x86_64-pc-windows-msvc --frozen --offline` in the extracted source directory.

**Release evidence boundary:** this is the frozen prepublication alpha.21 guide used by packaging. Confirm actual successful CI and three assets for `v0.1.0-alpha.21` before treating it as available. Alpha.20's [published receipt](distribution-and-trust.md#published-alpha20-receipt) remains historical evidence for that unchanged release, not proof for this one. No native effects, full parity, Windows 11 VM geometry or security-product approval are certified by packaging.

**Historical alpha.10 only:** `v0.1.0-alpha.10` at `4866d3a` added native dock context menus, validated window commands and read-only geometry checks. Its immutable CI run is https://github.com/redstone-md/tessera/actions/runs/37675695227. That diagnostic fixture receipt neither certifies the new candidate nor explains the user's VM offset; alpha.8/.9/.10 assets are unchanged.
