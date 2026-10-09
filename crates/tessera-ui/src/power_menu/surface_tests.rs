// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::source_measure_tests as source_measure;

use crate::generated::{PopoverMotion, PopoverTokens, PowerMenuAction, PowerMenuSurface};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

const ACTIONS: [(&str, &str, PowerMenuAction); 6] = [
    ("lock", "Lock session", PowerMenuAction::LockSession),
    ("log-out", "Log out", PowerMenuAction::LogOut),
    ("power-off", "Power off", PowerMenuAction::PowerOff),
    ("reboot", "Reboot", PowerMenuAction::Reboot),
    ("suspend", "Suspend", PowerMenuAction::Suspend),
    ("hibernate", "Hibernate", PowerMenuAction::Hibernate),
];
const UPDATES: &str = "There's pending system updates. Install now?";

struct Surface {
    ui: PowerMenuSurface,
    requests: Rc<RefCell<Vec<PowerMenuAction>>>,
    choices: Rc<RefCell<Vec<bool>>>,
}

impl Surface {
    fn new(width: f32, height: f32) -> Self {
        i_slint_backend_testing::init_no_event_loop();
        Self::on_platform(width, height)
    }

    fn on_platform(width: f32, height: f32) -> Self {
        let ui = PowerMenuSurface::new().unwrap();
        ui.global::<PopoverMotion>().set_enabled(false);
        ui.set_selected_width(width);
        ui.set_selected_height(height);
        ui.window().set_size(LogicalSize::new(width, height));
        ui.invoke_set_presentation_opacity(1.0);
        let requests = Rc::new(RefCell::new(Vec::new()));
        let recorded = requests.clone();
        ui.on_action_requested(move |action| recorded.borrow_mut().push(action));
        let choices = Rc::new(RefCell::new(Vec::new()));
        let recorded = choices.clone();
        ui.on_updates_choice_changed(move |choice| recorded.borrow_mut().push(choice));
        ui.show().unwrap();
        ui.invoke_focus_content();
        Self {
            ui,
            requests,
            choices,
        }
    }

    fn element(&self, id: &str) -> ElementHandle {
        ElementHandle::find_by_element_id(&self.ui, &format!("PowerMenuSurface::{id}"))
            .next()
            .unwrap_or_else(|| panic!("genuine generated {id}"))
    }

    fn pending_measure(&self) -> source_measure::PendingTextMeasure {
        let typography = self.ui.global::<PopoverTokens>();
        source_measure::measure_pending_text(
            self.ui.get_source_font_size(),
            typography.get_font_family(),
            typography.get_line_height(),
            self.ui.get_metric_scale(),
            self.ui.window().scale_factor(),
        )
    }

    fn separator_offset(&self) -> f32 {
        self.element("separator").absolute_position().y - self.element("body").absolute_position().y
    }

    fn key(&self, key: Key) {
        self.ui
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        self.ui
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }

    fn click(&self, element: &ElementHandle) {
        let origin = element.absolute_position();
        let size = element.size();
        let position =
            LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        self.ui
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        self.ui
            .window()
            .dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
        self.ui
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        self.ui.hide().unwrap();
    }
}

fn close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.1, "{actual} != {expected}");
}

#[test]
fn six_surface_genuine_pointer_buttons_emit_exact_row_major_actions() {
    let surface = Surface::new(800.0, 700.0);
    for (id, label, action) in ACTIONS {
        let tile = surface.element(id);
        assert_eq!(tile.accessible_role(), Some(AccessibleRole::Button));
        assert_eq!(tile.accessible_label().as_deref(), Some(label));
        assert_eq!(tile.accessible_enabled(), Some(true));
        surface.click(&tile);
        assert_eq!(surface.requests.borrow().last(), Some(&action));
    }
    assert_eq!(
        *surface.requests.borrow(),
        ACTIONS.map(|(_, _, action)| action)
    );
}

#[test]
fn six_surface_tab_return_requires_fresh_press_and_space_requires_release() {
    let surface = Surface::new(800.0, 700.0);
    for (_, _, action) in ACTIONS {
        surface.key(Key::Tab);
        let before = surface.requests.borrow().len();
        surface.ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Return.into(),
        });
        assert_eq!(surface.requests.borrow().len(), before + 1);
        assert_eq!(surface.requests.borrow().last(), Some(&action));
        surface
            .ui
            .window()
            .dispatch_event(WindowEvent::KeyPressRepeated {
                text: Key::Return.into(),
            });
        assert_eq!(
            surface.requests.borrow().len(),
            before + 1,
            "held Return never repeats a command"
        );
        surface
            .ui
            .window()
            .dispatch_event(WindowEvent::KeyReleased {
                text: Key::Return.into(),
            });
        surface.key(Key::Return);
        assert_eq!(surface.requests.borrow().len(), before + 2);
        surface.ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Space.into(),
        });
        surface
            .ui
            .window()
            .dispatch_event(WindowEvent::KeyPressRepeated {
                text: Key::Space.into(),
            });
        assert_eq!(
            surface.requests.borrow().len(),
            before + 2,
            "Space press only arms"
        );
        surface
            .ui
            .window()
            .dispatch_event(WindowEvent::KeyReleased {
                text: Key::Space.into(),
            });
        assert_eq!(surface.requests.borrow().len(), before + 3);
        surface
            .ui
            .window()
            .dispatch_event(WindowEvent::KeyReleased {
                text: Key::Space.into(),
            });
        assert_eq!(
            surface.requests.borrow().len(),
            before + 3,
            "duplicate release cannot replay"
        );
    }
}

#[test]
fn six_surface_busy_disables_every_button_and_cancels_armed_space() {
    let surface = Surface::new(800.0, 700.0);
    surface.key(Key::Tab);
    surface.ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Space.into(),
    });
    surface.ui.set_lock_busy(true);
    surface
        .ui
        .window()
        .dispatch_event(WindowEvent::KeyReleased {
            text: Key::Space.into(),
        });
    for (id, _, _) in ACTIONS {
        let tile = surface.element(id);
        assert_eq!(tile.accessible_enabled(), Some(false));
        surface.click(&tile);
        tile.invoke_accessible_default_action();
    }
    surface.key(Key::Return);
    assert!(surface.requests.borrow().is_empty());
    surface.ui.set_lock_busy(false);
    surface.ui.invoke_focus_content();
    surface.key(Key::Tab);
    surface
        .ui
        .window()
        .dispatch_event(WindowEvent::KeyReleased {
            text: Key::Space.into(),
        });
    assert!(
        surface.requests.borrow().is_empty(),
        "enable cannot revive an old gesture"
    );
    surface.key(Key::Space);
    assert_eq!(*surface.requests.borrow(), [PowerMenuAction::LockSession]);
    surface.ui.set_action_enabled(false);
    for (id, _, _) in ACTIONS {
        assert_eq!(surface.element(id).accessible_enabled(), Some(false));
    }
}

#[test]
fn six_surface_pending_choice_is_native_pointer_space_tab_control_not_command() {
    let surface = Surface::new(800.0, 700.0);
    surface.ui.set_updates_known_pending(true);
    assert!(
        surface.ui.get_install_updates(),
        "new component source default"
    );
    let checkbox = surface.element("updates-choice");
    assert_eq!(checkbox.accessible_role(), Some(AccessibleRole::Checkbox));
    assert_eq!(checkbox.accessible_label().as_deref(), Some(UPDATES));
    assert_eq!(checkbox.accessible_checkable(), Some(true));
    assert_eq!(checkbox.accessible_checked(), Some(true));
    assert_eq!(
        surface.element("power-off").accessible_label().as_deref(),
        Some("Update and shut down")
    );
    assert_eq!(
        surface.element("reboot").accessible_label().as_deref(),
        Some("Update and restart")
    );
    surface.click(&checkbox);
    assert!(!surface.ui.get_install_updates());
    assert_eq!(*surface.choices.borrow(), [false]);
    surface.ui.invoke_focus_content();
    surface.key(Key::Tab);
    surface.key(Key::Space);
    assert!(surface.ui.get_install_updates());
    assert_eq!(*surface.choices.borrow(), [false, true]);
    assert!(surface.requests.borrow().is_empty());
    surface.ui.invoke_focus_content();
    surface.key(Key::Tab);
    surface.key(Key::Space);
    assert!(
        !surface.ui.get_install_updates(),
        "checkbox precedes grid in native Tab order"
    );
    surface.key(Key::Tab);
    surface.key(Key::Return);
    assert_eq!(*surface.requests.borrow(), [PowerMenuAction::LockSession]);
    surface.ui.set_lock_busy(true);
    assert_eq!(checkbox.accessible_enabled(), Some(false));
    surface.click(&checkbox);
    checkbox.invoke_accessible_default_action();
    assert!(!surface.ui.get_install_updates());
    assert_eq!(*surface.choices.borrow(), [false, true, false]);
}

#[test]
fn six_surface_unknown_status_has_no_choice_and_hidden_pending_retains_choice() {
    let surface = Surface::new(800.0, 700.0);
    assert!(
        ElementHandle::find_by_accessible_label(&surface.ui, UPDATES)
            .next()
            .is_none()
    );
    surface
        .ui
        .set_updates_status("Update status unavailable".into());
    assert!(
        ElementHandle::find_by_accessible_label(&surface.ui, UPDATES)
            .next()
            .is_none()
    );
    assert!(
        ElementHandle::find_by_accessible_label(&surface.ui, "Update status unavailable")
            .next()
            .is_some()
    );
    surface.ui.set_updates_known_pending(true);
    surface.click(&surface.element("updates-choice"));
    assert!(!surface.ui.get_install_updates());
    surface.ui.set_updates_known_pending(false);
    assert!(!surface.ui.get_install_updates());
    assert!(
        ElementHandle::find_by_accessible_label(&surface.ui, UPDATES)
            .next()
            .is_none()
    );
    for (id, label, _) in ACTIONS {
        assert_eq!(
            surface.element(id).accessible_label().as_deref(),
            Some(label)
        );
        assert_eq!(surface.element(id).accessible_enabled(), Some(true));
    }
    surface.ui.set_updates_known_pending(true);
    assert_eq!(
        surface.element("updates-choice").accessible_checked(),
        Some(false)
    );
    assert_eq!(
        surface.element("power-off").accessible_label().as_deref(),
        Some("Power off")
    );
    assert!(surface.requests.borrow().is_empty());
}

#[test]
fn six_surface_source_geometry_scales_body_grid_header_separator_and_switch() {
    let surface = Surface::new(1000.0, 900.0);
    for scale in [1.0, 1.375, 2.0] {
        surface.ui.set_metric_scale(scale);
        let body = surface.element("body");
        close(body.size().width, 460.0 * scale); // actual396 + two32 shadow halos
        close(surface.element("header").size().height, 48.0 * scale);
        close(surface.element("separator").size().height, scale);
        close(surface.element("separator").size().width, 348.0 * scale);
        let first = surface.element("lock").absolute_position();
        for (index, (id, _, _)) in ACTIONS.into_iter().enumerate() {
            let tile = surface.element(id);
            close(tile.size().width, 100.0 * scale);
            close(tile.size().height, 100.0 * scale);
            close(
                tile.absolute_position().x - first.x,
                (index % 3) as f32 * 124.0 * scale,
            );
            close(
                tile.absolute_position().y - first.y,
                (index / 3) as f32 * 124.0 * scale,
            );
        }
        close(
            first.x - surface.element("separator").absolute_position().x,
            0.0,
        );
        close(
            first.y - surface.element("separator").absolute_position().y,
            25.0 * scale,
        );
        surface.ui.set_updates_known_pending(true);
        close(surface.element("switch-track").size().width, 36.0 * scale);
        close(surface.element("switch-track").size().height, 18.0 * scale);
        close(surface.element("switch-thumb").size().width, 14.0 * scale);
        let track = surface.element("switch-track").absolute_position();
        let thumb = surface.element("switch-thumb").absolute_position();
        close(thumb.x - track.x, 20.0 * scale);
        close(thumb.y - track.y, 2.0 * scale);
        surface.ui.set_install_updates(false);
        close(
            surface.element("switch-thumb").absolute_position().x - track.x,
            2.0 * scale,
        );
        surface.ui.set_install_updates(true);
        surface.ui.set_updates_known_pending(false);
    }
}

#[test]
fn six_surface_tiny_viewport_native_tab_scroll_reveals_checkbox_and_all_six() {
    let surface = Surface::new(160.0, 160.0);
    surface.ui.set_updates_known_pending(true);
    surface.ui.invoke_focus_content();
    surface.key(Key::Tab);
    let checkbox = surface.element("updates-choice");
    // An oversized associated row reveals its leading stock focus anchor.
    let check_origin = checkbox.absolute_position();
    assert!(check_origin.x >= 0.0 && check_origin.x < 160.0);
    assert!(check_origin.y >= 0.0 && check_origin.y + checkbox.size().height <= 160.0);
    surface.key(Key::Space);
    assert!(!surface.ui.get_install_updates());
    for (id, _, action) in ACTIONS {
        surface.key(Key::Tab);
        let tile = surface.element(id);
        let origin = tile.absolute_position();
        let size = tile.size();
        assert!(
            origin.x >= 0.0 && origin.y >= 0.0,
            "{id} starts within selected viewport"
        );
        assert!(
            origin.x + size.width <= 160.0 && origin.y + size.height <= 160.0,
            "{id} tail remains reachable by actual ScrollView reveal"
        );
        surface.key(Key::Return);
        assert_eq!(surface.requests.borrow().last(), Some(&action));
    }
    surface.ui.invoke_focus_content();
    surface.key(Key::Tab);
    surface.key(Key::Space);
    assert!(
        surface.ui.get_install_updates(),
        "Tab wrap can return to the choice"
    );
}

#[test]
fn six_surface_software_paints_body_and_real_tiles_without_certifying_native_transform() {
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    use slint::platform::{Platform, WindowAdapter};
    struct SoftwarePlatform(Rc<MinimalSoftwareWindow>);
    impl Platform for SoftwarePlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(SoftwarePlatform(window.clone()))).unwrap();
    let surface = Surface::on_platform(800.0, 700.0);
    let mut pixels = vec![slint::Rgb8Pixel::default(); 800 * 700];
    assert!(window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 800);
    }));
    assert!(
        pixels.windows(2).any(|pair| pair[0] != pair[1]),
        "body and labels genuinely paint"
    );
    for (id, _, _) in ACTIONS {
        close(surface.element(id).size().width, 100.0);
    }
    let tile = surface.element("lock");
    let before = tile.size();
    let origin = tile.absolute_position();
    surface
        .ui
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(origin.x + 50.0, origin.y + 50.0),
        });
    close(tile.size().width, before.width);
    close(tile.size().height, before.height);
    // Software ignores nativeTransform scale/shadow fidelity. These assertions
    // certify stable layout, not the GL1.05/.95 paint or blurred shadow.
    assert!(window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 800);
    }));
}

#[test]
fn six_surface_switch_motion_obeys_permission_without_rewriting_choice() {
    let surface = Surface::new(800.0, 700.0);
    surface.ui.set_updates_known_pending(true);
    surface.ui.global::<PopoverMotion>().set_enabled(true);
    surface.ui.set_install_updates(false);
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(350));
    let track = surface.element("switch-track").absolute_position();
    close(
        surface.element("switch-thumb").absolute_position().x - track.x,
        2.0,
    );
    assert!(!surface.ui.get_install_updates());
    surface.ui.global::<PopoverMotion>().set_enabled(false);
    surface.ui.set_install_updates(true);
    close(
        surface.element("switch-thumb").absolute_position().x - track.x,
        20.0,
    );
    assert!(
        surface.choices.borrow().is_empty(),
        "projection is not genuine user input"
    );
}

#[test]
fn six_surface_update_badges_exist_only_for_known_pending_selected_off_and_reboot() {
    let surface = Surface::new(800.0, 700.0);
    let badges =
        || ElementHandle::find_by_element_id(&surface.ui, "PowerActionTile::update-badge").count();
    assert_eq!(badges(), 0);
    surface.ui.set_updates_known_pending(true);
    assert_eq!(badges(), 2);
    surface.ui.set_install_updates(false);
    assert_eq!(badges(), 0);
    surface.ui.set_install_updates(true);
    assert_eq!(badges(), 2);
    surface.ui.set_updates_known_pending(false);
    assert_eq!(badges(), 0);
    assert!(
        surface.ui.get_install_updates(),
        "hiding the native choice never rewrites it"
    );
}

#[test]
fn six_surface_header_uses_only_supplied_real_name_and_omits_unknown_profile() {
    let surface = Surface::new(800.0, 700.0);
    assert_eq!(
        surface.element("greeting").accessible_label().as_deref(),
        Some("User identity unavailable")
    );
    surface.ui.set_user_name("Actual account".into());
    assert_eq!(
        surface.element("greeting").accessible_label().as_deref(),
        Some("Goodbye Actual account")
    );
    assert!(
        ElementHandle::find_by_element_id(&surface.ui, "PowerMenuSurface::avatar")
            .next()
            .is_none()
    );
    assert!(
        ElementHandle::find_by_element_id(&surface.ui, "PowerMenuSurface::email")
            .next()
            .is_none()
    );
    close(surface.element("header").size().height, 48.0);
}

#[test]
fn six_surface_pending_row_uses_independent_source_text_height_and_switch_minimum() {
    let surface = Surface::new(800.0, 700.0);
    close(surface.ui.get_source_font_size(), 16.0);
    let separator_without_row = surface.separator_offset();
    let nominal = surface.pending_measure();
    close(nominal.span_width, 292.0);
    if nominal.unwrapped_width <= nominal.span_width {
        close(nominal.text_height, nominal.single_line_height);
    } else {
        assert!(
            nominal.text_height > nominal.single_line_height,
            "full source prompt really wraps in this native font"
        );
    }
    surface.ui.set_updates_known_pending(true);
    let row = surface.element("updates-row");
    let check_native_extent = || {
        let checkbox = surface.element("updates-choice");
        close(checkbox.size().width, row.size().width);
        assert!(
            (checkbox.size().height - row.size().height).abs() < 0.1,
            "stock checkbox allocated height {} must match measured row {}",
            checkbox.size().height,
            row.size().height,
        );
        close(checkbox.absolute_position().x, row.absolute_position().x);
        close(checkbox.absolute_position().y, row.absolute_position().y);
    };
    check_native_extent();
    close(row.size().height, nominal.row_height);
    close(
        surface.element("updates-label").size().width,
        nominal.span_width,
    );
    close(
        surface.element("updates-label").size().height,
        nominal.row_height,
    );
    close(
        surface.separator_offset() - separator_without_row,
        nominal.row_height + 24.0,
    );
    close(
        surface.element("body").size().height,
        433.0 + 24.0 + nominal.row_height,
    );
    // 475px is the source flow only when the actual full text fits the 18px switch.
    if nominal.text_height <= 18.0 {
        close(surface.element("body").size().height, 475.0);
    }
    surface.ui.global::<PopoverTokens>().set_font_size(28.0);
    close(surface.ui.get_source_font_size(), 16.0);
    close(row.size().height, nominal.row_height);
    surface.ui.set_source_font_size(28.0);
    let enlarged = surface.pending_measure();
    assert!(
        enlarged.text_height > nominal.text_height,
        "real source-font enlargement expands the independently shaped text"
    );
    close(row.size().height, enlarged.row_height);
    close(
        surface.element("updates-label").size().height,
        enlarged.row_height,
    );
    close(
        surface.separator_offset() - separator_without_row,
        enlarged.row_height + 24.0,
    );
    check_native_extent();
    surface.ui.invoke_focus_content();
    surface.key(Key::Tab);
    surface.key(Key::Space);
    assert!(
        !surface.ui.get_install_updates(),
        "measured layout preserves genuine native choice input"
    );
    surface.ui.set_updates_known_pending(false);
    let status = "Update status unavailable. The read-only system status service did not return a usable observation; the six native commands remain available.";
    surface.ui.set_updates_status(status.into());
    let typography = surface.ui.global::<PopoverTokens>();
    let warning_height = source_measure::measure_source_text(
        status,
        348.0,
        surface.ui.get_source_font_size(),
        typography.get_font_family(),
        typography.get_line_height(),
        surface.ui.get_metric_scale(),
        surface.ui.window().scale_factor(),
    );
    close(
        surface.element("updates-message").size().height,
        warning_height,
    );
    close(
        surface.separator_offset() - separator_without_row,
        warning_height + 24.0,
    );
}
