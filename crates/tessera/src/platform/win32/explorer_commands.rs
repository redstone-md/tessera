// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The entries packaged and sparse applications add to the menus of files
//! of Windows 11 (`IExplorerCommand`): Terminal's "Open in Terminal",
//! PowerToys, Notepad, Visual Studio Code and the like, which the classic
//! menu of the Shell (`IContextMenu`) does not hold.
//!
//! Each package declares them in its `AppxManifest.xml`, under the
//! extension `windows.fileExplorerContextMenus` (`desktop4` and `desktop5`):
//! the kinds of items (`ItemType Type=".txt"`, `*`, `Directory`,
//! `Directory\Background`) and for each a `Verb` with its class. Sync
//! providers declare theirs under `CloudFilesContextMenus` (the classic
//! OneDrive client registers its verbs as a shell extension, which the
//! Shell's own menu shows). The packages of the user are read once with
//! `PackageManager::FindPackagesByUserSecurityId`, each manifest once per
//! full name of its package, on a thread of their own.
//!
//! For a menu, each class that fits what is chosen is made
//! (`CoCreateInstance`, in this process or its own) and asked, with the
//! items chosen as an `IShellItemArray`, for its title, icon and state;
//! what it hides or disables is left out, and its subcommands make a
//! submenu. A click makes the class again and invokes the command, on a
//! thread of its own, as a slow or hanging command would hold up the other
//! work of the Shell.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use async_channel::Receiver;
use windows::Management::Deployment::PackageManager;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CoCreateInstance};
use windows::Win32::UI::Shell::{
    ECF_HASSUBCOMMANDS, ECF_ISSEPARATOR, ECS_DISABLED, ECS_HIDDEN, IExplorerCommand,
    IShellItemArray, SHCreateShellItemArrayFromIDLists,
};
use windows::core::{GUID, HSTRING};

use super::IconPixels;
use super::ShellThread;
use super::shell_item::IdList;
use super::shell_items::{IconPlace, take_text};

/// How large the icon of a command is read, in pixels.
const ICON_SIZE: u32 = 32;

/// How deep submenus of commands are read.
const DEEPEST: usize = 3;

/// How long the packages read stay current before they are read again.
const FRESH: Duration = Duration::from_secs(600);

/// The kinds of items a verb declares for, as manifests write them.
const BACKGROUND: &str = "directory\\background";
const DIRECTORY: &str = "directory";
const ANY_FILE: &str = "*";

/// A verb a package declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredVerb {
    /// The kinds of items, in small letters: `.txt`, `*`, `directory`,
    /// `directory\background`.
    pub types: Vec<String>,
    pub id: String,
    pub clsid: GUID,
}

/// What the packages of the user declare, once read.
#[derive(Default)]
struct Declared {
    /// The verbs of each package, by its full name.
    by_package: HashMap<String, Vec<DeclaredVerb>>,
    read_at: Option<Instant>,
}

static DECLARED: Mutex<Option<Declared>> = Mutex::new(None);
static READING: AtomicBool = AtomicBool::new(false);

/// Which command of a class a click invokes: the class, and the places of
/// the subcommands that lead to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandPath {
    clsid: GUID,
    steps: Vec<usize>,
}

/// A command for the items of a menu, as the command answered.
#[derive(Clone, Debug)]
pub struct ExplorerCommand {
    pub title: String,
    pub icon: Option<IconPixels>,
    /// Its subcommands, which make a submenu; it is invoked itself only
    /// where it has none.
    pub children: Vec<ExplorerCommand>,
    pub path: CommandPath,
}

/// The commands of the menus of Windows 11.
pub struct ExplorerCommands;

impl ExplorerCommands {
    /// Starts reading what the packages of the user declare, away from the
    /// user interface, where it is not read or is old; it returns at once.
    pub fn prepare() {
        let fresh = { DECLARED.lock().unwrap_or_else(PoisonError::into_inner) }
            .as_ref()
            .and_then(|declared| declared.read_at)
            .is_some_and(|at| at.elapsed() < FRESH);
        if fresh || READING.swap(true, Ordering::Relaxed) {
            return;
        }
        ShellThread::run_alone("explorer commands", || {
            if let Err(error) = read_packages() {
                report!(
                    "the entries of applications for the menus of files are not read: {error:#}"
                );
            }
            READING.store(false, Ordering::Relaxed);
        });
    }

    /// The commands for `items` (paths, or the folder of a menu's
    /// background where `background`), asked on a thread of their own; the
    /// receiver yields once. Packages not read yet give none.
    pub fn for_items(items: Vec<PathBuf>, background: bool) -> Receiver<Vec<ExplorerCommand>> {
        Self::prepare();
        ShellThread::call_alone("explorer commands", move || {
            let verbs = fitting_verbs(&items, background);
            if verbs.is_empty() || items.is_empty() {
                return Vec::new();
            }
            let array = match item_array(&items) {
                Ok(array) => array,
                Err(error) => {
                    report!("the entries of applications are not asked: {error:#}");
                    return Vec::new();
                }
            };
            { verbs.iter() }
                .filter_map(|verb| {
                    let command = make(&verb.clsid)
                        .inspect_err(|error| {
                            report!("the entry {} is not made: {error:#}", verb.id)
                        })
                        .ok()?;
                    let path = CommandPath {
                        clsid: verb.clsid,
                        steps: Vec::new(),
                    };
                    read_command(&command, &array, path, 0)
                })
                .collect()
        })
    }

    /// Invokes the command at `path` for `items`, on a thread of its own.
    pub fn invoke(items: Vec<PathBuf>, path: CommandPath) {
        ShellThread::run_alone("explorer command", move || {
            let invoked = (|| -> Result<()> {
                let array = item_array(&items)?;
                let mut command = make(&path.clsid)?;
                for &step in &path.steps {
                    command = subcommand(&command, step)?;
                }
                // SAFETY: the command and the items are alive on this
                // thread for the call.
                unsafe { command.Invoke(&array, None) }.context("the command failed")
            })();
            if let Err(error) = invoked {
                report!("{error:#}");
            }
        });
    }
}

/// The verbs declared for every one of `items`, without a class twice.
fn fitting_verbs(items: &[PathBuf], background: bool) -> Vec<DeclaredVerb> {
    let declared = DECLARED.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(declared) = declared.as_ref() else {
        return Vec::new();
    };
    let kinds: Vec<Vec<String>> = { items.iter() }
        .map(|item| item_kinds(item, background, item.is_dir()))
        .collect();
    let mut fitting: Vec<DeclaredVerb> = Vec::new();
    let mut packages: Vec<&String> = declared.by_package.keys().collect();
    packages.sort();
    for package in packages {
        for verb in &declared.by_package[package] {
            let fits = !kinds.is_empty() && { kinds.iter() }
                .all(|kinds| verb.types.iter().any(|kind| kinds.contains(kind)));
            if fits && !fitting.iter().any(|other| other.clsid == verb.clsid) {
                fitting.push(verb.clone());
            }
        }
    }
    fitting
}

/// The kinds a verb may declare for the item at `path`, a folder or not,
/// or the background of a folder, in small letters.
pub fn item_kinds(path: &Path, background: bool, folder: bool) -> Vec<String> {
    if background {
        return vec![BACKGROUND.to_owned()];
    }
    if folder {
        return vec![DIRECTORY.to_owned()];
    }
    let mut kinds = vec![ANY_FILE.to_owned()];
    if let Some(extension) = path.extension() {
        kinds.push(format!(".{}", extension.to_string_lossy().to_lowercase()));
    }
    kinds
}

/// Reads the verbs the packages of the user declare, each manifest only
/// once for its package's full name.
fn read_packages() -> Result<()> {
    let manager = PackageManager::new().context("the package manager is missing")?;
    let packages = { manager.FindPackagesByUserSecurityId(&HSTRING::new()) }
        .context("the packages of the user are not listed")?;
    let known: HashMap<String, Vec<DeclaredVerb>> = { DECLARED.lock() }
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|declared| declared.by_package.clone())
        .unwrap_or_default();
    let mut by_package = HashMap::new();
    for package in packages {
        let Ok(full_name) = package.Id().and_then(|id| id.FullName()) else {
            continue;
        };
        let full_name = full_name.to_string();
        if let Some(verbs) = known.get(&full_name) {
            by_package.insert(full_name, verbs.clone());
            continue;
        }
        let Ok(folder) = package.InstalledPath() else {
            continue;
        };
        let manifest = PathBuf::from(folder.to_string()).join("AppxManifest.xml");
        let verbs = std::fs::read_to_string(&manifest)
            .map(|text| verbs_of_manifest(&text))
            .unwrap_or_default();
        by_package.insert(full_name, verbs);
    }
    *DECLARED.lock().unwrap_or_else(PoisonError::into_inner) = Some(Declared {
        by_package,
        read_at: Some(Instant::now()),
    });
    Ok(())
}

/// The verbs a manifest declares for the menus of files: those of its
/// `FileExplorerContextMenus`, by their item types, and those of a sync
/// provider's `CloudFilesContextMenus`, for files and folders.
pub fn verbs_of_manifest(xml: &str) -> Vec<DeclaredVerb> {
    let Ok(document) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    let named =
        |node: &roxmltree::Node, name: &str| node.is_element() && node.tag_name().name() == name;
    let verb_of = |node: roxmltree::Node, types: Vec<String>| -> Option<DeclaredVerb> {
        let clsid = node.attribute("Clsid")?.trim().trim_matches(['{', '}']);
        Some(DeclaredVerb {
            types,
            id: node.attribute("Id").unwrap_or_default().to_owned(),
            clsid: GUID::try_from(clsid).ok()?,
        })
    };
    let mut verbs = Vec::new();
    for menus in document
        .descendants()
        .filter(|node| named(node, "FileExplorerContextMenus"))
    {
        for item in menus.children().filter(|node| named(node, "ItemType")) {
            let Some(kind) = item.attribute("Type") else {
                continue;
            };
            let kind = kind.trim().to_lowercase();
            for verb in item.children().filter(|node| named(node, "Verb")) {
                verbs.extend(verb_of(verb, vec![kind.clone()]));
            }
        }
    }
    for menus in document
        .descendants()
        .filter(|node| named(node, "CloudFilesContextMenus"))
    {
        for verb in menus.children().filter(|node| named(node, "Verb")) {
            verbs.extend(verb_of(
                verb,
                vec![ANY_FILE.to_owned(), DIRECTORY.to_owned()],
            ));
        }
    }
    verbs
}

/// Makes the command of the class `clsid`, in this process or its own.
fn make(clsid: &GUID) -> Result<IExplorerCommand> {
    // SAFETY: COM is initialized on this thread (`run_alone`); the class
    // is made as the interface asked for.
    unsafe { CoCreateInstance(clsid, None, CLSCTX_LOCAL_SERVER | CLSCTX_INPROC_SERVER) }
        .with_context(|| format!("the class {clsid:?} cannot be made"))
}

/// The items at `paths` as the Shell hands them to a command.
fn item_array(paths: &[PathBuf]) -> Result<IShellItemArray> {
    let lists: Vec<IdList> = { paths.iter() }
        .map(|path| IdList::of_path(path))
        .collect::<Result<_>>()?;
    let pointers: Vec<_> = lists.iter().map(IdList::as_ptr).collect();
    // SAFETY: the identifier lists are alive for the call, which copies
    // them into the array.
    unsafe { SHCreateShellItemArrayFromIDLists(&pointers) }
        .context("the items cannot be handed over")
}

/// The subcommand of `command` at `index`.
fn subcommand(command: &IExplorerCommand, index: usize) -> Result<IExplorerCommand> {
    // SAFETY: the command is alive on this thread; the enumerator gives
    // each subcommand once, into the room of one.
    unsafe {
        let all = command
            .EnumSubCommands()
            .context("the command has no subcommands")?;
        let mut at = 0;
        loop {
            let mut one = [None];
            let mut fetched = 0u32;
            let next = all.Next(&mut one, Some(&raw mut fetched));
            if next.is_err() || fetched == 0 {
                return Err(anyhow!("the subcommand is gone"));
            }
            if at == index {
                return one[0]
                    .take()
                    .ok_or_else(|| anyhow!("the subcommand is gone"));
            }
            at += 1;
        }
    }
}

/// What `command` answers for `items`: its title, icon and subcommands,
/// unless it hides itself, is disabled or is a separator.
fn read_command(
    command: &IExplorerCommand,
    items: &IShellItemArray,
    path: CommandPath,
    depth: usize,
) -> Option<ExplorerCommand> {
    // SAFETY: the command and the items are alive on this thread for the
    // calls; the texts the command allocates are taken and freed once.
    unsafe {
        let state = command.GetState(items, true).unwrap_or(0);
        let hidden = (ECS_HIDDEN.0 | ECS_DISABLED.0) as u32;
        if state & hidden != 0 {
            return None;
        }
        let flags = command.GetFlags().unwrap_or(0);
        if flags & ECF_ISSEPARATOR.0 as u32 != 0 {
            return None;
        }
        let title = take_text(command.GetTitle(items).ok()?)?;
        let title = title.replace('&', "");
        if title.trim().is_empty() {
            return None;
        }
        let icon = { command.GetIcon(items).ok() }
            .and_then(|text| take_text(text))
            .and_then(|location| IconPlace::from_location(&location))
            .and_then(|place| place.pixels(ICON_SIZE));
        let mut children = Vec::new();
        if flags & ECF_HASSUBCOMMANDS.0 as u32 != 0 && depth < DEEPEST {
            if let Ok(all) = command.EnumSubCommands() {
                let mut index = 0;
                loop {
                    let mut one = [None];
                    let mut fetched = 0u32;
                    let next = all.Next(&mut one, Some(&raw mut fetched));
                    let Some(child) = one[0].take().filter(|_| next.is_ok() && fetched > 0) else {
                        break;
                    };
                    let mut steps = path.steps.clone();
                    steps.push(index);
                    let child_path = CommandPath {
                        clsid: path.clsid,
                        steps,
                    };
                    children.extend(read_command(&child, items, child_path, depth + 1));
                    index += 1;
                }
            }
            if children.is_empty() {
                return None;
            }
        }
        Some(ExplorerCommand {
            title,
            icon,
            children,
            path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verbs_of_a_manifest_are_read_by_their_item_types() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:desktop4="http://schemas.microsoft.com/appx/manifest/desktop/windows10/4"
  xmlns:desktop5="http://schemas.microsoft.com/appx/manifest/desktop/windows10/5"
  xmlns:cloudfiles="http://schemas.microsoft.com/appx/manifest/cloudfiles/windows10">
  <Applications><Application Id="App"><Extensions>
    <desktop4:Extension Category="windows.fileExplorerContextMenus">
      <desktop4:FileExplorerContextMenus>
        <desktop5:ItemType Type="Directory">
          <desktop5:Verb Id="OpenTerminalHere" Clsid="9F156763-7844-4DC4-B2B1-901F640F5155" />
        </desktop5:ItemType>
        <desktop5:ItemType Type="Directory\Background">
          <desktop5:Verb Id="OpenTerminalHere" Clsid="{9F156763-7844-4DC4-B2B1-901F640F5155}" />
        </desktop5:ItemType>
        <desktop4:ItemType Type=".TXT">
          <desktop4:Verb Id="Edit" Clsid="not a class" />
        </desktop4:ItemType>
      </desktop4:FileExplorerContextMenus>
    </desktop4:Extension>
    <cloudfiles:Extension Category="windows.cloudFiles"><cloudfiles:CloudFiles>
      <cloudfiles:CloudFilesContextMenus>
        <cloudfiles:Verb Id="Pin" Clsid="11111111-2222-3333-4444-555555555555" />
      </cloudfiles:CloudFilesContextMenus>
    </cloudfiles:CloudFiles></cloudfiles:Extension>
  </Extensions></Application></Applications>
</Package>"#;
        let verbs = verbs_of_manifest(xml);
        assert_eq!(verbs.len(), 3);
        assert_eq!(verbs[0].types, ["directory"]);
        assert_eq!(verbs[1].types, ["directory\\background"]);
        assert_eq!(verbs[0].clsid, verbs[1].clsid);
        assert_eq!(verbs[0].id, "OpenTerminalHere");
        assert_eq!(verbs[2].types, ["*", "directory"]);
        assert!(verbs_of_manifest("not xml").is_empty());
    }

    #[test]
    fn items_are_of_the_kinds_manifests_name() {
        let file = item_kinds(Path::new(r"C:\a\Notes.TXT"), false, false);
        assert_eq!(file, ["*", ".txt"]);
        assert_eq!(item_kinds(Path::new(r"C:\a"), false, true), ["directory"]);
        assert_eq!(
            item_kinds(Path::new(r"C:\a"), true, true),
            ["directory\\background"]
        );
    }
}
