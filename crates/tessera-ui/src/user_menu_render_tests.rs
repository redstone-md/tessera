// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Genuine generated-component input, accessibility and software pixels.
//! Callbacks are recorded only: this fixture has no folder host, OS open,
//! persistence, controller replacement or production-only testing switches.

use std::cell::RefCell;
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, Rgb8Pixel};

use crate::generated::{UserFolderKind, UserFolderRow, UserMenu};
use crate::theme::{PresentationTheme, ThemedComponent};

// Source order is an independent literal contract, not derived from the
// implementation's model or labels. Keys deliberately are not paths/indices.
const FOLDERS: [(UserFolderKind, &str, &str); 7] = [
    (
        UserFolderKind::Recent,
        "Open Recent",
        "opaque::session-A::recent/π",
    ),
    (
        UserFolderKind::Desktop,
        "Open Desktop",
        "opaque::session-A::desktop?ß",
    ),
    (
        UserFolderKind::Downloads,
        "Open Downloads",
        "opaque::session-A::download#語",
    ),
    (
        UserFolderKind::Documents,
        "Open Documents",
        "opaque::session-A::document%20",
    ),
    (
        UserFolderKind::Music,
        "Open Music",
        "opaque::session-A::music+♪",
    ),
    (
        UserFolderKind::Pictures,
        "Open Pictures",
        "opaque::session-A::picture=λ",
    ),
    (
        UserFolderKind::Videos,
        "Open Videos",
        "opaque::session-A::video&א",
    ),
];

fn ready_rows() -> Vec<UserFolderRow> {
    FOLDERS
        .iter()
        .map(|(kind, _, key)| UserFolderRow {
            kind: *kind,
            key: (*key).into(),
            ready: true,
            status: "".into(),
        })
        .collect()
}

#[derive(Debug, PartialEq)]
enum Request {
    Open(UserFolderKind, String),
    Retry,
    Hide,
}

struct Fixture {
    window: Rc<MinimalSoftwareWindow>,
    popup: UserMenu,
    requests: Rc<RefCell<Vec<Request>>>,
}

impl Fixture {
    fn new(configure: impl FnOnce(&UserMenu)) -> Self {
        struct TestPlatform(Rc<MinimalSoftwareWindow>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
        let popup = UserMenu::new().unwrap();
        popup.apply_presentation_theme(PresentationTheme::uniform(
            slint::language::ColorScheme::Light,
        ));
        popup.set_user_name("Actual fixture account".into());
        popup.set_rows(slint::ModelRc::new(slint::VecModel::from(ready_rows())));
        let requests = Rc::new(RefCell::new(Vec::new()));
        let recorded = requests.clone();
        popup.on_folder_open_requested(move |kind, key| {
            recorded
                .borrow_mut()
                .push(Request::Open(kind, key.to_string()));
        });
        let recorded = requests.clone();
        popup.on_retry_requested(move || recorded.borrow_mut().push(Request::Retry));
        let recorded = requests.clone();
        popup.on_hide_requested(move || recorded.borrow_mut().push(Request::Hide));
        configure(&popup);
        popup.show().unwrap();
        window
            .window()
            .dispatch_event(WindowEvent::WindowActiveChanged(true));
        let fixture = Self {
            window,
            popup,
            requests,
        };
        fixture.render_fit(1.0);
        fixture
    }

    fn ready() -> Self {
        Self::new(|_| {})
    }

    // Slint's raw label query includes paint-only Text with role=None and an
    // automatic label=text. Query the actual semantic AX nodes, retaining
    // every real role so duplicate buttons/text/images still fail uniqueness.
    fn labeled_accessible_elements(&self, label: &str) -> Vec<ElementHandle> {
        ElementHandle::find_by_accessible_label(&self.popup, label)
            .filter(|element| {
                matches!(element.accessible_role(), Some(role) if role != AccessibleRole::None)
            })
            .collect()
    }

    fn element(&self, label: &str) -> ElementHandle {
        let matches = self.labeled_accessible_elements(label);
        let diagnostics = matches
            .iter()
            .map(|element| {
                (
                    element.accessible_role(),
                    element.id(),
                    element.type_name(),
                    element.absolute_position(),
                    element.size(),
                )
            })
            .collect::<Vec<_>>();
        let mut elements = matches.into_iter();
        let element = elements
            .next()
            .unwrap_or_else(|| panic!("missing accessible element: {label}"));
        assert!(
            elements.next().is_none(),
            "duplicate accessible label: {label}; semantic matches (role,id,type,origin,size): {diagnostics:?}"
        );
        element
    }

    fn has_element(&self, label: &str) -> bool {
        !self.labeled_accessible_elements(label).is_empty()
    }

    fn buttons(&self) -> Vec<ElementHandle> {
        ElementQuery::from_root(&self.popup)
            .match_accessible_role(AccessibleRole::Button)
            .find_all()
    }

    fn render_fit(&self, scale: f32) -> Vec<Rgb8Pixel> {
        self.render(
            (self.popup.get_popup_content_width() * scale).ceil() as u32,
            (self.popup.get_popup_content_height() * scale).ceil() as u32,
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

    fn center(element: &ElementHandle) -> LogicalPosition {
        let origin = element.absolute_position();
        let size = element.size();
        assert!(
            size.width > 0.0 && size.height > 0.0,
            "input requires a noncollapsed element: {size:?}"
        );
        LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
    }

    fn press(&self, position: LogicalPosition) {
        let size = self.window.window().size();
        let scale = self.window.window().scale_factor();
        assert!(
            position.x >= 0.0
                && position.y >= 0.0
                && position.x * scale < size.width as f32
                && position.y * scale < size.height as f32,
            "input point {position:?} must fit actual viewport {size:?} at {scale}x"
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
        let position = Self::center(element);
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

    fn key(&self, key: Key) {
        self.key_press(key);
        self.key_release(key);
    }

    fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }
}

#[test]
fn user_menu_real_pointer_opens_each_exact_kind_and_opaque_key_without_projection_side_effects() {
    let fixture = Fixture::ready();
    assert!(
        fixture.take_requests().is_empty(),
        "reading/projecting/rendering never opens folders"
    );
    for (kind, label, key) in FOLDERS {
        let element = fixture.element(label);
        assert_eq!(element.accessible_role(), Some(AccessibleRole::Button));
        assert_eq!(element.accessible_enabled(), Some(true));
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerMoved {
                position: Fixture::center(&element),
            });
        assert!(
            fixture.take_requests().is_empty(),
            "hover is not activation"
        );
        fixture.click(&element);
        assert_eq!(
            fixture.take_requests(),
            vec![Request::Open(kind, key.to_owned())]
        );
    }
    fixture
        .popup
        .set_user_name("A different genuine account name".into());
    fixture.render_fit(1.0);
    assert!(
        fixture.take_requests().is_empty(),
        "identity changes are presentation only"
    );
}

#[test]
fn user_menu_accessibility_actions_forward_closed_kinds_and_do_not_invent_account_or_preview_actions()
 {
    let fixture = Fixture::ready();
    assert_eq!(
        fixture
            .buttons()
            .iter()
            .map(|button| button.accessible_label().unwrap().to_string())
            .collect::<Vec<_>>(),
        FOLDERS
            .iter()
            .map(|(_, label, _)| (*label).to_owned())
            .collect::<Vec<_>>(),
        "the open-only projection has exactly seven truthful actions in source order"
    );
    for (kind, label, key) in FOLDERS {
        fixture.element(label).invoke_accessible_default_action();
        assert_eq!(
            fixture.take_requests(),
            vec![Request::Open(kind, key.to_owned())]
        );
    }
    for absent in [
        "Settings",
        "Log out",
        "Sign out",
        "Power",
        "OneDrive",
        "Account",
        "Home",
        "Expand Recent",
        "Show files",
        "Select in Explorer",
    ] {
        assert!(
            !fixture.has_element(absent),
            "unsupported action is not a decorative preview: {absent}"
        );
    }
    assert!(
        !fixture.has_element("Retry folders"),
        "healthy ready state needs no speculative retry"
    );
}

#[test]
fn user_menu_tab_return_space_skip_unavailable_folders_and_escape_bubbles_from_real_buttons() {
    let fixture = Fixture::new(|popup| {
        let mut rows = ready_rows();
        rows[1].ready = false;
        rows[1].status = "Access denied".into();
        popup.set_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    });
    fixture.popup.invoke_focus_content();
    assert!(
        fixture.take_requests().is_empty(),
        "requesting focus does not open a folder"
    );
    fixture.key(Key::Tab);
    assert!(fixture.take_requests().is_empty(), "Tab changes focus only");
    fixture.key_press(Key::Return);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Open(
            UserFolderKind::Recent,
            FOLDERS[0].2.to_owned()
        )],
    );
    fixture.key_release(Key::Return);
    assert!(
        fixture.take_requests().is_empty(),
        "Return release cannot dispatch twice"
    );
    fixture.key(Key::Tab);
    fixture.key_press(Key::Space);
    assert!(
        fixture.take_requests().is_empty(),
        "Space is armed, not activated, on key down"
    );
    fixture.key_release(Key::Space);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Open(
            UserFolderKind::Downloads,
            FOLDERS[2].2.to_owned()
        )],
        "real Tab navigation skips the disabled Desktop row",
    );
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Open(
            UserFolderKind::Documents,
            FOLDERS[3].2.to_owned()
        )],
    );
    fixture.key(Key::Escape);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Hide],
        "Escape bubbles from an enabled folder button"
    );
    fixture.popup.invoke_focus_content();
    fixture.key(Key::Escape);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Hide],
        "Escape also dismisses from the popup's initial scope"
    );
}

#[test]
fn user_menu_loading_opening_unavailable_and_empty_keys_block_pointer_keyboard_and_accessibility() {
    let fixture = Fixture::ready();
    fixture.popup.invoke_focus_content();
    fixture.key(Key::Tab);
    for loading in [true, false] {
        fixture.popup.set_loading(loading);
        fixture.popup.set_opening(!loading);
        fixture.render_fit(1.0);
        assert!(
            fixture.has_element(if loading {
                "Loading folders…"
            } else {
                "Opening folder…"
            }),
            "in-flight state is visible and accessible, not just a blocked callback"
        );
        for (_, label, _) in FOLDERS {
            let element = fixture.element(label);
            assert_eq!(element.accessible_enabled(), Some(false));
            fixture.click(&element);
            element.invoke_accessible_default_action();
        }
        fixture.key(Key::Return);
        fixture.key(Key::Space);
        assert!(
            fixture.take_requests().is_empty(),
            "loading/opening is not permission to dispatch"
        );
    }

    fixture.popup.set_opening(false);
    let mut rows = ready_rows();
    rows[1].ready = false;
    rows[1].status = "Access denied".into();
    rows[4].key = "".into();
    fixture
        .popup
        .set_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    fixture.render_fit(1.0);
    for label in ["Open Desktop", "Open Music"] {
        let element = fixture.element(label);
        assert_eq!(element.accessible_enabled(), Some(false));
        fixture.click(&element);
        element.invoke_accessible_default_action();
    }
    assert!(
        fixture.take_requests().is_empty(),
        "unavailable rows and absent opaque keys have no open action"
    );
    let downloads = fixture.element("Open Downloads");
    assert_eq!(downloads.accessible_enabled(), Some(true));
    fixture.click(&downloads);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Open(
            UserFolderKind::Downloads,
            FOLDERS[2].2.to_owned()
        )],
        "one unavailable folder does not disable an independently ready folder",
    );

    // Cancel a genuine armed Space activation when the capability disappears.
    fixture.key_press(Key::Space);
    fixture.popup.set_loading(true);
    fixture.key_release(Key::Space);
    assert!(
        fixture.take_requests().is_empty(),
        "loading cancels an already armed key gesture"
    );
}

#[test]
fn user_menu_seven_rows_and_seventy_pixel_profile_fit_reference_body_in_light_dark_one_and_two_x() {
    let fixture = Fixture::ready();
    for (scheme, scale, background) in [
        (slint::language::ColorScheme::Light, 1.0, [242, 242, 242]),
        (slint::language::ColorScheme::Light, 2.0, [242, 242, 242]),
        (slint::language::ColorScheme::Dark, 1.0, [24, 24, 24]),
        (slint::language::ColorScheme::Dark, 2.0, [24, 24, 24]),
    ] {
        fixture
            .popup
            .apply_presentation_theme(PresentationTheme::uniform(scheme));
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        let pixels = fixture.render_fit(scale);
        let logical_width = fixture.popup.get_popup_content_width();
        let logical_height = fixture.popup.get_popup_content_height();
        assert!(
            (350.0..=400.0).contains(&(logical_width - 20.0)),
            "source body is 350–400 logical pixels, excluding two 10px shadow margins"
        );
        assert!(
            (250.0..700.0).contains(&logical_height),
            "seven actual folders determine height; there is no fake fixed 700px preview"
        );
        let width = (logical_width * scale).ceil() as usize;
        let sample = |x: f32, y: f32| {
            let pixel = pixels[(y * scale) as usize * width + (x * scale) as usize];
            [pixel.r, pixel.g, pixel.b]
        };
        assert_eq!(
            sample(logical_width - 12.0, 40.0),
            background,
            "neutral reference body remains opaque in both themes and scales"
        );
        assert_ne!(
            sample(0.0, 0.0),
            background,
            "shadow margin is not painted as an opaque body"
        );

        let profile = fixture.element("Default user profile");
        assert_eq!(profile.accessible_role(), Some(AccessibleRole::Image));
        assert_eq!(profile.size().width, 70.0);
        assert_eq!(profile.size().height, 70.0);
        let profile_origin = profile.absolute_position();
        assert!(profile_origin.x >= 18.0 && profile_origin.y >= 18.0);
        let profile_size = profile.size();
        let mut profile_ink = 0;
        for y in 0..70 {
            for x in 0..70 {
                if sample(profile_origin.x + x as f32, profile_origin.y + y as f32) != background {
                    profile_ink += 1;
                }
            }
        }
        assert!(
            profile_ink > 50,
            "the generic profile is genuinely drawn, not an empty fixture rectangle"
        );

        let mut previous_bottom = profile_origin.y + profile_size.height;
        for (index, (_, label, _)) in FOLDERS.into_iter().enumerate() {
            let element = fixture.element(label);
            let origin = element.absolute_position();
            let size = element.size();
            assert!(
                origin.x >= 18.0 && origin.x + size.width <= logical_width - 18.0 + 0.01,
                "{label} respects the real 8px body padding"
            );
            assert!(
                origin.y >= previous_bottom,
                "{label} follows source folder order without overlaps"
            );
            if index > 0 {
                assert!(
                    (origin.y - previous_bottom - 8.0).abs() < 0.01,
                    "folder stack preserves the source 8px gap"
                );
            }
            assert!(
                origin.y + size.height <= logical_height - 18.0 + 0.01,
                "{label} remains inside the opaque body and viewport"
            );
            assert!(
                size.width > 200.0 && size.height >= 30.0,
                "{label} is a real usable folder row"
            );
            assert!(
                (size.height - 33.92).abs() < 0.01,
                "ready folder has source 12.8px × 1.4 text line plus 8px top/bottom padding"
            );
            previous_bottom = origin.y + size.height;
        }
        assert_eq!(fixture.buttons().len(), 7);
        assert!(
            fixture.take_requests().is_empty(),
            "theme/DPI/geometry is silent"
        );
        assert!(
            !fixture.window.draw_if_needed(|renderer| {
                let mut unchanged = pixels.clone();
                renderer.render(&mut unchanged, width);
            }),
            "settled native popup adds no idle redraw"
        );
    }
    fixture
        .popup
        .apply_presentation_theme(PresentationTheme::uniform(
            slint::language::ColorScheme::Light,
        ));
    fixture
        .window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    let height = fixture.popup.get_popup_content_height().ceil() as u32;
    for viewport_width in [420, 500] {
        let pixels = fixture.render(viewport_width, height);
        let row = fixture.element("Open Recent");
        assert!(
            (row.size().width + 16.0 - 400.0).abs() < 0.01,
            "growing native viewport stops the body at the source 400px maximum"
        );
        let centered_body_x = (viewport_width as f32 - 420.0) / 2.0 + 10.0;
        assert!(
            (row.absolute_position().x - centered_body_x - 8.0).abs() < 0.01,
            "clamped body remains centered with actual 8px content padding"
        );
        let x = (centered_body_x + 398.0) as usize;
        let pixel = pixels[40 * viewport_width as usize + x];
        assert_eq!([pixel.r, pixel.g, pixel.b], [242, 242, 242]);
        assert_eq!(fixture.buttons().len(), 7);
        assert!(fixture.take_requests().is_empty());
    }
}

#[test]
fn user_menu_long_and_rtl_identity_is_bounded_without_losing_accessible_name_or_folder_actions() {
    let fixture = Fixture::ready();
    let original_height = fixture.popup.get_popup_content_height();
    fixture.popup.set_user_name("".into());
    fixture.render_fit(1.0);
    assert!(
        fixture.has_element("User name unavailable"),
        "missing identity is reported honestly rather than fabricating profile data"
    );
    assert!(fixture.take_requests().is_empty());
    for name in [
        "A genuine account with an exceptionally long display name ".repeat(8),
        "مستخدم طويل الاسم שלום משתמש בעל שם ארוך ".repeat(8),
    ] {
        fixture.popup.set_user_name(name.clone().into());
        let pixels = fixture.render_fit(1.0);
        let width = fixture.popup.get_popup_content_width();
        let height = fixture.popup.get_popup_content_height();
        assert!(
            (350.0..=400.0).contains(&(width - 20.0)),
            "long identity cannot widen the native body without bound"
        );
        assert_eq!(
            height, original_height,
            "a bounded profile identity cannot turn into a fake full-height preview"
        );
        let identity = fixture.element(&name);
        assert_eq!(identity.accessible_role(), Some(AccessibleRole::Text));
        let origin = identity.absolute_position();
        let size = identity.size();
        let profile = fixture.element("Default user profile");
        let profile_right = profile.absolute_position().x + profile.size().width;
        assert!(
            origin.x >= profile_right + 12.0 - 0.01,
            "real profile/name gap remains 12px; name-first={:?}, identity origin={origin:?} size={size:?}, profile origin={:?} size={:?}, profile-right={profile_right}, measured-gap={}",
            name.chars().next(),
            profile.absolute_position(),
            profile.size(),
            origin.x - profile_right,
        );
        assert!(
            size.width > 0.0 && origin.x + size.width <= width - 18.0 + 0.01,
            "long and RTL identity is elided inside body padding rather than leaking over the edge"
        );
        assert_eq!(profile.size().width, 70.0);
        assert_eq!(profile.size().height, 70.0);
        let physical_width = width.ceil() as usize;
        let strip_x = physical_width - 12;
        for y in 30..80 {
            let pixel = pixels[y * physical_width + strip_x];
            assert_eq!(
                [pixel.r, pixel.g, pixel.b],
                [242, 242, 242],
                "profile/name glyphs must not paint into the outer body gutter"
            );
        }
        assert_eq!(fixture.buttons().len(), 7);
        fixture.click(&fixture.element("Open Videos"));
        assert_eq!(
            fixture.take_requests(),
            vec![Request::Open(
                UserFolderKind::Videos,
                FOLDERS[6].2.to_owned()
            )],
            "long/RTL identity does not cover or replace the last genuine folder action",
        );
    }
}

#[test]
fn user_menu_tiny_viewport_clips_content_but_native_tab_reveals_and_activates_real_rows() {
    let fixture = Fixture::ready();
    let natural_width = fixture.popup.get_popup_content_width();
    let natural_height = fixture.popup.get_popup_content_height();
    for (scale, width, height) in [(1.0, 180, 120), (2.0, 360, 240)] {
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        let pixels = fixture.render(width, height);
        assert_eq!(pixels.len(), (width * height) as usize);
        let pixel = pixels[(height * width - 1) as usize];
        assert_ne!(
            [pixel.r, pixel.g, pixel.b],
            [242, 242, 242],
            "opaque content never leaks into the last pixel outside the inset native body"
        );
        assert!(
            !fixture.has_element("Open Videos"),
            "the clipped last row is not a phantom accessible hit target"
        );
        assert!(
            fixture.buttons().len() < 7,
            "actual query visibility follows the native clipped viewport"
        );
        assert_eq!(
            fixture.popup.get_popup_content_width(),
            natural_width,
            "placement clipping is independent from desired content metrics"
        );
        assert_eq!(fixture.popup.get_popup_content_height(), natural_height);
        assert!(
            fixture.take_requests().is_empty(),
            "native viewport clipping alone never dispatches"
        );
        fixture.popup.invoke_focus_content();
        for (kind, label, key) in FOLDERS {
            fixture.key(Key::Tab);
            let row = fixture.element(label);
            let center = Fixture::center(&row);
            assert!(
                center.x >= 0.0
                    && center.x < width as f32 / scale
                    && center.y >= 0.0
                    && center.y < height as f32 / scale,
                "Tab must reveal the actual focused {label} inside the clipped native viewport"
            );
            assert!(
                fixture.take_requests().is_empty(),
                "scroll-to-focus is not folder activation"
            );
            fixture.key(Key::Return);
            assert_eq!(
                fixture.take_requests(),
                vec![Request::Open(kind, key.to_owned())],
                "tiny viewport retains typed authority for the actual focused row"
            );
        }
        fixture.render(1, 1);
        assert!(
            fixture.take_requests().is_empty(),
            "tiny layout/DPI changes cannot open, retry or hide"
        );
        fixture.popup.invoke_focus_content();
        fixture.render_fit(scale);
        assert_eq!(
            fixture.buttons().len(),
            7,
            "genuine folders recover after the work area grows"
        );
    }
}

#[test]
fn user_menu_unknown_loading_has_no_fake_rows_and_unavailable_statuses_remain_readable_with_real_retry()
 {
    let fixture = Fixture::new(|popup| {
        popup.set_rows(slint::ModelRc::new(
            slint::VecModel::<UserFolderRow>::default(),
        ));
        popup.set_loading(true);
    });
    assert!(fixture.has_element("Loading folders…"));
    assert!(
        fixture.buttons().is_empty(),
        "unknown availability is not seven fabricated ready folders"
    );
    let loading_height = fixture.popup.get_popup_content_height();
    let statuses = [
        "Recent folder unavailable",
        "Desktop access denied",
        "Downloads target changed",
        "Documents provider stopped",
        "Music folder unavailable",
        "Pictures access denied",
        "Videos folder unavailable",
    ];
    let rows = FOLDERS
        .iter()
        .zip(statuses)
        .map(|((kind, _, key), status)| UserFolderRow {
            kind: *kind,
            key: (*key).into(),
            ready: false,
            status: status.into(),
        })
        .collect::<Vec<_>>();
    fixture.popup.set_loading(false);
    fixture
        .popup
        .set_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    fixture.render_fit(1.0);
    assert!(!fixture.has_element("Loading folders…"));
    assert!(
        fixture.popup.get_popup_content_height() > loading_height,
        "measured height grows with actual observed folder/status rows"
    );
    for ((_, label, _), status) in FOLDERS.iter().zip(statuses) {
        let row = fixture.element(label);
        assert_eq!(row.accessible_description().as_deref(), Some(status));
        assert_eq!(row.accessible_enabled(), Some(false));
        assert!(
            row.size().height > 50.0,
            "readable status reserves a real second text line"
        );
        fixture.click(&row);
        row.invoke_accessible_default_action();
    }
    assert!(
        fixture.take_requests().is_empty(),
        "unavailable status is not a folder permission"
    );

    let unavailable_height = fixture.popup.get_popup_content_height();
    let notice = "Folder observation is unavailable. Retry to read current folders.";
    fixture.popup.set_notice(notice.into());
    fixture.popup.set_retry_enabled(true);
    fixture.render_fit(1.0);
    assert!(fixture.has_element(notice));
    assert!(
        fixture.popup.get_popup_content_height() > unavailable_height,
        "notice and retry are measured content, not a reserved fake preview region"
    );
    fixture.click(&fixture.element("Retry folders"));
    assert_eq!(fixture.take_requests(), vec![Request::Retry]);
    fixture
        .element("Retry folders")
        .invoke_accessible_default_action();
    assert_eq!(fixture.take_requests(), vec![Request::Retry]);
    fixture.popup.invoke_focus_content();
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Retry],
        "native Tab skips all unavailable folders and reaches genuine recovery"
    );

    for loading in [true, false] {
        fixture.popup.set_loading(loading);
        fixture.popup.set_opening(!loading);
        fixture.render_fit(1.0);
        let retry = fixture.element("Retry folders");
        assert_eq!(retry.accessible_enabled(), Some(false));
        fixture.click(&retry);
        retry.invoke_accessible_default_action();
        fixture.key(Key::Return);
        fixture.key(Key::Space);
        assert!(
            fixture.take_requests().is_empty(),
            "read/open in flight blocks duplicate retry input"
        );
    }
    fixture.popup.set_opening(false);
    fixture.popup.set_retry_enabled(false);
    fixture.render_fit(1.0);
    assert!(
        !fixture.has_element("Retry folders"),
        "retry-disabled removes a nonexistent recovery capability"
    );
}

#[test]
fn user_menu_long_status_is_accessible_while_visible_text_and_wrapped_notice_stay_clipped_to_body()
{
    let status = "Access denied: the current folder cannot be opened; no substitute path or preview has been created. ".repeat(6);
    let fixture = Fixture::new(|popup| {
        let mut rows = ready_rows();
        rows[2].ready = false;
        rows[2].status = status.clone().into();
        popup.set_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    });
    let row = fixture.element("Open Downloads");
    assert_eq!(
        row.accessible_description().as_deref(),
        Some(status.as_str()),
        "visually elided error retains the full genuine accessible description"
    );
    assert_eq!(row.accessible_enabled(), Some(false));
    let original_height = fixture.popup.get_popup_content_height();
    let notice = "Folder observation failed. This is a real provider error, not a fabricated file preview. Retry to obtain current availability. ".repeat(10);
    fixture.popup.set_notice(notice.clone().into());
    let pixels = fixture.render_fit(1.0);
    let width = fixture.popup.get_popup_content_width().ceil() as usize;
    let height = fixture.popup.get_popup_content_height().ceil() as usize;
    assert!(
        fixture.popup.get_popup_content_height() > original_height,
        "wrapped feedback contributes real measured height"
    );
    assert!(
        height <= 720,
        "long feedback is scrollable under the source 700px body cap"
    );
    let strip_x = width - 12;
    for y in 30..height - 30 {
        let pixel = pixels[y * width + strip_x];
        assert_eq!(
            [pixel.r, pixel.g, pixel.b],
            [242, 242, 242],
            "neither long row status nor global feedback paints into the outer gutter"
        );
    }
    assert!(
        fixture.take_requests().is_empty(),
        "measuring provider feedback is never an open or retry"
    );
    fixture.popup.set_notice("".into());
    fixture.render_fit(1.0);
    assert_eq!(
        fixture.popup.get_popup_content_height(),
        original_height,
        "clearing actual feedback removes its height without retaining a fake preview"
    );
    fixture.click(&fixture.element("Open Downloads"));
    assert!(
        fixture.take_requests().is_empty(),
        "a visually clipped status remains unavailable"
    );
}
