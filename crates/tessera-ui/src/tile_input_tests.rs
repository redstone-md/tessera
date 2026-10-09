// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Real shared TileButton, native input and AX; no platform effects or replacement controls.

use std::cell::RefCell;
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::{Key, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, SharedString};

use crate::render_tests::{draw, software_window};

slint::slint! {
    import { TileButton } from "../ui/tiles.slint";

    export component TileInputWindow inherits Window {
        width: 100px;
        height: 60px;
        in-out property <string> authority-key;
        in property <bool> tile-enabled: true;
        in property <bool> tile-interactive: true;
        in property <bool> allow-repeats: true;
        in property <bool> pointer-focus: true;
        out property <bool> tile-pressed: tile.pressed;
        out property <bool> tile-focused: tile.has-focus;
        out property <bool> host-focused: host-focus.has-focus;
        callback primary(string);
        callback middle(string);
        callback context(string);
        callback pointer-down;
        public function focus-tile() { tile.focus-for-keyboard(); }
        public function cancel-tile-input() { tile.cancel-input(); }
        public function focus-host() { host-focus.focus(); }

        host-focus := FocusScope {
            width: 0px;
            height: 0px;
            focus-on-tab-navigation: false;
        }

        tile := TileButton {
            x: 20px;
            y: 10px;
            width: 60px;
            height: 40px;
            radius: 4px;
            accessible-text: "Authority tile";
            activation-key: root.authority-key;
            enabled: root.tile-enabled;
            interactive: root.tile-interactive;
            allow-return-repeat: root.allow-repeats;
            focus-on-pointer: root.pointer-focus;
            report-pointer-metadata: true;
            clicked => { root.primary(root.authority-key); }
            middle-clicked => { root.middle(root.authority-key); }
            context-requested(position) => { root.context(root.authority-key); }
            pointer-metadata(event, bounds, position) => {
                if event.kind == PointerEventKind.down { root.pointer-down(); }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    Primary,
    Middle,
    Context,
}

#[derive(Clone, Copy, Debug)]
enum Gesture {
    Space,
    Pointer(PointerEventButton),
}

impl Gesture {
    const ALL: [Self; 4] = [
        Self::Space,
        Self::Pointer(PointerEventButton::Left),
        Self::Pointer(PointerEventButton::Middle),
        Self::Pointer(PointerEventButton::Right),
    ];

    fn effect(self) -> Effect {
        match self {
            Self::Space | Self::Pointer(PointerEventButton::Left) => Effect::Primary,
            Self::Pointer(PointerEventButton::Middle) => Effect::Middle,
            Self::Pointer(PointerEventButton::Right) => Effect::Context,
            Self::Pointer(_) => unreachable!("fixture uses only activating buttons"),
        }
    }
}

struct Fixture {
    window: Rc<MinimalSoftwareWindow>,
    tile: TileInputWindow,
    effects: Rc<RefCell<Vec<(Effect, SharedString)>>>,
}

impl Fixture {
    fn new() -> Self {
        let window = software_window();
        let tile = TileInputWindow::new().unwrap();
        let effects = Rc::new(RefCell::new(Vec::new()));
        let recorded = effects.clone();
        tile.on_primary(move |key| recorded.borrow_mut().push((Effect::Primary, key)));
        let recorded = effects.clone();
        tile.on_middle(move |key| recorded.borrow_mut().push((Effect::Middle, key)));
        let recorded = effects.clone();
        tile.on_context(move |key| recorded.borrow_mut().push((Effect::Context, key)));
        tile.show().unwrap();
        window
            .window()
            .dispatch_event(WindowEvent::WindowActiveChanged(true));
        window.set_size(PhysicalSize::new(100, 60));
        slint::platform::update_timers_and_animations();
        draw(&window, 100, 60);
        Self {
            window,
            tile,
            effects,
        }
    }

    fn press(&self, gesture: Gesture) {
        match gesture {
            Gesture::Space => {
                self.tile.invoke_focus_tile();
                self.key_pressed(Key::Space);
            }
            Gesture::Pointer(button) => {
                self.window
                    .window()
                    .dispatch_event(WindowEvent::PointerPressed {
                        position: LogicalPosition::new(50.0, 30.0),
                        button,
                    });
            }
        }
    }

    fn release(&self, gesture: Gesture) {
        let event = match gesture {
            Gesture::Space => WindowEvent::KeyReleased {
                text: Key::Space.into(),
            },
            Gesture::Pointer(button) => WindowEvent::PointerReleased {
                position: LogicalPosition::new(50.0, 30.0),
                button,
            },
        };
        self.window.window().dispatch_event(event);
    }

    fn key_pressed(&self, key: Key) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    }

    fn repeat(&self, key: Key) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressRepeated { text: key.into() });
    }

    fn take(&self) -> Vec<(Effect, SharedString)> {
        std::mem::take(&mut *self.effects.borrow_mut())
    }

    fn fresh(&self, gesture: Gesture, key: &str) {
        self.press(gesture);
        self.release(gesture);
        assert_eq!(self.take(), vec![(gesture.effect(), key.into())]);
    }
}

#[test]
fn captured_authority_rejects_key_only_rebinding_without_an_event_turn() {
    let fixture = Fixture::new();
    for gesture in Gesture::ALL {
        fixture.tile.set_authority_key("issued-old".into());
        fixture.press(gesture);
        fixture.tile.set_authority_key("issued-new".into());
        assert!(!fixture.tile.get_tile_pressed());
        if matches!(gesture, Gesture::Space) {
            fixture.repeat(Key::Space);
        }
        // No timer update, drawing, disabled phase or control reconstruction.
        fixture.release(gesture);
        assert!(
            fixture.take().is_empty(),
            "old {gesture:?} must not retarget"
        );
        fixture.fresh(gesture, "issued-new");
    }

    // Metadata is external code; authority must be captured before it runs.
    let weak = fixture.tile.as_weak();
    fixture.tile.on_pointer_down(move || {
        weak.upgrade()
            .unwrap()
            .set_authority_key("metadata-new".into());
    });
    for button in [
        PointerEventButton::Left,
        PointerEventButton::Middle,
        PointerEventButton::Right,
    ] {
        fixture.tile.set_authority_key("metadata-old".into());
        let gesture = Gesture::Pointer(button);
        fixture.press(gesture);
        fixture.release(gesture);
        assert!(
            fixture.take().is_empty(),
            "metadata rebind must revoke {button:?}"
        );
    }
}

#[test]
fn synchronous_cancel_revokes_even_a_coalesced_disable_and_reenable() {
    let fixture = Fixture::new();
    for gesture in Gesture::ALL {
        fixture.press(gesture);
        fixture.tile.invoke_cancel_tile_input();
        assert!(!fixture.tile.get_tile_pressed());
        fixture.tile.set_tile_enabled(false);
        fixture.tile.set_tile_interactive(false);
        fixture.tile.hide().unwrap();
        fixture.tile.set_tile_enabled(true);
        fixture.tile.set_tile_interactive(true);
        fixture.tile.show().unwrap();
        if matches!(gesture, Gesture::Space) {
            fixture.repeat(Key::Space);
        }
        fixture.release(gesture);
        assert!(
            fixture.take().is_empty(),
            "cancelled {gesture:?} must stay cancelled"
        );
        // Empty activation keys preserve the original callers' fresh gestures.
        fixture.fresh(gesture, "");
    }
}

#[test]
fn committed_disabled_state_and_native_pointer_cancel_revoke_gestures() {
    let fixture = Fixture::new();
    for gesture in Gesture::ALL {
        fixture.press(gesture);
        fixture.tile.set_tile_enabled(false);
        // Slint observes the disabled state, not an unobservable false -> true.
        slint::platform::update_timers_and_animations();
        fixture.window.request_redraw();
        draw(&fixture.window, 100, 60);
        fixture.tile.set_tile_enabled(true);
        fixture.release(gesture);
        assert!(
            fixture.take().is_empty(),
            "observed disable must revoke {gesture:?}"
        );
        fixture.fresh(gesture, "");
    }

    for button in [
        PointerEventButton::Left,
        PointerEventButton::Middle,
        PointerEventButton::Right,
    ] {
        let gesture = Gesture::Pointer(button);
        fixture.press(gesture);
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerExited);
        fixture.release(gesture);
        assert!(
            fixture.take().is_empty(),
            "native cancel must revoke {button:?}"
        );
        fixture.fresh(gesture, "");
    }
}

#[test]
fn outside_release_is_inert_and_fresh_accessibility_and_return_use_current_authority() {
    let fixture = Fixture::new();
    // Retain the actual native semantic item across all inputs and rebinding.
    let held_ax = ElementHandle::find_by_accessible_label(&fixture.tile, "Authority tile")
        .find(|element| element.accessible_role() == Some(AccessibleRole::Button))
        .expect("the rendered TileButton needs compiler-emitted element metadata");
    assert_eq!(held_ax.accessible_enabled(), Some(true));
    let origin = held_ax.absolute_position();
    let size = held_ax.size();
    assert_eq!((origin.x, origin.y), (20.0, 10.0));
    assert_eq!((size.width, size.height), (60.0, 40.0));
    for button in [
        PointerEventButton::Left,
        PointerEventButton::Middle,
        PointerEventButton::Right,
    ] {
        let gesture = Gesture::Pointer(button);
        fixture.press(gesture);
        // Captured release without a preceding move: has-hover can still be true.
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position: LogicalPosition::new(90.0, 30.0),
                button,
            });
        assert!(
            fixture.take().is_empty(),
            "outside {button:?} release must be inert"
        );
        fixture.fresh(gesture, "");
    }

    fixture.tile.set_authority_key("current-authority".into());
    assert_eq!(held_ax.accessible_enabled(), Some(true));
    held_ax.invoke_accessible_default_action();
    assert_eq!(
        fixture.take(),
        vec![(Effect::Primary, "current-authority".into())]
    );

    fixture.tile.invoke_focus_tile();
    for (allow_repeats, expected) in [(true, 2), (false, 1)] {
        fixture.tile.set_allow_repeats(allow_repeats);
        fixture.key_pressed(Key::Return);
        fixture.repeat(Key::Return);
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::KeyReleased {
                text: Key::Return.into(),
            });
        assert_eq!(
            fixture.take(),
            vec![(Effect::Primary, "current-authority".into()); expected],
            "Return repeat policy is unchanged"
        );
    }
}

#[test]
fn held_return_repeats_cannot_revive_cancelled_or_rebound_authority() {
    let fixture = Fixture::new();
    for rebind in [false, true] {
        fixture.tile.set_authority_key("return-old".into());
        fixture.tile.invoke_focus_tile();
        fixture.key_pressed(Key::Return);
        assert_eq!(fixture.take(), vec![(Effect::Primary, "return-old".into())]);
        let current = if rebind {
            fixture.tile.set_authority_key("return-new".into());
            "return-new"
        } else {
            fixture.tile.invoke_cancel_tile_input();
            "return-old"
        };
        fixture.repeat(Key::Return);
        assert!(
            fixture.take().is_empty(),
            "held Return must not revive revoked authority"
        );
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::KeyReleased {
                text: Key::Return.into(),
            });
        fixture.key_pressed(Key::Return);
        fixture.repeat(Key::Return);
        fixture
            .window
            .window()
            .dispatch_event(WindowEvent::KeyReleased {
                text: Key::Return.into(),
            });
        assert_eq!(fixture.take(), vec![(Effect::Primary, current.into()); 2]);
    }
}

#[test]
fn pointer_activation_can_preserve_native_command_focus() {
    let fixture = Fixture::new();
    fixture.tile.set_pointer_focus(false);
    fixture.tile.invoke_focus_host();
    fixture.fresh(Gesture::Pointer(PointerEventButton::Left), "");
    assert!(fixture.tile.get_host_focused());
    assert!(!fixture.tile.get_tile_focused());

    fixture.tile.set_pointer_focus(true);
    fixture.fresh(Gesture::Pointer(PointerEventButton::Left), "");
    assert!(fixture.tile.get_tile_focused());
    assert!(!fixture.tile.get_host_focused());
}
