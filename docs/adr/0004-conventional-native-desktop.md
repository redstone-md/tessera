# Conventional desktop behavior and native shell customization

Tessera's primary product is a customizable desktop shell, not a tiling-first window manager. Its baseline is familiar Windows, KDE, or GNOME-style interaction: floating application windows, mouse dragging and resizing, minimize, maximize, and application fullscreen. Tiling requires an explicit choice. `WindowMode::default()` is `Floating`; the existing `MainStack` demo deliberately exercises an optional layout algorithm and does not define the product's default workflow.

## Customization boundary

Profiles may combine different docks, panels, launchers, desktop surfaces, widgets, themes, and motion settings. A macOS-inspired dock-and-top-panel arrangement is a presentation choice, not a change to application-window semantics or permission policy. Assets still need suitable licenses; a visual reference does not authorize copying proprietary icons or fonts.

First-party and extension presentation must be rendered by a native Rust UI host, without WebView, Electron, or an HTML/CSS/JavaScript UI pipeline. Extensions provide controlled UI descriptions and events through the restricted host described in [the plugin decision](0002-restricted-plugins.md), not arbitrary native libraries or a browser renderer. Appearance and module selection remain separate from trusted permissions and recovery.

Animation of Tessera-owned surfaces belongs to presentation. Animation or restyling of foreign application windows, Windows notification integration, and system-flyout replacement are separate platform capabilities requiring explicit scope, permissions where applicable, compatibility handling, and Windows validation. A custom notification panel does not by itself intercept or suppress native toasts. Tessera is not a replacement for DWM and does not promise universal control of other applications' rendering.

## Reference and trade-off

[Seelen UI's official product page](https://seelen.io/apps/seelen-ui) provides a functional reference: customizable dock and toolbar, launcher, widgets, themes, and notification panels. Its [upstream README](https://github.com/eythaann/Seelen-UI) documents a required WebView runtime and independently selectable components, including its tiling manager. Tessera adopts the modular product pattern, not the web stack or implementation code.

Native-only presentation gives up browser-based widget and CSS theme reuse. It requires a native customization model and a maintained toolkit with suitable Windows support. A toolkit has not been selected, and a plugin UI format is not frozen before a working first-party surface exists. Toolkit evaluation must cover DPI behavior, keyboard and accessibility support, animation performance, resource usage, and dependency licensing.

The next visible layer is a small native dock/panel alongside Explorer, driven by the existing read-only observation boundary. It must work without automatically rearranging application windows. Broader customization and restricted extensions grow on that working surface; opt-in shell activation remains gated by [recovery](0003-shell-activation-and-recovery.md). The current build remains a CLI foundation, not an implemented graphical shell.
