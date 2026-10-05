// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

#![forbid(unsafe_code)]

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    assert_eq!(
        env::var("CARGO_CFG_TARGET_ENV").as_deref(),
        Ok("msvc"),
        "Tessera currently supports Windows builds with the MSVC toolchain"
    );
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo package directory"))
            .join("app.manifest");
    // Embed before signing; no elevation, UIAccess bypass, or external manifest.
    println!("cargo:rustc-link-arg-bin=tessera=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=tessera=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
