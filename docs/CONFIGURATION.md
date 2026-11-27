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

## Actions

A `[[key]]` can name an action instead of one of its other fields: `action` is its name and `arg`, where the action takes one, what it is done with. The modules of Tessera register the actions, so one comes with the feature that brings it, and a name that no module registered, or an `arg` the action refuses (one that takes none refuses every `arg`), is reported when the configuration is read, like any other mistake in a `[[key]]`. An action may ask for its keys to be taken from Windows where Windows keeps them to itself (as it does with Win+D, Win+V or Win+E); Tessera then takes them with its keyboard hook, as it does for `switch`, `dock` and `files`.

```toml
[[key]]
press = "win+v"
action = "popup"      # the popup `arg` names, opened or closed
arg = "clipboard"
```

An action may also bring keys of its own, the keys Windows has for the same thing (as Win+M for minimizing every window). They are bound where `windows_keys = true` is at the top of `config.toml`, or, without that key, when Tessera is the shell of the session; `windows_keys = false` leaves them out. A `[[key]]` that presses the same keys wins over such a key, whatever it does. No action brings keys yet.

From a console, `tessera do <action> [arg]` does an action in the running shell, as `tessera do popup launcher`; the words after the name are joined by spaces into the `arg`.

| Action | `arg` | What it does |
| --- | --- | --- |
| `popup` | the name of a popup | opens that popup, or closes it when it is open; its keys are taken from Windows |
| `desktop` | `toggle`, `minimize`, `restore`, `others`, or none | without `arg` or with `toggle`, shows the desktop, or brings the windows back, as the widget `desktop` does: through the taskbar of Explorer where it runs, and otherwise by minimizing the application windows itself and restoring those still minimized on the next press; the keyboard goes to the desktop, and the windows come back in their order with the one that was in front active. `minimize` minimizes every application window (as Win+M), `restore` brings back those Tessera minimized (as Win+Shift+M) and `others` minimizes all but the window in front (as Win+Home); these three are Tessera's own even where Explorer runs, so `restore` does not bring back what the taskbar of Explorer put aside. Its keys (Win+D, Win+M, Win+Home) are taken from Windows |
| `trash` | `open`, `restore`, `empty`, or none | without `arg` or with `open`, has Explorer open the Recycle Bin, as a click on the widget `trash` does; `restore` asks the user first and, on yes, brings every item in it back to where it was deleted from; `empty` empties the bins of all drives once the user said yes in the dialog of Windows. It brings no keys |
| `files` | a folder, as `files` names one, or none | opens a window of the file manager there, as `files = "..."` does; without `arg`, at the folder of the user; its keys (Win+E) are taken from Windows |
| `shell` | `install`, `revert` or `explorer` | `install` makes Tessera the shell of the user's next sessions and `revert` gives that back to what the machine names, as `tessera shell install` and `tessera shell revert` do; `explorer` hands this session to Explorer now, when Tessera is its shell: Explorer starts as the shell once Tessera has quit |
| `run` | none | opens the Run dialog, the popup `run`, or closes it; its keys (Win+R) are taken from Windows, and Win+R is bound to it by default where the keys of Windows are in use |
| `runner` | none | opens the compact runner, the popup `runner`, or closes it; it brings no key |

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
