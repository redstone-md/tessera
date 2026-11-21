// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Runs extension code in a WebAssembly sandbox.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, anyhow};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

#[allow(unsafe_code)]
mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "extension",
    });
}

pub use bindings::{Node, NodeKind};

use super::access::Access;
use super::http::{HttpRequest, HttpResponse};

/// Fuel granted to one call into an extension; a call that runs out traps.
const FUEL_PER_CALL: u64 = 100_000_000;

/// The most linear memory one extension may hold.
const MEMORY_LIMIT: usize = 64 << 20;

/// The most requests of one extension that may be under way at a time.
const REQUEST_LIMIT: usize = 4;

/// A request an extension started, and the number it knows it by.
pub type NumberedRequest = (u32, HttpRequest);

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
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|state| &mut state.limits);
        store.set_fuel(FUEL_PER_CALL).map_err(describe)?;
        let bindings = bindings::Extension::instantiate(&mut store, &component, &self.linker)
            .map_err(describe)
            .context("the extension does not match the Tessera extension interface")?;
        Ok(Instance { store, bindings })
    }
}

/// A running extension.
pub struct Instance {
    store: Store<HostState>,
    bindings: bindings::Extension,
}

impl Instance {
    pub fn render(&mut self, widget: &str) -> anyhow::Result<Vec<Node>> {
        self.store.set_fuel(FUEL_PER_CALL).map_err(describe)?;
        self.bindings
            .call_render(&mut self.store, widget)
            .map_err(describe)
    }

    pub fn activate(&mut self, widget: &str, action: &str) -> anyhow::Result<()> {
        self.store.set_fuel(FUEL_PER_CALL).map_err(describe)?;
        self.bindings
            .call_activate(&mut self.store, widget, action)
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
        self.bindings
            .call_fetched(
                &mut self.store,
                request,
                outcome.as_ref().map_err(String::as_str),
            )
            .map_err(describe)
    }

    /// The requests the extension started since this was last called.
    pub fn take_requests(&mut self) -> Vec<NumberedRequest> {
        std::mem::take(&mut self.store.data_mut().accepted)
    }
}

/// Whether `wasm` starts with the binary header of a component, as opposed
/// to that of a core module: the layer field after the magic is 1.
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
            state_file: None,
            bluetooth: None,
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
            state_file: None,
            bluetooth: Some(vec![
                device("Headphones", true, None),
                device("Mouse", true, Some(80)),
                device("Old Speaker", false, Some(15)),
            ]),
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
