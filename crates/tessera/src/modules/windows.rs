// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One button per pinned application and per open application window.
//! Widget: `windows`.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, StatefulInteractiveElement, Styled, WeakEntity, Window, canvas, div, img,
};

use crate::icons::{AppIcons, Icon};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{AppWindow, AppWindowId, DisplayKey, Shortcut, WindowEvents};
use crate::popup::PopupAnchor;
use crate::preview::Previews;
use crate::theme::Theme;
use crate::ui::state_layer;

/// How often it is checked whether something happened to a window.
const CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// How many checks pass before the window list is read again although
/// nothing was reported: every half second when Windows reports nothing at
/// all, and every five seconds otherwise, for what goes unreported.
const CHECKS_UNWATCHED: u32 = 5;
const CHECKS_WATCHED: u32 = 50;

/// How much the icon under the pointer grows, as a share of its size.
const HOVER_GROWTH: f32 = 0.12;

/// How much a pressed icon shrinks, as a share of its size.
const PRESS_SHRINK: f32 = 0.16;

pub struct WindowsModule;

impl Module for WindowsModule {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("windows", |context, cx| {
            let display = context.display;
            let pins = Pin::find(&context.texts("pinned").unwrap_or_default());
            cx.new(|cx| WindowList::new(display, pins, cx)).into()
        })
    }
}

/// An application that keeps a button whether it runs or not.
struct Pin {
    /// What a click opens while the application has no window.
    shortcut: Shortcut,
    /// The program its windows belong to, when that is known.
    program: Option<PathBuf>,
}

impl Pin {
    /// The applications `names` stand for. A name is the path of a file or
    /// what the Start menu calls a shortcut; one that is neither is
    /// reported and skipped.
    fn find(names: &[String]) -> Vec<Self> {
        let by_name = names.iter().any(|name| !Path::new(name).is_absolute());
        let start_menu = if by_name {
            Shortcut::start_menu()
        } else {
            Vec::new()
        };
        let pins = names.iter().filter_map(|name| {
            let path = Path::new(name);
            let shortcut = if path.is_absolute() {
                path.is_file().then(|| Shortcut {
                    name: name.clone(),
                    path: path.to_owned(),
                })
            } else {
                let known = |shortcut: &&Shortcut| shortcut.name.eq_ignore_ascii_case(name);
                start_menu.iter().find(known).cloned()
            };
            if shortcut.is_none() {
                eprintln!("tessera: nothing to pin is called `{name}`");
            }
            shortcut.map(Self::of)
        });
        pins.collect()
    }

    fn of(shortcut: Shortcut) -> Self {
        let program = if is_link(&shortcut.path) {
            shortcut.link().target
        } else {
            Some(shortcut.path.clone())
        };
        Self { shortcut, program }
    }

    /// Whether `window` belongs to the pinned application.
    fn owns(&self, window: &AppWindow) -> bool {
        let (Some(program), Some(executable)) = (&self.program, &window.executable) else {
            return false;
        };
        same_file(program, executable)
    }

    fn icon(&self, icons: &AppIcons) -> Icon {
        if is_link(&self.shortcut.path) {
            icons.of_shortcut(&self.shortcut)
        } else {
            icons.of_executable(&self.shortcut.path)
        }
    }
}

fn is_link(path: &Path) -> bool {
    let extension = path.extension();
    extension.is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
}

/// Whether two paths as Windows writes them name one file: it ignores case
/// and accepts both kinds of slashes.
fn same_file(one: &Path, other: &Path) -> bool {
    let plain = |path: &Path| path.to_string_lossy().replace('/', "\\").to_lowercase();
    plain(one) == plain(other)
}

/// One button and the motion it is in.
struct DockItem {
    /// Identifies the button for as long as it is shown.
    key: usize,
    /// The pinned application the button stands for. Such a button stays
    /// when its window closes.
    pin: Option<Pin>,
    /// The window the button switches to; a button without one is that of
    /// a pinned application that does not run on this display.
    window: Option<AppWindow>,
    /// 1 while there is a window, 0 while there is none.
    running: Spring,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    /// 1 while the window is the foreground window.
    active: Spring,
    interaction: Interaction,
    /// The middle of the button in its surface, once it has been drawn:
    /// where the preview of its window opens.
    middle: Rc<Cell<Option<Pixels>>>,
    /// The window is gone and its button is shrinking away.
    leaving: bool,
}

/// Lists the pinned applications and the application windows of one display,
/// starts the former and switches between the latter.
struct WindowList {
    /// The display whose windows are listed; every window when unknown.
    display: Option<DisplayKey>,
    /// In display order, including buttons that are still leaving.
    items: Vec<DockItem>,
    foreground: Option<AppWindowId>,
    next_key: usize,
}

impl WindowList {
    fn new(display: Option<DisplayKey>, pins: Vec<Pin>, cx: &mut Context<Self>) -> Self {
        let patience = if WindowEvents::watch() {
            CHECKS_WATCHED
        } else {
            eprintln!("tessera: Windows does not report window changes, so they are polled");
            CHECKS_UNWATCHED
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut seen = WindowEvents::generation();
            let mut quiet = 0;
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                let now = WindowEvents::generation();
                quiet += 1;
                if now == seen && quiet < patience {
                    if this.upgrade().is_none() {
                        break;
                    }
                    continue;
                }
                (seen, quiet) = (now, 0);
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
        };
        for pin in pins {
            list.add(Some(pin), None, cx);
        }
        list.refresh(cx);
        list
    }

    /// Appends a button, which grows in.
    fn add(&mut self, pin: Option<Pin>, window: Option<AppWindow>, cx: &Context<Self>) {
        let motion = cx.global::<Theme>().motion;
        let mut presence = Spring::new(motion.spatial, 0.);
        presence.animate_to(1.);
        self.items.push(DockItem {
            key: self.next_key,
            pin,
            window,
            running: Spring::new(motion.spatial, 0.),
            presence,
            active: Spring::new(motion.spatial, 0.),
            interaction: Interaction::new(&motion),
            middle: Rc::default(),
            leaving: false,
        });
        self.next_key += 1;
    }

    /// The pointer came onto the button `key`, or left it: the preview of
    /// its window opens after a moment, or closes.
    fn preview(&self, key: usize, hovered: bool, window: &Window, cx: &mut Context<Self>) {
        let item = self.items.iter().find(|item| item.key == key);
        let target = item
            .filter(|item| !item.leaving)
            .and_then(|item| Some((item.window.as_ref()?.id, item.middle.get()?)));
        let Some((id, middle)) = target.filter(|_| hovered) else {
            let left = target.filter(|_| !hovered).map(|(id, _)| id);
            Previews::leave(left, cx);
            return;
        };
        match PopupAnchor::at(window, middle, cx) {
            Ok(anchor) => Previews::request(id, anchor, cx),
            Err(error) => eprintln!("tessera: a preview has nowhere to open: {error:#}"),
        }
    }

    /// Re-reads the window list and repaints only when it changed. Known
    /// windows keep their place, new ones are appended, and closed ones
    /// start leaving. A pinned application takes one of its windows; the
    /// buttons of pinned applications come first, so they choose first.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let mut windows = AppWindow::enumerate();
        windows.retain(|window| self.display.is_none_or(|display| window.display == display));
        let foreground = AppWindowId::foreground();
        let mut changed = foreground != self.foreground;
        self.foreground = foreground;
        let shows = |items: &[DockItem], id| {
            let shown =
                |item: &DockItem| !item.leaving && item.window.as_ref().map(|w| w.id) == Some(id);
            items.iter().any(shown)
        };
        let previewed = Previews::shown(cx).filter(|&id| shows(&self.items, id));

        for item in &mut self.items {
            let held = item.window.as_ref().map(|window| window.id);
            let mut found = windows.iter().position(|window| Some(window.id) == held);
            let Some(pin) = &item.pin else {
                let present = found.is_some();
                if let Some(window) = found.map(|index| windows.remove(index))
                    && Some(&window) != item.window.as_ref()
                {
                    item.window = Some(window);
                    changed = true;
                }
                if item.leaving == present {
                    item.leaving = !present;
                    item.presence.animate_to(if present { 1. } else { 0. });
                    changed = true;
                }
                continue;
            };
            if found.is_none() {
                found = windows.iter().position(|window| pin.owns(window));
            }
            let window = found.map(|index| windows.remove(index));
            if window != item.window {
                item.window = window;
                changed = true;
            }
        }
        for window in windows {
            self.add(None, Some(window), cx);
            changed = true;
        }
        // A preview does not outlive its window.
        if previewed.is_some_and(|id| !shows(&self.items, id)) {
            Previews::close(cx);
        }
        for item in &mut self.items {
            let shown = item.window.as_ref().filter(|_| !item.leaving);
            let is_foreground = shown.is_some_and(|window| foreground == Some(window.id));
            item.active.animate_to(if is_foreground { 1. } else { 0. });
            let runs = item.window.is_some();
            item.running.animate_to(if runs { 1. } else { 0. });
        }
        if changed {
            cx.notify();
        }
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
        let running = item.running.value(window);
        let state = item.interaction.state(window);
        let key = item.key;
        let id = item.window.as_ref().map(|window| window.id);
        let shortcut = item.pin.as_ref().map(|pin| pin.shortcut.clone());

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
            .on_click(move |_, _, cx| {
                Previews::close(cx);
                match (id, &shortcut) {
                    (Some(id), _) => id.activate_or_minimize(),
                    (None, Some(shortcut)) => shortcut.launch(),
                    (None, None) => {}
                }
            })
            .child({
                let middle = item.middle.clone();
                let measure =
                    move |bounds: gpui::Bounds<Pixels>, _: &mut Window, _: &mut gpui::App| {
                        middle.set(Some(bounds.origin.x + bounds.size.width / 2.));
                    };
                canvas(measure, |_, (), _, _| {}).absolute().size_full()
            })
            .child(match icon {
                Some(icon) => img(icon).size(icon_size).into_any_element(),
                None => div()
                    .size(icon_size)
                    .rounded(theme.shape.small)
                    .bg(theme.colors.outline)
                    .into_any_element(),
            })
            .child(Self::render_indicator(active, running, theme));
        let button = Interaction::track_hover(
            button,
            cx,
            move |this: &mut Self| {
                let item = this.items.iter_mut().find(|item| item.key == key)?;
                Some(&mut item.interaction)
            },
            move |this, hovered, window, cx| this.preview(key, hovered, window, cx),
        );

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

    /// A dot under every window; it grows into a pill for the foreground
    /// one. `running` is 0 under an application without a window, which
    /// has no dot.
    fn render_indicator(active: f32, running: f32, theme: Theme) -> AnyElement {
        let dot = theme.dock.indicator_height;
        let growth = theme.dock.indicator_active_width - dot;
        div()
            .h(dot)
            .w(dot + growth * active.max(0.))
            .rounded(theme.shape.full)
            .opacity(running.clamp(0., 1.))
            .bg(mix(theme.colors.outline, theme.colors.primary, active))
            .into_any_element()
    }
}

impl Render for WindowList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let items = &mut self.items;
        let elements: Vec<AnyElement> = items
            .iter_mut()
            .map(|item| {
                let icons = cx.global::<AppIcons>();
                let icon = match (&item.pin, &item.window) {
                    (Some(pin), _) => pin.icon(icons),
                    (None, Some(window)) => {
                        let executable = window.executable.as_deref();
                        executable.and_then(|executable| icons.of_executable(executable))
                    }
                    (None, None) => None,
                };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_of_one_file_are_told_whatever_their_spelling() {
        let one = Path::new(r"C:\Program Files\App\app.exe");
        assert!(same_file(one, Path::new("c:/program files/APP/App.exe")));
        assert!(!same_file(
            one,
            Path::new(r"C:\Program Files\App\other.exe")
        ));
    }

    #[test]
    fn only_shortcut_files_are_read_as_links() {
        assert!(is_link(Path::new(r"C:\Menu\App.LNK")));
        assert!(!is_link(Path::new(r"C:\Apps\app.exe")));
    }
}
