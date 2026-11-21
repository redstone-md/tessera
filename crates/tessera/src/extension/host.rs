// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Keeps one extension running for all of its widgets.

use gpui::{AsyncApp, Context, WeakEntity};

use super::http::{self, HttpResponse};
use super::runtime::{Instance, Node};

/// A running extension. It notifies its observers whenever the content of
/// its widgets may have changed.
pub struct Host {
    /// The extension's identifier, for messages.
    id: String,
    /// `None` once a call into the extension has failed.
    instance: Option<Instance>,
}

impl Host {
    pub fn new(id: String, instance: Instance) -> Self {
        Self {
            id,
            instance: Some(instance),
        }
    }

    /// The content of `widget` now, or `None` if the extension has failed.
    pub fn render(&mut self, widget: &str, cx: &mut Context<Self>) -> Option<Vec<Node>> {
        self.call(cx, |instance| instance.render(widget))
    }

    /// Tells the extension that the button with `action` was pressed.
    pub fn activate(&mut self, widget: &str, action: &str, cx: &mut Context<Self>) {
        self.call(cx, |instance| instance.activate(widget, action));
        cx.notify();
    }

    fn fetched(
        &mut self,
        request: u32,
        outcome: Result<HttpResponse, String>,
        cx: &mut Context<Self>,
    ) {
        self.call(cx, |instance| instance.fetched(request, outcome));
        cx.notify();
    }

    /// Calls into the extension and sends the requests it started
    /// meanwhile. A failure is reported and ends the extension.
    fn call<T>(
        &mut self,
        cx: &mut Context<Self>,
        call: impl FnOnce(&mut Instance) -> anyhow::Result<T>,
    ) -> Option<T> {
        let instance = self.instance.as_mut()?;
        match call(instance) {
            Ok(value) => {
                for (number, request) in instance.take_requests() {
                    cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                        let outcome = cx
                            .background_executor()
                            .spawn(async move { http::send(&request) })
                            .await;
                        this.update(cx, |this, cx| this.fetched(number, outcome, cx))
                            .ok();
                    })
                    .detach();
                }
                Some(value)
            }
            Err(error) => {
                eprintln!("tessera: the extension `{}` stopped: {error:#}", self.id);
                self.instance = None;
                cx.notify();
                None
            }
        }
    }
}
