// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Runs extension code in a WebAssembly sandbox.

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

/// Fuel granted to one call into an extension; a call that runs out traps.
const FUEL_PER_CALL: u64 = 100_000_000;

/// The most linear memory one extension may hold.
const MEMORY_LIMIT: usize = 64 << 20;

/// What the host functions of one extension can reach.
struct HostState {
    id: String,
    limits: StoreLimits,
}

impl bindings::ExtensionImports for HostState {
    fn log(&mut self, message: String) {
        eprintln!("tessera: [{}] {message}", self.id);
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
    pub fn instantiate(&self, id: &str, wasm: &[u8]) -> anyhow::Result<Instance> {
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
            .instantiate("counter", &wasm)
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
        assert!(runtime.instantiate("probe", b"not wasm").is_err());
    }
}
