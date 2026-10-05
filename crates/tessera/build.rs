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
    // Both launchers keep the same privilege/DPI contract. Embed before signing.
    for binary in ["tessera", "tessera-desktop"] {
        println!("cargo:rustc-link-arg-bin={binary}=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin={binary}=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
    // Linux cross-checks validate Rust types, not Windows SDK resource compilation.
    if env::var("HOST").is_ok_and(|host| host.contains("windows")) {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set("ProductName", "Tessera")
            .set("FileDescription", "Tessera native desktop alpha")
            .set("ProductVersion", env!("CARGO_PKG_VERSION"))
            .set("LegalCopyright", "Copyright (C) 2026 Tessera contributors")
            .set_version_info(winresource::VersionInfo::FILEFLAGS, 0x2);
        resource
            .compile()
            .expect("Compile Windows version resources");
    }
}
