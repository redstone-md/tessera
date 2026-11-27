# Configuration

Everything a surface shows is a widget provided by a module. The compiled-in modules provide the widgets `clock`, `calendar`, `label`, `launcher`, `apps`, `power`, `notifications`, `settings`, `media`, `media-controls`, `network`, `connections`, `radio-switch`, `wifi`, `bluetooth`, `bluetooth-button`, `volume`, `volume-slider`, `microphone-slider`, `volume-mixer`, `audio-outputs`, `brightness`, `brightness-slider`, `cpu`, `memory`, `battery`, `disk`, `network`, `files`, `command`, `tray`, `problems`, `problem-list`, `tiling`, `quit`, `user`, `profile`, `places`, `workspaces`, `overview`, `focused`, `keyboard`, `appearance`, `desktop`, `plan`, `hdr`, `trash`, `update`, `marketplace` and `windows`; extensions add more.

The layout is read from `%APPDATA%\tessera\config.toml`. It lists the surfaces — screen edge, thickness, all displays or only the primary one, and whether the surface slides away while the pointer is elsewhere (`hide = true`, or `hide = "overlap"` to do so only while a window lies in its place; it then reserves no screen space and comes back when the pointer reaches that edge of the display) — and the widgets in the start, center and end of each. A widget is written as its name, or as a table that adds options the widget understands (the clock takes a `format`, the calendar a `week_start`, a label an `icon` and a `text`, the tray a `limit` and whether it shows `tips`) and a `popup` to open when the widget is clicked. Popups are declared in the same file as a name, a size and a list of widgets, so any widget, including one from an extension, can open a popup or be placed in one. One popup is open at a time, and popups cannot be resized. Without that file the [built-in layout](../crates/tessera/src/default_config.toml) is used; it documents the format. A file that fails to parse is ignored as a whole, and an unknown widget name is skipped; both are reported on standard error, which only a debug build started from a terminal shows.

The `tray` widget shows no description for an icon whose application draws its own (one of version 4 that did not ask for the description with `NIF_SHOWTIP`); the application is told when the pointer comes onto the icon and leaves it instead, whatever `tips` says. Where no taskbar of Explorer exists, as when Tessera is the shell, the notification area and the screen edges that programs reserve are served by Tessera itself; there is nothing to configure for that, and the room the bars leave to windows follows from the bars of the layout and of other programs.

The layout file and the theme file it names are checked every second, and a change is applied about two seconds after the file was written, without a restart: surfaces are added, removed and resized, their widgets are built again and slide in, and an open popup closes. A changed file that fails to parse changes nothing and is reported the same way. Changes to the `[icons]` table and to the settings of extensions go the same way but have not been tried on a running shell yet. Extension files and icon image files are not watched.

 If the environment variable `TESSERA_CONFIG_DIR` is set, that directory is used instead of `%APPDATA%\tessera` for everything read from it: the layout, themes, languages, icons and extensions.

`theme = "accent"` names no file: it derives every color from the accent color of the Windows settings, dark or light as those settings say for the taskbar, and the settings popup offers it as **Windows accent**. A theme file can do the same with `seed = "accent"`, or derive the colors from a color of its own with `seed = "#rrggbb"`; what it sets under `[colors]` is applied over the derived colors. The roles are tones of palettes with the hue of the seed, as in Material 3, but built in the OKLCH color space instead of Material's HCT, so they are near the colors the Material tools give, not equal to them. A theme file chooses between them with `scheme = "dark"`, the default, `"light"` or `"system"`, which follows the Windows setting for the taskbar; a light scheme without a seed is derived from the green of the built-in colors. While the colors follow the accent color or that setting, both are asked for again every second. Choosing the accent theme and a light theme file were tried on a running shell, on a machine set to dark; changing the accent color or the mode while it runs was not, since that would have changed the settings of the machine it was tried on. On a light bar the white notification icons that applications draw for a dark taskbar are hard to see.

Whatever the theme, the colors that are read (text and icons, the accent, outlines) are then made lighter or darker by as little as gives them the contrast WCAG 2.1 asks for at level AA to the surfaces of the theme: 4.5 to 1 for text and the accent, 3 to 1 for outlines; hue and saturation stay, and a color that already has the contrast is not touched. `contrast = false` in the layout file, or the switch under **General** in the settings, leaves a theme exactly as written. A bar of a theme that leaves out the fields behind its widgets (`flat`) lies on the desktop, so there the bar looks at the desktop under it: every five seconds the desktop window of Explorer draws the strip under the bar into a bitmap, and a widget whose text does not have 4.5 to 1 to all of the desktop under it fades its field back in, on which the colors of the theme are read; where the wallpaper is calm and dark or light enough, the widget stays without one. What is looked at is the desktop window (the wallpaper of Windows, a `[wallpaper]` of Tessera, the icons of the desktop), not a window that lies over it: a bar is not kept readable over a window, and a live wallpaper that another program draws in a window of its own was not tried. The colors of application and tray icons and what an extension draws with colors of its own are not counted, and no size of text is checked.

`theme = "name"` in that file applies `%APPDATA%\tessera\themes\name.toml` over the built-in theme. Besides `seed` and `scheme`, a theme can replace single colors, as `#rrggbb` under `[colors]`, using the role names `surface_container`, `surface_container_highest`, `on_surface`, `on_surface_variant`, `outline`, `primary` and `secondary_container`. It can also replace lengths, in pixels: the corner radii under `[shape]` (`small`, `medium`, `large`, `full`), the distances under `[spacing]` (`tiny`, `small`, `medium`), and the measurements of the parts under `[bar]`, `[dock]`, `[calendar]`, `[slider]`, `[meter]` and `[launcher]`; the text under `[typography]` (`family`, the name of an installed font, `label_size` and `icon_size`); and the three springs every animation uses, `[motion.spatial]`, `[motion.spatial_fast]` and `[motion.effects]`, each with a `stiffness` from 20 to 5000 and a `damping_ratio` from 0.2 to 2, where 1 settles without overshoot and less bounces. A value outside what is usable is reported and left as built in; a key the theme format does not know makes the whole file unusable, which is reported too. The font of the icons cannot be replaced. `flat = true` under `[bar]` leaves out the fields behind the widgets of a bar, so that they stand on the wallpaper; what marks a state, such as the shown workspace, stays. [`themes/`](../themes) holds three themes to copy and start from, `expressive`, `compact` and `void`, whose comments name the keys; the registry of this repository offers them too. `void` is flat and goes with the arrangement `Void`: a thin top bar and a small dock, after the desktops of tiling window managers. Its bar starts with the `focused` widget, the name of the program whose window is in front (or the title of that window, with `show = "title"`). There is no widget for the weather, and nothing is drawn on the desktop. It was tried on a running shell on one display. `expressive` and `compact` were tried on a running shell, where the colors, corners, sizes and font changed as the files say; how their springs move was not compared by eye, and the reports about unusable values were tried only in tests.

`language = "de"` in that file, or the choice under **General** in the settings, takes the texts from `%APPDATA%\tessera\languages\de.toml`, or, where that is absent, from `languages\de.toml` beside `tessera.exe`, where the download keeps them; any name of letters, digits, `-` and `_` other than the built-in `en` and `ru` names such a file. The file gives `name`, what the language calls itself, and under `[texts]` the English texts of Tessera with what they are in the language; a text it leaves out stays English. `months`, `months-of` (the months as they follow a day, where they differ), `short-months`, `weekdays` and `short-weekdays` (Monday first) are optional lists for the calendar and the clock. A file with an unknown key or a list of the wrong length is refused with a line in the log and Tessera speaks English. The files are looked for when the settings open and a changed file is read again when the layout is. [`languages/`](../languages) holds German (`de`), Spanish (`es`), French (`fr`) and Ukrainian (`uk`), each a template for others: a test keeps them in step with the texts Tessera has, but the wording was written by a machine and no native speaker has reviewed it. Tried once on a running shell with the files beside the program and none in the configuration directory: with `language = "uk"` the clock of the bar showed the date in Ukrainian; the other texts of those files were not looked at on screen. As with the built-in languages, what the widgets of the bars say stays English.

## The file manager

`[files]` sets how the [file manager](FEATURES.md) shows folders:

```toml
[files]
own = true            # the folders of the user menu and the launcher open here, not in Explorer
hidden = false        # show hidden and system files
folders_first = true  # folders before files, whatever the order
view = "details"      # or "tiles"
# view may also be "list" or "icons"; "tiles" shows the type and size beside each icon
group = "none"        # or "type" or "date": what a folder is first grouped by
per_folder = false    # each folder keeps its own look, zoom, order and groups
restore = false       # a window opened without a folder opens with the tabs of the last one closed
explorer = true       # follow Explorer's folder options: extensions, hidden and protected files, desktop.ini names

[[key]]
press = "win+e"
files = "~"           # a path, "~" for the folder of the user, or "computer"
```

From a console, `tessera files`, `tessera files computer` and `tessera files D:\Projects` open a window in the running shell.

`tessera files home` (or `files = "home"`) opens Quick access: the folders Explorer keeps there and the files used lately. With `restore = true`, `tessera files` and a `files` key without a folder open the tabs of the last window closed instead of the folder of the user; they are kept in `%LOCALAPPDATA%\tessera\files-session.toml`, with the shares of other computers visited, which the side shows under Network. With `per_folder = true` the looks of the folders are kept in `%LOCALAPPDATA%\tessera\files-views.toml`, the 500 seen last; neither file is configuration, and either can be deleted to forget what it holds. `explorer = true` reads the folder options of Explorer (`HideFileExt`, `Hidden` and `ShowSuperHidden` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced`) each time a folder is read; with `explorer = false` every name is shown as it is and protected files of the system show with the hidden ones. `hidden = true` shows hidden files whatever Explorer says; Ctrl+H switches them for a pane.
## The clipboard

`[clipboard]` sets the history of the clipboard that Tessera keeps while it runs (see [FEATURES.md](FEATURES.md)):

```toml
[clipboard]
enabled = true   # keep what is copied; false forgets all but the pinned items
size = 25        # items kept besides the pinned ones, 1 to 200
images = true    # keep pictures too, as PNG files in cache\clipboard of the configuration directory

[[key]]
press = "win+v"
action = "clipboard"   # the popup "clipboard", noting the window in front to paste into
```

The widget `clipboard` is the list, for a popup, and the popup `clipboard` holds it: 440 by 560 pixels at the middle of the bar, unless a `[[popup]]` of the same name says otherwise (`place = "center"` puts it in the middle of the display). The widget `clipboard-button` is a button of a bar that opens it. In the list the arrows choose an item, Enter or a click puts it back on the clipboard and presses Ctrl+V in the window that was in front, Delete (with nothing written in the search) takes it out, and Esc closes the popup; the buttons of a row pin it, take it out, or open the web address it is. **Clear all** keeps the pinned items. Pinned items are written to `clipboard.toml` in the configuration directory, which Tessera rewrites; the pictures among them stay in `cache\clipboard`. A `clipboard.toml` that cannot be read is reported and left as it is, and pins made then are not written until Tessera starts again, so that it can be mended without losing them.

Where `windows_keys` binds the keys of Windows, Win+V is bound to `clipboard` only when `enabled` was true as Tessera started; with `enabled = false` it stays with the history of Windows. A change of `enabled` moves Win+V at the next start. A `[[key]]` that names the action, as above, takes Win+V either way.
## The window switcher

The window switcher opens for a `[[key]]` with `switch = "next"` or `"previous"`, or one with `action = "switcher"`, and `[switcher]` sets how it looks and which windows it offers:

```toml
[switcher]
style = "tiles"       # or "thumbnails", "list", "icons"
previews = false      # pictures of the windows in the tiles of "tiles"
scope = "workspace"   # or "display", "all"
minimized = "keep"    # or "last", "hide"

[[key]]
press = "alt+backquote"
action = "switcher"
arg = "app-next"      # the windows of the application of the window in front
```

Where the keys of Windows are in use (`windows_keys = true`, or Tessera being the shell), the action `switcher` brings Alt+Tab, Alt+Shift+Tab, Alt+` and Alt+Shift+` without such entries; the entry above is for when they are not.

| Key | Values | What it does |
| --- | --- | --- |
| `style` | `tiles` (default), `thumbnails`, `list`, `icons` | `tiles`: tiles of icon and title, eight in a row, or with `previews = true` larger tiles with a picture of the window, six in a row. `thumbnails`: a picture of each window, as large (up to 320 by 200) and as many in a row as nine tenths of the display give room for, made smaller down to half that size before the rows scroll. `list`: one line for each window with its icon, its title and the name of its program. `icons`: the icons alone in a row (more rows where they do not fit), with the title of the chosen window under them. A minimized window, and one hidden on another workspace, has no picture to give and shows its icon in the place of one |
| `scope` | `workspace` (default), `display`, `all` | `workspace`: the windows of the workspace that is shown, on every display; the windows of other virtual desktops of Windows are never offered. `display`: of those, the windows on the display the switcher opens on, the display the pointer is on. `all`: those of the workspace shown, then the windows of the other workspaces of Tessera's own (`[workspaces]` with `own`), workspace by workspace; where every display has workspaces of its own, those of the display the switcher opens on. Choosing such a window goes to its workspace |
| `minimized` | `keep` (default), `last`, `hide` | `keep`: minimized windows stand where they were last used, like the others. `last`: after all the windows that are not minimized. `hide`: they are not offered |

Rows that do not fit on the display scroll to the chosen window. While the switcher is open:

- typing narrows the windows to those whose title, or the name of whose program, has every word typed, whatever the case; the line of what was typed stands above the windows, Backspace takes the last letter back, and Esc forgets all of it (a second Esc closes the switcher). While the keys of the switcher are held, as Alt is with Alt+Tab, a letter is taken as typed although Windows writes nothing for it;
- the wheel moves the choice one window on for each notch down, and back for each notch up;
- Delete or Ctrl+W asks the chosen window to close, and a click with the middle button the window clicked, as its close button does. A window that goes, or only hides, within a second and a half is no longer offered; one whose application asks the user first, or refuses, stays;
- Tab and Shift+Tab move the choice when the switcher was opened with no key held;
- an action `switcher` with `app-next` or `app-previous` narrows the open switcher to the windows of the application of the chosen window, and one with `next` or `previous` offers all of them again.

The arrows, Enter, Esc, Delete and Backspace are taken with the keyboard hook whatever has the keyboard, so they reach no other program while the switcher is open. Letters, Tab, Ctrl+W and the wheel reach the switcher while its window has the keyboard: Tessera brings it in front when it opens, and asks Windows again in other ways where Windows refuses at first; where Windows still refuses, those go to the window in front. The settings show the style, the scope and the minimized windows on the **Window switcher** card of the **Windows** part.

## Actions

A `[[key]]` can name an action instead of one of its other fields: `action` is its name and `arg`, where the action takes one, what it is done with. The modules of Tessera register the actions, so one comes with the feature that brings it, and a name that no module registered, or an `arg` the action refuses (one that takes none refuses every `arg`), is reported when the configuration is read, like any other mistake in a `[[key]]`. An action may ask for its keys to be taken from Windows where Windows keeps them to itself (as it does with Win+D, Win+V or Win+E); Tessera then takes them with its keyboard hook, as it does for `switch`, `dock` and `files`.

```toml
[[key]]
press = "win+v"
action = "popup"      # the popup `arg` names, opened or closed
arg = "clipboard"
```

An action may also bring keys of its own, the keys Windows has for the same thing (as Win+M for minimizing every window). They are bound where `windows_keys = true` is at the top of `config.toml`, or, without that key, when Tessera is the shell of the session; `windows_keys = false` leaves them out. A `[[key]]` that presses the same keys wins over such a key, whatever it does. The actions below that bring keys name them, as do those of snapping and the window menu (`snap`, `snap-display`, `snap-layouts` and `window-menu`); see also [the keys of Windows without Explorer](#the-keys-of-windows-without-explorer).

From a console, `tessera do <action> [arg]` does an action in the running shell, as `tessera do popup launcher`; the words after the name are joined by spaces into the `arg`.

| Action | `arg` | What it does |
| --- | --- | --- |
| `popup` | the name of a popup | opens that popup, or closes it when it is open; its keys are taken from Windows |
| `desktop` | `toggle`, `minimize`, `restore`, `others`, or none | without `arg` or with `toggle`, shows the desktop, or brings the windows back, as the widget `desktop` does: through the taskbar of Explorer where it runs, and otherwise by minimizing the application windows itself and restoring those still minimized on the next press; the keyboard goes to the desktop, and the windows come back in their order with the one that was in front active. `minimize` minimizes every application window (as Win+M), `restore` brings back those Tessera minimized (as Win+Shift+M) and `others` minimizes all but the window in front (as Win+Home); these three are Tessera's own even where Explorer runs, so `restore` does not bring back what the taskbar of Explorer put aside. Its keys (Win+D, Win+M, Win+Home) are taken from Windows |
| `trash` | `open`, `restore`, `empty`, or none | without `arg` or with `open`, has Explorer open the Recycle Bin, as a click on the widget `trash` does; `restore` asks the user first and, on yes, brings every item in it back to where it was deleted from; `empty` empties the bins of all drives once the user said yes in the dialog of Windows. It brings no keys |
| `snap` | `left`, `right`, `up`, `down`, `left-half`, `right-half`, `top-left`, `top-right`, `bottom-left`, `bottom-right`, `maximize`, `restore` or `minimize` | snaps the window in front, as [snapping](#snapping-window-rules-and-hot-corners) describes; its keys (Win and the arrows) are taken from Windows and bound by default where the keys of Windows are |
| `snap-display` | `next`, `previous`, or none | sends the window in front to the next display from the left (round to the first), or the one before; its keys (Win+Shift+Right, Win+Shift+Left) are taken from Windows and bound by default |
| `snap-layouts` | none | opens the snap layouts for the window in front; Win+Z is taken from Windows and bound by default |
| `window-menu` | none | opens the window menu of the window in front; Alt+F3 is bound by default where the keys of Windows are |
| `files` | a folder, as `files` names one, or none | opens a window of the file manager there, as `files = "..."` does; without `arg`, at the folder of the user; its keys are taken from Windows, and it brings Win+E where `windows_keys` is on |
| `shell` | `install`, `revert` or `explorer` | `install` makes Tessera the shell of the user's next sessions and `revert` gives that back to what the machine names, as `tessera shell install` and `tessera shell revert` do; `explorer` hands this session to Explorer now, when Tessera is its shell: Explorer starts as the shell once Tessera has quit |
| `run` | none | opens the Run dialog, the popup `run`, or closes it; its keys (Win+R) are taken from Windows, and Win+R is bound to it by default where the keys of Windows are in use |
| `runner` | none | opens the compact runner, the popup `runner`, or closes it; it brings no key |
| `winx` | none | opens the menu of Win+X, or closes it: at the widget `winx` where a bar has one, else at the Start button (the button that opens `launcher`), else at the left end of a bar. Brings Win+X |
| `power-dialog` | none | opens the dialog that asks what the computer is to do (switch user, sign out, sleep, hibernate, shut down, restart, and while updates wait update and shut down or update and restart); nothing is done before OK or Enter. Brings no key: where `windows_keys` is on, Alt+F4 opens it while the desktop is in front (no window, or the desktop window), and Alt+F4 elsewhere still closes the window in front |
| `display-mode` | `pc-screen-only`, `duplicate`, `extend`, `second-screen-only`, or none | without `arg` opens the popup `display-mode`, the choice of Win+P; with one switches the displays to that mode at once. Brings Win+P |
| `input-language` | `next`, `previous`, `list`, or none | asks the window in front for the next keyboard layout, or the one before with `previous`; where a window of Tessera is in front, Tessera takes on that layout itself. `list` opens the popup `keyboard-list`, whose choice is for the window that was in front before it. Brings Win+Space and Win+Shift+Space (`previous`) |
| `open` | a file, a program, a folder or an address | opens it through the Shell, as a double click does. Brings Win+I (`ms-settings:`), Win+U (`ms-settings:easeofaccess`), Win+K (`ms-settings-connectabledevices:devicediscovery`), Win+G (`ms-gamebar:`), Win+Shift+S (`ms-screenclip:`) and Win+Pause (`ms-settings:about`) |
| `start`, `search`, `quick-settings`, `notification-center` | the name of a popup, or none | opens that popup, or closes it; without `arg`, for `start` (Ctrl+Esc) the popup of `windows_key`, the same one the Windows key opens, or the launcher where `windows_key` names none, `launcher` for `search` (Win+S, Win+Q), `quick-settings` (Win+A; the built-in layout has this popup with the tiles of the [quick settings](#quick-settings), and for a layout without it the module brings one with the connections, the switches of Wi-Fi and Bluetooth, the volume and the brightness) and `calendar` (Win+N) |
| `screenshot` | none | saves a picture of all displays as `Screenshot (N).png` in the Screenshots folder of Pictures, N counted on from where Windows counts. Brings Win+PrtScn |
| `dock` | 1 to 9 | does what `dock = n` does: what a click on the button of that number in the dock does. Brings no key |
| `quiet` | `toggle`, `on`, `off`, `morning`, a number of minutes, or none | switches [do not disturb](#notifications): without `arg` or with `toggle` the other way than it is, `on` until it is switched off, `morning` until eight in the morning, `30` for thirty minutes; `off` also lets go of what holds it by itself until that ends |
| `notify` | the text of a notice | posts a notice of Tessera's own, listed with the notifications and shown as a banner where banners are: `tessera do notify Backup done \| 3 files copied` has the title before ` \| ` and the text after it |
| `clipboard` | none | opens the history of the clipboard in the popup `clipboard`, or closes it, and notes the window in front as the one to paste into; its keys (Win+V) are taken from Windows, and Win+V is bound to it where `windows_keys` binds the keys of Windows and `[clipboard]` had the history on as Tessera started |
| `night-light` | `on`, `off`, `toggle`, or none | switches night light on or off, or over without `arg` or with `toggle`: that of Windows or Tessera's own, as `[night_light] source` says |
| `volume` | `up`, `down`, `mute` or `mute-microphone` | turns the default output a step up (and lifts its mute) or down, as Windows counts steps for it (2 of 100 for most), mutes it or lifts its mute, or mutes the default microphone or lifts its mute; the flyout shows what came of it where `[flyout]` wants it. |
| `brightness` | `up` or `down` | makes the display under the pointer a tenth brighter or darker, a monitor over DDC/CI or the panel of a laptop, and shows it in the flyout where `[flyout]` has `brightness = true` |
| `media` | `toggle`, `play`, `pause`, `stop`, `next` or `previous` | passes the command to the player chosen in `media-controls`, or to the one Windows considers current. |
| `wallpaper` | `next`, `previous`, or none | has every display show the next picture of its list (without `arg` too), or the one before, at once; the interval begins anew |
| `eject` | the letter of a drive, as `E` or `E:` | ejects that drive as its **Eject** button in the popup `devices` does: the device it is on, with every drive of it, or its medium where it is a disc or a card in a reader built in; a notice says how it went |
| `switcher` | `next`, `previous`, `app-next`, `app-previous`, or none | opens the [window switcher](#the-window-switcher), or goes one window on (`next`, or none) or back (`previous`) in the open one; `app-next` and `app-previous` go through the windows of one application only. The choice is taken when the keys to hold of the `press` that did it are let go; for a `press` with none, the switcher stays open until a window is chosen. Done by `tessera do switcher`, it waits for the keys that are down as it is done, which are usually none. Its keys are taken from Windows, and it brings Alt+Tab (`next`), Alt+Shift+Tab (`previous`), Alt+` (`app-next`) and Alt+Shift+` (`app-previous`) where the keys of Windows are in use |

The keys a `press` can end with are a digit, a letter, `f1` to `f24`, `space`, `tab`, `enter`, `left`, `up`, `right`, `down`, `home`, `end`, `pageup`, `pagedown`, `insert`, `delete`, `backspace`, `escape`, `pause`, `printscreen`, and the keys a US layout has for `period`, `comma`, `plus`, `minus` and `backquote` (on another layout, the keys in their place).

## The shell of the session

`tessera shell install` makes Tessera the shell Windows starts at the user's next sign-in, `tessera shell revert` gives that back, and `tessera shell status` prints what Windows reads and which program it will start; [SHELL.md](SHELL.md) says how this works, the ways back, and what does not work without Explorer. The widget `shell`, in the popup `shell` that comes with it, shows the same and has buttons for both, and, as the shell, one that hands the session to Explorer at once.

`[shell]` says what Tessera does as the shell; beside Explorer it changes nothing:

```toml
[shell]
emergency_keys = true    # Ctrl+Win+Alt+E: Task Manager, and Explorer as the shell; Ctrl+Win+Alt+R: Tessera again
quit_to_explorer = true  # a Tessera that quits starts Explorer as the shell, so the session keeps one
media_keys = true        # the keys of the volume and of media work as with Explorer
```

As the shell, `[workspaces]` without `own` keeps four workspaces of Tessera's own, as the virtual desktops of Windows are part of Explorer.

## Startup programs

When Tessera is the shell of the session, it starts what Explorer would start at sign-in, once per session, a moment after the bars are up; beside Explorer it starts nothing, as Explorer does it. `[startup]` changes that:

```toml
[startup]
run = true            # false: Tessera, as the shell, starts none of them
delay = 2             # seconds after the bars opened
active_setup = true   # false: the components of Active Setup do not run
packaged = true       # false: the startup tasks of Store apps do not start
```

What starts, in this order: the components of Active Setup that the machine has in a newer version than the user (each waited for, a minute at most); `RunOnce` of the machine, 64-bit and then 32-bit, each waited for, only where Tessera may delete its values (it usually may not, and it then reports how many it left); `Policies\Explorer\Run` and `Run` of the machine, its 32-bit `Run`, then those of the user; the Startup folders of all users and of the user; `RunOnce` of the user; the startup tasks of Store apps. A value of `RunOnce` is deleted before its program runs, or after it when its name starts with `!`; in Safe Mode only those whose name starts with `*` run. A session counts as started once a volatile key named by the session and the logon is made under `HKCU\Software\Tessera\Startup`, so a Tessera that restarts in the same session starts nothing again; Windows deletes that key at sign-out. A Tessera run as administrator, with the full token of an administrator who also has a filtered one, starts nothing and makes no such key, so that the programs of the session never get that token. `[startup]` changed while Tessera runs counts at once: `run = true` then starts the programs if they did not start yet in this session, and `run = false` before the delay is over keeps them from starting.

The widget `startup`, made for a popup or a card, lists the programs that start with every sign-in (not those of Active Setup and `RunOnce`) with a switch each, and **Startup apps** under **General** in the settings shows the same list. A switch writes what Task Manager writes, in `Explorer\StartupApproved` (`Run`, `Run32`, `StartupFolder`) of the program's own hive, which Explorer and Tessera both honour. For a program of all users, Windows lets only an administrator write the machine's choice; where it refuses, switching the program off marks it off for this user alone in `HKCU\Software\Tessera\StartupApproved`, which Tessera alone reads (Explorer, as the shell, still starts it), and switching it back on removes that mark, or says it needs an administrator where the machine's choice is off. A program a policy starts has no switch. The switch of a Store app writes the state Windows keeps for its startup task.

## The launcher and its runners

The `launcher` widget, the compact `runner` widget and the Run dialog `run` answer what is typed with the runners of Tessera, in the manner of KRunner. The answers a runner is sure of stand above the applications, the others after the files. Enter does what the chosen row does, Shift+Enter runs a command in a terminal, Ctrl+Shift+Enter runs a command or an application as an administrator, and Ctrl+C copies the result of a calculation or a conversion. Ending the session or a process waits for Enter a second time.

| Runner | Answers |
| --- | --- |
| `calculator` | arithmetic, as `2^10 / 3`, `sqrt(2) * pi`, `15% * 80`, `0xff + 1`, `255 to hex`; after `=` any expression |
| `units` | conversions, as `5 km to mi`, `100 f in c`, `2 GiB in MB`, `1,5 кг в г`, and of currencies, as `100 usd to eur` |
| `command` | paths, addresses, `shell:` places, `::{CLSID}`, programs of the App Paths and of `PATH` with arguments; `> command` runs in a terminal |
| `web` | `g:`, `gg:`, `google:`, `ddg:`, `bing:`, `wiki:`, `wp:`, `gh:`, `yt:`, `maps:` and `ya:` searches, and the keywords of `web` |
| `search` | a last row that searches the web for anything with the engine of `engine` |
| `windows` | open windows by their titles and programs, to switch to or close |
| `processes` | `kill <name>`: ends the processes of the programs of that name |
| `power` | locking, sleep, hibernation, signing out, restarting, shutting down |
| `settings` | pages of the Settings app, items of the Control Panel and tools of Windows |
| `emoji` | `:name`: emoji and symbols, copied when chosen |
| `recent` | the documents of the Recent folder, by their names |

The `launcher` and `runner` widgets take these options besides those of the launcher (`placeholder`, `folders`, `view`):

- `runners`, the runners to ask, by the names of the table; all of them without the option.
- `web`, a table of keywords and addresses that adds web searches, as `web = { so = "https://stackoverflow.com/search?q={}" }`.
- `engine`, the keyword of the engine the last row searches the web with (`ddg`).
- `currency = false` keeps Tessera from fetching exchange rates; rates kept from before are still used (`true`).
- `recent = true` lists the documents opened lately while nothing is typed, in the list view (`false` for the launcher, `true` for the runner).
- `settings = false` leaves out the pages of the settings (`true`).

```toml
[[popup]]
name = "launcher"
width = 640
height = 720
content = ["apps", { widget = "launcher", view = "list", recent = true, engine = "so", web = { so = "https://stackoverflow.com/search?q={}" } }, "power"]
```

In an address of `web`, `{}` stands for what is searched and `{lang}` for the two letters of the language of Tessera; a keyword of `web` replaces a built-in one of the same name. The exchange rates are the daily reference rates of the European Central Bank, kept in `%LOCALAPPDATA%\tessera\rates.xml` and fetched again from `www.ecb.europa.eu` when a conversion between currencies is asked and they are older than 12 hours; a fetch that failed is not tried again for ten minutes. What is chosen is remembered in `launcher-history.toml` in the configuration directory, 300 entries at the most; deleting the file forgets it.

The popups `runner` and `run` come with Tessera and can be replaced by a `[[popup]]` of the same name. `place = "top"` puts any popup near the top of the display, in the middle of its width, without a veil:

```toml
[[popup]]
name = "runner"
place = "top"
width = 640
height = 380
content = [{ widget = "runner", recent = false }]

[[key]]
press = "alt+space"
action = "runner"
```

The Run dialog reads and writes the history of the Run dialog of Explorer (`HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\RunMRU`), so both offer the same commands.

## The keys of Windows without Explorer

When Tessera is the shell, nothing of Explorer answers the keys Explorer answers in Windows, so the actions above bring them, and `windows_keys` (on by default in shell mode) binds them. Beside Explorer, `windows_keys = true` takes them from Explorer as well; each is then answered by Tessera instead. A `[[key]]` that presses the same keys wins, so a single key can be given back to another action, or turned into something else:

```toml
windows_keys = true

[[key]]
press = "win+a"
action = "quick-settings"
arg = "sound"         # the popup Win+A opens instead of `quick-settings`
```

Win+L locks the session through Windows itself, and Ctrl+Shift+Esc, Alt+Shift and Ctrl+Shift are answered by Windows too, so they keep working in shell mode and Tessera does not take them. While `windows_key` takes over the Windows key, Ctrl+Esc opens the same popup, as it opens the Start menu in Windows, through `start` above, unless a `[[key]]` holds it or gives `start` another popup as `arg`. Without `windows_key` the Windows key by itself opens nothing, in shell mode as well; set `windows_key = "launcher"` for it to open the launcher. Win+E belongs to the file manager (`files`), Win+R to the Run dialog, Win+V to the clipboard history and Win+1 to Win+9, Win+T and Win+B to the dock, which bring their own keys. Not answered by Tessera: Win+period (the emoji panel of Windows; whether Windows still opens it without Explorer is not known), Win+comma (the peek at the desktop), Win+Tab, Win+Ctrl+D and the other keys of the virtual desktops of Windows, which need Explorer.
## Snapping, window rules and hot corners

`[snap]` snaps windows that are dragged to an edge of a display: the left or right edge gives that half, the part of an edge within `corner` pixels (96) of a corner gives the quarter there, and the top edge the whole display; the pointer is to be within `edge` pixels (8) of the edge. A translucent shape in the primary color shows where the window goes before it is let go, and moves from place to place. A snapped window that is dragged away takes its size from before it was snapped once it is let go, with the pointer at the same share of its width; Windows itself gives it back while it is dragged, which a program outside the window cannot. A window dragged by its border is being resized and does not snap; Tessera asks the window whether the drag began on its border (`WM_NCHITTEST`), so a window whose size changes as it is dragged onto a display of another scale still snaps. With Shift held, the `[[snap.zone]]` entries of the display under the pointer are the places instead. `gap` leaves that many pixels between snapped windows and around them (0).

```toml
[snap]
enabled = true      # without it, only when Tessera is the shell
gap = 4
assist = true       # offer the other windows for the rest of the display
hover = true        # the snap layouts open over a maximize button (as enabled)
hover_delay = 450   # milliseconds

[[snap.zone]]
display = 1         # counted from 1 from the left; every display without it
rect = [0, 0, 0.3333, 1]   # x, y, width, height in shares of the work area

[[snap.layout]]     # replaces the six layouts of Windows
zones = [[0, 0, 0.7, 1], [0.7, 0, 0.3, 1]]
```

While dragging snaps, Tessera switches the snapping of Windows off (`SPI_SETWINARRANGING`) for as long as it runs, so that a window is not placed twice; the setting "Snap windows" of Windows stays as the user wrote it, and Tessera switches it on again when it quits, and at its next start if it was terminated. Whether Windows still snaps dragged windows without Explorer has not been seen, which is why dragging snaps by default only when Tessera is the shell. With the snapping of Windows off, Windows may not let a maximized window be dragged by its title bar; Win+Down or a double click on the title bar restores it then. Switching it off also switches off the keys of Windows for it, Win with the arrows and Win+Z: they then do something only where Tessera binds its own, as it does where `windows_keys = true` (by default when Tessera is the shell) or a `[[key]]` names the action `snap`, and otherwise Tessera reports that they do nothing. Windows that are tiled are left to the tiling, and Windows does not let Tessera move a window of a program that runs as administrator.

The action `snap` does the same with the keys: with `left` and `right` a window goes to that half, from the other half back to where it was, and from a half or a quarter at the edge on to the next display; `up` goes from a half to its top quarter and from there to the whole display, and `down` takes those steps back and minimizes a window that is where the user put it. `snap-display` sends the window in front to the next display, at the same share of the way across and down, snapped or maximized as it was. `snap-layouts` (Win+Z) opens six small pictures of ways to divide the display under the right end of the title bar of the window in front, and a click on a zone puts the window there; with `hover` they also open when the pointer rests on the maximize button of a window, which Tessera asks the window about (`WM_NCHITTEST`) a few times a second while the pointer is near the top right corner of a window, and close once the pointer has moved away. After a window took a half by dragging or with the keys, or a zone of the layouts, the other windows are offered for the rest of the display, part after part (`assist`); Esc, a click elsewhere or another window coming in front leaves the rest as it is, and the keys of snapping go on with the window that took the half.

Each `[[rule]]` names windows by `exe` (the file name of the program), `app` (the AppUserModelID of the packaged application, as `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App`), `title` and `class` (the window class), each a pattern in which `*` stands for any text and `?` for one character, whatever the case of the letters, or a regular expression after `re:`, and says what is done to them when they are first seen: `float = true` leaves them out of the tiles and `tile = true` tiles them whatever `[tiling]` says of their program; `workspace` (from 1) sends them to a workspace of Tessera's own (the virtual desktops of Windows take no window of another program); `display` (from 1 from the left) to a display; `position` (`"center"` or `[x, y]`) and `size` (`[width, height]`, in shares of the work area) place them; `maximize`, `above` (kept above the other windows), `opacity` (0.1 to 1) and `border = false` (no thin border, Windows 11) do what they say. `apply = "always"` does it again whenever the window changes, as far as it differs, which keeps a window where its rule puts it; otherwise a window that moves itself within a second of being shown, as many programs do to go back to where they were, is put back once. Every rule that names a window applies in the order of the file, a later one having the last word. Windows that are open already when Tessera starts or the rules change only get the rules that apply always. A tiled window is not placed by a rule. The windows of the applications of the Universal Windows Platform, such as Settings and Calculator, all belong to one program, `ApplicationFrameHost.exe`, and one class, `ApplicationFrameWindow`, so `exe` and `class` name all of them at once; `app` names one. A program that gives its window its title a moment after showing it gets what a `title` rule says as soon as the title comes, within the first second; a title that comes later needs `apply = "always"`. A window that has no border by a rule keeps none when the tiling colors the border of the window in front. `skip_dock` and `skip_switcher` are read but neither the dock nor the window switcher leaves windows out yet. A rule without a name, with an `opacity` out of range, or with a `size` beyond the work area is reported and left out.

```toml
[[rule]]
app = "Microsoft.WindowsCalculator_*"
float = true
above = true
position = "center"

[[rule]]
title = "*Picture-in-picture*"
above = true
apply = "always"
```

The window menu (`window-menu`, Alt+F3 where the keys of Windows are bound) opens under the title bar of the window in front: keep it above other windows or no longer, three opacities, the other workspaces of Tessera's own, the next display, the left or right half, maximize or restore, minimize, close, and **Create a rule for this window**, which writes a `[[rule]]` with its program, its window class, where it is and how large, whether it is kept above and how opaque, and opens `config.toml` to show it.

Each `[[corner]]` names a corner or an edge (`at`: `top-left`, `top-right`, `bottom-left`, `bottom-right`, `top`, `bottom`, `left`, `right`), optionally one `display` (from 1 from the left), a `delay` in milliseconds the pointer rests there first (150, at most 5000), and one of `action` (with `arg`) as a `[[key]]` names one, `popup` or `run`. It is done once, and again only after the pointer has gone 40 pixels away; nothing is done while a button of the pointer is held, as while a window is dragged, or while the window in front fills the display. A corner of its own beats an edge it lies on. The pointer is looked at from a thread of its own ten times a second, fifty times while it is near a corner or edge in use, and not at all while there is no `[[corner]]`.

```toml
[[corner]]
at = "top-left"
popup = "launcher"

[[corner]]
at = "bottom-right"
action = "desktop"
delay = 300
```

## Notifications

Tessera reads the notification center of Windows every two seconds and keeps what it read: the list of notifications, the banners, the bell of the bar and the badges of the dock all show that one reading. Notifications that leave the notification center without being dismissed in Tessera, as when Windows lets them expire, stay as a history in `notifications.toml` of `%LOCALAPPDATA%\tessera`.

```toml
[notifications]
banners = true        # a notification that arrives comes as a banner
seconds = 6           # how long a banner stays, 2 to 60
corner = "top-right"  # or "top-left", "bottom-right", "bottom-left"
stack = 4             # banners shown at once, 1 to 8
history = 200         # notifications kept after they left Windows, at most 1000
quiet = false         # do not disturb until switched off
quiet_when = ["fullscreen", "game", "presenting", "duplicating"]  # the default
schedule = { from = "22:00", to = "07:00", days = ["fri", "sat"] }
# Without `banners`, banners are off beside Explorer and on while Tessera is the shell,
# where Windows draws none; `banners = false` switches them off there too.

[[notification_rule]]
app = "Microsoft Teams*"  # the name or the identifier, with * and ?
priority = true           # banners even during do not disturb

[[notification_rule]]
app = "*OneDrive*"
banners = false           # listed, but no banners
badge = false             # not counted by the dock
# show = false            # not listed, kept, shown or counted at all
```

Do not disturb holds the banners back; what arrives meanwhile is listed but not shown later. It holds while `quiet = true` (which the settings, the moon button of the list of notifications, the `focus` widget and the `quiet` action write), until a time chosen under the moon button (for an hour, until morning) or given to `quiet`, during the hours of `schedule` (`days` are the days the hours start on; `from` equal to `to` is the 24 hours from `from`), and while the user is busy in one of the ways `quiet_when` names: an application that fills the display, a game drawn full screen with Direct3D, presentation mode, or a display that shows the picture of another. Switching it off while the schedule or being busy holds it lets go of that reason until it ends. The banners of an application whose rule has `priority = true` come all the same.

The first `[[notification_rule]]` whose `app` names an application decides for it; small and capital letters are alike. The name is the one its notifications show; the identifier is the one it posts under, which for a packaged application is its application user model ID, such as `Microsoft.WindowsStore_8wekyb3d8bbwe!App`.

The widget `notifications-button` is a bell for a bar: it counts the notifications that came since a list of them was last open and opens the popup `notifications`. The widget `focus` is a switch for do not disturb, for a popup.

## Quick settings

The popup `quick-settings` of the built-in configuration holds them as Windows 11 does: tiles in two columns, then the sliders of the volume and the brightness. Win+A opens it where the keys of Windows are bound (the action `quick-settings`); a widget given `popup = "quick-settings"` or a `[[key]]` opens it too:

```toml
[[key]]
press = "win+a"          # taken from Windows, whose quick settings it opens
action = "popup"
arg = "quick-settings"
```

The widget `tiles` lays tiles out in rows; its option `tiles` lists them as any widgets are listed, and `columns` says how many stand in a row (2, at most 4). A tile with details has a narrow button beside it that unfolds them under its row; `detail` names another widget to unfold (a name or a table), or `""` for none.

```toml
[[popup]]
name = "quick-settings"
width = 380
height = 720
content = [{ widget = "tiles", columns = 2, tiles = [
    { widget = "radio-tile", radio = "wifi" },      # unfolds "wifi"
    { widget = "radio-tile", radio = "bluetooth" }, # unfolds "bluetooth"
    "airplane",                                      # unfolds "radios"
    { widget = "hotspot", detail = { widget = "hotspot-details", qr = false } },
    "power-mode",                                    # unfolds "power-modes"
    "night-light",                                   # unfolds "night-light-slider"
] }, "volume-slider", "brightness-slider"]
```

| Widget | What it is |
| --- | --- |
| `tiles` | the grid above |
| `radio-tile` | switches Wi-Fi or Bluetooth (`radio = "wifi"` or `"bluetooth"`) |
| `airplane` | switches every radio off and later back on those that were on; lit while every radio is off. The airplane mode flag of Windows is not set, since Windows lets only its own quick settings set it |
| `radios` | every radio of the machine with a switch of its own |
| `hotspot` | starts and stops the mobile hotspot of the connection the machine reaches the internet through; its note counts the devices on it |
| `hotspot-details` | the name of its network, its password (dots until a click), the devices on it, the band to send it on where the adapter has several, and, while the password is shown, a QR code a phone camera joins the network with, which holds the password (`qr = false` leaves it out); the name and the password are changed in the Windows settings it leads to |
| `power-mode` | the power mode of Windows; a click puts the next one in use. Windows offers the modes only while the Balanced plan is in use |
| `power-modes` | the three modes to choose from, and whether battery saver is on |
| `night-light` | switches night light; with Tessera's own and a schedule it says until when |
| `night-light-slider` | how warm the light is, in kelvins, and when it comes on |

`[night_light]` chooses which night light these switch:

```toml
[night_light]
source = "auto"      # "windows", "tessera", or "auto": that of Windows, or
                     # Tessera's own when Tessera is the shell or Windows
                     # keeps no state of it yet
kelvins = 3400       # how warm Tessera's own is, 1200 to 6500
schedule = "sun"     # "off", "sun" (needs latitude and longitude) or "hours"
latitude = 55.75     # degrees, north positive
longitude = 37.62    # degrees, east positive
from = "21:00"       # for schedule = "hours"
to = "07:00"
```

The night light of Windows is read from and written to the records Windows keeps for it in the CloudStore of the account, whose layout Windows does not document; Windows switches it by the schedule of its own settings, and `schedule` here does not apply to it. Tessera's own multiplies the gamma ramp of every display with the color of a light of that temperature and gives the displays their own ramps back when Tessera quits; while they are tinted, their own ramps are kept in the file `night-light-ramps` of the configuration directory, so that a Tessera stopped without giving them back gives them back when it starts again. `kelvins` is how warm it is when Tessera starts: the warmth set on `night-light-slider` for Tessera's own is not written down and is forgotten when Tessera quits or `kelvins` is changed. Windows refuses ramps far from the straight line unless the DWORD `GdiIcmGammaRange` = 256 is set under `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ICM`; a refused ramp is reported once per display. With a schedule, a click on the tile holds until the schedule next changes its mind, as with Windows.

## Quick settings: power plans, sound, media and flyouts

The `plan` widget shows the power plan in use, and a click on it opens the popup `plan`, which the module brings along: the widget `plan-list`, every plan Windows lists, where a click on one that is not in use puts it in use. A popup of the same name in `config.toml` is used instead. `cycle = true` gives the widget the click it had before the list: the next plan, the first after the last; add `popup = ""` so that the popup does not open as well.

```toml
start = [{ widget = "plan", name = false }]            # opens the list
end = [{ widget = "plan", cycle = true, popup = "" }]  # goes on to the next plan
```

The sound popup holds, besides `volume-slider`, `microphone-slider`, `audio-outputs` and `volume-mixer`, the widget `audio-inputs`: the microphones, where a click on one makes it the default, each with its own mute button and slider (`volumes = false` leaves those out). `audio-outputs` takes `volumes = true` for a slider under each output too. `spatial-sound` is a row that says whether the default output renders spatial sound, and nothing where it cannot; Windows lets no program choose the format, so its click opens the sound settings, as the row `sound-settings` does. The built-in popup is this one:

```toml
[[popup]]
name = "sound"
width = 360
height = 680
content = ["volume-slider", "microphone-slider", "audio-outputs", "audio-inputs", "spatial-sound", "volume-mixer", "sound-settings"]
```

`media-controls` shows a chip for each media session while there are several (a browser has one for each tab that plays), and shuffle and repeat buttons where the player tells them; it needs no option.

`[flyout]` says which changes a flyout shows, each `false` unless set: `volume`, `brightness` (the built-in panel of a laptop, whose keys change it), `microphone` (muted or not), `locks` (Caps Lock and Num Lock) and `layout` (the window in front switched to another keyboard layout). `place` is where it hangs in what the bars leave of the display the pointer is on: `bottom` (the default), `top`, `top-left`, `top-right`, `bottom-left` or `bottom-right`. In shell mode Windows shows no indicator of its own, so these are the only ones.

```toml
[flyout]
volume = true
brightness = true
locks = true
layout = true
place = "top-right"
```

`media = true` shows another piece being played by the session Windows considers current, read once a second: a card with its picture, its title and its artist with the application that plays it (the piece that plays when Tessera starts is not shown, nor a piece without a title). `workspace = true` shows going to another workspace on the display the pointer is on, Tessera's own or the virtual desktops of Windows: its name, or its number, and a dot for each workspace, the one shown long and lit. With `layout = true` the action `input-language` (Win+Space where the keys of Windows are bound) also lists the installed layouts with the names Windows gives them, the one switched to lit, for as long as the Windows key is held and `seconds` after, as the switcher of Explorer does. `seconds` is how long a flyout stays after the last change, from 1 to 10 (1.6 without the key), and `margin` how far it hangs from the sides of the space, in logical pixels (24 without the key, at most 400). `place` also takes `left` and `right`, the middle of a side, from which the flyout slides in, and `center`.

```toml
[flyout]
media = true
workspace = true
layout = true
seconds = 3
margin = 40
place = "right"
```

## The wallpaper

`[wallpaper]` sets what Tessera shows behind the icons of the desktop; the [features](FEATURES.md) tell what was tried. Without it Tessera shows the wallpaper of Windows, which the desktop window draws, and draws nothing itself.

```toml
[wallpaper]
source = "files"        # "files", "windows", "daily" or "color"; without it, from what is given
images = ["C:/Users/me/Pictures/Wallpapers"]   # files and directories
images_light = []       # in place of images while Windows is light
images_dark = ["C:/Users/me/Pictures/Night"]   # ... or dark
dark_hours = ["19:00", "07:00"]   # dark by the clock instead of by Windows
interval = 600          # seconds a picture stays, at least 5
shuffle = false         # pictures and videos in a random order
fit = "cover"           # cover (fill), contain (fit), stretch, center, tile or span
color = "#202020"       # around a picture that leaves room, or the wallpaper alone
gradient = ["#1e3c72", "#2a5298"]   # top to bottom, in place of color
video = "C:/Users/me/Videos/sea.mp4"
videos = ["C:/Users/me/Videos/hill.mp4"]   # played after video, one after the other
pause_on_battery = true # videos stand still on battery
daily = "bing"          # or "wikimedia": a picture of the day, fetched once a day
mirror = false          # make the picture the wallpaper of Windows too, put back on exit

[[wallpaper.display]]   # one display, counted from the left; before the table
number = 2
images = ["C:/Users/me/Pictures/second.jpg"]
fit = "center"

[[wallpaper.workspace]] # one workspace, the first being 1; before its display
number = 3
video = "C:/Users/me/Videos/focus.mp4"
```

| Key | What it does |
| --- | --- |
| `source` | `files` shows `images` and videos; `windows` the wallpaper of Windows (Tessera draws it only where the desktop window does not); `daily` a picture of the day; `color` the `color` or `gradient` alone. Without it: `files` where pictures or videos are given, else `daily` where `daily` is, else `color` where a color is, else `windows`. |
| `images`, `images_light`, `images_dark` | Picture files and directories (`.jpg`, `.png`, `.bmp`, `.gif`, `.tif`, `.webp`, and `.heic`, `.avif`, `.jxr` where Windows reads them). The light and dark lists replace `images` while Windows is light or dark, or by `dark_hours`. |
| `interval`, `shuffle` | How long a picture stays, and whether the order is random. |
| `fit` | How a picture is fitted: `cover`, `contain`, `stretch`, `center`, `tile`, `span`; `fill` and `fit` are the names Windows gives `cover` and `contain`. |
| `color`, `gradient` | `#rrggbb`: what a picture leaves free, or the wallpaper alone. |
| `video`, `videos` | Videos played in turn, again from the first; the pictures are shown where none can be played. |
| `pause_on_battery` | Videos stand still while the machine runs on its battery. |
| `daily` | `bing` or `wikimedia`; only then is anything fetched from the network. |
| `mirror` | The picture is made the wallpaper of Windows too, and the one before is put back as Tessera exits. |
| `[[wallpaper.display]]`, `[[wallpaper.workspace]]` | `number`, and `images`, `video`, `videos` and `fit` for one display or one workspace; an entry with no pictures or videos shows the wallpaper of Windows there. |

The widget `wallpaper` and the action `wallpaper` (`next`, the default, or `previous`) have every display show the next or the previous picture of its list at once.

`theme = "wallpaper"`, or `seed = "wallpaper"` in a theme file, derives the colors from the wallpaper, as `accent` does from the accent color.

## Notices, devices and privacy

Explorer tells of a few things that nothing else tells of once Tessera is the shell: a drive that was plugged in, a device that may be pulled out, a battery running low, and the balloons applications show at their icons of the notification area (those are always given, as only Tessera serving the icons receives them). Tessera gives these as notices of its own, listed with the [notifications](#notifications) and shown as banners where banners are on, with buttons for what can be done about them. `[notices]` says which:

```toml
[notices]
drives = true    # a drive plugged in, or a disc or a card put in, with Open and Eject
battery = true   # warnings at the battery levels of the power plan in use
```

Without `drives` or `battery`, each is on only while Tessera is the shell of the session, since Explorer tells of them otherwise. What an ejection came to is always told. A notice that is not urgent comes as a banner as a notification does: only where `banners = true` in `[notifications]` and do not disturb does not hold, staying `seconds` there. A refusal to eject and the warnings at the reserve and critical levels are urgent: they come as banners even while banners are off or do not disturb holds (`quiet` in `[notifications]`), and stay until they are put away.

The built-in layouts place the widgets that go with them before the notification area, as here:

```toml
[[surface]]
edge = "top"
thickness = 48
end = ["devices", "privacy", "tray", "volume"]
```

`devices` is an icon that is there only while something can be taken out of the machine; a click opens the popup `devices` with `devices-list`, a row for each drive and device with **Open** and **Eject**. `privacy` shows an icon for the microphone, the camera and the location while an application uses it, with a tip that names the applications; a click opens the popup `privacy` with `privacy-list`. Both popups can be replaced by a `[[popup]]` of the same name, or the lists put into another one.

## The clock and the calendar

```toml
[[surface]]
edge = "top"
thickness = 48
center = [{ widget = "clock", popup = "calendar", format = "%a %-d %b  %H:%M", zones = ["Europe/Berlin", "Tokyo", "Eastern Standard Time"] }]
```

`zones` names time zones in any of three ways, in small or capital letters: an identifier of the IANA database (`Europe/Berlin`, `America/New_York`, old ones such as `Asia/Calcutta` and `US/Eastern` too), a zone as Windows names it (`W. Europe Standard Time`, as in the time zone settings of Windows) or a city (`Berlin`, `New York`, `São Paulo`, the cities that name zones and some large ones that do not, such as `Munich` or `Mumbai`). While the pointer rests on the clock, its tip gives the date and, for each zone, its city, the time there and how far it is from universal time, as `Tokyo 22:05 UTC+09:00`; the calendar lists the same under the month. Windows is asked for the time in a zone, with the rules of daylight saving time it keeps for every year, so a zone Windows does not have tells no time. A name that names no zone is reported in the log and left out.

```toml
[[popup]]
name = "calendar"
width = 620
height = 520
content = [{ widget = "calendar", week_start = "monday", week_numbers = true, calendars = ["C:/Users/me/Documents/family.ics", "webcal://example.org/holidays.ics"] }]
side = ["notifications"]
```

`week_numbers = true` puts the week of the year before each row, counted as ISO 8601 does: the week of its Thursday. `calendars` names iCalendar files (`.ics`) by their path, and feeds by an `https://` or `webcal://` address (a `webcal:` feed is read over HTTPS); they are read when a calendar first names them and every half hour after, on a thread that may wait, up to 16 MB each. Their events put a dot under each day they take place on; a click on a day lists its events under the month, those of all day first, then by their start, each with its place. Times in universal time and in a zone named by `TZID` (an IANA identifier or a zone of Windows, as Outlook writes them) are shown in local time; a `TZID` Tessera does not know is read as local time. Recurring events follow `RRULE` with `FREQ=DAILY`, `WEEKLY`, `MONTHLY` or `YEARLY` and `INTERVAL`, `COUNT`, `UNTIL`, `BYDAY` (such as `MO,WE`, `-1FR` or `4TH`), `BYMONTHDAY` and `BYMONTH`; `EXDATE` takes times out, an event with `RECURRENCE-ID` moves one, and `STATUS:CANCELLED` takes one away. A rule with parts Tessera does not follow, such as `BYSETPOS` or `FREQ=HOURLY`, shows the event at its first time only. Tessera has no holidays of its own: a public feed of the holidays of a country, which many calendar services offer as an `.ics` address, is named here like any other.

