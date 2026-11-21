// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Has the linker put the application manifest into the program.

fn main() {
    println!("cargo:rerun-if-changed=tessera.manifest");
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tessera.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
