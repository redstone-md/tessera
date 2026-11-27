// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Search of the settings: a field at the top of the rail that finds every
//! card and row of every section, of Tessera's own settings, of the
//! installed extensions and of the themes in use, by what they say in the
//! language shown and in English. Ctrl+F goes to the field, Escape empties
//! it; Up, Down and Enter move through the results, which are grouped by
//! section, and a click or Enter goes to the section and lights the row.

use std::ops::Range;

use gpui::{
    AnyElement, AppContext, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, UniformListScrollHandle,
    Window, div, px, uniform_list,
};

use super::{Section, Settings, note, theme_title};
use crate::field::TextField;
use crate::i18n::Language;
use crate::motion::mix;
use crate::options::OptionDecl;
use crate::theme::Theme;
use crate::ui::icon;

/// How tall a line of the results is, a section's or a result's.
const LINE_HEIGHT: f32 = 36.;

/// The glyph of the field.
const ICON_SEARCH: &str = "\u{e721}";

/// The cards drawn by hand, by section and English title, which search
/// finds by their titles.
const HAND_DRAWN: &[(Section, &str)] = &[
    (Section::General, "Arrangement"),
    (Section::General, "Language"),
    (Section::General, "Icon packs"),
    (Section::General, "Startup apps"),
    (Section::General, "Backups"),
    (Section::Bars, "By hand"),
    (Section::Bars, "Hiding"),
    (Section::Desktop, "Cards"),
    (Section::Widgets, "Placed widgets"),
    (Section::Dock, "Pinned applications"),
    (Section::Keys, "Keys"),
    (Section::Themes, "Theme"),
    (Section::Themes, "Layers"),
    (Section::Extensions, "Installed extensions"),
];

/// What a text that is searched says of what it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Weight {
    /// What it is called: a title, a label, a choice.
    Label,
    /// What is said of it.
    Description,
}

/// How a text holds what is looked for; the first is the best.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Found {
    /// As whole words.
    Words,
    /// At the start of a word.
    Start,
    /// Anywhere.
    Within,
}

/// How well `text` holds `query`, both in lowercase: as whole words, at
/// the start of a word, or anywhere; none where it does not.
fn found(text: &str, query: &str) -> Option<Found> {
    let letter = |character: Option<char>| character.is_some_and(char::is_alphanumeric);
    let mut best = None;
    for (at, _) in text.match_indices(query) {
        let before = text[..at].chars().next_back();
        let after = text[at + query.len()..].chars().next();
        let kind = match (letter(before), letter(after)) {
            (false, false) => Found::Words,
            (false, true) => Found::Start,
            _ => Found::Within,
        };
        best = Some(best.map_or(kind, |best: Found| best.min(kind)));
    }
    best
}

/// How well something said by `texts` answers `query`: lower is better;
/// none where no text holds it. A whole word beats the start of a word,
/// which beats any other place, and a label beats a description.
pub(super) fn rank(query: &str, texts: &[(String, Weight)]) -> Option<u8> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return None;
    }
    { texts.iter() }
        .filter_map(|(text, weight)| {
            let kind = found(&text.to_lowercase(), &query)?;
            Some(kind as u8 * 2 + *weight as u8)
        })
        .min()
}

/// Something search finds: where it stands and what it says.
#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub section: Section,
    /// What `open_at` names it by, to light it.
    pub item: String,
    /// What it is called, in the language shown.
    pub title: String,
    /// The card it stands on, in the language shown; none for a card.
    pub card: Option<String>,
    /// What it says, in the language shown and in English.
    pub texts: Vec<(String, Weight)>,
}

impl Entry {
    fn new(section: Section, item: String, title: String, card: Option<String>) -> Self {
        Self {
            section,
            item,
            title,
            card,
            texts: Vec::new(),
        }
    }

    /// Adds `english` and what it is in `language`.
    fn say(&mut self, english: &str, weight: Weight, language: Language) {
        let shown = language.translate(english);
        if shown != english {
            self.texts.push((shown, weight));
        }
        self.texts.push((english.to_owned(), weight));
    }

    /// Adds what a declared setting says: its label, the name of no
    /// value, its choices and what it does.
    fn declared(&mut self, decl: &OptionDecl, language: Language) {
        let mut labels = vec![&decl.label];
        labels.extend(decl.unset_label.as_ref());
        labels.extend(
            decl.choices
                .iter()
                .filter_map(|choice| choice.label.as_ref()),
        );
        for label in labels {
            self.label(label, Weight::Label, language);
        }
        if let Some(description) = &decl.description {
            self.label(description, Weight::Description, language);
        }
    }

    fn label(&mut self, label: &crate::options::Label, weight: Weight, language: Language) {
        let (shown, english) = (label.text(language), label.english());
        if shown != english {
            self.texts.push((shown, weight));
        }
        self.texts.push((english, weight));
    }
}

/// A line of the results: the title of a section, or a result by its
/// place among the entries.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    Section(Section),
    Result(usize),
}

/// What the search keeps: its field, what it searches, and what it found.
pub(super) struct SearchState {
    pub field: Entity<TextField>,
    /// What was looked for last.
    query: String,
    entries: Vec<Entry>,
    lines: Vec<Line>,
    /// The place among the lines of the result chosen with Up and Down.
    chosen: Option<usize>,
    scroll: UniformListScrollHandle,
}

impl SearchState {
    pub(super) fn new(cx: &mut Context<Settings>) -> Self {
        let placeholder = Language::of(cx).say("Search");
        let field = cx.new(|cx| TextField::new(placeholder.into(), cx).with_glyph(ICON_SEARCH));
        cx.observe(&field, |this: &mut Settings, _, cx| this.searched(cx))
            .detach();
        Self {
            field,
            query: String::new(),
            entries: Vec::new(),
            lines: Vec::new(),
            chosen: None,
            scroll: UniformListScrollHandle::new(),
        }
    }

    /// Whether something is looked for, so that the results stand in place
    /// of the cards.
    pub(super) fn is_active(&self) -> bool {
        !self.query.trim().is_empty()
    }
}

/// The lines of the results of `query` among `entries`: by section in the
/// order of the rail, the title of each first, each result by how well it
/// answers and then in the order of the entries.
fn lines_of(query: &str, entries: &[Entry]) -> Vec<Line> {
    let mut found: Vec<(usize, u8, usize)> = { entries.iter().enumerate() }
        .filter_map(|(at, entry)| {
            let rank = rank(query, &entry.texts)?;
            let section = Section::ALL
                .iter()
                .position(|known| *known == entry.section)?;
            Some((section, rank, at))
        })
        .collect();
    // The best result of a section brings it up.
    let best = |section: usize, found: &[(usize, u8, usize)]| {
        { found.iter() }
            .filter(|(known, ..)| *known == section)
            .map(|(_, rank, _)| *rank)
            .min()
            .unwrap_or(u8::MAX)
    };
    let ordered = found.clone();
    found.sort_by_key(|&(section, rank, at)| (best(section, &ordered), section, rank, at));
    let mut lines = Vec::new();
    let mut last = None;
    for (section, _, at) in found {
        if last != Some(section) {
            lines.push(Line::Section(Section::ALL[section]));
            last = Some(section);
        }
        lines.push(Line::Result(at));
    }
    lines
}

impl Settings {
    /// What search finds: every card and row of the rows files, every
    /// installed extension and theme in use with what they declare.
    fn search_entries(&self, language: Language) -> Vec<Entry> {
        let mut entries = Vec::new();
        for &(section, title) in HAND_DRAWN {
            let shown = language.say(title).to_owned();
            let mut card = Entry::new(section, title.to_owned(), shown, None);
            card.say(title, Weight::Label, language);
            entries.push(card);
        }
        for group in self.declared.builtin.groups() {
            let card = language.translate(&group.title);
            let mut whole = Entry::new(group.section, group.title.clone(), card.clone(), None);
            whole.say(&group.title, Weight::Label, language);
            for word in &group.search {
                whole.say(word, Weight::Label, language);
            }
            for link in &group.links {
                whole.label(&link.label, Weight::Label, language);
                if let Some(description) = &link.description {
                    whole.label(description, Weight::Description, language);
                }
            }
            for page in &group.pages {
                whole.say(page, Weight::Label, language);
            }
            entries.push(whole);
            for (target, decl) in &group.rows {
                let title = decl.label.text(language);
                let mut row = Entry::new(group.section, target.item(), title, Some(card.clone()));
                row.declared(decl, language);
                entries.push(row);
            }
        }
        for extension in self.declared.extensions() {
            let id = extension.id.clone();
            let mut whole = Entry::new(Section::Extensions, id.clone(), id.clone(), None);
            whole.texts.push((id.clone(), Weight::Label));
            entries.push(whole);
            for decl in &extension.settings.decls {
                let title = decl.label.text(language);
                let mut row = Entry::new(Section::Extensions, id.clone(), title, Some(id.clone()));
                row.declared(decl, language);
                entries.push(row);
            }
        }
        for (name, declared) in self.declared.themes() {
            let shown = theme_title(name, language).to_string();
            let mut whole = Entry::new(Section::Themes, name.clone(), shown.clone(), None);
            whole.texts.push((shown.clone(), Weight::Label));
            whole.texts.push((name.clone(), Weight::Label));
            entries.push(whole);
            for decl in declared.iter().flat_map(|(set, _)| &set.decls) {
                let title = decl.label.text(language);
                let mut row = Entry::new(Section::Themes, name.clone(), title, Some(shown.clone()));
                row.declared(decl, language);
                entries.push(row);
            }
        }
        entries
    }

    /// Looks again for what the field holds.
    fn searched(&mut self, cx: &mut Context<Self>) {
        let query = self.search.field.read(cx).text();
        if query == self.search.query {
            return;
        }
        if self.search.query.trim().is_empty() {
            // What is searched is read once each time a search starts.
            self.search.entries = self.search_entries(Language::of(cx));
        }
        self.search.lines = lines_of(&query, &self.search.entries);
        self.search.query = query;
        self.search.chosen =
            { self.search.lines.iter() }.position(|line| matches!(line, Line::Result(_)));
        cx.notify();
    }

    /// Ctrl+F goes to the field, Escape empties it; Up and Down choose a
    /// result and Enter goes to it.
    pub(super) fn search_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keys = &event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "f" if keys.control => {
                self.search.field.read(cx).focus(window);
                cx.stop_propagation();
            }
            "escape" if self.search.is_active() => {
                self.search.field.update(cx, |field, cx| field.clear(cx));
                cx.stop_propagation();
            }
            "down" | "up" if self.search.is_active() => {
                let up = event.keystroke.key == "up";
                self.choose_next(up, cx);
                cx.stop_propagation();
            }
            "enter" if self.search.is_active() => {
                if let Some(at) = self.search.chosen {
                    self.go_to_result(at, cx);
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    /// Chooses the result after the one chosen, or before it.
    fn choose_next(&mut self, up: bool, cx: &mut Context<Self>) {
        let results: Vec<usize> = { self.search.lines.iter().enumerate() }
            .filter(|(_, line)| matches!(line, Line::Result(_)))
            .map(|(at, _)| at)
            .collect();
        let now =
            { self.search.chosen }.and_then(|chosen| results.iter().position(|at| *at == chosen));
        let next = match (now, up) {
            (None, _) => results.first(),
            (Some(now), true) => results.get(now.saturating_sub(1)),
            (Some(now), false) => results.get(now + 1).or(results.last()),
        };
        self.search.chosen = next.copied();
        if let Some(chosen) = self.search.chosen {
            self.search
                .scroll
                .scroll_to_item(chosen, gpui::ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Goes to the result at `line`: its section, with it lit, and the
    /// field emptied.
    fn go_to_result(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(Line::Result(at)) = self.search.lines.get(line).copied() else {
            return;
        };
        let Some(entry) = self.search.entries.get(at).cloned() else {
            return;
        };
        self.search.field.update(cx, |field, cx| field.clear(cx));
        self.go_to((entry.section, Some(entry.item)), cx);
    }

    /// The lines of the results at `range`.
    fn result_lines(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut lines = Vec::new();
        for at in range {
            let Some(line) = self.search.lines.get(at).copied() else {
                continue;
            };
            let element = match line {
                Line::Section(section) => div()
                    .h(px(LINE_HEIGHT))
                    .px(theme.spacing.small)
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .text_color(theme.colors.primary)
                    .child(icon(&theme, section.glyph()))
                    .child(language.say(section.title()))
                    .into_any_element(),
                Line::Result(index) => {
                    let Some(entry) = self.search.entries.get(index) else {
                        continue;
                    };
                    let chosen = self.search.chosen == Some(at);
                    let ground = if chosen {
                        theme.colors.secondary_container
                    } else {
                        theme.colors.surface_container_highest
                    };
                    let hover = mix(ground, theme.colors.on_surface, crate::ui::HOVER_LAYER);
                    let card = { entry.card.clone() }.map(|card| {
                        note(&theme, card)
                            .flex_none()
                            .text_size(theme.typography.label_size * super::CAPTION_SHARE)
                    });
                    div()
                        .id(("search-result", at))
                        .h(px(LINE_HEIGHT))
                        .pl(theme.spacing.medium + theme.spacing.small)
                        .pr(theme.spacing.small)
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(theme.spacing.small)
                        .rounded(theme.shape.medium)
                        .bg(ground)
                        .hover(move |style| style.bg(hover))
                        .cursor_pointer()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(theme.colors.on_surface)
                                .child(SharedString::from(entry.title.clone())),
                        )
                        .children(card)
                        .on_click(cx.listener(move |this, _, _, cx| this.go_to_result(at, cx)))
                        .into_any_element()
                }
            };
            lines.push(element);
        }
        lines
    }

    /// The results, as tall as the room they are given, or a note that
    /// there are none.
    pub(super) fn search_results(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        if self.search.lines.is_empty() {
            let language = Language::of(cx);
            return note(&theme, language.say("Nothing matches")).into_any_element();
        }
        uniform_list(
            "settings-search",
            self.search.lines.len(),
            cx.processor(|this, range: Range<usize>, _, cx| this.result_lines(range, cx)),
        )
        .track_scroll(self.search.scroll.clone())
        .flex_1()
        .min_h(px(0.))
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(said: &[(&str, Weight)]) -> Vec<(String, Weight)> {
        said.iter()
            .map(|(text, weight)| ((*text).to_owned(), *weight))
            .collect()
    }

    #[test]
    fn a_whole_word_beats_the_start_of_one_which_beats_any_place() {
        let words = texts(&[("Gap between tiles", Weight::Label)]);
        let start = texts(&[("Gaps", Weight::Label)]);
        let within = texts(&[("Megapixels", Weight::Label)]);
        let [words, start, within] = [&words, &start, &within].map(|said| rank("gap", said));
        assert!(
            words < start && start < within,
            "{words:?} {start:?} {within:?}"
        );
        assert_eq!(rank("gap", &texts(&[("Columns", Weight::Label)])), None);
        assert_eq!(rank("  ", &words_of("anything")), None);
    }

    fn words_of(text: &str) -> Vec<(String, Weight)> {
        vec![(text.to_owned(), Weight::Label)]
    }

    #[test]
    fn a_label_beats_a_description_of_the_same_find() {
        let label = texts(&[("Dark hours", Weight::Label)]);
        let description = texts(&[("When it is dark", Weight::Description)]);
        assert!(rank("dark", &label) < rank("dark", &description));
        // Each kind of find still comes first.
        let described = texts(&[("dark", Weight::Description)]);
        let started = texts(&[("darkness", Weight::Label)]);
        assert!(rank("dark", &described) < rank("dark", &started));
    }

    #[test]
    fn search_is_without_regard_to_case_and_finds_phrases() {
        let said = texts(&[("Keep text readable (WCAG 2.1 AA)", Weight::Label)]);
        assert_eq!(rank("WCAG", &said), rank("wcag", &said));
        assert!(rank("text readable", &said).is_some());
        assert_eq!(rank("readable text", &said), None);
        let russian = texts(&[("Ночной свет", Weight::Label)]);
        assert!(rank("ночной", &russian).is_some());
    }

    #[test]
    fn results_are_grouped_by_section_the_best_section_first() {
        let entry = |section, title: &str| Entry {
            section,
            item: title.to_owned(),
            title: title.to_owned(),
            card: None,
            texts: words_of(title),
        };
        let entries = vec![
            entry(Section::General, "Megagap"),
            entry(Section::Windows, "Gap"),
            entry(Section::General, "Gap of cards"),
            entry(Section::Files, "Nothing"),
        ];
        let lines = lines_of("gap", &entries);
        assert_eq!(
            lines,
            [
                Line::Section(Section::General),
                Line::Result(2),
                Line::Result(0),
                Line::Section(Section::Windows),
                Line::Result(1),
            ]
        );
    }
}
