// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Arranging the dock by hand: a button or a separator dragged along it
//! (GPUI's drag inside the window, the pointer read while a pin is dragged
//! to see it let go outside), and what other programs drop on it (through
//! the drop target of its surface), each opening a gap where it would go.

use std::path::PathBuf;

use gpui::{
    AsyncApp, Context, IntoElement, Pixels, Point, Render, Styled, WeakEntity, Window, div, img, px,
};

use super::pins;
use super::{
    DRAG_HOVER, DRAG_WATCH, DROP_GRACE, DockItem, Docks, EMPTY_PART, Layout, Pin, Sorting,
    WindowList, entries, options, repin,
};
use crate::icons::Icon;
use crate::menu::Menus;
use crate::platform::{PathDrop, PathDropTarget, PathZones, pointer_at, pointer_held};
use crate::preview::Previews;

/// The drop target of a surface with a dock, kept for as long as Tessera
/// runs so that a dock made anew on the surface takes it over.
pub(super) struct DropSlot {
    pub(super) _target: PathDropTarget,
    /// The places that take drops, which the dock writes as it draws.
    pub(super) zones: PathZones,
    /// The dock that hears of the drops.
    pub(super) dock: WeakEntity<WindowList>,
}

/// What a drag from another program is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OutsideDrag {
    /// The button, by its key; `None` over the empty part.
    pub(super) over: Option<usize>,
    /// Counts the buttons the drag came onto, so that a wait for one that
    /// was left meanwhile is dropped.
    pub(super) visit: u64,
}

/// What is carried while a button is dragged along the dock.
pub(super) struct DockDrag {
    pub(super) key: usize,
    /// The dock it comes from.
    pub(super) list: gpui::EntityId,
}

/// What follows the pointer while a button is dragged: its icon.
pub(super) struct DragGhost {
    pub(super) icon: Icon,
    pub(super) size: Pixels,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match self.icon.clone() {
            Some(icon) => img(icon).size(self.size).into_any_element(),
            None => div().size(self.size).into_any_element(),
        }
    }
}

impl WindowList {
    /// The buttons that stay in their places while `dragged` is lifted
    /// out, in the order of the dock.
    pub(super) fn staying(&self, dragged: Option<usize>) -> impl Iterator<Item = &DockItem> {
        { self.items.iter() }.filter(move |item| !item.leaving && Some(item.key) != dragged)
    }

    /// Where something dragged at `position` goes in among the buttons
    /// that stay, `dragged` being the button of the dock dragged, if it is
    /// one; `None` where it may not go in there.
    pub(super) fn aim_at(&self, position: Point<Pixels>, dragged: Option<usize>) -> Option<usize> {
        if !self.options.sort.keeps_pins_first() {
            return None;
        }
        let along = |point: Point<Pixels>| match self.layout {
            Layout::Row => point.x / px(1.),
            Layout::Column { .. } => point.y / px(1.),
        };
        let middles: Vec<f32> = { self.staying(dragged) }
            .filter_map(|item| Some(along(item.bounds.get()?.center())))
            .collect();
        let index = options::insertion(&middles, along(position));
        let pins = self
            .staying(dragged)
            .filter(|item| item.pin.is_some())
            .count();
        let pinned = dragged
            .and_then(|key| self.item(key))
            .map(|item| item.pin.is_some());
        match pinned {
            // A pin stays among the pins.
            Some(true) => Some(index.min(pins)),
            // A button that runs is pinned where it is dropped among the
            // pins, and only `manual` keeps it elsewhere.
            Some(false) => {
                let pinning = index <= pins && self.pins_shown;
                (pinning || self.options.sort == Sorting::Manual).then_some(index)
            }
            // What other programs drop is pinned.
            None => self.pins_shown.then_some(index.min(pins)),
        }
    }

    /// The button `key` of this dock is dragged at `position` of the
    /// window, over the dock where `over`.
    pub(super) fn drag_moved(
        &mut self,
        key: usize,
        position: Point<Pixels>,
        over: bool,
        cx: &mut Context<Self>,
    ) {
        if self.dragging != Some(key) {
            self.dragging = Some(key);
            Previews::close(cx);
            Menus::close(cx);
            if let Some(item) = self.items.iter_mut().find(|item| item.key == key) {
                item.lift.animate_to(0.);
            }
            self.watch_release(key, cx);
            cx.notify();
        }
        let aim = over.then(|| self.aim_at(position, Some(key))).flatten();
        if aim != self.aim {
            self.aim = aim;
            cx.notify();
        }
    }

    /// Reads the pointer while the button `key` is dragged, to see it let
    /// go outside the window, which GPUI does not tell.
    pub(super) fn watch_release(&self, key: usize, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(DRAG_WATCH).await;
                if pointer_held() {
                    let going = this.update(cx, |this, _| this.dragging == Some(key));
                    if !matches!(going, Ok(true)) {
                        break;
                    }
                    continue;
                }
                let at = pointer_at();
                this.update(cx, |this, cx| this.released(key, at, cx)).ok();
                break;
            }
        })
        .detach();
    }

    /// The button of the pointer went up at `at` on the screen while the
    /// button `key` was dragged: outside the window a pin is unpinned,
    /// inside it GPUI's drop, which comes then, ends the drag.
    pub(super) fn released(&mut self, key: usize, at: Option<(i32, i32)>, cx: &mut Context<Self>) {
        if self.dragging != Some(key) {
            return;
        }
        let outside = match (at, self.frame) {
            (Some((x, y)), Some(frame)) => {
                x < frame.left || x >= frame.right || y < frame.top || y >= frame.bottom
            }
            _ => false,
        };
        if outside {
            self.end_drag(cx);
            self.unpin_lifted(key, cx);
            self.stop_gpui_drag(cx);
            return;
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(DROP_GRACE).await;
            this.update(cx, |this, cx| {
                if this.dragging == Some(key) {
                    this.end_drag(cx);
                    this.stop_gpui_drag(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Ends GPUI's drag of the surface, whose end GPUI did not see.
    pub(super) fn stop_gpui_drag(&self, cx: &mut Context<Self>) {
        if let Some(surface) = self.surface {
            cx.defer(move |cx| {
                surface
                    .update(cx, |_, window, cx| {
                        cx.stop_active_drag(window);
                    })
                    .ok();
            });
        }
    }

    /// The drag along the dock is over: the lifted button goes back.
    pub(super) fn end_drag(&mut self, cx: &mut Context<Self>) {
        self.dragging = None;
        self.aim = None;
        for item in &mut self.items {
            item.lift.animate_to(1.);
        }
        cx.notify();
    }

    /// Unpins the pin `key`, dragged off the dock: it shrinks away where
    /// it stood.
    pub(super) fn unpin_lifted(&mut self, key: usize, cx: &mut Context<Self>) {
        let Some(item) = self.items.iter_mut().find(|item| item.key == key) else {
            return;
        };
        let Some(pin) = &item.pin else {
            return;
        };
        let (place, entry) = (pin.place, pin.entry.clone());
        item.leaving = true;
        item.lift.jump_to(1.);
        item.presence.animate_to(0.);
        repin(cx, move |pins| pins::removed(pins, place, &entry));
    }

    /// The button `key` was dropped on the dock: it goes where it was
    /// aimed, a pin among the pins, a button that runs among the pins as a
    /// new pin or, with `manual`, among the others.
    pub(super) fn dropped(&mut self, key: usize, cx: &mut Context<Self>) {
        let aim = self.aim;
        self.end_drag(cx);
        let Some(index) = aim else {
            return;
        };
        let places: Vec<usize> = { self.staying(Some(key)) }
            .filter_map(|item| item.pin.as_ref().map(|pin| pin.place))
            .collect();
        let pins = places.len();
        let Some(item) = self.item(key) else {
            return;
        };
        let place = options::pin_place(&places, index);
        match &item.pin {
            Some(pin) => {
                let to = if pin.place < place { place - 1 } else { place };
                if to == pin.place {
                    return;
                }
                let (from, entry) = (pin.place, pin.entry.clone());
                repin(cx, move |list| pins::moved(list, from, &entry, to));
            }
            // Right after the last pin it is pinned, but with `manual`,
            // where it goes first among the others.
            None if self.pins_shown
                && (index < pins || (index == pins && self.options.sort != Sorting::Manual)) =>
            {
                let Some(entry) = entries::pin_entry(item) else {
                    return;
                };
                repin(cx, move |list| pins::inserted(list, place, &[entry]));
            }
            None => {
                let Some(group) = item
                    .group
                    .clone()
                    .filter(|group| !group.starts_with("window:"))
                else {
                    return;
                };
                let mut running: Vec<String> = { self.staying(Some(key)) }
                    .filter(|item| item.pin.is_none())
                    .filter_map(|item| item.group.clone())
                    .filter(|group| !group.starts_with("window:"))
                    .collect();
                running.insert(index.saturating_sub(pins).min(running.len()), group);
                // Every dock is told, this one too, once its update is
                // over.
                cx.defer(move |cx| Docks::reorder(&running, cx));
            }
        }
    }

    /// Takes the drops of other programs on the surface of `window`, unless
    /// another dock of it does; tried again at each drawing until it is
    /// done.
    pub(super) fn take_drops(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.drops_taken {
            return;
        }
        let id = window.window_handle().window_id();
        let me = cx.weak_entity();
        let docks = cx.default_global::<Docks>();
        if let Some(slot) = docks.drop_windows.get_mut(&id) {
            if slot.dock.upgrade().is_none() {
                slot.dock = me;
                self.zones = slot.zones.clone();
                self.drops_taken = true;
            }
            return;
        }
        self.drops_taken = true;
        let zones = PathZones::default();
        let (target, events) = match PathDropTarget::register(window, zones.clone()) {
            Ok(registered) => registered,
            Err(error) => return report!("the dock takes no drops from other programs: {error:#}"),
        };
        self.zones = zones.clone();
        let slot = DropSlot {
            _target: target,
            zones,
            dock: me,
        };
        cx.default_global::<Docks>().drop_windows.insert(id, slot);
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            while let Ok(event) = events.recv().await {
                let dock = cx.update(|cx| {
                    let docks = cx.try_global::<Docks>()?;
                    docks.drop_windows.get(&id)?.dock.upgrade()
                });
                if let Ok(Some(dock)) = dock {
                    dock.update(cx, |dock, cx| dock.hear_drop(event, cx)).ok();
                }
            }
        })
        .detach();
    }

    /// Takes in what a drag from another program does over the dock.
    pub(super) fn hear_drop(&mut self, event: PathDrop, cx: &mut Context<Self>) {
        match event {
            PathDrop::Over { tag, at } => {
                let over = tag.filter(|&tag| tag != EMPTY_PART).map(|tag| tag as usize);
                // What is dropped anywhere but on a pin that opens it is
                // pinned where the gap opens.
                let aim = match tag.is_some() && !self.opens_dropped(over) {
                    true => self.aim_at(at, None),
                    false => None,
                };
                if aim != self.aim {
                    self.aim = aim;
                    cx.notify();
                }
                self.drag_over(over, cx);
            }
            PathDrop::Left => {
                self.outside = None;
                self.aim = None;
                cx.notify();
            }
            PathDrop::Dropped {
                tag, paths, links, ..
            } => {
                let aim = self.aim.take();
                self.outside = None;
                cx.notify();
                self.take_dropped(tag, aim, paths, links, cx);
            }
        }
    }

    /// A drag from another program is over the button `over`, or over no
    /// button: after a moment on an application with windows, its window
    /// comes forward, so that what is dragged can be dropped into it.
    pub(super) fn drag_over(&mut self, over: Option<usize>, cx: &mut Context<Self>) {
        if self.outside.is_some_and(|outside| outside.over == over) {
            return;
        }
        let visit = self.outside.map_or(0, |outside| outside.visit + 1);
        let now = OutsideDrag { over, visit };
        self.outside = Some(now);
        cx.notify();
        let Some(key) = over else {
            return;
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(DRAG_HOVER).await;
            this.update(cx, |this, _| {
                if this.outside != Some(now) {
                    return;
                }
                if let Some(front) = this.item(key).and_then(DockItem::front) {
                    front.id.activate();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Whether what is dropped on the button `over` is opened with its
    /// pinned application, which takes files.
    pub(super) fn opens_dropped(&self, over: Option<usize>) -> bool {
        { over.and_then(|key| self.item(key)?.pin.as_ref()) }.is_some_and(Pin::takes_files)
    }

    /// What another program dropped on the zone `tag`: pinned at `aim` on
    /// the empty part, or opened with the pinned application of a button.
    pub(super) fn take_dropped(
        &mut self,
        tag: u64,
        aim: Option<usize>,
        paths: Vec<PathBuf>,
        links: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let over = (tag != EMPTY_PART).then_some(tag as usize);
        if self.opens_dropped(over) {
            if let Some(pin) = over.and_then(|key| self.item(key)?.pin.as_ref()) {
                pin.open_with(&paths, &links);
            }
            return;
        }
        let places: Vec<usize> = { self.staying(None) }
            .filter_map(|item| item.pin.as_ref().map(|pin| pin.place))
            .collect();
        let place = match aim {
            Some(index) => options::pin_place(&places, index),
            None => usize::MAX,
        };
        let paths = paths.iter().map(|path| pins::entry_of(path));
        let found: Vec<String> = paths.chain(links).collect();
        if !found.is_empty() {
            repin(cx, move |list| pins::inserted(list, place, &found));
        }
    }

    /// Opens the gap where something dragged is aimed, before the button
    /// it goes in before or after the last one, and closes the others.
    pub(super) fn open_gap(&mut self) {
        let lifted = self.dragging;
        let aim = self.aim;
        let mut index = 0;
        for item in &mut self.items {
            if item.leaving || Some(item.key) == lifted {
                item.shift.animate_to(0.);
                continue;
            }
            item.shift
                .animate_to(if aim == Some(index) { 1. } else { 0. });
            index += 1;
        }
        self.end_gap.animate_to(if aim == Some(index) && index > 0 {
            1.
        } else {
            0.
        });
    }
}
