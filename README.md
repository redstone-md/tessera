# Tessera

A modular, native desktop shell for Windows 11, written in Rust with [GPUI](https://www.gpui.rs).

## Status

**Early rewrite. Not usable as a desktop shell yet.**

What exists today, by default on every display:

- A top bar with a clock and a **Quit Tessera** button, drawn as floating rounded pills. Clicking the clock opens a popup with a calendar to page through by month; it closes when clicked again or when another window takes focus. The calendar shows dates only: no events and no notifications.
- The output volume in the top bar. Clicking it opens a popup with a mute button and a slider that also follows the mouse wheel. It controls the master volume of the default output device only: no device picker, no per-application volume, no microphone. Changes made elsewhere show up within a second.
- A dock with one button per application window on that display; a minimized window counts for the display it restores to. Clicking a button brings its window to the foreground, or minimizes it if it already is there. The window list is polled twice a second. Packaged (Store) apps show a placeholder instead of their icon, and windows of elevated processes may ignore clicks.

Motion is driven by springs: surfaces slide in from their edge at startup, dock buttons grow in and shrink away as windows open and close, the foreground indicator stretches, popups fade and slide in and out, and buttons respond to hover and press. Nothing else is animated yet: text changes, the clock and extension content switch instantly, and surfaces do not animate out on exit.

While Tessera runs:

- Both surfaces stay above other windows and reserve their screen edge, so maximized windows stop at them. Windows releases that space when Tessera exits.
- The Windows taskbar is hidden. To stop it reserving space, Tessera switches it to auto-hide, which is a persistent Explorer setting. A second `tessera` process waits for the shell to exit — normally, by crashing or by being terminated — and then restores the previous auto-hide setting and shows the taskbar again. If that watchdog process is itself terminated first, the taskbar stays hidden until you turn off "Automatically hide the taskbar" in Windows Settings or restart Explorer.

Tessera registers no hooks. Anything the taskbar provided that is not listed above — Start button, system tray, notifications, pinned apps — has no replacement yet; the Start menu still opens with the Windows key.

## Configuration

Everything a surface shows is a widget provided by a module. The compiled-in modules provide the widgets `clock`, `calendar`, `volume`, `volume-slider`, `quit` and `windows`; extensions add more.

The layout is read once at startup from `%APPDATA%\tessera\config.toml`. It lists the surfaces — screen edge, thickness, all displays or only the primary one — and the widgets in the start, center and end of each. A widget is written as its name, or as a table that adds options the widget understands (the clock takes a `format`, the calendar a `week_start`) and a `popup` to open when the widget is clicked. Popups are declared in the same file as a name, a size and a list of widgets, so any widget, including one from an extension, can open a popup or be placed in one. One popup is open at a time, and popups cannot be scrolled or resized. Without that file the [built-in layout](crates/tessera/src/default_config.toml) is used; it documents the format. A file that fails to parse is ignored as a whole, and an unknown widget name is skipped; both are reported on standard error, which only a debug build started from a terminal shows.

`theme = "name"` in that file applies `%APPDATA%\tessera\themes\name.toml` over the built-in theme. A theme can replace colors only, as `#rrggbb` under `[colors]`, using the role names `surface_container`, `surface_container_highest`, `on_surface`, `on_surface_variant`, `outline`, `primary` and `secondary_container`. Shapes, sizes, fonts and motion are not themeable yet.

## Extensions

An extension is a WebAssembly file that Tessera runs in a [Wasmtime](https://wasmtime.dev) sandbox. It describes the content of its widgets as a tree of rows, text and buttons, which Tessera draws with the active theme, and it is told when one of its buttons is pressed. [`wit/extension.wit`](wit/extension.wit) is the whole interface. It is unstable and will change.

What an extension can do today is deliberately little:

- Its only access to the outside is a `log` function. It cannot reach files, the network, the clock, devices or other processes, so an extension that shows a battery level or talks to Bluetooth cannot be written yet; that needs host APIs that do not exist.
- It can only add widgets to a surface. It cannot open popups or screens of its own, and it has no icons, images or layout beyond a row.
- Each call into an extension is limited in computation, and an extension is limited to 64 MiB of memory. A widget whose extension exceeds a limit or crashes shows "stopped working" until Tessera restarts.
- Extensions are loaded once at startup. A widget placed on several displays shares one extension instance, and pressing a button on one display does not repaint the others until their next refresh.
- There is no marketplace, registry, installer, signing or update mechanism. Installing means copying files by hand.

To install one, create `%APPDATA%\tessera\extensions\<any name>\` with two files: `extension.wasm` and an `extension.toml` manifest that gives the extension `id` and its widgets. A layout then refers to a widget as `<id>.<widget name>`. [`extensions/counter`](extensions/counter) is a working example; build it with:

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown --manifest-path extensions/counter/Cargo.toml --target-dir target/extensions
```

and copy `target/extensions/wasm32-unknown-unknown/release/tessera_example_counter.wasm` as `extension.wasm` next to a copy of its `extension.toml`. `cargo test -- --include-ignored` then also runs the test that loads it.

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
