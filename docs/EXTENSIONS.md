# Extensions

An extension is a WebAssembly file that Tessera runs in a [Wasmtime](https://wasmtime.dev) sandbox. It describes the content of its widgets as a tree of rows, columns, text, captions, buttons and progress bars, which Tessera draws with the active theme, and it is told when one of its buttons is pressed. A column inside a column is drawn as a card, and what a column holds keeps its height where it does not all fit, the widget scrolling instead; before, progress bars there were squeezed flat. Neither was tried on a running shell. [`wit/extension.wit`](wit/extension.wit) is the whole interface. It is unstable and will change.

Besides drawing, an extension can:

- read the clock and write to the shell's log;
- read its settings, the keys of the `[extension.<id>]` table in `config.toml`;
- keep up to 64 KiB of text across restarts, stored as `state.txt` in its own directory;
- read the files its manifest lists under `permissions.files`, and no others;
- send HTTPS requests to the hosts its manifest lists under `permissions.network`, and no others. Requests run in the background and the answer is delivered later; redirects are not followed, a response is cut off at 1 MiB, and servers are verified against the Windows certificate store;
- send plain HTTP requests to the servers its manifest lists under `permissions.local`, each a name or address with its port, as `nas.local:8080`. A request is sent only when the name stands for addresses of this machine or of a private or link-local network; nothing protects what is sent from others on that network, and the name is looked up a second time for the request itself, so a name server that answers differently then is not guarded against. The marketplace lists such a server as `http://nas.local:8080` among the hosts an extension asks to contact;
- learn which Bluetooth devices are paired with the machine, whether each is connected and the charge Windows knows of it, when its manifest lists `bluetooth` under `permissions.devices`. Tessera reads the devices on a thread of its own every twenty seconds while an extension asks, so an answer can be that old, and the first question is answered with an error while the first reading runs. An extension cannot connect, disconnect or talk to a device.

The limits of that:

- Permissions are granted by the manifest alone. The `marketplace` widget shows what an extension asks for before it installs it; an extension copied in by hand is loaded without a question, and nothing shows what an installed extension is permitted, so read `extension.toml`: an extension with a file and a host permitted can send that file to that host.
- Settings, API keys included, are plain text in `config.toml`.
- It still cannot reach devices, Bluetooth, other processes or anything else of Windows, so an extension that shows a mouse's battery level cannot be written yet.
- It adds widgets and popups made of widgets. Its manifest can list `[[popup]]` entries, written as in a layout, and name one of them as the `popup` of a widget: a click on that widget then opens it without anything in `config.toml`. A layout knows such a popup as `<id>.<popup name>`; a `[[popup]]` of that name in the layout replaces it, a `popup` given to the widget in the layout opens that one instead, and `popup = ""` opens none. A popup of an extension can hold widgets of the shell and of other extensions too. An extension cannot open or close a popup from its code, cannot add a surface or a screen of another kind, and has no icons, images, text input or scrolling.
- Each call into an extension is limited in computation, and an extension is limited to 64 MiB of memory. A widget whose extension exceeds a limit or crashes shows "stopped working" until Tessera restarts.
- Extensions are loaded at startup, and again when the `marketplace` widget installs or removes one, when the manifest or the WebAssembly file of one is put into the extensions directory, replaced or removed by hand (noticed within about two seconds), and when an `[extension.<id>]` table of the layout file changes; all of them start anew then. Tried on a running shell with the `ai-limits` example and no credentials: its widget appeared in the bar after its directory was copied in, and showed one service fewer after the setting that lists them was changed, both without a restart. Text in a row an extension describes stays on one line, so a full bar does not break it into lines, and a column that is the whole widget scrolls when it is taller than its place, or, in a window fewer than three pills high such as a bar, is laid out as a row, which was not tried on a running shell; neither was seen going wrong before the change on the machine at hand, so that the change cures what a user reported as a broken widget in the bar is not confirmed. A widget placed on several displays shares one extension instance.
- Nothing is signed and nobody reviews what a registry offers; see below.

## Registries

The `marketplace` widget lists what registries offer and installs, updates and removes it: extensions, themes, icon packs, wallpapers and tiling layouts. Its option `show` names the kinds it shows, one (`show = "themes"`) or a list (`show = ["icon-packs", "wallpapers"]`), each of `extensions`, `themes`, `icon-packs`, `wallpapers` and `tiling-layouts`; without the option it shows every kind, with a tab for each and the number of updates on the tab. `columns` says in how many columns the offers stand (1 to 4, 1 unless said). The settings show the extensions under **Extensions** and the themes under **Themes**; the other kinds are shown by a `marketplace` widget placed in a popup of one's own, as the settings do not list them yet.

The registries are read when Tessera starts, once a day after that, and when **Check for updates** is pressed; every marketplace shows that one reading, and one that opens more than an hour after it reads them again. Their number of updates (offers whose version differs from the installed one) is shown next to **Update all**, which downloads every update shown and installs it at once, except that an extension whose new version asks for other things than the installed one asks first, as an install does. Nothing is updated without a click.

`registries` in `config.toml` lists the registries to read, each an `https://` address or an absolute path of a file on this machine. Without the key it is the [registry of this repository](../registry/registry.toml); `registries = []` reads none. A registry is a TOML file:

```toml
name = "Mine"

[[extension]]
id = "lights"
name = "Lights"
description = "Shown under the name."
version = "1.2.0"
manifest = "lights/extension.toml"
manifest_sha256 = "…"
wasm = "lights/extension.wasm"
wasm_sha256 = "…"
preview = "lights/icon.png"     # optional: shown as its icon
preview_sha256 = "…"
place = { surface = "bar", slot = "end" }   # optional, see below

[[theme]]
id = "night"
name = "Night"
description = "Shown under the name."
version = "1.0.0"
file = "themes/night.toml"
sha256 = "…"
preview = "previews/night.jpg"  # optional: shown above it
preview_sha256 = "…"

[[icon_pack]]
id = "line"
name = "Line"
version = "1.0.0"
manifest = "icon-packs/line/icon-pack.toml"
manifest_sha256 = "…"
files = [
    { path = "apps/notepad.svg", sha256 = "…" },   # relative to the manifest
]
preview = "icon-packs/line.png"
preview_sha256 = "…"

[[wallpaper]]
id = "aurora"
name = "Aurora"
version = "1.0.0"
file = "wallpapers/aurora.jpg"    # JPEG, PNG, BMP or WebP
sha256 = "…"

[[tiling_layout]]
id = "wide"
name = "Wide"
version = "1.0.0"
file = "tiling-layouts/wide.toml"
sha256 = "…"

[[registry]]
url = "https://example.org/more/registry.toml"
```

`manifest`, `wasm`, `file` and `url` are `https://` addresses or paths relative to the registry file, which cannot lead out of its directory; a registry on the web cannot name files of this machine. A registry can name further registries, and those are read as well, down to three steps from a listed one and at most 16 registries in all; each is read once. When two registries offer something of the same kind and `id`, the one read first wins, and the listed ones are read before those they name. A Tessera from before icon packs, wallpapers and tiling layouts were offered reads such a registry and skips what it does not know.

What this does and does not guard:

- Every file is checked against the SHA-256 sum in the registry, and the `id` in a manifest has to be the offered one. That catches a damaged or swapped download; it says nothing about the author, because whoever controls a registry controls the sums too. There are no signatures and no review.
- Before installing or updating an extension, the widget says what the manifest asks for (the hosts to contact, the files to read, the devices to be told of) and waits for `Allow and install`. Nothing else of the extension is inspected. The other kinds say what they change and wait for `Install`.
- Installing an extension writes `extension.wasm`, `extension.toml` and `installed.toml` (the version and the registry it came from) to `%APPDATA%\tessera\extensions\<id>\`. It does not change the layout: the widget lists the names to put into `config.toml`, such as `counter.total`. Where the registry entry has `place = { surface = "bar", slot = "end" }` (`surface` is `bar`, the first surface along the top of a display, or `dock`, the first along the bottom, `bar` unless said; `slot` is `start`, `center` or `end`; `widget` may name which of the extension's widgets, the first unless said), the question before the install offers **Allow, install and add to the bar** (or **to the dock**) beside **Allow and install**, which installs the extension, loads the extensions again and then puts the widget at the end of that part of the first such surface. A place that names no widget of the extension, or no slot, is reported and not offered.
- An icon pack is written to `%APPDATA%\tessera\icon-packs\<id>\`; see [icon packs](ICON-PACKS.md) for what is checked and what an update and a removal delete. **Use** puts it first in `icon_packs`, **Stop using** takes it out; removing a pack in use takes it out as well.
- A wallpaper is written to `%APPDATA%\tessera\wallpapers\<id>.<extension>` with `<id>.installed`, and has to be a picture. **Apply** makes it the only picture of `[wallpaper] images`, sets `source = "files"` and takes out `video`, `videos`, `images_light` and `images_dark`, which would show in its place; the other keys of `[wallpaper]` stay.
- A tiling layout is written to `%APPDATA%\tessera\tiling-layouts\<id>.toml` with `<id>.installed`, and has to be a TOML file of at most 64 KiB; whether the tiling can use it is only known when it is chosen, by the name the marketplace shows under it.
- `Update` is offered when the version of the registry differs from the installed one.
- `Remove` deletes those files, and an extension's `state.txt`. What was put there by hand (an extension without `installed.toml`, a file without its `.installed` file) is neither replaced nor removed. What is installed and offered by no registry that was read is listed before the offers, with its name and the widgets it gives a layout, and with no buttons; this was not tried on a running shell.
- A registry or an offer that cannot be used is listed at the top of the widget with the reason.
- A `preview` (a PNG or JPEG of at most 2 MiB) is checked against its `preview_sha256` like the other files and drawn above a theme, an icon pack, a wallpaper or a layout, or as the icon of an extension. One that cannot be had leaves the initial of the name in its place. [`registry/preview.py`](../registry/preview.py) draws the pictures of the themes and extensions of this repository: a desktop mock-up in the colors and shapes of each theme over its wallpaper, and the icon of each extension.
- A theme is checked against its SHA-256 sum and has to parse as a theme file, at most 64 KiB of it. Installing writes `<id>.toml` and `<id>.installed` (the version and the registry) to `%APPDATA%\tessera\themes\`; it does not choose the theme, which the settings or `theme = "<id>"` do. Removing the theme in use leaves the layout naming a missing file, which is reported, and the built-in theme shows.

### Links that install

A web page can offer something of a registry for a one-click install with a `tessera://` link, as Seelen UI's `seelen-ui://` links do:

```text
tessera://install?registry=<https address of the registry>&kind=<kind>&id=<id>
tessera://open?settings=<part>&item=<id>
```

`kind` is `extension` (unless said), `theme`, `icon-pack`, `wallpaper` or `tiling-layout`, and the address is written with its `:` and `/` escaped (`%3A`, `%2F`) or as is. The links reach Tessera once **Register tessera:// links** at the foot of the marketplace was pressed: that writes `HKCU\Software\Classes\tessera`, whose command is `"<tessera.exe>" do install-resource "%1"`, and **Unregister tessera:// links** takes the key away again. Nothing is registered before. The same can be asked without a browser with `tessera do install-resource "tessera://install?…"`. A link is only taken by a Tessera that runs; with none running, nothing happens.

An install link never installs by itself. It opens the popup `install-resource` in the middle of the display, which reads that one registry, downloads and checks the offer as the marketplace does, and shows the address of the registry, what is offered and what it asks for, with **Install** (**Allow and install** for an extension) and **Cancel**. A registry that `registries` does not name is marked as such, with a warning to install only what one trusts. Only `https://` registries are taken from links. For an extension whose registry suggests a place for its widget the popup also offers **Allow, install and add to the bar** (or **to the dock**); after the install of an icon pack it offers **Use**, after that of a wallpaper **Apply**. Installing an extension loads the extensions again, which closes the popup. An open link opens the settings; until they can be opened at a part, they open as the gear opens them, and `item` is not used yet.

Tried on a running shell: listing, installing and removing the `counter` example, from a registry file on disk and from the registry of this repository on the web, with the widgets appearing in and leaving the bar. Installing the theme `void` from a registry file on disk was tried too, after which the settings offered it. `Update`, failed downloads, nested registries and removing a theme are covered by tests only. Icon packs, wallpapers, tiling layouts, the update checks, the places, the links and their registration were written against the Windows API and checked to build for Windows; they were not run on Windows yet, and what can be was tried in tests only.

[`registry/update.sh`](../registry/update.sh) rebuilds the examples, copies the themes of [`themes/`](../themes) and the tiling layouts that come with Tessera, lists the icon packs of `registry/icon-packs/` and some pictures of `registry/wallpapers/`, and rewrites the registry of this repository. [`registry/preview.py`](../registry/preview.py) draws the pictures it lists for themes and extensions; run it first.

## By hand

To install one without a registry, create `%APPDATA%\tessera\extensions\<any name>\` with two files: `extension.wasm` and an `extension.toml` manifest that gives the extension `id`, its widgets and its permissions. A layout then refers to a widget as `<id>.<widget name>`. Build an example with:

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown --manifest-path extensions/counter/Cargo.toml --target-dir target/extensions
```

and copy `target/extensions/wasm32-unknown-unknown/release/tessera_example_counter.wasm` as `extension.wasm` next to a copy of its `extension.toml`. Once the examples are built, `cargo test -- --include-ignored` also runs the tests that load them.

## Examples

[`extensions/counter`](../extensions/counter) counts presses of its buttons. `counter.clicks` is the count with a button for a bar; `counter.total` is the count alone, and a click on it opens the popup the extension brings, with buttons that add to the count and reset it.

[`extensions/weather`](../extensions/weather) shows the weather at a place, asked of [Open-Meteo](https://open-meteo.com), which needs no account. `weather.now` is the temperature and the sky in words for a bar; a click on it opens the popup the extension brings, with what it feels like, the wind, the humidity and today and the three days after. `latitude` and `longitude` in `[extension.weather]` name the place in degrees, `unit = "fahrenheit"` changes the scale and `interval_minutes` (20, at least 5) how often it is asked. The place is sent to Open-Meteo rounded to a hundredth of a degree and to nobody else; without it nothing is asked and the bar says so. Tried once on a running shell with the place of the example in the Open-Meteo documentation: the bar showed `6° Rain` and the popup the four days. Fahrenheit, a failing request and an answer hours old were not tried on a running shell; `Refresh` was pressed once, in a card. The words are English only and there are no pictures of the sky.

[`extensions/device-battery`](../extensions/device-battery) shows the charge of a Bluetooth device. `device-battery.charge` is the name of one device with its charge as a bar and a number, for a bar; a click on it opens the popup the extension brings, which lists every paired device. `device = "mouse"` in `[extension.device-battery]` picks the first device with that in its name; without it the first connected device that reports a charge is shown, or else one whose charge Windows remembers, marked `was`. Tried on a running shell with four paired devices, none connected, one with a remembered charge: the bar showed that device with `was 80%` and the popup listed the four. A connected device, a changing charge and the `device` setting were tried only in the test that feeds the extension made-up devices.

[`extensions/ai-limits`](../extensions/ai-limits) shows what is left of the usage limits of AI services: the plan, each limit as a share and a bar, and when it resets. Its `summary` widget is one line for a bar; its `details` widget is meant for a popup, which has a fixed size, so size it for the services you enable:

```toml
[[surface]]
edge = "top"
end = [{ widget = "ai-limits.summary", popup = "ai" }]

[[popup]]
name = "ai"
width = 380
height = 420
content = ["ai-limits.details"]

[extension.ai-limits]
providers = ["claude", "codex", "openrouter", "deepseek"]  # default: claude and codex
summary = ["claude"]            # services in the bar; default: all of `providers`
interval_minutes = 5            # how often each service is asked
openrouter_key = "..."
deepseek_key = "..."
```

The buttons of `details` choose whether the plan, the reset times and the bars are shown and whether shares count what is left or what is used; the choice is remembered.

How far each service has been tried:

| Service | Asked with | Tried against the real service |
| --- | --- | --- |
| Claude | the sign-in of Claude Code, `~/.claude/.credentials.json` | No. The usage endpoint is undocumented; answers are parsed in tests against a made-up sample. |
| Codex | the sign-in of the Codex CLI, `~/.codex/auth.json` | No. Same as Claude. |
| OpenRouter | `openrouter_key` | Only that a wrong key is refused and reported. Answers are parsed against a sample in the documented format. |
| DeepSeek | `deepseek_key` | No. Answers are parsed against a sample in the documented format. |

The Claude and Codex endpoints may change or stop answering at any time, and their answers may differ from what this extension expects; it then shows an error instead of numbers. It never refreshes a sign-in: once the token in the file has expired, it shows an error until Claude Code or Codex has renewed it. A token is sent only to the service it belongs to.

## The plugins of this repository

Besides the four examples above, the registry of this repository offers these. Each is a crate under [`extensions/`](../extensions) built on [`extensions/kit`](../extensions/kit), which generates the bindings once and gives a plugin a tree builder for its widgets, settings with defaults, state kept as `key=value` lines, timed requests with retries and a queue, time and number formatting and a small random generator. A plugin implements `kit::Extension` and exports itself with `kit::extension!`. Each manifest explains its settings in comments.

| Plugin | Widgets (bar, panel) | Popup | Settings under `[extension.<id>]` | Reaches |
| --- | --- | --- | --- | --- |
| `pomodoro` | `timer`, `controls` | `panel` | `work_minutes`, `break_minutes`, `long_break_minutes`, `rounds`, `auto_start`, `utc_offset` | nothing |
| `breaks` | `reminder`, `controls` | `panel` | `every_minutes`, `break_seconds`, `snooze_minutes` | nothing |
| `habits` | `today`, `checklist` | `list` | `habits`, `utc_offset` | nothing |
| `countdown` | `next`, `events` | `dates` | `events` | nothing |
| `world-clock` | `clock`, `zones` | `places` | `zones`, `utc_offset`, `hour12` | nothing |
| `moon` | `phase`, `details` | `sky` | `glyph` | nothing |
| `dice` | `roll`, `table` | `tray` | — | nothing |
| `crypto` | `price`, `list` | `prices` | `coins`, `currency`, `interval_minutes` | api.coingecko.com |
| `exchange` | `rate`, `rates` | `board` | `base`, `symbols`, `interval_minutes` | open.er-api.com |
| `hacker-news` | `top`, `stories` | `front-page` | `count`, `interval_minutes` | hacker-news.firebaseio.com |
| `github-stats` | `stars`, `repos` | `repositories` | `repos`, `token`, `interval_minutes` | api.github.com |
| `air-quality` | `index`, `details` | `air` | `latitude`, `longitude`, `interval_minutes` | air-quality-api.open-meteo.com |

For example:

```toml
[[surface]]
edge = "top"
end = ["pomodoro.timer", "world-clock.clock", "crypto.price"]

[extension.world-clock]
zones = "Tokyo=+9; London=+0; New York=-5"

[extension.crypto]
coins = "bitcoin,ethereum"
currency = "eur"
```

What their authors could and could not try: every widget was rendered in a stand-in host built on the same Wasmtime and component versions as the shell, with the shell's manifest rules, and the requests of `crypto`, `exchange`, `hacker-news` and `air-quality` were sent to the real services. `github-stats` was tried only against made-up answers. None has run inside Tessera on Windows yet. `world-clock` applies fixed offsets, without daylight saving time; `moon` uses the mean lunar month, so its times of new and full moon can be off by about half a day; GitHub counts pull requests among open issues.
