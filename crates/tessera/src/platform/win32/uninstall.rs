// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Removing an application of the Start menu, as the Start menu of
//! Windows does it: a packaged one is removed for the user by the package
//! manager, a program is removed by the uninstaller it registered under
//! the `Uninstall` keys of the registry, and one that cannot be told is
//! left to the page of the installed applications of the Settings app.
//! Also the other things that menu does with an application: its page of
//! the Settings app and a shortcut to it on the desktop.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use async_channel::Receiver;
use windows::Management::Deployment::PackageManager;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_SAM_FLAGS,
};
use windows::Win32::UI::Shell::{ILFree, IShellLinkW, SHParseDisplayName, ShellLink};
use windows::core::{HSTRING, Interface};

use super::account::Place;
use super::execute::Launch;
use super::file_op::unique_name;
use super::packaged::app_id_in;
use super::registry::Key;
use super::shell_thread::ShellThread;
use super::shortcuts::Shortcut;

/// Where programs register how they are removed, under the user and the
/// machine.
const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

/// The page of the Settings app with the installed applications.
const APPS_PAGE: &str = "ms-settings:appsfeatures";

/// A program as its `Uninstall` key tells of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    /// The folder it is installed in, where it says.
    pub location: Option<PathBuf>,
    /// The program file of its icon, which is often its own.
    pub icon: Option<PathBuf>,
    /// The command that removes it.
    pub uninstall: String,
}

/// How an application is removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Removal {
    /// A packaged application, by the family of its package.
    Package { family: String },
    /// A program, by the command it registered.
    Program(Installed),
    /// Not known: the page of the installed applications is opened.
    Settings,
}

impl Removal {
    /// Removes the application `shortcut` starts, on a thread of its own,
    /// as [`Self::perform`] says. The receiver tells once how it went:
    /// how the application was removed, or why it was not. Await it from
    /// a task; never block the thread of GPUI on it.
    pub fn remove(shortcut: Shortcut) -> Receiver<Result<Self, String>> {
        ShellThread::call_alone("uninstall", move || {
            let removal = Self::of(&shortcut);
            let done = removal.perform().map(|()| removal);
            done.map_err(|error| format!("{error:#}"))
        })
    }

    /// How the application `shortcut` starts is removed. It reads the
    /// shortcut, with COM initialized on the calling thread, and the
    /// registry: not for the thread of GPUI.
    fn of(shortcut: &Shortcut) -> Self {
        if let Some(id) = app_id_in(&shortcut.path) {
            return match id.split_once('!') {
                Some((family, _)) => Self::Package {
                    family: family.to_owned(),
                },
                None => Self::Settings,
            };
        }
        let target = match is_link(&shortcut.path) {
            true => shortcut.link().target,
            false => Some(shortcut.path.clone()),
        };
        let installed = installed_programs();
        match best_match(&installed, &shortcut.name, target.as_deref()) {
            Some(program) => Self::Program(program.clone()),
            None => Self::Settings,
        }
    }

    /// Removes the application: a package at once, for the user, waiting
    /// until it is gone; a program by starting its uninstaller, which
    /// asks the user itself; otherwise the Settings app opens. It may
    /// wait long: not for the thread of GPUI.
    fn perform(&self) -> Result<()> {
        match self {
            Self::Package { family } => remove_package(family),
            Self::Program(program) => {
                let (target, arguments) = split_command(&program.uninstall);
                let target = target.context("the uninstaller is not named")?;
                Launch::new(target)
                    .arguments(arguments)
                    .named(format!("the uninstaller of {}", program.name))
                    .start();
                Ok(())
            }
            Self::Settings => {
                Launch::new(APPS_PAGE).start();
                Ok(())
            }
        }
    }
}

/// Whether `path` is that of a shortcut file.
fn is_link(path: &Path) -> bool {
    { path.extension() }.is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
}

/// Removes every package of the family `family` the user has.
fn remove_package(family: &str) -> Result<()> {
    let manager = PackageManager::new().context("the package manager is missing")?;
    let packages = manager
        .FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(family))
        .context("the package is not found")?;
    let mut removed = false;
    for package in packages {
        let full_name = package.Id()?.FullName()?;
        let operation = manager.RemovePackageAsync(&full_name)?;
        let result = operation
            .get()
            .map_err(|error| anyhow!("{}", error.message()))?;
        let failed = result.ExtendedErrorCode()?;
        if failed.is_err() {
            let text = result
                .ErrorText()
                .map(|text| text.to_string())
                .unwrap_or_default();
            bail!("{family} was not removed: {text} ({failed:?})");
        }
        removed = true;
    }
    if !removed {
        bail!("the user has no package of {family}");
    }
    Ok(())
}

/// The programs registered under the `Uninstall` keys of the user and of
/// the machine, in both views of the registry, without the parts of the
/// system and the updates.
fn installed_programs() -> Vec<Installed> {
    let roots: [(HKEY, REG_SAM_FLAGS); 4] = [
        (HKEY_CURRENT_USER, REG_SAM_FLAGS(0)),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
        (HKEY_CURRENT_USER, KEY_WOW64_32KEY),
    ];
    let mut found: Vec<Installed> = Vec::new();
    for (root, view) in roots {
        let Some(key) = Key::read(root, UNINSTALL, view) else {
            continue;
        };
        for name in key.subkeys() {
            let Some(program) = key.child(&name) else {
                continue;
            };
            let hidden = program.number("SystemComponent") == Some(1)
                || program.text("ParentKeyName").is_some()
                || program.text("ReleaseType").is_some();
            let (Some(name), Some(uninstall)) =
                (program.text("DisplayName"), program.text("UninstallString"))
            else {
                continue;
            };
            if hidden || uninstall.trim().is_empty() {
                continue;
            }
            let location = { program.expanded_text("InstallLocation") }
                .map(|location| location.trim().trim_matches('"').to_owned())
                .filter(|location| !location.is_empty())
                .map(PathBuf::from);
            let icon = program
                .expanded_text("DisplayIcon")
                .map(|icon| icon_file(&icon));
            let installed = Installed {
                name,
                location,
                icon,
                uninstall,
            };
            if !found.contains(&installed) {
                found.push(installed);
            }
        }
    }
    found
}

/// The file of an icon as `DisplayIcon` names it, without the quotes and
/// the number of the icon after a comma.
fn icon_file(written: &str) -> PathBuf {
    let written = written.trim();
    let written = match written.rsplit_once(',') {
        Some((file, index)) if index.trim().parse::<i32>().is_ok() => file,
        _ => written,
    };
    PathBuf::from(written.trim().trim_matches('"'))
}

/// The folders many programs share, which a program that names one as
/// its folder does not hold all that lies there.
const SHARED_FOLDERS: [&str; 10] = [
    "program files",
    "program files (x86)",
    "programdata",
    "programs",
    "appdata",
    "local",
    "roaming",
    "windows",
    "system32",
    "users",
];

/// Whether `folder` is the root of a drive or a folder many programs
/// share.
fn is_shared(folder: &Path) -> bool {
    let plain = folder.to_string_lossy().replace('/', "\\");
    let plain = plain.trim_end_matches('\\');
    match plain.rsplit_once('\\') {
        Some((_, last)) => SHARED_FOLDERS.contains(&last.to_lowercase().as_str()),
        None => true,
    }
}

/// Whether `inner` is `outer` or lies below it, as Windows compares paths.
fn lies_in(inner: &Path, outer: &Path) -> bool {
    let plain = |path: &Path| {
        let path = path.to_string_lossy().replace('/', "\\").to_lowercase();
        path.trim_end_matches('\\').to_owned()
    };
    let (inner, outer) = (plain(inner), plain(outer));
    !outer.is_empty() && (inner == outer || inner.starts_with(&format!("{outer}\\")))
}

/// The program of `installed` that the application called `name`, whose
/// shortcut opens `target`, most likely is: the one whose folder holds
/// the target, the deepest folder first; else the one whose icon is the
/// target; else the one of the same name, or whose name starts with the
/// name of the application or the other way round.
pub fn best_match<'a>(
    installed: &'a [Installed],
    name: &str,
    target: Option<&Path>,
) -> Option<&'a Installed> {
    if let Some(target) = target {
        let holding = { installed.iter() }
            .filter(|program| {
                let location = program.location.as_deref();
                location.is_some_and(|at| !is_shared(at) && lies_in(target, at))
            })
            .max_by_key(|program| {
                program
                    .location
                    .as_ref()
                    .map_or(0, |at| at.as_os_str().len())
            });
        if holding.is_some() {
            return holding;
        }
        let drawn = { installed.iter() }.find(|program| {
            program
                .icon
                .as_deref()
                .is_some_and(|icon| lies_in(target, icon))
        });
        if drawn.is_some() {
            return drawn;
        }
    }
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return None;
    }
    let same = installed
        .iter()
        .find(|program| program.name.to_lowercase() == name);
    same.or_else(|| {
        installed.iter().find(|program| {
            let program = program.name.to_lowercase();
            program.starts_with(&format!("{name} ")) || name.starts_with(&format!("{program} "))
        })
    })
}

/// The program and the arguments of a command line as the `Uninstall`
/// keys write it: the program in quotes, or the longest start that ends
/// in `.exe`, as many write paths with spaces without quotes, or the
/// first word.
pub fn split_command(line: &str) -> (Option<String>, String) {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix('"') {
        return match rest.split_once('"') {
            Some((program, arguments)) => (Some(program.to_owned()), arguments.trim().to_owned()),
            None => (Some(rest.to_owned()), String::new()),
        };
    }
    // Lowered in ASCII alone, so that its places are those of `line`.
    let lower = line.to_ascii_lowercase();
    let ends = { lower.match_indices(".exe") }
        .map(|(at, _)| at + 4)
        .filter(|&end| lower[end..].is_empty() || lower[end..].starts_with(' '));
    if let Some(end) = ends.last() {
        return (Some(line[..end].to_owned()), line[end..].trim().to_owned());
    }
    match line.split_once(' ') {
        Some((program, arguments)) => (Some(program.to_owned()), arguments.trim().to_owned()),
        None => ((!line.is_empty()).then(|| line.to_owned()), String::new()),
    }
}

/// The page of the Settings app of the packaged application `app_id`;
/// `None` for an application that has no package.
pub fn app_settings(app_id: &str) -> Option<Launch> {
    let (family, _) = app_id.split_once('!')?;
    Some(Launch::new(format!("{APPS_PAGE}-app?{family}")))
}

/// Puts a shortcut to the application `shortcut` starts on the desktop of
/// the user, named as the application, on the shell thread: a copy of a
/// shortcut file, or a new shortcut to a program or a packaged
/// application. The receiver tells once where it was put, or why not.
pub fn put_on_desktop(shortcut: Shortcut) -> Receiver<Result<PathBuf, String>> {
    ShellThread::call(move || place_on_desktop(&shortcut).map_err(|error| format!("{error:#}")))
}

/// What [`put_on_desktop`] does, on the calling thread, where COM must
/// be initialized.
fn place_on_desktop(shortcut: &Shortcut) -> Result<PathBuf> {
    let desktop = Place::Desktop.path().context("the desktop has no folder")?;
    let file = unique_name(&format!("{}.lnk", shortcut.name), |name| {
        desktop.join(name).exists()
    });
    let placed = desktop.join(file);
    if is_link(&shortcut.path) {
        std::fs::copy(&shortcut.path, &placed).context("the shortcut was not copied")?;
        return Ok(placed);
    }
    // SAFETY: COM is initialized on this thread; the names outlive the
    // calls, and the list of items made from the name is freed once the
    // shortcut has it.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        if app_id_in(&shortcut.path).is_some() {
            let name = HSTRING::from(shortcut.path.to_string_lossy().replace('/', "\\"));
            let mut items = std::ptr::null_mut();
            SHParseDisplayName(&name, None, &raw mut items, 0, None)
                .context("the application is not found")?;
            let set = link.SetIDList(items);
            ILFree(Some(items));
            set?;
        } else {
            link.SetPath(&HSTRING::from(shortcut.path.as_os_str()))?;
        }
        link.cast::<IPersistFile>()?
            .Save(&HSTRING::from(placed.as_os_str()), true)
            .context("the shortcut was not saved")?;
    }
    Ok(placed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(name: &str, location: Option<&str>, icon: Option<&str>) -> Installed {
        Installed {
            name: name.to_owned(),
            location: location.map(PathBuf::from),
            icon: icon.map(PathBuf::from),
            uninstall: format!("{name} uninstall"),
        }
    }

    #[test]
    fn the_program_is_found_by_its_folder_its_icon_or_its_name() {
        let installed = [
            program(
                "Mozilla Firefox (x64 en-US)",
                Some(r"C:\Program Files\Mozilla Firefox"),
                None,
            ),
            program(
                "7-Zip 24.08 (x64)",
                None,
                Some(r"C:\Program Files\7-Zip\7zFM.exe"),
            ),
            program("Tools", Some(r"C:\Program Files"), None),
            program("Paint.NET", None, None),
            program("Git", Some(r"C:\Program Files\Git\"), None),
        ];
        let firefox = Path::new(r"C:\Program Files\Mozilla Firefox\firefox.exe");
        let found = best_match(&installed, "Firefox", Some(firefox));
        assert_eq!(
            found.map(|p| p.name.as_str()),
            Some("Mozilla Firefox (x64 en-US)")
        );
        let zip = Path::new(r"c:\program files\7-zip\7zFM.exe");
        assert_eq!(
            best_match(&installed, "7-Zip File Manager", Some(zip)).map(|p| p.name.as_str()),
            Some("7-Zip 24.08 (x64)")
        );
        assert_eq!(
            best_match(&installed, "paint.net", None).map(|p| p.name.as_str()),
            Some("Paint.NET")
        );
        let bash = Path::new(r"C:\Program Files\Git\git-bash.exe");
        assert_eq!(
            best_match(&installed, "Git Bash", Some(bash)).map(|p| p.name.as_str()),
            Some("Git")
        );
        assert_eq!(
            best_match(
                &installed,
                "Notepad",
                Some(Path::new(r"C:\Windows\notepad.exe"))
            ),
            None
        );
        assert_eq!(
            best_match(&installed, "Git", None).map(|p| p.name.as_str()),
            Some("Git")
        );
        assert_eq!(best_match(&installed, "", None), None);
        assert!(is_shared(Path::new(r"C:\Program Files\")));
        assert!(is_shared(Path::new("D:")));
        assert!(!is_shared(Path::new(r"C:\Python312")));
    }

    #[test]
    fn a_command_line_of_an_uninstaller_is_split_into_program_and_arguments() {
        let split = |line: &str| {
            let (program, arguments) = split_command(line);
            (program.unwrap_or_default(), arguments)
        };
        assert_eq!(
            split(r#""C:\Program Files\App\unins000.exe" /SILENT"#),
            (
                r"C:\Program Files\App\unins000.exe".to_owned(),
                "/SILENT".to_owned()
            )
        );
        assert_eq!(
            split(r"C:\Program Files\Some App\uninstall.exe /S"),
            (
                r"C:\Program Files\Some App\uninstall.exe".to_owned(),
                "/S".to_owned()
            )
        );
        assert_eq!(
            split("MsiExec.exe /X{1234-ABCD}"),
            ("MsiExec.exe".to_owned(), "/X{1234-ABCD}".to_owned())
        );
        assert_eq!(
            split("rundll32 thing,Entry"),
            ("rundll32".to_owned(), "thing,Entry".to_owned())
        );
        assert_eq!(split_command("  "), (None, String::new()));
        // A letter longer or shorter when lowered moves nothing.
        assert_eq!(
            split(r"C:\İzmir ẞ\App.EXE /quiet"),
            (r"C:\İzmir ẞ\App.EXE".to_owned(), "/quiet".to_owned())
        );
    }

    #[test]
    fn the_file_of_an_icon_is_read_without_its_number() {
        assert_eq!(
            icon_file(r#""C:\App\app.exe",0"#),
            PathBuf::from(r"C:\App\app.exe")
        );
        assert_eq!(
            icon_file(r"C:\App\app.ico"),
            PathBuf::from(r"C:\App\app.ico")
        );
        assert_eq!(
            icon_file(r"C:\A,B\app.exe,-101"),
            PathBuf::from(r"C:\A,B\app.exe")
        );
    }

    #[test]
    fn a_packaged_application_has_a_page_of_the_settings() {
        let launch = app_settings("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App");
        let expected =
            Launch::new("ms-settings:appsfeatures-app?Microsoft.WindowsCalculator_8wekyb3d8bbwe");
        assert_eq!(launch, Some(expected));
        assert_eq!(app_settings("Chrome"), None);
    }
}
