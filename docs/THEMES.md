# Themes

A theme is a TOML file in the `themes` directory of the configuration directory (`%APPDATA%\tessera\themes\<name>.toml`), installed there by the marketplace or copied in by hand. It sets tokens of the appearance every surface is drawn with; what it leaves out stays as built in. `theme = "<name>"` at the top of `config.toml`, or the choice under **Themes** in the settings, makes it the base theme. The names `accent` and `wallpaper` are no files: they derive every color from the accent color of Windows, or from the wallpaper, dark or light as Windows says for the taskbar.

## The file

```toml
seed = "#3fd0e8"     # a color every color role is derived from; "accent" or "wallpaper" follow Windows
scheme = "dark"      # "dark", "light" or "system"; dark where no theme says

[colors]             # single roles as #rrggbb, applied over the derived ones
primary = "#3fd0e8"

[shape]              # corner radii in pixels: small, medium, large, full
small = 12

[spacing]            # tiny, small, medium
[typography]         # family (an installed font), label_size, icon_size
[motion.spatial]     # stiffness 20 to 5000, damping_ratio 0.2 to 2; also spatial_fast and effects
[bar]                # pill_height, flat
[dock]               # container_height, item_size, icon_size, indicator_height, indicator_active_width
[calendar]           # cell_size
[slider]             # track_height, handle_width, handle_height, handle_gap, inner_radius, value_width
[meter]              # width, stroke, amplitude, wavelength, gap
[launcher]           # field_height, row_height, icon_size
```

[CONFIGURATION.md](CONFIGURATION.md) tells what each part does. A value outside what is usable is reported and left as it was; a key the format does not know makes the whole file unusable, which is reported too.

## Layers

`theme_layers = ["dense", "round"]` at the top of `config.toml`, or **Layers** under **Themes** in the settings, stacks themes over the base theme, in order. A layer is an ordinary theme file; one meant as a layer sets only what it changes, as the bundled `dense` (less space, smaller bars and dock) and `round` (rounder corners) do, and leaves the colors alone. The settings add a layer, move it up or down the stack and take it off; a theme already in use is not offered again.

The theme in use is made exactly so:

1. Start from the built-in theme.
2. Apply the base theme, and then each layer in turn, each over the result so far, in three steps:
   1. its `seed` and `scheme`: where it sets either, every color role is derived again, from the seed and the scheme last set by it or by a theme below it, so a layer that sets a seed replaces the `[colors]` of the themes below, and a layer that sets only `scheme = "light"` derives the colors of the seed below it in light; where no seed was set, a dark scheme gives the built-in colors again;
   2. its own tokens: `[colors]`, then the lengths, font and springs;
   3. its own settings, with the values `[theme_options.<its name>]` gives them (see below), in the order they are declared.
3. Correct the contrast once, at the end, unless `contrast = false`.

One function does all of it, for the base theme, the layers, the overlays of choices and the values of settings alike. A file of any of them that changes is applied again within about two seconds: every layer's file is watched, not only the base theme's. A layer that cannot be read is reported and left out.

## Settings of a theme

A theme declares what of it can be set with `[[setting]]` entries of the schema in [DECLARED-SETTINGS.md](DECLARED-SETTINGS.md). The settings show a card for each theme in use, the base theme and every layer, with a control for each; the values are kept in `config.toml` under `[theme_options.<theme name>]` and applied in step 2.3 above:

```toml
[[setting]]
key = "variant"
kind = "choice"
label = "Accent variant"
choices = [
    { value = "unset", label = "Original" },
    { value = "teal", label = "Teal", overlay = { seed = "#2fb3a8" } },
]

[[setting]]
key = "corners"
kind = "slider"
label = "Corner roundness"
min = 0
max = 2
step = 0.1
default = 1.0
scale = ["shape.small", "shape.medium", "shape.large"]

[[setting]]
key = "font"
kind = "font"
label = "Font"
optional = true
affects = ["typography.family"]
```

```toml
# config.toml
theme = "ocean"
[theme_options.ocean]
variant = "teal"
corners = 1.4
```

- `affects` puts the value at the tokens it names, as if the theme file said so.
- `scale` multiplies the lengths it names by the value, as they are after everything before it; `shape.full` is never scaled.
- An `overlay` of a choice is a table of tokens written as in a theme file, applied when that choice is made. An accent variant is an overlay with a `seed`, which derives every color role, not only `primary`, and so replaces the theme's own `[colors]`.

A default is applied like a value. A setting whose default would change the theme as written, such as a color, is best declared `optional` without a default, so that the theme looks as written until it is set.

## The settings of the built-in theme

Where no theme file is named, the built-in theme declares these settings, kept under `[theme_options.builtin]`; the themes `accent` and `wallpaper` declare the same without the accent color, which Windows gives them, under `[theme_options.accent]` and `[theme_options.wallpaper]`:

| Key | Kind | What it does |
| --- | --- | --- |
| `accent` | color, optional | Every color is derived from it (`affects = ["seed"]`). |
| `scheme` | choice: Default, Dark, Light, As Windows | Derives the colors dark, light or as Windows says for the taskbar. |
| `corners` | slider 0 to 2, default 1 | Multiplies the small, medium and large corner radii. |
| `density` | choice: Default, Compact, Comfortable | Spacing of 3/6/12 or 5/10/20 pixels. |
| `font` | font, optional | The family of the text. |
| `bar_height` | number 24 to 64 px, optional | The height of the fields of a bar. |
| `dock_size` | slider 0.6 to 1.6, default 1 | Multiplies the height of the dock, its buttons and their icons. |

## The bundled themes

[`themes/`](../themes) holds the themes this repository offers. `ocean`, `forest`, `sakura`, `ember`, `glacier`, `lilac`, `paper`, `nebula`, `expressive` and `compact` declare accent variants, a corner slider and a font, most of them density and dock size as well; `void` declares whether its bars are flat, corners and a font; `dense` and `round` are layers with a setting each. The others declare nothing yet.

Written against the Windows API and checked to build for Windows; not run on Windows yet. How the layers, the overlays and the scales are applied is checked by unit tests only.
