// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the registries offer, read when the shell starts, once a day
//! after that, and whenever a marketplace asks: every marketplace shows
//! the same reading, and with it how many of the installed things have an
//! update. Nothing is updated without a click.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{App, AsyncApp, Global};

use crate::extension::registry::Catalog;

/// How often the registries are read by themselves.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How old a reading may be for a marketplace that opens to show it
/// instead of reading again.
const FRESH: Duration = Duration::from_secs(60 * 60);

/// The last reading of the registries.
#[derive(Default)]
pub struct Updates {
    registries: Vec<String>,
    catalog: Option<Rc<Catalog>>,
    read_at: Option<Instant>,
    reading: bool,
    /// Counts the changes of the registries, so that a reading of those
    /// named before is dropped.
    generation: u64,
}

impl Global for Updates {}

impl Updates {
    /// Reads `registries` now and once a day from now on.
    pub fn start(registries: Vec<String>, cx: &mut App) {
        cx.set_global(Self {
            registries,
            ..Self::default()
        });
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                if cx.update(Self::check).is_err() {
                    break;
                }
                cx.background_executor().timer(CHECK_INTERVAL).await;
            }
        })
        .detach();
    }

    /// Reads `registries` from now on, and at once if they are others.
    pub fn configure(registries: Vec<String>, cx: &mut App) {
        let this = cx.default_global::<Self>();
        if this.registries == registries {
            return;
        }
        this.registries = registries;
        this.catalog = None;
        this.read_at = None;
        this.reading = false;
        this.generation += 1;
        Self::check(cx);
    }

    /// Reads the registries again, away from the user interface, unless a
    /// reading is under way.
    pub fn check(cx: &mut App) {
        let this = cx.default_global::<Self>();
        if this.reading {
            return;
        }
        this.reading = true;
        let registries = this.registries.clone();
        let generation = this.generation;
        cx.spawn(async move |cx: &mut AsyncApp| {
            let catalog = cx
                .background_executor()
                .spawn(async move { Catalog::browse(&registries) })
                .await;
            cx.update(|cx| {
                let this = cx.global_mut::<Self>();
                if this.generation == generation {
                    this.catalog = Some(Rc::new(catalog));
                    this.read_at = Some(Instant::now());
                    this.reading = false;
                }
            })
            .ok();
        })
        .detach();
        // Observers learn that a reading is under way.
        cx.global_mut::<Self>();
    }

    /// Reads the registries again unless the last reading is recent.
    pub fn check_if_stale(cx: &mut App) {
        let this = cx.default_global::<Self>();
        let fresh = this.read_at.is_some_and(|at| at.elapsed() < FRESH);
        if !fresh {
            Self::check(cx);
        }
    }

    /// The last reading, if there is one yet.
    pub fn catalog(cx: &App) -> Option<Rc<Catalog>> {
        cx.try_global::<Self>()?.catalog.clone()
    }

    /// The registries that are read.
    pub fn registries(cx: &App) -> Vec<String> {
        cx.try_global::<Self>()
            .map(|this| this.registries.clone())
            .unwrap_or_default()
    }

    /// Whether the registries are being read.
    pub fn reading(cx: &App) -> bool {
        cx.try_global::<Self>().is_some_and(|this| this.reading)
    }
}
