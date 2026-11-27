// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Runs extension code in a WebAssembly sandbox.
//!
//! The exports of interface 0.3 are looked up by name and called as they
//! always were; those added in 0.4 are looked up the same way and called
//! only where the component has them, so that a component built for 0.3
//! loads and runs as before. What its `render` returns is read with the
//! node kinds of 0.3 and taken over into those of 0.4.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, anyhow};
use wasmtime::component::{Component, ComponentNamedList, HasSelf, Lift, Linker, Lower, TypedFunc};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

#[allow(unsafe_code)]
mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "extension",
    });

    /// The node kinds of interface 0.3, which `render` of a component
    /// built for it returns. Only the names of the cases and fields count,
    /// not those of the types.
    pub mod legacy {
        use wasmtime::component::{ComponentType, Lift};

        #[derive(ComponentType, Lift)]
        #[component(record)]
        pub struct Button {
            pub label: String,
            pub action: String,
        }

        #[derive(ComponentType, Lift)]
        #[component(variant)]
        pub enum NodeKind {
            #[component(name = "row")]
            Row,
            #[component(name = "column")]
            Column,
            #[component(name = "text")]
            Text(String),
            #[component(name = "caption")]
            Caption(String),
            #[component(name = "button")]
            Button(Button),
            #[component(name = "progress")]
            Progress(f32),
        }

        #[derive(ComponentType, Lift)]
        #[component(record)]
        pub struct Node {
            pub kind: NodeKind,
            pub children: Vec<u32>,
        }
    }
}

#[cfg(test)]
pub use bindings::Image;
pub use bindings::{Answer, AnswerAction, ImageSource, Node, NodeKind, Slider, TextInput, Toggle};

use super::access::{Access, SystemKind};
use super::http::{HttpRequest, HttpResponse};
use super::system::{self, MediaDeed};
use super::windows_api::{self, WindowDeed};
use crate::platform::{AppWindowId, Link};

/// Fuel granted to one call into an extension; a call that runs out traps.
const FUEL_PER_CALL: u64 = 100_000_000;

/// The most linear memory one extension may hold.
const MEMORY_LIMIT: usize = 64 << 20;

/// The most requests of one extension that may be under way at a time.
const REQUEST_LIMIT: usize = 4;

/// How many notices an extension may post in a minute.
const NOTICE_LIMIT: usize = 5;

/// How many actions an extension may do in ten seconds.
const ACTION_LIMIT: usize = 10;

/// The longest title and text of a notice of an extension, in letters.
const NOTICE_TITLE: usize = 200;
const NOTICE_BODY: usize = 1000;

/// A request an extension started, and the number it knows it by.
pub type NumberedRequest = (u32, HttpRequest);

/// What an extension asked to be done once its call has returned, by the
/// thread that draws.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Opens its popup of this name, as the manifest names it.
    OpenPopup(String),
    ClosePopup(String),
    RunAction {
        name: String,
        arg: Option<String>,
    },
    Notify {
        title: String,
        body: String,
    },
    Window(AppWindowId, WindowDeed),
}

/// Counts what happened within a while, to let only so much of it happen.
#[derive(Debug)]
struct Rate {
    limit: usize,
    within: Duration,
    times: VecDeque<Instant>,
}

impl Rate {
    fn new(limit: usize, within: Duration) -> Self {
        Self {
            limit,
            within,
            times: VecDeque::new(),
        }
    }

    /// Whether one more may happen at `now`, which then counts.
    fn admit(&mut self, now: Instant) -> bool {
        while let Some(&first) = self.times.front() {
            if now.saturating_duration_since(first) < self.within {
                break;
            }
            self.times.pop_front();
        }
        let admitted = self.times.len() < self.limit;
        if admitted {
            self.times.push_back(now);
        }
        admitted
    }
}

/// What the host functions of one extension can reach.
struct HostState {
    id: String,
    limits: StoreLimits,
    access: Access,
    /// Requests accepted since they were last taken for sending.
    accepted: Vec<NumberedRequest>,
    /// Requests whose outcome the extension has not been given yet.
    under_way: usize,
    next_request: u32,
    /// What is to be done once the call returns.
    effects: Vec<Effect>,
    /// Whether `answer` runs, during which nothing is to be done.
    answering: bool,
    notices: Rate,
    actions: Rate,
}

impl HostState {
    /// Fails unless the manifest permits `kind`.
    fn permits(&self, kind: SystemKind) -> Result<(), String> {
        self.access.permissions.check_system(kind)
    }

    /// Fails unless something may be done now and the manifest permits
    /// `kind`, where it takes a permission.
    fn may_do(&self, kind: Option<SystemKind>) -> Result<(), String> {
        if self.answering {
            return Err("nothing is done while the extension answers the launcher".to_owned());
        }
        kind.map_or(Ok(()), |kind| self.permits(kind))
    }

    /// Fails unless `name` is a popup of the manifest.
    fn own_popup(&self, name: &str) -> Result<(), String> {
        match self.access.popups.iter().any(|popup| popup == name) {
            true => Ok(()),
            false => Err(format!("the manifest has no popup named `{name}`")),
        }
    }
}

impl bindings::ExtensionImports for HostState {
    fn log(&mut self, message: String) {
        report!("[{}] {message}", self.id);
    }

    fn now(&mut self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs())
    }

    fn setting(&mut self, key: String) -> Option<String> {
        self.access.settings.get(&key).cloned()
    }

    fn load_state(&mut self) -> String {
        self.access.load_state()
    }

    fn save_state(&mut self, state: String) -> Result<(), String> {
        self.access.save_state(&state)
    }

    fn read_file(&mut self, path: String) -> Result<String, String> {
        self.access.read_file(&path)
    }

    fn bluetooth_devices(&mut self) -> Result<Vec<bindings::BluetoothDevice>, String> {
        let devices = self.access.bluetooth()?.into_iter();
        let devices = devices.map(|device| bindings::BluetoothDevice {
            name: device.name,
            connected: device.connected,
            battery: device.battery,
        });
        Ok(devices.collect())
    }

    fn fetch(&mut self, request: bindings::Request) -> Result<u32, String> {
        self.may_do(None)?;
        self.access.permissions.check_url(&request.url)?;
        if self.under_way >= REQUEST_LIMIT {
            return Err(format!("{REQUEST_LIMIT} requests are already under way"));
        }
        let number = self.next_request;
        self.next_request = number.wrapping_add(1);
        self.under_way += 1;
        let headers = request.headers.into_iter();
        self.accepted.push((
            number,
            HttpRequest {
                method: request.method,
                url: request.url,
                headers: headers.map(|header| (header.name, header.value)).collect(),
                body: request.body,
            },
        ));
        Ok(number)
    }

    fn media_now(&mut self) -> Result<Option<bindings::Media>, String> {
        self.permits(SystemKind::Media)?;
        let playing = system::media()?;
        Ok(playing.map(|playing| bindings::Media {
            title: playing.title,
            artist: playing.artist,
            app: playing.app,
            playing: playing.playing,
            position: playing.position.map(|position| bindings::MediaPosition {
                elapsed: position.elapsed,
                length: position.length,
            }),
        }))
    }

    fn media_control(&mut self, command: bindings::MediaCommand) -> Result<(), String> {
        self.may_do(Some(SystemKind::MediaControl))?;
        system::media_control(match command {
            bindings::MediaCommand::Play => MediaDeed::Play,
            bindings::MediaCommand::Pause => MediaDeed::Pause,
            bindings::MediaCommand::Toggle => MediaDeed::Toggle,
            bindings::MediaCommand::Next => MediaDeed::Next,
            bindings::MediaCommand::Previous => MediaDeed::Previous,
        })
    }

    fn system_meters(&mut self) -> Result<bindings::Meters, String> {
        self.permits(SystemKind::Meters)?;
        let meters = system::meters();
        let memory = meters.memory;
        Ok(bindings::Meters {
            cpu: meters.cpu,
            memory_used: memory.map_or(0, |memory| memory.used),
            memory_total: memory.map_or(0, |memory| memory.total),
            battery: meters.battery.map(|battery| bindings::Battery {
                percent: battery.percent,
                plugged_in: battery.plugged_in,
            }),
            received: meters.traffic.map(|(received, _)| received),
            sent: meters.traffic.map(|(_, sent)| sent),
        })
    }

    fn network_state(&mut self) -> Result<Option<bindings::Network>, String> {
        self.permits(SystemKind::Network)?;
        Ok(system::network().map(|network| bindings::Network {
            name: network.name,
            link: match network.link {
                Link::Wired => bindings::NetworkLink::Wired,
                Link::Wireless(bars) => bindings::NetworkLink::Wireless(bars),
                Link::Cellular(bars) => bindings::NetworkLink::Cellular(bars),
            },
            internet: network.internet,
        }))
    }

    fn windows(&mut self) -> Result<Vec<bindings::WindowInfo>, String> {
        self.permits(SystemKind::Windows)?;
        let shell = system::shell();
        let rows = windows_api::list(shell.as_ref()).into_iter();
        let windows = rows.map(|row| bindings::WindowInfo {
            id: row.id,
            title: row.title,
            app: row.app,
            display: row.display,
            workspace: row.workspace,
            minimized: row.minimized,
            focused: row.focused,
        });
        Ok(windows.collect())
    }

    fn window_command(&mut self, id: u64, action: bindings::WindowAction) -> Result<(), String> {
        self.may_do(Some(SystemKind::WindowControl))?;
        let deed = match action {
            bindings::WindowAction::Focus => WindowDeed::Focus,
            bindings::WindowAction::Minimize => WindowDeed::Minimize,
            bindings::WindowAction::Close => WindowDeed::Close,
            bindings::WindowAction::MoveToWorkspace(place) => WindowDeed::Move(place),
        };
        deed.check(system::shell().as_ref())?;
        let window =
            windows_api::find(id).ok_or_else(|| format!("there is no window numbered {id}"))?;
        self.effects.push(Effect::Window(window, deed));
        Ok(())
    }

    fn workspace_state(&mut self) -> Result<bindings::Workspaces, String> {
        self.permits(SystemKind::Workspaces)?;
        let workspaces = shell_now()?.workspaces;
        Ok(bindings::Workspaces {
            count: workspaces.count,
            current: workspaces.current,
            names: workspaces.names,
            tessera: workspaces.tessera,
        })
    }

    fn theme_colors(&mut self) -> Result<bindings::Palette, String> {
        self.permits(SystemKind::Theme)?;
        let [
            surface,
            surface_highest,
            on_surface,
            on_surface_variant,
            outline,
            primary,
            secondary_container,
        ] = shell_now()?.palette;
        Ok(bindings::Palette {
            surface,
            surface_highest,
            on_surface,
            on_surface_variant,
            outline,
            primary,
            secondary_container,
        })
    }

    fn open_popup(&mut self, name: String) -> Result<(), String> {
        self.may_do(Some(SystemKind::Popups))?;
        self.own_popup(&name)?;
        self.effects.push(Effect::OpenPopup(name));
        Ok(())
    }

    fn close_popup(&mut self, name: String) -> Result<(), String> {
        self.may_do(Some(SystemKind::Popups))?;
        self.own_popup(&name)?;
        self.effects.push(Effect::ClosePopup(name));
        Ok(())
    }

    fn run_action(&mut self, name: String, arg: Option<String>) -> Result<(), String> {
        self.may_do(None)?;
        self.access.permissions.check_action(&name)?;
        if !self.actions.admit(Instant::now()) {
            return Err(format!(
                "at most {ACTION_LIMIT} actions are done in ten seconds"
            ));
        }
        self.effects.push(Effect::RunAction { name, arg });
        Ok(())
    }

    fn notify(&mut self, title: String, body: String) -> Result<(), String> {
        self.may_do(Some(SystemKind::Notify))?;
        if title.trim().is_empty() {
            return Err("a notice needs a title".to_owned());
        }
        if !self.notices.admit(Instant::now()) {
            return Err(format!(
                "at most {NOTICE_LIMIT} notices are posted in a minute"
            ));
        }
        let cut = |text: String, limit: usize| text.chars().take(limit).collect::<String>();
        self.effects.push(Effect::Notify {
            title: cut(title, NOTICE_TITLE),
            body: cut(body, NOTICE_BODY),
        });
        Ok(())
    }
}

/// Compiles and instantiates extensions.
pub struct Runtime {
    engine: Engine,
    linker: Linker<HostState>,
}

impl Runtime {
    pub fn new() -> anyhow::Result<Self> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(describe)?;
        let mut linker = Linker::new(&engine);
        bindings::Extension::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
            .map_err(describe)?;
        Ok(Self { engine, linker })
    }

    /// Instantiates the extension `id` from `wasm`, which is either a
    /// component or a core module carrying its component type information.
    pub fn instantiate(&self, id: &str, wasm: &[u8], access: Access) -> anyhow::Result<Instance> {
        let component = if is_component(wasm) {
            Component::new(&self.engine, wasm)
        } else {
            let bytes = wit_component::ComponentEncoder::default()
                .module(wasm)
                .and_then(|encoder| encoder.validate(true).encode())
                .map_err(describe)
                .context("the module is not a Tessera extension")?;
            Component::new(&self.engine, bytes)
        }
        .map_err(describe)
        .context("the extension does not compile")?;

        let state = HostState {
            id: id.to_owned(),
            limits: StoreLimitsBuilder::new().memory_size(MEMORY_LIMIT).build(),
            access,
            accepted: Vec::new(),
            under_way: 0,
            next_request: 0,
            effects: Vec::new(),
            answering: false,
            notices: Rate::new(NOTICE_LIMIT, Duration::from_secs(60)),
            actions: Rate::new(ACTION_LIMIT, Duration::from_secs(10)),
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|state| &mut state.limits);
        store.set_fuel(FUEL_PER_CALL).map_err(describe)?;
        let instance = self
            .linker
            .instantiate(&mut store, &component)
            .map_err(describe)
            .context("the extension does not match the Tessera extension interface")?;
        let exports = Exports::of(&instance, &mut store)
            .context("the extension does not match the Tessera extension interface")?;
        Ok(Instance { store, exports })
    }
}

/// How `render` of a component is called.
enum Render {
    /// As interface 0.4 has it.
    Current(TypedFunc<(String,), (Vec<Node>,)>),
    /// As interface 0.3 had it, with fewer node kinds.
    Legacy(TypedFunc<(String,), (Vec<bindings::legacy::Node>,)>),
}

type Fetched = TypedFunc<(u32, Result<bindings::Response, String>), ()>;

/// The exports of a component, each looked up by its name.
struct Exports {
    render: Render,
    activate: TypedFunc<(String, String), ()>,
    fetched: Fetched,
    /// Added in 0.4; a component built for 0.3 has none of these.
    settings_changed: Option<TypedFunc<(), ()>>,
    answer: Option<TypedFunc<(String,), (Vec<Answer>,)>>,
    run_answer: Option<TypedFunc<(String,), ()>>,
    shortcut: Option<TypedFunc<(String,), ()>>,
}

impl Exports {
    fn of(
        instance: &wasmtime::component::Instance,
        store: &mut Store<HostState>,
    ) -> anyhow::Result<Self> {
        let render = match instance.get_typed_func(&mut *store, "render") {
            Ok(current) => Render::Current(current),
            Err(current) => match instance.get_typed_func(&mut *store, "render") {
                Ok(legacy) => Render::Legacy(legacy),
                Err(_) => return Err(describe(current)).context("`render` does not match"),
            },
        };
        Ok(Self {
            render,
            activate: required(instance, store, "activate")?,
            fetched: required(instance, store, "fetched")?,
            settings_changed: optional(instance, store, "settings-changed")?,
            answer: optional(instance, store, "answer")?,
            run_answer: optional(instance, store, "run-answer")?,
            shortcut: optional(instance, store, "shortcut")?,
        })
    }
}

/// The export `name`, which every component has.
fn required<Params, Results>(
    instance: &wasmtime::component::Instance,
    store: &mut Store<HostState>,
    name: &str,
) -> anyhow::Result<TypedFunc<Params, Results>>
where
    Params: ComponentNamedList + Lower,
    Results: ComponentNamedList + Lift,
{
    optional(instance, store, name)?.ok_or_else(|| anyhow!("`{name}` is not exported"))
}

/// The export `name`, where the component has it; one that is there with
/// another type than the interface gives it is an error.
fn optional<Params, Results>(
    instance: &wasmtime::component::Instance,
    store: &mut Store<HostState>,
    name: &str,
) -> anyhow::Result<Option<TypedFunc<Params, Results>>>
where
    Params: ComponentNamedList + Lower,
    Results: ComponentNamedList + Lift,
{
    let Some(func) = instance.get_func(&mut *store, name) else {
        return Ok(None);
    };
    func.typed(&*store)
        .map(Some)
        .map_err(describe)
        .with_context(|| format!("`{name}` does not match"))
}

/// A running extension.
pub struct Instance {
    store: Store<HostState>,
    exports: Exports,
}

impl Instance {
    pub fn render(&mut self, widget: &str) -> anyhow::Result<Vec<Node>> {
        self.store.set_fuel(FUEL_PER_CALL).map_err(describe)?;
        let widget = (widget.to_owned(),);
        match &self.exports.render {
            Render::Current(render) => render.call(&mut self.store, widget).map(|(nodes,)| nodes),
            Render::Legacy(render) => render
                .call(&mut self.store, widget)
                .map(|(nodes,)| nodes.into_iter().map(current).collect()),
        }
        .map_err(describe)
    }

    pub fn activate(&mut self, widget: &str, action: &str) -> anyhow::Result<()> {
        self.store.set_fuel(FUEL_PER_CALL).map_err(describe)?;
        let arguments = (widget.to_owned(), action.to_owned());
        self.exports
            .activate
            .call(&mut self.store, arguments)
            .map_err(describe)
    }

    /// Gives the extension the outcome of the request numbered `request`.
    pub fn fetched(
        &mut self,
        request: u32,
        outcome: Result<HttpResponse, String>,
    ) -> anyhow::Result<()> {
        let state = self.store.data_mut();
        state.under_way = state.under_way.saturating_sub(1);
        let outcome = outcome.map(|response| bindings::Response {
            status: response.status,
            body: response.body,
        });
        self.store.set_fuel(FUEL_PER_CALL).map_err(describe)?;
        self.exports
            .fetched
            .call(&mut self.store, (request, outcome))
            .map_err(describe)
    }

    /// Takes over `settings` as the extension's table of the
    /// configuration and tells the extension, where it listens; whether
    /// it does is returned.
    pub fn settings_changed(&mut self, settings: HashMap<String, String>) -> anyhow::Result<bool> {
        self.store.data_mut().access.settings = settings;
        let Some(changed) = &self.exports.settings_changed else {
            return Ok(false);
        };
        self.store.set_fuel(FUEL_PER_CALL).map_err(describe)?;
        changed.call(&mut self.store, ()).map_err(describe)?;
        Ok(true)
    }

    /// What the extension offers for `query`; nothing where it does not
    /// answer the launcher. Nothing it asks to do meanwhile is done.
    pub fn answer(&mut self, query: &str) -> anyhow::Result<Vec<Answer>> {
        let Some(answer) = &self.exports.answer else {
            return Ok(Vec::new());
        };
        self.store.set_fuel(FUEL_PER_CALL).map_err(describe)?;
        self.store.data_mut().answering = true;
        let answers = answer.call(&mut self.store, (query.to_owned(),));
        self.store.data_mut().answering = false;
        answers.map(|(answers,)| answers).map_err(describe)
    }

    /// Tells the extension that its answer `id` was chosen.
    pub fn run_answer(&mut self, id: &str) -> anyhow::Result<()> {
        let Some(run) = &self.exports.run_answer else {
            return Ok(());
        };
        self.store.set_fuel(FUEL_PER_CALL).map_err(describe)?;
        run.call(&mut self.store, (id.to_owned(),))
            .map_err(describe)
    }

    /// Tells the extension that the keys of its shortcut `id` were pressed.
    pub fn shortcut(&mut self, id: &str) -> anyhow::Result<()> {
        let Some(shortcut) = &self.exports.shortcut else {
            return Ok(());
        };
        self.store.set_fuel(FUEL_PER_CALL).map_err(describe)?;
        shortcut
            .call(&mut self.store, (id.to_owned(),))
            .map_err(describe)
    }

    /// Whether the component was built for interface 0.4 or later.
    #[cfg(test)]
    pub fn is_current(&self) -> bool {
        matches!(self.exports.render, Render::Current(_))
    }

    /// What the extension may reach.
    pub fn access(&self) -> &Access {
        &self.store.data().access
    }

    /// The requests the extension started since this was last called.
    pub fn take_requests(&mut self) -> Vec<NumberedRequest> {
        std::mem::take(&mut self.store.data_mut().accepted)
    }

    /// What the extension asked to be done since this was last called.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.store.data_mut().effects)
    }
}

/// A node of interface 0.3 as one of 0.4.
fn current(node: bindings::legacy::Node) -> Node {
    use bindings::legacy::NodeKind as Legacy;
    let kind = match node.kind {
        Legacy::Row => NodeKind::Row,
        Legacy::Column => NodeKind::Column,
        Legacy::Text(text) => NodeKind::Text(text),
        Legacy::Caption(text) => NodeKind::Caption(text),
        Legacy::Button(button) => NodeKind::Button(bindings::Button {
            label: button.label,
            action: button.action,
        }),
        Legacy::Progress(share) => NodeKind::Progress(share),
    };
    Node {
        kind,
        children: node.children,
    }
}

/// Whether `wasm` starts with the binary header of a component, as opposed
/// to that of a core module: the layer field after the magic is 1.
/// What the shell shows now, or why it is not known: the thread that
/// draws writes it down from the time a widget, shortcut or chosen answer
/// of an extension that may read it was first used, until the extensions
/// are loaded again.
fn shell_now() -> Result<system::ShellState, String> {
    system::shell().ok_or_else(|| {
        "what the shell shows is not known yet: it is told once a widget, shortcut \
         or chosen answer of the extension was used"
            .to_owned()
    })
}

fn is_component(wasm: &[u8]) -> bool {
    wasm.get(..4) == Some(b"\0asm") && wasm.get(6..8) == Some(&[1, 0])
}

/// Flattens an error of the WebAssembly tooling, with its causes.
fn describe(error: impl std::fmt::Debug + std::fmt::Display) -> anyhow::Error {
    anyhow!("{error:#}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_are_told_from_core_modules_by_header() {
        assert!(is_component(b"\0asm\x0d\0\x01\0"));
        assert!(!is_component(b"\0asm\x01\0\0\0"));
        assert!(!is_component(b"\0asm"));
        assert!(!is_component(b""));
    }

    #[test]
    fn only_so_much_happens_within_a_while() {
        let mut rate = Rate::new(2, Duration::from_secs(10));
        let start = Instant::now();
        assert!(rate.admit(start));
        assert!(rate.admit(start + Duration::from_secs(1)));
        assert!(!rate.admit(start + Duration::from_secs(2)));
        assert!(rate.admit(start + Duration::from_secs(10)));
        assert!(!rate.admit(start + Duration::from_secs(10)));
        assert!(rate.admit(start + Duration::from_secs(21)));
    }

    /// The `counter` example as the registry offered it for interface
    /// 0.3, a core module with its component type, kept as it was.
    const COUNTER_0_3: &[u8] = include_bytes!("testdata/counter-0.3.wasm");

    #[test]
    fn a_component_built_for_0_3_loads_and_renders_as_before() {
        let mut counter = Runtime::new()
            .unwrap()
            .instantiate("counter", COUNTER_0_3, Access::default())
            .unwrap();
        assert!(!counter.is_current());
        let nodes = counter.render("clicks").unwrap();
        assert!(matches!(nodes[0].kind, NodeKind::Row));
        assert!(matches!(&nodes[1].kind, NodeKind::Text(text) if text == "Clicks: 0"));
        assert!(matches!(&nodes[3].kind, NodeKind::Button(button) if button.action == "increment"));
        counter.activate("clicks", "increment").unwrap();
        let nodes = counter.render("clicks").unwrap();
        assert!(matches!(&nodes[1].kind, NodeKind::Text(text) if text == "Clicks: 1"));
        // What 0.4 adds is not there, and asking for it does nothing.
        assert!(!counter.settings_changed(HashMap::new()).unwrap());
        assert!(counter.answer("anything").unwrap().is_empty());
        counter.run_answer("x").unwrap();
        counter.shortcut("x").unwrap();
        assert!(counter.take_effects().is_empty());
    }

    /// Needs the example extension, built as the README describes.
    #[test]
    #[ignore = "requires extensions/system-monitor to be built first"]
    fn the_system_monitor_example_answers_with_the_exports_of_0_4() {
        let wasm = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/extensions/wasm32-unknown-unknown/release/tessera_example_system_monitor.wasm"
        );
        let wasm = std::fs::read(wasm).unwrap();
        let access = Access {
            permissions: toml::from_str("system = [\"meters\", \"popups\"]").unwrap(),
            popups: vec!["panel".to_owned()],
            ..Access::default()
        };
        let mut monitor = Runtime::new()
            .unwrap()
            .instantiate("system-monitor", &wasm, access)
            .unwrap();
        assert!(monitor.is_current());
        let nodes = monitor.render("meters").unwrap();
        assert!(matches!(&nodes[1].kind, NodeKind::Icon(_)));
        monitor.shortcut("panel").unwrap();
        assert_eq!(
            monitor.take_effects(),
            [Effect::OpenPopup("panel".to_owned())]
        );
        // The windows are not permitted, so nothing matches.
        assert!(monitor.answer("notepad").unwrap().is_empty());
        assert!(!monitor.answer("").unwrap().is_empty());
        assert!(monitor.settings_changed(HashMap::new()).unwrap());
    }

    /// Needs the example extension, built as the README describes.
    #[test]
    #[ignore = "requires extensions/counter to be built first"]
    fn the_counter_example_counts_button_presses() {
        let wasm = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/extensions/wasm32-unknown-unknown/release/tessera_example_counter.wasm"
        );
        let wasm = std::fs::read(wasm).unwrap();
        let mut counter = Runtime::new()
            .unwrap()
            .instantiate("counter", &wasm, Access::default())
            .unwrap();

        let text_of = |nodes: Vec<Node>| match &nodes[1].kind {
            NodeKind::Text(text) => text.clone(),
            other => panic!("expected text, got {other:?}"),
        };
        assert_eq!(text_of(counter.render("clicks").unwrap()), "Clicks: 0");
        counter.activate("clicks", "increment").unwrap();
        assert_eq!(text_of(counter.render("clicks").unwrap()), "Clicks: 1");
    }

    /// Needs the example extension, built as the README describes.
    #[test]
    #[ignore = "requires extensions/ai-limits to be built first"]
    fn the_ai_limits_example_asks_a_service_and_shows_its_answer() {
        let wasm = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/extensions/wasm32-unknown-unknown/release/tessera_example_ai_limits.wasm"
        );
        let wasm = std::fs::read(wasm).unwrap();
        let setting = |key: &str, value: &str| (key.to_owned(), value.to_owned());
        let access = Access {
            permissions: toml::from_str("network = [\"api.deepseek.com\"]").unwrap(),
            settings: [
                setting("providers", "deepseek, claude"),
                setting("deepseek_key", "key-1"),
            ]
            .into(),
            ..Access::default()
        };
        let mut limits = Runtime::new()
            .unwrap()
            .instantiate("ai-limits", &wasm, access)
            .unwrap();
        let texts = |nodes: Vec<Node>| -> Vec<String> {
            let text = |node: Node| match node.kind {
                NodeKind::Text(text) | NodeKind::Caption(text) => Some(text),
                _ => None,
            };
            nodes.into_iter().filter_map(text).collect()
        };

        // Claude's credentials are not permitted, so only one request starts.
        let asking = texts(limits.render("details").unwrap());
        assert_eq!(asking[..2], ["DeepSeek", "Asking…"]);
        assert!(asking[3].contains("does not permit"), "got {asking:?}");
        let requests = limits.take_requests();
        let [(number, request)] = requests.as_slice() else {
            panic!("expected one request, got {requests:?}");
        };
        assert_eq!(request.url, "https://api.deepseek.com/user/balance");
        let authorization = ("Authorization".to_owned(), "Bearer key-1".to_owned());
        assert!(request.headers.contains(&authorization));

        let body = r#"{"is_available": true,
            "balance_infos": [{"currency": "USD", "total_balance": "110.00"}]}"#;
        let answer = HttpResponse {
            status: 200,
            body: body.to_owned(),
        };
        limits.fetched(*number, Ok(answer)).unwrap();
        let shown = texts(limits.render("summary").unwrap());
        assert_eq!(shown, ["DeepSeek 110.00 USD", "Claude ?"]);
        assert!(limits.take_requests().is_empty());
    }

    /// Needs the example extension, built as the README describes.
    #[test]
    #[ignore = "requires extensions/device-battery to be built first"]
    fn the_device_battery_example_shows_the_charge_of_a_device() {
        let wasm = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/extensions/wasm32-unknown-unknown/release/tessera_example_device_battery.wasm"
        );
        let wasm = std::fs::read(wasm).unwrap();
        let device = |name: &str, connected, battery| crate::platform::PairedDevice {
            name: name.to_owned(),
            connected,
            battery,
            audio: None,
        };
        let access = |permitted: bool, device_setting: Option<&str>| Access {
            permissions: if permitted {
                toml::from_str("devices = [\"bluetooth\"]").unwrap()
            } else {
                Default::default()
            },
            settings: device_setting
                .map(|name| ("device".to_owned(), name.to_owned()))
                .into_iter()
                .collect(),
            bluetooth: Some(vec![
                device("Headphones", true, None),
                device("Mouse", true, Some(80)),
                device("Old Speaker", false, Some(15)),
            ]),
            ..Access::default()
        };
        let texts = |nodes: Vec<Node>| -> Vec<String> {
            let text = |node: Node| match node.kind {
                NodeKind::Text(text) | NodeKind::Caption(text) => Some(text),
                _ => None,
            };
            nodes.into_iter().filter_map(text).collect()
        };
        let runtime = Runtime::new().unwrap();
        let start = |access| {
            runtime
                .instantiate("device-battery", &wasm, access)
                .unwrap()
        };

        // Unless one is named, the first connected device with a charge.
        let mut any = start(access(true, None));
        let shown = any.render("charge").unwrap();
        assert!(
            shown
                .iter()
                .any(|node| matches!(node.kind, NodeKind::Progress(level) if level == 0.8))
        );
        assert_eq!(texts(shown), ["Mouse", "80%"]);
        let all = texts(any.render("all").unwrap());
        assert_eq!(
            all,
            [
                "Headphones",
                "Connected",
                "Mouse",
                "Connected, 80%",
                "Old Speaker",
                "Not connected, was 15%"
            ]
        );

        let mut named = start(access(true, Some("speak")));
        assert_eq!(
            texts(named.render("charge").unwrap()),
            ["Old Speaker", "was 15%"]
        );
        let mut unknown = start(access(true, Some("keyboard")));
        let shown = texts(unknown.render("charge").unwrap());
        assert_eq!(shown, ["No paired device is called `keyboard`"]);

        let mut refused = start(access(false, None));
        let shown = texts(refused.render("charge").unwrap());
        assert!(shown[0].contains("does not permit"), "got {shown:?}");
    }

    #[test]
    fn bytes_that_are_not_wasm_are_rejected() {
        let runtime = Runtime::new().unwrap();
        assert!(
            runtime
                .instantiate("probe", b"not wasm", Access::default())
                .is_err()
        );
    }
}
