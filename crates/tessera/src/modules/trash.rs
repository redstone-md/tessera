// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `trash` widget: how many items lie in the Recycle Bin, a click that
//! opens it, and a menu that brings everything back or empties it; and the
//! `trash` action, which does the same from a key.

use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, WeakEntity, Window,
    div,
};

use crate::actions::{Action, Actions};
use crate::files::size_text;
use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{BinUsage, RecycleBin};
use crate::popup::PopupAnchor;
use crate::theme::Theme;
use crate::ui::{icon, pill, state_layer};

const ICON_BIN: &str = "\u{e74d}";
const ICON_OPEN: &str = "\u{e838}";
const ICON_RESTORE: &str = "\u{e7a7}";

/// How often the bin is looked into: Windows does not announce a change
/// to a program that shows no folder.
const CHECK_INTERVAL: Duration = Duration::from_secs(3);

pub struct TrashModule;

impl Module for TrashModule {
    fn name(&self) -> &'static str {
        "trash"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("trash", |_, cx| cx.new(Trash::new).into())
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        let trash = Action::new(|arg, cx| {
            if let Some(deed) = BinDeed::named(arg) {
                deed.run(None, cx);
            }
        })
        .checking(|arg| match BinDeed::named(arg) {
            Some(_) => Ok(()),
            None => Err("`trash` takes `open`, `empty`, `restore` or no `arg`".to_owned()),
        });
        actions.register("trash", trash)
    }
}

/// What can be done with the whole bin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BinDeed {
    /// Has Explorer show it.
    Open,
    /// Brings every item back to where it was deleted from.
    Restore,
    /// Erases every item, once the user said yes in the dialog of Windows.
    Empty,
}

impl BinDeed {
    /// The deed the `arg` of the action `trash` names; none names
    /// [`Self::Open`].
    fn named(arg: Option<&str>) -> Option<Self> {
        match arg {
            None | Some("open") => Some(Self::Open),
            Some("restore") => Some(Self::Restore),
            Some("empty") => Some(Self::Empty),
            Some(_) => None,
        }
    }

    /// Does it, without waiting; a failure is reported. `widget` is told
    /// to count again once the bin has changed.
    fn run(self, widget: Option<WeakEntity<Trash>>, cx: &mut App) {
        match self {
            Self::Open => {
                if let Err(error) = RecycleBin::open() {
                    report!("the Recycle Bin was not opened: {error:#}");
                }
            }
            Self::Restore => {
                let restored = RecycleBin::restore_all();
                cx.spawn(async move |cx| {
                    if let Ok(Err(error)) = restored.recv().await {
                        report!("the items of the Recycle Bin were not brought back: {error}");
                    }
                    recount(widget, cx);
                })
                .detach();
            }
            Self::Empty => {
                let emptied = RecycleBin::empty();
                cx.spawn(async move |cx| {
                    if let Ok(Err(error)) = emptied.recv().await {
                        report!("{error}");
                    }
                    recount(widget, cx);
                })
                .detach();
            }
        }
    }
}

/// Has `widget`, where there is one, count the bin again at once.
fn recount(widget: Option<WeakEntity<Trash>>, cx: &mut AsyncApp) {
    if let Some(widget) = widget {
        widget.update(cx, |trash, cx| trash.count(cx)).ok();
    }
}

/// Shows the number of items that was last counted.
struct Trash {
    /// `None` until Windows has told.
    usage: Option<BinUsage>,
    interaction: Interaction,
    /// 1 at the moment the number changes, then back to 0.
    changed: Spring,
}

impl Trash {
    fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let usage = cx
                    .background_executor()
                    .spawn(async { RecycleBin::usage() });
                let usage = usage.await;
                if this.update(cx, |this, cx| this.take(usage, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(CHECK_INTERVAL).await;
            }
        })
        .detach();
        Self {
            usage: None,
            interaction: Interaction::new(&motion),
            changed: Spring::new(motion.effects, 0.),
        }
    }

    /// Counts the bin once more, away from the thread of GPUI.
    fn count(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let usage = cx
                .background_executor()
                .spawn(async { RecycleBin::usage() });
            let usage = usage.await;
            this.update(cx, |this, cx| this.take(usage, cx)).ok();
        })
        .detach();
    }

    /// Takes in what was counted; a count that failed keeps the last one.
    fn take(&mut self, usage: Option<BinUsage>, cx: &mut Context<Self>) {
        if usage.is_none() || usage == self.usage {
            return;
        }
        // The first count is no change, nor is a change of the size alone.
        let items = |usage: Option<BinUsage>| usage.map(|usage| usage.items);
        if self.usage.is_some() && items(usage) != items(self.usage) {
            self.changed.jump_to(1.);
            self.changed.animate_to(0.);
        }
        self.usage = usage;
        cx.notify();
    }

    /// Opens the menu of the widget at the pointer.
    fn menu(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = match PopupAnchor::at(window, event.position.x, cx) {
            Ok(anchor) => anchor,
            Err(error) => {
                report!("the menu of the Recycle Bin has nowhere to open: {error:#}");
                return;
            }
        };
        let language = Language::of(cx);
        let widget = cx.entity().downgrade();
        let entries = menu_deeds(self.usage)
            .into_iter()
            .map(|(deed, glyph, label)| {
                let widget = widget.clone();
                let label = match (deed, self.usage) {
                    (BinDeed::Empty, Some(usage)) => {
                        format!("{} ({})", language.say(label), size_text(usage.bytes))
                    }
                    _ => language.say(label).to_owned(),
                };
                MenuEntry::new(glyph, label, move |cx| deed.run(Some(widget.clone()), cx))
            })
            .collect();
        Menus::open(entries, anchor, cx);
    }
}

/// What the menu offers for a bin that holds what `usage` says: opening
/// it, and bringing back or erasing what it holds, if anything.
fn menu_deeds(usage: Option<BinUsage>) -> Vec<(BinDeed, &'static str, &'static str)> {
    let mut deeds = vec![(BinDeed::Open, ICON_OPEN, "Open")];
    // A bin that was not counted may hold something.
    if usage.is_none_or(|usage| usage.items > 0) {
        deeds.push((BinDeed::Restore, ICON_RESTORE, "Restore all items"));
        deeds.push((BinDeed::Empty, ICON_BIN, "Empty the Recycle Bin"));
    }
    deeds
}

/// What the widget says next to its icon: the number of items, and
/// nothing for an empty bin or one that was not counted.
fn label(items: Option<u64>) -> Option<String> {
    items
        .filter(|items| *items > 0)
        .map(|items| items.to_string())
}

impl Render for Trash {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let colors = theme.colors;
        let changed = self.changed.value(window).clamp(0., 1.);
        let state = self.interaction.state(window);
        let ground = mix(
            theme.on_bar(colors.surface_container),
            colors.secondary_container,
            changed,
        );
        let label = label(self.usage.map(|usage| usage.items));
        let ink = match label {
            Some(_) => colors.on_surface,
            None => colors.on_surface_variant,
        };
        let button = pill(&theme)
            .id("trash")
            .bg(state_layer(&theme, ground, state))
            .text_color(ink)
            .on_click(|_, _, cx| {
                Menus::close(cx);
                BinDeed::Open.run(None, cx);
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event, window, cx| this.menu(event, window, cx)),
            )
            .child(icon(&theme, ICON_BIN))
            .children(label.map(|label| div().font_weight(FontWeight::MEDIUM).child(label)));
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_bin_shows_no_number() {
        assert_eq!(label(None), None);
        assert_eq!(label(Some(0)), None);
        assert_eq!(label(Some(12)).as_deref(), Some("12"));
    }

    #[test]
    fn the_action_names_a_deed_of_the_bin() {
        assert_eq!(BinDeed::named(None), Some(BinDeed::Open));
        assert_eq!(BinDeed::named(Some("open")), Some(BinDeed::Open));
        assert_eq!(BinDeed::named(Some("restore")), Some(BinDeed::Restore));
        assert_eq!(BinDeed::named(Some("empty")), Some(BinDeed::Empty));
        assert_eq!(BinDeed::named(Some("erase")), None);
    }

    #[test]
    fn an_empty_bin_can_only_be_opened() {
        let deeds = |usage| -> Vec<BinDeed> {
            menu_deeds(usage)
                .into_iter()
                .map(|(deed, _, _)| deed)
                .collect()
        };
        let empty = BinUsage { items: 0, bytes: 0 };
        let full = BinUsage {
            items: 3,
            bytes: 2048,
        };
        assert_eq!(deeds(Some(empty)), [BinDeed::Open]);
        assert_eq!(
            deeds(Some(full)),
            [BinDeed::Open, BinDeed::Restore, BinDeed::Empty]
        );
        assert_eq!(deeds(None).len(), 3);
    }
}
