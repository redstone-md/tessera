// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Who is signed in, and the folders of that account. Widgets: `user`,
//! `profile`, `places`. A click on `user` opens a popup of the other two
//! and the power buttons unless the layout names another.

use std::path::PathBuf;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity,
    Window, div, img, px,
};

use crate::config::{PopupConfig, WidgetSpec};
use crate::files::Files;
use crate::i18n::Language;
use crate::icons::{Icon, image_of_bytes};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, staggered};
use crate::platform::{Account, Place, Shortcut};
use crate::popup::Popups;
use crate::theme::Theme;
use crate::ui::place_look as look;
use crate::ui::{icon, list_row, one_line, pill, state_layer};

const POPUP: &str = "user";

/// Side of the picture in a bar.
const SMALL_PICTURE: Pixels = px(20.);
/// Side of the picture in a popup.
const LARGE_PICTURE: Pixels = px(56.);
/// Side, in pixels, the picture file is scaled down to.
const PICTURE_PIXELS: u32 = 112;
const NAME_SIZE: Pixels = px(18.);
const LOGIN_SIZE: Pixels = px(12.);
/// How far below its place a row starts, in pixels.
const ROW_TRAVEL: f32 = 12.;

const ICON_PERSON: &str = "\u{e77b}";

/// The folders `places` shows when the layout names none.
const SHOWN: [Place; 5] = [
    Place::Documents,
    Place::Downloads,
    Place::Pictures,
    Place::Music,
    Place::Videos,
];

pub struct UserModule;

impl Module for UserModule {
    fn name(&self) -> &'static str {
        "user"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("user", |context, cx| {
            let named = context.flag("name").unwrap_or(true);
            cx.new(|cx| Person::new(false, named, cx)).into()
        })?;
        widgets.register_panel("profile", |_, cx| {
            cx.new(|cx| Person::new(true, true, cx)).into()
        })?;
        widgets.register_panel("places", |context, cx| {
            let places = match context.texts("show") {
                Some(names) => names.iter().filter_map(|name| place_named(name)).collect(),
                None => SHOWN.to_vec(),
            };
            cx.new(|cx| Places::new(places, cx)).into()
        })?;
        let widget = |name: &str| WidgetSpec {
            widget: name.to_owned(),
            popup: None,
            options: toml::Table::new(),
        };
        widgets.register_popup(PopupConfig {
            place: Default::default(),
            name: POPUP.to_owned(),
            width: 320.,
            height: 476.,
            side: Vec::new(),
            content: vec![widget("profile"), widget("places"), widget("power")],
        })?;
        widgets.open_with("user", POPUP);
        Ok(())
    }
}

/// The folder a layout calls `name`; another name is reported.
fn place_named(name: &str) -> Option<Place> {
    let place = Place::ALL.into_iter().find(|place| place.code() == name);
    if place.is_none() {
        report!("`places` shows no folder named `{name}`");
    }
    place
}

/// The letter that stands for `name` where there is no picture.
fn initial(name: &str) -> String {
    let letter = name.trim().chars().next();
    letter.map_or_else(String::new, |letter| letter.to_uppercase().collect())
}

/// The picture and the name of the account.
struct Person {
    /// Whether it is the card of a popup rather than the pill of a bar.
    large: bool,
    /// Whether the name stands next to the picture.
    named: bool,
    /// `None` until Windows has told.
    account: Option<Account>,
    picture: Icon,
    /// 0 until the account is known, 1 once it is in place.
    arrival: Spring,
}

impl Person {
    fn new(large: bool, named: bool, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx.background_executor().spawn(async {
                let account = Account::current();
                let file = account.picture.as_ref().and_then(|path| {
                    std::fs::read(path)
                        .map_err(|error| report!("{}: {error}", path.display()))
                        .ok()
                });
                let picture = file.and_then(|bytes| {
                    image_of_bytes(&bytes, PICTURE_PIXELS)
                        .map_err(|error| report!("the picture of the account: {error:#}"))
                        .ok()
                        .flatten()
                });
                (account, picture)
            });
            let (account, picture) = read.await;
            this.update(cx, |this, cx| {
                this.account = Some(account);
                this.picture = picture;
                this.arrival.animate_to(1.);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            large,
            named,
            account: None,
            picture: None,
            arrival: Spring::new(cx.global::<Theme>().motion.spatial, 0.),
        }
    }

    /// The picture in a circle `side` across, grown to `arrived` of it; the
    /// first letter of the name where the account has no picture.
    fn portrait(&self, theme: &Theme, side: Pixels, arrived: f32) -> AnyElement {
        let across = side * arrived.max(0.);
        let frame = div()
            .size(side)
            .flex_none()
            .flex()
            .items_center()
            .justify_center();
        let face = match (&self.picture, &self.account) {
            (Some(picture), _) => img(picture.clone())
                .size(across)
                .rounded(theme.shape.full)
                .into_any_element(),
            (None, account) => {
                let letter = account.as_ref().map(|account| initial(&account.name));
                let mark = match letter {
                    Some(letter) if !letter.is_empty() => div()
                        .text_size(side * 0.5)
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from(letter)),
                    _ => icon(theme, ICON_PERSON),
                };
                div()
                    .size(across)
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .rounded(theme.shape.full)
                    .bg(theme.colors.primary)
                    .text_color(theme.colors.surface_container)
                    .child(mark)
                    .into_any_element()
            }
        };
        frame.child(face).into_any_element()
    }
}

impl Render for Person {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let arrived = self.arrival.value(window);
        let shown = arrived.clamp(0., 1.);
        let account = self.account.clone().unwrap_or_default();
        if !self.large {
            let name = self.named.then(|| {
                div()
                    .opacity(shown)
                    .font_weight(FontWeight::MEDIUM)
                    .child(one_line(account.name.clone()))
            });
            return pill(&theme)
                .gap(theme.spacing.small)
                .child(self.portrait(&theme, SMALL_PICTURE, arrived))
                .children(name)
                .into_any_element();
        }
        // The name of the account stands under the full name where the two
        // differ.
        let login = (account.login != account.name).then(|| {
            div()
                .min_w_0()
                .text_size(LOGIN_SIZE)
                .text_color(theme.colors.on_surface_variant)
                .child(one_line(account.login.clone()))
        });
        div()
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .pb(theme.spacing.medium)
            .child(self.portrait(&theme, LARGE_PICTURE, arrived))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .relative()
                    .left(px(ROW_TRAVEL * (1. - shown)))
                    .opacity(shown)
                    .child(
                        div()
                            .min_w_0()
                            .text_size(NAME_SIZE)
                            .font_weight(FontWeight::MEDIUM)
                            .child(one_line(account.name)),
                    )
                    .children(login),
            )
            .into_any_element()
    }
}

/// A folder of the account, as a row.
struct PlaceRow {
    place: Place,
    path: PathBuf,
    interaction: Interaction,
}

/// The folders of the account; a click opens one.
struct Places {
    /// The folders Windows has told the place of.
    rows: Vec<PlaceRow>,
    /// 0 until the folders are known, 1 once the rows are in place.
    arrival: Spring,
}

impl Places {
    fn new(places: Vec<Place>, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let found = cx.background_executor().spawn(async move {
                let found = |place: Place| Some((place, place.path()?));
                places.into_iter().filter_map(found).collect::<Vec<_>>()
            });
            let found = found.await;
            this.update(cx, |this, cx| {
                let motion = cx.global::<Theme>().motion;
                let row = |(place, path)| PlaceRow {
                    place,
                    path,
                    interaction: Interaction::new(&motion),
                };
                this.rows = found.into_iter().map(row).collect();
                this.arrival.animate_to(1.);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            rows: Vec::new(),
            arrival: Spring::new(cx.global::<Theme>().motion.spatial, 0.),
        }
    }

    fn open(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(index) {
            let folder = Shortcut {
                name: look(row.place).0.to_owned(),
                path: row.path.clone(),
            };
            if !Files::open_folder(&row.path, cx) {
                folder.launch();
            }
            Popups::close(cx);
        }
    }
}

impl Render for Places {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let arrival = self.arrival.value(window).clamp(0., 1.);
        let count = self.rows.len();
        let mut rows = Vec::with_capacity(count);
        for (index, row) in self.rows.iter_mut().enumerate() {
            let (name, glyph) = look(row.place);
            let arrived = staggered(arrival, index, count);
            let state = row.interaction.state(window);
            let ground = theme.colors.surface_container_highest;
            let note = row.path.display().to_string();
            let element = list_row(&theme, glyph, false, language.say(name), note)
                .id(("place", index))
                .relative()
                .top(px(ROW_TRAVEL * (1. - arrived)))
                .opacity(arrived)
                .bg(state_layer(&theme, ground, state))
                .on_click(cx.listener(move |this, _, _, cx| this.open(index, cx)));
            let element = Interaction::track(element, cx, move |this: &mut Self| {
                Some(&mut this.rows.get_mut(index)?.interaction)
            });
            rows.push(element);
        }
        div()
            .id("places")
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .overflow_y_scroll()
            .children(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_stood_for_by_its_first_letter() {
        assert_eq!(initial("ada lovelace"), "A");
        assert_eq!(initial("  юзер"), "Ю");
        assert_eq!(initial(" "), "");
    }

    #[test]
    fn a_layout_names_the_folders() {
        assert_eq!(place_named("downloads"), Some(Place::Downloads));
        for place in Place::ALL {
            assert_eq!(place_named(place.code()), Some(place));
        }
    }
}
