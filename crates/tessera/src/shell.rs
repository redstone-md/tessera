// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The running shell: the configuration in use, the surfaces that show it,
//! and the watch that applies the configuration again when its files change.

use std::mem;
use std::time::Duration;

use anyhow::bail;
use gpui::{App, AsyncApp, DisplayId, Global, WindowHandle, px};

use crate::config::{Config, Displays};
use crate::icons::AppIcons;
use crate::menu::Menus;
use crate::module::WidgetRegistry;
use crate::modules;
use crate::panel::Panel;
use crate::platform::ScreenEdge;
use crate::popup::Popups;
use crate::preview::Previews;
use crate::surface::{EdgeSurface, EdgeSurfaceView};
use crate::theme::Theme;

/// How often the configuration files are checked for changes. A change is
/// applied once the files have stayed the same for one interval, so that a
/// file an editor is still writing is not read.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

/// An open surface and where it is.
struct Surface {
    display: DisplayId,
    edge: ScreenEdge,
    window: WindowHandle<EdgeSurfaceView<Panel>>,
}

pub struct Shell {
    /// The configuration the open surfaces were made from.
    config: Config,
    surfaces: Vec<Surface>,
}

impl Global for Shell {}

impl Shell {
    /// Applies `config`, opens its surfaces and starts watching its files.
    pub fn start(config: Config, cx: &mut App) -> anyhow::Result<()> {
        cx.set_global(Theme::load(config.theme.as_deref()));
        cx.set_global(WidgetRegistry::from_modules(&modules::builtin(&config)));
        cx.set_global(Popups::new(&config.popups));
        cx.set_global(Previews::default());
        cx.set_global(Menus::default());
        cx.set_global(AppIcons::new(&config.icons));
        let surfaces = Self::open_surfaces(&config, &mut Vec::new(), cx);
        if surfaces.is_empty() {
            bail!("the layout opened no surface");
        }
        cx.set_global(Self { config, surfaces });
        Self::watch(cx);
        Ok(())
    }

    fn watch(cx: &mut App) {
        let mut applied = cx.global::<Self>().config.stamp();
        let mut seen = applied;
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(WATCH_INTERVAL).await;
                let Ok(stamp) = cx.update(|cx| cx.global::<Self>().config.stamp()) else {
                    break;
                };
                // Colors that follow the accent color of Windows are
                // derived again when it changes.
                let stale = cx.update(|cx| {
                    let stale = cx.global::<Theme>().is_stale();
                    if stale {
                        let name = cx.global::<Self>().config.theme.clone();
                        cx.set_global(Theme::load(name.as_deref()));
                        cx.refresh_windows();
                    }
                    stale
                });
                if stale.is_err() {
                    break;
                }
                if stamp != applied && stamp == seen {
                    // A new configuration may name another theme file, so
                    // what was applied is asked for again afterwards.
                    let reloaded = cx.update(|cx| {
                        Self::reload(cx, false);
                        cx.global::<Self>().config.stamp()
                    });
                    match reloaded {
                        Ok(stamp) => applied = stamp,
                        Err(_) => break,
                    }
                }
                seen = stamp;
            }
        })
        .detach();
    }

    /// Reads the configuration again and applies it. One that cannot be
    /// used is reported, as when starting, and changes nothing.
    fn reload(cx: &mut App, written_here: bool) {
        let Some(mut config) = Config::reload() else {
            report!("the configuration in use stays");
            return;
        };
        cx.set_global(Theme::load(config.theme.as_deref()));

        let old = &cx.global::<Self>().config;
        let extensions =
            old.extensions != config.extensions || old.registries() != config.registries();
        let icons = old.icons != config.icons;
        let surfaces = old.surfaces != config.surfaces;
        // An open popup stays while it is still what the configuration
        // describes, so that the settings do not close at every change.
        if extensions || icons || (old.popups != config.popups && !written_here) {
            Popups::close(cx);
            cx.set_global(Popups::new(&config.popups));
        } else {
            // The widget that wrote the file already shows what it wrote.
            Popups::reconfigure(&config.popups, cx);
        }
        // Widgets keep what they were built with, so new settings of
        // extensions, new registries and new icons need new widgets as well.
        if extensions {
            cx.set_global(WidgetRegistry::from_modules(&modules::builtin(&config)));
        }
        if icons {
            cx.set_global(AppIcons::new(&config.icons));
        }
        if (extensions || icons || surfaces) && !Self::arrange(&config, cx) {
            config.surfaces = cx.global::<Self>().config.surfaces.clone();
        }
        cx.global_mut::<Self>().config = config;
        cx.refresh_windows();
    }

    /// Takes over the configuration file right after Tessera wrote it,
    /// without waiting to notice the change.
    pub fn config_changed(cx: &mut App) {
        Self::reload(cx, true);
    }

    /// Loads the extensions again and builds the widgets again, after an
    /// extension was installed or removed.
    pub fn extensions_changed(cx: &mut App) {
        let config = cx.global::<Self>().config.clone();
        cx.set_global(WidgetRegistry::from_modules(&modules::builtin(&config)));
        Self::arrange(&config, cx);
        cx.refresh_windows();
    }

    /// Makes the open surfaces those of `config`. A surface that stays at
    /// its edge of its display keeps its window and gets new content; the
    /// others open or close. If nothing would be left open, the surfaces
    /// that were open stay and the answer is `false`.
    fn arrange(config: &Config, cx: &mut App) -> bool {
        let mut old = mem::take(&mut cx.global_mut::<Self>().surfaces);
        let new = Self::open_surfaces(config, &mut old, cx);
        if new.is_empty() {
            report!("the new layout opened no surface, the previous surfaces stay");
            cx.global_mut::<Self>().surfaces = old;
            return false;
        }
        // The surfaces left over close only now that others exist, because
        // GPUI quits when the last window closes.
        for surface in old {
            let closed = surface
                .window
                .update(cx, |_, window, _| window.remove_window());
            closed.ok();
        }
        cx.global_mut::<Self>().surfaces = new;
        true
    }

    /// The surfaces of `config` on the displays each one asks for, taken
    /// out of `open` where one is already at that edge of that display and
    /// opened otherwise. One that does not open is reported and left out.
    fn open_surfaces(config: &Config, open: &mut Vec<Surface>, cx: &mut App) -> Vec<Surface> {
        let primary = cx.primary_display().map(|display| display.id());
        let mut surfaces = Vec::new();
        for display in cx.displays() {
            for layout in &config.surfaces {
                if layout.displays == Displays::Primary && Some(display.id()) != primary {
                    continue;
                }
                let (edge, thickness) = (layout.edge, px(layout.thickness));
                let kept = open
                    .iter()
                    .position(|surface| surface.display == display.id() && surface.edge == edge)
                    .map(|index| open.swap_remove(index))
                    .filter(|surface| {
                        let refilled = surface.window.update(cx, |view, window, cx| {
                            view.refill(thickness, window, cx, |display, cx| {
                                Panel::new(layout, display, cx)
                            });
                        });
                        refilled.is_ok()
                    });
                if let Some(surface) = kept {
                    surfaces.push(surface);
                    continue;
                }
                let opened = EdgeSurface { edge, thickness }
                    .open(&display, cx, |display, cx| Panel::new(layout, display, cx));
                match opened {
                    Ok(window) => surfaces.push(Surface {
                        display: display.id(),
                        edge,
                        window,
                    }),
                    Err(error) => report!("a surface did not open: {error:#}"),
                }
            }
        }
        surfaces
    }
}
