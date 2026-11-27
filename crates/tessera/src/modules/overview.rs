// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `overview` widget, for a popup: a card for each workspace of
//! Tessera's own with its windows spread over it, those of the workspace
//! shown as live pictures that Windows draws and keeps up to date, the
//! others as their icons. A click on a card goes to that workspace, a
//! click on a window to that window, and a window dragged onto another
//! card has its window live there.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Bounds, Context, FontWeight, InteractiveElement,
    IntoElement, ObjectFit, ParentElement, Pixels, Render, RenderImage, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, WeakEntity, Window, canvas, div, img, px,
};

use crate::i18n::Language;
use crate::icons::AppIcons;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix, staggered};
use crate::platform::{AppWindowId, DisplayKey, NativeSurface, ScreenRect, Thumbnail};
use crate::theme::Theme;
use crate::ui::spread::spread;
use crate::ui::{one_line, state_layer};
use crate::workspaces::{Resident, Workspaces as Kept};

/// How often the workspaces are looked at.
const CHECK_INTERVAL: Duration = Duration::from_millis(300);

/// How far below its place a card starts when it arrives, in pixels.
const CARD_TRAVEL: f32 = 12.;

/// How high the room of the windows of a card is, how far apart they
/// are, how wide the room is taken to be before it was measured, and the
/// largest icon of a window without a picture, in logical pixels.
const WINDOWS_HEIGHT: f32 = 112.;
const WINDOWS_GAP: i32 = 6;
const FIRST_WIDTH: f32 = 280.;
const LARGEST_ICON: f32 = 32.;

/// The shape a window without a picture is spread by.
const CARD_SHAPE: (i32, i32) = (160, 100);

/// Where a picture that is not to be seen is put.
const NOWHERE: ScreenRect = ScreenRect {
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
};

pub struct OverviewModule;

impl Module for OverviewModule {
    fn name(&self) -> &'static str {
        "overview"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("overview", |context, cx| {
            // A popup that does not say its display is on the one the
            // pointer was on when it opened.
            let display = context.display.or_else(DisplayKey::under_pointer);
            cx.new(|cx| Overview::new(display, cx)).into()
        })
    }
}

/// A window on its way to another workspace, and what follows the
/// pointer meanwhile.
#[derive(Clone)]
struct Carried {
    id: AppWindowId,
    picture: Option<Arc<RenderImage>>,
}

impl Render for Carried {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let size = theme.launcher.icon_size;
        div()
            .p(theme.spacing.tiny)
            .rounded(theme.shape.small)
            .bg(theme.colors.surface_container_highest)
            .border_1()
            .border_color(theme.colors.primary)
            .child(match self.picture.clone() {
                Some(picture) => img(picture).size(size).into_any_element(),
                None => div()
                    .size(size)
                    .rounded(theme.shape.small)
                    .bg(theme.colors.outline)
                    .into_any_element(),
            })
    }
}

/// What is shown of the workspaces.
#[derive(Clone, PartialEq, Default)]
struct Seen {
    /// The windows of each workspace; `None` when Tessera keeps none.
    residents: Option<Vec<Vec<Resident>>>,
    current: usize,
    labels: Vec<SharedString>,
    /// Where each window of the workspace shown stands on the screen,
    /// where it is seen there: neither minimized nor hidden.
    frames: HashMap<AppWindowId, ScreenRect>,
}

impl Seen {
    fn now(display: Option<DisplayKey>, cx: &App) -> Self {
        let residents = Kept::residents(display, cx);
        let count = residents.as_ref().map_or(0, Vec::len);
        let current = Kept::now(display, cx).current;
        let shown = { residents.as_deref() }.and_then(|residents| residents.get(current));
        let frames = { shown.into_iter().flatten() }
            .filter(|resident| !resident.id.is_minimized())
            .filter_map(|resident| Some((resident.id, resident.id.frame()?)))
            .collect();
        Self {
            residents,
            current,
            labels: (0..count)
                .map(|index| Kept::label(index, cx).into())
                .collect(),
            frames,
        }
    }
}

/// A live picture of a window, with the physical size of the window.
struct Picture {
    thumbnail: Thumbnail,
    source: (i32, i32),
}

impl Picture {
    /// Draws the picture over `bounds` of the window, where they lie
    /// wholly within what may be drawn on now; Windows draws it over
    /// everything else, so a picture cut by the edge of a scrolled list
    /// or of the popup is not drawn at all.
    fn place(&self, bounds: Bounds<Pixels>, window: &Window) {
        let mask = window.content_mask().bounds;
        let within = bounds.left() >= mask.left()
            && bounds.top() >= mask.top()
            && bounds.right() <= mask.right()
            && bounds.bottom() <= mask.bottom();
        if !within {
            self.thumbnail.place(NOWHERE, 0.);
            return;
        }
        let scale = window.scale_factor();
        let physical = |length: Pixels| (f32::from(length) * scale).round() as i32;
        let area = ScreenRect {
            left: physical(bounds.left()),
            top: physical(bounds.top()),
            right: physical(bounds.right()),
            bottom: physical(bounds.bottom()),
        };
        self.thumbnail.place(area, 1.);
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
    /// The display whose workspaces are shown, where every display has
    /// its own.
    display: Option<DisplayKey>,
    seen: Seen,
    cards: Vec<Card>,
    /// 0 when the widget appears, 1 once the cards are in place.
    arrival: Spring,
    /// The pictures of the windows of the workspace shown; `None` for a
    /// window Windows gave none of.
    pictures: HashMap<AppWindowId, Option<Rc<Picture>>>,
    /// How wide the room of the windows of a card was drawn last, in
    /// logical pixels.
    width: Rc<Cell<f32>>,
}

impl Overview {
    fn new(display: Option<DisplayKey>, cx: &mut Context<Self>) -> Self {
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
            display,
            seen: Seen::now(display, cx),
            cards: Vec::new(),
            arrival: Spring::new(motion.spatial, 0.),
            pictures: HashMap::new(),
            width: Rc::new(Cell::new(FIRST_WIDTH)),
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
        let seen = Seen::now(self.display, cx);
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

    /// Has a picture for each window of the workspace shown, and none for
    /// the others: a window hidden on another workspace, or minimized,
    /// has no picture for Windows to give. Asked once a window, so that a
    /// window Windows gives none of is not asked again at every frame.
    fn take_pictures(&mut self, window: &Window) {
        let frames = &self.seen.frames;
        self.pictures.retain(|id, picture| {
            let keep = frames.contains_key(id);
            if let (false, Some(picture)) = (keep, picture) {
                picture.thumbnail.place(NOWHERE, 0.);
            }
            keep
        });
        let Ok(native) = NativeSurface::of(window) else {
            return;
        };
        for &id in frames.keys() {
            self.pictures.entry(id).or_insert_with(|| {
                let thumbnail = Thumbnail::of(id, native).ok()?;
                let source = thumbnail.source_size()?;
                Some(Rc::new(Picture { thumbnail, source }))
            });
        }
    }

    /// The shape the window `id` is spread by: that of its picture where
    /// its window stands, of its window, or of a card.
    fn shape_of(&self, id: AppWindowId) -> ScreenRect {
        let frame = self.seen.frames.get(&id).copied();
        let picture = self.pictures.get(&id).cloned().flatten();
        match (frame, picture) {
            (Some(frame), Some(picture)) => ScreenRect {
                right: frame.left + picture.source.0,
                bottom: frame.top + picture.source.1,
                ..frame
            },
            (Some(frame), None) => frame,
            (None, _) => ScreenRect {
                right: CARD_SHAPE.0,
                bottom: CARD_SHAPE.1,
                ..NOWHERE
            },
        }
    }

    /// The window `resident`, the one at `place` of the card at `card`,
    /// at `rect`, in logical pixels of the room of the windows of the
    /// card: its live picture, its still picture, or its icon.
    #[allow(clippy::too_many_arguments, reason = "the parts of one card")]
    fn render_window(
        &mut self,
        card: usize,
        place: usize,
        resident: &Resident,
        rect: ScreenRect,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let icon = resident
            .program
            .as_deref()
            .and_then(|program| cx.global::<AppIcons>().of_executable(program));
        let state = self.cards[card].windows[place].state(window);
        let id = resident.id;
        let carried = Carried {
            id,
            picture: icon.clone(),
        };
        let picture =
            { self.pictures.get(&id).cloned().flatten() }.filter(|_| card == self.seen.current);
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        let side = LARGEST_ICON
            .min(width as f32 - 4.)
            .min(height as f32 - 4.)
            .max(1.);
        // A window hidden on another workspace shows the still picture
        // taken as it left, where there is one.
        let still = match (&picture, card == self.seen.current) {
            (None, false) => Kept::still(id, cx),
            _ => None,
        };
        let content = match (picture, still, icon) {
            // Windows draws the picture over the room it is placed in.
            (Some(picture), _, _) => canvas(
                |_, _, _| {},
                move |bounds, (), window, _| {
                    picture.place(bounds, window);
                },
            )
            .size_full()
            .into_any_element(),
            (None, Some(still), _) => img(still)
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            (None, None, Some(icon)) => img(icon).size(px(side)).into_any_element(),
            (None, None, None) => div()
                .size(px(side))
                .rounded(theme.shape.small)
                .bg(theme.colors.outline)
                .into_any_element(),
        };
        let face = div()
            .id(("resident", id.raw() as usize))
            .absolute()
            .left(px(rect.left as f32))
            .top(px(rect.top as f32))
            .w(px(width as f32))
            .h(px(height as f32))
            .p(px(2.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.small)
            .border_1()
            .border_color(theme.colors.primary.opacity(state.hover.clamp(0., 1.)))
            .bg(state_layer(
                theme,
                theme.colors.surface_container_highest,
                state,
            ))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                Kept::show_window(id, cx);
            })
            .on_drag(carried, |carried: &Carried, _, _, cx| {
                let carried = carried.clone();
                cx.new(|_| carried)
            })
            .child(content);
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
            let room = ScreenRect {
                right: self.width.get().round() as i32,
                bottom: WINDOWS_HEIGHT as i32,
                ..NOWHERE
            };
            let shapes: Vec<ScreenRect> = { residents.iter() }
                .map(|resident| self.shape_of(resident.id))
                .collect();
            let places = spread(&shapes, room, WINDOWS_GAP);
            let windows: Vec<AnyElement> = { residents.iter().zip(places).enumerate() }
                .map(|(place, (resident, rect))| {
                    self.render_window(index, place, resident, rect, theme, window, cx)
                })
                .collect();
            // The room is measured as it is drawn, and the windows are
            // spread anew where it is not as wide as they were spread for.
            let width = self.width.clone();
            let measure = canvas(
                move |bounds, window, _| {
                    let drawn = f32::from(bounds.size.width);
                    if (drawn - width.get()).abs() > 0.5 {
                        width.set(drawn);
                        window.refresh();
                    }
                },
                |_, (), _, _| {},
            )
            .absolute()
            .size_full();
            div()
                .relative()
                .w_full()
                .h(px(WINDOWS_HEIGHT))
                .child(measure)
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
            .border_1()
            .border_color(colors.primary.opacity(0.))
            .bg(state_layer(theme, ground, state))
            .text_size(theme.typography.label_size)
            .on_click(cx.listener(move |this, _, _, cx| {
                Kept::show(index, this.display, cx);
                this.look(cx);
            }))
            .drag_over::<Carried>(move |style, _, _, _| style.border_color(colors.primary))
            .on_drop(cx.listener(move |this, carried: &Carried, _, cx| {
                Kept::move_window(carried.id, index, cx);
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
        self.take_pictures(window);
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
