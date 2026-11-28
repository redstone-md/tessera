// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Dropping files, folders and links from other programs onto a window of
//! Tessera whose view decides itself what a drop does, as the dock does:
//! it pins what is dropped on its empty part and opens it with the
//! application of a pinned button.
//!
//! Unlike [`DropTarget`](super::DropTarget), which copies and moves files
//! into folders, a [`PathDropTarget`] does nothing with what is dropped:
//! it tells the view where a drag is and what was dropped where, on a
//! channel, as [`PathDrop`]s. The view lists the places that take a drop
//! as [`PathZones`], each with the effect a drop there has, which the
//! cursor shows. The source is never told that anything was moved, so it
//! deletes nothing.
//!
//! OLE calls the target on the thread of GPUI, from its message loop,
//! outside any update. Once a window has one, GPUI's own drop events no
//! longer arrive there.
#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context as _, Result};
use async_channel::{Receiver, Sender};
use gpui::{Bounds, Pixels, Point, Window, point, px};
use windows::Win32::Foundation::{HWND, POINT, POINTL, S_OK};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, IDataObject, TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, RegisterDragDrop,
    RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{CFSTR_INETURLW, CLSID_DragDropHelper, IDropTargetHelper};
use windows::core::{Ref, implement};

use super::NativeSurface;
use super::drag::{DropEffect, DropEffects, dropped_paths, format, global_bytes, request};

/// What a drop on a zone does, as the cursor shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ZoneEffect {
    /// The zone is told of the drag, and takes no drop.
    #[default]
    None,
    /// What is dropped is opened or used.
    Copy,
    /// What is dropped is pinned or linked to.
    Link,
}

impl ZoneEffect {
    fn drop_effect(self) -> DropEffect {
        match self {
            Self::None => DropEffect::None,
            Self::Copy => DropEffect::Copy,
            Self::Link => DropEffect::Link,
        }
    }
}

/// A place of a window that takes a drop: `tag` tells the view which,
/// and `effect` is what the cursor shows over it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathZone {
    /// Where it is, in the coordinates of the window.
    pub bounds: Bounds<Pixels>,
    pub tag: u64,
    pub effect: ZoneEffect,
}

/// The places of a window that take a drop, written by its view as it
/// draws. A later zone lies above an earlier one.
#[derive(Clone, Debug, Default)]
pub struct PathZones(Rc<RefCell<Vec<PathZone>>>);

impl PathZones {
    /// Puts `zones` in place of the zones there were.
    pub fn set(&self, zones: Vec<PathZone>) {
        if let Ok(mut held) = self.0.try_borrow_mut() {
            *held = zones;
        }
    }

    /// The topmost zone at `at`.
    fn hit(&self, at: Point<Pixels>) -> Option<PathZone> {
        let zones = self.0.try_borrow().ok()?;
        top_path_zone(&zones, at)
    }
}

/// The topmost of `zones` at `at`: the last one listed that holds it.
fn top_path_zone(zones: &[PathZone], at: Point<Pixels>) -> Option<PathZone> {
    zones
        .iter()
        .rev()
        .find(|zone| zone.bounds.contains(&at))
        .copied()
}

/// What happens to a drag over the window.
#[derive(Clone, Debug, PartialEq)]
pub enum PathDrop {
    /// Something that is taken is dragged at `at`, over the zone `tag` or
    /// none. Told at each move.
    Over { tag: Option<u64>, at: Point<Pixels> },
    /// The drag left the window, or was dropped where nothing takes it.
    Left,
    /// It was dropped at `at` on the zone `tag`: the files and folders of
    /// `paths`, or the web addresses of `links` where it carried no files.
    Dropped {
        tag: u64,
        at: Point<Pixels>,
        paths: Vec<PathBuf>,
        links: Vec<String>,
    },
}

/// A drop target in place of GPUI's on one window, until this is dropped.
#[derive(Debug)]
pub struct PathDropTarget {
    window: isize,
}

impl PathDropTarget {
    /// Takes the drops into `window` over the places `zones` lists, and
    /// tells of them on the channel returned. It is to be called on the
    /// thread of GPUI, which owns the window.
    pub fn register(window: &Window, zones: PathZones) -> Result<(Self, Receiver<PathDrop>)> {
        let hwnd = NativeSurface::of(window)?.hwnd;
        let (events, told) = async_channel::unbounded();
        // SAFETY: the helper is created on this thread, which has OLE
        // initialized, as GPUI does it.
        let helper: Option<IDropTargetHelper> =
            unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }.ok();
        let target: IDropTarget = Target {
            window: hwnd,
            zones,
            helper,
            events,
            carried: RefCell::new(None),
            told: Cell::new(false),
        }
        .into();
        // SAFETY: the window belongs to this thread; OLE keeps the target
        // until it is revoked, which `Drop` does.
        unsafe {
            let _ = RevokeDragDrop(hwnd);
            RegisterDragDrop(hwnd, &target).context("the window takes no drops")?;
        }
        Ok((
            Self {
                window: hwnd.0 as isize,
            },
            told,
        ))
    }
}

impl Drop for PathDropTarget {
    fn drop(&mut self) {
        // SAFETY: revoking fails without harm for a window that is gone.
        let _ = unsafe { RevokeDragDrop(HWND(self.window as *mut c_void)) };
    }
}

/// What a drag over the window carries that is taken.
struct Carried {
    paths: Vec<PathBuf>,
    links: Vec<String>,
    allowed: DropEffects,
}

impl Carried {
    /// What `data` carries: files and folders, or else a web address;
    /// `None` where it carries neither.
    fn of(data: &IDataObject, allowed: u32) -> Option<Self> {
        let offered = |format: u16| {
            let request = request(format, -1, TYMED_HGLOBAL.0);
            // SAFETY: the data object is alive; the call only asks.
            unsafe { data.QueryGetData(&raw const request) == S_OK }
        };
        let allowed = DropEffects::of_raw(allowed);
        if offered(CF_HDROP.0) {
            let paths = dropped_paths(data);
            return (!paths.is_empty()).then_some(Self {
                paths,
                links: Vec::new(),
                allowed,
            });
        }
        let link = format(CFSTR_INETURLW)
            .filter(|&format| offered(format))
            .and_then(|format| global_bytes(data, format, -1))
            .and_then(|bytes| link_of_utf16(&bytes))?;
        Some(Self {
            paths: Vec::new(),
            links: vec![link],
            allowed,
        })
    }
}

/// The text of a NUL-terminated UTF-16 buffer, trimmed, unless it is
/// empty.
fn link_of_utf16(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = { bytes.chunks_exact(2) }
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|&unit| unit != 0)
        .collect();
    let text = String::from_utf16_lossy(&units).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// The effect a drop on `zone` has where the drag allows `allowed`: the
/// zone's own, or else another one that is allowed, or none.
fn effect_on(zone: Option<&PathZone>, allowed: DropEffects) -> DropEffect {
    let Some(zone) = zone.filter(|zone| zone.effect != ZoneEffect::None) else {
        return DropEffect::None;
    };
    let offered = [
        zone.effect.drop_effect(),
        DropEffect::Link,
        DropEffect::Copy,
    ];
    { offered.into_iter() }
        .find(|&effect| allowed.allows(effect))
        .unwrap_or(DropEffect::None)
}

/// The drop target itself, called by OLE on the thread that registered
/// it, the thread of GPUI.
#[implement(IDropTarget, Agile = false)]
struct Target {
    window: HWND,
    zones: PathZones,
    helper: Option<IDropTargetHelper>,
    events: Sender<PathDrop>,
    carried: RefCell<Option<Carried>>,
    /// Whether the view was told of the drag.
    told: Cell<bool>,
}

impl Target {
    /// The point of the screen `at` in the coordinates of the window, as
    /// GPUI lays it out.
    fn local(&self, at: &POINTL) -> Point<Pixels> {
        let mut client = POINT { x: at.x, y: at.y };
        // SAFETY: the calls only read the window's place and resolution.
        let dpi = unsafe {
            let _ = ScreenToClient(self.window, &raw mut client);
            GetDpiForWindow(self.window)
        };
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        point(px(client.x as f32 / scale), px(client.y as f32 / scale))
    }

    /// The zone at `at`, where the drag is taken, and the effect there.
    fn aim(&self, at: &POINTL) -> (Option<PathZone>, Point<Pixels>, DropEffect) {
        let local = self.local(at);
        let carried = self.carried.borrow();
        let Some(carried) = carried.as_ref() else {
            return (None, local, DropEffect::None);
        };
        let zone = self.zones.hit(local);
        let effect = effect_on(zone.as_ref(), carried.allowed);
        (zone, local, effect)
    }

    /// Tells the view where the drag is, for something that is taken.
    fn tell_over(&self, zone: Option<&PathZone>, at: Point<Pixels>) {
        if self.carried.borrow().is_none() {
            return;
        }
        self.told.set(true);
        let tag = zone.map(|zone| zone.tag);
        self.events.try_send(PathDrop::Over { tag, at }).ok();
    }

    fn tell_left(&self) {
        if self.told.replace(false) {
            self.events.try_send(PathDrop::Left).ok();
        }
    }
}

#[allow(non_snake_case, reason = "the names of the methods of COM")]
impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        data: Ref<'_, IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        at: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let data = data.ok()?;
        // SAFETY: OLE gives a valid pointer to the effects the source
        // allows, which the answer is written over.
        let allowed = unsafe { (*effect).0 };
        *self.carried.borrow_mut() = Carried::of(data, allowed);
        let (zone, local, chosen) = self.aim(at);
        // SAFETY: as above; the helper draws the drag image over the
        // window, at the point of the screen.
        unsafe {
            *effect = chosen.raw();
            if let Some(helper) = &self.helper {
                let screen = POINT { x: at.x, y: at.y };
                let _ = helper.DragEnter(self.window, data, &raw const screen, chosen.raw());
            }
        }
        self.tell_over(zone.as_ref(), local);
        Ok(())
    }

    fn DragOver(
        &self,
        _: MODIFIERKEYS_FLAGS,
        at: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let (zone, local, chosen) = self.aim(at);
        // SAFETY: OLE gives a valid pointer for the answer.
        unsafe {
            *effect = chosen.raw();
            if let Some(helper) = &self.helper {
                let screen = POINT { x: at.x, y: at.y };
                let _ = helper.DragOver(&raw const screen, chosen.raw());
            }
        }
        self.tell_over(zone.as_ref(), local);
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if let Some(helper) = &self.helper {
            // SAFETY: the helper only takes its drag image away.
            let _ = unsafe { helper.DragLeave() };
        }
        self.carried.borrow_mut().take();
        self.tell_left();
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<'_, IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        at: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let data = data.ok()?;
        if self.carried.borrow().is_none() {
            // SAFETY: OLE gives a valid pointer to the allowed effects.
            let allowed = unsafe { (*effect).0 };
            *self.carried.borrow_mut() = Carried::of(data, allowed);
        }
        let (zone, local, chosen) = self.aim(at);
        if let Some(helper) = &self.helper {
            let screen = POINT { x: at.x, y: at.y };
            // SAFETY: the helper takes its drag image away.
            let _ = unsafe { helper.Drop(data, &raw const screen, chosen.raw()) };
        }
        let carried = self.carried.borrow_mut().take();
        self.told.set(false);
        let answer = match (carried, zone) {
            (Some(carried), Some(zone)) if chosen != DropEffect::None => {
                let dropped = PathDrop::Dropped {
                    tag: zone.tag,
                    at: local,
                    paths: carried.paths,
                    links: carried.links,
                };
                self.events.try_send(dropped).ok();
                chosen.raw()
            }
            _ => {
                self.events.try_send(PathDrop::Left).ok();
                DROPEFFECT_NONE
            }
        };
        // SAFETY: OLE gives a valid pointer for the answer.
        unsafe { *effect = answer };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    fn zone(x: f32, width: f32, tag: u64, effect: ZoneEffect) -> PathZone {
        PathZone {
            bounds: Bounds::new(point(px(x), px(0.)), size(px(width), px(40.))),
            tag,
            effect,
        }
    }

    #[test]
    fn a_button_over_the_dock_takes_the_drop_with_its_effect() {
        let zones = [
            zone(0., 400., 0, ZoneEffect::Link),
            zone(40., 40., 7, ZoneEffect::Copy),
        ];
        let at = |x: f32| top_path_zone(&zones, point(px(x), px(10.))).map(|zone| zone.tag);
        assert_eq!(at(50.), Some(7));
        assert_eq!(at(10.), Some(0));
        assert_eq!(at(500.), None);
        let button = zones[1];
        assert_eq!(effect_on(Some(&button), DropEffects::ALL), DropEffect::Copy);
        assert_eq!(
            effect_on(Some(&button), DropEffects::LINK),
            DropEffect::Link
        );
        let refused = zone(0., 10., 1, ZoneEffect::None);
        assert_eq!(
            effect_on(Some(&refused), DropEffects::ALL),
            DropEffect::None
        );
        assert_eq!(effect_on(None, DropEffects::ALL), DropEffect::None);
    }

    #[test]
    fn a_dragged_link_is_read_up_to_its_end() {
        let bytes: Vec<u8> = { "https://example.org/\0junk".encode_utf16() }
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(
            link_of_utf16(&bytes).as_deref(),
            Some("https://example.org/")
        );
        assert_eq!(link_of_utf16(&[0, 0]), None);
    }
}
