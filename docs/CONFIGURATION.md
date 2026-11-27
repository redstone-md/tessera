# Configuration

Everything a surface shows is a widget provided by a module. The compiled-in modules provide the widgets `clock`, `calendar`, `label`, `launcher`, `apps`, `power`, `notifications`, `settings`, `media`, `media-controls`, `network`, `connections`, `radio-switch`, `wifi`, `bluetooth`, `bluetooth-button`, `volume`, `volume-slider`, `microphone-slider`, `volume-mixer`, `audio-outputs`, `brightness`, `brightness-slider`, `cpu`, `memory`, `battery`, `disk`, `network`, `files`, `command`, `tray`, `problems`, `problem-list`, `tiling`, `quit`, `user`, `profile`, `places`, `workspaces`, `overview`, `focused`, `keyboard`, `appearance`, `desktop`, `plan`, `hdr`, `trash`, `update`, `marketplace` and `windows`; extensions add more.

The layout is read from `%APPDATA%\tessera\config.toml`. It lists the surfaces — screen edge, thickness, all displays or only the primary one, and whether the surface slides away while the pointer is elsewhere (`hide = true`, or `hide = "overlap"` to do so only while a window lies in its place; it then reserves no screen space and comes back when the pointer reaches that edge of the display) — and the widgets in the start, center and end of each. A widget is written as its name, or as a table that adds options the widget understands (the clock takes a `format`, the calendar a `week_start`, a label an `icon` and a `text`, the tray a `limit` and whether it shows `tips`) and a `popup` to open when the widget is clicked. Popups are declared in the same file as a name, a size and a list of widgets, so any widget, including one from an extension, can open a popup or be placed in one. One popup is open at a time, and popups cannot be resized. Without that file the [built-in layout](../crates/tessera/src/default_config.toml) is used; it documents the format. A file that fails to parse is ignored as a whole, and an unknown widget name is skipped; both are reported on standard error, which only a debug build started from a terminal shows.

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

[[key]]
press = "win+e"
files = "~"           # a path, "~" for the folder of the user, or "computer"
```

From a console, `tessera files`, `tessera files computer` and `tessera files D:\Projects` open a window in the running shell.

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
| `snap` | `left`, `right`, `up`, `down`, `left-half`, `right-half`, `top-left`, `top-right`, `bottom-left`, `bottom-right`, `maximize`, `restore` or `minimize` | snaps the window in front, as [snapping](#snapping-window-rules-and-hot-corners) describes; its keys (Win and the arrows) are taken from Windows and bound by default where the keys of Windows are |
| `snap-display` | `next`, `previous`, or none | sends the window in front to the next display from the left (round to the first), or the one before; its keys (Win+Shift+Right, Win+Shift+Left) are taken from Windows and bound by default |
| `snap-layouts` | none | opens the snap layouts for the window in front; Win+Z is taken from Windows and bound by default |
| `window-menu` | none | opens the window menu of the window in front; Alt+F3 is bound by default where the keys of Windows are |
| `files` | a folder, as `files` names one, or none | opens a window of the file manager there, as `files = "..."` does; without `arg`, at the folder of the user; its keys (Win+E) are taken from Windows |

The keys a `press` can end with are a digit, a letter, `f1` to `f24`, `space`, `tab`, `enter`, `left`, `up`, `right`, `down`, `home`, `end`, `pageup`, `pagedown`, `insert`, `delete`, `backspace`, `escape`, `pause`, `printscreen`, and the keys a US layout has for `period`, `comma`, `plus`, `minus` and `backquote` (on another layout, the keys in their place).

## Snapping, window rules and hot corners

`[snap]` snaps windows that are dragged to an edge of a display: the left or right edge gives that half, the part of an edge within `corner` pixels (96) of a corner gives the quarter there, and the top edge the whole display; the pointer is to be within `edge` pixels (8) of the edge. A translucent shape in the primary color shows where the window goes before it is let go, and moves from place to place. A snapped window that is dragged away takes its size from before it was snapped once it is let go, with the pointer at the same share of its width; Windows itself gives it back while it is dragged, which a program outside the window cannot. A window dragged by its border is being resized and does not snap. With Shift held, the `[[snap.zone]]` entries of the display under the pointer are the places instead. `gap` leaves that many pixels between snapped windows and around them (0).

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

While dragging snaps, Tessera switches the snapping of Windows off (`SPI_SETWINARRANGING`) for as long as it runs, so that a window is not placed twice; the setting "Snap windows" of Windows stays as the user wrote it, and Tessera switches it on again when it quits, and at its next start if it was terminated. Whether Windows still snaps dragged windows without Explorer has not been seen, which is why dragging snaps by default only when Tessera is the shell. With the snapping of Windows off, Windows may not let a maximized window be dragged by its title bar; Win+Down or a double click on the title bar restores it then. Windows that are tiled are left to the tiling, and Windows does not let Tessera move a window of a program that runs as administrator.

The action `snap` does the same with the keys: with `left` and `right` a window goes to that half, from the other half back to where it was, and from a half or a quarter at the edge on to the next display; `up` goes from a half to its top quarter and from there to the whole display, and `down` takes those steps back and minimizes a window that is where the user put it. `snap-display` sends the window in front to the next display, at the same share of the way across and down, snapped or maximized as it was. `snap-layouts` (Win+Z) opens six small pictures of ways to divide the display under the right end of the title bar of the window in front, and a click on a zone puts the window there; with `hover` they also open when the pointer rests on the maximize button of a window, which Tessera asks the window about (`WM_NCHITTEST`) a few times a second while the pointer is near the top right corner of a window, and close once the pointer has moved away. After a window took a half by dragging, or a zone of the layouts, the other windows are offered for the rest of the display, part after part (`assist`); Esc or a click elsewhere leaves the rest as it is.

Each `[[rule]]` names windows by `exe` (the file name of the program), `title` and `class` (the window class), each a pattern in which `*` stands for any text and `?` for one character, whatever the case of the letters, or a regular expression after `re:`, and says what is done to them when they are first seen: `float = true` leaves them out of the tiles and `tile = true` tiles them whatever `[tiling]` says of their program; `workspace` (from 1) sends them to a workspace of Tessera's own (the virtual desktops of Windows take no window of another program); `display` (from 1 from the left) to a display; `position` (`"center"` or `[x, y]`) and `size` (`[width, height]`, in shares of the work area) place them; `maximize`, `above` (kept above the other windows), `opacity` (0.1 to 1) and `border = false` (no thin border, Windows 11) do what they say. `apply = "always"` does it again whenever the window changes, as far as it differs, which keeps a window where its rule puts it; otherwise a window that moves itself within a second of being shown, as many programs do to go back to where they were, is put back once. Every rule that names a window applies in the order of the file, a later one having the last word. Windows that are open already when Tessera starts or the rules change only get the rules that apply always. A tiled window is not placed by a rule. `skip_dock` and `skip_switcher` are read but neither the dock nor the window switcher leaves windows out yet. A rule without a name, with an `opacity` out of range, or with a `size` beyond the work area is reported and left out.

```toml
[[rule]]
exe = "CalculatorApp.exe"
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
