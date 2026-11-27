// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The power plans of Windows. Widgets: `plan`, the plan in use, whose
//! click opens the popup `plan` (or, with `cycle = true`, puts the next
//! plan in use), and `plan-list`, every plan to choose from.

use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Context, Entity, Global, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    WeakEntity, Window, div, px,
};

use crate::config::{PopupConfig, WidgetSpec};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix, staggered};
use crate::platform::PowerPlan;
use crate::theme::Theme;
use crate::ui::{icon, list_row, list_title, one_line, pill, state_layer};

const ICON_PLAN: &str = "\u{e945}";

/// How often the plans are read: another program may change the one in
/// use, and Windows tells only a window that asks for it.
const CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// The widest the name of a plan is drawn.
const NAME_WIDTH: f32 = 160.;

/// The popup the `plan` widget opens unless the layout names another.
const POPUP: &str = "plan";

/// How far below its place a row starts when the list arrives.
const ROW_TRAVEL: f32 = 8.;

pub struct PlanModule;

impl Module for PlanModule {
    fn name(&self) -> &'static str {
        "plan"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("plan", |context, cx| {
            let named = context.flag("name").unwrap_or(true);
            let cycles = context.flag("cycle").unwrap_or(false);
            cx.new(|cx| PlanButton::new(named, cycles, cx)).into()
        })?;
        widgets.register_panel("plan-list", |_, cx| cx.new(PlanList::new).into())?;
        widgets.register_popup(PopupConfig {
            name: POPUP.to_owned(),
            place: Default::default(),
            width: 320.,
            height: 280.,
            content: vec![WidgetSpec {
                widget: "plan-list".to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
        })?;
        widgets.open_with("plan", POPUP);
        Ok(())
    }
}

/// The plan that follows the one in use, the first after the last; `None`
/// where there is no other.
fn next(plans: &[PowerPlan]) -> Option<&PowerPlan> {
    let active = plans.iter().position(|plan| plan.active)?;
    let next = &plans[(active + 1) % plans.len()];
    (!next.active).then_some(next)
}

/// The plans as they were last read, shared by every plan widget.
struct Plans {
    plans: Vec<PowerPlan>,
    /// How many plans were put in use from here. A reading begun before
    /// the last one is older than what the list shows, and is dropped.
    chosen: usize,
}

struct SharedPlans(WeakEntity<Plans>);

impl Global for SharedPlans {}

impl Plans {
    /// The one instance, which lives for as long as a widget keeps it and
    /// reads the plans every few seconds meanwhile.
    fn shared(cx: &mut App) -> Entity<Self> {
        let shared = cx.try_global::<SharedPlans>();
        if let Some(plans) = shared.and_then(|shared| shared.0.upgrade()) {
            return plans;
        }
        let plans = cx.new(|cx: &mut Context<Self>| {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    let Ok(chosen) = this.read_with(cx, |plans, _| plans.chosen) else {
                        break;
                    };
                    let read = cx.background_executor().spawn(async { PowerPlan::all() });
                    let read = read.await;
                    let taken = this.update(cx, |plans, cx| {
                        if plans.chosen == chosen {
                            plans.take(read, cx);
                        }
                    });
                    if taken.is_err() {
                        break;
                    }
                    cx.background_executor().timer(CHECK_INTERVAL).await;
                }
            })
            .detach();
            Self {
                plans: Vec::new(),
                chosen: 0,
            }
        });
        cx.set_global(SharedPlans(plans.downgrade()));
        plans
    }

    fn take(&mut self, plans: Vec<PowerPlan>, cx: &mut Context<Self>) {
        if plans != self.plans {
            self.plans = plans;
            cx.notify();
        }
    }

    fn active(&self) -> Option<&PowerPlan> {
        self.plans.iter().find(|plan| plan.active)
    }

    /// Shows `plan` as the one in use at once and has Windows put it in
    /// use; the plans are read again afterwards either way.
    fn choose(&mut self, plan: PowerPlan, cx: &mut Context<Self>) {
        if plan.active {
            return;
        }
        self.chosen += 1;
        PowerPlan::mark_in_use(&mut self.plans, &plan);
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx.background_executor().spawn(async move {
                if let Err(error) = plan.activate() {
                    report!("the power plan `{}` was not put in use: {error}", plan.name);
                }
                PowerPlan::all()
            });
            let read = read.await;
            let _ = this.update(cx, |plans, cx| plans.take(read, cx));
        })
        .detach();
    }

    /// Puts the plan after the one in use in use.
    fn go_on(&mut self, cx: &mut Context<Self>) {
        if let Some(plan) = next(&self.plans).cloned() {
            self.choose(plan, cx);
        }
    }
}

/// The plan in use, as an icon and its name.
struct PlanButton {
    plans: Entity<Plans>,
    /// Whether the name of the plan stands beside the icon.
    named: bool,
    /// Whether a click puts the next plan in use, instead of leaving the
    /// click to the popup.
    cycles: bool,
    /// The name of the plan in use that was last shown.
    shown: Option<String>,
    interaction: Interaction,
    /// 1 at the moment the plan in use changes, then back to 0.
    changed: Spring,
    _changes: Subscription,
}

impl PlanButton {
    fn new(named: bool, cycles: bool, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let plans = Plans::shared(cx);
        Self {
            _changes: cx.observe(&plans, |this: &mut Self, _, cx| this.follow(cx)),
            shown: plans.read(cx).active().map(|plan| plan.name.clone()),
            plans,
            named,
            cycles,
            interaction: Interaction::new(&motion),
            changed: Spring::new(motion.effects, 0.),
        }
    }

    /// Lights up when the plan in use is another one than before; the
    /// first reading is no change.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let active = self.plans.read(cx).active().map(|plan| plan.name.clone());
        if active != self.shown {
            if self.shown.is_some() {
                self.changed.jump_to(1.);
                self.changed.animate_to(0.);
            }
            self.shown = active;
        }
        cx.notify();
    }
}

impl Render for PlanButton {
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
        let name = (self.shown.clone())
            .filter(|_| self.named)
            .map(SharedString::from);
        let button = pill(&theme)
            .id("plan")
            .bg(state_layer(&theme, ground, state))
            .text_color(colors.on_surface)
            .child(icon(&theme, ICON_PLAN))
            .children(name.map(|name| div().max_w(px(NAME_WIDTH)).child(one_line(name))));
        let button = if self.cycles {
            button.on_click(cx.listener(|this, _, _, cx| {
                this.plans.update(cx, Plans::go_on);
            }))
        } else {
            button
        };
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}

/// Every plan, the one in use lit; a click on another puts it in use.
struct PlanList {
    plans: Entity<Plans>,
    rows: Vec<Interaction>,
    /// 0 when the list appears, 1 once it is in place.
    arrival: Spring,
    _changes: Subscription,
}

impl PlanList {
    fn new(cx: &mut Context<Self>) -> Self {
        let plans = Plans::shared(cx);
        let mut arrival = Spring::new(cx.global::<Theme>().motion.spatial, 0.);
        arrival.animate_to(1.);
        Self {
            _changes: cx.observe(&plans, |_, _, cx| cx.notify()),
            plans,
            rows: Vec::new(),
            arrival,
        }
    }
}

impl Render for PlanList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let plans = self.plans.read(cx).plans.clone();
        self.rows
            .resize_with(plans.len(), || Interaction::new(&theme.motion));
        let arrival = self.arrival.value(window).clamp(0., 1.);
        let count = plans.len();
        let rows = plans.into_iter().enumerate().map(|(index, plan)| {
            let arrived = staggered(arrival, index, count);
            let note = if plan.active {
                language.say("In use")
            } else {
                ""
            };
            let row = list_row(&theme, ICON_PLAN, plan.active, plan.name.clone(), note)
                .id(("power-plan", index))
                .relative()
                .top(px(ROW_TRAVEL * (1. - arrived)))
                .opacity(arrived);
            if plan.active {
                return row.into_any_element();
            }
            let state = self.rows[index].state(window);
            let ground = theme.colors.surface_container_highest;
            let row = row.bg(state_layer(&theme, ground, state));
            Interaction::track(row, cx, move |this: &mut Self| this.rows.get_mut(index))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let plan = plan.clone();
                    this.plans.update(cx, |plans, cx| plans.choose(plan, cx));
                }))
                .into_any_element()
        });
        let rows: Vec<_> = rows.collect();
        let list = div()
            .id("plan-list")
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .overflow_y_scroll()
            .child(list_title(&theme, language.say("Power plan")));
        if rows.is_empty() {
            return list.child(
                div()
                    .px(theme.spacing.small)
                    .text_color(theme.colors.on_surface_variant)
                    .child(language.say("Windows tells of no power plan")),
            );
        }
        list.children(rows)
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
