# Conventional desktop behavior and native shell customization

Tessera's primary product is a customizable desktop shell, not a tiling-first window manager. Its baseline is familiar Windows, KDE, or GNOME-style interaction: floating application windows, mouse dragging and resizing, minimize, maximize, and application fullscreen. Tiling requires an explicit choice. `WindowMode::default()` is `Floating`; the existing `MainStack` demo deliberately exercises an optional layout algorithm and does not define the product's default workflow.

## Customization boundary

Profiles may combine different docks, panels, launchers, desktop surfaces, widgets, themes, and motion settings. A macOS-inspired dock-and-top-panel arrangement is a presentation choice, not a change to application-window semantics or permission policy. Assets still need suitable licenses; a visual reference does not authorize copying proprietary icons or fonts.

First-party and extension presentation must be rendered by a native Rust UI host, without WebView, Electron, or an HTML/CSS/JavaScript UI pipeline. Extensions provide controlled UI descriptions and events through the restricted host described in [the plugin decision](0002-restricted-plugins.md), not arbitrary native libraries or a browser renderer. Appearance and module selection remain separate from trusted permissions and recovery.

Animation of Tessera-owned surfaces belongs to presentation. Animation or restyling of foreign application windows, Windows notification integration, and system-flyout replacement are separate platform capabilities requiring explicit scope, permissions where applicable, compatibility handling, and Windows validation. A custom notification panel does not by itself intercept or suppress native toasts. Tessera is not a replacement for DWM and does not promise universal control of other applications' rendering.

## Reference and trade-off

[Seelen UI's official product page](https://seelen.io/apps/seelen-ui/customizable-shell) is the visual reference for Tessera's default dock, toolbar, and application menu, as well as the functional reference for broader widgets, themes, and notification panels. Its [upstream README](https://github.com/eythaann/Seelen-UI) documents a required WebView runtime and independently selectable components, including its tiling manager. Tessera adopts the established presentation and modular product pattern, not the web stack or implementation code. The pinned standard-theme measurements are in [the presentation decision](0005-native-presentation.md).

Native-only presentation gives up browser-based widget and CSS theme reuse. It requires a native customization model and a maintained toolkit with suitable Windows support. [Slint has been selected](0005-native-presentation.md); the plugin UI format remains unfrozen. Native presentation still requires DPI, keyboard, accessibility, animation-performance, resource-usage, and dependency-licensing validation.

The first visible layer was a small native panel alongside Explorer. Its public alpha observes application candidates, filters titles, and switches foreground only on explicit input; it does not automatically rearrange windows. The working source now layers the Seelen-style desktop presentation and a reversible, supervised primary-taskbar handoff onto that foundation. Appearance preferences remain separate from system integration, and ordinary presentation does not change the sign-in shell. Broader customization and restricted extensions grow on the working surface; persistent opt-in shell activation remains gated by [recovery](0003-shell-activation-and-recovery.md). This is not yet a complete graphical shell.
