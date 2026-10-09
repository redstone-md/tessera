// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Physical holds keep a cancellation tombstone until a genuine release.
//! The standard Slider owns value mechanics; this module owns only UI intent.

#[derive(Clone)]
pub(super) enum InputScope<T> {
    Held(Option<T>),
    Fresh,
}

pub(super) struct SeekInput<T, C = u64> {
    pointer_held: bool,
    #[cfg(any(windows, test))]
    pointer_release_pending: bool,
    pointer: Option<T>,
    // Only the four native Slider seek keys enter this bounded set.
    keys_held: Vec<String>,
    key: Option<T>,
    // A touch may be promoted to the toolkit's shared mouse route. None of
    // those synthesized presses is native left-button authority.
    touches_held: Vec<C>,
    touch_release_pending: bool,
    touch_exhausted: bool,
}

impl<T, C> Default for SeekInput<T, C> {
    fn default() -> Self {
        Self {
            pointer_held: false,
            #[cfg(any(windows, test))]
            pointer_release_pending: false,
            pointer: None,
            keys_held: Vec::new(),
            key: None,
            touches_held: Vec::new(),
            touch_release_pending: false,
            touch_exhausted: false,
        }
    }
}

impl<T: Clone, C: Eq> SeekInput<T, C> {
    #[cfg(any(windows, test))]
    pub(super) fn pointer_down(&mut self, scope: Option<T>) {
        self.pointer_held = true;
        self.pointer_release_pending = false;
        self.pointer = scope;
    }

    #[cfg(any(windows, test))]
    pub(super) fn pointer_up(&mut self) {
        // Winit flushes buffered moves AFTER its raw release observer. Keep
        // the captured hold (including cancellation) through that dispatch.
        self.pointer_held = true;
        self.pointer_release_pending = true;
    }

    #[cfg(any(windows, test))]
    pub(super) fn can_capture_pointer(&self) -> bool {
        self.touches_held.is_empty() && !self.touch_release_pending && !self.touch_exhausted
    }

    #[cfg(any(windows, test))]
    pub(super) fn touch_start(&mut self, id: C) {
        self.touch_release_pending = false;
        if !self.touches_held.contains(&id) {
            if self.touches_held.len() < 16 {
                self.touches_held.push(id);
            } else {
                // Unknown excess contacts cannot be safely counted back down.
                self.touch_exhausted = true;
            }
        }
        self.cancel();
    }

    #[cfg(any(windows, test))]
    pub(super) fn touch_move(&mut self, id: C) {
        if !self.touches_held.contains(&id) {
            self.touch_start(id);
        }
        self.cancel();
    }

    #[cfg(any(windows, test))]
    pub(super) fn touch_end(&mut self, id: C) -> bool {
        let Some(index) = self.touches_held.iter().position(|held| held == &id) else {
            return false;
        };
        self.touches_held.swap_remove(index);
        self.touch_release_pending = self.touches_held.is_empty();
        self.cancel();
        self.touch_release_pending
    }

    /// Called only after native dispatch. Fresh presses clear their own pending
    /// flag, so deferred completion cannot retire a newer physical hold.
    #[cfg(any(windows, test))]
    pub(super) fn finish_releases(&mut self) -> bool {
        let finished = self.pointer_release_pending || self.touch_release_pending;
        if self.pointer_release_pending {
            self.pointer_held = false;
            self.pointer = None;
            self.pointer_release_pending = false;
        }
        self.touch_release_pending = false;
        finished
    }

    pub(super) fn key_down(&mut self, key: &str, scope: Option<T>) {
        // Repeats and another key within a hold never acquire a newer target.
        if self.keys_held.is_empty() {
            self.key = scope;
        }
        if self.keys_held.len() < 4 && !self.keys_held.iter().any(|held| held == key) {
            self.keys_held.push(key.to_owned());
        }
    }

    pub(super) fn key_up(&mut self, key: &str) {
        self.keys_held.retain(|held| held != key);
        if self.keys_held.is_empty() {
            self.key = None;
        }
    }

    pub(super) fn cancel(&mut self) {
        self.pointer = None;
        self.key = None;
    }

    pub(super) fn scope(&self) -> InputScope<T> {
        if !self.touches_held.is_empty() || self.touch_release_pending || self.touch_exhausted {
            InputScope::Held(None)
        } else if self.pointer_held {
            InputScope::Held(self.pointer.clone())
        } else if !self.keys_held.is_empty() {
            InputScope::Held(self.key.clone())
        } else {
            InputScope::Fresh
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InputScope, SeekInput};

    #[test]
    fn unchanged_thumb_capture_cannot_redirect_after_metadata_invalidation() {
        let mut input = SeekInput::<i32>::default();
        input.pointer_down(Some(19));
        input.cancel();
        assert!(matches!(input.scope(), InputScope::Held(None)));
        // A changed event cannot mint fresh AX/keyboard authority during this hold.
        assert!(!matches!(input.scope(), InputScope::Fresh));
        input.pointer_up();
        assert!(matches!(input.scope(), InputScope::Held(None)));
        input.finish_releases();
        assert!(matches!(input.scope(), InputScope::Fresh));
        input.pointer_down(Some(20));
        assert!(matches!(input.scope(), InputScope::Held(Some(20))));
    }

    #[test]
    fn key_repeat_and_second_key_do_not_reacquire_cancelled_scope() {
        let mut input = SeekInput::<i32>::default();
        input.key_down("right", Some(1));
        input.cancel();
        input.key_down("right", Some(2));
        input.key_down("end", Some(2));
        assert!(matches!(input.scope(), InputScope::Held(None)));
        input.key_up("end");
        assert!(matches!(input.scope(), InputScope::Held(None)));
        input.key_up("right");
        assert!(matches!(input.scope(), InputScope::Fresh));
    }

    #[test]
    fn releasing_first_of_two_keys_does_not_redirect_still_held_second_key() {
        let mut input = SeekInput::<i32>::default();
        input.key_down("right", Some(1));
        input.key_down("end", Some(1));
        input.cancel();
        input.key_up("right");
        input.key_down("end", Some(2));
        assert!(matches!(input.scope(), InputScope::Held(None)));
        input.key_up("end");
        assert!(matches!(input.scope(), InputScope::Fresh));
    }

    #[test]
    fn key_release_cannot_end_a_cancelled_physical_pointer_hold() {
        let mut input = SeekInput::<i32>::default();
        input.pointer_down(Some(1));
        input.cancel();
        input.key_down("right", None);
        input.key_up("right");
        assert!(matches!(input.scope(), InputScope::Held(None)));
        input.pointer_up();
        input.finish_releases();
        assert!(matches!(input.scope(), InputScope::Fresh));
    }

    #[test]
    fn press_outside_actual_slider_remains_rejected_until_release() {
        let mut input = SeekInput::<u8>::default();
        input.pointer_down(None);
        assert!(matches!(input.scope(), InputScope::Held(None)));
        input.cancel();
        assert!(matches!(input.scope(), InputScope::Held(None)));
        input.pointer_up();
        input.finish_releases();
        assert!(matches!(input.scope(), InputScope::Fresh));
    }

    #[test]
    fn cancelled_release_keeps_buffered_move_fenced_until_dispatch_finishes() {
        let mut input = SeekInput::<i32>::default();
        input.pointer_down(Some(1));
        input.cancel();
        input.pointer_up();
        assert!(matches!(input.scope(), InputScope::Held(None)));
        assert!(input.finish_releases());
        assert!(matches!(input.scope(), InputScope::Fresh));
    }

    #[test]
    fn deferred_old_release_cannot_clear_fresh_physical_press() {
        let mut input = SeekInput::<i32>::default();
        input.pointer_down(Some(1));
        input.pointer_up();
        input.pointer_down(Some(2));
        assert!(!input.finish_releases());
        assert!(matches!(input.scope(), InputScope::Held(Some(2))));
    }

    #[test]
    fn touch_promotion_and_unknown_termination_never_release_another_contact() {
        let mut input = SeekInput::<u8>::default();
        input.touch_start(11);
        input.touch_start(22);
        assert!(!input.touch_end(99));
        assert!(!input.touch_end(11));
        assert!(!input.touch_end(11));
        input.finish_releases();
        assert!(matches!(input.scope(), InputScope::Held(None)));
        input.touch_move(22);
        assert!(matches!(input.scope(), InputScope::Held(None)));
        assert!(input.touch_end(22));
        assert!(matches!(input.scope(), InputScope::Held(None)));
        input.finish_releases();
        assert!(matches!(input.scope(), InputScope::Fresh));
    }

    #[test]
    fn same_contact_number_on_different_devices_has_independent_lifetime() {
        let mut input = SeekInput::<u8, (u8, u64)>::default();
        input.touch_start((1, 7));
        input.touch_start((2, 7));
        assert!(!input.touch_end((1, 7)));
        input.finish_releases();
        assert!(matches!(input.scope(), InputScope::Held(None)));
        assert!(input.touch_end((2, 7)));
        assert!(matches!(input.scope(), InputScope::Held(None)));
        input.finish_releases();
        assert!(matches!(input.scope(), InputScope::Fresh));
    }

    #[test]
    fn touch_release_does_not_end_cancelled_real_mouse_hold() {
        let mut input = SeekInput::<i32>::default();
        input.pointer_down(Some(1));
        input.touch_start(11);
        input.touch_end(11);
        input.finish_releases();
        assert!(matches!(input.scope(), InputScope::Held(None)));
        input.pointer_up();
        input.finish_releases();
        assert!(matches!(input.scope(), InputScope::Fresh));
    }

    #[test]
    fn excess_touch_identity_storage_fails_closed_without_unbounded_allocation() {
        let mut input = SeekInput::<u8>::default();
        for id in 0..17 {
            input.touch_start(id);
        }
        assert_eq!(input.touches_held.len(), 16);
        for id in 0..17 {
            input.touch_end(id);
        }
        input.finish_releases();
        assert!(matches!(input.scope(), InputScope::Held(None)));
        assert!(!input.can_capture_pointer());
    }
}
