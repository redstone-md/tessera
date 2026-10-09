// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use slint::ComponentHandle;
use slint::platform::WindowEvent;

slint::slint! {
    export component PowerSourceTextMeasure inherits Window {
        in property <length> source-font-size;
        in property <string> source-font-family;
        in property <float> source-line-height;
        in property <length> source-span-width;
        in property <string> source-text: "There's pending system updates. Install now?";
        out property <length> wrapped-height: prompt.preferred-height;
        out property <length> unwrapped-width: prompt.preferred-width;
        out property <length> single-line-height: line.preferred-height;
        property <float> line-factor: natural.preferred-height > 0px
            ? root.source-font-size * root.source-line-height / natural.preferred-height : 1;
        natural := Text {
            width: 0px;
            height: 0px;
            text: "M";
            font-size: root.source-font-size;
            font-family: root.source-font-family;
            accessible-role: none;
        }
        line := Text {
            width: 0px;
            height: 0px;
            text: "M";
            font-size: root.source-font-size;
            font-family: root.source-font-family;
            line-height-factor: root.line-factor;
            accessible-role: none;
        }
        prompt := Text {
            width: root.source-span-width;
            height: 0px;
            text: root.source-text;
            font-size: root.source-font-size;
            font-family: root.source-font-family;
            line-height-factor: root.line-factor;
            wrap: word-wrap;
            accessible-role: none;
        }
    }
}

pub(crate) struct PendingTextMeasure {
    pub(crate) row_height: f32,
    pub(crate) text_height: f32,
    pub(crate) single_line_height: f32,
    pub(crate) unwrapped_width: f32,
    pub(crate) span_width: f32,
}

// The pinned source uses a natural-height flex row: 348px minus a 36px
// switch and 20px gap. Only the switch is fixed at 18px. Measure a separate
// ComplexText tree, never the Power surface's own row/probe/height getters.
pub(crate) fn measure_pending_text(
    font_size: f32,
    font_family: slint::SharedString,
    line_height: f32,
    metric_scale: f32,
    window_scale: f32,
) -> PendingTextMeasure {
    let span_width = 348.0 - 36.0 - 20.0;
    let measure = source_text_measure(
        span_width,
        font_size,
        font_family,
        line_height,
        metric_scale,
        window_scale,
    );
    let text_height = measure.get_wrapped_height();
    PendingTextMeasure {
        row_height: text_height.max(18.0 * metric_scale),
        text_height,
        single_line_height: measure.get_single_line_height(),
        unwrapped_width: measure.get_unwrapped_width(),
        span_width: span_width * metric_scale,
    }
}

// Dimensions/font size are unscaled source CSS pixels; this applies the same
// metric and root-window DPI inputs as production before native text shaping.
pub(crate) fn measure_source_text(
    text: &str,
    span_width: f32,
    font_size: f32,
    font_family: slint::SharedString,
    line_height: f32,
    metric_scale: f32,
    window_scale: f32,
) -> f32 {
    let measure = source_text_measure(
        span_width,
        font_size,
        font_family,
        line_height,
        metric_scale,
        window_scale,
    );
    measure.set_source_text(text.into());
    measure.get_wrapped_height()
}

fn source_text_measure(
    span_width: f32,
    font_size: f32,
    font_family: slint::SharedString,
    line_height: f32,
    metric_scale: f32,
    window_scale: f32,
) -> PowerSourceTextMeasure {
    let measure = PowerSourceTextMeasure::new().unwrap();
    measure
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: window_scale,
        });
    measure.set_source_font_size(font_size * metric_scale);
    measure.set_source_font_family(font_family);
    measure.set_source_line_height(line_height);
    measure.set_source_span_width(span_width * metric_scale);
    measure
}
