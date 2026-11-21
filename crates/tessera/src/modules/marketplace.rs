// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Offers the extensions of the registries for installing, updating and
//! removing. Widget: `marketplace`.

use std::mem;

use anyhow::Context as _;
use gpui::{
    AnyElement, AppContext, AsyncApp, Context, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};

use crate::extension::extensions_dir;
use crate::extension::registry::{Catalog, Installed, Offer, Package};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, InteractionState, Spring};
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::state_layer;

/// The size of secondary text, as a share of the label size.
const CAPTION_SHARE: f32 = 0.85;

/// The most room the question below an extension takes.
const QUESTION_ROOM: Pixels = px(160.);

/// How far below its place an extension starts when it appears.
const ENTRANCE_OFFSET: Pixels = px(16.);

pub struct MarketplaceModule {
    /// Where the registries to read are.
    pub registries: Vec<String>,
}

impl Module for MarketplaceModule {
    fn name(&self) -> &'static str {
        "marketplace"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        let registries = self.registries.clone();
        widgets.register("marketplace", move |_, cx| {
            cx.new(|cx| Marketplace::new(registries.clone(), cx)).into()
        })
    }
}

/// The extensions that are installed now.
fn installed() -> Vec<Installed> {
    extensions_dir().map_or_else(Vec::new, |directory| Installed::list(&directory))
}

/// What the user is doing with an offered extension.
enum Stage {
    Idle,
    Downloading,
    /// Downloaded and checked; the user is asked to allow what it asks for.
    Offered(Package),
    /// The user is asked whether to delete it.
    Leaving,
}

struct Row {
    offer: Offer,
    installed: Option<Installed>,
    stage: Stage,
    /// Why the last attempt failed.
    failure: Option<SharedString>,
    /// 0 when the row appears, 1 once it is in place.
    presence: Spring,
    /// How far the question below the row is unfolded, 0 to 1.
    question: Spring,
    buttons: [Interaction; 2],
}

impl Row {
    /// Moves on to `stage`. Its buttons are other ones than those shown
    /// until now, so they start out untouched.
    fn turn(&mut self, stage: Stage, cx: &Context<Marketplace>) -> Stage {
        self.buttons = [Interaction::new(&cx.global::<Theme>().motion); 2];
        mem::replace(&mut self.stage, stage)
    }

    fn refresh(&mut self) {
        self.installed = installed()
            .into_iter()
            .find(|installed| installed.id == self.offer.id);
    }
}

pub struct Marketplace {
    registries: Vec<String>,
    reading: bool,
    problems: Vec<SharedString>,
    rows: Vec<Row>,
    reload: Interaction,
}

/// What a press of a button of the row at an index does.
type Action = fn(&mut Marketplace, usize, &mut Context<Marketplace>);

/// Something a row lets the user do.
struct Choice {
    label: &'static str,
    /// Whether it is what the row suggests.
    prominent: bool,
    action: Action,
}

impl Choice {
    fn suggested(label: &'static str, action: Action) -> Self {
        Self {
            label,
            prominent: true,
            action,
        }
    }

    fn other(label: &'static str, action: Action) -> Self {
        Self {
            label,
            prominent: false,
            action,
        }
    }
}

impl Marketplace {
    fn new(registries: Vec<String>, cx: &mut Context<Self>) -> Self {
        let theme = cx.global::<Theme>();
        let mut this = Self {
            registries,
            reading: false,
            problems: Vec::new(),
            rows: Vec::new(),
            reload: Interaction::new(&theme.motion),
        };
        this.read(cx);
        this
    }

    /// Reads the registries again; what they offer replaces the rows.
    fn read(&mut self, cx: &mut Context<Self>) {
        if mem::replace(&mut self.reading, true) {
            return;
        }
        let registries = self.registries.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let catalog = cx
                .background_executor()
                .spawn(async move { Catalog::browse(&registries) })
                .await;
            this.update(cx, |this, cx| this.show(catalog, cx)).ok();
        })
        .detach();
        cx.notify();
    }

    fn show(&mut self, catalog: Catalog, cx: &mut Context<Self>) {
        let motion = cx.global::<Theme>().motion;
        let mut installed = installed();
        self.reading = false;
        self.problems = catalog.problems.into_iter().map(Into::into).collect();
        let rows = catalog.offers.into_iter().map(|offer| {
            let known = installed.iter().position(|known| known.id == offer.id);
            let mut presence = Spring::new(motion.spatial, 0.);
            presence.animate_to(1.);
            Row {
                installed: known.map(|index| installed.swap_remove(index)),
                offer,
                stage: Stage::Idle,
                failure: None,
                presence,
                question: Spring::new(motion.spatial, 0.),
                buttons: [Interaction::new(&motion); 2],
            }
        });
        self.rows = rows.collect();
        cx.notify();
    }

    /// Downloads the extension of the row, then asks before installing it.
    fn download(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get_mut(index) else {
            return;
        };
        row.turn(Stage::Downloading, cx);
        row.failure = None;
        let offer = row.offer.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let id = offer.id.clone();
            let package = cx
                .background_executor()
                .spawn(async move { offer.fetch() })
                .await;
            let shown = this.update(cx, |this, cx| {
                // The registries may have been read again meanwhile.
                let waiting = this
                    .rows
                    .iter_mut()
                    .find(|row| row.offer.id == id && matches!(row.stage, Stage::Downloading));
                let Some(row) = waiting else {
                    return;
                };
                match package {
                    Ok(package) => {
                        row.turn(Stage::Offered(package), cx);
                        row.question.animate_to(1.);
                    }
                    Err(error) => {
                        row.turn(Stage::Idle, cx);
                        row.failure = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            });
            shown.ok();
        })
        .detach();
        cx.notify();
    }

    /// Asks whether to delete the extension of the row.
    fn ask_to_remove(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get_mut(index) {
            row.turn(Stage::Leaving, cx);
            row.failure = None;
            row.question.animate_to(1.);
            cx.notify();
        }
    }

    /// Does what the row asked about: installs or deletes its extension,
    /// and has the shell load the extensions again.
    fn confirm(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get_mut(index) else {
            return;
        };
        let done = match row.turn(Stage::Idle, cx) {
            Stage::Offered(package) => extensions_dir()
                .context("the configuration directory is unknown")
                .and_then(|directory| package.install(&directory)),
            Stage::Leaving => row.installed.as_ref().map_or(Ok(()), Installed::remove),
            stage => {
                row.stage = stage;
                return;
            }
        };
        row.question.animate_to(0.);
        row.failure = done.err().map(|error| format!("{error:#}").into());
        row.refresh();
        cx.defer(Shell::extensions_changed);
        cx.notify();
    }

    fn cancel(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get_mut(index) {
            row.turn(Stage::Idle, cx);
            row.question.animate_to(0.);
            cx.notify();
        }
    }

    /// A button of the row at `index`; `slot` says which of its two
    /// interactions it drives.
    fn button(
        index: usize,
        slot: usize,
        choice: Choice,
        state: InteractionState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Choice {
            label,
            prominent,
            action,
        } = choice;
        let base = if prominent {
            theme.colors.secondary_container
        } else {
            theme.colors.surface_container_highest
        };
        let button = div()
            .id((label, index))
            .flex_none()
            .px(theme.spacing.small + theme.spacing.tiny)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.full)
            .text_color(theme.colors.primary)
            .bg(state_layer(theme, base, state))
            .on_click(cx.listener(move |this, _, _, cx| action(this, index, cx)))
            .child(label);
        Interaction::track(button, cx, move |this: &mut Self| {
            Some(&mut this.rows.get_mut(index)?.buttons[slot])
        })
        .into_any_element()
    }

    fn render_row(
        index: usize,
        row: &mut Row,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let presence = row.presence.value(window);
        let question = row.question.value(window).max(0.);
        let states = row.buttons.each_mut().map(|button| button.state(window));
        let caption_size = theme.typography.label_size * CAPTION_SHARE;
        let mut button = |slot: usize, choice: Choice| {
            Self::button(index, slot, choice, states[slot], theme, cx)
        };
        let caption = |text: SharedString| {
            div()
                .text_size(caption_size)
                .text_color(theme.colors.on_surface_variant)
                .child(text)
        };

        let version = row.installed.as_ref().map(|known| known.version.as_deref());
        let mut actions: Vec<AnyElement> = Vec::new();
        match (&row.stage, version) {
            (Stage::Idle, None) => {
                actions.push(button(0, Choice::suggested("Install", Self::download)))
            }
            (Stage::Idle, Some(None)) => {}
            (Stage::Idle, Some(Some(version))) => {
                if version != row.offer.version {
                    actions.push(button(0, Choice::suggested("Update", Self::download)));
                }
                actions.push(button(1, Choice::other("Remove", Self::ask_to_remove)));
            }
            (Stage::Downloading, _) => {
                actions.push(caption("Downloading…".into()).into_any_element());
            }
            (Stage::Offered(_) | Stage::Leaving, _) => {}
        }
        let title = div()
            .flex_1()
            .min_w(px(0.))
            .truncate()
            .text_color(theme.colors.on_surface)
            .child(SharedString::from(format!(
                "{} {}",
                row.offer.name, row.offer.version
            )));
        let header = div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(title)
            .children(actions);
        let origin = div()
            .text_size(caption_size)
            .text_color(theme.colors.primary)
            .child(SharedString::from(format!(
                "{} · {}",
                row.offer.id, row.offer.registry
            )));
        let description = (!row.offer.description.is_empty())
            .then(|| caption(row.offer.description.clone().into()));
        let status = row.installed.as_ref().map(|known| {
            let version = match &known.version {
                Some(version) => format!("Installed {version}"),
                None => "Installed by hand, so left alone".to_owned(),
            };
            caption(
                format!(
                    "{version}. Widgets for the layout: {}",
                    known.widgets.join(", ")
                )
                .into(),
            )
        });

        let asked = match &row.stage {
            Stage::Offered(package) => Some((asks(package), "Allow and install")),
            Stage::Leaving => Some((
                "Deletes the extension and what it remembered.".to_owned(),
                "Remove",
            )),
            Stage::Idle | Stage::Downloading => None,
        };
        let asked = asked.map(|(text, accept)| {
            div()
                .pt(theme.spacing.small)
                .flex()
                .flex_col()
                .gap(theme.spacing.small)
                .child(caption(text.into()).text_color(theme.colors.on_surface))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(theme.spacing.small)
                        .child(button(1, Choice::other("Cancel", Self::cancel)))
                        .child(button(0, Choice::suggested(accept, Self::confirm))),
                )
        });
        // The question unfolds and folds; while it folds it is already empty.
        let asked = div()
            .max_h(QUESTION_ROOM * question)
            .opacity(question.clamp(0., 1.))
            .overflow_hidden()
            .children(asked);
        let failure = row.failure.clone().map(caption);

        let card = div()
            .px(theme.spacing.medium - theme.spacing.tiny)
            .py(theme.spacing.small)
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container_highest)
            .child(header)
            .child(origin)
            .children(description)
            .children(status)
            .child(asked)
            .children(failure);
        div()
            .w_full()
            .flex_none()
            .pt(ENTRANCE_OFFSET * (1. - presence).max(0.))
            .pb(theme.spacing.small)
            .opacity(presence.clamp(0., 1.))
            .child(card)
            .into_any_element()
    }
}

/// What installing `package` allows it, in a sentence.
fn asks(package: &Package) -> String {
    let mut wishes = Vec::new();
    if !package.network.is_empty() {
        wishes.push(format!("contact {}", package.network.join(", ")));
    }
    if !package.files.is_empty() {
        wishes.push(format!("read {}", package.files.join(", ")));
    }
    if !package.devices.is_empty() {
        let kinds: Vec<_> = package.devices.iter().map(|kind| kind.name()).collect();
        wishes.push(format!("be told of the {} devices", kinds.join(", ")));
    }
    if wishes.is_empty() {
        "Asks for nothing beyond showing its widgets.".to_owned()
    } else {
        format!("Asks to {}.", wishes.join(" and to "))
    }
}

impl Render for Marketplace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let caption_size = theme.typography.label_size * CAPTION_SHARE;
        let mut rows = mem::take(&mut self.rows);
        let cards: Vec<AnyElement> = rows
            .iter_mut()
            .enumerate()
            .map(|(index, row)| Self::render_row(index, row, &theme, window, cx))
            .collect();
        self.rows = rows;

        let state = self.reload.state(window);
        let reload = div()
            .id("read-registries")
            .px(theme.spacing.small + theme.spacing.tiny)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.full)
            .text_color(theme.colors.primary)
            .bg(state_layer(&theme, theme.colors.surface_container, state))
            .on_click(cx.listener(|this, _, _, cx| this.read(cx)))
            .child("Refresh");
        let reload = Interaction::track(reload, cx, |this: &mut Self| Some(&mut this.reload));
        let header = div()
            .h(theme.bar.pill_height)
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .text_color(theme.colors.on_surface)
            .child("Extensions")
            .children((!self.reading).then_some(reload));
        let problems = self.problems.iter().map(|problem| {
            div()
                .flex_none()
                .pb(theme.spacing.tiny)
                .text_size(caption_size)
                .text_color(theme.colors.on_surface_variant)
                .child(problem.clone())
        });

        let note = |text: &'static str| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.colors.on_surface_variant)
                .child(text)
                .into_any_element()
        };
        let list = match (cards.is_empty(), self.reading) {
            (true, true) => note("Reading the registries…"),
            (true, false) => note("The registries offer no extensions"),
            (false, _) => div()
                .id("offered-extensions")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(div().w_full().flex().flex_col().children(cards))
                .into_any_element(),
        };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .text_size(theme.typography.label_size)
            .child(header)
            .children(problems)
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::DeviceKind;

    #[test]
    fn what_an_extension_asks_for_is_said_in_full() {
        let package = |network: &[&str], files: &[&str], devices: &[DeviceKind]| {
            let names = |names: &[&str]| names.iter().map(|&name| name.to_owned()).collect();
            Package::asking(names(network), names(files), devices.to_vec())
        };
        assert_eq!(
            asks(&package(&[], &[], &[])),
            "Asks for nothing beyond showing its widgets."
        );
        assert_eq!(
            asks(&package(&["a.example", "b.example"], &["~/x"], &[])),
            "Asks to contact a.example, b.example and to read ~/x."
        );
        assert_eq!(
            asks(&package(&[], &[], &[DeviceKind::Bluetooth])),
            "Asks to be told of the bluetooth devices."
        );
    }
}
