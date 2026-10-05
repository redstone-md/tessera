# Native presentation uses Slint

The first presentation module uses [Slint](https://slint.dev/) 1.18.1 with its native winit backend, software renderer, and operating-system accessibility integration. The `.slint` descriptions compile into Rust; they are not HTML, CSS, or a WebView interface. The host uses first-party standard widgets instead of rebuilding focus, button, or accessibility mechanics.

Slint and Iced were evaluated as maintained native Rust toolkits. Slint was selected for its declarative custom-component model, explicit accessibility support, and stable 1.x interface, which fit a customizable shell. This costs a larger dependency graph and raises the workspace minimum Rust version to 1.92, required by the current Slint release. Keeping an older toolkit solely to preserve the earlier 1.85 minimum was rejected.

Default toolkit features are disabled. The host enables only `std`, `backend-winit`, `renderer-software`, `accessibility`, and `compat-1-18`. Qt, browser rendering, GPU renderers, tray support, live inspection servers, and system-testing servers are not enabled. Software rendering avoids a GPU-driver requirement for this small surface; idle behavior remains event-driven. A hardware renderer may later use Slint's existing renderer interface after measured compatibility and animation testing, without changing the application-facing presentation seam.

## Presentation seam

`tessera-ui` receives portable `PanelSnapshot`/`PanelWindow` data from a `DesktopHost` adapter. It does not depend on Windows or receive native handles. The application host owns the ephemeral activation-key registry and preference storage. Startup and manual Refresh use one asynchronous path; failed refreshes retain stale data and pause actions. Search projects the retained full view without observing again, preserving opaque keys. Activation runs directly on explicit UI input, because foreground eligibility depends on user input; Windows owns its validation and denial policy. Preference preview is independent of an explicit Save. Weak component handles deliver worker results without retaining a closed window.

The generated Slint module is isolated from handwritten Rust. Win32 integration remains in the platform module; any compiler-generated toolkit unsafe code is not permission to add handwritten FFI to presentation. Selecting Slint does not choose or freeze a plugin runtime, third-party UI protocol, or shell-activation mechanism.

## Verification and limits

The official [preliminary headless testing backend](https://docs.slint.dev/latest/docs/rust/i_slint_backend_testing/) is a development-only dependency. Its version must exactly match Slint and the compiler, so all three are pinned to 1.18.1 and upgraded together. It tests callbacks and UI state without a display; it does not render pixels or validate Windows accessibility, focus, DPI, or antivirus behavior. The production binary must not include its testing or inspection facilities.

Development builds emit static widget metadata for headless element queries; release builds omit it. This is compiler metadata, not a running inspection server. Controller and text-safety tests do not need that metadata; the headless accessible-button assertion runs in debug builds. Real Windows accessibility remains an interactive release check.

The alpha is an ordinary window-switching utility alongside Explorer, not yet a dock or taskbar replacement. A private unsigned tester package is an explicit exception to consumer distribution, not evidence of Windows 11 or antivirus approval. Interactive testing and the [distribution trust gates](../distribution-and-trust.md) remain prerequisites for a consumer release.
