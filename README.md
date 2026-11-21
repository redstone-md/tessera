# Tessera

A modular, native desktop shell for Windows 11, written in Rust with [GPUI](https://www.gpui.rs).

## Status

**Early rewrite. Not usable as a desktop shell yet.**

What exists today, by default on every display:

- A top bar with a clock and a **Quit Tessera** button, drawn as floating rounded pills. Clicking the clock opens a popup with a calendar to page through by month; it closes when clicked again or when another window takes focus. The calendar shows dates only, without events. Below it the popup lists the notifications waiting in the Windows notification center, newest first: application, age, title and up to two lines of text, each with a button that dismisses it, and **Clear all** for the lot. The list is read from Windows every two seconds through the notification listener, which on the one machine it was tried on was allowed without a prompt for a Tessera that is not packaged; if Windows refuses, the list says so. Reading and dismissing one notification were tried, **Clear all** was not. Tessera shows no banner when a notification arrives — Windows still shows its own — and a notification cannot be clicked to open its application, has no buttons, images or icon, and is cut to a fixed height.
- An application launcher, opened from the **Apps** button in the top bar. It lists the shortcuts of the Start menu folders, narrows them as you type, and starts the selected one with Enter or a click; the arrow keys move the selection and Escape closes it. Applications without a shortcut file there — most packaged (Store) apps — are not listed. There is no pinning, no recent list and no search beyond application names, and the list is read each time the launcher opens. An application whose shortcut names no usable icon shows a placeholder.
- The output volume in the top bar. Clicking it opens a popup with a mute button and a slider that also follows the mouse wheel. It controls the master volume of the default output device only: no device picker, no per-application volume, no microphone. Changes made elsewhere show up within a second.
- Meters in the top bar for processor load and memory in use, each a wavy line with a value, read every two seconds; memory can be shown in gigabytes and either meter as the value alone. A battery meter of the same kind, read every twenty seconds, shows nothing on a machine without a battery; the only machine Tessera was tried on has none, so that meter has never been seen working. There are no meters for network, disks, temperatures or the graphics card.
- A notification area ("system tray") in the top bar: the icons applications add with `Shell_NotifyIcon`. Pointing at an icon unrolls its description at the left end of the strip; a left or right click is passed on to the application, which then opens its own window or menu. How it works and what that costs:
  - Tessera creates an invisible window of the taskbar's class above Explorer's, which is where applications deliver their icons, and passes every message on to Explorer, so Explorer's own notification area stays complete. At startup it broadcasts `TaskbarCreated`, the message Explorer sends after a restart, so that running applications add their icons again; an application that ignores it shows up only once it changes its icon.
  - Tried on one machine: the icons of about fifteen applications and of Windows itself (volume, microphone, Bluetooth, security, safe removal) appeared with their descriptions, and a right click opened the menu of ShareX. A right click on the volume and Bluetooth icons, which belong to Explorer, opened nothing. Left clicks are sent the same way but have not been tried.
  - The network, battery and clock items of the Windows 11 taskbar are not notification icons and do not appear. There are no balloon notifications, no overflow area, no reordering and no hiding of icons, and an application running as administrator does not receive clicks from a Tessera that does not.
- A dock with one button per application window on that display; a minimized window counts for the display it restores to. Clicking a button brings its window to the foreground, or minimizes it if it already is there. The window list is polled twice a second. Packaged (Store) apps show a placeholder instead of their icon, and windows of elevated processes may ignore clicks. The `[icons]` table of the configuration replaces the icon of an application, chosen by the file name of its executable, with a PNG, JPEG or ICO file; it is read once at startup.

Motion is driven by springs: surfaces slide in from their edge at startup, dock buttons, notification icons and notifications grow in and shrink away as they come and go, the foreground indicator stretches, the wavy lines of the meters travel as their level changes, popups fade and slide in and out, and buttons respond to hover and press. Nothing else is animated yet: text changes and the clock switch instantly, as does extension content other than its progress bars, and surfaces do not animate out on exit.

While Tessera runs:

- Both surfaces stay above other windows and reserve their screen edge, so maximized windows stop at them. Windows releases that space when Tessera exits.
- The Windows taskbar is hidden. To stop it reserving space, Tessera switches it to auto-hide, which is a persistent Explorer setting. A second `tessera` process waits for the shell to exit — normally, by crashing or by being terminated — and then restores the previous auto-hide setting and shows the taskbar again. If that watchdog process is itself terminated first, the taskbar stays hidden until you turn off "Automatically hide the taskbar" in Windows Settings or restart Explorer.

Tessera registers no hooks. Anything the taskbar provided that is not listed above, such as pinned apps, has no replacement yet. The Windows key still opens the Windows Start menu, not the launcher.

## Configuration

Everything a surface shows is a widget provided by a module. The compiled-in modules provide the widgets `clock`, `calendar`, `label`, `launcher`, `notifications`, `volume`, `volume-slider`, `cpu`, `memory`, `battery`, `tray`, `quit` and `windows`; extensions add more.

The layout is read once at startup from `%APPDATA%\tessera\config.toml`. It lists the surfaces — screen edge, thickness, all displays or only the primary one — and the widgets in the start, center and end of each. A widget is written as its name, or as a table that adds options the widget understands (the clock takes a `format`, the calendar a `week_start`, a label an `icon` and a `text`, the tray the side its `tips` unroll at) and a `popup` to open when the widget is clicked. Popups are declared in the same file as a name, a size and a list of widgets, so any widget, including one from an extension, can open a popup or be placed in one. One popup is open at a time, and popups cannot be resized. Without that file the [built-in layout](crates/tessera/src/default_config.toml) is used; it documents the format. A file that fails to parse is ignored as a whole, and an unknown widget name is skipped; both are reported on standard error, which only a debug build started from a terminal shows.

`theme = "name"` in that file applies `%APPDATA%\tessera\themes\name.toml` over the built-in theme. A theme can replace colors only, as `#rrggbb` under `[colors]`, using the role names `surface_container`, `surface_container_highest`, `on_surface`, `on_surface_variant`, `outline`, `primary` and `secondary_container`. Shapes, sizes, fonts and motion are not themeable yet.

## Extensions

An extension is a WebAssembly file that Tessera runs in a [Wasmtime](https://wasmtime.dev) sandbox. It describes the content of its widgets as a tree of rows, columns, text, captions, buttons and progress bars, which Tessera draws with the active theme, and it is told when one of its buttons is pressed. [`wit/extension.wit`](wit/extension.wit) is the whole interface. It is unstable and will change.

Besides drawing, an extension can:

- read the clock and write to the shell's log;
- read its settings, the keys of the `[extension.<id>]` table in `config.toml`;
- keep up to 64 KiB of text across restarts, stored as `state.txt` in its own directory;
- read the files its manifest lists under `permissions.files`, and no others;
- send HTTPS requests to the hosts its manifest lists under `permissions.network`, and no others. Requests run in the background and the answer is delivered later; redirects are not followed, a response is cut off at 1 MiB, and servers are verified against the Windows certificate store.

The limits of that:

- Permissions are granted by the manifest alone. Tessera does not ask before loading an extension and shows nowhere what an extension is permitted, so read `extension.toml` before installing one: an extension with a file and a host permitted can send that file to that host.
- Settings, API keys included, are plain text in `config.toml`.
- It still cannot reach devices, Bluetooth, other processes or anything else of Windows, so an extension that shows a mouse's battery level cannot be written yet.
- It can only add widgets. It cannot declare popups or screens of its own — the user places its widgets in a popup in `config.toml` — and it has no icons, images, text input or scrolling.
- Each call into an extension is limited in computation, and an extension is limited to 64 MiB of memory. A widget whose extension exceeds a limit or crashes shows "stopped working" until Tessera restarts.
- Extensions are loaded once at startup. A widget placed on several displays shares one extension instance.
- There is no marketplace, registry, installer, signing or update mechanism. Installing means copying files by hand.

To install one, create `%APPDATA%\tessera\extensions\<any name>\` with two files: `extension.wasm` and an `extension.toml` manifest that gives the extension `id`, its widgets and its permissions. A layout then refers to a widget as `<id>.<widget name>`. Build an example with:

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown --manifest-path extensions/counter/Cargo.toml --target-dir target/extensions
```

and copy `target/extensions/wasm32-unknown-unknown/release/tessera_example_counter.wasm` as `extension.wasm` next to a copy of its `extension.toml`. Once both examples are built, `cargo test -- --include-ignored` also runs the tests that load them.

### Examples

[`extensions/counter`](extensions/counter) counts presses of its button.

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
