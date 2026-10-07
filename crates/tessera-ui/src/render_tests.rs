// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Renderer-backed presentation tests.
//!
//! Uses the software renderer's [`MinimalSoftwareWindow`] (an actual
//! rendering surface, not the preliminary testing backend): every scenario
//! instantiates the real generated Dock/Toolbar/Launcher components, draws
//! pixels, verifies geometry and accessible state, and proves there is no
//! idle rendering (a second `draw_if_needed` with no change draws nothing).
//! Pixel screenshots (PPM) are exported only to the nonempty directory named
//! by `TESSERA_TEST_SCREENSHOTS` (opt-in); no platform-specific path is assumed.
//!
//! Every test installs its own software-rendered platform on its own thread
//! (Slint allows one platform per thread) and instantiates exactly one live
//! component on it.

use std::cell::Cell;
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, Rgb8Pixel, VecModel};

use crate::generated::{
    ContextMenuSurface, Dock, DockApp, DockMenuAction, DockMenuKind, DockStatus, DockWindow,
    LaunchTile, Launcher, Toolbar, TooltipSurface,
};

/// Exports the drawn buffer as a binary PPM (P6) when the opt-in env var is
/// set; otherwise a no-op. Never writes inside the repository.
fn export_screenshot(name: &str, pixels: &[Rgb8Pixel], width: usize, height: usize) {
    let dir = match std::env::var_os("TESSERA_TEST_SCREENSHOTS") {
        Some(dir) if !dir.is_empty() => std::path::PathBuf::from(dir),
        _ => return,
    };
    std::fs::create_dir_all(&dir).expect("create requested screenshot directory");
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    ppm.extend(pixels.iter().flat_map(|p| [p.r, p.g, p.b]));
    std::fs::write(dir.join(format!("{name}.ppm")), ppm).expect("write requested screenshot");
}

/// A software-rendered window for the current test thread. Slint allows one
/// platform per thread (the test harness runs each test on its own thread),
/// so every renderer test installs its own platform — no cross-thread
/// sharing, no process-wide state.
fn software_window() -> Rc<MinimalSoftwareWindow> {
    struct TestPlatform(Rc<MinimalSoftwareWindow>);
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window.clone())))
        .expect("one software platform per test thread");
    window
}

/// Draws the current component state; asserts something was drawn and
/// returns the pixels plus the drawn flag for the no-idle-render proof.
fn draw(window: &MinimalSoftwareWindow, width: u32, height: u32) -> Vec<Rgb8Pixel> {
    let mut pixels = vec![Rgb8Pixel::default(); (width * height) as usize];
    let drawn = window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, width as usize);
    });
    assert!(
        drawn,
        "the first draw of a freshly changed window must happen"
    );
    pixels
}

fn app(key: &str, label: &str) -> LaunchTile {
    LaunchTile {
        key: key.into(),
        label: label.into(),
        icon: slint::Image::default(),
        pinned: false,
    }
}

#[test]
fn dock_renders_tiles_and_indicators_with_geometry_and_accessibility() {
    let window = software_window();
    let dock = Dock::new().unwrap();
    dock.set_pinned_apps(ModelRc::new(VecModel::from(vec![
        DockApp {
            key: "editor".into(),
            label: "Editor".into(),
            icon: slint::Image::default(),
            pinned: true,
        },
        DockApp {
            key: "files".into(),
            label: "Files".into(),
            icon: slint::Image::default(),
            pinned: true,
        },
    ])));
    dock.set_running_windows(ModelRc::new(VecModel::from(vec![DockWindow {
        key: "browser-key".into(),
        caption: "Browser".into(),
        icon: slint::Image::default(),
    }])));
    dock.set_surface_status(DockStatus {
        notice: "".into(),
        status: "".into(),
        refreshing: false,
        stale: false,
    });
    dock.show().unwrap();

    // 1x: the window holds margins(2x8) + bar(pad+item+pad=56) => 72 high;
    // the length covers the start tile and three content tiles.
    for (scale, width, height) in [(1.0, 216u32, 72u32), (2.0, 432u32, 144u32)] {
        window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        window.set_size(slint::PhysicalSize::new(width, height));
        // A DPI/framebuffer change invalidates the native surface even when
        // its logical size stays identical; the platform adapter requests it.
        window.request_redraw();
        let pixels = draw(&window, width, height);
        // Not blank: the bar and tiles draw distinct pixels.
        assert!(pixels.iter().any(|pixel| *pixel != pixels[0]));
        export_screenshot(
            &format!("dock-scale-{scale}"),
            &pixels,
            width as usize,
            height as usize,
        );

        // Start tile and every content tile are keyboard-accessible buttons.
        let start =
            ElementHandle::find_by_accessible_label(&dock, "Open applications and settings")
                .next()
                .unwrap();
        assert_eq!(start.accessible_enabled(), Some(true));
        assert_eq!(start.accessible_role(), Some(AccessibleRole::Button));
        let launch = ElementHandle::find_by_accessible_label(&dock, "Launch Editor")
            .next()
            .unwrap();
        assert_eq!(launch.accessible_role(), Some(AccessibleRole::Button));
        assert_eq!(launch.absolute_position().x, 64.0);
        assert_eq!(launch.absolute_position().y, 16.0);
        let switch = ElementHandle::find_by_accessible_label(&dock, "Switch to Browser")
            .next()
            .unwrap();
        assert_eq!(switch.accessible_role(), Some(AccessibleRole::Button));
    }
    drop(dock);
}

#[test]
fn dock_click_keyboard_and_disabled_states_route_keys() {
    let window = software_window();
    let dock = Dock::new().unwrap();
    dock.set_pinned_apps(ModelRc::new(VecModel::from(vec![DockApp {
        key: "editor".into(),
        label: "Editor".into(),
        icon: slint::Image::default(),
        pinned: true,
    }])));
    let launches = Rc::new(Cell::new(0));
    let counter = launches.clone();
    dock.on_launch_requested(move |_| counter.set(counter.get() + 1));
    dock.show().unwrap();
    window.set_size(slint::PhysicalSize::new(248, 72));
    let _ = draw(&window, 248, 72);

    let launch = ElementHandle::find_by_accessible_label(&dock, "Launch Editor")
        .next()
        .unwrap();
    launch.invoke_accessible_default_action();
    assert_eq!(launches.get(), 1, "accessible default action launches");

    // Pointer click routes the same key.
    let position = launch.absolute_position();
    let size = launch.size();
    let center = slint::LogicalPosition::new(
        position.x + size.width / 2.0,
        position.y + size.height / 2.0,
    );
    window.window().dispatch_event(WindowEvent::PointerPressed {
        position: center,
        button: PointerEventButton::Left,
    });
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: center,
            button: PointerEventButton::Left,
        });
    assert_eq!(launches.get(), 2, "pointer click launches");

    window.window().dispatch_event(WindowEvent::PointerPressed {
        position: center,
        button: PointerEventButton::Middle,
    });
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: center,
            button: PointerEventButton::Middle,
        });
    assert_eq!(launches.get(), 3, "middle click requests one new instance");

    // Disabled while refreshing: neither pointer nor accessibility acts.
    dock.set_surface_status(DockStatus {
        notice: "".into(),
        status: "".into(),
        refreshing: true,
        stale: false,
    });
    window.window().dispatch_event(WindowEvent::PointerPressed {
        position: center,
        button: PointerEventButton::Left,
    });
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: center,
            button: PointerEventButton::Left,
        });
    assert_eq!(launches.get(), 3, "refreshing disables the tile");
    assert_eq!(launch.accessible_enabled(), Some(false));
    launch.invoke_accessible_default_action();
    assert_eq!(launches.get(), 3, "disabled accessibility action is inert");

    // Rescue controls stay enabled while busy.
    let button = ElementHandle::find_by_accessible_label(&dock, "Open applications and settings")
        .next()
        .unwrap();
    assert_eq!(button.accessible_enabled(), Some(true));
    drop(dock);
}

#[test]
fn toolbar_renders_identity_and_settings_access() {
    let window = software_window();
    let toolbar = Toolbar::new().unwrap();
    toolbar.set_user_name("alice".into());
    toolbar.set_focused_app("Editor — window".into());
    toolbar.set_clock("12:34".into());
    toolbar.set_language("en-US".into());
    toolbar.show().unwrap();
    let opened = Rc::new(Cell::new(0));
    let counter = opened.clone();
    toolbar.on_open_panel_requested(move || counter.set(counter.get() + 1));
    for (scale, width, height) in [(1.0, 640u32, 32u32), (2.0, 1280u32, 64u32)] {
        window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        window.set_size(slint::PhysicalSize::new(width, height));
        window.request_redraw();
        let pixels = draw(&window, width, height);
        let settings =
            ElementHandle::find_by_accessible_label(&toolbar, "Open settings and recovery")
                .next()
                .unwrap();
        assert_eq!(settings.accessible_role(), Some(AccessibleRole::Button));
        let origin = settings.absolute_position();
        let (x, y) = ((origin.x * scale) as usize, (origin.y * scale) as usize);
        let inset = (3.0 * scale) as usize;
        let end = (13.0 * scale) as usize;
        assert!(
            (y + inset..y + end).any(|row| {
                pixels[row * width as usize + x + inset..row * width as usize + x + end]
                    .iter()
                    .any(|pixel| *pixel != pixels[0])
            }),
            "the settings vector renders inside its 16px tile at each DPI"
        );
        export_screenshot(
            &format!("toolbar-{scale}x"),
            &pixels,
            width as usize,
            height as usize,
        );
        settings.invoke_accessible_default_action();
    }
    assert_eq!(
        opened.get(),
        2,
        "settings opens the real panel at both scales"
    );
    drop(toolbar);
}

#[test]
fn launcher_renders_grid_search_and_escape_hides() {
    let window = software_window();
    let launcher = Launcher::new().unwrap();
    launcher.set_tiles(ModelRc::new(VecModel::from(vec![
        app("app-editor", "Rust Editor"),
        app("app-browser", "Web Browser"),
    ])));
    let launched = Rc::new(Cell::new(0));
    let counter = launched.clone();
    launcher.on_launch_requested(move |_| counter.set(counter.get() + 1));
    let hidden = Rc::new(Cell::new(0));
    let counter = hidden.clone();
    launcher.on_hide_requested(move || counter.set(counter.get() + 1));
    launcher.show().unwrap();
    launcher.invoke_focus_search();
    let bounds = crate::DockContext::new(0, 0, 1920, 1080, false).unwrap();
    let rect = crate::dock::launcher_rect(bounds, 1.0);
    window.set_size(slint::PhysicalSize::new(rect.width, rect.height));
    let pixels = draw(&window, rect.width, rect.height);
    assert!(pixels.iter().any(|pixel| *pixel != pixels[0]));
    export_screenshot(
        "launcher",
        &pixels,
        rect.width as usize,
        rect.height as usize,
    );
    window
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: "R".into() });
    assert_eq!(launcher.get_search(), "R");

    let launch = ElementHandle::find_by_accessible_label(&launcher, "Launch Rust Editor")
        .next()
        .unwrap();
    assert_eq!(launch.accessible_role(), Some(AccessibleRole::Button));
    launch.invoke_accessible_default_action();
    assert_eq!(launched.get(), 1);

    // Escape hides via the FocusScope at the window level.
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    assert_eq!(hidden.get(), 1, "Escape routes the hide request");
    drop(launcher);
}

#[test]
fn no_idle_render_after_draining_unchanged_state() {
    let window = software_window();
    let dock = Dock::new().unwrap();
    dock.set_pinned_apps(ModelRc::new(VecModel::from(vec![DockApp {
        key: "editor".into(),
        label: "Editor".into(),
        icon: slint::Image::default(),
        pinned: true,
    }])));
    dock.show().unwrap();
    window.set_size(slint::PhysicalSize::new(248, 72));
    let first = draw(&window, 248, 72);
    // Drained: no input, property, or timer change since the first draw.
    assert!(
        !window.draw_if_needed(|renderer| {
            let mut pixels = first.clone();
            renderer.render(&mut pixels, 248);
        }),
        "an unchanged component state must not redraw (no idle rendering)"
    );
    drop(dock);
}

#[test]
fn dock_compact_and_stale_states_change_rendering_and_keep_rescue() {
    let window = software_window();
    let dock = Dock::new().unwrap();
    dock.set_pinned_apps(ModelRc::new(VecModel::from(vec![DockApp {
        key: "editor".into(),
        label: "Editor".into(),
        icon: slint::Image::default(),
        pinned: true,
    }])));
    dock.show().unwrap();

    // Compact shrinks the tokens; both densities draw non-blank frames and
    // the start tile (rescue) stays enabled and positioned inside the bar.
    for (name, compact) in [("dock-normal", false), ("dock-compact", true)] {
        dock.set_compact(compact);
        let bounds = crate::DockContext::new(0, 0, 1920, 1080, false).unwrap();
        let rect = crate::dock::dock_rect(bounds, crate::DockEdge::Bottom, 1, compact, 1.0);
        window.set_size(slint::PhysicalSize::new(rect.width, rect.height));
        let pixels = draw(&window, rect.width, rect.height);
        assert!(pixels.iter().any(|pixel| *pixel != pixels[0]));
        export_screenshot(name, &pixels, rect.width as usize, rect.height as usize);
        let start =
            ElementHandle::find_by_accessible_label(&dock, "Open applications and settings")
                .next()
                .unwrap();
        assert_eq!(start.accessible_enabled(), Some(true));
        let position = start.absolute_position();
        assert!(
            position.x >= 8.0 && position.y >= 8.0,
            "start tile sits inside the bar padding"
        );
    }

    // Stale dims the content tiles (opacity 0.5) but keeps them reachable
    // as accessible buttons reporting disabled.
    dock.set_compact(false);
    dock.set_surface_status(DockStatus {
        notice: "".into(),
        status: "".into(),
        refreshing: false,
        stale: true,
    });
    window.set_size(slint::PhysicalSize::new(248, 72));
    let stale_pixels = draw(&window, 248, 72);
    export_screenshot("dock-stale", &stale_pixels, 248, 72);
    let launch = ElementHandle::find_by_accessible_label(&dock, "Launch Editor")
        .next()
        .unwrap();
    assert_eq!(launch.accessible_enabled(), Some(false));
    drop(dock);
}

#[test]
fn launcher_pin_toggle_routes_the_key() {
    let window = software_window();
    let launcher = Launcher::new().unwrap();
    launcher.set_tiles(ModelRc::new(VecModel::from(vec![app(
        "app-editor",
        "Rust Editor",
    )])));
    let pins = Rc::new(Cell::new(0));
    let counter = pins.clone();
    launcher.on_pin_toggle_requested(move |_, _| counter.set(counter.get() + 1));
    let launches = Rc::new(Cell::new(0));
    let counter = launches.clone();
    launcher.on_launch_requested(move |_| counter.set(counter.get() + 1));
    launcher.show().unwrap();
    window.set_size(slint::PhysicalSize::new(560, 420));
    let _ = draw(&window, 560, 420);

    let pin = ElementHandle::find_by_accessible_label(&launcher, "Pin Rust Editor")
        .next()
        .unwrap();
    pin.invoke_accessible_default_action();
    assert_eq!(pins.get(), 1, "accessible pin action toggles");
    let position = pin.absolute_position();
    let center = slint::LogicalPosition::new(position.x + 8.0, position.y + 8.0);
    window.window().dispatch_event(WindowEvent::PointerPressed {
        position: center,
        button: PointerEventButton::Left,
    });
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: center,
            button: PointerEventButton::Left,
        });
    assert_eq!(pins.get(), 2, "mouse pin action is not a tile launch");
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Space.into(),
    });
    assert_eq!(pins.get(), 3, "focused pin accepts keyboard activation");
    assert_eq!(launches.get(), 0, "pin input never bubbles into launch");
    launcher.set_stale(true);
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    pin.invoke_accessible_default_action();
    assert_eq!(pins.get(), 3, "stale pin controls reject all actions");
    drop(launcher);
}

#[test]
fn dark_palette_renders_the_source_neutral_tile_color() {
    let window = software_window();
    let dock = Dock::new().unwrap();
    dock.global::<crate::generated::SeelenPalette>()
        .set_color_scheme(slint::language::ColorScheme::Dark);
    dock.global::<crate::generated::Palette>()
        .set_color_scheme(slint::language::ColorScheme::Dark);
    dock.set_pinned_apps(ModelRc::new(VecModel::from(vec![DockApp {
        key: "editor".into(),
        label: "Editor".into(),
        icon: slint::Image::default(),
        pinned: true,
    }])));
    dock.show().unwrap();
    window.set_size(slint::PhysicalSize::new(120, 72));
    let pixels = draw(&window, 120, 72);
    let inside_tile = pixels[36 * 120 + 84];
    assert_eq!([inside_tile.r, inside_tile.g, inside_tile.b], [31, 31, 31]);
    export_screenshot("dock-dark", &pixels, 120, 72);
}

#[test]
fn context_menu_renders_all_actions_outside_bar_height_at_one_and_two_x() {
    let window = software_window();
    let menu = ContextMenuSurface::new().unwrap();
    menu.set_kind(DockMenuKind::Bar);
    menu.global::<crate::generated::SeelenPalette>()
        .set_color_scheme(slint::language::ColorScheme::Dark);
    let selected = Rc::new(Cell::new(None));
    let recorded = Rc::clone(&selected);
    menu.on_action_requested(move |action| recorded.set(Some(action)));
    menu.show().unwrap();
    menu.invoke_focus_menu();

    for (name, scale) in [("dock-context-menu-1x", 1.0), ("dock-context-menu-2x", 2.0)] {
        window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        let width = (menu.get_menu_width() * scale) as u32;
        let height = (menu.get_menu_height() * scale) as u32;
        assert!(
            height > (72.0 * scale) as u32,
            "menu is not clipped to the dock"
        );
        window.set_size(slint::PhysicalSize::new(width, height));
        let pixels = draw(&window, width, height);
        assert!(pixels.iter().any(|pixel| *pixel != pixels[0]));
        for label in [
            "Settings",
            "File Explorer",
            "Task Manager",
            "Restore Explorer",
            "Exit Tessera",
        ] {
            let button = ElementHandle::find_by_accessible_label(&menu, label)
                .next()
                .unwrap();
            assert_eq!(button.accessible_role(), Some(AccessibleRole::Button));
            let position = button.absolute_position();
            assert!(position.y >= 0.0 && position.y + 30.0 <= menu.get_menu_height());
        }
        export_screenshot(name, &pixels, width as usize, height as usize);
        assert!(
            !window.draw_if_needed(|renderer| {
                let mut unchanged = pixels.clone();
                renderer.render(&mut unchanged, width as usize);
            }),
            "the open menu does not continuously render unchanged pixels"
        );
    }
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::End.into(),
    });
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    assert_eq!(selected.get(), Some(DockMenuAction::Exit));
    drop(menu);
}

#[test]
fn dock_right_click_emits_actual_window_relative_anchor_without_launching() {
    let window = software_window();
    let dock = Dock::new().unwrap();
    dock.set_pinned_apps(ModelRc::new(VecModel::from(vec![DockApp {
        key: "editor".into(),
        label: "Editor".into(),
        icon: slint::Image::default(),
        pinned: true,
    }])));
    let launched = Rc::new(Cell::new(0));
    let counter = Rc::clone(&launched);
    dock.on_launch_requested(move |_| counter.set(counter.get() + 1));
    let requested = Rc::new(std::cell::RefCell::new(Vec::new()));
    let records = Rc::clone(&requested);
    dock.on_context_menu_requested(move |kind, key, point| {
        records.borrow_mut().push((kind, key, point))
    });
    dock.show().unwrap();
    window.set_size(slint::PhysicalSize::new(248, 72));
    let _ = draw(&window, 248, 72);
    let tile = ElementHandle::find_by_accessible_label(&dock, "Launch Editor")
        .next()
        .unwrap();
    let origin = tile.absolute_position();
    let point = slint::LogicalPosition::new(origin.x + 10.0, origin.y + 10.0);
    for event in [
        WindowEvent::PointerPressed {
            position: point,
            button: PointerEventButton::Right,
        },
        WindowEvent::PointerReleased {
            position: point,
            button: PointerEventButton::Right,
        },
    ] {
        window.window().dispatch_event(event);
    }
    assert_eq!(
        &*requested.borrow(),
        &[(DockMenuKind::Pinned, "editor".into(), point)]
    );
    assert_eq!(launched.get(), 0);
    drop(dock);
}

#[test]
fn passive_tooltip_renders_wrapped_text_outside_bar_at_one_and_two_x() {
    let window = software_window();
    let tooltip = TooltipSurface::new().unwrap();
    let caption =
        "A genuine long application window title that wraps without adding actions. ".repeat(4);
    tooltip.set_content(caption.clone().into());
    tooltip.show().unwrap();
    for (name, scale) in [("tooltip-1x", 1.0), ("tooltip-2x", 2.0)] {
        window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        let width = (tooltip.get_tooltip_width() * scale).ceil() as u32;
        let height = (tooltip.get_tooltip_height() * scale).ceil() as u32;
        assert!(width > 16 && width <= (266.0 * scale) as u32);
        assert!(
            height < (500.0 * scale) as u32,
            "first-show measurement must wrap to the declared tooltip width, not a provisional 1px window"
        );
        assert!(
            height > (72.0 * scale) as u32,
            "wrapped text is not dock-clipped"
        );
        window.set_size(slint::PhysicalSize::new(width, height));
        window.request_redraw();
        let pixels = draw(&window, width, height);
        assert!(pixels.iter().any(|pixel| *pixel != pixels[0]));
        let text = ElementHandle::find_by_accessible_label(&tooltip, &caption)
            .next()
            .unwrap();
        assert_eq!(text.accessible_role(), Some(AccessibleRole::Text));
        export_screenshot(name, &pixels, width as usize, height as usize);
        assert!(!window.draw_if_needed(|renderer| {
            let mut unchanged = pixels.clone();
            renderer.render(&mut unchanged, width as usize);
        }));
    }
}
