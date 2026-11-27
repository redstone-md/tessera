// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The editor of one rule: the names it gives, with how they are matched
//! and the tables of `not`, `any` and `all` in them, and every key of what
//! it says, each with its control; then how many windows it names now,
//! why it cannot be used if it cannot, and the buttons that write it.

use gpui::{
    AnyElement, AppContext, Context, Div, Entity, FontWeight, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div,
};

use crate::field::TextField;
use crate::i18n::Language;
use crate::platform::{FocusLevel, WindowKind};
use crate::rules::Apply;
use crate::theme::Theme;
use crate::ui::one_line;

use super::detect::Suggestion;
use super::draft::{
    Branch, Control, Draft, MODES, NAME_KEYS, OPACITIES, SAYS, SIDES, Step, categories, share_value,
};
use super::parts::{card, chip, field_box, glyph_button, labelled, note, text_button, wrapping};
use super::{LISTED, RulesEditor, Slot};

/// How deep tables of names are offered in tables of names; deeper ones
/// are kept as they are written.
const DEEPEST: usize = 3;

const ICON_SAVE: &str = "\u{e74e}";
const ICON_CANCEL: &str = "\u{e711}";
const ICON_REMOVE: &str = "\u{e74d}";
const ICON_ADD: &str = "\u{e710}";
const ICON_TAKE: &str = "\u{e7c9}";
const ICON_LESS: &str = "\u{e738}";
const ICON_MORE: &str = "\u{e710}";
const ICON_UNTIL_CLOSED: &str = "\u{e823}";

/// What the words of `match` are called.
fn mode_label(mode: &str) -> &'static str {
    match mode {
        "glob" => "Pattern with * and ?",
        "equals" => "Equals",
        "starts" => "Starts with",
        "ends" => "Ends with",
        "contains" => "Contains",
        _ => "Regular expression",
    }
}

/// What the names of a rule are called.
fn name_label(key: &str) -> &'static str {
    match key {
        "exe" => "Program",
        "path" => "Path of the program",
        "app" => "Packaged application",
        "title" => "Title",
        _ => "Window class",
    }
}

/// What the kinds of window are called.
fn kind_label(kind: WindowKind) -> &'static str {
    match kind {
        WindowKind::Normal => "Normal window",
        WindowKind::Dialog => "Dialog",
        WindowKind::Utility => "Utility window",
        WindowKind::Tool => "Tool window",
        WindowKind::Splash => "Splash screen",
        WindowKind::Popup => "Popup",
    }
}

/// What the levels of focus stealing prevention are called.
fn focus_label(level: FocusLevel) -> &'static str {
    match level {
        FocusLevel::None => "None",
        FocusLevel::Low => "Low",
        FocusLevel::Medium => "Medium",
        FocusLevel::High => "High",
        FocusLevel::Extreme => "Extreme",
    }
}

/// What the policies are called.
fn apply_label(apply: Apply) -> &'static str {
    match apply {
        Apply::Initially => "When first seen",
        Apply::Always => "Always",
        Apply::Remember => "Remember where it was",
        Apply::Now => "Now, once",
    }
}

/// The element id of a control of the editor.
fn id_of(parts: std::fmt::Arguments<'_>) -> SharedString {
    SharedString::from(format!("rule-editor-{parts}"))
}

/// A path written out for element ids.
fn path_text(path: &[Step]) -> String {
    let steps: Vec<String> = path.iter().map(|step| format!("{step:?}")).collect();
    steps.join("-")
}

impl RulesEditor {
    /// The field of `slot`, made with what the draft holds the first time.
    fn field(
        &mut self,
        slot: Slot,
        placeholder: &'static str,
        cx: &mut Context<Self>,
    ) -> Entity<TextField> {
        if let Some(field) = self.fields.get(&slot) {
            return field.clone();
        }
        let shown = self.slot_text(&slot);
        let placeholder = SharedString::new_static(Language::of(cx).say(placeholder));
        let field = cx.new(|cx| TextField::new(placeholder, cx));
        field.update(cx, |field, cx| field.put_text(&shown, cx));
        let observed = slot.clone();
        cx.observe(&field, move |this: &mut Self, field, cx| {
            let text = field.read(cx).text();
            this.typed(&observed, &text, cx);
        })
        .detach();
        self.fields.insert(slot, field.clone());
        field
    }

    /// What the draft holds for `slot`, as a field shows it.
    fn slot_text(&self, slot: &Slot) -> String {
        let Some(draft) = &self.draft else {
            return String::new();
        };
        match slot {
            Slot::Name(path, key) => draft.text_at(path, key).unwrap_or_default().to_owned(),
            Slot::Text(key) => draft.text(key).unwrap_or_default().to_owned(),
            Slot::Pair(key, at) => draft
                .pair(key)
                .map(|pair| number_text(pair[*at]))
                .unwrap_or_default(),
        }
    }

    /// Shows in the fields of the pair `key` what the draft holds now.
    fn show_pair(&mut self, key: &'static str, cx: &mut Context<Self>) {
        self.pair_faults.remove(key);
        for at in 0..2 {
            let slot = Slot::Pair(key, at);
            let text = self.slot_text(&slot);
            if let Some(field) = self.fields.get(&slot) {
                field.update(cx, |field, cx| field.put_text(&text, cx));
            }
        }
    }

    /// `text` was typed into the field of `slot`.
    fn typed(&mut self, slot: &Slot, text: &str, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        let text = if text.trim().is_empty() { "" } else { text };
        let changed = match slot {
            Slot::Name(path, key) => draft.set_text_at(path, key, text),
            Slot::Text(key) => draft.set_text_at(&[], key, text.trim()),
            Slot::Pair(key, _) => {
                let texts: Vec<String> = { (0..2).map(|at| Slot::Pair(key, at)) }
                    .map(|slot| {
                        self.fields
                            .get(&slot)
                            .map(|field| field.read(cx).text().trim().to_owned())
                            .unwrap_or_default()
                    })
                    .collect();
                let whole = matches!(*key, "min_size" | "max_size");
                let value = pair_value(&texts[0], &texts[1], whole);
                match value {
                    Ok(value) => {
                        self.pair_faults.remove(key);
                        draft.set(key, value)
                    }
                    Err(()) => {
                        self.pair_faults.insert(key);
                        true
                    }
                }
            }
        };
        if changed {
            self.changed(cx);
        }
    }

    /// Changes the draft with `change`, and the tables of names with it,
    /// whose fields are made anew.
    fn restructure(&mut self, change: impl FnOnce(&mut Draft) -> bool, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        if change(draft) {
            self.fields
                .retain(|slot, _| !matches!(slot, Slot::Name(..)));
            self.changed(cx);
        }
    }

    /// Changes a key of what the draft says.
    fn say(&mut self, key: &'static str, value: Option<toml::Value>, cx: &mut Context<Self>) {
        if let Some(draft) = self.draft.as_mut()
            && draft.set(key, value)
        {
            if matches!(
                SAYS.iter().find(|(said, ..)| *said == key),
                Some((_, _, Control::Position | Control::Size | Control::Sides))
            ) {
                self.show_pair(key, cx);
            }
            if let Some(field) = self.fields.get(&Slot::Text(key)) {
                let text = self
                    .draft
                    .as_ref()
                    .and_then(|draft| draft.text(key))
                    .unwrap_or_default()
                    .to_owned();
                field.update(cx, |field, cx| field.put_text(&text, cx));
            }
            self.changed(cx);
        }
    }

    /// The editor of the rule being changed, if one is.
    pub(super) fn editor_card(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let draft = self.draft.clone()?;
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let title = match draft.index {
            Some(index) => format!("{} {}", language.say("Rule"), index + 1),
            None => language.say("New rule").to_owned(),
        };
        let enabled = draft.enabled();
        let mut body = card(&theme, title).child(
            wrapping(&theme)
                .child(
                    chip(&theme, "rule-editor-on", language.say("On"), enabled)
                        .on_click(cx.listener(|this, _, _, cx| this.switch_draft(true, cx))),
                )
                .child(
                    chip(&theme, "rule-editor-off", language.say("Off"), !enabled)
                        .on_click(cx.listener(|this, _, _, cx| this.switch_draft(false, cx))),
                ),
        );
        body = body
            .child(section_title(&theme, language.say("Names the windows")))
            .child(self.names_node(Vec::new(), &draft, cx));
        body = body.child(section_title(&theme, language.say("Does to them")));
        for (key, label, control) in SAYS {
            let row = self.said(key, control, &draft, cx);
            body = body.child(labelled(&theme, language.say(label), row));
        }
        body = body.child(self.outcome(cx));
        let fault = self.draft_fault.is_some();
        let mut buttons = wrapping(&theme);
        if !fault {
            buttons = buttons.child(
                text_button(
                    &theme,
                    "rule-editor-save",
                    ICON_SAVE,
                    language.say("Save"),
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            );
        }
        if self.detected.is_some() && !fault {
            buttons = buttons.child(
                text_button(
                    &theme,
                    "rule-editor-until-closed",
                    ICON_UNTIL_CLOSED,
                    language.say("Apply to this window until it closes"),
                    self.temporarily,
                )
                .on_click(cx.listener(|this, _, _, cx| this.give_until_closed(cx))),
            );
        }
        buttons = buttons.child(
            text_button(
                &theme,
                "rule-editor-cancel",
                ICON_CANCEL,
                language.say("Cancel"),
                false,
            )
            .on_click(cx.listener(|this, _, _, cx| this.drop_draft(cx))),
        );
        Some(body.child(buttons).into_any_element())
    }

    /// Switches the rule being changed on or off.
    fn switch_draft(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(draft) = self.draft.as_mut()
            && draft.set_enabled(on)
        {
            self.changed(cx);
        }
    }

    /// How many windows the rule names now and which, or why it cannot be
    /// used.
    fn outcome(&self, cx: &Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        if let Some(fault) = &self.draft_fault {
            return note(&theme, fault.clone()).text_color(theme.colors.primary);
        }
        let Some(named) = &self.named_now else {
            return note(&theme, language.say("Counting the windows it names…"));
        };
        let mut told = div()
            .flex()
            .flex_col()
            .child(div().font_weight(FontWeight::MEDIUM).child(format!(
                "{}: {}",
                language.say("Windows it names now"),
                named.len()
            )));
        for title in named.iter().take(LISTED) {
            told = told.child(note(&theme, one_line(title.clone())));
        }
        if named.len() > LISTED {
            told = told.child(note(&theme, "…"));
        }
        told
    }

    /// The editor of the table of names at `path`.
    fn names_node(&mut self, path: Vec<Step>, draft: &Draft, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let Some(node) = draft.node(&path) else {
            return div();
        };
        let here = path_text(&path);
        let mut body = div().flex().flex_col().gap(theme.spacing.tiny);
        // How the names of this table are matched.
        let mode = node.get("match").and_then(toml::Value::as_str);
        let mut modes = wrapping(&theme);
        if !path.is_empty() {
            let at = path.clone();
            modes = modes.child(
                chip(
                    &theme,
                    id_of(format_args!("mode-{here}-inherit")),
                    language.say("As around it"),
                    mode.is_none(),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.restructure(|draft| draft.set_at(&at, "match", None), cx);
                })),
            );
        }
        for word in MODES {
            let chosen =
                mode == Some(word) || (path.is_empty() && mode.is_none() && word == "glob");
            let at = path.clone();
            modes = modes.child(
                chip(
                    &theme,
                    id_of(format_args!("mode-{here}-{word}")),
                    language.say(mode_label(word)),
                    chosen,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let value = (!(at.is_empty() && word == "glob")).then(|| word.into());
                    this.restructure(|draft| draft.set_at(&at, "match", value), cx);
                })),
            );
        }
        body = body.child(labelled(&theme, language.say("Matched as"), modes));
        for key in NAME_KEYS {
            let field = self.field(Slot::Name(path.clone(), key), name_label(key), cx);
            body = body.child(labelled(
                &theme,
                language.say(name_label(key)),
                field_box(&theme, &field),
            ));
        }
        // The kind of window.
        let kind = node.get("type").and_then(toml::Value::as_str);
        let mut kinds = wrapping(&theme);
        let at = path.clone();
        kinds = kinds.child(
            chip(
                &theme,
                id_of(format_args!("type-{here}-any")),
                language.say("Any kind"),
                kind.is_none(),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.restructure(|draft| draft.set_at(&at, "type", None), cx);
            })),
        );
        for each in WindowKind::ALL {
            let word = each.word();
            let at = path.clone();
            kinds = kinds.child(
                chip(
                    &theme,
                    id_of(format_args!("type-{here}-{word}")),
                    language.say(kind_label(each)),
                    kind == Some(word),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.restructure(|draft| draft.set_at(&at, "type", Some(word.into())), cx);
                })),
            );
        }
        body = body.child(labelled(&theme, language.say("Kind of window"), kinds));
        if path.len() >= DEEPEST {
            let deeper = ["not", "any", "all"]
                .iter()
                .any(|key| node.contains_key(*key));
            if deeper {
                body = body.child(note(
                    &theme,
                    language.say("More tables of names, kept as written."),
                ));
            }
            return body;
        }
        // The tables of names in this one.
        let branches = [
            (Branch::Not, "Must not be", "Add what it must not be"),
            (Branch::Any, "One of", "Add a choice"),
            (Branch::All, "All of", "Add a group"),
        ];
        let mut adding = wrapping(&theme);
        for (branch, title, add) in branches {
            let members: Vec<Step> = match branch {
                Branch::Not => node
                    .contains_key("not")
                    .then_some(Step::Not)
                    .into_iter()
                    .collect(),
                Branch::Any => (0..list_len(node, "any")).map(Step::Any).collect(),
                Branch::All => (0..list_len(node, "all")).map(Step::All).collect(),
            };
            for step in members {
                let mut inner = path.clone();
                inner.push(step);
                let removed = inner.clone();
                let header = div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::MEDIUM)
                            .child(language.say(title)),
                    )
                    .child(
                        glyph_button(
                            &theme,
                            id_of(format_args!("remove-{}", path_text(&inner))),
                            ICON_REMOVE,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.restructure(|draft| draft.remove_branch(&removed), cx);
                        })),
                    );
                let nested = self.names_node(inner, draft, cx);
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.spacing.tiny)
                        .ml(theme.spacing.medium)
                        .p(theme.spacing.small)
                        .rounded(theme.shape.medium)
                        .border_1()
                        .border_color(theme.colors.outline)
                        .child(header)
                        .child(nested),
                );
            }
            if branch == Branch::Not && node.contains_key("not") {
                continue;
            }
            let at = path.clone();
            adding = adding.child(
                text_button(
                    &theme,
                    id_of(format_args!("add-{here}-{}", branch.key())),
                    ICON_ADD,
                    language.say(add),
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.restructure(|draft| draft.add_branch(&at, branch), cx);
                })),
            );
        }
        body.child(adding)
    }

    /// The control of the key `key` of what the rule says.
    fn said(
        &mut self,
        key: &'static str,
        control: Control,
        draft: &Draft,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let value = draft.table.get(key).cloned();
        let unset = chip(
            &theme,
            id_of(format_args!("{key}-unset")),
            language.say("Not set"),
            value.is_none(),
        )
        .on_click(cx.listener(move |this, _, _, cx| this.say(key, None, cx)));
        let mut row = wrapping(&theme).child(unset);
        let choose = |row: Div,
                      word: &str,
                      label: &'static str,
                      written: toml::Value,
                      chosen: bool,
                      cx: &mut Context<Self>| {
            row.child(
                chip(
                    &theme,
                    id_of(format_args!("{key}-{word}")),
                    language.say(label),
                    chosen,
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.say(key, Some(written.clone()), cx)),
                ),
            )
        };
        match control {
            Control::Flag => {
                let flag = draft.flag(key);
                row = choose(row, "on", "On", true.into(), flag == Some(true), cx);
                row = choose(row, "off", "Off", false.into(), flag == Some(false), cx);
            }
            Control::Workspace | Control::Display => {
                let count = match control {
                    Control::Workspace => self.workspaces,
                    _ => self.displays,
                };
                let now = draft.number(key).map(|number| number as usize);
                let last = count.max(now.unwrap_or(1)).max(1);
                for number in 1..=last {
                    let text = number.to_string();
                    row = row.child(
                        chip(
                            &theme,
                            id_of(format_args!("{key}-{number}")),
                            text,
                            now == Some(number),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.say(key, Some(toml::Value::Integer(number as i64)), cx);
                        })),
                    );
                }
                if control == Control::Workspace && self.workspaces == 0 {
                    row = row.child(note(
                        &theme,
                        language.say("Only for Tessera's own workspaces."),
                    ));
                }
            }
            Control::Opacity => {
                let now = draft.number(key);
                let shown = now.map_or_else(
                    || "–".to_owned(),
                    |share| format!("{} %", (share * 100.).round()),
                );
                row = row
                    .child(
                        glyph_button(&theme, id_of(format_args!("{key}-less")), ICON_LESS)
                            .on_click(cx.listener(move |this, _, _, cx| this.step(key, false, cx))),
                    )
                    .child(
                        div()
                            .min_w(super::parts::BUTTON * 2.)
                            .flex()
                            .justify_center()
                            .child(shown),
                    )
                    .child(
                        glyph_button(&theme, id_of(format_args!("{key}-more")), ICON_MORE)
                            .on_click(cx.listener(move |this, _, _, cx| this.step(key, true, cx))),
                    )
                    .child(note(
                        &theme,
                        format!("{} – {} %", OPACITIES.0 * 100., OPACITIES.1 * 100.),
                    ));
            }
            Control::Position => {
                let center = draft.text(key) == Some("center");
                let at = draft.pair(key).is_some();
                row = choose(row, "center", "Centered", "center".into(), center, cx);
                let corner = toml::Value::Array(vec![share_value(0.), share_value(0.)]);
                row = choose(row, "at", "At shares of the work area", corner, at, cx);
                if at {
                    row = self.pair_fields(row, key, ("Across", "Down"), cx);
                }
                row = self.taken_from_window(row, key, cx);
            }
            Control::Size => {
                let set = draft.pair(key).is_some();
                let half = toml::Value::Array(vec![share_value(0.5), share_value(0.5)]);
                row = choose(row, "set", "Shares of the work area", half, set, cx);
                if set {
                    row = self.pair_fields(row, key, ("Width", "Height"), cx);
                }
                row = self.taken_from_window(row, key, cx);
            }
            Control::Sides => {
                row = self.pair_fields(row, key, ("Width", "Height"), cx);
                row = row.child(note(&theme, format!("{} – {} px", SIDES.0, SIDES.1)));
            }
            Control::Category => {
                let field = self.field(Slot::Text(key), "Category", cx);
                row = row.child(field_box(&theme, &field));
                let tables: Vec<toml::Table> = { crate::rules::templates::names() }
                    .flat_map(crate::rules::templates::tables_of)
                    .chain(self.rules.iter().flatten().cloned())
                    .collect();
                let now = draft.text(key);
                for category in categories(&tables) {
                    let chosen = now == Some(category.as_str());
                    let word = category.clone();
                    row = row.child(
                        chip(
                            &theme,
                            id_of(format_args!("{key}-is-{category}")),
                            category,
                            chosen,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.say(key, Some(word.clone().into()), cx)
                        })),
                    );
                }
            }
            Control::Text => {
                let placeholder = match key {
                    "icon" => "A picture file",
                    _ => "An application identifier",
                };
                let field = self.field(Slot::Text(key), placeholder, cx);
                row = row.child(field_box(&theme, &field));
            }
            Control::Focus => {
                let now = draft.text(key);
                for level in FocusLevel::ALL {
                    let word = level.word();
                    row = choose(
                        row,
                        word,
                        focus_label(level),
                        word.into(),
                        now == Some(word),
                        cx,
                    );
                }
            }
            Control::Apply => {
                let now = draft
                    .text(key)
                    .map(|word| if word == "force" { "always" } else { word });
                for policy in Apply::ALL {
                    let word = policy.word();
                    row = choose(
                        row,
                        word,
                        apply_label(policy),
                        word.into(),
                        now == Some(word),
                        cx,
                    );
                }
            }
        }
        row
    }

    /// Steps the opacity `key` of the draft.
    fn step(&mut self, key: &'static str, up: bool, cx: &mut Context<Self>) {
        if let Some(draft) = self.draft.as_mut()
            && draft.step_opacity(key, up)
        {
            self.changed(cx);
        }
    }

    /// The two fields of the pair `key` after `row`, with their
    /// placeholders.
    fn pair_fields(
        &mut self,
        row: Div,
        key: &'static str,
        placeholders: (&'static str, &'static str),
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = *cx.global::<Theme>();
        let first = self.field(Slot::Pair(key, 0), placeholders.0, cx);
        let second = self.field(Slot::Pair(key, 1), placeholders.1, cx);
        row.child(
            div()
                .flex()
                .gap(theme.spacing.tiny)
                .min_w(super::parts::LABEL_WIDTH)
                .child(field_box(&theme, &first))
                .child(field_box(&theme, &second)),
        )
    }

    /// "Take from the window" after `row`, where a window was read whose
    /// place or size the pair `key` can take.
    fn taken_from_window(&mut self, row: Div, key: &'static str, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let Some(detected) = &self.detected else {
            return row;
        };
        let suggestion = match key {
            "position" => detected.position.map(Suggestion::Position),
            _ => detected.size.map(Suggestion::Size),
        };
        let Some(suggestion) = suggestion else {
            return row;
        };
        row.child(
            text_button(
                &theme,
                id_of(format_args!("{key}-take")),
                ICON_TAKE,
                Language::of(cx).say("Take from the window"),
                false,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(draft) = this.draft.as_mut() {
                    suggestion.take_into(draft);
                    this.show_pair(key, cx);
                    this.changed(cx);
                }
            })),
        )
    }
}

/// The title over a part of the editor.
fn section_title(theme: &Theme, text: &'static str) -> Div {
    div()
        .pt(theme.spacing.small)
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.on_surface_variant)
        .child(text)
}

/// How many tables the list `key` of `node` holds.
fn list_len(node: &toml::Table, key: &str) -> usize {
    node.get(key)
        .and_then(toml::Value::as_array)
        .map_or(0, Vec::len)
}

/// A number as a field shows it: without a fraction where it has none.
fn number_text(number: f64) -> String {
    match number.fract() == 0. {
        true => format!("{number:.0}"),
        false => format!("{number}"),
    }
}

/// What two fields of a pair hold as the value of the pair: none where both
/// are empty, two whole numbers within [`SIDES`] where `whole`, else two
/// shares from 0 to 1; `Err` where they are not.
fn pair_value(first: &str, second: &str, whole: bool) -> Result<Option<toml::Value>, ()> {
    if first.is_empty() && second.is_empty() {
        return Ok(None);
    }
    let values = match whole {
        true => {
            let side = |text: &str| {
                text.parse::<i64>()
                    .ok()
                    .filter(|side| (SIDES.0..=SIDES.1).contains(side))
                    .map(toml::Value::Integer)
            };
            [side(first), side(second)]
        }
        false => {
            let share = |text: &str| {
                text.replace(',', ".")
                    .parse::<f64>()
                    .ok()
                    .filter(|share| (0. ..=1.).contains(share))
                    .map(share_value)
            };
            [share(first), share(second)]
        }
    };
    match values {
        [Some(first), Some(second)] => Ok(Some(toml::Value::Array(vec![first, second]))),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_fields_make_a_pair_or_none() {
        assert_eq!(pair_value("", "", false), Ok(None));
        assert_eq!(
            pair_value("0.5", "0,25", false),
            Ok(Some(toml::Value::Array(vec![0.5.into(), 0.25.into()])))
        );
        assert_eq!(pair_value("0.5", "", false), Err(()));
        assert_eq!(pair_value("1.5", "0.5", false), Err(()));
        assert_eq!(
            pair_value("400", "300", true),
            Ok(Some(toml::Value::Array(vec![400.into(), 300.into()])))
        );
        assert_eq!(pair_value("0", "300", true), Err(()));
        assert_eq!(pair_value("4.5", "300", true), Err(()));
        assert_eq!(number_text(400.), "400");
        assert_eq!(number_text(0.25), "0.25");
    }

    #[test]
    fn every_control_has_its_words() {
        for mode in MODES {
            assert!(!mode_label(mode).is_empty());
        }
        assert_ne!(mode_label("glob"), mode_label("regex"));
        for key in NAME_KEYS {
            assert!(!name_label(key).is_empty());
        }
        assert_eq!(path_text(&[Step::Not, Step::Any(2)]), "Not-Any(2)");
    }
}
