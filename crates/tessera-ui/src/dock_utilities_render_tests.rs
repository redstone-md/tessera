// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Generated Dock, genuine native input/AX and owned software pixels only.
//! Recorded callbacks have no desktop host, COM, recovery or persistence effects.

use std::cell::RefCell;
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, TargetPixel};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, ModelRc, PhysicalSize, Rgb8Pixel, VecModel};

use crate::generated::{Dock, DockApp, DockMenuKind, DockReservedAction, DockStatus, DockWindow};
use crate::theme::{PresentationTheme, ThemedComponent};

const START: &str = "Open applications and settings";
const DESKTOP: &str = "Show desktop";

#[derive(Debug, PartialEq)]
enum Request {
    Start,
    Desktop(DockReservedAction),
    Launch(String),
    Window(String),
    Context(DockMenuKind, String),
    Wake,
}

struct Fixture {
    window: Rc<MinimalSoftwareWindow>,
    dock: Dock,
    requests: Rc<RefCell<Vec<Request>>>,
    tooltips: Rc<RefCell<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        // The same owned native adapter as the existing renderer fixtures.
        struct TestPlatform(Rc<MinimalSoftwareWindow>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
        let dock = Dock::new().unwrap();
        dock.apply_presentation_theme(PresentationTheme::uniform(
            slint::language::ColorScheme::Light,
        ));
        let requests = Rc::new(RefCell::new(Vec::new()));
        let recorded = requests.clone();
        dock.on_open_applications_requested(move || recorded.borrow_mut().push(Request::Start));
        let recorded = requests.clone();
        dock.on_reserved_action_requested(move |action| {
            recorded.borrow_mut().push(Request::Desktop(action))
        });
        let recorded = requests.clone();
        dock.on_launch_requested(move |key| {
            recorded.borrow_mut().push(Request::Launch(key.to_string()))
        });
        let recorded = requests.clone();
        dock.on_window_command_requested(move |key, _| {
            recorded.borrow_mut().push(Request::Window(key.to_string()))
        });
        let recorded = requests.clone();
        dock.on_context_menu_requested(move |kind, key, _| {
            recorded
                .borrow_mut()
                .push(Request::Context(kind, key.to_string()))
        });
        let recorded = requests.clone();
        dock.on_utility_event_ready(move || recorded.borrow_mut().push(Request::Wake));
        let tooltips = Rc::new(RefCell::new(Vec::new()));
        let recorded = tooltips.clone();
        dock.on_tooltip_requested(move |text, _| recorded.borrow_mut().push(text.to_string()));
        dock.show().unwrap();
        window
            .window()
            .dispatch_event(WindowEvent::WindowActiveChanged(true));
        let fixture = Self {
            window,
            dock,
            requests,
            tooltips,
        };
        fixture.render(216, 72, 1.0);
        fixture
    }

    fn apps(&self, count: usize) {
        self.dock.set_pinned_apps(ModelRc::new(VecModel::from(
            (0..count)
                .map(|index| DockApp {
                    key: format!("opaque::app/{index}::π").into(),
                    label: format!("Fixture {index}").into(),
                    icon: slint::Image::default(),
                    pinned: true,
                })
                .collect::<Vec<_>>(),
        )));
    }

    fn render(&self, width: u32, height: u32, scale: f32) -> Vec<Rgb8Pixel> {
        self.window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        self.window.set_size(PhysicalSize::new(width, height));
        self.window.request_redraw();
        let mut pixels = vec![Rgb8Pixel::default(); (width * height) as usize];
        assert!(self.window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, width as usize);
        }));
        pixels
    }

    fn element(&self, label: &str) -> ElementHandle {
        let mut elements = ElementHandle::find_by_accessible_label(&self.dock, label)
            .filter(|element| matches!(element.accessible_role(), Some(role) if role != AccessibleRole::None));
        let element = elements
            .next()
            .unwrap_or_else(|| panic!("missing semantic node {label}"));
        assert!(elements.next().is_none(), "duplicate semantic node {label}");
        element
    }

    fn viewport(&self) -> ElementHandle {
        ElementHandle::find_by_element_id(&self.dock, "ScrollView::flickable")
            .next()
            .unwrap()
    }

    fn center(element: &ElementHandle) -> LogicalPosition {
        let origin = element.absolute_position();
        let size = element.size();
        LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
    }

    fn pointer(&self, element: &ElementHandle, button: PointerEventButton) {
        let position = Self::center(element);
        self.window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        self.window
            .window()
            .dispatch_event(WindowEvent::PointerPressed { position, button });
        self.window
            .window()
            .dispatch_event(WindowEvent::PointerReleased { position, button });
    }

    fn press(&self, key: Key) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    }

    fn release(&self, key: Key) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }

    fn repeat(&self, key: Key) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressRepeated { text: key.into() });
    }

    fn key(&self, key: Key) {
        self.press(key);
        self.release(key);
    }

    fn take(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }
}

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.02,
        "expected {expected}, actual {actual}"
    );
}

#[test]
fn dock_utilities_fixed_slots_and_app_order_on_four_edges_compact_themes_and_dpi() {
    let fixture = Fixture::new();
    fixture.apps(2);
    fixture
        .dock
        .set_running_windows(ModelRc::new(VecModel::from(vec![DockWindow {
            key: "opaque::window/後".into(),
            caption: "Fixture window".into(),
            icon: slint::Image::default(),
        }])));
    for edge in 0..4 {
        fixture.dock.set_edge(edge);
        for compact in [false, true] {
            fixture.dock.set_compact(compact);
            let factor: f32 = if compact { 0.8 } else { 1.0 };
            for scheme in [
                slint::language::ColorScheme::Light,
                slint::language::ColorScheme::Dark,
            ] {
                fixture
                    .dock
                    .apply_presentation_theme(PresentationTheme::uniform(scheme));
                for scale in [1.0, 2.0] {
                    let (width, height) = if edge < 2 {
                        (312.0, 72.0)
                    } else {
                        (72.0, 312.0)
                    };
                    fixture.render(
                        (width * factor * scale).ceil() as u32,
                        (height * factor * scale).ceil() as u32,
                        scale,
                    );
                    let labels = [
                        START,
                        DESKTOP,
                        "Launch Fixture 0",
                        "Launch Fixture 1",
                        "Switch to Fixture window",
                    ];
                    for (index, label) in labels.iter().enumerate() {
                        let tile = fixture.element(label);
                        assert_eq!(tile.accessible_role(), Some(AccessibleRole::Button));
                        assert_eq!(tile.accessible_enabled(), Some(true));
                        let origin = tile.absolute_position();
                        let along = (16.0 + index as f32 * 48.0) * factor;
                        close(origin.x, if edge < 2 { along } else { 16.0 * factor });
                        close(origin.y, if edge < 2 { 16.0 * factor } else { along });
                        close(tile.size().width, 40.0 * factor);
                        close(tile.size().height, 40.0 * factor);
                    }
                    let viewport = fixture.viewport();
                    close(
                        if edge < 2 {
                            viewport.absolute_position().x
                        } else {
                            viewport.absolute_position().y
                        },
                        112.0 * factor,
                    );
                    assert_eq!(
                        fixture.dock.get_pinned_apps().row_data(0).unwrap().key,
                        "opaque::app/0::π"
                    );
                    assert_eq!(
                        fixture.dock.get_pinned_apps().row_data(1).unwrap().key,
                        "opaque::app/1::π"
                    );
                    assert_eq!(
                        fixture.dock.get_running_windows().row_data(0).unwrap().key,
                        "opaque::window/後"
                    );
                    assert!(
                        fixture.take().is_empty(),
                        "layout/theme/DPI is never an action"
                    );
                }
            }
        }
    }
}

#[test]
fn dock_utilities_real_pointer_tab_enter_space_ax_tooltip_and_bar_context() {
    let fixture = Fixture::new();
    fixture.apps(1);
    fixture.render(216, 72, 1.0);
    let desktop = fixture.element(DESKTOP);
    fixture
        .window
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: Fixture::center(&desktop),
        });
    assert!(fixture.take().is_empty());
    assert_eq!(
        fixture.tooltips.borrow().last().map(String::as_str),
        Some(DESKTOP)
    );
    fixture
        .dock
        .set_show_desktop_notice("Desktop toggle requested".into());
    fixture.render(216, 72, 1.0);
    assert_eq!(
        fixture.element(DESKTOP).accessible_label().as_deref(),
        Some(DESKTOP)
    );
    assert_eq!(
        fixture.tooltips.borrow().last().map(String::as_str),
        Some("Show desktop\nDesktop toggle requested")
    );
    fixture.pointer(&desktop, PointerEventButton::Left);
    assert_eq!(
        fixture.take(),
        vec![Request::Desktop(DockReservedAction::ShowDesktop)]
    );
    fixture.pointer(&desktop, PointerEventButton::Right);
    assert_eq!(
        fixture.take(),
        vec![Request::Context(DockMenuKind::Bar, String::new())]
    );
    desktop.invoke_accessible_default_action();
    assert_eq!(
        fixture.take(),
        vec![Request::Desktop(DockReservedAction::ShowDesktop)]
    );
    // Native pointer focuses Start; Tab must reach the literal second tile.
    fixture.pointer(&fixture.element(START), PointerEventButton::Left);
    assert_eq!(fixture.take(), vec![Request::Start]);
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    assert_eq!(
        fixture.take(),
        vec![Request::Desktop(DockReservedAction::ShowDesktop)]
    );
    fixture.key(Key::Menu);
    assert_eq!(
        fixture.take(),
        vec![Request::Context(DockMenuKind::Bar, String::new())]
    );
    fixture.press(Key::Space);
    assert!(
        fixture.take().is_empty(),
        "Space activates on release, not press"
    );
    fixture.release(Key::Space);
    assert_eq!(
        fixture.take(),
        vec![Request::Desktop(DockReservedAction::ShowDesktop)]
    );
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    assert_eq!(
        fixture.take(),
        vec![Request::Launch("opaque::app/0::π".to_owned())]
    );
    fixture.dock.invoke_utility_event_ready();
    assert_eq!(
        fixture.take(),
        vec![Request::Wake],
        "mailbox wake is not a desktop intent"
    );
}

#[test]
fn dock_utilities_busy_cancels_held_space_and_repeats_without_blocking_start_or_recovery() {
    let fixture = Fixture::new();
    fixture.apps(1);
    fixture.render(216, 72, 1.0);
    fixture.pointer(&fixture.element(START), PointerEventButton::Left);
    fixture.take();
    fixture.key(Key::Tab);
    fixture.press(Key::Space);
    fixture.repeat(Key::Space);
    fixture.dock.set_show_desktop_busy(true);
    fixture
        .dock
        .set_show_desktop_notice("Desktop toggle pending".into());
    fixture.dock.set_surface_status(DockStatus {
        refreshing: true,
        stale: true,
        ..DockStatus::default()
    });
    fixture.render(216, 72, 1.0);
    let desktop = fixture.element(DESKTOP);
    assert_eq!(desktop.accessible_enabled(), Some(false));
    assert_eq!(fixture.element(START).accessible_enabled(), Some(true));
    assert_eq!(
        fixture.element("Launch Fixture 0").accessible_enabled(),
        Some(false)
    );
    fixture.release(Key::Space);
    fixture.pointer(&desktop, PointerEventButton::Left);
    desktop.invoke_accessible_default_action();
    assert!(
        fixture.take().is_empty(),
        "busy utility rejects held, pointer and AX input"
    );
    fixture.pointer(&fixture.element(START), PointerEventButton::Right);
    assert_eq!(
        fixture.take(),
        vec![Request::Context(DockMenuKind::Bar, String::new())]
    );
    fixture.pointer(&fixture.element(START), PointerEventButton::Left);
    assert_eq!(fixture.take(), vec![Request::Start]);
    fixture.dock.set_show_desktop_busy(false);
    fixture.render(216, 72, 1.0);
    assert_eq!(fixture.element(DESKTOP).accessible_enabled(), Some(true));
    fixture.key(Key::Tab);
    fixture.press(Key::Space);
    fixture.dock.set_show_desktop_busy(true);
    fixture.render(216, 72, 1.0);
    fixture.dock.set_show_desktop_busy(false);
    fixture.render(216, 72, 1.0);
    fixture.repeat(Key::Space); // Still held: repeat cannot revive the cancelled arm.
    fixture.release(Key::Space);
    assert!(fixture.take().is_empty());
    fixture.key(Key::Space);
    assert_eq!(
        fixture.take(),
        vec![Request::Desktop(DockReservedAction::ShowDesktop)]
    );
    for (refreshing, stale) in [(true, false), (false, true)] {
        fixture.dock.set_surface_status(DockStatus {
            refreshing,
            stale,
            ..DockStatus::default()
        });
        fixture.render(216, 72, 1.0);
        fixture.pointer(&fixture.element(DESKTOP), PointerEventButton::Left);
        assert_eq!(
            fixture.take(),
            vec![Request::Desktop(DockReservedAction::ShowDesktop)]
        );
        fixture.pointer(
            &fixture.element("Launch Fixture 0"),
            PointerEventButton::Left,
        );
        assert!(
            fixture.take().is_empty(),
            "application freshness is separate from utility authority"
        );
    }
}

#[test]
fn dock_utilities_wheel_keeps_reserved_tiles_fixed_and_reaches_unbounded_apps_with_finite_tiny_bounds()
 {
    let fixture = Fixture::new();
    fixture.apps(64);
    for edge in 0..4 {
        fixture.dock.set_edge(edge);
        let (width, height) = if edge < 2 { (216, 72) } else { (72, 216) };
        fixture.render(width, height, 1.0);
        let desktop_origin = fixture.element(DESKTOP).absolute_position();
        let viewport = fixture.viewport();
        let position = Fixture::center(&viewport);
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerScrolled {
                position,
                delta_x: if edge < 2 { -100_000.0 } else { 0.0 },
                delta_y: if edge < 2 { 0.0 } else { -100_000.0 },
            });
        fixture.render(width, height, 1.0);
        assert_eq!(fixture.element(DESKTOP).absolute_position(), desktop_origin);
        assert_eq!(fixture.dock.get_pinned_apps().row_count(), 64);
        fixture.pointer(
            &fixture.element("Launch Fixture 63"),
            PointerEventButton::Left,
        );
        assert_eq!(
            fixture.take(),
            vec![Request::Launch("opaque::app/63::π".to_owned())]
        );
        // Wheel over a reserved utility is not routed into application scrolling.
        let final_origin = fixture.element("Launch Fixture 63").absolute_position();
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerScrolled {
                position: Fixture::center(&fixture.element(DESKTOP)),
                delta_x: 100_000.0,
                delta_y: 100_000.0,
            });
        fixture.render(width, height, 1.0);
        assert_eq!(
            fixture.element("Launch Fixture 63").absolute_position(),
            final_origin
        );
        assert!(fixture.take().is_empty());
        for scale in [1.0, 2.0] {
            for (tiny_width, tiny_height) in [(1, 1), (8, 72), (72, 8), (24, 24)] {
                fixture.render(tiny_width, tiny_height, scale);
                for element in ElementQuery::from_root(&fixture.dock).find_all() {
                    let origin = element.absolute_position();
                    let size = element.size();
                    assert!(origin.x.is_finite() && origin.y.is_finite());
                    assert!(size.width.is_finite() && size.height.is_finite());
                    assert!(
                        size.width >= 0.0 && size.height >= 0.0,
                        "nonnegative native bounds: {size:?}"
                    );
                }
                assert!(fixture.take().is_empty(), "tiny geometry cannot dispatch");
            }
        }
    }
}

#[test]
fn dock_utilities_exact_duotone_screen_bezel_and_exterior_mask_in_semantic_native_pixels() {
    // The independent official SVG's full-resolution mask, not a redrawn glyph.
    let source = slint::Image::load_from_svg_data(include_bytes!(
        "../assets/icons/phosphor-core-2.1.1/desktop-duotone.svg"
    ))
    .unwrap();
    let mask = source.to_rgba8().unwrap();
    assert_eq!((mask.width(), mask.height()), (256, 256));
    assert_eq!(
        mask.as_slice()[100 * 256 + 128].a,
        51,
        "original screen path has exactly 20% alpha"
    );
    assert_eq!(
        mask.as_slice()[152 * 256 + 128].a,
        255,
        "original bezel is opaque"
    );
    assert_eq!(
        mask.as_slice()[128 * 256 + 8].a,
        0,
        "original exterior is transparent"
    );
    assert_duotone_mask_matches_native_pixels(&source, DESKTOP);
}

#[test]
fn dock_utilities_independent_generic_trash_interior_lid_and_exterior_mask_in_semantic_native_pixels()
 {
    // One legal generic identity mark; not reference empty/full artwork.
    let source = slint::Image::load_from_svg_data(include_bytes!(
        "../assets/icons/phosphor-core-2.1.1/trash-duotone.svg"
    ))
    .unwrap();
    let mask = source.to_rgba8().unwrap();
    assert_eq!((mask.width(), mask.height()), (256, 256));
    assert_eq!(mask.as_slice()[128 * 256 + 128].a, 51, "original interior");
    assert_eq!(mask.as_slice()[56 * 256 + 128].a, 255, "original lid");
    assert_eq!(mask.as_slice()[128 * 256 + 8].a, 0, "original exterior");
    assert_duotone_mask_matches_native_pixels(&source, "Recycle Bin\nUnknown");
}

fn assert_duotone_mask_matches_native_pixels(source: &slint::Image, label: &str) {
    let fixture = Fixture::new();
    for edge in 0..4 {
        fixture.dock.set_edge(edge);
        for compact in [false, true] {
            fixture.dock.set_compact(compact);
            let factor: f32 = if compact { 0.8 } else { 1.0 };
            for scheme in [
                slint::language::ColorScheme::Light,
                slint::language::ColorScheme::Dark,
            ] {
                fixture
                    .dock
                    .apply_presentation_theme(PresentationTheme::uniform(scheme));
                let (tile, foreground) = if scheme == slint::language::ColorScheme::Dark {
                    (31, 228)
                } else {
                    (252, 18)
                };
                for scale in [1.0, 2.0] {
                    let (width, height) = if edge < 2 {
                        (168.0, 72.0)
                    } else {
                        (72.0, 168.0)
                    };
                    let width = (width * factor * scale).ceil() as u32;
                    let pixels =
                        fixture.render(width, (height * factor * scale).ceil() as u32, scale);
                    let origin = fixture.element(label).absolute_position();
                    let image = fixture
                        .element(label)
                        .query_descendants()
                        .match_type_name("Image")
                        .find_first()
                        .unwrap();
                    let image_origin = image.absolute_position();
                    close(image_origin.x - origin.x, 6.0 * factor);
                    close(image_origin.y - origin.y, 6.0 * factor);
                    close(image.size().width, 28.0 * factor);
                    close(image.size().height, 28.0 * factor);
                    // Pinned SDK 1.18.1 draw_image_impl rounds the physical
                    // destination rect and rasterizes SVG at that exact size.
                    // Keep this unstable SDK instrumentation test-local.
                    let x = (image_origin.x * scale).round() as usize;
                    let y = (image_origin.y * scale).round() as usize;
                    let right = ((image_origin.x + image.size().width) * scale).round() as usize;
                    let bottom = ((image_origin.y + image.size().height) * scale).round() as usize;
                    let raster =
                        slint::private_unstable_api::re_exports::image_to_rgba8_with_target_size(
                            source,
                            slint::private_unstable_api::re_exports::IntSize::new(
                                (right - x) as u32,
                                (bottom - y) as u32,
                            ),
                        )
                        .unwrap();
                    let mut screen_pixels = 0;
                    let mut opaque_pixels = 0;
                    let mut exterior_pixels = 0;
                    for row in 0..raster.height() as usize {
                        for column in 0..raster.width() as usize {
                            let alpha = raster.as_slice()[row * raster.width() as usize + column].a;
                            let mut expected = Rgb8Pixel {
                                r: tile,
                                g: tile,
                                b: tile,
                            };
                            expected.blend(
                                slint::Color::from_argb_u8(
                                    alpha, foreground, foreground, foreground,
                                )
                                .into(),
                            );
                            let actual = pixels[(y + row) * width as usize + x + column];
                            assert_eq!(
                                actual, expected,
                                "exact source mask/colorize at edge={edge} compact={compact} scale={scale} pixel=({column},{row}) alpha={alpha}"
                            );
                            screen_pixels += usize::from(alpha == 51);
                            opaque_pixels += usize::from(alpha == 255);
                            exterior_pixels += usize::from(alpha == 0);
                        }
                    }
                    assert!(
                        screen_pixels > 0,
                        "real secondary path retains 20% coverage"
                    );
                    assert!(
                        opaque_pixels > 0,
                        "real primary path retains opaque coverage"
                    );
                    assert!(exterior_pixels > 0, "real exterior stays transparent");
                    assert!(
                        fixture.take().is_empty(),
                        "glyph inspection never dispatches a utility"
                    );
                }
            }
        }
    }
}
