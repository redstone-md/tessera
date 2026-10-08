// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Production-renderer pixels on an owned virtual display, in an isolated test process.

use std::cell::Cell;
use std::rc::Rc;

use slint::ComponentHandle;
use slint::language::ColorScheme;
use slint::winit_030::{SlintEvent, WinitWindowAccessor, winit};
use winit::platform::x11::EventLoopBuilderExtX11;

use crate::generated::{ContextMenuSurface, DockMenuKind, Launcher, TooltipSurface};
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
    let menu = ContextMenuSurface::new().unwrap();
    menu.set_kind(DockMenuKind::Bar);
    menu.set_selected_index(-1);
    menu.window().set_size(slint::LogicalSize::new(
        menu.get_menu_width(),
        menu.get_menu_height(),
    ));
    menu.show().unwrap();

    let completed = Rc::new(Cell::new(false));
    let result = Rc::clone(&completed);
    slint::spawn_local(async move {
        launcher.window().winit_window().await.unwrap();
        tooltip.window().winit_window().await.unwrap();
        menu.window().winit_window().await.unwrap();
        verify_frame("launcher", &launcher);
        verify_frame("tooltip", &tooltip);
        verify_frame("context-menu", &menu);
        // Stock control colors have their own 150ms transitions. Let the
        // genuine loop settle them; cold frame pixels are not theme proof.
        launcher.apply_presentation_theme(PresentationTheme::uniform(ColorScheme::Dark));
        slint::Timer::single_shot(std::time::Duration::from_millis(200), move || {
            verify_launcher_controls(ColorScheme::Dark, &launcher);
            launcher.apply_presentation_theme(PresentationTheme::uniform(ColorScheme::Light));
            slint::Timer::single_shot(std::time::Duration::from_millis(200), move || {
                verify_launcher_controls(ColorScheme::Light, &launcher);
                launcher.hide().unwrap();
                tooltip.hide().unwrap();
                menu.hide().unwrap();
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
