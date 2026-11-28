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

- It makes the desktop window, `Progman` ("Program Manager"), over all displays, with `SHELLDLL_DefView` and `WorkerW` in it as Explorer of Windows 11 24H2 lays them out, so programs that look for the desktop find it and Tessera's own wallpapers go behind where the icons would be. It draws the wallpaper of Windows with its style (`PaintDesktop`). It is the shell window of Windows (`SetShellWindowEx`), so it stays below every other window. Alt+F4 on the desktop closes nothing: it runs the action `power-dialog`, the dialog to shut down (where `windows_keys` is on, the keyboard hook takes Alt+F4 on the desktop first and opens the same dialog); without that action it opens the popup that holds the `power` widget (the launcher by default), and where there is neither, the problems list says so.
- It is the task manager of the shell (`SetTaskmanWindow`): Ctrl+Esc and the Windows key, where no Start menu takes it, open the popup of `windows_key`, or the launcher where none is named. The keys of the volume, of the microphone and of media that the window in front does not take run the actions `volume` and `media` (the volume of the default output by 2 in 100, a step up ending muting; the media application), whose flyouts show them, as Explorer does; `[shell] media_keys = false` leaves them alone. "End task" of the shell asks the window to close and ends it if it does not answer.
- Minimized windows go out of sight instead of lying at the bottom of the screen as small title bars, as with Explorer.
- Once the bars are open, it signals the events Windows and programs wait for from a shell that is ready (`msgina: ShellReadyEvent`, `ShellDesktopSwitchEvent`).
- Its workspaces are Tessera's own, four unless `[workspaces] own` says how many: the virtual desktops of Windows are part of Explorer.
- It hides no taskbar of Explorer, as there is none.
- Task View and Win+Tab work in shell mode through Tessera: the action `taskview`, which Win+Tab does where `windows_keys` is on, spreads the windows of each display as live pictures (see "Task View" in FEATURES.md), with the workspaces of Tessera's own along the top.
- The input indicator of CJK input methods and the touch keyboard button are widgets of Tessera's own, `keyboard` and `touch-keyboard` (see "The input indicator of Explorer" in FEATURES.md).
- It draws the icons of the desktop itself, in a window per display inside its `SHELLDLL_DefView`, which also takes the clicks and the menu of the desktop where the icons are hidden (see "The icons of the desktop" in FEATURES.md and `[desktop]` in CONFIGURATION.md).
- Its `Shell_TrayWnd` stands where its taskbar stands, with the windows inside it that programs look for (`TrayNotifyWnd`, `TrayClockWClass`, `SysPager`, `ToolbarWindow32`, `ReBarWindow32`, `MSTaskSwWClass`, `MSTaskListWClass`), and each other display with a bar has a `Shell_SecondaryTrayWnd` there; a program that asks the taskbar to hide (`ABM_SETSTATE`) has Tessera's taskbar hide until Tessera ends; and the numbered commands scripts and `Shell.Application` post to the taskbar (`WM_COMMAND`, as for `MinimizeAll`, `TileVertically` or the Run dialog) are answered. See "The windows of the taskbar where programs look for them" and "The numbered commands" in FEATURES.md.

In both modes, Tessera hears the shell hook of Windows (windows made, destroyed, activated, flashing), the locking of the session, and the end of the session: it asks Windows to be ended after the applications, and when the session ends its own thread shows the windows the workspaces hid (from the record they keep, so this works also when the user interface does not answer), then waits until the user interface has quit and given back the space the bars held before it lets Windows go on, at most four seconds in all. When the user changes the environment variables, programs Tessera starts afterwards get the new ones.

## Services for other programs

Explorer hosts services that other programs talk to. As the shell, Tessera hosts these, as `[shell_services]` in CONFIGURATION.md sets; all of them are written against the Windows API and checked to build for Windows, and none was run on Windows yet.

- The list of folder windows, `ShellWindows`, with late binding: `Shell.Application.Windows()` in scripts, `_NewEnum`, each window of Tessera Files as an `IWebBrowser2` (`LocationURL`, `LocationName`, `Navigate`, `Navigate2`, `Refresh`, `Quit`, `Document`), and the desktop's view for "ShellExecute from the desktop": `FindWindowSW(SWC_DESKTOP)`, its browser, its view and the view's background, whose `Application` is the Shell's `Shell.Application` made in Tessera's process, so an installer that starts a program without elevation through `IShellDispatch2::ShellExecute` has it started by Tessera. The same view is the desktop's `IFolderView` (the items of the desktop and its folder; where the icons are is not told yet). To try on Windows: whether other processes reach the list Tessera registers, `(New-Object -ComObject Shell.Application).Windows()` in PowerShell, and an installer that starts its program at the end without elevation.
- The program manager's DDE (`PROGMAN`, and `Folders` and `AppProperties`): old installers make groups and shortcuts of the Start menu with `[CreateGroup(...)]`, `[AddItem(...)]` and the other commands of the program manager, which Tessera executes in the Start menu of the user, or of all users where the user may write it. To try on Windows: an installer of the 1990s, or `[CreateGroup(Test)][AddItem(notepad.exe,Notepad)]` sent with a DDE client.
- The objects registered under `ShellServiceObjects` (off by default, `service_objects`): made on a thread of their own, opened once the desktop is ready and closed at the end of the session, within its bounded wait. Some of these are Windows' own and may expect Explorer; one that fails is reported and left out. `SharedTaskScheduler` has had no effect since Windows Vista and is not hosted.
- A record of the messages the desktop and the task manager window leave unanswered, in `%LOCALAPPDATA%\tessera\shell-messages.log` (`log_messages`, on by default this release): it shows which protocols programs still speak to the shell, so that testers can send it.
- The server of the Shell's change notifications (`SHChangeNotifyRegister`), which file dialogs and Office use to see new files at once, is not served yet. Its delivery is written after ReactOS's shell32, whose protocol Windows 11 may not speak: the windows that would answer it wait until the record above shows the desktop window being asked for the server (`WM_USER+25`). Until then programs see changes as they did without Explorer, by looking again, and `change_server = true` only says so.

## What does not work without Explorer

- The notification area and the AppBar messages are Explorer's; without Explorer, Tessera answers them itself (see "The notification area and the edge bars without Explorer" in FEATURES.md), which was not run on Windows yet.
- Programs started at sign-in (the `Run` keys, the Startup folder) are started by the shell; Tessera starts them once per session as `[startup]` in `config.toml` sets (see FEATURES.md). If Explorer is started later in the same session, it may start them again.
- Settings > Personalization and some programs call into Explorer (the wallpaper per display, except for programs that ask COM for `IDesktopWallpaper`, which Tessera answers, see `[wallpaper]` in CONFIGURATION.md; pins of the Start menu, "Show in folder", Control Panel and virtual folders). Explorer still works as a file manager window for these.
- The virtual desktops of Windows, the Start menu and the search of Windows, and the keys of Windows that Tessera leaves to Windows: Win+H (voice typing), Win+W (Widgets) and Win+period and Win+semicolon (the emoji panel), which belong to components of Windows that may or may not answer without Explorer (not known yet; `emoji-panel` can be bound to Win+period instead). Tessera's actions bring the other keys Explorer answers where `windows_keys` is on, such as Win+X, Win+P, Win+I, Win+PrtScn, Win+Alt+D, Win+Shift+Up, Win+Plus, Win+Ctrl+Enter, Win+C and Win+Ctrl+F; see "The keys of Windows without Explorer" in CONFIGURATION.md.
- Whether user32 honours the undocumented `SetShellWindowEx`, `SetProgmanWindow` and `SetTaskmanWindow` on every version of Windows 11 is not verified; one that is missing is reported in the problems list.
- "Show in folder" of other programs (`SHOpenFolderAndSelectItems`) asks the list of folder windows of the Shell, which Explorer serves. As the shell, Tessera serves that list itself with the windows of Tessera Files and the desktop (see below and "Services for other programs"); whether Windows reaches it was not verified, and where it does not, "Show in folder" opens nothing until Explorer runs.
- AutoPlay is Explorer's; as the shell, Tessera does it itself (`[files] autoplay`, see CONFIGURATION.md).
- Settings > Personalization > Taskbar > Other system tray icons asks Explorer's notification area through `CLSID_TrayNotify` (the interface `ITrayNotify`), which is undocumented and which Tessera does not serve, so that page cannot read or change the icons Tessera shows. Tessera reads the choices that page wrote while Explorer ran (`HKCU\Control Panel\NotifyIconSettings`); its own are `[tray]` and Settings > Tray.

## Tessera Files as the file manager

**Status:** written against the Windows API and checked to build for Windows; not run on Windows yet.

`tessera files install` has other programs open the folders and drives of the file system in Tessera Files, for the user who runs it (no administrator, nothing changed for other users); `tessera files revert` gives that back and `tessera files status` prints what opens them now. The action `files-default` (`on` or `off`) does the same from a key or a menu. What it writes, and how it keeps what was there, is in [CONFIGURATION.md](CONFIGURATION.md#the-file-manager).

What goes to Tessera Files:

- a folder or a drive opened by a double click in another program, by `ShellExecute` with the verbs `open`, `explore` or `opennewwindow`, by the Run dialog or by a shortcut to a folder;
- `tessera files` with any of the command lines of `explorer.exe` that name a folder of the file system (`/select,`, `/e,`, `/n`, `/root,`, `/separate`, `shell:` names of known folders, This PC, Home, `search-ms:` with a folder, `file:` addresses);
- with `[files] own = true`, the folders the surfaces of Tessera open. `platform::reveal`, which shows a file in its folder in Tessera Files, is what Open file location of the Start menu, the launcher and the dock uses; the file manager's own "Show in Explorer" stays with Explorer on purpose.

What still opens in Explorer:

- the parts of virtual folders that are no lists of items: the applets of Control Panel with views of their own (System, Power Options, Network and Sharing Center, Ease of Access) open with their default verb, which starts Explorer or Settings, and on Windows 11 Devices and Printers goes to Settings. Control Panel itself, the libraries, Network, phones and cameras, ZIP files, the Gallery and the other places that list items open in Tessera Files, which hands a place it cannot read to Explorer. They belong to the class `Folder`, which `tessera files install` leaves alone on purpose, so a program that opens one through the Shell still gets Explorer;
- whatever a program opens by starting `explorer.exe` itself, as `explorer.exe /select,<file>`: `explorer.exe` is not intercepted (no Image File Execution Options, no replacement of the program);
- "Show in folder" (`SHOpenFolderAndSelectItems`) beside Explorer, which goes to Explorer's own list of windows. As the shell, Tessera registers that list (`CLSID_ShellWindows`) on its shell thread, with the windows of Tessera Files in it: a program that asks it finds a window of Tessera Files at the folder and has it choose the file, which brings the window to the front with the file chosen once its folder was read; the list follows the folder each window shows as it goes elsewhere. Whether `SHOpenFolderAndSelectItems` reaches a list registered this way, while Windows names Explorer's for the class, was not verified, nor what it does where no window shows the folder (it is expected to open the folder, which goes to Tessera Files after `tessera files install`). The list answers scripts too (`Shell.Application.Windows()`), by late binding, and each window's `Navigate` sends it elsewhere (see "Services for other programs" above).

Explorer keeps working as a file manager in shell mode: `explorer.exe <folder>` opens a window of its own, without becoming the shell.

When no Tessera runs, the command the verbs run (`"<tessera.exe>" files "%1"`) starts Tessera itself (the same program, without arguments, so as the shell where Explorer is not running, beside Explorer otherwise) and hands the folder over as soon as Tessera listens, waiting at most 15 seconds. That Tessera is started on its own: without a console, in a process group of its own, out of the job of the program that opened the folder where the job lets it go, and in the folder of `tessera.exe`, so that it does not hold the drive or share of the folder. Several folders opened at once start one Tessera between them: only the command that holds the mutex `Local\tessera-files-start` starts it, and the others wait for it to listen. Where another Tessera runs but does not listen yet, as one that is starting, the command that starts Tessera waits 3 seconds for it first (the other commands opened at once count as such too, so they wait as long). Relative paths are made absolute before they are handed over, as Tessera works in another folder. Only where Tessera cannot be started or never listens, or the words are longer than the 64 KiB Tessera takes at once, does Explorer get the folder, with `TESSERA_FILES_FALLBACK` set to the time it was handed: a command that Explorer runs with that variable within 30 seconds, because Explorer handed the folder back to the verbs, still hands it to a Tessera that listens by then, and otherwise shows a message box that says so instead of starting anything, so the two cannot hand a folder to each other for ever. Later on the variable counts for nothing, so folders opened from that Explorer, or from what it starts, go to Tessera as any other. An Explorer that was running already gets the folder from the one started without the variable; a folder it hands back is not caught this way.
