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

use crate::generated::{AudioRoute, DockMediaView, MediaAction, QuickSettings};
use crate::theme::{PresentationTheme, ThemedComponent};

#[derive(Debug, PartialEq)]
enum Request {
    Volume(AudioRoute, f32),
    Released(AudioRoute, f32),
    Mute(AudioRoute, bool),
    Media(MediaAction, String),
    Seek(f32),
    SeekReleased,
    SeekKeyPressed(String),
    SeekKeyReleased(String),
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
        popup.apply_presentation_theme(PresentationTheme::seelen_reference(
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
        let recorded = requests.clone();
        popup.on_media_action_requested(move |action, identity| {
            recorded
                .borrow_mut()
                .push(Request::Media(action, identity.to_string()));
        });
        let recorded = requests.clone();
        let weak = popup.as_weak();
        popup.on_media_seek_requested(move |progress| {
            recorded.borrow_mut().push(Request::Seek(progress));
            if let Some(popup) = weak.upgrade() {
                popup.set_seek_preview_progress(progress);
                popup.set_seek_preview_active(true);
                popup.invoke_project_seek();
            }
        });
        let recorded = requests.clone();
        let weak = popup.as_weak();
        popup.on_media_seek_released(move || {
            recorded.borrow_mut().push(Request::SeekReleased);
            if let Some(popup) = weak.upgrade() {
                popup.set_seek_preview_active(false);
                popup.invoke_project_seek();
            }
        });
        let recorded = requests.clone();
        popup.on_media_seek_key_pressed(move |key| {
            recorded
                .borrow_mut()
                .push(Request::SeekKeyPressed(key.to_string()));
        });
        let recorded = requests.clone();
        popup.on_media_seek_key_released(move |key| {
            recorded
                .borrow_mut()
                .push(Request::SeekKeyReleased(key.to_string()));
        });
        configure(&popup);
        popup.invoke_project_seek();
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

    fn player() -> Self {
        Self::new(|popup| {
            popup.set_output_ready(true);
            popup.set_input_ready(true);
            popup.set_watch_live(true);
            popup.set_media_view(current_player());
            popup.set_timeline_notice("Timeline unavailable".into());
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
        // Mirror the production projection-batch boundary before measuring or
        // reading AX. Native value assignments never update observed facts.
        self.popup.invoke_project_seek();
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

    fn key_repeat(&self, key: Key) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressRepeated { text: key.into() });
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

fn current_player() -> DockMediaView {
    DockMediaView {
        enabled: true,
        current_present: true,
        session_identity: "recorded-popup-1:session-1".into(),
        status: "Current session".into(),
        title: "Actual title 後".into(),
        author: "Recorded artist · Björk".into(),
        playback: "Paused".into(),
        previous_enabled: true,
        toggle_enabled: true,
        next_enabled: true,
        ..DockMediaView::default()
    }
}

fn image(rgb: [u8; 3]) -> slint::Image {
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(8, 8);
    for pixel in pixels.make_mut_bytes().as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    slint::Image::from_rgba8(pixels)
}

#[test]
fn quick_settings_current_player_pointer_tab_key_and_ax_emit_captured_source_actions() {
    let fixture = Fixture::player();
    assert!(fixture.has_element("Current player"));
    assert!(fixture.has_element("Actual title 後\nRecorded artist · Björk\nPaused"));
    assert!(
        fixture.take_requests().is_empty(),
        "projection is observation-only"
    );
    for (label, action) in [
        ("Previous track", MediaAction::Previous),
        ("Play", MediaAction::Toggle),
        ("Next track", MediaAction::Next),
    ] {
        let element = fixture.element(label);
        assert_eq!(element.accessible_role(), Some(AccessibleRole::Button));
        assert_eq!(element.accessible_enabled(), Some(true));
        fixture.click(&element);
        assert_eq!(
            fixture.take_requests(),
            [Request::Media(action, "recorded-popup-1:session-1".into())]
        );
        element.invoke_accessible_default_action();
        assert_eq!(
            fixture.take_requests(),
            [Request::Media(action, "recorded-popup-1:session-1".into())]
        );
    }

    fixture.click(&fixture.element("Previous track"));
    fixture.take_requests();
    fixture.key_press(Key::Tab);
    fixture.key_release(Key::Tab);
    fixture.key_press(Key::Return);
    fixture.key_repeat(Key::Return);
    fixture.key_release(Key::Return);
    assert_eq!(
        fixture.take_requests(),
        [Request::Media(
            MediaAction::Toggle,
            "recorded-popup-1:session-1".into()
        )]
    );
    fixture.key_press(Key::Tab);
    fixture.key_release(Key::Tab);
    fixture.key_press(Key::Space);
    fixture.key_repeat(Key::Space);
    assert!(fixture.take_requests().is_empty());
    fixture.key_release(Key::Space);
    assert_eq!(
        fixture.take_requests(),
        [Request::Media(
            MediaAction::Next,
            "recorded-popup-1:session-1".into()
        )]
    );
    fixture.key_press(Key::Escape);
    fixture.key_release(Key::Escape);
    assert_eq!(fixture.take_requests(), [Request::Dismiss]);
}

#[test]
fn quick_settings_current_player_rebinding_and_hide_cancel_held_pointer_and_space() {
    let fixture = Fixture::player();
    fixture.click(&fixture.element("Play"));
    fixture.take_requests();
    fixture.key_press(Key::Space);
    let mut replacement = current_player();
    replacement.session_identity = "recorded-popup-2:session-2".into();
    fixture.popup.set_media_view(replacement.clone());
    fixture.render_fit();
    fixture.key_release(Key::Space);
    assert!(fixture.take_requests().is_empty());

    let position = Fixture::point(&fixture.element("Play"), 0.5);
    fixture.press(position);
    fixture.popup.set_media_view(current_player());
    fixture.render_fit();
    fixture.release(position);
    assert!(fixture.take_requests().is_empty());

    // Synchronous cancellation is required even if hide/reopen coalesces
    // property changes and the final projected incarnation stays the same.
    fixture.click(&fixture.element("Play"));
    fixture.take_requests();
    fixture.key_press(Key::Space);
    fixture.popup.invoke_cancel_media_input();
    fixture.key_release(Key::Space);
    assert!(fixture.take_requests().is_empty());
    fixture.press(position);
    fixture.popup.invoke_cancel_media_input();
    fixture.release(position);
    assert!(fixture.take_requests().is_empty());

    for state in 0..3 {
        let mut disabled = current_player();
        match state {
            0 => disabled.busy = true,
            1 => disabled.stale = true,
            _ => disabled.toggle_enabled = false,
        }
        fixture.popup.set_media_view(disabled);
        fixture.render_fit();
        let play = fixture.element("Play");
        assert_eq!(play.accessible_enabled(), Some(false));
        fixture.click(&play);
        play.invoke_accessible_default_action();
        fixture.key_press(Key::Return);
        fixture.key_release(Key::Return);
        assert!(fixture.take_requests().is_empty());
        if state == 2 {
            assert_eq!(
                fixture.element("Previous track").accessible_enabled(),
                Some(true)
            );
            assert_eq!(
                fixture.element("Next track").accessible_enabled(),
                Some(true)
            );
        }
    }
    replacement.playing = true;
    fixture.popup.set_media_view(replacement);
    fixture.render_fit();
    fixture.element("Pause").invoke_accessible_default_action();
    assert_eq!(
        fixture.take_requests(),
        [Request::Media(
            MediaAction::Toggle,
            "recorded-popup-2:session-2".into()
        )]
    );
}

#[test]
fn quick_settings_unknown_and_observed_timeline_are_read_only_and_do_not_gate_transports() {
    let fixture = Fixture::player();
    let unknown = fixture.element("Timeline unavailable");
    assert_eq!(unknown.accessible_role(), Some(AccessibleRole::Text));
    for label in ["Previous track", "Play", "Next track"] {
        assert_eq!(fixture.element(label).accessible_enabled(), Some(true));
    }
    fixture.click(&unknown);
    assert!(fixture.take_requests().is_empty());
    let unknown_height = fixture.popup.get_preferred_popup_height();
    fixture.popup.set_timeline_notice("".into());
    fixture.popup.set_timeline_available(true);
    fixture.popup.set_timeline_time("0:30 / 3:00".into());
    fixture.popup.set_timeline_progress(1.0 / 6.0);
    fixture.render_fit();
    let observed = fixture.element("Observed position: 0:30 / 3:00");
    assert_eq!(observed.accessible_role(), Some(AccessibleRole::Text));
    fixture.click(&observed);
    observed.invoke_accessible_default_action();
    assert!(
        fixture.take_requests().is_empty(),
        "no seek path is fabricated"
    );
    assert!(fixture.popup.get_preferred_popup_height() > unknown_height);
    fixture.popup.set_timeline_time("0:45 / 3:00".into());
    fixture.popup.set_timeline_progress(0.25);
    fixture.render_fit();
    assert!(fixture.has_element("Observed position: 0:45 / 3:00"));
    assert_eq!(fixture.element("Play").accessible_enabled(), Some(true));
    assert!(fixture.take_requests().is_empty());

    fixture.popup.set_media_view(DockMediaView {
        enabled: true,
        status: "Not playing".into(),
        ..DockMediaView::default()
    });
    fixture.popup.set_timeline_available(false);
    fixture.popup.set_timeline_time("".into());
    fixture.render_fit();
    assert!(fixture.has_element("Current player"));
    assert!(fixture.has_element("Not playing"));
    for label in [
        "Previous track",
        "Play",
        "Next track",
        "Observed position: 0:45 / 3:00",
    ] {
        assert!(
            !fixture.has_element(label),
            "absence must not retain {label}"
        );
    }
    assert!(fixture.take_requests().is_empty());
}

#[test]
fn quick_settings_late_media_timeline_and_all_notices_fit_real_art_and_preserve_audio_footer() {
    let fixture = Fixture::ready();
    let audio_height = fixture.popup.get_preferred_popup_height();
    let mut view = current_player();
    view.artwork = image([219, 13, 73]);
    view.has_artwork = true;
    view.app_icon = image([11, 187, 31]);
    view.has_app_icon = true;
    view.read_notice = "Recorded read notice".into();
    view.watch_notice = "Recorded watch notice".into();
    view.action_notice = "Recorded action notice".into();
    view.artwork_notice = "Recorded artwork notice".into();
    fixture.popup.set_media_view(view.clone());
    fixture.popup.set_timeline_notice("Timeline observation is unavailable for this current player; transports still use independently observed capabilities.".into());
    let pixels = fixture.render_fit();
    let height = fixture.popup.get_preferred_popup_height();
    assert!(height > audio_height && height <= 720.0);
    assert!(fixture.has_element("Recorded read notice\nRecorded watch notice\nRecorded action notice\nRecorded artwork notice"));
    assert!(fixture.has_element("Current session"));
    assert!(fixture.has_element("Timeline observation is unavailable for this current player; transports still use independently observed capabilities."));
    // This fixture supplies raw square pixels, not the shared cover mask.
    // Verify genuine dimensions and the reference's separate metadata/icon
    // siblings without pretending that Slint clips image corners.
    let width = fixture.popup.get_preferred_popup_width().ceil() as usize;
    let coordinates = |rgb| {
        pixels
            .iter()
            .enumerate()
            .filter_map(|(index, pixel)| {
                ([pixel.r, pixel.g, pixel.b] == rgb).then_some((index % width, index / width))
            })
            .collect::<Vec<_>>()
    };
    let cover = coordinates([219, 13, 73]);
    let app = coordinates([11, 187, 31]);
    assert!(cover.len() >= 1400, "actual raw 40px cover must render");
    assert!(app.len() >= 200, "actual trusted 16px app icon must render");
    let cover_right = cover.iter().map(|(x, _)| *x).max().unwrap();
    let cover_left = cover.iter().map(|(x, _)| *x).min().unwrap();
    let cover_top = cover.iter().map(|(_, y)| *y).min().unwrap();
    let cover_bottom = cover.iter().map(|(_, y)| *y).max().unwrap();
    assert!((39..=40).contains(&(cover_right - cover_left + 1)));
    assert!((39..=40).contains(&(cover_bottom - cover_top + 1)));
    let app_left = app.iter().map(|(x, _)| *x).min().unwrap();
    let app_right = app.iter().map(|(x, _)| *x).max().unwrap();
    assert!((15..=16).contains(&(app_right - app_left + 1)));
    assert!(
        app_left > cover_right,
        "app icon is a sibling, never a cover overlay"
    );
    let metadata = fixture.element("Actual title 後\nRecorded artist · Björk\nPaused");
    let covered_metadata_x = metadata.absolute_position().x;
    assert!(
        app_left as f32 >= metadata.absolute_position().x + metadata.size().width,
        "trusted app icon belongs after the metadata column"
    );
    for label in [
        "Mute output",
        "Mute input",
        "Previous track",
        "Play",
        "Next track",
        "App Settings",
    ] {
        let element = fixture.element(label);
        let origin = element.absolute_position();
        let size = element.size();
        assert!(origin.x >= 18.0 && origin.y >= 18.0);
        assert!(origin.x + size.width <= 302.0);
        assert!(
            origin.y + size.height <= height - 18.0,
            "late data must not clip {label}"
        );
    }

    view.has_artwork = false;
    fixture.popup.set_media_view(view.clone());
    let pixels = fixture.render_fit();
    assert!(
        pixels
            .iter()
            .all(|pixel| [pixel.r, pixel.g, pixel.b] != [219, 13, 73])
    );
    assert!(
        pixels
            .iter()
            .filter(|pixel| [pixel.r, pixel.g, pixel.b] == [11, 187, 31])
            .count()
            >= 200,
        "trusted app icon remains independent when no cover was observed"
    );
    let uncovered_metadata = fixture.element("Actual title 後\nRecorded artist · Björk\nPaused");
    assert!(
        uncovered_metadata.absolute_position().x < covered_metadata_x,
        "missing cover must not reserve a fabricated album slot"
    );
    view.has_app_icon = false;
    fixture.popup.set_media_view(view);
    fixture.popup.set_timeline_notice("".into());
    fixture.popup.set_timeline_available(true);
    fixture.popup.set_timeline_time("0:30 / 3:00".into());
    fixture.popup.set_timeline_progress(1.0 / 6.0);
    let pixels = fixture.render_fit();
    assert!(
        pixels
            .iter()
            .all(|pixel| [pixel.r, pixel.g, pixel.b] != [219, 13, 73]
                && [pixel.r, pixel.g, pixel.b] != [11, 187, 31]),
        "no fabricated or retained artwork"
    );
    assert!(fixture.has_element("Observed position: 0:30 / 3:00"));
    fixture.render(180, 80);
    assert!(
        fixture.take_requests().is_empty(),
        "layout never emits transports or seek"
    );
}

#[test]
fn quick_settings_media_observation_failure_exposes_existing_refresh_without_transport_replay() {
    let fixture = Fixture::player();
    let mut view = current_player();
    view.read_notice = "Media read failed".into();
    view.stale = true;
    fixture.popup.set_media_view(view);
    fixture.render_fit();
    let refresh = fixture.element("Refresh audio and media");
    assert_eq!(refresh.accessible_role(), Some(AccessibleRole::Button));
    fixture.click(&refresh);
    assert_eq!(fixture.take_requests(), [Request::Refresh]);
    refresh.invoke_accessible_default_action();
    assert_eq!(fixture.take_requests(), [Request::Refresh]);
    assert_eq!(fixture.element("Play").accessible_enabled(), Some(false));
}

#[test]
fn quick_settings_seek_projection_is_silent_and_unsupported_slider_stays_native() {
    let fixture = Fixture::player();
    let slider = fixture.element("Media position");
    assert_eq!(slider.accessible_role(), Some(AccessibleRole::Slider));
    assert_eq!(slider.accessible_enabled(), Some(false));
    assert_eq!(slider.accessible_value_minimum(), Some(0.0));
    assert_eq!(slider.accessible_value_maximum(), Some(1.0));
    assert_eq!(slider.accessible_value_step(), Some(0.01));
    fixture.popup.set_seek_progress(0.3);
    fixture.popup.set_seek_notice("Seeking unsupported".into());
    fixture.render_fit();
    assert_eq!(slider.accessible_value().unwrap().as_str(), "0.3");
    assert!(fixture.has_element("Seeking unsupported"));
    assert!(fixture.has_element("Timeline unavailable"));
    slider.set_accessible_value("0.7");
    assert_eq!(
        slider.accessible_value().unwrap().as_str(),
        "0.7",
        "standard Slider AX can update its local value even while disabled"
    );
    assert_eq!(
        fixture.popup.get_seek_progress(),
        0.3,
        "native AX must not rewrite the observed position"
    );
    assert!(!fixture.popup.get_seek_preview_active());
    fixture.click(&slider);
    fixture.key_press(Key::End);
    fixture.key_release(Key::End);
    assert!(
        fixture.take_requests().is_empty(),
        "unsupported seeks never emit requests"
    );
    for label in ["Previous track", "Play", "Next track"] {
        assert_eq!(fixture.element(label).accessible_enabled(), Some(true));
    }

    fixture.popup.set_seek_enabled(true);
    fixture.popup.set_seek_progress(0.4);
    fixture.popup.set_seek_preview_progress(0.6);
    fixture.popup.set_seek_preview_active(true);
    fixture.render_fit();
    assert_eq!(slider.accessible_enabled(), Some(true));
    assert_eq!(slider.accessible_value().unwrap().as_str(), "0.6");
    fixture.popup.set_seek_progress(0.5);
    fixture.render_fit();
    assert_eq!(
        slider.accessible_value().unwrap().as_str(),
        "0.6",
        "observation does not overwrite local preview"
    );
    fixture.popup.set_seek_preview_active(false);
    fixture.render_fit();
    assert_eq!(slider.accessible_value().unwrap().as_str(), "0.5");
    assert!(
        fixture.take_requests().is_empty(),
        "projection is not a native changed action"
    );
    let invalidations = Rc::new(RefCell::new(0));
    let recorded = invalidations.clone();
    fixture
        .popup
        .on_media_seek_invalidated(move || *recorded.borrow_mut() += 1);
    let recorded = invalidations.clone();
    fixture
        .popup
        .on_media_seek_geometry_changed(move || *recorded.borrow_mut() += 1);
    let mut busy = current_player();
    busy.busy = true;
    fixture.popup.set_media_view(busy);
    fixture.render_fit();
    assert_eq!(
        slider.accessible_enabled(),
        Some(true),
        "transport busy does not disable seeking"
    );
    assert_eq!(
        *invalidations.borrow(),
        0,
        "busy-only projection must not invalidate the seek"
    );
    assert_eq!(fixture.element("Play").accessible_enabled(), Some(false));
    assert!(fixture.take_requests().is_empty());
}

#[test]
fn quick_settings_seek_genuine_ax_and_keys_capture_before_native_changed_without_release_command() {
    let fixture = Fixture::player();
    fixture.popup.set_seek_enabled(true);
    fixture.popup.set_seek_progress(0.4);
    fixture.render_fit();
    let slider = fixture.element("Media position");
    slider.set_accessible_value("0.7");
    assert_eq!(fixture.take_requests(), [Request::Seek(0.7)]);
    slider.set_accessible_value("0.7");
    slider.set_accessible_value("not a number");
    assert!(
        fixture.take_requests().is_empty(),
        "native AX ignores unchanged and invalid values"
    );
    slider.set_accessible_value("2");
    assert_eq!(fixture.take_requests(), [Request::Seek(1.0)]);
    slider.set_accessible_value("-1");
    assert_eq!(fixture.take_requests(), [Request::Seek(0.0)]);

    // Real pointer dispatch also exercises the unchanged native mechanics;
    // command-scope capture belongs to the controller adapter's fixtures.
    let start = Fixture::point(&slider, 0.7);
    let end = Fixture::point(&slider, 0.85);
    fixture.press(start);
    fixture
        .window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: end });
    let requests = fixture.take_requests();
    assert!(!requests.is_empty());
    assert!(
        requests.iter().all(|request| matches!(request,
        Request::Seek(progress) if (0.0..=1.0).contains(progress))),
        "native pointer changed is normalized, with no premature release: {requests:?}"
    );
    fixture.release(end);
    assert_eq!(
        fixture.take_requests(),
        [Request::SeekReleased],
        "pointer release never duplicates the command"
    );
    fixture.popup.set_seek_preview_active(false);
    fixture.popup.set_seek_progress(0.4);
    fixture.popup.invoke_project_seek();
    fixture.key_press(Key::RightArrow);
    let requests = fixture.take_requests();
    assert!(
        matches!(&requests[..],
        [Request::SeekKeyPressed(key), Request::Seek(progress)]
            if key == &String::from(slint::SharedString::from(Key::RightArrow))
                && (*progress - 0.41).abs() < 0.0001),
        "capture must precede the native Slider changed callback: {requests:?}"
    );
    fixture.key_release(Key::RightArrow);
    assert_eq!(
        fixture.take_requests(),
        [
            Request::SeekKeyReleased(slint::SharedString::from(Key::RightArrow).to_string()),
            Request::SeekReleased,
        ],
        "release is preview lifecycle, never an additional seek"
    );
    for (key, progress) in [(Key::Home, 0.0), (Key::End, 1.0)] {
        fixture.key_press(key);
        assert_eq!(
            fixture.take_requests(),
            [
                Request::SeekKeyPressed(slint::SharedString::from(key).to_string()),
                Request::Seek(progress),
            ]
        );
        fixture.key_release(key);
        assert_eq!(
            fixture.take_requests(),
            [
                Request::SeekKeyReleased(slint::SharedString::from(key).to_string()),
                Request::SeekReleased,
            ]
        );
    }
    fixture.click(&fixture.element("Output volume"));
    fixture.take_requests();
    fixture.key_press(Key::RightArrow);
    assert!(
        fixture
            .take_requests()
            .iter()
            .all(|request| matches!(request, Request::Volume(..))),
        "audio focus must never capture a media key"
    );
    fixture.key_release(Key::RightArrow);
    fixture.take_requests();
    fixture.key_press(Key::Escape);
    fixture.key_release(Key::Escape);
    assert_eq!(fixture.take_requests(), [Request::Dismiss]);
}

#[test]
fn quick_settings_seek_bounds_match_actual_widget_and_real_scroll_clip() {
    let fixture = Fixture::player();
    fixture.popup.set_seek_enabled(true);
    fixture
        .popup
        .set_seek_notice("Seek failed independently".into());
    fixture.render_fit();
    let slider = fixture.element("Media position");
    let bounds = fixture.popup.get_seek_bounds();
    assert_eq!(bounds.origin.x, slider.absolute_position().x);
    assert_eq!(bounds.origin.y, slider.absolute_position().y);
    assert_eq!(bounds.width, slider.size().width);
    assert_eq!(bounds.height, slider.size().height);
    assert!(bounds.width > 100.0 && bounds.height == 24.0);
    assert!(fixture.popup.get_seek_visible());
    let clip = fixture.popup.get_seek_clip_bounds();
    assert!(clip.origin.x.is_finite() && clip.origin.y.is_finite());
    assert!(clip.width.is_finite() && clip.width > 0.0);
    assert!(clip.height.is_finite() && clip.height > 0.0);
    assert!(bounds.origin.x >= clip.origin.x);
    assert!(bounds.origin.y >= clip.origin.y);
    assert!(bounds.origin.x + bounds.width <= clip.origin.x + clip.width);
    assert!(bounds.origin.y + bounds.height <= clip.origin.y + clip.height);
    assert!(fixture.has_element("Seek failed independently"));
    assert!(fixture.has_element("Timeline unavailable"));
    assert!(fixture.has_element("App Settings"));
    assert_eq!(fixture.element("Play").accessible_enabled(), Some(true));
    let geometry_changes = Rc::new(RefCell::new(0));
    let recorded = geometry_changes.clone();
    fixture
        .popup
        .on_media_seek_geometry_changed(move || *recorded.borrow_mut() += 1);
    let native_invalidations = Rc::new(RefCell::new(0));
    let recorded = native_invalidations.clone();
    fixture
        .popup
        .on_media_seek_invalidated(move || *recorded.borrow_mut() += 1);
    fixture.render(180, 80);
    assert!(
        !fixture.popup.get_seek_visible(),
        "off-scroll-viewport widget cannot authorize pointer input"
    );
    fixture.render_fit();
    assert!(fixture.popup.get_seek_visible());
    assert!(
        *geometry_changes.borrow() > 0,
        "actual widget resizing/clipping emits a geometry frame recheck"
    );
    assert_eq!(
        *native_invalidations.borrow(),
        0,
        "geometry notifications do not revoke native authority unconditionally"
    );
    let geometry_before_cancel = *geometry_changes.borrow();
    fixture.popup.invoke_cancel_media_input();
    assert_eq!(
        *native_invalidations.borrow(),
        1,
        "explicit input cancellation still revokes native authority"
    );
    assert_eq!(*geometry_changes.borrow(), geometry_before_cancel);
    assert!(
        fixture.take_requests().is_empty(),
        "geometry and notices never seek"
    );
}

#[test]
fn quick_settings_seek_preview_time_is_separate_silent_and_geometry_stable() {
    let fixture = Fixture::player();
    fixture.popup.set_seek_enabled(true);
    fixture.popup.set_timeline_available(true);
    fixture.popup.set_timeline_time("0:30 / 3:00".into());
    fixture.popup.set_timeline_progress(1.0 / 6.0);
    fixture.popup.set_seek_progress(1.0 / 6.0);
    fixture.render_fit();
    let bounds = fixture.popup.get_seek_bounds();
    let height = fixture.popup.get_preferred_popup_height();
    let invalidations = Rc::new(RefCell::new(0));
    let recorded = invalidations.clone();
    fixture
        .popup
        .on_media_seek_invalidated(move || *recorded.borrow_mut() += 1);
    let recorded = invalidations.clone();
    fixture
        .popup
        .on_media_seek_geometry_changed(move || *recorded.borrow_mut() += 1);

    fixture.popup.set_seek_preview_progress(0.75);
    fixture.popup.set_seek_preview_time("2:15 / 3:00".into());
    fixture.popup.set_seek_preview_active(true);
    fixture.render_fit();
    assert!(fixture.has_element("Preview position: 2:15 / 3:00"));
    assert!(fixture.has_element("Observed position: 0:30 / 3:00"));
    assert_eq!(fixture.popup.get_timeline_time().as_str(), "0:30 / 3:00");
    assert_eq!(fixture.popup.get_timeline_progress(), 1.0 / 6.0);
    assert_eq!(fixture.popup.get_seek_progress(), 1.0 / 6.0);
    assert_eq!(
        fixture
            .element("Media position")
            .accessible_value()
            .unwrap()
            .as_str(),
        "0.75"
    );

    // Even a longer formatted time is one elided line, not a resize that
    // invalidates the held native Slider's authority.
    fixture
        .popup
        .set_seek_preview_time("123456:59 / 999999:59".into());
    fixture.render_fit();
    assert!(fixture.has_element("Preview position: 123456:59 / 999999:59"));
    let preview_bounds = fixture.popup.get_seek_bounds();
    assert_eq!(
        (
            preview_bounds.origin.x,
            preview_bounds.origin.y,
            preview_bounds.width,
            preview_bounds.height
        ),
        (
            bounds.origin.x,
            bounds.origin.y,
            bounds.width,
            bounds.height
        ),
    );
    assert_eq!(fixture.popup.get_preferred_popup_height(), height);
    assert_eq!(*invalidations.borrow(), 0);

    fixture.popup.set_seek_preview_active(false);
    fixture.render_fit();
    assert!(!fixture.has_element("Preview position: 123456:59 / 999999:59"));
    assert!(fixture.has_element("Observed position: 0:30 / 3:00"));
    assert_eq!(fixture.popup.get_seek_bounds().origin.y, bounds.origin.y);
    assert_eq!(fixture.popup.get_preferred_popup_height(), height);
    assert_eq!(*invalidations.borrow(), 0);
    assert!(
        fixture.take_requests().is_empty(),
        "local time preview never emits changed IO"
    );
}
