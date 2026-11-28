# Configuration

The keys below have their rows in the settings (the `settings` popup), each in the section named after what it sets, drawn from the rows files of `crates/tessera/src/modules/settings/rows/` ([DECLARED-SETTINGS.md](DECLARED-SETTINGS.md)): the top-level keys under General (and `rule_templates` under Window rules), `[notifications]`, `[flyout]` and `[notices]` under Notifications, `[tiling]` and `[switcher]` under Windows, `[workspaces]` under Workspaces, `[files]` under Files, `[night_light]` and `[performance]` under Displays and power, `[startup]` under Session, `[system]` and the options of each placed widget under Widgets, and `[wallpaper]` under Wallpaper; each other part of Tessera adds the rows of its own tables. The search at the top of the rail finds a row by its label in the language shown or in English, and General > Elsewhere opens this file itself. Every change the settings make keeps the comments of the file, is checked as the shell reads the file before it is written, and is preceded by a backup in `%APPDATA%\tessera\backups` (once a minute at most, the newest 20 kept); General > Backups exports, imports and restores them. A key without a row is written here by hand, as before.

Everything a surface shows is a widget provided by a module. The compiled-in modules provide the widgets `clock`, `calendar`, `label`, `launcher`, `apps`, `power`, `notifications`, `settings`, `media`, `media-controls`, `network`, `connections`, `radio-switch`, `wifi`, `bluetooth`, `bluetooth-button`, `volume`, `volume-slider`, `microphone-slider`, `volume-mixer`, `audio-outputs`, `brightness`, `brightness-slider`, `cpu`, `memory`, `battery`, `disk`, `network-meter`, `files`, `command`, `tray`, `problems`, `problem-list`, `tiling`, `quit`, `user`, `profile`, `places`, `workspaces`, `overview`, `focused`, `keyboard`, `appearance`, `desktop`, `plan`, `hdr`, `trash`, `update`, `marketplace` and `windows`; extensions add more. The network meter is `network-meter`: `network` is the connection widget of the network module, and a configuration that names the meter `network` gets that widget, as it always did, since that module registers the name first.

The layout is read from `%APPDATA%\tessera\config.toml`. It lists the surfaces — screen edge, thickness, all displays or only the primary one, and whether the surface slides away while the pointer is elsewhere (`hide = true`, or `hide = "overlap"` to do so only while a window lies in its place; it then reserves no screen space and comes back when the pointer reaches that edge of the display) — and the widgets in the start, center and end of each. A widget is written as its name, or as a table that adds options the widget understands (the clock takes a `format`, the calendar a `week_start`, a label an `icon` and a `text`, the tray a `limit` and whether it shows `tips`) and a `popup` to open when the widget is clicked. Popups are declared in the same file as a name, a size and a list of widgets, so any widget, including one from an extension, can open a popup or be placed in one. One popup is open at a time, and popups cannot be resized. Without that file the [built-in layout](../crates/tessera/src/default_config.toml) is used; it documents the format. A file that fails to parse is ignored as a whole, and an unknown widget name is skipped; both are reported on standard error, which only a debug build started from a terminal shows.

The options of the widgets in the tables and lists below are also in the settings: **Widgets** has a card of the options of each placed widget, which **Configure…** in the menu of a widget of a bar opens, and the page **Popups** with every `[[popup]]` and its widgets; **Search** has the runners of the launchers, their order, the web shortcuts, the exchange rates and the history.

A surface stands at any edge: `edge` is `"top"`, `"bottom"`, `"left"` or `"right"`; at the left or the right its slots stand from the top down. `displays` is `"all"` (the default), `"primary"`, `"others"` (every display but the primary one), or one display or a list of them, each a number, its position from the left counted from 1; a text that begins with `\\.\`, the output GDI names, as `'\\.\DISPLAY2'`; or any other text, the name of the monitor as the display settings of Windows show it, where `*` stands for any text and `?` for one character, or which a regular expression after `re:` describes, compared without regard to case: `displays = [1, "DELL*"]`. A table `[surface.per_display."<display>"]`, `<display>` written as one entry of `displays`, gives the surface another `thickness`, `hide`, `start`, `center` or `end` on the displays it names; where several name a display they are applied in the order of their names as text, the last winning for what it gives. `show_delay` and `hide_delay` are the milliseconds the pointer rests at the edge before a surface that hides comes back (0) and is elsewhere before it goes (700), each at most 10000. `opacity`, from 0.3 to 1, draws a background of that strength behind the widgets, which a surface without it does not have; `length`, from 0.2 to 1, is the share of its edge a surface takes, and `align` (`"start"`, `"center"`, the default, or `"end"`) where a shorter one stands along it; `floating = true` or `false` gives the widgets of the surface fields of their own or none, whatever `flat` of the theme says; `solid_when = "maximized"` turns the background opaque while a maximized window touches the surface. Several surfaces may share an edge of a display, one after the other from the edge in the order of the file; those that hide stand beyond those that keep the edge. A `per_display` selector that names no display, such as `"0"` or `" "`, makes the file rejected. The widget `spacer` is empty room, `size` logical pixels long (12), or with `flexible = true` all the room left in its slot of the start or the end; `separator` is a thin line across the surface. A click with the right button on a surface opens a menu that changes these keys of it in the file, moves, removes and adds widgets, and adds and removes surfaces; on the widgets `windows`, `tray`, `trash`, `wallpaper` and `workspaces`, which answer the right button themselves, it opens with Shift held.

The `tray` widget shows no description for an icon whose application draws its own (one of version 4 that did not ask for the description with `NIF_SHOWTIP`); the application is told when the pointer comes onto the icon and leaves it instead, whatever `tips` says. Where no taskbar of Explorer exists, as when Tessera is the shell, the notification area and the screen edges that programs reserve are served by Tessera itself; there is nothing to configure for that, and the room the bars leave to windows follows from the bars of the layout and of other programs.

The layout file and the theme file it names are checked every second, and a change is applied about two seconds after the file was written, without a restart: surfaces are added, removed and resized, their widgets are built again and slide in, and an open popup closes. A changed file that fails to parse changes nothing and is reported the same way. Changes to the `[icons]` table and to the settings of extensions go the same way but have not been tried on a running shell yet. Extension files and icon image files are not watched.

 If the environment variable `TESSERA_CONFIG_DIR` is set, that directory is used instead of `%APPDATA%\tessera` for everything read from it: the layout, themes, languages, icons and extensions.

`icon_packs = ["line", "mine"]` at the top of `config.toml` names the [icon packs](ICON-PACKS.md) in use, by the names of their directories in `%APPDATA%\tessera\icon-packs`; for each icon of an application or a kind of file the first pack that has one gives it, below the `[icons]` table and above the icon the program or the file brings. Without the key no pack is used. The marketplace writes it with **Use** and **Stop using**, and **General** in the settings puts the installed packs in order, adds and takes them off; a change of the list is applied like the rest of the file; the files of the packs named are looked at every two seconds, so a picture that is replaced shows without a restart. The `marketplace` widget takes `show`, one kind or a list of kinds of `extensions`, `themes`, `icon-packs`, `wallpapers` and `tiling-layouts` (every kind, with a tab each, without it), and `columns`, 1 to 4; see [Registries](EXTENSIONS.md#registries).

`theme = "accent"` names no file: it derives every color from the accent color of the Windows settings, dark or light as those settings say for the taskbar, and the settings popup offers it as **Windows accent**. A theme file can do the same with `seed = "accent"`, or derive the colors from a color of its own with `seed = "#rrggbb"`; what it sets under `[colors]` is applied over the derived colors. The roles are tones of palettes with the hue of the seed, as in Material 3, but built in the OKLCH color space instead of Material's HCT, so they are near the colors the Material tools give, not equal to them. A theme file chooses between them with `scheme = "dark"`, the default, `"light"` or `"system"`, which follows the Windows setting for the taskbar; a light scheme without a seed is derived from the green of the built-in colors. While the colors follow the accent color or that setting, both are asked for again every second. Choosing the accent theme and a light theme file were tried on a running shell, on a machine set to dark; changing the accent color or the mode while it runs was not, since that would have changed the settings of the machine it was tried on. On a light bar the white notification icons that applications draw for a dark taskbar are hard to see.

Whatever the theme, the colors that are read (text and icons, the accent, outlines) are then made lighter or darker by as little as gives them the contrast WCAG 2.1 asks for at level AA to the surfaces of the theme: 4.5 to 1 for text and the accent, 3 to 1 for outlines; hue and saturation stay, and a color that already has the contrast is not touched. `contrast = false` in the layout file, or the switch under **General** in the settings, leaves a theme exactly as written. A bar of a theme that leaves out the fields behind its widgets (`flat`) lies on the desktop, so there the bar looks at the desktop under it: every five seconds the desktop window of Explorer draws the strip under the bar into a bitmap, and a widget whose text does not have 4.5 to 1 to all of the desktop under it fades its field back in, on which the colors of the theme are read; where the wallpaper is calm and dark or light enough, the widget stays without one. What is looked at is the desktop window (the wallpaper of Windows, a `[wallpaper]` of Tessera, the icons of the desktop), not a window that lies over it: a bar is not kept readable over a window, and a live wallpaper that another program draws in a window of its own was not tried. The colors of application and tray icons and what an extension draws with colors of its own are not counted, and no size of text is checked.

`theme = "name"` in that file applies `%APPDATA%\tessera\themes\name.toml` over the built-in theme. Besides `seed` and `scheme`, a theme can replace single colors, as `#rrggbb` under `[colors]`, using the role names `surface_container`, `surface_container_highest`, `on_surface`, `on_surface_variant`, `outline`, `primary` and `secondary_container`. It can also replace lengths, in pixels: the corner radii under `[shape]` (`small`, `medium`, `large`, `full`), the distances under `[spacing]` (`tiny`, `small`, `medium`), and the measurements of the parts under `[bar]`, `[dock]`, `[calendar]`, `[slider]`, `[meter]` and `[launcher]`; the text under `[typography]` (`family`, the name of an installed font, `label_size` and `icon_size`); and the three springs every animation uses, `[motion.spatial]`, `[motion.spatial_fast]` and `[motion.effects]`, each with a `stiffness` from 20 to 5000 and a `damping_ratio` from 0.2 to 2, where 1 settles without overshoot and less bounces. A value outside what is usable is reported and left as built in; a key the theme format does not know makes the whole file unusable, which is reported too. The font of the icons cannot be replaced. `flat = true` under `[bar]` leaves out the fields behind the widgets of a bar, so that they stand on the wallpaper; what marks a state, such as the shown workspace, stays. [`themes/`](../themes) holds three themes to copy and start from, `expressive`, `compact` and `void`, whose comments name the keys; the registry of this repository offers them too. `void` is flat and goes with the arrangement `Void`: a thin top bar and a small dock, after the desktops of tiling window managers. Its bar starts with the `focused` widget, the name of the program whose window is in front (or the title of that window, with `show = "title"`). There is no widget for the weather, and nothing is drawn on the desktop. It was tried on a running shell on one display. `expressive` and `compact` were tried on a running shell, where the colors, corners, sizes and font changed as the files say; how their springs move was not compared by eye, and the reports about unusable values were tried only in tests.

`language = "de"` in that file, or the choice under **General** in the settings, takes the texts from `%APPDATA%\tessera\languages\de.toml`, or, where that is absent, from `languages\de.toml` beside `tessera.exe`, where the download keeps them; any name of letters, digits, `-` and `_` other than the built-in `en` and `ru` names such a file. The file gives `name`, what the language calls itself, and under `[texts]` the English texts of Tessera with what they are in the language; a text it leaves out stays English. `months`, `months-of` (the months as they follow a day, where they differ), `short-months`, `weekdays` and `short-weekdays` (Monday first) are optional lists for the calendar and the clock. A file with an unknown key or a list of the wrong length is refused with a line in the log and Tessera speaks English. The files are looked for when the settings open and a changed file is read again when the layout is. [`languages/`](../languages) holds German (`de`), Spanish (`es`), French (`fr`) and Ukrainian (`uk`), each a template for others: a test keeps them in step with the texts Tessera has, but the wording was written by a machine and no native speaker has reviewed it. Tried once on a running shell with the files beside the program and none in the configuration directory: with `language = "uk"` the clock of the bar showed the date in Ukrainian; the other texts of those files were not looked at on screen. As with the built-in languages, what the widgets of the bars say stays English.

`theme_layers = ["dense", "round"]` stacks theme files over the base theme, in order, each applied over the result of those before it; `[theme_options.<theme name>]` holds the values of the settings a theme declares, `[theme_options.builtin]` those of the built-in theme, which declares an accent color, the scheme, corner roundness, density, font, the height of the bar fields and the dock size. The exact rule by which layers, overlays and values are applied, and the settings of each bundled theme, are in [THEMES.md](THEMES.md). The settings change all of it under **Themes**. An extension that declares its settings with `[[setting]]` entries in its manifest shows them under **Extensions**, where they are written to `[extension.<id>]` as by hand; a declared value that does not fit is reported and replaced by the declared default ([DECLARED-SETTINGS.md](DECLARED-SETTINGS.md)). When only settings of extensions change, the open popups, the extensions and their widgets stay as they are; a running extension is told of the new values at once and its widgets are drawn again.

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
autoplay = "shell"    # AutoPlay for drives that arrive: "shell" (only as the shell), "always" or "off"
archives = "browse"   # ZIP files and the other archives Windows opens as folders are browsed, or "open" with their program
home = "tessera"      # Quick access at the side leads to Tessera's Home, or "windows" to Windows' own

[[key]]
press = "win+e"
files = "~"           # a path, "~" for the folder of the user, or "computer"
```

From a console, `tessera files`, `tessera files computer` and `tessera files D:\Projects` open a window in the running shell.

`tessera files` takes the command lines of `explorer.exe` too: `tessera files /select,"C:\x\a b.txt"` opens `C:\x` with the file chosen, `/e,`, `/n`, `/separate` and `/root,<folder>` open the folder they name, `shell:Downloads` and the other names of known folders open those folders, `::{20D04FE0-3AEA-1069-A2D8-08002B30309D}` opens This PC and `shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}` Home, a `search-ms:` address with a `location` searches for its words below that folder, and a `file:` address opens its path. A file named without `/select` is shown in its folder. Whatever is no folder of the file system (Control Panel, the Recycle Bin, libraries, other `::{CLSID}` items, a search without a folder) is handed to Explorer with the same words. When no Tessera runs, `tessera files ...` starts one (the same program, without arguments) and hands the words over as soon as it listens, waiting at most 15 seconds; only if Tessera does not start or does not listen in that time are the words given to Explorer, with `TESSERA_FILES_FALLBACK` in its environment, so that a folder Explorer hands back to Tessera within 30 seconds ends in a message box instead of going round. Several folders opened at once start one Tessera between them, and relative paths are read against the folder `tessera files` was run in. `tessera files install`, `revert` and `status` are commands of their own, so a folder called `install` is opened as `tessera files .\install`.

`tessera files install` has other programs open folders and drives in Tessera Files, for the user who runs it: it writes the verbs `open`, `explore` and `opennewwindow` of `Directory` and `Drive` under `HKEY_CURRENT_USER\Software\Classes` with the command `"<tessera.exe>" files "%1"` and no `DelegateExecute`, after keeping what the user had there under `HKEY_CURRENT_USER\Software\Tessera\FolderHandler`. `tessera files revert` puts that back and removes the keys Tessera made; `tessera files status` prints what each verb runs now. The action `files-default` with `on` or `off` does the same and says in a notice how it went. The class `Folder` is not touched, since virtual folders belong to it as well; [SHELL.md](SHELL.md#tessera-files-as-the-file-manager) tells what still opens in Explorer.

`autoplay` says when Tessera does AutoPlay for a drive that arrives (a USB stick, a memory card, a disc): by default only as the shell, since Explorer does it otherwise and both would ask. What the drive holds is looked at (its first 400 entries, at the top and one folder down), the handlers Windows has registered for it are read, and the choice made in Settings > AutoPlay is run at once; where Windows is to ask, the notice of the drive (or a notice of its own, where `[notices]` tells of no drives) offers "Choose what to do", which opens the popup `autoplay` with the handlers, "Open folder to view files" (in Tessera Files where `own = true`) and "Take no action". What is chosen there is kept for that kind of drive, as Windows keeps it. `DisableAutoplay` of the user and the policy `NoDriveTypeAutoRun` switch it off.

Tessera Files browses the whole namespace of the Shell, not only the folders of the file system: `tessera files control-panel` (the category view; `all-control-panel-items` the icon view), `libraries`, `network`, `gallery`, `windows-home` and `saved-searches` open those places, and so does a parsing name of the Shell: `::{CLSID}` and `shell:` names, a bare `\\server` (its shares; where the Shell cannot list them, the server is asked with `NetShareEnum`), `search-ms:` searches, `ftp://` sites the Shell still serves, `.library-ms` and `.search-ms` files and paths into a ZIP file. A place is read on a thread of its own, in batches of 64 as the Shell hands them over, so that a slow network or a sleeping phone holds up nothing else; a place that cannot be read shows the Shell's reason. Items that list nothing, as most applets of Control Panel, open with their default verb, as a double click in Explorer does; where that verb starts Explorer (Windows 11 sends Devices and Printers to Settings), Explorer shows it. A place named by a parsing name that turns out to be a folder of the file system, This PC or the Recycle Bin opens as such. Sessions keep a place of the Shell by its parsing name. A command line of `tessera files` that names only such a place, as `tessera files ::{26EE0668-A00A-44D7-9371-BEB064C98683}`, opens it in Tessera Files rather than in Explorer, which gets it only where it cannot be read there. WebDAV opens through its UNC form, `\\host@SSL\path`; SFTP is not offered.

`archives = "browse"` shows what a ZIP file holds as a folder when it is opened, as Explorer does; 7z, rar, tar and gz are browsed as well on Windows 11 23H2 and later, and where Windows does not open an archive as a folder, it is opened with its program after all. `archives = "open"` opens every archive with its program. `home = "windows"` has Quick access at the side lead to Windows' own Home (`shell:::{f874310e-b6b7-47dc-bc84-b9e6b38f5903}`), with the favourites and recommendations Windows keeps, in place of Tessera's.

The places at the side beyond Quick access, which stays Explorer's own, are kept in `files-places.toml` of the configuration directory (`%APPDATA%\tessera`), which the file manager writes itself: `[[place]]` entries with `name`, `location` (a path, a word as above or a parsing name) and `group` (`places`, `remote`, `devices` or `searches`), and `hidden = [...]`, the groups not shown. Without the file, Places holds the Gallery, the libraries and Control Panel, and Remote holds Network. The menus of the side and of a pane add a place, rename and remove one, a place is dragged among them, and the menu of a heading shows or hides each group.

`tessera files home` (or `files = "home"`) opens Quick access: the folders Explorer keeps there and the files used lately; `tessera files computer` opens the drives and `tessera files recycle-bin` the Recycle Bin. With `restore = true`, `tessera files` and a `files` key without a folder open the tabs of the last window closed instead of the folder of the user; they are kept in `%LOCALAPPDATA%\tessera\files-session.toml`, with the shares of other computers visited, which the side shows under Network. With `per_folder = true` the looks of the folders are kept in `%LOCALAPPDATA%\tessera\files-views.toml`, the 500 seen last; neither file is configuration, and either can be deleted to forget what it holds. `explorer = true` reads the folder options of Explorer (`HideFileExt`, `Hidden` and `ShowSuperHidden` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced`) each time a folder is read; with `explorer = false` every name is shown as it is and protected files of the system show with the hidden ones. `hidden = true` shows hidden files whatever Explorer says; Ctrl+H switches them for a pane.

`[file_view]` sets the pictures of files and the panes beside the panes of files:

```toml
[file_view]
thumbnails = true         # pictures of what files hold, where icons are 32 pixels or larger
folder_thumbnails = true  # pictures of what folders hold too
thumbnail_size = 256      # the largest side asked of the Shell, in pixels of the display, 32 to 256
overlays = true           # the overlays of OneDrive, Git clients and other programs over icons
preview = false           # windows open with the preview pane (Alt+P)
details = false           # windows open with the details pane (Alt+Shift+P or F11)
preview_handlers = true   # programs' preview handlers draw PDF, Office and other kinds
preview_timeout = 5.0     # seconds a preview handler is waited for, 1 to 60
```

The columns of the details are kept per folder whatever `per_folder` says, in `%LOCALAPPDATA%\tessera\files-views.toml` beside the looks, the 500 used last; the Recycle Bin, Home and the results of a search keep one set each. A change of `[file_view]` applies to the windows that are open the next time they draw; `preview` and `details` only to windows opened afterwards.

### Searching, and the actions of the file menus

A search of the file manager (Enter in the filter field) reads what is typed as Explorer's Advanced Query Syntax: bare words and `"quoted phrases"` that names hold, `kind:` (`documents`, `pictures`, `music`, `videos`, `folders`, `archives`), `modified:` or `date:` (`today`, `yesterday`, `this-week`, `last-week`, `this-month`, `last-month`, `this-year`, `last-year`, `week`, `month` and `year` back from today, a day such as `2026-01-31` with `>`, `>=`, `<` or `<=`, or `2026-01-01..2026-01-31`), `size:` (`empty`, `tiny` up to 10 KB, `small` up to 100 KB, `medium` up to 1 MB, `large` up to 16 MB, `huge` up to 128 MB, `gigantic` above, a size with a comparison as `>10MB`, or `1MB..10MB`), `author:`, `tag:`, `ext:` or `type:` (`ext:png,jpg`) and `content:` (`content:invoice` or `content:"q1 plan"` must be in what a file holds; `content:on` looks for the bare words in what files hold as well as in their names). For example `report kind:documents modified:this-month content:budget`. The chips under the field write the same words. The last 20 searches are kept in `%LOCALAPPDATA%\tessera\files-searches.toml`, which is no configuration and can be deleted; "Save search" writes a `.search-ms` file into the Searches folder of the user.

`[[file_action]]` adds an entry to the menus of files of the file manager and of the desktop, before the verbs of the Shell, for the items it fits; the settings edit them on a page of the section Files:

```toml
[[file_action]]
name = "Optimize the picture"          # what the menu calls it
command = "oxipng.exe -o 4 %F"         # the program and its arguments
match = { extensions = ["png"], kind = "file", min = 1, max = 50 }
submenu = "Pictures"                   # actions of one submenu are grouped under it
icon = "E91B"                          # a glyph of Segoe Fluent Icons, an .ico or .png, or "C:\\Tools\\tool.exe,0"
terminal = false                       # run in a console window that stays open
elevated = false                       # run as an administrator
confirm = "Optimize the pictures?"     # asked before it runs

[[file_action]]
name = "Open a console here"
command = "wt.exe -d %d"
match = { kind = "folder", max = 1, background = true }
```

In `command`, `%f` is the first path chosen, `%F` all of them, `%d` the folder of the first, `%n` its name, `%N` its name without its extension, `%u` its `file:` address and `%%` a `%`; paths are put in quotes unless the command has them in quotes already (`"%f"`), and environment variables such as `%USERPROFILE%` are expanded. The command starts in the folder of the first item. `match` takes `names` (patterns such as `["*.png", "README*"]`, any of which each item's name matches, in any case), `kind` (`"file"`, `"folder"` or `"any"`, the default), `extensions` (any of which each file has, without the dot), `min` and `max` (how many items are chosen; at least one) and `background` (also offered in the menu of the folder shown, which is then the item). An action whose command uses `%f`, `%F`, `%d` or `%u` is not offered for items that have no path, such as This PC.
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
show_desktop = false  # a Show desktop entry after the windows

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
| `show_desktop` | `false` (default), `true` | `true` adds a **Show desktop** entry after the windows, which does what the action `desktop` does: through the taskbar of Explorer where it runs, and otherwise by minimizing the application windows and bringing them back on the next press |

`style` also takes `cover` and `flip`, which stand the windows on a stage, with the title of the chosen one under it. `cover`: the chosen window large in the middle, its neighbours to both sides, each 0.8 the size of the one nearer to the middle, further out and fainter, as many as the stage has room for. `flip`: the chosen window in front at the bottom, the windows after it stacked upward and to the side behind it, each 0.8 the size of the one in front of it, higher and fainter; up goes to the window behind, down to the one before. Windows draws the picture of a window as an upright rectangle, so the styles move, scale and fade flat pictures and do not turn them in 3D; no two pictures overlap where the choice rests. Windows that a `[[rule]]` gives `skip_switcher = true` are not offered (nor by Task View).

Rows that do not fit on the display scroll to the chosen window. While the switcher is open:

- typing narrows the windows to those whose title, or the name of whose program, has every word typed, whatever the case; the line of what was typed stands above the windows, Backspace takes the last letter back, and Esc forgets all of it (a second Esc closes the switcher). While the keys of the switcher are held, as Alt is with Alt+Tab, a letter is taken as typed although Windows writes nothing for it;
- the wheel moves the choice one window on for each notch down, and back for each notch up;
- Delete or Ctrl+W asks the chosen window to close, and a click with the middle button the window clicked, as its close button does. A window that goes, or only hides, within a second and a half is no longer offered; one whose application asks the user first, or refuses, stays;
- Tab and Shift+Tab move the choice when the switcher was opened with no key held;
- an action `switcher` with `app-next` or `app-previous` narrows the open switcher to the windows of the application of the chosen window, and one with `next` or `previous` offers all of them again.

The arrows, Enter, Esc, Delete and Backspace are taken with the keyboard hook whatever has the keyboard, so they reach no other program while the switcher is open. Letters, Tab, Ctrl+W and the wheel reach the switcher while its window has the keyboard: Tessera brings it in front when it opens, and asks Windows again in other ways where Windows refuses at first; where Windows still refuses, those go to the window in front. The settings show the style, the scope and the minimized windows on the **Window switcher** card of the **Windows** part.

## Task View

The action `taskview` opens Task View, or closes it: every display is dimmed and shows the windows that stand on it as live pictures, spread so that they are as large as the display gives room for, each keeping the shape of its window. Where the keys of Windows are in use (`windows_keys = true`, or Tessera being the shell) Win+Tab opens it unless a `[[key]]` presses those keys. `present-windows` does the same without the cards of the workspaces along the top and brings no key; hot corners and `[[key]]` entries can name either.

```toml
[[key]]
press = "ctrl+alt+tab"
action = "present-windows"
arg = "app"            # the windows of the application in front
```

| `arg` | What it shows |
| --- | --- |
| `workspace` (or none) | the windows of the workspace each display shows |
| `all` | the windows of every workspace of Tessera's own, grouped under the name of their workspace; with the virtual desktops of Windows, the windows of the desktop shown |
| `app` | the windows of the application of the window in front |

While it is open:

- typing narrows the windows of every display to those whose title, or the name of whose program, has every word typed, in the line of what is typed on the display the pointer is on; Backspace takes the last letter back and Esc forgets all of it (a second Esc closes Task View). Where no window has what is typed, the runners of the launcher answer it under the line (not the runners of windows, processes and power), the arrows up and down choose an answer and Enter or a click does it;
- the arrows go to the picture beside the chosen one, Tab and Shift+Tab to the next and the one before, and Enter or a click brings the window in front, going to its workspace first;
- Delete, Ctrl+W, a click with the middle button or a click on the close button over a picture asks its window to close;
- along the top a card for each workspace, Tessera's own or a virtual desktop of Windows, shows its name and the icons of its windows: a click goes to that workspace, a picture dragged onto a card of Tessera's own workspaces has its window live there, the card with a plus after the others makes a workspace (as `workspace` with `add`), and the cross on the corner of a card under the pointer removes its workspace (as `workspace` with `remove`);
- a click on no picture, Esc, or the keyboard going to another program closes Task View.

A minimized window has no picture to give and shows its icon on a card; one hidden on another workspace of Tessera's own shows the still picture taken as it was hidden. Windows that a `[[rule]]` gives `skip_switcher = true` are left out.

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
| `edit-zones` | the name of a `[[snap.layout]]`, or none for a new layout | opens the editor of snap zones on the display under the pointer, as [snapping](#snapping-window-rules-and-hot-corners) describes; Win+Alt+Z is bound by default where the keys of Windows are |
| `tiling` | the words of a `tiling` key, as `reserve left` or `width more` | does what a `[[key]]` with those words as `tiling` does, as [tiling](#tiling) describes; brings Win+Shift+J, L, I and K (`reserve left`, `right`, `up`, `down`) and Win+Alt+0 (`reset`) where the keys of Windows are bound |
| `window-menu` | none | opens the window menu of the window in front; Alt+F3 is bound by default where the keys of Windows are |
| `kill-window` | none | covers the displays; a click on a window asks whether its program is to be ended at once, as [the window menu](#snapping-window-rules-and-hot-corners) describes; Ctrl+Alt+Esc is bound by default where the keys of Windows are |
| `files` | a folder, as `files` names one, or none | opens a window of the file manager there, as `files = "..."` does; without `arg`, at the folder of the user; its keys are taken from Windows, and it brings Win+E where `windows_keys` is on |
| `files-default` | `on` or `off` | `on` has other programs open the folders and drives of the file system in Tessera Files, as `tessera files install` does; `off` gives that back, as `tessera files revert` does. A notice says how it went |
| `share` | paths separated by `\|` | opens the share window of Windows (nearby sharing, mail, the applications that take files) for those files and folders, over the window of Tessera in front. Brings no key |
| `map-drive` | none | shows the dialog "Map Network Drive" of Windows, with "Reconnect at sign-in" checked |
| `disconnect-drive` | the letter of a network drive, or none | disconnects that drive and forgets it for the next sign-in, with the questions of Windows where files on it are open; without `arg`, shows the dialog of Windows that lists the network drives |
| `add-network-location` | an address, `\\server\share` or `https://...`, with `\|name` after it where it is to be named, or none | makes a network location under This PC, as the wizard of Explorer does (a folder shortcut in Network Shortcuts), and opens This PC; without `arg`, opens that wizard of Windows |
| `shell` | `install`, `revert` or `explorer` | `install` makes Tessera the shell of the user's next sessions and `revert` gives that back to what the machine names, as `tessera shell install` and `tessera shell revert` do; `explorer` hands this session to Explorer now, when Tessera is its shell: Explorer starts as the shell once Tessera has quit |
| `run` | none | opens the Run dialog, the popup `run`, or closes it; its keys (Win+R) are taken from Windows, and Win+R is bound to it by default where the keys of Windows are in use |
| `runner` | none | opens the compact runner, the popup `runner`, or closes it; it brings no key |
| `winx` | none | opens the menu of Win+X, or closes it: at the widget `winx` where a bar has one, else at the Start button (the button that opens `launcher`), else at the left end of a bar. Brings Win+X |
| `power-dialog` | none | opens the dialog that asks what the computer is to do (switch user, sign out, sleep, hibernate, shut down, restart, and while updates wait update and shut down or update and restart); nothing is done before OK or Enter. Brings no key: where `windows_keys` is on, Alt+F4 opens it while the desktop is in front (no window, or the desktop window), and Alt+F4 elsewhere still closes the window in front |
| `display-mode` | `pc-screen-only`, `duplicate`, `extend`, `second-screen-only`, or none | without `arg` opens the popup `display-mode`, the choice of Win+P; with one switches the displays to that mode at once. Brings Win+P |
| `display-settings` | none | opens the popup `display-settings`, the [configuration of the displays](#displays-power-performance-and-meters), or closes it. Brings no key |
| `power-settings` | none | opens the popup `power-settings`, the [power settings](#displays-power-performance-and-meters), or closes it. Brings no key |
| `inhibit` | `on`, `off`, or none | blocks sleep and screen locking until it is switched off or Tessera quits, or lets go of that; without `arg` the other way than it is. Brings no key |
| `performance` | `off`, `minimal`, `extreme`, `auto`, or none | puts that [performance mode](#displays-power-performance-and-meters) in force by hand, or without `arg` the next one; `auto` gives the choice back to `[performance]`. Brings no key |
| `input-language` | `next`, `previous`, `list`, or none | asks the window in front for the next keyboard layout, or the one before with `previous`; where a window of Tessera is in front, Tessera takes on that layout itself. `list` opens the popup `keyboard-list`, whose choice is for the window that was in front before it. Brings Win+Space and Win+Shift+Space (`previous`) |
| `open` | a file, a program, a folder or an address | opens it through the Shell, as a double click does. Brings Win+I (`ms-settings:`), Win+U (`ms-settings:easeofaccess`), Win+K (`ms-settings-connectabledevices:devicediscovery`), Win+G (`ms-gamebar:`), Win+Shift+S (`ms-screenclip:`) and Win+Pause (`ms-settings:about`) |
| `start`, `search`, `quick-settings`, `notification-center` | the name of a popup, or none | opens that popup, or closes it; without `arg`, for `start` (Ctrl+Esc) the popup of `windows_key`, the same one the Windows key opens, or the launcher where `windows_key` names none, `launcher` for `search` (Win+S, Win+Q), `quick-settings` (Win+A; the built-in layout has this popup with the tiles of the [quick settings](#quick-settings), and for a layout without it the module brings one with the connections, the switches of Wi-Fi and Bluetooth, the volume and the brightness) and `calendar` (Win+N) |
| `screenshot` | none | saves a picture of all displays as `Screenshot (N).png` in the Screenshots folder of Pictures, N counted on from where Windows counts. Brings Win+PrtScn |
| `dock` | 1 to 9 | does what `dock = n` does: what a click on the button of that number in the dock does. Brings no key |
| `pin` | the path of a program, a shortcut, a file or a folder, a link, `separator` or a name of the Start menu | pins it to the dock, after the pins, unless it is pinned; `tessera pin` does the same from a console. Brings no key |
| `unpin` | what `pin` takes | unpins it, a path whatever its slashes and case; `tessera unpin` does the same. Brings no key |
| `import-taskbar-pins` | none | appends the pins of the taskbar of Windows that are not pinned yet, as the menu of the dock does. Brings no key |
| `pin-verb` | `on` or `off` | writes **Pin to Tessera dock** into the menus of files, folders and shortcuts, for the user only, or takes it away. Brings no key |
| `quiet` | `toggle`, `on`, `off`, `morning`, a number of minutes, or none | switches [do not disturb](#notifications): without `arg` or with `toggle` the other way than it is, `on` until it is switched off, `morning` until eight in the morning, `30` for thirty minutes; `off` also lets go of what holds it by itself until that ends |
| `notify` | the text of a notice | posts a notice of Tessera's own, listed with the notifications and shown as a banner where banners are: `tessera do notify Backup done \| 3 files copied` has the title before ` \| ` and the text after it |
| `clipboard` | none | opens the history of the clipboard in the popup `clipboard`, or closes it, and notes the window in front as the one to paste into; its keys (Win+V) are taken from Windows, and Win+V is bound to it where `windows_keys` binds the keys of Windows and `[clipboard]` had the history on as Tessera started |
| `night-light` | `on`, `off`, `toggle`, or none | switches night light on or off, or over without `arg` or with `toggle`: that of Windows or Tessera's own, as `[night_light] source` says |
| `volume` | `up`, `down`, `mute` or `mute-microphone` | turns the default output a step up (and lifts its mute) or down, as Windows counts steps for it (2 of 100 for most), mutes it or lifts its mute, or mutes the default microphone or lifts its mute; the flyout shows what came of it where `[flyout]` wants it. |
| `brightness` | `up` or `down` | makes the display under the pointer a tenth brighter or darker, a monitor over DDC/CI or the panel of a laptop, and shows it in the flyout where `[flyout]` has `brightness = true` |
| `media` | `toggle`, `play`, `pause`, `stop`, `next` or `previous` | passes the command to the player chosen in `media-controls`, or to the one Windows considers current. |
| `wallpaper` | `next`, `previous`, or none | has every display show the next picture of its list (without `arg` too), or the one before, at once; the interval begins anew |
| `wallpaper-settings` | none | opens the settings of the wallpaper, or closes them where they are open |
| `eject` | the letter of a drive, as `E` or `E:` | ejects that drive as its **Eject** button in the popup `devices` does: the device it is on, with every drive of it, or its medium where it is a disc or a card in a reader built in; a notice says how it went |
| `<extension>.<shortcut>` | none | tells the extension of that id that the keys of its `[[shortcut]]` of that id were pressed, as [EXTENSIONS.md](EXTENSIONS.md#shortcuts) describes; it brings the `keys` of the shortcut where the keys of Windows are in use. The shortcuts are read when Tessera starts |
| `extension-answer` | an extension and the id of one of its answers | tells the extension that its answer was chosen in the launcher, which starts `tessera do extension-answer` for an answer whose action is `run`; it brings no key |
| `switcher` | `next`, `previous`, `app-next`, `app-previous`, or none | opens the [window switcher](#the-window-switcher), or goes one window on (`next`, or none) or back (`previous`) in the open one; `app-next` and `app-previous` go through the windows of one application only. The choice is taken when the keys to hold of the `press` that did it are let go; for a `press` with none, the switcher stays open until a window is chosen. Done by `tessera do switcher`, it waits for the keys that are down as it is done, which are usually none. Its keys are taken from Windows, and it brings Alt+Tab (`next`), Alt+Shift+Tab (`previous`), Alt+` (`app-next`) and Alt+Shift+` (`app-previous`) where the keys of Windows are in use |
| `install-resource` | a `tessera://` link | follows the link: `tessera://install?registry=…&kind=…&id=…` opens the popup `install-resource`, which shows what that registry offers under that name and installs it only with a click; `tessera://open?settings=…&item=…` opens the settings at that part and card. Registered links of a browser come here; see [links that install](EXTENSIONS.md#links-that-install). Brings no key |
| `workspace` | `next`, `previous`, `left`, `right`, `up`, `down`, a number from 1 to 16, `add`, `remove` or `rename` | goes to the workspace after or before the one shown, beside it in its row or above or below it in its column (see `rows` and `wrap` under [workspaces](#workspaces)), or to the one of that number, on the display of the window in front; `add` makes a workspace at the end of the row and goes there, `remove` removes the one shown, `rename` opens the field that names it. For the virtual desktops of Windows the steps press Explorer's shortcuts, `add` and `remove` press Win+Ctrl+D and Win+Ctrl+F4, and `rename` is not offered. Its keys are taken from Windows, and it brings Win+Ctrl+Right (`next`), Win+Ctrl+Left (`previous`), Win+Ctrl+Up (`up`), Win+Ctrl+Down (`down`), Win+Ctrl+D (`add`) and Win+Ctrl+F4 (`remove`) where the keys of Windows are in use |
| `send-window` | `next`, `previous`, `left`, `right`, `up`, `down` or a number from 1 to 16 | sends the window that was last in front to that workspace of Tessera's own without going there; the virtual desktops of Windows take no window of another program. Brings Win+Ctrl+Shift+Right (`next`) and Win+Ctrl+Shift+Left (`previous`) |
| `sticky` | none | has the window in front seen on every workspace of Tessera's own, or, when it is, live on the workspace shown again. Brings no key |
| `taskview` | `workspace`, `all`, `app`, or none | opens [Task View](#task-view), or closes it: the windows of each display spread as live pictures, with the workspaces along the top; `all` shows those of every workspace, `app` those of the application in front. Its keys are taken from Windows, and it brings Win+Tab where the keys of Windows are in use |
| `present-windows` | `workspace`, `all`, `app`, or none | the same without the workspaces along the top; it brings no key |
| `peek-desktop` | none | shows the desktop in front of every window, with a thin outline where each window is, until the keys to hold of its `press` are let go; done by `tessera do peek-desktop`, or by a `press` without keys to hold, it shows the desktop until it is done again or a click lands on it. The desktop is the wallpaper Tessera shows on each display (its picture with the adjustments, or a still of its video), or, where Tessera draws none, the picture Windows paints behind the windows (`PaintDesktop`); the icons of the desktop are not in it. Its keys are taken from Windows, and it brings Win+comma where the keys of Windows are in use |
| `tray-focus` | none | puts the keyboard on the [notification area](#the-notification-area) of the display the pointer is on; Win+B is bound to it by default where the keys of Windows are in use |
| `none` | none | does nothing; a `[[key]]` of it turns off a key an action brings, which then stays with Windows or whoever holds it (Tessera holds nothing for it). The Shortcuts page writes one for **Turn off** |
| `pause-keys` | `on`, `off`, or none | pauses every key combination of Tessera, those of the workspaces and the mouse buttons included, but the keys of `pause-keys` itself, with `on`; holds them again with `off`; does the other of the two without `arg`. It brings no key; give it one, as `press = "ctrl+alt+win+p"`, to have a way back |
| `activate-window` | `exe:` and a program (`exe:code.exe`, `.exe` may be left out, a path stands for its file name), `class:` and a window class, `title:` and a title where `*` stands for any letters and `?` for one, or `app:` and an AppUserModelID; with `,start` after `exe:` or `app:` | brings forward the window it names that was used last (the one nearest the top, then those hidden on Tessera's other workspaces), restored if it is minimized, going to its workspace first; with `,start` it starts the program, or the packaged application through `shell:AppsFolder`, where no window matches. The window menu writes one with **Shortcut for this window…**. It brings no key |
| `date-and-time` | none | opens the popup `calendar`, or closes it. Brings Win+Alt+D |
| `notifications-keyboard` | none | opens the popup `calendar`, the notification center, as Win+N does. Brings Win+Shift+V |
| `sound-output` | none | opens the popup `sound`, with the outputs to choose from. Brings Win+Ctrl+V |
| `window-height` | `full` or `back` | `full` stretches the window in front from the top to the bottom of the work area of its display, keeping its sides, and `back` gives it the height it had before (kept while Tessera runs). Brings Win+Shift+Up (`full`) and Win+Shift+Down (`back`) |
| `window-half` | `top` or `bottom` | puts the window in front in the top or the bottom half of the work area of its display; through `snap` where `snap` takes `top-half` and `bottom-half`. Brings Win+Alt+Up (`top`) and Win+Alt+Down (`bottom`) |
| `magnifier` | `in`, `out` or `close` | `in` starts Magnifier (`magnify.exe`) where it is not running; once it runs, Magnifier answers Win+Plus and Win+Minus itself. `close` closes the windows of Magnifier. Brings Win+Plus (`in`), Win+Minus (`out`) and Win+Esc (`close`) |
| `narrator` | none | starts Narrator (`narrator.exe`); Narrator is turned off by its own keys. Brings Win+Ctrl+Enter |
| `color-filters` | none | opens the page of the color filters in Settings (`ms-settings:easeofaccess-colorfilter`); Windows has no documented way to switch them. Brings Win+Ctrl+C |
| `quick-assist` | none | opens Quick Assist (`ms-quick-assist:`). Brings Win+Ctrl+Q |
| `feedback-hub` | none | opens Feedback Hub (`feedback-hub:`). Brings Win+F |
| `copilot` | none | opens Copilot (`ms-copilot:`). Brings Win+C |
| `bar-settings` | none | opens Tessera's settings at **Bars**. Brings Win+Alt+Enter |
| `find-computers` | none | opens the search for computers of the network (`::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}`) in Tessera's file manager where it takes that place, and in Explorer otherwise. Brings Win+Ctrl+F |
| `emoji-panel` | none | opens the popup `emoji-panel`, the runner with only its emoji runner: type `:` and a name, as `:heart`. It brings no key; bind Win+period to it where the emoji panel of Windows does not open without Explorer |

The keys a `press` can end with are a digit, a letter, `f1` to `f24`, `space`, `tab`, `enter`, `left`, `up`, `right`, `down`, `home`, `end`, `pageup`, `pagedown`, `insert`, `delete`, `backspace`, `escape`, `pause`, `printscreen`, and the keys a US layout has for `period`, `comma`, `plus`, `minus` and `backquote` (on another layout, the keys in their place).

A `press` can also end with the keys of the number pad, `num0` to `num9`, `multiply`, `add`, `subtract`, `decimal` and `divide`, with `semicolon`, `slash`, `backslash`, `leftbracket`, `rightbracket` and `quote` (as on a US layout), `scrolllock` and `menu` (the key of the context menu). Up to four combinations pressed one after the other, written with commas between them, make a sequence, as KDE Plasma has: `press = "ctrl+alt+k, t"`. The first holds keys; a later one may be a key by itself. Only the first is held; once it is pressed, the next ones are held for a second and a half, a tip at the pointer shows what was pressed so far, and Esc or the time running out ends it. A sequence whose beginning is the whole `press` of another entry never comes to its end, as that entry is done at once; the Shortcuts page marks both. Keys held with `middle`, `back` or `forward` press a mouse button: `press = "ctrl+back"`. Those are bound through Tessera's mouse hook, which does not take them yet, so they do nothing so far, and the Shortcuts page says so.

```toml
[[key]]
press = "ctrl+alt+k, t"   # Ctrl+Alt+K, then T
action = "activate-window"
arg = "exe:WindowsTerminal.exe,start"

[[key]]
press = "win+d"           # gives Win+D back to Windows
action = "none"
```

The settings have a page of their own for the keys, **Keys > Shortcuts** (also the popup `shortcuts`, as `tessera do popup shortcuts`): every action, by its title, with the keys that do it, those the actions bring marked, and what became of each: in use, given way to a key of the user, held by another program, taken from Windows, paused, or off because `windows_keys` is off. Keys that clash are marked with the other row. The search finds rows by title, action and keys. A click on a row opens it: **Change** records the new keys as they are pressed (every combination of Tessera is given up meanwhile, and the next keys pressed reach no program), **Add another key** records one more, **Turn off** writes a `[[key]]` of `none` for a key an action brings, **Reset** takes the user's entry out again, and **Edit** changes a key of the user whole. **Add a shortcut** makes a new key: any action, found by typing, with its argument checked as it is typed, a program with **Browse** and its arguments, a popup, the words of `tiling`, a side, a way through the windows, a button of the dock or a folder. **Export…** writes the user's `[[key]]` entries to a TOML file of their own, and **Import…** reads one back, added to them (an entry of the file takes the place of one that presses the same keys) or in their place. The page writes `config.toml` as the other settings do, keeping its comments, and leaves an entry alone where the file changed meanwhile. The titles of the actions are the `[[action]]` entries of the rows files of the settings; the shortcuts of extensions are listed by their names under **Extensions**.

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
| `extensions` | the answers of the extensions whose manifest has a `[runner]` table, as [EXTENSIONS.md](EXTENSIONS.md#runners) describes |
| `bookmarks` | the bookmarks of Edge, Chrome, Brave, Vivaldi, Chromium and Firefox, by their titles and addresses |
| `spell` | `spell necessary` or `necesary?`: the spelling of a word, by the spell checker of Windows in the languages of the user |
| `time` | `time`, `time in UTC+3`, `time tokyo`, `15:00 berlin in new york`; until the table of zones of the clock is there, only offsets from UTC and `local` |
| `terminal` | the profiles of Windows Terminal, opened with `wt -p` |
| `sessions` | the folders and workspaces opened lately in Visual Studio Code, its Insiders release and VSCodium |
| `unicode` | `U+2192`, `char right arrow`, `char →`: Unicode characters, copied when chosen |
| `help` | `?`: the runners of the widget and how each is asked |
| `extensions` | what the installed extensions answer, where an extension declares a runner |

The name of a runner, or one of the short names `calc`, `convert`, `run`, `window`, `kill`, `bm`, `tz`, `wt`, `code` and `char`, and a colon before the text ask that runner alone: `calc: 2+2`, `char: arrow`, `wt: ubuntu`. With nothing after the colon, the help tells how that runner is asked. A keyword of a web search keeps its meaning, so a keyword of `web` that is also the name of a runner searches the web. The Run dialog runs commands only: the other runners, and a runner asked by its name, answer in the launcher and the runner.

Before anything is typed the launcher shows what `view` says: `all` (the default: the tiles of every application), `list` (all of them one under another), `favorites` (the applications pinned to Start, which an `apps` widget shows) or `categories` (a rail of the folders of the Start menu beside the list of the applications in the chosen one; the shortcuts in no folder and the packaged applications have a category each). `tabs = true` puts a row of tabs above the list to switch between them; `view = "favorites"` and `view = "categories"` bring it unless `tabs = false`. A right click on an application or a file, the Menu key or Shift+F10 opens its menu: open, run as an administrator, open the file location, pin to Start or to the dock, add to the desktop, the page of a packaged application in the Settings app, uninstall (after a second Enter or click) and properties.

The search for files of the index of Windows Search takes filters, alone or with words: `kind:` (`document`, `picture`, `video`, `music`, `folder`, `program`, `email`, `link`, `contact`, `calendar`, `note`, `playlist`, `game`), `ext:pdf,docx`, `modified:>2026-01-01` (`>`, `>=`, `<`, `<=`, `=`, or `today`, `yesterday`, `week`, `month`, `year`), `size:>10MB` and `in:~/Work`; `content:` before words looks for them in what files hold, and where four letters or more name no file that is looked for by itself. With `folders`, where the index is not asked, the filters do nothing.

The widget `recommended` is what the Start menu of Windows 11 recommends: the applications whose shortcuts were made in the last `days` (7) and the files opened lately, at most `shown` (6), `columns` (2) in a row; `files = false` or `apps = false` leaves either out. Remove from list in the menu of an entry deletes the shortcut of a file in the Recent folder, or remembers an application in `recommended.toml` in the configuration directory (deleting that file shows them again).

```toml
[[popup]]
name = "launcher"
width = 640
height = 760
content = ["apps", "recommended", { widget = "launcher", view = "categories" }, "power"]
```

The `launcher` and `runner` widgets take these options besides those of the launcher (`placeholder`, `folders`, `view`):

- `runners`, the runners to ask, by the names of the table; all of them without the option.
- `web`, a table of keywords and addresses that adds web searches, as `web = { so = "https://stackoverflow.com/search?q={}" }`.
- `engine`, the keyword of the engine the last row searches the web with (`ddg`).
- `currency = false` keeps Tessera from fetching exchange rates; rates kept from before are still used (`true`).
- `recent = true` lists the documents opened lately while nothing is typed, in the list view (`false` for the launcher, `true` for the runner).
- `settings = false` leaves out the pages of the settings (`true`).
- `view`, what is shown before anything is typed: `all`, `list`, `favorites` or `categories` (`all`); `tabs` (`true` for `favorites` and `categories`, `false` otherwise), the row of tabs that switches it.

```toml
[[popup]]
name = "launcher"
width = 640
height = 720
content = ["apps", { widget = "launcher", view = "list", recent = true, engine = "so", web = { so = "https://stackoverflow.com/search?q={}" } }, "power"]
```

The Search page of the settings switches the runners on and off and puts them in an order, written as `runners` of every `launcher` widget and of the `runner` widgets that name runners already; it also adds and removes the keywords of `web`, switches `currency` and `recent` for every launcher, and clears the history. A chip there for each placed launcher opens its card in the Widgets section, where one launcher alone is set.

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

Win+L locks the session through Windows itself, and Ctrl+Shift+Esc, Alt+Shift and Ctrl+Shift are answered by Windows too, so they keep working in shell mode and Tessera does not take them. While `windows_key` takes over the Windows key, Ctrl+Esc opens the same popup, as it opens the Start menu in Windows, through `start` above, unless a `[[key]]` holds it or gives `start` another popup as `arg`. Without `windows_key` the Windows key by itself opens nothing, in shell mode as well; set `windows_key = "launcher"` for it to open the launcher. Win+E belongs to the file manager (`files`), Win+R to the Run dialog, Win+V to the clipboard history, Win+1 to Win+9 and Win+T to the dock, Win+B to the notification area (`tray-focus`), Win+Tab to Task View (`taskview`) and Win+Ctrl with the arrows, D and F4 to the workspaces (`workspace`), which press Explorer's own keys where the workspaces are the virtual desktops of Windows; each brings its own keys. The other keys Explorer answers are brought by the actions of the table below. Win+H (voice typing), Win+W (Widgets) and Win+period and Win+semicolon (the emoji panel) belong to components of Windows, not to Explorer, and are left to Windows; whether each still answers without Explorer is not known yet. In shell mode `emoji-panel` can be bound to Win+period, as `press = "win+period"` with `action = "emoji-panel"`.

| Keys | Action | What they do |
| --- | --- | --- |
| Win+Alt+D | `date-and-time` | the popup `calendar` |
| Win+Shift+V | `notifications-keyboard` | the popup `calendar`, the notification center |
| Win+Ctrl+V | `sound-output` | the popup `sound`, with the outputs |
| Win+Shift+Up, Win+Shift+Down | `window-height` | the window in front stretched to the full height, and its height given back |
| Win+Alt+Up, Win+Alt+Down | `window-half` | the window in front in the top half, or the bottom half |
| Win+Plus, Win+Minus, Win+Esc | `magnifier` | Magnifier started (it then answers Win+Plus and Win+Minus itself), and closed |
| Win+Ctrl+Enter | `narrator` | Narrator started |
| Win+Ctrl+C | `color-filters` | the page of the color filters in Settings |
| Win+Ctrl+Q | `quick-assist` | Quick Assist |
| Win+F | `feedback-hub` | Feedback Hub |
| Win+C | `copilot` | Copilot |
| Win+Alt+Enter | `bar-settings` | Tessera's settings at Bars |
| Win+Ctrl+F | `find-computers` | the search for computers of the network |
| Win+H | none | left to Windows (voice typing); not known to answer without Explorer |
| Win+W | none | left to Windows (Widgets); not known to answer without Explorer |
| Win+period, Win+semicolon | none | left to Windows (the emoji panel); not known to answer without Explorer; `emoji-panel` can be bound to them |

Win+comma belongs to the peek at the desktop (`peek-desktop`), which shows the desktop while the keys are held.

## Tiling

`[tiling]` with `enabled = true` puts the windows of each display side by side in the space the bars leave. Each workspace of each display has tiles of its own: a layout, and the sizes given to its tiles, which `%APPDATA%\tessera\tiling-state.toml` keeps over restarts (Tessera writes it two seconds after the last change; it is not meant to be edited).

```toml
[tiling]
enabled = true
layout = "master"      # or columns, rows, grid, monocle, bsp, dwindle, ultrawide, centered, or a layout file
gap = 8                # between tiles, 0 to 200
padding = 8            # around the tiles; the gap without it; or { top = 4, right = 8, bottom = 8, left = 8 }
margin = { top = 0 }   # room left free at the sides of what the bars leave, 0 to 1000
resize_step = 0.05     # share of its room a tile gains or gives per step, 0.01 to 0.5
drag = "swap"          # a tile dropped on another trades places with it; "sort" takes its place in the order
float = { width = 0.6, height = 0.7 }   # a floated window goes to the middle at this share of the space
border = { width = 3, offset = 0, color = "#7aa2f7" }   # drawn by Tessera; border = true has Windows color its own
stack_bar = "as-needed"   # the bar of tabs over a stack: "always" (over every tile), "as-needed" (two windows or more) or "never"
stack_bar_height = 32     # how high the bar is, 16 to 96
auto_stack = ["browser"]  # a new window of a category the window rules give joins a tiled window of it; true: every category

[[tiling.display]]
match = "2"            # the second display from the left
layout = "ultrawide"
gap = 12

[[tiling.workspace]]
number = 3             # counted from 1
display = "1"          # without it, on every display
layout = "monocle"
```

The layouts: `master` (the first window on the left half, the others stacked on the right), `columns`, `rows`, `grid` and `monocle` (every window takes the whole space) as before; `bsp`, where each new window divides the tile of the window in front in two along its longer side; `dwindle`, where it divides the last tile, across and down in turn, so that the tiles wind inward; `ultrawide`, the first window in the middle, twice as wide as each side, the next ones stacked on its right and, once there are three, on its left; and `centered`, one column of windows in the middle, as wide as a display of 16 by 9 as high as the space, with room on both sides only where that leaves more than a tenth.

What decides the layout of a workspace: the layout chosen there while Tessera ran, then the last `[[tiling.workspace]]` entry that names it, then the last `[[tiling.display]]` entry that names its display, then `layout`; `gap`, `padding` and `margin` are taken the same way. `match` and `display` name a display by its place from the left, counted from 1, by its device name (`\\.\DISPLAY2`) or by the name of its monitor (a pattern with `*` and `?`, or a regular expression after `re:`), as the `displays` of a surface do. A display is kept in `tiling-state.toml` under its device name, a workspace under its lasting number, so a workspace that is removed hands what it kept to none of the others; what an older Tessera kept by the place of a display or a workspace is taken over by the display and the workspace at that place the first time they are tiled. When the configuration is read again and says something else of the layouts, the layouts chosen while Tessera ran give way to it.

A `[[key]]` with `tiling`, `tessera tiling <words>` and the action `tiling` (with its words as `arg`) take: `next` (the next layout on the workspace of the display in front, through the layouts of Tessera and then the layout files, then no tiling; after no tiling, tiling again with the layout each workspace had), `off`, a layout (a layout file is looked for when it is chosen, and a name without one is reported then), `float` (the window in front out of the tiles and back in), `wider` and `narrower` (the first window of `master` gets more of the width, or less; in `rows` the window in front more of the height; in the others the window in front more of the width), `width more`, `width less`, `height more` and `height less` (the window in front gets `resize_step` more or less of the nearest room around it that is divided that way, keeping at least a twentieth of it), `reset` (every tile of the workspace alike again), `reserve left`, `right`, `up`, `down` (the next window opens on that side of the window in front) or `reserve float` (the next window is not tiled), and `split horizontal` or `split vertical` (the next window divides the tile of the window in front across or down). In `bsp` and `dwindle` the reserved window divides the tile; in the other layouts, which are made again for each number of windows, left and up put it before the window in front in the order, right, down and `split` after it. The display in front is that of the tiled window in front, or else the one the pointer is on. Where the keys of Windows are bound (`windows_keys`), the action brings Win+Shift+J, L, I and K for reserving the left, right, upper and lower side and Win+Alt+0 for `reset`, while tiling is on; while it is off they are left to other programs.

Dragging a border between two tiles with the mouse, or a program resizing the window in front, moves that border in every layout; a window that is moved without being resized and let go with its middle on another tile trades places with that window, or takes its place with `drag = "sort"`, and one let go on another display goes to the tiles there.

Windows can be stacked, so that they take turns in one tile below a bar of tabs. The `tiling` words for it: `stack left`, `right`, `up` or `down` (the window in front joins the stack of the tile on that side, or makes one with its window), `unstack` (the window in front out of its stack, into a tile of its own after the stack's), `cycle-stack next` or `cycle-stack prev` (the next or the previous window of the stack in front) and `reserve stack` (the next window that opens joins the stack of the window in front). Where the keys of Windows are bound, the action brings Win+Ctrl+Alt with an arrow for `stack`, Win+Alt+U for `unstack`, Win+Alt+Period and Win+Alt+Comma for `cycle-stack next` and `prev`, and Win+Shift+U for `reserve stack`, while tiling is on. The window in front of a stack is placed below the bar and the others just behind it at the same place, not hidden, so that the dock, the switcher and Task View still show them; one of them that comes in front another way, as by Alt+Tab, becomes the one in front of its stack. On the bar a click brings a window in front, a middle click closes it, the wheel goes through the tabs, and a tab dragged down or up out of the bar takes its window out of the stack; a window let go of over the bar of a stack joins it. `stack_bar` says when the bar is drawn and `stack_bar_height` how high, in pixels of the screen; the tiles of a stack with a bar shrink by its height. With `auto_stack`, a window that opens joins the stack of a tiled window of the same `category` of the window rules (see [window rules](#snapping-window-rules-and-hot-corners)) on the same workspace of the same display, or of the first that comes with it; `"*"` among the categories is every one. `tiling-state.toml` keeps the stacks by the program, class and title of their windows, and a stack is made again once two of its windows are found, after a restart too; a window that changed its title is found by its program and class.

The **Tiling** card of the **Windows** section of the settings sets these keys, and the page **Tiling of displays and workspaces** after it adds, changes, moves and removes the entries of `[[tiling.display]]` and `[[tiling.workspace]]`, each with its layout, gap, padding and margin; a display is chosen there by its place. Every change is checked as the tiling reads it before it is written.

### Layout files

A layout file is a `.toml` file in `%APPDATA%\tessera\tiling-layouts`, chosen by its name without `.toml` (`layout = "my-layout"`); `tall`, `wide` and `three-columns` come with Tessera ([crates/tessera/src/tiling/layouts](../crates/tessera/src/tiling/layouts)), and a file of the same name takes their place. A file is a tree of containers under `[structure]`:

```toml
name = "Three columns"     # what the widget shows; the name of the file without it

[structure]
kind = "horizontal"        # its parts side by side; "vertical" one above the other

[[structure.children]]
kind = "leaf"              # one window
priority = 2               # the parts take windows from the lowest priority on (in the order of the file)
condition = { windows_at_least = 3 }   # or windows_at_most; the part is there only then

[[structure.children]]
kind = "leaf"
grow = 2                   # what it weighs against the other parts of its container (1)
priority = 1

[[structure.children]]
kind = "vertical"          # a container without parts takes `max` windows, or all that are left
priority = 3
```

A part of `kind = "stack"` holds as many windows as its `max`, or all that are left without one, in one tile below a bar of tabs, as a stack the user makes does; it has no parts. So a layout can say that the windows after the first few go into a stack: the bundled `tabbed` has the first window on the left, the second on the top right and every other one in a stack below it. `unstack` and a tab dragged out of its bar do nothing to a stack of a layout file.

A part that gets no window is left out. A file needs a container without `max` and without `condition` (in containers without one either), which takes the windows the others do not. Files are read when Tessera starts, with the configuration, and, while windows are tiled, within two seconds of a change; a file that cannot be used, one named as a layout of Tessera's own and one larger than 64 KB are reported in the problems and left out, and where a layout names a file that is not there, `master` is used.
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

The windows Tessera snapped on one display (halves, quarters, zones and layouts) share their edges, as the custom tiling of KWin and the snap groups of Windows do: when one of them is resized by its border along an edge it shares with others, their touching edges follow it while it is resized, and exactly once it is let go. Edges share when they lie `gap` apart within two logical pixels; a line runs on through the windows whose same edge lies on it too, so the middle line of four quarters moves all four, and windows that only meet at a corner do not follow. Each window keeps the least size it answers when asked (`WM_GETMINMAXINFO`, for 100 milliseconds at most, away from the thread that draws). Tessera hears the sizes of a window being resized from Windows (`EVENT_OBJECT_LOCATIONCHANGE`, asked for that window only between the start and the end of the resize, at most sixty times a second), and moves the others together in one step on a thread of its own. A window dragged away from where it was snapped leaves the group.

```toml
[snap]
edge_switch = true        # a dragged window pushed against an edge goes to the workspace beside
edge_switch_delay = 600   # milliseconds

[[snap.layout]]
name = "wide"             # what the editor and the settings call it
zones = [[0, 0, 0.7, 1], [0.7, 0, 0.3, 1]]
```

`edge_switch = true` has a window dragged against the outer left or right edge of the displays, or their top or bottom where the workspaces stand in rows, for `edge_switch_delay` milliseconds (600, from 100 to 5000) go to the workspace beside the one shown, and the workspace with it, as "Switch desktop on edge: only when moving windows" of Plasma does; the window stays seen while the workspaces change. An edge another display lies beyond never switches, and the pointer has to come 40 pixels back from the edge before it switches again; letting go right after a switch does not snap. It works with Tessera's own workspaces (`[workspaces] own`): the virtual desktops of Windows take no window of another program.

The action `edit-zones` (Win+Alt+Z where the keys of Windows are bound) opens the editor of snap zones over the work area of the display under the pointer, as Meta+T opens the editor of the custom tiling in KWin; with the name of a `[[snap.layout]]` as `arg` it opens that layout. A click in a zone splits it at the pointer, side by side or one above the other as chosen at the top; a line between zones is dragged to move it, or clicked with **Merge** to make the zones beside it one; the gap between the zones is set there too. Enter or **Save** saves the layout under its name, as a `[[snap.layout]]` with `name` (and `[snap] gap`), or as a layout file of the tiling, `%APPDATA%\tessera\tiling-layouts\<name>.toml`, whose containers lie as the zones do with their shares as `grow`, the windows going to the zones in the order they are read and the last zone taking every window the others do not; a file of that name is replaced, and the tiling reads it again by itself. Esc cancels. A layout whose zones cannot be cut with straight lines from side to side is changed in `config.toml` only. A layout without `name` is called by its number among the `[[snap.layout]]` entries, from 1; saving one of the six layouts of Windows writes it as a layout of the file, after which the file's layouts are shown in their place, and renaming a layout puts it last. **Settings > Windows** has the keys of `[snap]` under **Snapping**, and under **Snap zones** each layout with a small picture, **Edit on screen**, **Remove** and **New layout** with the display the editor opens on, and the `[[snap.zone]]` entries of Shift, which are written by hand.

Each `[[rule]]` names windows by `exe` (the file name of the program), `app` (the AppUserModelID of the packaged application, as `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App`), `title` and `class` (the window class), each a pattern in which `*` stands for any text and `?` for one character, whatever the case of the letters, or a regular expression after `re:`, and says what is done to them when they are first seen: `float = true` leaves them out of the tiles and `tile = true` tiles them whatever `[tiling]` says of their program; `workspace` (from 1) sends them to a workspace of Tessera's own (the virtual desktops of Windows take no window of another program); `display` (from 1 from the left) to a display; `position` (`"center"` or `[x, y]`) and `size` (`[width, height]`, in shares of the work area) place them; `maximize`, `above` (kept above the other windows), `opacity` (0.1 to 1) and `border = false` (no thin border, Windows 11) do what they say. `apply = "always"` does it again whenever the window changes, as far as it differs, which keeps a window where its rule puts it; otherwise a window that moves itself within a second of being shown, as many programs do to go back to where they were, is put back once. Every rule that names a window applies in the order of the file, a later one having the last word. Windows that are open already when Tessera starts or the rules change only get the rules that apply always. A tiled window is not placed by a rule. The windows of the applications of the Universal Windows Platform, such as Settings and Calculator, all belong to one program, `ApplicationFrameHost.exe`, and one class, `ApplicationFrameWindow`, so `exe` and `class` name all of them at once; `app` names one. A program that gives its window its title a moment after showing it gets what a `title` rule says as soon as the title comes, within the first second; a title that comes later needs `apply = "always"`. A window that has no border by a rule keeps none when the tiling colors the border of the window in front. `skip_switcher = true` keeps a window out of the window switcher and Task View; `skip_dock = true` out of the dock. A rule without a name, with an `opacity` out of range, or with a `size` beyond the work area is reported and left out.

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

A rule names windows also by `path`, the whole path of the program (as `C:\Program Files\Mozilla Firefox\firefox.exe`), and says how all names of its table are matched with `match`: `"glob"` (the default, with `*`, `?` and `re:` as above), `"equals"` (all of the name), `"starts"`, `"ends"`, `"contains"` or `"regex"`; small and capital letters are alike in all of them, and `*` is a star where `match` is not `glob`. `not = { ... }` is a table of names the window must not have, `any = [{ ... }, { ... }]` a list of tables of which one is to fit and `all = [...]` one of which all are to fit; their tables take the same keys, nest, and match as the table around them unless they give `match` themselves. Besides what is said above, a rule can minimize a window (`minimized = true`), have it cover all of its display without title bar or frame (`fullscreen = true`), keep it below the other windows (`below = true`: it is lowered again whenever another window comes in front, which a thread of Tessera's own is told of by Windows), draw it `inactive_opacity` opaque (0.1 to 1) while another window is in front, take its title bar away and keep the border that resizes it (`no_titlebar = true`; a program that draws its own title bar goes on drawing it), keep it within `min_size` and `max_size` (`[width, height]` in logical pixels), and give it a `category`, a word such as `"browser"` or `"game"` for what groups windows later. Tessera gives a window its title bar, its frame and its place back when the rule that took them is gone or changed and when Tessera quits. `min_size` and `max_size` are applied when a window first appears, when the rules change, after the window actions of Tessera (below) moved or resized it, and when the user stops moving or resizing it by its title bar or border. `sticky = true` shows the window on every workspace of Tessera's own, as the action `sticky` does; the virtual desktops of Windows cannot be asked to, which is reported once. `apply = "remember"` does what the rule says when the window is first seen, as `"initially"` does, and the next window the rule names opens where the last one was, maximized if it was: where each rule's last window was is kept in `rules-state.toml` beside `config.toml`, written two seconds after it changed. A rule that keeps a window both `above` and `below`, or whose `min_size` is larger than its `max_size`, is reported and left out.

```toml
[[rule]]
any = [{ exe = "chrome.exe" }, { exe = "msedge.exe" }]
not = { title = "Picture?in?picture" }
match = "equals"
category = "browser"
apply = "remember"

[[rule]]
path = 'C:\Games\'
match = "starts"
fullscreen = true
inactive_opacity = 0.85
```

`rule_templates`, a key at the top of `config.toml`, switches on sets of rules that come with Tessera, as the app templates of Seelen UI: `"browsers"` (the category `browser`, and the library and page information of Firefox float), `"games"` (the launchers of the game stores and the windows of anti-cheat services and game overlays float), `"password-managers"` (float), `"system-dialogs"` (dialogs of Windows and of every program, the progress of copying, Task Manager and the prompts for passwords float), `"development"` (the categories `editor` and `terminal`, and the dialogs of JetBrains tools float), `"adobe"` (every window of a program in an Adobe folder floats) and `"picture-in-picture"` (picture-in-picture windows of browsers float above the others, always). They come before the rules of `config.toml`, so a rule of the user has the last word. A name that is no set is reported.

```toml
rule_templates = ["system-dialogs", "picture-in-picture", "games"]
```

More of a rule (window-rules-editor). `enabled = false` keeps a rule in the file and out of use. `type` names the kind of window its styles tell it is: `"normal"`, `"dialog"` (a window of the dialog class of Windows, `#32770`, or a popup with a title bar that another window owns), `"utility"` (a window with a title bar that another window owns), `"tool"` (a tool window, `WS_EX_TOOLWINDOW`), `"splash"` (a popup without a title bar that no window owns and that is the only window of its program) or `"popup"` (another popup without a title bar); it is compared as it is and takes no `match`. The rules see only the windows a task switcher lists, which have no owner and are no tool windows, so `"utility"` and `"tool"` name none of those; the editor of the rules still tells them of a picked window. `apply = "now"` does a rule once to the windows that are open when the configuration is read, at Tessera's start or when the rules change, and to no window that comes later; `apply = "force"` is another word for `"always"`. `accept_focus = false` gives the window `WS_EX_NOACTIVATE`, so that a click or a program does not give it the keyboard; `closeable = false` greys Close of its system menu, which greys its close button too, while Alt+F4, which the program answers itself, still closes it. Tessera gives both back when the rule no longer says so and when it quits, and only where it took them. `icon = "<file>"` and `app_id = "<identifier>"` are the picture and the application the dock shows and groups the window by. `focus` keeps a window from taking the foreground by itself (KWin's focus stealing prevention): `"none"` leaves it to Windows, `"low"` lets it, `"medium"` and `"high"` give the foreground back to the window that had it and flash the newcomer in the dock, and `"extreme"` lets it in front only when it was clicked. Input counts as the user's when it came at most half a second ago and the last click went to the program of the window, or the keys went to a window of that program; at `"medium"`, also when the program was started at most ten seconds after the user clicked or typed in Tessera, as in its launcher, dock or Start menu. A thread of its own hears of every window that comes in front and, having no right to the foreground itself, gives it back by joining the input of the window's thread for that moment, or else after a press of Alt; whether Windows always lets it is not yet tried on Windows.

The editor of the rules is under **Window rules** in the settings: it lists the rules of `config.toml` in their order, each switched on and off with its switch, and adds, duplicates, takes away and moves them (by dragging, or with the arrows). Each rule is changed with a control for each name and each key, and the editor says how many windows the rule names now and why the configuration would leave it out before it is written; it writes one rule at a time and keeps the comments of the others. **Pick a window** hides the settings and has a window clicked (Escape or the right button cancel), and **Choose an open window** lists those open; what is read of it (program, class, title, path, packaged application, kind, place and size) is offered to tick, and **Apply to this window until it closes** gives what is set to that window alone, without writing it. **Import…** adds the rules of a file of `[[rule]]` entries after the others if every one of them can be used; **Export…** writes the rules to such a file. The sets of `rule_templates` are shown with their rules, each with **Copy to my rules**. The action `edit-window-rule <window>` opens the editor with that window read (its number as the window menu knows it), and with `,temporarily` after it offers to give the rule to that window until it closes.

```toml
[[rule]]
type = "splash"
above = true
position = "center"

[[rule]]
exe = "chat.exe"
focus = "high"
accept_focus = false
apply = "now"
```

The window menu also keeps the window below other windows or no longer, shows it on all workspaces of Tessera's own or no longer (**On all workspaces**, as the action `sticky` does), has it fill its display or no longer (as `fullscreen` does), hides or shows its title bar (as `no_titlebar` does), moves or resizes it with the arrow keys as Move and Size of its system menu do (Windows does this until Enter or Esc), and **Kill process** asks whether its program is to be ended at once. **Create a rule for this window** names the window word for word with `match = "equals"`, by its packaged application, or by its program and its class, or by its title and class where the program is the frame host or cannot be read, and writes `fullscreen`, `below` and `no_titlebar` too when the window is so.

The action `kill-window` (Ctrl+Alt+Esc where the keys of Windows are bound) covers every display with a veil and a crosshair, as xkill of KDE does: the window under the pointer is outlined, and a click on it asks, naming its program, its title and its process, whether the program is to be ended at once, as End task of Task Manager does; what it has not saved is lost. Enter or **Kill process** ends it, and Esc, **Cancel** or the right button leave without ending anything. A program that runs as administrator cannot be ended by a Tessera that does not, which is reported. For an application of the Microsoft Store, whose window Windows shows from ApplicationFrameHost.exe, the process of the application itself is ended, not the host of the windows of all of them; where that process is not found, nothing is ended. A program that ended while the question was open is not looked for again: nothing is ended then either.

`[window_actions]` moves and resizes windows with the pointer while a key is held, as the window actions of KWin do; all of it is off without the table, also when Tessera is the shell, because it takes presses from other programs. With `modifier = "alt"` or `"win"` held, the left button pressed anywhere on a window of another program moves it and the right button resizes it from the corner nearest to where it was pressed; the window gets nothing of those presses, and of the key alone nothing either, so that Alt does not open its menu and the Windows key not the start menu when let go. A maximized window is restored around the pointer first, which stays as far across and down it as it was. A minimized window and one that covers its display, as a game does, get the press as if no key were held; a window the tiling places is left alone, and the press is taken all the same. The window switcher, task view, start menu and search of Windows and the menus that pop up are never taken from. Tessera moves the window sixty times a second, and while it does, its sides are drawn to the edges of the displays and of their work areas, and to the edges of the other windows beside them, within `snap_distance` logical pixels (10; 0 for none). Windows that Windows itself moves, by their title bar or border, are not drawn to edges: Windows moves them in a loop of its own that a program outside them cannot adjust without being loaded into them. The presses are taken with a low-level hook of the pointer on a thread of its own that decides at once and never waits; Windows does not call it for presses meant for a program that runs as administrator, so such windows are not moved. `titlebar_middle` says what a click with the middle button on a title bar does: `"lower"` puts the window below the others, `"minimize"`, `"close"`, or `"none"` (the default); Tessera asks the window whether the click is on its title bar (`WM_NCHITTEST`, aside and for 60 milliseconds at most), and the window gets the click as well. `focus_follows_mouse = true` has Windows give the keyboard to the window the pointer rests on for `focus_delay` milliseconds (300, at most 5000), and bring it in front too with `focus_raises = true`: that is the setting "Activate a window by hovering over it with the mouse" of Windows, which Tessera sets until the user signs out and gives back as the user chose it when it quits, or at its next start if it was ended without doing so.

```toml
[window_actions]
modifier = "alt"
snap_distance = 10
titlebar_middle = "lower"
focus_follows_mouse = true
focus_delay = 300
focus_raises = false
```

More of `[window_actions]` (window-rules-editor). `placement` says where a new window goes, once, when it is first shown, if it has a title bar, is no owned dialog, is not tiled and no rule places it: `"windows"` (the default: where Windows and the program put it), `"centered"`, `"under-pointer"` (its middle at the pointer, kept within the work area), `"zero-corner"` (the top left corner of the work area), `"smart"` (where it covers the least of the other windows on its display), `"maximized"` or `"cascade"` (each a step below and to the right of the one placed before, back in the corner when the step would leave the work area). `titlebar_double` says what a double click on a title bar does: `"windows"` (the default, what Windows does), `"maximize"` (or restore), `"minimize"`, `"close"`, `"lower"`, `"above"` (kept above the others or no longer) or `"nothing"`. `titlebar_wheel` says what a turn of the wheel over a title bar does: `"nothing"` (the default), `"opacity"`, `"above-below"` (away from you keeps it above or ends keeping it below, toward you the other way round), `"workspace"` (sends it to the next workspace of Tessera's own or the one before) or `"minimize-restore"` (away maximizes, toward restores a maximized window and minimizes one that is not). With the key of `modifier` held, `modifier_wheel` over any window does `"opacity"`, `"workspace"` (goes to the next workspace or the one before) or `"volume"`, and `modifier_middle` does `"close"`, `"minimize"`, `"lower"` or `"window-menu"` (brings the window in front and opens the window menu); both are `"nothing"` by default and need a `modifier`. `inactive_click = "activate-only"` has a click on a window that is not in front bring it in front only, the window getting nothing of the click; `"windows"` (the default) passes it. The title bar is asked of the window with `WM_NCHITTEST`, for 50 milliseconds at most and never of a window that hangs, and only near the top of the window; a press or a turn of the wheel is taken only where an action is set for it.

```toml
[window_actions]
placement = "smart"
titlebar_double = "above"
titlebar_wheel = "opacity"
modifier = "alt"
modifier_wheel = "volume"
modifier_middle = "window-menu"
inactive_click = "activate-only"
```

Each `[[corner]]` names a corner or an edge (`at`: `top-left`, `top-right`, `bottom-left`, `bottom-right`, `top`, `bottom`, `left`, `right`), optionally one `display` (from 1 from the left), a `delay` in milliseconds the pointer rests there first (150, at most 5000), and one of `action` (with `arg`) as a `[[key]]` names one, `popup` or `run`. It is done once, and again only after the pointer has gone 40 pixels away; nothing is done while a button of the pointer is held, as while a window is dragged, or while the window in front fills the display. A corner of its own beats an edge it lies on. The pointer is looked at from a thread of its own ten times a second, fifty times while it is near a corner or edge in use, and not at all while there is no `[[corner]]`.
Each `[[corner]]` names a corner or an edge (`at`: `top-left`, `top-right`, `bottom-left`, `bottom-right`, `top`, `bottom`, `left`, `right`), optionally one `display` (from 1 from the left), a `delay` in milliseconds the pointer rests there first (150, at most 5000), a `cooldown` in milliseconds it waits after it was used before it can be used again (0, at most 5000; Plasma's reactivation delay), and one of `action` (with `arg`) as a `[[key]]` names one, `popup` or `run`. It is done once, and again only after the pointer has gone 40 pixels away; nothing is done while a button of the pointer is held, as while a window is dragged, or while the window in front fills the display. A corner of its own beats an edge it lies on. The pointer is looked at from a thread of its own four times a second, fifty times only while it is within 8 pixels of an edge of a display that has a corner in use, and four times a second again while a button is held or the window in front fills the display under the pointer; there is no such thread while there is no `[[corner]]`.

**Screen edges** in the settings draws the displays as they stand, each with its name and its four corners and four edges; a click on one shows what the pointer resting there does. The action is chosen from every action there is, with the titles of the Shortcuts page (a name without a title shows as itself), or **Run a program** with its command line; what it is done with is checked as a `[[key]]` is before it is written. **This display** and **All displays** set `display`, and the two steppers `delay` and `cooldown`. The page writes `[[corner]]` entries, keeping the comments above them; a spot that does nothing gets one once an action is chosen for it, for the display it is on where there are several. A `popup = "..."` entry shows as the action `popup`, and is written as one once its action is changed.

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

With `volume = true` the flyout also names the output Windows takes as the default for ordinary sound whenever it takes another, whoever chose it (`IMMNotificationClient`, heard while the flyout is wanted), and with `microphone = true` the default microphone the same way.

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

### Adjustments, collections and the settings

Each of these keys is one of `[wallpaper]` and of each `[[wallpaper.display]]` and `[[wallpaper.workspace]]` entry; an entry takes what it leaves out from the table. The popup `wallpaper-settings` (the action of the same name opens it) sets most of them.

```toml
[wallpaper]
collection = "Sea"      # the collection all displays show, in place of images and videos
blur = 8                # pixels; 0 for none
brightness = 0.9        # 1 is as it is
contrast = 1.1
saturation = 1.2
flip = "horizontal"     # "vertical", "both" or "none"
overlay = { color = "#00000060", blend = "multiply" }
position = "top"        # or [30, 70], in percent across and down
speed = 1.5             # videos, from 0.25 to 4
sound = true            # the sound of videos, at volume
volume = 0.3
pause_coverage = 0.9    # videos stand still where windows cover this much; 0 never

[[wallpaper.collections]]
name = "Sea"
images = ["C:/Users/me/Pictures/Sea"]
video = "C:/Users/me/Videos/waves.mp4"   # and videos, as in the table
interval = 300          # in place of interval and shuffle of the table
shuffle = true

[[wallpaper.display]]
number = 2
collection = "Sea"
blur = 0
```

| Key | What it does |
| --- | --- |
| `blur` | A blur of so many pixels, as `blur()` of CSS: the standard deviation, from 0 to 200. |
| `brightness`, `contrast`, `saturation` | Factors from 0 to 4, 1 for as it is, as the filters of CSS take them; brightness and contrast come first. |
| `flip` | `horizontal`, `vertical` or `both` mirrors the picture, or the video. |
| `overlay` | A color, `#rrggbbaa` or `#rrggbb`, laid over the picture as much as its alpha says, mixed as `blend` says: `normal`, `multiply`, `screen`, `overlay`, `darken`, `lighten` or `soft-light`. Over a video only `normal` and `darken` are done; the others are laid on as `normal`. |
| `position` | Where a picture that is cut to cover its display, is contained in it or is centered lies: `center`, `top`, `bottom`, `left`, `right`, `top-left`, `top-right`, `bottom-left`, `bottom-right`, or `[x, y]` in percent. A video keeps to it too. |
| `speed`, `sound`, `volume` | How fast videos play (0.25 to 4), whether their sound is heard and how loud (0 to 1). |
| `pause_coverage` | Only in the table: the share of a display, 0 to 1, that the windows shown cover together at which a video there stands still; 0.9 without it, 0 never. |
| `collection` | The name of one of `[[wallpaper.collections]]`, shown in place of `images`, `video` and `videos` of the same place. A name no collection has is reported, and the pictures of the place are shown. |
| `[[wallpaper.collections]]` | `name`, `images`, `video`, `videos`, and `interval` and `shuffle` in place of those of the table. The tables are called `collections` because TOML cannot hold a key `collection` and tables of that name side by side. |

Blur, brightness, contrast and saturation are not done to videos. Where the wallpaper of Windows is to be adjusted, Tessera draws it itself. `mirror` gives Windows the picture file without the adjustments.

`[[wallpaper.workspace]]` is chosen by the `number` of the place of a workspace in the row of workspaces (`Workspaces::now(..).current`), not by the workspace itself: when a workspace is taken away, the one that moves into its place shows the wallpaper of that number. This is deliberate: a wallpaper belongs to a place in the row, as Seelen UI chooses a wallpaper by the index of a workspace.

In shell mode Tessera answers programs that ask COM for `IDesktopWallpaper`, as Explorer would: a picture set for one display goes to its `[[wallpaper.display]]` entry, a picture for all, a slideshow, the position, the color and its options to `[wallpaper]`. A picture or slideshow for all takes the place of the pictures, videos and collection shown before, those of `[[wallpaper.display]]` and `[[wallpaper.workspace]]` entries and `images_light` and `images_dark` included. A color set alone writes `source` as it was, where the file left it out, so that the wallpaper does not become the color. Whether Settings > Personalization uses it has not been tried.

## The icons of the desktop

`[desktop]` sets how Tessera draws the icons of the desktop; the [features](FEATURES.md) tell what they do and what was tried.

```toml
[desktop]
icons = true      # Tessera draws the icons; without the key only where it is the shell
size = "medium"   # "small", "medium" or "large"; Ctrl+wheel and View > ... change it
arrange = "grid"  # "grid", "free" or "auto"
sort = "name"     # "name", "size", "type" or "date"
show = true       # View > Show desktop icons
layer = "child"   # or "window"
```

| Key | What it does |
| --- | --- |
| `icons` | Whether Tessera draws the icons. Without the key it does where it is the shell of the session, and beside Explorer it leaves them to Explorer. Where Tessera is the shell its windows of the icons cover the desktop of every display whatever `icons` says, so that a click and the menu of the desktop are always Tessera's; with `icons = false` they draw nothing. Beside Explorer, `icons = true` hides Explorer's icons while Tessera draws its own, and shows them again when Tessera stops drawing them, quits or ends in any other way. |
| `size` | 32, 48 or 96 logical pixels; Ctrl+wheel, Ctrl+plus and Ctrl+minus step it, Ctrl+Shift+2, 3 and 4 choose large, medium and small as on Explorer's desktop. |
| `arrange` | `grid` keeps the icons where they were put, in the cells of a grid; `free` keeps them anywhere; `auto` arranges them column after column from the top left of the primary display, in the order of `sort`, and they cannot be put elsewhere. |
| `sort` | The order of `auto`, and of the icons Sort by arranges once in the other two: the items of the Shell first, then folders, then files, by name, by size (the largest first), by type (the extension) or by date (the newest first). |
| `show` | Whether the icons are shown at all; the menu of the desktop still opens without them. |
| `layer` | How the window of the icons of a display is part of the desktop: `child` makes it a child window of Tessera's own desktop where Tessera is the shell, and beside Explorer of a window Tessera keeps below the others; `window` keeps it a window of its own below the others. `window` is there for a Windows where a child window does not show what GPUI draws. |

The menu of the desktop writes `size`, `arrange`, `sort` and `show` here. Where the icons stand is kept in `desktop-icons.toml` beside `config.toml`, as a column and a row (and a display, counted from the left) for each icon, for each arrangement of displays: a display that goes takes its icons to the primary one, and they are back where they were when it comes back.

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

## Workspaces

`[workspaces]` with `own` keeps workspaces of Tessera's own instead of the virtual desktops of Windows; as the shell, four are kept without it. The widget `workspaces` shows a button for each, the action `workspace` goes between them (see [actions](#actions)), and the overview, the window switcher and the task view list their windows.

```toml
[workspaces]
own = 4               # how many there are at first, 1 to 16
min = 1               # the fewest a row keeps when workspaces are removed
max = 16              # the most a row has when workspaces are made
rows = 2              # laid out in two rows, for Win+Ctrl+Up and Down and for the widget
wrap = true           # a step past the end comes round to the other end
add_button = true     # the widget ends with a button that makes a workspace
names = ["web", "code", "", "chat"]   # what the buttons say; an empty one keeps the number
icons = ["\uE774", "\uE943"]       # a character of the icon font for each, beside the name or instead of the number
per_display = false   # true: every display has its own row
keys = "alt"          # Alt+digit goes to a workspace, Alt+Shift+digit sends the window in front there
only = []             # the only programs whose windows belong to a workspace
except = ["taskmgr.exe"]   # programs whose windows are seen on every workspace
```

Workspaces are made with `workspace add` (Win+Ctrl+D where the keys of Windows are bound) or the `+` of the widget, and removed with `workspace remove` (Win+Ctrl+F4) or **Remove** in the menu of a button; the windows of a removed workspace go to the one before it, or after it for the first. A click with the right button on a button opens its menu: **Rename** writes `names`, **Icon** writes `icons`, both in this file, and **Send window here** sends the window that was last in front there. `names`, `icons` and `[[wallpaper.workspace]]` follow the place of a workspace: removing the second makes the third the second; where all displays share one row, the name and icon of a removed workspace are taken out of the lists. Changing `own`, `per_display`, `only`, `except` or `keys` shows every window and starts over on the first workspace; the other keys take effect at once.

Tessera writes `workspaces-state.toml` next to this file: the workspaces of each row by a number that stays while each exists, the one each row shows, and which window lives where, and which windows are sticky (the action `sticky`). At its next start the windows that are still there go back to their workspaces; quitting shows every window first. A state written for another `own` or `per_display` is not taken up. The file is Tessera's own and is not meant to be edited.

Without `own`, beside Explorer, the widget shows the virtual desktops of Windows, with the names given to them in the task view of Windows where `names` gives none. Tessera cannot rename them, show a window on all of them or move a window of another program between them, as Windows offers programs no documented way; making and removing one presses Explorer's shortcuts and works only while Explorer runs.
## The dock as a taskbar

The `windows` widget is the dock. Besides `pinned`, `start`, `start-icon`, `badges` and `all_displays`, its options make it a taskbar:

```toml
[[surface]]
edge = "bottom"
thickness = 56
center = [{ widget = "windows", start = "launcher", group = "never", labels = true, middle = "close", wheel = true }]
```

| Option | Values | What it does |
|---|---|---|
| `group` | `always` (default), `never` | `never` gives each window a button of its own; a pinned application keeps one window on its button and the others get their own. |
| `labels` | `false` (default), `true` | Each button shows the title of its window beside the icon, four buttons wide. In a column the titles show only where the dock is at least two and a half buttons wide. |
| `counter` | `false` (default), `true` | The number of windows beside the dot instead of up to two more dots. |
| `show` | `display` (default), `all`, `workspace`, `minimized` | Which windows get buttons: those of the dock's display, those of every display (as `all_displays = true`), those of the workspace shown, or only the minimized ones of the dock's display. Windows of other workspaces are hidden or cloaked and never listed; `workspace` reads Tessera's own workspaces and lists every display's windows where Windows' virtual desktops are in use. |
| `sort` | `pinned` (default), `opened`, `name`, `manual` | The order of the buttons: the pinned applications first and then the others as they opened, every application as it opened its first window (pinned ones that do not run after them), by the name of the application, or the pins first and then the others in the order they were dragged into, those never dragged after them as they opened. |
| `middle` | `new` (default), `close` | What a click with the middle button does: start another window, or close the windows of the button. |
| `wheel` | `false` (default), `true` | The wheel over a button goes through its windows, bringing each forward. |
| `reveal_on_attention` | `while_flashing` (default), `until_activated`, `never` | When a dock that hides is shown because a window of its display asks for attention: while the window flashes, until it is activated, or never. The other surfaces that hide on that display are shown with it. |
| `pinned_on` | `every` (default), `primary` | Which docks show the pins: those of every display, or only that of the primary display, the others showing only the applications that run. |
| `audio` | `true` (default), `false` | A speaker at the upper left of the button of an application that plays sound, read once a second and four times a second while something plays; a click on it mutes the application or gives it its sound back (then the speaker is crossed out), the wheel over it turns its volume, and its tip says which it is. A browser that plays from a process of its own is found through the process that started it. |

An entry of `pinned` is the path of a program or a shortcut, the name the Start menu gives an application, `shell:AppsFolder/` and the identifier of a packaged application, and also the path of a file (opened with its program) or a folder (opened in Tessera's file manager where it takes folders), a link of any scheme, such as `https://example.org` or `mailto:me@example.org` (its button shows the icon of the program Windows opens such links with; no icon of the site is fetched), or the word `separator`, a thin line between buttons:

```toml
center = [{ widget = "windows", pinned = ["Paint", "separator", "C:/Users/me/Documents", "https://example.org"] }]
```

A right click on the empty part of the dock offers **Pin a file…** and **Pin a folder…** (the dialog of Windows), **Pin a web address…** (a field; a bare name such as `example.org` gets `https://`), **Add separator** and **Import the pins of the Windows taskbar**; the menu of a pin has **Unpin**, that of a separator **Remove separator**.

Every button, pinned or not, and every separator is dragged with the left button into another place, as long as `sort` is `pinned` or `manual`: the others slide aside, in a column too. A pin goes to another place among the pins, which rewrites `pinned`; a button of an application that is not pinned, dropped among the pins, is pinned there; with `manual` it may also go among the others, and that order, by application, is the same for every dock and is kept in `dock-order.toml` beside `config.toml`, which is Tessera's own. A pin dragged off the dock and let go outside its window is unpinned and shrinks away; GPUI tells no drop outside its window, so the pointer is read 30 times a second while a pin is dragged. A dock on a display other than the primary one with `pinned_on = "primary"` shows no pins and pins nothing by a drag.

Files, folders, shortcuts and `.url` files dragged from Explorer or another program onto the empty part of the dock, or onto a button that does not open them, are pinned where the gap opens; a link dragged from a browser is pinned as its address. Dropped on a pinned program or shortcut, they are opened with it, as the taskbar of Windows does (a packaged application takes nothing dropped on it). A pin that would open them lights up while they are over it, and when they rest 0.6 seconds on the button of an application with a window, that window comes forward, so that they can be dropped into it. One dock of a surface takes the drops.

**Import the pins of the Windows taskbar** (the action `import-taskbar-pins`) appends the shortcuts in `%APPDATA%\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar` that are not pinned yet, in the order the taskbar keeps in the `Favorites` value of `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Taskband`, and the rest by name. That value is not documented and is read by looking for the names of the shortcuts in it, so the order may differ from that of the taskbar.

`tessera pin <path|link>` and `tessera unpin <path|link>` hand the request to the Tessera that runs, as the actions `pin` and `unpin` (a relative path is taken from the folder the command runs in). The action `pin-verb` with `on` writes **Pin to Tessera dock** for every file, for folders and for shortcuts under `HKCU\Software\Classes\*\shell\TesseraPin`, `Directory\shell\TesseraPin` and `lnkfile\shell\TesseraPin`, each running `"<tessera.exe>" pin "%1"`, so that the menus of Explorer and of other programs offer it; `off` takes it away. The Dock part of the settings has both, and the import, as links. Nothing is written until one is clicked.

The button of the application in front asks the icon packs for their active icon, which a pack that follows the theme draws in its primary color.

A window whose `[[rule]]` says `skip_dock = true` gets no button. A window that flashes to ask for attention marks its button with a pulsing tint and a dot until it is activated. A right click on the Start button opens the menu of Win+X (the action `winx`).

As the shell of the session, the dock also hears what applications tell the taskbar through `ITaskbarList3`: their progress (along the bottom of the button, in the primary color, a lighter hue while paused and red after a failure, a travelling segment where the end is not known), their overlay icon (at the lower right, in the place of the badge), the tooltip of their picture (the title of their preview) and the buttons they put under their picture (under it in the preview). Windows that minimize fly to their button. Beside Explorer all of that stays with Explorer's taskbar.

A dock on a bar at the left or right edge lays its buttons out in a column, with the dot, the progress and the mark of attention on the side away from that edge, and opens its previews and menus beside itself, centered on the button. Outside a bar, as in a popup, a dock taller than wide is a column with its marks on the side where the display has more room.

The keys of the dock are bound where `windows_keys` is on, as the taskbar binds them:

| Action | Keys | What it does |
|---|---|---|
| `dock` (arg `1` to `9`) | Win+1 to Win+9 | What a click on that button does. |
| `dock-new` (arg `1` to `9`) | Win+Shift+1 to Win+Shift+9 | Starts another window of that button's application. |
| `dock-cycle` (arg `1` to `9`) | Win+Ctrl+1 to Win+Ctrl+9 | Brings the next window of that button forward. |
| `dock-menu` (arg `1` to `9`) | Win+Alt+1 to Win+Alt+9 | Opens the menu of that button. |
| `dock-focus` | Win+T | Puts the keyboard on the dock: the arrows go from button to button, Enter does what a click does, a digit presses that button, the Menu key or Shift+F10 opens the menu, Esc gives the keyboard back. |

The numbers count the buttons of the dock on the display the pointer is on, or else of the first dock.
## The notification area

`[tray]` says which icons of the notification area stand in the bar and which wait in the overflow, the grid that the button at the end of a `tray` strip opens, and in what order they come:

```toml
[tray]
shown = ["app.exe"]
hidden = ["C:/Program Files/Other/other.exe#1", "{7820AE78-23E3-4229-82C1-E41CB67D5B9C}"]
order = ["C:/Program Files/App/app.exe#1"]
new = "shown"
```

An icon is named by the global identifier its application gave it, written as `{…}`, or else by its program and the number the program gave it, as `C:/Program Files/App/app.exe#1` (slashes either way, case ignored); an icon of a program Windows does not name, as one running with higher privileges, is named by the class of its window, as `class:Name#1`. An entry may name every icon of a program by its path or by its file name alone (`app.exe`), or one of them by file name and number (`app.exe#2`). Where entries of `shown` and `hidden` both name an icon, the one that names it more closely decides, and of two that name it as closely, `shown`. An icon no entry names stands where the user put it in the settings of Windows (Taskbar > Other system tray icons), and an icon Windows has no choice for either stands where `new` says: `"shown"` (the default) or `"hidden"`. `order` orders the icons of the bar and of the overflow alike; icons it does not name follow in the order they came.

Tessera writes these lists itself: dragging an icon writes `order`, and where it goes from the bar to the overflow or back, its exact name into `shown` or `hidden`; so does Tessera's own menu of an icon, opened with Shift and the right button or with the middle button, with **Move to overflow** and **Show in bar**. Entries of `order` for icons that are not there now keep their place after the icon they followed. The `limit` option of a `tray` widget caps how many icons stand in its bar; those over it wait in the overflow before the hidden ones. The `bluetooth` option of the widget still leaves the Bluetooth icon of Windows out unless it is `true`.

The action `tray-focus` puts the keyboard on the strip of the display the pointer is on; where the keys of Windows are in use (`windows_keys`), Win+B is bound to it unless a `[[key]]` uses those keys. The arrows, Tab, Home and End move through the icons and the button of the overflow, Enter or Space chooses, Shift+F10 or the menu key asks for the menu of an icon, and Esc gives the keyboard back.
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

## Typing and the wheel

The widget `keyboard` shows, after the letters of the layout of the window in front, the mode of its input method where that layout has one, as the input indicator of Explorer: `A` while it types letters as they are, `あ` (hiragana), `カ` (katakana) or `Ａ` (letters of full width) for Japanese, `가` for Korean, `中` or `英` for Chinese. It is read through the window in front (`WM_IME_CONTROL`) every 0.4 seconds while the widget is shown, on a thread that may wait; a click on the mode switches a Japanese input method on or off and a Korean or Chinese one that is on between its own letters and Latin ones (가 and A, 中 and 英), and where the input method does not answer, as that of a window of an administrator, no mode is shown; a click on the letters goes on to the next layout as before. With `layout = true` in `[flyout]`, the layout flyout gives the mode too. `touch-keyboard` is a button that shows and hides the touch keyboard of Windows, as the button of the taskbar of Explorer does; it starts the touch keyboard (TabTip.exe) where it does not run yet.

```toml
end = ["touch-keyboard", "keyboard", { widget = "volume", step = 5 }, { widget = "brightness", step = 10 }]
```

The wheel over `volume` turns the default output up or down by `step` percent (2 without the option, from 1 to 25) and over `brightness` makes its display brighter or darker by `step` percent (5 without it), each with its flyout where `[flyout]` wants it. Over `media` the wheel turns the volume as over `volume`, and with Shift held goes to the next piece (down) or back to the one before (up). A mouse wheel turns one step a notch, counted as the lines Windows scrolls for a notch (3 when that is set to 0), so the small turns of a precision touchpad add up to a step; a touchpad that scrolls by pixels makes one step for every 40 pixels.

## Displays, power, performance and meters

The popup `display-settings`, which the action of the same name opens, sets the displays: their arrangement (dragged in a drawing of the desktop, snapping to each other and kept against one another), which are on, which is the main one, and the resolution, refresh rate, scale and orientation of each. **Apply** tries the new configuration for 15 seconds and the old one comes back unless **Keep changes** is pressed in the popup or on the banner that asks. The popup `power-settings`, which the action of the same name opens, blocks sleep by hand and sets the performance mode, when the screen goes off and the computer sleeps, and what the lid and the power and sleep buttons do, on mains power and on battery, in the power plan in use. Both stand in the middle of the display; a `[[popup]]` of the same name in `config.toml` is used instead, and the widgets `display-settings` and `power-settings` can stand in another popup.

```toml
[[key]]
press = "win+shift+p"
action = "display-settings"

[[key]]
press = "win+shift+c"
action = "inhibit"         # "on", "off", or none to switch
```

`[performance]` chooses how much the shell spends on looks: `off` (everything moves as the theme says), `minimal` (what slides arrives at once and fades take half the time) or `extreme` (nothing moves). `default` is the mode on mains power, `on_battery` on battery and `on_energy_saver` while energy saver, or battery saver on an older Windows, is on, whatever the source; a line left out falls back to the one before. The tile `performance` and the action `performance` put another mode in force by hand until the power source or a saver changes; `performance auto` gives the choice back to these lines.

```toml
[performance]
default = "off"
on_battery = "minimal"
on_energy_saver = "extreme"
```

`[system] interval` is how many seconds pass between two readings of the meters (`cpu`, `memory`, `network-meter`, `gpu`), from 1 to 60, two by default; `battery` and `disk` are read no more than every 20 and 30 seconds. A meter's own option `interval` wins over both. Meters of the same kind and interval share one reading.

```toml
[system]
interval = 3

[[surface]]
edge = "top"
start = ["cpu", { widget = "gpu", show = "memory", interval = 5 }, "memory"]
```

The tiles `inhibit` and `performance` are for the `tiles` of the quick settings, as `{ widget = "tiles", tiles = ["inhibit", "performance", "power-mode"] }`.
