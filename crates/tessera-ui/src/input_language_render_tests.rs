// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Generated selector, owned native software pixels and genuine input/AX only.
//! Explicit presentation fixtures do not enumerate or change any OS profile.

use crate::generated::{InputLanguageMenu, InputProfileRow, PopoverMotion, SeelenPalette};
use crate::native_typography_oracle::SourceTypographyText;
use crate::theme::{PresentationTheme, ThemedComponent};
use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, TargetPixel};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::private_unstable_api::re_exports::{
    ComplexText, ImageItem, Item, ItemRc, Orientation, TextHorizontalAlignment, WindowInner,
};
use slint::{ComponentHandle, LogicalPosition, ModelRc, PhysicalSize, Rgb8Pixel, VecModel};
use std::cell::{Cell, RefCell};
use std::ops::ControlFlow;
use std::rc::Rc;

const SETTINGS_KEY: &str = "opaque::fixture-session/Settings-π";
fn row_key(index: usize) -> String {
    format!("opaque::fixture-profile/{index}/語")
}
fn language(index: usize) -> String {
    if index < 2 {
        "Fixture localized 日本語".into()
    } else {
        format!("Fixture localized لغة {index}")
    }
}
fn description(index: usize) -> String {
    if index == 1 {
        "Fixture enabled TIP 日本語".into()
    } else {
        format!("Fixture enabled keyboard {index}")
    }
}
fn label(index: usize) -> String {
    format!("{} - {}", language(index), description(index))
}
fn rows(count: usize) -> Vec<InputProfileRow> {
    (0..count)
        .map(|index| InputProfileRow {
            key: row_key(index).into(),
            language_name: language(index).into(),
            layout_name: description(index).into(),
            active: index == 0 || index == 2,
        })
        .collect()
}
fn ready(popup: &InputLanguageMenu, count: usize) {
    popup.set_rows(ModelRc::new(VecModel::from(rows(count))));
    popup.set_settings_key(SETTINGS_KEY.into());
    popup.set_observed(true);
    popup.set_unknown_active(false);
}
#[derive(Debug, PartialEq)]
enum Request {
    Profile(String),
    Settings(String),
    Retry,
    Hide,
}
struct Fixture {
    window: Rc<MinimalSoftwareWindow>,
    source_window: Rc<MinimalSoftwareWindow>,
    popup: InputLanguageMenu,
    requests: Rc<RefCell<Vec<Request>>>,
}
impl Fixture {
    fn new(configure: impl FnOnce(&InputLanguageMenu)) -> Self {
        struct TestPlatform {
            popup: Rc<MinimalSoftwareWindow>,
            source: Rc<MinimalSoftwareWindow>,
            popup_created: Cell<bool>,
        }
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(if self.popup_created.replace(true) {
                    self.source.clone()
                } else {
                    self.popup.clone()
                })
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        let source_window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform {
            popup: window.clone(),
            source: source_window.clone(),
            popup_created: Cell::new(false),
        }))
        .unwrap();
        let popup = InputLanguageMenu::new().unwrap();
        popup.apply_presentation_theme(PresentationTheme::seelen_reference(
            slint::language::ColorScheme::Light,
        ));
        let requests = Rc::new(RefCell::new(Vec::new()));
        let recorded = requests.clone();
        popup.on_select_profile(move |key| {
            recorded
                .borrow_mut()
                .push(Request::Profile(key.to_string()))
        });
        let recorded = requests.clone();
        popup.on_open_settings(move |key| {
            recorded
                .borrow_mut()
                .push(Request::Settings(key.to_string()))
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
        let f = Self {
            window,
            source_window,
            popup,
            requests,
        };
        f.render_fit(1.0);
        f
    }
    fn ready(count: usize) -> Self {
        Self::new(|popup| ready(popup, count))
    }
    fn element(&self, label: &str, role: AccessibleRole) -> ElementHandle {
        let matches = ElementHandle::find_by_accessible_label(&self.popup, label)
            .filter(|element| element.accessible_role() == Some(role))
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "unique semantic node required: {label}");
        matches.into_iter().next().unwrap()
    }
    fn button(&self, label: &str) -> ElementHandle {
        self.element(label, AccessibleRole::Button)
    }
    fn footer(&self) -> ElementHandle {
        self.button("More keyboard settings")
    }
    fn buttons(&self) -> Vec<ElementHandle> {
        ElementQuery::from_root(&self.popup)
            .match_accessible_role(AccessibleRole::Button)
            .find_all()
    }
    fn scale(&self, scale: f32) {
        self.event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
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
    fn event(&self, event: WindowEvent) {
        self.window.window().dispatch_event(event);
    }
    fn center(element: &ElementHandle) -> LogicalPosition {
        let p = element.absolute_position();
        let s = element.size();
        assert!(s.width > 0.0 && s.height > 0.0);
        LogicalPosition::new(p.x + s.width / 2.0, p.y + s.height / 2.0)
    }
    fn hover(&self, p: LogicalPosition) {
        self.event(WindowEvent::PointerMoved { position: p });
    }
    fn press(&self, p: LogicalPosition) {
        self.hover(p);
        self.event(WindowEvent::PointerPressed {
            position: p,
            button: PointerEventButton::Left,
        });
    }
    fn release(&self, p: LogicalPosition) {
        self.event(WindowEvent::PointerReleased {
            position: p,
            button: PointerEventButton::Left,
        });
    }
    fn click(&self, element: &ElementHandle) {
        let p = Self::center(element);
        self.press(p);
        self.release(p);
    }
    fn key_press(&self, key: Key) {
        self.event(WindowEvent::KeyPressed { text: key.into() });
    }
    fn key_release(&self, key: Key) {
        self.event(WindowEvent::KeyReleased { text: key.into() });
    }
    fn repeat(&self, key: Key) {
        self.event(WindowEvent::KeyPressRepeated { text: key.into() });
    }
    fn key(&self, key: Key) {
        self.key_press(key);
        self.key_release(key);
    }
    fn take(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }
}
fn rgb(color: slint::Color) -> [u8; 3] {
    let color = color.to_argb_u8();
    [color.red, color.green, color.blue]
}
fn sample(pixels: &[Rgb8Pixel], width: usize, scale: f32, x: f32, y: f32) -> [u8; 3] {
    let p = pixels[(y * scale).floor() as usize * width + (x * scale).floor() as usize];
    [p.r, p.g, p.b]
}

fn near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.1,
        "actual={actual}, source={expected}"
    );
}

fn native_text(popup: &InputLanguageMenu, content: &str) -> ItemRc {
    ItemRc::new_root(WindowInner::from_pub(popup.window()).component())
        .visit_descendants(|item| {
            if item
                .downcast::<ComplexText>()
                .is_some_and(|text| text.as_pin_ref().text().as_str() == content)
            {
                ControlFlow::Break(item.clone())
            } else {
                ControlFlow::Continue(())
            }
        })
        .expect("real native selector Text")
}

fn assert_source_line(popup: &InputLanguageMenu, content: &str) {
    let item = native_text(popup, content);
    let text = item.downcast::<ComplexText>().unwrap();
    let text = text.as_pin_ref();
    // reset.css:11–12/97–98, shared/index.scss:107 and
    // keyboard-selector.scss:8/38/42–43/55–56: all inherit 0.8rem/600.
    near(text.font_size().get(), 12.8);
    assert_eq!(text.font_weight(), 600);
    near(text.height().get(), 17.92);
    let adapter = WindowInner::from_pub(popup.window()).window_adapter();
    near(
        text.layout_info(Orientation::Vertical, -1.0, &adapter, &item)
            .preferred,
        17.92,
    );
}

fn assert_revealed_between_header_and_footer(f: &Fixture, item: &ElementHandle) {
    let header = f.element("Keyboard layout", AccessibleRole::Text);
    let viewport = ElementHandle::find_by_element_id(&f.popup, "ScrollView::flickable")
        .next()
        .expect("genuine native selector scroll viewport");
    let top = viewport.absolute_position().y;
    let bottom = top + viewport.size().height;
    assert!(top >= header.absolute_position().y + header.size().height);
    assert!(bottom <= f.footer().absolute_position().y);
    let p = item.absolute_position();
    let s = item.size();
    assert!(
        p.y >= top && p.y + s.height <= bottom,
        "focused native control must be wholly revealed inside its actual viewport"
    );
}

#[test]
fn profile_pointer_release_and_ax_keep_native_labels_order_and_observed_selection() {
    let f = Fixture::ready(3);
    assert_eq!(f.buttons().len(), 4);
    for index in 0..3 {
        let row = f.button(&label(index));
        assert_eq!(row.accessible_item_selectable(), Some(true));
        assert_eq!(
            row.accessible_item_selected(),
            Some(index == 0 || index == 2)
        );
        assert_eq!(
            row.accessible_description().as_deref(),
            Some(if index == 0 || index == 2 {
                "Active keyboard profile"
            } else {
                ""
            })
        );
        let p = Fixture::center(&row);
        f.press(p);
        assert!(f.take().is_empty(), "press is not activation");
        f.release(p);
        assert_eq!(f.take(), vec![Request::Profile(row_key(index))]);
        row.invoke_accessible_default_action();
        assert_eq!(f.take(), vec![Request::Profile(row_key(index))]);
        assert_eq!(
            row.accessible_item_selected(),
            Some(index == 0 || index == 2),
            "selection is not optimistically changed by presentation input"
        );
    }
    f.click(&f.footer());
    assert_eq!(f.take(), vec![Request::Settings(SETTINGS_KEY.into())]);
    f.footer().invoke_accessible_default_action();
    assert_eq!(f.take(), vec![Request::Settings(SETTINGS_KEY.into())]);
}

#[test]
fn real_tab_shift_tab_fresh_return_space_and_escape_follow_selector_controls() {
    let f = Fixture::ready(3);
    f.popup.invoke_focus_content();
    f.key(Key::Tab);
    assert!(f.take().is_empty());
    f.key_press(Key::Return);
    assert_eq!(f.take(), vec![Request::Profile(row_key(0))]);
    f.repeat(Key::Return);
    f.repeat(Key::Return);
    f.key_release(Key::Return);
    assert!(
        f.take().is_empty(),
        "held Return never repeats a non-idempotent action"
    );
    f.key(Key::Tab);
    f.key_press(Key::Space);
    f.repeat(Key::Space);
    assert!(f.take().is_empty(), "Space commits only on release");
    f.key_release(Key::Space);
    assert_eq!(f.take(), vec![Request::Profile(row_key(1))]);
    f.key_press(Key::Shift);
    f.key(Key::Tab);
    f.key_release(Key::Shift);
    f.key(Key::Return);
    assert_eq!(f.take(), vec![Request::Profile(row_key(0))]);
    for _ in 0..3 {
        f.key(Key::Tab);
    }
    f.key(Key::Space);
    assert_eq!(f.take(), vec![Request::Settings(SETTINGS_KEY.into())]);
    f.key(Key::Escape);
    assert_eq!(f.take(), vec![Request::Hide]);
    f.popup.invoke_focus_content();
    f.key(Key::Escape);
    assert_eq!(f.take(), vec![Request::Hide]);
}

#[test]
fn loading_blocks_profiles_but_retains_settings_and_action_pending_blocks_all_effects() {
    let f = Fixture::ready(3);
    f.popup.set_loading(true);
    f.render_fit(1.0);
    f.element("Loading keyboard profiles…", AccessibleRole::Text);
    for index in 0..3 {
        let row = f.button(&label(index));
        assert_eq!(row.accessible_enabled(), Some(false));
        f.click(&row);
        row.invoke_accessible_default_action();
    }
    assert!(f.take().is_empty());
    assert_eq!(f.footer().accessible_enabled(), Some(true));
    f.popup.invoke_focus_content();
    f.key(Key::Tab);
    f.key(Key::Return);
    assert_eq!(f.take(), vec![Request::Settings(SETTINGS_KEY.into())]);
    f.popup.set_action_pending(true);
    f.render_fit(1.0);
    f.element("Applying keyboard request…", AccessibleRole::Text);
    f.click(&f.footer());
    f.footer().invoke_accessible_default_action();
    f.key(Key::Space);
    f.key(Key::Return);
    assert!(f.take().is_empty());
    assert_eq!(f.footer().accessible_enabled(), Some(false));
}

#[test]
fn cancelled_space_and_held_return_cannot_revive_when_capabilities_reappear() {
    let f = Fixture::ready(1);
    f.popup.invoke_focus_content();
    f.key(Key::Tab);
    f.key_press(Key::Space);
    f.popup.set_loading(true);
    // Commit the disabled presentation on its event-loop turn, while the
    // physical Space remains down. Slint coalesces same-turn reverted setters.
    slint::platform::update_timers_and_animations();
    f.popup.set_loading(false);
    f.repeat(Key::Space);
    f.key_release(Key::Space);
    assert!(f.take().is_empty());
    f.key(Key::Space);
    assert_eq!(f.take(), vec![Request::Profile(row_key(0))]);
    f.key_press(Key::Return);
    assert_eq!(f.take(), vec![Request::Profile(row_key(0))]);
    f.popup.set_action_pending(true);
    slint::platform::update_timers_and_animations();
    f.popup.set_action_pending(false);
    f.repeat(Key::Return);
    f.key_release(Key::Return);
    assert!(f.take().is_empty());
    f.key(Key::Tab);
    f.key_press(Key::Space);
    f.popup.set_settings_key("".into());
    f.popup
        .set_settings_key("opaque::new-projection/settings".into());
    f.repeat(Key::Space);
    f.key_release(Key::Space);
    assert!(
        f.take().is_empty(),
        "revoked Settings key cancels an armed Space"
    );
    f.key(Key::Space);
    assert_eq!(
        f.take(),
        vec![Request::Settings("opaque::new-projection/settings".into())]
    );
}

#[test]
fn empty_unavailable_unknown_active_retry_and_footer_have_distinct_real_accessibility() {
    let f = Fixture::ready(0);
    f.element("No keyboard profiles are enabled.", AccessibleRole::Text);
    assert_eq!(f.buttons().len(), 1);
    f.popup.set_observed(false);
    f.popup
        .set_notice("Keyboard profile information is unavailable.".into());
    f.popup.set_retry_enabled(true);
    f.render_fit(1.0);
    f.element(
        "Keyboard profile information is unavailable.",
        AccessibleRole::Text,
    );
    f.popup.invoke_focus_content();
    f.key(Key::Tab);
    f.key(Key::Return);
    assert_eq!(f.take(), vec![Request::Retry]);
    f.key(Key::Tab);
    f.key(Key::Space);
    assert_eq!(f.take(), vec![Request::Settings(SETTINGS_KEY.into())]);
    f.popup.set_retry_enabled(false);
    f.popup.set_notice("".into());
    f.popup.set_observed(true);
    let mut observed = rows(3);
    for row in &mut observed {
        row.active = false;
    }
    f.popup.set_rows(ModelRc::new(VecModel::from(observed)));
    f.popup.set_unknown_active(true);
    f.render_fit(1.0);
    f.element("Active keyboard profile unavailable.", AccessibleRole::Text);
    for index in 0..3 {
        assert_eq!(
            f.button(&label(index)).accessible_item_selected(),
            Some(false)
        );
    }
}

#[test]
fn input_language_300px_body_source_row_metrics_and_theme_scale_matrix_render_without_idle_drawing()
{
    let f = Fixture::ready(3);
    for scheme in [
        slint::language::ColorScheme::Light,
        slint::language::ColorScheme::Dark,
        slint::language::ColorScheme::Unknown,
    ] {
        f.popup
            .apply_presentation_theme(PresentationTheme::seelen_reference(scheme));
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            f.scale(scale);
            let pixels = f.render_fit(scale);
            assert_eq!(f.popup.get_popup_content_width(), 320.0);
            let width = (320.0 * scale).ceil() as usize;
            let surface = rgb(f.popup.global::<SeelenPalette>().get_surface());
            assert_eq!(sample(&pixels, width, scale, 308.0, 40.0), surface);
            assert_ne!(sample(&pixels, width, scale, 0.0, 0.0), surface);
            let first = f.button(&label(0));
            let second = f.button(&label(1));
            // keyboard-selector.scss:21/42, reset.css:12: two inherited
            // 0.8rem lines, with 8px top/bottom padding, NOT font-size / 0.8.
            near(first.size().height, 51.84);
            // keyboard-selector.scss:14 and spacings.css:3: independent 8px gap.
            near(
                second.absolute_position().y - first.absolute_position().y,
                59.84,
            );
            near(
                second.absolute_position().y - first.absolute_position().y - first.size().height,
                8.0,
            );
            let icons = second
                .query_descendants()
                .match_type_name("Image")
                .find_all();
            assert_eq!(
                icons.len(),
                1,
                "one genuine original keyboard vector per profile"
            );
            let icon = &icons[0];
            let icon_origin = icon.absolute_position();
            near(icon.size().width, 20.0);
            near(icon.size().height, 16.0);
            near(icon_origin.x - second.absolute_position().x, 12.0);
            near(
                icon_origin.y - second.absolute_position().y,
                (51.84 - 16.0) / 2.0,
            );
            let native_icon = ItemRc::new_root(WindowInner::from_pub(f.popup.window()).component())
                .visit_descendants(|item| {
                    let absolute = item.map_to_window(item.geometry().origin);
                    if item.downcast::<ImageItem>().is_some()
                        && (absolute.x - icon_origin.x).abs() < 0.01
                        && (absolute.y - icon_origin.y).abs() < 0.01
                    {
                        ControlFlow::Break(item.clone())
                    } else {
                        ControlFlow::Continue(())
                    }
                })
                .expect("actual native keyboard ImageItem at its semantic row");
            let image = native_icon.downcast::<ImageItem>().unwrap();
            let image = image.as_pin_ref();
            let intrinsic = image.source().size();
            assert_eq!((intrinsic.width, intrinsic.height), (576, 512));
            near(image.width().get(), 20.0);
            near(image.height().get(), 16.0);
            let dark = f.popup.global::<SeelenPalette>().get_dark();
            let (body_gray, foreground) = if dark { (24, 228) } else { (242, 18) };
            assert_eq!(
                image.colorize(),
                slint::Color::from_rgb_u8(foreground, foreground, foreground,).into(),
                "actual native SVG colorize is the independent source foreground"
            );
            let mut visible_ink = 0;
            let mut full_ink = 0;
            for y in (icon_origin.y * scale).ceil() as usize
                ..((icon_origin.y + 16.0) * scale).floor() as usize
            {
                for x in (icon_origin.x * scale).ceil() as usize
                    ..((icon_origin.x + 20.0) * scale).floor() as usize
                {
                    let pixel = pixels[y * width + x];
                    assert_eq!(
                        pixel.r, pixel.g,
                        "original SVG uses a neutral foreground mask"
                    );
                    assert_eq!(pixel.g, pixel.b);
                    assert!(
                        (body_gray.min(foreground)..=body_gray.max(foreground)).contains(&pixel.r),
                        "SVG ROI contains only source foreground coverage over the inactive row"
                    );
                    visible_ink += usize::from(pixel.r != body_gray);
                    full_ink += usize::from(pixel.r == foreground);
                }
            }
            assert!(
                visible_ink > 5,
                "original keyboard SVG needs visible tinted ink"
            );
            if scale == 2.0 {
                assert!(
                    full_ink > 0,
                    "2x native SVG must contain exact literal foreground pixels"
                );
            }
            near(first.size().width, 284.0);
            near(first.absolute_position().x, 18.0); // 10px shadow + source 8px inset.
            let header = f.element("Keyboard layout", AccessibleRole::Text);
            near(header.absolute_position().x, 18.0);
            near(header.absolute_position().y, 18.0);
            near(header.size().height, 17.92);
            near(
                first.absolute_position().y - header.absolute_position().y - header.size().height,
                8.0,
            );
            let viewport = ElementHandle::find_by_element_id(&f.popup, "ScrollView::flickable")
                .next()
                .expect("genuine native selector scroll viewport");
            near(viewport.absolute_position().x, 14.0); // 10px shadow + 4px residual inset.
            near(viewport.absolute_position().y, 10.0 + 29.92);
            near(viewport.size().width, 292.0);
            // The preferred source extent stays fractional. PhysicalSize uses
            // ceil, so only the flexible native viewport receives this residue.
            let allocated_height = (265.76_f32 * scale).ceil() / scale;
            let allocation_residue = allocated_height - 265.76;
            near(
                viewport.size().height,
                2.0 * 4.0 + 3.0 * 51.84 + 2.0 * 8.0 + allocation_residue,
            );
            let third = f.button(&label(2));
            let footer = f.footer();
            near(
                footer.absolute_position().y - third.absolute_position().y - third.size().height,
                8.0 + allocation_residue,
            );
            near(footer.size().height, 24.32); // buttons.scss:2: 0.25em vertical padding.
            near(footer.size().width, 284.0);
            near(footer.absolute_position().x, 18.0);
            near(f.popup.get_popup_content_height() - 20.0, 245.76);
            near(
                allocated_height - 10.0 - footer.absolute_position().y - footer.size().height,
                8.0,
            );
            for content in [
                "Keyboard layout",
                &language(0),
                &description(0),
                "More keyboard settings",
            ] {
                assert_source_line(&f.popup, content);
            }
            let language_text = native_text(&f.popup, &language(0));
            let description_text = native_text(&f.popup, &description(0));
            near(language_text.geometry().origin.x, 40.0);
            near(language_text.geometry().origin.y, 8.0);
            near(description_text.geometry().origin.y, 8.0 + 17.92);
            let footer_text = native_text(&f.popup, "More keyboard settings");
            near(footer_text.geometry().origin.x, 6.4);
            near(footer_text.geometry().origin.y, 3.2);
            let footer_text = footer_text.downcast::<ComplexText>().unwrap();
            let footer_text = footer_text.as_pin_ref();
            near(footer_text.width().get(), 284.0 - 12.8);
            assert_eq!(
                footer_text.horizontal_alignment(),
                TextHorizontalAlignment::Center
            );
            assert!(
                !f.window
                    .draw_if_needed(|_| panic!("settled selector must not draw idle frames"))
            );
            ready(&f.popup, 1);
            f.render_fit(scale);
            near(f.popup.get_popup_content_height() - 20.0, 126.08);
            ready(&f.popup, 3);
            f.render_fit(scale);
        }
    }
}

#[test]
fn overflowing_profiles_use_real_scroll_and_keep_footer_reachable_in_tiny_native_viewport() {
    let f = Fixture::ready(40);
    near(f.popup.get_popup_content_height(), 720.0); // Source max700 body + 20 shadow.
    for scale in [1.0_f32, 2.0] {
        f.scale(scale);
        f.popup.invoke_focus_content();
        f.render((180.0 * scale) as u32, (160.0 * scale) as u32);
        let footer = f.footer();
        let footer_center = Fixture::center(&footer);
        assert!(footer_center.y < 160.0 && footer_center.x < 180.0);
        f.click(&footer);
        assert_eq!(f.take(), vec![Request::Settings(SETTINGS_KEY.into())]);
        f.popup.invoke_focus_content();
        for index in 0..40 {
            f.key(Key::Tab);
            let row = f.button(&label(index));
            assert_revealed_between_header_and_footer(&f, &row);
            f.key(Key::Return);
            assert_eq!(f.take(), vec![Request::Profile(row_key(index))]);
            f.click(&row);
            assert_eq!(f.take(), vec![Request::Profile(row_key(index))]);
        }
        f.key(Key::Tab);
        f.key(Key::Space);
        assert_eq!(f.take(), vec![Request::Settings(SETTINGS_KEY.into())]);
        f.popup.invoke_focus_content();
        f.event(WindowEvent::PointerScrolled {
            position: LogicalPosition::new(90.0, 80.0),
            delta_x: 0.0,
            delta_y: -120.0,
        });
        assert!(
            f.take().is_empty(),
            "scrolling installed profiles never activates one"
        );
        assert_eq!(f.footer().absolute_position(), footer.absolute_position());
    }
}

#[test]
fn long_localized_labels_elide_without_invented_fallbacks_or_footer_overlap() {
    let f = Fixture::ready(1);
    let natural_row_height = f.button(&label(0)).size().height;
    let name = "日本語 — العربية — שלום ".repeat(30);
    let layout = "Observed description (TIP) / клавиатура ".repeat(30);
    f.popup
        .set_rows(ModelRc::new(VecModel::from(vec![InputProfileRow {
            key: "opaque::long-profile".into(),
            language_name: name.clone().into(),
            layout_name: layout.clone().into(),
            active: false,
        }])));
    for scale in [1.0_f32, 1.25, 1.5, 2.0] {
        f.scale(scale);
        let pixels = f.render_fit(scale);
        let row = f.button(&format!("{name} - {layout}"));
        assert!(
            (row.size().height - natural_row_height).abs() < 0.01,
            "long labels elide on their original single-line metrics"
        );
        assert!(row.absolute_position().y + row.size().height <= f.footer().absolute_position().y);
        let width = (320.0 * scale).ceil() as usize;
        assert_eq!(
            sample(&pixels, width, scale, 308.0, 65.0),
            [242, 242, 242],
            "elided native labels never paint into the body gutter"
        );
        f.click(&row);
        assert_eq!(
            f.take(),
            vec![Request::Profile("opaque::long-profile".into())]
        );
    }
    f.popup
        .set_rows(ModelRc::new(VecModel::from(vec![InputProfileRow {
            key: "opaque::missing-metadata".into(),
            language_name: "".into(),
            layout_name: "".into(),
            active: false,
        }])));
    f.scale(1.0);
    f.render_fit(1.0);
    let unavailable = "Language name unavailable - Keyboard profile name unavailable";
    let row = f.button(unavailable);
    assert_eq!(row.accessible_label().as_deref(), Some(unavailable));
    assert!(row.size().width > 0.0 && row.size().height > 0.0);
    f.click(&row);
    assert_eq!(
        f.take(),
        vec![Request::Profile("opaque::missing-metadata".into())]
    );
}

#[test]
fn long_unavailable_feedback_scrolls_to_genuine_retry_without_hiding_settings_footer() {
    let f = Fixture::ready(0);
    let notice = "Keyboard profiles unavailable. ملف تعريف الإدخال غير متاح. ".repeat(60);
    f.popup.set_observed(false);
    f.popup.set_notice(notice.clone().into());
    f.popup.set_retry_enabled(true);
    f.render(240, 160);
    assert!(f.popup.get_popup_content_height() <= 720.0);
    assert_eq!(f.footer().accessible_enabled(), Some(true));
    f.popup.invoke_focus_content();
    f.key(Key::Tab);
    let retry = f.button("Retry keyboard profiles");
    assert_revealed_between_header_and_footer(&f, &retry);
    f.key(Key::Return);
    assert_eq!(f.take(), vec![Request::Retry]);
    f.click(&f.footer());
    assert_eq!(f.take(), vec![Request::Settings(SETTINGS_KEY.into())]);
}

#[test]
fn genuine_hover_press_and_keyboard_focus_paint_then_motion_off_settles() {
    let f = Fixture::ready(3);
    let baseline = f.render_fit(1.0);
    let row = f.button(&label(1));
    let center = Fixture::center(&row);
    f.hover(center);
    let hover = f.render_fit(1.0);
    assert_ne!(baseline, hover);
    f.press(center);
    let pressed = f.render_fit(1.0);
    assert_ne!(hover, pressed);
    f.release(center);
    assert_eq!(f.take(), vec![Request::Profile(row_key(1))]);
    f.hover(LogicalPosition::new(0.0, 0.0));
    f.popup.invoke_focus_content();
    f.key(Key::Tab);
    f.key(Key::Tab);
    let focused = f.render_fit(1.0);
    assert_ne!(baseline, focused);
    let motion = f.popup.global::<PopoverMotion>();
    motion.set_enabled(true);
    f.popup.invoke_set_presentation_opacity(0.0);
    motion.set_enabled(false);
    f.popup.invoke_set_presentation_opacity(1.0);
    f.render_fit(1.0);
    assert!(
        !f.window
            .draw_if_needed(|_| panic!("motion-off surface must settle"))
    );
    assert!(f.take().is_empty());
}

#[test]
fn source_active_tenth_overlay_and_secondary_footer_use_real_native_glyph_composition() {
    // Independent source Text on a separate owned software adapter, at the
    // exact production geometry/physical phase. No inferred glyph-mask math.
    for dark in [false, true] {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            std::thread::spawn(move || {
                let f = Fixture::ready(1);
                let scheme = if dark {
                    slint::language::ColorScheme::Dark
                } else {
                    slint::language::ColorScheme::Light
                };
                f.popup.apply_presentation_theme(PresentationTheme::seelen_reference(scheme));
                f.popup.global::<SeelenPalette>()
                    .set_accent(slint::Color::from_rgb_u8(64, 128, 192).into());
                f.scale(scale);
                let actual = f.render_fit(scale);
                let width = (320.0 * scale).ceil() as usize;
                let active = f.button(&label(0));
                let origin = active.absolute_position();
                let size = active.size();
                // shared/index.scss:35, independent literal .10, sampled away
                // from text/icon/corners; no readback of the tested overlay.
                let surface = if dark { 24 } else { 242 };
                let mut overlay = Rgb8Pixel::new(surface, surface, surface);
                overlay.blend(slint::Color::from_rgb_u8(64, 128, 192).with_alpha(0.10).into());
                assert_eq!(
                    sample(&actual, width, scale, origin.x + size.width - 6.0, origin.y + size.height / 2.0),
                    [overlay.r, overlay.g, overlay.b],
                );
                assert_eq!(active.accessible_item_selected(), Some(true));
                f.popup.invoke_focus_content();
                f.key(Key::Tab);
                let focused = f.render_fit(scale);
                assert_eq!(
                    sample(&focused, width, scale, origin.x + size.width - 6.0, origin.y + size.height / 2.0),
                    [overlay.r, overlay.g, overlay.b],
                    "observed selection is independent of the external focus ring",
                );
                f.hover(Fixture::center(&active));
                let hovered = f.render_fit(scale);
                let hover_surface = if dark { 18 } else { 232 };
                let mut hover_overlay = Rgb8Pixel::new(hover_surface, hover_surface, hover_surface);
                hover_overlay.blend(slint::Color::from_rgb_u8(64, 128, 192).with_alpha(0.10).into());
                assert_eq!(
                    sample(&hovered, width, scale, origin.x + size.width - 6.0, origin.y + size.height / 2.0),
                    [hover_overlay.r, hover_overlay.g, hover_overlay.b],
                    "native hover layers below, never replaces, observed active overlay",
                );
                assert_eq!(active.accessible_item_selected(), Some(true));
                f.hover(LogicalPosition::new(0.0, 0.0));
                f.popup.invoke_focus_content();
                let actual = f.render_fit(scale);
                let item = native_text(&f.popup, "More keyboard settings");
                let text = item.downcast::<ComplexText>().unwrap();
                let text = text.as_pin_ref();
                let caption = ComplexText::FIELD_OFFSETS.text().apply_pin(text);
                let production_caption = caption.get();
                // keyboard-selector.scss:57 and shared gray-600 role: literal
                // source foreground, not a palette getter or sampled glyph.
                let secondary = if dark { 186 } else { 61 };
                assert_eq!(text.color(), slint::Color::from_rgb_u8(secondary, secondary, secondary).into());
                caption.set("".into());
                let background = f.render_fit(scale);
                caption.set(production_caption.clone());
                let restored = f.render_fit(scale);
                assert_eq!(actual, restored, "reference capture restores exact native footer raster");
                let source = SourceTypographyText::new().unwrap();
                source.set_source_background(slint::Color::from_rgb_u8(surface, surface, surface));
                source.set_source_foreground(slint::Color::from_rgb_u8(secondary, secondary, secondary));
                source.set_source_weight(600);
                source.set_source_tracking(0.0);
                source.set_centered(true);
                source.set_caption(production_caption.clone());
                let geometry = item.geometry();
                let absolute = item.map_to_window(geometry.origin);
                source.set_text_x(absolute.x);
                source.set_text_y(absolute.y);
                source.set_text_width(text.width().get());
                source.show().unwrap();
                f.source_window.window().dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
                let window_size = f.window.window().size();
                f.source_window.set_size(window_size);
                f.source_window.request_redraw();
                let mut reference = vec![Rgb8Pixel::default(); actual.len()];
                assert!(f.source_window.draw_if_needed(|renderer| {
                    renderer.render(&mut reference, width);
                }));
                let mut glyph_pixels = 0;
                // Compare every pixel of the whole native line crop, including
                // blank pixels. Exact native shaping/composition is the oracle.
                for y in (absolute.y * scale).ceil() as usize
                    ..((absolute.y + 17.92) * scale).floor() as usize
                {
                    for x in (absolute.x * scale).ceil() as usize
                        ..((absolute.x + text.width().get()) * scale).floor() as usize
                    {
                        let offset = y * width + x;
                        glyph_pixels += usize::from(reference[offset] != background[offset]);
                        assert_eq!(actual[offset], reference[offset],
                            "independent source footer glyph dark={dark} scale={scale} at=({x},{y}) origin={absolute:?} text_geometry={geometry:?} caption={production_caption:?} blank={:?}",
                            background[offset]);
                    }
                }
                assert!(glyph_pixels > 30, "proof requires actual visible native footer glyphs");
                assert!(f.take().is_empty());
            }).join().unwrap();
        }
    }
}
