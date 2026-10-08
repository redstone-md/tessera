// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Production-renderer pixels on an owned virtual display, in an isolated test process.

use std::cell::Cell;
use std::rc::Rc;

use slint::ComponentHandle;
use slint::language::ColorScheme;
use slint::winit_030::{SlintEvent, WinitWindowAccessor, winit};
use winit::platform::x11::EventLoopBuilderExtX11;

use crate::generated::{
    ContextMenuSurface, DockMenuAction, DockMenuKind, LaunchTile, Launcher, QuickSettings,
    TooltipSurface,
};
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
    launcher.set_tiles(slint::ModelRc::new(slint::VecModel::from(vec![
        LaunchTile {
            key: "native-app".into(),
            label: "Native app".into(),
            icon: slint::Image::default(),
            favorite: false,
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

    let completed = Rc::new(Cell::new(false));
    let result = Rc::clone(&completed);
    slint::spawn_local(async move {
        launcher.window().winit_window().await.unwrap();
        tooltip.window().winit_window().await.unwrap();
        menu.window().winit_window().await.unwrap();
        quick.window().winit_window().await.unwrap();
        verify_frame("launcher", &launcher);
        verify_frame("tooltip", &tooltip);
        verify_frame("context-menu", &menu);
        verify_frame("quick-settings", &quick);
        // Stock control colors have their own 150ms transitions. Let the
        // genuine loop settle them; cold frame pixels are not theme proof.
        launcher.apply_presentation_theme(PresentationTheme::uniform(ColorScheme::Dark));
        menu.apply_presentation_theme(PresentationTheme::uniform(ColorScheme::Dark));
        slint::Timer::single_shot(std::time::Duration::from_millis(200), move || {
            verify_launcher_controls(ColorScheme::Dark, &launcher);
            verify_menu_press_scale(&menu);
            verify_menu_application_image(&menu);
            launcher.apply_presentation_theme(PresentationTheme::uniform(ColorScheme::Light));
            menu.apply_presentation_theme(PresentationTheme::uniform(ColorScheme::Light));
            slint::Timer::single_shot(std::time::Duration::from_millis(200), move || {
                verify_launcher_controls(ColorScheme::Light, &launcher);
                verify_menu_press_scale(&menu);
                verify_menu_application_image(&menu);
                launcher.hide().unwrap();
                tooltip.hide().unwrap();
                menu.hide().unwrap();
                quick.hide().unwrap();
                result.set(true);
                slint::quit_event_loop().unwrap();
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
        assert_eq!(
            (body.r, body.g, body.b, body.a),
            (background, background, background, 255),
            "{name} {theme} body"
        );
        assert_eq!(sample(0.0, 0.0).a, 0, "{name} outside frame");
        let shadow = sample(logical_width / 2.0, logical_height - 8.0);
        assert!(
            shadow.a > 0 && shadow.a < 255,
            "{name} {theme} {scale}x reference shadow: {shadow:?}"
        );
        assert_eq!((shadow.r, shadow.g, shadow.b), (0, 0, 0));
        export_frame(&format!("gl-{name}-{theme}-{scale}x"), &frame);
    }
}

fn verify_launcher_controls(scheme: ColorScheme, launcher: &Launcher) {
    let theme = match scheme {
        ColorScheme::Dark => "dark",
        ColorScheme::Light => "light",
        _ => panic!("The settlement scenario requires an explicit color scheme"),
    };
    let frame = launcher.window().take_snapshot().unwrap();
    let scale = launcher.window().scale_factor();
    for label in [
        "Open settings and recovery",
        "Refresh the desktop",
        "Exit Tessera",
    ] {
        let button =
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(launcher, label)
                .next()
                .unwrap();
        let position = button.absolute_position();
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
    }
    export_frame(&format!("gl-launcher-{theme}-settled-{scale}x"), &frame);
    verify_launcher_press_scale(launcher);
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
    for pixel in pixels.make_mut_bytes().chunks_exact_mut(4) {
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
