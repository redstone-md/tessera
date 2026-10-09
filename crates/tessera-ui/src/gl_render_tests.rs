// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Production-renderer pixels on an owned virtual display, in an isolated test process.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::ComponentHandle;
use slint::language::ColorScheme;
use slint::winit_030::{SlintEvent, WinitWindowAccessor, winit};
use winit::platform::x11::EventLoopBuilderExtX11;

use crate::generated::{
    BluetoothDeviceRow, BluetoothMenu, BluetoothRadioRow, BluetoothRadioStatus,
    BluetoothTransportKind, CalendarDayCell, CalendarMenu, CalendarWeekRow, ContextMenuSurface,
    Dock, DockApp, DockMenuAction, DockMenuKind, InputLanguageMenu, InputProfileRow, LaunchRow,
    LaunchTile, Launcher, LauncherDisplayMode, LauncherDragVisual, NetworkMenu, NetworkRow,
    QuickSettings, TileBounds, TooltipSurface, UserFolderKind, UserFolderRow, UserMenu,
};
use crate::generated::{FocusTokens, PowerMenuAction, PowerMenuSurface};
use crate::theme::{PresentationTheme, ThemedComponent};

#[test]
#[ignore = "Requires an owned X11 display and isolated --exact test process"]
fn native_gl_frames_render_reference_shadow_alpha() {
    let mut builder = winit::event_loop::EventLoop::<SlintEvent>::with_user_event();
    builder.with_x11().with_any_thread(true);
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("femtovg".into())
        .require_opengl()
        .with_winit_event_loop_builder(builder)
        .with_winit_window_attributes_hook(|attributes| attributes.with_active(false))
        .select()
        .unwrap();

    let launcher = Launcher::new().unwrap();
    launcher.set_application_count(1);
    launcher.set_rows(slint::ModelRc::new(slint::VecModel::from(vec![
        LaunchRow {
            tiles: slint::ModelRc::new(slint::VecModel::from(vec![LaunchTile {
                key: "native-app".into(),
                label: "Native app".into(),
                icon: slint::Image::default(),
                favorite: false,
            }])),
        },
    ])));
    launcher
        .window()
        .set_size(slint::LogicalSize::new(560.0, 420.0));
    launcher.show().unwrap();
    let tooltip = TooltipSurface::new().unwrap();
    tooltip.set_content("Native tooltip".into());
    tooltip.window().set_size(slint::LogicalSize::new(
        tooltip.get_tooltip_width(),
        tooltip.get_tooltip_height(),
    ));
    tooltip.show().unwrap();
    let menu = ContextMenuSurface::new_with_metrics().unwrap();
    menu.set_kind(DockMenuKind::Bar);
    menu.set_selected_index(-1);
    menu.window().set_size(slint::LogicalSize::new(
        menu.get_menu_width(),
        menu.get_menu_height(),
    ));
    menu.show().unwrap();
    let quick = QuickSettings::new().unwrap();
    quick.set_output_ready(true);
    quick.set_output_percent(37.0);
    quick.set_input_ready(true);
    quick.set_input_percent(62.0);
    quick.window().set_size(slint::LogicalSize::new(
        quick.get_preferred_popup_width(),
        quick.get_preferred_popup_height(),
    ));
    quick.show().unwrap();
    // Render the genuine popup component with a closed, paint-only fixture:
    // no OS folder resolution/opening is performed by this renderer test.
    let user = UserMenu::new().unwrap();
    user.set_user_name("Native user".into());
    user.set_profile_name("Native current profile".into());
    user.set_profile_key("gl-current-profile".into());
    user.set_profile_actions_ready(true);
    let photo =
        tessera_system::profile::ProfilePhoto::new(70, 70, [0, 150, 230, 255].repeat(70 * 70))
            .unwrap();
    user.set_profile_photo(crate::user_menu::prepare_profile_photo(&photo));
    user.set_has_photo(true);
    user.set_rows(slint::ModelRc::new(slint::VecModel::from(
        [
            UserFolderKind::Recent,
            UserFolderKind::Desktop,
            UserFolderKind::Downloads,
            UserFolderKind::Documents,
            UserFolderKind::Music,
            UserFolderKind::Pictures,
            UserFolderKind::Videos,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, kind)| UserFolderRow {
            kind,
            key: format!("gl-folder-{index}").into(),
            ready: true,
            status: Default::default(),
        })
        .collect::<Vec<_>>(),
    )));
    user.window().set_size(slint::LogicalSize::new(
        user.get_popup_content_width(),
        user.get_popup_content_height(),
    ));
    user.show().unwrap();
    // Closed selector presentation fixtures; no OS enumeration, activation,
    // WLAN scan/connect, Bluetooth discovery or mutation provider is bound.
    let keyboard = InputLanguageMenu::new().unwrap();
    keyboard.set_observed(true);
    keyboard.set_unknown_active(false);
    keyboard.set_settings_key("gl-keyboard-settings".into());
    keyboard.set_rows(slint::ModelRc::new(slint::VecModel::from(
        (0..3)
            .map(|index| InputProfileRow {
                key: format!("gl-input-profile-{index}").into(),
                language_name: "Fixture language".into(),
                layout_name: format!("Fixture layout {index}").into(),
                active: index == 0,
            })
            .collect::<Vec<_>>(),
    )));
    keyboard.window().set_size(slint::LogicalSize::new(
        keyboard.get_popup_content_width(),
        keyboard.get_popup_content_height(),
    ));
    keyboard.show().unwrap();

    let network = NetworkMenu::new().unwrap();
    network.set_radio_text("Fixture adapter: Wi-Fi On".into());
    network.set_summary("Controlled cached observations, not live Internet connectivity.".into());
    network.set_refresh_key("gl-network-refresh".into());
    network.set_settings_key("gl-network-settings".into());
    network.set_connected(slint::ModelRc::new(slint::VecModel::from(vec![
        NetworkRow {
            label: "Fixture cached network".into(),
            details: "Fixture adapter · 81% signal · Connection unknown".into(),
            description: "Controlled read-only cache entry".into(),
            bands: "5G".into(),
        },
    ])));
    network.window().set_size(slint::LogicalSize::new(
        network.get_popup_content_width(),
        network.get_popup_content_height(),
    ));
    network.show().unwrap();

    let bluetooth = BluetoothMenu::new().unwrap();
    bluetooth.set_has_snapshot(true);
    bluetooth.set_refresh_enabled(true);
    bluetooth.set_radios(slint::ModelRc::new(slint::VecModel::from(vec![
        BluetoothRadioRow {
            name: "Fixture Bluetooth radio".into(),
            state: BluetoothRadioStatus::On,
        },
    ])));
    bluetooth.set_connected(slint::ModelRc::new(slint::VecModel::from(vec![
        BluetoothDeviceRow {
            name: "Fixture paired device".into(),
            transport: BluetoothTransportKind::Classic,
        },
    ])));
    bluetooth.set_unknown(slint::ModelRc::new(slint::VecModel::from(vec![
        BluetoothDeviceRow {
            name: "Fixture uncertain device".into(),
            transport: BluetoothTransportKind::LowEnergy,
        },
    ])));
    bluetooth.window().set_size(slint::LogicalSize::new(
        bluetooth.get_popup_content_width(),
        bluetooth.get_popup_content_height(),
    ));
    bluetooth.show().unwrap();
    // Closed six-week paint fixture, not an OS date/locale fallback.
    let calendar = CalendarMenu::new().unwrap();
    calendar.set_title_text("Native calendar".into());
    calendar.set_action_key("gl-calendar-actions".into());
    calendar.set_can_previous(true);
    calendar.set_can_next(true);
    calendar.set_weekdays(slint::ModelRc::new(slint::VecModel::from(
        ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
            .map(slint::SharedString::from)
            .to_vec(),
    )));
    calendar.set_weeks(slint::ModelRc::new(slint::VecModel::from(
        (0..6)
            .map(|week| CalendarWeekRow {
                days: slint::ModelRc::new(slint::VecModel::from(
                    (0..7)
                        .map(|column| {
                            let index = week * 7 + column;
                            CalendarDayCell {
                                key: format!("gl-calendar-day-{index}").into(),
                                label: (index % 31 + 1).to_string().into(),
                                description: format!("GL civil day {index}").into(),
                                off_month: week == 5,
                                today: index == 10,
                                selected: index == 10,
                            }
                        })
                        .collect::<Vec<_>>(),
                )),
            })
            .collect::<Vec<_>>(),
    )));
    calendar.window().set_size(slint::LogicalSize::new(
        calendar.get_popup_content_width(),
        calendar.get_popup_content_height(),
    ));
    calendar.show().unwrap();

    // Paint-only recording fixture: no DesktopHost/native Shell is wired here.
    let dock = Dock::new().unwrap();
    dock.set_pinned_apps(slint::ModelRc::new(slint::VecModel::from(vec![DockApp {
        key: "fixture-app".into(),
        label: "Fixture app".into(),
        icon: slint::Image::default(),
        pinned: true,
    }])));
    dock.window().set_size(slint::LogicalSize::new(216.0, 72.0));
    dock.show().unwrap();

    // Paint-only Power fixture: no actor, DesktopHost, native query or OS action.
    let power = PowerMenuSurface::new().unwrap();
    power
        .window()
        .set_size(slint::LogicalSize::new(1600.0, 900.0));
    power.set_user_name("Native fixture user".into());
    power.show().unwrap();

    let completed = Rc::new(Cell::new(false));
    let result = Rc::clone(&completed);
    slint::spawn_local(async move {
        launcher.window().winit_window().await.unwrap();
        tooltip.window().winit_window().await.unwrap();
        menu.window().winit_window().await.unwrap();
        quick.window().winit_window().await.unwrap();
        user.window().winit_window().await.unwrap();
        calendar.window().winit_window().await.unwrap();
        dock.window().winit_window().await.unwrap();
        power.window().winit_window().await.unwrap();
        let power_scale = power.window().scale_factor();
        assert!(power_scale == 1.0 || power_scale == 2.0);
        // Physical aggregate [-640,-160..960,740], selected full [0,0..960,740].
        // Native root DPI is observed, never borrowed from another surface.
        power.window().set_size(slint::PhysicalSize::new(1600, 900));
        power.set_selected_x(640.0 / power_scale);
        power.set_selected_y(160.0 / power_scale);
        power.set_selected_width(960.0 / power_scale);
        power.set_selected_height(740.0 / power_scale);
        power.set_metric_scale(1.5 / power_scale);
        power
            .global::<FocusTokens>()
            .set_outline_width(3.0 / power_scale);
        power
            .global::<FocusTokens>()
            .set_outline_offset(3.0 / power_scale);
        verify_frame("launcher", &launcher);
        verify_launcher_fullscreen_edges(&launcher);
        verify_frame("tooltip", &tooltip);
        verify_frame("context-menu", &menu);
        verify_frame("quick-settings", &quick);
        verify_frame("user-menu", &user);
        verify_user_fallback_frames(&user);
        verify_frame("calendar", &calendar);
        verify_frame("keyboard-selector", &keyboard);
        verify_frame("network-selector", &network);
        verify_frame("bluetooth-selector", &bluetooth);
        verify_frame("dock", &dock);
        verify_dock_reference_paint(&dock);
        // Stock control colors have their own 150ms transitions. Let the
        // genuine loop settle them; cold frame pixels are not theme proof.
        launcher.apply_presentation_theme(PresentationTheme::uniform(ColorScheme::Dark));
        menu.apply_presentation_theme(PresentationTheme::uniform(ColorScheme::Dark));
        slint::Timer::single_shot(std::time::Duration::from_millis(200), move || {
            verify_power_frames(&power);
            verify_launcher_controls(ColorScheme::Dark, &launcher);
            verify_menu_press_scale(&menu);
            verify_menu_application_image(&menu);
            launcher.apply_presentation_theme(PresentationTheme::uniform(ColorScheme::Light));
            menu.apply_presentation_theme(PresentationTheme::uniform(ColorScheme::Light));
            slint::Timer::single_shot(std::time::Duration::from_millis(200), move || {
                verify_launcher_controls(ColorScheme::Light, &launcher);
                verify_menu_press_scale(&menu);
                verify_menu_application_image(&menu);
                verify_launcher_reorder_preview(
                    launcher,
                    Box::new(move |launcher| {
                        launcher.hide().unwrap();
                        tooltip.hide().unwrap();
                        menu.hide().unwrap();
                        quick.hide().unwrap();
                        user.hide().unwrap();
                        calendar.hide().unwrap();
                        keyboard.hide().unwrap();
                        network.hide().unwrap();
                        bluetooth.hide().unwrap();
                        dock.hide().unwrap();
                        power.hide().unwrap();
                        result.set(true);
                        slint::quit_event_loop().unwrap();
                    }),
                );
            });
        });
    })
    .unwrap();
    slint::run_event_loop().unwrap();
    assert!(
        completed.get(),
        "The real GL scenario must reach every frame assertion"
    );
}

fn verify_power_frames(power: &PowerMenuSurface) {
    use i_slint_backend_testing::ElementQuery;
    use slint::platform::{Key, PointerEventButton, WindowEvent};

    let native_gl = Rc::new(Cell::new(false));
    power
        .window()
        .set_rendering_notifier({
            let native_gl = Rc::clone(&native_gl);
            move |_, graphics| {
                if matches!(graphics, slint::GraphicsAPI::NativeOpenGL { .. }) {
                    native_gl.set(true);
                }
            }
        })
        .expect("Power pixels must traverse production native GL");
    let actions = Rc::new(Cell::new(0));
    power.on_action_requested({
        let actions = Rc::clone(&actions);
        move |action| {
            assert_eq!(action, PowerMenuAction::LockSession);
            actions.set(actions.get() + 1);
        }
    });
    let element = |id: &str| {
        let matches = ElementQuery::from_root(power)
            .match_id(format!("PowerMenuSurface::{id}"))
            .find_all();
        assert_eq!(matches.len(), 1, "one genuine Power {id}");
        matches.into_iter().next().unwrap()
    };
    let tile_child = |tile: &str, child: &str| {
        let matches = element(tile)
            .query_descendants()
            .match_id(format!("PowerActionTile::{child}"))
            .find_all();
        assert_eq!(matches.len(), 1, "one genuine {tile} {child}");
        matches.into_iter().next().unwrap()
    };
    let window = power.window();
    let scale = window.scale_factor();
    let metric = power.get_metric_scale();
    let near = |actual: f32, expected: f32| {
        assert!((actual - expected).abs() < 0.05, "{actual} != {expected}");
    };
    near(metric * scale, 1.5);
    let snapshot = || {
        let frame = window.take_snapshot().expect("real Power GL pixels");
        assert!(native_gl.get(), "never silently substitute software pixels");
        assert_eq!((frame.width(), frame.height()), (1600, 900));
        frame
    };
    let sample = |frame: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, x: f32, y: f32| {
        let x = (x * scale).floor() as usize;
        let y = (y * scale).floor() as usize;
        assert!(x < frame.width() as usize && y < frame.height() as usize);
        frame.as_slice()[y * frame.width() as usize + x]
    };
    let ink_count = |frame: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
                     origin: slint::LogicalPosition,
                     extent: f32,
                     foreground: u8| {
        let left = (origin.x * scale).floor() as usize;
        let top = (origin.y * scale).floor() as usize;
        let right = ((origin.x + extent) * scale).ceil() as usize;
        let bottom = ((origin.y + extent) * scale).ceil() as usize;
        (top..bottom)
            .flat_map(|y| (left..right).map(move |x| (x, y)))
            .filter(|(x, y)| {
                let p = frame.as_slice()[y * frame.width() as usize + x];
                p.a == 255
                    && [p.r, p.g, p.b]
                        .into_iter()
                        .all(|c| c.abs_diff(foreground) <= 1)
            })
            .count()
    };
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));
    for (theme, scheme, background, foreground) in [
        ("dark", ColorScheme::Dark, 31, 228),
        ("light", ColorScheme::Light, 252, 18),
    ] {
        power.apply_presentation_theme(PresentationTheme::uniform(scheme));
        window.dispatch_event(WindowEvent::PointerExited);
        power.invoke_focus_content();
        let idle = snapshot();
        let viewport = element("selected-viewport");
        near(viewport.absolute_position().x * scale, 640.0);
        near(viewport.absolute_position().y * scale, 160.0);
        near(viewport.size().width * scale, 960.0);
        near(viewport.size().height * scale, 740.0);
        let body = element("body");
        let origin = body.absolute_position();
        let size = body.size();
        near(size.width, 460.0 * metric);
        near(size.height, 433.0 * metric);
        near((origin.x + size.width / 2.0) * scale, 1120.0);
        near((origin.y + size.height / 2.0) * scale, 530.0);
        let left = origin.x + 32.0 * metric;
        let top = origin.y + 32.0 * metric;
        let right = origin.x + size.width - 32.0 * metric;
        let bottom = origin.y + size.height - 32.0 * metric;
        let center_x = (left + right) / 2.0;
        let center_y = (top + bottom) / 2.0;
        let body_pixel = sample(&idle, right - 12.0 * metric, top + 40.0 * metric);
        assert_eq!(
            (body_pixel.r, body_pixel.g, body_pixel.b, body_pixel.a),
            (background, background, background, 255),
            "Power {theme} bg-light role",
        );
        let scrim = sample(&idle, 1.0, 1.0);
        assert_eq!(scrim.a, 102, "source .4 scrim, not opaque desktop paint");
        assert!(
            [scrim.r, scrim.g, scrim.b]
                .into_iter()
                .all(|c| c.abs_diff(7) <= 1)
        );
        assert!(sample(&idle, left + 2.0 * metric, top + 2.0 * metric).a < 255);
        assert_eq!(
            sample(&idle, left + 16.0 * metric, top + 2.0 * metric),
            body_pixel
        );
        // Premultiplied black shadow over the scrim: .4 + .6*.24 <= .544.
        // The source positive8 offset must be observable on BOTH axes.
        for (positive, negative, farther) in [
            (
                sample(&idle, right + 8.0 * metric, center_y),
                sample(&idle, left - 8.0 * metric, center_y),
                sample(&idle, right + 24.0 * metric, center_y),
            ),
            (
                sample(&idle, center_x, bottom + 8.0 * metric),
                sample(&idle, center_x, top - 8.0 * metric),
                sample(&idle, center_x, bottom + 24.0 * metric),
            ),
        ] {
            assert!(
                positive.a > negative.a,
                "Power {theme} actual positive8 shadow offset"
            );
            assert!(
                positive.a > scrim.a && positive.a <= 140,
                "Power {theme} black-.24 shadow: {positive:?}"
            );
            assert!(
                farther.a >= scrim.a && farther.a < positive.a,
                "actual24 blur fades"
            );
            assert_eq!(positive.r, positive.g);
            assert_eq!(positive.g, positive.b);
            let shadow_alpha = f32::from(positive.a - scrim.a) / f32::from(255 - scrim.a);
            let expected = (f32::from(scrim.r) * (1.0 - shadow_alpha)).round() as u8;
            assert!(
                positive.r.abs_diff(expected) <= 1,
                "black shadow retains premultiplication"
            );
        }
        let lock = element("lock");
        let lock_origin = lock.absolute_position();
        let lock_size = lock.size();
        near(lock_size.width, 100.0 * metric);
        near(lock_size.height, 100.0 * metric);
        let icon = tile_child("lock", "icon");
        let icon_origin = icon.absolute_position();
        near(icon.size().width, 25.0 * metric);
        near(icon.size().height, 25.0 * metric);
        near(
            tile_child("lock", "label").absolute_position().y - icon_origin.y - icon.size().height,
            4.0 * metric,
        );
        let idle_ink = ink_count(&idle, icon_origin, 25.0 * metric, foreground);
        assert!(idle_ink > 50, "licensed real Lock glyph must render/tint");
        export_frame(&format!("gl-power-{theme}-idle-{scale}x"), &idle);

        let center = slint::LogicalPosition::new(
            lock_origin.x + lock_size.width / 2.0,
            lock_origin.y + lock_size.height / 2.0,
        );
        window.dispatch_event(WindowEvent::PointerMoved { position: center });
        let hover = snapshot();
        near(lock.absolute_position().x, lock_origin.x - 2.5 * metric);
        near(lock.absolute_position().y, lock_origin.y - 6.5 * metric);
        assert_eq!(
            lock.size(),
            lock_size,
            "hover never rewrites action layout bounds"
        );
        let hover_icon = icon.absolute_position();
        near(hover_icon.x, center.x + (icon_origin.x - center.x) * 1.05);
        near(
            hover_icon.y,
            center.y + (icon_origin.y - center.y) * 1.05 - 4.0 * metric,
        );
        assert!(
            ink_count(&hover, hover_icon, 25.0 * metric * 1.05, foreground) > idle_ink,
            "native GL scales the actual icon subtree, not only queried geometry"
        );
        let action_shadow_point = (lock_origin.x - 8.0 * metric, center.y);
        let action_shadow = sample(&hover, action_shadow_point.0, action_shadow_point.1);
        assert_eq!(action_shadow.a, 255, "action shadow overlays opaque body");
        assert!(
            action_shadow.r < background
                && action_shadow.r >= (f32::from(background) * 0.8).round() as u8 - 1,
            "real hover black-.2 shadow0x8/blur16: {action_shadow:?}"
        );
        export_frame(&format!("gl-power-{theme}-hover-{scale}x"), &hover);

        let before = actions.get();
        window.dispatch_event(WindowEvent::PointerPressed {
            position: center,
            button: PointerEventButton::Left,
        });
        let pressed = snapshot();
        assert_eq!(actions.get(), before, "press is not a command");
        near(lock.absolute_position().x, lock_origin.x + 2.5 * metric);
        near(lock.absolute_position().y, lock_origin.y + 2.5 * metric);
        assert_eq!(lock.size(), lock_size);
        assert_eq!(
            sample(&pressed, lock_origin.x + 0.5 * metric, center.y),
            body_pixel,
            "actual .95 rendering uncovers original edge"
        );
        assert_ne!(
            sample(&pressed, lock_origin.x + 3.5 * metric, center.y),
            body_pixel,
            "actual .95 rendering shrinks, not hides, pressed action"
        );
        assert_eq!(
            sample(&pressed, action_shadow_point.0, action_shadow_point.1),
            body_pixel,
            "active removes hover shadow"
        );
        export_frame(&format!("gl-power-{theme}-pressed-{scale}x"), &pressed);
        window.dispatch_event(WindowEvent::PointerReleased {
            position: center,
            button: PointerEventButton::Left,
        });
        assert_eq!(
            actions.get(),
            before + 1,
            "only one typed paint-fixture intent"
        );
        window.dispatch_event(WindowEvent::PointerExited);
        power.invoke_focus_content();
        snapshot();
        window.dispatch_event(WindowEvent::KeyPressed {
            text: Key::Tab.into(),
        });
        window.dispatch_event(WindowEvent::KeyReleased {
            text: Key::Tab.into(),
        });
        let focused = snapshot();
        near(lock.absolute_position().x, lock_origin.x);
        near(lock.absolute_position().y, lock_origin.y);
        let ring = sample(&focused, lock_origin.x - 3.0 * metric, center.y);
        assert_ne!(ring, body_pixel, "real external2 outline/2 offset");
        assert_eq!(
            sample(&focused, lock_origin.x - 5.0 * metric, center.y),
            body_pixel,
            "focus paint stops outside its source4 margin"
        );
        assert_eq!(
            actions.get(),
            before + 1,
            "native Tab/focus never activates Lock"
        );
        export_frame(&format!("gl-power-{theme}-focused-{scale}x"), &focused);

        // Six additional genuine GL frames per palette; the original four
        // Lock-state frames above remain independent and keep their names.
        power.invoke_focus_content();
        let grid = snapshot();
        let ids = [
            "lock",
            "log-out",
            "power-off",
            "reboot",
            "suspend",
            "hibernate",
        ];
        for (index, id) in ids.into_iter().enumerate() {
            let tile = element(id);
            assert_eq!(tile.accessible_enabled(), Some(true));
            let position = tile.absolute_position();
            near(
                position.x - lock_origin.x,
                (index % 3) as f32 * 124.0 * metric,
            );
            near(
                position.y - lock_origin.y,
                (index / 3) as f32 * 124.0 * metric,
            );
            near(tile.size().width, 100.0 * metric);
            near(tile.size().height, 100.0 * metric);
            let tile_icon = tile_child(id, "icon");
            near(tile_icon.size().width, 25.0 * metric);
            near(tile_icon.size().height, 25.0 * metric);
            let ink = ink_count(
                &grid,
                tile_icon.absolute_position(),
                25.0 * metric,
                foreground,
            );
            if id == "suspend" {
                let suspend_image_size = power.get_suspend_icon().size();
                assert_eq!(
                    (suspend_image_size.width, suspend_image_size.height),
                    (0, 0)
                );
                assert_eq!(
                    ink, 0,
                    "license-blocked moon slot is EMPTY, not pixel parity"
                );
            } else {
                assert!(ink > 0, "genuine {id} artwork must paint");
            }
        }
        export_frame(&format!("gl-power-{theme}-full-six-grid-{scale}x"), &grid);

        power.set_user_name("Native bounded account".into());
        let header = snapshot();
        assert_eq!(
            element("greeting").accessible_label().as_deref(),
            Some("Goodbye Native bounded account")
        );
        export_frame(
            &format!("gl-power-{theme}-source-header-username-{scale}x"),
            &header,
        );
        power.set_user_name("Native fixture user".into());

        power.set_updates_known_pending(true);
        power.set_install_updates(true);
        power.invoke_focus_content();
        let pending_checked = snapshot();
        let expected_row = crate::power_menu_render_tests::source_pending_row_height(power, metric);
        near(element("updates-row").size().height, expected_row);
        near(
            element("body").size().height,
            (433.0 * metric + 24.0 * metric + expected_row)
                .min(element("selected-viewport").size().height),
        );
        near(element("updates-choice").size().width, 348.0 * metric);
        near(element("updates-choice").size().height, expected_row);
        let pending_origin = element("body").absolute_position();
        for (index, id) in ids.into_iter().enumerate() {
            let position = element(id).absolute_position();
            near(
                position.x,
                pending_origin.x + (56.0 + (index % 3) as f32 * 124.0) * metric,
            );
            near(
                position.y,
                pending_origin.y + (177.0 + (index / 3) as f32 * 124.0) * metric + expected_row,
            );
        }
        assert_eq!(
            element("power-off").accessible_label().as_deref(),
            Some("Update and shut down")
        );
        assert_eq!(
            element("reboot").accessible_label().as_deref(),
            Some("Update and restart")
        );
        for id in ["power-off", "reboot"] {
            let badge = tile_child(id, "update-badge");
            near(badge.size().width, 10.0 * metric);
            near(badge.size().height, 10.0 * metric);
            let badge_origin = badge.absolute_position();
            let pixel = sample(
                &pending_checked,
                badge_origin.x + 5.0 * metric,
                badge_origin.y + 5.0 * metric,
            );
            assert_eq!(pixel.a, 255);
            assert_ne!(
                (pixel.r, pixel.g, pixel.b),
                (background, background, background)
            );
        }
        export_frame(
            &format!("gl-power-{theme}-pending-checked-{scale}x"),
            &pending_checked,
        );
        // Tab enters the actual public std-widgets CheckBox before the grid.
        // Space changes local choice only; no actor or native host is connected.
        for key in [Key::Tab, Key::Space] {
            window.dispatch_event(WindowEvent::KeyPressed { text: key.into() });
            window.dispatch_event(WindowEvent::KeyReleased { text: key.into() });
        }
        assert!(!power.get_install_updates());
        let pending_unchecked = snapshot();
        assert_eq!(
            element("power-off").accessible_label().as_deref(),
            Some("Power off")
        );
        assert_eq!(
            element("reboot").accessible_label().as_deref(),
            Some("Reboot")
        );
        export_frame(
            &format!("gl-power-{theme}-pending-unchecked-{scale}x"),
            &pending_unchecked,
        );

        power.set_updates_known_pending(false);
        power.set_updates_status("Update status unavailable".into());
        power.invoke_focus_content();
        let warning = snapshot();
        let message = element("updates-message");
        assert_eq!(
            message.accessible_label().as_deref(),
            Some("Update status unavailable")
        );
        let expected_message = crate::power_menu_render_tests::source_text_height(
            power,
            "Update status unavailable",
            348.0,
            metric,
        );
        near(message.size().height, expected_message);
        near(
            element("body").size().height,
            (433.0 * metric + expected_message + 24.0 * metric)
                .min(element("selected-viewport").size().height),
        );
        assert_eq!(element("suspend").accessible_enabled(), Some(true));
        export_frame(
            &format!("gl-power-{theme}-unknown-warning-{scale}x"),
            &warning,
        );

        power.set_updates_status("".into());
        power.invoke_focus_content();
        snapshot();
        for _ in 0..6 {
            window.dispatch_event(WindowEvent::KeyPressed {
                text: Key::Tab.into(),
            });
            window.dispatch_event(WindowEvent::KeyReleased {
                text: Key::Tab.into(),
            });
        }
        let last_focus = snapshot();
        let hibernate = element("hibernate");
        let last_origin = hibernate.absolute_position();
        let ring = sample(
            &last_focus,
            last_origin.x - 3.0 * metric,
            last_origin.y + 50.0 * metric,
        );
        assert_ne!(
            (ring.r, ring.g, ring.b),
            (background, background, background)
        );
        assert_eq!(
            ring.a, 255,
            "last real action owns the external focus outline"
        );
        export_frame(
            &format!("gl-power-{theme}-last-tab-focus-{scale}x"),
            &last_focus,
        );
        assert_eq!(
            actions.get(),
            before + 1,
            "status, checkbox and focus have zero OS effects"
        );
        power.set_install_updates(true);
        power.invoke_focus_content();
    }
    assert_eq!(actions.get(), 2, "paint fixture cannot invoke an OS action");
}

fn verify_user_fallback_frames(user: &UserMenu) {
    use i_slint_backend_testing::ElementHandle;

    user.set_has_photo(false);
    user.set_profile_fallback(true);
    for (theme, scheme, gray) in [
        ("dark", ColorScheme::Dark, 61),
        ("light", ColorScheme::Light, 193),
    ] {
        user.apply_presentation_theme(PresentationTheme::uniform(scheme));
        let frame = user.window().take_snapshot().unwrap();
        let avatars = ElementHandle::find_by_accessible_label(user, "Default user profile")
            .collect::<Vec<_>>();
        assert_eq!(avatars.len(), 1);
        let avatar = avatars[0].absolute_position();
        let scale = user.window().scale_factor();
        let point = frame.as_slice()[((avatar.y + 20.0) * scale) as usize * frame.width() as usize
            + ((avatar.x + 14.0) * scale) as usize];
        assert_eq!(
            (point.r, point.g, point.b, point.a),
            (gray, gray, gray, 255)
        );
        export_frame(&format!("gl-user-fallback-{theme}-{scale}x"), &frame);
    }
    user.set_profile_fallback(false);
    user.set_has_photo(true);
}

fn verify_frame<C: ThemedComponent>(name: &str, component: &C) {
    let native_gl = Rc::new(Cell::new(false));
    component
        .window()
        .set_rendering_notifier({
            let native_gl = Rc::clone(&native_gl);
            move |_, graphics| {
                if matches!(graphics, slint::GraphicsAPI::NativeOpenGL { .. }) {
                    native_gl.set(true);
                }
            }
        })
        .expect("The selected renderer must support native GL, never silently fall back");
    for (theme, scheme, background) in [
        ("dark", ColorScheme::Dark, 24),
        ("light", ColorScheme::Light, 242),
    ] {
        component.apply_presentation_theme(PresentationTheme::uniform(scheme));
        let window = component.window();
        let scale = window.scale_factor();
        let frame = window
            .take_snapshot()
            .expect("The production GL renderer must supply actual pixels");
        assert!(
            native_gl.get(),
            "The snapshot must traverse the real OpenGL renderer"
        );
        let width = frame.width() as usize;
        let height = frame.height() as usize;
        let logical_width = width as f32 / scale;
        let logical_height = height as f32 / scale;
        let sample =
            |x: f32, y: f32| frame.as_slice()[(y * scale) as usize * width + (x * scale) as usize];
        let body = sample(logical_width / 2.0, 15.0);
        if name == "dock" {
            // Native GL snapshots retain premultiplied channels. Account for
            // separate RGBA8 draw-pass rounding; opaque popup checks stay exact.
            let body_color = (f32::from(background) * 0.8).round() as u8;
            for channel in [body.r, body.g, body.b] {
                assert!(
                    channel.abs_diff(body_color) <= 1,
                    "{name} {theme} body: {body:?}"
                );
            }
            assert_eq!(body.a, 204, "{name} {theme} .8-alpha bar");
            let foreground = if scheme == ColorScheme::Dark { 228 } else { 18 };
            // TileButton is filled: its opaque neutral tile, not the bare
            // translucent bar, is the duotone screen's actual substrate.
            let tile_color = if scheme == ColorScheme::Dark { 31 } else { 252 };
            let tile = sample(72.0, 20.0);
            assert_eq!(
                (tile.r, tile.g, tile.b, tile.a),
                (tile_color, tile_color, tile_color, 255),
                "{name} {theme} utility tile"
            );
            let tint_color = (f32::from(foreground) * 0.2).round() as u8;
            let screen = sample(84.0, 33.0);
            let screen_color = (f32::from(tint_color) + f32::from(tile_color) * 0.8).round() as u8;
            assert_eq!(screen.a, 255, "{name} {theme} screen over opaque tile");
            for channel in [screen.r, screen.g, screen.b] {
                assert!(
                    channel.abs_diff(screen_color) <= 1,
                    "{name} {theme} screen tint: {screen:?}"
                );
            }
            let bezel = sample(84.0, 27.0);
            assert_eq!(bezel.a, 255, "{name} {theme} opaque bezel: {bezel:?}");
            for channel in [bezel.r, bezel.g, bezel.b] {
                assert_eq!(channel, foreground, "{name} {theme} bezel tint");
            }
            // Generic independent Trash identity artwork, not reference
            // empty/full SVG parity. The trailing tile is fixed at x=160.
            let trash_exterior = sample(166.0, 36.0);
            assert_eq!(
                (
                    trash_exterior.r,
                    trash_exterior.g,
                    trash_exterior.b,
                    trash_exterior.a
                ),
                (tile_color, tile_color, tile_color, 255),
                "{name} {theme} transparent Trash exterior over neutral tile"
            );
            let trash_interior = sample(180.0, 36.0);
            assert_eq!(trash_interior.a, 255, "{name} {theme} Trash interior");
            for channel in [trash_interior.r, trash_interior.g, trash_interior.b] {
                assert!(
                    channel.abs_diff(screen_color) <= 1,
                    "{name} {theme} original .2-alpha Trash interior: {trash_interior:?}"
                );
            }
            let trash_lid = sample(180.0, 28.0);
            assert_eq!(trash_lid.a, 255, "{name} {theme} opaque Trash lid");
            for channel in [trash_lid.r, trash_lid.g, trash_lid.b] {
                assert_eq!(channel, foreground, "{name} {theme} Trash lid tint");
            }
        } else {
            assert_eq!(
                (body.r, body.g, body.b, body.a),
                (background, background, background, 255),
                "{name} {theme} body"
            );
        }
        assert_eq!(sample(0.0, 0.0).a, 0, "{name} outside frame");
        if name == "dock" {
            assert_eq!(
                sample(logical_width / 2.0, logical_height - 4.0).a,
                0,
                "Dock outer margin"
            );
        } else {
            let shadow = sample(logical_width / 2.0, logical_height - 8.0);
            assert!(
                shadow.a > 0 && shadow.a < 255,
                "{name} {theme} {scale}x reference shadow: {shadow:?}"
            );
            assert_eq!((shadow.r, shadow.g, shadow.b), (0, 0, 0));
        }
        export_frame(&format!("gl-{name}-{theme}-{scale}x"), &frame);
    }
}

fn verify_launcher_fullscreen_edges(launcher: &Launcher) {
    let window = launcher.window();
    for (theme, scheme, background) in [
        ("dark", ColorScheme::Dark, 24),
        ("light", ColorScheme::Light, 242),
    ] {
        launcher.apply_presentation_theme(PresentationTheme::uniform(scheme));
        launcher.set_display_mode(LauncherDisplayMode::Windowed);
        let windowed = window.take_snapshot().expect("actual GL windowed frame");
        let width = windowed.width() as usize;
        let height = windowed.height() as usize;
        let scale = window.scale_factor();
        let shadow_index = (height - (8.0 * scale) as usize) * width + width / 2;
        let shadow = windowed.as_slice()[shadow_index];
        assert_eq!(windowed.as_slice()[0].a, 0);
        assert!(
            shadow.a > 0 && shadow.a < 255,
            "windowed shadow remains real"
        );
        launcher.set_display_mode(LauncherDisplayMode::Fullscreen);
        let frame = window.take_snapshot().expect("actual GL fullscreen frame");
        assert_eq!(
            (frame.width(), frame.height()),
            (windowed.width(), windowed.height())
        );
        for index in (0..width)
            .chain((height - 1) * width..height * width)
            .chain((0..height).flat_map(|y| [y * width, y * width + width - 1]))
        {
            let pixel = frame.as_slice()[index];
            assert_eq!(
                (pixel.r, pixel.g, pixel.b, pixel.a),
                (background, background, background, 255),
                "native GL {theme} {scale}x fullscreen edge {index}"
            );
        }
        verify_launcher_footer_glyph(launcher, scheme, "Contract applications menu", &frame);
        assert!(
            !window.is_fullscreen(),
            "the monitor overlay is not backend fullscreen"
        );
        export_frame(&format!("gl-launcher-fullscreen-{theme}-{scale}x"), &frame);
        launcher.set_display_mode(LauncherDisplayMode::Windowed);
        let restored = window.take_snapshot().expect("actual GL restored frame");
        assert_eq!(restored.as_slice()[0].a, 0);
        assert_eq!(
            restored.as_slice()[shadow_index],
            shadow,
            "windowed shadow restores"
        );
    }
}

fn verify_launcher_controls(scheme: ColorScheme, launcher: &Launcher) {
    let theme = match scheme {
        ColorScheme::Dark => "dark",
        ColorScheme::Light => "light",
        _ => panic!("The settlement scenario requires an explicit color scheme"),
    };
    let assert_control = |label: &str, frame: &slint::SharedPixelBuffer<slint::Rgba8Pixel>| {
        let button =
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(launcher, label)
                .next()
                .unwrap();
        let position = button.absolute_position();
        let scale = launcher.window().scale_factor();
        let x = ((position.x + button.size().width / 2.0) * scale) as usize;
        let y = ((position.y + 4.0) * scale) as usize;
        let pixel = frame.as_slice()[y * frame.width() as usize + x];
        assert_eq!(pixel.a, 255, "{label} must be opaque");
        assert!(
            if scheme == ColorScheme::Dark {
                pixel.r < 128 && pixel.g < 128 && pixel.b < 128
            } else {
                pixel.r > 200 && pixel.g > 200 && pixel.b > 200
            },
            "{label} must settle to its {theme} idle color: {pixel:?}",
        );
    };
    let frame = launcher.window().take_snapshot().unwrap();
    let scale = launcher.window().scale_factor();
    for label in [
        "Open user menu",
        "Open settings and recovery",
        "Expand applications menu",
        "Open power menu",
    ] {
        assert_control(label, &frame);
    }
    for label in ["Refresh the desktop", "Exit Tessera"] {
        assert!(
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(launcher, label)
                .next()
                .is_none(),
            "{label} is not a normal source footer control",
        );
    }
    verify_launcher_footer_glyph(launcher, scheme, "Expand applications menu", &frame);
    verify_launcher_footer_glyph(launcher, scheme, "Open power menu", &frame);
    export_frame(&format!("gl-launcher-{theme}-settled-{scale}x"), &frame);
    launcher.set_feedback_visible(true);
    let recovery = launcher.window().take_snapshot().unwrap();
    for label in ["Refresh the desktop", "Exit Tessera"] {
        assert_control(label, &recovery);
    }
    export_frame(&format!("gl-launcher-{theme}-recovery-{scale}x"), &recovery);
    launcher.set_feedback_visible(false);
    verify_launcher_press_scale(launcher);
}

// Genuine fixed input slots, native GL shadow pixels and paint-only press offset.
fn verify_dock_reference_paint(dock: &Dock) {
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::{Key, PointerEventButton, WindowEvent};

    let actions = Rc::new(Cell::new(0));
    let count = actions.clone();
    dock.on_open_applications_requested(move || count.set(count.get() + 1));
    let tile = ElementHandle::find_by_accessible_label(dock, "Open applications and settings")
        .next()
        .unwrap();
    let image = tile
        .query_descendants()
        .match_type_name("Image")
        .find_first()
        .unwrap();
    let position = tile.absolute_position();
    let size = tile.size();
    let scale = dock.window().scale_factor();
    let center = slint::LogicalPosition::new(
        position.x + size.width / 2.0,
        position.y + size.height / 2.0,
    );
    let sample = |frame: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, x: f32, y: f32| {
        frame.as_slice()[(y * scale) as usize * frame.width() as usize + (x * scale) as usize]
    };
    dock.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    for (theme, scheme) in [("light", ColorScheme::Light), ("dark", ColorScheme::Dark)] {
        dock.apply_presentation_theme(PresentationTheme::uniform(scheme));
        dock.window().dispatch_event(WindowEvent::PointerExited);
        let idle = dock.window().take_snapshot().unwrap();
        let shadow = sample(&idle, position.x + size.width + 0.5, center.y);
        let body = sample(&idle, position.x + size.width + 5.5, center.y);
        assert!(
            shadow.a > body.a,
            "the ordinary tile shadow must paint outside its fixed slot: {theme} {shadow:?} vs {body:?}"
        );
        assert!(
            shadow.r <= body.r && shadow.g <= body.g && shadow.b <= body.b,
            "the source black shadow must not brighten the bar"
        );
        export_frame(&format!("gl-dock-{theme}-shadow-{scale}x"), &idle);
        for source in [PressSource::Pointer, PressSource::Space] {
            dock.window()
                .dispatch_event(WindowEvent::PointerMoved { position: center });
            let hover = dock.window().take_snapshot().unwrap();
            let image_position = image.absolute_position();
            let before = actions.get();
            match source {
                PressSource::Pointer => dock.window().dispatch_event(WindowEvent::PointerPressed {
                    position: center,
                    button: PointerEventButton::Left,
                }),
                PressSource::Space => {
                    dock.window().dispatch_event(WindowEvent::KeyPressed {
                        text: Key::Space.into(),
                    });
                    dock.window().dispatch_event(WindowEvent::KeyPressRepeated {
                        text: Key::Space.into(),
                    });
                }
            }
            let pressed = dock.window().take_snapshot().unwrap();
            assert_eq!(actions.get(), before, "press/repeat never activates");
            assert_eq!(
                tile.absolute_position(),
                position,
                "the input slot never moves"
            );
            assert_eq!(
                tile.size(),
                size,
                "ordinary dock tiles do not scale on press"
            );
            assert_eq!(
                image.absolute_position(),
                slint::LogicalPosition::new(image_position.x, image_position.y + 2.0)
            );
            assert_ne!(
                sample(&hover, center.x, position.y + 0.5),
                sample(&pressed, center.x, position.y + 0.5),
                "native GL must paint the 2px translation"
            );
            export_frame(
                &format!("gl-dock-{theme}-{source:?}-pressed-{scale}x"),
                &pressed,
            );
            match source {
                PressSource::Pointer => {
                    dock.window().dispatch_event(WindowEvent::PointerReleased {
                        position: center,
                        button: PointerEventButton::Left,
                    })
                }
                PressSource::Space => dock.window().dispatch_event(WindowEvent::KeyReleased {
                    text: Key::Space.into(),
                }),
            }
            assert_eq!(actions.get(), before + 1, "one real release acts once");
            assert_eq!(image.absolute_position(), image_position);
            assert_eq!(tile.absolute_position(), position);
            assert_eq!(tile.size(), size);
            dock.window().dispatch_event(WindowEvent::PointerExited);
        }
    }
    assert_eq!(actions.get(), 4);
}

fn verify_launcher_footer_glyph(
    launcher: &Launcher,
    scheme: ColorScheme,
    label: &str,
    frame: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
) {
    let control = i_slint_backend_testing::ElementHandle::find_by_accessible_label(launcher, label)
        .next()
        .unwrap();
    let position = control.absolute_position();
    let scale = launcher.window().scale_factor();
    let width = frame.width() as usize;
    let foreground_pixels = (0..20)
        .flat_map(|y| (0..20).map(move |x| (x, y)))
        .filter(|(x, y)| {
            let x = ((position.x + 6.0 + *x as f32) * scale) as usize;
            let y = ((position.y + 6.0 + *y as f32) * scale) as usize;
            let pixel = frame.as_slice()[y * width + x];
            pixel.a == 255
                && if scheme == ColorScheme::Dark {
                    pixel.r > 128 && pixel.g > 128 && pixel.b > 128
                } else {
                    pixel.r < 128 && pixel.g < 128 && pixel.b < 128
                }
        })
        .count();
    assert!(
        foreground_pixels > 8,
        "the licensed {label} glyph must actually render/tint"
    );
}

fn verify_launcher_press_scale(launcher: &Launcher) {
    launcher.invoke_focus_search();
    let launches = Rc::new(Cell::new(0));
    let count = launches.clone();
    launcher.on_launch_requested(move |_| count.set(count.get() + 1));
    let tile = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        launcher,
        "Launch Native app",
    )
    .next()
    .unwrap();
    for source in [PressSource::Pointer, PressSource::Space] {
        verify_native_press_scale(launcher.window(), &tile, 0.95, 0.0, source, &launches);
    }
    assert_eq!(launches.get(), 2, "pointer and Space each launch once");
}

// Owned Mesa/OpenGL pixels and real SDK dispatch, not Windows compositor,
// OLE capture, persistence, or a production snapshot-based feedback path.
struct GlDragSource {
    tile: LaunchTile,
    bounds: TileBounds,
    press: slint::LogicalPosition,
}

fn publish_gl_drag_visual(
    launcher: &Launcher,
    source: &RefCell<Option<Rc<GlDragSource>>>,
    source_scale: &Cell<Option<f32>>,
    event: &slint::language::DropEvent,
    origin: slint::LogicalPosition,
) -> bool {
    let Some(payload) = event
        .data
        .user_data()
        .and_then(|data| data.downcast::<GlDragSource>().ok())
    else {
        return false;
    };
    if !launcher.get_reorder_dragging()
        || !source
            .borrow()
            .as_ref()
            .is_some_and(|source| Rc::ptr_eq(source, &payload))
    {
        return false;
    }
    let Some(source_scale) = source_scale.get() else {
        return false;
    };
    launcher.set_reorder_visual(LauncherDragVisual {
        visible: true,
        source: payload.tile.clone(),
        source_scale,
        bounds: TileBounds {
            origin: slint::LogicalPosition::new(
                payload.bounds.origin.x + origin.x + event.position.x - payload.press.x,
                payload.bounds.origin.y + origin.y + event.position.y - payload.press.y,
            ),
            width: payload.bounds.width,
            height: payload.bounds.height,
        },
    });
    true
}

// Metadata additions/subtractions round at the native f32 input precision,
// not at a renderer pixel or an arbitrary logical-geometry tolerance.
fn assert_gl_input_position(
    actual: slint::LogicalPosition,
    expected: slint::LogicalPosition,
    inputs: &[slint::LogicalPosition],
) {
    let precision = f32::EPSILON
        * inputs
            .iter()
            .flat_map(|position| [position.x.abs(), position.y.abs()])
            .fold(1.0, f32::max);
    assert!(
        (actual.x - expected.x).abs() <= precision && (actual.y - expected.y).abs() <= precision,
        "native f32 position {actual:?} != {expected:?}, input precision {precision}"
    );
}

fn assert_gl_launcher_tile_geometry(
    tile: &i_slint_backend_testing::ElementHandle,
    appearance: f32,
) {
    let origin = tile.absolute_position();
    let size = tile.size();
    let icon = tile
        .query_descendants()
        .match_type_name("ApplicationIcon")
        .find_first()
        .unwrap();
    let label = tile
        .query_descendants()
        .match_type_name("Text")
        .find_all()
        .into_iter()
        // Exclude the zero-sized font metrics probe, not the actual label track.
        .find(|label| label.size().height > 0.0)
        .unwrap();
    let track = tile
        .query_descendants()
        .match_id("LauncherTile::label-track")
        .find_first()
        .unwrap();
    let side = (size.height - 59.2)
        .max(0.0)
        .min((size.width - 16.0).max(0.0));
    assert!((icon.size().width - side).abs() < 0.001);
    assert!(
        (icon.size().height - side).abs() < 0.001,
        "genuine and fallback have one square frame"
    );
    assert_gl_input_position(
        icon.absolute_position(),
        slint::LogicalPosition::new(
            origin.x + (size.width - side) / 2.0 * appearance,
            origin.y + 8.0 * appearance,
        ),
        &[origin, slint::LogicalPosition::new(size.width, size.height)],
    );
    assert!(
        (track.size().height - 35.2).abs() < 0.001,
        "label track is fixed at both window widths"
    );
    assert!(
        (label.size().height - 35.84).abs() < 0.001,
        "native allocation fits both 17.92px line boxes"
    );
    let label_inputs = [
        origin,
        slint::LogicalPosition::new(size.width, size.height),
        track.absolute_position(),
    ];
    assert_gl_input_position(
        label.absolute_position(),
        track.absolute_position(),
        &label_inputs,
    );
    assert_gl_input_position(
        label.absolute_position(),
        slint::LogicalPosition::new(
            origin.x + 8.0 * appearance,
            origin.y + (size.height - 43.2) * appearance,
        ),
        &label_inputs,
    );
}

fn gl_region(
    frame: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    origin: slint::LogicalPosition,
    size: slint::LogicalSize,
    scale: f32,
) -> Vec<slint::Rgba8Pixel> {
    let mut pixels = Vec::new();
    for y in (origin.y * scale).ceil() as usize..((origin.y + size.height) * scale).floor() as usize
    {
        for x in
            (origin.x * scale).ceil() as usize..((origin.x + size.width) * scale).floor() as usize
        {
            pixels.push(frame.as_slice()[y * frame.width() as usize + x]);
        }
    }
    pixels
}

// The pinned compiler scales paint uniformly around the raw tile's center.
// Public logical bounds, hotspot and layout remain deliberately unscaled.
fn gl_scaled_tile_region(
    tile: &TileBounds,
    appearance: f32,
    origin: slint::LogicalPosition,
    size: slint::LogicalSize,
) -> (slint::LogicalPosition, slint::LogicalSize) {
    (
        slint::LogicalPosition::new(
            tile.origin.x + tile.width / 2.0 + (origin.x - tile.width / 2.0) * appearance,
            tile.origin.y + tile.height / 2.0 + (origin.y - tile.height / 2.0) * appearance,
        ),
        slint::LogicalSize::new(size.width * appearance, size.height * appearance),
    )
}

struct GlTilePaintSource {
    bounds: TileBounds,
    favorite: TileBounds,
}

fn assert_gl_whole_tile_pixels(
    pressed: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    baseline: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    floating: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    source: GlTilePaintSource,
    visual: LauncherDragVisual,
    scale: f32,
    genuine: bool,
) {
    let GlTilePaintSource {
        bounds: source,
        favorite,
    } = source;
    let appearance = visual.source_scale;
    let ghost = visual.bounds;
    // Integer logical translation retains subpixel phase at genuine 1x/2x.
    // Compare the whole icon/name skin, not just an image-size getter.
    let content = slint::LogicalSize::new(source.width - 16.0, source.height - 16.0);
    let (source_content_origin, scaled_content) = gl_scaled_tile_region(
        &source,
        appearance,
        slint::LogicalPosition::new(8.0, 8.0),
        content,
    );
    let (ghost_content_origin, _) = gl_scaled_tile_region(
        &ghost,
        appearance,
        slint::LogicalPosition::new(8.0, 8.0),
        content,
    );
    let original_content = gl_region(pressed, source_content_origin, scaled_content, scale);
    let ghost_content = gl_region(floating, ghost_content_origin, scaled_content, scale);
    assert_eq!(ghost_content.len(), original_content.len());
    // A passive drag ghost deliberately omits the real interactive checkbox.
    // Exclude only its measured native slot; icon, name, skin and outline keep
    // the same-position 2/255 comparison everywhere else.
    let (favorite_origin, favorite_size) = gl_scaled_tile_region(
        &source,
        appearance,
        slint::LogicalPosition::new(
            favorite.origin.x - source.origin.x,
            favorite.origin.y - source.origin.y,
        ),
        slint::LogicalSize::new(favorite.width, favorite.height),
    );
    let favorite_left = (favorite_origin.x * scale).ceil() as usize;
    let favorite_right = ((favorite_origin.x + favorite_size.width) * scale).floor() as usize;
    let favorite_top = (favorite_origin.y * scale).ceil() as usize;
    let favorite_bottom = ((favorite_origin.y + favorite_size.height) * scale).floor() as usize;
    let in_favorite = |index: usize, origin: slint::LogicalPosition, size: slint::LogicalSize| {
        let start_x = (origin.x * scale).ceil() as usize;
        let width = ((origin.x + size.width) * scale).floor() as usize - start_x;
        let x = start_x + index % width;
        let y = (origin.y * scale).ceil() as usize + index / width;
        x >= favorite_left && x < favorite_right && y >= favorite_top && y < favorite_bottom
    };
    assert!(
        ghost_content.iter().zip(&original_content).enumerate().any(
            |(index, (actual, expected))| {
                in_favorite(index, source_content_origin, scaled_content)
                    && (actual.r.abs_diff(expected.r) > 2
                        || actual.g.abs_diff(expected.g) > 2
                        || actual.b.abs_diff(expected.b) > 2)
            }
        ),
        "the actual hover checkbox decoration differs from the noninteractive ghost",
    );
    // Same-position comparison allows only 2/255 antialias/compositing rounding
    // (<0.8% per channel), never spatial matching or missing icon/name content.
    let (index, difference) = ghost_content
        .iter()
        .zip(&original_content)
        .enumerate()
        .filter(|(index, _)| !in_favorite(*index, source_content_origin, scaled_content))
        .map(|(index, (actual, expected))| {
            let difference = [
                actual.r.abs_diff(expected.r),
                actual.g.abs_diff(expected.g),
                actual.b.abs_diff(expected.b),
                actual.a.abs_diff(expected.a),
            ]
            .into_iter()
            .max()
            .unwrap();
            (index, difference)
        })
        .max_by_key(|(_, difference)| *difference)
        .unwrap();
    eprintln!(
        "native GL full pressed content: genuine={genuine} scale={scale} appearance={appearance} source={source:?} ghost={ghost:?} max_channel_delta={difference}"
    );
    assert!(
        difference <= 2,
        "shared ordinary/ghost icon AND two-line label pixel {index}: {:?} != {:?}",
        ghost_content[index],
        original_content[index],
    );
    // Selected skin's external 4px outline supplies a whole-tile pixel bbox.
    // A one-physical-pixel edge tolerance accounts only for raster coverage.
    let (outline_origin, outline_size) = gl_scaled_tile_region(
        &ghost,
        appearance,
        slint::LogicalPosition::new(-4.0, -4.0),
        slint::LogicalSize::new(ghost.width + 8.0, ghost.height + 8.0),
    );
    let (source_outline_origin, _) = gl_scaled_tile_region(
        &source,
        appearance,
        slint::LogicalPosition::new(-4.0, -4.0),
        slint::LogicalSize::new(source.width + 8.0, source.height + 8.0),
    );
    let source_skin = gl_region(pressed, source_outline_origin, outline_size, scale);
    let ghost_skin = gl_region(floating, outline_origin, outline_size, scale);
    assert_eq!(source_skin.len(), ghost_skin.len());
    for (index, (expected, actual)) in source_skin.iter().zip(&ghost_skin).enumerate() {
        if in_favorite(index, source_outline_origin, outline_size) {
            continue;
        }
        assert!(
            expected.r.abs_diff(actual.r) <= 2
                && expected.g.abs_diff(actual.g) <= 2
                && expected.b.abs_diff(actual.b) <= 2
                && expected.a.abs_diff(actual.a) <= 2,
            "same-position whole pressed skin/selected outline pixel {index}: {expected:?} != {actual:?}"
        );
    }
    assert_gl_scaled_tile_pixels(baseline, floating, &ghost, scale, appearance, genuine);
}

fn assert_gl_scaled_tile_pixels(
    baseline: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    floating: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    ghost: &TileBounds,
    scale: f32,
    appearance: f32,
    genuine: bool,
) {
    let (outline_origin, outline_size) = gl_scaled_tile_region(
        ghost,
        appearance,
        slint::LogicalPosition::new(-4.0, -4.0),
        slint::LogicalSize::new(ghost.width + 8.0, ghost.height + 8.0),
    );
    let mut changed = Vec::new();
    for y in ((ghost.origin.y - 5.0) * scale).floor() as usize
        ..((ghost.origin.y + ghost.height + 5.0) * scale).ceil() as usize
    {
        for x in ((ghost.origin.x - 5.0) * scale).floor() as usize
            ..((ghost.origin.x + ghost.width + 5.0) * scale).ceil() as usize
        {
            let index = y * floating.width() as usize + x;
            if floating.as_slice()[index] != baseline.as_slice()[index] {
                changed.push((x, y));
            }
        }
    }
    for (actual, expected) in [
        (
            changed.iter().map(|pixel| pixel.0).min().unwrap() as f32,
            outline_origin.x * scale,
        ),
        (
            changed.iter().map(|pixel| pixel.1).min().unwrap() as f32,
            outline_origin.y * scale,
        ),
        (
            changed.iter().map(|pixel| pixel.0).max().unwrap() as f32 + 1.0,
            (outline_origin.x + outline_size.width) * scale,
        ),
        (
            changed.iter().map(|pixel| pixel.1).max().unwrap() as f32 + 1.0,
            (outline_origin.y + outline_size.height) * scale,
        ),
    ] {
        assert!(
            (actual - expected).abs() <= 1.0,
            "physical WHOLE ghost bbox, including original selected outline"
        );
    }
    let raw_side = (ghost.height - 59.2)
        .max(0.0)
        .min((ghost.width - 16.0).max(0.0));
    let (icon_origin, icon_size) = gl_scaled_tile_region(
        ghost,
        appearance,
        slint::LogicalPosition::new((ghost.width - raw_side) / 2.0, 8.0),
        slint::LogicalSize::new(raw_side, raw_side),
    );
    let icon_pixels = gl_region(floating, icon_origin, icon_size, scale);
    assert!(
        icon_pixels.iter().any(|pixel| {
            if genuine {
                (pixel.r, pixel.g, pixel.b, pixel.a) == (255, 0, 255, 255)
            } else {
                pixel.r < 200 && pixel.g < 200 && pixel.b < 200 && pixel.a == 255
            }
        }),
        "actual genuine/fallback icon pixels, including the narrow residual row"
    );
    if genuine {
        let colored = floating
            .as_slice()
            .iter()
            .enumerate()
            .filter(|(_, pixel)| (pixel.r, pixel.g, pixel.b, pixel.a) == (255, 0, 255, 255))
            .map(|(index, _)| {
                (
                    index % floating.width() as usize,
                    index / floating.width() as usize,
                )
            })
            .collect::<Vec<_>>();
        let left = colored.iter().map(|pixel| pixel.0).min().unwrap() as f32;
        let right = colored.iter().map(|pixel| pixel.0).max().unwrap() as f32 + 1.0;
        let top = colored.iter().map(|pixel| pixel.1).min().unwrap() as f32;
        let bottom = colored.iter().map(|pixel| pixel.1).max().unwrap() as f32 + 1.0;
        for (actual, expected) in [
            (left, icon_origin.x * scale),
            (right, (icon_origin.x + icon_size.width) * scale),
            (top, icon_origin.y * scale),
            (bottom, (icon_origin.y + icon_size.height) * scale),
        ] {
            assert!(
                (actual - expected).abs() <= 1.0,
                "physical icon bbox {actual} vs {expected} at {scale}x"
            );
        }
    }
    for offset in [0.0, 17.6] {
        let (label_origin, label_size) = gl_scaled_tile_region(
            ghost,
            appearance,
            slint::LogicalPosition::new(8.0, ghost.height - 43.2 + offset),
            slint::LogicalSize::new(ghost.width - 16.0, 17.6),
        );
        let label = gl_region(floating, label_origin, label_size, scale);
        assert!(
            label
                .iter()
                .filter(|pixel| pixel.r < 180 && pixel.g < 180 && pixel.b < 180)
                .count()
                > 4,
            "both lines of the original label must actually paint"
        );
    }
}

fn verify_launcher_reorder_preview(launcher: Launcher, completed: Box<dyn FnOnce(Launcher)>) {
    launcher
        .window()
        .set_size(slint::LogicalSize::new(560.0, 420.0));
    await_launcher_gl_width(
        launcher,
        560.0,
        std::time::Instant::now(),
        Box::new(move |launcher| {
            verify_launcher_reorder_at_width(&launcher, 560.0);
            launcher
                .window()
                .set_size(slint::LogicalSize::new(840.0, 420.0));
            await_launcher_gl_width(
                launcher,
                840.0,
                std::time::Instant::now(),
                Box::new(move |launcher| {
                    verify_launcher_reorder_at_width(&launcher, 840.0);
                    completed(launcher);
                }),
            );
        }),
    );
}

// X11 resize acknowledgement is asynchronous. Yield only for native/cache
// size agreement; never poll snapshots or create another renderer/window.
fn await_launcher_gl_width(
    launcher: Launcher,
    width: f32,
    started: std::time::Instant,
    ready: Box<dyn FnOnce(Launcher)>,
) {
    let window = launcher.window();
    let (native_size, native_scale) = window
        .with_winit_window(|native| (native.inner_size(), native.scale_factor()))
        .expect("the owned production winit window exists");
    let scale = window.scale_factor();
    assert_eq!(
        native_scale as f32, scale,
        "native display and renderer scale must agree, not just a Slint override"
    );
    let expected = slint::PhysicalSize::new((width * scale) as u32, (420.0 * scale) as u32);
    if native_size.width == expected.width
        && native_size.height == expected.height
        && window.size() == expected
    {
        eprintln!(
            "owned GL native/cache settled: logical={width}x420 physical={expected:?} native_scale={native_scale} renderer_scale={scale}"
        );
        ready(launcher);
        return;
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "native GL resize not acknowledged: native={native_size:?}, cached={:?}, expected={expected:?}",
        window.size()
    );
    slint::Timer::single_shot(std::time::Duration::from_millis(10), move || {
        await_launcher_gl_width(launcher, width, started, ready);
    });
}

fn verify_launcher_reorder_at_width(launcher: &Launcher, width: f32) {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
    use slint::language::{DragAction, PointerEventKind};
    use slint::platform::{PointerEventButton, WindowEvent};
    let scale = launcher.window().scale_factor();
    assert!(
        scale == 1.0 || scale == 2.0,
        "owned GL run must have genuine 1x or 2x scale, got {scale}"
    );
    if let Ok(expected) = std::env::var("SLINT_SCALE_FACTOR") {
        assert_eq!(
            scale,
            expected.parse::<f32>().unwrap(),
            "requested scale must be the real window scale"
        );
    }
    for genuine in [true, false] {
        let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(16, 16);
        for pixel in pixels.make_mut_bytes().as_chunks_mut::<4>().0.iter_mut() {
            pixel.copy_from_slice(&[255, 0, 255, 255]);
        }
        let tile = LaunchTile {
            key: "native-app".into(),
            label: "Native\napplication".into(),
            icon: if genuine {
                slint::Image::from_rgba8(pixels)
            } else {
                slint::Image::default()
            },
            favorite: true,
        };
        let preview = LaunchTile {
            key: "preview-app".into(),
            label: "Preview app".into(),
            icon: slint::Image::default(),
            favorite: true,
        };
        let retained = tile.clone();
        let rows = move |reversed: bool| {
            let tiles = if reversed {
                vec![preview.clone(), tile.clone()]
            } else {
                vec![tile.clone(), preview.clone()]
            };
            slint::ModelRc::new(slint::VecModel::from(vec![LaunchRow {
                tiles: slint::ModelRc::new(slint::VecModel::from(tiles)),
            }]))
        };
        launcher.set_application_count(2);
        launcher.set_rows(rows(false));
        launcher.set_selected_key("native-app".into());
        launcher.invoke_focus_search();
        launcher.window().dispatch_event(WindowEvent::PointerExited);
        let original = launcher.window().take_snapshot().unwrap();
        assert_eq!(original.width(), (width * scale) as u32);
        assert_eq!(original.height(), (420.0 * scale) as u32);
        let source_weak =
            ElementHandle::find_by_accessible_label(launcher, "Launch Native\napplication")
                .next()
                .unwrap();
        assert_gl_launcher_tile_geometry(&source_weak, 1.0);
        let bounds = TileBounds {
            origin: source_weak.absolute_position(),
            width: source_weak.size().width,
            height: source_weak.size().height,
        };
        let favorite = source_weak
            .query_descendants()
            .find_all()
            .into_iter()
            .find(|element| element.accessible_role() == Some(AccessibleRole::Checkbox))
            .expect("measure the real native favorite input slot, not an arbitrary pixel mask");
        let favorite_bounds = TileBounds {
            origin: favorite.absolute_position(),
            width: favorite.size().width,
            height: favorite.size().height,
        };
        assert_eq!(
            (favorite_bounds.width, favorite_bounds.height),
            (16.0, 16.0)
        );
        assert_eq!(favorite_bounds.origin.y, bounds.origin.y);
        assert!(
            (favorite_bounds.origin.x + favorite_bounds.width - bounds.origin.x - bounds.width)
                .abs()
                < 0.001
        );
        // The accessible tile is inside a centered Transform. Retain its
        // untransformed enclosing slot, not that paint-mapped tile position.
        let source_slot = ElementQuery::from_root(launcher)
            .match_type_name("Rectangle")
            .find_all()
            .into_iter()
            .find(|element| {
                element.absolute_position() == bounds.origin
                    && element.size() == slint::LogicalSize::new(bounds.width, bounds.height)
                    && element.accessible_role() == Some(AccessibleRole::None)
                    && element
                        .query_descendants()
                        .find_all()
                        .into_iter()
                        .any(|child| {
                            child.accessible_role() == Some(AccessibleRole::Button)
                                && child
                                    .accessible_label()
                                    .is_some_and(|label| label == "Launch Native\napplication")
                        })
            })
            .expect("retain the untransformed source slot enclosing the accessible native tile");
        let extent = launcher.get_reorder_metrics().content_height;
        let source = Rc::new(RefCell::new(None::<Rc<GlDragSource>>));
        let source_scale = Rc::new(Cell::new(None::<f32>));
        let appearance_override = Rc::new(Cell::new(None::<f32>));
        let mut marker = slint::DataTransfer::default();
        marker.set_user_data(Rc::new(()));
        launcher.set_reorder_data(marker);
        launcher.set_reorder_enabled(true);
        let weak = launcher.as_weak();
        let state = source.clone();
        let captured_scale = source_scale.clone();
        let override_scale = appearance_override.clone();
        launcher.on_reorder_origin(move |key, event, bounds, press| {
            let launcher = weak.upgrade().unwrap();
            if event.kind == PointerEventKind::Down {
                state.borrow_mut().take();
                captured_scale.set(None);
                launcher.set_reorder_visual(LauncherDragVisual::default());
                if let Some(appearance) = override_scale.get() {
                    launcher.set_source_appearance_scale(appearance);
                }
                let appearance = launcher.get_source_appearance_scale();
                if event.button == PointerEventButton::Left
                    && key == retained.key
                    && appearance.is_finite()
                    && appearance > 0.0
                {
                    captured_scale.set(Some(appearance));
                    let token = Rc::new(GlDragSource {
                        tile: retained.clone(),
                        bounds,
                        press,
                    });
                    *state.borrow_mut() = Some(token.clone());
                    let mut data = slint::DataTransfer::default();
                    data.set_user_data(token);
                    launcher.set_reorder_data(data);
                }
            } else if event.kind == PointerEventKind::Up && !launcher.get_reorder_dragging() {
                state.borrow_mut().take();
                captured_scale.set(None);
            }
        });
        let weak = launcher.as_weak();
        let state = source.clone();
        let captured_scale = source_scale.clone();
        let previewed = Rc::new(Cell::new(false));
        let did_preview = previewed.clone();
        launcher.on_reorder_can_drop(move |event, origin| {
            let launcher = weak.upgrade().unwrap();
            if !publish_gl_drag_visual(&launcher, &state, &captured_scale, &event, origin) {
                return DragAction::None;
            }
            if !did_preview.replace(true) {
                launcher.set_rows(rows(true));
            }
            DragAction::Move
        });
        let weak = launcher.as_weak();
        let state = source.clone();
        let captured_scale = source_scale.clone();
        let outside = Rc::new(Cell::new(0));
        let count = outside.clone();
        launcher.on_reorder_window_hover(move |event, origin| {
            if publish_gl_drag_visual(
                &weak.upgrade().unwrap(),
                &state,
                &captured_scale,
                &event,
                origin,
            ) {
                count.set(count.get() + 1);
            }
        });
        let drops = Rc::new(Cell::new(0));
        let count = drops.clone();
        let weak = launcher.as_weak();
        let state = source.clone();
        let captured_scale = source_scale.clone();
        launcher.on_reorder_dropped(move |event, origin| {
            if !publish_gl_drag_visual(
                &weak.upgrade().unwrap(),
                &state,
                &captured_scale,
                &event,
                origin,
            ) {
                return DragAction::None;
            }
            count.set(count.get() + 1);
            DragAction::Move
        });
        let finished = Rc::new(RefCell::new(Vec::new()));
        let log = finished.clone();
        let weak = launcher.as_weak();
        let state = source.clone();
        let captured_scale = source_scale.clone();
        launcher.on_reorder_finished(move |action| {
            state.borrow_mut().take();
            captured_scale.set(None);
            weak.upgrade()
                .unwrap()
                .set_reorder_visual(LauncherDragVisual::default());
            log.borrow_mut().push(action);
        });
        let launches = Rc::new(Cell::new(0));
        let count = launches.clone();
        launcher.on_launch_requested(move |_| count.set(count.get() + 1));
        let favorites = Rc::new(Cell::new(0));
        let count = favorites.clone();
        launcher.on_favorite_toggle_requested(move |_, _| count.set(count.get() + 1));
        let press = slint::LogicalPosition::new(bounds.origin.x + 11.0, bounds.origin.y + 13.0);
        let target = slint::LogicalPosition::new(press.x + 280.0, press.y + 120.0);
        launcher
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position: press });
        launcher
            .window()
            .dispatch_event(WindowEvent::PointerPressed {
                position: press,
                button: PointerEventButton::Left,
            });
        assert!(
            !launcher.get_reorder_visual().visible,
            "Down only arms source data"
        );
        let appearance = source_scale.get().unwrap();
        assert_eq!(appearance, launcher.get_source_appearance_scale());
        assert!(appearance > 0.0 && appearance < 1.0);
        let pressed = launcher.window().take_snapshot().unwrap();
        assert!(source_slot.is_valid());
        assert_eq!(source_slot.absolute_position(), bounds.origin);
        assert_eq!(
            source_slot.size(),
            slint::LogicalSize::new(bounds.width, bounds.height),
            "pressed paint cannot shrink the logical source slot"
        );
        let captured_bounds = source.borrow().as_ref().unwrap().bounds.clone();
        let captured_press = source.borrow().as_ref().unwrap().press;
        assert_eq!(captured_bounds.origin, source_weak.absolute_position());
        assert_eq!(captured_bounds.width, bounds.width);
        assert_eq!(captured_bounds.height, bounds.height);
        assert_gl_input_position(
            slint::LogicalPosition::new(
                captured_press.x - captured_bounds.origin.x,
                captured_press.y - captured_bounds.origin.y,
            ),
            slint::LogicalPosition::new(press.x - bounds.origin.x, press.y - bounds.origin.y),
            &[captured_press, captured_bounds.origin, press, bounds.origin],
        );
        assert_eq!(launcher.get_reorder_metrics().content_height, extent);
        assert_ne!(
            gl_region(
                &pressed,
                bounds.origin,
                slint::LogicalSize::new(bounds.width, bounds.height),
                scale,
            ),
            gl_region(
                &original,
                bounds.origin,
                slint::LogicalSize::new(bounds.width, bounds.height),
                scale,
            ),
            "ordinary source really paints its pressed appearance before SDK activation"
        );
        launcher.set_source_appearance_scale(1.2);
        launcher.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(press.x + 12.0, press.y),
        });
        assert!(launcher.get_reorder_dragging());
        assert!(
            !launcher.get_reorder_visual().visible,
            "SDK activation seeds drag authority but has not delivered typed feedback"
        );
        launcher
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position: target });
        assert!(launcher.get_reorder_dragging());
        assert!(previewed.get());
        let floating = launcher.window().take_snapshot().unwrap();
        assert!(
            !source_weak.is_valid(),
            "native source delegate is really evicted by preview"
        );
        let visual = launcher.get_reorder_visual();
        assert_eq!(visual.source.key, "native-app");
        assert_eq!(visual.source.label, "Native\napplication");
        assert_eq!(visual.source_scale, appearance);
        assert_eq!(visual.bounds.width, bounds.width);
        assert_eq!(visual.bounds.height, bounds.height);
        assert_gl_input_position(
            visual.bounds.origin,
            slint::LogicalPosition::new(target.x - 11.0, target.y - 13.0),
            &[target, captured_press, captured_bounds.origin],
        );
        let ghost = ElementHandle::find_by_element_id(launcher, "Launcher::drag-visual")
            .next()
            .unwrap();
        let (paint_origin, _) = gl_scaled_tile_region(
            &visual.bounds,
            appearance,
            slint::LogicalPosition::new(0.0, 0.0),
            slint::LogicalSize::new(bounds.width, bounds.height),
        );
        assert_gl_input_position(
            ghost.absolute_position(),
            paint_origin,
            &[visual.bounds.origin, target],
        );
        assert_eq!(
            ghost.size(),
            slint::LogicalSize::new(bounds.width, bounds.height)
        );
        for (actual, expected) in [
            (ghost.absolute_position().x * scale, paint_origin.x * scale),
            (ghost.absolute_position().y * scale, paint_origin.y * scale),
            (ghost.size().width * scale, bounds.width * scale),
            (ghost.size().height * scale, bounds.height * scale),
        ] {
            assert!(
                (actual - expected).abs()
                    <= f32::EPSILON
                        * target
                            .x
                            .abs()
                            .max(target.y.abs())
                            .max(bounds.width)
                            .max(bounds.height)
                        * scale,
                "physical center-scaled paint origin with raw whole-ghost size/noncenter hotspot"
            );
        }
        assert_gl_launcher_tile_geometry(&ghost, appearance);
        export_frame(
            &format!("launcher-reorder-source-{width}-{genuine}-{scale}x"),
            &original,
        );
        export_frame(
            &format!("launcher-reorder-pressed-{width}-{genuine}-{scale}x"),
            &pressed,
        );
        export_frame(
            &format!("launcher-reorder-floating-{width}-{genuine}-{scale}x"),
            &floating,
        );
        assert_gl_whole_tile_pixels(
            &pressed,
            &original,
            &floating,
            GlTilePaintSource {
                bounds: bounds.clone(),
                favorite: favorite_bounds,
            },
            visual.clone(),
            scale,
            genuine,
        );
        let slot_origin =
            slint::LogicalPosition::new(bounds.origin.x + bounds.width + 8.0, bounds.origin.y);
        let placeholder = ElementHandle::find_by_element_type_name(launcher, "Rectangle")
            .find(|element| {
                element.absolute_position() == slot_origin
                    && element.size() == slint::LogicalSize::new(bounds.width, bounds.height)
            })
            .expect(
                "the explicit source slot stays visible when its LauncherTile subtree is hidden",
            );
        assert_eq!(
            placeholder.size(),
            slint::LogicalSize::new(bounds.width, bounds.height)
        );
        assert_eq!(
            placeholder.absolute_position(),
            slint::LogicalPosition::new(bounds.origin.x + bounds.width + 8.0, bounds.origin.y)
        );
        assert_eq!(placeholder.accessible_role(), Some(AccessibleRole::None));
        placeholder.invoke_accessible_default_action();
        assert!(
            !ElementHandle::find_by_accessible_label(launcher, "Launch Native\napplication")
                .any(|element| element.accessible_role() == Some(AccessibleRole::Button)),
            "neither hidden native source nor passive ghost exposes a visible launch Button"
        );
        assert!(
            !ElementHandle::find_by_accessible_label(
                launcher,
                "Remove from favorites: Native\napplication"
            )
            .any(|element| element.accessible_role() == Some(AccessibleRole::Checkbox)),
            "hidden source and passive ghost expose no visible favorite Checkbox"
        );
        assert_eq!(launcher.get_reorder_metrics().content_height, extent);
        let hidden = gl_region(
            &floating,
            placeholder.absolute_position(),
            placeholder.size(),
            scale,
        );
        assert!(
            hidden
                .iter()
                .all(|pixel| (pixel.r, pixel.g, pixel.b, pixel.a) == (242, 242, 242, 255)),
            "source slot is background only, not a duplicate icon/name/corner"
        );
        assert_eq!(ghost.accessible_role(), Some(AccessibleRole::None));
        for child in ghost
            .query_descendants()
            .find_all()
            .into_iter()
            .chain(std::iter::once(ghost))
        {
            assert_ne!(child.accessible_role(), Some(AccessibleRole::Button));
            assert_ne!(child.accessible_role(), Some(AccessibleRole::Checkbox));
            child.invoke_accessible_default_action();
        }
        export_frame(
            &format!("launcher-reorder-preview-{width}-{genuine}-{scale}x"),
            &floating,
        );
        // Final move/release stay adjacent: no snapshot/getter/query.
        launcher
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position: target });
        launcher
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position: target,
                button: PointerEventButton::Left,
            });
        assert_eq!(drops.get(), 1);
        assert_eq!(*finished.borrow(), vec![DragAction::Move]);
        assert!(!launcher.get_reorder_visual().visible);
        assert!(source_scale.get().is_none());
        let restored_source =
            ElementHandle::find_by_accessible_label(launcher, "Launch Native\napplication")
                .find(|element| element.accessible_role() == Some(AccessibleRole::Button))
                .expect("drop restores the original visible native source Button");
        assert_eq!(restored_source.accessible_enabled(), Some(true));
        assert_eq!(restored_source.absolute_position(), slot_origin);
        assert_eq!(restored_source.size(), placeholder.size());
        let trail_origin =
            slint::LogicalPosition::new(visual.bounds.origin.x - 4.0, visual.bounds.origin.y - 4.0);
        let trail_size = slint::LogicalSize::new(bounds.width + 8.0, bounds.height + 8.0);
        let restored = launcher.window().take_snapshot().unwrap();
        assert_eq!(
            gl_region(&restored, trail_origin, trail_size, scale),
            gl_region(&original, trail_origin, trail_size, scale),
            "drop leaves no whole-tile trail",
        );
        let source_tile =
            ElementHandle::find_by_accessible_label(launcher, "Launch Native\napplication")
                .next()
                .unwrap();
        let press = slint::LogicalPosition::new(
            source_tile.absolute_position().x + 11.0,
            source_tile.absolute_position().y + 13.0,
        );
        let grid_top = launcher.get_reorder_metrics().viewport.origin.y;
        // Reach actual fallback SVG ink above the boundary, not only its
        // transparent inset at the source header's new fractional raster phase.
        let outside_pointer = slint::LogicalPosition::new(320.0, grid_top - 20.0);
        // Alternate positive metadata uses the same public Down callback route;
        // its transport and centered paint must not collapse to the default skin.
        appearance_override.set(Some(0.8));
        launcher
            .window()
            .dispatch_event(WindowEvent::PointerPressed {
                position: press,
                button: PointerEventButton::Left,
            });
        assert_eq!(source_scale.get(), Some(0.8));
        launcher.set_source_appearance_scale(1.3);
        launcher.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(press.x + 12.0, press.y),
        });
        assert!(!launcher.get_reorder_visual().visible);
        launcher.window().dispatch_event(WindowEvent::PointerMoved {
            position: outside_pointer,
        });
        let outside_frame = launcher.window().take_snapshot().unwrap();
        let outside_visual = launcher.get_reorder_visual();
        assert_eq!(outside_visual.source_scale, 0.8);
        assert_eq!(outside_visual.bounds.width, bounds.width);
        assert_eq!(outside_visual.bounds.height, bounds.height);
        assert_eq!(launcher.get_reorder_metrics().content_height, extent);
        assert_gl_scaled_tile_pixels(
            &restored,
            &outside_frame,
            &outside_visual.bounds,
            scale,
            outside_visual.source_scale,
            genuine,
        );
        assert_gl_input_position(
            outside_visual.bounds.origin,
            slint::LogicalPosition::new(outside_pointer.x - 11.0, outside_pointer.y - 13.0),
            &[
                outside_pointer,
                source.borrow().as_ref().unwrap().press,
                source.borrow().as_ref().unwrap().bounds.origin,
            ],
        );
        assert!(outside.get() > 0, "rejecting root receives header hover");
        let side = (outside_visual.bounds.height - 59.2)
            .max(0.0)
            .min((outside_visual.bounds.width - 16.0).max(0.0));
        let (above_origin, icon_size) = gl_scaled_tile_region(
            &outside_visual.bounds,
            outside_visual.source_scale,
            slint::LogicalPosition::new((outside_visual.bounds.width - side) / 2.0, 8.0),
            slint::LogicalSize::new(side, side),
        );
        let above_size = slint::LogicalSize::new(
            icon_size.width,
            icon_size.height.min(grid_top - above_origin.y),
        );
        let unclipped_icon = gl_region(&outside_frame, above_origin, above_size, scale);
        let clean_header = gl_region(&restored, above_origin, above_size, scale);
        assert!(
            unclipped_icon
                .iter()
                .zip(&clean_header)
                .any(|(pixel, clean)| {
                    pixel != clean
                        && if genuine {
                            (pixel.r, pixel.g, pixel.b) == (255, 0, 255)
                        } else {
                            pixel.r < 200 && pixel.g < 200 && pixel.b < 200
                        }
                }),
            "actual icon pixels ABOVE the ListView boundary, not just a below-grid fragment or existing header text"
        );
        export_frame(
            &format!("launcher-reorder-outside-{width}-{genuine}-{scale}x"),
            &outside_frame,
        );
        launcher.window().dispatch_event(WindowEvent::PointerExited);
        assert!(!launcher.get_reorder_visual().visible);
        assert!(source_scale.get().is_none());
        assert_eq!(
            source_tile.accessible_role(),
            Some(AccessibleRole::Button),
            "cancel restores ordinary source AX on GL"
        );
        assert_eq!(source_tile.accessible_enabled(), Some(true));
        assert!(
            ElementHandle::find_by_accessible_label(launcher, "Launch Native\napplication")
                .any(|element| element.accessible_role() == Some(AccessibleRole::Button)),
            "cancel restores a publicly visible native source Button"
        );
        assert_eq!(*finished.borrow(), vec![DragAction::Move, DragAction::None]);
        assert_eq!(drops.get(), 1, "outside observer never accepts a save/drop");
        let canceled = launcher.window().take_snapshot().unwrap();
        assert_eq!(
            gl_region(&canceled, trail_origin, trail_size, scale),
            gl_region(&original, trail_origin, trail_size, scale),
            "cancel leaves no previous ghost trail",
        );
        let outside_trail = slint::LogicalPosition::new(
            outside_visual.bounds.origin.x - 4.0,
            outside_visual.bounds.origin.y - 4.0,
        );
        assert_eq!(
            gl_region(&canceled, outside_trail, trail_size, scale),
            gl_region(&restored, outside_trail, trail_size, scale),
            "cancel removes the current out-of-grid ghost as well as older positions",
        );
        assert_eq!(launches.get(), 0);
        assert_eq!(favorites.get(), 0);
        launcher.set_reorder_enabled(false);
    }
}

fn verify_menu_press_scale(menu: &ContextMenuSurface) {
    menu.invoke_focus_menu();
    let actions = Rc::new(Cell::new(0));
    let count = actions.clone();
    menu.on_action_requested(move |action| {
        assert_eq!(action, DockMenuAction::Settings);
        count.set(count.get() + 1);
    });
    let tile = i_slint_backend_testing::ElementHandle::find_by_accessible_label(menu, "Settings")
        .next()
        .unwrap();
    for source in [PressSource::Pointer, PressSource::Space] {
        verify_native_press_scale(menu.window(), &tile, 0.98, 1.0, source, &actions);
    }
    assert_eq!(
        actions.get(),
        2,
        "pointer and Space each request one action"
    );
}

fn verify_menu_application_image(menu: &ContextMenuSurface) {
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(32, 32);
    for pixel in pixels.make_mut_bytes().as_chunks_mut::<4>().0.iter_mut() {
        pixel.copy_from_slice(&[255, 0, 255, 255]);
    }
    menu.set_kind(DockMenuKind::Pinned);
    menu.set_target_icon(slint::Image::from_rgba8(pixels));
    menu.invoke_focus_menu();
    let row = i_slint_backend_testing::ElementHandle::find_by_accessible_label(menu, "Open")
        .next()
        .unwrap();
    let point = row.absolute_position();
    let size = row.size();
    let x = point.x + 8.0;
    let y = point.y + (size.height - 16.0) / 2.0;
    let scale = menu.window().scale_factor();
    let frame = menu.window().take_snapshot().unwrap();
    let sample = |x: f32, y: f32| {
        let pixel =
            frame.as_slice()[(y * scale) as usize * frame.width() as usize + (x * scale) as usize];
        (pixel.r, pixel.g, pixel.b, pixel.a)
    };
    let background = sample(point.x - 5.0, point.y + size.height / 2.0);
    assert_eq!(
        sample(x + 8.0, y + 8.0),
        (255, 0, 255, 255),
        "a genuine application image stays untinted on the production renderer"
    );
    assert_eq!(
        sample(x + 0.5, y + 0.5),
        background,
        "the production renderer clips the reference 4px application-image corner"
    );
    assert_eq!(
        sample(x + 20.0, y + 8.0),
        background,
        "the native image slot preserves the 8px label gap"
    );
    menu.set_target_icon(slint::Image::default());
    menu.set_kind(DockMenuKind::Bar);
    menu.invoke_focus_menu();
}

#[derive(Clone, Copy, Debug)]
enum PressSource {
    Pointer,
    Space,
}

fn verify_native_press_scale(
    window: &slint::Window,
    tile: &i_slint_backend_testing::ElementHandle,
    pressed_scale: f32,
    translation_y: f32,
    source: PressSource,
    actions: &Cell<usize>,
) {
    use slint::platform::{Key, PointerEventButton, WindowEvent};
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));
    let position = tile.absolute_position();
    let size = tile.size();
    let scale = window.scale_factor();
    let center = slint::LogicalPosition::new(
        position.x + size.width / 2.0,
        position.y + size.height / 2.0,
    );
    let sample = |frame: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, x: f32| {
        frame.as_slice()
            [(center.y * scale) as usize * frame.width() as usize + (x * scale) as usize]
    };
    window.dispatch_event(WindowEvent::PointerMoved { position: center });
    let hover = window.take_snapshot().unwrap();
    let body = sample(&hover, position.x - 5.0);
    let edge = sample(&hover, position.x + 0.5);
    assert_ne!(edge, body, "the original tile edge must actually render");
    let before = actions.get();
    match source {
        PressSource::Pointer => window.dispatch_event(WindowEvent::PointerPressed {
            position: center,
            button: PointerEventButton::Left,
        }),
        PressSource::Space => {
            window.dispatch_event(WindowEvent::KeyPressed {
                text: Key::Space.into(),
            });
            window.dispatch_event(WindowEvent::KeyPressRepeated {
                text: Key::Space.into(),
            });
        }
    }
    assert_eq!(
        actions.get(),
        before,
        "{source:?} press/repeat must not activate before release",
    );
    let pressed = window.take_snapshot().unwrap();
    assert_eq!(
        tile.size(),
        size,
        "press transforms do not resize layout bounds"
    );
    let transformed = tile.absolute_position();
    assert!(
        (transformed.x - position.x - size.width * (1.0 - pressed_scale) / 2.0).abs() < 0.01,
        "native scale {pressed_scale}: original {position:?}, actual {transformed:?}, size {size:?}",
    );
    assert!(
        (transformed.y - position.y - translation_y - size.height * (1.0 - pressed_scale) / 2.0)
            .abs()
            < 0.01,
        "native translation {translation_y}: original {position:?}, actual {transformed:?}, size {size:?}",
    );
    assert_eq!(
        sample(&pressed, position.x + 0.5),
        body,
        "native press scaling must uncover the original tile edge",
    );
    assert_ne!(
        sample(&pressed, position.x + 3.0),
        body,
        "press scaling must shrink, not hide, the tile",
    );
    match source {
        PressSource::Pointer => window.dispatch_event(WindowEvent::PointerReleased {
            position: center,
            button: PointerEventButton::Left,
        }),
        PressSource::Space => window.dispatch_event(WindowEvent::KeyReleased {
            text: Key::Space.into(),
        }),
    }
    assert_eq!(
        actions.get(),
        before + 1,
        "one {source:?} release acts once"
    );
    let restored = window.take_snapshot().unwrap();
    match source {
        PressSource::Pointer => assert_eq!(sample(&restored, position.x + 0.5), edge),
        // Space changes focus modality; the launcher's focus skin differs
        // from hover, but its original edge must be fully visible again.
        PressSource::Space => assert_ne!(sample(&restored, position.x + 0.5), body),
    }
    assert_eq!(tile.absolute_position(), position);
    assert_eq!(
        tile.size(),
        size,
        "the surrounding layout geometry stays fixed"
    );
    window.dispatch_event(WindowEvent::PointerExited);
}

fn export_frame(name: &str, frame: &slint::SharedPixelBuffer<slint::Rgba8Pixel>) {
    let Some(dir) = std::env::var_os("TESSERA_TEST_SCREENSHOTS").filter(|dir| !dir.is_empty())
    else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A synthetic neutral-gray backdrop makes black shadow alpha visible.
    // This is a renderer snapshot, never a screenshot of a Windows compositor.
    let mut ppm = format!("P6\n{} {}\n255\n", frame.width(), frame.height()).into_bytes();
    for pixel in frame.as_slice() {
        for color in [pixel.r, pixel.g, pixel.b] {
            ppm.push(
                ((u16::from(color) * u16::from(pixel.a) + 128 * (255 - u16::from(pixel.a))) / 255)
                    as u8,
            );
        }
    }
    std::fs::write(dir.join(format!("{name}.ppm")), ppm).unwrap();
}
