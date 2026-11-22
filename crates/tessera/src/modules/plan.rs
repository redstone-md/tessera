// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `plan` widget: the power plan of Windows that is in use; a click
//! puts the next one in use.

use std::time::Duration;

use gpui::{
    AppContext, AsyncApp, Context, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::PowerPlan;
use crate::theme::Theme;
use crate::ui::{icon, one_line, pill, state_layer};

const ICON_PLAN: &str = "\u{e945}";

/// How often the plans are read: another program may change the one in
/// use, and Windows tells only a window that asks for it.
const CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// The widest the name of a plan is drawn.
const NAME_WIDTH: f32 = 160.;

pub struct PlanModule;

impl Module for PlanModule {
    fn name(&self) -> &'static str {
        "plan"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("plan", |context, cx| {
            let named = context.flag("name").unwrap_or(true);
            cx.new(|cx| Plan::new(named, cx)).into()
        })
    }
}

/// The plan that follows the one in use, the first after the last; `None`
/// where there is no other.
fn next(plans: &[PowerPlan]) -> Option<&PowerPlan> {
    let active = plans.iter().position(|plan| plan.active)?;
    let next = &plans[(active + 1) % plans.len()];
    (!next.active).then_some(next)
}

/// Shows the plans that were last read.
struct Plan {
    plans: Vec<PowerPlan>,
    /// Whether the name of the plan stands beside the icon.
    named: bool,
    interaction: Interaction,
    /// 1 at the moment the plan in use changes, then back to 0.
    changed: Spring,
}

impl Plan {
    fn new(named: bool, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let plans = cx.background_executor().spawn(async { PowerPlan::all() });
                let plans = plans.await;
                if this.update(cx, |this, cx| this.take(plans, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(CHECK_INTERVAL).await;
            }
        })
        .detach();
        Self {
            plans: Vec::new(),
            named,
            interaction: Interaction::new(&motion),
            changed: Spring::new(motion.effects, 0.),
        }
    }

    /// Takes in the plans that were read.
    fn take(&mut self, plans: Vec<PowerPlan>, cx: &mut Context<Self>) {
        if plans == self.plans {
            return;
        }
        // The first reading is no change.
        if !self.plans.is_empty() {
            self.changed.jump_to(1.);
            self.changed.animate_to(0.);
        }
        self.plans = plans;
        cx.notify();
    }

    /// Puts the next plan in use and reads the plans again.
    fn go_on(&mut self, cx: &mut Context<Self>) {
        let Some(plan) = next(&self.plans) else {
            return;
        };
        if let Err(error) = plan.activate() {
            report!("the power plan `{}` was not put in use: {error}", plan.name);
            return;
        }
        self.take(PowerPlan::all(), cx);
    }
}

impl Render for Plan {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let colors = theme.colors;
        let changed = self.changed.value(window).clamp(0., 1.);
        let state = self.interaction.state(window);
        let ground = mix(
            theme.on_bar(colors.surface_container),
            colors.secondary_container,
            changed,
        );
        let active = self.plans.iter().find(|plan| plan.active);
        let name = active
            .filter(|_| self.named)
            .map(|plan| SharedString::new(plan.name.as_str()));
        let button = pill(&theme)
            .id("plan")
            .bg(state_layer(&theme, ground, state))
            .text_color(colors.on_surface)
            .on_click(cx.listener(|this, _, _, cx| this.go_on(cx)))
            .child(icon(&theme, ICON_PLAN))
            .children(name.map(|name| div().max_w(gpui::px(NAME_WIDTH)).child(one_line(name))));
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_another_plan_there_is_no_next() {
        assert!(next(&[]).is_none());
        let plans = PowerPlan::all();
        if plans.len() == 1 && plans[0].active {
            assert!(next(&plans).is_none());
        }
        if plans.len() > 1 && plans.iter().any(|plan| plan.active) {
            assert!(next(&plans).is_some_and(|plan| !plan.active));
        }
    }
}
