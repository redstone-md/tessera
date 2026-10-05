# Tessera

A modular desktop environment for Windows. It combines window management with replaceable shell elements, keeping the user in control of their working environment.

## Terminology

**Shell**:
The environment through which the user launches applications, switches windows, and interacts with the desktop.
_Avoid_: "window manager" as the name for the whole environment.

**Window management**:
Organizing the position, size, and interaction of application windows.
_Avoid_: using "shell" as a synonym for window management.

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

**Plugin**:
An installable extension that provides additional capabilities or replaces a shell module.

**Permission**:
An explicitly granted right of an extension to perform certain actions or access certain data.
_Avoid_: unconditional trust in the entire extension.

**Profile**:
A saved collection of behavior and appearance settings and selected shell modules.
_Avoid_: using "workspace" as a synonym for configuration.
