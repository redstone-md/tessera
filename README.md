# Tessera

A modular, native desktop shell for Windows 11, written in Rust with [GPUI](https://www.gpui.rs).

## Status

**Early rewrite. Not usable as a desktop shell yet.**

What exists today, by default on every display:

- A top bar with a clock and a **Quit Tessera** button, drawn as floating rounded pills. Clicking the clock opens a popup with a calendar to page through by month; it closes when clicked again or when another window takes focus. The calendar shows dates only, without events. Below it the popup lists the notifications waiting in the Windows notification center, newest first: application, age, title and up to two lines of text, each with a button that dismisses it, and **Clear all** for the lot. The list is read from Windows every two seconds through the notification listener, which on the one machine it was tried on was allowed without a prompt for a Tessera that is not packaged; if Windows refuses, the list says so. Reading and dismissing one notification were tried, **Clear all** was not. Tessera shows no banner when a notification arrives — Windows still shows its own — and a notification cannot be clicked to open its application, has no buttons, images or icon, and is cut to a fixed height.
- An application launcher, opened from the **Apps** button in the top bar. It lists the shortcuts of the Start menu folders and the packaged (Store) applications Windows reports as installed, narrows them as you type, and starts the selected one with Enter or a click; the arrow keys move the selection and Escape closes it. There is no pinning, no recent list and no search beyond application names, and the list is read each time the launcher opens. An application whose shortcut names no usable icon shows a placeholder.
- A Start menu around that launcher, in the same popup: above the list a grid of pinned applications, below it buttons that lock the session, put the computer to sleep, sign out, restart and shut down. The button on a row of the list pins or unpins its application, a right click on a pinned one unpins it, and both rewrite `config.toml`; a click on a pinned one starts it. Every power button except the lock asks for a second click within four seconds. The grid and the buttons are the widgets `apps` and `power`, so a layout can leave them out, reorder them or put them elsewhere. Tried on a running shell: pinning, unpinning both ways, starting a pinned application, and the first click on the sleep and shut-down buttons with the question going away by itself. No power button was ever confirmed and the lock button was never clicked, so none of the five actions has been carried out by Tessera yet. The Windows key does not open it, packaged (Store) applications are not listed, and a name too long for two lines is cut.
- The network connection, in the top bar: an icon for a cable, Wi-Fi or a mobile network with its signal strength, and the name of the network, with "no internet" added when only the local network is reached and "Offline" when nothing is connected. Windows reports changes of the connection; the signal strength is read again every thirty seconds. Seen on a running shell only with a cable connected: Wi-Fi, mobile networks, losing the connection and the report of a change were not tried, and networks cannot be listed or joined from Tessera.
- The Wi-Fi networks in reach, in a popup that a click on the network connection opens: each with its signal, a lock if it takes a password, and whether it is connected or Windows has saved settings for it. A click on a saved one asks Windows to join it. A network without saved settings cannot be joined here, as there is no place to type a password; Wi-Fi cannot be switched on or off, and a network cannot be left or forgotten. The list is read every three seconds while the popup is open, and the adapters are asked to look anew every fifteen. Windows shows the networks only to applications that may use the location. The only machine Tessera was tried on has Wi-Fi switched off, so only the message that says so was seen on a running shell: the list itself and joining a network are untested.
- Settings, behind the gear in the top bar: every widget of every bar with buttons to move it one step, into the neighboring part of the bar when it is at the end of its own, or to remove it; a list of the widgets that can be added to each part; the pinned applications, with the running ones offered for pinning; and the theme, chosen among the files of the themes directory. Each click rewrites `config.toml`, keeping its comments, and is applied at once while the settings stay open. A widget is added without options, so one that needs a popup or other options still has to be finished in the file; bars, popups, icon replacements and extensions cannot be changed there, and nothing is dragged. Tried on a running shell: moving, removing and adding a widget, pinning and unpinning, and choosing a theme.
- What is playing, in the top bar: the artist and the title that the application Windows considers current reports, shown only while some application reports anything. Clicking it opens a popup with the picture the application offers for the piece, if it offers one, and buttons to skip back, pause or play, and skip on. It is read once a second, so a change shows up to a second late; the picture is read when the title or the artist changes and for a few seconds after, so one that an application swaps later under the same title stays stale. When the application tells how long the piece is, the popup also shows how far into it the player is, as two times and a wavy line; between the reports of the application the time is counted on at the usual speed, so it is off for a player that runs faster or slower. The position can only be read, not set, and there is no choice between several applications that play. Tried on a running shell: the readout and the popup, with its picture and position, for a paused player; a change of piece and a position that moves were not. The buttons were not pressed there, so whether an application follows them is untested.
- The output volume in the top bar. Clicking it opens a popup with a mute button and a slider that also follows the mouse wheel. It controls the master volume of the default output device only: no device picker, no per-application volume, no microphone. Changes made elsewhere show up within a second.
- Meters in the top bar for processor load and memory in use, each a wavy line with a value, read every two seconds; memory can be shown in gigabytes and either meter as the value alone. A battery meter of the same kind, read every twenty seconds, shows nothing on a machine without a battery; the only machine Tessera was tried on has none, so that meter has never been seen working. There are no meters for network, disks, temperatures or the graphics card.
- A `command` widget for content of your own without writing an extension: it runs a program you name in the layout every few seconds, without a window, and shows the first line it prints, after an optional icon. A run is stopped after ten seconds; a program that fails, is stopped or cannot be started leaves a warning sign in the pill and the reason on the error stream. The output is read as UTF-8, so a program that prints in the console code page shows wrong characters for anything but ASCII. The default layout does not use it.
- A notification area ("system tray") in the top bar: the icons applications add with `Shell_NotifyIcon`. Pointing at an icon unrolls its description at the left end of the strip; a left or right click is passed on to the application, which then opens its own window or menu. How it works and what that costs:
  - Tessera creates an invisible window of the taskbar's class above Explorer's, which is where applications deliver their icons, and passes every message on to Explorer, so Explorer's own notification area stays complete. Edge bars, Tessera's own surfaces among them, talk to Explorer through the same window, so their requests are passed on too. At startup it broadcasts `TaskbarCreated`, the message Explorer sends after a restart, so that running applications add their icons again; an application that ignores it shows up only once it changes its icon.
  - Tried on one machine: the icons of about fifteen applications and of Windows itself (volume, microphone, Bluetooth, security, safe removal) appeared with their descriptions, and a right click opened the menus of ShareX and of the Bluetooth icon, which belongs to Explorer; the Bluetooth menu closed again on a click on another window or on the bar. Icons that Explorer added before Tessera started and only updates since, the volume and security icons on that machine, never say which message they want and do not react to clicks. Left clicks are sent the same way but have not been tried.
  - The network, battery and clock items of the Windows 11 taskbar are not notification icons and do not appear. There are no balloon notifications, no overflow area, no reordering and no hiding of icons, and an application running as administrator does not receive clicks from a Tessera that does not.
- An **Extensions** button in the top bar that lists the extensions of the configured registries and installs, updates and removes them; see [Registries](#registries) for what that does and does not check.
- A dock with one button per application window on that display; a minimized window counts for the display it restores to. The `pinned` option of the `windows` widget lists applications that keep a button at the start of the dock whether they run or not: each is the path of a program or shortcut, the name the Start menu gives an application, or `shell:AppsFolder/` followed by the identifier of a packaged (Store) application. A click starts the application; once it has a window on that display, the button stands for that window and gets its dot, and further windows of the application get buttons of their own after the others. A window is matched to its pin by the program file, so an application whose shortcut starts another program than the one that owns the window gets a second button instead. A packaged application is matched by its identifier, and its icon is the one Windows draws for it. Tried on a running shell: the packaged applications of one machine were listed with their icons, and three of them were pinned by name and by identifier and showed their icons. Starting one, and matching the window of a running one to its pin, were not tried. The built-in layout pins nothing; pins are changed from the menu of a button, from the settings, or in `config.toml`. Tried on a running shell: pinning by path and by name, starting a pinned program, and switching to and minimizing its window; a pin outliving its window was not watched. Clicking a button brings its window to the foreground, or minimizes it if it already is there. While the pointer rests on a button, a preview opens above it: the title of the window, a button that closes the window, and a live picture of it that Windows draws (a DWM thumbnail); a click on the picture switches to the window. The preview stays while the pointer is on it or on its button and follows the pointer to another button. Tried on a running shell: opening, moving between buttons and onto the preview and back, closing when the pointer leaves, and switching to and closing a test window from its preview. A window Windows gives no picture of gets the title and the close button only; that case was not met. A right click on a button opens a menu above it: another window of the program (or "Open" for a pinned application that does not run), pinning the program to the dock or unpinning it, which rewrites `config.toml`, and closing the window. The menu takes the keyboard focus and closes when something is chosen or another window is clicked. Tried on a running shell: opening the menu, pinning the program of a test window, closing that window from the menu of its pinned button, and unpinning the program afterwards. "New window" and "Open" were not tried. The window list is read again when Windows reports a change to a window. Packaged (Store) apps show a placeholder instead of their icon, and windows of elevated processes may ignore clicks. The `[icons]` table of the configuration replaces the icon of an application, chosen by the file name of its executable, with a PNG, JPEG or ICO file. The image files are read once: replacing one needs a restart.

Motion is driven by springs: surfaces slide in from their edge at startup, dock buttons, notification icons and notifications grow in and shrink away as they come and go, the foreground indicator stretches, the wavy lines of the meters travel as their level changes, the output of a `command` fades in when it changes, popups, menus and window previews fade and slide in and out, and buttons respond to hover and press. Nothing else is animated yet: text changes and the clock switch instantly, as does extension content other than its progress bars, and surfaces do not animate out on exit.

While Tessera runs:

- Both surfaces stay above other windows and reserve their screen edge, so maximized windows stop at them. Windows releases that space when Tessera exits.
- The Windows taskbar is hidden. To stop it reserving space, Tessera switches it to auto-hide, which is a persistent Explorer setting. A second `tessera` process waits for the shell to exit — normally, by crashing or by being terminated — and then restores the previous auto-hide setting and shows the taskbar again. If that watchdog process is itself terminated first, the taskbar stays hidden until you turn off "Automatically hide the taskbar" in Windows Settings or restart Explorer.

Tessera injects nothing into other processes. To learn when windows open, close or come to the foreground it asks Windows for out-of-context window events (`SetWinEventHook`), which arrive in its own process; the list is also read again every five seconds for anything that goes unreported. Anything the taskbar provided that is not listed above, such as jump lists, has no replacement yet. The Windows key still opens the Windows Start menu, not the launcher.

Measured once on one machine (three displays, a release build, the built-in layout, nothing clicked): the shell process held about 80 MB of memory, used about 1.9 seconds of processor time to start, and about half a second of processor time for every 30 seconds it then sat idle. Most of the idle work is widgets asking Windows every second or so for values that Windows does not report on its own, such as the processor load.

## Configuration

Everything a surface shows is a widget provided by a module. The compiled-in modules provide the widgets `clock`, `calendar`, `label`, `launcher`, `apps`, `power`, `notifications`, `settings`, `media`, `media-controls`, `network`, `wifi`, `volume`, `volume-slider`, `cpu`, `memory`, `battery`, `command`, `tray`, `quit` and `windows`; extensions add more.

The layout is read from `%APPDATA%\tessera\config.toml`. It lists the surfaces — screen edge, thickness, all displays or only the primary one — and the widgets in the start, center and end of each. A widget is written as its name, or as a table that adds options the widget understands (the clock takes a `format`, the calendar a `week_start`, a label an `icon` and a `text`, the tray the side its `tips` unroll at) and a `popup` to open when the widget is clicked. Popups are declared in the same file as a name, a size and a list of widgets, so any widget, including one from an extension, can open a popup or be placed in one. One popup is open at a time, and popups cannot be resized. Without that file the [built-in layout](crates/tessera/src/default_config.toml) is used; it documents the format. A file that fails to parse is ignored as a whole, and an unknown widget name is skipped; both are reported on standard error, which only a debug build started from a terminal shows.

The layout file and the theme file it names are checked every second, and a change is applied about two seconds after the file was written, without a restart: surfaces are added, removed and resized, their widgets are built again and slide in, and an open popup closes. A changed file that fails to parse changes nothing and is reported the same way. Changes to the `[icons]` table and to the settings of extensions go the same way but have not been tried on a running shell yet. Extension files and icon image files are not watched.

 If the environment variable `TESSERA_CONFIG_DIR` is set, that directory is used instead of `%APPDATA%\tessera` for everything read from it: the layout, themes, icons and extensions.

`theme = "accent"` names no file: it derives every color from the accent color of the Windows settings, dark or light as those settings say for the taskbar, and the settings popup offers it as **Windows accent**. A theme file can do the same with `seed = "accent"`, or derive the colors from a color of its own with `seed = "#rrggbb"`; what it sets under `[colors]` is applied over the derived colors. The roles are tones of palettes with the hue of the seed, as in Material 3, but built in the OKLCH color space instead of Material's HCT, so they are near the colors the Material tools give, not equal to them. A theme file chooses between them with `scheme = "dark"`, the default, `"light"` or `"system"`, which follows the Windows setting for the taskbar; a light scheme without a seed is derived from the green of the built-in colors. While the colors follow the accent color or that setting, both are asked for again every second. Choosing the accent theme and a light theme file were tried on a running shell, on a machine set to dark; changing the accent color or the mode while it runs was not, since that would have changed the settings of the machine it was tried on. On a light bar the white notification icons that applications draw for a dark taskbar are hard to see.

`theme = "name"` in that file applies `%APPDATA%\tessera\themes\name.toml` over the built-in theme. Besides `seed` and `scheme`, a theme can replace single colors, as `#rrggbb` under `[colors]`, using the role names `surface_container`, `surface_container_highest`, `on_surface`, `on_surface_variant`, `outline`, `primary` and `secondary_container`. Shapes, sizes, fonts and motion are not themeable yet.

## Extensions

An extension is a WebAssembly file that Tessera runs in a [Wasmtime](https://wasmtime.dev) sandbox. It describes the content of its widgets as a tree of rows, columns, text, captions, buttons and progress bars, which Tessera draws with the active theme, and it is told when one of its buttons is pressed. [`wit/extension.wit`](wit/extension.wit) is the whole interface. It is unstable and will change.

Besides drawing, an extension can:

- read the clock and write to the shell's log;
- read its settings, the keys of the `[extension.<id>]` table in `config.toml`;
- keep up to 64 KiB of text across restarts, stored as `state.txt` in its own directory;
- read the files its manifest lists under `permissions.files`, and no others;
- send HTTPS requests to the hosts its manifest lists under `permissions.network`, and no others. Requests run in the background and the answer is delivered later; redirects are not followed, a response is cut off at 1 MiB, and servers are verified against the Windows certificate store.

The limits of that:

- Permissions are granted by the manifest alone. The `marketplace` widget shows what an extension asks for before it installs it; an extension copied in by hand is loaded without a question, and nothing shows what an installed extension is permitted, so read `extension.toml`: an extension with a file and a host permitted can send that file to that host.
- Settings, API keys included, are plain text in `config.toml`.
- It still cannot reach devices, Bluetooth, other processes or anything else of Windows, so an extension that shows a mouse's battery level cannot be written yet.
- It adds widgets and popups made of widgets. Its manifest can list `[[popup]]` entries, written as in a layout, and name one of them as the `popup` of a widget: a click on that widget then opens it without anything in `config.toml`. A layout knows such a popup as `<id>.<popup name>`; a `[[popup]]` of that name in the layout replaces it, a `popup` given to the widget in the layout opens that one instead, and `popup = ""` opens none. A popup of an extension can hold widgets of the shell and of other extensions too. An extension cannot open or close a popup from its code, cannot add a surface or a screen of another kind, and has no icons, images, text input or scrolling.
- Each call into an extension is limited in computation, and an extension is limited to 64 MiB of memory. A widget whose extension exceeds a limit or crashes shows "stopped working" until Tessera restarts.
- Extensions are loaded at startup, and again when the `marketplace` widget installs or removes one; all of them start anew then. Files changed by hand are not noticed until a restart. A widget placed on several displays shares one extension instance.
- Nothing is signed and nobody reviews what a registry offers; see below.

### Registries

The `marketplace` widget lists the extensions that registries offer and installs, updates and removes them. The built-in layout opens it from the `Extensions` button. It contacts the registries when it is shown and when `Refresh` is pressed, not before.

`registries` in `config.toml` lists the registries to read, each an `https://` address or an absolute path of a file on this machine. Without the key it is the [registry of this repository](registry/registry.toml); `registries = []` reads none. A registry is a TOML file:

```toml
name = "Mine"

[[extension]]
id = "lights"
name = "Lights"
description = "Shown under the name."
version = "1.2.0"
manifest = "lights/extension.toml"
manifest_sha256 = "…"
wasm = "lights/extension.wasm"
wasm_sha256 = "…"

[[registry]]
url = "https://example.org/more/registry.toml"
```

`manifest`, `wasm` and `url` are `https://` addresses or paths relative to the registry file, which cannot lead out of its directory; a registry on the web cannot name files of this machine. A registry can name further registries, and those are read as well, down to three steps from a listed one and at most 16 registries in all; each is read once. When two registries offer the same `id`, the one read first wins, and the listed ones are read before those they name.

What this does and does not guard:

- Both files are checked against the SHA-256 sums in the registry, and the `id` in the manifest has to be the offered one. That catches a damaged or swapped download; it says nothing about the author, because whoever controls a registry controls the sums too. There are no signatures and no review.
- Before installing or updating, the widget names the hosts and files the manifest asks for and waits for `Allow and install`. Nothing else of the extension is inspected.
- Installing writes `extension.wasm`, `extension.toml` and `installed.toml` (the version and the registry it came from) to `%APPDATA%\tessera\extensions\<id>\`. It does not change the layout: the widget lists the names to put into `config.toml`, such as `counter.total`, and that is still done by hand.
- `Update` is offered when the version of the registry differs from the installed one; nothing checks for updates on its own.
- `Remove` deletes those files and the extension's `state.txt`. An extension without `installed.toml` was copied in by hand and is neither replaced nor removed.
- A registry or an offer that cannot be used is listed at the top of the widget with the reason.

Tried on a running shell: listing, installing and removing the `counter` example, from a registry file on disk and from the registry of this repository on the web, with the widgets appearing in and leaving the bar. `Update`, failed downloads and nested registries are covered by tests only.

[`registry/update.sh`](registry/update.sh) rebuilds the examples and rewrites the registry of this repository.

### By hand

To install one without a registry, create `%APPDATA%\tessera\extensions\<any name>\` with two files: `extension.wasm` and an `extension.toml` manifest that gives the extension `id`, its widgets and its permissions. A layout then refers to a widget as `<id>.<widget name>`. Build an example with:

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown --manifest-path extensions/counter/Cargo.toml --target-dir target/extensions
```

and copy `target/extensions/wasm32-unknown-unknown/release/tessera_example_counter.wasm` as `extension.wasm` next to a copy of its `extension.toml`. Once both examples are built, `cargo test -- --include-ignored` also runs the tests that load them.

### Examples

[`extensions/counter`](extensions/counter) counts presses of its buttons. `counter.clicks` is the count with a button for a bar; `counter.total` is the count alone, and a click on it opens the popup the extension brings, with buttons that add to the count and reset it.

[`extensions/ai-limits`](extensions/ai-limits) shows what is left of the usage limits of AI services: the plan, each limit as a share and a bar, and when it resets. Its `summary` widget is one line for a bar; its `details` widget is meant for a popup, which has a fixed size, so size it for the services you enable:

```toml
[[surface]]
edge = "top"
end = [{ widget = "ai-limits.summary", popup = "ai" }]

[[popup]]
name = "ai"
width = 380
height = 420
content = ["ai-limits.details"]

[extension.ai-limits]
providers = ["claude", "codex", "openrouter", "deepseek"]  # default: claude and codex
summary = ["claude"]            # services in the bar; default: all of `providers`
interval_minutes = 5            # how often each service is asked
openrouter_key = "..."
deepseek_key = "..."
```

The buttons of `details` choose whether the plan, the reset times and the bars are shown and whether shares count what is left or what is used; the choice is remembered.

How far each service has been tried:

| Service | Asked with | Tried against the real service |
| --- | --- | --- |
| Claude | the sign-in of Claude Code, `~/.claude/.credentials.json` | No. The usage endpoint is undocumented; answers are parsed in tests against a made-up sample. |
| Codex | the sign-in of the Codex CLI, `~/.codex/auth.json` | No. Same as Claude. |
| OpenRouter | `openrouter_key` | Only that a wrong key is refused and reported. Answers are parsed against a sample in the documented format. |
| DeepSeek | `deepseek_key` | No. Answers are parsed against a sample in the documented format. |

The Claude and Codex endpoints may change or stop answering at any time, and their answers may differ from what this extension expects; it then shows an error instead of numbers. It never refreshes a sign-in: once the token in the file has expired, it shows an error until Claude Code or Codex has renewed it. A token is sent only to the service it belongs to.

## History

The previous Slint-based implementation was removed. Its source and test builds remain available at the [`v0.1.0-alpha.22`](https://github.com/redstone-md/tessera/releases/tag/v0.1.0-alpha.22) tag; it is unmaintained.

## Goals

- A dock, top bar, launcher, system popups and settings that replace the everyday Explorer shell surfaces.
- Familiar floating-window behavior by default; tiling is optional.
- [Seelen UI](https://github.com/eythaann/Seelen-UI) as the functional reference, with a Material 3 Expressive appearance.
- Native rendering only: no WebView, Electron or HTML/CSS/JavaScript UI stack.
- Every control does what it says. Missing capabilities are reported as unavailable, never faked.

These are goals, not a list of completed features. [CONTEXT.md](CONTEXT.md) defines the project vocabulary.

## Building

Requires stable Rust with the MSVC toolchain and the Windows SDK.

```sh
cargo run
```

The dependency graph is large. On machines with limited memory, bound the parallelism with `cargo build -j 4`.

## Contributing

English is the project language. See the [contribution guide](CONTRIBUTING.md), the [Code of Conduct](CODE_OF_CONDUCT.md) and the [security policy](SECURITY.md).

## License

Copyright (C) 2026 Tessera contributors.

Tessera is licensed under the **GNU General Public License, version 3 only** (`GPL-3.0-only`). See [LICENSE](LICENSE). The software is provided without warranty. Third-party dependencies retain their own licenses.
