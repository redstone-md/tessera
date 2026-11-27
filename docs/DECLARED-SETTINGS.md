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

There is no kind for passwords, tokens or keys: those an extension reads stay in `config.toml`, written by hand, and its description says so.

What is typed in a field is written once typing pauses for 300 ms, when Enter is pressed or when the field is left; Escape gives back what the file holds. A stepper writes once per click. A value that does not fit its declaration is said above the cards and not written. Every setting the file sets has a button that takes it out again (Reset), and a card whose settings the file sets has **Reset all**.

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
