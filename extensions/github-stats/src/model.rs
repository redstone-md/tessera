// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The repositories asked about and how GitHub's answers read.

use serde_json::Value;
use tessera_extension_kit::settings::parse_list;

/// Reads repositories written as `rust-lang/rust, owner/name`.
pub fn parse_repos(text: &str) -> Result<Vec<String>, String> {
    let repos = parse_list(text);
    if repos.is_empty() {
        return Err("Name repositories in the setting `repos`, as \"rust-lang/rust\".".to_owned());
    }
    let part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    };
    for repo in &repos {
        let fine = repo
            .split_once('/')
            .is_some_and(|(owner, name)| part(owner) && part(name) && name != "." && name != "..");
        if !fine {
            return Err(format!(
                "`{repo}` is not a repository written as owner/name."
            ));
        }
    }
    Ok(repos)
}

pub fn address(repo: &str) -> String {
    format!("https://api.github.com/repos/{repo}")
}

/// The counts of a repository.
#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub stars: u64,
    pub forks: u64,
    /// Open issues and pull requests, which GitHub counts together.
    pub issues: u64,
}

impl Stats {
    pub fn read(body: &str) -> Result<Self, String> {
        let answer: Value =
            serde_json::from_str(body).map_err(|_| "GitHub's answer is unreadable.".to_owned())?;
        let count = |key: &str| {
            answer[key]
                .as_u64()
                .ok_or_else(|| "GitHub gave no counts for it.".to_owned())
        };
        Ok(Self {
            stars: count("stargazers_count")?,
            forks: count("forks_count")?,
            issues: count("open_issues_count")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_are_checked() {
        let repos = parse_repos("rust-lang/rust; redstone-md/tessera").unwrap();
        assert_eq!(repos, ["rust-lang/rust", "redstone-md/tessera"]);
        assert_eq!(
            address(&repos[0]),
            "https://api.github.com/repos/rust-lang/rust"
        );
        for wrong in ["rust", "a/b/c", "a/..", "a b/c", "/x"] {
            assert!(parse_repos(wrong).is_err(), "{wrong}");
        }
        assert!(parse_repos(" ").is_err());
    }

    #[test]
    fn counts_are_read() {
        let body = r#"{"full_name":"a/b","stargazers_count":98123,"forks_count":12900,"open_issues_count":9876}"#;
        let stats = Stats::read(body).unwrap();
        assert_eq!(
            (stats.stars, stats.forks, stats.issues),
            (98_123, 12_900, 9876)
        );
        assert!(Stats::read(r#"{"message":"Not Found"}"#).is_err());
    }
}
