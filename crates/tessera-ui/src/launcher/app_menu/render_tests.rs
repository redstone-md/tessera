// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The adapter's actual generated surface, with SDK input and software pixels.
//! Native screen positions are not recorded by MinimalSoftwareWindow; negative
//! monitor contexts exercise production placement sizing, not OS positioning.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, Rgb8Pixel};

use super::LauncherAppMenu;
use super::tests::{click_row, press, setup_with_platform};
use crate::DockContext;
use crate::generated::{ContextMenuSurface, Launcher, PopoverTokens, SeelenPalette};
use crate::theme::{PresentationTheme, ThemedComponent};

type Requests = Rc<RefCell<Vec<(String, bool)>>>;

struct Fixture {
    launcher: Launcher,
    menu: Rc<LauncherAppMenu>,
    window: Rc<MinimalSoftwareWindow>,
    requests: Requests,
}

impl Fixture {
    fn new() -> Self {
        // Same software-window harness as the existing renderer tests, with one
        // adapter per real component instead of sharing the launcher's window.
        struct TestPlatform(Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
                self.0.borrow_mut().push(window.clone());
                Ok(window)
            }

            fn duration_since_start(&self) -> Duration {
                Duration::ZERO
            }
        }
        let windows = Rc::new(RefCell::new(Vec::new()));
        slint::platform::set_platform(Box::new(TestPlatform(windows.clone()))).unwrap();
        let (_host, launcher, menu) = setup_with_platform();
        menu.disable_motion();
        let surface_window = menu.surface.window();
        let window = windows
            .borrow()
            .iter()
            .find(|window| std::ptr::eq(window.window(), surface_window))
            .expect("the adapter owns a distinct software-rendered native window")
            .clone();
        assert!(!std::ptr::eq(window.window(), launcher.window()));
        let requests = Rc::new(RefCell::new(Vec::new()));
        let recorded = requests.clone();
        launcher.on_favorite_toggle_requested(move |key, desired| {
            recorded.borrow_mut().push((key.to_string(), desired));
        });
        Self {
            launcher,
            menu,
            window,
            requests,
        }
    }

    fn surface(&self) -> &ContextMenuSurface {
        &self.menu.surface
    }

    fn show(&self, favorite: bool, scale: f32, context: DockContext) {
        self.menu.hide();
        for window in [self.launcher.window(), self.surface().window()] {
            window.dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        }
        self.menu
            .show(
                &self.launcher,
                "opaque app key".into(),
                favorite,
                LogicalPosition::new(150.0, 200.0),
                context,
            )
            .unwrap();
        self.window
            .window()
            .dispatch_event(WindowEvent::WindowActiveChanged(true));
        assert!(self.menu.is_open());
    }

    fn render(&self) -> (Vec<Rgb8Pixel>, u32, u32) {
        let PhysicalSize { width, height } = self.window.window().size();
        assert!(width > 0 && height > 0);
        self.window.request_redraw();
        let mut pixels = vec![Rgb8Pixel::default(); (width * height) as usize];
        assert!(self.window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, width as usize);
        }));
        (pixels, width, height)
    }

    fn row(&self, label: &str) -> ElementHandle {
        let mut rows = ElementHandle::find_by_accessible_label(self.surface(), label)
            .filter(|row| row.accessible_role() == Some(AccessibleRole::Button));
        let row = rows.next().expect("the real Pin/Unpin row is accessible");
        assert!(
            rows.next().is_none(),
            "the action is not duplicated by metric probes"
        );
        row
    }

    fn assert_only_row(&self, label: &str) {
        let buttons = ElementQuery::from_root(self.surface())
            .match_accessible_role(AccessibleRole::Button)
            .find_all();
        assert_eq!(
            buttons.len(),
            1,
            "no dock or speculative application actions"
        );
        assert_eq!(buttons[0].accessible_label().unwrap().as_str(), label);
    }

    fn take_requests(&self) -> Vec<(String, bool)> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }
}

fn sample(pixels: &[Rgb8Pixel], width: u32, scale: f32, x: f32, y: f32) -> [u8; 3] {
    let pixel = pixels[(y * scale) as usize * width as usize + (x * scale) as usize];
    [pixel.r, pixel.g, pixel.b]
}

#[test]
fn launcher_app_menu_real_pixels_ax_and_focus_outline_at_both_scales_and_negative_monitors() {
    let fixture = Fixture::new();
    for (scheme, background) in [
        (slint::language::ColorScheme::Light, [242, 242, 242]),
        (slint::language::ColorScheme::Dark, [24, 24, 24]),
    ] {
        fixture
            .launcher
            .apply_presentation_theme(PresentationTheme::uniform(scheme));
        for scale in [1.0, 2.0] {
            for (favorite, label) in [(false, "Pin"), (true, "Unpin")] {
                fixture.show(
                    favorite,
                    scale,
                    DockContext::new(-1920, -1080, 1920, 1080, false).unwrap(),
                );
                let surface = fixture.surface();
                let tokens = surface.global::<PopoverTokens>();
                let margin = tokens.get_shadow_margin();
                let row_height = tokens.get_font_size() * tokens.get_line_height() + 16.0;
                assert!(
                    (surface.get_menu_height() - (2.0 * margin + 16.0 + row_height)).abs() < 0.001
                );
                let (pixels, width, height) = fixture.render();
                assert_eq!(width, (surface.get_menu_width() * scale).ceil() as u32);
                assert_eq!(height, (surface.get_menu_height() * scale).ceil() as u32);
                fixture.assert_only_row(label);
                let row = fixture.row(label);
                let origin = row.absolute_position();
                let size = row.size();
                assert!((origin.x - margin - 8.0).abs() < 0.001);
                assert!((origin.y - margin - 8.0).abs() < 0.001);
                assert!((size.height - row_height).abs() < 0.001);
                assert!(origin.x + size.width <= surface.get_menu_width() - margin - 8.0 + 0.001);
                let middle_y = origin.y + size.height / 2.0;
                assert_eq!(
                    sample(&pixels, width, scale, margin + 2.0, middle_y),
                    background
                );
                assert!(
                    pixels
                        .iter()
                        .any(|pixel| [pixel.r, pixel.g, pixel.b] != background)
                );
                assert!(
                    fixture.take_requests().is_empty(),
                    "opening/rendering/focus never changes favorites"
                );

                press(&fixture.menu, Key::Home);
                let (focused, width, _) = fixture.render();
                let accent = surface
                    .global::<SeelenPalette>()
                    .get_accent()
                    .color()
                    .to_argb_u8();
                let accent = [accent.red, accent.green, accent.blue];
                assert_eq!(
                    sample(&focused, width, scale, origin.x - 3.0, middle_y),
                    accent
                );
                assert_eq!(
                    sample(&focused, width, scale, origin.x - 1.0, middle_y),
                    background,
                    "keyboard outline preserves its external two-pixel gap"
                );
                let middle_x = origin.x + size.width / 2.0;
                for y in [origin.y - 3.0, origin.y + size.height + 3.0] {
                    assert_eq!(
                        sample(&focused, width, scale, middle_x, y),
                        accent,
                        "single-row outline fits both viewport gutters"
                    );
                }
                assert!(
                    !fixture.window.draw_if_needed(|renderer| {
                        let mut unchanged = focused.clone();
                        renderer.render(&mut unchanged, width as usize);
                    }),
                    "settled focus has no idle redraw loop"
                );
                assert!(fixture.take_requests().is_empty());
                press(&fixture.menu, Key::Return);
                assert_eq!(
                    fixture.take_requests(),
                    vec![("opaque app key".into(), !favorite)]
                );
                assert!(!fixture.menu.is_open());
            }
        }
    }
}

#[test]
fn launcher_app_menu_real_label_metrics_grow_and_shrink_without_phantom_ax_rows() {
    let fixture = Fixture::new();
    for scale in [1.0, 2.0] {
        for (favorite, label) in [(false, "Pin"), (true, "Unpin")] {
            let tokens = fixture.surface().global::<PopoverTokens>();
            tokens.set_font_size(12.8);
            let minimum = 200.0 + 2.0 * tokens.get_shadow_margin();
            fixture.show(
                favorite,
                scale,
                DockContext::new(-4096, -2160, 4096, 2160, false).unwrap(),
            );
            assert!((fixture.surface().get_menu_width() - minimum).abs() < 0.001);
            // Short real labels require a genuinely large theme font to exceed
            // the minimum. Width comes from the installed native font metrics.
            tokens.set_font_size(192.0);
            let preferred = fixture.surface().get_menu_width();
            assert!(
                preferred > minimum,
                "{label} must grow from measured text, not a character estimate"
            );
            fixture.show(
                favorite,
                scale,
                DockContext::new(-4096, -2160, 4096, 2160, false).unwrap(),
            );
            fixture.render();
            fixture.assert_only_row(label);
            let row = fixture.row(label);
            assert!(row.size().width > 184.0);
            assert!(
                row.absolute_position().x + row.size().width
                    <= preferred - tokens.get_shadow_margin() - 8.0 + 0.001
            );
            assert!(fixture.take_requests().is_empty());
            tokens.set_font_size(12.8);
            assert!(
                (fixture.surface().get_menu_width() - minimum).abs() < 0.001,
                "font changes must return to the shared minimum, not retain a stale width"
            );
            fixture.menu.hide();
        }
    }
}

#[test]
fn launcher_app_menu_tiny_negative_viewport_keeps_native_keyboard_action_reachable() {
    let fixture = Fixture::new();
    for scale in [1.0, 2.0] {
        for (favorite, label) in [(false, "Pin"), (true, "Unpin")] {
            let width = (140.0 * scale) as u32;
            let height = (60.0 * scale) as u32;
            fixture.show(
                favorite,
                scale,
                DockContext::new(-800, -600, width, height, false).unwrap(),
            );
            let natural_width = fixture.surface().get_menu_width();
            let natural_height = fixture.surface().get_menu_height();
            let (pixels, actual_width, actual_height) = fixture.render();
            assert_eq!((actual_width, actual_height), (width, height));
            assert!(natural_width * scale > width as f32);
            assert!(natural_height * scale > height as f32);
            assert_eq!(
                sample(&pixels, width, scale, 0.0, 0.0),
                [0, 0, 0],
                "clipped opaque body does not leak into the transparent window corner"
            );
            assert!(fixture.take_requests().is_empty());
            press(&fixture.menu, Key::End);
            fixture.render();
            fixture.assert_only_row(label);
            let row = fixture.row(label);
            let origin = row.absolute_position();
            let size = row.size();
            let center =
                LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
            assert!(center.x >= 0.0 && center.x < width as f32 / scale);
            assert!(
                center.y >= 0.0 && center.y < height as f32 / scale,
                "the genuine keyboard-focused row remains inside the clipped native viewport"
            );
            assert_eq!(fixture.surface().get_menu_width(), natural_width);
            assert_eq!(fixture.surface().get_menu_height(), natural_height);
            assert!(
                fixture.take_requests().is_empty(),
                "focus is not activation"
            );
            press(&fixture.menu, Key::Space);
            assert_eq!(
                fixture.take_requests(),
                vec![("opaque app key".into(), !favorite)]
            );
            assert!(!fixture.menu.is_open());
        }
    }
}

#[test]
fn launcher_app_menu_real_ax_and_pointer_actions_preserve_exact_favorite_intent() {
    let fixture = Fixture::new();
    for scale in [1.0, 2.0] {
        for (favorite, label) in [(false, "Pin"), (true, "Unpin")] {
            for accessibility in [true, false] {
                fixture.show(
                    favorite,
                    scale,
                    DockContext::new(-1920, 0, 1920, 1080, false).unwrap(),
                );
                fixture.render();
                fixture.assert_only_row(label);
                if accessibility {
                    fixture.row(label).invoke_accessible_default_action();
                } else {
                    click_row(&fixture.menu, label);
                }
                assert_eq!(
                    fixture.take_requests(),
                    vec![("opaque app key".into(), !favorite)]
                );
                assert!(!fixture.menu.is_open());
            }
        }
    }
}
