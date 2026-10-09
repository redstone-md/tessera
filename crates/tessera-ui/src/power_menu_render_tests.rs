// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Generated SDK input and software pixels only. Both callbacks record intent;
//! there is no controller, host, native power invocation or session-state claim.

use std::cell::RefCell;
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, Rgb8Pixel};

use crate::generated::{FocusTokens, PowerMenuAction, PowerMenuSurface};
use crate::theme::{PresentationTheme, ThemedComponent};

#[derive(Debug, PartialEq)]
enum Request {
    Action(PowerMenuAction),
    Hide,
}

struct Fixture {
    window: Rc<MinimalSoftwareWindow>,
    surface: PowerMenuSurface,
    requests: Rc<RefCell<Vec<Request>>>,
}

impl Fixture {
    fn new() -> Self {
        struct TestPlatform {
            primary: Rc<MinimalSoftwareWindow>,
            primary_available: std::cell::Cell<bool>,
        }
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                if self.primary_available.replace(false) {
                    Ok(self.primary.clone())
                } else {
                    Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
                }
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform {
            primary: window.clone(),
            primary_available: std::cell::Cell::new(true),
        }))
        .unwrap();
        let surface = PowerMenuSurface::new().unwrap();
        surface.apply_presentation_theme(PresentationTheme::seelen_reference(
            slint::language::ColorScheme::Light,
        ));
        surface.set_action_enabled(true);
        let requests = Rc::new(RefCell::new(Vec::new()));
        let recorded = requests.clone();
        surface.on_action_requested(move |action| {
            recorded.borrow_mut().push(Request::Action(action));
        });
        let recorded = requests.clone();
        surface.on_hide_requested(move || recorded.borrow_mut().push(Request::Hide));
        surface.show().unwrap();
        window
            .window()
            .dispatch_event(WindowEvent::WindowActiveChanged(true));
        let fixture = Self {
            window,
            surface,
            requests,
        };
        fixture.project(1.0, 1.0, [0.0, 0.0, 960.0, 720.0]);
        fixture.render(960, 720);
        fixture
    }

    // These are already-projected logical monitor bounds, not an actor/display
    // implementation. Root DPI always enters through the actual SDK event.
    fn project(&self, root_scale: f32, metric_scale: f32, selected: [f32; 4]) {
        self.event(WindowEvent::ScaleFactorChanged {
            scale_factor: root_scale,
        });
        self.surface.set_metric_scale(metric_scale);
        self.surface
            .global::<FocusTokens>()
            .set_outline_width(2.0 * metric_scale);
        self.surface
            .global::<FocusTokens>()
            .set_outline_offset(2.0 * metric_scale);
        self.surface.set_selected_x(selected[0]);
        self.surface.set_selected_y(selected[1]);
        self.surface.set_selected_width(selected[2]);
        self.surface.set_selected_height(selected[3]);
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

    fn one(query: ElementQuery) -> ElementHandle {
        let elements = query.find_all();
        assert_eq!(elements.len(), 1, "expected exactly one genuine element");
        elements.into_iter().next().unwrap()
    }

    fn by_id(&self, id: &str) -> ElementHandle {
        Self::one(
            ElementQuery::from_root(&self.surface).match_id(format!("PowerMenuSurface::{id}")),
        )
    }

    fn lock(&self) -> ElementHandle {
        self.by_id("lock")
    }

    fn tile_child(&self, tile: &str, child: &str) -> ElementHandle {
        Self::one(
            self.by_id(tile)
                .query_descendants()
                .match_id(format!("PowerActionTile::{child}")),
        )
    }

    fn center(element: &ElementHandle) -> LogicalPosition {
        let origin = element.absolute_position();
        let size = element.size();
        assert!(size.width > 0.0 && size.height > 0.0);
        LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
    }

    fn event(&self, event: WindowEvent) {
        self.window.window().dispatch_event(event);
    }

    fn move_to(&self, position: LogicalPosition) {
        self.event(WindowEvent::PointerMoved { position });
    }

    fn press(&self, position: LogicalPosition) {
        let size = self.window.window().size();
        let scale = self.window.window().scale_factor();
        assert!(
            position.x >= 0.0
                && position.y >= 0.0
                && position.x * scale < size.width as f32
                && position.y * scale < size.height as f32,
            "input point must be in the actual native viewport"
        );
        self.move_to(position);
        self.event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
    }

    fn release(&self, position: LogicalPosition) {
        self.event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    }

    fn click_at(&self, position: LogicalPosition) {
        self.press(position);
        self.release(position);
    }

    fn key_press(&self, key: Key) {
        self.event(WindowEvent::KeyPressed { text: key.into() });
    }

    fn key_release(&self, key: Key) {
        self.event(WindowEvent::KeyReleased { text: key.into() });
    }

    fn key(&self, key: Key) {
        self.key_press(key);
        self.key_release(key);
    }

    fn repeat(&self, key: Key) {
        self.event(WindowEvent::KeyPressRepeated { text: key.into() });
    }

    fn take(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }

    fn expect_lock(&self) {
        assert_eq!(
            self.take(),
            vec![Request::Action(PowerMenuAction::LockSession)]
        );
    }
}

fn actions() -> [(&'static str, &'static str, PowerMenuAction); 6] {
    [
        ("lock", "Lock session", PowerMenuAction::LockSession),
        ("log-out", "Log out", PowerMenuAction::LogOut),
        ("power-off", "Power off", PowerMenuAction::PowerOff),
        ("reboot", "Reboot", PowerMenuAction::Reboot),
        ("suspend", "Suspend", PowerMenuAction::Suspend),
        ("hibernate", "Hibernate", PowerMenuAction::Hibernate),
    ]
}

fn near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.05,
        "expected {expected}, got {actual}"
    );
}

fn sample(pixels: &[Rgb8Pixel], width: u32, scale: f32, position: LogicalPosition) -> [u8; 3] {
    let x = (position.x * scale).floor() as usize;
    let y = (position.y * scale).floor() as usize;
    assert!(x < width as usize && y < pixels.len() / width as usize);
    let pixel = pixels[y * width as usize + x];
    [pixel.r, pixel.g, pixel.b]
}

// Source paint AABB, separate from Slint's stable layout bounds. Software
// 1.18.1 ignores transform-scale (and shadows), so its unscaled footprint is
// included explicitly rather than pretending these pixels prove native scale.
#[derive(Clone, Copy)]
struct ActionPaint {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl ActionPaint {
    fn source(origin: LogicalPosition, metric: f32, scale: f32, lift: f32, outline: f32) -> Self {
        let inset = (100.0 * (1.0 - scale) / 2.0 - outline * scale) * metric;
        Self {
            left: origin.x + inset,
            top: origin.y + inset + lift * metric,
            right: origin.x + 100.0 * metric - inset,
            bottom: origin.y + (100.0 + lift) * metric - inset,
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

// Genuine state layers must not repaint the header, scrim or another region.
fn changed_only_in_action(
    before: &[Rgb8Pixel],
    after: &[Rgb8Pixel],
    width: u32,
    root_scale: f32,
    bounds: ActionPaint,
) -> usize {
    assert_eq!(before.len(), after.len());
    let mut changed = 0;
    for (index, (a, b)) in before.iter().zip(after).enumerate() {
        if (a.r, a.g, a.b) == (b.r, b.g, b.b) {
            continue;
        }
        let x = (index % width as usize) as f32 / root_scale;
        let y = (index / width as usize) as f32 / root_scale;
        // Only pixel-edge quantization, not an arbitrary logical margin.
        let pixel = 1.0 / root_scale;
        assert!(
            x >= bounds.left - pixel
                && y >= bounds.top - pixel
                && x <= bounds.right + pixel
                && y <= bounds.bottom + pixel,
            "state paint escaped source action AABB union at ({x}, {y})"
        );
        changed += 1;
    }
    assert!(
        changed > 0,
        "state must genuinely paint, not just update metadata"
    );
    changed
}

#[test]
fn power_surface_has_six_typed_pointer_and_ax_actions_without_preview_or_fake_profile_routes() {
    let fixture = Fixture::new();
    fixture.surface.set_user_name("Native fixture user".into());
    fixture.render(960, 720);
    assert_eq!(
        fixture.by_id("greeting").accessible_label().as_deref(),
        Some("Goodbye Native fixture user")
    );
    assert_eq!(
        ElementQuery::from_root(&fixture.surface)
            .match_accessible_role(AccessibleRole::Button)
            .find_all()
            .len(),
        6
    );
    for (id, label, action) in actions() {
        let tile = fixture.by_id(id);
        assert_eq!(tile.accessible_role(), Some(AccessibleRole::Button));
        assert_eq!(tile.accessible_label().as_deref(), Some(label));
        assert_eq!(tile.accessible_enabled(), Some(true));
        fixture.click_at(Fixture::center(&tile));
        assert_eq!(fixture.take(), vec![Request::Action(action)]);
        fixture.move_to(LogicalPosition::new(1.0, 1.0));
        tile.invoke_accessible_default_action();
        assert_eq!(fixture.take(), vec![Request::Action(action)]);
    }
    let suspend_image_size = fixture.surface.get_suspend_icon().size();
    assert_eq!(
        (suspend_image_size.width, suspend_image_size.height),
        (0, 0)
    );
    let suspend_icon = fixture.tile_child("suspend", "icon");
    near(suspend_icon.size().width, 25.0);
    near(suspend_icon.size().height, 25.0);
    assert!(
        ElementQuery::from_root(&fixture.surface)
            .match_accessible_role(AccessibleRole::Image)
            .find_all()
            .is_empty(),
        "missing licensed moon and profile artwork must not become fake images"
    );
    for absent in ["Power menu", "Confirm", "Exit", "native@example.com"] {
        assert!(
            ElementHandle::find_by_accessible_label(&fixture.surface, absent)
                .next()
                .is_none(),
            "no invented route or profile detail: {absent}"
        );
    }
    fixture.move_to(LogicalPosition::new(1.0, 1.0));
    assert!(fixture.take().is_empty());

    // A body click is consumed; it is not an outside dismiss or a Lock request.
    let body = fixture.by_id("body");
    let origin = body.absolute_position();
    let size = body.size();
    fixture.click_at(LogicalPosition::new(
        origin.x + size.width - 44.0,
        origin.y + 72.0,
    ));
    assert!(fixture.take().is_empty());
    fixture.click_at(LogicalPosition::new(4.0, 4.0));
    assert_eq!(fixture.take(), vec![Request::Hide]);
}

#[test]
fn power_surface_tab_shift_tab_fresh_return_space_and_escape_use_real_focus_and_repeat_events() {
    let fixture = Fixture::new();
    fixture.surface.invoke_focus_content();
    fixture.key(Key::Tab);
    assert!(fixture.take().is_empty());
    fixture.key_press(Key::Return);
    fixture.expect_lock();
    for _ in 0..3 {
        fixture.repeat(Key::Return);
    }
    fixture.key_release(Key::Return);
    assert!(
        fixture.take().is_empty(),
        "held Return/release must not replay Lock"
    );
    fixture.key(Key::Return);
    fixture.expect_lock();

    fixture.surface.invoke_focus_content();
    fixture.key_press(Key::Shift);
    fixture.key(Key::Tab);
    fixture.key_release(Key::Shift);
    assert!(
        fixture.take().is_empty(),
        "reverse navigation only changes focus"
    );
    fixture.key_press(Key::Space);
    for _ in 0..3 {
        fixture.repeat(Key::Space);
    }
    assert!(
        fixture.take().is_empty(),
        "Space activates only on armed release"
    );
    fixture.key_release(Key::Space);
    assert_eq!(
        fixture.take(),
        vec![Request::Action(PowerMenuAction::Hibernate)]
    );
    fixture.key_release(Key::Space);
    assert!(
        fixture.take().is_empty(),
        "a second release has no fresh gesture"
    );
    fixture.key(Key::Escape);
    assert_eq!(fixture.take(), vec![Request::Hide]);
    fixture.surface.invoke_focus_content();
    fixture.key(Key::Escape);
    assert_eq!(fixture.take(), vec![Request::Hide]);
}

#[test]
fn power_surface_disabled_and_busy_reject_pointer_ax_keys_and_cancel_armed_space() {
    let fixture = Fixture::new();
    for (enabled, busy) in [(false, false), (true, true), (false, true)] {
        fixture.surface.set_action_enabled(true);
        fixture.surface.set_lock_busy(false);
        fixture.surface.invoke_focus_content();
        fixture.key(Key::Tab);
        fixture.key_press(Key::Space);
        fixture.surface.set_action_enabled(enabled);
        fixture.surface.set_lock_busy(busy);
        fixture.key_release(Key::Space);
        fixture.render(960, 720);
        for (id, _, _) in actions() {
            let tile = fixture.by_id(id);
            assert_eq!(tile.accessible_enabled(), Some(false));
            fixture.click_at(Fixture::center(&tile));
            tile.invoke_accessible_default_action();
        }
        fixture.key(Key::Return);
        fixture.repeat(Key::Return);
        fixture.key(Key::Space);
        fixture.repeat(Key::Space);
        fixture.key_release(Key::Space);
        assert!(
            fixture.take().is_empty(),
            "disabled/busy has no command authority"
        );
        fixture.surface.invoke_focus_content();
        fixture.key(Key::Escape);
        assert_eq!(
            fixture.take(),
            vec![Request::Hide],
            "busy does not trap dismissal"
        );
    }
    fixture.surface.set_action_enabled(true);
    fixture.surface.set_lock_busy(false);
    fixture.surface.invoke_focus_content();
    fixture.key(Key::Tab);
    fixture.repeat(Key::Return);
    assert!(
        fixture.take().is_empty(),
        "re-enabling does not admit a held Return"
    );
    fixture.repeat(Key::Space);
    fixture.key_release(Key::Space);
    assert!(
        fixture.take().is_empty(),
        "a repeat cannot revive cancelled Space"
    );
    fixture.key(Key::Space);
    fixture.expect_lock();
}

#[test]
fn power_surface_selected_monitor_center_and_metrics_follow_current_root_not_desktop_center() {
    let fixture = Fixture::new();
    // Physical desktop [-1280,-240 .. 1920,1080], selected [0,0 .. 1920,1080].
    // Literals are independent worked projections of those mixed-origin bounds.
    // Presentation DPI/text scale 1.5 stays physically stable as root DPI changes.
    for (root_scale, metric, selected, center, physical_body_width) in [
        (
            1.0,
            1.0,
            [1280.0, 240.0, 1920.0, 1080.0],
            [2240.0, 780.0],
            460.0,
        ),
        (
            2.0,
            1.0,
            [640.0, 120.0, 960.0, 540.0],
            [1120.0, 390.0],
            920.0,
        ),
        (
            1.0,
            1.5,
            [1280.0, 240.0, 1920.0, 1080.0],
            [2240.0, 780.0],
            690.0,
        ),
        (
            2.0,
            0.75,
            [640.0, 120.0, 960.0, 540.0],
            [1120.0, 390.0],
            690.0,
        ),
        (
            1.25,
            1.2,
            [1024.0, 192.0, 1536.0, 864.0],
            [1792.0, 624.0],
            690.0,
        ),
    ] {
        fixture.project(root_scale, metric, selected);
        fixture.render(3200, 1320);
        let body = fixture.by_id("body");
        let actual = Fixture::center(&body);
        near(actual.x, center[0]);
        near(actual.y, center[1]);
        near(body.size().width * root_scale, physical_body_width);
        near(body.size().height, 433.0 * metric);
        assert!(
            (actual.x - 1600.0 / root_scale).abs() > 100.0,
            "selected full monitor is not the aggregate desktop center"
        );
        let viewport = fixture.by_id("selected-viewport");
        near(viewport.absolute_position().x, selected[0]);
        near(viewport.absolute_position().y, selected[1]);
        near(viewport.size().width, selected[2]);
        near(viewport.size().height, selected[3]);
        let lock = fixture.lock();
        near(lock.size().width, 100.0 * metric);
        near(lock.size().height, 100.0 * metric);
        let icon = fixture.tile_child("lock", "icon");
        near(icon.size().width, 25.0 * metric);
        near(icon.size().height, 25.0 * metric);
        let label = fixture.tile_child("lock", "label");
        near(label.size().height, 16.0 * 1.4 * metric);
        near(
            label.absolute_position().y - icon.absolute_position().y - icon.size().height,
            4.0 * metric,
        );
        assert!(
            fixture.take().is_empty(),
            "DPI/projection never activates or dismisses"
        );
    }
    // Selected left monitor in the same aggregate: genuinely different center.
    fixture.project(1.0, 1.0, [0.0, 0.0, 1280.0, 1024.0]);
    fixture.render(3200, 1320);
    let center = Fixture::center(&fixture.by_id("body"));
    near(center.x, 640.0);
    near(center.y, 512.0);
    assert!(fixture.take().is_empty());
}

#[test]
fn power_surface_light_dark_body_scrim_icon_and_rounded_state_paint_scale_coherently() {
    let fixture = Fixture::new();
    for (scheme, background) in [
        (slint::language::ColorScheme::Light, [252, 252, 252]),
        (slint::language::ColorScheme::Dark, [31, 31, 31]),
    ] {
        for (root_scale, metric, width, height) in [
            (1.0, 1.0, 960, 720),
            (2.0, 1.0, 1920, 1440),
            (1.25, 1.2, 1200, 900),
        ] {
            fixture
                .surface
                .apply_presentation_theme(PresentationTheme::seelen_reference(scheme));
            fixture.project(root_scale, metric, [0.0, 0.0, 960.0, 720.0]);
            fixture.move_to(LogicalPosition::new(1.0, 1.0));
            fixture.surface.invoke_focus_content();
            let idle = fixture.render(width, height);
            let body = fixture.by_id("body");
            let origin = body.absolute_position();
            let size = body.size();
            let body_sample = LogicalPosition::new(
                origin.x + size.width - 44.0 * metric,
                origin.y + 72.0 * metric,
            );
            assert_eq!(sample(&idle, width, root_scale, body_sample), background);
            let scrim = sample(&idle, width, root_scale, LogicalPosition::new(1.0, 1.0));
            assert!(
                scrim.into_iter().all(|channel| (6..=8).contains(&channel)),
                "#12121266 scrim over the fixture's black buffer is about RGB7"
            );
            assert_ne!(
                sample(
                    &idle,
                    width,
                    root_scale,
                    LogicalPosition::new(origin.x + 32.0 * metric, origin.y + 32.0 * metric,)
                ),
                background,
                "source 16px body corner is genuinely rounded"
            );
            // Slint 1.18.1 software draw_box_shadow is intentionally a no-op.
            // Prove reserved space here; shadow pixels require the parent's GL gate.
            assert_eq!(
                sample(
                    &idle,
                    width,
                    root_scale,
                    LogicalPosition::new(origin.x + 28.0 * metric, origin.y + size.height / 2.0,)
                ),
                scrim,
                "reserved shadow space must not become an opaque body extension"
            );
            let icon = fixture.tile_child("lock", "icon");
            let icon_origin = icon.absolute_position();
            let mut ink = 0;
            for y in 0..25 {
                for x in 0..25 {
                    if sample(
                        &idle,
                        width,
                        root_scale,
                        LogicalPosition::new(
                            icon_origin.x + (x as f32 + 0.5) * metric,
                            icon_origin.y + (y as f32 + 0.5) * metric,
                        ),
                    ) != background
                    {
                        ink += 1;
                    }
                }
            }
            assert!(
                ink > 50,
                "the real lock SVG must paint, not an empty image box"
            );
            let lock = fixture.lock();
            let center = Fixture::center(&lock);
            let idle_origin = lock.absolute_position();
            let idle_label = fixture.tile_child("lock", "label").absolute_position();
            near(lock.size().width, 100.0 * metric);
            near(lock.size().height, 100.0 * metric);
            near(icon.size().width, 25.0 * metric);
            near(icon.size().height, 25.0 * metric);
            let idle_bounds = ActionPaint::source(idle_origin, metric, 1.0, 0.0, 0.0);
            let hover_bounds = ActionPaint::source(idle_origin, metric, 1.05, -4.0, 0.0);
            let pressed_bounds = ActionPaint::source(idle_origin, metric, 0.95, 0.0, 0.0);
            fixture.move_to(center);
            let hover = fixture.render(width, height);
            changed_only_in_action(
                &idle,
                &hover,
                width,
                root_scale,
                idle_bounds.union(hover_bounds),
            );
            near(lock.absolute_position().x, idle_origin.x);
            near(lock.absolute_position().y, idle_origin.y - 4.0 * metric);
            near(icon.absolute_position().y, icon_origin.y - 4.0 * metric);
            near(
                fixture.tile_child("lock", "label").absolute_position().y,
                idle_label.y - 4.0 * metric,
            );
            let corner = lock.absolute_position();
            assert_eq!(
                sample(
                    &hover,
                    width,
                    root_scale,
                    LogicalPosition::new(corner.x + metric, corner.y + metric,)
                ),
                background,
                "source 10px tile radius clips the hover corner"
            );
            fixture.press(center);
            let pressed = fixture.render(width, height);
            // Source hover/press union also contains the software renderer's
            // unscaled press box; this is not a native scaled-pixel claim.
            changed_only_in_action(
                &hover,
                &pressed,
                width,
                root_scale,
                hover_bounds.union(pressed_bounds).union(idle_bounds),
            );
            near(lock.absolute_position().y, idle_origin.y);
            near(icon.absolute_position().y, icon_origin.y);
            near(
                fixture.tile_child("lock", "label").absolute_position().y,
                idle_label.y,
            );
            assert!(fixture.take().is_empty(), "pointer down is not Lock");
            fixture.release(center);
            fixture.expect_lock();
            fixture.move_to(LogicalPosition::new(1.0, 1.0));
            fixture.surface.invoke_focus_content();
            let unfocused = fixture.render(width, height);
            let focus_origin = lock.absolute_position();
            let focus_center = Fixture::center(&lock);
            near(focus_origin.x, idle_origin.x);
            near(focus_origin.y, idle_origin.y);
            fixture.key(Key::Tab);
            let focused = fixture.render(width, height);
            changed_only_in_action(
                &unfocused,
                &focused,
                width,
                root_scale,
                ActionPaint::source(focus_origin, metric, 1.0, 0.0, 4.0),
            );
            assert_ne!(
                sample(
                    &unfocused,
                    width,
                    root_scale,
                    LogicalPosition::new(focus_origin.x - 3.0 * metric, focus_center.y,)
                ),
                sample(
                    &focused,
                    width,
                    root_scale,
                    LogicalPosition::new(focus_origin.x - 3.0 * metric, focus_center.y,)
                ),
                "external focus outline remains visible outside the tile"
            );
            assert!(fixture.take().is_empty(), "paint/focus is not activation");
        }
    }
}

#[test]
fn power_surface_tiny_both_axis_viewport_clips_but_focus_reveals_all_six_actions_and_outline() {
    let fixture = Fixture::new();
    for (root_scale, metric, width, height) in [
        (1.0, 1.0, 124, 124),
        (2.0, 1.0, 248, 248),
        (1.25, 1.2, 186, 186),
    ] {
        let logical_extent = 124.0 * metric;
        fixture.project(
            root_scale,
            metric,
            [0.0, 0.0, logical_extent, logical_extent],
        );
        fixture.surface.invoke_focus_content();
        let clipped = fixture.render(width, height);
        assert_eq!(clipped.len(), (width * height) as usize);
        assert!(fixture.take().is_empty());
        for (id, _, action) in actions() {
            fixture.key(Key::Tab);
            fixture.render(width, height);
            let tile = fixture.by_id(id);
            let origin = tile.absolute_position();
            let size = tile.size();
            let margin = 4.0 * metric;
            assert!(
                origin.x - margin >= -0.05
                    && origin.y - margin >= -0.05
                    && origin.x + size.width + margin <= logical_extent + 0.05
                    && origin.y + size.height + margin <= logical_extent + 0.05,
                "both-axis focus reveal must reserve {id} and its entire outline"
            );
            assert!(
                fixture.take().is_empty(),
                "scroll-to-focus is not activation"
            );
            fixture.key(Key::Return);
            assert_eq!(fixture.take(), vec![Request::Action(action)]);
            fixture.click_at(Fixture::center(&tile));
            assert_eq!(fixture.take(), vec![Request::Action(action)]);
            fixture.move_to(LogicalPosition::new(0.0, 0.0));
        }
        fixture.key(Key::Escape);
        assert_eq!(fixture.take(), vec![Request::Hide]);
        fixture.render(1, 1);
        assert!(
            fixture.take().is_empty(),
            "even a 1px viewport has no phantom request"
        );
    }
    fixture.project(1.0, 1.0, [100.0, 100.0, 124.0, 124.0]);
    fixture.surface.invoke_focus_content();
    let inset = fixture.render(960, 720);
    for point in [
        LogicalPosition::new(99.0, 162.0),
        LogicalPosition::new(225.0, 162.0),
        LogicalPosition::new(162.0, 99.0),
        LogicalPosition::new(162.0, 225.0),
    ] {
        assert!(
            sample(&inset, 960, 1.0, point)
                .into_iter()
                .all(|channel| (6..=8).contains(&channel)),
            "both-axis overflow content stays clipped inside the selected body"
        );
    }
    assert!(fixture.take().is_empty());
    fixture.project(1.0, 1.0, [0.0, 0.0, 960.0, 720.0]);
    fixture.surface.invoke_focus_content();
    fixture.render(960, 720);
    near(fixture.by_id("body").size().width, 460.0);
    near(fixture.lock().size().width, 100.0);
    assert!(
        fixture.take().is_empty(),
        "viewport growth restores the ordinary real surface"
    );
}

#[test]
fn power_surface_presentation_opacity_affects_body_not_desktop_scrim_or_callback_authority() {
    let fixture = Fixture::new();
    let visible = fixture.render(960, 720);
    let body = fixture.by_id("body");
    let origin = body.absolute_position();
    let point = LogicalPosition::new(origin.x + body.size().width - 44.0, origin.y + 72.0);
    fixture.surface.invoke_set_presentation_opacity(0.0);
    let faded = fixture.render(960, 720);
    assert_ne!(
        sample(&visible, 960, 1.0, point),
        sample(&faded, 960, 1.0, point)
    );
    let outside = LogicalPosition::new(1.0, 1.0);
    assert_eq!(
        sample(&visible, 960, 1.0, outside),
        sample(&faded, 960, 1.0, outside)
    );
    assert!(
        fixture.take().is_empty(),
        "motion never requests Lock or dismissal"
    );
    fixture.surface.invoke_set_presentation_opacity(1.0);
    let restored = fixture.render(960, 720);
    assert_eq!(
        sample(&visible, 960, 1.0, point),
        sample(&restored, 960, 1.0, point)
    );
    assert!(fixture.take().is_empty());
}

// The testing renderer supports native transforms; unlike software pixels this
// queries the actual SDK transform chain, including transcluded icon/text.
#[test]
fn power_action_native_transform_chain_preserves_layout_and_scales_icon_label_about_center() {
    i_slint_backend_testing::init_no_event_loop();
    let surface = PowerMenuSurface::new().unwrap();
    surface.apply_presentation_theme(PresentationTheme::seelen_reference(
        slint::language::ColorScheme::Light,
    ));
    surface.window().set_size(PhysicalSize::new(960, 720));
    surface.set_selected_width(960.0);
    surface.set_selected_height(720.0);
    surface.show().unwrap();
    let requests = Rc::new(RefCell::new(Vec::new()));
    let recorded = requests.clone();
    surface.on_action_requested(move |action| recorded.borrow_mut().push(action));
    let element = |id: &str| {
        Fixture::one(ElementQuery::from_root(&surface).match_id(format!("PowerMenuSurface::{id}")))
    };
    let lock = element("lock");
    let icon = Fixture::one(lock.query_descendants().match_id("PowerActionTile::icon"));
    let label = Fixture::one(lock.query_descendants().match_id("PowerActionTile::label"));
    let origin = lock.absolute_position();
    let icon_origin = icon.absolute_position();
    let label_origin = label.absolute_position();
    near(label_origin.x, origin.x);
    let center = Fixture::center(&lock);
    let check = |scale: f32, lift: f32| {
        let bounds = ActionPaint::source(origin, 1.0, scale, lift, 0.0);
        near(lock.absolute_position().x, bounds.left);
        near(lock.absolute_position().y, bounds.top);
        // Layout metrics stay source-sized; the renderer scales the complete
        // subtree, not a separately resized icon or text/font fallback.
        near(lock.size().width, 100.0);
        near(lock.size().height, 100.0);
        near(icon.size().width, 25.0);
        near(icon.size().height, 25.0);
        near(label.size().width, 100.0);
        for (child, idle) in [(&icon, icon_origin), (&label, label_origin)] {
            near(
                child.absolute_position().x,
                center.x + (idle.x - center.x) * scale,
            );
            near(
                child.absolute_position().y,
                center.y + (idle.y - center.y) * scale + lift,
            );
        }
    };
    check(1.0, 0.0);
    surface
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: center });
    check(1.05, -4.0);
    assert!(
        requests.borrow().is_empty(),
        "hover has no command authority"
    );
    surface
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position: center,
            button: PointerEventButton::Left,
        });
    check(0.95, 0.0);
    assert!(
        requests.borrow().is_empty(),
        "press has no premature effect"
    );
    surface
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: center,
            button: PointerEventButton::Left,
        });
    assert_eq!(*requests.borrow(), vec![PowerMenuAction::LockSession]);
    check(1.05, -4.0);
    surface.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(1.0, 1.0),
    });
    check(1.0, 0.0);
    assert_eq!(
        requests.borrow().len(),
        1,
        "settling cannot replay activation"
    );
}

#[test]
fn power_surface_source_grid_pending_and_warning_rows_have_real_measured_bounds_and_tab_order() {
    let fixture = Fixture::new();
    for (scale, metric, width, height) in [(1.0, 1.0, 960, 720), (2.0, 1.0, 1920, 1440)] {
        fixture.project(scale, metric, [0.0, 0.0, 960.0, 720.0]);
        fixture.surface.set_updates_known_pending(false);
        fixture.surface.set_updates_status("".into());
        fixture.render(width, height);
        let body = fixture.by_id("body");
        near(body.size().width, 460.0 * metric);
        near(body.size().height, 433.0 * metric);
        let lock_origin = fixture.lock().absolute_position();
        near(lock_origin.x, body.absolute_position().x + 56.0 * metric);
        near(lock_origin.y, body.absolute_position().y + 153.0 * metric);
        for (index, (id, _, _)) in actions().into_iter().enumerate() {
            let origin = fixture.by_id(id).absolute_position();
            near(
                origin.x - lock_origin.x,
                (index % 3) as f32 * 124.0 * metric,
            );
            near(
                origin.y - lock_origin.y,
                (index / 3) as f32 * 124.0 * metric,
            );
        }
        fixture.surface.set_updates_known_pending(true);
        fixture.surface.set_install_updates(true);
        fixture.render(width, height);
        let row = fixture.by_id("updates-row");
        let expected_row = source_pending_row_height(&fixture.surface, metric);
        near(row.size().height, expected_row);
        near(
            body.size().height,
            433.0 * metric + 24.0 * metric + expected_row,
        );
        let choice = fixture.by_id("updates-choice");
        near(choice.size().width, row.size().width);
        near(choice.size().height, row.size().height);
        for (index, (id, _, _)) in actions().into_iter().enumerate() {
            let origin = fixture.by_id(id).absolute_position();
            near(
                origin.x,
                body.absolute_position().x + (56.0 + (index % 3) as f32 * 124.0) * metric,
            );
            near(
                origin.y,
                body.absolute_position().y
                    + (177.0 + (index / 3) as f32 * 124.0) * metric
                    + expected_row,
            );
        }
        assert_eq!(
            fixture.by_id("power-off").accessible_label().as_deref(),
            Some("Update and shut down")
        );
        assert_eq!(
            fixture.by_id("reboot").accessible_label().as_deref(),
            Some("Update and restart")
        );
        near(
            fixture.tile_child("power-off", "update-badge").size().width,
            10.0 * metric,
        );
        // Invisible measurement probes and decorative skin have no input/AX route.
        for id in [
            "updates-probe",
            "status-probe",
            "switch-track",
            "switch-thumb",
        ] {
            let element = fixture.by_id(id);
            assert_ne!(element.accessible_role(), Some(AccessibleRole::Button));
            assert_ne!(element.accessible_role(), Some(AccessibleRole::Text));
        }
        fixture.surface.invoke_focus_content();
        fixture.key(Key::Tab);
        fixture.key(Key::Space);
        assert!(!fixture.surface.get_install_updates());
        assert!(
            fixture.take().is_empty(),
            "native checkbox changes choice, not OS state"
        );
        for (_, _, action) in actions() {
            fixture.key(Key::Tab);
            fixture.key(Key::Return);
            assert_eq!(fixture.take(), vec![Request::Action(action)]);
        }
        fixture.surface.set_updates_known_pending(false);
        fixture
            .surface
            .set_updates_status("Update status unavailable".into());
        fixture.render(width, height);
        let message = fixture.by_id("updates-message");
        assert_eq!(
            message.accessible_label().as_deref(),
            Some("Update status unavailable")
        );
        let expected_message =
            source_text_height(&fixture.surface, "Update status unavailable", 348.0, metric);
        near(message.size().height, expected_message);
        near(
            body.size().height,
            433.0 * metric + expected_message + 24.0 * metric,
        );
        assert_eq!(fixture.by_id("suspend").accessible_enabled(), Some(true));
        fixture.surface.set_updates_status("".into());
        fixture.surface.invoke_focus_content();
        fixture.render(width, height);
    }
}

use crate::power_menu::source_measure_tests as source_measure;

// Share the independently laid-out native Text oracle with the GL assertions.
// Neither wrapper reads the production surface's row/probe height binding.
pub(crate) fn source_pending_row_height(surface: &PowerMenuSurface, metric: f32) -> f32 {
    let measure = source_measure::measure_pending_text(
        16.0,
        surface
            .global::<crate::generated::PopoverTokens>()
            .get_font_family(),
        1.4,
        metric,
        surface.window().scale_factor(),
    );
    near(measure.span_width, 292.0 * metric);
    near(measure.row_height, measure.text_height.max(18.0 * metric));
    assert!(measure.single_line_height > 0.0);
    if measure.unwrapped_width > measure.span_width {
        assert!(measure.text_height >= measure.single_line_height);
    }
    measure.row_height
}

pub(crate) fn source_text_height(
    surface: &PowerMenuSurface,
    text: &str,
    source_width: f32,
    metric: f32,
) -> f32 {
    source_measure::measure_source_text(
        text,
        source_width,
        16.0,
        surface
            .global::<crate::generated::PopoverTokens>()
            .get_font_family(),
        1.4,
        metric,
        surface.window().scale_factor(),
    )
}
