// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The running shell: the configuration in use, the surfaces that show it,
//! and the watch that applies the configuration again when its files change.

use std::mem;
use std::time::Duration;

use anyhow::{Context as _, bail};
use gpui::{App, AsyncApp, DisplayId, Global, Window, WindowHandle, px};

use crate::actions::Actions;
use crate::banners::Banners;
use crate::clipboard::ClipHistory;
use crate::config::Config;
use crate::corners::Corners;
use crate::desk::Cards;
use crate::extension::installed_stamp;
use crate::files::Files;
use crate::flyout::Flyouts;
use crate::icons::AppIcons;
use crate::keys::Shortcuts;
use crate::menu::Menus;
use crate::module::WidgetRegistry;
use crate::modules;
use crate::modules::DesktopClosing;
use crate::modules::night::Night;
use crate::notices::Notices;
use crate::notifications_store::NotificationStore;
use crate::panel::Panel;
use crate::platform::{DisplayKey, NativeSurface, ScreenEdge, Taskbar, WindowsKey};
use crate::platform::{DisplayName, ShellEvent, ShellHost, display_device, display_names};
use crate::popup::{PopupAnchor, Popups};
use crate::preview::Previews;
use crate::rules::Rules;
use crate::snap::Snapper;
use crate::startup::Startup;
use crate::surface::{EdgeSurface, EdgeSurfaceView, edge_of};
use crate::switcher::Switcher;
use crate::theme::Theme;
use crate::tiling::Tiler;
use crate::tip::Tips;
use crate::wallpaper::Wallpaper;
use crate::workspaces::Workspaces;

/// How often the configuration files are checked for changes. A change is
/// applied once the files have stayed the same for one interval, so that a
/// file an editor is still writing is not read.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

/// How long the displays are to stay as they are after a change before
/// the surfaces follow: Windows tells of a change several times while it
/// makes it.
const DISPLAYS_SETTLE: Duration = Duration::from_millis(600);

/// An open surface and where it is.
struct Surface {
    display: DisplayId,
    /// The output of that display, as GDI names it, where it is known:
    /// the ids of GPUI count the displays anew when one comes or goes.
    device: Option<String>,
    edge: ScreenEdge,
    window: WindowHandle<EdgeSurfaceView<Panel>>,
}

impl Surface {
    /// Whether the surface is on the display GPUI calls `display`, whose
    /// output is `device`.
    fn is_on(&self, display: DisplayId, device: Option<&str>) -> bool {
        match (self.device.as_deref(), device) {
            (Some(own), Some(device)) => own.eq_ignore_ascii_case(device),
            _ => self.display == display,
        }
    }
}

/// What the displays are called and where they stand, as read when the
/// shell started and again whenever the displays changed.
#[derive(Default)]
pub struct KnownDisplays(Vec<DisplayName>);

impl Global for KnownDisplays {}

impl KnownDisplays {
    /// The displays, as last read.
    pub fn all(cx: &App) -> &[DisplayName] {
        cx.try_global::<Self>().map_or(&[], |known| &known.0)
    }

    /// What the display GPUI calls `display` is called, and its output.
    fn name_of(display: DisplayId, cx: &App) -> (Option<DisplayName>, Option<String>) {
        let device = display_device(u32::from(display) as usize);
        let named = device.as_deref().and_then(|device| {
            let known = Self::all(cx).iter();
            { known }.find(|name| name.device.eq_ignore_ascii_case(device))
        });
        (named.cloned(), device)
    }

    /// The display the surface in `window` is on, as last read.
    pub fn of_window(window: &Window, cx: &App) -> Option<DisplayName> {
        let display = window.display(cx)?.id();
        Self::name_of(display, cx).0
    }
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
        let modules = modules::builtin(&config);
        Actions::start(&modules, cx);
        cx.set_global(WidgetRegistry::from_modules(&modules));
        cx.set_global(Popups::new(&config.popups));
        cx.set_global(Previews::default());
        cx.set_global(Tips::default());
        cx.set_global(Menus::default());
        cx.set_global(AppIcons::new(&config.icons));
        // Without Explorer the surfaces reserve their edges through the tray.
        crate::platform::Tray::start_before_surfaces();
        Night::start(&config.night_light, cx);
        // The surfaces choose their displays by these names, so they are
        // read before the first opens; it asks the display driver once.
        cx.set_global(KnownDisplays(display_names()));
        let surfaces = Self::open_surfaces(&config, &mut Vec::new(), cx);
        if surfaces.is_empty() {
            bail!("the layout opened no surface");
        }
        crate::session::surfaces_ready(cx);
        Wallpaper::start(&config.wallpaper, cx);
        Banners::configure(&config.notifications, cx);
        NotificationStore::configure(&config.notifications, &config.notification_rules, cx);
        Flyouts::configure(&config.flyout, cx);
        Switcher::configure(&config.switcher, cx);
        Files::configure(&config.files, cx);
        Startup::start(&config.startup, cx);
        Snapper::start(&config.snap, config.windows_keys, &config.keys, cx);
        Rules::start(&config.rules, cx);
        Corners::start(&config.corners, cx);
        ClipHistory::start(&config.clipboard, cx);
        Notices::configure(&config.notices, cx);
        // The keys come last: what a `[[key]]` names is checked against
        // the actions, which every part started above has added by now.
        Shortcuts::start(&config.keys, config.windows_keys, cx);
        DesktopClosing::start(config.windows_keys, cx);
        cx.set_global(Self {
            config,
            surfaces,
            cards: Cards::default(),
            extensions,
        });
        Self::show_cards_later(cx);
        Self::watch(cx);
        Self::answer_windows_key(cx);
        Self::follow_displays(cx);
        Ok(())
    }

    /// Has the surfaces follow the displays: when one comes, goes or
    /// changes, and the displays have stayed as they are for a moment,
    /// their names are read again away from the thread of the interface
    /// and the surfaces open, close or are refilled as the layout says
    /// for the displays there are now.
    fn follow_displays(cx: &mut App) {
        let events = ShellHost::events();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(event) = events.recv().await {
                if !matches!(event, ShellEvent::DisplaysChanged) {
                    continue;
                }
                // What else comes meanwhile is the same change.
                loop {
                    cx.background_executor().timer(DISPLAYS_SETTLE).await;
                    let mut more = false;
                    while events.try_recv().is_ok() {
                        more = true;
                    }
                    if !more {
                        break;
                    }
                }
                let names = cx
                    .background_executor()
                    .spawn(async { display_names() })
                    .await;
                let followed = cx.update(|cx| {
                    cx.set_global(KnownDisplays(names));
                    let config = cx.global::<Self>().config.clone();
                    Self::arrange(&config, cx);
                    cx.refresh_windows();
                });
                if followed.is_err() {
                    break;
                }
            }
        })
        .detach();
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

    /// Answers Ctrl+Esc and the Windows key, which Windows hands to the
    /// shell of the session: opens the popup of the Windows key, or the
    /// launcher where the configuration names none, or closes it.
    pub fn start_requested(cx: &mut App) {
        let named = cx.global::<Self>().config.windows_key.clone();
        Self::toggle_popup(named.as_deref().unwrap_or("launcher"), cx);
    }

    /// Opens the popup of the Windows key, or closes it when it is open.
    fn windows_key_pressed(cx: &mut App) {
        if let Some(name) = cx.global::<Self>().config.windows_key.clone() {
            Self::toggle_popup(&name, cx);
        }
    }

    /// Opens the popup `name`, or closes it when it is open: at the widget
    /// that opens it with a click, on the display the pointer is on if it
    /// has one, and in the middle of a surface when no widget does.
    pub fn toggle_popup(name: &str, cx: &mut App) {
        match Self::popup_anchor(name, cx) {
            Some((_, anchor)) => Popups::toggle(name, anchor, cx),
            None => report!("the popup `{name}` has nowhere to open"),
        }
    }

    /// Where the popup `name` opens: at the widget that opens it with a
    /// click, on the display the pointer is on if it has one, with `true`;
    /// in the middle of a surface when no widget does, with `false`; and
    /// `None` when there is no surface.
    pub fn popup_anchor(name: &str, cx: &mut App) -> Option<(bool, PopupAnchor)> {
        let mut anywhere = None;
        for surface in Self::surfaces_nearest_first(cx) {
            let found = surface.update(cx, |view, window, cx| {
                let viewport = window.viewport_size();
                let halfway = gpui::point(viewport.width / 2., viewport.height / 2.);
                let middle = view.content().read(cx).popup_center(name);
                // Where a widget said it opens the popup, along the surface.
                let standing = || {
                    let along = Popups::standing(name, window, cx)?;
                    let vertical = edge_of(window, cx).is_some_and(ScreenEdge::is_vertical);
                    Some(match vertical {
                        true => gpui::point(halfway.x, along),
                        false => gpui::point(along, halfway.y),
                    })
                };
                let middle = middle.or_else(standing);
                let at = middle.unwrap_or(halfway);
                (middle.is_some(), PopupAnchor::at_point(window, at, cx))
            });
            match found {
                Ok((true, Ok(anchor))) => return Some((true, anchor)),
                Ok((false, Ok(anchor))) => {
                    anywhere.get_or_insert(anchor);
                }
                _ => {}
            }
        }
        anywhere.map(|anchor| (false, anchor))
    }

    /// Where a window opens that stands in the middle of a display: on
    /// the display the pointer is on if it has a surface.
    pub fn middle_anchor(cx: &mut App) -> Option<PopupAnchor> {
        Self::surfaces_nearest_first(cx)
            .into_iter()
            .find_map(|surface| {
                let anchor = surface.update(cx, |_, window, cx| {
                    let viewport = window.viewport_size();
                    let halfway = gpui::point(viewport.width / 2., viewport.height / 2.);
                    PopupAnchor::at_point(window, halfway, cx)
                });
                anchor.ok()?.ok()
            })
    }

    /// The surfaces, those of the display the pointer is on first, then
    /// those of the primary one.
    fn surfaces_nearest_first(cx: &mut App) -> Vec<WindowHandle<EdgeSurfaceView<Panel>>> {
        let this = cx.global::<Self>();
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
        surfaces.into_iter().map(|(_, surface)| surface).collect()
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
        crate::session::adjust(&mut config);
        crate::session::reconfigure(&config.shell, cx);
        cx.set_global(Theme::load(config.theme.as_deref(), config.contrast));
        cx.set_global(config.language);
        Tiler::configure(&config.tiling, cx);
        Workspaces::configure(&config.workspaces, cx);
        Shortcuts::configure(&config.keys, config.windows_keys, cx);
        DesktopClosing::configure(config.windows_keys);
        Wallpaper::configure(&config.wallpaper, cx);
        Banners::configure(&config.notifications, cx);
        NotificationStore::configure(&config.notifications, &config.notification_rules, cx);
        Flyouts::configure(&config.flyout, cx);
        Switcher::configure(&config.switcher, cx);
        Files::configure(&config.files, cx);
        Startup::configure(&config.startup, cx);
        Snapper::configure(&config.snap, config.windows_keys, &config.keys, cx);
        Rules::configure(&config.rules, cx);
        Corners::configure(&config.corners, cx);
        ClipHistory::configure(&config.clipboard, cx);
        Night::configure(&config.night_light, cx);
        Notices::configure(&config.notices, cx);

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

    /// The configuration in use.
    pub fn config(cx: &App) -> &Config {
        &cx.global::<Self>().config
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
    ///
    /// A surface appears on the displays its `displays` includes, with
    /// what the overrides that name a display give there. Surfaces that
    /// hide at the same edge of a display stand one after the other from
    /// the edge, in the order of the file.
    fn open_surfaces(config: &Config, open: &mut Vec<Surface>, cx: &mut App) -> Vec<Surface> {
        let primary = cx.primary_display().map(|display| display.id());
        let mut surfaces = Vec::new();
        for display in cx.displays() {
            let (name, device) = KnownDisplays::name_of(display.id(), cx);
            let is_primary = name
                .as_ref()
                .map_or(Some(display.id()) == primary, |name| name.primary);
            // How far from each edge the next surface that hides stands.
            let mut insets: Vec<(ScreenEdge, f32)> = Vec::new();
            for (index, layout) in config.surfaces.iter().enumerate() {
                if !layout.displays.includes(name.as_ref(), is_primary) {
                    continue;
                }
                let (placed, replaced) = layout.on_display(name.as_ref());
                let edge = placed.edge;
                let inset = match placed.hide.hides() {
                    true => {
                        let taken = insets.iter_mut().find(|(at, _)| *at == edge);
                        let inset = taken.as_ref().map_or(0., |(_, inset)| *inset);
                        match taken {
                            Some((_, taken)) => *taken += placed.thickness,
                            None => insets.push((edge, placed.thickness)),
                        }
                        inset
                    }
                    false => 0.,
                };
                let surface = EdgeSurface::of(&placed, px(inset));
                let placed = &placed;
                let kept = open
                    .iter()
                    .position(|open| {
                        open.is_on(display.id(), device.as_deref()) && open.edge == edge
                    })
                    .map(|index| open.swap_remove(index))
                    .filter(|open| {
                        let refilled = open.window.update(cx, |view, window, cx| {
                            view.refill(surface, window, cx, |display, cx| {
                                Panel::new(index, placed, replaced, display, cx)
                            });
                        });
                        refilled.is_ok()
                    });
                if let Some(mut kept) = kept {
                    (kept.display, kept.device) = (display.id(), device.clone());
                    surfaces.push(kept);
                    continue;
                }
                let opened = surface.open(&display, cx, |display, cx| {
                    Panel::new(index, placed, replaced, display, cx)
                });
                match opened {
                    Ok(window) => surfaces.push(Surface {
                        display: display.id(),
                        device: device.clone(),
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

/// Ends this Tessera and starts the program again, as it is on the disk
/// by then.
pub fn restart(cx: &mut App) {
    let started = std::env::current_exe()
        .context("the Tessera executable could not be located")
        .and_then(|executable| Taskbar::start_successor(&executable));
    match started {
        Ok(()) => cx.quit(),
        Err(error) => report!("Tessera does not start again: {error:#}"),
    }
}
