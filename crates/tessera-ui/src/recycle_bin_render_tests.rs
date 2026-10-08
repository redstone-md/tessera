// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Genuine generated presentation/input only. Recorded requests do not prove
//! root/provider dispatch, native Shell effects, or Windows watcher coverage.

use std::cell::RefCell;
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, ModelRc, PhysicalSize, Rgb8Pixel, VecModel};

use crate::generated::{
    Dock, DockApp, DockMenuKind, DockRecycleAction, DockRecycleState, DockStatus,
};
use crate::theme::{PresentationTheme, ThemedComponent};

#[derive(Debug, PartialEq)]
enum Request {
    Open(DockRecycleAction),
    Context(DockMenuKind, String),
    App(String),
    Start,
    Desktop,
}

struct Fixture {
    window: Rc<MinimalSoftwareWindow>,
    dock: Dock,
    requests: Rc<RefCell<Vec<Request>>>,
    tooltips: Rc<RefCell<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        struct TestPlatform(Rc<MinimalSoftwareWindow>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
        let dock = Dock::new().unwrap();
        let requests = Rc::new(RefCell::new(Vec::new()));
        let recorded = requests.clone();
        dock.on_recycle_action_requested(move |action| {
            recorded.borrow_mut().push(Request::Open(action))
        });
        let recorded = requests.clone();
        dock.on_context_menu_requested(move |kind, key, _| {
            recorded
                .borrow_mut()
                .push(Request::Context(kind, key.to_string()))
        });
        let recorded = requests.clone();
        dock.on_launch_requested(move |key| {
            recorded.borrow_mut().push(Request::App(key.to_string()))
        });
        let recorded = requests.clone();
        dock.on_open_applications_requested(move || recorded.borrow_mut().push(Request::Start));
        let recorded = requests.clone();
        dock.on_reserved_action_requested(move |_| recorded.borrow_mut().push(Request::Desktop));
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
                    key: format!("app/{index}").into(),
                    label: format!("App {index}").into(),
                    icon: slint::Image::default(),
                    pinned: true,
                })
                .collect::<Vec<_>>(),
        )));
    }

    fn render(&self, width: u32, height: u32, scale: f32) -> Vec<Rgb8Pixel> {
        self.event(WindowEvent::ScaleFactorChanged {
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
        ElementHandle::find_by_accessible_label(&self.dock, label)
            .find(|element| element.accessible_role() == Some(AccessibleRole::Button))
            .unwrap_or_else(|| panic!("missing button {label}"))
    }

    fn trash(&self) -> ElementHandle {
        let mut nodes = ElementQuery::from_root(&self.dock)
            .find_all()
            .into_iter()
            .filter(|element| {
                element.accessible_role() == Some(AccessibleRole::Button)
                    && element
                        .accessible_label()
                        .is_some_and(|label| label.starts_with("Recycle Bin\n"))
            });
        let tile = nodes.next().expect("literal Trash button");
        assert!(nodes.next().is_none());
        tile
    }

    fn center(element: &ElementHandle) -> LogicalPosition {
        let origin = element.absolute_position();
        let size = element.size();
        LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
    }

    fn event(&self, event: WindowEvent) {
        self.window.window().dispatch_event(event);
    }
    fn pointer(&self, element: &ElementHandle, button: PointerEventButton) {
        let position = Self::center(element);
        self.event(WindowEvent::PointerMoved { position });
        self.event(WindowEvent::PointerPressed { position, button });
        self.event(WindowEvent::PointerReleased { position, button });
    }
    fn key(&self, key: Key) {
        self.event(WindowEvent::KeyPressed { text: key.into() });
        self.event(WindowEvent::KeyReleased { text: key.into() });
    }
    fn take(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }
}

fn close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.03, "{actual} != {expected}");
}

#[test]
fn recycle_fixed_trailing_layout_all_edges_compact_themes_and_dpi() {
    let fixture = Fixture::new();
    fixture.apps(1);
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
                        (216.0, 72.0)
                    } else {
                        (72.0, 216.0)
                    };
                    let physical_width = (width * factor * scale).ceil() as u32;
                    let physical_height = (height * factor * scale).ceil() as u32;
                    fixture.render(physical_width, physical_height, scale);
                    for (tile, along, trailing) in [
                        (
                            fixture.element("Open applications and settings"),
                            16.0,
                            false,
                        ),
                        (fixture.element("Show desktop"), 64.0, false),
                        (fixture.element("Launch App 0"), 112.0, false),
                        (fixture.trash(), 160.0, true),
                    ] {
                        // Native physical ceil adds a real fractional logical
                        // remainder only to the fixed end-anchored tile.
                        let along = if trailing {
                            let physical_along = if edge < 2 {
                                physical_width
                            } else {
                                physical_height
                            };
                            physical_along as f32 / scale - 56.0 * factor
                        } else {
                            along * factor
                        };
                        let origin = tile.absolute_position();
                        close(origin.x, if edge < 2 { along } else { 16.0 * factor });
                        close(origin.y, if edge < 2 { 16.0 * factor } else { along });
                        close(tile.size().width, 40.0 * factor);
                        close(tile.size().height, 40.0 * factor);
                    }
                    assert!(fixture.take().is_empty());
                }
            }
        }
    }
}

#[test]
fn recycle_unknown_confirmed_count_stale_and_independent_safe_notices() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.trash().accessible_label().as_deref(),
        Some("Recycle Bin\nUnknown")
    );
    fixture.dock.set_recycle_item_count_label("0 items".into());
    fixture.dock.set_recycle_read_busy(true);
    fixture.render(216, 72, 1.0);
    assert_eq!(
        fixture.trash().accessible_label().as_deref(),
        Some("Recycle Bin\nLoading")
    );
    fixture.dock.set_recycle_read_busy(false);
    fixture.dock.set_recycle_state(DockRecycleState::Empty);
    fixture.render(216, 72, 1.0);
    assert_eq!(
        fixture.trash().accessible_label().as_deref(),
        Some("Recycle Bin\n0 items")
    );
    fixture.dock.set_recycle_state(DockRecycleState::Full);
    fixture
        .dock
        .set_recycle_item_count_label("18446744073709551615 items".into());
    fixture.dock.set_recycle_stale(true);
    fixture
        .dock
        .set_recycle_read_notice("Read unavailable".into());
    fixture
        .dock
        .set_recycle_watch_notice("Live updates unavailable".into());
    fixture
        .dock
        .set_recycle_open_notice("Open unavailable".into());
    fixture.render(216, 72, 1.0);
    let expected = "Recycle Bin\n18446744073709551615 items\nStale\nRead unavailable\nLive updates unavailable\nOpen unavailable";
    assert_eq!(
        fixture.trash().accessible_label().as_deref(),
        Some(expected)
    );
    fixture.event(WindowEvent::PointerMoved {
        position: Fixture::center(&fixture.trash()),
    });
    assert_eq!(
        fixture.tooltips.borrow().last().map(String::as_str),
        Some(expected)
    );
    assert!(fixture.take().is_empty());
}

#[test]
fn recycle_real_input_tab_ax_context_and_open_independence() {
    let fixture = Fixture::new();
    fixture.apps(1);
    fixture.render(216, 72, 1.0);
    fixture.pointer(
        &fixture.element("Open applications and settings"),
        PointerEventButton::Left,
    );
    assert_eq!(fixture.take(), vec![Request::Start]);
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    assert_eq!(fixture.take(), vec![Request::Desktop]);
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    assert_eq!(fixture.take(), vec![Request::App("app/0".into())]);
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    assert_eq!(fixture.take(), vec![Request::Open(DockRecycleAction::Open)]);
    fixture.event(WindowEvent::KeyPressRepeated {
        text: Key::Return.into(),
    });
    assert!(fixture.take().is_empty());
    fixture.event(WindowEvent::KeyPressed {
        text: Key::Space.into(),
    });
    assert!(fixture.take().is_empty());
    fixture.event(WindowEvent::KeyReleased {
        text: Key::Space.into(),
    });
    assert_eq!(fixture.take(), vec![Request::Open(DockRecycleAction::Open)]);
    fixture.key(Key::Menu);
    assert_eq!(
        fixture.take(),
        vec![Request::Context(DockMenuKind::Recycle, String::new())]
    );
    fixture.event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    fixture.key(Key::F10);
    fixture.event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
    assert_eq!(
        fixture.take(),
        vec![Request::Context(DockMenuKind::Recycle, String::new())]
    );
    fixture.pointer(&fixture.trash(), PointerEventButton::Right);
    assert_eq!(
        fixture.take(),
        vec![Request::Context(DockMenuKind::Recycle, String::new())]
    );
    fixture.dock.set_surface_status(DockStatus {
        stale: true,
        refreshing: true,
        ..Default::default()
    });
    fixture.dock.set_show_desktop_busy(true);
    fixture.dock.set_recycle_read_busy(true);
    fixture
        .dock
        .set_recycle_watch_notice("Live updates unavailable".into());
    fixture.render(216, 72, 1.0);
    fixture.pointer(&fixture.trash(), PointerEventButton::Left);
    fixture.trash().invoke_accessible_default_action();
    assert_eq!(
        fixture.take(),
        vec![
            Request::Open(DockRecycleAction::Open),
            Request::Open(DockRecycleAction::Open)
        ]
    );
    fixture.dock.set_recycle_open_busy(true);
    fixture.render(216, 72, 1.0);
    assert_eq!(fixture.trash().accessible_enabled(), Some(false));
    fixture.pointer(&fixture.trash(), PointerEventButton::Left);
    fixture.trash().invoke_accessible_default_action();
    fixture.key(Key::Return);
    assert!(fixture.take().is_empty());
}

#[test]
fn recycle_scroll_retains_all_apps_fixed_utilities_and_finite_tiny_bounds() {
    let fixture = Fixture::new();
    fixture.apps(64);
    for edge in 0..4 {
        fixture.dock.set_edge(edge);
        let (width, height) = if edge < 2 { (216, 72) } else { (72, 216) };
        fixture.render(width, height, 1.0);
        let trash_origin = fixture.trash().absolute_position();
        let viewport = ElementHandle::find_by_element_id(&fixture.dock, "ScrollView::flickable")
            .next()
            .unwrap();
        fixture.event(WindowEvent::PointerScrolled {
            position: Fixture::center(&viewport),
            delta_x: if edge < 2 { -100_000.0 } else { 0.0 },
            delta_y: if edge < 2 { 0.0 } else { -100_000.0 },
        });
        fixture.render(width, height, 1.0);
        assert_eq!(fixture.trash().absolute_position(), trash_origin);
        assert_eq!(fixture.dock.get_pinned_apps().row_count(), 64);
        fixture.pointer(&fixture.element("Launch App 63"), PointerEventButton::Left);
        assert_eq!(fixture.take(), vec![Request::App("app/63".into())]);
        let app_origin = fixture.element("Launch App 63").absolute_position();
        fixture.event(WindowEvent::PointerScrolled {
            position: Fixture::center(&fixture.trash()),
            delta_x: 100_000.0,
            delta_y: 100_000.0,
        });
        fixture.render(width, height, 1.0);
        assert_eq!(
            fixture.element("Launch App 63").absolute_position(),
            app_origin
        );
        for scale in [1.0, 2.0] {
            for (width, height) in [(1, 1), (8, 72), (72, 8), (24, 24)] {
                fixture.render(width, height, scale);
                for element in ElementQuery::from_root(&fixture.dock).find_all() {
                    let origin = element.absolute_position();
                    let size = element.size();
                    assert!(origin.x.is_finite() && origin.y.is_finite());
                    assert!(size.width.is_finite() && size.height.is_finite());
                    assert!(size.width >= 0.0 && size.height >= 0.0);
                }
                assert!(fixture.take().is_empty());
            }
        }
    }
}

#[test]
fn recycle_context_surface_has_only_real_retry_row_and_preserves_other_scopes() {
    use crate::generated::{ContextMenuSurface, DockMenuAction};

    let fixture = Fixture::new();
    fixture.dock.hide().unwrap();
    let menu = ContextMenuSurface::new_with_metrics().unwrap();
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = actions.clone();
    menu.on_action_requested(move |action| recorded.borrow_mut().push(action));
    menu.set_kind(DockMenuKind::Recycle);
    menu.show().unwrap();
    fixture.render(240, 100, 1.0);
    let buttons = ElementQuery::from_root(&menu)
        .find_all()
        .into_iter()
        .filter(|element| element.accessible_role() == Some(AccessibleRole::Button))
        .collect::<Vec<_>>();
    assert_eq!(buttons.len(), 1);
    assert_eq!(buttons[0].accessible_label().as_deref(), Some("Retry"));
    fixture.pointer(&buttons[0], PointerEventButton::Left);
    assert_eq!(*actions.borrow(), vec![DockMenuAction::RecycleRetry]);
    for (kind, expected) in [
        (DockMenuKind::Bar, 5),
        (DockMenuKind::Pinned, 2),
        (DockMenuKind::Window, 3),
    ] {
        menu.set_kind(kind);
        fixture.render(240, 360, 1.0);
        let buttons = ElementQuery::from_root(&menu)
            .find_all()
            .into_iter()
            .filter(|element| element.accessible_role() == Some(AccessibleRole::Button))
            .collect::<Vec<_>>();
        assert_eq!(buttons.len(), expected);
        assert!(
            buttons
                .iter()
                .all(|element| element.accessible_label().as_deref() != Some("Retry"))
        );
    }
    menu.hide().unwrap();
}

#[test]
fn recycle_empty_and_capped_viewport_never_truncate_models() {
    let fixture = Fixture::new();
    for (count, along) in [(0, 168), (1, 216), (29, 1560), (32, 1560), (64, 1560)] {
        fixture.apps(count);
        for edge in 0..4 {
            fixture.dock.set_edge(edge);
            let (width, height) = if edge < 2 { (along, 72) } else { (72, along) };
            fixture.render(width, height, 1.0);
            let origin = fixture.trash().absolute_position();
            close(
                if edge < 2 { origin.x } else { origin.y },
                along as f32 - 56.0,
            );
            assert_eq!(fixture.dock.get_pinned_apps().row_count(), count);
            let viewport =
                ElementHandle::find_by_element_id(&fixture.dock, "ScrollView::flickable")
                    .next()
                    .unwrap();
            let size = viewport.size();
            close(
                if edge < 2 { size.width } else { size.height },
                (along as f32 - 176.0).max(0.0),
            );
            assert!(fixture.take().is_empty());
        }
    }
}
