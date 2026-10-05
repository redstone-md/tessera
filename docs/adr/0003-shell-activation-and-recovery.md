# Shell replacement is explicitly enabled and has a recovery path

Target systems are supported releases of Windows 11, including Home and Pro. Tessera eventually replaces the Explorer shell, but not the DWM, and does not have to replace the file manager: Explorer remains the default, and the user will be able to choose a different program.

Shell replacement requires a separate user action after the onboarding stage. Before it is enabled, Explorer recovery, emergency exit, and failure behavior must be verified; safety must not depend on a third-party plugin. The first working stage evolves alongside Explorer and does not change the sign-in configuration.

[Microsoft Shell Launcher](https://learn.microsoft.com/en-us/windows/configuration/shell-launcher/) is available only in Enterprise, Education, and IoT Enterprise editions. For Home/Pro, the necessary undocumented integrations are acceptable, but only inside an isolated platform module with compatibility checks and safe failure; this is not a promise of identical implementation across all editions. Code injection into Explorer, disabling UAC, and running the whole shell elevated are not the baseline integration approach.

The activation and recovery mechanism is not yet implemented. Windows updates, launching the chosen file manager, and the possible reappearance of the Explorer shell require verification in a real Windows session.
