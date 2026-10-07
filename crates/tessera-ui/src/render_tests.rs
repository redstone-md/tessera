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
use std::time::Duration;

use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, Rgb8Pixel, VecModel};

use crate::generated::{
    ContextMenuSurface, Dock, DockApp, DockMenuAction, DockMenuKind, DockStatus, DockWindow,
    LaunchTile, Launcher, Toolbar, TooltipSurface,
};
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::transient_window::TransientComponent;

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
    software_window_with_clock(Rc::new(Cell::new(Duration::ZERO)))
}

fn software_window_with_clock(clock: Rc<Cell<Duration>>) -> Rc<MinimalSoftwareWindow> {
    struct TestPlatform(Rc<MinimalSoftwareWindow>, Rc<Cell<Duration>>);
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }

        fn duration_since_start(&self) -> Duration {
            self.1.get()
        }
    }
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window.clone(), clock)))
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
    let hints = Rc::new(std::cell::RefCell::new(Vec::new()));
    let records = Rc::clone(&hints);
    toolbar
        .on_tooltip_requested(move |content, bounds| records.borrow_mut().push((content, bounds)));
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
        for point in [
            slint::LogicalPosition::new(0.0, 0.0),
            slint::LogicalPosition::new(origin.x + 8.0, origin.y + 8.0),
        ] {
            window
                .window()
                .dispatch_event(WindowEvent::PointerMoved { position: point });
            slint::platform::update_timers_and_animations();
        }
        let requested = hints.borrow();
        let (content, bounds) = requested
            .last()
            .expect("actual settings hover requests a hint");
        assert_eq!(content, "Settings and recovery");
        assert_eq!(bounds.origin, origin);
        assert_eq!((bounds.width, bounds.height), (16.0, 16.0));
        drop(requested);
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
    let selected = Rc::new(Cell::new(None));
    let recorded = Rc::clone(&selected);
    menu.on_action_requested(move |action| recorded.set(Some(action)));
    menu.show().unwrap();
    menu.invoke_focus_menu();

    for (name, scheme, scale, background) in [
        (
            "dock-context-menu-1x",
            slint::language::ColorScheme::Dark,
            1.0,
            [24, 24, 24],
        ),
        (
            "dock-context-menu-2x",
            slint::language::ColorScheme::Dark,
            2.0,
            [24, 24, 24],
        ),
        (
            "dock-context-menu-light-1x",
            slint::language::ColorScheme::Light,
            1.0,
            [242, 242, 242],
        ),
        (
            "dock-context-menu-light-2x",
            slint::language::ColorScheme::Light,
            2.0,
            [242, 242, 242],
        ),
    ] {
        menu.apply_presentation_theme(PresentationTheme::uniform(scheme));
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
        let margin = menu
            .global::<crate::generated::PopoverTokens>()
            .get_shadow_margin();
        let inset = ((margin + 2.0) * scale) as usize;
        let pixel = pixels[(height as usize / 2) * width as usize + inset];
        assert_eq!(
            [pixel.r, pixel.g, pixel.b],
            background,
            "the shared menu body is opaque and theme-adaptive"
        );
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
            assert!(position.x >= margin + 8.0 && position.y >= margin + 8.0);
            assert!(position.y + 30.0 <= menu.get_menu_height() - margin - 8.0);
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
    for (name, scheme, scale, background) in [
        (
            "tooltip-dark-1x",
            slint::language::ColorScheme::Dark,
            1.0,
            [24, 24, 24],
        ),
        (
            "tooltip-dark-2x",
            slint::language::ColorScheme::Dark,
            2.0,
            [24, 24, 24],
        ),
        (
            "tooltip-light-1x",
            slint::language::ColorScheme::Light,
            1.0,
            [242, 242, 242],
        ),
        (
            "tooltip-light-2x",
            slint::language::ColorScheme::Light,
            2.0,
            [242, 242, 242],
        ),
    ] {
        tooltip.apply_presentation_theme(PresentationTheme::uniform(scheme));
        window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        let tokens = tooltip.global::<crate::generated::PopoverTokens>();
        for lines in [1, 2, 9] {
            tooltip.set_content(vec!["Measured line"; lines].join("\n").into());
            let expected_height = lines as f32 * tokens.get_font_size() * 1.4
                + 8.0
                + 2.0 * tokens.get_shadow_margin();
            // Software intrinsic heights snap to logical pixels, before the
            // native window size is scaled to physical pixels.
            assert_eq!(
                tooltip.get_tooltip_height(),
                expected_height.round(),
                "font-size-relative line-height: lines={lines}, scale={scale}"
            );
        }
        tooltip.set_content(caption.clone().into());
        let width = (tooltip.get_tooltip_width() * scale).ceil() as u32;
        let height = (tooltip.get_tooltip_height() * scale).ceil() as u32;
        assert!(width > 20 && width <= (520.0 * scale) as u32);
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
        assert_eq!(tokens.get_font_size(), 12.8);
        assert_eq!(tokens.get_shadow_margin(), 10.0);
        let inset = ((tokens.get_shadow_margin() + 2.0) * scale) as usize;
        let pixel = pixels[(height as usize / 2) * width as usize + inset];
        assert_eq!(
            [pixel.r, pixel.g, pixel.b],
            background,
            "the body is opaque and follows the selected theme at each DPI"
        );
        let bottom = height as usize - (tokens.get_shadow_margin() * scale) as usize;
        let padding_top = bottom - (4.0 * scale) as usize;
        let corner_inset =
            ((tokens.get_shadow_margin() + tokens.get_radius() + 1.0) * scale) as usize;
        assert!(
            (padding_top..bottom).all(|row| {
                pixels
                    [row * width as usize + corner_inset..(row + 1) * width as usize - corner_inset]
                    .iter()
                    .all(|pixel| [pixel.r, pixel.g, pixel.b] == background)
            }),
            "all wrapped lines must fit before the body's bottom padding, not be clipped into it"
        );
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

#[test]
fn dock_hover_reports_bounds_and_dismisses_on_click_disable_and_scrolling() {
    let window = software_window();
    let dock = Dock::new().unwrap();
    let label = "An editor label that is much longer than the forty-pixel tile";
    let mut apps = vec![DockApp {
        key: "editor".into(),
        label: label.into(),
        icon: slint::Image::default(),
        pinned: true,
    }];
    apps.extend((0..5).map(|index| DockApp {
        key: format!("extra-{index}").into(),
        label: format!("App {index}").into(),
        icon: slint::Image::default(),
        pinned: true,
    }));
    dock.set_pinned_apps(ModelRc::new(VecModel::from(apps)));
    let hints = Rc::new(std::cell::RefCell::new(Vec::new()));
    let records = Rc::clone(&hints);
    dock.on_tooltip_requested(move |content, bounds| records.borrow_mut().push((content, bounds)));
    let dismissed = Rc::new(std::cell::RefCell::new(Vec::new()));
    let records = Rc::clone(&dismissed);
    dock.on_tooltip_dismissed(move |delayed, origin| records.borrow_mut().push((delayed, origin)));
    dock.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
    window.set_size(slint::PhysicalSize::new(496, 144));
    let _ = draw(&window, 496, 144);
    let tile = ElementHandle::find_by_accessible_label(&dock, &format!("Launch {label}"))
        .next()
        .unwrap();
    let origin = tile.absolute_position();
    let point = slint::LogicalPosition::new(origin.x + 10.0, origin.y + 10.0);
    window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: point });
    slint::platform::update_timers_and_animations();
    let requested = hints.borrow();
    assert_eq!(requested.len(), 1);
    assert_eq!(requested[0].0, label);
    assert_eq!(requested[0].1.origin, origin);
    assert_eq!((requested[0].1.width, requested[0].1.height), (40.0, 40.0));
    drop(requested);
    window.window().dispatch_event(WindowEvent::PointerPressed {
        position: point,
        button: PointerEventButton::Left,
    });
    assert_eq!(dismissed.borrow().last(), Some(&(false, origin)));
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: point,
            button: PointerEventButton::Left,
        });
    window.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    slint::platform::update_timers_and_animations();
    assert_eq!(dismissed.borrow().last(), Some(&(true, origin)));
    window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: point });
    slint::platform::update_timers_and_animations();
    dismissed.borrow_mut().clear();
    let mut status = dock.get_surface_status();
    status.refreshing = true;
    dock.set_surface_status(status);
    slint::platform::update_timers_and_animations();
    assert!(dismissed.borrow().contains(&(false, origin)));
    let mut status = dock.get_surface_status();
    status.refreshing = false;
    dock.set_surface_status(status);
    window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: point });
    slint::platform::update_timers_and_animations();
    dismissed.borrow_mut().clear();
    window
        .window()
        .dispatch_event(WindowEvent::PointerScrolled {
            position: point,
            delta_x: -48.0,
            delta_y: 0.0,
        });
    slint::platform::update_timers_and_animations();
    assert!(
        dismissed.borrow().iter().any(|(delayed, _)| !delayed),
        "wheel cancels stale native hints immediately"
    );
    assert!(
        tile.absolute_position().x < origin.x,
        "rejecting the tile wheel event preserves real parent scrolling"
    );
}

#[test]
fn popover_show_motion_settles_cancels_and_skips_when_not_permitted() {
    let clock = Rc::new(Cell::new(Duration::ZERO));
    let window = software_window_with_clock(clock.clone());
    let tooltip = TooltipSurface::new().unwrap();
    tooltip.apply_presentation_theme(PresentationTheme::uniform(
        slint::language::ColorScheme::Dark,
    ));
    tooltip.set_content("Native hover".into());
    tooltip.reset_presentation();
    tooltip.show().unwrap();
    let width = tooltip.get_tooltip_width().ceil() as u32;
    let height = tooltip.get_tooltip_height().ceil() as u32;
    window.set_size(slint::PhysicalSize::new(width, height));
    let index = height as usize / 2 * width as usize + 12;
    let pixel = |frame: &[Rgb8Pixel]| [frame[index].r, frame[index].g, frame[index].b];
    assert_eq!(pixel(&draw(&window, width, height)), [0, 0, 0]);

    tooltip.reveal(true);
    assert_eq!(pixel(&draw(&window, width, height)), [0, 0, 0]);
    clock.set(Duration::from_millis(75));
    slint::platform::update_timers_and_animations();
    let middle = pixel(&draw(&window, width, height));
    assert!(middle.iter().all(|channel| *channel > 0 && *channel < 24));
    clock.set(Duration::from_millis(150));
    slint::platform::update_timers_and_animations();
    let settled = draw(&window, width, height);
    assert_eq!(pixel(&settled), [24, 24, 24]);
    assert!(!window.window().has_active_animations());
    assert!(!window.draw_if_needed(|renderer| {
        let mut pixels = settled.clone();
        renderer.render(&mut pixels, width as usize);
    }));
    export_screenshot(
        "tooltip-show-settled",
        &settled,
        width as usize,
        height as usize,
    );

    tooltip.reset_presentation();
    assert_eq!(pixel(&draw(&window, width, height)), [0, 0, 0]);
    tooltip.reveal(true);
    draw(&window, width, height);
    clock.set(Duration::from_millis(225));
    slint::platform::update_timers_and_animations();
    draw(&window, width, height);
    tooltip.reset_presentation();
    assert_eq!(pixel(&draw(&window, width, height)), [0, 0, 0]);
    // Slint's driver caches activity for the current tick. Cancellation has
    // already drawn zero opacity; the next loop tick clears the prior flag.
    clock.set(Duration::from_millis(226));
    slint::platform::update_timers_and_animations();
    assert!(!window.window().has_active_animations());

    tooltip.reveal(false);
    let skipped = draw(&window, width, height);
    assert_eq!(pixel(&skipped), [24, 24, 24]);
    assert!(!window.window().has_active_animations());
    assert!(!window.draw_if_needed(|renderer| {
        let mut pixels = skipped.clone();
        renderer.render(&mut pixels, width as usize);
    }));
}
