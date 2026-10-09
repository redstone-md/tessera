// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

fn main() {
    // Headless element queries need static metadata, not an inspection server.
    // Keep that metadata out of consumer release builds.
    // Inline slint! test fixtures have their own compiler configuration.
    if cfg!(debug_assertions) {
        println!("cargo:rustc-env=SLINT_EMIT_DEBUG_INFO=1");
    }
    let config = slint_build::CompilerConfiguration::new()
        // The versioned native-grid metric adapter owns Fluent's input geometry.
        .with_style("fluent".into())
        .with_debug_info(cfg!(debug_assertions));
    slint_build::compile_with_config("ui/panel.slint", config).unwrap();
}
