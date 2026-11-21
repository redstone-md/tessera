// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One button per open application window. Widget: `windows`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, InteractiveElement, IntoElement, ParentElement,
    Render, RenderImage, StatefulInteractiveElement, Styled, WeakEntity, Window, div, img,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{AppWindow, AppWindowId, DisplayKey, IconPixels};
use crate::theme::Theme;
use crate::ui::state_layer;

/// How often the window list is re-read.
const REFRESH_INTERVAL: Duration = Duration::from_millis(500);

/// Pixel size icons are extracted at; large enough for 200% scaling.
const ICON_SOURCE_SIZE: u32 = 64;

/// How much the icon under the pointer grows, as a share of its size.
const HOVER_GROWTH: f32 = 0.12;

/// How much a pressed icon shrinks, as a share of its size.
const PRESS_SHRINK: f32 = 0.16;

type Icon = Option<Arc<RenderImage>>;

pub struct WindowsModule;

impl Module for WindowsModule {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("windows", |context, cx| {
            let display = context.display;
            cx.new(|cx| WindowList::new(display, cx)).into()
        })
    }
}

/// One window's button and the motion it is in.
struct DockItem {
    /// Identifies the button for as long as it is shown.
    key: usize,
    window: AppWindow,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    /// 1 while the window is the foreground window.
    active: Spring,
    interaction: Interaction,
    /// The window is gone and its button is shrinking away.
    leaving: bool,
}

/// Lists the application windows of one display and switches between them.
struct WindowList {
    /// The display whose windows are listed; every window when unknown.
    display: Option<DisplayKey>,
    /// In display order, including buttons that are still leaving.
    items: Vec<DockItem>,
    foreground: Option<AppWindowId>,
    next_key: usize,
    /// Icons by executable, including executables known to have none.
    icons: HashMap<PathBuf, Icon>,
}

impl WindowList {
    fn new(display: Option<DisplayKey>, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(REFRESH_INTERVAL).await;
                if this.update(cx, Self::refresh).is_err() {
                    break;
                }
            }
        })
        .detach();

        let mut list = Self {
            display,
            items: Vec::new(),
            foreground: None,
            next_key: 0,
            icons: HashMap::new(),
        };
        list.refresh(cx);
        list
    }

    /// Re-reads the window list and repaints only when it changed. Known
    /// windows keep their place, new ones are appended, and closed ones
    /// start leaving.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let motion = cx.global::<Theme>().motion;
        let mut windows = AppWindow::enumerate();
        windows.retain(|window| self.display.is_none_or(|display| window.display == display));
        let foreground = AppWindowId::foreground();
        let mut changed = foreground != self.foreground;
        self.foreground = foreground;

        for item in &mut self.items {
            let found = windows
                .iter()
                .position(|window| window.id == item.window.id);
            let present = found.is_some();
            if let Some(window) = found.map(|index| windows.remove(index))
                && window != item.window
            {
                item.window = window;
                changed = true;
            }
            if item.leaving == present {
                item.leaving = !present;
                item.presence.animate_to(if present { 1. } else { 0. });
                changed = true;
            }
        }
        for window in windows {
            let mut presence = Spring::new(motion.spatial, 0.);
            presence.animate_to(1.);
            self.items.push(DockItem {
                key: self.next_key,
                window,
                presence,
                active: Spring::new(motion.spatial, 0.),
                interaction: Interaction::new(&motion),
                leaving: false,
            });
            self.next_key += 1;
            changed = true;
        }
        for item in &mut self.items {
            let is_foreground = !item.leaving && foreground == Some(item.window.id);
            item.active.animate_to(if is_foreground { 1. } else { 0. });
        }
        if changed {
            cx.notify();
        }
    }

    fn icon_for(icons: &mut HashMap<PathBuf, Icon>, executable: &Path) -> Icon {
        if let Some(icon) = icons.get(executable) {
            return icon.clone();
        }
        let icon = IconPixels::of_executable(executable, ICON_SOURCE_SIZE).and_then(render_image);
        icons.insert(executable.to_owned(), icon.clone());
        icon
    }

    fn render_item(
        item: &mut DockItem,
        icon: Icon,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let presence = item.presence.value(window);
        let active = item.active.value(window);
        let state = item.interaction.state(window);
        let (id, key) = (item.window.id, item.key);

        let icon_size =
            theme.dock.icon_size * (1. + HOVER_GROWTH * state.hover - PRESS_SHRINK * state.press);
        let background = mix(
            theme.colors.surface_container,
            theme.colors.secondary_container,
            active,
        );
        let button = div()
            .id(("dock-item", key))
            .size(theme.dock.item_size)
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(theme.spacing.tiny)
            .rounded(theme.shape.medium)
            .bg(state_layer(&theme, background, state))
            .on_click(move |_, _, _| id.activate_or_minimize())
            .child(match icon {
                Some(icon) => img(icon).size(icon_size).into_any_element(),
                None => div()
                    .size(icon_size)
                    .rounded(theme.shape.small)
                    .bg(theme.colors.outline)
                    .into_any_element(),
            })
            .child(Self::render_indicator(active, theme));
        let button = Interaction::track(button, cx, move |this: &mut Self| {
            let item = this.items.iter_mut().find(|item| item.key == key)?;
            Some(&mut item.interaction)
        });

        // The slot includes the spacing between buttons, so that a button
        // that comes or goes moves its neighbors smoothly.
        let slot = theme.dock.item_size + theme.spacing.tiny;
        div()
            .w(slot * presence.max(0.))
            .h(theme.dock.item_size)
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .opacity(presence.clamp(0., 1.))
            .child(button)
            .into_any_element()
    }

    /// A dot under every window; it grows into a pill for the foreground one.
    fn render_indicator(active: f32, theme: Theme) -> AnyElement {
        let dot = theme.dock.indicator_height;
        let growth = theme.dock.indicator_active_width - dot;
        div()
            .h(dot)
            .w(dot + growth * active.max(0.))
            .rounded(theme.shape.full)
            .bg(mix(theme.colors.outline, theme.colors.primary, active))
            .into_any_element()
    }
}

fn render_image(pixels: IconPixels) -> Icon {
    let buffer = image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

impl Render for WindowList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let Self { items, icons, .. } = self;
        let elements: Vec<AnyElement> = items
            .iter_mut()
            .map(|item| {
                let executable = item.window.executable.as_deref();
                let icon = executable.and_then(|executable| Self::icon_for(icons, executable));
                Self::render_item(item, icon, theme, window, cx)
            })
            .collect();
        // A button that has finished leaving was just drawn at no width.
        items.retain(|item| !(item.leaving && item.presence.is_settled()));

        if elements.is_empty() {
            return div();
        }
        div()
            .h(theme.dock.container_height)
            .px(theme.spacing.small - theme.spacing.tiny / 2.)
            .flex()
            .items_center()
            .rounded(theme.shape.large)
            .bg(theme.colors.surface_container)
            .children(elements)
    }
}
