// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The running shell: the configuration in use, the surfaces that show it,
//! and the watch that applies the configuration again when its files change.

use std::mem;
use std::time::Duration;

use anyhow::bail;
use gpui::{App, AsyncApp, DisplayId, Global, WindowHandle, px};

use crate::config::{Config, Displays};
use crate::desk::Cards;
use crate::extension::installed_stamp;
use crate::icons::AppIcons;
use crate::menu::Menus;
use crate::module::WidgetRegistry;
use crate::modules;
use crate::panel::Panel;
use crate::platform::{DisplayKey, NativeSurface, ScreenEdge, WindowsKey};
use crate::popup::{PopupAnchor, Popups};
use crate::preview::Previews;
use crate::surface::{EdgeSurface, EdgeSurfaceView};
use crate::theme::Theme;
use crate::tiling::Tiler;
use crate::tip::Tips;
use crate::wallpaper::Wallpaper;
use crate::workspaces::Workspaces;

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
    cards: Cards,
    /// What the installed extensions were made of when they were loaded.
    extensions: u64,
}

/// How long after the bars the cards are opened when the shell starts.
const CARDS_AFTER_BARS: Duration = Duration::from_millis(3500);

impl Global for Shell {}

impl Shell {
    /// Applies `config`, opens its surfaces and starts watching its files.
    pub fn start(config: Config, cx: &mut App) -> anyhow::Result<()> {
        cx.set_global(Theme::load(config.theme.as_deref(), config.contrast));
        cx.set_global(config.language);
        Tiler::start(&config.tiling, cx);
        Workspaces::start(&config.workspaces, cx);
        let extensions = installed_stamp();
        cx.set_global(WidgetRegistry::from_modules(&modules::builtin(&config)));
        cx.set_global(Popups::new(&config.popups));
        cx.set_global(Previews::default());
        cx.set_global(Tips::default());
        cx.set_global(Menus::default());
        cx.set_global(AppIcons::new(&config.icons));
        let surfaces = Self::open_surfaces(&config, &mut Vec::new(), cx);
        if surfaces.is_empty() {
            bail!("the layout opened no surface");
        }
        Wallpaper::start(&config.wallpaper, cx);
        cx.set_global(Self {
            config,
            surfaces,
            cards: Cards::default(),
            extensions,
        });
        Self::show_cards_later(cx);
        Self::watch(cx);
        Self::answer_windows_key(cx);
        Ok(())
    }

    /// Takes the Windows key over while the configuration names a popup
    /// for it, and opens that popup whenever the key is pressed by itself.
    fn answer_windows_key(cx: &mut App) {
        Self::take_windows_key(cx);
        let presses = WindowsKey::presses();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while presses.recv().await.is_ok() {
                if cx.update(Self::windows_key_pressed).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn take_windows_key(cx: &App) {
        let taken = cx.global::<Self>().config.windows_key.is_some();
        if let Err(error) = WindowsKey::take_over(taken) {
            report!("the Windows key stays with Windows: {error:#}");
        }
    }

    /// Opens the popup of the Windows key, or closes it when it is open:
    /// at the widget that opens it with a click, on the primary display if
    /// it has one, and in the middle of a surface when no widget does.
    fn windows_key_pressed(cx: &mut App) {
        let this = cx.global::<Self>();
        let Some(name) = this.config.windows_key.clone() else {
            return;
        };
        // The display the pointer is on comes first, then the primary one.
        let primary = cx.primary_display().map(|display| display.id());
        let pointed = DisplayKey::under_pointer();
        let mut surfaces: Vec<_> = this
            .surfaces
            .iter()
            .map(|surface| (Some(surface.display) != primary, surface.window))
            .collect();
        let mut surfaces: Vec<_> = surfaces
            .drain(..)
            .map(|(elsewhere, surface)| {
                let display = surface
                    .update(cx, |_, window, _| NativeSurface::of(window).ok())
                    .ok()
                    .flatten()
                    .map(NativeSurface::display);
                let away = pointed.is_none() || display != pointed;
                ((away, elsewhere), surface)
            })
            .collect();
        surfaces.sort_by_key(|&(order, _)| order);
        let mut anywhere = None;
        for (_, surface) in surfaces {
            let found = surface.update(cx, |view, window, cx| {
                let middle = view.content().read(cx).popup_middle(&name);
                let middle = middle.or_else(|| Popups::standing(&name, window, cx));
                let x = middle.unwrap_or(window.viewport_size().width / 2.);
                (middle.is_some(), PopupAnchor::at(window, x, cx))
            });
            match found {
                Ok((true, Ok(anchor))) => return Popups::toggle(&name, anchor, cx),
                Ok((false, Ok(anchor))) => {
                    anywhere.get_or_insert(anchor);
                }
                _ => {}
            }
        }
        match anywhere {
            Some(anchor) => Popups::toggle(&name, anchor, cx),
            None => report!("the popup `{name}` has nowhere to open"),
        }
    }

    fn watch(cx: &mut App) {
        let mut applied = cx.global::<Self>().config.stamp();
        let mut seen = applied;
        let mut installed = cx.global::<Self>().extensions;
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(WATCH_INTERVAL).await;
                // An extension put into the directory, replaced or taken
                // out of it by hand is loaded or dropped once its files
                // have stayed the same for a moment.
                let found = installed_stamp();
                let settled = found == installed;
                installed = found;
                let changed = cx.update(|cx| {
                    if settled && cx.global::<Self>().extensions != found {
                        Self::extensions_changed(cx);
                    }
                });
                if changed.is_err() {
                    break;
                }
                let Ok(stamp) = cx.update(|cx| cx.global::<Self>().config.stamp()) else {
                    break;
                };
                // Colors that follow the accent color of Windows are
                // derived again when it changes.
                let stale = cx.update(|cx| {
                    let stale = cx.global::<Theme>().is_stale();
                    if stale {
                        let config = &cx.global::<Self>().config;
                        let theme = Theme::load(config.theme.as_deref(), config.contrast);
                        cx.set_global(theme);
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
        cx.set_global(Theme::load(config.theme.as_deref(), config.contrast));
        cx.set_global(config.language);
        Tiler::configure(&config.tiling, cx);
        Workspaces::configure(&config.workspaces, cx);
        Wallpaper::configure(&config.wallpaper, cx);

        let old = &cx.global::<Self>().config;
        let extensions =
            old.extensions != config.extensions || old.registries() != config.registries();
        let icons = old.icons != config.icons;
        let surfaces = old.surfaces != config.surfaces;
        let cards = old.cards != config.cards;
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
            cx.global_mut::<Self>().extensions = installed_stamp();
            cx.set_global(WidgetRegistry::from_modules(&modules::builtin(&config)));
        }
        if icons {
            cx.set_global(AppIcons::new(&config.icons));
        }
        if (extensions || icons || surfaces) && !Self::arrange(&config, cx) {
            config.surfaces = cx.global::<Self>().config.surfaces.clone();
        }
        cx.global_mut::<Self>().config = config;
        // Widgets keep what they were built with, here as on the surfaces.
        if extensions || icons || cards {
            Self::show_cards(cx);
        }
        Self::take_windows_key(cx);
        cx.refresh_windows();
    }

    /// Takes over the configuration file right after Tessera wrote it,
    /// without waiting to notice the change.
    pub fn config_changed(cx: &mut App) {
        Self::reload(cx, true);
    }

    /// Loads the extensions again and builds the widgets again, after an
    /// extension was installed or removed. The popups are made again as
    /// well, as an extension may bring some.
    pub fn extensions_changed(cx: &mut App) {
        let config = cx.global::<Self>().config.clone();
        cx.global_mut::<Self>().extensions = installed_stamp();
        Popups::close(cx);
        cx.set_global(Popups::new(&config.popups));
        cx.set_global(WidgetRegistry::from_modules(&modules::builtin(&config)));
        Self::arrange(&config, cx);
        Self::show_cards(cx);
        cx.refresh_windows();
    }

    /// Opens the cards once what the bars took to be made has been let
    /// go. Every window is charged with some megabytes more for about
    /// three seconds after it is made; bars and cards made together took
    /// the shell above the memory it is to stay under for that long.
    fn show_cards_later(cx: &mut App) {
        if cx.global::<Self>().config.cards.is_empty() {
            return;
        }
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor().timer(CARDS_AFTER_BARS).await;
            cx.update(Self::show_cards).ok();
        })
        .detach();
    }

    /// Opens the cards of the configuration in use in place of the open
    /// ones.
    fn show_cards(cx: &mut App) {
        let mut cards = mem::take(&mut cx.global_mut::<Self>().cards);
        let configs = cx.global::<Self>().config.cards.clone();
        cards.show(&configs, cx);
        cx.global_mut::<Self>().cards = cards;
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
            for (index, layout) in config.surfaces.iter().enumerate() {
                if layout.displays == Displays::Primary && Some(display.id()) != primary {
                    continue;
                }
                let (edge, thickness) = (layout.edge, px(layout.thickness));
                let hide = layout.hide;
                let kept = open
                    .iter()
                    .position(|surface| surface.display == display.id() && surface.edge == edge)
                    .map(|index| open.swap_remove(index))
                    .filter(|surface| {
                        let refilled = surface.window.update(cx, |view, window, cx| {
                            view.refill(thickness, hide, window, cx, |display, cx| {
                                Panel::new(index, layout, display, cx)
                            });
                        });
                        refilled.is_ok()
                    });
                if let Some(surface) = kept {
                    surfaces.push(surface);
                    continue;
                }
                let surface = EdgeSurface {
                    edge,
                    thickness,
                    hide,
                };
                let opened = surface.open(&display, cx, |display, cx| {
                    Panel::new(index, layout, display, cx)
                });
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
