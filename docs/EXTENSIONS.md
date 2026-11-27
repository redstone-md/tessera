# Extensions

An extension is a WebAssembly file that Tessera runs in a [Wasmtime](https://wasmtime.dev) sandbox. It describes the content of its widgets as a tree of rows, columns, text, captions, buttons and progress bars, and since 0.4 of pictures, icons, fields, sliders, toggles, scrolling columns and spaces, which Tessera draws with the active theme, and it is told when one of its buttons is pressed or one of its fields, sliders or toggles changes. A column inside a column is drawn as a card, and what a column holds keeps its height where it does not all fit, the widget scrolling instead; before, progress bars there were squeezed flat. Neither was tried on a running shell. [`wit/extension.wit`](../wit/extension.wit) is the whole interface. It is at version 0.4 and unstable until 1.0; since 0.3 a version only adds, so components built for 0.3 keep working, as [Interface 0.4](#interface-04) describes.

Besides drawing, an extension can:

- read the clock and write to the shell's log;
- read its settings, the keys of the `[extension.<id>]` table in `config.toml`;
- declare its settings with `[[setting]]` entries in its manifest, which the settings then show under **Extensions** and check, with defaults that answer the `setting` import where the file says nothing ([DECLARED-SETTINGS.md](DECLARED-SETTINGS.md)); an extension or theme of a registry that declares settings needs a Tessera build with declared settings, as an older one refuses its manifest;
- keep up to 64 KiB of text across restarts, stored as `state.txt` in its own directory;
- read the files its manifest lists under `permissions.files`, and no others;
- send HTTPS requests to the hosts its manifest lists under `permissions.network`, and no others. Requests run in the background and the answer is delivered later; redirects are not followed, a response is cut off at 1 MiB, and servers are verified against the Windows certificate store;
- send plain HTTP requests to the servers its manifest lists under `permissions.local`, each a name or address with its port, as `nas.local:8080`. A request is sent only when the name stands for addresses of this machine or of a private or link-local network; nothing protects what is sent from others on that network, and the name is looked up a second time for the request itself, so a name server that answers differently then is not guarded against. The marketplace lists such a server as `http://nas.local:8080` among the hosts an extension asks to contact;
- since 0.4, with the permissions of `[permissions] system`: learn what media plays and control it, read the meters of the machine and the network connection, list the windows and bring them in front, minimize, close and move them between workspaces, know the workspaces and the colors of the theme, open and close its own popups, do the actions of Tessera its manifest lists under `permissions.actions`, and post notices;
- since 0.4: be drawn again when what a widget watches changes, answer the launcher, and be told when its keys are pressed or its settings change;
- learn which Bluetooth devices are paired with the machine, whether each is connected and the charge Windows knows of it, when its manifest lists `bluetooth` under `permissions.devices`. Tessera reads the devices on a thread of its own every twenty seconds while an extension asks, so an answer can be that old, and the first question is answered with an error while the first reading runs. An extension cannot connect, disconnect or talk to a device.

The limits of that:

- Permissions are granted by the manifest alone. The `marketplace` widget shows what an extension asks for before it installs it; an extension copied in by hand is loaded without a question, and nothing shows what an installed extension is permitted, so read `extension.toml`: an extension with a file and a host permitted can send that file to that host.
- Settings, API keys included, are plain text in `config.toml`.
- Beyond what is listed above it cannot reach devices, other processes or anything else of Windows: it cannot connect to a device, start a program, read the registry or see the contents of other windows.
- It adds widgets and popups made of widgets. Its manifest can list `[[popup]]` entries, written as in a layout, and name one of them as the `popup` of a widget: a click on that widget then opens it without anything in `config.toml`. A layout knows such a popup as `<id>.<popup name>`; a `[[popup]]` of that name in the layout replaces it, a `popup` given to the widget in the layout opens that one instead, and `popup = ""` opens none. A popup of an extension can hold widgets of the shell and of other extensions too. Since 0.4 an extension can open and close its own popups from its code; it cannot open those of the shell or of other extensions but through `run-action` with the action `popup`, and cannot add a surface or a screen of another kind.
- Each extension runs on a thread of its own, so what it computes and what it asks of Windows does not hold up the shell. Each call into an extension is limited in computation, and an extension is limited to 64 MiB of memory. A widget whose extension exceeds a limit or crashes shows "stopped working" until Tessera restarts.
- Extensions are loaded at startup, and again when the `marketplace` widget installs or removes one, when the manifest or the WebAssembly file of one is put into the extensions directory, replaced or removed by hand (noticed within about two seconds), and when the registries change; all of them start anew then. A changed `[extension.<id>]` table no longer loads them again: the new values reach an extension when it is loaded next, as when Tessera starts (see [DECLARED-SETTINGS.md](DECLARED-SETTINGS.md)). Tried on a running shell with the `ai-limits` example and no credentials, before settings stopped loading the extensions again: its widget appeared in the bar after its directory was copied in, and showed one service fewer after the setting that lists them was changed, both without a restart. Text in a row an extension describes stays on one line, so a full bar does not break it into lines, and a column that is the whole widget scrolls when it is taller than its place, or, in a window fewer than three pills high such as a bar, is laid out as a row, which was not tried on a running shell; neither was seen going wrong before the change on the machine at hand, so that the change cures what a user reported as a broken widget in the bar is not confirmed. A widget placed on several displays shares one extension instance.
- Nothing is signed and nobody reviews what a registry offers; see below.

## Interface 0.4

[`wit/extension.wit`](../wit/extension.wit) is the package `tessera:extension@0.4.0`, one world named `extension`. Everything below was written against the Windows API and checked to build for Windows; it has not run on Windows yet. The parts that need no Windows were tried: the 0.3 `counter` component as the registry offers it and the `system-monitor` example built for 0.4 were loaded in a stand-in host on the same Wasmtime version, which called their exports, and the unit tests below cover the rules.

### Compatibility of components built for 0.3

The imports and exports of the world are names without a version, so a version of the interface only adds: 0.4 adds imports, exports and node kinds and changes none that 0.3 had. Tessera provides every import of 0.3 unchanged, looks each export up by its name, and calls `settings-changed`, `answer`, `run-answer` and `shortcut` only where a component has them. `render` of a component built for 0.3 returns the six node kinds of 0.3, which Tessera reads as such. So every extension built for 0.3, the `.wasm` files the registry of this repository offers among them, loads and runs as before, without being built again; a test loads the `counter` example exactly as the registry offered it for 0.3 and renders it.

What does need a change is building such an extension again from its source: a crate that calls `wit_bindgen::generate!` on this world itself must export all of it, so it adds the four exports, each doing nothing. An extension built on [`extensions/kit`](../extensions/kit) has them from the kit and needs no change.

### Exports

| Export | Called when |
| --- | --- |
| `render(widget) -> list<node>` | the content of a widget is wanted: when it is placed, after each of the calls below, every `refresh_seconds` of the widget, and when what it `watch`es changed |
| `activate(widget, action)` | a button was pressed, or a field, a slider or a toggle changed (see [Nodes](#nodes)) |
| `fetched(request, outcome)` | a request of `fetch` came back |
| `settings-changed()` | 0.4: the `[extension.<id>]` table of the configuration changed; `setting` answers with the new values from then on |
| `answer(query) -> list<answer>` | 0.4: something was typed into the launcher and the manifest has a `[runner]` table |
| `run-answer(id)` | 0.4: an answer whose action is `run` was chosen |
| `shortcut(id)` | 0.4: the keys of a `[[shortcut]]` were pressed |

Each extension runs on a thread of its own, and the calls reach it in order. The thread that draws does not wait for it: a widget shows what the extension described last until a new content comes, and nothing before the first. What an extension asks to be done (a request, a popup, an action, a notice, something done to a window) is done once the call has returned, by the thread that draws.

### Nodes

Besides `row`, `column`, `text`, `caption`, `button` and `progress`:

- `image` draws a picture `height` logical pixels high (from 1 to 512, or the height of a line of text for 0), as wide as it is in proportion: the `bytes` of a PNG or JPEG file of at most 2 MiB, a PNG or JPEG of at most 2 MiB at an `https://` address of a host the manifest lists under `permissions.network` (fetched once while the widget shows it, without following redirects), or the icon of an `app`, the path of its `.exe` file on a local drive (a network path is refused) or the AppUserModelID of a packaged application, as the dock draws it. A picture of more than 4096 pixels in width or height is not shown. A picture is decoded off the thread that draws, and its place stays empty until it is there.
- `icon` draws a character of the icon font of the shell, Segoe Fluent Icons. A `button` whose label is one such character draws it in that font too, which makes buttons that play or skip.
- `text-input` is a field of one line with an `id`, a `placeholder` and a `value`. What is typed comes to `activate` as `input-changed:<id>:<text>`, and Enter as `submitted:<id>:<text>`; letters typed while the extension is busy come together, as the last text. The field shows `value` when it is first drawn, and again when a `render` gives a value other than the one before that is not one of the texts it was told of and has not given back yet (they come back in the order they were told), so an extension can keep the text and give it back without taking the caret away, and can empty the field after Enter. A field takes the keys of the window it is in; a bar of Tessera does not take the keyboard from the window in front, so a field is meant for a popup.
- `slider` has an `id`, a `min`, a `max` greater than `min`, and a `value`; a move comes as `slider-changed:<id>:<value>`, moves made while the extension is busy as the last one. For a moment after the user moved it, a value the extension gives back does not move the handle.
- `toggle` is a switch with an `id`; a click comes as `toggled:<id>:true` or `toggled:<id>:false`, the state it was switched to, and the switch moves at once.
- `scroll(max-height)` is a column at most that many logical pixels high (1 to 4000) that scrolls what does not fit, wherever it stands; in a bar it is laid out as a row, as a column is.
- `spacer` takes what room is left in its row or column, pushing what comes after it to the end.

The `id` of a field, a slider or a toggle is made of a-z, 0-9 and `-`, at most 64 of them, and differs from the others of the widget. A node that breaks a rule is left out, with what it holds, and the reason is written to the problems of Tessera once: an `id` that is not one or that another input has, a slider whose range is empty, a picture that is too large, of another kind or from a host that is not permitted, an `app` that is no `.exe` path or AppUserModelID, an `icon` of more than one or two characters. A node is the child of one node only, the first that names it, and of none that comes after it; other mentions of it are ignored. A widget has at most 4000 nodes; those after are left out. Numbers out of range are moved into it.

### Imports

Those of 0.3 are unchanged. Each import added in 0.4 fails, saying why, unless the manifest lists its permission under `[permissions] system`; `run-action` needs the action under `[permissions] actions`. While `answer` runs, the imports that do something (`fetch`, `media-control`, `window-command`, the popups, `run-action`, `notify`) fail as well.

| Import | Permission | What it gives or does |
| --- | --- | --- |
| `media-now()` | `media` | what the media player Windows shows controls for plays (title, artist, application, whether it plays, how far, when the player says), read at most half a second ago |
| `media-control(command)` | `media-control` | `play`, `pause`, `toggle`, `next` or `previous` to that player |
| `system-meters()` | `meters` | the share of the processors that was busy, the memory in use and in all, the battery, and the bytes per second received and sent over every adapter; read at most a second ago and shared by the extensions; the processor and the network count from the reading before, so the first reading has neither |
| `network-state()` | `network` | the connection Windows prefers: its name, wired, Wi-Fi or mobile with bars, and whether it reaches the internet |
| `windows()` | `windows` | the windows a task switcher shows, the one in front first: a number for each, its title, its program or packaged application, a number for its display, the workspace of Tessera it lives on, whether it is minimized and whether it is in front |
| `window-command(id, action)` | `window-control` | `focus` (going to its workspace where it lives on another), `minimize`, `close` (as its close button) or `move-to-workspace(n)`, for a window `windows` lists; no other window, and none of Tessera, can be reached |
| `workspace-state()` | `workspaces` | how many workspaces the display under the pointer has, which one it shows, their names, and whether they are Tessera's own or the virtual desktops of Windows |
| `theme-colors()` | `theme` | the seven colors of the active theme as `#rrggbb` |
| `open-popup(name)`, `close-popup(name)` | `popups` | opens a popup of the manifest unless the extension opened it and it is open, or closes it if it is; see below where it opens |
| `run-action(name, arg)` | the name under `actions` | does an [action](CONFIGURATION.md#actions) of Tessera, as a `[[key]]` does; ten in ten seconds at most |
| `notify(title, body)` | `notify` | posts a notice with the notifications of Tessera, shown as a banner where banners are; five in a minute at most |

A popup opened from `activate` opens at the widget that was pressed. Opened from any other call it opens at the first widget of the extension on the display the pointer is on, or else in the middle of a bar of that display. Opening it closes any other popup, as opening a popup does.

The workspaces, the workspace of each window and the colors of the theme are what only the thread that draws knows. It writes them down for all extensions at once: every second from the time an extension with one of the permissions `windows`, `window-control`, `workspaces` or `theme` has a widget placed, a shortcut pressed or an answer chosen, until the extensions are loaded again, and every quarter of a second while a widget watches them. Before that, and when what was written down is more than three seconds old, `workspace-state` and `theme-colors` fail saying so, and `windows` gives no workspaces; so `answer` of an extension that shows nothing else cannot read them yet.

### Watching

`watch = ["media", "meters", "network", "windows", "workspaces", "theme"]` in a `[[widget]]` of the manifest has the widget drawn again when one of them changes, at most four times a second: changes within a quarter of a second come to one `render` at its end. Each needs the permission of the same name (`media`, `meters`, `network`, `windows`, `workspaces`, `theme`), or the manifest is refused. While a widget watches the media or the meters, a thread of Tessera reads them every second; a change of the media is another piece, player or state of playing, not the position, which a widget that shows it asks for with `refresh_seconds`. The windows and the network change as Windows reports, the workspaces and the theme as Tessera sees them.

### Runners

An extension whose manifest has a `[runner]` table answers what is typed into the launcher, the compact runner and the Run dialog, in the manner of a plugin of KRunner:

```toml
[runner]
prefix = "sys"   # optional: only queries starting with `sys` are handed over, without it
```

The runner `extensions` of the launcher (named in its `runners` option like the others) asks every such extension at once, each on its thread, and puts their answers together; answers that come later than 300 milliseconds after the query are left out, and so are those of an extension that is busy with something else meanwhile. Without a prefix every query is handed over. With one, the query must start with it followed by a space or `:`, or be the prefix alone, which hands over an empty query. Each `answer` has an `id` (up to 64 letters, digits, `-`, `_` and `.`), a `title`, a `detail`, an `icon` (a character of the icon font; that of extensions where it names none), a `score` up to 1000 of which at most 800 counts, so that an extension does not stand above everything, and an `action`: `copy(text)`, `open(url)` for an `https://` address, or `run`, which calls `run-answer` with the `id`. At most 20 answers of an extension are taken, each `id` once, and the launcher remembers a chosen answer as `extension:<id>:<answer id>`. The launcher can only start commands, so `run` starts `tessera do extension-answer <extension> <answer id>`, which hands the choice to the running Tessera.

### Shortcuts

```toml
[[shortcut]]
id = "panel"                     # a-z, 0-9 and `-`; handed to `shortcut`
label = "Open the system monitor"  # what it does, for whoever reads the manifest
keys = "win+alt+m"               # optional: its keys unless the user binds them otherwise
```

Each `[[shortcut]]` becomes the [action](CONFIGURATION.md#actions) `<extension id>.<shortcut id>`, which calls `shortcut` with the `id`. Its `keys` are a combination as a `[[key]]` writes it (a manifest with keys Tessera cannot hold is refused), and are bound by default where Tessera takes the keys of Windows (`windows_keys`, on by default when Tessera is the shell), as the keys actions bring are; otherwise, or for other keys, a `[[key]]` names the action: `press = "ctrl+alt+m"`, `action = "system-monitor.panel"`. `tessera do system-monitor.panel` does it from a console. The shortcuts are read once, when Tessera starts: those of an extension installed later work from the next start. Tessera shows the `label` nowhere yet.

### Permissions

```toml
[permissions]
network = ["api.example.com"]        # hosts reached over HTTPS, and pictures fetched from them
local = ["nas.local:8080"]           # servers of the local network reached over plain HTTP
files = ["~/notes.txt"]              # files read with read-file
devices = ["bluetooth"]              # paired Bluetooth devices
system = ["media", "media-control", "meters", "network", "windows",
          "window-control", "workspaces", "theme", "popups", "notify"]
actions = ["media", "popup"]         # actions of Tessera run-action may do
```

`Permissions::wishes` gives what a manifest asks for as one line for each kind, in the language of Tessera, for the prompt the marketplace shows before it installs an extension: the hosts, the files and the devices with the words of 0.3, then each permission of `system`, then the actions.

## Registries

The `marketplace` widget lists the extensions that registries offer and installs, updates and removes them; with the option `show = "themes"` it does the same for the themes they offer, and `columns` says in how many columns the offers stand (1 to 4, 1 unless said). The settings show both, under **Extensions** and **Themes**. It contacts the registries when it is shown and when `Refresh` is pressed, not before.

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

[[theme]]
id = "night"
name = "Night"
description = "Shown under the name."
version = "1.0.0"
file = "themes/night.toml"
sha256 = "…"
preview = "previews/night.jpg"  # optional: shown above it
preview_sha256 = "…"

[[registry]]
url = "https://example.org/more/registry.toml"
```

`manifest`, `wasm`, `file` and `url` are `https://` addresses or paths relative to the registry file, which cannot lead out of its directory; a registry on the web cannot name files of this machine. A registry can name further registries, and those are read as well, down to three steps from a listed one and at most 16 registries in all; each is read once. When two registries offer an extension or a theme of the same `id`, the one read first wins, and the listed ones are read before those they name.

What this does and does not guard:

- Both files are checked against the SHA-256 sums in the registry, and the `id` in the manifest has to be the offered one. That catches a damaged or swapped download; it says nothing about the author, because whoever controls a registry controls the sums too. There are no signatures and no review.
- Before installing or updating, the widget names the hosts and files the manifest asks for and waits for `Allow and install`. Nothing else of the extension is inspected.
- Installing writes `extension.wasm`, `extension.toml` and `installed.toml` (the version and the registry it came from) to `%APPDATA%\tessera\extensions\<id>\`. It does not change the layout: the widget lists the names to put into `config.toml`, such as `counter.total`, and that is still done by hand.
- `Update` is offered when the version of the registry differs from the installed one; nothing checks for updates on its own.
- `Remove` deletes those files and the extension's `state.txt`. An extension without `installed.toml` was copied in by hand and is neither replaced nor removed. What is installed and offered by no registry that was read is listed before the offers, with its name and the widgets it gives a layout, and with no buttons; this was not tried on a running shell.
- A registry or an offer that cannot be used is listed at the top of the widget with the reason.
- A `preview` (a PNG or JPEG of at most 2 MiB) is checked against its `preview_sha256` like the other files and drawn above a theme, or as the icon of an extension. One that cannot be had leaves the initial of the name in its place. [`registry/preview.py`](../registry/preview.py) draws the pictures of this repository: a desktop mock-up in the colors and shapes of each theme over its wallpaper, and the icon of each extension.
- A theme is checked against its SHA-256 sum and has to parse as a theme file, at most 64 KiB of it. Installing writes `<id>.toml` and `<id>.installed` (the version and the registry) to `%APPDATA%\tessera\themes\`; it does not choose the theme, which the settings or `theme = "<id>"` do. A theme file without its `.installed` file was written by hand and is neither replaced nor removed. Removing the theme in use leaves the layout naming a missing file, which is reported, and the built-in theme shows.

Tried on a running shell: listing, installing and removing the `counter` example, from a registry file on disk and from the registry of this repository on the web, with the widgets appearing in and leaving the bar. Installing the theme `void` from a registry file on disk was tried too, after which the settings offered it. `Update`, failed downloads, nested registries and removing a theme are covered by tests only.

[`registry/update.sh`](../registry/update.sh) rebuilds the examples, copies the themes of [`themes/`](../themes) and rewrites the registry of this repository. [`registry/preview.py`](../registry/preview.py) draws the pictures it lists; run it first.

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

[`extensions/system-monitor`](../extensions/system-monitor) is an example of the interface 0.4. `system-monitor.meters` is the processor and the memory in use for a bar, drawn again when the meters change, and a click on it opens the popup it brings, `system-monitor.panel`: the meters with the network and the battery, what plays with buttons that go back, play or pause and skip, and the open windows with their icons, filtered by a field, in a list that scrolls, each with a button that brings it in front. In the launcher, `sys` lists the meters, to copy, and `sys notes` the windows whose title or program has `notes`, to bring in front; its shortcut `system-monitor.panel` opens the popup and is bound to Win+Alt+M where Tessera takes the keys of Windows. `windows` in `[extension.system-monitor]` says how many windows the panel lists (8, from 1 to 50). It asks for `meters`, `media`, `media-control`, `windows`, `window-control` and `popups`, and sends nothing anywhere. It builds for `wasm32-unknown-unknown` and its model is covered by tests; the stand-in host rendered its widgets and called its shortcut and its answers, which none of it has done in Tessera on Windows yet.

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

Besides the four examples above, the registry of this repository offers these. Each is a crate under [`extensions/`](../extensions) built on [`extensions/kit`](../extensions/kit), which generates the bindings once and gives a plugin a tree builder for its widgets, settings with defaults, state kept as `key=value` lines, timed requests with retries and a queue, time and number formatting and a small random generator. A plugin implements `kit::Extension` and exports itself with `kit::extension!`, which answers every export of the interface, those of 0.4 doing nothing unless the plugin says otherwise; the kit also builds the nodes of 0.4 and wraps its imports ([its README](../extensions/kit/README.md)). Each manifest explains its settings in comments.

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
