// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Generated-component input, semantic accessibility, and real software pixels.
//! No native host, discovery, mutation, or source-string assertions are involved.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, Rgb8Pixel};

use crate::generated::{
    BluetoothDeviceRow, BluetoothMenu, BluetoothRadioRow, BluetoothRadioStatus,
    BluetoothTransportKind,
};
use crate::native_typography_oracle::SourceTypographyText;
use crate::sanitize::bounded_text;
use crate::theme::{PresentationTheme, ThemedComponent};

#[derive(Debug, PartialEq)]
enum Request {
    Refresh,
    Hide,
}

struct Fixture {
    window: Rc<MinimalSoftwareWindow>,
    source_window: Rc<MinimalSoftwareWindow>,
    popup: BluetoothMenu,
    requests: Rc<RefCell<Vec<Request>>>,
}

impl Fixture {
    fn new(configure: impl FnOnce(&BluetoothMenu)) -> Self {
        struct TestPlatform {
            popup: Rc<MinimalSoftwareWindow>,
            source: Rc<MinimalSoftwareWindow>,
            popup_created: Cell<bool>,
        }
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(if self.popup_created.replace(true) {
                    self.source.clone()
                } else {
                    self.popup.clone()
                })
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        let source_window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform {
            popup: window.clone(),
            source: source_window.clone(),
            popup_created: Cell::new(false),
        }))
        .unwrap();
        let popup = BluetoothMenu::new().unwrap();
        popup.apply_presentation_theme(PresentationTheme::uniform(
            slint::language::ColorScheme::Light,
        ));
        popup.set_refresh_enabled(true);
        let requests = Rc::new(RefCell::new(Vec::new()));
        let recorded = requests.clone();
        popup.on_refresh_requested(move || recorded.borrow_mut().push(Request::Refresh));
        let recorded = requests.clone();
        popup.on_hide_requested(move || recorded.borrow_mut().push(Request::Hide));
        configure(&popup);
        popup.show().unwrap();
        window
            .window()
            .dispatch_event(WindowEvent::WindowActiveChanged(true));
        let fixture = Self {
            window,
            source_window,
            popup,
            requests,
        };
        fixture.render_fit(1.0);
        fixture
    }

    fn elements(&self, label: &str) -> Vec<ElementHandle> {
        ElementHandle::find_by_accessible_label(&self.popup, label)
            .filter(|element| matches!(element.accessible_role(), Some(role) if role != AccessibleRole::None))
            .collect()
    }
    fn element(&self, label: &str) -> ElementHandle {
        let mut elements = self.elements(label).into_iter();
        let element = elements
            .next()
            .unwrap_or_else(|| panic!("missing semantic label {label}"));
        assert!(
            elements.next().is_none(),
            "duplicate semantic label {label}"
        );
        element
    }
    fn buttons(&self) -> Vec<ElementHandle> {
        ElementQuery::from_root(&self.popup)
            .match_accessible_role(AccessibleRole::Button)
            .find_all()
    }
    fn render_fit(&self, scale: f32) -> Vec<Rgb8Pixel> {
        self.window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        self.render(
            (self.popup.get_popup_content_width() * scale).ceil() as u32,
            (self.popup.get_popup_content_height() * scale).ceil() as u32,
        )
    }
    fn render(&self, width: u32, height: u32) -> Vec<Rgb8Pixel> {
        self.window.set_size(PhysicalSize::new(width, height));
        self.window.request_redraw();
        let mut pixels = vec![Rgb8Pixel::default(); (width * height) as usize];
        assert!(self.window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, width as usize);
        }));
        pixels
    }
    fn key(&self, key: Key) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }
    fn click(&self, label: &str) {
        let element = self.element(label);
        let origin = element.absolute_position();
        let size = element.size();
        assert!(size.width > 0.0 && size.height > 0.0);
        let position =
            LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        self.window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        self.window
            .window()
            .dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
        self.window
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
    }
    fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }
}

fn model<T: Clone + 'static>(rows: Vec<T>) -> slint::ModelRc<T> {
    slint::ModelRc::new(slint::VecModel::from(rows))
}
fn row(name: &str, transport: BluetoothTransportKind) -> BluetoothDeviceRow {
    BluetoothDeviceRow {
        name: bounded_text(name, 128).into(),
        transport,
    }
}
fn observed(root: &BluetoothMenu) {
    root.set_has_snapshot(true);
    root.set_radios(model(vec![BluetoothRadioRow {
        name: "Native Bluetooth radio".into(),
        state: BluetoothRadioStatus::On,
    }]));
    root.set_connected(model(vec![row("Headset", BluetoothTransportKind::Classic)]));
    root.set_paired(model(vec![row(
        "Keyboard",
        BluetoothTransportKind::Classic,
    )]));
    root.set_unknown(model(vec![row(
        "Sensor",
        BluetoothTransportKind::LowEnergy,
    )]));
}

#[test]
fn bluetooth_source_heading_and_name_native_glyphs_match_both_themes_and_three_scales() {
    let long_name = bounded_text(&format!("Clavier אוזניות {}", "界".repeat(180)), 128);
    let fixture = Fixture::new(|root| {
        observed(root);
        root.set_paired(model(vec![
            row("Keyboard", BluetoothTransportKind::Classic),
            row(&long_name, BluetoothTransportKind::LowEnergy),
        ]));
    });
    let source = SourceTypographyText::new().unwrap();
    source.show().unwrap();
    for scheme in [
        slint::language::ColorScheme::Light,
        slint::language::ColorScheme::Dark,
    ] {
        fixture
            .popup
            .apply_presentation_theme(PresentationTheme::uniform(scheme));
        // Independent pinned source neutrals; never read the tested palette.
        let (background, foreground, secondary) = match scheme {
            slint::language::ColorScheme::Dark => (0x181818, 0xe4e4e4, 0xbababa),
            _ => (0xf2f2f2, 0x121212, 0x3d3d3d),
        };
        source.set_source_background(slint::Color::from_rgb_u8(
            (background >> 16) as u8,
            (background >> 8) as u8,
            background as u8,
        ));
        for scale in [1.0_f32, 1.5, 2.0] {
            let actual = fixture.render_fit(scale);
            let window_size = fixture.window.window().size();
            let width = window_size.width as usize;
            let height = window_size.height as usize;
            fixture
                .source_window
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
            fixture.source_window.set_size(window_size);
            for (label, caption, heading) in [
                ("Connected", "CONNECTED", true),
                ("Paired", "PAIRED", true),
                ("Connection unknown", "CONNECTION UNKNOWN", true),
                ("Headset", "Headset", false),
                ("Keyboard", "Keyboard", false),
                ("Sensor", "Sensor", false),
                (long_name.as_str(), long_name.as_str(), false),
            ] {
                // The semantic caption remains the untranslated-case @tr result,
                // separate from the visible uppercase heading. Device AX names
                // remain complete even when their visible text is elided.
                let node = fixture.element(label);
                assert_eq!(node.accessible_role(), Some(AccessibleRole::Text));
                if heading {
                    assert!(fixture.elements(caption).is_empty());
                }
                let origin = node.absolute_position();
                let size = node.size();
                assert!(origin.x >= 0.0 && origin.y >= 0.0);
                assert!(size.width > 8.0 && size.height >= 17.92 - 0.02);
                assert!((origin.x + size.width) * scale <= window_size.width as f32 + 1.0);
                assert!((origin.y + size.height) * scale <= window_size.height as f32 + 1.0);
                let inset = if heading { 0.0 } else { 4.0 };
                let x = origin.x + inset;
                let y = origin.y + inset;
                let text_width = size.width - 2.0 * inset;
                source.set_text_x(x);
                source.set_text_y(y);
                source.set_text_width(text_width);
                source.set_source_weight(if heading { 600 } else { 500 });
                source.set_source_tracking(if heading { 0.5 } else { 0.0 });
                source.set_caption(caption.into());
                let ink = if heading { secondary } else { foreground };
                source.set_source_foreground(slint::Color::from_rgb_u8(
                    (ink >> 16) as u8,
                    (ink >> 8) as u8,
                    ink as u8,
                ));
                fixture.source_window.request_redraw();
                let mut reference = vec![Rgb8Pixel::default(); width * height];
                assert!(fixture.source_window.draw_if_needed(|renderer| {
                    renderer.render(&mut reference, width);
                }));
                let mut glyph_pixels = 0;
                for py in (y * scale).ceil() as usize..((y + 17.92) * scale).floor() as usize {
                    for px in
                        (x * scale).ceil() as usize..((x + text_width) * scale).floor() as usize
                    {
                        let offset = py * width + px;
                        let expected = reference[offset];
                        glyph_pixels += usize::from(
                            [expected.r, expected.g, expected.b]
                                != [
                                    (background >> 16) as u8,
                                    (background >> 8) as u8,
                                    background as u8,
                                ],
                        );
                        assert_eq!(
                            actual[offset], expected,
                            "independent source glyph {label:?}, heading={heading}, \
                             scheme={scheme:?}, scale={scale}, pixel=({px},{py})"
                        );
                    }
                }
                assert!(glyph_pixels > 8, "{label:?} needs visible native glyphs");
            }
            assert_eq!(
                fixture.buttons().len(),
                1,
                "only genuine Refresh is a button"
            );
            assert!(fixture.take_requests().is_empty());
        }
    }
}

#[test]
fn bluetooth_generated_real_pointer_keyboard_accessibility_refresh_and_escape() {
    let fixture = Fixture::new(observed);
    assert!(
        fixture.take_requests().is_empty(),
        "show/layout never refreshes by itself"
    );
    assert_eq!(
        fixture.buttons().len(),
        1,
        "no effects masquerading as controls"
    );
    let refresh = fixture.element("Refresh Bluetooth");
    assert_eq!(refresh.accessible_role(), Some(AccessibleRole::Button));
    assert_eq!(refresh.accessible_enabled(), Some(true));
    fixture.click("Refresh Bluetooth");
    assert_eq!(fixture.take_requests(), vec![Request::Refresh]);
    fixture.popup.invoke_focus_content();
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    fixture.key(Key::Space);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Refresh, Request::Refresh]
    );
    refresh.invoke_accessible_default_action();
    assert_eq!(fixture.take_requests(), vec![Request::Refresh]);
    fixture.key(Key::Escape);
    assert_eq!(fixture.take_requests(), vec![Request::Hide]);
    fixture.element("Connected");
    fixture.element("Paired");
    fixture.element("Connection unknown");
    for (label, description) in [
        ("Headset", "Connected, Classic"),
        ("Keyboard", "Disconnected, Classic"),
        ("Sensor", "Connection unknown, Low Energy"),
    ] {
        let device = fixture.element(label);
        assert_eq!(device.accessible_role(), Some(AccessibleRole::Text));
        assert_eq!(
            device.accessible_description().as_deref(),
            Some(description)
        );
        device.invoke_accessible_default_action();
    }
    assert!(
        fixture.take_requests().is_empty(),
        "device text has no selection or effects"
    );
}

#[test]
fn bluetooth_generated_off_disabled_unknown_radios_and_zero_exposed_radios_are_honest() {
    let fixture = Fixture::new(observed);
    for (state, label) in [
        (BluetoothRadioStatus::Off, "Off"),
        (BluetoothRadioStatus::Disabled, "Disabled"),
        (BluetoothRadioStatus::Unknown, "Unknown"),
    ] {
        fixture.popup.set_radios(model(vec![BluetoothRadioRow {
            name: "Observed radio".into(),
            state,
        }]));
        let pixels = fixture.render_fit(1.0);
        assert!(
            pixels
                .iter()
                .any(|pixel| pixel.r != pixel.g || pixel.r != 0)
        );
        assert_eq!(
            fixture
                .element("Observed radio")
                .accessible_description()
                .unwrap(),
            format!("Bluetooth radio, {label}")
        );
        assert_eq!(fixture.buttons().len(), 1);
        fixture.element("Headset");
        fixture.element("Sensor");
        assert!(fixture.elements("No Bluetooth radios exposed").is_empty());
    }
    fixture.popup.set_radios(model(vec![]));
    fixture.popup.set_no_radios(true);
    fixture.render_fit(2.0);
    fixture.element("No Bluetooth radios exposed");
    fixture.element("Sensor");
    assert!(
        fixture.elements("No Bluetooth adapter").is_empty(),
        "SDK exposure is not physical absence"
    );
    assert!(
        ElementQuery::from_root(&fixture.popup)
            .match_accessible_role(AccessibleRole::Checkbox)
            .find_all()
            .is_empty()
    );
    assert!(fixture.take_requests().is_empty());
}

#[test]
fn bluetooth_generated_loading_empty_and_partial_failure_text_never_implies_discovery() {
    let fixture = Fixture::new(|root| root.set_loading(true));
    fixture.element("Reading paired Bluetooth observations…");
    assert!(fixture.elements("Connected").is_empty());
    assert!(fixture.elements("No paired Bluetooth devices.").is_empty());
    assert_eq!(
        fixture.buttons().len(),
        1,
        "manual read-only refresh, no discovery controls"
    );
    fixture.popup.set_loading(false);
    fixture.popup.set_has_snapshot(true);
    fixture.popup.set_no_radios(true);
    fixture.popup.set_empty_inventory(true);
    fixture.render_fit(1.0);
    fixture.element("No Bluetooth radios exposed");
    fixture.element("No paired Bluetooth devices.");
    let notice = "Classic: Bluetooth access was denied. Native policy denied inventory.";
    fixture.popup.set_partial_inventory(true);
    fixture.popup.set_notice(notice.into());
    fixture.render_fit(1.0);
    fixture.element("No paired devices in the inventories read.");
    fixture.element(notice);
    assert!(fixture.elements("No paired Bluetooth devices.").is_empty());
    fixture.popup.set_empty_inventory(false);
    fixture.popup.set_unknown(model(vec![row(
        "Connection property absent",
        BluetoothTransportKind::LowEnergy,
    )]));
    fixture.render_fit(1.0);
    fixture.element("Connection unknown");
    assert_eq!(
        fixture
            .element("Connection property absent")
            .accessible_description()
            .unwrap(),
        "Connection unknown, Low Energy"
    );
    fixture.popup.set_loading(true);
    fixture.render_fit(1.0);
    fixture.element("Refreshing Bluetooth observations…");
    fixture.element(notice);
    assert!(fixture.elements("Available").is_empty());
    assert!(fixture.elements("Not found").is_empty());
    assert_eq!(fixture.buttons().len(), 1);
    assert!(fixture.take_requests().is_empty());
}

#[test]
fn bluetooth_generated_long_rtl_control_names_remain_bounded_semantic_text_in_both_themes() {
    let raw = format!("\u{202e}\u{0007}  אוזניות\n{}\u{200d} ", "界".repeat(250));
    let expected = bounded_text(&raw, 128);
    let fixture = Fixture::new(|root| {
        root.set_has_snapshot(true);
        root.set_unknown(model(vec![
            row(&raw, BluetoothTransportKind::LowEnergy),
            row(" \u{0007}\u{202e} ", BluetoothTransportKind::Classic),
        ]));
    });
    assert!(expected.chars().count() <= 129);
    assert!(!expected.contains('\u{202e}'));
    assert!(!expected.contains('\u{0007}'));
    assert!(expected.starts_with("אוזניות "));
    let name = fixture.element(&expected);
    assert_eq!(name.accessible_role(), Some(AccessibleRole::Text));
    fixture.element("Unnamed device");
    let light = fixture.render_fit(1.0);
    fixture
        .popup
        .apply_presentation_theme(PresentationTheme::uniform(
            slint::language::ColorScheme::Dark,
        ));
    let dark = fixture.render_fit(1.0);
    assert_ne!(
        light, dark,
        "actual palette/theme must affect software pixels"
    );
    for scale in [1.0, 1.5, 2.0] {
        let pixels = fixture.render_fit(scale);
        let element = fixture.element(&expected);
        let origin = element.absolute_position();
        let size = element.size();
        let window = fixture.window.window().size();
        assert!(size.width > 0.0 && size.height > 0.0);
        assert!(origin.x >= 0.0 && origin.y >= 0.0);
        assert!((origin.x + size.width) * scale <= window.width as f32 + 1.0);
        assert!((origin.y + size.height) * scale <= window.height as f32 + 1.0);
        assert_eq!(pixels.len(), (window.width * window.height) as usize);
    }
    assert!(fixture.take_requests().is_empty());
}

#[test]
fn bluetooth_generated_large_inventory_scrolls_in_bounded_viewport_and_tab_reveals_refresh() {
    let fixture = Fixture::new(|root| {
        root.set_has_snapshot(true);
        root.set_paired(model(
            (0..80)
                .map(|index| row(&format!("Paired {index}"), BluetoothTransportKind::Classic))
                .collect(),
        ));
    });
    assert!(fixture.popup.get_popup_content_height() <= 620.0);
    let first = fixture.element("Paired 0");
    assert!(
        first.size().height >= 24.0,
        "inventory rows retain readable height"
    );
    assert!(
        fixture.elements("Paired 79").is_empty(),
        "native viewport excludes clipped rows"
    );
    let position = LogicalPosition::new(100.0, 160.0);
    fixture
        .window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    fixture
        .window
        .window()
        .dispatch_event(WindowEvent::PointerScrolled {
            position,
            delta_x: 0.0,
            delta_y: -100_000.0,
        });
    fixture.render_fit(1.0);
    assert!(
        fixture.elements("Paired 0").is_empty(),
        "wheel scroll must retire the clipped head"
    );
    let tail = fixture.element("Paired 79");
    let tail_position = tail.absolute_position();
    let viewport = fixture.window.window().size();
    assert!(
        tail_position.y >= 0.0 && tail_position.y + tail.size().height <= viewport.height as f32
    );
    assert!(
        fixture.take_requests().is_empty(),
        "scrolling is not refresh/discovery"
    );
    fixture.popup.invoke_focus_content();
    fixture.key(Key::Tab);
    let refresh = fixture.element("Refresh Bluetooth");
    assert!(refresh.absolute_position().y >= 0.0 && refresh.absolute_position().y < 100.0);
    fixture.key(Key::Return);
    assert_eq!(fixture.take_requests(), vec![Request::Refresh]);
    // A tiny monitor still clips/scrolls content instead of growing off-screen.
    fixture.render(180, 120);
    fixture.popup.invoke_focus_content();
    fixture.key(Key::Tab);
    fixture.key(Key::Space);
    assert_eq!(fixture.take_requests(), vec![Request::Refresh]);
    assert_eq!(fixture.buttons().len(), 1);
}
