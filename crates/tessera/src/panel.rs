// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The content of an edge surface: widgets arranged in three slots.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, App, Bounds, Context, Div, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, StatefulInteractiveElement, Styled, Window, canvas, div,
};

use crate::config::{SurfaceConfig, WidgetSpec};
use crate::module::WidgetRegistry;
use crate::motion::{Interaction, Spring};
use crate::platform::{DisplayKey, ScreenEdge};
use crate::popup::{PopupAnchor, Popups};
use crate::theme::Theme;
use crate::ui::{HOVER_LAYER, PRESS_LAYER, centered_row};

/// A widget in a slot, and the popup a click on it opens.
struct Placed {
    view: AnyView,
    popup: Option<String>,
    interaction: Interaction,
    /// The middle of the widget in its surface, once it has been drawn:
    /// where its popup belongs when something other than a click opens it.
    middle: Rc<Cell<Option<Pixels>>>,
}

/// Hosts the widgets a layout assigns to one surface.
pub struct Panel {
    /// The start, center and end slots.
    slots: [Vec<Placed>; 3],
    /// The edge the content slides in from when the surface opens.
    edge: ScreenEdge,
    /// How far the content has come in: 0 is hidden behind the edge.
    entrance: Spring,
}

impl Panel {
    /// Instantiates the widgets `layout` names. Unknown names are reported
    /// and left out.
    pub fn new(layout: &SurfaceConfig, display: Option<DisplayKey>, cx: &mut App) -> Self {
        let motion = cx.global::<Theme>().motion;
        let mut build = |specs: &[WidgetSpec]| -> Vec<Placed> {
            specs
                .iter()
                .filter_map(|spec| {
                    Some(Placed {
                        view: WidgetRegistry::build(spec, display, cx)?,
                        popup: cx.global::<WidgetRegistry>().popup_of(spec),
                        interaction: Interaction::new(&motion),
                        middle: Rc::default(),
                    })
                })
                .collect()
        };
        let slots = [
            build(&layout.start),
            build(&layout.center),
            build(&layout.end),
        ];
        let mut entrance = Spring::new(motion.spatial, 0.);
        entrance.animate_to(1.);
        Self {
            slots,
            edge: layout.edge,
            entrance,
        }
    }

    /// The middle of the first widget that opens the popup `name`, if
    /// there is one and it has been drawn.
    pub fn popup_middle(&self, name: &str) -> Option<Pixels> {
        let mut placed = self.slots.iter().flatten();
        placed
            .find(|placed| placed.popup.as_deref() == Some(name))?
            .middle
            .get()
    }

    fn open_popup(&self, name: &str, window: &Window, cx: &mut App) {
        match PopupAnchor::at(window, window.mouse_position().x, cx) {
            Ok(anchor) => Popups::toggle(name, anchor, cx),
            Err(error) => report!("the popup `{name}` has nowhere to open: {error:#}"),
        }
    }

    /// The widget at `index` of `slot`. One that opens a popup is wrapped in
    /// a button with a state layer over it.
    fn widget(
        &mut self,
        slot: usize,
        index: usize,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let placed = &mut self.slots[slot][index];
        let Some(popup) = placed.popup.clone() else {
            return placed.view.clone().into_any_element();
        };
        let state = placed.interaction.state(window);
        let layer = HOVER_LAYER * state.hover + PRESS_LAYER * state.press.clamp(0., 1.);
        let middle = placed.middle.clone();
        let measure = move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
            middle.set(Some(bounds.origin.x + bounds.size.width / 2.));
        };
        let button = div()
            .id(("panel-widget", slot * 1000 + index))
            .relative()
            .on_click(cx.listener(move |this, _, window, cx| this.open_popup(&popup, window, cx)))
            .child(placed.view.clone())
            .child(canvas(measure, |_, (), _, _| {}).absolute().size_full())
            .child(
                div()
                    .absolute()
                    .size_full()
                    .top_0()
                    .left_0()
                    .rounded(theme.shape.full)
                    .bg(theme.colors.on_surface.opacity(layer)),
            );
        Interaction::track(button, cx, move |this: &mut Self| {
            Some(&mut this.slots.get_mut(slot)?.get_mut(index)?.interaction)
        })
        .into_any_element()
    }

    fn slot(&mut self, slot: usize, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> Div {
        let widgets: Vec<AnyElement> = (0..self.slots[slot].len())
            .map(|index| self.widget(slot, index, theme, window, cx))
            .collect();
        div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .children(widgets)
    }
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let entrance = self.entrance.value(window);
        let hidden = window.viewport_size().height * (1. - entrance);
        let offset = match self.edge {
            ScreenEdge::Top => -hidden,
            ScreenEdge::Bottom => hidden,
        };
        let [start, center, end] = [0, 1, 2].map(|slot| self.slot(slot, &theme, window, cx));
        div()
            .size_full()
            .relative()
            .top(offset)
            .opacity(entrance.clamp(0., 1.))
            .child(centered_row(&theme, start, center, end))
    }
}
