# Tessera as the shell of the session

Tessera normally runs beside Explorer: it hides the taskbar of Explorer while it runs and gives it back when it quits. It can also be the shell Windows starts at sign-in, in the place of Explorer. This page says how to do that, how to undo it, and what changes.

**Status:** everything here is written against the Windows API and checked to build for Windows; it has not been run on Windows yet. Try it on a virtual machine or a spare account first, and keep the way back below at hand. A shell that fails at sign-in leaves a session with no taskbar and no desktop.

## Making Tessera the shell

From a console (Windows Terminal, the command prompt or PowerShell), as the user whose shell it is to be:

```
tessera shell install
```

This writes `"<path of tessera.exe>" --shell` as the `Shell` value of `HKEY_CURRENT_USER\Software\Microsoft\Windows NT\CurrentVersion\Winlogon`. It needs no administrator and changes nothing for other users. It takes effect at the next sign-in. The **shell** panel (the popup `shell`, or the widget `shell` in a popup or card of its own) does the same with a button.

`tessera shell status` (or `tessera shell`) prints the values Windows reads and which program it will start:

- the `Shell` value of the user, and that of the machine (`HKEY_LOCAL_MACHINE\...\Winlogon`);
- the policy "Custom User Interface" (a `Shell` value under `Software\Microsoft\Windows\CurrentVersion\Policies\System`, of the user or of the machine), which comes before both, and which only an administrator can change;
- the mapping of `system.ini` under `HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion\IniFileMapping\system.ini\boot`. A mapping that starts with `USR:` reads the value of the user first. Windows comes with `SYS:`, which names the value of the machine; Windows is widely reported to read the value of the user all the same. That is not verified by Tessera. If it does not on your computer, an administrator has to change the mapping or the value of the machine; Tessera does neither.

## Giving the shell back to Explorer

```
tessera shell revert
```

This removes the value of the user, so Windows starts what the machine names, which is Explorer unless an administrator changed it. Where the machine itself names Tessera, the value of the user is set to `explorer.exe` instead. It takes effect at the next sign-in.

From a session where Tessera is the shell and nothing answers: press Ctrl+Alt+Del, open Task Manager, choose **File > Run new task**, run `tessera shell revert` (with the full path of `tessera.exe` if it is not on the `PATH`), then sign out. To get Explorer at once, also run `explorer.exe`; since the value names Tessera, Explorer may come only as a file manager window until the next sign-in.

## Ways out that Tessera gives itself

- **Ctrl+Shift at sign-in**: holding Ctrl and Shift while Tessera starts as the shell starts Explorer as the shell instead, for this session only.
- **Safe Mode**: in Safe Mode, Tessera does the same.
- **Three starts within a minute**: Tessera writes down each start as the shell in `%LOCALAPPDATA%\tessera\shell-starts.txt` before it does anything else. When it starts for the third time within 60 seconds, it removes itself as the user's shell as `tessera shell revert` does, shows the windows its workspaces had hidden, starts Explorer, and ends. The next time Tessera starts, the problems list says so. A start that takes over from another Tessera (Restart) is not counted. This only helps where something starts Tessera again after it ends: Winlogon is meant to start the shell again when it ends unexpectedly (`AutoRestartShell`), but that is not verified for a shell other than Explorer.
- **Ctrl+Win+Alt+E**: starts Task Manager, then hands the session to Explorer: Explorer starts as the shell once Tessera is gone, and Tessera quits. **Ctrl+Win+Alt+R** starts Tessera again. Both are held by the thread of the shell, not by the user interface, so they work while the user interface does not answer; if Tessera has not quit 10 seconds later, it ends itself. `[shell] emergency_keys = false` leaves both keys free.
- **Quitting**: Tessera that quits as the shell (the **Quit** of the settings, `tessera quit`), whose surfaces fail to open, or that panics on the thread of its user interface, other than at the end of the session or to start again, starts Explorer as the shell after it is gone, so the session keeps a shell. `[shell] quit_to_explorer = false` leaves the session without one. Whether Windows then starts Tessera again by itself (Winlogon's `AutoRestartShell`) is not verified.

Starting Explorer as the shell works like this: while the values name Tessera, the value of the user is set to `explorer.exe` for the moment Explorer takes to show its taskbar (at most 30 seconds), and put back afterwards, so Tessera is still the shell at the next sign-in. A policy that names the shell cannot be gone around; Explorer is started all the same and may come only as a file manager window.

## What Tessera does as the shell

- It makes the desktop window, `Progman` ("Program Manager"), over all displays, with `SHELLDLL_DefView` and `WorkerW` in it as Explorer of Windows 11 24H2 lays them out, so programs that look for the desktop find it and Tessera's own wallpapers go behind where the icons would be. It draws the wallpaper of Windows with its style (`PaintDesktop`). It is the shell window of Windows (`SetShellWindowEx`), so it stays below every other window. Alt+F4 on the desktop closes nothing: it runs the action `power-dialog` where another part of Tessera brings one, and until then opens the popup that holds the `power` widget (the launcher by default); where there is neither, the problems list says so.
- It is the task manager of the shell (`SetTaskmanWindow`): Ctrl+Esc and the Windows key, where no Start menu takes it, open the popup of `windows_key`, or the launcher where none is named. The keys of the volume and of media that the window in front does not take change the volume of the default output (by 2 in 100, ending muting) and go to the media application, as Explorer does; `[shell] media_keys = false` leaves them alone. "End task" of the shell asks the window to close and ends it if it does not answer.
- Minimized windows go out of sight instead of lying at the bottom of the screen as small title bars, as with Explorer.
- Once the bars are open, it signals the events Windows and programs wait for from a shell that is ready (`msgina: ShellReadyEvent`, `ShellDesktopSwitchEvent`).
- Its workspaces are Tessera's own, four unless `[workspaces] own` says how many: the virtual desktops of Windows are part of Explorer.
- It hides no taskbar of Explorer, as there is none.

In both modes, Tessera hears the shell hook of Windows (windows made, destroyed, activated, flashing), the locking of the session, and the end of the session: it asks Windows to be ended after the applications, and when the session ends its own thread shows the windows the workspaces hid (from the record they keep, so this works also when the user interface does not answer), then waits until the user interface has quit and given back the space the bars held before it lets Windows go on, at most four seconds in all. When the user changes the environment variables, programs Tessera starts afterwards get the new ones.

## What does not work without Explorer

- The notification area and the AppBar messages are Explorer's; Tessera takes them over in another part of its work (until then, icons of the tray may not show).
- Programs started at sign-in (the `Run` keys, the Startup folder) are started by the shell; that is another part of Tessera's work. If Explorer is started later in the same session, it may start them again.
- Settings > Personalization and some programs call into Explorer (the wallpaper per display, pins of the Start menu, "Show in folder", Control Panel and virtual folders). Explorer still works as a file manager window for these.
- The virtual desktops of Windows, Task View, Win+Tab, the Start menu and the search of Windows, the input indicator of CJK input methods, the touch keyboard button, Win+PrtScn and other keys Explorer answers.
- Whether user32 honours the undocumented `SetShellWindowEx`, `SetProgmanWindow` and `SetTaskmanWindow` on every version of Windows 11 is not verified; one that is missing is reported in the problems list.
