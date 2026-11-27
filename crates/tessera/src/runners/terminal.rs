// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The profiles of Windows Terminal, from its settings file: typing a
//! part of the name of one (`ubuntu`, `powershell`) offers to open
//! Windows Terminal with it, as `wt -p <name>` does. The settings of the
//! release from the Store, of its preview and of a release installed
//! without the Store are read; hidden profiles are left out.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::{env, fs};

use crate::platform::windows_terminal;

use super::command::Command;
use super::json::Json;
use super::{Answer, AnswerAction, Kept, Query, Runner, fill, rank, ranked_score};

const ICON_TERMINAL: &str = "\u{e756}";

/// The score of a profile whose name starts with what is typed, and how
/// many letters the search waits for.
const SCORE: u16 = 640;
const LETTERS: usize = 2;

/// How long the profiles read last are used again.
const KEPT: Duration = Duration::from_secs(30);

/// Where the settings of Windows Terminal lie, under the local
/// application data of the user.
const SETTINGS: [&str; 3] = [
    r"Packages\Microsoft.WindowsTerminal_8wekyb3d8bbwe\LocalState\settings.json",
    r"Packages\Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe\LocalState\settings.json",
    r"Microsoft\Windows Terminal\settings.json",
];

/// A profile of Windows Terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Profile {
    name: String,
    /// What it runs, where the settings say.
    command: Option<String>,
}

/// The profiles the settings `text` list, without the hidden ones. The
/// profiles are an array, or an object with the array as `list`.
fn profiles_in(text: &str) -> Vec<Profile> {
    let Some(settings) = Json::parse(text) else {
        return Vec::new();
    };
    let profiles = settings.get("profiles");
    let list = match profiles {
        Some(Json::Array(list)) => list.as_slice(),
        Some(object) => object.get("list").map_or(&[][..], Json::items),
        None => &[],
    };
    { list.iter() }
        .filter(|profile| profile.get("hidden").and_then(Json::as_bool) != Some(true))
        .filter_map(|profile| {
            Some(Profile {
                name: profile.text("name")?.to_owned(),
                command: profile.text("commandline").map(str::to_owned),
            })
        })
        .collect()
}

/// The profiles of every settings file there is, each name once, read
/// again once what was read is older than [`KEPT`].
fn profiles() -> Arc<Vec<Profile>> {
    static READ: Kept<Vec<Profile>> = Kept::new(KEPT);
    READ.get(read_profiles)
}

/// The profiles of every settings file there is, each name once, read
/// from the disk.
fn read_profiles() -> Vec<Profile> {
    let local = env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let mut found: Vec<Profile> = Vec::new();
    for file in SETTINGS
        .iter()
        .filter_map(|file| Some(local.as_ref()?.join(file)))
    {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        for profile in profiles_in(&text) {
            if !found.iter().any(|known| known.name == profile.name) {
                found.push(profile);
            }
        }
    }
    found
}

/// The arguments that open Windows Terminal with the profile `name`.
fn arguments(name: &str) -> String {
    format!("-p \"{}\"", name.replace('"', "\\\""))
}

pub struct Terminal;

impl Runner for Terminal {
    fn id(&self) -> &'static str {
        "terminal"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        if query.letters() < LETTERS {
            return Vec::new();
        }
        let profiles = profiles();
        let found: Vec<(u8, &Profile)> = { profiles.iter() }
            .filter_map(|profile| Some((rank(&profile.name, query.lower())?, profile)))
            .collect();
        if found.is_empty() {
            return Vec::new();
        }
        let Some(program) = windows_terminal() else {
            return Vec::new();
        };
        let program = program.to_string_lossy().into_owned();
        { found.into_iter() }
            .map(|(rank, profile)| {
                let arguments = arguments(&profile.name);
                let command = Command {
                    typed: format!("wt {arguments}"),
                    target: program.clone(),
                    arguments: Some(arguments),
                    folder: false,
                    terminal: false,
                };
                let title = fill(query.say("Open {} in Windows Terminal"), &[&profile.name]);
                let detail = profile
                    .command
                    .clone()
                    .unwrap_or_else(|| query.say("Terminal profile").to_owned());
                let key = format!("terminal:{}", profile.name.to_lowercase());
                Answer::new(title, detail, ICON_TERMINAL, ranked_score(SCORE, rank), {
                    AnswerAction::Run(command)
                })
                .keyed(key)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profiles_of_the_settings_are_read_without_the_hidden_ones() {
        let settings = r#"
        // This file was initially generated by Windows Terminal.
        {
            "$schema": "https://aka.ms/terminal-profiles-schema",
            "defaultProfile": "{61c54bbd-c2c6-5271-96e7-009a87ff44bf}",
            "profiles": {
                "defaults": {},
                "list": [
                    { "name": "Windows PowerShell", "commandline": "powershell.exe", "hidden": false },
                    { "name": "Command Prompt", "commandline": "cmd.exe", "hidden": false },
                    { "name": "Azure Cloud Shell", "hidden": true },
                    { "name": "Ubuntu", "source": "Windows.Terminal.Wsl" },
                ]
            },
        }"#;
        let names: Vec<String> = { profiles_in(settings).into_iter() }
            .map(|profile| profile.name)
            .collect();
        assert_eq!(names, ["Windows PowerShell", "Command Prompt", "Ubuntu"]);
        let old = r#"{ "profiles": [ { "name": "cmd", "commandline": "cmd.exe" } ] }"#;
        assert_eq!(
            profiles_in(old),
            [Profile {
                name: "cmd".to_owned(),
                command: Some("cmd.exe".to_owned())
            }]
        );
        assert!(profiles_in("not json").is_empty());
    }

    #[test]
    fn a_profile_is_named_in_quotes() {
        assert_eq!(arguments("Ubuntu 22.04"), "-p \"Ubuntu 22.04\"");
        assert_eq!(arguments("a\"b"), "-p \"a\\\"b\"");
    }
}
