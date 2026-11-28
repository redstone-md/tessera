# Declared settings

An extension or a theme can say what of it can be set: each setting with the control that sets it, what it is called, what it may be and what it affects. The settings then show a card of controls for it, under **Extensions** or **Themes**, and check what `config.toml` holds against it; editing `config.toml` by hand keeps working and is checked the same way. Tessera declares the settings of its built-in theme with the same entries. They are written as `[[setting]]` entries of TOML, in `extension.toml` or in the theme file, and read by `crates/tessera/src/options.rs`.

A Tessera build from before declared settings refuses a manifest or a theme that holds `[[setting]]`, as both formats reject keys they do not know. The bundled extensions declare their settings from version 0.2.0 on, and the bundled themes that declare settings do so in the versions the registry offers next; installing those needs a Tessera build with declared settings.

## An entry

```toml
[[setting]]
key = "unit"                    # the key it is written under
kind = "choice"                 # see below
label = { en = "Unit", ru = "Единицы" }   # or one text: label = "Unit"
description = "How temperatures are shown."  # optional, said under it
section = "Place"               # optional: settings of one section follow each other
default = "celsius"             # optional: what it is while the file says nothing
optional = false                # whether it may have no value at all
unset_label = "Off"             # what the state without a value is called
visible_if = { key = "auto", equals = false }   # shown only while another setting has this value
affects = ["now", "forecast"]   # widgets of an extension, tokens of a theme
choices = [
    { value = "celsius", label = "Celsius" },
    { value = "fahrenheit", label = { en = "Fahrenheit", ru = "Фаренгейт" } },
]
```

`key`, `kind` and `label` are required; a key is made of letters, digits, `_`, `-` and `.`. A label, a description, a section, a placeholder and an unset label are one text, or a table of texts by language code (`en`, `ru`, or the code of a language file such as `de`), English being used where the language in use has none. A plain text that Tessera's own tables know is said in the language in use. A declaration that cannot be used (a choice without choices, a slider without bounds, bounds the wrong way round, a default that does not fit, a key declared twice) is reported in the problems list and left out; the others stay.

## Kinds

| Kind | Control | Value in the file |
| --- | --- | --- |
| `toggle` | A switch. With `optional = true` and an `unset_label`, three buttons: that label, On and Off. | `true` or `false` (also read: `"yes"`, `"on"`, `"1"` and their opposites) |
| `text` | A field; `placeholder` is what it says while empty, `max_length` the most letters. An empty field takes the key out. | A text of one line |
| `number` | A field between a minus and a plus button. `min`, `max`, `step` (1 unless said) and `unit` (shown after it, such as `min`, `%` or `px`). An optional number goes one step under `min` to no value. | A whole number where `step` and both bounds are whole, otherwise a fraction; clamped to the bounds and rounded to the step counted from `min` |
| `slider` | A track; `min` and `max` are required. It writes once it is let go, and once the wheel rests. | As for `number` |
| `choice` | Up to four choices as buttons side by side, more in a list that opens. A choice whose `value` is the word `unset` takes the key out; an optional choice without one gets a button labelled by `unset_label` (Default unless said). | One of the values: a text, a number or `true`/`false` |
| `color` | A swatch, a field for the hex value and a palette of eight colors to click. | `#rrggbb` or `#rrggbbaa`, written in small letters |
| `path` | A field and **Browse**, which opens the file dialog of Windows; `pick = "folder"` asks for a folder, `"file"` (the default) for a file. The settings give way to the dialog and open again where they were. | A path, with forward slashes when chosen with Browse |
| `font` | The family in use, which opens a list of the installed font families with a field to narrow it down. An optional font has a button that takes it out. | The name of a family |
| `list` | Rows of texts, with a handle to drag a row to another place, a button that takes it out, and **Add**. | For an extension the text `a, b, c`; for Tessera itself a TOML array. An item may not hold `,` or `;`. |
| `pairs` | Rows of a name and a value, the same way. | The text `Tokyo=+9; London=+0` |
| `time` | A field between buttons that step the hour (left) and the minutes, to the next or the last full five (right), around the clock. What is typed is written when Enter is pressed or the field is left, not while typing pauses, and only as a whole time; `7:30` is written `07:30`. | A time of day `HH:MM`, two digits each, from `00:00` to `23:59` |
| `sides` | **Every side** with one field, or **Each side** with a field for top, right, bottom and left. `min` and `max` bound each number. | A number for every side, or a table `{ top, right, bottom, left }` of numbers, a side left out being 0 |

A `list` with `choices` is a choice of several: a box for each choice, ticked while the list holds it, written in the order of the choices whatever order they were ticked in; an optional one leaves the file when no box is ticked. A `list` with `min` and `max` bounds how many items it has (`min = 2` and `max = 2` for exactly two): it shows at least `min` rows, offers no **Add** at `max`, keeps its last `min` rows, and is written only once it has enough items.
| `time` | A field for now; a control of its own is not done yet. | A time of day `HH:MM`, two digits each, from `00:00` to `23:59` |
| `sides` | A field for now, which takes one number; a control of its own is not done yet. `min` and `max` bound each number. | A number for every side, or a table `{ top, right, bottom, left }` of numbers, a side left out being 0 |

A `list` with `choices` takes only their values as items, and its `min` and `max` bound how many items it has (`min = 2` and `max = 2` for exactly two); its rows are not drawn as choices yet.

There is no kind for passwords, tokens or keys: those an extension reads stay in `config.toml`, written by hand, and its description says so.

What is typed in a field is written once typing pauses for 300 ms, when Enter is pressed or when the field is left; Escape gives back what the file holds. A stepper writes once per click. A value that does not fit its declaration is said under its row and not written. Before anything is written, the file as the change leaves it is read as the shell reads it; what the shell could not use (a key the configuration does not know, a value its types refuse) is not written either, and why is said under the row, so that one bad value never makes the whole configuration fail to load. Every setting the file sets has a button that takes it out again (Reset), and a card whose settings the file sets has **Reset all**.

## Lists and pairs for extensions

The settings of an extension reach it as text, through the `setting` import. A list is written as its items apart by `, `, which `kit::settings::parse_list` splits again; a list written in `config.toml` by hand as an array, `coins = ["bitcoin", "ethereum"]`, reaches the extension as that text as well (before, it arrived as `["bitcoin", "ethereum"]`, which the kit could not split). Pairs are written as `name=value` apart by `; `, which `kit::settings::parse_pairs` reads: split at the first `=`, a name and a value each required, no `;` in either and no `=` in a name. What the extension makes of a value, such as reading `+5:30` as an offset, is its own check.

## In an extension

What an extension reads through `setting` is now the value of `[extension.<id>]` checked against its declaration, or the declared default where the file has none or one that does not fit, which is reported. Keys it does not declare reach it as written, so an extension can read a token from `config.toml` without declaring it. A setting without a default and without a value in the file is not there, as before.

`affects` names widgets of the manifest; a name that is not one is reported. The settings show the widgets a setting affects as `Affects: weather.now, weather.forecast`, or all widgets of the extension where it names none. When only settings of extensions change, the open popups, the extensions and their widgets stay as they are, and the new settings are kept as what each extension reads. A running extension is told of them at once: `setting` answers with the new values from then on, an extension built for 0.4 has its export `settings-changed` called, and its widgets are drawn again; one built for 0.3 reads them the next time it asks. Before, any change of an `[extension.<id>]` table loaded every extension again and reset what each kept in memory.

```toml
id = "weather"

[[setting]]
key = "latitude"
kind = "number"
label = { en = "Latitude", ru = "Широта" }
min = -90
max = 90
step = 0.01
unit = "°"
optional = true

[[setting]]
key = "interval_minutes"
kind = "number"
label = "Minutes between updates"
min = 5
max = 1440
default = 20
unit = "min"
```

Every bundled extension that reads settings declares them, with the defaults and ranges of its code; see their `extension.toml` files. Latitude and longitude have no default, as 0, 0 would ask for the weather of the Gulf of Guinea; without them nothing is fetched. `dice` and `counter` read no settings.

## In a theme

A setting of a theme changes tokens of the theme, named by their path in a theme file: `seed`, `scheme`, `colors.primary`, `shape.small`, `spacing.medium`, `typography.family`, `bar.pill_height`, `bar.flat`, `dock.item_size` and so on. It does one of three things:

- `affects = ["colors.primary"]` puts its value there: a color, a font, a number or a switch, as the token takes it. A color with an alpha part loses it, since the colors of a theme are opaque.
- `scale = ["shape.small", "shape.medium", "shape.large"]`, for a number or a slider, multiplies those lengths by its value. `shape.full`, the radius of fully rounded corners, cannot be scaled.
- A choice whose choices carry an `overlay` table brings the tokens of the choice made, written as in a theme file: `{ seed = "#3d7bd9" }`, or `{ spacing = { tiny = 3, small = 6, medium = 12 } }`.

A setting of a theme that names no token, or a token that does not exist, makes the theme unusable for the registry, which refuses to install it, and is left out where the theme is already installed. Lists, pairs and paths are not kinds a token has. The settings show what a setting of a theme affects by group: Colors, Corners, Spacing, Font, Motion, Bar, Dock, Calendar, Sliders, Meters and Launcher. How the values are applied, and the settings of the built-in theme, are in [THEMES.md](THEMES.md).

## Rows of the built-in settings

Tessera's own settings are declared the same way, in the rows files of `crates/tessera/src/modules/settings/rows/`, one file per part of Tessera (`core.toml` for the settings, others for each package), listed in `ROW_FILES` of `rows.rs`. The settings draw every file with the controls above: one renderer for the settings of Tessera, of extensions, of themes and of a placed widget.

```toml
[[card]]
section = "Windows"          # the English title of a section, see below
title = "Tiling"             # cards with one title in one section are one card
table = "tiling"             # its rows write keys of [tiling]; none for keys at the top
# widget = "clock"           # instead: its rows set an option of every clock widget
search = ["Tiles"]           # more English words search finds the card by

[[card.row]]                 # an entry as above; `key` is a dotted path below `table`
key = "gap"
kind = "number"
label = "Gap"
min = 0
max = 48
step = 4
default = 0
optional = true
unset_label = "Default"

[[card.link]]                # a row that runs an action instead of writing a key
label = "Open the configuration file"
description = "config.toml, in the program that opens TOML files."
action = "open"
arg = "{config}"             # `{config}` stands for the path of config.toml

[[card]]
section = "Wallpaper"
title = "Wallpaper settings"
page = "wallpaper-settings"  # a widget drawn inside the section as a page

[[action]]                   # what the Shortcuts page calls a registered action
name = "example"
title = "Example"
arg = "A name"               # optional: what its argument is, in English
```

Plain labels, descriptions, unset labels, choice labels, link labels, card titles, search words and `[[action]]` titles are English texts of the RUSSIAN table in `i18n.rs` (and so of every language file); a text that is not to be translated, such as `14:05`, is written `{ en = "14:05" }`. A row writes its value with the comments of the file kept: a dotted key makes the tables on its way and takes them away once they are empty, and an option of a widget is set on every widget of that name, on the bars, in both columns of the popups and on the cards of the desktop. A row that is not optional and is set to its `default` leaves the file. A card whose widget the layout lacks says so instead of its rows.

**Sections** are, in the order of the rail: General, Bars, Desktop, Wallpaper, Widgets, Dock, Tray, Notifications, Windows, Window rules, Screen edges, Workspaces, Keys, Search, Files, Clipboard and screenshots, Displays and power, Session, Themes, Extensions and About. `Settings::open_at(section, item)` takes the title in any case, spaces included. A section that has no card to draw (no rows, no link whose action is registered, no page whose widget is registered) is left out of the rail. Bars, Desktop, Dock, Themes and Extensions keep their cards drawn by hand above those of the rows files; Keys keeps its key cards until a `shortcuts` widget is registered.

**Pages** are widgets drawn inside their section: built with the option `item` set to the item `open_at` was given, once each time the section is shown, and dropped when another is shown; only while that widget is registered. A page that is also a popup of its own is drawn at the height of that popup.

**Items** of `open_at`: a card by its English title, a row by its path (`tiling.gap`, or `clock.format` for an option of widgets), an extension by its identifier, a theme by its name. In Widgets, `surface/<n>/<start|center|end>/<i>`, `popup/<name>/<i>` (counting `content` and then `side`), `card/<i>` and `card/<i>/<j>` open the card of the options of that one placed widget, lit. For a page, the item is passed to it.

**Lists only the running shell knows** are filled in Rust by one table, `DYNAMIC` of `modules/settings/dynamic.rs`, by the table of a card (`""` for keys at the top, `*` for any table) and the key: the tiling layouts with the layout files of the user, the places of flyouts, the displays (`all`, `primary`, `others` and each display by its name), the installed icon packs and themes, and the interface languages. Their choices come after those the file lists, each once. What they need from the disk or Windows is read away from the thread that draws, when the settings open and again when the marketplace installs or removes something.

**The options of one placed widget** are declared by `crate::widget_options` (`widget_options/*.toml`); the Widgets section lists every widget placed on a bar, in a popup of the configuration file or on a card, and opens a card of its declared options, written for it alone. Options its entry holds that nothing declares are listed as written.

**Checks.** The tests of `modules/settings/builtin.rs` read every rows file: every row of a table or of the top writes a key the configuration reads (its default, or its first choice, its least, false, an empty text or `00:00`, parsed as the whole configuration), a row that is not optional has the default the configuration has without the key, every text is translated, every card names a known section, and no two rows write one key.

**Search** finds every card and row of every section, the cards drawn by hand by their titles, and the installed extensions and themes with what they declare, by what they say in the language shown and in English. A whole word beats the start of a word, which beats any other place, and a label beats a description.
