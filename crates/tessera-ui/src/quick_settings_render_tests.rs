// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Renderer-backed Quick Settings tests. All traffic is recorded locally;
//! the fixture never constructs an audio host or writes a hardware setting.

use std::cell::RefCell;
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, Rgb8Pixel};

use crate::generated::{AudioRoute, QuickSettings};
use crate::theme::{PresentationTheme, ThemedComponent};

#[derive(Debug, PartialEq)]
enum Request {
    Volume(AudioRoute, f32),
    Released(AudioRoute, f32),
    Mute(AudioRoute, bool),
    Settings,
    Refresh,
    Dismiss,
}

struct Fixture {
    window: Rc<MinimalSoftwareWindow>,
    popup: QuickSettings,
    requests: Rc<RefCell<Vec<Request>>>,
}

impl Fixture {
    fn new(configure: impl FnOnce(&QuickSettings)) -> Self {
        struct TestPlatform(Rc<MinimalSoftwareWindow>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
        let popup = QuickSettings::new().unwrap();
        popup.apply_presentation_theme(PresentationTheme::uniform(
            slint::language::ColorScheme::Light,
        ));
        let requests = Rc::new(RefCell::new(Vec::new()));
        let recorded = requests.clone();
        popup.on_volume_requested(move |route, value| {
            recorded.borrow_mut().push(Request::Volume(route, value));
        });
        let recorded = requests.clone();
        popup.on_volume_released(move |route, value| {
            recorded.borrow_mut().push(Request::Released(route, value));
        });
        let recorded = requests.clone();
        popup.on_mute_requested(move |route, desired| {
            recorded.borrow_mut().push(Request::Mute(route, desired));
        });
        let recorded = requests.clone();
        popup.on_app_settings_requested(move || recorded.borrow_mut().push(Request::Settings));
        let recorded = requests.clone();
        popup.on_refresh_requested(move || recorded.borrow_mut().push(Request::Refresh));
        let recorded = requests.clone();
        popup.on_dismiss_requested(move || recorded.borrow_mut().push(Request::Dismiss));
        configure(&popup);
        popup.show().unwrap();
        let fixture = Self {
            window,
            popup,
            requests,
        };
        fixture.render_fit();
        fixture
    }

    fn ready() -> Self {
        Self::new(|popup| {
            popup.set_output_ready(true);
            popup.set_input_ready(true);
            popup.set_output_percent(25.0);
            popup.set_input_percent(60.0);
            popup.set_watch_live(true);
        })
    }

    fn element(&self, label: &str) -> ElementHandle {
        ElementHandle::find_by_accessible_label(&self.popup, label)
            .next()
            .unwrap_or_else(|| panic!("missing accessible element: {label}"))
    }

    fn has_element(&self, label: &str) -> bool {
        ElementHandle::find_by_accessible_label(&self.popup, label)
            .next()
            .is_some()
    }

    fn render_fit(&self) -> Vec<Rgb8Pixel> {
        self.render(
            self.popup.get_preferred_popup_width().ceil() as u32,
            self.popup.get_preferred_popup_height().ceil() as u32,
        )
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

    fn point(element: &ElementHandle, fraction: f32) -> LogicalPosition {
        let origin = element.absolute_position();
        let size = element.size();
        assert!(
            size.width > 0.0 && size.height > 0.0,
            "real input requires a noncollapsed accessible control: {size:?}"
        );
        LogicalPosition::new(
            origin.x + size.width * fraction,
            origin.y + size.height / 2.0,
        )
    }

    fn press(&self, position: LogicalPosition) {
        let size = self.window.window().size();
        assert!(
            position.x >= 0.0
                && position.x < size.width as f32
                && position.y >= 0.0
                && position.y < size.height as f32,
            "pointer target {position:?} must be inside the actual native viewport {size:?}"
        );
        self.window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        self.window
            .window()
            .dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
    }

    fn release(&self, position: LogicalPosition) {
        self.window
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
    }

    fn click(&self, element: &ElementHandle) {
        let position = Self::point(element, 0.5);
        self.press(position);
        self.release(position);
    }

    fn key_press(&self, key: Key) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    }

    fn key_release(&self, key: Key) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }

    fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }
}

#[test]
fn quick_settings_projection_is_silent_and_capabilities_remain_independent() {
    let fixture = Fixture::ready();
    fixture.popup.set_output_percent(42.6);
    fixture.popup.set_input_percent(17.2);
    fixture.popup.set_output_muted(true);
    fixture.popup.set_input_muted(false);
    fixture.popup.set_status("Audio observation failed".into());
    fixture
        .popup
        .set_output_status("Output change failed".into());
    fixture.popup.set_input_ready(false);
    fixture
        .popup
        .set_input_status("No default input endpoint".into());
    fixture.render_fit();

    let output = fixture.element("Output volume");
    assert_eq!(output.accessible_role(), Some(AccessibleRole::Slider));
    assert_eq!(output.accessible_value_minimum(), Some(0.0));
    assert_eq!(output.accessible_value_maximum(), Some(100.0));
    assert_eq!(output.accessible_value_step(), Some(1.0));
    assert_eq!(
        output.accessible_description().unwrap().as_str(),
        "Muted; 43 percent"
    );
    assert_eq!(
        fixture.element("Unmute output").accessible_checked(),
        Some(true)
    );
    assert!(!fixture.has_element("Input volume"));
    assert!(fixture.has_element("Audio observation failed"));
    assert!(fixture.has_element("Output: Output change failed"));
    assert!(fixture.has_element("Input: No default input endpoint"));
    assert!(fixture.has_element("Retry audio"));
    assert!(
        fixture.take_requests().is_empty(),
        "projection must never write audio"
    );
}

#[test]
fn quick_settings_native_slider_pointer_and_keyboard_route_changed_and_released() {
    let fixture = Fixture::ready();
    let output = fixture.element("Output volume");
    assert!(
        output.size().width > 100.0,
        "the native Slider must receive the row's remaining width, not collapse to its zero preferred width"
    );
    let start = Fixture::point(&output, 0.7);
    let end = Fixture::point(&output, 0.85);
    fixture.press(start);
    fixture
        .window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: end });
    let during_drag = fixture.take_requests();
    assert!(!during_drag.is_empty());
    assert!(
        during_drag.iter().all(|request| matches!(
            request, Request::Volume(AudioRoute::Output, value) if (0.0..=100.0).contains(value)
        )),
        "pointer changes route only output, with no premature release"
    );
    fixture.release(end);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Released(
            AudioRoute::Output,
            fixture.popup.get_output_percent()
        ),]
    );

    // Obtain real native focus by pointer input, then project a value. The
    // next actual key event, not a manufactured callback or forced draw,
    // observes that projection and advances by Slider.step (one percent).
    fixture.click(&fixture.element("Input volume"));
    fixture.take_requests();
    fixture.popup.set_input_percent(41.0);
    assert!(fixture.take_requests().is_empty());
    fixture.key_press(Key::RightArrow);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Volume(AudioRoute::Input, 42.0)]
    );
    assert_eq!(fixture.popup.get_input_percent(), 42.0);
    fixture.key_release(Key::RightArrow);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Released(AudioRoute::Input, 42.0)]
    );

    for (key, expected) in [(Key::Home, 0.0), (Key::End, 100.0)] {
        fixture.key_press(key);
        assert_eq!(
            fixture.take_requests(),
            vec![Request::Volume(AudioRoute::Input, expected)]
        );
        fixture.key_release(key);
        assert_eq!(
            fixture.take_requests(),
            vec![Request::Released(AudioRoute::Input, expected)]
        );
    }
    fixture.key_press(Key::Escape);
    fixture.key_release(Key::Escape);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Dismiss],
        "Escape bubbles from the native slider"
    );
}

#[test]
fn quick_settings_loading_and_missing_endpoints_reject_commands_without_fake_rows() {
    let fixture = Fixture::new(|popup| popup.set_loading(true));
    assert!(fixture.has_element("Loading audio devices…"));
    assert!(!fixture.has_element("Volume"));
    assert!(!fixture.has_element("Output volume"));
    assert!(!fixture.has_element("Input volume"));
    assert_eq!(
        fixture.element("Refresh audio").accessible_enabled(),
        Some(false)
    );
    fixture
        .element("Refresh audio")
        .invoke_accessible_default_action();
    assert!(fixture.take_requests().is_empty());

    fixture.popup.set_loading(false);
    fixture.render_fit();
    assert!(fixture.has_element("No audio endpoints available"));
    let empty_height = fixture.popup.get_preferred_popup_height();
    fixture.popup.set_output_ready(true);
    fixture.popup.set_output_percent(33.0);
    assert!(
        fixture.popup.get_preferred_popup_height() > empty_height,
        "state-driven metrics must include the real output row before deferred conditional layout"
    );
    fixture.render_fit();
    assert!(fixture.has_element("Input: Unavailable"));
    assert!(!fixture.has_element("Input volume"));
    let slider = fixture.element("Output volume");
    fixture.click(&slider);
    fixture.take_requests();
    let before_loading = fixture.popup.get_output_percent();
    fixture.popup.set_loading(true);
    assert_eq!(slider.accessible_enabled(), Some(false));
    fixture.click(&slider);
    fixture.key_press(Key::RightArrow);
    fixture.key_release(Key::RightArrow);
    fixture
        .element("Mute output")
        .invoke_accessible_default_action();
    assert!(
        fixture.take_requests().is_empty(),
        "loading rejects native and accessible commands"
    );
    assert_eq!(
        fixture.popup.get_output_percent(),
        before_loading,
        "blocked input must preserve the current projection"
    );
}

#[test]
fn quick_settings_mute_is_absolute_and_footer_actions_route_once() {
    let fixture = Fixture::ready();
    fixture.popup.invoke_focus_content();
    fixture.key_press(Key::Escape);
    fixture.key_release(Key::Escape);
    assert_eq!(fixture.take_requests(), vec![Request::Dismiss]);
    fixture.click(&fixture.element("Mute output"));
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Mute(AudioRoute::Output, true)]
    );
    // The host owns confirmation: repeating before observation requests the
    // same absolute state, never a blind OS toggle or an optimistic mutation.
    fixture.click(&fixture.element("Mute output"));
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Mute(AudioRoute::Output, true)]
    );
    assert!(!fixture.popup.get_output_muted());
    fixture.popup.set_output_muted(true);
    fixture
        .element("Unmute output")
        .invoke_accessible_default_action();
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Mute(AudioRoute::Output, false)]
    );
    fixture.popup.set_input_muted(true);
    fixture.click(&fixture.element("Unmute input"));
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Mute(AudioRoute::Input, false)]
    );

    assert!(
        !fixture.has_element("Refresh audio"),
        "healthy live watch needs no refresh button"
    );
    fixture.click(&fixture.element("App Settings"));
    assert_eq!(fixture.take_requests(), vec![Request::Settings]);
    fixture.key_press(Key::Escape);
    fixture.key_release(Key::Escape);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Dismiss],
        "Escape bubbles from a native tile"
    );
    fixture.popup.set_watch_live(false);
    fixture.render_fit();
    fixture.click(&fixture.element("Refresh audio"));
    assert_eq!(fixture.take_requests(), vec![Request::Refresh]);
    fixture.popup.set_watch_live(true);
    fixture
        .popup
        .set_status("Notifications disconnected".into());
    fixture.render_fit();
    fixture
        .element("Retry audio")
        .invoke_accessible_default_action();
    assert_eq!(fixture.take_requests(), vec![Request::Refresh]);
    fixture.popup.set_loading(true);
    fixture.click(&fixture.element("App Settings"));
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Settings],
        "app settings is not an audio command"
    );
}

#[test]
fn quick_settings_content_fits_opaque_body_and_small_viewport_clips_safely() {
    let fixture = Fixture::ready();
    assert_eq!(fixture.popup.get_preferred_popup_width(), 320.0);
    let height = fixture.popup.get_preferred_popup_height();
    assert!(
        (100.0..200.0).contains(&height),
        "audio-only popup must fit its content, not reserve 720px"
    );
    let output = fixture.element("Output volume");
    let input = fixture.element("Input volume");
    assert!(output.absolute_position().y < input.absolute_position().y);
    for label in ["Mute output", "Mute input", "App Settings"] {
        let element = fixture.element(label);
        let position = element.absolute_position();
        let size = element.size();
        assert!(position.x >= 18.0 && position.y >= 18.0);
        assert!(position.x + size.width <= 302.0);
        assert!(position.y + size.height <= height - 18.0);
    }
    let pixels = fixture.render_fit();
    let interior = pixels[20 * 320 + 290];
    assert_eq!(
        (interior.r, interior.g, interior.b),
        (242, 242, 242),
        "body is opaque reference surface"
    );

    fixture.popup.set_status("Audio observation is temporarily unavailable. Retry to reconnect notifications and observe the current endpoints.".into());
    fixture.render_fit();
    assert!(fixture.popup.get_preferred_popup_height() > height);
    assert!(fixture.popup.get_preferred_popup_height() <= 720.0);
    let clipped = fixture.render(180, 80);
    assert_eq!(clipped.len(), 180 * 80);
    let corner = clipped[79 * 180 + 179];
    assert_ne!(
        (corner.r, corner.g, corner.b),
        (242, 242, 242),
        "body must not leak outside its clipped bounds"
    );
    fixture.render(1, 1);
    assert!(
        fixture.take_requests().is_empty(),
        "layout and clipping are presentation only"
    );
}
