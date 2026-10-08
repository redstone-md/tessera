# Tessera

A modular desktop environment for Windows. Its primary product is a customizable native shell with conventional floating-window behavior. Automatic tiling is an optional capability, not the baseline interaction model.

The presentation target is full Seelen UI behavior and appearance, independently implemented on the native stack. Current missing modules remain work to do, not exclusions from that target; [the native presentation decision](docs/adr/0005-native-presentation.md) records the reference and acceptance contract.

## Terminology

**Shell**:
The environment through which the user launches applications, switches windows, and interacts with the desktop.
_Avoid_: "window manager" as the name for the whole environment.

**Shell activation**:
An explicit choice to use Tessera instead of the default desktop shell for a user's sign-in session.
_Avoid_: treating installation or a theme change as activation.

**Desktop session**:
A temporary use of Tessera's dock, toolbar, and launcher within the current Windows session, without changing which shell starts at the next sign-in. Leaving the session restores the prior taskbar presentation.
_Avoid_: "shell activation" for ordinary launch.

**Shell recovery**:
Returning the user to their previous working shell when Tessera is stopped, fails, or is no longer wanted.
_Avoid_: treating recovery as dependent on the failed shell's UI.

**Window management**:
Organizing the position, size, and interaction of application windows.
_Avoid_: using "shell" as a synonym for window management.

**Conventional desktop**:
A working environment with floating windows and ordinary dragging, resizing, minimize, maximize, and application fullscreen. Automatic tiling requires an explicit choice.
_Avoid_: using "floating" to mean a lack of window controls.

**Managed window**:
An application window for which Tessera determines placement rules. Its identity belongs to the current session, not to a persisted application profile.

**Observed window**:
A window whose details were obtained from the current working environment. Detecting a window does not mean Tessera manages its placement.
_Avoid_: using "managed window" as a mandatory synonym.

**Desktop snapshot**:
A set of observed facts about screens and windows, collected in a single pass. It describes an observation, not a guaranteed simultaneous state of all objects.
_Avoid_: "placement plan", "atomic snapshot".

**Monitor**:
A screen with its own position in the overall desktop space.
_Avoid_: using "workspace" as a synonym for monitor.

**Work area**:
The part of the screen available for placing windows after space reserved for panels is excluded.
_Avoid_: "full screen size".

**Workspace**:
A collection of windows the user treats as a separate working environment on a monitor.
_Avoid_: using Windows virtual desktops as a mandatory synonym.

**Monitor group**:
Monitors on which the user chooses to switch workspaces together instead of independently.

**Layout**:
A rule for distributing windows across the work area.
_Avoid_: "theme", "workspace".

**Placement plan**:
A proposed set of window positions and sizes, not yet applied to the desktop.
_Avoid_: "current window state".

**Tiled window**:
A window whose position and size are determined by the layout.

**Floating window**:
A window whose position and size are not determined by the layout.
_Avoid_: "always on top".

**Fullscreen window**:
A window occupying the screen in fullscreen mode and, by default, excluded from automatic placement.
_Avoid_: using "maximized window" as a mandatory synonym.

**Main window**:
The first tiled window in the main-and-stack layout, occupying the main area.
_Avoid_: "active window"; focus and position in the layout are independent.

**Shell module**:
A replaceable part of the environment, such as a taskbar, launcher, or desktop.
_Avoid_: using "plugin" as a mandatory synonym; a module may ship with Tessera.

**Widget**:
A shell module presenting a focused piece of information or interaction, optionally with a popup. A widget's appearance or configuration does not grant extra permissions.
_Avoid_: treating arbitrary web-script execution as necessary for widget behavior.

**Application**:
A launchable program with an identity suitable for saved preferences. It may have several windows or processes.
_Avoid_: equating an application identity with a window title or process identifier.

**Dock pin**:
A user's choice to retain an application entry in the dock even when it has no observed windows. It is independent of membership in the launcher's Favorites view.
_Avoid_: "favorite" as a synonym for a dock pin.

**Favorite application**:
An application the user has chosen for the launcher's Favorites view, independently from the dock. A saved choice may outlive installation and does not itself authorize a launch.
_Avoid_: "pinned application" when referring to launcher favorites.

**Favorite order**:
The saved sequence of launcher favorite identities, including choices that are currently unavailable. A drag preview is transient and becomes saved order only after persistence succeeds.
_Avoid_: equating a drag preview with an applied preference or tying this order to dock pins.

**Launcher display mode**:
A saved choice between a centered, windowed launcher and a monitor-filling launcher overlay. It does not change the display mode or placement rules of application windows.
_Avoid_: equating the monitor-filling overlay with exclusive fullscreen or a window backend's fullscreen flag.

**Known folder**:
An OS-resolved location such as Desktop or Documents, whose current native identity and availability come from the platform adapter rather than an assembled path.
_Avoid_: equating a known folder with a Favorite application-group folder or treating its displayed label as an open target.

**Favorite folder**:
A named launcher grouping of Favorite applications, independent of OS directory contents.
_Avoid_: using it as a synonym for a known folder or an application group of observed windows.

**Folder open intent**:
An explicit request to open one confirmed known-folder target. Acceptance means the native Shell accepted dispatch, not that a file-manager window became visible.
_Avoid_: treating pending intent, a Ready label or automatic retry as a completed open.

**User identity**:
The current OS user's identity displayed by shell presentation; unavailable identity remains unavailable, and a generic profile image is not an account photograph.
_Avoid_: treating a local user identity as a saved configuration profile, or implying a connected cloud account from a local user name.

**Application group**:
A dock entry representing related windows of one application.
_Avoid_: grouping unrelated windows merely because they share a hosting executable.

**Popup**:
A transient shell surface for details or actions, such as a calendar, device selector or context menu.
_Avoid_: treating it as a managed application window.

**Civil date**:
A validated Gregorian year, month and day, without a clock time, timezone or UTC instant. The Calendar's real Today is acquired from the current OS-local date.
_Avoid_: deriving Today by parsing displayed clock text, or substituting an arbitrary date when acquisition fails.

**Calendar selection**:
The day selected within the Calendar's transient state, independent of the displayed month/year and real Today. Browsing or selecting does not set the system clock or save a preference.
_Avoid_: equating navigation, displayed date, selection and current OS date.

**Audio endpoint**:
An independently identified destination for output sound or source for input sound, with its own volume and mute state. It is not an application's audio session.
_Avoid_: treating every endpoint as a distinct physical device or an application-volume control.

**Default multimedia endpoint**:
The currently selected endpoint for ordinary media output or input. A separate default may serve communications.
_Avoid_: equating the multimedia default with every default-device role.

**Audio intent**:
An explicit user choice of desired volume or mute state for an identified endpoint. Pending intent is not confirmed audio state.
_Avoid_: treating an accepted request or an optimistic display as proof of an applied change.

**Theme**:
Appearance settings for shell surfaces, including colors, typography, icons, spacing, and motion. A theme does not grant system permissions or automatically restyle foreign application windows.
_Avoid_: using "profile" or "shell module" as a mandatory synonym.

**Plugin**:
An installable extension that provides additional capabilities or replaces a shell module.

**Permission**:
An explicitly granted right of an extension to perform certain actions or access certain data.
_Avoid_: unconditional trust in the entire extension.

**Profile**:
A saved collection of behavior and appearance settings and selected shell modules.
_Avoid_: using "workspace" as a synonym for configuration.
