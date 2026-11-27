// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The rows of Tessera's own settings: the cards of every rows file of
//! [`super::rows`], drawn by the controls that draw what extensions and
//! themes declare, so that one renderer draws every setting. A card names
//! the table its rows write keys of, or a widget whose option they set;
//! a card may also hold rows that run an action, and pages, which are
//! widgets drawn inside the settings.

use std::collections::HashMap;

use gpui::{AnyElement, App, Context, IntoElement, ParentElement, Styled, Window, div, px};

use super::controls::{CardParts, Owner, RowKey};
use super::dynamic;
use super::rows::{LinkRow, RowCard, cards};
use super::{Card, Control, Face, ICON_FORWARD, Section, Settings, note};
use crate::actions::Actions;
use crate::config::edit::ConfigFile;
use crate::config::{Config, WidgetSpec};
use crate::i18n::Language;
use crate::module::WidgetRegistry;
use crate::options::OptionDecl;
use crate::theme::Theme;

/// Where a row of a rows file writes its value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum RowTarget {
    /// A key at the top of the file, its path apart by dots.
    Key(String),
    /// A key of a table, its path below the table apart by dots.
    Table(String, String),
    /// An option of every widget of a name.
    Widget(String, String),
}

impl RowTarget {
    /// Where the row `key` of `card` writes.
    pub(super) fn of(card: &RowCard, key: &str) -> Self {
        match (&card.table, &card.widget) {
            (_, Some(widget)) => Self::Widget(widget.clone(), key.to_owned()),
            (Some(table), None) => Self::Table(table.clone(), key.to_owned()),
            (None, None) => Self::Key(key.to_owned()),
        }
    }

    /// The parts of the path of a key in the file, its table first; none
    /// for an option of widgets.
    pub(super) fn path(&self) -> Option<Vec<&str>> {
        match self {
            Self::Key(path) => Some(path.split('.').collect()),
            Self::Table(table, path) => Some(
                std::iter::once(table.as_str())
                    .chain(path.split('.'))
                    .collect(),
            ),
            Self::Widget(..) => None,
        }
    }

    /// The widget whose option it is.
    pub(super) fn widget(&self) -> Option<&str> {
        match self {
            Self::Widget(widget, _) => Some(widget),
            Self::Key(_) | Self::Table(..) => None,
        }
    }

    /// What names it: its path, as `tiling.gap`, or the widget and its
    /// option, as `clock.format`.
    pub(super) fn item(&self) -> String {
        match self {
            Self::Key(path) => path.clone(),
            Self::Table(table, path) => format!("{table}.{path}"),
            Self::Widget(widget, option) => format!("{widget}.{option}"),
        }
    }

    /// What `table`, the file as a table, holds at it; for an option, what
    /// the first widget of the name has.
    pub(super) fn written(
        &self,
        table: Option<&toml::Table>,
        config: &Config,
    ) -> Option<toml::Value> {
        match self {
            Self::Widget(widget, option) => config.option(widget, option).cloned(),
            _ => {
                let path = self.path()?;
                let (last, tables) = path.split_last()?;
                let mut at = table?;
                for part in tables {
                    at = at.get(*part)?.as_table()?;
                }
                at.get(*last).cloned()
            }
        }
    }

    /// Writes `value` at it; `None` takes the key away. Whether it could
    /// be written is returned.
    pub(super) fn write(&self, file: &mut ConfigFile, value: Option<toml_edit::Value>) -> bool {
        match self {
            Self::Widget(widget, option) => file.set_option(widget, option, value),
            _ => self.path().is_some_and(|path| file.set_at(&path, value)),
        }
    }
}

/// The cards of the rows files with one title in one section, drawn as
/// one card.
#[derive(Clone, Debug)]
pub(super) struct RowGroup {
    pub section: Section,
    /// Its English title.
    pub title: String,
    /// Its rows, each with where it writes, in the order of the files.
    pub rows: Vec<(RowTarget, OptionDecl)>,
    pub links: Vec<LinkRow>,
    /// The widgets drawn in it as pages of the settings.
    pub pages: Vec<String>,
    /// More English words search finds it by.
    pub search: Vec<String>,
}

/// The rows of Tessera's own settings, as the rows files declare them,
/// with the choices only the running shell knows.
#[derive(Clone, Debug, Default)]
pub(super) struct BuiltinRows {
    groups: Vec<RowGroup>,
    decls: HashMap<RowTarget, OptionDecl>,
}

impl BuiltinRows {
    /// Reads the cards of every rows file, the choices of their rows made
    /// from what the shell knows now.
    pub(super) fn read(cx: &App) -> Self {
        let mut this = Self::of_cards(cards());
        for group in &mut this.groups {
            for (target, decl) in &mut group.rows {
                let table = match target {
                    RowTarget::Key(_) => "",
                    RowTarget::Table(table, _) => table.as_str(),
                    RowTarget::Widget(widget, _) => widget.as_str(),
                };
                if let Some(choices) = dynamic::choices_for(table, &decl.key, cx) {
                    dynamic::merge(decl, choices);
                }
            }
        }
        this.index();
        this
    }

    /// The cards, one for each title in a section, in the order they
    /// first appear; a card in no section there is is left out.
    fn of_cards(cards: Vec<RowCard>) -> Self {
        let mut groups: Vec<RowGroup> = Vec::new();
        for card in cards {
            let Some(section) = Section::named(&card.section) else {
                report!(
                    "the card `{}` of the settings stands in `{}`, which is no section",
                    card.title,
                    card.section
                );
                continue;
            };
            let at = { groups.iter() }
                .position(|group| group.section == section && group.title == card.title);
            let at = at.unwrap_or_else(|| {
                groups.push(RowGroup {
                    section,
                    title: card.title.clone(),
                    rows: Vec::new(),
                    links: Vec::new(),
                    pages: Vec::new(),
                    search: Vec::new(),
                });
                groups.len() - 1
            });
            let group = &mut groups[at];
            for row in &card.rows {
                group
                    .rows
                    .push((RowTarget::of(&card, &row.key), row.clone()));
            }
            group.links.extend(card.links.iter().cloned());
            group.pages.extend(card.page.iter().cloned());
            group.search.extend(card.search.iter().cloned());
        }
        let mut this = Self {
            groups,
            decls: HashMap::new(),
        };
        this.index();
        this
    }

    fn index(&mut self) {
        self.decls = { self.groups.iter() }
            .flat_map(|group| group.rows.iter().cloned())
            .collect();
    }

    /// The cards of `section`, in order.
    pub(super) fn of(&self, section: Section) -> impl Iterator<Item = &RowGroup> {
        { self.groups.iter() }.filter(move |group| group.section == section)
    }

    /// Every card, in order.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "searched by the settings search")
    )]
    pub(super) fn groups(&self) -> &[RowGroup] {
        &self.groups
    }

    /// The declaration of the row that writes at `target`.
    pub(super) fn declaration(&self, target: &RowTarget) -> Option<&OptionDecl> {
        self.decls.get(target)
    }
}

/// The page `widget`, built for `item`.
fn build_page(widget: &str, item: Option<&str>, cx: &mut App) -> Option<gpui::AnyView> {
    let mut options = toml::Table::new();
    if let Some(item) = item {
        options.insert("item".to_owned(), item.into());
    }
    let spec = WidgetSpec {
        widget: widget.to_owned(),
        popup: None,
        options,
    };
    WidgetRegistry::build(&spec, None, cx)
}

impl Settings {
    /// Whether `link` can be run: whether its action is registered.
    fn can_run(link: &LinkRow, cx: &App) -> bool {
        cx.try_global::<Actions>()
            .is_some_and(|actions| actions.get(&link.action).is_some())
    }

    /// Whether a card of `group` is drawn: it has rows, a link that can
    /// be run, or a page that is registered.
    fn drawn(&self, group: &RowGroup, cx: &App) -> bool {
        !group.rows.is_empty()
            || group.links.iter().any(|link| Self::can_run(link, cx))
            || group.pages.iter().any(|page| self.available.contains(page))
    }

    /// The cards of the rows files in the section shown: its pages, as
    /// wide as the page, and its cards of rows and links, which share it.
    pub(super) fn builtin_cards(
        &mut self,
        config: &Config,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Vec<AnyElement>, Vec<Card>) {
        let language = Language::of(cx);
        let groups: Vec<RowGroup> = self.declared.builtin.of(self.section).cloned().collect();
        let mut pages = Vec::new();
        let mut shared = Vec::new();
        for group in &groups {
            let registered = { group.pages.iter() }.filter(|page| self.available.contains(page));
            for page in registered.cloned().collect::<Vec<_>>() {
                pages.push(self.page(&page, cx));
            }
            let missing = |target: &RowTarget| {
                target
                    .widget()
                    .is_some_and(|widget| config.widget(widget).is_none())
            };
            let rows: Vec<(RowKey, OptionDecl)> = { group.rows.iter() }
                .filter(|(target, _)| !missing(target))
                .map(|(target, decl)| {
                    let owner = Owner::Builtin(target.clone());
                    (RowKey::new(owner, &decl.key), decl.clone())
                })
                .collect();
            let lacking = group.rows.iter().any(|(target, _)| missing(target));
            let links: Vec<LinkRow> = { group.links.iter() }
                .filter(|link| Self::can_run(link, cx))
                .cloned()
                .collect();
            if rows.is_empty() && links.is_empty() && !lacking {
                continue;
            }
            let mut footer = Vec::new();
            for link in &links {
                footer.push(self.link_row(link, window, cx));
            }
            let weight = 1 + rows.len() + links.len() + usize::from(lacking);
            let item = group.title.clone();
            if self.declared.seek.as_deref().is_some_and(|sought| {
                sought == item || rows.iter().any(|(row, _)| row.owner.item() == sought)
            }) {
                self.declared.sought_shared = true;
            }
            let parts = CardParts {
                item,
                title: language.translate(&group.title).into(),
                rows,
                problems: &[],
                remark: lacking.then_some("Not in the layout"),
                footer,
                affects: &|_| Vec::new(),
            };
            shared.push((weight, self.declared_card(parts, window, cx)));
        }
        (pages, shared)
    }

    /// A row that runs an action: its label and a button, what it does
    /// under them.
    fn link_row(
        &mut self,
        link: &LinkRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let control = Control::Link(link.action.clone(), link.arg.clone());
        let go = self.control(control, Face::Glyph(ICON_FORWARD), window, cx);
        let description = { link.description.as_ref() }
            .map(|description| note(&theme, description.text(language)));
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(
                div()
                    .min_h(super::CHIP_HEIGHT)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.spacing.small)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme.colors.on_surface)
                            .child(link.label.text(language)),
                    )
                    .child(go),
            )
            .children(description)
            .into_any_element()
    }

    /// The page `widget`, built once while its section is shown. A page
    /// that is also a popup of its own is given the height of that popup.
    fn page(&mut self, widget: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let view = match self.declared.pages.get(widget) {
            Some(view) => Some(view.clone()),
            None => {
                let item = self.declared.page_item.clone();
                let view = build_page(widget, item.as_deref(), cx);
                if let Some(view) = &view {
                    self.declared.pages.insert(widget.to_owned(), view.clone());
                }
                view
            }
        };
        let Some(view) = view else {
            return div().into_any_element();
        };
        let popup = cx
            .global::<WidgetRegistry>()
            .popup(widget)
            .map(|popup| popup.height);
        match popup {
            Some(height) => div()
                .h(px(height))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(theme.shape.large)
                .bg(theme.colors.surface_container_highest)
                .child(view)
                .into_any_element(),
            None => view.into_any_element(),
        }
    }

    /// Whether `section` has a card to draw: hand-drawn ones, or a card of
    /// the rows files that is drawn.
    pub(super) fn section_drawn(&self, section: Section, cx: &App) -> bool {
        self.hand_drawn(section)
            || self
                .declared
                .builtin
                .of(section)
                .any(|group| self.drawn(group, cx))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::super::rows::{ROW_FILES, RowFile, SECTIONS};
    use super::*;
    use crate::options::{Kind, Label};

    /// Every rows file, by the package that fills it.
    fn files() -> Vec<(&'static str, RowFile)> {
        { ROW_FILES.iter() }
            .map(|(package, text)| {
                let file = toml::from_str(text)
                    .unwrap_or_else(|error| panic!("the rows of {package} do not parse: {error}"));
                (*package, file)
            })
            .collect()
    }

    /// A value of the kind of `decl` that the configuration takes: its
    /// default, or else the first choice, the least, false, an empty text,
    /// midnight; none for a choice that offers only to unset.
    fn sample(decl: &OptionDecl) -> Option<toml::Value> {
        if let Some(default) = &decl.default {
            return Some(default.clone());
        }
        let offered = { decl.choices.iter() }.find(|choice| !choice.is_unset());
        let least = decl.number_value(decl.fit(decl.min.unwrap_or(0.)));
        Some(match decl.kind {
            Kind::Toggle => false.into(),
            Kind::Text | Kind::Path | Kind::Font => "".into(),
            Kind::Number | Kind::Slider | Kind::Sides => least,
            Kind::Choice => offered?.value.clone(),
            Kind::Color => "#000000".into(),
            Kind::Time => "00:00".into(),
            Kind::Pairs => toml::Value::Table(toml::Table::new()),
            Kind::List => {
                let count = decl.min.unwrap_or(0.) as usize;
                let items: Vec<toml::Value> = match decl.choices.is_empty() {
                    true => vec!["".into(); count],
                    false => { decl.choices.iter() }
                        .filter(|choice| !choice.is_unset())
                        .take(count)
                        .map(|choice| choice.value.clone())
                        .collect(),
                };
                toml::Value::Array(items)
            }
        })
    }

    /// A document that holds only `value` at `path`.
    fn holding(path: &[&str], value: toml::Value) -> String {
        let mut value = value;
        for part in path.iter().rev() {
            let mut table = toml::Table::new();
            table.insert((*part).to_owned(), value);
            value = toml::Value::Table(table);
        }
        match value {
            toml::Value::Table(table) => toml::to_string(&table).expect("it is written"),
            _ => String::new(),
        }
    }

    /// Every row of a table or a key at the top, with its package and
    /// where it writes.
    fn keyed_rows() -> Vec<(&'static str, RowTarget, OptionDecl)> {
        let mut rows = Vec::new();
        for (package, file) in files() {
            for card in &file.card {
                for row in &card.rows {
                    let target = RowTarget::of(card, &row.key);
                    if target.path().is_some() {
                        rows.push((package, target, row.clone()));
                    }
                }
            }
        }
        rows
    }

    #[test]
    fn every_row_names_a_key_the_configuration_reads() {
        for (package, target, decl) in keyed_rows() {
            let Some(value) = sample(&decl) else {
                continue;
            };
            let path = target.path().expect("only keys were kept");
            let text = holding(&path, value);
            if let Err(error) = toml::from_str::<Config>(&text) {
                panic!(
                    "{package}: the row `{}` writes what the configuration does not read:\n{text}\n{error}",
                    target.item()
                );
            }
        }
    }

    #[test]
    fn every_row_default_is_the_configuration_default() {
        let empty: Config = toml::from_str("").expect("an empty file is read");
        for (package, target, decl) in keyed_rows() {
            let (false, Some(default)) = (decl.optional, &decl.default) else {
                continue;
            };
            let path = target.path().expect("only keys were kept");
            let text = holding(&path, default.clone());
            let config: Config = toml::from_str(&text).unwrap_or_else(|error| {
                panic!(
                    "{package}: `{}` = its default is not read: {error}",
                    target.item()
                )
            });
            assert!(
                config == empty,
                "{package}: the default of `{}` is not what the file without it gives",
                target.item()
            );
        }
    }

    #[test]
    fn every_row_text_is_translated() {
        let file: toml::Table =
            toml::from_str(include_str!("../../../../../languages/de.toml")).unwrap();
        let known: HashSet<&str> = file["texts"]
            .as_table()
            .expect("the language file has texts")
            .keys()
            .map(String::as_str)
            .collect();
        let plain = |label: Option<&Label>| match label {
            Some(Label::Plain(text)) => Some(text.clone()),
            _ => None,
        };
        for (package, file) in files() {
            let mut texts: Vec<String> = Vec::new();
            for card in &file.card {
                texts.push(card.title.clone());
                texts.extend(card.search.iter().cloned());
                for row in &card.rows {
                    texts.extend(plain(Some(&row.label)));
                    texts.extend(plain(row.description.as_ref()));
                    texts.extend(plain(row.unset_label.as_ref()));
                    texts.extend(plain(row.section.as_ref()));
                    texts.extend(plain(row.placeholder.as_ref()));
                    for choice in &row.choices {
                        texts.extend(plain(choice.label.as_ref()));
                    }
                }
                for link in &card.links {
                    texts.extend(plain(Some(&link.label)));
                    texts.extend(plain(link.description.as_ref()));
                }
            }
            texts.extend(file.action.iter().map(|action| action.title.clone()));
            for text in texts {
                assert!(
                    known.contains(text.as_str()),
                    "{package}: `{text}` is no text of the RUSSIAN table"
                );
            }
        }
    }

    #[test]
    fn every_card_names_a_known_section() {
        for (package, file) in files() {
            for card in &file.card {
                assert!(
                    SECTIONS.contains(&card.section.as_str())
                        && Section::named(&card.section).is_some(),
                    "{package}: the card `{}` stands in `{}`, which is no section",
                    card.title,
                    card.section
                );
            }
        }
    }

    #[test]
    fn no_two_rows_write_one_key() {
        let mut written: HashMap<String, &str> = HashMap::new();
        for (package, file) in files() {
            for card in &file.card {
                for row in &card.rows {
                    let target = RowTarget::of(card, &row.key);
                    let at = match target.widget() {
                        Some(_) => format!("widget {}", target.item()),
                        None => target.item(),
                    };
                    if let Some(other) = written.insert(at.clone(), package) {
                        panic!("`{at}` has a row of {other} and one of {package}");
                    }
                }
            }
        }
    }

    #[test]
    fn cards_of_one_title_in_one_section_are_one_card() {
        let text = r#"
[[card]]
section = "Windows"
title = "Tiling"
table = "tiling"
[[card.row]]
key = "enabled"
kind = "toggle"
label = "Tile windows"

[[card]]
section = "Windows"
title = "Tiling"
widget = "tiling"
[[card.row]]
key = "show"
kind = "toggle"
label = "Shows"

[[card]]
section = "Nowhere"
title = "Lost"
"#;
        let file: RowFile = toml::from_str(text).unwrap();
        let rows = BuiltinRows::of_cards(file.card);
        let [tiling] = rows.groups() else {
            panic!("{} cards", rows.groups().len());
        };
        assert_eq!(tiling.rows.len(), 2);
        let enabled = RowTarget::Table("tiling".into(), "enabled".into());
        assert_eq!(enabled.item(), "tiling.enabled");
        assert_eq!(enabled.path(), Some(vec!["tiling", "enabled"]));
        assert!(rows.declaration(&enabled).is_some());
        let shows = RowTarget::Widget("tiling".into(), "show".into());
        assert_eq!(shows.path(), None);
        assert!(rows.declaration(&shows).is_some());
    }

    #[test]
    fn a_row_reads_what_the_file_holds_at_its_path() {
        let table: toml::Table =
            toml::from_str("contrast = false\n[tiling.border]\nwidth = 4\n").unwrap();
        let config: Config = toml::from_str(&toml::to_string(&table).unwrap()).unwrap();
        let width = RowTarget::Table("tiling".into(), "border.width".into());
        assert_eq!(width.written(Some(&table), &config), Some(4.into()));
        let contrast = RowTarget::Key("contrast".into());
        assert_eq!(contrast.written(Some(&table), &config), Some(false.into()));
        let gap = RowTarget::Table("tiling".into(), "gap".into());
        assert_eq!(gap.written(Some(&table), &config), None);
        assert_eq!(
            holding(&["night_light", "from"], "21:00".into()),
            "[night_light]\nfrom = \"21:00\"\n"
        );
    }
}
