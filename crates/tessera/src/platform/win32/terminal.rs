// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The terminal that Tessera opens, for the menu of Win+X, the `>` of
//! the launcher and "Open in terminal" of the file manager: Windows
//! Terminal where it is installed, or else PowerShell, or the command
//! prompt where a command line is to run, as the lines are written for
//! it.

use std::env;
use std::path::{Path, PathBuf};

use super::{Launch, find_program};

/// Where Windows Terminal is started from: the alias of the application
/// that Windows puts on `PATH`, or the program its App Paths key names;
/// `None` where it is not installed. It reads the registry and the disk:
/// not for the thread of GPUI.
pub fn windows_terminal() -> Option<PathBuf> {
    find_program("wt.exe").or_else(|| {
        // The alias is a link that only Windows can follow, so it is
        // looked at itself, for a `PATH` that leaves its folder out.
        let local = PathBuf::from(env::var_os("LOCALAPPDATA")?);
        let alias = local.join(r"Microsoft\WindowsApps\wt.exe");
        alias.symlink_metadata().is_ok().then_some(alias)
    })
}

/// What opens a terminal in `folder` (where it is given), running `line`
/// of the command prompt and staying open (where it is given), and with
/// `admin` as an administrator. It looks for Windows Terminal on the
/// disk: not for the thread of GPUI.
pub fn terminal(folder: Option<&Path>, line: Option<&str>, admin: bool) -> Launch {
    let found = windows_terminal();
    let comspec = env::var("ComSpec").ok();
    let (program, arguments) = planned(found, comspec, folder, line);
    let mut launch = Launch::new(program);
    if !arguments.is_empty() {
        launch = launch.arguments(arguments);
    }
    if let Some(folder) = folder {
        launch = launch.directory(folder);
    }
    if let Some(line) = line {
        launch = launch.named(format!("`{line}`"));
    }
    match admin {
        true => launch.verb("runas"),
        false => launch,
    }
}

/// The program and its arguments for [`terminal`], with Windows Terminal
/// at `found` and the command prompt at `comspec` where they are known.
/// The folder is given in the arguments too, as an elevated program does
/// not always start in the folder it is handed.
fn planned(
    found: Option<PathBuf>,
    comspec: Option<String>,
    folder: Option<&Path>,
    line: Option<&str>,
) -> (PathBuf, String) {
    let folder = folder.map(|folder| folder.to_string_lossy().into_owned());
    if let Some(found) = found {
        let mut arguments = Vec::new();
        if let Some(folder) = &folder {
            arguments.push(format!("-d {}", quoted(&tab_safe(folder))));
        }
        if let Some(line) = line {
            arguments.push(format!("cmd.exe /k {}", tab_safe(line)));
        }
        return (found, arguments.join(" "));
    }
    if let Some(line) = line {
        let shell = comspec.unwrap_or_else(|| "cmd.exe".to_owned());
        let arguments = match &folder {
            Some(folder) => format!("/k cd /d {} && {line}", quoted(folder)),
            None => format!("/k {line}"),
        };
        return (PathBuf::from(shell), arguments);
    }
    let arguments = match &folder {
        Some(folder) => format!(
            "-NoExit -Command \"Set-Location -LiteralPath '{}'\"",
            folder.replace('\'', "''")
        ),
        None => String::new(),
    };
    (PathBuf::from("powershell.exe"), arguments)
}

/// `text` with every `;` escaped, as Windows Terminal would start
/// another tab at a bare one.
fn tab_safe(text: &str) -> String {
    text.replace(';', "\\;")
}

/// `path` in quotes, as one argument: a path has no `"`, but a `\` at
/// its end would escape the closing quote and is doubled.
fn quoted(path: &str) -> String {
    match path.ends_with('\\') {
        true => format!("\"{path}\\\""),
        false => format!("\"{path}\""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt() -> Option<PathBuf> {
        Some(PathBuf::from(
            r"C:\Users\Me\AppData\Local\Microsoft\WindowsApps\wt.exe",
        ))
    }

    #[test]
    fn windows_terminal_opens_in_the_folder_and_runs_the_line() {
        let (program, arguments) = planned(wt(), None, Some(Path::new(r"C:\")), None);
        assert_eq!(program, wt().unwrap());
        assert_eq!(arguments, r#"-d "C:\\""#);
        let folder = Path::new(r"D:\a;b");
        let (_, arguments) = planned(wt(), None, Some(folder), Some("dir & echo; done"));
        assert_eq!(arguments, r#"-d "D:\a\;b" cmd.exe /k dir & echo\; done"#);
        assert_eq!(planned(wt(), None, None, None).1, "");
    }

    #[test]
    fn without_windows_terminal_a_line_runs_in_the_command_prompt() {
        let comspec = Some(r"C:\Windows\system32\cmd.exe".to_owned());
        let (program, arguments) = planned(None, comspec, Some(Path::new(r"D:\x y")), Some("ver"));
        assert_eq!(program, PathBuf::from(r"C:\Windows\system32\cmd.exe"));
        assert_eq!(arguments, r#"/k cd /d "D:\x y" && ver"#);
        let (program, arguments) = planned(None, None, None, Some("ver"));
        assert_eq!(
            (program, arguments),
            (PathBuf::from("cmd.exe"), "/k ver".to_owned())
        );
    }

    #[test]
    fn without_windows_terminal_and_a_line_powershell_opens() {
        let (program, arguments) = planned(None, None, Some(Path::new(r"C:\Bob's")), None);
        assert_eq!(program, PathBuf::from("powershell.exe"));
        assert_eq!(
            arguments,
            r#"-NoExit -Command "Set-Location -LiteralPath 'C:\Bob''s'""#
        );
        assert_eq!(planned(None, None, None, None).1, "");
    }
}
