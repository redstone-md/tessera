// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The startup tasks of packaged (Store) applications, which Explorer
//! starts at sign-in.
//!
//! `Windows.ApplicationModel.StartupTask` answers only for the package of
//! the program that asks, so Tessera cannot use it for the others. It
//! reads what that API is built on instead: the `windows.startupTask`
//! extensions in the manifest of every package of the user, and the
//! `State` the system keeps for each task under
//! `HKCU\Software\Classes\Local Settings\...\AppModel\SystemAppData\<family>\<task>`
//! (the numbers of `StartupTaskState`). A task is started by opening its
//! application through `shell:AppsFolder`, which is not the activation
//! of a startup task: an application that shows nothing when Windows
//! starts it at sign-in may open its window.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};
use windows::ApplicationModel::Package;
use windows::Management::Deployment::{PackageManager, PackageTypes};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SAM_FLAGS};
use windows::Win32::UI::Shell::SHLoadIndirectString;
use windows::core::HSTRING;

use super::registry::{Failure, Key};

/// Where the system keeps the state of the startup tasks of a package.
const STATES: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";

/// The category of the extension that declares a startup task.
const CATEGORY: &str = "windows.startupTask";

/// The longest name a resource is read into, in UTF-16 units.
const NAME_LENGTH: usize = 512;

/// A startup task a manifest declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DeclaredTask {
    /// The application of the package the task belongs to.
    pub app_id: String,
    pub task_id: String,
    /// Whether the manifest has it on before the user or the application
    /// chose.
    pub enabled: bool,
    /// The name it shows, which may be a reference to a resource.
    pub display_name: Option<String>,
}

/// The startup tasks the manifest `text` declares; none if it cannot be
/// read. Old (`desktop:`) and new (`uap5:`) manifests name the elements
/// the same, in different namespaces.
pub(super) fn declared_tasks(text: &str) -> Vec<DeclaredTask> {
    let Ok(document) = roxmltree::Document::parse(text) else {
        return Vec::new();
    };
    let extensions = document.descendants().filter(|node| {
        node.tag_name().name() == "Extension" && node.attribute("Category") == Some(CATEGORY)
    });
    extensions
        .filter_map(|extension| {
            let application = extension
                .ancestors()
                .find(|node| node.tag_name().name() == "Application")?;
            let task = extension
                .children()
                .find(|node| node.tag_name().name() == "StartupTask")?;
            Some(DeclaredTask {
                app_id: application.attribute("Id")?.to_owned(),
                task_id: task.attribute("TaskId")?.to_owned(),
                enabled: task
                    .attribute("Enabled")
                    .is_some_and(|value| value.eq_ignore_ascii_case("true")),
                display_name: task.attribute("DisplayName").map(str::to_owned),
            })
        })
        .collect()
}

/// Whether a task is on, and whether the user may switch it, by the state
/// the system keeps (`None` before any was kept) and what its manifest
/// declares: 0 disabled, 1 disabled by the user, 2 enabled, 3 disabled
/// by a policy, 4 enabled by a policy.
pub(super) fn task_on(state: Option<u32>, declared: bool) -> (bool, bool) {
    match state {
        None => (declared, true),
        Some(2) => (true, true),
        Some(3) => (false, false),
        Some(4) => (true, false),
        Some(_) => (false, true),
    }
}

/// The source text `SHLoadIndirectString` reads the resource `text` of
/// the package `full_name` (named `name`) from, for a name written as
/// `ms-resource:Key`, `ms-resource:/Resources/Key` or a full
/// `ms-resource://` address; `None` for a plain name.
pub(super) fn indirect_source(full_name: &str, name: &str, text: &str) -> Option<String> {
    let rest = text.strip_prefix("ms-resource:")?;
    let address = if rest.starts_with("//") {
        format!("ms-resource:{rest}")
    } else if rest.starts_with('/') {
        format!("ms-resource://{name}{rest}")
    } else {
        format!("ms-resource://{name}/Resources/{rest}")
    };
    Some(format!("@{{{full_name}?{address}}}"))
}

/// A startup task of a package of the user, as it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PackageTask {
    pub family: String,
    pub app_id: String,
    pub task_id: String,
    pub name: String,
    pub on: bool,
    pub switchable: bool,
}

impl PackageTask {
    /// The identifier the application is started by.
    pub fn app(&self) -> String {
        format!("{}!{}", self.family, self.app_id)
    }

    /// Every startup task of the main packages of the user.
    pub fn all() -> Result<Vec<Self>> {
        let manager = PackageManager::new().context("the packages cannot be listed")?;
        let packages = manager
            .FindPackagesByUserSecurityIdWithPackageTypes(&HSTRING::new(), PackageTypes::Main)
            .context("the packages cannot be listed")?;
        let mut tasks = Vec::new();
        for package in packages {
            // A package that cannot be read is one without tasks to list.
            tasks.extend(Self::of_package(&package).unwrap_or_default());
        }
        Ok(tasks)
    }

    fn of_package(package: &Package) -> windows::core::Result<Vec<Self>> {
        let folder = package.InstalledPath()?.to_string();
        let Ok(manifest) = fs::read_to_string(Path::new(&folder).join("AppxManifest.xml")) else {
            return Ok(Vec::new());
        };
        // Most packages have no startup task; those are not parsed.
        if !manifest.contains(CATEGORY) {
            return Ok(Vec::new());
        }
        let id = package.Id()?;
        let (family, full_name, name) = (
            id.FamilyName()?.to_string(),
            id.FullName()?.to_string(),
            id.Name()?.to_string(),
        );
        let package_name = package.DisplayName().map(|name| name.to_string());
        let tasks = declared_tasks(&manifest).into_iter().map(|task| {
            let shown = task.display_name.as_deref().and_then(|text| {
                match indirect_source(&full_name, &name, text) {
                    Some(source) => load_indirect(&source),
                    None => Some(text.to_owned()),
                }
            });
            let shown = { shown.filter(|shown| !shown.is_empty()) }
                .or_else(|| package_name.clone().ok())
                .unwrap_or_else(|| name.clone());
            let (on, switchable) = task_on(state(&family, &task.task_id), task.enabled);
            Self {
                family: family.clone(),
                app_id: task.app_id,
                task_id: task.task_id,
                name: shown,
                on,
                switchable,
            }
        });
        Ok(tasks.collect())
    }

    /// Switches the task on or off as the user's choice, as the
    /// switches of Task Manager do.
    pub fn set_on(&self, on: bool) -> Result<(), Failure> {
        let path = format!(r"{STATES}\{}\{}", self.family, self.task_id);
        let (key, _) = Key::create(HKEY_CURRENT_USER, &path, REG_SAM_FLAGS(0), false)?;
        key.set_number("State", if on { 2 } else { 1 })
    }
}

/// The state the system keeps for the task `task` of the package family
/// `family`.
fn state(family: &str, task: &str) -> Option<u32> {
    let path = format!(r"{STATES}\{family}\{task}");
    Key::read(HKEY_CURRENT_USER, &path, REG_SAM_FLAGS(0))?.number("State")
}

/// The text the indirect string `source` names.
fn load_indirect(source: &str) -> Option<String> {
    let mut text = [0u16; NAME_LENGTH];
    // SAFETY: the source is a NUL-terminated string that outlives the
    // call, and the buffer carries its length.
    unsafe { SHLoadIndirectString(&HSTRING::from(source), &mut text, None) }.ok()?;
    let length = text.iter().position(|&unit| unit == 0)?;
    Some(String::from_utf16_lossy(&text[..length]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:uap5="http://schemas.microsoft.com/appx/manifest/uap/windows10/5"
         xmlns:desktop="http://schemas.microsoft.com/appx/manifest/desktop/windows10">
  <Applications>
    <Application Id="App" Executable="app.exe" EntryPoint="Windows.FullTrustApplication">
      <Extensions>
        <uap5:Extension Category="windows.startupTask" Executable="app.exe" EntryPoint="Windows.FullTrustApplication">
          <uap5:StartupTask TaskId="AppStartup" Enabled="true" DisplayName="ms-resource:StartupName" />
        </uap5:Extension>
        <uap5:Extension Category="windows.appExecutionAlias" Executable="app.exe" />
      </Extensions>
    </Application>
    <Application Id="Helper" Executable="helper.exe">
      <Extensions>
        <desktop:Extension Category="windows.startupTask" Executable="helper.exe">
          <desktop:StartupTask TaskId="HelperTask" Enabled="false" DisplayName="Helper" />
        </desktop:Extension>
      </Extensions>
    </Application>
  </Applications>
</Package>"#;

    #[test]
    fn a_manifest_declares_its_startup_tasks_in_either_namespace() {
        let tasks = declared_tasks(MANIFEST);
        assert_eq!(
            tasks,
            [
                DeclaredTask {
                    app_id: "App".to_owned(),
                    task_id: "AppStartup".to_owned(),
                    enabled: true,
                    display_name: Some("ms-resource:StartupName".to_owned()),
                },
                DeclaredTask {
                    app_id: "Helper".to_owned(),
                    task_id: "HelperTask".to_owned(),
                    enabled: false,
                    display_name: Some("Helper".to_owned()),
                },
            ]
        );
        assert!(declared_tasks("<not xml").is_empty());
    }

    #[test]
    fn the_kept_state_of_a_task_wins_over_its_manifest() {
        assert_eq!(task_on(None, true), (true, true));
        assert_eq!(task_on(None, false), (false, true));
        assert_eq!(task_on(Some(0), true), (false, true));
        assert_eq!(task_on(Some(1), true), (false, true));
        assert_eq!(task_on(Some(2), false), (true, true));
        assert_eq!(task_on(Some(3), true), (false, false));
        assert_eq!(task_on(Some(4), false), (true, false));
    }

    #[test]
    fn a_resource_name_is_read_from_its_package() {
        let full = "Vendor.App_1.0.0.0_x64__abc";
        let source = |text| indirect_source(full, "Vendor.App", text);
        assert_eq!(
            source("ms-resource:StartupName").as_deref(),
            Some("@{Vendor.App_1.0.0.0_x64__abc?ms-resource://Vendor.App/Resources/StartupName}")
        );
        assert_eq!(
            source("ms-resource:/Strings/Name").as_deref(),
            Some("@{Vendor.App_1.0.0.0_x64__abc?ms-resource://Vendor.App/Strings/Name}")
        );
        assert_eq!(
            source("ms-resource://Other/Resources/X").as_deref(),
            Some("@{Vendor.App_1.0.0.0_x64__abc?ms-resource://Other/Resources/X}")
        );
        assert_eq!(source("Plain name"), None);
    }
}
