// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `overview` widget, for a popup: a card for each workspace of
//! Tessera's own with the icons of the windows that live there. A click
//! on a card goes to that workspace, a click on an icon to its window.

use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window,
    div, img, px,
};

use crate::i18n::Language;
use crate::icons::AppIcons;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix, staggered};
use crate::theme::Theme;
use crate::ui::{one_line, state_layer};
use crate::workspaces::{Resident, Workspaces as Kept};

/// How often the workspaces are looked at.
const CHECK_INTERVAL: Duration = Duration::from_millis(300);

/// How far below its place a card starts when it arrives, in pixels.
const CARD_TRAVEL: f32 = 12.;

pub struct OverviewModule;

impl Module for OverviewModule {
    fn name(&self) -> &'static str {
        "overview"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("overview", |_, cx| cx.new(Overview::new).into())
    }
}

/// What is shown of the workspaces.
#[derive(Clone, PartialEq, Default)]
struct Seen {
    /// The windows of each workspace; `None` when Tessera keeps none.
    residents: Option<Vec<Vec<Resident>>>,
    current: usize,
    labels: Vec<SharedString>,
}

impl Seen {
    fn now(cx: &App) -> Self {
        let residents = Kept::residents(cx);
        let count = residents.as_ref().map_or(0, Vec::len);
        Self {
            residents,
            current: Kept::now(cx).current,
            labels: (0..count)
                .map(|index| Kept::label(index, cx).into())
                .collect(),
        }
    }
}

/// The card of one workspace and the motion it is in.
struct Card {
    /// 1 while this is the workspace shown.
    lit: Spring,
    interaction: Interaction,
    /// One for each window of the workspace.
    windows: Vec<Interaction>,
}

struct Overview {
    seen: Seen,
    cards: Vec<Card>,
    /// 0 when the widget appears, 1 once the cards are in place.
    arrival: Spring,
}

impl Overview {
    fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                if this.update(cx, Self::look).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut this = Self {
            seen: Seen::now(cx),
            cards: Vec::new(),
            arrival: Spring::new(motion.spatial, 0.),
        };
        this.follow(cx);
        // The workspace shown at the start is lit without getting there.
        for card in &mut this.cards {
            let lit = card.lit.target();
            card.lit.jump_to(lit);
        }
        this.arrival.animate_to(1.);
        this
    }

    fn look(&mut self, cx: &mut Context<Self>) {
        let seen = Seen::now(cx);
        if seen != self.seen {
            self.seen = seen;
            self.follow(cx);
            cx.notify();
        }
    }

    /// Has a card for each workspace, lit where the workspace is shown.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let motion = cx.global::<Theme>().motion;
        let residents = self.seen.residents.as_deref().unwrap_or_default();
        self.cards.resize_with(residents.len(), || Card {
            lit: Spring::new(motion.effects, 0.),
            interaction: Interaction::new(&motion),
            windows: Vec::new(),
        });
        for (index, card) in self.cards.iter_mut().enumerate() {
            let shown = index == self.seen.current;
            card.lit.animate_to(if shown { 1. } else { 0. });
            card.windows
                .resize_with(residents[index].len(), || Interaction::new(&motion));
        }
    }

    fn render_window(
        &mut self,
        card: usize,
        place: usize,
        resident: &Resident,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let size = theme.launcher.icon_size;
        let picture = resident
            .program
            .as_deref()
            .and_then(|program| cx.global::<AppIcons>().of_executable(program));
        let state = self.cards[card].windows[place].state(window);
        let id = resident.id;
        let face = div()
            .id(("resident", id.raw() as usize))
            .p(theme.spacing.tiny)
            .rounded(theme.shape.small)
            .bg(state_layer(
                theme,
                theme.colors.on_surface.opacity(0.),
                state,
            ))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                Kept::show_window(id, cx);
            })
            .child(match picture {
                Some(picture) => img(picture).size(size).into_any_element(),
                None => div()
                    .size(size)
                    .rounded(theme.shape.small)
                    .bg(theme.colors.outline)
                    .into_any_element(),
            });
        Interaction::track(face, cx, move |this: &mut Self| {
            this.cards.get_mut(card)?.windows.get_mut(place)
        })
        .into_any_element()
    }

    fn render_card(
        &mut self,
        index: usize,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let residents = self.seen.residents.as_deref().unwrap_or_default()[index].clone();
        let label = self.seen.labels[index].clone();
        let lit = self.cards[index].lit.value(window).clamp(0., 1.);
        let state = self.cards[index].interaction.state(window);
        let ground = mix(
            colors.surface_container_highest,
            colors.secondary_container,
            lit,
        );
        let count = residents.len();
        let title = div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .text_color(mix(colors.on_surface_variant, colors.on_surface, lit))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .child(one_line(label)),
            )
            .children((count > 0).then(|| div().flex_none().child(count.to_string())));
        let body = if residents.is_empty() {
            div()
                .text_color(colors.on_surface_variant)
                .child(Language::of(cx).say("Empty"))
        } else {
            let windows: Vec<AnyElement> = residents
                .iter()
                .enumerate()
                .map(|(place, resident)| {
                    self.render_window(index, place, resident, theme, window, cx)
                })
                .collect();
            div()
                .flex()
                .flex_wrap()
                .gap(theme.spacing.tiny)
                .children(windows)
        };
        let card = div()
            .id(("overview", index))
            .w_full()
            .p(theme.spacing.medium)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .rounded(theme.shape.large)
            .bg(state_layer(theme, ground, state))
            .text_size(theme.typography.label_size)
            .on_click(cx.listener(move |this, _, _, cx| {
                Kept::show(index, cx);
                this.look(cx);
            }))
            .child(title)
            .child(body);
        Interaction::track(card, cx, move |this: &mut Self| {
            Some(&mut this.cards.get_mut(index)?.interaction)
        })
        .into_any_element()
    }
}

impl Render for Overview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let frame = div()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.small);
        if self.seen.residents.is_none() {
            let said = Language::of(cx).say("Tessera keeps no workspaces of its own");
            return frame.child(
                div()
                    .p(theme.spacing.medium)
                    .text_size(theme.typography.label_size)
                    .text_color(theme.colors.on_surface_variant)
                    .child(said),
            );
        }
        let arrival = self.arrival.value(window).clamp(0., 1.);
        let count = self.cards.len();
        let cards: Vec<AnyElement> = (0..count)
            .map(|index| {
                // Each card arrives a little after the one above it.
                let arrived = staggered(arrival, index, count);
                div()
                    .relative()
                    .top(px(CARD_TRAVEL * (1. - arrived)))
                    .opacity(arrived)
                    .child(self.render_card(index, &theme, window, cx))
                    .into_any_element()
            })
            .collect();
        frame.children(cards)
    }
}
