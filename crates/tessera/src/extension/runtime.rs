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
        eprintln!("tessera: [{}] {message}", self.id);
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
