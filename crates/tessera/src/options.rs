// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Declared settings: what an extension, a theme or Tessera itself says
//! can be set, in `[[setting]]` entries of TOML, so that the settings can
//! show a control for each and check what the configuration holds.
//!
//! One declaration names a key, the kind of control that sets it, what
//! the control is called, and what the value may be. [`OptionDecl::validate`]
//! checks a value against it; [`OptionSet::resolve`] fills in the
//! defaults of a table of values and drops the values that do not fit.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::i18n::Language;

/// The choice value that stands for no value: the key leaves the file.
pub const UNSET: &str = "unset";

/// How many choices are shown side by side as buttons; more are offered
/// in a list that opens.
pub const SEGMENTS: usize = 4;

/// What is shown for a setting: one text, or a text per language by its
/// code (`en`, `ru`, `de` and so on), English being the one used where
/// the language in use has none.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Label {
    Plain(String),
    Translated(BTreeMap<String, String>),
}

impl Label {
    /// The label in `language`. A plain one is looked up in Tessera's own
    /// table, so that the settings Tessera declares speak every language
    /// it does.
    pub fn text(&self, language: Language) -> String {
        match self {
            Self::Plain(text) => language.translate(text),
            Self::Translated(texts) => texts
                .get(language.code())
                .or_else(|| texts.get("en"))
                .or_else(|| texts.values().next())
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// The label as English, which is what search and messages use.
    pub fn english(&self) -> String {
        self.text(Language::English)
    }
}

/// The control a setting is set with, and so what its value is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// On or off: `true` or `false`.
    Toggle,
    /// A line of text.
    Text,
    /// A number typed, or changed a step at a time.
    Number,
    /// A number on a track from `min` to `max`.
    Slider,
    /// One of `choices`.
    Choice,
    /// `#rrggbb` or `#rrggbbaa`.
    Color,
    /// A file or a folder.
    Path,
    /// The name of an installed font family.
    Font,
    /// Texts in order.
    List,
    /// Names, each with a value.
    Pairs,
}

/// Whether a path names a file or a folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PathPick {
    #[default]
    File,
    Folder,
}

/// One answer of a setting of the kind `choice`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    /// What is written; the word `unset` takes the key out instead.
    pub value: toml::Value,
    /// What its button says; the value where it is missing.
    pub label: Option<Label>,
    /// Values of a theme's tokens that come with this choice, written as
    /// in a theme file, such as `{ seed = "#3d7bd9" }`. Only themes have
    /// them.
    pub overlay: Option<toml::Table>,
}

impl Choice {
    /// Whether choosing it takes the key out of the file.
    pub fn is_unset(&self) -> bool {
        self.value.as_str() == Some(UNSET)
    }

    /// What its button says in `language`.
    pub fn label(&self, language: Language) -> String {
        match &self.label {
            Some(label) => label.text(language),
            None => text_of(&self.value),
        }
    }
}

/// When a setting is shown: while another one has a value.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub key: String,
    pub equals: toml::Value,
}

/// A setting, as an extension, a theme or Tessera declares it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionDecl {
    /// The key it is written under.
    pub key: String,
    pub kind: Kind,
    pub label: Label,
    /// A sentence on what it does.
    pub description: Option<Label>,
    /// The group it is shown in; settings of one group follow each other.
    pub section: Option<Label>,
    /// What it is while the configuration does not say.
    pub default: Option<toml::Value>,
    /// Whether it may have no value at all, which `unset_label` names.
    #[serde(default)]
    pub optional: bool,
    pub unset_label: Option<Label>,
    pub visible_if: Option<Condition>,
    /// What it changes: widgets of an extension by name, or tokens of a
    /// theme by path, such as `colors.primary`.
    #[serde(default)]
    pub affects: Vec<String>,
    /// Tokens of a theme that the value multiplies, for a number or a
    /// slider: `["shape.small", "shape.medium", "shape.large"]`.
    #[serde(default)]
    pub scale: Vec<String>,
    /// What an empty text field says.
    pub placeholder: Option<Label>,
    /// The most letters a text may have.
    pub max_length: Option<usize>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// What a number is rounded to, counted from `min`.
    pub step: Option<f64>,
    /// What a number counts, shown after it: `min`, `%`, `px`.
    pub unit: Option<String>,
    #[serde(default)]
    pub choices: Vec<Choice>,
    /// Whether a path is a file or a folder.
    pub pick: Option<PathPick>,
}

/// A value as text: a text as it is, a list as its items apart by
/// commas, anything else as TOML writes it.
pub fn text_of(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        toml::Value::Array(items) => {
            let items: Vec<String> = items.iter().map(text_of).collect();
            items.join(", ")
        }
        other => other.to_string(),
    }
}

/// Whether two values are the same, a whole number being the same as the
/// same number with a fraction of zero.
pub fn same(one: &toml::Value, other: &toml::Value) -> bool {
    let number = |value: &toml::Value| match value {
        toml::Value::Integer(number) => Some(*number as f64),
        toml::Value::Float(number) => Some(*number),
        _ => None,
    };
    match (number(one), number(other)) {
        (Some(one), Some(other)) => one == other,
        _ => one == other,
    }
}

/// The number a value of the file stands for; a text that holds one,
/// as hand-written settings sometimes are, counts.
fn number_of(value: &toml::Value) -> Option<f64> {
    match value {
        toml::Value::Integer(number) => Some(*number as f64),
        toml::Value::Float(number) => Some(*number),
        toml::Value::String(text) => text.trim().trim_matches('"').parse().ok(),
        _ => None,
    }
}

/// The items of a list as an extension reads them, `a, b; c`: trimmed,
/// without blank ones. The same rule as `parse_list` of the extension kit.
pub fn parse_list(text: &str) -> Vec<String> {
    text.split([',', ';', '\n'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The entries of `Tokyo=+9; London=+0`, split at the first `=`, apart by
/// `;` or a line break: the same rule as `parse_pairs` of the extension
/// kit. An entry without a name or a value is named in the error.
pub fn parse_pairs(text: &str) -> Result<Vec<(String, String)>, String> {
    text.split([';', '\n'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (name, value) = entry
                .split_once('=')
                .map(|(name, value)| (name.trim(), value.trim()))
                .filter(|(name, value)| !name.is_empty() && !value.is_empty())
                .ok_or_else(|| format!("`{entry}` is not written as name=value"))?;
            Ok((name.to_owned(), value.to_owned()))
        })
        .collect()
}

/// Pairs as an extension reads them.
pub fn write_pairs(pairs: &[(String, String)]) -> String {
    let entries: Vec<String> = { pairs.iter() }
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    entries.join("; ")
}

/// Whether `text` is a color the settings can write: `#rrggbb` or
/// `#rrggbbaa`.
pub fn is_color(text: &str) -> bool {
    let digits = text.strip_prefix('#').unwrap_or_default();
    matches!(digits.len(), 6 | 8) && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl OptionDecl {
    /// An optional number from `min` to `max` in steps of `step` that
    /// starts at `start`, for a row of Tessera's own settings to step as a
    /// declared number steps.
    pub fn counter(min: f64, max: f64, step: f64, start: f64) -> Self {
        Self {
            key: String::new(),
            kind: Kind::Number,
            label: Label::Plain(String::new()),
            description: None,
            section: None,
            default: Some(toml::Value::Float(start)),
            optional: true,
            unset_label: None,
            visible_if: None,
            affects: Vec::new(),
            scale: Vec::new(),
            placeholder: None,
            max_length: None,
            min: Some(min),
            max: Some(max),
            step: Some(step),
            unit: None,
            choices: Vec::new(),
            pick: None,
        }
    }

    /// Whether the declaration itself is usable: what is wrong with it, as
    /// the end of a sentence that starts with the setting's key.
    pub fn check(&self) -> Result<(), String> {
        let plain = |byte: u8| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte);
        if self.key.is_empty() || !self.key.bytes().all(plain) {
            return Err(format!(
                "`{}` is not a key made of letters, digits, `_`, `-` and `.`",
                self.key
            ));
        }
        let choice = self.kind == Kind::Choice;
        if choice && self.choices.is_empty() {
            return Err("is a choice without choices".to_owned());
        }
        if !choice && !self.choices.is_empty() {
            return Err("has choices but is no choice".to_owned());
        }
        for (index, one) in self.choices.iter().enumerate() {
            if !matches!(
                one.value,
                toml::Value::String(_)
                    | toml::Value::Integer(_)
                    | toml::Value::Float(_)
                    | toml::Value::Boolean(_)
            ) {
                return Err(format!(
                    "has a choice that is no text, number or flag: {}",
                    one.value
                ));
            }
            let again = self.choices[..index]
                .iter()
                .any(|other| same(&other.value, &one.value));
            if again {
                return Err(format!("offers `{}` twice", text_of(&one.value)));
            }
        }
        if matches!(self.kind, Kind::Number | Kind::Slider) {
            let finite = |bound: Option<f64>| bound.is_none_or(f64::is_finite);
            if !finite(self.min) || !finite(self.max) || !finite(self.step) {
                return Err("has a bound or a step that is no number".to_owned());
            }
            if let (Some(min), Some(max)) = (self.min, self.max)
                && min > max
            {
                return Err(format!("goes from {min} down to {max}"));
            }
            if self.step.is_some_and(|step| step <= 0.) {
                return Err("has a step that is not above 0".to_owned());
            }
            if self.kind == Kind::Slider && (self.min.is_none() || self.max.is_none()) {
                return Err("is a slider without `min` and `max`".to_owned());
            }
        }
        if !self.scale.is_empty() && !matches!(self.kind, Kind::Number | Kind::Slider) {
            return Err("scales tokens but is no number".to_owned());
        }
        if let Some(default) = &self.default {
            self.validate(default)
                .map_err(|error| format!("has a default that {error}"))?;
        }
        Ok(())
    }

    /// Whether a number is written as a whole number: when its step and
    /// both its bounds are whole.
    pub fn whole(&self) -> bool {
        let whole = |bound: Option<f64>| bound.is_none_or(|bound| bound.fract() == 0.);
        whole(self.min) && whole(self.max) && self.step().fract() == 0.
    }

    /// What a number is rounded to: 1 where the declaration does not say.
    pub fn step(&self) -> f64 {
        self.step.unwrap_or(1.)
    }

    /// `number` moved into the range and rounded to the step.
    pub fn fit(&self, number: f64) -> f64 {
        let step = self.step();
        let origin = self.min.unwrap_or(0.);
        let mut number = origin + ((number - origin) / step).round() * step;
        if let Some(min) = self.min {
            number = number.max(min);
        }
        if let Some(max) = self.max {
            number = number.min(max);
        }
        // What rounding to a step such as 0.1 leaves over.
        (number * 1e9).round() / 1e9
    }

    /// `number` as the file writes it.
    pub fn number_value(&self, number: f64) -> toml::Value {
        if self.whole() {
            toml::Value::Integer(number.round() as i64)
        } else {
            toml::Value::Float(number)
        }
    }

    /// `value` as this setting holds it: of its type, within its range and
    /// on its step, and one of its choices. What cannot be made so is
    /// said, as the end of a sentence that starts with the value.
    pub fn validate(&self, value: &toml::Value) -> Result<toml::Value, String> {
        let shown = value.to_string();
        match self.kind {
            Kind::Toggle => match value {
                toml::Value::Boolean(flag) => Ok((*flag).into()),
                toml::Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
                    "true" | "yes" | "on" | "1" => Ok(true.into()),
                    "false" | "no" | "off" | "0" => Ok(false.into()),
                    _ => Err(format!("{shown} is neither true nor false")),
                },
                _ => Err(format!("{shown} is neither true nor false")),
            },
            Kind::Text | Kind::Path | Kind::Font => {
                let text = match value {
                    toml::Value::String(text) => text.clone(),
                    toml::Value::Integer(_) | toml::Value::Float(_) => value.to_string(),
                    _ => return Err(format!("{shown} is no text")),
                };
                if self.kind != Kind::Text && text.trim().is_empty() {
                    return Err("is empty".to_owned());
                }
                if let Some(most) = self.max_length
                    && text.chars().count() > most
                {
                    return Err(format!("{shown} is longer than {most} letters"));
                }
                if text.contains(['\n', '\r']) {
                    return Err(format!("{shown} is more than one line"));
                }
                Ok(match self.kind {
                    Kind::Text => text,
                    _ => text.trim().to_owned(),
                }
                .into())
            }
            Kind::Number | Kind::Slider => {
                let number = number_of(value)
                    .filter(|number| number.is_finite())
                    .ok_or_else(|| format!("{shown} is no number"))?;
                Ok(self.number_value(self.fit(number)))
            }
            Kind::Choice => {
                let found = { self.choices.iter() }
                    .filter(|choice| !choice.is_unset())
                    .find(|choice| same(&choice.value, value));
                match found {
                    Some(choice) => Ok(choice.value.clone()),
                    None => {
                        let offered: Vec<String> = { self.choices.iter() }
                            .filter(|choice| !choice.is_unset())
                            .map(|choice| text_of(&choice.value))
                            .collect();
                        Err(format!("{shown} is not one of {}", offered.join(", ")))
                    }
                }
            }
            Kind::Color => match value.as_str() {
                Some(text) if is_color(text.trim()) => Ok(text.trim().to_ascii_lowercase().into()),
                _ => Err(format!(
                    "{shown} is no color of the form #rrggbb or #rrggbbaa"
                )),
            },
            Kind::List => {
                let items: Vec<String> = match value {
                    toml::Value::String(text) => parse_list(text),
                    toml::Value::Array(items) => {
                        let mut texts = Vec::new();
                        for item in items {
                            match item.as_str() {
                                Some(text) if !text.trim().is_empty() => {
                                    texts.push(text.trim().to_owned());
                                }
                                Some(_) => {}
                                None => {
                                    return Err(format!("{shown} holds {item}, which is no text"));
                                }
                            }
                        }
                        texts
                    }
                    _ => return Err(format!("{shown} is no list")),
                };
                // An item with a separator in it would come apart.
                if let Some(item) = items.iter().find(|item| item.contains([',', ';'])) {
                    return Err(format!("{shown} has `{item}`, which holds a `,` or a `;`"));
                }
                Ok(toml::Value::Array(
                    items.into_iter().map(Into::into).collect(),
                ))
            }
            Kind::Pairs => {
                let pairs = match value {
                    toml::Value::String(text) => parse_pairs(text)?,
                    toml::Value::Table(table) => { table.iter() }
                        .map(|(name, value)| (name.clone(), text_of(value)))
                        .collect(),
                    _ => return Err(format!("{shown} is not written as name=value; name=value")),
                };
                check_pairs(&pairs)?;
                Ok(write_pairs(&pairs).into())
            }
        }
    }

    /// The number one step up from `current`, or down. An optional number
    /// leaves the file one step under its least, where `None` stands for
    /// that; a step up from there gives the default, or the least.
    pub fn stepped(&self, current: Option<f64>, up: bool) -> Option<f64> {
        let step = self.step();
        let start = { self.default.as_ref() }
            .and_then(number_of)
            .or(self.min)
            .unwrap_or(0.);
        match (current, up) {
            (None, true) => Some(self.fit(start)),
            (None, false) if self.optional => None,
            (None, false) => Some(self.fit(start - step)),
            (Some(number), true) => Some(self.fit(number + step)),
            (Some(number), false) => {
                let below = self.min.is_some_and(|min| number - step < min - 1e-9);
                if below && self.optional {
                    None
                } else {
                    Some(self.fit(number - step))
                }
            }
        }
    }

    /// The number `value` holds, as a number.
    pub fn number(value: &toml::Value) -> Option<f64> {
        number_of(value)
    }

    /// The choice the file holding `value` stands at, by its place among
    /// the choices; for no value, the `unset` choice if there is one.
    pub fn chosen(&self, value: Option<&toml::Value>) -> Option<usize> {
        match value {
            Some(value) => { self.choices.iter() }
                .position(|choice| !choice.is_unset() && same(&choice.value, value)),
            None => self.choices.iter().position(Choice::is_unset),
        }
    }

    /// A number with its unit.
    pub fn shown_number(&self, number: f64) -> String {
        let text = if self.whole() {
            format!("{}", number.round() as i64)
        } else {
            let text = format!("{number:.3}");
            text.trim_end_matches('0').trim_end_matches('.').to_owned()
        };
        match self.unit.as_deref() {
            Some("%") => format!("{text}%"),
            Some(unit) if !unit.is_empty() => format!("{text} {unit}"),
            _ => text,
        }
    }
}

/// Whether pairs survive being written as `name=value; name=value`: no
/// separator in a name or a value, and no `=` in a name.
pub fn check_pairs(pairs: &[(String, String)]) -> Result<(), String> {
    for (name, value) in pairs {
        if name.trim().is_empty() || value.trim().is_empty() {
            return Err(format!("`{name}={value}` lacks a name or a value"));
        }
        if name.contains(['=', ';', '\n']) || value.contains([';', '\n']) {
            return Err(format!(
                "`{name}={value}` holds a `;`, or a `=` in its name"
            ));
        }
    }
    Ok(())
}

/// The settings one extension or theme declares, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptionSet {
    pub decls: Vec<OptionDecl>,
}

/// What [`OptionSet::resolve`] makes of a table.
#[derive(Debug, Default, PartialEq)]
pub struct Resolved {
    pub values: toml::Table,
    /// The values that were dropped, each said as a sentence.
    pub problems: Vec<String>,
}

impl OptionSet {
    /// The declarations that are usable; what is wrong with the others is
    /// said, each as a sentence.
    pub fn checked(decls: Vec<OptionDecl>) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        let mut kept: Vec<OptionDecl> = Vec::new();
        for decl in decls {
            if kept.iter().any(|other| other.key == decl.key) {
                problems.push(format!("the setting `{}` is declared twice", decl.key));
                continue;
            }
            match decl.check() {
                Ok(()) => kept.push(decl),
                Err(error) => problems.push(format!("the setting `{}` {error}", decl.key)),
            }
        }
        (Self { decls: kept }, problems)
    }

    pub fn get(&self, key: &str) -> Option<&OptionDecl> {
        self.decls.iter().find(|decl| decl.key == key)
    }

    pub fn is_empty(&self) -> bool {
        self.decls.is_empty()
    }

    /// `table` with the defaults of what it lacks, and without the values
    /// that do not fit their declaration; those are said in `problems` and
    /// replaced by their default. Keys nothing declares are kept as they
    /// are, since an extension may read keys it does not declare.
    pub fn resolve_table(&self, table: &toml::Table) -> Resolved {
        let mut resolved = Resolved {
            values: table.clone(),
            problems: Vec::new(),
        };
        for decl in &self.decls {
            let value = match table.get(&decl.key) {
                Some(value) => match decl.validate(value) {
                    Ok(value) => Some(value),
                    Err(error) => {
                        resolved
                            .problems
                            .push(format!("the setting `{}` is not used: {error}", decl.key));
                        decl.default.clone()
                    }
                },
                None => decl.default.clone(),
            };
            match value {
                Some(value) => resolved.values.insert(decl.key.clone(), value),
                None => resolved.values.remove(&decl.key),
            };
        }
        resolved
    }

    /// The same, with the problems reported under `owner`.
    pub fn resolve(&self, table: &toml::Table, owner: &str) -> toml::Table {
        let resolved = self.resolve_table(table);
        for problem in resolved.problems {
            report!("{owner}: {problem}");
        }
        resolved.values
    }

    /// Whether `decl` is shown while the settings hold `values`, which
    /// are resolved.
    pub fn visible(decl: &OptionDecl, values: &toml::Table) -> bool {
        match &decl.visible_if {
            None => true,
            Some(condition) => {
                { values.get(&condition.key) }.is_some_and(|value| same(value, &condition.equals))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(text: &str) -> OptionDecl {
        #[derive(Deserialize)]
        struct File {
            setting: Vec<OptionDecl>,
        }
        let file: File = toml::from_str(text).unwrap();
        let decl = file.setting.into_iter().next().unwrap();
        decl.check().unwrap();
        decl
    }

    fn value(text: &str) -> toml::Value {
        let table: toml::Table = toml::from_str(&format!("v = {text}")).unwrap();
        table["v"].clone()
    }

    #[test]
    fn a_toggle_is_true_or_false() {
        let toggle = decl("[[setting]]\nkey = \"on\"\nkind = \"toggle\"\nlabel = \"On\"\n");
        assert_eq!(toggle.validate(&value("true")), Ok(true.into()));
        assert_eq!(toggle.validate(&value("\"off\"")), Ok(false.into()));
        assert!(toggle.validate(&value("3")).is_err());
    }

    #[test]
    fn a_text_keeps_to_its_length_and_one_line() {
        let text = decl(
            "[[setting]]\nkey = \"name\"\nkind = \"text\"\nlabel = \"Name\"\nmax_length = 4\n",
        );
        assert_eq!(text.validate(&value("\"abc\"")), Ok("abc".into()));
        assert_eq!(text.validate(&value("12")), Ok("12".into()));
        assert!(text.validate(&value("\"abcde\"")).is_err());
        assert!(text.validate(&value("\"a\\nb\"")).is_err());
        assert!(text.validate(&value("true")).is_err());
    }

    #[test]
    fn numbers_are_clamped_rounded_and_written_whole_where_they_can_be() {
        let minutes = decl(
            "[[setting]]\nkey = \"m\"\nkind = \"number\"\nlabel = \"M\"\nmin = 5\nmax = 60\nstep = 5\ndefault = 20\n",
        );
        assert!(minutes.whole());
        assert_eq!(minutes.validate(&value("22")), Ok(20.into()));
        assert_eq!(minutes.validate(&value("23.5")), Ok(25.into()));
        assert_eq!(minutes.validate(&value("1")), Ok(5.into()));
        assert_eq!(minutes.validate(&value("900")), Ok(60.into()));
        assert_eq!(minutes.validate(&value("\"30\"")), Ok(30.into()));
        assert!(minutes.validate(&value("\"many\"")).is_err());

        let scale = decl(
            "[[setting]]\nkey = \"s\"\nkind = \"slider\"\nlabel = \"S\"\nmin = 0.5\nmax = 2\nstep = 0.1\n",
        );
        assert!(!scale.whole());
        assert_eq!(scale.validate(&value("1.234")), Ok(toml::Value::Float(1.2)));
        assert_eq!(scale.validate(&value("1")), Ok(toml::Value::Float(1.0)));
        assert_eq!(scale.validate(&value("7")), Ok(toml::Value::Float(2.0)));
        assert_eq!(scale.shown_number(1.5), "1.5");

        let degrees = decl(
            "[[setting]]\nkey = \"latitude\"\nkind = \"number\"\nlabel = \"L\"\nmin = -90\nmax = 90\nstep = 0.01\noptional = true\n",
        );
        assert_eq!(
            degrees.validate(&value("52.523")),
            Ok(toml::Value::Float(52.52))
        );
        assert_eq!(
            degrees.validate(&value("-100")),
            Ok(toml::Value::Float(-90.0))
        );
    }

    #[test]
    fn an_optional_number_leaves_the_file_a_step_under_its_least() {
        let own = decl(
            "[[setting]]\nkey = \"own\"\nkind = \"number\"\nlabel = \"Own\"\nmin = 2\nmax = 9\nstep = 1\noptional = true\nunset_label = \"Off\"\n",
        );
        assert_eq!(own.stepped(None, true), Some(2.));
        assert_eq!(own.stepped(Some(2.), false), None);
        assert_eq!(own.stepped(None, false), None);
        assert_eq!(own.stepped(Some(9.), true), Some(9.));
        assert_eq!(own.stepped(Some(4.), false), Some(3.));
        let plain = decl(
            "[[setting]]\nkey = \"n\"\nkind = \"number\"\nlabel = \"N\"\nmin = 1\nmax = 3\ndefault = 2\n",
        );
        assert_eq!(plain.stepped(Some(1.), false), Some(1.));
        assert_eq!(plain.stepped(None, true), Some(2.));
    }

    #[test]
    fn a_choice_is_one_of_its_values_and_may_be_unset() {
        let unit = decl(concat!(
            "[[setting]]\nkey = \"unit\"\nkind = \"choice\"\nlabel = \"Unit\"\ndefault = \"celsius\"\n",
            "choices = [{ value = \"celsius\", label = \"Celsius\" }, { value = \"fahrenheit\" }, { value = \"unset\", label = \"Off\" }]\n",
        ));
        assert_eq!(
            unit.validate(&value("\"fahrenheit\"")),
            Ok("fahrenheit".into())
        );
        assert!(unit.validate(&value("\"kelvin\"")).is_err());
        // The word itself is no value to write.
        assert!(unit.validate(&value("\"unset\"")).is_err());
        assert_eq!(unit.chosen(Some(&"celsius".into())), Some(0));
        assert_eq!(unit.chosen(None), Some(2));
        assert_eq!(unit.choices[1].label(Language::English), "fahrenheit");
        let numbers = decl(
            "[[setting]]\nkey = \"n\"\nkind = \"choice\"\nlabel = \"N\"\nchoices = [{ value = 1 }, { value = 2.5 }]\n",
        );
        assert_eq!(numbers.validate(&value("1.0")), Ok(1.into()));
        assert_eq!(numbers.chosen(Some(&value("2.5"))), Some(1));
    }

    #[test]
    fn a_color_is_six_or_eight_hex_digits() {
        let color = decl("[[setting]]\nkey = \"c\"\nkind = \"color\"\nlabel = \"C\"\n");
        assert_eq!(color.validate(&value("\"#3D7BD9\"")), Ok("#3d7bd9".into()));
        assert_eq!(
            color.validate(&value("\"#3d7bd980\"")),
            Ok("#3d7bd980".into())
        );
        for wrong in ["\"3d7bd9\"", "\"#3d7\"", "\"#3d7bd9f\"", "\"#zzzzzz\"", "4"] {
            assert!(color.validate(&value(wrong)).is_err(), "{wrong} was taken");
        }
    }

    #[test]
    fn a_list_goes_through_the_text_the_kit_reads() {
        let coins = decl("[[setting]]\nkey = \"coins\"\nkind = \"list\"\nlabel = \"Coins\"\n");
        let listed = coins
            .validate(&value("\"bitcoin, ethereum; monero\""))
            .unwrap();
        assert_eq!(listed, value("[\"bitcoin\", \"ethereum\", \"monero\"]"));
        let text = text_of(&listed);
        assert_eq!(text, "bitcoin, ethereum, monero");
        assert_eq!(parse_list(&text), ["bitcoin", "ethereum", "monero"]);
        assert_eq!(
            coins.validate(&value("[\" a \", \"\", \"b\"]")),
            Ok(value("[\"a\", \"b\"]"))
        );
        assert!(coins.validate(&value("[\"a,b\"]")).is_err());
        assert!(coins.validate(&value("[1]")).is_err());
    }

    #[test]
    fn pairs_go_through_the_text_the_kit_reads() {
        let zones = decl("[[setting]]\nkey = \"zones\"\nkind = \"pairs\"\nlabel = \"Zones\"\n");
        let written = zones
            .validate(&value("\"Tokyo = +9;London=+0\nA=b=c\""))
            .unwrap();
        assert_eq!(written.as_str(), Some("Tokyo=+9; London=+0; A=b=c"));
        let pairs = parse_pairs(written.as_str().unwrap()).unwrap();
        assert_eq!(pairs[2], ("A".to_owned(), "b=c".to_owned()));
        assert_eq!(write_pairs(&pairs), written.as_str().unwrap());
        assert!(zones.validate(&value("\"Tokyo\"")).is_err());
        assert!(zones.validate(&value("\"=3\"")).is_err());
        assert!(check_pairs(&[("A".to_owned(), "b;c".to_owned())]).is_err());
        assert!(check_pairs(&[("A=".to_owned(), "b".to_owned())]).is_err());
    }

    #[test]
    fn defaults_fill_what_is_missing_and_replace_what_is_wrong() {
        let (set, problems) = OptionSet::checked(
            toml::from_str::<toml::Table>(concat!(
                "[[setting]]\nkey = \"unit\"\nkind = \"choice\"\nlabel = \"Unit\"\ndefault = \"c\"\n",
                "choices = [{ value = \"c\" }, { value = \"f\" }]\n",
                "[[setting]]\nkey = \"minutes\"\nkind = \"number\"\nlabel = \"M\"\nmin = 5\ndefault = 20\n",
                "[[setting]]\nkey = \"latitude\"\nkind = \"number\"\nlabel = \"L\"\noptional = true\n",
                "[[setting]]\nkey = \"minutes\"\nkind = \"toggle\"\nlabel = \"Again\"\n",
                "[[setting]]\nkey = \"bad\"\nkind = \"number\"\nlabel = \"B\"\nmin = 3\nmax = 1\n",
            ))
            .unwrap()["setting"]
                .clone()
                .try_into::<Vec<OptionDecl>>()
                .unwrap(),
        );
        assert_eq!(set.decls.len(), 3);
        assert_eq!(problems.len(), 2);
        let table: toml::Table =
            toml::from_str("unit = \"kelvin\"\nminutes = 2\ntoken = \"secret\"\n").unwrap();
        let resolved = set.resolve_table(&table);
        assert_eq!(resolved.values["unit"].as_str(), Some("c"));
        assert_eq!(resolved.values["minutes"].as_integer(), Some(5));
        assert!(!resolved.values.contains_key("latitude"));
        // What is not declared reaches the extension as written.
        assert_eq!(resolved.values["token"].as_str(), Some("secret"));
        assert_eq!(resolved.problems.len(), 1);
        assert!(resolved.problems[0].contains("unit"));
        let empty = set.resolve_table(&toml::Table::new());
        assert_eq!(empty.values["minutes"].as_integer(), Some(20));
        assert!(empty.problems.is_empty());
    }

    #[test]
    fn a_default_that_does_not_fit_makes_the_declaration_unusable() {
        let text = "[[setting]]\nkey = \"n\"\nkind = \"number\"\nlabel = \"N\"\nmax = 3\ndefault = \"x\"\n";
        let file: toml::Table = toml::from_str(text).unwrap();
        let decls: Vec<OptionDecl> = file["setting"].clone().try_into().unwrap();
        assert!(decls[0].check().is_err());
        let choice = "[[setting]]\nkey = \"c\"\nkind = \"choice\"\nlabel = \"C\"\n";
        let file: toml::Table = toml::from_str(choice).unwrap();
        let decls: Vec<OptionDecl> = file["setting"].clone().try_into().unwrap();
        assert!(decls[0].check().is_err());
    }

    #[test]
    fn a_setting_is_shown_while_its_condition_holds() {
        let shown = decl(concat!(
            "[[setting]]\nkey = \"times\"\nkind = \"number\"\nlabel = \"T\"\n",
            "visible_if = { key = \"auto\", equals = false }\n",
        ));
        let values: toml::Table = toml::from_str("auto = false").unwrap();
        assert!(OptionSet::visible(&shown, &values));
        let values: toml::Table = toml::from_str("auto = true").unwrap();
        assert!(!OptionSet::visible(&shown, &values));
        assert!(!OptionSet::visible(&shown, &toml::Table::new()));
    }

    #[test]
    fn a_label_speaks_the_language_in_use_or_english() {
        let translated = decl(
            "[[setting]]\nkey = \"u\"\nkind = \"toggle\"\nlabel = { en = \"Unit\", ru = \"Единица\" }\n",
        );
        assert_eq!(translated.label.text(Language::Russian), "Единица");
        assert_eq!(translated.label.text(Language::English), "Unit");
        let english = Label::Translated([("en".to_owned(), "Only".to_owned())].into());
        assert_eq!(english.text(Language::Russian), "Only");
        // A plain label that Tessera knows is said in the language.
        let plain = Label::Plain("Quit".to_owned());
        assert_eq!(plain.text(Language::Russian), "Выйти");
        assert_eq!(
            Label::Plain("Unheard of".to_owned()).text(Language::Russian),
            "Unheard of"
        );
    }

    #[test]
    fn a_key_is_plain() {
        let text = "[[setting]]\nkey = \"a b\"\nkind = \"toggle\"\nlabel = \"A\"\n";
        let file: toml::Table = toml::from_str(text).unwrap();
        let decls: Vec<OptionDecl> = file["setting"].clone().try_into().unwrap();
        assert!(decls[0].check().is_err());
    }
}
