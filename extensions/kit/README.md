# Tessera extension kit

What the extensions of this repository share: the bindings of
[`wit/extension.wit`](../../wit/extension.wit), generated once, and small helpers
around them. An extension is a crate of type `cdylib` built for
`wasm32-unknown-unknown`, which depends on the kit by path:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
tessera-extension-kit = { path = "../kit" }
```

It implements `kit::Extension` for a type that holds its state and hands that type
to `kit::extension!`, which keeps one value of it for the life of the extension and
answers every export of the interface with it. Each method but `start` and `render`
has an answer that does nothing, so an extension writes only what it uses:

| Method | Called when |
| --- | --- |
| `start(now)` | before the first call, to make the state |
| `render(widget, now)` | Tessera wants the content of `widget`: after every other call, every `refresh_seconds`, and when what the widget `watch`es changed |
| `activate(widget, action, now)` | a button of `widget` was pressed |
| `input(widget, event, now)` | a button was pressed, a field typed into or submitted, a slider moved or a toggle flipped; it hands presses on to `activate` unless the extension says otherwise |
| `fetched(request, outcome, now)` | a request of `fetch` came back |
| `settings_changed(now)` | the `[extension.<id>]` table of the configuration changed; Tessera does not start the extension again, so what `start` read of the settings is read again here (`Feed::retune`, `Schedule::retune` and `Fetcher::clear` ask anew at once) |
| `answer(query, now)` | something was typed into the launcher and the manifest has a `[runner]` table; it has 300 milliseconds |
| `run_answer(id, now)` | an answer whose action is `AnswerAction::Run` was chosen |
| `shortcut(id, now)` | the keys of a `[[shortcut]]` of the manifest were pressed |

```rust
use tessera_extension_kit::{self as kit, Event, Extension, Ui};

struct Hello {
    name: String,
}

impl Extension for Hello {
    fn start(_now: u64) -> Self {
        Hello { name: String::new() }
    }

    fn render(&mut self, _widget: &str, _now: u64) -> Ui {
        let mut ui = Ui::panel();
        ui.text_input(Ui::ROOT, "name", "Your name", self.name.clone());
        ui.text(Ui::ROOT, format!("Hello, {}", self.name));
        ui
    }

    fn input(&mut self, _widget: &str, event: Event<'_>, _now: u64) {
        if let Event::InputChanged { id: "name", text } = event {
            self.name = text.to_owned();
        }
    }
}

kit::extension!(Hello);
```

## What the kit has

- `Ui`, a builder of the tree `render` returns: `Ui::bar()` for a widget of a bar,
  `Ui::panel()` for a popup, and `row`, `column`, `scroll`, `text`, `caption`, `button`,
  `progress`, `icon`, `image` (the bytes of a PNG or JPEG, at most `IMAGE_LIMIT`),
  `image_url`, `app_icon`, `text_input`, `slider`, `toggle` and `spacer` to add nodes
  under a parent. Numbers are kept within what Tessera accepts.
- `Event`, what the user did to a widget, read from the action `activate` is given:
  `Pressed`, `InputChanged`, `Submitted`, `SliderChanged` and `Toggled`. A button whose
  action starts like one of the forms Tessera writes for inputs (`input-changed:`,
  `submitted:`, `slider-changed:`, `toggled:`) is read as that form.
- `shell`, the state of the machine and of the shell and what can be done to it, each
  behind a permission of the manifest: `media` and `media_control`, `meters` and
  `memory_share`, `network`, `windows` with `focus`, `minimize`, `close` and
  `move_to_workspace`, `workspaces`, `palette` with `rgb`, `open_popup`,
  `close_popup`, `run` and `notify`.
- `settings`, readers for the settings with defaults and ranges; `store`, state kept
  across restarts as `key=value` lines; `net`, requests on a timer, with retries and in
  a queue; `time`, calendar and clock arithmetic; `format`, numbers and text for small
  spaces; `rng`, a small random number generator.

Everything but the calls into Tessera can be tested natively with `cargo test`.

[`docs/EXTENSIONS.md`](../../docs/EXTENSIONS.md) describes the manifest, the
permissions and what Tessera does with each call.
