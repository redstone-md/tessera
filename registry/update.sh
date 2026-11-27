#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
# Copyright (C) 2026 Tessera contributors.
#
# Builds the example extensions and writes the registry that offers them,
# the themes of themes/, the icon packs of icon-packs/, some pictures of
# wallpapers/ and the tiling layouts that come with Tessera:
# registry.toml and the files it lists, with the pictures preview.py
# drew. Run preview.py first, then this, after changing an example, a
# theme, an icon pack or a layout; a changed theme, icon pack or wallpaper
# needs a new version here to be offered as an update.
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
    preview "$id/icon.png"
}

# The lines that name the picture of an offer, when preview.py drew one.
preview() {
    if [ -f "registry/$1" ]; then
        printf 'preview = "%s"\npreview_sha256 = "%s"\n' "$1" \
            "$(sha256sum "registry/$1" | cut -d ' ' -f 1)"
    fi
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
    preview "previews/$id.jpg"
}

# An icon pack of registry/icon-packs/<id>/: its manifest and every
# other file in its directory, each with its checksum.
offer_icon_pack() {
    id=$1 name=$2 version=$3 description=$4
    pack=registry/icon-packs/$id
    cat <<END

[[icon_pack]]
id = "$id"
name = "$name"
description = "$description"
version = "$version"
manifest = "icon-packs/$id/icon-pack.toml"
manifest_sha256 = "$(sha256sum "$pack/icon-pack.toml" | cut -d ' ' -f 1)"
files = [
END
    (cd "$pack" && find . -type f ! -name icon-pack.toml | sed 's|^\./||' | LC_ALL=C sort) |
        while read -r file; do
            printf '    { path = "%s", sha256 = "%s" },\n' "$file" \
                "$(sha256sum "$pack/$file" | cut -d ' ' -f 1)"
        done
    echo "]"
    preview "icon-packs/$id.png"
}

# A picture of registry/wallpapers/, shown as its own picture.
offer_wallpaper() {
    id=$1 name=$2 version=$3 description=$4
    cat <<END

[[wallpaper]]
id = "$id"
name = "$name"
description = "$description"
version = "$version"
file = "wallpapers/$id.jpg"
sha256 = "$(sha256sum "registry/wallpapers/$id.jpg" | cut -d ' ' -f 1)"
END
    preview "wallpapers/$id.jpg"
}

# Every tiling layout that comes with Tessera, copied to
# registry/tiling-layouts/, by its file name, with the `name` and
# `description` it gives itself, if any; none while there are none.
offer_layouts() {
    version=$1
    for source in crates/tessera/src/tiling/layouts/*.toml; do
        [ -f "$source" ] || continue
        id=$(basename "$source" .toml)
        mkdir -p registry/tiling-layouts
        tr -d '\r' < "$source" > "registry/tiling-layouts/$id.toml"
        name=$(sed -n 's/^name = "\(.*\)"/\1/p' "$source" | head -n 1)
        description=$(sed -n 's/^description = "\(.*\)"/\1/p' "$source" | head -n 1)
        cat <<END

[[tiling_layout]]
id = "$id"
name = "${name:-$id}"
description = "${description:-A tiling layout that comes with Tessera.}"
version = "$version"
file = "tiling-layouts/$id.toml"
sha256 = "$(sha256sum "registry/tiling-layouts/$id.toml" | cut -d ' ' -f 1)"
END
    done
}

{
    cat <<END
# The extensions, themes, icon packs, wallpapers and tiling layouts
# Tessera offers to install unless its configuration names other
# registries. Written by update.sh; see the README for the format.
name = "Tessera"
END
    offer counter "Counter" \
        "An example: counts presses of its buttons, in a bar and in a popup of its own."
    offer ai-limits "AI limits" \
        "What is left of the usage limits of Claude, Codex, OpenRouter and DeepSeek, and when they reset."
    offer device-battery "Device battery"         "An example: the charge of a Bluetooth device in the bar, and every paired device in a popup."
    offer weather "Weather"         "The weather now and on the next days at a place named in the configuration, from Open-Meteo."
    offer pomodoro "Pomodoro" \
        "A focus timer of work and breaks for the bar, counting the rounds done today; it keeps time across restarts."
    offer crypto "Crypto prices" \
        "Prices of coins and their change over a day, from CoinGecko."
    offer exchange "Exchange rates" \
        "Currency exchange rates against a base currency, from ExchangeRate-API, updated daily."
    offer hacker-news "Hacker News" \
        "The top stories of Hacker News with their points and comments."
    offer moon "Moon" \
        "The phase of the moon, how much of it is lit and when it is next full or new, worked out offline."
    offer countdown "Countdown" \
        "The time left until named dates of the configuration, the nearest one in the bar."
    offer world-clock "World clock" \
        "The time in other places, at fixed offsets from UTC named in the configuration."
    offer habits "Habits" \
        "Daily habits to check off in a popup, with streaks kept across restarts."
    offer github-stats "GitHub stats" \
        "Stars, forks and open issues of GitHub repositories named in the configuration."
    offer air-quality "Air quality" \
        "The European Air Quality Index, particles and ozone at a place named in the configuration, from Open-Meteo."
    offer dice "Dice" \
        "Dice, a coin and answers to questions asked in silence, offline."
    offer breaks "Eye breaks" \
        "Reminders to look away from the screen for a moment, every twenty minutes unless set otherwise."
    offer system-monitor "System monitor" "An example of the interface 0.4: the processor and the memory in the bar; the network, the battery, what plays and the open windows in a popup; sys in the launcher."
    theme void "Void" 1.1.0 \
        "Flat bars without fields behind the widgets, small and quiet, after the desktops of tiling window managers."
    theme expressive "Expressive" 1.1.0 \
        "Larger, rounder and livelier than what is built in."
    theme compact "Compact" 1.1.0 \
        "Small, square and calm; nothing overshoots."
    theme aurora "Aurora" 1.0.0 \
        "Night teal and violet, after northern lights over a frozen fjord."
    theme sakura "Sakura" 1.1.0 \
        "Soft blossom pinks on cream, light and round, with a gentle bounce."
    theme synthwave "Synthwave" 1.0.0 \
        "Neon pink and electric cyan on a deep purple night."
    theme ember "Ember" 1.1.0 \
        "Warm charcoal with glowing orange, like coals after the fire."
    theme glacier "Glacier" 1.1.0 \
        "Crisp ice blues on white; light, tidy and calm."
    theme forest "Forest" 1.1.0 \
        "Deep moss greens and misty light from an old-growth wood."
    theme paper "Paper" 1.1.0 \
        "Ink on warm paper. Flat bars, square-ish shapes, no fuss."
    theme oled "OLED" 1.0.0 \
        "True black and white for OLED panels: flat, small and still."
    theme ocean "Ocean" 1.1.0 \
        "Navy depths and aqua light from beneath the waves."
    theme lilac "Lilac" 1.1.0 \
        "Lavender fields at golden hour, pastel and airy."
    theme terminal "Terminal" 1.0.0 \
        "Phosphor green on black in a monospaced font; for those who live in a shell."
    theme sunset "Sunset" 1.0.0 \
        "Dusk over the dunes: burning orange under a purple sky."
    theme mocha "Mocha" 1.0.0 \
        "Coffee browns and caramel; cosy and easy on the eyes."
    theme citrus "Citrus" 1.0.0 \
        "Lemon and lime on a sunny cream; playful and bright."
    theme nebula "Nebula" 1.1.0 \
        "Magenta and indigo clouds of a distant nebula."
    theme sandstone "Sandstone" 1.0.0 \
        "Terracotta and sand from a sunlit canyon; warm and light."
    theme contrast "High Contrast" 1.0.0 \
        "Black and signal yellow at the highest contrast, for clear reading."
    theme arcade "Arcade" 1.0.0 \
        "Square pixels, red neon and teal: an evening at the arcade."
    theme dense "Dense" 1.0.0 \
        "A layer: less space between and inside things and smaller bars and dock, over any theme."
    theme round "Round" 1.0.0 \
        "A layer: rounder corners over any theme, and nothing else."
    offer_icon_pack line "Line" 1.0.0 \
        "Thin line icons in the colors of the theme for common programs and kinds of files."
    offer_wallpaper aurora "Aurora" 1.0.0 \
        "Northern lights over a frozen fjord."
    offer_wallpaper forest "Forest" 1.0.0 \
        "Misty light in an old-growth wood."
    offer_wallpaper ocean "Ocean" 1.0.0 \
        "Light from beneath the waves."
    offer_wallpaper sunset "Sunset" 1.0.0 \
        "Dusk over the dunes."
    offer_wallpaper glacier "Glacier" 1.0.0 \
        "Ice blues on a clear day."
    offer_wallpaper nebula "Nebula" 1.0.0 \
        "Clouds of a distant nebula."
    offer_layouts 1.0.0
} > registry/registry.toml.new
mv registry/registry.toml.new registry/registry.toml
