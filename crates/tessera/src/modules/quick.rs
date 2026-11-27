// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The quick settings: tiles that switch something of the machine with a
//! click, as those of Windows 11 do, and the widget `tiles`, which lays
//! tiles out in a grid and unfolds the details of one under its row.
//!
//! The tiles of the modules airplane, hotspot, power_mode and night are
//! built from what this file gives them: a [`Reading`] of something of the
//! machine shared by every widget that shows it and read off the thread of
//! the interface, and a [`TileView`] that shows a [`Face`].

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AnyView, App, AppContext, AsyncApp, Context, Entity, Global, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, div, px,
};

use crate::config::WidgetSpec;
use crate::i18n::Language;
use crate::module::{Module, WidgetContext, WidgetRegistry};
use crate::motion::{Interaction, InteractionState, Spring};
use crate::platform::{DisplayKey, Launch};
use crate::theme::Theme;
use crate::ui::{TileFace, icon, one_line, state_layer, tile, tile_more};

/// How many tiles stand in a row unless the layout says, and the most.
const COLUMNS: usize = 2;
const MOST_COLUMNS: usize = 4;

/// The space between the tile and the button that unfolds its details.
const SPLIT_GAP: f32 = 2.;

/// How far above its place the details start when they unfold.
const DETAIL_TRAVEL: f32 = 8.;

/// Something of the machine the quick settings show: how to ask for it,
/// and how often while a widget shows it.
pub trait Source: 'static {
    type State: Clone + PartialEq + Send + 'static;

    /// How long a reading is taken to hold.
    const INTERVAL: Duration;

    /// Asks the machine. It may wait, so it is called off the thread of
    /// the interface.
    fn read() -> Self::State;
}

/// What was last read of a [`Source`], shared by every widget that shows
/// it, and the changes asked of it.
pub struct Reading<S: Source> {
    /// `None` until the first reading.
    state: Option<S::State>,
    /// Whether it is being read now.
    reading: bool,
    /// Whether it is to be read again once the reading under way is in.
    again: bool,
    /// When it was last read.
    read_at: Option<Instant>,
    /// Whether a change is under way.
    changing: bool,
    /// Counts the changes, so that a reading that began before one is not
    /// taken after it.
    changes: u64,
}

struct SharedReading<S: Source>(Entity<Reading<S>>);

impl<S: Source> Global for SharedReading<S> {}

impl<S: Source> Reading<S> {
    /// The one reading of `S`.
    pub fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(shared) = cx.try_global::<SharedReading<S>>() {
            return shared.0.clone();
        }
        let reading = cx.new(|_| Self {
            state: None,
            reading: false,
            again: false,
            read_at: None,
            changing: false,
            changes: 0,
        });
        cx.set_global(SharedReading(reading.clone()));
        reading
    }

    /// What was last read; `None` before the first reading.
    pub fn state(&self) -> Option<&S::State> {
        self.state.as_ref()
    }

    /// Has the view of `cx` follow `reading`: it is drawn again whenever
    /// the reading changes, and the machine is asked every
    /// [`Source::INTERVAL`] while the view is there. The subscription is
    /// kept by the view.
    pub fn follow<V: 'static>(reading: &Entity<Self>, cx: &mut Context<V>) -> Subscription {
        Self::poll(reading, cx);
        cx.observe(reading, |_, _, cx| cx.notify())
    }

    /// Asks the machine every [`Source::INTERVAL`] while the view of `cx`
    /// is there, without drawing it again: for a view that follows
    /// something made of the reading instead.
    pub fn poll<V: 'static>(reading: &Entity<Self>, cx: &mut Context<V>) {
        let polled = reading.clone();
        cx.spawn(async move |view: WeakEntity<V>, cx: &mut AsyncApp| {
            while view.upgrade().is_some() {
                if polled
                    .update(cx, |reading, cx| reading.ask(false, cx))
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(S::INTERVAL).await;
            }
        })
        .detach();
    }

    /// Reads the machine, unless it was read a moment ago by another
    /// widget or is being read; `now` asks even so.
    fn ask(&mut self, now: bool, cx: &mut Context<Self>) {
        if self.reading {
            self.again |= now;
            return;
        }
        let fresh = self
            .read_at
            .is_some_and(|at| at.elapsed() < S::INTERVAL / 2);
        if fresh && !now {
            return;
        }
        self.reading = true;
        let (changes, during) = (self.changes, self.changing);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let state = cx.background_executor().spawn(async { S::read() }).await;
            this.update(cx, |this, cx| {
                this.reading = false;
                this.read_at = Some(Instant::now());
                // What was read before a change, or during one, is not
                // what the change made. One under way reads again when it
                // is done; after one that is done, it is read again now.
                let stale = during || this.changing || this.changes != changes;
                if !stale && this.state.as_ref() != Some(&state) {
                    this.state = Some(state);
                    cx.notify();
                }
                let again = std::mem::take(&mut this.again);
                if (stale && !this.changing) || again {
                    this.ask(true, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Reads the machine now, or once more when it is being read.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.ask(true, cx);
    }

    /// Does `work` off the thread of the interface, showing `shown` until
    /// the machine is read again after it; `then` is given what the work
    /// gave, after the reading took the change in. A failure is reported.
    /// A change asked while another is under way is dropped, which is said
    /// by `false`: the tile shows the first one until it is done.
    pub fn change<T: Send + 'static>(
        &mut self,
        shown: Option<S::State>,
        work: impl FnOnce() -> Result<T, String> + Send + 'static,
        then: impl FnOnce(T, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.changing {
            return false;
        }
        self.changing = true;
        self.changes += 1;
        if shown.is_some() {
            self.state = shown;
        }
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let done = cx.background_executor().spawn(async move { work() }).await;
            let finished = this.update(cx, |this, cx| {
                this.changing = false;
                this.ask(true, cx);
                cx.notify();
            });
            match done {
                // Once the reading is no longer being updated, so that
                // `then` may change it again.
                Ok(value) if finished.is_ok() => cx.update(|cx| then(value, cx)).unwrap_or(()),
                Ok(_) => {}
                Err(error) => report!("{error}"),
            }
        })
        .detach();
        true
    }
}

/// What a tile shows and what a click on it does.
pub trait Face: Sized + 'static {
    /// What the tile shows, shared with the other widgets of its kind.
    type Model: 'static;

    fn model(&self, cx: &mut App) -> Entity<Self::Model>;

    /// Has the tile follow its model; most models are a [`Reading`],
    /// which is followed with [`Reading::follow`].
    fn follow(model: &Entity<Self::Model>, cx: &mut Context<TileView<Self>>) -> Subscription;

    /// What the tile shows; `cx` reaches what the model is made of.
    fn face(&self, model: &Self::Model, language: Language, cx: &App) -> TileFace;

    /// What a click on a tile that can be used does.
    fn click(&self, model: &Entity<Self::Model>, cx: &mut App);
}

/// A tile, as a widget.
pub struct TileView<F: Face> {
    id: &'static str,
    face: F,
    model: Entity<F::Model>,
    interaction: Interaction,
    /// 0 while what it stands for is off, 1 while on.
    lit: Spring,
    /// Whether the tile has been drawn, after which a change is shown in
    /// motion.
    drawn: bool,
    _follow: Subscription,
}

impl<F: Face> TileView<F> {
    fn new(id: &'static str, face: F, cx: &mut Context<Self>) -> Self {
        let model = face.model(cx);
        let follow = F::follow(&model, cx);
        let motion = cx.global::<Theme>().motion;
        Self {
            id,
            face,
            model,
            interaction: Interaction::new(&motion),
            lit: Spring::new(motion.effects, 0.),
            drawn: false,
            _follow: follow,
        }
    }
}

impl<F: Face> Render for TileView<F> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let face = self.face.face(self.model.read(cx), Language::of(cx), cx);
        let lit = if face.lit { 1. } else { 0. };
        match self.drawn {
            true => self.lit.animate_to(lit),
            false => self.lit.jump_to(lit),
        }
        self.drawn = true;
        let state = self.interaction.state(window);
        let shown = tile(&theme, self.id, &face, self.lit.value(window), state);
        if !face.usable {
            return shown.into_any_element();
        }
        let shown = shown.on_click(cx.listener(|this, _, _, cx| {
            let model = this.model.clone();
            this.face.click(&model, cx);
        }));
        Interaction::track(shown, cx, |this: &mut Self| Some(&mut this.interaction))
            .into_any_element()
    }
}

/// Makes the tile `make` builds for each placement available as `id`.
pub fn register_tile<F: Face>(
    widgets: &mut WidgetRegistry,
    id: &'static str,
    make: impl Fn(&WidgetContext) -> F + 'static,
) -> anyhow::Result<()> {
    widgets.register(id, move |context, cx| {
        let face = make(context);
        cx.new(|cx| TileView::new(id, face, cx)).into()
    })
}

/// A small rounded button with a text, filled while it is `chosen`: one of
/// a few choices that stand in a row.
pub fn chip(
    theme: &Theme,
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    chosen: bool,
    state: InteractionState,
) -> gpui::Stateful<gpui::Div> {
    let colors = theme.colors;
    let ground = match chosen {
        true => colors.secondary_container,
        false => colors.surface_container_highest,
    };
    div()
        .id(id)
        .flex_none()
        .h(px(28.))
        .px(theme.spacing.medium)
        .flex()
        .items_center()
        .rounded(theme.shape.full)
        .bg(state_layer(theme, ground, state))
        .text_color(match chosen {
            true => colors.on_surface,
            false => colors.on_surface_variant,
        })
        .child(one_line(label.into()))
}

/// How high the row is that leads to a page of the Windows settings.
const LINK_HEIGHT: f32 = 40.;

const ICON_SETTINGS: &str = "\u{e713}";
const ICON_OPEN: &str = "\u{e76c}";

/// A page of the Windows settings that the details of a tile lead to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsPage {
    /// What Windows opens the page by, as `ms-settings:nightlight`.
    pub address: &'static str,
    /// What the row that leads to it says, in English.
    pub text: &'static str,
}

impl SettingsPage {
    /// The row that opens the page, under a pointer in `state`; whoever
    /// shows it tracks the pointer.
    pub fn link(
        self,
        theme: &Theme,
        language: Language,
        state: InteractionState,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(self.address)
            .h(px(LINK_HEIGHT))
            .flex_none()
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .rounded(theme.shape.medium)
            .bg(state_layer(theme, theme.colors.surface_container, state))
            .text_color(theme.colors.on_surface_variant)
            .child(icon(theme, ICON_SETTINGS).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(one_line(language.say(self.text))),
            )
            .child(icon(theme, ICON_OPEN).flex_none())
            .on_click(move |_, _, _| Launch::new(self.address).start())
    }
}

/// The note of a tile while what it stands for is `on` or not.
pub fn on_or_off(on: bool, language: Language) -> &'static str {
    language.say(if on { "On" } else { "Off" })
}

/// The note of a tile that cannot be used: before the machine was first
/// asked, and once it said it has nothing of the kind.
pub fn unusable_note(asked: bool, language: Language) -> &'static str {
    language.say(if asked { "Unavailable" } else { "Reading…" })
}

/// `text` with its first letter made a capital, for a reason that is
/// shown by itself.
pub fn sentence(text: &str) -> String {
    let mut letters = text.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

pub struct QuickModule;

impl Module for QuickModule {
    fn name(&self) -> &'static str {
        "quick"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("tiles", |context, cx| {
            let context = context.clone();
            cx.new(|cx| Tiles::new(&context, cx)).into()
        })
    }
}

/// The details a tile unfolds where the layout names none.
fn default_detail(spec: &WidgetSpec) -> Option<&'static str> {
    Some(match spec.widget.as_str() {
        "airplane" => "radios",
        "hotspot" => "hotspot-details",
        "power-mode" => "power-modes",
        "night-light" => "night-light-slider",
        "radio-tile" => match spec.options.get("radio").and_then(toml::Value::as_str) {
            Some("bluetooth") => "bluetooth",
            Some("wifi") | None => "wifi",
            Some(_) => return None,
        },
        _ => return None,
    })
}

/// The widget the tile `spec` unfolds: the one its option `detail` names,
/// as a name or a table like any widget, none for `""`, or the one that
/// goes with the tile.
fn detail_of(spec: &WidgetSpec) -> Option<WidgetSpec> {
    let named = |widget: &str| WidgetSpec {
        widget: widget.to_owned(),
        popup: None,
        options: toml::Table::new(),
    };
    match spec.options.get("detail") {
        Some(toml::Value::String(name)) if name.is_empty() => None,
        Some(value) => match value.clone().try_into::<WidgetSpec>() {
            Ok(detail) => Some(detail),
            Err(error) => {
                report!(
                    "the option `detail` of `{}` is ignored: {error}",
                    spec.widget
                );
                default_detail(spec).map(named)
            }
        },
        None => default_detail(spec).map(named),
    }
}

/// The widgets the option `tiles` lists, each its name or a table.
fn tile_specs(context: &WidgetContext) -> Vec<WidgetSpec> {
    let Some(listed) = context.options.get("tiles") else {
        return Vec::new();
    };
    let Some(listed) = listed.as_array() else {
        report!("the option `tiles` must be a list of widgets, `{listed}` is ignored");
        return Vec::new();
    };
    listed
        .iter()
        .filter_map(|value| match value.clone().try_into::<WidgetSpec>() {
            Ok(spec) => Some(spec),
            Err(error) => {
                report!("a tile of `tiles` is ignored: {error}");
                None
            }
        })
        .collect()
}

/// How many tiles stand in a row, for the option `columns`.
fn columns_of(asked: Option<f64>) -> usize {
    match asked {
        Some(columns) if columns >= 1. => (columns as usize).min(MOST_COLUMNS),
        _ => COLUMNS,
    }
}

/// One tile of the grid.
struct Cell {
    view: AnyView,
    detail: Option<WidgetSpec>,
    more: Interaction,
}

/// The tiles in rows, and the details of one of them under its row.
struct Tiles {
    display: Option<DisplayKey>,
    cells: Vec<Cell>,
    columns: usize,
    /// The tile whose details are shown, and the widget of them.
    unfolded: Option<(usize, AnyView)>,
    /// 0 when details unfold, 1 once they are in place.
    unfolding: Spring,
}

impl Tiles {
    fn new(context: &WidgetContext, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let cells = tile_specs(context)
            .iter()
            .filter_map(|spec| {
                Some(Cell {
                    view: WidgetRegistry::build(spec, context.display, cx)?,
                    detail: detail_of(spec),
                    more: Interaction::new(&motion),
                })
            })
            .collect();
        Self {
            display: context.display,
            cells,
            columns: columns_of(context.number("columns")),
            unfolded: None,
            unfolding: Spring::new(motion.spatial, 0.),
        }
    }

    /// Unfolds the details of the tile at `index`, or folds them when they
    /// are shown. The widget of the details is made each time they unfold
    /// and dropped when they fold, so that it asks the machine only while
    /// it is seen.
    fn unfold(&mut self, index: usize, cx: &mut Context<Self>) {
        let shown = self.unfolded.as_ref().map(|(at, _)| *at);
        self.unfolded = None;
        if shown != Some(index)
            && let Some(spec) = self.cells.get(index).and_then(|cell| cell.detail.clone())
            && let Some(view) = WidgetRegistry::build(&spec, self.display, cx)
        {
            self.unfolded = Some((index, view));
            self.unfolding.jump_to(0.);
            self.unfolding.animate_to(1.);
        }
        cx.notify();
    }

    fn cell(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let unfolded = self.unfolded.as_ref().is_some_and(|(at, _)| *at == index);
        let cell = &mut self.cells[index];
        let row = div()
            .flex_1()
            .min_w_0()
            .flex()
            .gap(px(SPLIT_GAP))
            .child(div().flex_1().min_w_0().child(cell.view.clone()));
        if cell.detail.is_none() {
            return row.into_any_element();
        }
        let state = cell.more.state(window);
        let more = tile_more(&theme, ("tile-more", index), unfolded, state)
            .on_click(cx.listener(move |this, _, _, cx| this.unfold(index, cx)));
        let more = Interaction::track(more, cx, move |this: &mut Self| {
            this.cells.get_mut(index).map(|cell| &mut cell.more)
        });
        row.child(more).into_any_element()
    }
}

impl Render for Tiles {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let columns = self.columns.max(1);
        let count = self.cells.len();
        let mut rows: Vec<AnyElement> = Vec::new();
        for start in (0..count).step_by(columns) {
            let end = (start + columns).min(count);
            let mut cells: Vec<AnyElement> = (start..end)
                .map(|index| self.cell(index, window, cx))
                .collect();
            // The last row keeps the width of a tile in the others.
            cells.extend((end..start + columns).map(|_| div().flex_1().into_any_element()));
            rows.push(
                div()
                    .w_full()
                    .flex()
                    .gap(theme.spacing.small)
                    .children(cells)
                    .into_any_element(),
            );
            let Some((at, detail)) = &self.unfolded else {
                continue;
            };
            if (start..end).contains(at) {
                let unfolded = self.unfolding.value(window).clamp(0., 1.);
                rows.push(
                    div()
                        .w_full()
                        .relative()
                        .top(px(-DETAIL_TRAVEL * (1. - unfolded)))
                        .opacity(unfolded)
                        .p(theme.spacing.small)
                        .flex()
                        .flex_col()
                        .rounded(theme.shape.medium)
                        .bg(theme.colors.surface_container)
                        .child(detail.clone())
                        .into_any_element(),
                );
            }
        }
        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .children(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(text: &str) -> WidgetSpec {
        let table: toml::Table = toml::from_str(&format!("spec = {text}")).unwrap();
        table["spec"].clone().try_into().unwrap()
    }

    #[test]
    fn a_tile_unfolds_the_details_that_go_with_it() {
        let detail = |text: &str| detail_of(&spec(text)).map(|detail| detail.widget);
        assert_eq!(detail("\"hotspot\"").as_deref(), Some("hotspot-details"));
        assert_eq!(detail("\"airplane\"").as_deref(), Some("radios"));
        assert_eq!(
            detail("{ widget = \"radio-tile\", radio = \"bluetooth\" }").as_deref(),
            Some("bluetooth")
        );
        assert_eq!(
            detail("{ widget = \"radio-tile\" }").as_deref(),
            Some("wifi")
        );
        assert_eq!(detail("\"clock\""), None);
    }

    #[test]
    fn the_layout_chooses_the_details_of_a_tile() {
        let detail = detail_of(&spec(
            "{ widget = \"hotspot\", detail = { widget = \"hotspot-details\", qr = false } }",
        ))
        .unwrap();
        assert_eq!(detail.widget, "hotspot-details");
        assert_eq!(detail.options["qr"], toml::Value::Boolean(false));
        let none = detail_of(&spec("{ widget = \"hotspot\", detail = \"\" }"));
        assert_eq!(none, None);
        let other = detail_of(&spec("{ widget = \"label\", detail = \"calendar\" }"));
        assert_eq!(
            other.map(|detail| detail.widget).as_deref(),
            Some("calendar")
        );
    }

    #[test]
    fn a_reason_shown_by_itself_starts_with_a_capital() {
        assert_eq!(sentence("the band is disturbed"), "The band is disturbed");
        assert_eq!(sentence("ночной свет"), "Ночной свет");
        assert_eq!(sentence(""), "");
    }

    #[test]
    fn the_grid_has_as_many_columns_as_it_is_asked_for_within_reason() {
        assert_eq!(columns_of(None), COLUMNS);
        assert_eq!(columns_of(Some(3.)), 3);
        assert_eq!(columns_of(Some(0.)), COLUMNS);
        assert_eq!(columns_of(Some(9.)), MOST_COLUMNS);
    }
}
