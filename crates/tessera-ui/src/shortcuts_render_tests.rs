// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Genuine SDK focused keys, native Tab/checkbox and software pixels. Only
//! controlled in-memory shortcut recording; no hook, registration or user input.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery, ElementRoot};
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::{Key, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalSize, PhysicalSize};
use tessera_system::shortcuts::mocks::RecordingShortcutHost;
use tessera_system::shortcuts::{KeyChord, KeyModifiers, ShortcutConfig};

use crate::render_tests::{draw, native_click, native_key, software_window};
use crate::shortcuts::{ShortcutsBindings, ShortcutsController};

slint::slint! {
    import { ShortcutPreferences } from "../ui/shortcut-preferences.slint";
    import { Button, LineEdit } from "std-widgets.slint";

    export component ShortcutInputWindow inherits Window {
        preferred-width: 400px;
        preferred-height: 560px;
        in property <bool> draft-enabled: true;
        in property <bool> controls-enabled: true;
        in property <bool> compact: false;
        in property <string> settings-label: "Win + K";
        in property <bool> capturing: false;
        in property <bool> dirty: false;
        in property <bool> applying: false;
        in property <string> launcher-status: "Not applied";
        in property <string> settings-status: "Not applied";
        in property <string> message;
        out property <string> unrelated-text: unrelated.text;
        callback enabled-edited(bool);
        callback capture-requested;
        callback capture-cancelled;
        callback reset-requested;
        callback capture-pressed(KeyEvent) -> bool;
        callback capture-released(KeyEvent) -> bool;
        callback save-requested;
        public function focus-capture() { shortcuts.focus-capture(); }
        public function cancel-input() { shortcuts.cancel-input(); }
        VerticalLayout {
            padding: 8px;
            spacing: 8px;
            shortcuts := ShortcutPreferences {
                compact: root.compact;
                controls-enabled: root.controls-enabled;
                draft-enabled: root.draft-enabled;
                settings-label: root.settings-label;
                capturing: root.capturing;
                dirty: root.dirty;
                applying: root.applying;
                launcher-status: root.launcher-status;
                settings-status: root.settings-status;
                message: root.message;
                enabled-edited(value) => { root.enabled-edited(value); }
                capture-requested => { root.capture-requested(); }
                capture-cancelled => { root.capture-cancelled(); }
                settings-reset-requested => { root.reset-requested(); }
                capture-pressed(event) => { return root.capture-pressed(event); }
                capture-released(event) => { return root.capture-released(event); }
            }
            unrelated := LineEdit { accessible-label: "Unrelated text input"; }
            Button {
                text: "Save all preferences";
                clicked => { root.save-requested(); }
            }
            Rectangle { vertical-stretch: 1; }
        }
    }
}

struct Fixture {
    window: Rc<MinimalSoftwareWindow>,
    surface: ShortcutInputWindow,
    controller: Rc<ShortcutsController>,
    host: Arc<RecordingShortcutHost>,
    saves: Rc<RefCell<Vec<ShortcutConfig>>>,
}

impl Fixture {
    fn new() -> Self {
        let window = software_window();
        let surface = ShortcutInputWindow::new().unwrap();
        let host = Arc::new(RecordingShortcutHost::new());
        let provider = host.clone();
        let root = surface.as_weak();
        let current = surface.as_weak();
        let controller = Rc::new(ShortcutsController::new(
            ShortcutConfig::default(),
            ShortcutsBindings {
                factory: Rc::new(move || Ok(Some(provider.clone()))),
                is_current: Rc::new(move || current.upgrade().is_some()),
                project: Rc::new(move |view| {
                    let Some(surface) = root.upgrade() else {
                        return;
                    };
                    surface.set_draft_enabled(view.enabled);
                    surface.set_settings_label(view.settings_label.into());
                    surface.set_capturing(view.capturing);
                    surface.set_dirty(view.dirty);
                    surface.set_applying(view.applying);
                    surface.set_launcher_status(view.launcher_status.into());
                    surface.set_settings_status(view.settings_status.into());
                    surface.set_message(view.message.into());
                }),
                deliver: Rc::new(|_| panic!("render fixture never dispatches native actions")),
                wake: Arc::new(|| {}),
            },
        ));
        let actor = Rc::downgrade(&controller);
        surface.on_enabled_edited(move |enabled| {
            if let Some(actor) = actor.upgrade() {
                actor.set_draft_enabled(enabled);
            }
        });
        let actor = Rc::downgrade(&controller);
        surface.on_capture_requested(move || {
            if let Some(actor) = actor.upgrade() {
                actor.begin_capture();
            }
        });
        let actor = Rc::downgrade(&controller);
        surface.on_capture_cancelled(move || {
            if let Some(actor) = actor.upgrade() {
                actor.cancel_capture();
            }
        });
        let actor = Rc::downgrade(&controller);
        surface.on_reset_requested(move || {
            if let Some(actor) = actor.upgrade() {
                actor.reset_settings();
            }
        });
        let actor = Rc::downgrade(&controller);
        surface.on_capture_pressed(move |event| {
            actor
                .upgrade()
                .is_some_and(|actor| actor.key_pressed(event))
        });
        let actor = Rc::downgrade(&controller);
        surface.on_capture_released(move |event| {
            actor
                .upgrade()
                .is_some_and(|actor| actor.key_released(event))
        });
        let saves = Rc::new(RefCell::new(Vec::new()));
        let recorded = saves.clone();
        let actor = Rc::downgrade(&controller);
        surface.on_save_requested(move || {
            if let Some(actor) = actor.upgrade() {
                let config = actor.draft_config();
                // This recording callback represents successful parent full Save,
                // not a component-owned shortcut-only storage transaction.
                recorded.borrow_mut().push(config.clone());
                actor.apply_saved(config);
            }
        });
        controller.project();
        surface.show().unwrap();
        window
            .window()
            .dispatch_event(WindowEvent::WindowActiveChanged(true));
        window.set_size(PhysicalSize::new(400, 560));
        draw(&window, 400, 560);
        Self {
            window,
            surface,
            controller,
            host,
            saves,
        }
    }

    fn element(&self, label: &str, role: AccessibleRole) -> ElementHandle {
        ElementHandle::find_by_accessible_label(&self.surface, label)
            .find(|element| element.accessible_role() == Some(role))
            .unwrap_or_else(|| panic!("missing real accessible control: {label}"))
    }

    fn capture(&self) -> ElementHandle {
        self.element(
            &format!(
                "Change Settings shortcut: {}",
                self.controller.view().settings_label
            ),
            AccessibleRole::Button,
        )
    }

    fn key_down(&self, text: impl Into<slint::SharedString>) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.into() });
    }

    fn key_up(&self, text: impl Into<slint::SharedString>) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text: text.into() });
    }
}

#[test]
fn actual_focused_sdk_press_repeat_release_edits_draft_only_then_parent_save_applies() {
    let fixture = Fixture::new();
    native_click(&fixture.window, &fixture.capture());
    assert!(fixture.surface.get_capturing());
    assert_eq!(
        fixture
            .element(
                "Press a shortcut. Escape cancels capture.",
                AccessibleRole::Button
            )
            .accessible_enabled(),
        Some(true)
    );
    fixture.key_down(Key::Control);
    fixture.key_down("l");
    fixture
        .window
        .window()
        .dispatch_event(WindowEvent::KeyPressRepeated { text: "l".into() });
    assert_eq!(fixture.controller.draft_config(), ShortcutConfig::default());
    fixture.key_up(Key::Control);
    fixture.key_up("l");
    assert!(!fixture.surface.get_capturing());
    assert_eq!(
        fixture.controller.draft_config().settings_override(),
        Some(
            &KeyChord::new(
                KeyModifiers {
                    control: true,
                    ..KeyModifiers::default()
                },
                0x4c,
            )
            .unwrap()
        )
    );
    assert_eq!(fixture.surface.get_settings_label(), "Ctrl + L");
    assert!(fixture.host.configurations().is_empty());
    assert!(fixture.saves.borrow().is_empty());
    native_click(
        &fixture.window,
        &fixture.element("Save all preferences", AccessibleRole::Button),
    );
    assert_eq!(fixture.saves.borrow().len(), 1);
    assert_eq!(fixture.host.configurations(), *fixture.saves.borrow());
    fixture
        .host
        .take_next_completion()
        .unwrap()
        .complete_registered();
    fixture.controller.process_pending();
    assert!(
        fixture
            .surface
            .get_settings_status()
            .starts_with("Registered")
    );
}

#[test]
fn capture_escape_then_native_tab_space_reset_uses_existing_tile_input() {
    let fixture = Fixture::new();
    fixture
        .controller
        .set_settings_override(Some(
            KeyChord::new(
                KeyModifiers {
                    control: true,
                    ..KeyModifiers::default()
                },
                0x4c,
            )
            .unwrap(),
        ))
        .unwrap();
    native_click(&fixture.window, &fixture.capture());
    fixture.key_down(Key::Control);
    fixture.key_down("k");
    native_key(&fixture.window, Key::Escape.into());
    fixture.key_up("k");
    fixture.key_up(Key::Control);
    assert!(!fixture.surface.get_capturing());
    assert_eq!(fixture.surface.get_settings_label(), "Ctrl + L");
    // Escape returns actual native focus to the capture tile. The next Tab
    // reaches Reset; Space follows TileButton's genuine press/release grammar.
    native_key(&fixture.window, Key::Tab.into());
    native_key(&fixture.window, Key::Space.into());
    assert_eq!(fixture.controller.draft_config().settings_override(), None);
    assert_eq!(fixture.surface.get_settings_label(), "Win + K");
    assert!(fixture.host.configurations().is_empty());
}

#[test]
fn readonly_row_native_checkbox_and_unrelated_text_keep_accessibility_and_key_scope() {
    let fixture = Fixture::new();
    let readonly = fixture.element(
        "Launcher shortcut: Win. Read-only system shortcut.",
        AccessibleRole::Text,
    );
    assert_eq!(readonly.accessible_role(), Some(AccessibleRole::Text));
    assert!(
        ElementHandle::find_by_accessible_label(&fixture.surface, "Change Launcher shortcut")
            .next()
            .is_none()
    );
    let checkbox = fixture.element("Enable global shortcuts", AccessibleRole::Checkbox);
    assert_eq!(checkbox.accessible_checked(), Some(true));
    native_click(&fixture.window, &checkbox);
    assert!(!fixture.controller.draft_config().enabled());
    assert_eq!(checkbox.accessible_checked(), Some(false));
    assert_eq!(fixture.capture().accessible_enabled(), Some(false));
    native_click(&fixture.window, &checkbox);
    let input = fixture.element("Unrelated text input", AccessibleRole::TextInput);
    native_click(&fixture.window, &input);
    native_key(&fixture.window, "x".into());
    assert_eq!(fixture.surface.get_unrelated_text(), "x");
    assert_eq!(fixture.controller.draft_config().settings_override(), None);
    assert!(fixture.host.configurations().is_empty());
    assert!(fixture.saves.borrow().is_empty());
}

#[test]
fn tiny_rtl_labels_native_tab_and_scales_render_without_repeater_reset_or_authority_change() {
    let fixture = Fixture::new();
    let canonical = fixture.controller.draft_config();
    let rtl = "اختصار الإعدادات — Ctrl + Shift + K — إعدادات";
    fixture.surface.set_settings_label(rtl.into());
    fixture
        .surface
        .set_settings_status("Conflict (Win + K)".into());
    fixture
        .surface
        .set_message("This shortcut conflicts with another binding.".into());
    for compact in [false, true] {
        fixture.surface.set_compact(compact);
        for scale in [1.0, 1.5, 2.0] {
            fixture
                .window
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
            let width = (240.0 * scale) as u32;
            let height = (720.0 * scale) as u32;
            fixture.window.set_size(PhysicalSize::new(width, height));
            fixture
                .surface
                .window()
                .dispatch_event(WindowEvent::Resized {
                    size: LogicalSize::new(240.0, 720.0),
                });
            native_key(&fixture.window, Key::Tab.into());
            fixture.window.request_redraw();
            let pixels = draw(&fixture.window, width, height);
            assert_eq!(
                fixture.window.window().size(),
                PhysicalSize::new(width, height),
                "adapter physical extent must match the actual native resize",
            );
            let actual = fixture.surface.root_element().size();
            assert!(
                (actual.width - 240.0).abs() <= 0.5 && (actual.height - 720.0).abs() <= 0.5,
                "native root logical extent must be 240x720 before testing children: scale={scale} actual={actual:?}",
            );
            assert!(
                pixels.windows(2).any(|pair| pair[0] != pair[1]),
                "real glyph/control pixels must render"
            );
            let capture = fixture.element(
                &format!("Change Settings shortcut: {rtl}"),
                AccessibleRole::Button,
            );
            let reset =
                fixture.element("Reset Settings shortcut to Win + K", AccessibleRole::Button);
            for element in [&capture, &reset] {
                let origin = element.absolute_position();
                let size = element.size();
                assert!(size.width > 0.0 && size.height >= 32.0);
                assert!(
                    origin.x >= 0.0 && origin.x + size.width <= 240.5,
                    "tile exceeds 240px: compact={compact} scale={scale} origin={origin:?} size={size:?}",
                );
                assert!(
                    origin.y >= 0.0 && origin.y + size.height <= 720.5,
                    "tile exceeds measured vertical space: compact={compact} scale={scale} origin={origin:?} size={size:?}",
                );
            }
            assert!(
                reset.absolute_position().y
                    >= capture.absolute_position().y + capture.size().height
            );
            let checkbox = fixture.element("Enable global shortcuts", AccessibleRole::Checkbox);
            let captions = ElementQuery::from_root(&fixture.surface)
                .match_id("ShortcutPreferences::enabled-label")
                .find_all();
            assert_eq!(captions.len(), 1, "one genuine wrapped checkbox caption");
            let caption = &captions[0];
            for element in [&checkbox, caption] {
                let origin = element.absolute_position();
                let size = element.size();
                assert!(origin.x >= 0.0 && origin.x + size.width <= 240.5);
                assert!(origin.y >= 0.0 && origin.y + size.height <= 720.5);
            }
            assert!(
                caption.absolute_position().y + caption.size().height
                    <= checkbox.absolute_position().y + checkbox.size().height + 0.5,
                "native checkbox row reserves actual wrapped caption height",
            );
            assert_eq!(
                fixture.controller.draft_config(),
                canonical,
                "display RTL label never becomes authority"
            );
        }
    }
    assert!(fixture.host.configurations().is_empty());
}

#[test]
fn disabled_surface_and_explicit_cancel_retire_active_focused_gesture() {
    let fixture = Fixture::new();
    native_click(&fixture.window, &fixture.capture());
    fixture.key_down(Key::Control);
    fixture.key_down("l");
    fixture.surface.invoke_cancel_input();
    fixture.key_up("l");
    fixture.key_up(Key::Control);
    assert_eq!(fixture.controller.draft_config().settings_override(), None);
    assert!(!fixture.surface.get_capturing());
    fixture.surface.set_controls_enabled(false);
    slint::platform::update_timers_and_animations();
    assert_eq!(fixture.capture().accessible_enabled(), Some(false));
    fixture.capture().invoke_accessible_default_action();
    assert!(!fixture.surface.get_capturing());
    assert!(fixture.host.configurations().is_empty());
}
