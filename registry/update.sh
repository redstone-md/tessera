#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
# Copyright (C) 2026 Tessera contributors.
#
# Builds the example extensions and writes the registry that offers them
# and the themes of themes/: registry.toml and the files it lists. Run it
# after changing an example or a theme; a changed theme needs a new version
# here to be offered as an update.
set -eu
cd "$(dirname "$0")/.."

offer() {
    id=$1 name=$2 description=$3
    source=extensions/$id
    crate=tessera_example_$(printf %s "$id" | tr - _)
    cargo build --release --target wasm32-unknown-unknown \
        --manifest-path "$source/Cargo.toml" --target-dir target/extensions
    mkdir -p "registry/$id"
    cp "target/extensions/wasm32-unknown-unknown/release/$crate.wasm" "registry/$id/extension.wasm"
    cp "$source/extension.toml" "registry/$id/extension.toml"
    version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$source/Cargo.toml" | head -n 1)
    cat <<END

[[extension]]
id = "$id"
name = "$name"
description = "$description"
version = "$version"
manifest = "$id/extension.toml"
manifest_sha256 = "$(sha256sum "registry/$id/extension.toml" | cut -d ' ' -f 1)"
wasm = "$id/extension.wasm"
wasm_sha256 = "$(sha256sum "registry/$id/extension.wasm" | cut -d ' ' -f 1)"
END
}

theme() {
    id=$1 name=$2 version=$3 description=$4
    mkdir -p registry/themes
    tr -d '\r' < "themes/$id.toml" > "registry/themes/$id.toml"
    cat <<END

[[theme]]
id = "$id"
name = "$name"
description = "$description"
version = "$version"
file = "themes/$id.toml"
sha256 = "$(sha256sum "registry/themes/$id.toml" | cut -d ' ' -f 1)"
END
}

{
    cat <<END
# The extensions and themes Tessera offers to install unless its
# configuration names other registries. Written by update.sh; see the README for the format.
name = "Tessera"
END
    offer counter "Counter" \
        "An example: counts presses of its buttons, in a bar and in a popup of its own."
    offer ai-limits "AI limits" \
        "What is left of the usage limits of Claude, Codex, OpenRouter and DeepSeek, and when they reset."
    offer device-battery "Device battery"         "An example: the charge of a Bluetooth device in the bar, and every paired device in a popup."
    offer weather "Weather"         "The weather now and on the next days at a place named in the configuration, from Open-Meteo."
    theme void "Void" 1.0.0 \
        "Flat bars without fields behind the widgets, small and quiet, after the desktops of tiling window managers."
    theme expressive "Expressive" 1.0.0 \
        "Larger, rounder and livelier than what is built in."
    theme compact "Compact" 1.0.0 \
        "Small, square and calm; nothing overshoots."
} > registry/registry.toml.new
mv registry/registry.toml.new registry/registry.toml
