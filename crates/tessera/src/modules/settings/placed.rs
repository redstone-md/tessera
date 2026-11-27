// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Widgets section: every widget placed on a bar, in a popup of the
//! configuration file or on a card of the desktop, each a chip that opens
//! a card of the options it declares, set for that one widget.

use gpui::{AnyElement, Context, IntoElement, ParentElement, Window};

use super::controls::{CardParts, Owner, RowKey};
use super::{Control, Face, Settings, card, held, note, surface_title, widget_title, wrap};
use crate::config::edit::{Slot, WidgetPlace};
use crate::config::{Config, WidgetSpec};
use crate::i18n::Language;
use crate::options::{OptionDecl, text_of};
use crate::theme::Theme;

/// The options of a widget's entry that are no options of the widget.
const NOT_OPTIONS: [&str; 2] = ["widget", "popup"];

/// What holds placed widgets: its title, and each of its widgets with its
/// place.
type Holder<'a> = (String, Vec<(WidgetPlace, &'a WidgetSpec)>);

/// Every holder of placed widgets in `config`: the surfaces by edge and
/// slot, the popups of the file by name, the cards of the desktop.
fn holders<'a>(config: &'a Config, language: Language) -> Vec<Holder<'a>> {
    let mut holders = Vec::new();
    for (surface, config_of) in config.surfaces.iter().enumerate() {
        let mut widgets = Vec::new();
        for slot in Slot::ALL {
            for (index, spec) in held(config_of, slot).iter().enumerate() {
                let place = WidgetPlace::Surface {
                    surface,
                    slot,
                    index,
                };
                widgets.push((place, spec));
            }
        }
        holders.push((surface_title(config_of, language), widgets));
    }
    for popup in &config.popups {
        let widgets = { popup.widgets().enumerate() }.map(|(index, spec)| {
            let name = popup.name.clone();
            (WidgetPlace::Popup { name, index }, spec)
        });
        let title = format!("{} {}", language.say("Popup"), popup.name);
        holders.push((title, widgets.collect()));
    }
    for (index, desk_card) in config.cards.iter().enumerate() {
        let widgets = { desk_card.content.iter().enumerate() }
            .map(|(widget, spec)| (WidgetPlace::Card { index, widget }, spec));
        let title = format!("{} {}", language.say("Card"), index + 1);
        holders.push((title, widgets.collect()));
    }
    holders
}

impl Settings {
    /// The cards of the Widgets section, as wide as the page: what it
    /// holds, then a card for each holder of placed widgets with the card
    /// of the options of the widget opened in it. `first` is the place of
    /// the first among the cards of the page, for the widget asked for to
    /// be brought into sight.
    pub(super) fn placed_widgets(
        &mut self,
        config: &Config,
        first: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let about = "Every widget placed on a bar, in a popup of the configuration file or on a card of the desktop. Its options are set for it alone.";
        let mut cards = vec![
            card(&theme, language.say("Placed widgets"))
                .child(note(&theme, language.say(about)))
                .into_any_element(),
        ];
        let expanded = self.expanded.clone();
        for (title, widgets) in holders(config, language) {
            if widgets.is_empty() {
                continue;
            }
            let mut chips = Vec::new();
            let mut opened = None;
            for (place, spec) in widgets {
                let open = expanded.as_ref() == Some(&place);
                if open {
                    opened = Some((place.clone(), spec));
                }
                let face = Face::Chip {
                    text: widget_title(spec, language).into(),
                    selected: open,
                };
                chips.push(self.control(Control::Expand(place), face, window, cx));
            }
            let options = opened.map(|(place, spec)| {
                self.seek(&place.item(), first + cards.len());
                self.widget_options_card(&place, spec, window, cx)
            });
            cards.push(
                card(&theme, title)
                    .child(wrap(&theme).children(chips))
                    .children(options)
                    .into_any_element(),
            );
        }
        cards
    }

    /// The card of the options the widget at `place` declares, and of
    /// those its entry holds that nothing declares, which are said as
    /// they are written.
    fn widget_options_card(
        &mut self,
        place: &WidgetPlace,
        spec: &WidgetSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let decls = self.widget_declarations(&spec.widget, cx);
        let declared = |key: &str| decls.iter().any(|decl| decl.key == key);
        let undeclared: Vec<String> = { spec.options.iter() }
            .filter(|(key, _)| !declared(key) && !NOT_OPTIONS.contains(&key.as_str()))
            .map(|(key, value)| format!("{key} = {}", text_of(value)))
            .collect();
        let mut footer = Vec::new();
        if !undeclared.is_empty() {
            let set_by_hand = "These options are set in config.toml:";
            footer.push(note(&theme, language.say(set_by_hand)).into_any_element());
            for line in undeclared {
                footer.push(note(&theme, line).into_any_element());
            }
        }
        let owner = Owner::Widget(place.clone());
        let rows: Vec<(RowKey, OptionDecl)> = { decls.into_iter() }
            .map(|decl| (RowKey::new(owner.clone(), &decl.key), decl))
            .collect();
        let remark = rows
            .is_empty()
            .then_some("This widget declares no options; what it reads is written in config.toml.");
        let parts = CardParts {
            item: place.item(),
            title: widget_title(spec, language).into(),
            rows,
            problems: &[],
            remark,
            footer,
            affects: &|_| Vec::new(),
        };
        self.declared_card(parts, window, cx)
    }
}
