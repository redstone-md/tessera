// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Which shell the terminal runs, what kind of shell it is, and how it is
//! sent to another folder: `cd /d` for cmd, `Set-Location -LiteralPath`
//! for PowerShell and `cd` with the path as WSL has it for the shells of
//! Linux, each after the line typed so far is cleared. Also the paths of
//! Windows a shell of Linux reports, and the other way round.

use std::path::{Path, PathBuf};

use crate::platform::{find_program, wsl_distributions};

/// The variable cmd reads its prompt from.
const PROMPT: &str = "PROMPT";

/// The prompt cmd is given where the user set none: the usual one, after
/// an OSC 9;9 that tells the terminal the folder.
const CMD_PROMPT: &str = "$e]9;9;$P$e\\$P$G";

/// Where WSL shows the files of a distribution to Windows.
const WSL_ROOTS: [&str; 2] = [r"\\wsl.localhost\", r"\\wsl$\"];

/// What kind of shell runs, which says how it changes folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    Cmd,
    PowerShell,
    /// A shell of Linux, in WSL.
    Posix,
    /// Another program, which is never sent anywhere.
    Other,
}

/// The shell the terminal starts, and what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellPlan {
    /// The command line it starts with.
    pub command_line: String,
    pub kind: ShellKind,
    /// The distribution of WSL it runs in, where it is known.
    pub distro: Option<String>,
    /// Variables of the environment given to it beside those of Tessera.
    pub environment: Vec<(String, String)>,
}

impl ShellPlan {
    /// The shell `setting` names, as the configuration writes it: none for
    /// PowerShell 7 where `pwsh` (where it is found) is given and Windows
    /// PowerShell otherwise; `pwsh`, `powershell`, `cmd`, `wsl` or
    /// `wsl:<distribution>`; or else a command line of its own.
    pub fn of(setting: Option<&str>, pwsh: Option<&Path>) -> Self {
        let quoted = |program: &Path| format!("\"{}\" -NoLogo", program.display());
        let windows_powershell = || "powershell.exe -NoLogo".to_owned();
        let setting = setting.map(str::trim).filter(|setting| !setting.is_empty());
        let (command_line, distro) = match setting {
            None => (pwsh.map_or_else(windows_powershell, quoted), None),
            Some(name) if name.eq_ignore_ascii_case("pwsh") => (
                pwsh.map_or_else(|| "pwsh.exe -NoLogo".to_owned(), quoted),
                None,
            ),
            Some(name) if name.eq_ignore_ascii_case("powershell") => (windows_powershell(), None),
            Some(name) if name.eq_ignore_ascii_case("cmd") => ("cmd.exe".to_owned(), None),
            Some(name) if name.eq_ignore_ascii_case("wsl") => ("wsl.exe".to_owned(), None),
            Some(name) if name.len() > 4 && name[..4].eq_ignore_ascii_case("wsl:") => {
                let distro = name[4..].trim().to_owned();
                (format!("wsl.exe -d \"{distro}\""), Some(distro))
            }
            Some(line) => (line.to_owned(), distro_in(line)),
        };
        let kind = kind_of(program_of(&command_line));
        Self {
            command_line,
            kind,
            distro,
            environment: vec![("COLORTERM".to_owned(), "truecolor".to_owned())],
        }
    }

    /// The shell `setting` names, as [`Self::of`] has it, with PowerShell 7
    /// looked for on the disk, cmd given a prompt that tells the folder
    /// where the user set none, and the one distribution of WSL there is
    /// taken for a shell of Linux that names none. Blocks on the disk and
    /// the registry: not for the thread of GPUI.
    pub fn resolve(setting: Option<&str>) -> Self {
        let pwsh = find_program("pwsh.exe").or_else(|| {
            let programs = std::env::var_os("ProgramFiles")?;
            let path = PathBuf::from(programs).join(r"PowerShell\7\pwsh.exe");
            path.is_file().then_some(path)
        });
        let mut plan = Self::of(setting, pwsh.as_deref());
        if plan.kind == ShellKind::Cmd && std::env::var_os(PROMPT).is_none() {
            plan.environment
                .push((PROMPT.to_owned(), CMD_PROMPT.to_owned()));
        }
        if plan.kind == ShellKind::Posix && plan.distro.is_none() {
            let distributions = wsl_distributions();
            if let [only] = distributions.as_slice() {
                plan.distro = Some(only.name.clone());
            }
        }
        plan
    }

    /// What sends the shell to `folder`: the line typed so far cleared,
    /// then the command that changes the folder and Enter. None where the
    /// shell is of no kind that is sent, or the folder cannot be written
    /// for it.
    pub fn going_to(&self, folder: &Path) -> Option<Vec<u8>> {
        let path = folder.to_str()?;
        let line = match self.kind {
            // A `%` could stand for a variable that cmd puts in its place.
            ShellKind::Cmd if path.contains(['%', '"']) => return None,
            // Escape clears what cmd has on its line.
            ShellKind::Cmd => format!("\x1bcd /d \"{path}\"\r"),
            // Escape clears the line of PSReadLine.
            ShellKind::PowerShell => format!(
                "\x1bSet-Location -LiteralPath '{}'\r",
                path.replace('\'', "''")
            ),
            // Ctrl+E Ctrl+U clear the line; a space first keeps the
            // command out of the history where the shell is set so.
            ShellKind::Posix => {
                let path = posix_path(folder)?;
                format!("\x05\x15 cd '{}'\r", path.replace('\'', "'\\''"))
            }
            ShellKind::Other => return None,
        };
        Some(line.into_bytes())
    }

    /// The folder of Windows a folder the shell reported names: as cmd and
    /// PowerShell write it, as an address of OSC 7 writes it (`/C:/Users`),
    /// a drive under `/mnt` of WSL, or another folder of WSL inside the
    /// distribution of the shell, where that is known.
    pub fn windows_folder(&self, reported: &str) -> Option<PathBuf> {
        windows_path(reported, self.distro.as_deref())
    }
}

/// The program a command line starts: its first word, without quotes.
fn program_of(line: &str) -> &str {
    let line = line.trim_start();
    match line.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default(),
        None => line.split_whitespace().next().unwrap_or_default(),
    }
}

/// What kind of shell `program` is, by the name of its file.
fn kind_of(program: &str) -> ShellKind {
    let name = { program.rsplit(['\\', '/']).next() }
        .unwrap_or_default()
        .to_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    const LINUX: [&str; 7] = [
        "ubuntu", "debian", "kali", "opensuse", "sles", "alpine", "fedora",
    ];
    match stem {
        "cmd" => ShellKind::Cmd,
        "powershell" | "pwsh" => ShellKind::PowerShell,
        "wsl" | "bash" | "zsh" | "fish" | "sh" => ShellKind::Posix,
        stem if LINUX.iter().any(|linux| stem.starts_with(linux)) => ShellKind::Posix,
        _ => ShellKind::Other,
    }
}

/// The distribution a command line of `wsl` names with `-d` or
/// `--distribution`.
fn distro_in(line: &str) -> Option<String> {
    if kind_of(program_of(line)) != ShellKind::Posix {
        return None;
    }
    let mut words = line.split_whitespace();
    while let Some(word) = words.next() {
        if word == "-d" || word == "--distribution" {
            return words.next().map(|name| name.trim_matches('"').to_owned());
        }
    }
    None
}

/// Whether two paths name one folder, as Windows compares them: in any
/// case, with or without a separator at the end.
pub fn same_folder(one: &Path, other: &Path) -> bool {
    let plain = |path: &Path| {
        let text = path.to_string_lossy().replace('/', "\\").to_lowercase();
        match text.trim_end_matches('\\') {
            // The root of a drive keeps its separator.
            root if root.len() == 2 && root.ends_with(':') => format!("{root}\\"),
            trimmed => trimmed.to_owned(),
        }
    };
    plain(one) == plain(other)
}

/// `folder` as a shell of WSL names it: a drive under `/mnt`, and a folder
/// of a distribution inside it. None for a share of the network.
pub fn posix_path(folder: &Path) -> Option<String> {
    let text = folder.to_str()?;
    for root in WSL_ROOTS {
        if text.len() > root.len() && text[..root.len()].eq_ignore_ascii_case(root) {
            let inside = &text[root.len()..];
            let path = inside.split_once('\\').map_or("", |(_, path)| path);
            return Some(format!("/{}", path.replace('\\', "/")));
        }
    }
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = text[2..].trim_start_matches('\\').replace('\\', "/");
        return Some(match rest.is_empty() {
            true => format!("/mnt/{drive}"),
            false => format!("/mnt/{drive}/{rest}"),
        });
    }
    None
}

/// The folder of Windows that `reported`, a folder a shell wrote, names;
/// a folder of Linux other than a drive under `/mnt` only with the
/// `distro` it is in.
pub fn windows_path(reported: &str, distro: Option<&str>) -> Option<PathBuf> {
    let reported = reported.trim();
    let drive_path = |text: &str| -> Option<PathBuf> {
        let bytes = text.as_bytes();
        let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        drive.then(|| {
            let rest = text[2..].trim_start_matches(['\\', '/']).replace('/', "\\");
            PathBuf::from(format!(
                "{}:\\{rest}",
                (bytes[0] as char).to_ascii_uppercase()
            ))
        })
    };
    if reported.starts_with(r"\\") {
        return Some(PathBuf::from(reported));
    }
    if let Some(path) = drive_path(reported) {
        return Some(path);
    }
    let posix = reported.strip_prefix('/')?;
    // `/C:/Users`, as an address of OSC 7 from Windows writes it.
    if let Some(path) = drive_path(posix) {
        return Some(path);
    }
    if let Some(mounted) = posix.strip_prefix("mnt/") {
        let (drive, rest) = mounted.split_once('/').unwrap_or((mounted, ""));
        if drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic() {
            return drive_path(&format!("{drive}:/{rest}"));
        }
    }
    let distro = distro?;
    let inside = posix.trim_end_matches('/').replace('/', "\\");
    Some(PathBuf::from(format!("{}{distro}\\{inside}", WSL_ROOTS[0])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_names_a_shell_or_is_a_command_line() {
        let pwsh = Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe");
        let plan = ShellPlan::of(None, Some(pwsh));
        assert_eq!(
            plan.command_line,
            r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo"#
        );
        assert_eq!(plan.kind, ShellKind::PowerShell);
        assert_eq!(
            ShellPlan::of(None, None).command_line,
            "powershell.exe -NoLogo"
        );
        assert_eq!(ShellPlan::of(Some("CMD"), None).kind, ShellKind::Cmd);
        let wsl = ShellPlan::of(Some("wsl:Ubuntu-22.04"), None);
        assert_eq!(wsl.command_line, "wsl.exe -d \"Ubuntu-22.04\"");
        assert_eq!(
            (wsl.kind, wsl.distro.as_deref()),
            (ShellKind::Posix, Some("Ubuntu-22.04"))
        );
        let line = ShellPlan::of(Some(r#""C:\tools\nu.exe" --login"#), None);
        assert_eq!(line.kind, ShellKind::Other);
        let named = ShellPlan::of(Some("wsl.exe --distribution Debian"), None);
        assert_eq!(named.distro.as_deref(), Some("Debian"));
        assert_eq!(
            ShellPlan::of(Some("ubuntu2204.exe"), None).kind,
            ShellKind::Posix
        );
    }

    #[test]
    fn each_shell_is_sent_to_a_folder_its_own_way() {
        let folder = Path::new(r"C:\Users\me\it's here");
        let command = |kind| {
            let plan = ShellPlan {
                command_line: String::new(),
                kind,
                distro: None,
                environment: Vec::new(),
            };
            plan.going_to(folder)
                .map(|bytes| String::from_utf8(bytes).unwrap())
        };
        assert_eq!(
            command(ShellKind::Cmd).as_deref(),
            Some("\x1bcd /d \"C:\\Users\\me\\it's here\"\r")
        );
        assert_eq!(
            command(ShellKind::PowerShell).as_deref(),
            Some("\x1bSet-Location -LiteralPath 'C:\\Users\\me\\it''s here'\r")
        );
        assert_eq!(
            command(ShellKind::Posix).as_deref(),
            Some("\x05\x15 cd '/mnt/c/Users/me/it'\\''s here'\r")
        );
        assert_eq!(command(ShellKind::Other), None);
        let plan = ShellPlan::of(Some("cmd"), None);
        assert_eq!(plan.going_to(Path::new(r"C:\100%")), None);
    }

    #[test]
    fn folders_are_the_same_in_any_case_and_with_a_separator_at_the_end() {
        assert!(same_folder(
            Path::new(r"C:\Users\Me\"),
            Path::new(r"c:\users\me")
        ));
        assert!(same_folder(Path::new("C:/a"), Path::new(r"C:\a")));
        assert!(same_folder(Path::new(r"D:\"), Path::new("D:")));
        assert!(!same_folder(Path::new(r"C:\a"), Path::new(r"C:\ab")));
    }

    #[test]
    fn folders_of_windows_and_of_wsl_name_each_other() {
        assert_eq!(posix_path(Path::new(r"D:\")).as_deref(), Some("/mnt/d"));
        assert_eq!(
            posix_path(Path::new(r"\\wsl.localhost\Ubuntu\home\me")).as_deref(),
            Some("/home/me")
        );
        assert_eq!(
            posix_path(Path::new(r"\\WSL$\Ubuntu")).as_deref(),
            Some("/")
        );
        assert_eq!(posix_path(Path::new(r"\\server\share")), None);
        let windows = |text, distro| windows_path(text, distro);
        assert_eq!(
            windows("/C:/Users/me", None),
            Some(PathBuf::from(r"C:\Users\me"))
        );
        assert_eq!(windows("c:\\work", None), Some(PathBuf::from(r"C:\work")));
        assert_eq!(
            windows("/mnt/d/projects", None),
            Some(PathBuf::from(r"D:\projects"))
        );
        assert_eq!(windows("/mnt/e", None), Some(PathBuf::from(r"E:\")));
        assert_eq!(
            windows("/home/me/", Some("Ubuntu")),
            Some(PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me"))
        );
        assert_eq!(windows("/home/me", None), None);
        assert_eq!(
            windows(r"\\server\share", None),
            Some(PathBuf::from(r"\\server\share"))
        );
    }
}
