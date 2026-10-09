// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

pub(super) enum Outcome {
    Read(Result<InputLanguageSnapshot, InputLanguageError>),
    Action(Result<InputLanguageOutcome, InputLanguageError>),
}

pub(super) struct Completion {
    pub token: Token,
    pub outcome: Outcome,
}

/// One accepted operation survives hide/reopen; only its original token can retire it.
#[derive(Default)]
pub(super) struct Mailbox {
    pub expected: Option<Token>,
    pub completion: Option<Completion>,
    pub wake_queued: bool,
}

pub(super) fn complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<InputLanguageMenu>,
    token: Token,
    outcome: Outcome,
) {
    {
        let mut mailbox = mailbox.lock();
        if mailbox.expected != Some(token) || mailbox.completion.is_some() {
            return;
        }
        mailbox.completion = Some(Completion { token, outcome });
        if mailbox.wake_queued {
            return;
        }
        mailbox.wake_queued = true;
    }
    // All deliveries, including inline provider replies and immediate rejection,
    // cross the UI mailbox. A completion never projects on the provider stack.
    if root
        .upgrade_in_event_loop(|root| root.invoke_input_event_ready())
        .is_err()
    {
        mailbox.lock().wake_queued = false;
    }
}
