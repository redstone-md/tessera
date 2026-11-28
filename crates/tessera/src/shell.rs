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
use crate::config::{Config, Displays};
use crate::corners::Corners;
use crate::desk::Cards;
use crate::extension::ExtensionsModule;
use crate::extension::installed_stamp;
use crate::files::Files;
use crate::flyout::Flyouts;
use crate::icon_packs::IconPacks;
use crate::icons::AppIcons;
use crate::keys::Shortcuts;
use crate::menu::Menus;
use crate::module::WidgetRegistry;
use crate::modules;
use crate::modules::DesktopClosing;
use crate::modules::Updates;
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
    /// Where its layout stands among the surfaces of the configuration.
    index: usize,
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

    /// Whether the display GPUI calls `display`, named `name`, is the
    /// primary one: as its name says, or else as GPUI says.
    fn is_primary(name: Option<&DisplayName>, display: DisplayId, cx: &App) -> bool {
        let primary = cx.primary_display().map(|primary| primary.id());
        name.map_or(primary == Some(display), |name| name.primary)
    }

    /// Whether `displays` chooses the display GPUI calls `display`.
    pub fn includes(displays: &Displays, display: DisplayId, cx: &App) -> bool {
        let name = Self::name_of(display, cx).0;
        displays.includes(name.as_ref(), Self::is_primary(name.as_ref(), display, cx))
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
        cx.set_global(Theme::from_config(&config));
        cx.set_global(config.language);
        Tiler::start(&config.tiling, cx);
        Workspaces::start(&config.workspaces, cx);
        crate::performance::Performance::start(&config.performance, cx);
        crate::modules::system::Meters::configure(&config.system, cx);
        crate::platform::ExplorerCommands::prepare();
        let extensions = installed_stamp();
        let modules = modules::builtin(&config);
        Actions::start(&modules, cx);
        cx.set_global(WidgetRegistry::from_modules(&modules));
        cx.set_global(Popups::new(&config.popups));
        cx.set_global(Previews::default());
        cx.set_global(Tips::default());
        cx.set_global(Menus::default());
        cx.set_global(AppIcons::from_config(&config));
        IconPacks::start(cx);
        // Without Explorer the surfaces reserve their edges through the tray.
        crate::platform::Tray::start_before_surfaces();
        // ITaskbarList3 reaches a taskbar through the tray, dock or none.
        crate::platform::TaskbarList::start(crate::platform::shell_mode());
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
        // The peek at the desktop shows the wallpaper from its first time.
        Wallpaper::keep_pictures(cx);
        Updates::start(config.registries(), cx);
        crate::desktop::Desktop::start(&config.desktop, cx);
        Banners::configure(&config.notifications, cx);
        NotificationStore::configure(&config.notifications, &config.notification_rules, cx);
        Flyouts::configure(&config.flyout, cx);
        Switcher::configure(&config.switcher, cx);
        crate::taskview::TaskView::start(cx);
        Files::configure(&config.files, cx);
        config.file_view.configure(cx);
        Startup::start(&config.startup, cx);
        Snapper::start(&config.snap, config.windows_keys, &config.keys, cx);
        Rules::use_templates(&config.rule_templates, cx);
        Rules::start(&config.rules, cx);
        crate::window_actions::WindowActions::start(&config.window_actions, cx);
        Corners::start(&config.corners, cx);
        ClipHistory::start(&config.clipboard, cx);
        Notices::configure(&config.notices, cx);
        crate::tray_settings::TraySettings::configure(&config.tray, cx);
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
                        let theme = Theme::from_config(config);
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
        cx.set_global(Theme::from_config(&config));
        cx.set_global(config.language);
        Tiler::configure(&config.tiling, cx);
        Workspaces::configure(&config.workspaces, cx);
        crate::performance::Performance::configure(&config.performance, cx);
        crate::modules::system::Meters::configure(&config.system, cx);
        Shortcuts::configure(&config.keys, config.windows_keys, cx);
        DesktopClosing::configure(config.windows_keys);
        Wallpaper::configure(&config.wallpaper, cx);
        Updates::configure(config.registries(), cx);
        crate::desktop::Desktop::configure(&config.desktop, cx);
        Banners::configure(&config.notifications, cx);
        NotificationStore::configure(&config.notifications, &config.notification_rules, cx);
        Flyouts::configure(&config.flyout, cx);
        Switcher::configure(&config.switcher, cx);
        Files::configure(&config.files, cx);
        config.file_view.configure(cx);
        Startup::configure(&config.startup, cx);
        Snapper::configure(&config.snap, config.windows_keys, &config.keys, cx);
        Rules::use_templates(&config.rule_templates, cx);
        Rules::configure(&config.rules, cx);
        crate::window_actions::WindowActions::configure(&config.window_actions, cx);
        Corners::configure(&config.corners, cx);
        ClipHistory::configure(&config.clipboard, cx);
        Night::configure(&config.night_light, cx);
        Notices::configure(&config.notices, cx);
        crate::tray_settings::TraySettings::configure(&config.tray, cx);

        // New settings of extensions alone keep the popups and the widgets:
        // the settings themselves write them.
        let (reloaded, extensions) = Self::extensions_reloaded(&config, cx);
        let old = &cx.global::<Self>().config;
        let icons = old.icons != config.icons || old.icon_packs != config.icon_packs;
        let surfaces = old.surfaces != config.surfaces;
        let cards = old.cards != config.cards;
        // An open popup stays while it is still what the configuration
        // describes, so that the settings do not close at every change.
        // New icons written here, as by the icon packs of the settings or
        // the marketplace, keep it too, as `icons_changed` does; it shows
        // them the next time it opens, as every popup is made anew then.
        if reloaded || (!written_here && (icons || old.popups != config.popups)) {
            Popups::close(cx);
            cx.set_global(Popups::new(&config.popups));
        } else {
            // The widget that wrote the file already shows what it wrote.
            Popups::reconfigure(&config.popups, cx);
        }
        // Widgets keep what they were built with, so new registries and new
        // icons need new widgets as well.
        if extensions {
            cx.global_mut::<Self>().extensions = installed_stamp();
            cx.set_global(WidgetRegistry::from_modules(&modules::builtin(&config)));
        }
        if icons {
            cx.set_global(AppIcons::from_config(&config));
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

    /// What changed of the extensions in `config`: whether they are to be
    /// loaded again, which closes the popups, as new registries need; and
    /// whether their widgets are to be made again. New settings alone keep
    /// the extensions and their widgets as they are: each extension whose
    /// settings changed is told by [`ExtensionsModule::settings_changed`].
    /// Extensions put into or taken out of the directory are noticed by
    /// [`Self::watch`] once their files have settled.
    fn extensions_reloaded(config: &Config, cx: &mut App) -> (bool, bool) {
        let old = &cx.global::<Self>().config;
        if old.registries() != config.registries() {
            return (true, true);
        }
        if old.extensions == config.extensions {
            return (false, false);
        }
        let mut ids: Vec<String> = { old.extensions.keys() }
            .chain(config.extensions.keys())
            .filter(|id| old.extensions.get(*id) != config.extensions.get(*id))
            .cloned()
            .collect();
        ids.sort();
        ids.dedup();
        for id in ids {
            ExtensionsModule::settings_changed(&id, config.extensions.get(&id), cx);
        }
        (false, false)
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

    /// Builds the surfaces again after the pictures of the icons changed,
    /// as a reload does when `icons` changes, without loading the
    /// extensions again or closing the open popup.
    pub fn icons_changed(cx: &mut App) {
        let config = cx.global::<Self>().config.clone();
        Self::arrange(&config, cx);
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
    /// keep the same edge of a display stand one after the other from the
    /// edge, and those that hide there stand beyond them, one after the
    /// other, in the order of the file.
    ///
    /// A surface already open at the edge of the display keeps its window,
    /// the one of the same layout first, so that reading the file again
    /// moves nothing that did not change.
    fn open_surfaces(config: &Config, open: &mut Vec<Surface>, cx: &mut App) -> Vec<Surface> {
        let mut surfaces = Vec::new();
        for display in cx.displays() {
            let (name, device) = KnownDisplays::name_of(display.id(), cx);
            let is_primary = KnownDisplays::is_primary(name.as_ref(), display.id(), cx);
            let layouts: Vec<_> = config
                .surfaces
                .iter()
                .enumerate()
                .filter(|(_, layout)| layout.displays.includes(name.as_ref(), is_primary))
                .map(|(index, layout)| (index, layout.on_display(name.as_ref())))
                .collect();
            // How far from each edge the next surface that hides stands:
            // past the surfaces that keep that edge.
            let mut insets = EdgeInsets::default();
            for (_, (placed, _)) in &layouts {
                if !placed.hide.hides() {
                    insets.take(placed.edge, placed.thickness);
                }
            }
            for (index, (placed, replaced)) in layouts {
                let edge = placed.edge;
                let inset = match placed.hide.hides() {
                    true => insets.take(edge, placed.thickness),
                    false => 0.,
                };
                let surface = EdgeSurface::of(&placed, px(inset));
                let placed = &placed;
                let here = |open: &Surface| {
                    open.is_on(display.id(), device.as_deref()) && open.edge == edge
                };
                let kept = open
                    .iter()
                    .position(|open| here(open) && open.index == index)
                    .or_else(|| open.iter().position(here))
                    .map(|at| open.remove(at))
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
                    kept.index = index;
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
                        index,
                        window,
                    }),
                    Err(error) => report!("a surface did not open: {error:#}"),
                }
            }
        }
        surfaces
    }
}

/// How deep the surfaces of a display already stand at each of its edges.
#[derive(Default)]
struct EdgeInsets(Vec<(ScreenEdge, f32)>);

impl EdgeInsets {
    /// Puts a surface `thickness` deep at `edge`, and says how far from
    /// the edge it stands: past those put there before.
    fn take(&mut self, edge: ScreenEdge, thickness: f32) -> f32 {
        match self.0.iter_mut().find(|(at, _)| *at == edge) {
            Some((_, depth)) => {
                let inset = *depth;
                *depth += thickness;
                inset
            }
            None => {
                self.0.push((edge, thickness));
                0.
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surfaces_at_one_edge_stand_one_after_the_other() {
        let mut insets = EdgeInsets::default();
        assert_eq!(insets.take(ScreenEdge::Top, 48.), 0.);
        assert_eq!(insets.take(ScreenEdge::Left, 40.), 0.);
        assert_eq!(insets.take(ScreenEdge::Top, 32.), 48.);
        assert_eq!(insets.take(ScreenEdge::Top, 20.), 80.);
        assert_eq!(insets.take(ScreenEdge::Left, 40.), 40.);
    }
}
