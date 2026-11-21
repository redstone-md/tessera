// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the widgets show.

use crate::clock::span;
use crate::model::{Meter, Provider, Report};
use crate::{Button, Node, NodeKind};

/// What is known about one service.
#[derive(Clone, Debug)]
pub struct Slot {
    pub provider: Provider,
    /// The number of the request under way.
    pub pending: Option<u32>,
    /// The plan the credentials named when the service was last asked.
    pub plan: Option<String>,
    pub report: Option<Report>,
    /// Why there is no report.
    pub error: Option<String>,
    /// When the service was last asked, in seconds since 1970.
    pub asked_at: Option<u64>,
}

impl Slot {
    pub fn new(provider: Provider) -> Self {
        Self {
            provider,
            pending: None,
            plan: None,
            report: None,
            error: None,
            asked_at: None,
        }
    }
}

/// What the user chose to see; kept across restarts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub plan: bool,
    pub reset: bool,
    pub bars: bool,
    /// Whether shares are given as what is left rather than what is used.
    pub remaining: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            plan: true,
            reset: true,
            bars: true,
            remaining: true,
        }
    }
}

impl Prefs {
    const NAMES: [&'static str; 4] = ["plan", "reset", "bars", "remaining"];

    fn flag(&mut self, name: &str) -> Option<&mut bool> {
        match name {
            "plan" => Some(&mut self.plan),
            "reset" => Some(&mut self.reset),
            "bars" => Some(&mut self.bars),
            "remaining" => Some(&mut self.remaining),
            _ => None,
        }
    }

    /// Flips the preference `name`; `false` if there is none.
    pub fn toggle(&mut self, name: &str) -> bool {
        self.flag(name).map(|flag| *flag = !*flag).is_some()
    }

    /// Reads what `store` wrote; anything else keeps the defaults.
    pub fn restore(state: &str) -> Self {
        let mut prefs = Self::default();
        for (name, value) in state.lines().filter_map(|line| line.split_once('=')) {
            if let (Some(flag), Ok(value)) = (prefs.flag(name), value.parse()) {
                *flag = value;
            }
        }
        prefs
    }

    pub fn store(mut self) -> String {
        let lines = Self::NAMES.map(|name| {
            let value = self.flag(name).is_some_and(|flag| *flag);
            format!("{name}={value}\n")
        });
        lines.concat()
    }
}

/// The services a setting lists, such as `claude, codex` or a TOML array
/// of the same names. Unknown names are returned for reporting.
pub fn providers(setting: &str) -> (Vec<Provider>, Vec<String>) {
    let (mut known, mut unknown) = (Vec::new(), Vec::new());
    let names = setting
        .split(',')
        .map(|name| name.trim_matches(|c: char| c.is_whitespace() || "[]\"'".contains(c)))
        .filter(|name| !name.is_empty());
    for name in names {
        match Provider::ALL.iter().find(|provider| provider.id() == name) {
            Some(&provider) if !known.contains(&provider) => known.push(provider),
            Some(_) => {}
            None => unknown.push(name.to_owned()),
        }
    }
    (known, unknown)
}

/// A content tree under construction.
struct Tree(Vec<Node>);

impl Tree {
    fn new(root: NodeKind) -> Self {
        Self(vec![Node {
            kind: root,
            children: Vec::new(),
        }])
    }

    /// Adds a node under `parent` and returns its index.
    fn add(&mut self, parent: u32, kind: NodeKind) -> u32 {
        let index = self.0.len() as u32;
        self.0.push(Node {
            kind,
            children: Vec::new(),
        });
        self.0[parent as usize].children.push(index);
        index
    }

    fn button(&mut self, parent: u32, label: String, action: &str) {
        let action = action.to_owned();
        self.add(parent, NodeKind::Button(Button { label, action }));
    }
}

fn percent(share: f32) -> String {
    format!("{:.0}%", share * 100.)
}

/// The share of a meter the way the user wants it counted.
fn counted(used: f32, prefs: Prefs) -> (f32, &'static str) {
    if prefs.remaining {
        (1. - used, "left")
    } else {
        (used, "used")
    }
}

/// One line for a bar: the tightest limit of each of `shown`.
pub fn summary(slots: &[Slot], shown: &[Provider], prefs: Prefs) -> Vec<Node> {
    let mut tree = Tree::new(NodeKind::Row);
    let slots = shown
        .iter()
        .filter_map(|provider| slots.iter().find(|slot| slot.provider == *provider));
    let mut any = false;
    for slot in slots {
        any = true;
        let name = slot.provider.name();
        let meters = slot.report.iter().flat_map(|report| &report.meters);
        let tightest = meters
            .clone()
            .filter_map(|meter| meter.used)
            .max_by(f32::total_cmp);
        let amount = meters.filter_map(|meter| meter.amount.as_deref()).next();
        let text = match (tightest, amount) {
            (Some(used), _) => format!("{name} {}", percent(counted(used, prefs).0)),
            (None, Some(amount)) => format!("{name} {amount}"),
            (None, None) if slot.error.is_some() => format!("{name} ?"),
            (None, None) => format!("{name} …"),
        };
        let known = slot.report.is_some();
        tree.add(
            0,
            if known {
                NodeKind::Text(text)
            } else {
                NodeKind::Caption(text)
            },
        );
    }
    if !any {
        tree.add(0, NodeKind::Caption("No AI service to show".to_owned()));
    }
    tree.0
}

/// Everything known about every service, and the buttons that choose what
/// is shown.
pub fn details(slots: &[Slot], prefs: Prefs, now: u64) -> Vec<Node> {
    let mut tree = Tree::new(NodeKind::Column);
    for slot in slots {
        let section = tree.add(0, NodeKind::Column);
        let heading = tree.add(section, NodeKind::Row);
        tree.add(heading, NodeKind::Text(slot.provider.name().to_owned()));
        let plan = slot.report.as_ref().and_then(|report| report.plan.clone());
        if let Some(plan) = plan.or_else(|| slot.plan.clone()).filter(|_| prefs.plan) {
            tree.add(heading, NodeKind::Caption(plan));
        }
        match (&slot.report, &slot.error) {
            (Some(report), _) => {
                for meter in &report.meters {
                    add_meter(&mut tree, section, meter, prefs, now);
                }
            }
            (None, Some(error)) => {
                tree.add(section, NodeKind::Caption(error.clone()));
            }
            (None, None) => {
                tree.add(section, NodeKind::Caption("Asking…".to_owned()));
            }
        }
    }
    if slots.is_empty() {
        let hint = "No service is enabled; see `providers` in the settings.";
        tree.add(0, NodeKind::Caption(hint.to_owned()));
    }

    let on = |shown: bool| if shown { "on" } else { "off" };
    let toggles = tree.add(0, NodeKind::Row);
    tree.button(toggles, format!("Plan: {}", on(prefs.plan)), "plan");
    tree.button(toggles, format!("Reset: {}", on(prefs.reset)), "reset");
    tree.button(toggles, format!("Bars: {}", on(prefs.bars)), "bars");
    let actions = tree.add(0, NodeKind::Row);
    let counting = if prefs.remaining { "left" } else { "used" };
    tree.button(actions, format!("Count: {counting}"), "remaining");
    tree.button(actions, "Ask again".to_owned(), "refresh");
    tree.0
}

fn add_meter(tree: &mut Tree, section: u32, meter: &Meter, prefs: Prefs, now: u64) {
    let line = tree.add(section, NodeKind::Row);
    tree.add(line, NodeKind::Caption(meter.label.clone()));
    match (meter.used, &meter.amount) {
        (Some(used), amount) => {
            let (share, word) = counted(used, prefs);
            tree.add(line, NodeKind::Text(format!("{} {word}", percent(share))));
            if prefs.bars {
                tree.add(section, NodeKind::Progress(share));
            }
            if let Some(amount) = amount {
                tree.add(section, NodeKind::Caption(amount.clone()));
            }
        }
        (None, Some(amount)) => {
            tree.add(line, NodeKind::Text(amount.clone()));
        }
        (None, None) => {}
    }
    if let Some(resets_at) = meter.resets_at.filter(|_| prefs.reset) {
        let text = match resets_at.checked_sub(now) {
            Some(left) if left > 0 => format!("Resets in {}", span(left)),
            _ => "Reset is due".to_owned(),
        };
        tree.add(section, NodeKind::Caption(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_772_361_000;

    fn meter(label: &str, used: f32, resets_in: Option<u64>) -> Meter {
        Meter {
            label: label.to_owned(),
            used: Some(used),
            amount: None,
            resets_at: resets_in.map(|seconds| NOW + seconds),
        }
    }

    fn claude() -> Slot {
        Slot {
            plan: Some("Max plan".to_owned()),
            report: Some(Report {
                plan: None,
                meters: vec![
                    meter("5-hour session", 0.42, Some(7500)),
                    meter("Week, all models", 0.9, None),
                ],
            }),
            ..Slot::new(Provider::Claude)
        }
    }

    /// The text of every node, in order; buttons as `[label]`.
    fn texts(nodes: &[Node]) -> Vec<String> {
        nodes
            .iter()
            .filter_map(|node| match &node.kind {
                NodeKind::Text(text) | NodeKind::Caption(text) => Some(text.clone()),
                NodeKind::Button(button) => Some(format!("[{}]", button.label)),
                NodeKind::Progress(share) => Some(format!("<{share:.2}>")),
                NodeKind::Row | NodeKind::Column => None,
            })
            .collect()
    }

    #[test]
    fn details_show_plan_limits_and_resets() {
        let nodes = details(&[claude()], Prefs::default(), NOW);
        assert_eq!(
            texts(&nodes),
            [
                "Claude",
                "Max plan",
                "5-hour session",
                "58% left",
                "<0.58>",
                "Resets in 2 h 5 min",
                "Week, all models",
                "10% left",
                "<0.10>",
                "[Plan: on]",
                "[Reset: on]",
                "[Bars: on]",
                "[Count: left]",
                "[Ask again]",
            ]
        );
    }

    #[test]
    fn preferences_remove_what_they_turn_off() {
        let prefs = Prefs {
            plan: false,
            reset: false,
            bars: false,
            remaining: false,
        };
        let nodes = details(&[claude()], prefs, NOW);
        assert_eq!(
            texts(&nodes)[..5],
            [
                "Claude",
                "5-hour session",
                "42% used",
                "Week, all models",
                "90% used"
            ]
        );
    }

    #[test]
    fn details_explain_a_service_without_a_report() {
        let failed = Slot {
            error: Some("The API key was refused (HTTP 401).".to_owned()),
            ..Slot::new(Provider::DeepSeek)
        };
        let asking = Slot::new(Provider::Codex);
        let nodes = details(&[failed, asking], Prefs::default(), NOW);
        assert_eq!(
            texts(&nodes)[..4],
            [
                "DeepSeek",
                "The API key was refused (HTTP 401).",
                "Codex",
                "Asking…"
            ]
        );
    }

    #[test]
    fn a_reset_in_the_past_is_due() {
        let mut slot = claude();
        slot.report.as_mut().unwrap().meters[0].resets_at = Some(NOW - 1);
        let nodes = details(&[slot], Prefs::default(), NOW);
        assert!(texts(&nodes).contains(&"Reset is due".to_owned()));
    }

    #[test]
    fn children_follow_their_parents() {
        let nodes = details(&[claude()], Prefs::default(), NOW);
        for (index, node) in nodes.iter().enumerate() {
            assert!(node.children.iter().all(|&child| child as usize > index));
        }
        assert!(matches!(nodes[0].kind, NodeKind::Column));
    }

    #[test]
    fn the_summary_shows_the_tightest_limit_of_each_service() {
        let balance = Slot {
            report: Some(Report {
                plan: None,
                meters: vec![Meter {
                    label: "Balance".to_owned(),
                    used: None,
                    amount: Some("110.00 USD".to_owned()),
                    resets_at: None,
                }],
            }),
            ..Slot::new(Provider::DeepSeek)
        };
        let slots = [claude(), balance, Slot::new(Provider::Codex)];
        let shown = [Provider::DeepSeek, Provider::Claude, Provider::Codex];
        let nodes = summary(&slots, &shown, Prefs::default());
        assert!(matches!(nodes[0].kind, NodeKind::Row));
        assert_eq!(
            texts(&nodes),
            ["DeepSeek 110.00 USD", "Claude 10%", "Codex …"]
        );
        let nodes = summary(&slots, &[], Prefs::default());
        assert_eq!(texts(&nodes), ["No AI service to show"]);
    }

    #[test]
    fn preferences_survive_storing() {
        let mut prefs = Prefs::default();
        assert!(prefs.toggle("bars"));
        assert!(prefs.toggle("remaining"));
        assert!(!prefs.toggle("unknown"));
        assert_eq!(Prefs::restore(&prefs.store()), prefs);
        assert_eq!(Prefs::restore(""), Prefs::default());
        assert_eq!(Prefs::restore("bars=maybe\nnonsense"), Prefs::default());
    }

    #[test]
    fn services_are_listed_as_text_or_as_an_array() {
        let (known, unknown) = providers("claude, codex,claude");
        assert_eq!(known, [Provider::Claude, Provider::Codex]);
        assert!(unknown.is_empty());
        let (known, unknown) = providers(r#"["deepseek", "gemini"]"#);
        assert_eq!(known, [Provider::DeepSeek]);
        assert_eq!(unknown, ["gemini"]);
    }
}
