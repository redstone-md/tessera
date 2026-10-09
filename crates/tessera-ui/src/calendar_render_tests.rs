// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Generated CalendarMenu, genuine native input/AX and owned software pixels.
//! All labels, keys and state combinations below are explicit presentation
//! fixtures, not OS date/locale data. No host, clock mutation or OS action exists.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, TargetPixel};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, ModelRc, PhysicalSize, Rgb8Pixel, VecModel};

use crate::generated::{
    CalendarDayCell, CalendarMenu, CalendarMode, CalendarMonthCell, CalendarMonthRow,
    CalendarWeekRow, Palette, SeelenPalette,
};
use crate::theme::{PresentationTheme, ThemedComponent};

const ACTION_KEY: &str = "opaque::calendar/session-π::actions";
const WEEKDAYS: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn model<T: Clone + 'static>(values: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(values))
}

fn day_key(index: usize) -> String {
    format!("opaque::day/{index}::?語")
}

fn day_description(index: usize) -> String {
    format!("Fixture civil day {index}")
}

fn day(index: usize) -> CalendarDayCell {
    CalendarDayCell {
        key: day_key(index).into(),
        label: (index % 31 + 1).to_string().into(),
        description: day_description(index).into(),
        selected: matches!(index, 0 | 3),
        today: matches!(index, 1 | 3 | 4),
        off_month: matches!(index, 2..=4),
    }
}

fn weeks(count: usize) -> ModelRc<CalendarWeekRow> {
    model(
        (0..count)
            .map(|row| CalendarWeekRow {
                days: model((row * 7..row * 7 + 7).map(day).collect()),
            })
            .collect(),
    )
}

fn month_key(index: usize) -> String {
    format!("opaque::month/{index}::#א")
}

fn month_rows(labels: &[String; 12]) -> ModelRc<CalendarMonthRow> {
    model(
        (0..4)
            .map(|row| CalendarMonthRow {
                months: model(
                    (row * 3..row * 3 + 3)
                        .map(|index| CalendarMonthCell {
                            key: month_key(index).into(),
                            label: labels[index].clone().into(),
                            current: index == 1,
                        })
                        .collect(),
                ),
            })
            .collect(),
    )
}

fn set_ready(popup: &CalendarMenu, week_count: usize) {
    popup.set_title_text("Fixture February 2021".into());
    popup.set_action_key(ACTION_KEY.into());
    popup.set_can_previous(true);
    popup.set_can_next(true);
    popup.set_weekdays(model(
        WEEKDAYS.iter().map(|label| (*label).into()).collect(),
    ));
    popup.set_weeks(weeks(week_count));
    popup.set_month_rows(month_rows(&MONTHS.map(str::to_owned)));
}

#[derive(Debug, PartialEq)]
enum Request {
    Navigate(bool, String),
    Toggle(String),
    Today(String),
    Day(String),
    Month(String),
    Retry,
    Hide,
}

struct Fixture {
    window: Rc<MinimalSoftwareWindow>,
    popup: CalendarMenu,
    requests: Rc<RefCell<Vec<Request>>>,
}

impl Fixture {
    // Same owned software adapter as user_menu_render_tests: one generated
    // component per native test thread, no testing-only replacement window.
    fn new(configure: impl FnOnce(&CalendarMenu)) -> Self {
        struct TestPlatform(Rc<MinimalSoftwareWindow>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
        let popup = CalendarMenu::new().unwrap();
        popup.apply_presentation_theme(PresentationTheme::uniform(
            slint::language::ColorScheme::Light,
        ));
        let requests = Rc::new(RefCell::new(Vec::new()));
        let recorded = requests.clone();
        popup.on_navigate_requested(move |forward, key| {
            recorded
                .borrow_mut()
                .push(Request::Navigate(forward, key.to_string()));
        });
        let recorded = requests.clone();
        popup.on_toggle_view_requested(move |key| {
            recorded.borrow_mut().push(Request::Toggle(key.to_string()));
        });
        let recorded = requests.clone();
        popup.on_today_requested(move |key| {
            recorded.borrow_mut().push(Request::Today(key.to_string()));
        });
        let recorded = requests.clone();
        popup.on_day_selected(move |key| {
            recorded.borrow_mut().push(Request::Day(key.to_string()));
        });
        let recorded = requests.clone();
        popup.on_month_selected(move |key| {
            recorded.borrow_mut().push(Request::Month(key.to_string()));
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

    fn ready(count: usize) -> Self {
        Self::new(|popup| set_ready(popup, count))
    }

    // Raw label queries include paint-only Text. Drop ONLY missing/None roles:
    // any duplicate genuine Button/Text remains visible and fails uniqueness.
    fn labeled(&self, label: &str) -> Vec<ElementHandle> {
        ElementHandle::find_by_accessible_label(&self.popup, label)
            .filter(|element| {
                matches!(element.accessible_role(), Some(role) if role != AccessibleRole::None)
            })
            .collect()
    }

    fn element(&self, label: &str) -> ElementHandle {
        let matches = self.labeled(label);
        let diagnostics = matches
            .iter()
            .map(|element| {
                (
                    element.accessible_role(),
                    element.id(),
                    element.absolute_position(),
                    element.size(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            matches.len(),
            1,
            "unique semantic node required for {label}: {diagnostics:?}"
        );
        matches.into_iter().next().unwrap()
    }

    fn buttons(&self) -> Vec<ElementHandle> {
        ElementQuery::from_root(&self.popup)
            .match_accessible_role(AccessibleRole::Button)
            .find_all()
    }
    fn geometry_diagnostics(&self) -> String {
        let buttons = self.buttons();
        let nodes = buttons
            .iter()
            .take(46)
            .map(|element| {
                (
                    element.accessible_label(),
                    element.accessible_role(),
                    element.absolute_position(),
                    element.size(),
                )
            })
            .collect::<Vec<_>>();
        format!(
            "mode={:?} scale={} physical_window={:?} preferred={}x{} weeks={} month_rows={} buttons={} semantic_nodes(label,role,origin,size)={nodes:?}",
            self.popup.get_view_mode(),
            self.window.window().scale_factor(),
            self.window.window().size(),
            self.popup.get_popup_content_width(),
            self.popup.get_popup_content_height(),
            self.popup.get_weeks().row_count(),
            self.popup.get_month_rows().row_count(),
            buttons.len(),
        )
    }

    fn render_fit(&self, scale: f32) -> Vec<Rgb8Pixel> {
        let width = (self.popup.get_popup_content_width() * scale).ceil() as u32;
        // Width-dependent square rows must be measured at the intended width.
        // This is one ordered native resize/measurement, not snapshot polling.
        self.window.set_size(PhysicalSize::new(
            width,
            (self.popup.get_popup_content_height() * scale).ceil() as u32,
        ));
        self.render(
            width,
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

    // Native font400 glyphs need not contain any fully covered pixel. Capture
    // the same glyph with two contrasting fixture palettes and glyph-free
    // backgrounds, then project their exact SDK coverage onto the actual paint.
    // Selected/today flags, native font weights, keys, bounds and pointer stay
    // fixed. References clear only off-month (which does not affect the font),
    // retaining the independent full-opacity font400 proof for muted text.
    fn assert_day_mask(
        &self,
        actual: &[Rgb8Pixel],
        width: usize,
        scale: f32,
        index: usize,
        foreground: [u8; 3],
    ) {
        let row = self.popup.get_weeks().row_data(index / 7).unwrap();
        let original = row.days.row_data(index % 7).unwrap();
        let element = self.element(&day_description(index));
        let origin = element.absolute_position();
        let size = element.size();
        let id = element.id();
        let mut blank = original.clone();
        blank.label = "".into();
        let mut reference_day = original.clone();
        reference_day.off_month = false;
        let mut reference_blank = reference_day.clone();
        reference_blank.label = "".into();
        let capture = |cell: CalendarDayCell| {
            row.days.set_row_data(index % 7, cell);
            let pixels = self.render_fit(scale);
            let current = self.element(&day_description(index));
            assert_eq!(current.id(), id, "reference must keep the genuine day node");
            assert_eq!(current.absolute_position(), origin);
            assert_eq!(current.size(), size);
            assert_eq!(current.accessible_role(), Some(AccessibleRole::Button));
            assert_eq!(pixels.len(), actual.len());
            pixels
        };
        let background = capture(blank.clone());
        let theme = self.popup.presentation_theme();
        let accent = self.popup.global::<SeelenPalette>().get_accent();
        let mut references = Vec::new();
        for (scheme, accent, selected, today, normal) in [
            (slint::language::ColorScheme::Light, 0, 255, 0, 18),
            (slint::language::ColorScheme::Dark, 255, 0, 255, 228),
        ] {
            self.popup
                .apply_presentation_theme(PresentationTheme::uniform(scheme));
            self.popup
                .global::<SeelenPalette>()
                .set_accent(slint::Color::from_rgb_u8(accent, accent, accent).into());
            // These are independent source constants, not property getters or
            // colors sampled from the glyph being tested.
            let reference_foreground = if original.selected {
                selected
            } else if original.today {
                today
            } else {
                normal
            };
            let glyph = capture(reference_day.clone());
            let base = capture(reference_blank.clone());
            references.push((glyph, base, reference_foreground));
        }
        self.popup.apply_presentation_theme(theme);
        self.popup.global::<SeelenPalette>().set_accent(accent);
        let restored = capture(original);
        assert!(
            restored.as_slice() == actual,
            "restoring fixture labels/flags/palettes must restore the exact same calendar raster"
        );
        let mut glyph_pixels = 0;
        let mut visible_pixels = 0;
        for y in
            (origin.y * scale).ceil() as usize..((origin.y + size.height) * scale).floor() as usize
        {
            for x in (origin.x * scale).ceil() as usize
                ..((origin.x + size.width) * scale).floor() as usize
            {
                let offset = y * width + x;
                let base = background[offset];
                let painted = actual[offset];
                if references
                    .iter()
                    .all(|(glyph, background, _)| glyph[offset] == background[offset])
                {
                    assert_eq!(
                        painted, base,
                        "day recolor cannot add glyph/background pixels at ({x},{y})"
                    );
                    continue;
                }
                glyph_pixels += 1;
                visible_pixels += usize::from(painted != base);
                // Quantization may leave several exact coverage candidates.
                // Intersect both independent native references, then require
                // the actual RGB to equal the SDK blend of the expected source
                // foreground over its own glyph-free background. Rounded fills
                // and hover overlays are thus preserved, not treated as glyphs.
                let matches = (0..=u8::MAX).any(|coverage| {
                    let reference_matches = references.iter().all(|(glyph, base, color)| {
                        let mut expected = base[offset];
                        expected.blend(
                            slint::Color::from_argb_u8(coverage, *color, *color, *color).into(),
                        );
                        expected == glyph[offset]
                    });
                    let mut expected = base;
                    expected.blend(
                        slint::Color::from_argb_u8(
                            coverage,
                            foreground[0],
                            foreground[1],
                            foreground[2],
                        )
                        .into(),
                    );
                    reference_matches && expected == painted
                });
                assert!(
                    matches,
                    "day {index} must paint its priority foreground with the same native glyph coverage at ({x},{y}) scale={scale}: actual={painted:?} glyph_free_background={base:?}; {}",
                    paint_diagnostics(
                        actual,
                        width,
                        scale,
                        &element,
                        foreground,
                        [base.r, base.g, base.b],
                    ),
                );
            }
        }
        assert!(
            glyph_pixels > 0 && visible_pixels > 0,
            "independent references and actual day {index} must contain a real visible native glyph"
        );
    }

    fn scale(&self, scale: f32) {
        self.window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
    }

    fn center(element: &ElementHandle) -> LogicalPosition {
        let origin = element.absolute_position();
        let size = element.size();
        assert!(size.width > 0.0 && size.height > 0.0);
        LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
    }

    fn hover(&self, position: LogicalPosition) {
        self.window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
    }

    fn press(&self, position: LogicalPosition) {
        let size = self.window.window().size();
        let scale = self.window.window().scale_factor();
        assert!(
            position.x >= 0.0
                && position.y >= 0.0
                && position.x * scale < size.width as f32
                && position.y * scale < size.height as f32
        );
        self.hover(position);
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

    fn wheel(&self, position: LogicalPosition, delta_y: f32) {
        self.window
            .window()
            .dispatch_event(WindowEvent::PointerScrolled {
                position,
                delta_x: 0.0,
                delta_y,
            });
    }

    fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }
}

fn rgb(color: slint::Color) -> [u8; 3] {
    let color = color.to_argb_u8();
    [color.red, color.green, color.blue]
}

fn sample(pixels: &[Rgb8Pixel], width: usize, scale: f32, x: f32, y: f32) -> [u8; 3] {
    let pixel = pixels[(y * scale) as usize * width + (x * scale) as usize];
    [pixel.r, pixel.g, pixel.b]
}

// Full-coverage glyph pixels provide a real raster assertion without font
// snapshots, synthetic element receipts or antialiasing error tolerances.
fn ink(
    pixels: &[Rgb8Pixel],
    width: usize,
    scale: f32,
    element: &ElementHandle,
    color: [u8; 3],
) -> Vec<(usize, usize)> {
    let origin = element.absolute_position();
    let size = element.size();
    let mut points = Vec::new();
    for y in (origin.y * scale).ceil() as usize..((origin.y + size.height) * scale).floor() as usize
    {
        for x in
            (origin.x * scale).ceil() as usize..((origin.x + size.width) * scale).floor() as usize
        {
            let pixel = pixels[y * width + x];
            if [pixel.r, pixel.g, pixel.b] == color {
                points.push((x, y));
            }
        }
    }
    points
}

// Bounded failure-only diagnostics: actual raster histogram, nearest color and
// nonbackground glyph bounds. These do not relax foreground/pixel assertions.
fn paint_diagnostics(
    pixels: &[Rgb8Pixel],
    width: usize,
    scale: f32,
    element: &ElementHandle,
    expected: [u8; 3],
    background: [u8; 3],
) -> String {
    let origin = element.absolute_position();
    let size = element.size();
    let mut colors = BTreeMap::<[u8; 3], usize>::new();
    let mut glyph_bounds: Option<(usize, usize, usize, usize)> = None;
    let x_range =
        (origin.x * scale).ceil() as usize..((origin.x + size.width) * scale).floor() as usize;
    let y_range =
        (origin.y * scale).ceil() as usize..((origin.y + size.height) * scale).floor() as usize;
    for y in y_range.clone() {
        for x in x_range.clone() {
            let pixel = pixels[y * width + x];
            let color = [pixel.r, pixel.g, pixel.b];
            *colors.entry(color).or_default() += 1;
            if color != background {
                glyph_bounds = Some(match glyph_bounds {
                    Some((left, top, right, bottom)) => {
                        (left.min(x), top.min(y), right.max(x), bottom.max(y))
                    }
                    None => (x, y, x, y),
                });
            }
        }
    }
    let nearest = colors
        .iter()
        .min_by_key(|(color, _)| {
            color
                .iter()
                .zip(expected)
                .map(|(actual, expected)| u32::from(actual.abs_diff(expected)))
                .sum::<u32>()
        })
        .map(|(color, count)| (*color, *count));
    let distinct = colors.len();
    let mut histogram = colors.into_iter().collect::<Vec<_>>();
    histogram.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    histogram.truncate(24);
    format!(
        "expected_foreground={expected:?} background={background:?} role={:?} label={:?} logical_origin={origin:?} logical_size={size:?} raster_roi=({x_range:?},{y_range:?}) nonbackground_glyph_bbox={glyph_bounds:?} distinct_colors={distinct} nearest_actual_color_and_count={nearest:?} most_frequent_colors_and_counts={histogram:?}",
        element.accessible_role(),
        element.accessible_label(),
    )
}

#[test]
fn calendar_four_five_six_square_weeks_and_three_by_four_year_have_source_geometry_at_both_themes_and_scales()
 {
    let fixture = Fixture::ready(4);
    for (scheme, background) in [
        (slint::language::ColorScheme::Light, [242, 242, 242]),
        (slint::language::ColorScheme::Dark, [24, 24, 24]),
    ] {
        fixture
            .popup
            .apply_presentation_theme(PresentationTheme::uniform(scheme));
        for scale in [1.0_f32, 2.0] {
            fixture.scale(scale);
            for count in [4_usize, 5, 6] {
                fixture.popup.set_view_mode(CalendarMode::Month);
                fixture.popup.set_weeks(weeks(count));
                let pixels = fixture.render_fit(scale);
                assert_eq!(fixture.popup.get_popup_content_width(), 320.0);
                let cell = (284.0_f32 - 6.0 * 4.0) / 7.0;
                let expected_height = 20.0
                    + 16.0
                    + 25.92
                    + 16.0
                    + 33.92
                    + 8.0
                    + count as f32 * cell
                    + (count - 1) as f32 * 4.0;
                assert_eq!(
                    (fixture.popup.get_popup_content_height() * scale).ceil(),
                    (expected_height * scale).ceil(),
                    "scheme={scheme:?} requested_weeks={count} expected_logical_height={expected_height}; {}",
                    fixture.geometry_diagnostics()
                );
                let width = (320.0 * scale) as usize;
                assert_eq!(sample(&pixels, width, scale, 308.0, 40.0), background);
                assert_ne!(sample(&pixels, width, scale, 0.0, 0.0), background);
                for label in WEEKDAYS {
                    assert_eq!(
                        fixture.element(label).accessible_role(),
                        Some(AccessibleRole::Text)
                    );
                }
                let title = fixture.element("Toggle calendar view");
                assert_eq!((title.size().height * scale).ceil(), (25.92 * scale).ceil());
                let navigators =
                    ["Previous month", "Today", "Next month"].map(|label| fixture.element(label));
                for navigator in &navigators {
                    assert_eq!(
                        (navigator.size().width * scale).ceil(),
                        (28.8 * scale).ceil()
                    );
                    assert_eq!(
                        (navigator.size().height * scale).ceil(),
                        (22.4 * scale).ceil()
                    );
                }
                for pair in navigators.windows(2) {
                    assert_eq!(
                        ((pair[1].absolute_position().x - pair[0].absolute_position().x) * scale)
                            .round(),
                        ((28.8 + 4.0) * scale).round(),
                        "source navigator gap stays four logical pixels"
                    );
                }
                for index in 0..count * 7 {
                    let element = fixture.element(&day_description(index));
                    assert_eq!(element.accessible_role(), Some(AccessibleRole::Button));
                    assert_eq!(
                        element.size().width,
                        element.size().height,
                        "day {index} must be square"
                    );
                    // Compare actual physical layout cells, not invented fuzzy
                    // logical tolerances around the fractional seven-column grid.
                    assert_eq!((element.size().width * scale).ceil(), (cell * scale).ceil());
                    let origin = element.absolute_position();
                    assert_eq!(
                        (origin.x * scale).floor(),
                        ((18.0 + (index % 7) as f32 * (cell + 4.0)) * scale).floor()
                    );
                    assert_eq!(
                        (origin.y * scale).floor(),
                        ((101.84 + (index / 7) as f32 * (cell + 4.0)) * scale).floor()
                    );
                }
                assert_eq!(fixture.buttons().len(), 4 + count * 7);
                assert!(fixture.take_requests().is_empty());
                assert!(
                    !fixture.window.draw_if_needed(|renderer| {
                        let mut unchanged = pixels.clone();
                        renderer.render(&mut unchanged, width);
                    }),
                    "settled calendar has no idle drawing"
                );
            }
            fixture.popup.set_view_mode(CalendarMode::Year);
            fixture.popup.set_title_text("2021".into());
            let pixels = fixture.render_fit(scale);
            let year_height = 20.0_f32 + 16.0 + 25.92 + 16.0 + 8.0 + 4.0 * 49.92 + 3.0 * 8.0;
            assert_eq!(
                (fixture.popup.get_popup_content_height() * scale).ceil(),
                (year_height * scale).ceil()
            );
            let cell_width = (284.0_f32 - 8.0 - 2.0 * 8.0) / 3.0;
            for (index, label) in MONTHS.iter().enumerate() {
                let element = fixture.element(label);
                assert_eq!(element.accessible_role(), Some(AccessibleRole::Button));
                assert_eq!(
                    (element.size().width * scale).ceil(),
                    (cell_width * scale).ceil()
                );
                assert_eq!(
                    (element.size().height * scale).ceil(),
                    (49.92 * scale).ceil()
                );
                let origin = element.absolute_position();
                assert_eq!(
                    (origin.x * scale).floor(),
                    ((22.0 + (index % 3) as f32 * (cell_width + 8.0)) * scale).floor()
                );
                assert_eq!(
                    (origin.y * scale).floor(),
                    ((63.92 + (index / 3) as f32 * 57.92) * scale).floor()
                );
            }
            assert_eq!(fixture.buttons().len(), 16);
            assert!(fixture.labeled(&day_description(0)).is_empty());
            assert_eq!(
                sample(&pixels, (320.0 * scale) as usize, scale, 308.0, 40.0),
                background
            );
            assert!(fixture.take_requests().is_empty());
        }
    }
}

#[test]
fn calendar_pointer_and_accessibility_dispatch_exact_current_opaque_keys_without_optimistic_projection()
 {
    let fixture = Fixture::ready(4);
    for (label, expected) in [
        (
            "Toggle calendar view",
            Request::Toggle(ACTION_KEY.to_owned()),
        ),
        (
            "Previous month",
            Request::Navigate(false, ACTION_KEY.to_owned()),
        ),
        ("Today", Request::Today(ACTION_KEY.to_owned())),
        ("Next month", Request::Navigate(true, ACTION_KEY.to_owned())),
    ] {
        let element = fixture.element(label);
        assert_eq!(element.accessible_enabled(), Some(true));
        fixture.hover(Fixture::center(&element));
        assert!(fixture.take_requests().is_empty());
        fixture.click(&element);
        assert_eq!(fixture.take_requests(), vec![expected]);
    }
    for index in 0..28 {
        let element = fixture.element(&day_description(index));
        fixture.click(&element);
        assert_eq!(fixture.take_requests(), vec![Request::Day(day_key(index))]);
        element.invoke_accessible_default_action();
        assert_eq!(fixture.take_requests(), vec![Request::Day(day_key(index))]);
    }
    assert_eq!(fixture.popup.get_view_mode(), CalendarMode::Month);
    assert_eq!(
        fixture.popup.get_title_text().as_str(),
        "Fixture February 2021"
    );
    for (label, expected) in [
        (
            "Toggle calendar view",
            Request::Toggle(ACTION_KEY.to_owned()),
        ),
        (
            "Previous month",
            Request::Navigate(false, ACTION_KEY.to_owned()),
        ),
        ("Today", Request::Today(ACTION_KEY.to_owned())),
        ("Next month", Request::Navigate(true, ACTION_KEY.to_owned())),
    ] {
        fixture.element(label).invoke_accessible_default_action();
        assert_eq!(fixture.take_requests(), vec![expected]);
    }
    fixture.popup.set_view_mode(CalendarMode::Year);
    fixture.render_fit(1.0);
    assert!(fixture.labeled("Previous month").is_empty());
    assert!(fixture.labeled("Next month").is_empty());
    for (index, label) in MONTHS.iter().enumerate() {
        let element = fixture.element(label);
        fixture.click(&element);
        assert_eq!(
            fixture.take_requests(),
            vec![Request::Month(month_key(index))]
        );
        element.invoke_accessible_default_action();
        assert_eq!(
            fixture.take_requests(),
            vec![Request::Month(month_key(index))]
        );
    }
    fixture
        .element("Previous year")
        .invoke_accessible_default_action();
    fixture
        .element("Next year")
        .invoke_accessible_default_action();
    assert_eq!(
        fixture.take_requests(),
        vec![
            Request::Navigate(false, ACTION_KEY.to_owned()),
            Request::Navigate(true, ACTION_KEY.to_owned())
        ]
    );
    assert_eq!(
        fixture.popup.get_view_mode(),
        CalendarMode::Year,
        "presentation does not select months on its own"
    );
}

#[test]
fn calendar_tab_enter_space_escape_and_wheel_are_real_input_with_source_direction() {
    let fixture = Fixture::ready(4);
    fixture.popup.invoke_focus_content();
    for expected in [
        Request::Toggle(ACTION_KEY.to_owned()),
        Request::Navigate(false, ACTION_KEY.to_owned()),
        Request::Today(ACTION_KEY.to_owned()),
        Request::Navigate(true, ACTION_KEY.to_owned()),
        Request::Day(day_key(0)),
    ] {
        fixture.key(Key::Tab);
        assert!(fixture.take_requests().is_empty());
        fixture.key_press(Key::Return);
        assert_eq!(fixture.take_requests(), vec![expected]);
        fixture.key_release(Key::Return);
        assert!(
            fixture.take_requests().is_empty(),
            "Enter release is not a second activation"
        );
    }
    fixture.key(Key::Tab);
    fixture.key_press(Key::Space);
    assert!(fixture.take_requests().is_empty(), "Space arms on press");
    fixture.key_release(Key::Space);
    assert_eq!(fixture.take_requests(), vec![Request::Day(day_key(1))]);
    fixture.key(Key::Escape);
    assert_eq!(fixture.take_requests(), vec![Request::Hide]);
    fixture.popup.invoke_focus_content();
    fixture.key(Key::Escape);
    assert_eq!(fixture.take_requests(), vec![Request::Hide]);
    for mode in [CalendarMode::Month, CalendarMode::Year] {
        fixture.popup.set_view_mode(mode);
        fixture.render_fit(1.0);
        let label = if mode == CalendarMode::Month {
            day_description(5)
        } else {
            MONTHS[5].to_owned()
        };
        let position = Fixture::center(&fixture.element(&label));
        fixture.wheel(position, 120.0);
        fixture.wheel(position, -120.0);
        assert_eq!(
            fixture.take_requests(),
            vec![
                Request::Navigate(true, ACTION_KEY.to_owned()),
                Request::Navigate(false, ACTION_KEY.to_owned())
            ],
            "wheel up advances, down retreats"
        );
        fixture.wheel(position, 0.0);
        assert!(fixture.take_requests().is_empty());
    }
}

#[test]
fn calendar_loading_errors_empty_keys_and_navigation_bounds_block_actions_but_keep_genuine_retry() {
    let fixture = Fixture::new(|popup| popup.set_loading(true));
    assert!(
        fixture.popup.get_title_text().is_empty(),
        "default UI does not invent a date title"
    );
    assert_eq!(
        fixture.element("Loading calendar…").accessible_role(),
        Some(AccessibleRole::Text)
    );
    assert!(fixture.labeled(&day_description(0)).is_empty());
    assert!(fixture.labeled(MONTHS[0]).is_empty());
    assert!(fixture.labeled(WEEKDAYS[0]).is_empty());
    assert!(
        fixture
            .buttons()
            .iter()
            .all(|button| button.accessible_enabled() == Some(false))
    );
    set_ready(&fixture.popup, 4);
    fixture.render_fit(1.0);
    for loading in [true, false] {
        fixture.popup.set_loading(loading);
        fixture
            .popup
            .set_action_key(if loading { ACTION_KEY } else { "" }.into());
        fixture.render_fit(1.0);
        for mode in [CalendarMode::Month, CalendarMode::Year] {
            fixture.popup.set_view_mode(mode);
            fixture.render_fit(1.0);
            for button in fixture.buttons() {
                assert_eq!(button.accessible_enabled(), Some(false));
                fixture.click(&button);
                button.invoke_accessible_default_action();
            }
            fixture.popup.invoke_focus_content();
            fixture.key(Key::Tab);
            fixture.key(Key::Return);
            fixture.key(Key::Space);
            fixture.wheel(LogicalPosition::new(80.0, 120.0), 120.0);
            assert!(fixture.take_requests().is_empty());
        }
    }
    fixture.popup.set_view_mode(CalendarMode::Month);
    fixture.popup.set_action_key(ACTION_KEY.into());
    fixture.popup.set_can_previous(false);
    fixture.popup.set_can_next(false);
    let mut first = (0..7).map(day).collect::<Vec<_>>();
    first[0].key = "".into();
    fixture
        .popup
        .set_weeks(model(vec![CalendarWeekRow { days: model(first) }]));
    fixture.render_fit(1.0);
    for label in ["Previous month", "Next month", &day_description(0)] {
        let button = fixture.element(label);
        assert_eq!(button.accessible_enabled(), Some(false));
        fixture.click(&button);
        button.invoke_accessible_default_action();
    }
    fixture.wheel(
        Fixture::center(&fixture.element(&day_description(1))),
        120.0,
    );
    fixture.wheel(
        Fixture::center(&fixture.element(&day_description(1))),
        -120.0,
    );
    assert!(fixture.take_requests().is_empty());
    fixture.popup.invoke_focus_content();
    for expected in [
        Request::Toggle(ACTION_KEY.to_owned()),
        Request::Today(ACTION_KEY.to_owned()),
        Request::Day(day_key(1)),
    ] {
        fixture.key(Key::Tab);
        fixture.key(Key::Return);
        assert_eq!(
            fixture.take_requests(),
            vec![expected],
            "Tab skips disabled nav and keyless day"
        );
    }
    fixture.key_press(Key::Space);
    fixture.popup.set_loading(true);
    fixture.key_release(Key::Space);
    assert!(
        fixture.take_requests().is_empty(),
        "loading cancels an armed Space gesture"
    );
    fixture.popup.set_loading(false);
    fixture.popup.set_view_mode(CalendarMode::Year);
    fixture.popup.set_month_rows(model(vec![CalendarMonthRow {
        months: model(vec![
            CalendarMonthCell {
                key: "".into(),
                label: "Keyless fixture month".into(),
                current: false,
            },
            CalendarMonthCell {
                key: month_key(1).into(),
                label: "Ready fixture month".into(),
                current: true,
            },
        ]),
    }]));
    fixture.render_fit(1.0);
    let keyless = fixture.element("Keyless fixture month");
    assert_eq!(keyless.accessible_enabled(), Some(false));
    fixture.click(&keyless);
    keyless.invoke_accessible_default_action();
    assert!(fixture.take_requests().is_empty());
    fixture.popup.invoke_focus_content();
    for expected in [
        Request::Toggle(ACTION_KEY.to_owned()),
        Request::Today(ACTION_KEY.to_owned()),
        Request::Month(month_key(1)),
    ] {
        fixture.key(Key::Tab);
        fixture.key(Key::Space);
        assert_eq!(
            fixture.take_requests(),
            vec![expected],
            "Tab skips keyless month and blocked navigation"
        );
    }
    fixture.popup.set_view_mode(CalendarMode::Month);
    fixture.popup.set_action_key("".into());
    fixture.popup.set_weeks(ModelRc::default());
    fixture.popup.set_weekdays(ModelRc::default());
    fixture.popup.set_month_rows(ModelRc::default());
    let notice = "Calendar is unavailable. Retry to read the native date and locale.";
    fixture.popup.set_notice(notice.into());
    fixture.popup.set_retry_enabled(true);
    fixture.render_fit(1.0);
    assert_eq!(
        fixture.element(notice).accessible_role(),
        Some(AccessibleRole::Text)
    );
    let retry = fixture.element("Retry calendar");
    assert_eq!(retry.accessible_enabled(), Some(true));
    fixture.click(&retry);
    assert_eq!(fixture.take_requests(), vec![Request::Retry]);
    retry.invoke_accessible_default_action();
    assert_eq!(fixture.take_requests(), vec![Request::Retry]);
    fixture.popup.invoke_focus_content();
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Retry],
        "Retry works without date-action authority"
    );
    fixture.key(Key::Space);
    assert_eq!(fixture.take_requests(), vec![Request::Retry]);
    fixture.popup.set_loading(true);
    fixture.render_fit(1.0);
    let retry = fixture.element("Retry calendar");
    assert_eq!(retry.accessible_enabled(), Some(false));
    fixture.click(&retry);
    retry.invoke_accessible_default_action();
    fixture.key(Key::Space);
    assert!(fixture.take_requests().is_empty());
    fixture.popup.set_loading(false);
    fixture.popup.set_retry_enabled(false);
    fixture.render_fit(1.0);
    assert!(fixture.labeled("Retry calendar").is_empty());
}

#[test]
fn calendar_selected_today_and_off_month_priorities_are_real_pixels_not_shared_button_selection() {
    let fixture = Fixture::ready(4);
    let accent = slint::Color::from_rgb_u8(64, 128, 192);
    for (scheme, background, foreground, muted) in [
        (
            slint::language::ColorScheme::Light,
            [242, 242, 242],
            [18, 18, 18],
            [111, 111, 111],
        ),
        (
            slint::language::ColorScheme::Dark,
            [24, 24, 24],
            [228, 228, 228],
            [137, 137, 137],
        ),
    ] {
        fixture
            .popup
            .apply_presentation_theme(PresentationTheme::uniform(scheme));
        fixture
            .popup
            .global::<SeelenPalette>()
            .set_accent(accent.into());
        let inverse = if scheme == slint::language::ColorScheme::Dark {
            [0, 0, 0]
        } else {
            [255, 255, 255]
        };
        assert_eq!(
            rgb(fixture
                .popup
                .global::<Palette>()
                .get_accent_foreground()
                .color()),
            inverse,
            "selected glyphs use the source Fluent inverse foreground"
        );
        for scale in [1.0_f32, 2.0] {
            fixture.scale(scale);
            fixture.hover(LogicalPosition::new(0.0, 0.0));
            let pixels = fixture.render_fit(scale);
            let width = (320.0 * scale) as usize;
            for (index, text_color, fill) in [
                (0, inverse, rgb(accent)),
                (1, rgb(accent), background),
                (2, muted, background),
                (3, inverse, rgb(accent)),
                (4, rgb(accent), background),
                (5, foreground, background),
            ] {
                let element = fixture.element(&day_description(index));
                let origin = element.absolute_position();
                assert_eq!(
                    sample(&pixels, width, scale, origin.x + 10.0, origin.y + 5.0),
                    fill
                );
                fixture.assert_day_mask(&pixels, width, scale, index, text_color);
                if matches!(index, 0 | 3) {
                    assert_eq!(
                        sample(
                            &pixels,
                            width,
                            scale,
                            origin.x - 2.0,
                            origin.y + element.size().height / 2.0
                        ),
                        background,
                        "semantic selection must not add TileButton's external focus outline"
                    );
                }
            }
            for index in [0, 3] {
                let element = fixture.element(&day_description(index));
                fixture.hover(Fixture::center(&element));
                let hovered = fixture.render_fit(scale);
                let origin = element.absolute_position();
                let hover_color = if scheme == slint::language::ColorScheme::Dark {
                    accent.brighter(0.2)
                } else {
                    accent.darker(0.2)
                };
                assert_eq!(
                    sample(&hovered, width, scale, origin.x + 10.0, origin.y + 5.0),
                    rgb(hover_color)
                );
                fixture.assert_day_mask(&hovered, width, scale, index, inverse);
            }
            for index in [1, 2, 4, 5] {
                let element = fixture.element(&day_description(index));
                fixture.hover(Fixture::center(&element));
                let hovered = fixture.render_fit(scale);
                let origin = element.absolute_position();
                let actual = sample(&hovered, width, scale, origin.x + 10.0, origin.y + 5.0);
                let mut expected = Rgb8Pixel::new(background[0], background[1], background[2]);
                expected.blend(slint::Color::from_argb_u8(51, 64, 128, 192).into());
                assert_eq!(
                    actual,
                    [expected.r, expected.g, expected.b],
                    "unselected hover is exactly a 20% accent overlay using the SDK's native compositor"
                );
                let text = if matches!(index, 1 | 4) {
                    rgb(accent)
                } else if index == 2 {
                    muted
                } else {
                    foreground
                };
                fixture.assert_day_mask(&hovered, width, scale, index, text);
            }
            assert!(
                fixture.take_requests().is_empty(),
                "hover/theme/DPI never selects dates"
            );
            fixture.popup.set_view_mode(CalendarMode::Year);
            fixture.hover(LogicalPosition::new(0.0, 0.0));
            let pixels = fixture.render_fit(scale);
            for (index, text) in [(0, foreground), (1, rgb(accent))] {
                let element = fixture.element(MONTHS[index]);
                let origin = element.absolute_position();
                assert_eq!(
                    sample(&pixels, width, scale, origin.x + 10.0, origin.y + 5.0),
                    background,
                    "current month is accent text, not a fabricated selected month"
                );
                assert!(!ink(&pixels, width, scale, &element, text).is_empty());
                fixture.hover(Fixture::center(&element));
                let hovered = fixture.render_fit(scale);
                let mut expected = Rgb8Pixel::new(background[0], background[1], background[2]);
                expected.blend(slint::Color::from_argb_u8(51, 64, 128, 192).into());
                assert_eq!(
                    sample(&hovered, width, scale, origin.x + 10.0, origin.y + 5.0),
                    [expected.r, expected.g, expected.b]
                );
                assert!(!ink(&hovered, width, scale, &element, text).is_empty());
            }
            fixture.popup.set_view_mode(CalendarMode::Month);
            assert!(fixture.take_requests().is_empty());
        }
    }
}

#[test]
fn calendar_only_navigators_move_on_pointer_press_while_title_day_and_month_glyphs_stay_fixed() {
    let fixture = Fixture::ready(4);
    fixture.scale(2.0);
    let width = 640;
    for (mode, label, request) in [
        (
            CalendarMode::Month,
            "Toggle calendar view".to_owned(),
            Request::Toggle(ACTION_KEY.to_owned()),
        ),
        (
            CalendarMode::Month,
            day_description(5),
            Request::Day(day_key(5)),
        ),
        (
            CalendarMode::Year,
            MONTHS[5].to_owned(),
            Request::Month(month_key(5)),
        ),
    ] {
        fixture.popup.set_view_mode(mode);
        fixture.render_fit(2.0);
        let element = fixture.element(&label);
        let position = Fixture::center(&element);
        fixture.hover(position);
        let hovered = fixture.render_fit(2.0);
        let before = ink(&hovered, width, 2.0, &element, [18, 18, 18]);
        assert!(!before.is_empty());
        fixture.press(position);
        let pressed = fixture.render_fit(2.0);
        assert_eq!(
            ink(&pressed, width, 2.0, &element, [18, 18, 18]),
            before,
            "source title/day/month have no press transform"
        );
        assert!(fixture.take_requests().is_empty());
        fixture.release(position);
        assert_eq!(fixture.take_requests(), vec![request]);
    }
    fixture.popup.set_view_mode(CalendarMode::Month);
    fixture.render_fit(2.0);
    for (label, request) in [
        (
            "Previous month",
            Request::Navigate(false, ACTION_KEY.to_owned()),
        ),
        ("Today", Request::Today(ACTION_KEY.to_owned())),
        ("Next month", Request::Navigate(true, ACTION_KEY.to_owned())),
    ] {
        let element = fixture.element(label);
        let position = Fixture::center(&element);
        fixture.hover(position);
        let hovered = fixture.render_fit(2.0);
        let before = ink(&hovered, width, 2.0, &element, [18, 18, 18]);
        assert!(!before.is_empty());
        fixture.press(position);
        let pressed = fixture.render_fit(2.0);
        let after = ink(&pressed, width, 2.0, &element, [18, 18, 18]);
        assert!(!after.is_empty());
        assert_ne!(
            after, before,
            "real navigator glyph paints its .98/+1px press transform"
        );
        let before_y = before.iter().map(|(_, y)| *y).sum::<usize>() as f64 / before.len() as f64;
        let after_y = after.iter().map(|(_, y)| *y).sum::<usize>() as f64 / after.len() as f64;
        assert!(
            after_y > before_y,
            "navigator press moves glyph down, not its layout cell"
        );
        assert!(fixture.take_requests().is_empty());
        fixture.release(position);
        assert_eq!(fixture.take_requests(), vec![request]);
    }
}

#[test]
fn calendar_tiny_native_viewport_clips_then_tab_reveals_real_days_and_months_without_phantom_targets()
 {
    let fixture = Fixture::ready(6);
    let natural_width = fixture.popup.get_popup_content_width();
    let natural_height = fixture.popup.get_popup_content_height();
    for (scale, width, height) in [(1.0, 180, 120), (2.0, 360, 240)] {
        fixture.scale(scale);
        fixture.popup.invoke_focus_content();
        let pixels = fixture.render(width, height);
        assert!(fixture.labeled(&day_description(41)).is_empty());
        assert!(fixture.buttons().len() < 46);
        let last = pixels[(width * height - 1) as usize];
        assert_ne!([last.r, last.g, last.b], [242, 242, 242]);
        assert_eq!(fixture.popup.get_popup_content_width(), natural_width);
        assert!(fixture.popup.get_popup_content_height().is_finite());
        assert!(fixture.take_requests().is_empty());
        for _ in 0..4 {
            fixture.key(Key::Tab);
        }
        for index in 0..42 {
            fixture.key(Key::Tab);
            let element = fixture.element(&day_description(index));
            let center = Fixture::center(&element);
            assert!(
                center.x >= 0.0
                    && center.x < width as f32 / scale
                    && center.y >= 0.0
                    && center.y < height as f32 / scale,
                "Tab must reveal actual day {index}, not a clipped hit target"
            );
            assert!(fixture.take_requests().is_empty());
            fixture.key(Key::Return);
            assert_eq!(fixture.take_requests(), vec![Request::Day(day_key(index))]);
            fixture.click(&element);
            assert_eq!(
                fixture.take_requests(),
                vec![Request::Day(day_key(index))],
                "revealed day center must also accept genuine pointer input, not scrollbar capture"
            );
        }
        fixture.popup.set_view_mode(CalendarMode::Year);
        fixture.popup.invoke_focus_content();
        fixture.render(width, height);
        assert!(fixture.labeled(MONTHS[11]).is_empty());
        for _ in 0..4 {
            fixture.key(Key::Tab);
        }
        for (index, label) in MONTHS.iter().enumerate() {
            fixture.key(Key::Tab);
            let element = fixture.element(label);
            let center = Fixture::center(&element);
            assert!(
                center.x >= 0.0
                    && center.x < width as f32 / scale
                    && center.y >= 0.0
                    && center.y < height as f32 / scale,
                "Tab must reveal genuine month {label} inside the tiny native viewport"
            );
            assert!(fixture.take_requests().is_empty());
            fixture.key(Key::Space);
            assert_eq!(
                fixture.take_requests(),
                vec![Request::Month(month_key(index))]
            );
            fixture.click(&element);
            assert_eq!(
                fixture.take_requests(),
                vec![Request::Month(month_key(index))]
            );
        }
        fixture.popup.set_view_mode(CalendarMode::Month);
        fixture.render(1, 1);
        assert!(fixture.popup.get_popup_content_height().is_finite());
        assert!(fixture.take_requests().is_empty());
        fixture.popup.invoke_focus_content();
        fixture.render_fit(scale);
        assert_eq!(fixture.buttons().len(), 46);
        assert_eq!(
            fixture.popup.get_popup_content_height(),
            natural_height,
            "tiny recovery from {width}x{height} scale={scale} expected_natural_height={natural_height}; {}",
            fixture.geometry_diagnostics()
        );
    }
}

#[test]
fn calendar_long_rtl_titles_weekdays_months_and_error_text_remain_bounded_and_accessible() {
    let fixture = Fixture::ready(4);
    let natural_height = fixture.popup.get_popup_content_height();
    let title = "شهر طويل للغاية שלום כותרת לוח שנה ".repeat(12);
    fixture.popup.set_title_text(title.into());
    fixture.popup.set_weekdays(model(
        (0..7)
            .map(|index| format!("יום الأسبوع الطويل {index} {}", "שלום مرحبا ".repeat(8)).into())
            .collect(),
    ));
    let labels =
        std::array::from_fn(|index| format!("شهر {index} חודש {}", "שם ארוך للغاية ".repeat(12)));
    fixture.popup.set_month_rows(month_rows(&labels));
    for scale in [1.0_f32, 2.0] {
        fixture.scale(scale);
        let pixels = fixture.render_fit(scale);
        assert_eq!(fixture.popup.get_popup_content_width(), 320.0);
        assert_eq!(fixture.popup.get_popup_content_height(), natural_height);
        let title = fixture.element("Toggle calendar view");
        let nav = fixture.element("Previous month");
        assert!(title.absolute_position().x + title.size().width <= nav.absolute_position().x);
        for index in 0..7 {
            let label = format!("יום الأسبوع الطويل {index} {}", "שלום مرحبا ".repeat(8));
            let weekday = fixture.element(&label);
            assert_eq!(weekday.accessible_role(), Some(AccessibleRole::Text));
            assert!(weekday.absolute_position().x >= 18.0);
            assert!(weekday.absolute_position().x + weekday.size().width <= 302.0);
        }
        for y in 30..90 {
            assert_eq!(
                sample(&pixels, (320.0 * scale) as usize, scale, 308.0, y as f32),
                [242, 242, 242],
                "elided title/weekdays do not paint into the body gutter"
            );
        }
        fixture.popup.set_view_mode(CalendarMode::Year);
        let pixels = fixture.render_fit(scale);
        for (index, label) in labels.iter().enumerate() {
            let month = fixture.element(label);
            assert_eq!(month.accessible_role(), Some(AccessibleRole::Button));
            assert!(month.absolute_position().x + month.size().width <= 298.0);
            fixture.click(&month);
            assert_eq!(
                fixture.take_requests(),
                vec![Request::Month(month_key(index))]
            );
        }
        for y in 30..280 {
            assert_eq!(
                sample(&pixels, (320.0 * scale) as usize, scale, 308.0, y as f32),
                [242, 242, 242]
            );
        }
        fixture.popup.set_view_mode(CalendarMode::Month);
        fixture.render_fit(scale);
    }
    fixture.popup.set_action_key("".into());
    fixture.popup.set_weeks(ModelRc::default());
    fixture.popup.set_weekdays(ModelRc::default());
    let notice = "Calendar unavailable. שלום التقويم غير متاح. ".repeat(40);
    fixture.popup.set_notice(notice.clone().into());
    fixture.popup.set_retry_enabled(true);
    let pixels = fixture.render_fit(2.0);
    assert_eq!(fixture.popup.get_popup_content_width(), 320.0);
    assert!(fixture.popup.get_popup_content_height() <= 720.0);
    assert_eq!(
        fixture.element(&notice).accessible_role(),
        Some(AccessibleRole::Text)
    );
    let width = 640;
    let height = (fixture.popup.get_popup_content_height() * 2.0).ceil() as usize;
    assert_eq!(pixels.len(), width * height);
    fixture.popup.invoke_focus_content();
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    assert_eq!(
        fixture.take_requests(),
        vec![Request::Retry],
        "native Tab reveals actual Retry below a long wrapped notice"
    );
}
