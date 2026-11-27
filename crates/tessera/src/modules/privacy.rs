// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Which applications use the microphone, the camera or the location now,
//! as the privacy icons of Explorer's notification area tell. Widgets:
//! `privacy`, an icon for each of the three that is in use, there only
//! while one is, whose tip names the applications; and `privacy-list`, for
//! a popup, the applications under each, with the way to the privacy
//! settings of Windows. The popup `privacy` holds the list, and a click on
//! the icons opens it.
//!
//! What is in use is read from the record Windows keeps of it, on a thread
//! that waits for the record to change; see `platform::PrivacyWatch`.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Bounds, Context, Entity, Global, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, canvas, div,
};

use crate::config::{PopupConfig, WidgetSpec};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, staggered};
use crate::platform::{Capability, CapabilityUse, Launch, PrivacyWatch};
use crate::popup::{PopupAnchor, Popups};
use crate::theme::Theme;
use crate::tip::Tips;
use crate::ui::{LIST_MIN_HEIGHT, icon, list_row, list_title, one_line, pill, state_layer};

/// The popup a click on `privacy` opens when the layout names none.
const POPUP: &str = "privacy";

/// What the tip of the icons is known by.
const TIP_KIND: &str = "privacy";

/// How far below its place a row starts when the list arrives.
const ROW_TRAVEL: f32 = 10.;

const ICON_MICROPHONE: &str = "\u{e720}";
const ICON_CAMERA: &str = "\u{e714}";
const ICON_LOCATION: &str = "\u{e81d}";

pub struct PrivacyModule;

impl Module for PrivacyModule {
    fn name(&self) -> &'static str {
        "privacy"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("privacy", |_, cx| cx.new(Indicator::new).into())?;
        widgets.register_panel("privacy-list", |_, cx| cx.new(List::new).into())?;
        widgets.register_popup(PopupConfig {
            name: POPUP.to_owned(),
            place: Default::default(),
            width: 360.,
            height: 320.,
            side: Vec::new(),
            content: vec![WidgetSpec {
                widget: "privacy-list".to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
        })?;
        widgets.open_with("privacy", POPUP);
        Ok(())
    }
}

/// How the user knows a capability: its icon, its name and the page of the
/// Windows settings about it.
fn look(capability: Capability) -> (&'static str, &'static str, &'static str) {
    match capability {
        Capability::Microphone => (
            ICON_MICROPHONE,
            "Microphone",
            "ms-settings:privacy-microphone",
        ),
        Capability::Camera => (ICON_CAMERA, "Camera", "ms-settings:privacy-webcam"),
        Capability::Location => (ICON_LOCATION, "Location", "ms-settings:privacy-location"),
    }
}

/// The applications that use `capability` among `uses`.
fn users(uses: &[CapabilityUse], capability: Capability) -> Vec<&str> {
    let users = uses.iter().filter(|used| used.capability == capability);
    users.map(|used| used.app.as_str()).collect()
}

/// What the tip of the icons says: a line for each capability in use, with
/// the applications that use it.
fn tip_text(uses: &[CapabilityUse], language: Language) -> String {
    let lines = Capability::ALL.into_iter().filter_map(|capability| {
        let users = users(uses, capability);
        if users.is_empty() {
            return None;
        }
        let (_, name, _) = look(capability);
        Some(format!("{}: {}", language.say(name), users.join(", ")))
    });
    lines.collect::<Vec<_>>().join("\n")
}

/// The uses there are now, shared by every widget.
#[derive(Default)]
struct PrivacyState {
    /// `None` until the first reading.
    uses: Option<Vec<CapabilityUse>>,
}

/// The one state, made when the first widget asks for it.
struct SharedPrivacy(Entity<PrivacyState>);

impl Global for SharedPrivacy {}

impl PrivacyState {
    fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(shared) = cx.try_global::<SharedPrivacy>() {
            return shared.0.clone();
        }
        let state = cx.new(|cx: &mut Context<Self>| {
            let uses = PrivacyWatch::start();
            cx.spawn(async move |this, cx: &mut AsyncApp| {
                while let Ok(read) = uses.recv().await {
                    let taken = this.update(cx, |state, cx| {
                        state.uses = Some(read);
                        cx.notify();
                    });
                    if taken.is_err() {
                        break;
                    }
                }
            })
            .detach();
            Self::default()
        });
        cx.set_global(SharedPrivacy(state.clone()));
        state
    }

    fn uses(&self) -> &[CapabilityUse] {
        self.uses.as_deref().unwrap_or_default()
    }
}

/// The icons in the bar, one for each capability in use.
struct Indicator {
    state: Entity<PrivacyState>,
    /// 0 while nothing is in use, 1 while the icons are shown.
    presence: Spring,
    interaction: Interaction,
    /// Where the icons were last drawn, for the tip.
    drawn: Rc<Cell<Option<Bounds<Pixels>>>>,
    _changes: Subscription,
}

impl Indicator {
    fn new(cx: &mut Context<Self>) -> Self {
        let state = PrivacyState::shared(cx);
        let motion = cx.global::<Theme>().motion;
        let mut this = Self {
            _changes: cx.observe(&state, |this: &mut Self, _, cx| this.follow(cx)),
            state,
            presence: Spring::new(motion.spatial, 0.),
            interaction: Interaction::new(&motion),
            drawn: Rc::default(),
        };
        this.follow(cx);
        this
    }

    fn follow(&mut self, cx: &mut Context<Self>) {
        let used = !self.state.read(cx).uses().is_empty();
        self.presence.animate_to(if used { 1. } else { 0. });
        cx.notify();
    }

    /// The pointer came onto the icons, or left them: the tip naming the
    /// applications comes out of the bar under them, or goes.
    fn tip(&self, hovered: bool, window: &Window, cx: &mut Context<Self>) {
        let key = (TIP_KIND, cx.entity_id().as_u64());
        let text = tip_text(self.state.read(cx).uses(), Language::of(cx));
        let Some(drawn) = self.drawn.get().filter(|_| hovered && !text.is_empty()) else {
            Tips::leave(key, cx);
            return;
        };
        let middle = drawn.origin.x + drawn.size.width / 2.;
        match PopupAnchor::at(window, middle, cx) {
            Ok(anchor) => Tips::request(key, text.into(), anchor, cx),
            Err(error) => report!("a tip has nowhere to open: {error:#}"),
        }
    }
}

impl Render for Indicator {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let presence = self.presence.value(window).clamp(0., 1.);
        if presence == 0. {
            return div().into_any_element();
        }
        let uses = self.state.read(cx).uses().to_vec();
        let state = self.interaction.state(window);
        let icons = Capability::ALL
            .into_iter()
            .filter(|capability| !users(&uses, *capability).is_empty())
            .map(|capability| icon(&theme, look(capability).0).text_color(theme.colors.primary));
        let drawn = self.drawn.clone();
        let measure = move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
            drawn.set(Some(bounds));
        };
        let button = pill(&theme)
            .id("privacy")
            .relative()
            .opacity(presence)
            .bg(state_layer(
                &theme,
                theme.on_bar(theme.colors.surface_container),
                state,
            ))
            .child(canvas(measure, |_, (), _, _| {}).absolute().size_full())
            .children(icons);
        Interaction::track_hover(
            button,
            cx,
            |this: &mut Self| Some(&mut this.interaction),
            |this, hovered, window, cx| this.tip(hovered, window, cx),
        )
        .into_any_element()
    }
}

/// The applications under each capability in use, for a popup.
struct List {
    state: Entity<PrivacyState>,
    arrival: Spring,
    /// The buttons to the settings, one for each capability.
    settings: [Interaction; 3],
    _changes: Subscription,
}

impl List {
    fn new(cx: &mut Context<Self>) -> Self {
        let state = PrivacyState::shared(cx);
        let motion = cx.global::<Theme>().motion;
        let mut arrival = Spring::new(motion.spatial, 0.);
        arrival.animate_to(1.);
        Self {
            _changes: cx.observe(&state, |_, _, cx| cx.notify()),
            state,
            arrival,
            settings: std::array::from_fn(|_| Interaction::new(&motion)),
        }
    }

    fn group(
        &mut self,
        slot: usize,
        users: &[&str],
        arrived: f32,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = Language::of(cx);
        let (glyph, name, page) = look(Capability::ALL[slot]);
        let state = self.settings[slot].state(window);
        let settings = div()
            .id(("privacy-settings", slot))
            .flex_none()
            .px(theme.spacing.small + theme.spacing.tiny)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.full)
            .text_color(theme.colors.primary)
            .bg(state_layer(theme, theme.colors.surface_container, state))
            .on_click(move |_, _, cx| {
                Launch::new(page).start();
                Popups::close(cx);
            })
            .child(language.say("Settings"));
        let settings = Interaction::track(settings, cx, move |this: &mut Self| {
            this.settings.get_mut(slot)
        });
        let title = div()
            .flex()
            .items_center()
            .justify_between()
            .child(list_title(theme, language.say(name)))
            .child(settings);
        let rows = users.iter().map(|app| {
            list_row(
                theme,
                glyph,
                true,
                SharedString::from(app.to_string()),
                language.say("In use"),
            )
        });
        div()
            .relative()
            .top(gpui::px(ROW_TRAVEL * (1. - arrived)))
            .opacity(arrived)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .child(title)
            .children(rows)
            .into_any_element()
    }
}

impl Render for List {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let arrival = self.arrival.value(window).clamp(0., 1.);
        let state = self.state.read(cx);
        let read = state.uses.is_some();
        let uses = state.uses().to_vec();
        let frame = div()
            .id("privacy-list")
            .w_full()
            .flex_1()
            .min_h(LIST_MIN_HEIGHT)
            .flex()
            .flex_col()
            .gap(theme.spacing.medium)
            .overflow_y_scroll();
        let groups: Vec<(usize, Vec<&str>)> = Capability::ALL
            .into_iter()
            .enumerate()
            .map(|(slot, capability)| (slot, users(&uses, capability)))
            .filter(|(_, users)| !users.is_empty())
            .collect();
        if groups.is_empty() {
            let text = match read {
                true => "No application uses the microphone, the camera or the location",
                false => "Reading…",
            };
            return frame.child(
                div()
                    .relative()
                    .top(gpui::px(ROW_TRAVEL * (1. - arrival)))
                    .opacity(arrival)
                    .text_color(theme.colors.on_surface_variant)
                    .child(one_line(language.say(text))),
            );
        }
        let count = groups.len();
        let shown: Vec<AnyElement> = groups
            .iter()
            .enumerate()
            .map(|(index, (slot, users))| {
                let arrived = staggered(arrival, index, count);
                self.group(*slot, users, arrived, &theme, window, cx)
            })
            .collect();
        frame.children(shown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn used(capability: Capability, app: &str) -> CapabilityUse {
        CapabilityUse {
            capability,
            app: app.to_owned(),
        }
    }

    #[test]
    fn the_tip_names_the_applications_under_each_capability() {
        let uses = [
            used(Capability::Microphone, "Teams"),
            used(Capability::Microphone, "Zoom"),
            used(Capability::Location, "Maps"),
        ];
        assert_eq!(
            tip_text(&uses, Language::English),
            "Microphone: Teams, Zoom\nLocation: Maps"
        );
        assert_eq!(tip_text(&[], Language::English), "");
        assert_eq!(
            tip_text(&uses[2..], Language::Russian),
            "Местоположение: Maps"
        );
    }

    #[test]
    fn each_capability_leads_to_its_page_of_the_settings() {
        let pages: Vec<&str> = Capability::ALL
            .into_iter()
            .map(|capability| look(capability).2)
            .collect();
        assert_eq!(
            pages,
            [
                "ms-settings:privacy-microphone",
                "ms-settings:privacy-webcam",
                "ms-settings:privacy-location"
            ]
        );
    }
}
