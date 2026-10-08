// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

fn main() {
    // Headless element queries need static metadata, not an inspection server.
    // Keep that metadata out of consumer release builds.
    let config = slint_build::CompilerConfiguration::new()
        // The versioned native-grid metric adapter owns Fluent's input geometry.
        .with_style("fluent".into())
        .with_debug_info(cfg!(debug_assertions));
    slint_build::compile_with_config("ui/panel.slint", config).unwrap();
}
