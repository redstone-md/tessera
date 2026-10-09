// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Generated popup, real native input/AX and owned software-renderer pixels.
//! These are test-only presentation fixtures, not WLAN/Windows privacy evidence.

use crate::generated::{NetworkMenu, NetworkRow, SeelenPalette};
use crate::theme::{PresentationTheme, ThemedComponent};
use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, ModelRc, PhysicalSize, Rgb8Pixel, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

const REFRESH: &str = "opaque::network/cache-refresh/session-π";
const SETTINGS: &str = "opaque::network/fixed-settings/session-π";
const ROW_HEIGHT: f32 = 12.8 + 3.0 * 17.92;
const SECTION_HEADER: f32 = 17.92 + 12.8;

fn model(rows: Vec<NetworkRow>) -> ModelRc<NetworkRow> {
    ModelRc::new(VecModel::from(rows))
}
fn row(section: &str, index: usize) -> NetworkRow {
    NetworkRow {
        label: format!("Fixture {section} {index}").into(),
        details: "Fixture adapter · 81% signal · Secured · Connection unknown".into(),
        description: format!(
            "Fixture {section} row {index}; 81% signal; Secured; 5G; Authentication unknown"
        )
        .into(),
        bands: "5G".into(),
    }
}
fn row_description(section: &str, index: usize) -> String {
    row(section, index).description.to_string()
}
fn ready(popup: &NetworkMenu) {
    popup.set_radio_text("Fixture adapter: Wi-Fi On".into());
    popup.set_summary("Best-effort cache, not an Internet connectivity assertion.".into());
    popup.set_connected(model(vec![NetworkRow {
        label: "Fixture connected".into(),
        details: "Fixture adapter · 40% signal · Secured · Current AP 01:01:01:01:01:01".into(),
        description: "Fixture exact connected BSSID, not every AP of this SSID".into(),
        bands: "2.4G / 5G".into(),
    }]));
    popup.set_saved(model(vec![row("saved", 0)]));
    popup.set_available(model(vec![row("available", 0)]));
    popup.set_hidden(model(vec![NetworkRow {
        label: "Hidden Network (2)".into(),
        details: "Fixture adapter · Security unknown or mixed · Saved profile unknown".into(),
        description: "Fixture hidden group of two raw empty SSIDs".into(),
        bands: "6G".into(),
    }]));
    popup.set_refresh_key(REFRESH.into());
    popup.set_settings_key(SETTINGS.into());
}
#[derive(Debug, PartialEq, Eq)]
enum Request {
    Refresh(String),
    Settings(String),
    Hide,
}
struct Fixture {
    window: Rc<MinimalSoftwareWindow>,
    popup: NetworkMenu,
    requests: Rc<RefCell<Vec<Request>>>,
}
impl Fixture {
    fn new(configure: impl FnOnce(&NetworkMenu)) -> Self {
        struct TestPlatform(Rc<MinimalSoftwareWindow>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
        let popup = NetworkMenu::new().unwrap();
        popup.apply_presentation_theme(PresentationTheme::uniform(
            slint::language::ColorScheme::Light,
        ));
        let requests = Rc::new(RefCell::new(Vec::new()));
        let recorded = requests.clone();
        popup.on_refresh_requested(move |key| {
            recorded
                .borrow_mut()
                .push(Request::Refresh(key.to_string()))
        });
        let recorded = requests.clone();
        popup.on_settings_requested(move |key| {
            recorded
                .borrow_mut()
                .push(Request::Settings(key.to_string()))
        });
        let recorded = requests.clone();
        popup.on_hide_requested(move || recorded.borrow_mut().push(Request::Hide));
        configure(&popup);
        popup.show().unwrap();
        window
            .window()
            .dispatch_event(WindowEvent::WindowActiveChanged(true));
        let fixture = Self {
            window,
            popup,
            requests,
        };
        fixture.render_fit(1.0);
        fixture
    }
    fn element(&self, label: &str) -> ElementHandle {
        let nodes = ElementHandle::find_by_accessible_label(&self.popup, label)
            .filter(
                |node| matches!(node.accessible_role(), Some(role) if role != AccessibleRole::None),
            )
            .collect::<Vec<_>>();
        assert_eq!(nodes.len(), 1, "exactly one semantic node for {label}");
        nodes.into_iter().next().unwrap()
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
        let width = (self.popup.get_popup_content_width() * scale).ceil() as u32;
        self.window.set_size(PhysicalSize::new(
            width,
            (self.popup.get_popup_content_height() * scale).ceil() as u32,
        ));
        self.render(
            width,
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
    fn center(node: &ElementHandle) -> LogicalPosition {
        let p = node.absolute_position();
        let s = node.size();
        LogicalPosition::new(p.x + s.width / 2.0, p.y + s.height / 2.0)
    }
    fn click(&self, node: &ElementHandle) {
        let position = Self::center(node);
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
    fn key(&self, key: Key) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }
    fn wheel(&self, position: LogicalPosition, delta_y: f32) {
        self.window
            .window()
            .dispatch_event(WindowEvent::PointerScrolled {
                position,
                delta_x: 0.0,
                delta_y,
            });
    }
    fn requests(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }
}
fn rgb(color: slint::Color) -> [u8; 3] {
    let c = color.to_argb_u8();
    [c.red, c.green, c.blue]
}
fn sample(pixels: &[Rgb8Pixel], width: usize, scale: f32, x: f32, y: f32) -> [u8; 3] {
    let pixel = pixels[(y * scale) as usize * width + (x * scale) as usize];
    [pixel.r, pixel.g, pixel.b]
}
fn has_ink(
    pixels: &[Rgb8Pixel],
    width: usize,
    height: usize,
    scale: f32,
    node: &ElementHandle,
    color: [u8; 3],
) -> bool {
    let p = node.absolute_position();
    let s = node.size();
    let left = (p.x * scale).ceil().max(0.0) as usize;
    let top = (p.y * scale).ceil().max(0.0) as usize;
    let right = ((p.x + s.width) * scale).floor().max(0.0) as usize;
    let bottom = ((p.y + s.height) * scale).floor().max(0.0) as usize;
    (top..bottom.min(height)).any(|y| {
        (left..right.min(width)).any(|x| {
            let pixel = pixels[y * width + x];
            [pixel.r, pixel.g, pixel.b] == color
        })
    })
}

#[test]
fn network_popup_four_real_sections_have_source_spacing_and_pixels_at_light_dark_scale_one_two() {
    let f = Fixture::new(ready);
    for scheme in [
        slint::language::ColorScheme::Light,
        slint::language::ColorScheme::Dark,
    ] {
        f.popup
            .apply_presentation_theme(PresentationTheme::uniform(scheme));
        for scale in [1.0_f32, 2.0] {
            let pixels = f.render_fit(scale);
            let size = f.window.window().size();
            let width = size.width as usize;
            let height = size.height as usize;
            let background = rgb(f.popup.global::<SeelenPalette>().get_surface());
            let foreground = rgb(f.popup.global::<SeelenPalette>().get_foreground());
            assert_eq!(
                sample(
                    &pixels,
                    width,
                    scale,
                    f.popup.get_popup_content_width() - 12.0,
                    40.0
                ),
                background
            );
            assert_ne!(sample(&pixels, width, scale, 0.0, 0.0), background);
            let descriptions = [
                "Fixture exact connected BSSID, not every AP of this SSID".into(),
                row_description("saved", 0),
                row_description("available", 0),
                "Fixture hidden group of two raw empty SSIDs".into(),
            ];
            let rows = descriptions
                .iter()
                .map(|label| f.element(label))
                .collect::<Vec<_>>();
            for row in &rows {
                assert_eq!(row.accessible_role(), Some(AccessibleRole::Text));
                assert!((row.size().height - ROW_HEIGHT).abs() < 0.02);
                assert!(
                    has_ink(&pixels, width, height, scale, row, foreground),
                    "actual SSID glyph pixels, scheme={scheme:?} scale={scale}"
                );
            }
            for pair in rows.windows(2) {
                assert!(
                    (pair[1].absolute_position().y
                        - pair[0].absolute_position().y
                        - ROW_HEIGHT
                        - SECTION_HEADER)
                        .abs()
                        < 0.02
                );
            }
            assert_eq!(
                f.buttons().len(),
                2,
                "only Refresh and Settings have button semantics"
            );
            assert!(
                f.requests().is_empty(),
                "theme/scale/render do not cause effects"
            );
            assert!(
                !f.window.draw_if_needed(|renderer| {
                    let mut pixels = vec![Rgb8Pixel::default(); width * height];
                    renderer.render(&mut pixels, width);
                }),
                "settled read-only popup has no idle frames"
            );
        }
    }
}

#[test]
fn network_popup_genuine_pointer_ax_and_keys_only_dispatch_refresh_and_fixed_settings_keys() {
    let f = Fixture::new(ready);
    for (label, request) in [
        ("Refresh", Request::Refresh(REFRESH.into())),
        ("More Network Settings", Request::Settings(SETTINGS.into())),
    ] {
        let node = f.element(label);
        f.click(&node);
        assert_eq!(f.requests(), vec![request]);
        node.invoke_accessible_default_action();
        let expected = if label == "Refresh" {
            Request::Refresh(REFRESH.into())
        } else {
            Request::Settings(SETTINGS.into())
        };
        assert_eq!(f.requests(), vec![expected]);
    }
    let readonly = f.element(&row_description("available", 0));
    f.click(&readonly);
    readonly.invoke_accessible_default_action();
    assert!(
        f.requests().is_empty(),
        "read rows expose no mutation/default action"
    );
    f.popup.invoke_focus_content();
    f.key(Key::Tab);
    f.key(Key::Return);
    assert_eq!(f.requests(), vec![Request::Refresh(REFRESH.into())]);
    f.key(Key::Tab);
    f.key(Key::Space);
    assert_eq!(f.requests(), vec![Request::Settings(SETTINGS.into())]);
    f.key(Key::Escape);
    assert_eq!(f.requests(), vec![Request::Hide]);
}

#[test]
fn network_popup_loading_and_revoked_keys_reject_pointer_keyboard_ax_without_fake_controls() {
    let f = Fixture::new(ready);
    f.popup.set_loading(true);
    f.popup.set_settings_loading(true);
    f.render_fit(1.0);
    for label in ["Refresh", "More Network Settings"] {
        let node = f.element(label);
        f.click(&node);
        node.invoke_accessible_default_action();
    }
    f.popup.invoke_focus_content();
    f.key(Key::Tab);
    f.key(Key::Space);
    assert!(f.requests().is_empty());
    f.popup.set_loading(false);
    f.popup.set_settings_loading(false);
    f.popup.set_refresh_key("".into());
    f.popup.set_settings_key("".into());
    f.render_fit(1.0);
    for node in f.buttons() {
        f.click(&node);
        node.invoke_accessible_default_action();
    }
    assert!(f.requests().is_empty());
    assert_eq!(f.buttons().len(), 2);
    f.key(Key::Escape);
    assert_eq!(f.requests(), vec![Request::Hide]);
}

#[test]
fn network_popup_unavailable_off_absent_empty_partial_watch_states_paint_and_keep_recovery_reachable()
 {
    let f = Fixture::new(|popup| {
        popup.set_refresh_key(REFRESH.into());
        popup.set_settings_key(SETTINGS.into());
    });
    for (radio, summary, notice, watch) in [
        (
            "No Wi-Fi adapter found",
            "Windows confirmed no adapters.",
            "",
            "Notifications unsupported; use Refresh.",
        ),
        (
            "Fixture adapter: Wi-Fi Off",
            "Enable Wi-Fi in Windows Settings, then Refresh.",
            "",
            "",
        ),
        (
            "Fixture adapter: Wi-Fi Unknown",
            "",
            "Windows denied Wi-Fi access. Check location permissions, then Refresh.",
            "",
        ),
        (
            "Fixture adapter: Wi-Fi On",
            "No Wi-Fi networks in the Windows cache.",
            "",
            "Notifications active; no active scanning.",
        ),
        (
            "Fixture adapter: Wi-Fi On · Other: Unavailable",
            "Partial cache read. Another adapter is unavailable.",
            "",
            "Notifications unavailable; use Refresh.",
        ),
    ] {
        f.popup.set_radio_text(radio.into());
        f.popup.set_summary(summary.into());
        f.popup.set_notice(notice.into());
        f.popup.set_watch_status(watch.into());
        for scheme in [
            slint::language::ColorScheme::Light,
            slint::language::ColorScheme::Dark,
        ] {
            f.popup
                .apply_presentation_theme(PresentationTheme::uniform(scheme));
            for scale in [1.0_f32, 2.0] {
                let pixels = f.render_fit(scale);
                let size = f.window.window().size();
                let foreground = rgb(f.popup.global::<SeelenPalette>().get_foreground());
                assert!(has_ink(
                    &pixels,
                    size.width as usize,
                    size.height as usize,
                    scale,
                    &f.element(radio),
                    foreground
                ));
                let feedback = if notice.is_empty() { summary } else { notice };
                assert!(has_ink(
                    &pixels,
                    size.width as usize,
                    size.height as usize,
                    scale,
                    &f.element(feedback),
                    foreground
                ));
                assert_eq!(f.buttons().len(), 2);
                f.popup.invoke_focus_content();
                f.key(Key::Tab);
                f.key(Key::Return);
                assert_eq!(f.requests(), vec![Request::Refresh(REFRESH.into())]);
                f.key(Key::Tab);
                f.key(Key::Space);
                assert_eq!(f.requests(), vec![Request::Settings(SETTINGS.into())]);
            }
        }
    }
}

#[test]
fn network_popup_sections_scroll_independently_at_three_hundred_pixels_with_native_thumb_raster() {
    let f = Fixture::new(|popup| {
        popup.set_radio_text("Fixture adapter: Wi-Fi On".into());
        popup.set_saved(model((0..10).map(|i| row("saved", i)).collect()));
        popup.set_available(model((0..10).map(|i| row("available", i)).collect()));
        popup.set_refresh_key(REFRESH.into());
        popup.set_settings_key(SETTINGS.into());
    });
    let before = f.render_fit(1.0);
    let saved = f.element(&row_description("saved", 0));
    let available = f.element(&row_description("available", 0));
    let saved_origin = saved.absolute_position();
    let available_origin = available.absolute_position();
    assert!(
        (available_origin.y - saved_origin.y - 300.0 - SECTION_HEADER).abs() < 0.02,
        "independent section viewport caps at 300 logical pixels"
    );
    f.wheel(Fixture::center(&saved), -150.0);
    let size = f.window.window().size();
    let after = f.render(size.width, size.height);
    assert!(saved.absolute_position().y < saved_origin.y);
    assert_eq!(
        available.absolute_position(),
        available_origin,
        "saved wheel cannot move the other section or outer viewport"
    );
    let width = size.width as usize;
    let x_start = (saved_origin.x + saved.size().width).ceil().max(0.0) as usize;
    let x_end = (x_start + 14).min(width);
    let y_start = saved_origin.y.ceil().max(0.0) as usize;
    let y_end = (y_start + 300).min(size.height as usize);
    assert!(
        (y_start..y_end).any(|y| (x_start..x_end).any(|x| {
            let a = before[y * width + x];
            let b = after[y * width + x];
            [a.r, a.g, a.b] != [b.r, b.g, b.b]
        })),
        "actual Fluent scrollbar-thumb pixels must move, not a synthetic scrolling receipt"
    );
    let saved_position = saved.absolute_position();
    f.wheel(Fixture::center(&available), -100.0);
    f.render(size.width, size.height);
    assert!(available.absolute_position().y < available_origin.y);
    assert_eq!(saved.absolute_position(), saved_position);
    assert!(
        f.requests().is_empty(),
        "scroll does not refresh or mutate WLAN"
    );
}

#[test]
fn network_popup_long_rtl_non_utf8_labels_clip_in_tiny_viewport_and_tab_reveals_footer() {
    let f = Fixture::new(|popup| {
        ready(popup);
        popup.set_available(model(vec![NetworkRow {
            label: format!("شبكة עברית {} replacement �", "long".repeat(24)).into(),
            details: "Signal 0%; Security unknown; Saved profile unknown; Connection unknown".into(),
            description: "Fixture long RTL and lossy non-UTF8 label, retained raw identity lives in presenter".into(),
            bands: "".into(),
        }]));
    });
    for scale in [1.0_f32, 2.0] {
        f.window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        let width = (180.0 * scale) as u32;
        let height = (180.0 * scale) as u32;
        f.popup.invoke_focus_content();
        let pixels = f.render(width, height);
        let background = rgb(f.popup.global::<SeelenPalette>().get_surface());
        assert_eq!(
            sample(&pixels, width as usize, scale, 168.0, 40.0),
            background
        );
        let description =
            "Fixture long RTL and lossy non-UTF8 label, retained raw identity lives in presenter";
        let mut visible_pixels = pixels;
        let mut row_visible = false;
        // Native ScrollView omits clipped semantic children. Wheel over its
        // outer scrollbar gutter so section scrollviews cannot consume the input.
        for _ in 0..64 {
            row_visible =
                ElementHandle::find_by_accessible_label(&f.popup, description).any(|node| {
                    let top = node.absolute_position().y;
                    node.accessible_role() == Some(AccessibleRole::Text)
                        && top >= 16.0
                        && top + node.size().height <= 164.0
                });
            if row_visible {
                break;
            }
            f.wheel(LogicalPosition::new(160.0, 80.0), -32.0);
            visible_pixels = f.render(width, height);
        }
        assert!(
            row_visible,
            "native scrolling must reveal the full readonly row in the constrained viewport"
        );
        let readonly = f.element(description);
        assert_eq!(readonly.accessible_role(), Some(AccessibleRole::Text));
        assert!(has_ink(
            &visible_pixels,
            width as usize,
            height as usize,
            scale,
            &readonly,
            rgb(f.popup.global::<SeelenPalette>().get_foreground())
        ));
        readonly.invoke_accessible_default_action();
        assert!(
            f.requests().is_empty(),
            "the visible row must remain readonly"
        );
        f.popup.invoke_focus_content();
        f.key(Key::Tab);
        f.render(width, height);
        let refresh = f.element("Refresh");
        let center = Fixture::center(&refresh);
        assert!(center.y >= 10.0 && center.y < 170.0 && center.x > 10.0 && center.x < 170.0);
        f.click(&refresh);
        assert_eq!(f.requests(), vec![Request::Refresh(REFRESH.into())]);
        f.key(Key::Tab);
        f.render(width, height);
        let settings = f.element("More Network Settings");
        assert!(Fixture::center(&settings).y < 170.0);
        assert_eq!(
            f.buttons().len(),
            2,
            "only Refresh and More Network Settings are exposed after footer reveal"
        );
        f.element("Refresh");
        f.key(Key::Space);
        assert_eq!(f.requests(), vec![Request::Settings(SETTINGS.into())]);
        f.key(Key::Escape);
        assert_eq!(f.requests(), vec![Request::Hide]);
    }
    // The generated component permits a native work-area clamp all the way to
    // one pixel. No negative geometry, custom fake scrolling or popup minimum.
    f.render(1, 1);
    assert_eq!(f.window.window().size(), PhysicalSize::new(1, 1));
}
