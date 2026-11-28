// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A rule as the editor holds it while it is changed: the table the file
//! will hold, the keys of what it can say with the control each is set
//! with, and the one-line summaries of the list. Nothing here draws.

use crate::rules::RuleConfig;
use crate::rules::matching::Naming;

/// The keys that name windows by a text, in the order they are offered.
pub const NAME_KEYS: [&str; 5] = ["exe", "path", "app", "title", "class"];

/// The words of `match`, in the order they are offered: every mode
/// rules/matching.rs reads.
pub const MODES: [&str; 6] = ["glob", "equals", "starts", "ends", "contains", "regex"];

/// How a key of what a rule says is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// On, off or not said.
    Flag,
    /// A workspace of Tessera's own, from 1.
    Workspace,
    /// A display, from 1 from the left.
    Display,
    /// How opaque, from 0.1 to 1.
    Opacity,
    /// `center`, or `[x, y]` in shares of the work area.
    Position,
    /// `[width, height]` in shares of the work area.
    Size,
    /// `[width, height]` in logical pixels.
    Sides,
    /// A word for the kind of window, offering those in use.
    Category,
    /// A text: a file or an identifier.
    Text,
    /// How strictly the window is kept from taking the foreground.
    Focus,
    /// When the rule does what it says.
    Apply,
}

/// Every key of what a rule says (`Says` in rules.rs), with its English
/// label and its control, in the order they are offered.
pub const SAYS: [(&str, &str, Control); 27] = [
    ("float", "Float", Control::Flag),
    ("tile", "Tile", Control::Flag),
    ("workspace", "Workspace", Control::Workspace),
    ("display", "Display", Control::Display),
    ("position", "Position", Control::Position),
    ("size", "Size", Control::Size),
    ("maximize", "Maximized", Control::Flag),
    ("minimized", "Minimized", Control::Flag),
    ("fullscreen", "Full screen", Control::Flag),
    ("above", "Keep above other windows", Control::Flag),
    ("below", "Keep below other windows", Control::Flag),
    ("sticky", "On every workspace", Control::Flag),
    ("opacity", "Opacity", Control::Opacity),
    (
        "inactive_opacity",
        "Opacity while not in front",
        Control::Opacity,
    ),
    ("border", "Border", Control::Flag),
    ("no_titlebar", "Without title bar", Control::Flag),
    ("min_size", "Least size", Control::Sides),
    ("max_size", "Largest size", Control::Sides),
    ("category", "Category", Control::Category),
    ("skip_dock", "Not in the dock", Control::Flag),
    ("skip_switcher", "Not in the window switcher", Control::Flag),
    ("accept_focus", "Takes the keyboard", Control::Flag),
    ("closeable", "Can be closed", Control::Flag),
    ("icon", "Icon", Control::Text),
    ("app_id", "Application", Control::Text),
    ("focus", "Focus stealing prevention", Control::Focus),
    ("apply", "When it applies", Control::Apply),
];

/// The least and most opacity, and the step of the buttons that change it.
pub const OPACITIES: (f64, f64, f64) = (0.1, 1., 0.05);

/// The least and most side of `min_size` and `max_size`.
pub const SIDES: (i64, i64) = (1, 20_000);

/// A step from a table of names into one in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    /// The table of `not`.
    Not,
    /// A table of `any`, by its place.
    Any(usize),
    /// A table of `all`, by its place.
    All(usize),
}

/// A list of tables of names that a table can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Branch {
    Not,
    Any,
    All,
}

impl Branch {
    pub fn key(self) -> &'static str {
        match self {
            Self::Not => "not",
            Self::Any => "any",
            Self::All => "all",
        }
    }
}

/// A rule being changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    /// The rule as the file will hold it.
    pub table: toml::Table,
    /// Its place among the rules of the file; `None` for a new one.
    pub index: Option<usize>,
}

impl Draft {
    pub fn new(table: toml::Table, index: Option<usize>) -> Self {
        Self { table, index }
    }

    /// The table of names at `path`.
    pub fn node(&self, path: &[Step]) -> Option<&toml::Table> {
        let mut node = &self.table;
        for step in path {
            node = match step {
                Step::Not => node.get("not")?.as_table()?,
                Step::Any(at) => node.get("any")?.as_array()?.get(*at)?.as_table()?,
                Step::All(at) => node.get("all")?.as_array()?.get(*at)?.as_table()?,
            };
        }
        Some(node)
    }

    fn node_mut(&mut self, path: &[Step]) -> Option<&mut toml::Table> {
        let mut node = &mut self.table;
        for step in path {
            node = match step {
                Step::Not => node.get_mut("not")?.as_table_mut()?,
                Step::Any(at) => node
                    .get_mut("any")?
                    .as_array_mut()?
                    .get_mut(*at)?
                    .as_table_mut()?,
                Step::All(at) => node
                    .get_mut("all")?
                    .as_array_mut()?
                    .get_mut(*at)?
                    .as_table_mut()?,
            };
        }
        Some(node)
    }

    /// The text of `key` in the table at `path`.
    pub fn text_at(&self, path: &[Step], key: &str) -> Option<&str> {
        self.node(path)?.get(key)?.as_str()
    }

    /// Sets `key` of the table at `path` to `value`, or takes it out.
    /// Whether the draft changed is returned.
    pub fn set_at(&mut self, path: &[Step], key: &str, value: Option<toml::Value>) -> bool {
        let Some(node) = self.node_mut(path) else {
            return false;
        };
        let before = node.get(key).cloned();
        if before == value {
            return false;
        }
        match value {
            Some(value) => {
                node.insert(key.to_owned(), value);
            }
            None => {
                node.remove(key);
            }
        }
        true
    }

    /// Sets the text `key` of the table at `path`; an empty text takes it
    /// out.
    pub fn set_text_at(&mut self, path: &[Step], key: &str, text: &str) -> bool {
        let value = (!text.is_empty()).then(|| toml::Value::String(text.to_owned()));
        self.set_at(path, key, value)
    }

    /// Sets a key of what the rule says; `None` takes it out.
    pub fn set(&mut self, key: &str, value: Option<toml::Value>) -> bool {
        self.set_at(&[], key, value)
    }

    /// The flag `key`, where it is said.
    pub fn flag(&self, key: &str) -> Option<bool> {
        self.table.get(key)?.as_bool()
    }

    /// The number `key`, where it is said.
    pub fn number(&self, key: &str) -> Option<f64> {
        let value = self.table.get(key)?;
        value
            .as_float()
            .or_else(|| value.as_integer().map(|whole| whole as f64))
    }

    /// The pair of numbers `key`, as `[x, y]` or `[width, height]`.
    pub fn pair(&self, key: &str) -> Option<[f64; 2]> {
        let items = self.table.get(key)?.as_array()?;
        let number = |value: &toml::Value| {
            value
                .as_float()
                .or_else(|| value.as_integer().map(|whole| whole as f64))
        };
        match items.as_slice() {
            [first, second] => Some([number(first)?, number(second)?]),
            _ => None,
        }
    }

    /// The text `key` of what the rule says.
    pub fn text(&self, key: &str) -> Option<&str> {
        self.table.get(key)?.as_str()
    }

    /// Whether the rule is used.
    pub fn enabled(&self) -> bool {
        self.flag("enabled") != Some(false)
    }

    /// Switches the rule on or off; on is written by leaving `enabled` out.
    pub fn set_enabled(&mut self, on: bool) -> bool {
        self.set("enabled", (!on).then_some(toml::Value::Boolean(false)))
    }

    /// Steps the opacity `key` up or down, from 1 where it is not said.
    pub fn step_opacity(&mut self, key: &str, up: bool) -> bool {
        let (least, most, step) = OPACITIES;
        let now = self.number(key).unwrap_or(most);
        let next = if up { now + step } else { now - step };
        let next = ((next.clamp(least, most)) * 100.).round() / 100.;
        self.set(key, Some(toml::Value::Float(next)))
    }

    /// Adds a table of names to the list `branch` of the table at `path`:
    /// `not` holds one, `any` and `all` hold many. Whether it was added is
    /// returned.
    pub fn add_branch(&mut self, path: &[Step], branch: Branch) -> bool {
        let Some(node) = self.node_mut(path) else {
            return false;
        };
        let empty = toml::Value::Table(toml::Table::new());
        match branch {
            Branch::Not if node.contains_key("not") => false,
            Branch::Not => {
                node.insert("not".to_owned(), empty);
                true
            }
            Branch::Any | Branch::All => {
                let list =
                    { node.entry(branch.key()) }.or_insert_with(|| toml::Value::Array(Vec::new()));
                match list.as_array_mut() {
                    Some(items) => {
                        items.push(empty);
                        true
                    }
                    None => false,
                }
            }
        }
    }

    /// Takes the table of names at `path` away from the one it is in, and
    /// a list of `any` or `all` that is left empty with it.
    pub fn remove_branch(&mut self, path: &[Step]) -> bool {
        let Some((last, parent)) = path.split_last() else {
            return false;
        };
        let Some(node) = self.node_mut(parent) else {
            return false;
        };
        let (key, at) = match last {
            Step::Not => return node.remove("not").is_some(),
            Step::Any(at) => ("any", *at),
            Step::All(at) => ("all", *at),
        };
        let Some(items) = node.get_mut(key).and_then(toml::Value::as_array_mut) else {
            return false;
        };
        if at >= items.len() {
            return false;
        }
        items.remove(at);
        if items.is_empty() {
            node.remove(key);
        }
        true
    }

    /// The rule as the configuration reads it, or why it cannot be read
    /// or used.
    pub fn check(&self) -> Result<RuleConfig, String> {
        let rule = RuleConfig::of_table(self.table.clone())?;
        match rule.fault() {
            Some(fault) => Err(fault.to_owned()),
            None => Ok(rule),
        }
    }
}

/// A value as a rule writes it, in one line.
fn written(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        toml::Value::Table(table) => {
            let parts: Vec<String> = { table.iter() }
                .map(|(key, value)| format!("{key} {}", written(value)))
                .collect();
            format!("({})", parts.join(", "))
        }
        toml::Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(written).collect();
            format!("[{}]", parts.join(", "))
        }
        other => other.to_string(),
    }
}

/// What a rule names and what it says, each in one line of its keys and
/// values as the file writes them, for the list of rules.
pub fn summary(table: &toml::Table) -> (String, String) {
    let mut names = Vec::new();
    for key in Naming::KEYS {
        if let Some(value) = table.get(key) {
            names.push(format!("{key} {}", written(value)));
        }
    }
    let says: Vec<String> = { table.iter() }
        .filter(|(key, _)| !Naming::KEYS.contains(&key.as_str()) && *key != "enabled")
        .map(|(key, value)| match value {
            toml::Value::Boolean(true) => key.clone(),
            toml::Value::Boolean(false) => format!("{key} off"),
            other => format!("{key} {}", written(other)),
        })
        .collect();
    (names.join(" · "), says.join(" · "))
}

/// The categories `tables` give, each once, in the order they come.
pub fn categories<'a>(tables: impl IntoIterator<Item = &'a toml::Table>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for table in tables {
        if let Some(category) = table.get("category").and_then(toml::Value::as_str)
            && !found.iter().any(|known| known == category)
        {
            found.push(category.to_owned());
        }
    }
    found
}

/// A share of the work area as a rule writes it: three places after the
/// point at most.
pub fn share_value(share: f64) -> toml::Value {
    toml::Value::Float((share.clamp(0., 1.) * 1000.).round() / 1000.)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(text: &str) -> Draft {
        Draft::new(toml::from_str(text).unwrap(), Some(0))
    }

    #[test]
    fn every_key_a_rule_can_say_has_a_control() {
        // The rule refuses a key it does not know, and names those it does.
        let error = RuleConfig::of_table(toml::from_str("exe = \"a\"\nunknown_key = 1").unwrap())
            .unwrap_err();
        let expected = error.split("expected one of").nth(1).unwrap_or_default();
        let known: Vec<&str> = { expected.split(',') }
            .map(|key| key.trim().trim_matches('`'))
            .filter(|key| !key.is_empty())
            .collect();
        let offered: Vec<&str> = SAYS.iter().map(|(key, _, _)| *key).collect();
        let mut known_sorted = known.clone();
        known_sorted.sort_unstable();
        let mut offered_sorted = offered.clone();
        offered_sorted.sort_unstable();
        assert_eq!(known_sorted, offered_sorted, "{error}");
    }

    #[test]
    fn every_mode_is_one_the_rules_read() {
        for mode in MODES {
            let text = format!("exe = \"a.exe\"\nmatch = \"{mode}\"");
            assert!(draft(&text).check().is_ok(), "{mode}");
        }
        assert!(draft("exe = \"a.exe\"\nmatch = \"fuzzy\"").check().is_err());
    }

    #[test]
    fn names_are_set_in_the_tables_of_names() {
        let mut rule = draft("exe = \"a.exe\"");
        assert!(rule.add_branch(&[], Branch::Not));
        assert!(!rule.add_branch(&[], Branch::Not));
        assert!(rule.set_text_at(&[Step::Not], "title", "Setup"));
        assert!(rule.add_branch(&[], Branch::Any));
        assert!(rule.add_branch(&[], Branch::Any));
        assert!(rule.set_text_at(&[Step::Any(1)], "class", "B"));
        assert!(rule.add_branch(&[Step::Any(0)], Branch::All));
        assert!(rule.set_text_at(&[Step::Any(0), Step::All(0)], "exe", "c.exe"));
        assert_eq!(rule.text_at(&[Step::Not], "title"), Some("Setup"));
        assert_eq!(
            rule.text_at(&[Step::Any(0), Step::All(0)], "exe"),
            Some("c.exe")
        );
        assert!(rule.check().is_ok(), "{:?}", rule.check());
        // An empty text takes the name out.
        assert!(rule.set_text_at(&[Step::Not], "title", ""));
        assert_eq!(rule.text_at(&[Step::Not], "title"), None);
        assert!(!rule.set_text_at(&[Step::Not], "title", ""));
        assert!(rule.remove_branch(&[Step::Any(0)]));
        assert_eq!(rule.text_at(&[Step::Any(0)], "class"), Some("B"));
        assert!(rule.remove_branch(&[Step::Any(0)]));
        assert!(!rule.table.contains_key("any"));
        assert!(rule.remove_branch(&[Step::Not]));
        assert!(!rule.remove_branch(&[]));
    }

    #[test]
    fn what_a_rule_says_is_set_and_checked() {
        let mut rule = draft("exe = \"a.exe\"");
        assert!(rule.set("float", Some(true.into())));
        assert_eq!(rule.flag("float"), Some(true));
        assert!(rule.step_opacity("opacity", false));
        assert_eq!(rule.number("opacity"), Some(0.95));
        for _ in 0..30 {
            rule.step_opacity("opacity", false);
        }
        assert_eq!(rule.number("opacity"), Some(0.1));
        assert!(rule.set(
            "size",
            Some(toml::Value::Array(vec![share_value(0.5), share_value(1.2)]))
        ));
        assert_eq!(rule.pair("size"), Some([0.5, 1.]));
        assert!(rule.check().is_ok());
        assert!(rule.set("workspace", Some(0.into())));
        assert!(rule.check().unwrap_err().contains("counted from 1"));
        assert!(rule.set("workspace", None));
        assert!(rule.set("positon", Some("center".into())));
        assert!(rule.check().unwrap_err().contains("positon"));
        let mut nameless = draft("float = true");
        assert!(nameless.check().unwrap_err().contains("names no windows"));
        assert!(nameless.set_text_at(&[], "type", "dialog"));
        assert!(nameless.check().is_ok());
    }

    #[test]
    fn a_rule_is_switched_off_and_on() {
        let mut rule = draft("exe = \"a.exe\"");
        assert!(rule.enabled());
        assert!(rule.set_enabled(false));
        assert_eq!(
            rule.table.get("enabled"),
            Some(&toml::Value::Boolean(false))
        );
        assert!(!rule.check().unwrap().is_enabled());
        assert!(rule.set_enabled(true));
        assert!(!rule.table.contains_key("enabled"));
    }

    #[test]
    fn a_rule_is_summed_up_in_two_lines() {
        let table = toml::from_str(
            "exe = \"code.exe\"\nnot = { title = \"Settings\" }\nfloat = true\nborder = false\nopacity = 0.8\nenabled = false",
        )
        .unwrap();
        let (names, says) = summary(&table);
        assert_eq!(names, "exe code.exe · not (title Settings)");
        let mut said: Vec<&str> = says.split(" · ").collect();
        said.sort_unstable();
        assert_eq!(said, ["border off", "float", "opacity 0.8"]);
        let tables: Vec<toml::Table> = [
            "category = \"game\"",
            "category = \"browser\"",
            "category = \"game\"",
        ]
        .into_iter()
        .map(|text| toml::from_str(text).unwrap())
        .collect();
        assert_eq!(categories(&tables), ["game", "browser"]);
    }
}
