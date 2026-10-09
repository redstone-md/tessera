// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Independent pinned-source typography for exact native raster comparisons.
//! No production component, palette, or text-style import is used here.

// Seelen v2.8.8 shared/index.scss:107 and reset.css:12 establish 12.8px/1.4.
// Bluetooth headings/names use 600/0.5px and 500/0px respectively;
// keyboard-selector.scss:55–57 and buttons.scss:5 use centered 600/0px.
slint::slint! {
    export component SourceTypographyText inherits Window {
        no-frame: true;
        in property <color> source-background;
        in property <color> source-foreground;
        in property <string> caption;
        in property <int> source-weight;
        in property <length> source-tracking;
        in property <bool> centered;
        in property <length> text-x;
        in property <length> text-y;
        in property <length> text-width;
        // Existing Keyboard/Bluetooth source layout remains the default.
        in property <length> text-height: 17.92px;
        in property <bool> vertically-centered: false;
        in property <bool> native-line-height: false;
        background: root.source-background;
        probe := Text {
            width: 0px; height: 0px; opacity: 0;
            text: "M"; font-size: 12.8px;
        }
        Text {
            x: root.text-x; y: root.text-y;
            width: root.text-width; height: root.text-height;
            text: root.caption;
            color: root.source-foreground;
            font-size: 12.8px;
            font-weight: root.source-weight;
            letter-spacing: root.source-tracking;
            horizontal-alignment: root.centered ? center : left;
            vertical-alignment: root.vertically-centered ? center : top;
            line-height-factor: root.native-line-height ? 1 : 17.92px / probe.preferred-height;
            overflow: elide;
        }
    }
}
