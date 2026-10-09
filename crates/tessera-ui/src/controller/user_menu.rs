// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Launcher-origin ownership of the independent current-user folder popup.

use super::popups::PopupKind;
use super::{PanelController, Rc};
use crate::generated::TileBounds;
use crate::power_menu::UserLogoutAuthority;
use crate::theme::ThemedComponent;
use crate::transient_window::TransientCache;
use crate::user_menu::UserLogoutIntent;
use crate::user_menu::UserMenuController;
use slint::ComponentHandle;
use std::cell::RefCell;

pub(super) type UserPopups = TransientCache<UserMenuController>;

impl PanelController {
    pub(super) fn open_user_menu(&self, bounds: TileBounds) {
        self.dismiss_tooltip(false);
        let Some(launcher) = self.launcher_and_upgrade() else {
            return;
        };
        if !self.root_current()
            || !self.launcher_popup_ready()
            || !launcher.window().is_visible()
            || launcher.get_reorder_dragging()
            || !bounds.origin.x.is_finite()
            || !bounds.origin.y.is_finite()
            || !bounds.width.is_finite()
            || !bounds.height.is_finite()
            || bounds.width <= 0.0
            || bounds.height <= 0.0
        {
            return;
        }
        let Some(context) = self.launcher_context() else {
            self.report_message("The user menu needs the launcher's real monitor geometry.");
            return;
        };
        let existing = self.user_menu.borrow().clone();
        let user = match existing {
            Some(user) => user,
            None => match UserMenuController::new(self.core.host().clone()) {
                Ok(user) => {
                    *self.user_menu.borrow_mut() = Some(Rc::clone(&user));
                    let admission = Rc::downgrade(&self.admission);
                    let power = Rc::downgrade(&self.power_admission_closed);
                    let cache = Rc::downgrade(&self.user_menu);
                    let expected = Rc::downgrade(&user);
                    let panel = self.panel.clone();
                    let source = launcher.as_weak();
                    let core = std::sync::Arc::downgrade(&self.core);
                    let profile_admission: Rc<dyn Fn() -> bool> = Rc::new(move || {
                        // Do not borrow the folder/profile actor during its predicate.
                        let Some(admission) = admission.upgrade() else {
                            return false;
                        };
                        if !admission.alive.get()
                            || admission.active_popup.get() != Some(PopupKind::User)
                            || power.upgrade().is_none_or(|closed| closed.get())
                            || panel.upgrade().is_none()
                            || source
                                .upgrade()
                                .is_none_or(|source| !source.window().is_visible())
                            || core
                                .upgrade()
                                .and_then(|core| core.dock_context())
                                .is_none_or(|context| context.fullscreen_active())
                        {
                            return false;
                        }
                        let Some(cache) = cache.upgrade() else {
                            return false;
                        };
                        let Some(expected) = expected.upgrade() else {
                            return false;
                        };
                        cache.try_borrow().ok().is_some_and(|cached| {
                            cached
                                .as_ref()
                                .is_some_and(|current| Rc::ptr_eq(current, &expected))
                        }) && expected.component().window().is_visible()
                    });
                    let admission = Rc::clone(&profile_admission);
                    user.bind_profile_admission(move || admission());
                    let pending = Rc::new(RefCell::new(None::<UserLogoutIntent>));
                    let relay = Rc::downgrade(&pending);
                    let panel = self.panel.clone();
                    user.bind_logout(
                        move || profile_admission(),
                        move |intent| {
                            let Some(pending) = relay.upgrade() else {
                                return;
                            };
                            // One synchronous typed receipt, not a command queue.
                            if pending.borrow().is_some() {
                                return;
                            }
                            pending.replace(Some(intent));
                            if let Some(panel) = panel.upgrade() {
                                panel.invoke_user_logout_event_ready();
                            }
                            pending.borrow_mut().take();
                        },
                    );
                    if let Some(panel) = self.panel.upgrade() {
                        let root = self.clone();
                        panel.on_user_logout_event_ready(move || {
                            let intent = pending.borrow_mut().take();
                            if let Some(intent) = intent {
                                root.request_user_logout(intent);
                            }
                        });
                    }
                    user
                }
                Err(error) => {
                    self.report_message(&format!("Could not create the user menu: {error}"));
                    return;
                }
            },
        };
        // Reserve current User intent before profile activation inside show().
        // Actor session checks retire Hide/reopen; this identity retires Root reentry.
        let operation = Rc::new(());
        self.popup_operation.replace(Rc::clone(&operation));
        let previous = self.admission.active_popup.replace(Some(PopupKind::User));
        user.apply_theme(launcher.presentation_theme());
        if !self.root_current() || !Rc::ptr_eq(&operation, &self.popup_operation.borrow()) {
            return;
        }
        let power = self.power_menu.borrow().clone();
        user.set_logout_busy(power.is_some_and(|power| power.command_busy()));
        if !self.root_current() || !Rc::ptr_eq(&operation, &self.popup_operation.borrow()) {
            return;
        }
        let source_current = self.launcher_source_guard();
        let result = user.show_for_root(
            launcher.window(),
            bounds,
            context,
            &launcher.get_user_name(),
            || {
                self.root_current()
                    && Rc::ptr_eq(&operation, &self.popup_operation.borrow())
                    && source_current()
            },
        );
        if !self.root_current() || !Rc::ptr_eq(&operation, &self.popup_operation.borrow()) {
            return;
        }
        if !user.is_open() {
            self.admission.active_popup.set(previous);
        }
        self.popup_presentation_finished(PopupKind::User, user.is_open(), result);
    }

    fn request_user_logout(&self, intent: UserLogoutIntent) {
        let user = self.user_menu.borrow().clone();
        let Some(user) = user else {
            return;
        };
        let source_current = self.launcher_source_guard();
        let origin = self.popup_operation.borrow().clone();
        let cache_current = || {
            self.user_menu
                .borrow()
                .as_ref()
                .is_some_and(|cached| Rc::ptr_eq(cached, &user))
        };
        let current = || {
            self.root_current()
                && self.admission.active_popup.get() == Some(PopupKind::User)
                && Rc::ptr_eq(&origin, &self.popup_operation.borrow())
                && cache_current()
                && source_current()
        };
        if !current() || !user.logout_current(&intent) {
            return;
        }
        let Some(power) = self.ensure_power_controller(current) else {
            return;
        };
        let power_current = || {
            self.power_menu
                .borrow()
                .as_ref()
                .is_some_and(|cached| Rc::ptr_eq(cached, &power))
        };
        let retirement = RefCell::new(None::<Rc<()>>);
        let retired = || {
            self.root_current()
                && self.admission.active_popup.get().is_none()
                && cache_current()
                && power_current()
                && source_current()
                && retirement
                    .borrow()
                    .as_ref()
                    .is_some_and(|expected| Rc::ptr_eq(expected, &self.popup_operation.borrow()))
        };
        let retire = || {
            if !current() || !power_current() || !self.dismiss_popups_except(None) {
                return false;
            }
            // The coordinator intentionally creates a NEW transaction identity.
            // Only its successful exact result authorizes this retired source.
            retirement.replace(Some(self.popup_operation.borrow().clone()));
            retired()
        };
        let authority = UserLogoutAuthority::new(&user, &intent, &current, &retire, &retired);
        power.request_user_logout(&authority);
    }

    pub(super) fn hide_user_menu(&self) {
        let user = self.user_menu.borrow().clone();
        if self.admission.active_popup.get() == Some(PopupKind::User) {
            self.admission.active_popup.set(None);
        }
        if let Some(user) = user {
            user.hide();
        }
    }
}
