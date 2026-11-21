// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The content of an edge surface: widgets arranged in three slots.

use gpui::{
    AnyElement, AnyView, App, Context, Div, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div,
};

use crate::config::{SurfaceConfig, WidgetSpec};
use crate::module::WidgetRegistry;
use crate::motion::{Interaction, Spring};
use crate::platform::{DisplayKey, NativeSurface, ScreenEdge};
use crate::popup::{PopupAnchor, Popups};
use crate::theme::Theme;
use crate::ui::{HOVER_LAYER, PRESS_LAYER, centered_row};

/// A widget in a slot, and the popup a click on it opens.
struct Placed {
    view: AnyView,
    popup: Option<String>,
    interaction: Interaction,
}

/// Hosts the widgets a layout assigns to one surface.
pub struct Panel {
    /// The start, center and end slots.
    slots: [Vec<Placed>; 3],
    /// The edge the content slides in from when the surface opens.
    edge: ScreenEdge,
    display: Option<DisplayKey>,
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
                        popup: spec.popup.clone(),
                        interaction: Interaction::new(&motion),
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
            display,
            entrance,
        }
    }

    fn open_popup(&self, name: &str, window: &Window, cx: &mut App) {
        let frames = NativeSurface::of(window)
            .and_then(|native| Ok((native.frame()?, native.display_frame()?)));
        let (surface, screen) = match frames {
            Ok(frames) => frames,
            Err(error) => {
                eprintln!("tessera: the popup `{name}` has nowhere to open: {error:#}");
                return;
            }
        };
        let scale = window.scale_factor();
        let anchor = PopupAnchor {
            surface,
            edge: self.edge,
            x: surface.left + (f32::from(window.mouse_position().x) * scale).round() as i32,
            screen,
            scale,
            display_id: window.display(cx).map(|display| display.id()),
            display: self.display,
        };
        Popups::toggle(name, anchor, cx);
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
        let button = div()
            .id(("panel-widget", slot * 1000 + index))
            .relative()
            .on_click(cx.listener(move |this, _, window, cx| this.open_popup(&popup, window, cx)))
            .child(placed.view.clone())
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
