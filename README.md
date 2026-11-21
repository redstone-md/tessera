# Tessera

A modular, native desktop shell for Windows 11, written in Rust with [GPUI](https://www.gpui.rs).

## Status

**Early rewrite. Not usable as a desktop shell yet.**

The repository currently contains a single top bar with a clock. It opens as an ordinary overlay window: it does not hide the Windows taskbar, reserve screen space, register hooks or change any system setting.

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
