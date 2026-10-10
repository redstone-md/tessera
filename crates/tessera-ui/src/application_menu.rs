// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One application-menu flight, independent from its disposable popup presentation.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use slint::ComponentHandle;
use tessera_system::application_menu::{
    ApplicationAction, ApplicationActionOutcome, ApplicationActionTarget, ApplicationMenuError,
    ApplicationMenuHost, ApplicationMenuSnapshot,
};

use crate::DockContext;
use crate::generated::{ContextMenuSurface, DockMenuAction};

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct WindowFrame {
    position: slint::PhysicalPosition,
    size: slint::PhysicalSize,
    scale: f32,
}

impl WindowFrame {
    pub(crate) fn capture(window: &slint::Window) -> Option<Self> {
        let frame = Self {
            position: window.position(),
            size: window.size(),
            scale: window.scale_factor(),
        };
        (window.is_visible()
            && frame.size.width > 0
            && frame.size.height > 0
            && frame.scale.is_finite()
            && frame.scale > 0.0)
            .then_some(frame)
    }
}

#[derive(Clone)]
pub(crate) struct ApplicationScope {
    pub(crate) generation: u64,
    pub(crate) source: u64,
    pub(crate) key: String,
    pub(crate) dock_frame: WindowFrame,
    pub(crate) popup_frame: WindowFrame,
    pub(crate) anchor: slint::PhysicalPosition,
    pub(crate) context: DockContext,
    pub(crate) admission: Rc<dyn Fn() -> bool>,
}

struct Presentation {
    scope: ApplicationScope,
    provider: Arc<dyn ApplicationMenuHost>,
    targets: Vec<ApplicationActionTarget>,
}

pub(crate) struct ApprovedApplicationCommand {
    pub(crate) scope: ApplicationScope,
    provider: Arc<dyn ApplicationMenuHost>,
    target: ApplicationActionTarget,
}

#[derive(Clone, Copy)]
pub(crate) enum FlightKind {
    Inspect,
    Request {
        retired_generation: u64,
        action: ApplicationAction,
    },
}

struct Flight {
    ticket: u64,
    scope: ApplicationScope,
    kind: FlightKind,
    _provider: Arc<dyn ApplicationMenuHost>,
}

pub(crate) enum ApplicationReply {
    Inspect(Result<ApplicationMenuSnapshot, ApplicationMenuError>),
    Request(Result<ApplicationActionOutcome, ApplicationMenuError>),
}

struct Receipt {
    ticket: u64,
    reply: ApplicationReply,
}

pub(crate) struct ApplicationMenus {
    presentation: RefCell<Option<Presentation>>,
    flight: RefCell<Option<Flight>>,
    next_ticket: Cell<Option<u64>>,
    mailbox: Arc<Mutex<Option<Receipt>>>,
    surface: slint::Weak<ContextMenuSurface>,
}

impl ApplicationMenus {
    pub(crate) fn new(surface: &ContextMenuSurface) -> Self {
        Self {
            presentation: RefCell::default(),
            flight: RefCell::default(),
            next_ticket: Cell::new(Some(0)),
            mailbox: Arc::default(),
            surface: surface.as_weak(),
        }
    }

    /// Retirement drops only UI capabilities. An accepted provider operation survives.
    pub(crate) fn retire(&self) {
        let old = self.presentation.borrow_mut().take();
        drop(old);
    }

    pub(crate) fn scope(&self) -> Option<ApplicationScope> {
        self.presentation
            .borrow()
            .as_ref()
            .map(|presented| presented.scope.clone())
    }
    pub(crate) fn prepare(&self, scope: ApplicationScope, provider: Arc<dyn ApplicationMenuHost>) {
        let old = self.presentation.replace(Some(Presentation {
            scope,
            provider,
            targets: Vec::new(),
        }));
        drop(old);
    }

    fn reserve(
        &self,
        scope: ApplicationScope,
        kind: FlightKind,
        provider: Arc<dyn ApplicationMenuHost>,
    ) -> Result<u64, ApplicationMenuError> {
        if self.flight.borrow().is_some() {
            return Err(ApplicationMenuError::Busy);
        }
        let ticket = self
            .next_ticket
            .get()
            .ok_or(ApplicationMenuError::Unavailable)?;
        self.next_ticket.set(ticket.checked_add(1));
        self.flight.replace(Some(Flight {
            ticket,
            scope,
            kind,
            _provider: provider,
        }));
        Ok(ticket)
    }

    fn complete_immediate_error(&self, ticket: u64) {
        let old = {
            let mut flight = self.flight.borrow_mut();
            if flight
                .as_ref()
                .is_some_and(|flight| flight.ticket == ticket)
            {
                flight.take()
            } else {
                None
            }
        };
        drop(old);
    }

    pub(crate) fn inspect(
        &self,
        scope: ApplicationScope,
        provider: Arc<dyn ApplicationMenuHost>,
    ) -> Result<(), ApplicationMenuError> {
        let key = scope.key.clone();
        let ticket = self.reserve(scope.clone(), FlightKind::Inspect, Arc::clone(&provider))?;
        let mailbox = Arc::clone(&self.mailbox);
        let weak = self.surface.clone();
        let keep_alive = Arc::clone(&provider);
        let result = provider.inspect(
            &key,
            Box::new(move |reply| {
                let _keep_alive = keep_alive;
                deliver(
                    &mailbox,
                    &weak,
                    Receipt {
                        ticket,
                        reply: ApplicationReply::Inspect(reply),
                    },
                );
            }),
        );
        if result.is_err() {
            self.complete_immediate_error(ticket);
        }
        result
    }

    pub(crate) fn receive(&self) -> Option<(ApplicationScope, FlightKind, ApplicationReply)> {
        let receipt = self.mailbox.lock().ok()?.take()?;
        let flight = {
            let mut slot = self.flight.borrow_mut();
            if slot
                .as_ref()
                .is_some_and(|flight| flight.ticket == receipt.ticket)
            {
                slot.take()
            } else {
                None
            }
        }?;
        Some((flight.scope, flight.kind, receipt.reply))
    }

    pub(crate) fn publish_targets(
        &self,
        scope: &ApplicationScope,
        snapshot: ApplicationMenuSnapshot,
    ) -> Result<Vec<ApplicationAction>, ApplicationMenuError> {
        if snapshot.application_key != scope.key || snapshot.targets.len() > 2 {
            return Err(ApplicationMenuError::Unconfirmed);
        }
        let actions: Vec<_> = snapshot
            .targets
            .iter()
            .map(ApplicationActionTarget::action)
            .collect();
        if actions.len() == 2 && actions[0] == actions[1] {
            return Err(ApplicationMenuError::Unconfirmed);
        }
        let old = {
            let mut slot = self.presentation.borrow_mut();
            let presented = slot
                .as_mut()
                .filter(|presented| presented.scope.generation == scope.generation)
                .ok_or(ApplicationMenuError::Unconfirmed)?;
            std::mem::replace(&mut presented.targets, snapshot.targets)
        };
        drop(old);
        Ok(actions)
    }

    pub(crate) fn reframe(&self, scope: &ApplicationScope, frame: WindowFrame) {
        if let Some(presented) = self
            .presentation
            .borrow_mut()
            .as_mut()
            .filter(|presented| presented.scope.generation == scope.generation)
        {
            presented.scope.popup_frame = frame;
        }
    }

    pub(crate) fn capture(
        &self,
        generation: u64,
        action: ApplicationAction,
    ) -> Option<ApprovedApplicationCommand> {
        if self.flight.borrow().is_some() {
            return None;
        }
        let slot = self.presentation.borrow();
        let presented = slot
            .as_ref()
            .filter(|presented| presented.scope.generation == generation)?;
        let target = presented
            .targets
            .iter()
            .find(|target| target.action() == action)?
            .clone();
        Some(ApprovedApplicationCommand {
            scope: presented.scope.clone(),
            provider: Arc::clone(&presented.provider),
            target,
        })
    }

    pub(crate) fn request(
        &self,
        command: ApprovedApplicationCommand,
        retired_generation: u64,
    ) -> Result<(), ApplicationMenuError> {
        let ticket = self.reserve(
            command.scope,
            FlightKind::Request {
                retired_generation,
                action: command.target.action(),
            },
            Arc::clone(&command.provider),
        )?;
        let mailbox = Arc::clone(&self.mailbox);
        let weak = self.surface.clone();
        let keep_alive = Arc::clone(&command.provider);
        let result = command.provider.request(
            command.target,
            Box::new(move |reply| {
                let _keep_alive = keep_alive;
                deliver(
                    &mailbox,
                    &weak,
                    Receipt {
                        ticket,
                        reply: ApplicationReply::Request(reply),
                    },
                );
            }),
        );
        if result.is_err() {
            self.complete_immediate_error(ticket);
        }
        result
    }
}

fn deliver(
    mailbox: &Mutex<Option<Receipt>>,
    weak: &slint::Weak<ContextMenuSurface>,
    receipt: Receipt,
) {
    if let Ok(mut slot) = mailbox.lock() {
        *slot = Some(receipt);
    }
    let _ = weak.upgrade_in_event_loop(|surface| surface.invoke_application_event_ready());
}

pub(crate) fn application_action(action: DockMenuAction) -> Option<ApplicationAction> {
    match action {
        DockMenuAction::RunAsAdministrator => Some(ApplicationAction::RunAsAdministrator),
        DockMenuAction::OpenFileLocation => Some(ApplicationAction::OpenFileLocation),
        _ => None,
    }
}

pub(crate) fn action_row(
    action: ApplicationAction,
    generation: u64,
) -> crate::generated::MenuEntry {
    let (label, action) = match action {
        ApplicationAction::RunAsAdministrator => {
            ("Run as administrator", DockMenuAction::RunAsAdministrator)
        }
        ApplicationAction::OpenFileLocation => {
            ("Open file location", DockMenuAction::OpenFileLocation)
        }
    };
    crate::generated::MenuEntry {
        label: label.into(),
        action,
        application_scope: generation.to_string().into(),
        ..Default::default()
    }
}

pub(crate) fn error_notice(error: ApplicationMenuError) -> &'static str {
    match error {
        ApplicationMenuError::Busy => "Application actions were busy. Reopen this menu.",
        ApplicationMenuError::Unavailable => "Application actions are unavailable.",
        ApplicationMenuError::Unconfirmed => "Application actions could not be confirmed.",
        ApplicationMenuError::InvalidTarget => "This application action is no longer available.",
    }
}

pub(crate) fn outcome_notice(
    action: ApplicationAction,
    outcome: ApplicationActionOutcome,
) -> &'static str {
    match (action, outcome) {
        (_, ApplicationActionOutcome::Declined) => "The application request was declined.",
        (ApplicationAction::RunAsAdministrator, ApplicationActionOutcome::Accepted) => {
            "Windows accepted the elevation request; elevation is not confirmed."
        }
        (ApplicationAction::OpenFileLocation, ApplicationActionOutcome::Accepted) => {
            "Windows accepted the file location request; an Explorer window is not confirmed."
        }
    }
}
