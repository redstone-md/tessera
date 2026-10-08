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

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, Rgb8Pixel, VecModel};

use crate::generated::{
    ContextMenuSurface, Dock, DockApp, DockMenuAction, DockMenuKind, DockStatus, DockWindow,
    LaunchRow, LaunchTile, Launcher, LauncherNavigation, LauncherView, Toolbar, TooltipSurface,
};
use crate::icons::IconCache;
use crate::launcher::{LauncherInventory, LauncherRows, LauncherSelection, Navigation};
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

fn assert_rgb_overlay(actual: [u8; 3], source: [u8; 3], background: [u8; 3], alpha: f32) {
    for ((actual, source), base) in actual.into_iter().zip(source).zip(background) {
        let expected = (f32::from(source) * alpha + f32::from(base) * (1.0 - alpha)).round() as u8;
        assert!(
            actual.abs_diff(expected) <= 2,
            "reference overlay alpha {alpha}: expected {expected}, actual {actual}",
        );
    }
}

fn app(key: &str, label: &str) -> LaunchTile {
    LaunchTile {
        key: key.into(),
        label: label.into(),
        icon: slint::Image::default(),
        favorite: false,
    }
}

/// Short row fixtures preserve native ListView layout without changing the
/// existing standalone tests into image-cache/model tests.
fn set_launcher_tiles(launcher: &Launcher, tiles: Vec<LaunchTile>) {
    launcher.set_application_count(tiles.len() as i32);
    let columns = launcher.get_grid_columns() as usize;
    let rows = tiles
        .chunks(columns)
        .map(|tiles| LaunchRow {
            tiles: ModelRc::new(VecModel::from(tiles.to_vec())),
        })
        .collect::<Vec<_>>();
    launcher.set_rows(ModelRc::new(VecModel::from(rows)));
}

fn native_key(window: &MinimalSoftwareWindow, text: slint::SharedString) {
    window
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    window
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text });
}

fn native_click(window: &MinimalSoftwareWindow, element: &ElementHandle) {
    let position = element.absolute_position();
    let size = element.size();
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
}

fn launch_elements(launcher: &Launcher) -> Vec<ElementHandle> {
    ElementQuery::from_root(launcher)
        .match_predicate(|element| {
            element
                .accessible_label()
                .is_some_and(|label| label.starts_with("Launch Inventory "))
        })
        .find_all()
}

fn favorite_elements(launcher: &Launcher) -> Vec<ElementHandle> {
    ElementQuery::from_root(launcher)
        .match_predicate(|element| {
            element.accessible_label().is_some_and(|label| {
                label.starts_with("Add to favorites: Inventory ")
                    || label.starts_with("Remove from favorites: Inventory ")
            })
        })
        .find_all()
}

/// Public element queries omit viewport-clipped items. A launch square can
/// still intersect the viewport after its 16px favorite corner is clipped.
/// Match the exact expected corner identities, not a row-alignment count.
fn assert_favorite_viewport_visibility(launcher: &Launcher) -> usize {
    let viewport = ElementHandle::find_by_element_id(launcher, "ScrollView::flickable")
        .next()
        .expect("the native list has its pinned ScrollView viewport");
    let origin = viewport.absolute_position();
    let size = viewport.size();
    let expected = launch_elements(launcher)
        .iter()
        .filter(|tile| {
            let position = tile.absolute_position();
            let tile_size = tile.size();
            let corner_x = position.x + tile_size.width - 16.0;
            let corner_y = position.y;
            // Slint 1.18.1 ItemRc::is_visible includes touching boundaries.
            corner_x <= origin.x + size.width
                && corner_x + 16.0 >= origin.x
                && corner_y <= origin.y + size.height
                && corner_y + 16.0 >= origin.y
        })
        .map(inventory_index)
        .collect::<std::collections::BTreeSet<_>>();
    let favorites = favorite_elements(launcher);
    let actual = favorites
        .iter()
        .map(|favorite| {
            let label = favorite.accessible_label().unwrap();
            label
                .strip_prefix("Add to favorites: Inventory ")
                .or_else(|| label.strip_prefix("Remove from favorites: Inventory "))
                .unwrap()
                .parse::<usize>()
                .unwrap()
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        actual.len(),
        favorites.len(),
        "each exposed native favorite has one exact application identity"
    );
    assert_eq!(
        actual, expected,
        "queried favorite identities must exactly match the native 16px corner/viewport intersections"
    );
    favorites.len()
}

fn inventory_index(element: &ElementHandle) -> usize {
    element
        .accessible_label()
        .unwrap()
        .strip_prefix("Launch Inventory ")
        .unwrap()
        .parse()
        .unwrap()
}

fn inventory_tile(launcher: &Launcher, index: usize) -> ElementHandle {
    ElementHandle::find_by_accessible_label(launcher, &format!("Launch Inventory {index}"))
        .next()
        .unwrap_or_else(|| panic!("inventory tile {index} must be exposed in the native viewport"))
}

fn inventory_key(index: usize) -> String {
    format!("opaque::{index:04}::retained/launch")
}

/// The actual immutable row adapter and bounded production cache are under
/// test. Only the host's selection/callback seam is supplied by this fixture.
struct NativeLauncherInventory {
    launcher: Launcher,
    inventory: Rc<LauncherInventory>,
    converted: Rc<RefCell<std::collections::BTreeSet<usize>>>,
    launches: Rc<RefCell<Vec<String>>>,
    favorites: Rc<RefCell<Vec<(String, bool)>>>,
}

impl NativeLauncherInventory {
    fn new(count: usize) -> Self {
        let launcher = Launcher::new().unwrap();
        let applications = (0..count)
            .map(|index| crate::projection::AppProjection {
                key: inventory_key(index),
                label: format!("Inventory {index}"),
                pinned: false,
                icon: Some(
                    crate::PixelIcon::new(1, 1, vec![index as u8, (index >> 8) as u8, 180, 255])
                        .unwrap(),
                ),
            })
            .collect();
        let inventory = Rc::new(LauncherInventory::new(applications));
        let converted = Rc::new(RefCell::new(std::collections::BTreeSet::new()));
        let visits = converted.clone();
        let cache = Rc::new(RefCell::new(IconCache::default()));
        let rows = LauncherRows::new(
            inventory.clone(),
            vec![inventory_key(count.saturating_sub(1))],
            launcher.get_grid_columns() as usize,
            move |icon| {
                let icon = icon.expect("every inventory fixture has unique native pixels");
                let bytes = icon.rgba();
                visits
                    .borrow_mut()
                    .insert(usize::from(bytes[0]) | (usize::from(bytes[1]) << 8));
                cache.borrow_mut().image(icon)
            },
        );
        launcher.set_view(LauncherView::All);
        launcher.set_application_count(inventory.len() as i32);
        launcher.set_rows(ModelRc::new(rows));

        let selection = Rc::new(RefCell::new(LauncherSelection::default()));
        let weak = launcher.as_weak();
        let state = selection.clone();
        let current = inventory.clone();
        launcher.on_select_requested(move |key| {
            let launcher = weak.upgrade().unwrap();
            Self::update_selection(&launcher, &state, |state| {
                state.select(current.keys(), &key)
            });
        });
        let weak = launcher.as_weak();
        let state = selection.clone();
        let current = inventory.clone();
        launcher.on_navigate_requested(move |direction| {
            let launcher = weak.upgrade().unwrap();
            let direction = match direction {
                LauncherNavigation::Up => Navigation::Up,
                LauncherNavigation::Down => Navigation::Down,
                LauncherNavigation::Left => Navigation::Left,
                LauncherNavigation::Right => Navigation::Right,
            };
            Self::update_selection(&launcher, &state, |state| {
                state.navigate(
                    current.keys(),
                    direction,
                    launcher.get_grid_columns() as usize,
                )
            });
        });
        let weak = launcher.as_weak();
        let state = selection.clone();
        let current = inventory.clone();
        launcher.on_search_changed(move || {
            let launcher = weak.upgrade().unwrap();
            Self::update_selection(&launcher, &state, |state| {
                state.search_changed(current.keys(), !launcher.get_search().is_empty())
            });
        });
        let launches = Rc::new(RefCell::new(Vec::new()));
        let log = launches.clone();
        let current = inventory.clone();
        launcher.on_launch_requested(move |key| {
            assert!(current.keys().iter().any(|current| current == key.as_str()));
            log.borrow_mut().push(key.to_string());
        });
        let weak = launcher.as_weak();
        let log = launches.clone();
        let current = inventory.clone();
        launcher.on_activate_selected_requested(move || {
            let key = weak.upgrade().unwrap().get_selected_key().to_string();
            assert!(current.keys().contains(&key));
            log.borrow_mut().push(key);
        });
        let favorites = Rc::new(RefCell::new(Vec::new()));
        let log = favorites.clone();
        launcher.on_favorite_toggle_requested(move |key, desired| {
            log.borrow_mut().push((key.to_string(), desired));
        });
        Self {
            launcher,
            inventory,
            converted,
            launches,
            favorites,
        }
    }

    fn update_selection(
        launcher: &Launcher,
        state: &RefCell<LauncherSelection>,
        update: impl FnOnce(&mut LauncherSelection) -> Option<usize>,
    ) {
        let (key, index) = {
            let mut state = state.borrow_mut();
            let index = update(&mut state);
            (state.key().unwrap_or_default().to_owned(), index)
        };
        launcher.set_selected_key(key.into());
        if let Some(index) = index {
            launcher.invoke_ensure_visible(index as i32);
        }
    }

    fn show(&self, window: &MinimalSoftwareWindow, width: u32, height: u32) {
        self.launcher.show().unwrap();
        window
            .window()
            .dispatch_event(WindowEvent::WindowActiveChanged(true));
        window.set_size(slint::PhysicalSize::new(width, height));
        self.launcher.invoke_focus_search();
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
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Space.into(),
    });
    assert_eq!(launches.get(), 3, "holding Space must not launch");

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

    dock.set_surface_status(DockStatus::default());
    assert_eq!(launch.accessible_enabled(), Some(true));
    window
        .window()
        .dispatch_event(WindowEvent::KeyPressRepeated {
            text: Key::Space.into(),
        });
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Space.into(),
    });
    assert_eq!(
        launches.get(),
        3,
        "disable/re-enable cancels the earlier held Space",
    );
    drop(dock);
}

#[test]
fn dock_pointer_and_keyboard_focus_have_reference_outline_geometry_at_both_scales() {
    let window = software_window();
    let dock = Dock::new().unwrap();
    let opens = Rc::new(Cell::new(0));
    let count = opens.clone();
    dock.on_open_applications_requested(move || count.set(count.get() + 1));
    dock.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));

    for theme in [
        slint::language::ColorScheme::Light,
        slint::language::ColorScheme::Dark,
    ] {
        dock.apply_presentation_theme(PresentationTheme::uniform(theme));
        for scale in [1.0_f32, 2.0] {
            window
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
            let width = (248.0 * scale) as u32;
            let height = (72.0 * scale) as u32;
            window.set_size(slint::PhysicalSize::new(width, height));
            window.window().dispatch_event(WindowEvent::PointerExited);
            let baseline = draw(&window, width, height);
            let start =
                ElementHandle::find_by_accessible_label(&dock, "Open applications and settings")
                    .next()
                    .unwrap();
            let position = start.absolute_position();
            let size = start.size();
            let center = slint::LogicalPosition::new(
                position.x + size.width / 2.0,
                position.y + size.height / 2.0,
            );
            // Sample the straight edge, away from corner antialiasing:
            // two empty logical pixels, then the two-pixel outline.
            let sample = |pixels: &[Rgb8Pixel], offset: f32| {
                let x = ((position.x - offset) * scale) as usize;
                let y = (center.y * scale) as usize;
                pixels[y * width as usize + x]
            };
            let bar_color = sample(&baseline, 5.5);
            let click = || {
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
            };
            let before = opens.get();
            click();
            assert_eq!(opens.get(), before + 1);
            let pointer = draw(&window, width, height);
            for offset in [0.5, 1.5, 2.5, 3.5, 4.5] {
                assert_eq!(
                    sample(&pointer, offset),
                    bar_color,
                    "pointer focus must not paint a keyboard outline ({theme:?}, {scale}x)",
                );
            }

            window.window().dispatch_event(WindowEvent::KeyPressed {
                text: Key::Space.into(),
            });
            assert_eq!(
                opens.get(),
                before + 1,
                "Space must arm the actual button, not activate before release",
            );
            window
                .window()
                .dispatch_event(WindowEvent::KeyPressRepeated {
                    text: Key::Space.into(),
                });
            assert_eq!(
                opens.get(),
                before + 1,
                "held-Space repeat must not dispatch extra actions",
            );
            window.window().dispatch_event(WindowEvent::KeyReleased {
                text: Key::Space.into(),
            });
            assert_eq!(opens.get(), before + 2, "keyboard activation is preserved");
            let keyboard = draw(&window, width, height);
            for offset in [0.5, 1.5, 4.5] {
                assert_eq!(
                    sample(&keyboard, offset),
                    bar_color,
                    "outline keeps a two-pixel gap and bounded outer edge",
                );
            }
            assert_ne!(
                sample(&keyboard, 2.5),
                bar_color,
                "keyboard outline starts two logical pixels outside the tile",
            );
            assert_eq!(
                sample(&keyboard, 2.5),
                sample(&keyboard, 3.5),
                "the outline is exactly two logical pixels wide",
            );
            window.window().dispatch_event(WindowEvent::KeyPressed {
                text: Key::Space.into(),
            });
            assert_eq!(opens.get(), before + 2);
            let held = draw(&window, width, height);
            assert_ne!(
                sample(&held, -2.0),
                sample(&keyboard, -2.0),
                "held Space must paint the shared pressed state",
            );

            window
                .window()
                .dispatch_event(WindowEvent::WindowActiveChanged(false));
            let inactive = draw(&window, width, height);
            assert_eq!(sample(&inactive, 2.5), bar_color);
            window
                .window()
                .dispatch_event(WindowEvent::WindowActiveChanged(true));
            window
                .window()
                .dispatch_event(WindowEvent::KeyPressRepeated {
                    text: Key::Space.into(),
                });
            window.window().dispatch_event(WindowEvent::KeyReleased {
                text: Key::Space.into(),
            });
            assert_eq!(
                opens.get(),
                before + 2,
                "losing window focus cancels a held Space without activation",
            );
            let keyboard_restored = draw(&window, width, height);
            assert_eq!(sample(&keyboard_restored, 2.5), sample(&keyboard, 2.5));

            // A click on an already-focused tile must switch back to pointer
            // presentation even though no focus-gained callback runs again.
            click();
            assert_eq!(opens.get(), before + 3);
            let pointer_again = draw(&window, width, height);
            assert_eq!(sample(&pointer_again, 2.5), bar_color);
            window
                .window()
                .dispatch_event(WindowEvent::WindowActiveChanged(false));
            window
                .window()
                .dispatch_event(WindowEvent::WindowActiveChanged(true));
            let mut pointer_restored = pointer_again;
            window.draw_if_needed(|renderer| {
                renderer.render(&mut pointer_restored, width as usize);
            });
            assert_eq!(sample(&pointer_restored, 2.5), bar_color);
            assert!(
                !window.draw_if_needed(|_| panic!("unchanged focus must not redraw")),
                "no idle repaint after focus presentation settles",
            );
        }
    }
}

#[test]
fn dock_tab_navigation_activates_real_tiles_and_skips_disabled_items() {
    let window = software_window();
    let dock = Dock::new().unwrap();
    dock.set_pinned_apps(ModelRc::new(VecModel::from(vec![DockApp {
        key: "editor".into(),
        label: "Editor".into(),
        icon: slint::Image::default(),
        pinned: true,
    }])));
    let opens = Rc::new(Cell::new(0));
    let count = opens.clone();
    dock.on_open_applications_requested(move || count.set(count.get() + 1));
    let launches = Rc::new(Cell::new(0));
    let count = launches.clone();
    dock.on_launch_requested(move |key| {
        assert_eq!(key, "editor");
        count.set(count.get() + 1);
    });
    dock.show().unwrap();
    window.set_size(slint::PhysicalSize::new(248, 72));
    window
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    let baseline = draw(&window, 248, 72);
    let start = ElementHandle::find_by_accessible_label(&dock, "Open applications and settings")
        .next()
        .unwrap();
    let position = start.absolute_position();
    let outline_index =
        (position.y + start.size().height / 2.0) as usize * 248 + (position.x - 2.5) as usize;
    let press = |text: slint::SharedString| {
        window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    };
    press(Key::Tab.into());
    let tab_focused = draw(&window, 248, 72);
    assert_ne!(
        tab_focused[outline_index], baseline[outline_index],
        "Tab focus paints the keyboard outline before activation",
    );
    press(Key::Return.into());
    assert_eq!((opens.get(), launches.get()), (1, 0));
    press(Key::Tab.into());
    press(Key::Return.into());
    assert_eq!((opens.get(), launches.get()), (1, 1));
    dock.set_surface_status(DockStatus {
        refreshing: true,
        ..DockStatus::default()
    });
    press(Key::Tab.into());
    press(Key::Return.into());
    assert_eq!((opens.get(), launches.get()), (2, 1));
    press(Key::Tab.into());
    press(Key::Return.into());
    assert_eq!(
        (opens.get(), launches.get()),
        (3, 1),
        "disabled tiles are skipped, not dead keyboard stops",
    );
}

#[test]
fn toolbar_renders_identity_and_settings_access() {
    use slint::language::ColorScheme;
    let window = software_window();
    let toolbar = Toolbar::new().unwrap();
    toolbar.set_user_name("alice".into());
    toolbar.set_focused_app("Editor — window".into());
    toolbar.set_clock("12:34".into());
    toolbar.set_language("en-US".into());
    toolbar
        .global::<crate::generated::SeelenPalette>()
        .set_accent(slint::Color::from_rgb_u8(37, 171, 86).into());
    toolbar.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    let opened = Rc::new(Cell::new(0));
    let counter = opened.clone();
    toolbar.on_quick_settings_requested(move |_| counter.set(counter.get() + 1));
    let hints = Rc::new(std::cell::RefCell::new(Vec::new()));
    let records = Rc::clone(&hints);
    toolbar
        .on_tooltip_requested(move |content, bounds| records.borrow_mut().push((content, bounds)));
    for (scheme, scale, width, height) in [
        (ColorScheme::Light, 1.0, 640u32, 32u32),
        (ColorScheme::Light, 2.0, 1280, 64),
        (ColorScheme::Dark, 1.0, 640, 32),
        (ColorScheme::Dark, 2.0, 1280, 64),
    ] {
        toolbar.apply_presentation_theme(PresentationTheme::uniform(scheme));
        window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        window.set_size(slint::PhysicalSize::new(width, height));
        window.window().dispatch_event(WindowEvent::PointerExited);
        window.request_redraw();
        let pixels = draw(&window, width, height);
        let settings = ElementHandle::find_by_accessible_label(&toolbar, "Open quick settings")
            .next()
            .unwrap();
        assert_eq!(settings.accessible_role(), Some(AccessibleRole::Button));
        let origin = settings.absolute_position();
        let size = settings.size();
        assert_eq!(origin.y, 8.0);
        assert_eq!((size.width, size.height), (16.0, 16.0));
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
        let center = slint::LogicalPosition::new(origin.x + 8.0, origin.y + 8.0);
        // Straight edge outside the independent SVG's stroke bounds.
        let index =
            (center.y * scale) as usize * width as usize + ((origin.x + 0.5) * scale) as usize;
        let base = pixels[index];
        assert_eq!(base, pixels[0], "idle Settings has no filled background");
        let accent = toolbar
            .global::<crate::generated::SeelenPalette>()
            .get_accent()
            .color()
            .to_argb_u8();
        let overlay = |frame: &[Rgb8Pixel], alpha| {
            let pixel = frame[index];
            assert_rgb_overlay(
                [pixel.r, pixel.g, pixel.b],
                [accent.red, accent.green, accent.blue],
                [base.r, base.g, base.b],
                alpha,
            );
        };
        window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position: center });
        let hovered = draw(&window, width, height);
        overlay(&hovered, 0.2);
        let requested = hints.borrow();
        let (content, bounds) = requested
            .last()
            .expect("actual settings hover requests a hint");
        assert_eq!(content, "Quick settings");
        assert_eq!(bounds.origin, origin);
        assert_eq!((bounds.width, bounds.height), (16.0, 16.0));
        drop(requested);
        let before = opened.get();
        window.window().dispatch_event(WindowEvent::PointerPressed {
            position: center,
            button: PointerEventButton::Left,
        });
        assert_eq!(opened.get(), before);
        let pressed = draw(&window, width, height);
        overlay(&pressed, 0.3);
        assert_eq!(
            settings.absolute_position(),
            origin,
            "toolbar div has no button translation"
        );
        assert_eq!(
            settings.size(),
            size,
            "toolbar icon geometry stays unchanged"
        );
        window
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position: center,
                button: PointerEventButton::Left,
            });
        assert_eq!(opened.get(), before + 1);
        overlay(&draw(&window, width, height), 0.2);
        window.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Space.into(),
        });
        window
            .window()
            .dispatch_event(WindowEvent::KeyPressRepeated {
                text: Key::Space.into(),
            });
        assert_eq!(
            opened.get(),
            before + 1,
            "held/repeated Space does not open Settings"
        );
        overlay(&draw(&window, width, height), 0.3);
        assert_eq!(settings.absolute_position(), origin);
        assert_eq!(settings.size(), size);
        window.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Space.into(),
        });
        assert_eq!(opened.get(), before + 2);
        overlay(&draw(&window, width, height), 0.2);
        window.window().dispatch_event(WindowEvent::PointerExited);
        let idle = draw(&window, width, height);
        assert_eq!(
            idle[index], base,
            "release and leave restore transparent idle"
        );
        settings.invoke_accessible_default_action();
        assert_eq!(opened.get(), before + 3);
        assert!(!window.draw_if_needed(|_| panic!("settled toolbar must not redraw")));
        export_screenshot(
            &format!("toolbar-{scheme:?}-{scale}x"),
            &pixels,
            width as usize,
            height as usize,
        );
    }
    assert_eq!(
        opened.get(),
        12,
        "three real activation paths in all themes/scales"
    );
    drop(toolbar);
}

#[test]
fn launcher_renders_grid_search_and_escape_hides() {
    let window = software_window();
    let launcher = Launcher::new().unwrap();
    set_launcher_tiles(
        &launcher,
        vec![
            app("app-editor", "Rust Editor"),
            app("app-browser", "Web Browser"),
        ],
    );
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
fn launcher_header_requests_real_views_and_reset_scroll_preserves_native_focus() {
    use std::cell::RefCell;

    let window = software_window();
    let launcher = Launcher::new().unwrap();
    assert_eq!(launcher.get_view(), LauncherView::Favorites);
    let views = Rc::new(RefCell::new(Vec::new()));
    let log = views.clone();
    launcher.on_view_requested(move |view| log.borrow_mut().push(view));
    let launches = Rc::new(RefCell::new(Vec::new()));
    let log = launches.clone();
    launcher.on_launch_requested(move |key| log.borrow_mut().push(key));
    let selected_activations = Rc::new(Cell::new(0));
    let count = selected_activations.clone();
    launcher.on_activate_selected_requested(move || count.set(count.get() + 1));
    launcher.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    let key = |text: slint::SharedString| {
        window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    };
    let click = |element: &ElementHandle| {
        let position = element.absolute_position();
        let size = element.size();
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
    };
    for scale in [1.0_f32, 2.0] {
        window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        let width = (560.0 * scale) as u32;
        let height = (300.0 * scale) as u32;
        window.set_size(slint::PhysicalSize::new(width, height));
        launcher.set_view(LauncherView::Favorites);
        launcher.set_search("".into());
        launcher.set_selected_key("".into());
        let mut editor = app("editor", "Editor");
        editor.favorite = true;
        set_launcher_tiles(&launcher, vec![editor.clone()]);
        launcher.set_saved_favorites_present(true);
        launcher.invoke_reset_scroll();
        launcher.invoke_focus_search();
        let _ = draw(&window, width, height);
        views.borrow_mut().clear();
        launches.borrow_mut().clear();
        let all = ElementHandle::find_by_accessible_label(&launcher, "All Apps")
            .next()
            .unwrap();
        assert_eq!(all.accessible_role(), Some(AccessibleRole::Button));
        click(&all);
        assert_eq!(views.borrow().as_slice(), &[LauncherView::All]);
        // The pinned stock Button's pointer handler does not take focus.
        // Native Tab moves from the still-focused search field to the header.
        key(Key::Tab.into());
        key(Key::Return.into());
        key(Key::Space.into());
        assert_eq!(views.borrow().as_slice(), &[LauncherView::All; 3]);
        assert_eq!(
            launcher.get_view(),
            LauncherView::Favorites,
            "requests do not optimistically change view"
        );
        assert!(
            launches.borrow().is_empty(),
            "header Return/Space never launches"
        );

        // The parent accepts the requested view and supplies its real data.
        launcher.set_view(LauncherView::All);
        set_launcher_tiles(
            &launcher,
            (0..23)
                .map(|i| app(&format!("app-{i}"), &format!("App {i}")))
                .collect(),
        );
        launcher.invoke_reset_scroll();
        launcher.invoke_focus_search();
        let _ = draw(&window, width, height);
        assert_eq!(
            views.borrow().len(),
            3,
            "programmatic view/model changes are silent"
        );
        assert!(
            ElementHandle::find_by_accessible_label(&launcher, "Launch Editor")
                .next()
                .is_none()
        );
        assert!(
            ElementHandle::find_by_accessible_label(&launcher, "All Apps")
                .next()
                .is_none()
        );
        let first = ElementHandle::find_by_accessible_label(&launcher, "Launch App 0")
            .next()
            .unwrap();
        let initial_position = first.absolute_position();
        // Offscreen delegates may be destroyed; never use their handles to
        // infer viewport motion or focus preservation.
        launcher.invoke_ensure_visible(22);
        let _ = draw(&window, width, height);
        let last = ElementHandle::find_by_accessible_label(&launcher, "Launch App 22")
            .next()
            .unwrap();
        assert!(last.absolute_position().y > initial_position.y);
        assert!(
            ElementHandle::find_by_accessible_label(&launcher, "Launch App 0")
                .next()
                .is_none(),
            "ensure-visible moves the first row out of the real viewport"
        );
        launcher.invoke_reset_scroll();
        let _ = draw(&window, width, height);
        let first = ElementHandle::find_by_accessible_label(&launcher, "Launch App 0")
            .next()
            .unwrap();
        assert_eq!(
            first.absolute_position(),
            initial_position,
            "reset-scroll returns the native viewport to its top"
        );
        click(&first);
        window
            .window()
            .dispatch_event(WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(
                    initial_position.x + first.size().width / 2.0,
                    initial_position.y + first.size().height / 2.0,
                ),
                delta_x: 0.0,
                delta_y: -8.0,
            });
        let _ = draw(&window, width, height);
        assert!(
            first.is_valid(),
            "a small scroll retains the genuinely focused row"
        );
        assert!(
            (first.absolute_position().y - initial_position.y + 8.0).abs() < 0.01,
            "the focus-preservation check starts from a nonzero native scroll offset"
        );
        launcher.invoke_reset_scroll();
        key(Key::Space.into());
        assert_eq!(
            launches.borrow().as_slice(),
            &[
                slint::SharedString::from("app-0"),
                slint::SharedString::from("app-0")
            ],
            "reset-scroll does not steal the tile's actual keyboard focus",
        );

        let back = ElementHandle::find_by_accessible_label(&launcher, "Back to favorites")
            .next()
            .unwrap();
        click(&back);
        assert_eq!(views.borrow().last(), Some(&LauncherView::Favorites));
        assert_eq!(
            launcher.get_view(),
            LauncherView::All,
            "Back is also host-accepted"
        );
        launcher.set_view(LauncherView::Favorites);
        set_launcher_tiles(&launcher, vec![editor]);
        launcher.set_selected_key("".into());
        launcher.set_search("".into());
        launcher.invoke_reset_scroll();
        launcher.invoke_focus_search();
        let _ = draw(&window, width, height);
        key("q".into());
        assert_eq!(
            launcher.get_search(),
            "q",
            "accepted transition restores native search focus"
        );
        assert_eq!(
            views.borrow().len(),
            4,
            "projection/reset/focus/search emit no navigation request"
        );
        assert_eq!(selected_activations.get(), 0);
        assert_eq!(launches.borrow().len(), 2);
    }
}

#[test]
fn launcher_empty_unavailable_and_no_match_states_are_distinct_and_keep_recovery() {
    let window = software_window();
    let launcher = Launcher::new().unwrap();
    launcher.show().unwrap();
    let messages = [
        "Welcome to Tessera.\nYour favorite applications appear here. Open All Apps to add favorites.",
        "Saved favorites are unavailable. Refresh to check installed applications.",
        "No matching applications.",
        "Working; results appear after the current refresh.",
    ];
    for scale in [1.0_f32, 2.0] {
        window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        let width = (560.0 * scale) as u32;
        let height = (420.0 * scale) as u32;
        window.set_size(slint::PhysicalSize::new(width, height));
        let mut previous_pixels = None;
        for (index, view, saved, refreshing) in [
            (0, LauncherView::Favorites, false, false),
            (1, LauncherView::Favorites, true, false),
            (2, LauncherView::All, true, false),
            (3, LauncherView::All, true, true),
        ] {
            launcher.set_view(view);
            launcher.set_saved_favorites_present(saved);
            launcher.set_refreshing(refreshing);
            launcher.set_search(if view == LauncherView::All {
                "missing".into()
            } else {
                "".into()
            });
            let pixels = draw(&window, width, height);
            let message = ElementHandle::find_by_accessible_label(&launcher, messages[index])
                .next()
                .unwrap();
            assert_eq!(message.accessible_role(), Some(AccessibleRole::Text));
            for (other, text) in messages.iter().enumerate() {
                if other != index {
                    assert!(
                        ElementHandle::find_by_accessible_label(&launcher, text)
                            .next()
                            .is_none()
                    );
                }
            }
            if let Some(previous) = previous_pixels {
                assert_ne!(
                    pixels, previous,
                    "different empty states must genuinely render different text"
                );
            }
            previous_pixels = Some(pixels);
            for label in ["Open settings and recovery", "Exit Tessera"] {
                let rescue = ElementHandle::find_by_accessible_label(&launcher, label)
                    .next()
                    .unwrap();
                assert_eq!(rescue.accessible_enabled(), Some(true));
                assert!(
                    message.absolute_position().y + message.size().height
                        < rescue.absolute_position().y
                );
            }
            let refresh = ElementHandle::find_by_accessible_label(&launcher, "Refresh the desktop")
                .next()
                .unwrap();
            assert_eq!(refresh.accessible_enabled(), Some(!refreshing));
            let navigation_label = if view == LauncherView::Favorites {
                "All Apps"
            } else {
                "Back to favorites"
            };
            let navigation = ElementHandle::find_by_accessible_label(&launcher, navigation_label)
                .next()
                .unwrap();
            assert_eq!(navigation.accessible_enabled(), Some(!refreshing));
        }
    }
}

#[test]
fn launcher_native_list_boundary_arrow_return_and_space_target_exact_new_keys_without_draw() {
    let window = software_window();
    let fixture = NativeLauncherInventory::new(1024);
    assert!(
        fixture.converted.borrow().is_empty(),
        "constructing the logical inventory does not produce Slint images"
    );
    fixture.show(&window, 560, 300);
    let _ = draw(&window, 560, 300);
    let initially_visible = launch_elements(&fixture.launcher);
    let last_visible_index = initially_visible.iter().map(inventory_index).max().unwrap();
    let last_converted_index = *fixture.converted.borrow().last().unwrap();
    let first_offscreen_row = last_visible_index.max(last_converted_index) / 7 + 1;
    let boundary_index = first_offscreen_row * 7;
    assert!(boundary_index < 64);
    assert!(
        !fixture.converted.borrow().contains(&boundary_index),
        "the boundary target is not an eagerly converted native row"
    );
    assert!(
        ElementHandle::find_by_accessible_label(
            &fixture.launcher,
            &format!("Launch Inventory {boundary_index}"),
        )
        .next()
        .is_none(),
        "the unconverted target is also absent from the initial visible native controls"
    );
    native_key(&window, Key::Tab.into()); // Genuine header control.
    native_key(&window, Key::Tab.into()); // Genuine first application tile.
    assert_eq!(fixture.launcher.get_selected_key(), inventory_key(0));

    // No draw, event-loop pump, getter, accessible query, or callback
    // invocation between these native arrows and the immediate Return.
    for _ in 0..first_offscreen_row {
        native_key(&window, Key::DownArrow.into());
    }
    native_key(&window, Key::Return.into());
    assert_eq!(
        fixture.launches.borrow().as_slice(),
        &[inventory_key(boundary_index)],
        "crossing a lazy native row boundary launches the new opaque key once"
    );
    assert!(fixture.converted.borrow().contains(&boundary_index));

    for _ in 0..first_offscreen_row {
        native_key(&window, Key::UpArrow.into());
    }
    native_key(&window, Key::Return.into());
    assert_eq!(
        fixture.launches.borrow().as_slice(),
        &[inventory_key(boundary_index), inventory_key(0)],
        "upward materialization restores actual tile focus without a draw"
    );
    for _ in 0..20 {
        native_key(&window, Key::DownArrow.into());
    }
    native_key(&window, Key::Return.into());
    assert_eq!(
        fixture.launches.borrow().as_slice(),
        &[
            inventory_key(boundary_index),
            inventory_key(0),
            inventory_key(140)
        ],
        "rapid arrows reach the full retained tail beyond the former UI cap"
    );
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Space.into(),
    });
    for _ in 0..3 {
        native_key(&window, Key::DownArrow.into());
    }
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Space.into(),
    });
    assert_eq!(fixture.launcher.get_selected_key(), inventory_key(161));
    assert_eq!(
        fixture.launches.borrow().len(),
        3,
        "evicting an armed Space tile launches neither its old key nor its replacement"
    );
    native_key(&window, Key::Tab.into()); // Current tile's own favorite.
    native_key(&window, Key::Return.into());
    native_key(&window, Key::Space.into());
    assert_eq!(
        fixture.favorites.borrow().as_slice(),
        &[(inventory_key(161), true), (inventory_key(161), true)],
        "real Tab reaches the newly materialized favorite and keeps activation isolated"
    );
    assert_eq!(fixture.launches.borrow().len(), 3);
    native_key(&window, Key::Tab.into()); // Next currently instantiated tile.
    native_key(&window, Key::Return.into());
    assert_eq!(fixture.launches.borrow().last(), Some(&inventory_key(162)));
    assert_eq!(fixture.launches.borrow().len(), 4);
    assert!(
        !fixture.converted.borrow().contains(&1023),
        "logical selection/authorization never walks the unvisited image tail"
    );
}

#[test]
fn launcher_native_rows_bound_delegates_and_lazy_images_across_large_seeks() {
    let window = software_window();
    let small_counts = {
        let fixture = NativeLauncherInventory::new(70);
        fixture.show(&window, 560, 300);
        let _ = draw(&window, 560, 300);
        let counts = (
            launch_elements(&fixture.launcher).len(),
            assert_favorite_viewport_visibility(&fixture.launcher),
        );
        assert!(counts.0 > 0 && counts.0 < 70);
        assert_eq!(counts.0, counts.1);
        counts
    };
    let fixture = NativeLauncherInventory::new(1024);
    fixture.show(&window, 560, 300);
    let _ = draw(&window, 560, 300);
    assert_eq!(fixture.inventory.len(), 1024);
    assert_eq!(
        (
            launch_elements(&fixture.launcher).len(),
            assert_favorite_viewport_visibility(&fixture.launcher),
        ),
        small_counts,
        "identical native geometry exposes identical controls, not catalog-sized visible UI"
    );
    let mut previous = launch_elements(&fixture.launcher);
    let initial_converted = fixture.converted.borrow().len();
    assert!(initial_converted < 64);
    assert!(!fixture.converted.borrow().contains(&1023));
    for target in [511, 1023, 126, 768, 0] {
        fixture.launcher.invoke_ensure_visible(target);
        let _ = draw(&window, 560, 300);
        let tile = inventory_tile(&fixture.launcher, target as usize);
        assert!(tile.size().height > 0.0);
        assert!(
            previous.iter().all(|element| !element.is_valid()),
            "native row eviction destroys former delegates, not merely their accessibility exposure"
        );
        let current = launch_elements(&fixture.launcher);
        assert!(
            current.len() <= small_counts.0 + 7,
            "only one partially intersecting native row may differ at another scroll offset"
        );
        assert_favorite_viewport_visibility(&fixture.launcher);
        assert!(fixture.converted.borrow().contains(&(target as usize)));
        previous = current;
    }
    assert!(fixture.converted.borrow().len() > initial_converted);
    assert!(
        fixture.converted.borrow().len() < 256,
        "isolated seeks do not eagerly convert the skipped native inventory"
    );
    // Visit more unique icons than the real cache capacity, while proving
    // exposed native controls stay bounded after earlier delegates died.
    for target in (0..1024).step_by(35).skip(1) {
        fixture.launcher.invoke_ensure_visible(target);
        let _ = draw(&window, 560, 300);
        assert!(launch_elements(&fixture.launcher).len() <= small_counts.0 + 7);
        assert!(assert_favorite_viewport_visibility(&fixture.launcher) <= small_counts.1 + 7);
    }
    assert!(fixture.converted.borrow().len() > 256);
    fixture.launcher.invoke_reset_scroll();
    let _ = draw(&window, 560, 300);
    assert_eq!(
        (
            launch_elements(&fixture.launcher).len(),
            assert_favorite_viewport_visibility(&fixture.launcher),
        ),
        small_counts,
        "returning after cache eviction restores the same bounded visible controls"
    );
    assert_eq!(
        inventory_tile(&fixture.launcher, 0).absolute_position().x,
        26.0
    );
}

#[test]
fn launcher_native_wheel_scrollbar_partial_tail_outlines_and_resize_preserve_geometry() {
    let window = software_window();
    let fixture = NativeLauncherInventory::new(1024);
    fixture.show(&window, 560, 300);
    for scheme in [
        slint::language::ColorScheme::Light,
        slint::language::ColorScheme::Dark,
    ] {
        fixture
            .launcher
            .apply_presentation_theme(PresentationTheme::uniform(scheme));
        for scale in [1.0_f32, 2.0] {
            window
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
            for (logical_width, logical_height) in [(560.0, 300.0), (640.0, 420.0)] {
                let width = (logical_width * scale) as u32;
                let height = (logical_height * scale) as u32;
                window.set_size(slint::PhysicalSize::new(width, height));
                fixture.launcher.invoke_reset_scroll();
                fixture.launcher.invoke_focus_search();
                let _ = draw(&window, width, height);
                let grid =
                    ElementHandle::find_by_element_id(&fixture.launcher, "Launcher::grid-scroll")
                        .next()
                        .unwrap();
                let grid_position = grid.absolute_position();
                let grid_size = grid.size();
                let wheel_position = slint::LogicalPosition::new(
                    grid_position.x + grid_size.width / 2.0,
                    grid_position.y + grid_size.height / 2.0,
                );
                window
                    .window()
                    .dispatch_event(WindowEvent::PointerScrolled {
                        position: wheel_position,
                        delta_x: 0.0,
                        delta_y: -100_000.0,
                    });
                let _ = draw(&window, width, height);
                let tail = inventory_tile(&fixture.launcher, 1023);
                let last_position = tail.absolute_position();
                let last_size = tail.size();
                assert!(last_size.height > 0.0);
                assert!((last_size.width - last_size.height).abs() < 0.01);
                assert!(
                    (last_position.y + last_size.height + 4.0
                        - (grid_position.y + grid_size.height))
                        .abs()
                        < 0.05,
                    "native wheel reaches the real final row with exactly its outline gutter, not a fake page gap"
                );
                let first_partial = inventory_tile(&fixture.launcher, 1022);
                assert!((first_partial.absolute_position().y - last_position.y).abs() < 0.01);
                assert!(
                    (last_position.x - first_partial.absolute_position().x - last_size.width - 8.0)
                        .abs()
                        < 0.01
                );
                let partial_count = launch_elements(&fixture.launcher)
                    .iter()
                    .filter(|element| inventory_index(element) >= 1022)
                    .count();
                assert_eq!(
                    partial_count, 2,
                    "the incomplete tail has no invented cells"
                );
                let favorite = ElementHandle::find_by_accessible_label(
                    &fixture.launcher,
                    "Remove from favorites: Inventory 1023",
                )
                .next()
                .unwrap();
                assert_eq!(favorite.accessible_checked(), Some(true));
                assert!(favorite.absolute_position().y >= grid_position.y);
                let footer = ElementHandle::find_by_accessible_label(
                    &fixture.launcher,
                    "Open settings and recovery",
                )
                .next()
                .unwrap();
                assert!(last_position.y + last_size.height + 4.0 < footer.absolute_position().y);

                // The last complete row exercises both outer columns at
                // the real native end-of-inventory scroll position.
                for index in [1015, 1021] {
                    let tile = inventory_tile(&fixture.launcher, index);
                    let position = tile.absolute_position();
                    let size = tile.size();
                    assert_eq!(size, last_size);
                    let edge = if index == 1015 {
                        position.x
                    } else {
                        position.x + size.width
                    };
                    assert!(
                        (edge
                            - if index == 1015 {
                                26.0
                            } else {
                                logical_width - 26.0
                            })
                        .abs()
                            < 0.01
                    );
                    native_click(&window, &tile);
                    let pixels = draw(&window, width, height);
                    let outside = edge + if index == 1015 { -2.5 } else { 2.5 };
                    let ring = ((position.y + size.height / 2.0) * scale) as usize * width as usize
                        + (outside * scale) as usize;
                    let accent = fixture
                        .launcher
                        .global::<crate::generated::SeelenPalette>()
                        .get_accent()
                        .color()
                        .to_argb_u8();
                    assert_eq!(
                        pixels[ring],
                        Rgb8Pixel {
                            r: accent.red,
                            g: accent.green,
                            b: accent.blue
                        },
                        "the full tail row's external column outline renders unclipped ({scheme:?}, {scale}x)"
                    );
                    assert_eq!(
                        inventory_tile(&fixture.launcher, index).absolute_position(),
                        position
                    );
                    assert_eq!(inventory_tile(&fixture.launcher, index).size(), size);
                }
                let before = inventory_tile(&fixture.launcher, 1023).absolute_position();
                fixture.launcher.invoke_ensure_visible(1023);
                fixture.launcher.invoke_ensure_visible(-1);
                fixture.launcher.invoke_ensure_visible(1024);
                assert_eq!(
                    inventory_tile(&fixture.launcher, 1023).absolute_position(),
                    before,
                    "visible and invalid full-inventory indices do not move the viewport"
                );

                // Exercise the actual pinned native scrollbar independently
                // of ensure-visible and wheel scrolling.
                fixture.launcher.invoke_reset_scroll();
                let _ = draw(&window, width, height);
                let bar = ElementHandle::find_by_element_type_name(&fixture.launcher, "ScrollBar")
                    .find(|element| element.size().height > element.size().width)
                    .expect("the pinned native vertical scrollbar must exist");
                let thumb =
                    ElementHandle::find_by_element_id(&fixture.launcher, "ScrollBar::thumb")
                        .find(|element| element.size().height > element.size().width)
                        .unwrap();
                let bar_position = bar.absolute_position();
                let bar_size = bar.size();
                let thumb_position = thumb.absolute_position();
                let thumb_size = thumb.size();
                let start = slint::LogicalPosition::new(
                    bar_position.x + bar_size.width / 2.0,
                    thumb_position.y + thumb_size.height / 2.0,
                );
                let end = slint::LogicalPosition::new(
                    start.x,
                    bar_position.y + bar_size.height - 16.0 - thumb_size.height / 2.0,
                );
                window
                    .window()
                    .dispatch_event(WindowEvent::PointerMoved { position: start });
                window.window().dispatch_event(WindowEvent::PointerPressed {
                    position: start,
                    button: PointerEventButton::Left,
                });
                window
                    .window()
                    .dispatch_event(WindowEvent::PointerMoved { position: end });
                window
                    .window()
                    .dispatch_event(WindowEvent::PointerReleased {
                        position: end,
                        button: PointerEventButton::Left,
                    });
                let _ = draw(&window, width, height);
                assert!(
                    (inventory_tile(&fixture.launcher, 1023)
                        .absolute_position()
                        .y
                        - last_position.y)
                        .abs()
                        < 0.05,
                    "dragging the real scrollbar reaches the same genuine final partial row"
                );
            }
        }
    }
}

#[test]
fn launcher_native_final_partial_row_keys_tab_footer_and_narrow_rows_remain_real() {
    let window = software_window();
    let fixture = NativeLauncherInventory::new(1024);
    fixture.show(&window, 560, 300);
    let _ = draw(&window, 560, 300);
    native_key(&window, Key::Tab.into());
    native_key(&window, Key::Tab.into());
    // Reach the complete retained inventory through native grid input, not
    // a synthetic callback or an eagerly traversed image-bearing model.
    for _ in 0..146 {
        native_key(&window, Key::DownArrow.into());
    }
    native_key(&window, Key::RightArrow.into());
    native_key(&window, Key::Return.into());
    assert_eq!(fixture.launches.borrow().as_slice(), &[inventory_key(1023)]);
    native_key(&window, Key::RightArrow.into());
    native_key(&window, Key::DownArrow.into());
    native_key(&window, Key::Return.into());
    assert_eq!(
        fixture.launches.borrow().as_slice(),
        &[inventory_key(1023), inventory_key(1023)],
        "the incomplete final row neither wraps nor invents an inaccessible next application"
    );
    native_key(&window, Key::Tab.into());
    native_key(&window, Key::Space.into());
    native_key(&window, Key::Return.into());
    assert_eq!(
        fixture.favorites.borrow().as_slice(),
        &[(inventory_key(1023), false), (inventory_key(1023), false)],
        "the actual last favorite remains keyboard-accessible and requests removal only"
    );
    let settings = Rc::new(Cell::new(0));
    let requests = settings.clone();
    fixture.launcher.on_open_settings_requested(move || {
        requests.set(requests.get() + 1);
    });
    native_key(&window, Key::Tab.into());
    native_key(&window, Key::Return.into());
    assert_eq!(
        settings.get(),
        1,
        "native Tab after the real final favorite reaches recovery, not a fabricated next cell"
    );
    assert_eq!(fixture.launches.borrow().len(), 2);
    assert_eq!(fixture.favorites.borrow().len(), 2);
    for width in [128, 96, 560] {
        fixture.launcher.invoke_focus_search();
        fixture.launcher.invoke_reset_scroll();
        window.set_size(slint::PhysicalSize::new(width, 300));
        let _ = draw(&window, width, 300);
        let first = inventory_tile(&fixture.launcher, 0);
        let next_row = inventory_tile(&fixture.launcher, 7);
        assert!(first.size().width > 0.0 && first.size().height > 0.0);
        assert!((first.size().width - first.size().height).abs() < 0.01);
        assert!(
            (next_row.absolute_position().y
                - first.absolute_position().y
                - first.size().height
                - 8.0)
                .abs()
                < 0.01,
            "narrow and restored layouts retain uniform positive native row pitch"
        );
        fixture.launcher.invoke_ensure_visible(1023);
        let _ = draw(&window, width, 300);
        assert!(inventory_tile(&fixture.launcher, 1023).size().height > 0.0);
    }
}

#[test]
fn launcher_keyboard_selection_routes_native_input_and_scrolls_nearest() {
    use std::cell::RefCell;

    use crate::launcher::{LauncherSelection, Navigation};

    // Mock only the authoritative controller boundary; input, focus, layout
    // and pixels still come from the generated native Launcher.
    fn update_selection(
        launcher: &Launcher,
        state: &RefCell<LauncherSelection>,
        keys: &[String],
        update: impl FnOnce(&mut LauncherSelection, &[String]) -> Option<usize>,
    ) {
        let (key, index) = {
            let mut state = state.borrow_mut();
            let index = update(&mut state, keys);
            (state.key().unwrap_or_default().to_owned(), index)
        };
        launcher.set_selected_key(key.into());
        if let Some(index) = index {
            launcher.invoke_ensure_visible(index as i32);
        }
    }

    let window = software_window();
    let launcher = Launcher::new().unwrap();
    set_launcher_tiles(
        &launcher,
        (0..23)
            .map(|i| app(&format!("app-{i}"), &format!("App {i}")))
            .collect(),
    );
    let keys = Rc::new(RefCell::new(
        (0..23).map(|i| format!("app-{i}")).collect::<Vec<_>>(),
    ));
    let selection = Rc::new(RefCell::new(LauncherSelection::default()));
    let weak = launcher.as_weak();
    let state = selection.clone();
    let inventory_keys = keys.clone();
    launcher.on_search_changed(move || {
        let launcher = weak.upgrade().unwrap();
        let active = !launcher.get_search().is_empty();
        update_selection(
            &launcher,
            &state,
            &inventory_keys.borrow(),
            |state, keys| state.search_changed(keys, active),
        );
    });
    let directions = Rc::new(RefCell::new(Vec::new()));
    let log = directions.clone();
    let weak = launcher.as_weak();
    let state = selection.clone();
    let inventory_keys = keys.clone();
    launcher.on_navigate_requested(move |direction| {
        log.borrow_mut().push(direction);
        let launcher = weak.upgrade().unwrap();
        let direction = match direction {
            LauncherNavigation::Up => Navigation::Up,
            LauncherNavigation::Down => Navigation::Down,
            LauncherNavigation::Left => Navigation::Left,
            LauncherNavigation::Right => Navigation::Right,
        };
        update_selection(
            &launcher,
            &state,
            &inventory_keys.borrow(),
            |state, keys| state.navigate(keys, direction, launcher.get_grid_columns() as usize),
        );
    });
    let weak = launcher.as_weak();
    let state = selection.clone();
    let inventory_keys = keys.clone();
    launcher.on_select_requested(move |key| {
        let launcher = weak.upgrade().unwrap();
        update_selection(
            &launcher,
            &state,
            &inventory_keys.borrow(),
            |state, keys| state.select(keys, &key),
        );
    });
    let activations = Rc::new(RefCell::new(Vec::new()));
    let log = activations.clone();
    let weak = launcher.as_weak();
    launcher.on_activate_selected_requested(move || {
        log.borrow_mut()
            .push(weak.upgrade().unwrap().get_selected_key());
    });
    let launches = Rc::new(RefCell::new(Vec::new()));
    let log = launches.clone();
    launcher.on_launch_requested(move |key| log.borrow_mut().push(key));
    launcher.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    window.set_size(slint::PhysicalSize::new(560, 300));
    launcher.invoke_focus_search();
    let baseline = draw(&window, 560, 300);
    let key = |text: slint::SharedString| {
        window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    };
    let tile = |i| {
        ElementHandle::find_by_accessible_label(&launcher, &format!("Launch App {i}"))
            .next()
            .unwrap_or_else(|| {
                panic!("visible launcher tile {i} must expose its exact launch label")
            })
    };
    // Inspect handles only while their native row remains materialized.
    let first_tile = tile(0);
    let first_position = first_tile.absolute_position();
    let first_size = first_tile.size();
    let ring_index = (first_position.y + first_size.height / 2.0) as usize * 560
        + (first_position.x - 2.5) as usize;
    key(Key::Return.into());
    assert!(
        activations.borrow().is_empty(),
        "empty query has no first-item fallback"
    );
    key("q".into());
    assert_eq!(launcher.get_search(), "q");
    assert_eq!(launcher.get_selected_key(), "app-0");
    let selected = draw(&window, 560, 300);
    assert_ne!(
        selected[ring_index], baseline[ring_index],
        "search selection paints a real external outline"
    );
    assert_eq!(
        first_tile.absolute_position(),
        first_position,
        "selection does not move a visible row"
    );
    key(Key::LeftArrow.into());
    key("a".into());
    key(Key::RightArrow.into());
    assert_eq!(
        launcher.get_search(),
        "aq",
        "native caret editing remains intact"
    );
    assert!(
        directions.borrow().is_empty(),
        "search Left/Right are not grid navigation"
    );
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    window
        .window()
        .dispatch_event(WindowEvent::KeyPressRepeated {
            text: Key::Return.into(),
        });
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    assert_eq!(
        activations.borrow().as_slice(),
        &[slint::SharedString::from("app-0")],
        "held search Enter activates once"
    );

    key(Key::DownArrow.into());
    assert_eq!(launcher.get_selected_key(), "app-7");
    key("b".into());
    assert_eq!(
        launcher.get_search(),
        "aqb",
        "search arrows keep native input focus"
    );
    assert_eq!(launcher.get_selected_key(), "app-0");
    key(Key::DownArrow.into());
    key(Key::UpArrow.into());
    assert_eq!(launcher.get_selected_key(), "app-0");
    key(Key::DownArrow.into());
    assert_eq!(launcher.get_selected_key(), "app-7");
    key(Key::Tab.into()); // All Apps is a real header navigation control.
    key(Key::Tab.into()); // Actual focus on App 0.
    assert_eq!(
        launcher.get_selected_key(),
        "app-0",
        "actual Tab focus reconciles selection"
    );
    key(Key::RightArrow.into());
    assert_eq!(launcher.get_selected_key(), "app-1");
    // No rendering/event-loop pump between the arrow and Return: routing
    // must already target the newly selected, genuinely focused tile.
    key(Key::Return.into());
    assert_eq!(
        launches.borrow().as_slice(),
        &[slint::SharedString::from("app-1")],
        "grid Arrow then Return launches B, never the formerly focused A"
    );
    let moved = draw(&window, 560, 300);
    assert_eq!(
        moved[ring_index], baseline[ring_index],
        "grid navigation removes the former tile's real-focus outline"
    );
    key(Key::Tab.into()); // App 1's favorite has its own focus scope.
    key(Key::Tab.into()); // Actual focus on App 2.
    assert_eq!(launcher.get_selected_key(), "app-2");
    let reconciled = draw(&window, 560, 300);
    assert_eq!(
        reconciled[ring_index], baseline[ring_index],
        "moving Tab focus removes the stale first-tile outline"
    );
    key(Key::LeftArrow.into());
    assert_eq!(launcher.get_selected_key(), "app-1");
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Space.into(),
    });
    key(Key::RightArrow.into());
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Space.into(),
    });
    assert_eq!(
        launches.borrow().len(),
        1,
        "moving real tile focus cancels the armed Space gesture"
    );
    key(Key::LeftArrow.into());
    key(Key::DownArrow.into());
    key(Key::DownArrow.into());
    key(Key::DownArrow.into());
    assert_eq!(launcher.get_selected_key(), "app-22");
    let last_row = draw(&window, 560, 300);
    let scrolled = tile(22).absolute_position();
    assert!(scrolled.y > first_position.y);
    let footer = ElementHandle::find_by_accessible_label(&launcher, "Open settings and recovery")
        .next()
        .unwrap();
    assert!(
        scrolled.y + tile(22).size().height + 4.0 < footer.absolute_position().y,
        "last selected row and outline stay above the footer"
    );
    let bottom_ring = (scrolled.y + tile(22).size().height + 2.5) as usize * 560
        + (scrolled.x + tile(22).size().width / 2.0) as usize;
    let accent = launcher
        .global::<crate::generated::SeelenPalette>()
        .get_accent()
        .color()
        .to_argb_u8();
    assert_eq!(
        last_row[bottom_ring],
        Rgb8Pixel {
            r: accent.red,
            g: accent.green,
            b: accent.blue
        },
        "the scrolled last-row outline actually renders without clipping"
    );
    assert!(
        ElementHandle::find_by_accessible_label(&launcher, "Launch App 0")
            .next()
            .is_none(),
        "nearest selection scrolls the first row out of the actual viewport"
    );
    key(Key::RightArrow.into());
    key(Key::DownArrow.into());
    assert_eq!(
        launcher.get_selected_key(),
        "app-22",
        "incomplete last row does not wrap"
    );
    launcher.invoke_ensure_visible(22);
    launcher.invoke_ensure_visible(-1);
    launcher.invoke_ensure_visible(23);
    assert_eq!(
        tile(22).absolute_position(),
        scrolled,
        "visible/invalid targets do not move the viewport"
    );
    key(Key::UpArrow.into());
    key(Key::UpArrow.into());
    assert_eq!(launcher.get_selected_key(), "app-8");
    let _ = draw(&window, 560, 300);
    assert!(
        (tile(8).absolute_position().y - first_position.y).abs() < 0.01,
        "nearest upward scrolling aligns the outline gutter, not the tile bottom"
    );

    launcher.invoke_focus_search();
    key(Key::DownArrow.into());
    key("c".into());
    assert_eq!(
        launcher.get_search(),
        "aqbc",
        "focus-search resets grid-navigation intent before native editing"
    );
    let before = activations.borrow().len();
    for (stale, refreshing) in [(true, false), (false, true)] {
        launcher.set_stale(stale);
        launcher.set_refreshing(refreshing);
        key(Key::Return.into());
    }
    launcher.set_stale(false);
    launcher.set_refreshing(false);
    keys.borrow_mut().clear();
    set_launcher_tiles(&launcher, Vec::new());
    launcher.set_search("none".into());
    // Process the real pending query callback at the next input boundary;
    // neither a getter nor a forced draw should stand in for native input.
    key(Key::Return.into());
    assert_eq!(launcher.get_selected_key(), "");
    assert_eq!(
        activations.borrow().len(),
        before,
        "blocked or empty results never request activation"
    );
    assert_eq!(
        launches.borrow().as_slice(),
        &[slint::SharedString::from("app-1")],
        "only explicit Return launches; selection navigation never does"
    );
    let _ = draw(&window, 560, 300);
    assert!(
        !window.draw_if_needed(|renderer| {
            let mut pixels = vec![Rgb8Pixel::default(); 560 * 300];
            renderer.render(&mut pixels, 560);
        }),
        "settled keyboard selection does not continuously redraw"
    );
}

#[test]
fn launcher_grid_preserves_layout_and_renders_edge_tile_focus_outside_tiles() {
    let window = software_window();
    let launcher = Launcher::new().unwrap();
    let mut bitmap = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(32, 32);
    bitmap.make_mut_slice().fill(slint::Rgba8Pixel {
        r: 255,
        g: 0,
        b: 255,
        a: 255,
    });
    let icon = slint::Image::from_rgba8(bitmap);
    set_launcher_tiles(
        &launcher,
        (0..7)
            .map(|i| LaunchTile {
                icon: icon.clone(),
                ..app(&format!("app-{i}"), &format!("App {i}"))
            })
            .collect(),
    );
    let launches = Rc::new(Cell::new(0));
    let count = launches.clone();
    launcher.on_launch_requested(move |_| count.set(count.get() + 1));
    launcher.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    let custom_accent = slint::Color::from_rgb_u8(42, 160, 95);
    for (theme, accent_override) in [
        (slint::language::ColorScheme::Light, None),
        (slint::language::ColorScheme::Dark, None),
        (slint::language::ColorScheme::Light, Some(custom_accent)),
        (slint::language::ColorScheme::Dark, Some(custom_accent)),
    ] {
        launcher.apply_presentation_theme(PresentationTheme::uniform(theme));
        if let Some(accent) = accent_override {
            launcher
                .global::<crate::generated::SeelenPalette>()
                .set_accent(accent.into());
        }
        for scale in [1.0_f32, 2.0] {
            window
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
            let width = (560.0 * scale) as u32;
            let height = (420.0 * scale) as u32;
            window.set_size(slint::PhysicalSize::new(width, height));
            let baseline = draw(&window, width, height);
            assert!(
                baseline
                    .iter()
                    .any(|pixel| pixel.r == 255 && pixel.g == 0 && pixel.b == 255),
                "the nonempty icon must actually render before checking its clipping",
            );
            for i in [0, 6] {
                let tile =
                    ElementHandle::find_by_accessible_label(&launcher, &format!("Launch App {i}"))
                        .next()
                        .unwrap();
                let position = tile.absolute_position();
                let size = tile.size();
                assert!((size.width - 460.0 / 7.0).abs() < 0.01);
                assert!((size.width - size.height).abs() < 0.01);
                let edge = if i == 0 {
                    position.x
                } else {
                    position.x + size.width
                };
                assert!((edge - if i == 0 { 26.0 } else { 534.0 }).abs() < 0.01);
                let outside = edge + if i == 0 { -2.5 } else { 2.5 };
                let y = position.y + size.height / 2.0;
                let index = (y * scale) as usize * width as usize + (outside * scale) as usize;
                let body_color = baseline
                    [(y * scale) as usize * width as usize + ((position.x - 5.5) * scale) as usize];
                let left = (position.x * scale).ceil() as usize;
                let right = ((position.x + size.width) * scale).floor() as usize;
                let bottom = position.y + size.height;
                for row in ((bottom + 1.0) * scale).ceil() as usize
                    ..((bottom + 7.0) * scale).floor() as usize
                {
                    assert!(
                        baseline[row * width as usize + left..row * width as usize + right]
                            .iter()
                            .all(|pixel| *pixel == body_color),
                        "real icon/name content stays inside its square, not the outline gutter",
                    );
                }
                let center = slint::LogicalPosition::new(position.x + size.width / 2.0, y);
                let fill_index =
                    (y * scale) as usize * width as usize + ((position.x + 4.0) * scale) as usize;
                let accent = launcher
                    .global::<crate::generated::SeelenPalette>()
                    .get_accent()
                    .color()
                    .to_argb_u8();
                let assert_tint = |pixels: &[Rgb8Pixel], alpha: f32| {
                    let pixel = pixels[fill_index];
                    assert_rgb_overlay(
                        [pixel.r, pixel.g, pixel.b],
                        [accent.red, accent.green, accent.blue],
                        [body_color.r, body_color.g, body_color.b],
                        alpha,
                    );
                };
                window
                    .window()
                    .dispatch_event(WindowEvent::PointerMoved { position: center });
                let hovered = draw(&window, width, height);
                assert_tint(&hovered, 0.2);
                let before = launches.get();
                window.window().dispatch_event(WindowEvent::PointerPressed {
                    position: center,
                    button: PointerEventButton::Left,
                });
                let mut pressed = hovered;
                window.draw_if_needed(|renderer| {
                    renderer.render(&mut pressed, width as usize);
                });
                assert_tint(&pressed, 0.2);
                window
                    .window()
                    .dispatch_event(WindowEvent::PointerReleased {
                        position: center,
                        button: PointerEventButton::Left,
                    });
                window.window().dispatch_event(WindowEvent::KeyPressed {
                    text: Key::Space.into(),
                });
                window.window().dispatch_event(WindowEvent::KeyReleased {
                    text: Key::Space.into(),
                });
                assert_eq!(launches.get(), before + 2);
                let focused = draw(&window, width, height);
                assert_tint(&focused, 0.1);
                assert_ne!(
                    focused[index], baseline[index],
                    "the {i} edge tile's external keyboard outline must not be clipped ({theme:?}, {scale}x)",
                );
                assert_eq!(tile.absolute_position(), position);
                assert_eq!(tile.size(), size, "focus does not resize or move the grid");
                launcher.set_stale(true);
                let disabled = draw(&window, width, height);
                assert_eq!(
                    disabled[fill_index], body_color,
                    "disabled tiles have no state overlay"
                );
                launcher.set_stale(false);
            }
            launcher.invoke_focus_search();
            window.window().dispatch_event(WindowEvent::PointerExited);
        }
    }
}

#[test]
fn launcher_opaque_frame_is_transparent_outside_and_bounds_content_at_both_scales() {
    use slint::platform::software_renderer::PremultipliedRgbaColor;

    let window = software_window();
    let launcher = Launcher::new().unwrap();
    set_launcher_tiles(&launcher, vec![app("editor", "Rust Editor")]);
    launcher.show().unwrap();
    for (scheme, scale, background) in [
        (slint::language::ColorScheme::Dark, 1.0, 24),
        (slint::language::ColorScheme::Dark, 2.0, 24),
        (slint::language::ColorScheme::Light, 1.0, 242),
        (slint::language::ColorScheme::Light, 2.0, 242),
    ] {
        launcher.apply_presentation_theme(PresentationTheme::uniform(scheme));
        window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        let width = (560.0 * scale) as usize;
        let height = (420.0 * scale) as usize;
        window.set_size(slint::PhysicalSize::new(width as u32, height as u32));
        let mut pixels = vec![PremultipliedRgbaColor::default(); width * height];
        assert!(window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, width);
        }));
        let sample = |x: f32, y: f32| pixels[(y * scale) as usize * width + (x * scale) as usize];
        let surface = sample(280.0, 210.0);
        assert_eq!(
            (surface.red, surface.green, surface.blue, surface.alpha),
            (background, background, background, 255)
        );
        assert_eq!(
            sample(0.0, 0.0).alpha,
            0,
            "The outer window remains transparent"
        );
        for label in [
            "Launch Rust Editor",
            "Open settings and recovery",
            "Refresh the desktop",
            "Exit Tessera",
        ] {
            let element = ElementHandle::find_by_accessible_label(&launcher, label)
                .next()
                .unwrap();
            let position = element.absolute_position();
            let size = element.size();
            assert!(
                position.x >= 10.0
                    && position.y >= 10.0
                    && position.x + size.width <= 550.0
                    && position.y + size.height <= 410.0,
                "{label} must stay inside the opaque content bounds: {position:?}"
            );
        }
        assert!(
            !window.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, width);
            }),
            "The settled launcher must not continuously redraw"
        );
    }
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
fn launcher_favorite_toggle_routes_exact_desired_state_without_launching() {
    let window = software_window();
    let launcher = Launcher::new().unwrap();
    set_launcher_tiles(&launcher, vec![app("app-editor", "Rust Editor")]);
    let favorites = Rc::new(Cell::new(0));
    let counter = favorites.clone();
    let requests = Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = requests.clone();
    launcher.on_favorite_toggle_requested(move |key, desired| {
        counter.set(counter.get() + 1);
        log.borrow_mut().push((key, desired));
    });
    let launches = Rc::new(Cell::new(0));
    let counter = launches.clone();
    launcher.on_launch_requested(move |_| counter.set(counter.get() + 1));
    let selected_activations = Rc::new(Cell::new(0));
    let count = selected_activations.clone();
    launcher.on_activate_selected_requested(move || count.set(count.get() + 1));
    launcher.set_selected_key("app-editor".into());
    launcher.show().unwrap();
    window.set_size(slint::PhysicalSize::new(560, 420));
    let _ = draw(&window, 560, 420);

    let favorite =
        ElementHandle::find_by_accessible_label(&launcher, "Add to favorites: Rust Editor")
            .next()
            .unwrap();
    assert_eq!(favorite.accessible_role(), Some(AccessibleRole::Checkbox));
    assert_eq!(favorite.accessible_checked(), Some(false));
    favorite.invoke_accessible_default_action();
    assert_eq!(
        favorites.get(),
        1,
        "accessible favorite action requests addition"
    );
    assert_eq!(
        favorite.accessible_checked(),
        Some(false),
        "no optimistic membership"
    );
    let position = favorite.absolute_position();
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
    assert_eq!(
        favorites.get(),
        2,
        "mouse favorite action is not a tile launch"
    );
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Space.into(),
    });
    window
        .window()
        .dispatch_event(WindowEvent::KeyPressRepeated {
            text: Key::Space.into(),
        });
    assert_eq!(
        favorites.get(),
        2,
        "Space arms without activating or repeating"
    );
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Space.into(),
    });
    assert_eq!(
        favorites.get(),
        3,
        "focused favorite Space activates once on release"
    );
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    assert_eq!(
        favorites.get(),
        4,
        "focused favorite Return remains favorite-only"
    );
    assert_eq!(
        launches.get(),
        0,
        "favorite input never bubbles into launch"
    );
    assert_eq!(
        requests.borrow().as_slice(),
        &vec![(slint::SharedString::from("app-editor"), true); 4],
        "callbacks request absolute desired membership for the exact key",
    );
    let mut saved = app("app-editor", "Rust Editor");
    saved.favorite = true;
    set_launcher_tiles(&launcher, vec![saved]);
    let _ = draw(&window, 560, 420);
    assert_eq!(favorites.get(), 4, "programmatic projection is silent");
    let favorite =
        ElementHandle::find_by_accessible_label(&launcher, "Remove from favorites: Rust Editor")
            .next()
            .unwrap();
    assert_eq!(favorite.accessible_checked(), Some(true));
    favorite.invoke_accessible_default_action();
    assert_eq!(favorites.get(), 5);
    assert_eq!(
        requests.borrow().last(),
        Some(&(slint::SharedString::from("app-editor"), false))
    );
    launcher.set_stale(true);
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    favorite.invoke_accessible_default_action();
    assert_eq!(
        favorites.get(),
        5,
        "stale favorite controls reject all actions"
    );

    let actions = Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = actions.clone();
    launcher.on_open_settings_requested(move || log.borrow_mut().push("settings"));
    let log = actions.clone();
    launcher.on_refresh_requested(move || log.borrow_mut().push("refresh"));
    let log = actions.clone();
    launcher.on_exit_requested(move || log.borrow_mut().push("exit"));
    let hides = Rc::new(Cell::new(0));
    let count = hides.clone();
    launcher.on_hide_requested(move || count.set(count.get() + 1));
    let key = |text: slint::SharedString| {
        window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    };
    window
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    for (stale, refreshing, expected) in [
        (true, false, vec!["settings", "refresh", "exit"]),
        (false, true, vec!["settings", "exit"]),
    ] {
        launcher.set_stale(stale);
        launcher.set_refreshing(refreshing);
        launcher.set_search("".into());
        actions.borrow_mut().clear();
        launcher.invoke_focus_search();
        for index in 0..expected.len() {
            key(Key::Tab.into());
            key(Key::Return.into());
            assert_eq!(
                actions.borrow().as_slice(),
                &expected[..=index],
                "Tab must skip blocked header, application and favorite scopes",
            );
        }
        key(Key::Tab.into());
        key("z".into());
        assert_eq!(
            launcher.get_search(),
            "z",
            "Tab must wrap straight to search, not the Escape wrapper",
        );
        key(Key::Escape.into());
    }
    assert_eq!(hides.get(), 2, "Escape still bubbles from the search field");
    assert_eq!(
        favorites.get(),
        5,
        "blocked traversal never changes a favorite"
    );
    assert_eq!(launches.get(), 0, "blocked traversal never launches an app");
    assert_eq!(
        selected_activations.get(),
        0,
        "favorite and footer Return never request selected-app activation"
    );
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
    let menu = ContextMenuSurface::new_with_metrics().unwrap();
    menu.set_kind(DockMenuKind::Bar);
    let selected = Rc::new(Cell::new(None));
    let recorded = Rc::clone(&selected);
    menu.on_action_requested(move |action| recorded.set(Some(action)));
    menu.show().unwrap();
    menu.invoke_focus_menu();
    window
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    let mut icon = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(32, 32);
    icon.make_mut_slice().fill(slint::Rgba8Pixel {
        r: 255,
        g: 0,
        b: 255,
        a: 255,
    });
    let application_icon = slint::Image::from_rgba8(icon);

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
        for (kind, labels) in [
            (
                DockMenuKind::Bar,
                [
                    "Settings",
                    "File Explorer",
                    "Task Manager",
                    "Restore Explorer",
                    "Exit Tessera",
                ]
                .as_slice(),
            ),
            (DockMenuKind::Pinned, ["Open", "Unpin"].as_slice()),
            (
                DockMenuKind::Window,
                ["Switch to window", "Minimize", "Close"].as_slice(),
            ),
        ] {
            menu.set_kind(kind);
            menu.set_target_icon(slint::Image::default());
            menu.set_selected_index(0);
            menu.invoke_focus_menu();
            let tokens = menu.global::<crate::generated::PopoverTokens>();
            let margin = tokens.get_shadow_margin();
            let row_height = tokens.get_font_size() * tokens.get_line_height() + 16.0;
            let expected_height = 2.0 * margin
                + 16.0
                + labels.len() as f32 * row_height
                + labels.len().saturating_sub(1) as f32 * 8.0;
            assert!(
                menu.get_menu_width() >= 200.0 + 2.0 * margin,
                "all reference menus reserve a minimum 200px body",
            );
            assert!(
                (menu.get_menu_height() - expected_height).abs() < 0.001,
                "menu rows derive from the shared font line height, 8px padding and gaps",
            );
            // Match production placement's ceil before rendering fractional
            // font-derived dimensions; truncation invents a clipped viewport.
            let width = (menu.get_menu_width() * scale).ceil() as u32;
            let height = (menu.get_menu_height() * scale).ceil() as u32;
            assert!(
                height > (72.0 * scale) as u32,
                "menu is not clipped to the dock"
            );
            window.set_size(slint::PhysicalSize::new(width, height));
            let pixels = draw(&window, width, height);
            assert!(pixels.iter().any(|pixel| *pixel != pixels[0]));
            let inset = ((margin + 2.0) * scale) as usize;
            let pixel = pixels[(height as usize / 2) * width as usize + inset];
            assert_eq!(
                [pixel.r, pixel.g, pixel.b],
                background,
                "the shared menu body is opaque and theme-adaptive"
            );
            for (index, label) in labels.iter().enumerate() {
                let button = ElementHandle::find_by_accessible_label(&menu, label)
                    .next()
                    .unwrap();
                assert_eq!(button.accessible_role(), Some(AccessibleRole::Button));
                let position = button.absolute_position();
                let size = button.size();
                assert!((position.x - margin - 8.0).abs() < 0.001);
                assert!(
                    (position.y - margin - 8.0 - index as f32 * (row_height + 8.0)).abs() < 0.001,
                    "real rows preserve the reference 8px gap",
                );
                assert!((size.height - row_height).abs() < 0.001);
                assert!(position.y + size.height <= menu.get_menu_height() - margin - 8.0 + 0.001);
                if matches!(*label, "Settings" | "Open" | "Switch to window" | "Close") {
                    let left = ((position.x + 8.0) * scale).ceil() as usize;
                    let top = ((position.y + (row_height - 16.0) / 2.0) * scale).ceil() as usize;
                    let right = ((position.x + 24.0) * scale).floor() as usize;
                    let bottom =
                        ((position.y + (row_height + 16.0) / 2.0) * scale).floor() as usize;
                    let icon_pixels = || {
                        (top..bottom).flat_map(|row| {
                            pixels[row * width as usize + left..row * width as usize + right].iter()
                        })
                    };
                    assert!(
                        icon_pixels().any(|pixel| [pixel.r, pixel.g, pixel.b] != background),
                        "{label} renders genuine named artwork in its 16px leading slot",
                    );
                    if matches!(*label, "Settings" | "Close") {
                        let palette = menu.global::<crate::generated::SeelenPalette>();
                        let color = if *label == "Close" {
                            palette.get_danger()
                        } else {
                            palette.get_muted()
                        };
                        let color = color.to_argb_u8();
                        assert!(
                            icon_pixels().any(|pixel| pixel.r.abs_diff(color.red) <= 2
                                && pixel.g.abs_diff(color.green) <= 2
                                && pixel.b.abs_diff(color.blue) <= 2),
                            "{label} uses the source-proven muted/danger SVG tint",
                        );
                    }
                }
            }
            export_screenshot(
                &format!("{name}-{kind:?}"),
                &pixels,
                width as usize,
                height as usize,
            );
            assert!(
                !window.draw_if_needed(|renderer| {
                    let mut unchanged = pixels.clone();
                    renderer.render(&mut unchanged, width as usize);
                }),
                "the open menu does not continuously render unchanged pixels"
            );
            let first = ElementHandle::find_by_accessible_label(&menu, labels[0])
                .next()
                .unwrap();
            let origin = first.absolute_position();
            let center = slint::LogicalPosition::new(
                origin.x + first.size().width / 2.0,
                origin.y + row_height / 2.0,
            );
            let accent = menu
                .global::<crate::generated::SeelenPalette>()
                .get_accent()
                .color()
                .to_argb_u8();
            let accent = [accent.red, accent.green, accent.blue];
            let sample = |frame: &[Rgb8Pixel], x: f32, y: f32| {
                let pixel = frame[(y * scale) as usize * width as usize + (x * scale) as usize];
                [pixel.r, pixel.g, pixel.b]
            };
            let overlay = |frame: &[Rgb8Pixel], alpha: f32| {
                let actual = sample(frame, origin.x + 4.0, center.y);
                assert_rgb_overlay(actual, accent, background, alpha);
            };
            window
                .window()
                .dispatch_event(WindowEvent::PointerMoved { position: center });
            let hovered = draw(&window, width, height);
            overlay(&hovered, 0.2);
            assert_eq!(
                sample(&hovered, origin.x - 3.0, center.y),
                background,
                "pointer hover has no keyboard outline",
            );
            window.window().dispatch_event(WindowEvent::PointerPressed {
                position: center,
                button: PointerEventButton::Left,
            });
            let pressed = draw(&window, width, height);
            overlay(&pressed, 0.3);
            window
                .window()
                .dispatch_event(WindowEvent::PointerReleased {
                    position: center,
                    button: PointerEventButton::Left,
                });
            let restored = draw(&window, width, height);
            overlay(&restored, 0.2);
            window.window().dispatch_event(WindowEvent::PointerExited);
            // Home covers mouse -> keyboard on the already-focused first
            // item; End covers a real focus move to the opposite edge row.
            for (key, index) in [(Key::Home, 0), (Key::End, labels.len() - 1)] {
                window
                    .window()
                    .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
                let focused = draw(&window, width, height);
                let item = ElementHandle::find_by_accessible_label(&menu, labels[index])
                    .next()
                    .unwrap();
                let point = item.absolute_position();
                let middle_y = point.y + row_height / 2.0;
                assert_eq!(
                    sample(&focused, point.x - 3.0, middle_y),
                    accent,
                    "keyboard selection must focus the real row and expose its external outline",
                );
                assert_eq!(
                    sample(&focused, point.x - 1.0, middle_y),
                    background,
                    "the keyboard outline preserves its 2px gap",
                );
                let middle_x = point.x + item.size().width / 2.0;
                for y in [point.y - 3.0, point.y + row_height + 3.0] {
                    assert_eq!(
                        sample(&focused, middle_x, y),
                        accent,
                        "first/last row outlines remain inside the vertical viewport gutter",
                    );
                }
                assert!(
                    !window.draw_if_needed(|renderer| {
                        let mut unchanged = focused.clone();
                        renderer.render(&mut unchanged, width as usize);
                    }),
                    "a settled keyboard outline adds no idle redraw",
                );
            }
            if kind != DockMenuKind::Bar {
                menu.set_target_icon(application_icon.clone());
                menu.invoke_focus_menu();
                let application = draw(&window, width, height);
                let first = ElementHandle::find_by_accessible_label(&menu, labels[0])
                    .next()
                    .unwrap();
                let point = first.absolute_position();
                let x = point.x + 8.0;
                let y = point.y + (row_height - 16.0) / 2.0;
                assert_eq!(
                    sample(&application, x + 8.0, y + 8.0),
                    [255, 0, 255],
                    "genuine application image pixels must not inherit named-SVG tint",
                );
                // The pinned software renderer ignores clip radii; the genuine
                // GL scenario separately proves the application's rounded corner.
                assert_eq!(
                    sample(&application, x + 20.0, y + 8.0),
                    background,
                    "the 8px icon/name gap stays empty",
                );
                menu.set_target_icon(slint::Image::default());
                let fallback = draw(&window, width, height);
                assert_ne!(
                    sample(&fallback, x + 8.0, y + 8.0),
                    [255, 0, 255],
                    "missing or stale application images return to real named artwork",
                );
            }
        }
    }
    menu.set_kind(DockMenuKind::Bar);
    menu.invoke_focus_menu();
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
fn context_menu_fits_native_label_metrics_and_returns_to_minimum_width() {
    let window = software_window();
    let menu = ContextMenuSurface::new_with_metrics().unwrap();
    for scheme in [
        slint::language::ColorScheme::Light,
        slint::language::ColorScheme::Dark,
    ] {
        menu.apply_presentation_theme(PresentationTheme::uniform(scheme));
        for scale in [1.0, 2.0] {
            window
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
            for (kind, label) in [
                (DockMenuKind::Bar, "Restore Explorer"),
                (DockMenuKind::Pinned, "Unpin"),
                (DockMenuKind::Window, "Switch to window"),
            ] {
                menu.set_kind(kind);
                let tokens = menu.global::<crate::generated::PopoverTokens>();
                let minimum = 200.0 + 2.0 * tokens.get_shadow_margin();
                tokens.set_font_size(12.8);
                assert!((menu.get_menu_width() - minimum).abs() < 0.001);
                // A real large theme font forces long labels beyond the
                // minimum; no guessed character-width/font-size estimator.
                tokens.set_font_size(48.0);
                let preferred = menu.get_menu_width();
                assert!(preferred >= minimum);
                if kind != DockMenuKind::Pinned {
                    assert!(
                        preferred > minimum,
                        "{kind:?} {scheme:?} {scale}x: fit-content must grow, preferred {preferred}, minimum {minimum}",
                    );
                }
                let width = (preferred * scale).ceil() as u32;
                let height = (menu.get_menu_height() * scale).ceil() as u32;
                window.set_size(slint::PhysicalSize::new(width, height));
                menu.show().unwrap();
                let pixels = draw(&window, width, height);
                let item = ElementHandle::find_by_accessible_label(&menu, label)
                    .next()
                    .unwrap();
                assert_eq!(item.accessible_role(), Some(AccessibleRole::Button));
                assert!(
                    item.size().width >= 184.0,
                    "{kind:?} {scheme:?} {scale}x: preferred {preferred}, physical {width}x{height}, item {:?}, logical height {}",
                    item.size(),
                    menu.get_menu_height(),
                );
                let point = item.absolute_position();
                assert!(
                    point.x + item.size().width <= preferred - tokens.get_shadow_margin() - 8.0
                );
                assert!(
                    !window.draw_if_needed(|renderer| {
                        let mut unchanged = pixels.clone();
                        renderer.render(&mut unchanged, width as usize);
                    }),
                    "native width measurement must not add an idle rendering loop",
                );
                tokens.set_font_size(12.8);
                assert!(
                    (menu.get_menu_width() - minimum).abs() < 0.001,
                    "preferred width must shrink back to the reference minimum",
                );
                menu.hide().unwrap();
            }
        }
    }
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
    tooltip.disable_motion();
    assert_eq!(
        pixel(&draw(&window, width, height)),
        [24, 24, 24],
        "live reduced-motion opt-out must settle the shown content, not keep fading or hide it"
    );
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
