# Tessera

A modular, native desktop shell for Windows 11, written in Rust with [GPUI](https://www.gpui.rs).

## Status

**Early rewrite. Not usable as a desktop shell yet.**

What exists today, by default on every display:

- A top bar with a clock and a **Quit Tessera** button, drawn as floating rounded pills.
- A dock with one button per application window on that display; a minimized window counts for the display it restores to. Clicking a button brings its window to the foreground, or minimizes it if it already is there. The window list is polled twice a second. Packaged (Store) apps show a placeholder instead of their icon, and windows of elevated processes may ignore clicks.

While Tessera runs:

- Both surfaces stay above other windows and reserve their screen edge, so maximized windows stop at them. Windows releases that space when Tessera exits.
- The Windows taskbar is hidden. To stop it reserving space, Tessera switches it to auto-hide, which is a persistent Explorer setting. A second `tessera` process waits for the shell to exit — normally, by crashing or by being terminated — and then restores the previous auto-hide setting and shows the taskbar again. If that watchdog process is itself terminated first, the taskbar stays hidden until you turn off "Automatically hide the taskbar" in Windows Settings or restart Explorer.

Tessera registers no hooks. Anything the taskbar provided that is not listed above — Start button, system tray, notifications, pinned apps — has no replacement yet; the Start menu still opens with the Windows key.

## Configuration

Everything a surface shows is a widget provided by a module. Three modules are compiled in, providing the widgets `clock`, `quit` and `windows`. **There is no way to load a module from outside the binary yet**: no extension API, no extension loading, no marketplace.

The layout is read once at startup from `%APPDATA%\tessera\config.toml`. It lists the surfaces — screen edge, thickness, all displays or only the primary one — and the widgets in the start, center and end of each. Without that file the [built-in layout](crates/tessera/src/default_config.toml) is used; it documents the format. A file that fails to parse is ignored as a whole, and an unknown widget name is skipped; both are reported on standard error, which only a debug build started from a terminal shows.

`theme = "name"` in that file applies `%APPDATA%\tessera\themes\name.toml` over the built-in theme. A theme can replace colors only, as `#rrggbb` under `[colors]`, using the role names `surface_container`, `surface_container_highest`, `on_surface`, `on_surface_variant`, `outline`, `primary` and `secondary_container`. Shapes, sizes, fonts and motion are not themeable yet.

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
