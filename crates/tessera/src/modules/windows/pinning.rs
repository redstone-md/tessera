// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pinning to the dock from elsewhere than the menu of a button: the menu
//! of the empty part of the dock (a file, a folder, a web address, a
//! separator, the pins of the taskbar of Windows), and the actions a
//! `[[key]]`, the command line and the menus of Explorer reach:
//!
//! - `pin <path|link>` and `unpin <path|link>`, which `tessera pin` and
//!   `tessera unpin` hand to the Tessera that runs;
//! - `import-taskbar-pins`, which appends the pins of the taskbar of
//!   Windows;
//! - `pin-verb on|off`, which writes or takes away "Pin to Tessera dock"
//!   in the menus of files, folders and shortcuts.

use std::path::{Path, PathBuf};

use gpui::{App, AsyncApp, PathPromptOptions, SharedString};

use super::address::AddressPrompt;
use super::pins::{SEPARATOR, entry_of};
use super::{repin, with};
use crate::actions::{Action, Actions};
use crate::banners::{Banners, Notice};
use crate::i18n::Language;
use crate::menu::MenuEntry;
use crate::platform::{PinVerb, windows_taskbar_pins};
use crate::popup::PopupAnchor;

const ICON_FILE: &str = "\u{e8a5}";
const ICON_FOLDER: &str = "\u{e838}";
const ICON_LINK: &str = "\u{e774}";
const ICON_SEPARATOR: &str = "\u{e76f}";
const ICON_IMPORT: &str = "\u{e896}";
const ICON_PIN: &str = "\u{e718}";

/// The verb that pins to the dock, as the menus of Explorer name it.
const VERB_LABEL: &str = "Pin to Tessera dock";

/// How `pinned` writes what `arg` names: a path made absolute where it is
/// one (slashes as the dock writes them), a link or a name as it is.
pub(super) fn entry_for(arg: &str) -> String {
    let arg = arg.trim().trim_matches('"');
    let path = Path::new(arg);
    match path.is_absolute() {
        true => entry_of(path),
        false => arg.to_owned(),
    }
}

/// Whether two entries of `pinned` name the same pin: paths whatever
/// their slashes and case, other entries whatever their case.
fn same_entry(one: &str, other: &str) -> bool {
    let plain = |entry: &str| entry.trim().replace('\\', "/").to_lowercase();
    plain(one) == plain(other)
}

/// `pins` without those that name what `entry` names.
fn without_entry(pins: &[String], entry: &str) -> Vec<String> {
    let kept = pins.iter().filter(|pin| !same_entry(pin, entry));
    kept.cloned().collect()
}

/// `pins` with `found` appended, those pinned already left out.
fn appended(pins: &[String], found: &[String]) -> Vec<String> {
    let mut pins = pins.to_vec();
    for entry in found {
        if !pins.iter().any(|pin| same_entry(pin, entry)) {
            pins.push(entry.clone());
        }
    }
    pins
}

/// Registers the actions of pinning.
pub(super) fn register(actions: &mut Actions) -> anyhow::Result<()> {
    let needs_arg = |what: &'static str| {
        move |arg: Option<&str>| match arg.map(str::trim) {
            Some(arg) if !arg.is_empty() => Ok(()),
            _ => Err(format!(
                "`{what}` takes the path of a file or folder, or a link, as `arg`"
            )),
        }
    };
    let pin = Action::new(|arg, cx| {
        let Some(arg) = arg.map(entry_for).filter(|arg| !arg.is_empty()) else {
            return;
        };
        repin(cx, move |pins| {
            match pins.iter().any(|pin| same_entry(pin, &arg)) {
                true => pins.to_vec(),
                false => with(pins, &arg),
            }
        });
    })
    .checking(needs_arg("pin"));
    actions.register("pin", pin)?;
    let unpin = Action::new(|arg, cx| {
        let Some(arg) = arg.map(entry_for).filter(|arg| !arg.is_empty()) else {
            return;
        };
        repin(cx, move |pins| without_entry(pins, &arg));
    })
    .checking(needs_arg("unpin"));
    actions.register("unpin", unpin)?;
    actions.register("import-taskbar-pins", Action::new(|_, cx| import(cx)))?;
    let verb = Action::new(|arg, cx| {
        let on = arg.is_some_and(|arg| arg.trim().eq_ignore_ascii_case("on"));
        pin_verb(on, cx);
    })
    .checking(
        |arg| match arg.map(|arg| arg.trim().to_lowercase()).as_deref() {
            Some("on" | "off") => Ok(()),
            _ => Err("`pin-verb` takes `on` or `off` as `arg`".to_owned()),
        },
    );
    actions.register("pin-verb", verb)
}

/// The menu of the empty part of the dock, opened at `anchor`.
pub(super) fn empty_part(anchor: PopupAnchor, cx: &App) -> Vec<MenuEntry> {
    let language = Language::of(cx);
    let say = |text: &'static str| language.say(text);
    vec![
        MenuEntry::new(ICON_FILE, say("Pin a file…"), |cx| pick(false, cx)),
        MenuEntry::new(ICON_FOLDER, say("Pin a folder…"), |cx| pick(true, cx)),
        MenuEntry::new(ICON_LINK, say("Pin a web address…"), move |cx| {
            // The menu closes first.
            cx.defer(move |cx| AddressPrompt::open(anchor, cx));
        }),
        MenuEntry::new(ICON_SEPARATOR, say("Add separator"), |cx| {
            repin(cx, |pins| {
                let mut pins = pins.to_vec();
                pins.push(SEPARATOR.to_owned());
                pins
            });
        }),
        MenuEntry::new(
            ICON_IMPORT,
            say("Import the pins of the Windows taskbar"),
            import,
        ),
    ]
}

/// Asks for files, or for folders, with the dialog of Windows, and pins
/// those chosen.
fn pick(folders: bool, cx: &mut App) {
    let language = Language::of(cx);
    let prompt = match folders {
        true => language.say("Pin a folder…"),
        false => language.say("Pin a file…"),
    };
    cx.spawn(async move |cx: &mut AsyncApp| {
        let options = PathPromptOptions {
            files: !folders,
            directories: folders,
            multiple: true,
            prompt: Some(SharedString::new_static(prompt)),
        };
        let Ok(chosen) = cx.update(|cx| cx.prompt_for_paths(options)) else {
            return;
        };
        let paths: Vec<PathBuf> = match chosen.await {
            Ok(Ok(Some(paths))) => paths,
            Ok(Err(error)) => return report!("nothing can be chosen to pin: {error:#}"),
            _ => return,
        };
        let found: Vec<String> = paths.iter().map(|path| entry_of(path)).collect();
        cx.update(|cx| repin(cx, move |pins| appended(pins, &found)))
            .ok();
    })
    .detach();
}

/// Appends the pins of the taskbar of Windows that are not pinned yet,
/// and tells how many came.
fn import(cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let found = cx
            .background_executor()
            .spawn(async { windows_taskbar_pins() })
            .await;
        let found: Vec<String> = found.iter().map(|path| entry_of(path)).collect();
        cx.update(|cx| {
            let language = Language::of(cx);
            let body = match found.len() {
                0 => language
                    .say("The taskbar of Windows has no pins.")
                    .to_owned(),
                count => language
                    .say("Pins found on the taskbar of Windows: {}")
                    .replace("{}", &count.to_string()),
            };
            let notice = Notice {
                app: language.say("Dock").to_owned(),
                title: language
                    .say("Import the pins of the Windows taskbar")
                    .to_owned(),
                body,
                glyph: Some(ICON_IMPORT),
                ..Notice::default()
            };
            Banners::post(notice, cx);
            if !found.is_empty() {
                repin(cx, move |pins| appended(pins, &found));
            }
        })
        .ok();
    })
    .detach();
}

/// Writes the verb that pins to the dock, or takes it away, and tells
/// how that went.
fn pin_verb(on: bool, cx: &mut App) {
    let language = Language::of(cx);
    let label = language.say(VERB_LABEL);
    cx.spawn(async move |cx: &mut AsyncApp| {
        let done = cx
            .background_executor()
            .spawn(async move {
                match on {
                    // Nothing is written where it runs this program already.
                    true if PinVerb::registered() => Ok(()),
                    true => PinVerb::register(label),
                    false => PinVerb::unregister(),
                }
            })
            .await;
        let (title, body) = match (on, done) {
            (true, Ok(())) => (
                "The menus of files and folders offer Pin to Tessera dock",
                String::new(),
            ),
            (false, Ok(())) => (
                "The menus of files and folders no longer offer Pin to Tessera dock",
                String::new(),
            ),
            (_, Err(error)) => (
                "The menus of files and folders were not changed",
                format!("{error:#}"),
            ),
        };
        cx.update(|cx| {
            let notice = Notice {
                app: language.say("Dock").to_owned(),
                title: language.say(title).to_owned(),
                body,
                glyph: Some(ICON_PIN),
                ..Notice::default()
            };
            Banners::post(notice, cx);
        })
        .ok();
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn an_entry_is_found_whatever_its_slashes_and_case() {
        assert!(same_entry(r"C:\Apps\App.exe", "c:/apps/app.EXE"));
        assert!(same_entry("Paint", "paint "));
        assert!(!same_entry("Paint", "Paint 3D"));
        let pins = list(&["C:/Apps/app.exe", "Paint", "separator"]);
        assert_eq!(
            without_entry(&pins, r"C:\APPS\app.exe"),
            list(&["Paint", "separator"])
        );
        assert_eq!(
            appended(&pins, &list(&["paint", "https://example.org"])),
            list(&[
                "C:/Apps/app.exe",
                "Paint",
                "separator",
                "https://example.org"
            ])
        );
        assert_eq!(entry_for("  \"Paint\" "), "Paint");
        assert_eq!(entry_for("https://example.org"), "https://example.org");
        assert_eq!(entry_for(r"C:\Apps\app.exe"), "C:/Apps/app.exe");
    }

    #[test]
    fn the_actions_of_pinning_take_what_they_need() {
        let mut actions = Actions::default();
        register(&mut actions).unwrap();
        assert!(actions.check("pin", Some("C:/a.txt")).is_ok());
        assert!(actions.check("pin", None).is_err());
        assert!(actions.check("unpin", Some(" ")).is_err());
        assert!(actions.check("pin-verb", Some("ON")).is_ok());
        assert!(actions.check("pin-verb", Some("maybe")).is_err());
        assert!(actions.check("import-taskbar-pins", None).is_ok());
        assert!(actions.check("import-taskbar-pins", Some("x")).is_err());
    }
}
