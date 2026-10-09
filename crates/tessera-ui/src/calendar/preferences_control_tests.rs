// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::cell::Cell;
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, PhysicalSize};

use crate::generated::Panel;
use crate::render_tests::{draw, native_click, native_key, software_window};

fn selector(panel: &Panel) -> ElementHandle {
    ElementHandle::find_by_accessible_label(panel, "Start of week")
        .find(|element| element.accessible_role() == Some(AccessibleRole::Combobox))
        .expect("Start of week must label the real native ComboBox, not its caption")
}

#[test]
fn native_general_combobox_keyboard_edits_only_the_draft_until_explicit_save() {
    let window = software_window();
    let panel = Panel::new().unwrap();
    panel.window().set_size(PhysicalSize::new(640, 620));
    panel.show().unwrap();
    let _ = draw(&window, 640, 620);
    let saves = Rc::new(Cell::new(0));
    let saved = saves.clone();
    panel.on_save_preferences_requested(move || saved.set(saved.get() + 1));
    let previews = Rc::new(Cell::new(0));
    let changed = previews.clone();
    panel.on_appearance_changed(move || changed.set(changed.get() + 1));
    let control = selector(&panel);
    assert_eq!(control.accessible_value().unwrap(), "Monday");
    assert_eq!(control.accessible_role(), Some(AccessibleRole::Combobox));
    assert_eq!(control.accessible_expandable(), Some(true));
    assert_eq!(control.accessible_enabled(), Some(true));
    let preview_before = previews.get();
    native_click(&window, &control);
    let _ = draw(&window, 640, 620);
    assert_eq!(control.accessible_expanded(), Some(true));
    native_key(&window, Key::DownArrow.into());
    let _ = draw(&window, 640, 620);
    assert_eq!(panel.get_start_of_week_index(), 1);
    assert_eq!(control.accessible_value().unwrap(), "Sunday");
    native_key(&window, Key::DownArrow.into());
    let _ = draw(&window, 640, 620);
    assert_eq!(panel.get_start_of_week_index(), 2);
    assert_eq!(control.accessible_value().unwrap(), "Saturday");
    native_key(&window, Key::DownArrow.into());
    assert_eq!(
        panel.get_start_of_week_index(),
        2,
        "selector stays inside source choices"
    );
    native_key(&window, Key::UpArrow.into());
    let _ = draw(&window, 640, 620);
    assert_eq!(panel.get_start_of_week_index(), 1);
    native_key(&window, Key::Return.into());
    let _ = draw(&window, 640, 620);
    assert_eq!(control.accessible_expanded(), Some(false));
    assert_eq!(
        saves.get(),
        0,
        "selecting a draft never initiates persistence"
    );
    assert_eq!(
        previews.get(),
        preview_before,
        "General is not live appearance preview"
    );
    panel.invoke_save_preferences_requested();
    assert_eq!(saves.get(), 1);
    assert_eq!(panel.get_start_of_week_index(), 1);
    panel.hide().unwrap();
}

#[test]
fn native_general_combobox_reflects_loaded_choice_in_all_panel_themes_densities_and_scales() {
    let window = software_window();
    let panel = Panel::new().unwrap();
    let saves = Rc::new(Cell::new(0));
    let saved = saves.clone();
    panel.on_save_preferences_requested(move || saved.set(saved.get() + 1));
    panel.show().unwrap();
    for theme in [0, 1, 2] {
        panel.set_theme_index(theme);
        for compact in [false, true] {
            panel.set_compact(compact);
            for scale in [1.0, 1.5, 2.0] {
                panel
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged {
                        scale_factor: scale,
                    });
                let width = (600.0 * scale) as u32;
                let height = (620.0 * scale) as u32;
                panel.window().set_size(PhysicalSize::new(width, height));
                for (index, value) in [(0, "Monday"), (1, "Sunday"), (2, "Saturday")] {
                    panel.set_start_of_week_index(index);
                    // Stock ComboBox updates externally loaded indexes in a deferred
                    // changed handler; pump the real event-loop phase before painting.
                    slint::platform::update_timers_and_animations();
                    let _ = draw(&window, width, height);
                    let control = selector(&panel);
                    assert_eq!(control.accessible_value().unwrap(), value);
                    assert_eq!(control.accessible_role(), Some(AccessibleRole::Combobox));
                    let origin = control.absolute_position();
                    let size = control.size();
                    assert!(size.width > 0.0 && size.height > 0.0);
                    assert!(origin.x >= 0.0 && origin.y >= 0.0);
                    assert!(
                        origin.x + size.width <= 600.5,
                        "native selector fits narrow panel"
                    );
                    assert!(origin.y + size.height <= 620.5);
                }
            }
        }
    }
    assert_eq!(
        saves.get(),
        0,
        "restart projection and appearance never save"
    );
    panel.hide().unwrap();
}
