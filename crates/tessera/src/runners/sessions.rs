// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The folders and workspaces opened lately in Visual Studio Code, its
//! Insiders release and VSCodium, as their `storage.json` keeps them:
//! the recent entries of the File menu, the windows open when the editor
//! was closed and those with unsaved work. Typing a part of the name of
//! a folder offers to open it in that editor again.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use std::{env, fs};

use super::command::Command;
use super::json::Json;
use super::{Answer, AnswerAction, Kept, Query, Runner, fill, rank, ranked_score};

const ICON_CODE: &str = "\u{e943}";

/// The score of a folder whose name starts with what is typed, how many
/// letters the search waits for, and how many are shown at most.
const SCORE: u16 = 620;
const LETTERS: usize = 2;
const SHOWN: usize = 12;

/// How long what was read last is used again.
const KEPT: Duration = Duration::from_secs(30);

/// The file of the state of an editor, under its folder in the
/// application data of the user.
const STORAGE: &str = r"User\globalStorage\storage.json";

/// An editor of the family of Visual Studio Code.
struct Editor {
    name: &'static str,
    /// Its folder in the roaming application data of the user.
    data: &'static str,
    /// Where its program lies, under the local application data of the
    /// user and under the program files.
    program: &'static str,
}

const EDITORS: [Editor; 3] = [
    Editor {
        name: "Visual Studio Code",
        data: "Code",
        program: r"Microsoft VS Code\Code.exe",
    },
    Editor {
        name: "Visual Studio Code Insiders",
        data: "Code - Insiders",
        program: r"Microsoft VS Code Insiders\Code - Insiders.exe",
    },
    Editor {
        name: "VSCodium",
        data: "VSCodium",
        program: r"VSCodium\VSCodium.exe",
    },
];

impl Editor {
    /// The program of the editor, where it is installed.
    fn program(&self) -> Option<PathBuf> {
        let local = env::var_os("LOCALAPPDATA").map(|local| Path::new(&local).join("Programs"));
        let machine = ["ProgramFiles", "ProgramFiles(x86)"].map(env::var_os);
        {
            local
                .into_iter()
                .chain(machine.into_iter().flatten().map(PathBuf::from))
        }
        .map(|folder| folder.join(self.program))
        .find(|program| program.is_file())
    }

    fn storage(&self) -> Option<PathBuf> {
        let roaming = env::var_os("APPDATA")?;
        Some(Path::new(&roaming).join(self.data).join(STORAGE))
    }
}

/// Something opened in an editor.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Opened {
    /// The address the editor knows it by, as `file:///c%3A/work/site`.
    uri: String,
    /// Whether it is a workspace file rather than a folder.
    workspace: bool,
}

impl Opened {
    /// What it is called: the last part of its path.
    fn name(&self) -> String {
        let path = shown_path(&self.uri);
        let name = path
            .trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next();
        let name = name.unwrap_or(&path);
        name.strip_suffix(".code-workspace")
            .unwrap_or(name)
            .to_owned()
    }
}

/// The text of `uri` with its `%` escapes undone.
fn decoded(uri: &str) -> String {
    let bytes = uri.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex =
            |at: usize| (bytes.get(at).copied()).and_then(|byte| char::from(byte).to_digit(16));
        if bytes[at] == b'%'
            && let (Some(high), Some(low)) = (hex(at + 1), hex(at + 2))
        {
            out.push((high * 16 + low) as u8);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// How `uri` is shown: the path of a local one as Windows writes it, and
/// any other address as it is, with its escapes undone.
fn shown_path(uri: &str) -> String {
    let text = decoded(uri);
    match text.strip_prefix("file:///") {
        Some(path) => {
            let path = path.replace('/', "\\");
            // The editor writes the drive letter small.
            let mut letters = path.chars();
            match (letters.next(), letters.next()) {
                (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => {
                    format!("{}{}", drive.to_ascii_uppercase(), &path[1..])
                }
                _ => path,
            }
        }
        None => match text.strip_prefix("file://") {
            Some(share) => format!(r"\\{}", share.replace('/', "\\")),
            None => text,
        },
    }
}

/// The address of a value that names something opened: a text, or an
/// object of the editor's own with `scheme`, `authority` and `path`.
fn uri_of(value: &Json) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    let scheme = value.text("scheme")?;
    let path = value.text("path")?;
    let authority = value.text("authority").unwrap_or_default();
    Some(format!("{scheme}://{authority}{path}"))
}

/// What `storage`, the text of a `storage.json`, tells was opened, the
/// latest first, each once.
fn opened_in(storage: &str) -> Vec<Opened> {
    let Some(state) = Json::parse(storage) else {
        return Vec::new();
    };
    let mut found: Vec<Opened> = Vec::new();
    let mut add = |uri: Option<String>, workspace: bool| {
        if let Some(uri) = uri
            && !{ found.iter() }
                .any(|known| decoded(&known.uri).eq_ignore_ascii_case(&decoded(&uri)))
        {
            found.push(Opened { uri, workspace });
        }
    };
    // The windows open when the editor was closed, the last one first.
    let windows = state.get("windowsState");
    let last = windows.and_then(|windows| windows.get("lastActiveWindow"));
    let open = windows.map_or(&[][..], |windows| {
        windows.get("openedWindows").map_or(&[][..], Json::items)
    });
    for window in last.into_iter().chain(open) {
        add(window.get("folder").and_then(uri_of), false);
        let workspace = window.get("workspaceIdentifier");
        add(
            workspace
                .and_then(|w| w.get("configURIPath"))
                .and_then(uri_of),
            true,
        );
    }
    // The recent entries of the File menu.
    let menus = state.at(&["lastKnownMenubarData", "menus", "File", "items"]);
    let recent = { menus.map_or(&[][..], Json::items).iter() }
        .find(|item| item.text("id") == Some("submenuitem.MenubarRecentMenu"));
    let items = recent.and_then(|recent| recent.at(&["submenu", "items"]));
    for item in items.map_or(&[][..], Json::items) {
        let workspace = match item.text("id") {
            Some("openRecentFolder") => false,
            Some("openRecentWorkspace") => true,
            _ => continue,
        };
        add(item.get("uri").and_then(uri_of), workspace);
    }
    // The windows with unsaved work.
    let backups = state.get("backupWorkspaces");
    for folder in backups.map_or(&[][..], |b| b.get("folders").map_or(&[][..], Json::items)) {
        add(folder.get("folderUri").and_then(uri_of), false);
    }
    for workspace in backups.map_or(&[][..], |b| {
        b.get("workspaces").map_or(&[][..], Json::items)
    }) {
        add(workspace.get("configURIPath").and_then(uri_of), true);
    }
    found
}

/// What was opened in each editor that is installed, with the editor
/// and its program, read again once older than [`KEPT`].
fn sessions() -> Arc<Vec<(&'static str, PathBuf, Opened)>> {
    static READ: Kept<Vec<(&'static str, PathBuf, Opened)>> = Kept::new(KEPT);
    READ.get(read_sessions)
}

/// What was opened in each editor that is installed, with the editor
/// and its program, read from the disk.
fn read_sessions() -> Vec<(&'static str, PathBuf, Opened)> {
    let mut sessions = Vec::new();
    for editor in &EDITORS {
        let (Some(storage), Some(program)) = (editor.storage(), editor.program()) else {
            continue;
        };
        let Ok(text) = fs::read_to_string(storage) else {
            continue;
        };
        for opened in opened_in(&text) {
            sessions.push((editor.name, program.clone(), opened));
        }
    }
    sessions
}

pub struct Sessions;

impl Runner for Sessions {
    fn id(&self) -> &'static str {
        "sessions"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        if query.letters() < LETTERS {
            return Vec::new();
        }
        let sessions = sessions();
        let mut found: Vec<(u8, usize, &(&str, PathBuf, Opened))> = { sessions.iter() }
            .enumerate()
            .filter_map(|(order, session)| {
                let name = session.2.name();
                let path = shown_path(&session.2.uri);
                let rank =
                    rank(&name, query.lower()).or_else(|| rank(&path, query.lower()).map(|_| 3))?;
                Some((rank, order, session))
            })
            .collect();
        found.sort_by_key(|&(rank, order, _)| (rank, order));
        { found.into_iter().take(SHOWN) }
            .map(|(rank, _, (editor, program, opened))| {
                let program = program.clone();
                let flag = if opened.workspace {
                    "--file-uri"
                } else {
                    "--folder-uri"
                };
                let arguments = format!("{flag} \"{}\"", opened.uri);
                let command = Command {
                    typed: format!("{} {arguments}", program.display()),
                    target: program.to_string_lossy().into_owned(),
                    arguments: Some(arguments),
                    folder: false,
                    terminal: false,
                };
                let title = fill(query.say("Open {} in {}"), &[&opened.name(), editor]);
                let detail = shown_path(&opened.uri);
                let key = format!("sessions:{}", opened.uri.to_lowercase());
                let score = ranked_score(SCORE, rank);
                Answer::new(title, detail, ICON_CODE, score, AnswerAction::Run(command))
                    .with_icon(program)
                    .keyed(key)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STORAGE_JSON: &str = r#"{
        "windowsState": {
            "lastActiveWindow": { "folder": "file:///c%3A/work/site", "backupPath": "x" },
            "openedWindows": [
                { "workspaceIdentifier": { "id": "1", "configURIPath": "file:///d%3A/all.code-workspace" } },
                { "folder": "file:///c%3A/work/site" }
            ]
        },
        "lastKnownMenubarData": { "menus": { "File": { "items": [
            { "id": "workbench.action.files.newUntitledFile", "label": "&&New Text File" },
            { "id": "submenuitem.MenubarRecentMenu", "label": "Open &&Recent", "submenu": { "items": [
                { "id": "openRecentFolder", "uri": { "$mid": 1, "path": "/c:/Users/me/notes", "scheme": "file" }, "enabled": true },
                { "id": "openRecentFolder", "uri": { "$mid": 1, "path": "/home/me/app", "scheme": "vscode-remote", "authority": "wsl+Ubuntu" } },
                { "id": "openRecentFile", "uri": { "$mid": 1, "path": "/c:/x.txt", "scheme": "file" } }
            ] } }
        ] } } },
        "backupWorkspaces": { "folders": [ { "folderUri": "file:///e%3A/draft" } ] }
    }"#;

    #[test]
    fn what_was_opened_is_read_from_the_state_of_the_editor() {
        let opened = opened_in(STORAGE_JSON);
        let uris: Vec<&str> = opened.iter().map(|opened| opened.uri.as_str()).collect();
        assert_eq!(
            uris,
            [
                "file:///c%3A/work/site",
                "file:///d%3A/all.code-workspace",
                "file:///c:/Users/me/notes",
                "vscode-remote://wsl+Ubuntu/home/me/app",
                "file:///e%3A/draft",
            ]
        );
        assert!(opened[1].workspace);
        assert_eq!(opened[1].name(), "all");
        assert_eq!(opened[0].name(), "site");
        assert!(opened_in("{").is_empty());
    }

    #[test]
    fn addresses_are_shown_as_paths() {
        assert_eq!(
            shown_path("file:///c%3A/work/my%20site"),
            r"C:\work\my site"
        );
        assert_eq!(shown_path("file://server/share/x"), r"\\server\share\x");
        assert_eq!(
            shown_path("vscode-remote://wsl%2BUbuntu/home/me"),
            "vscode-remote://wsl+Ubuntu/home/me"
        );
        assert_eq!(decoded("%e2%86%92 %zz"), "→ %zz");
        // Only a letter of ASCII before the colon is a drive.
        assert_eq!(shown_path("file:///é:/x"), r"é:\x");
    }
}
