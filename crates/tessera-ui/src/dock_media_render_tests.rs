// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Production generated Dock, genuine native input/AX, and owned software pixels.
//! These recordings exercise the tile's typed boundary, not an OS media provider.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, ModelRc, PhysicalSize, Rgb8Pixel, VecModel};

use crate::dock_media::DockMediaController;
use crate::generated::{Dock, DockApp, DockMediaView, MediaAction};
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::{DesktopHost, PanelPreferences, PanelSnapshot, SystemAction};
use tessera_system::media::{
    MediaAction as HostAction, MediaCapabilities, MediaCommand, MediaCommandCompletion, MediaError,
    MediaErrorKind, MediaEvent, MediaHost, MediaPlayback, MediaReadCompletion, MediaSession,
    MediaSessionKey, MediaSnapshot,
};

const PREVIOUS: &str = "Previous track";
const NEXT: &str = "Next track";
const INFO: &str = "Media information";
const RETRY: &str = "Retry media";
const TITLE: &str = "実際の曲 · Καλημέρα 🎵";
const AUTHOR: &str = "Recorded artist · Björk";

#[derive(Debug, PartialEq)]
enum Request {
    Command(MediaAction),
    Retry,
    Context,
    Other,
}

struct Fixture {
    window: Rc<MinimalSoftwareWindow>,
    dock: Dock,
    requests: Rc<RefCell<Vec<Request>>>,
    tooltips: Rc<RefCell<Vec<String>>>,
    dismissals: Rc<RefCell<Vec<bool>>>,
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
        dock.apply_presentation_theme(PresentationTheme::uniform(
            slint::language::ColorScheme::Light,
        ));
        let requests = Rc::new(RefCell::new(Vec::new()));
        let recorded = requests.clone();
        dock.on_media_action_requested(move |action| {
            recorded.borrow_mut().push(Request::Command(action));
        });
        let recorded = requests.clone();
        dock.on_media_retry_requested(move || recorded.borrow_mut().push(Request::Retry));
        let recorded = requests.clone();
        dock.on_media_context_requested(move |_| recorded.borrow_mut().push(Request::Context));
        let recorded = requests.clone();
        dock.on_launch_requested(move |_| recorded.borrow_mut().push(Request::Other));
        let recorded = requests.clone();
        dock.on_window_command_requested(move |_, _| recorded.borrow_mut().push(Request::Other));
        let recorded = requests.clone();
        dock.on_reserved_action_requested(move |_| recorded.borrow_mut().push(Request::Other));
        let tooltips = Rc::new(RefCell::new(Vec::new()));
        let recorded = tooltips.clone();
        dock.on_tooltip_requested(move |text, _| recorded.borrow_mut().push(text.to_string()));
        let dismissals = Rc::new(RefCell::new(Vec::new()));
        let recorded = dismissals.clone();
        dock.on_tooltip_dismissed(move |delayed, _| recorded.borrow_mut().push(delayed));
        dock.set_media_view(current());
        dock.show().unwrap();
        window
            .window()
            .dispatch_event(WindowEvent::WindowActiveChanged(true));
        let fixture = Self {
            window,
            dock,
            requests,
            tooltips,
            dismissals,
        };
        fixture.render(360, 72, 1.0);
        fixture
    }

    fn render(&self, width: u32, height: u32, scale: f32) -> Vec<Rgb8Pixel> {
        self.window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
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

    fn nodes(&self, label: &str) -> Vec<ElementHandle> {
        ElementHandle::find_by_accessible_label(&self.dock, label)
            .filter(|element| element.accessible_role() != Some(AccessibleRole::None))
            .collect()
    }

    fn element(&self, label: &str) -> ElementHandle {
        let mut nodes = self.nodes(label);
        assert_eq!(nodes.len(), 1, "unique semantic node {label}");
        nodes.remove(0)
    }

    fn center(element: &ElementHandle) -> LogicalPosition {
        let origin = element.absolute_position();
        let size = element.size();
        LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
    }

    fn pointer_at(&self, position: LogicalPosition, button: PointerEventButton) {
        for event in [
            WindowEvent::PointerMoved { position },
            WindowEvent::PointerPressed { position, button },
            WindowEvent::PointerReleased { position, button },
        ] {
            self.window.window().dispatch_event(event);
        }
    }

    fn pointer(&self, label: &str, button: PointerEventButton) {
        self.pointer_at(Self::center(&self.element(label)), button);
    }

    // An unobstructed point of the information button, not its control overlay.
    fn information_pointer(&self, label: &str, button: PointerEventButton) {
        let origin = self.element(label).absolute_position();
        self.pointer_at(LogicalPosition::new(origin.x + 8.0, origin.y + 8.0), button);
    }

    fn press(&self, key: Key) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    }

    fn release(&self, key: Key) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }

    fn repeat(&self, key: Key) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressRepeated { text: key.into() });
    }

    fn key(&self, key: Key) {
        self.press(key);
        self.release(key);
    }

    fn take(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }
}

fn current() -> DockMediaView {
    DockMediaView {
        enabled: true,
        current_present: true,
        session_identity: "recorded-enable-1:session-1".into(),
        status: "Current session".into(),
        title: TITLE.into(),
        author: AUTHOR.into(),
        playback: "Paused".into(),
        previous_enabled: true,
        toggle_enabled: true,
        next_enabled: true,
        ..DockMediaView::default()
    }
}

fn solid_image(rgb: [u8; 3]) -> slint::Image {
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(8, 8);
    for pixel in pixels.make_mut_bytes().as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    slint::Image::from_rgba8(pixels)
}

fn exact_pixels(pixels: &[Rgb8Pixel], rgb: [u8; 3]) -> usize {
    pixels
        .iter()
        .filter(|pixel| [pixel.r, pixel.g, pixel.b] == rgb)
        .count()
}

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.02,
        "expected {expected}, actual {actual}"
    );
}

#[test]
fn dock_media_pointer_tab_return_space_ax_dispatch_exact_typed_actions_and_context() {
    let fixture = Fixture::new();
    for (label, action) in [
        (PREVIOUS, MediaAction::Previous),
        ("Play", MediaAction::Toggle),
        (NEXT, MediaAction::Next),
    ] {
        let node = fixture.element(label);
        assert_eq!(node.accessible_role(), Some(AccessibleRole::Button));
        assert_eq!(node.accessible_enabled(), Some(true));
        fixture.pointer(label, PointerEventButton::Left);
        assert_eq!(fixture.take(), vec![Request::Command(action)]);
        // Backend dispatches AccessibilityAction::Default to the real item.
        node.invoke_accessible_default_action();
        assert_eq!(fixture.take(), vec![Request::Command(action)]);
    }
    fixture.pointer(PREVIOUS, PointerEventButton::Left);
    fixture.take();
    fixture.key(Key::Tab);
    fixture.press(Key::Return);
    fixture.repeat(Key::Return);
    fixture.release(Key::Return);
    assert_eq!(fixture.take(), vec![Request::Command(MediaAction::Toggle)]);
    fixture.key(Key::Tab);
    fixture.press(Key::Space);
    fixture.repeat(Key::Space);
    assert!(fixture.take().is_empty());
    fixture.release(Key::Space);
    assert_eq!(fixture.take(), vec![Request::Command(MediaAction::Next)]);
    fixture.key(Key::Menu);
    assert_eq!(fixture.take(), vec![Request::Context]);
    fixture.pointer(NEXT, PointerEventButton::Right);
    assert_eq!(fixture.take(), vec![Request::Context]);
    fixture.information_pointer(INFO, PointerEventButton::Right);
    assert_eq!(fixture.take(), vec![Request::Context]);
    fixture
        .window
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(0.0, 0.0),
        });
    fixture
        .window
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: Fixture::center(&fixture.element("Play")),
        });
    let tooltip = fixture.tooltips.borrow().last().cloned().unwrap();
    assert!(tooltip.contains(TITLE) && tooltip.contains(AUTHOR) && tooltip.contains("Paused"));
    fixture.pointer("Play", PointerEventButton::Left);
    assert!(fixture.dismissals.borrow().contains(&false));
}

#[test]
fn dock_media_capabilities_busy_stale_cancel_held_keys_and_reject_pointer_ax() {
    let fixture = Fixture::new();
    for state in 0..5 {
        fixture.dock.set_media_view(current());
        fixture.render(360, 72, 1.0);
        let armed_label = match state {
            0 => PREVIOUS,
            2 => NEXT,
            _ => "Play",
        };
        fixture.pointer(armed_label, PointerEventButton::Left);
        fixture.take();
        fixture.press(Key::Space);
        let mut view = current();
        match state {
            0 => view.previous_enabled = false,
            1 => view.toggle_enabled = false,
            2 => view.next_enabled = false,
            3 => view.busy = true,
            _ => view.stale = true,
        }
        fixture.dock.set_media_view(view);
        slint::platform::update_timers_and_animations();
        fixture.render(360, 72, 1.0);
        let labels: &[&str] = match state {
            0 => &[PREVIOUS],
            1 => &["Play"],
            2 => &[NEXT],
            _ => &[PREVIOUS, "Play", NEXT],
        };
        for label in labels {
            let node = fixture.element(label);
            assert_eq!(node.accessible_enabled(), Some(false));
            fixture.pointer(label, PointerEventButton::Left);
            node.invoke_accessible_default_action();
        }
        fixture.release(Key::Space);
        // Disabling the armed button cancels the held gesture, not just clicks.
        assert!(fixture.take().is_empty());
    }
    fixture.dock.set_media_view(current());
    fixture.render(360, 72, 1.0);
    fixture.pointer("Play", PointerEventButton::Left);
    fixture.take();
    fixture.press(Key::Space);
    fixture.dock.set_media_view(DockMediaView {
        busy: true,
        ..current()
    });
    slint::platform::update_timers_and_animations();
    fixture.render(360, 72, 1.0);
    fixture.dock.set_media_view(current());
    fixture.render(360, 72, 1.0);
    fixture.repeat(Key::Space);
    fixture.release(Key::Space);
    assert!(
        fixture.take().is_empty(),
        "cancelled Space cannot revive after pending clears"
    );
    // Reacquire the enabled semantic control after the cancelled gesture.
    fixture.pointer("Play", PointerEventButton::Left);
    assert_eq!(fixture.take(), vec![Request::Command(MediaAction::Toggle)]);
    fixture.key(Key::Space);
    assert_eq!(fixture.take(), vec![Request::Command(MediaAction::Toggle)]);
}

#[test]
fn dock_media_held_native_input_cannot_cross_reenabled_or_replaced_authority() {
    let fixture = Fixture::new();
    // Same-key fixture availability changes commit their disabled phase.
    // Production admission changes additionally rotate the opaque key.
    for state in 0..5 {
        fixture.dock.set_media_view(current());
        fixture.render(360, 72, 1.0);
        fixture.pointer("Play", PointerEventButton::Left);
        fixture.take();
        fixture.press(Key::Space);
        let mut unavailable = current();
        match state {
            0 => unavailable.toggle_enabled = false,
            1 => unavailable.busy = true,
            2 => unavailable.stale = true,
            3 => unavailable.enabled = false,
            _ => unavailable.current_present = false,
        }
        fixture.dock.set_media_view(unavailable.clone());
        slint::platform::update_timers_and_animations();
        fixture.render(360, 72, 1.0);
        fixture.dock.set_media_view(current());
        fixture.repeat(Key::Space);
        fixture.release(Key::Space);
        assert!(
            fixture.take().is_empty(),
            "held Space cancelled by state {state}"
        );

        let node = fixture.element("Play");
        let position = Fixture::center(&node);
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
        fixture.dock.set_media_view(unavailable);
        slint::platform::update_timers_and_animations();
        fixture.render(360, 72, 1.0);
        fixture.dock.set_media_view(current());
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
        assert!(
            fixture.take().is_empty(),
            "held pointer cancelled by state {state}"
        );
    }

    for identity in ["recorded-enable-1:session-2", "recorded-enable-2:session-1"] {
        fixture.dock.set_media_view(current());
        fixture.render(360, 72, 1.0);
        fixture.pointer("Play", PointerEventButton::Left);
        fixture.take();
        fixture.press(Key::Space);
        let replacement = DockMediaView {
            session_identity: identity.into(),
            ..current()
        };
        fixture.dock.set_media_view(replacement.clone());
        fixture.repeat(Key::Space);
        fixture.release(Key::Space);
        assert!(
            fixture.take().is_empty(),
            "held Space cannot target {identity}"
        );
        fixture.element("Play").invoke_accessible_default_action();
        assert_eq!(fixture.take(), vec![Request::Command(MediaAction::Toggle)]);

        fixture.dock.set_media_view(current());
        fixture.render(360, 72, 1.0);
        let position = Fixture::center(&fixture.element("Play"));
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
        fixture.dock.set_media_view(replacement.clone());
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
        assert!(
            fixture.take().is_empty(),
            "held pointer cannot target {identity}"
        );

        fixture.dock.set_media_view(current());
        fixture.render(360, 72, 1.0);
        fixture.pointer("Play", PointerEventButton::Left);
        fixture.take();
        fixture.press(Key::Return);
        assert_eq!(fixture.take(), vec![Request::Command(MediaAction::Toggle)]);
        fixture.dock.set_media_view(replacement);
        fixture.repeat(Key::Return);
        fixture.release(Key::Return);
        assert!(
            fixture.take().is_empty(),
            "held Return cannot target {identity}"
        );
    }
}

#[test]
fn dock_media_truthful_empty_unavailable_loading_and_retry_use_real_inputs() {
    let fixture = Fixture::new();
    for status in ["Not playing", "Media unavailable", "Media read failed"] {
        fixture.dock.set_media_view(DockMediaView {
            enabled: true,
            status: status.into(),
            read_notice: if status == "Media read failed" {
                "Recorded read failure".into()
            } else {
                "".into()
            },
            // Even inconsistent old flags cannot manufacture current controls/art.
            has_artwork: true,
            artwork: solid_image([255, 0, 255]),
            previous_enabled: true,
            toggle_enabled: true,
            next_enabled: true,
            ..DockMediaView::default()
        });
        let pixels = fixture.render(360, 72, 1.0);
        assert_eq!(exact_pixels(&pixels, [255, 0, 255]), 0);
        assert_eq!(
            fixture.element(status).accessible_role(),
            Some(AccessibleRole::Text)
        );
        for label in [PREVIOUS, "Play", "Pause", NEXT] {
            assert!(fixture.nodes(label).is_empty());
        }
        fixture.information_pointer(RETRY, PointerEventButton::Left);
        assert_eq!(fixture.take(), vec![Request::Retry]);
        fixture.element(RETRY).invoke_accessible_default_action();
        assert_eq!(fixture.take(), vec![Request::Retry]);
        fixture.press(Key::Return);
        fixture.repeat(Key::Return);
        fixture.release(Key::Return);
        assert_eq!(fixture.take(), vec![Request::Retry]);
        fixture.key(Key::Space);
        assert_eq!(fixture.take(), vec![Request::Retry]);
        fixture.information_pointer(RETRY, PointerEventButton::Right);
        assert_eq!(fixture.take(), vec![Request::Context]);
    }
    fixture.dock.set_media_view(DockMediaView {
        enabled: true,
        busy: true,
        status: "Loading media".into(),
        ..DockMediaView::default()
    });
    fixture.render(360, 72, 1.0);
    assert_eq!(fixture.element(RETRY).accessible_enabled(), Some(false));
    fixture.information_pointer(RETRY, PointerEventButton::Left);
    fixture.element(RETRY).invoke_accessible_default_action();
    fixture.key(Key::Return);
    fixture.key(Key::Space);
    assert!(fixture.take().is_empty());
}

#[test]
fn dock_media_observed_text_real_art_overlay_and_playback_are_not_invented() {
    let fixture = Fixture::new();
    let mut view = current();
    view.artwork = solid_image([255, 0, 255]);
    view.has_artwork = true;
    view.dark_foreground = true;
    view.app_icon = solid_image([0, 255, 0]);
    fixture.dock.set_media_view(view.clone());
    let first = fixture.render(360, 72, 1.0);
    assert!(
        exact_pixels(&first, [255, 0, 255]) > 500,
        "supplied thumbnail pixels rendered"
    );
    assert_eq!(
        exact_pixels(&first, [0, 255, 0]),
        0,
        "overlay requires genuine has-app-icon"
    );
    assert_eq!(
        fixture.element(TITLE).accessible_role(),
        Some(AccessibleRole::Text)
    );
    assert_eq!(
        fixture.element(AUTHOR).accessible_role(),
        Some(AccessibleRole::Text)
    );
    let title_bounds = fixture.element(TITLE);
    let origin = title_bounds.absolute_position();
    let size = title_bounds.size();
    let mut metadata_only = view.clone();
    metadata_only.title = "Different observed title".into();
    fixture.dock.set_media_view(metadata_only);
    let changed_text = fixture.render(360, 72, 1.0);
    let text_changed = (origin.y as usize..(origin.y + size.height) as usize).any(|y| {
        (origin.x as usize..(origin.x + size.width) as usize)
            .any(|x| first[y * 360 + x] != changed_text[y * 360 + x])
    });
    assert!(
        text_changed,
        "actual supplied title changes software glyph pixels"
    );
    fixture.dock.set_media_view(view.clone());
    fixture.render(360, 72, 1.0);
    fixture.pointer("Play", PointerEventButton::Left);
    assert_eq!(fixture.take(), vec![Request::Command(MediaAction::Toggle)]);
    assert_eq!(fixture.dock.get_media_view().title, TITLE);
    assert!(
        !fixture.dock.get_media_view().playing,
        "input is not an optimistic observation"
    );
    view.playing = true;
    view.playback = "Playing".into();
    view.title = "New observed title · 後".into();
    view.author = "New observed artist".into();
    view.artwork = solid_image([0, 128, 255]);
    view.has_app_icon = true;
    fixture.dock.set_media_view(view.clone());
    let observed = fixture.render(360, 72, 1.0);
    assert!(exact_pixels(&observed, [0, 128, 255]) > 500);
    assert!(exact_pixels(&observed, [0, 255, 0]) > 80);
    let module_origin = fixture.element("Media module").absolute_position();
    for (index, pixel) in observed.iter().enumerate() {
        if [pixel.r, pixel.g, pixel.b] == [0, 255, 0] {
            let x = (index % 360) as f32;
            let y = (index / 360) as f32;
            assert!(x >= module_origin.x + 24.0 && x < module_origin.x + 36.0);
            assert!(y >= module_origin.y + 24.0 && y < module_origin.y + 36.0);
        }
    }
    assert_eq!(exact_pixels(&observed, [255, 0, 255]), 0);
    assert!(fixture.nodes("Play").is_empty());
    assert_eq!(fixture.element("Pause").accessible_enabled(), Some(true));
    assert_eq!(
        fixture.element("New observed title · 後").accessible_role(),
        Some(AccessibleRole::Text)
    );
    assert_ne!(first, observed);
    view.has_artwork = false;
    view.has_app_icon = false;
    view.artwork_notice = "Artwork unavailable".into();
    fixture.dock.set_media_view(view);
    let missing = fixture.render(360, 72, 1.0);
    assert_eq!(exact_pixels(&missing, [0, 128, 255]), 0);
    assert_eq!(exact_pixels(&missing, [0, 255, 0]), 0);
    fixture
        .window
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: Fixture::center(&fixture.element("Pause")),
        });
    assert!(
        fixture
            .tooltips
            .borrow()
            .last()
            .unwrap()
            .contains("Artwork unavailable")
    );
    assert!(fixture.take().is_empty());
}

#[test]
fn dock_media_source_geometry_four_edges_compact_dpi_and_constrained_composition() {
    let fixture = Fixture::new();
    for edge in 0..4 {
        fixture.dock.set_edge(edge);
        for compact in [false, true] {
            fixture.dock.set_compact(compact);
            let factor: f32 = if compact { 0.8 } else { 1.0 };
            for scale in [1.0, 2.0] {
                let (along, cross) = (360.0 * factor * scale, 72.0 * factor * scale);
                let (width, height) = if edge < 2 {
                    (along, cross)
                } else {
                    (cross, along)
                };
                fixture.render(width.ceil() as u32, height.ceil() as u32, scale);
                let info = fixture.element("Media module");
                assert_eq!(info.accessible_role(), Some(AccessibleRole::Groupbox));
                close(
                    info.size().width,
                    if edge < 2 {
                        136.0 * factor
                    } else {
                        40.0 * factor
                    },
                );
                close(
                    info.size().height,
                    if edge < 2 {
                        40.0 * factor
                    } else {
                        136.0 * factor
                    },
                );
                let origin = info.absolute_position();
                let information = fixture.element(INFO);
                close(information.absolute_position().x, origin.x);
                close(information.absolute_position().y, origin.y);
                for label in [PREVIOUS, "Play", NEXT] {
                    let control = fixture.element(label);
                    let position = control.absolute_position();
                    let size = control.size();
                    assert!(position.x >= origin.x && position.y >= origin.y);
                    assert!(position.x + size.width <= origin.x + info.size().width + 0.02);
                    assert!(position.y + size.height <= origin.y + info.size().height + 0.02);
                    assert_eq!(control.accessible_enabled(), Some(true));
                }
                assert_eq!(fixture.nodes(TITLE).is_empty(), edge >= 2);
                fixture.pointer("Play", PointerEventButton::Left);
                assert_eq!(fixture.take(), vec![Request::Command(MediaAction::Toggle)]);
                fixture
                    .window
                    .window()
                    .dispatch_event(WindowEvent::PointerMoved {
                        position: Fixture::center(&fixture.element(NEXT)),
                    });
                assert!(fixture.tooltips.borrow().last().unwrap().contains(TITLE));
            }
        }
    }
    fixture.dock.set_pinned_apps(ModelRc::new(VecModel::from(
        (0..32)
            .map(|index| DockApp {
                key: format!("recorded/{index}").into(),
                label: format!("Fixture {index}").into(),
                icon: slint::Image::default(),
                pinned: true,
            })
            .collect::<Vec<_>>(),
    )));
    for edge in 0..4 {
        fixture.dock.set_edge(edge);
        fixture.dock.set_compact(false);
        let (width, height) = if edge < 2 { (216, 72) } else { (72, 216) };
        fixture.render(width, height, 1.0);
        let start_origin = fixture
            .element("Open applications and settings")
            .absolute_position();
        let desktop_origin = fixture.element("Show desktop").absolute_position();
        let viewport = ElementHandle::find_by_element_id(&fixture.dock, "ScrollView::flickable")
            .next()
            .unwrap();
        let position = Fixture::center(&viewport);
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerScrolled {
                position,
                delta_x: if edge < 2 { -100_000.0 } else { 0.0 },
                delta_y: if edge < 2 { 0.0 } else { -100_000.0 },
            });
        fixture.render(width, height, 1.0);
        assert_eq!(
            fixture
                .element("Open applications and settings")
                .absolute_position(),
            start_origin
        );
        assert_eq!(
            fixture.element("Show desktop").absolute_position(),
            desktop_origin
        );
        assert!(
            fixture.take().is_empty(),
            "overflow scrolling is never transport"
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

type MediaChangedCallback = Arc<dyn Fn(MediaEvent) + Send + Sync>;

// Production controller recording: inline completion deliberately leaves the
// command busy until its mailbox is processed. No native provider is acquired.
struct RecordingMedia {
    observation: Mutex<Result<MediaSnapshot, MediaError>>,
    commands: Mutex<Vec<MediaCommand>>,
    reads: AtomicUsize,
    changed: Mutex<Option<MediaChangedCallback>>,
}

impl MediaHost for RecordingMedia {
    fn read(&self, completion: MediaReadCompletion) -> Result<(), MediaError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let observation = self.observation.lock().clone();
        completion(observation);
        Ok(())
    }

    fn execute(
        &self,
        command: MediaCommand,
        completion: MediaCommandCompletion,
    ) -> Result<(), MediaError> {
        self.commands.lock().push(command);
        completion(Ok(()));
        Ok(())
    }

    fn subscribe(
        &self,
        changed: MediaChangedCallback,
    ) -> Result<Option<Box<dyn Send>>, MediaError> {
        *self.changed.lock() = Some(changed.clone());
        changed(MediaEvent::WatchReady);
        Ok(Some(Box::new(())))
    }
}

struct RecordingDesktop {
    media: Arc<RecordingMedia>,
    acquisitions: AtomicUsize,
}

impl DesktopHost for RecordingDesktop {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("media cannot observe desktop");
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("media cannot activate windows");
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("source id is not launch authority");
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("media cannot dispatch system actions");
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("parent owns persistence");
    }
    fn subscribe(&self, _: Arc<dyn Fn() + Send + Sync>) -> Result<Option<Box<dyn Send>>, String> {
        panic!("media cannot watch desktop");
    }
    fn media_host(&self) -> Result<Option<Arc<dyn MediaHost>>, MediaError> {
        self.acquisitions.fetch_add(1, Ordering::Relaxed);
        Ok(Some(self.media.clone()))
    }
}

#[test]
fn dock_media_native_inputs_record_exact_production_controller_session_commands() {
    let fixture = Fixture::new();
    let key = MediaSessionKey::issue().unwrap();
    let session = MediaSession {
        key,
        source_app_id: "recorded.player".into(),
        title: TITLE.into(),
        author: AUTHOR.into(),
        playback: MediaPlayback::Paused,
        capabilities: MediaCapabilities {
            previous: true,
            toggle: true,
            next: true,
        },
        artwork: None,
        artwork_notice: None,
        timeline: Err(MediaError::new(
            MediaErrorKind::Unavailable,
            "Timeline not observed",
        )),
    };
    let media = Arc::new(RecordingMedia {
        observation: Mutex::new(Ok(MediaSnapshot {
            current: Some(session.clone()),
        })),
        commands: Mutex::new(Vec::new()),
        reads: AtomicUsize::new(0),
        changed: Mutex::new(None),
    });
    let desktop = Arc::new(RecordingDesktop {
        media: media.clone(),
        acquisitions: AtomicUsize::new(0),
    });
    let controller = DockMediaController::new(desktop.clone(), &fixture.dock);
    let weak = Rc::downgrade(&controller);
    fixture.dock.on_media_action_requested(move |action| {
        if let Some(controller) = weak.upgrade() {
            controller.request(action);
        }
    });
    let weak = Rc::downgrade(&controller);
    fixture.dock.on_media_retry_requested(move || {
        if let Some(controller) = weak.upgrade() {
            controller.retry();
        }
    });
    assert_eq!(desktop.acquisitions.load(Ordering::Relaxed), 0);
    assert_eq!(media.reads.load(Ordering::Relaxed), 0);
    controller.set_enabled(true);
    controller.process_events();
    fixture.render(360, 72, 1.0);
    assert_eq!(desktop.acquisitions.load(Ordering::Relaxed), 1);
    assert_eq!(
        fixture.element(TITLE).accessible_role(),
        Some(AccessibleRole::Text)
    );

    fixture.pointer(PREVIOUS, PointerEventButton::Left);
    assert_eq!(
        *media.commands.lock(),
        vec![MediaCommand {
            expected_session: key,
            action: HostAction::Previous,
        }]
    );
    assert!(fixture.dock.get_media_view().busy);
    fixture.render(360, 72, 1.0);
    fixture.pointer("Play", PointerEventButton::Left);
    fixture.element(NEXT).invoke_accessible_default_action();
    fixture.repeat(Key::Return);
    assert_eq!(
        media.commands.lock().len(),
        1,
        "pending input is single-flight"
    );
    // One drain accepts the command and starts readback; the next accepts
    // that inline authoritative observation.
    controller.process_events();
    controller.process_events();
    fixture.render(360, 72, 1.0);
    assert!(
        !fixture.dock.get_media_view().playing,
        "request acceptance is not an observation"
    );

    fixture.information_pointer(INFO, PointerEventButton::Left);
    fixture.key(Key::Tab); // Previous track
    fixture.key(Key::Tab); // Play
    fixture.key(Key::Return);
    controller.process_events();
    controller.process_events();
    fixture.render(360, 72, 1.0);
    fixture.element(NEXT).invoke_accessible_default_action();
    controller.process_events();
    controller.process_events();
    fixture.render(360, 72, 1.0);
    let expected = vec![
        MediaCommand {
            expected_session: key,
            action: HostAction::Previous,
        },
        MediaCommand {
            expected_session: key,
            action: HostAction::Toggle,
        },
        MediaCommand {
            expected_session: key,
            action: HostAction::Next,
        },
    ];
    assert_eq!(*media.commands.lock(), expected);

    *media.observation.lock() = Ok(MediaSnapshot {
        current: Some(MediaSession {
            capabilities: MediaCapabilities::default(),
            ..session.clone()
        }),
    });
    controller.retry();
    controller.process_events();
    fixture.render(360, 72, 1.0);
    for label in [PREVIOUS, "Play", NEXT] {
        let reads = media.reads.load(Ordering::Relaxed);
        assert_eq!(fixture.element(label).accessible_enabled(), Some(false));
        fixture.pointer(label, PointerEventButton::Left);
        fixture.element(label).invoke_accessible_default_action();
        assert_eq!(
            media.reads.load(Ordering::Relaxed),
            reads,
            "disabled input cannot retry"
        );
        assert!(!fixture.dock.get_media_view().busy);
    }
    assert_eq!(*media.commands.lock(), expected);
    *media.observation.lock() = Ok(MediaSnapshot {
        current: Some(session.clone()),
    });
    controller.retry();
    controller.process_events();
    fixture.render(360, 72, 1.0);
    assert_eq!(fixture.element("Play").accessible_enabled(), Some(true));

    // Stale is the actual controller's failed observation, not a forged view.
    *media.observation.lock() = Err(MediaError::new(
        MediaErrorKind::Unavailable,
        "Recorded read failed",
    ));
    controller.retry();
    controller.process_events();
    fixture.render(360, 72, 1.0);
    assert!(fixture.dock.get_media_view().stale);
    let stale_reads = media.reads.load(Ordering::Relaxed);
    for label in [PREVIOUS, "Play", NEXT] {
        fixture.pointer(label, PointerEventButton::Left);
        fixture.element(label).invoke_accessible_default_action();
    }
    assert_eq!(*media.commands.lock(), expected);
    assert_eq!(
        media.reads.load(Ordering::Relaxed),
        stale_reads,
        "stale transport input cannot fall through into Retry"
    );
    assert!(
        !fixture.dock.get_media_view().busy,
        "no artificial pending Retry"
    );

    *media.observation.lock() = Ok(MediaSnapshot { current: None });
    let reads = media.reads.load(Ordering::Relaxed);
    fixture.element(RETRY).invoke_accessible_default_action();
    controller.process_events();
    fixture.render(360, 72, 1.0);
    assert_eq!(
        media.reads.load(Ordering::Relaxed),
        reads + 1,
        "Retry only reads"
    );
    assert!(!fixture.dock.get_media_view().current_present);
    assert_eq!(fixture.dock.get_media_view().status, "Not playing");
    for label in [PREVIOUS, "Play", NEXT] {
        assert!(fixture.nodes(label).is_empty());
    }
    fixture.information_pointer(RETRY, PointerEventButton::Left);
    controller.process_events();
    fixture.render(360, 72, 1.0);
    fixture.key(Key::Space);
    controller.process_events();
    assert_eq!(
        *media.commands.lock(),
        expected,
        "no-current keys only request reads"
    );
    assert_eq!(desktop.acquisitions.load(Ordering::Relaxed), 1);

    *media.observation.lock() = Ok(MediaSnapshot {
        current: Some(session.clone()),
    });
    controller.retry();
    controller.process_events();
    fixture.render(360, 72, 1.0);
    fixture.pointer("Play", PointerEventButton::Left);
    let mut commands = expected.clone();
    commands.push(MediaCommand {
        expected_session: key,
        action: HostAction::Toggle,
    });
    assert_eq!(*media.commands.lock(), commands);
    controller.process_events();
    controller.process_events();
    fixture.render(360, 72, 1.0);
    fixture.pointer("Play", PointerEventButton::Right); // focus without transport
    fixture.press(Key::Space);
    let replacement_key = MediaSessionKey::issue().unwrap();
    *media.observation.lock() = Ok(MediaSnapshot {
        current: Some(MediaSession {
            key: replacement_key,
            ..session
        }),
    });
    let changed = media.changed.lock().clone().unwrap();
    changed(MediaEvent::Changed);
    controller.process_events();
    controller.process_events();
    fixture.repeat(Key::Space);
    fixture.release(Key::Space);
    assert_eq!(
        *media.commands.lock(),
        commands,
        "held Space never commands the replacement"
    );
    fixture.element("Play").invoke_accessible_default_action();
    commands.push(MediaCommand {
        expected_session: replacement_key,
        action: HostAction::Toggle,
    });
    assert_eq!(
        *media.commands.lock(),
        commands,
        "fresh AX uses exact replacement identity"
    );
    controller.process_events();
    controller.process_events();
    fixture.render(360, 72, 1.0);

    fixture.pointer("Play", PointerEventButton::Right);
    fixture.press(Key::Space);
    let identity_before_read = fixture.dock.get_media_view().session_identity;
    let reads_before_refresh = media.reads.load(Ordering::Relaxed);
    controller.retry();
    controller.process_events();
    assert_ne!(
        fixture.dock.get_media_view().session_identity,
        identity_before_read
    );
    fixture.repeat(Key::Space);
    fixture.release(Key::Space);
    assert_eq!(
        *media.commands.lock(),
        commands,
        "held Space cannot cross read admission"
    );
    assert_eq!(
        media.reads.load(Ordering::Relaxed),
        reads_before_refresh + 1
    );

    let identity_before_disable = fixture.dock.get_media_view().session_identity;
    let position = Fixture::center(&fixture.element("Play"));
    fixture
        .window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    fixture
        .window
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
    controller.set_enabled(false);
    controller.set_enabled(true);
    controller.process_events();
    assert_ne!(
        fixture.dock.get_media_view().session_identity,
        identity_before_disable
    );
    fixture
        .window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    assert_eq!(
        *media.commands.lock(),
        commands,
        "held pointer cannot cross module generation"
    );
    fixture.element("Play").invoke_accessible_default_action();
    commands.push(MediaCommand {
        expected_session: replacement_key,
        action: HostAction::Toggle,
    });
    assert_eq!(
        *media.commands.lock(),
        commands,
        "fresh reenabled AX keeps provider key"
    );
    assert_eq!(desktop.acquisitions.load(Ordering::Relaxed), 2);
    controller.close();
}
