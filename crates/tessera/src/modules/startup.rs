// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The programs that start with the session, each with a switch that
//! turns it off and on again as Task Manager does. Widget: `startup`,
//! made for a popup or a card, and shown under **General** in the
//! settings.

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, AnyView, App, AppContext, AsyncApp, Context, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    WeakEntity, Window, div, px,
};

use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, SpringSpec, staggered};
use crate::platform::{StartupEntry, StartupSource, SwitchRefusal};
use crate::startup::Startup;
use crate::theme::Theme;
use crate::ui::{list_row, list_title, one_line, switch};

/// How far below its place a row starts when the list arrives.
const ROW_TRAVEL: f32 = 10.;

const ICON_COMMAND: &str = "\u{e756}";
const ICON_FOLDER: &str = "\u{e8b7}";
const ICON_STORE: &str = "\u{e719}";
const ICON_POLICY: &str = "\u{e72e}";

/// What the switch of an entry that a policy decides shows instead.
const FIXED: &str = "Set by policy";

pub struct StartupModule;

impl Module for StartupModule {
    fn name(&self) -> &'static str {
        "startup"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("startup", |_, cx| list(cx))
    }
}

/// A new list of the programs that start with the session.
pub fn list(cx: &mut App) -> AnyView {
    cx.new(StartupApps::new).into()
}

/// The icon of a program of `source`.
fn glyph(source: StartupSource) -> &'static str {
    match source {
        StartupSource::MachinePolicy | StartupSource::UserPolicy => ICON_POLICY,
        StartupSource::CommonFolder | StartupSource::UserFolder => ICON_FOLDER,
        StartupSource::Packaged => ICON_STORE,
        StartupSource::MachineRun | StartupSource::MachineRun32 | StartupSource::UserRun => {
            ICON_COMMAND
        }
    }
}

/// What a row says under the name of `entry`: where it is written down
/// and what it starts.
fn note(entry: &StartupEntry, language: Language) -> String {
    format!(
        "{} \u{b7} {}",
        language.say(entry.source.title()),
        entry.command
    )
}

/// What the user is told when a switch did not change.
fn refusal_text(refusal: &SwitchRefusal, language: Language) -> SharedString {
    match refusal {
        SwitchRefusal::NeedsAdministrator => language
            .say("Changing this for all users needs Tessera to run as administrator.")
            .into(),
        SwitchRefusal::Fixed => language.say(FIXED).into(),
        SwitchRefusal::Failed(error) => error.clone().into(),
    }
}

/// An entry as the springs of its switch know it, whatever its place in
/// the list.
type EntryKey = (StartupSource, String);

fn key_of(entry: &StartupEntry) -> EntryKey {
    (entry.source, entry.name.clone())
}

/// The list of the programs that start with the session.
struct StartupApps {
    /// `None` until they were read.
    entries: Option<Vec<StartupEntry>>,
    /// 1 for a switch that is on, by entry.
    travels: HashMap<EntryKey, Spring>,
    /// One for the switch of each entry, by its place.
    switches: Vec<Interaction>,
    /// 0 when the list arrives, 1 once it is in place.
    arrival: Spring,
    /// Why the last switch did not change.
    failure: Option<SharedString>,
    /// The places of the entries whose switch is being written.
    writing: HashSet<usize>,
}

impl StartupApps {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx
                .background_executor()
                .spawn(async { StartupEntry::all() });
            let entries = read.await;
            this.update(cx, |this, cx| this.take(entries, cx)).ok();
        })
        .detach();
        let motion = cx.global::<Theme>().motion;
        Self {
            entries: None,
            travels: HashMap::new(),
            switches: Vec::new(),
            arrival: Spring::new(motion.spatial, 0.),
            failure: None,
            writing: HashSet::new(),
        }
    }

    fn take(&mut self, entries: Vec<StartupEntry>, cx: &mut Context<Self>) {
        if self.entries.as_ref() == Some(&entries) {
            return;
        }
        if self.entries.is_none() {
            self.arrival.animate_to(1.);
        }
        let motion = cx.global::<Theme>().motion;
        for entry in &entries {
            self.follow(entry, motion.spatial);
        }
        self.switches
            .resize_with(entries.len(), || Interaction::new(&motion));
        self.entries = Some(entries);
        cx.notify();
    }

    /// Switches the entry at `index` over, and reads that entry again
    /// after; the other switches stay usable meanwhile.
    fn flip(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.writing.contains(&index) {
            return;
        }
        let Some(entry) = self
            .entries
            .as_mut()
            .and_then(|entries| entries.get_mut(index))
        else {
            return;
        };
        let on = !entry.enabled;
        let wanted = entry.clone();
        entry.enabled = on;
        if let Some(travel) = self.travels.get_mut(&key_of(entry)) {
            travel.animate_to(if on { 1. } else { 0. });
        }
        self.writing.insert(index);
        self.failure = None;
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let written = cx.background_executor().spawn(async move {
                let switched = wanted.set_enabled(on);
                (switched, wanted.refreshed())
            });
            let (switched, entry) = written.await;
            this.update(cx, |this, cx| {
                this.writing.remove(&index);
                if let Err(refusal) = switched {
                    this.failure = Some(refusal_text(&refusal, Language::of(cx)));
                }
                this.replace(index, entry, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Puts `entry`, read again, in its place `index`, its switch
    /// following what it now is.
    fn replace(&mut self, index: usize, entry: StartupEntry, cx: &mut Context<Self>) {
        let Some(place) = { self.entries.as_mut() }.and_then(|entries| entries.get_mut(index))
        else {
            return;
        };
        if key_of(place) != key_of(&entry) {
            return;
        }
        *place = entry.clone();
        self.follow(&entry, cx.global::<Theme>().motion.spatial);
    }

    /// Has the switch of `entry` travel to where it is on or off.
    fn follow(&mut self, entry: &StartupEntry, spring: SpringSpec) {
        let on = if entry.enabled { 1. } else { 0. };
        self.travels
            .entry(key_of(entry))
            .or_insert_with(|| Spring::new(spring, on))
            .animate_to(on);
    }

    /// The row of the entry at `index`, with its switch.
    fn row(
        &mut self,
        index: usize,
        entry: &StartupEntry,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = Language::of(cx);
        let row = list_row(
            theme,
            glyph(entry.source),
            entry.enabled,
            entry.name.clone(),
            note(entry, language),
        );
        if !entry.switchable {
            let fixed = div()
                .flex_none()
                .text_color(theme.colors.on_surface_variant)
                .child(language.say(FIXED));
            return row.child(fixed).into_any_element();
        }
        let travel =
            { self.travels.get_mut(&key_of(entry)) }.map_or(0., |travel| travel.value(window));
        let state = self.switches[index].state(window);
        let control = switch(theme, ("startup-switch", index), travel, state)
            .on_click(cx.listener(move |this, _, _, cx| this.flip(index, cx)));
        let control = Interaction::track(control, cx, move |this: &mut Self| {
            this.switches.get_mut(index)
        });
        row.child(control).into_any_element()
    }
}

impl Render for StartupApps {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let quiet = |text: SharedString| {
            div()
                .text_color(theme.colors.on_surface_variant)
                .child(one_line(text))
                .into_any_element()
        };
        let mut frame = div()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .child(
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(language.say(Startup::told(cx))),
            )
            .children(self.failure.clone().map(|failure| {
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.primary)
                    .child(failure)
            }));
        let Some(entries) = self.entries.clone() else {
            return frame.child(quiet(language.say("Reading…").into()));
        };
        if entries.is_empty() {
            let none = language.say("No program starts with Windows.");
            return frame.child(list_title(&theme, none).font_weight(FontWeight::NORMAL));
        }
        let arrival = self.arrival.value(window).clamp(0., 1.);
        let count = entries.len();
        for (index, entry) in entries.iter().enumerate() {
            let row = self.row(index, entry, &theme, window, cx);
            let arrived = staggered(arrival, index, count);
            frame = frame.child(
                div()
                    .id(("startup-row", index))
                    .relative()
                    .top(px(ROW_TRAVEL * (1. - arrived)))
                    .opacity(arrived)
                    .child(row),
            );
        }
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_is_said_in_russian() {
        let sources = [
            StartupSource::MachineRun,
            StartupSource::MachineRun32,
            StartupSource::MachinePolicy,
            StartupSource::UserRun,
            StartupSource::UserPolicy,
            StartupSource::CommonFolder,
            StartupSource::UserFolder,
            StartupSource::Packaged,
        ];
        for source in sources {
            let title = source.title();
            assert_ne!(Language::Russian.say(title), title, "`{title}`");
        }
        let administrator = refusal_text(&SwitchRefusal::NeedsAdministrator, Language::Russian);
        assert!(!administrator.is_ascii());
        assert_eq!(
            refusal_text(&SwitchRefusal::Failed("x".to_owned()), Language::Russian),
            SharedString::from("x")
        );
    }
}
