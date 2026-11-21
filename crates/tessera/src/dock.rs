// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The dock: one button per open application window.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AsyncApp, Context, InteractiveElement, IntoElement, ParentElement, Render, RenderImage,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div, img,
};

use crate::platform::{AppWindow, AppWindowId, DisplayKey, IconPixels};
use crate::theme::Theme;

/// How often the window list is re-read.
const REFRESH_INTERVAL: Duration = Duration::from_millis(500);

/// Pixel size icons are extracted at; large enough for 200% scaling.
const ICON_SOURCE_SIZE: u32 = 64;

type Icon = Option<Arc<RenderImage>>;

/// Lists the application windows of one display and switches between them.
pub struct Dock {
    /// The display whose windows are listed; every window when unknown.
    display: Option<DisplayKey>,
    windows: Vec<AppWindow>,
    foreground: Option<AppWindowId>,
    /// Icons by executable, including executables known to have none.
    icons: HashMap<PathBuf, Icon>,
}

impl Dock {
    pub fn new(display: Option<DisplayKey>, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(REFRESH_INTERVAL).await;
                if this.update(cx, Self::refresh).is_err() {
                    break;
                }
            }
        })
        .detach();

        let mut dock = Self {
            display,
            windows: Vec::new(),
            foreground: None,
            icons: HashMap::new(),
        };
        dock.refresh(cx);
        dock
    }

    /// Re-reads the window list and repaints only when it changed.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let mut windows = AppWindow::enumerate();
        windows.retain(|window| self.display.is_none_or(|display| window.display == display));
        // Keep a stable order: known windows stay put, new ones append.
        windows.sort_by_key(|window| {
            self.windows
                .iter()
                .position(|known| known.id == window.id)
                .unwrap_or(usize::MAX)
        });
        let foreground = AppWindowId::foreground();
        if windows == self.windows && foreground == self.foreground {
            return;
        }
        self.windows = windows;
        self.foreground = foreground;
        cx.notify();
    }

    fn icon_for(&mut self, executable: &Path) -> Icon {
        if let Some(icon) = self.icons.get(executable) {
            return icon.clone();
        }
        let icon = IconPixels::of_executable(executable, ICON_SOURCE_SIZE).and_then(render_image);
        self.icons.insert(executable.to_owned(), icon.clone());
        icon
    }
}

fn render_image(pixels: IconPixels) -> Icon {
    let buffer = image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

impl Render for Dock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let windows = self.windows.clone();
        let items = windows.into_iter().enumerate().map(|(index, window)| {
            let icon = window
                .executable
                .as_deref()
                .and_then(|executable| self.icon_for(executable));
            let is_foreground = self.foreground == Some(window.id);
            let id = window.id;

            // A pill marks the foreground window, a dot every other one.
            let (indicator_width, indicator_color) = if is_foreground {
                (theme.dock.indicator_active_width, theme.colors.primary)
            } else {
                (theme.dock.indicator_height, theme.colors.outline)
            };
            let indicator = div()
                .h(theme.dock.indicator_height)
                .w(indicator_width)
                .rounded(theme.shape.full)
                .bg(indicator_color);

            div()
                .id(("dock-item", index))
                .size(theme.dock.item_size)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(theme.spacing.tiny)
                .rounded(theme.shape.medium)
                .hover(|style| style.bg(theme.colors.surface_container_highest))
                .on_click(move |_, _, _| id.activate_or_minimize())
                .child(match icon {
                    Some(icon) => img(icon).size(theme.dock.icon_size).into_any_element(),
                    None => div()
                        .size(theme.dock.icon_size)
                        .rounded(theme.shape.small)
                        .bg(theme.colors.outline)
                        .into_any_element(),
                })
                .child(indicator)
        });

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .gap(theme.spacing.small)
            .children(items)
    }
}
