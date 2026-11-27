// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The places Explorer offers at its side besides the drives: Quick
//! access (Home on Windows 11), with the folders the user pinned there
//! and those used often, the files used lately, OneDrive, and the Linux
//! distributions of WSL. Quick access is read from Explorer's own folder
//! of it and pinned to with Explorer's own verbs, so that both show the
//! same pins.

#![allow(unsafe_code)]

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use windows::Win32::Storage::EnhancedStorage::PKEY_Home_IsPinned;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::System::SystemServices::{SFGAO_FILESYSTEM, SFGAO_FOLDER, SFGAO_STREAM};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_Recent, FOLDERID_SkyDrive, IEnumShellItems, IShellItem, IShellItem2,
    SHCreateItemFromParsingName, SIGDN, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY,
};
use windows::core::{HSTRING, Interface, PWSTR};

use super::explorer_view::{subkeys, text};
use super::file_op::same_path;
use super::shell_item::IdList;
use super::shell_items::known_folder;
use super::{ShellThread, Shortcut, shell_menu};

/// The folder of Quick access, which Windows 11 shows as Home.
const QUICK_ACCESS: &str = "shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}";

/// The verbs Explorer pins a folder to Quick access with, and unpins it.
const PIN: &str = "pintohome";
const UNPIN: &str = "unpinfromhome";

/// Where Windows keeps the distributions of WSL of the user.
const LXSS: &str = r"Software\Microsoft\Windows\CurrentVersion\Lxss";

/// The share WSL serves its distributions at.
const WSL_ROOT: &str = r"\\wsl.localhost";

/// A place the Shell names: what it is called and where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellPlace {
    pub name: String,
    pub path: PathBuf,
}

/// A folder of Quick access, and whether the user pinned it there or it
/// is there as one used often.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickFolder {
    pub place: ShellPlace,
    pub pinned: bool,
}

/// Quick access, as Explorer keeps it.
pub struct QuickAccess;

impl QuickAccess {
    /// The folders of Quick access, in Explorer's order: those pinned,
    /// then those used often. Only folders of the file system are given;
    /// a library, a virtual folder or an archive is left out. Runs on a
    /// thread of the Shell of its own, as the folder is a Shell folder and
    /// a folder of another computer there may keep it waiting on the
    /// network.
    pub async fn folders() -> Result<Vec<QuickFolder>> {
        ShellThread::call_alone("tessera-quick-access", read_quick_access)
            .recv()
            .await
            .context("the thread of Quick access is gone")?
    }

    /// Pins the folder at `path` to Quick access, as "Pin to Quick
    /// access" in Explorer does.
    pub async fn pin(path: PathBuf) -> Result<()> {
        ShellThread::call(move || {
            // SAFETY: the shell thread has COM initialized.
            let item = unsafe { item_at(&HSTRING::from(path.as_os_str())) }?;
            invoke(&item, PIN)
        })
        .recv()
        .await
        .context("the shell thread is gone")?
    }

    /// Unpins the folder at `path` from Quick access, as "Unpin from
    /// Quick access" in Explorer does. Fails where it is not pinned, as
    /// a folder that is only used often has no such verb.
    pub async fn unpin(path: PathBuf) -> Result<()> {
        ShellThread::call(move || {
            let item = { quick_access_items()?.into_iter() }
                .find(|(_, place)| same_path(&place.path, &path))
                .map(|(item, _)| item)
                .context("the folder is not in Quick access")?;
            invoke(&item, UNPIN)
        })
        .recv()
        .await
        .context("the shell thread is gone")?
    }
}

/// The files the user opened lately, the latest first, at most `most`,
/// as the Recent folder of Windows records them; a shortcut there whose
/// file is gone, or which is a folder, is left out. Runs on a thread of
/// the Shell of its own, as shortcuts are read through the Shell and a
/// file of another computer may keep the check that it is there waiting
/// on the network.
pub async fn recent_files(most: usize) -> Vec<ShellPlace> {
    ShellThread::call_alone("tessera-recent", move || read_recent(most))
        .recv()
        .await
        .unwrap_or_default()
}

/// The folder of OneDrive of the user, where OneDrive is set up.
pub fn onedrive() -> Option<PathBuf> {
    known_folder(&FOLDERID_SkyDrive)
        .ok()
        .filter(|path| path.is_dir())
}

/// The distributions of WSL of the user, each at its share under
/// `\\wsl.localhost`, as the registry names them; empty without WSL.
pub fn wsl_distributions() -> Vec<ShellPlace> {
    let lxss = HSTRING::from(LXSS);
    { subkeys(HKEY_CURRENT_USER, &lxss).into_iter() }
        .filter_map(|id| {
            let key = HSTRING::from(format!("{LXSS}\\{id}"));
            let name = text(HKEY_CURRENT_USER, &key, Some("DistributionName"))?;
            let path = PathBuf::from(format!("{WSL_ROOT}\\{name}"));
            Some(ShellPlace { name, path })
        })
        .collect()
}

/// The item of the Shell at `name`, a path or a `shell:` name.
///
/// # Safety
///
/// COM must be initialized on the calling thread.
unsafe fn item_at(name: &HSTRING) -> Result<IShellItem> {
    // SAFETY: COM is initialized, as the caller promises, and the name is
    // NUL-terminated.
    unsafe { SHCreateItemFromParsingName(name, None) }
        .with_context(|| format!("the Shell does not know {name}"))
}

/// The text the Shell gives of `item` as `kind`.
///
/// # Safety
///
/// COM must be initialized on the calling thread.
unsafe fn name_of(item: &IShellItem, kind: SIGDN) -> Option<String> {
    // SAFETY: the text the Shell allocates is copied and freed once.
    unsafe {
        let name: PWSTR = item.GetDisplayName(kind).ok()?;
        let text = name.to_string().ok();
        CoTaskMemFree(Some(name.0.cast()));
        text.filter(|text| !text.is_empty())
    }
}

/// The items of Quick access that are folders of the file system, each
/// with its name and path. Quick access lists the files used lately too,
/// and an archive is a folder of the Shell that is a file as well, which
/// is left out.
fn quick_access_items() -> Result<Vec<(IShellItem, ShellPlace)>> {
    let mut found = Vec::new();
    // SAFETY: this runs on the shell thread, which has COM initialized;
    // every item is released when it goes out of scope.
    unsafe {
        let folder = item_at(&HSTRING::from(QUICK_ACCESS))?;
        let items: IEnumShellItems = folder
            .BindToHandler(None, &BHID_EnumItems)
            .context("Quick access cannot be read")?;
        loop {
            let mut item = [None];
            let mut fetched = 0;
            if items.Next(&mut item, Some(&raw mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = item[0].take() else {
                break;
            };
            let wanted = SFGAO_FOLDER | SFGAO_FILESYSTEM;
            if item.GetAttributes(wanted | SFGAO_STREAM).ok() != Some(wanted) {
                continue;
            }
            let path = name_of(&item, SIGDN_FILESYSPATH);
            let name = name_of(&item, SIGDN_NORMALDISPLAY);
            if let (Some(path), Some(name)) = (path, name) {
                let place = ShellPlace {
                    name,
                    path: PathBuf::from(path),
                };
                found.push((item, place));
            }
        }
    }
    Ok(found)
}

fn read_quick_access() -> Result<Vec<QuickFolder>> {
    Ok({ quick_access_items()?.into_iter() }
        .map(|(item, place)| QuickFolder {
            pinned: is_pinned(&item),
            place,
        })
        .collect())
}

/// Whether the user pinned the item of Quick access `item` there, as the
/// property `System.Home.IsPinned` of Windows 11 says. Where it cannot be
/// read, the item is taken as pinned, so that "Unpin" is offered and says
/// so where it fails.
fn is_pinned(item: &IShellItem) -> bool {
    // SAFETY: this runs on a thread with COM initialized; the key is a
    // constant that outlives the call.
    unsafe {
        { item.cast::<IShellItem2>() }
            .and_then(|item| item.GetBool(&PKEY_Home_IsPinned))
            .map_or(true, |pinned| pinned.as_bool())
    }
}

/// Has the Shell do the verb `verb` of the menu of `item`, as a click on
/// it in Explorer's menu does. Runs on the shell thread.
fn invoke(item: &IShellItem, verb: &str) -> Result<()> {
    let list = IdList::of_object(item)?;
    let owner = ShellThread::owner().unwrap_or_default();
    shell_menu::invoke_verb_here(vec![list], verb, owner, true)
}

/// The files the Recent folder has shortcuts to, the latest first.
fn read_recent(most: usize) -> Vec<ShellPlace> {
    let Ok(folder) = known_folder(&FOLDERID_Recent) else {
        return Vec::new();
    };
    let Ok(items) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut links: Vec<(std::time::SystemTime, PathBuf)> = items
        .filter_map(Result::ok)
        .map(|item| item.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
        })
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .collect();
    links.sort_unstable_by_key(|link| std::cmp::Reverse(link.0));
    { links.into_iter() }
        .filter_map(|(_, path)| {
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let shortcut = Shortcut {
                name: name.clone(),
                path,
            };
            let target = shortcut.link().target?;
            target
                .is_file()
                .then_some(ShellPlace { name, path: target })
        })
        .take(most)
        .collect()
}
