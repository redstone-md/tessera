// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The sets of rules that come with Tessera, as the app templates of
//! Seelen UI: each is switched on by its name in `rule_templates` and
//! holds `[[rule]]` entries as `config.toml` writes them. The rules of the
//! user come after them and so have the last word.

use std::collections::HashMap;

use super::RuleConfig;

/// Each set by its name, with what it holds.
const TEMPLATES: [(&str, &str); 7] = [
    ("browsers", include_str!("templates/browsers.toml")),
    ("games", include_str!("templates/games.toml")),
    (
        "password-managers",
        include_str!("templates/password-managers.toml"),
    ),
    (
        "system-dialogs",
        include_str!("templates/system-dialogs.toml"),
    ),
    ("development", include_str!("templates/development.toml")),
    ("adobe", include_str!("templates/adobe.toml")),
    (
        "picture-in-picture",
        include_str!("templates/picture-in-picture.toml"),
    ),
];

/// The names of the sets there are.
pub fn names() -> impl Iterator<Item = &'static str> {
    TEMPLATES.iter().map(|&(name, _)| name)
}

/// The rules of the set `name`, or why there are none.
pub fn rules_of(name: &str) -> Result<Vec<RuleConfig>, String> {
    let (_, text) = { TEMPLATES.iter() }
        .find(|(known, _)| *known == name)
        .ok_or_else(|| {
            let known: Vec<&str> = names().collect();
            format!(
                "there is no set of rules `{name}`; there are `{}`",
                known.join("`, `")
            )
        })?;
    let mut file: HashMap<String, Vec<RuleConfig>> =
        toml::from_str(text).map_err(|error| format!("the set `{name}` is broken: {error}"))?;
    Ok(file.remove("rule").unwrap_or_default())
}

/// The rules of the set `name` as the tables its file writes, for the
/// editor of the rules to show and copy; none for a set there is not.
pub fn tables_of(name: &str) -> Vec<toml::Table> {
    let Some((_, text)) = TEMPLATES.iter().find(|(known, _)| *known == name) else {
        return Vec::new();
    };
    let file: toml::Table = toml::from_str(text).unwrap_or_default();
    {
        file.get("rule")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
    }
    .filter_map(|rule| rule.as_table().cloned())
    .collect()
}

/// The rules of the sets `names`, in their order, each named once; a name
/// that is no set is reported and left out.
pub fn rules_of_all(names: &[String]) -> Vec<RuleConfig> {
    let mut rules = Vec::new();
    for (index, name) in names.iter().enumerate() {
        if names[..index].contains(name) {
            continue;
        }
        match rules_of(name) {
            Ok(more) => rules.extend(more),
            Err(error) => report!("`rule_templates`: {error}"),
        }
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::matching::Named;

    #[test]
    fn every_set_is_read_and_every_rule_in_it_can_be_used() {
        for name in names() {
            let rules = rules_of(name).unwrap();
            assert!(!rules.is_empty(), "`{name}` holds no rules");
            for rule in &rules {
                assert_eq!(rule.fault(), None, "a rule of `{name}`");
            }
        }
        assert!(rules_of("nothing").is_err());
        for name in names() {
            assert_eq!(tables_of(name).len(), rules_of(name).unwrap().len());
        }
        assert!(tables_of("nothing").is_empty());
    }

    #[test]
    fn the_sets_name_the_windows_they_are_for() {
        let rules = rules_of_all(&["browsers".to_owned(), "picture-in-picture".to_owned()]);
        let window = |exe: &str, title: &str| Named {
            exe: Some(exe.to_owned()),
            path: None,
            app: None,
            title: title.to_owned(),
            class: String::new(),
            kind: None,
        };
        let page = super::super::Effective::of(&rules, &window("chrome.exe", "New Tab"));
        assert_eq!(page.category, Some("browser"));
        assert_eq!(page.above, None);
        let video =
            super::super::Effective::of(&rules, &window("chrome.exe", "Picture in picture"));
        assert_eq!(video.above, Some(true));
        assert_eq!(video.tile, Some(false));
        let firefox =
            super::super::Effective::of(&rules, &window("firefox.exe", "Picture-in-Picture"));
        assert_eq!(firefox.above, Some(true));
        let adobe = rules_of("adobe").unwrap();
        let photoshop = Named {
            path: Some(r"C:\Program Files\Adobe\Adobe Photoshop 2026\Photoshop.exe".to_owned()),
            ..window("Photoshop.exe", "Untitled-1")
        };
        assert_eq!(
            super::super::Effective::of(&adobe, &photoshop).tile,
            Some(false)
        );
    }
}
