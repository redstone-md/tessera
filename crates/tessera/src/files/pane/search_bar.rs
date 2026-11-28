// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The bar under the search field of a pane: filters, history and saving a
//! search. Filled by files-search-actions.
//!
//! The chips of the bar write the same words of the Advanced Query Syntax
//! a search can be typed with (`kind:pictures`, `modified:this-week`,
//! `size:large`, `tag:sea`, `content:on`), so that what they set is part of
//! the search and can be read and changed by hand ("Edit search"). It
//! shows while the pane shows a search, and while its field has the focus
//! or holds text, when it also offers the searches made last. A search is
//! saved as the `.search-ms` file Windows reads, into the user's Searches.
#![allow(
    dead_code,
    reason = "files-namespace's pane.rs draws the bar under the search field once both are merged"
)]

use std::path::{Path, PathBuf};

use chrono::{Days, Local};
use gpui::{
    AnyElement, App, AsyncApp, ClickEvent, Context, Div, InteractiveElement, IntoElement,
    ParentElement, SharedString, Stateful, StatefulInteractiveElement, Styled, WeakEntity, Window,
    div, px,
};

use super::FolderPane;
use crate::banners::{Banners, Notice};
use crate::files::Location;
use crate::files::search::aqs::{self, DateSpan, Filter, Kind, Query, SizeTier};
use crate::files::search::{history, notes_of, saved, tags};
use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::popup::PopupAnchor;
use crate::runners::fill;
use crate::theme::Theme;
use crate::ui::{icon, one_line};

const ICON_KIND: &str = "\u{e8fd}";
const ICON_DATE: &str = "\u{e787}";
const ICON_SIZE: &str = "\u{e9d9}";
const ICON_TAG: &str = "\u{e8ec}";
const ICON_CONTENTS: &str = "\u{e8a5}";
const ICON_EDIT: &str = "\u{e70f}";
const ICON_SAVE: &str = "\u{e74e}";
const ICON_CLEAR: &str = "\u{e894}";
const ICON_HISTORY: &str = "\u{e81c}";
const ICON_MORE: &str = "\u{e70d}";
const ICON_NONE: &str = "";

/// How many of the searches made last the bar offers.
const RECENT_SHOWN: usize = 8;

/// How many tags a menu of tags lists.
const TAGS_SHOWN: usize = 40;

/// What the menu of a chip offers: the English title of a choice, and the
/// value of the filter it sets as `with_filter` writes it.
type Choice = (&'static str, Option<String>);

impl FolderPane {
    /// The bar under the search field, where it is to be shown: while the
    /// pane shows a search, or its field has the focus or holds text.
    pub fn search_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let searching = matches!(self.history.current(), Location::Search { .. });
        let field = self.filter.read(cx);
        let typing = field.is_focused(window);
        let written = field.text();
        if !searching && !typing && written.trim().is_empty() {
            return None;
        }
        self.history.current().root()?;
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let text = self.query_text(cx);
        let query = Query::parse(&text);
        let mut bar = div()
            .id("files-search-bar")
            .flex_none()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .p(theme.spacing.small)
            .border_b_1()
            .border_color(theme.colors.outline.opacity(0.25))
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface);
        bar = bar.child(self.filter_chips(&query, &text, &theme, language, cx));
        if typing {
            if let Some(recent) = self.recent_chips(&theme, language, cx) {
                bar = bar.child(recent);
            }
            if !written.trim().is_empty() && !searching {
                let hint = language.say("Press Enter to search below this folder.");
                bar = bar.child(note(&theme, hint));
            }
        }
        let notes = notes_of(cx.entity_id()).filter(|_| searching);
        if let Some(notes) = notes.filter(|notes| notes.unsearched > 0) {
            let count = notes.unsearched.to_string();
            let said = language.say(
                "{} files were not searched for what they hold: they are no text Tessera reads, or larger than 8 MB.",
            );
            bar = bar.child(note(&theme, fill(said, &[&count])));
        }
        Some(bar.into_any_element())
    }

    /// What the search shown asks, or what the field holds where no search
    /// is shown.
    fn query_text(&self, cx: &App) -> String {
        match self.history.current() {
            Location::Search { query, .. } => query.clone(),
            _ => self.filter.read(cx).text(),
        }
    }

    /// The folder searches of the bar look below.
    fn search_root(&self) -> Option<PathBuf> {
        self.history.current().root().map(Path::to_owned)
    }

    /// Searches for `text` below the folder searched or shown; with
    /// nothing asked, goes back to the folder.
    fn search_for(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.search_root() else {
            return;
        };
        let location = match Query::parse(&text).is_empty() {
            true => Location::Folder(root),
            false => Location::Search {
                root,
                query: aqs::shown(&text),
            },
        };
        self.go(location, window, cx);
    }

    /// Sets `filter` of the search to `value`, or takes it out.
    fn set_filter(
        &mut self,
        filter: Filter,
        value: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = aqs::with_filter(&self.query_text(cx), filter, value);
        self.search_for(text, window, cx);
    }

    /// Has the pane set `filter` to `value` once the menu that chose it
    /// is gone.
    fn chosen_filter(pane: &WeakEntity<Self>, filter: Filter, value: Option<String>, cx: &mut App) {
        pane.update(cx, |pane, cx| {
            pane.after_menu(cx, move |pane, window, cx| {
                pane.set_filter(filter, value.as_deref(), window, cx)
            })
        })
        .ok();
    }

    /// The chips of the filters, and those that edit, save and clear the
    /// search.
    fn filter_chips(
        &self,
        query: &Query,
        text: &str,
        theme: &Theme,
        language: Language,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.spacing.tiny);
        let kind = match query.kinds.as_slice() {
            [] => language.say("Kind").to_owned(),
            [one] => language.say(one.title()).to_owned(),
            _ => language.say("Several kinds").to_owned(),
        };
        let kinds: Vec<Choice> = { Kind::ALL.into_iter() }
            .map(|kind| (kind.title(), Some(kind.word().to_owned())))
            .collect();
        let chosen = aqs::filter_value(text, Filter::Kind);
        row = row.child(self.menu_chip(
            ("kind", ICON_KIND, kind, !query.kinds.is_empty()),
            (Filter::Kind, "Any kind", kinds, chosen),
            theme,
            language,
            cx,
        ));

        let modified = match query.modified {
            None => language.say("Date modified").to_owned(),
            Some(span) => { DateSpan::CHIPS.iter() }
                .find(|(chip, _)| *chip == span)
                .map(|(_, title)| language.say(title).to_owned())
                .unwrap_or_else(|| span.written()),
        };
        let mut spans: Vec<Choice> = { DateSpan::CHIPS.into_iter() }
            .map(|(span, title)| (title, Some(span.written())))
            .collect();
        spans.push(("Last week", Some(DateSpan::LastWeek.written())));
        spans.push(("Last month", Some(DateSpan::LastMonth.written())));
        let chosen = query.modified.map(DateSpan::written);
        let date_chip = self.menu_chip(
            ("modified", ICON_DATE, modified, query.modified.is_some()),
            (Filter::Modified, "Any time", spans, chosen),
            theme,
            language,
            cx,
        );
        row = row.child(date_chip);

        let size = match query.size {
            None => language.say("Size").to_owned(),
            Some(aqs::SizeSpan::Tier(tier)) => language.say(tier.title()).to_owned(),
            Some(span) => span.written(),
        };
        let tiers: Vec<Choice> = { SizeTier::ALL.into_iter() }
            .map(|tier| (tier.title(), Some(tier.word().to_owned())))
            .collect();
        let chosen = query.size.map(aqs::SizeSpan::written);
        row = row.child(self.menu_chip(
            ("size", ICON_SIZE, size, query.size.is_some()),
            (Filter::Size, "Any size", tiers, chosen),
            theme,
            language,
            cx,
        ));

        let tag = match query.tags.as_slice() {
            [] => language.say("Tags").to_owned(),
            [one] => one.clone(),
            several => several.join(", "),
        };
        row = row.child(self.tag_chip(tag, !query.tags.is_empty(), theme, cx));

        let contents = query.contents;
        row = row.child(
            chip(
                theme,
                "contents",
                ICON_CONTENTS,
                language.say("Contents"),
                contents,
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                let value = (!contents).then_some("on");
                this.set_filter(Filter::Contents, value, window, cx);
            })),
        );

        if matches!(self.history.current(), Location::Search { .. }) {
            row = row.child(
                chip(theme, "edit", ICON_EDIT, language.say("Edit search"), false).on_click(
                    cx.listener(|this, _: &ClickEvent, window, cx| this.edit_search(window, cx)),
                ),
            );
            row = row.child(
                chip(theme, "save", ICON_SAVE, language.say("Save search"), false)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save_search(cx))),
            );
            let filtered = query.contents
                || !query.kinds.is_empty()
                || query.modified.is_some()
                || query.size.is_some()
                || !query.tags.is_empty();
            if filtered {
                row = row.child(
                    chip(
                        theme,
                        "clear",
                        ICON_CLEAR,
                        language.say("Clear filters"),
                        false,
                    )
                    .on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| {
                            let mut text = this.query_text(cx);
                            for filter in [
                                Filter::Kind,
                                Filter::Modified,
                                Filter::Size,
                                Filter::Tag,
                                Filter::Contents,
                            ] {
                                text = aqs::with_filter(&text, filter, None);
                            }
                            this.search_for(text, window, cx);
                        },
                    )),
                );
            }
        }
        row
    }

    /// A chip that opens a menu of `choices` for `filter`, with an entry
    /// that takes the filter out first, titled `none`; `chosen` is the
    /// value set now.
    fn menu_chip(
        &self,
        (key, glyph, label, lit): (&'static str, &'static str, String, bool),
        (filter, none, choices, chosen): (Filter, &'static str, Vec<Choice>, Option<String>),
        theme: &Theme,
        language: Language,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let pane = cx.entity().downgrade();
        let mut entries = vec![{
            let pane = pane.clone();
            MenuEntry::new(ICON_NONE, language.say(none), move |cx| {
                Self::chosen_filter(&pane, filter, None, cx)
            })
            .checked(chosen.is_none())
        }];
        for (title, value) in choices {
            let pane = pane.clone();
            let checked = chosen.is_some() && value == chosen;
            entries.push(
                MenuEntry::new(ICON_NONE, language.say(title), move |cx| {
                    Self::chosen_filter(&pane, filter, value.clone(), cx)
                })
                .checked(checked),
            );
        }
        if filter == Filter::Modified {
            let pane = pane.clone();
            entries.push(MenuEntry::new(
                ICON_EDIT,
                language.say("A range of days…"),
                move |cx| {
                    pane.update(cx, |pane, cx| {
                        pane.after_menu(cx, |pane, window, cx| pane.write_range(window, cx))
                    })
                    .ok();
                },
            ));
        }
        chip_with_menu(theme, key, glyph, label, lit)
            .on_click(move |event, window, cx| open_at(entries.clone(), event, window, cx))
    }

    /// The chip of the tags: its menu lists the tags used below the folder,
    /// as the search found them or as they are looked for when it opens.
    fn tag_chip(
        &self,
        label: String,
        lit: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let pane = cx.entity().downgrade();
        chip_with_menu(theme, "tags", ICON_TAG, label, lit).on_click(move |event, window, cx| {
            let anchor = match PopupAnchor::under(window, event.position(), cx) {
                Ok(anchor) => anchor,
                Err(error) => {
                    report!("the menu has no place: {error:#}");
                    return;
                }
            };
            let found = { notes_of(pane.entity_id()) }
                .map(|notes| notes.tags)
                .filter(|tags| !tags.is_empty());
            let Ok((root, hidden)) =
                pane.read_with(cx, |pane, _| (pane.search_root(), pane.shown().hidden))
            else {
                return;
            };
            let Some(root) = root else {
                return;
            };
            let pane = pane.clone();
            if let Some(tally) = found.or_else(|| tags::known(&root)) {
                let entries = tag_entries(&pane, tally, Language::of(cx));
                Menus::open(entries, anchor, cx);
                return;
            }
            let gathered = tags::gather(root, hidden);
            cx.spawn(async move |cx: &mut AsyncApp| {
                let Ok(tally) = gathered.recv().await else {
                    return;
                };
                cx.update(|cx| {
                    let entries = tag_entries(&pane, tally, Language::of(cx));
                    Menus::open(entries, anchor, cx);
                })
                .ok();
            })
            .detach();
        })
    }

    /// The chips of the searches made last, while the field has the focus;
    /// the history is read the first time it is wanted.
    fn recent_chips(
        &self,
        theme: &Theme,
        language: Language,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let Some(recent) = history::recent() else {
            let pane = cx.entity().downgrade();
            cx.spawn(async move |_, cx: &mut AsyncApp| {
                cx.background_executor()
                    .spawn(async { history::load() })
                    .await;
                pane.update(cx, |_, cx| cx.notify()).ok();
            })
            .detach();
            return None;
        };
        if recent.is_empty() {
            return None;
        }
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.spacing.tiny)
            .child(
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(language.say("Recent searches")),
            );
        for (index, text) in recent.into_iter().take(RECENT_SHOWN).enumerate() {
            row = row.child(
                chip(theme, ("recent", index), ICON_HISTORY, text.clone(), false).on_click(
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.search_for(text.clone(), window, cx)
                    }),
                ),
            );
        }
        row = row.child(
            chip(
                theme,
                "forget",
                ICON_CLEAR,
                language.say("Clear history"),
                false,
            )
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                let cleared = cx.background_executor().spawn(async { history::clear() });
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    cleared.await;
                    this.update(cx, |_, cx| cx.notify()).ok();
                })
                .detach();
            })),
        );
        Some(row)
    }

    /// Puts what the search shown asks into the field, to be changed and
    /// searched again with Enter.
    fn edit_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.query_text(cx);
        self.filter
            .update(cx, |field, cx| field.put_text(&text, cx));
        self.filter.read(cx).focus(window);
    }

    /// Puts a range of days into the field, the last thirty to today, to
    /// be changed and searched with Enter.
    fn write_range(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        let from = today.checked_sub_days(Days::new(30)).unwrap_or(today);
        let span = DateSpan::Between(from, today).written();
        let text = aqs::with_filter(&self.query_text(cx), Filter::Modified, Some(&span));
        self.filter
            .update(cx, |field, cx| field.put_text(&text, cx));
        self.filter.read(cx).focus(window);
    }

    /// Saves the search shown into the user's Searches, away from the user
    /// interface, and tells where it went.
    fn save_search(&mut self, cx: &mut Context<Self>) {
        let Location::Search { root, query } = self.history.current().clone() else {
            return;
        };
        let language = Language::of(cx);
        let folder = { root.file_name() }
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let title = fill(language.say("{} in {}"), &[&query, &folder]);
        let today = Local::now().date_naive();
        let saving = cx
            .background_executor()
            .spawn(async move { saved::save(&root, &query, &title, today) });
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            let saved = saving.await;
            cx.update(|cx| {
                let (title, body) = match saved {
                    Ok(path) => (language.say("Search saved"), path.display().to_string()),
                    Err(error) => (
                        language.say("The search was not saved"),
                        format!("{error:#}"),
                    ),
                };
                let notice = Notice {
                    app: language.say("Files").to_owned(),
                    title: title.to_owned(),
                    body,
                    glyph: Some(ICON_SAVE),
                    ..Notice::default()
                };
                Banners::post(notice, cx);
            })
            .ok();
        })
        .detach();
    }
}

/// The entries of a menu of tags: none, and each of `tally`, most used
/// first.
fn tag_entries(
    pane: &WeakEntity<FolderPane>,
    tally: tags::Tally,
    language: Language,
) -> Vec<MenuEntry> {
    let none = {
        let pane = pane.clone();
        MenuEntry::new(ICON_NONE, language.say("Any tag"), move |cx| {
            FolderPane::chosen_filter(&pane, Filter::Tag, None, cx)
        })
    };
    if tally.is_empty() {
        return vec![
            none,
            MenuEntry::heading(language.say("No tags were found here")),
        ];
    }
    let tagged = { tally.into_iter().take(TAGS_SHOWN) }.map(|(tag, count)| {
        let pane = pane.clone();
        let label = format!("{tag}  ({count})");
        MenuEntry::new(ICON_TAG, label, move |cx| {
            FolderPane::chosen_filter(&pane, Filter::Tag, Some(tag.clone()), cx)
        })
    });
    std::iter::once(none).chain(tagged).collect()
}

/// Opens the menu `entries` under the pointer of `event`.
fn open_at(entries: Vec<MenuEntry>, event: &ClickEvent, window: &mut Window, cx: &mut App) {
    match PopupAnchor::under(window, event.position(), cx) {
        Ok(anchor) => Menus::open(entries, anchor, cx),
        Err(error) => report!("the menu has no place: {error:#}"),
    }
}

/// A line that tells something of the search.
fn note(theme: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .text_color(theme.colors.on_surface_variant)
        .child(text.into())
}

/// A rounded chip with an icon and a text, filled while `lit`.
fn chip(
    theme: &Theme,
    id: impl Into<gpui::ElementId>,
    glyph: &'static str,
    label: impl Into<SharedString>,
    lit: bool,
) -> Stateful<Div> {
    let ground = match lit {
        true => theme.colors.secondary_container,
        false => theme.colors.surface_container_highest.opacity(0.6),
    };
    let hover = match lit {
        true => theme.colors.secondary_container.opacity(0.85),
        false => theme.colors.surface_container_highest,
    };
    div()
        .id(id)
        .flex_none()
        .max_w(px(260.))
        .h(px(28.))
        .flex()
        .items_center()
        .gap(theme.spacing.tiny)
        .px(theme.spacing.small)
        .rounded(theme.shape.full)
        .bg(ground)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .text_color(match lit {
            true => theme.colors.on_surface,
            false => theme.colors.on_surface_variant,
        })
        .child(icon(theme, glyph))
        .child(one_line(label.into()))
}

/// A chip that opens a menu, with the mark of one at its end.
fn chip_with_menu(
    theme: &Theme,
    id: &'static str,
    glyph: &'static str,
    label: String,
    lit: bool,
) -> Stateful<Div> {
    chip(theme, id, glyph, label, lit).child(icon(theme, ICON_MORE).text_size(px(10.)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chips_of_kinds_and_sizes_write_words_the_search_reads() {
        for kind in Kind::ALL {
            let text = aqs::with_filter("x", Filter::Kind, Some(kind.word()));
            assert_eq!(Query::parse(&text).kinds, [kind]);
        }
        for tier in SizeTier::ALL {
            let text = aqs::with_filter("", Filter::Size, Some(tier.word()));
            assert_eq!(Query::parse(&text).size, Some(aqs::SizeSpan::Tier(tier)));
        }
        for (span, _) in DateSpan::CHIPS {
            let text = aqs::with_filter("", Filter::Modified, Some(&span.written()));
            assert_eq!(Query::parse(&text).modified, Some(span));
        }
    }
}
