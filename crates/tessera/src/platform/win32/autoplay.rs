// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! AutoPlay, as Explorer does it when a drive arrives: what the drive
//! holds is looked at, the AutoPlay events that fit it are named (a
//! removable drive is `StorageOnArrival`, a memory card with a `DCIM`
//! folder `CameraAlternate\ShowPicturesOnArrival`, pictures
//! `ShowPicturesOnArrival`, an audio CD `PlayCDAudioOnArrival` and so
//! on), and the handlers registered for them under
//! `Software\Microsoft\Windows\CurrentVersion\Explorer\AutoplayHandlers`
//! (of the user and of the machine) are read, with the choice the user
//! made in Settings > AutoPlay (`UserChosenExecuteHandlers`, then
//! `EventHandlersDefaultSelection`).
//!
//! A handler is run as Explorer runs it: one with `InvokeProgID` and
//! `InvokeVerb` through that verb of that class with the root of the
//! drive, one with a `CLSID` (or a `ProgID` that names one) through
//! `IHWEventHandler::HandleEvent`. `MSOpenFolder` and `MSTakeNoAction`
//! are left to the caller, which opens the folder its own way. Handlers
//! of another kind (only a `DropTarget`) are not offered.
//!
//! `DisableAutoplay` of the user and the policy `NoDriveTypeAutoRun` are
//! honoured: with either, nothing is offered.
#![allow(unsafe_code)]

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};
use windows::Win32::Storage::FileSystem::GetDriveTypeW;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CLSIDFromString, CoCreateInstance,
};
use windows::Win32::System::Registry::{
    HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_SAM_FLAGS,
};
use windows::Win32::UI::Shell::{
    IHWEventHandler, SEE_MASK_CLASSNAME, SHELLEXECUTEINFOW, SHLoadIndirectString, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR};

use super::ShellThread;
use super::files::text_in;
use super::registry::{Key, registry_dword, registry_text};

/// The key of AutoPlay, of the user and of the machine.
const AUTOPLAY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\AutoplayHandlers";

/// The key of the policies of Explorer.
const POLICY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";

/// The handler that asks each time.
pub const PROMPT: &str = "MSPromptEachTime";
/// The handler that does nothing.
pub const NOTHING: &str = "MSTakeNoAction";
/// The handler that opens the folder of the drive.
pub const OPEN_FOLDER: &str = "MSOpenFolder";

/// How many entries of a drive are looked at to tell what it holds.
const LOOKED_AT: usize = 400;

/// How long a text of the registry is read, in UTF-16 units.
const TEXT_LENGTH: usize = 1024;

const PICTURES: [&str; 15] = [
    "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "heic", "heif", "webp", "raw", "cr2", "nef",
    "arw", "dng",
];
const VIDEOS: [&str; 12] = [
    "mp4", "mov", "avi", "mkv", "wmv", "m4v", "mts", "m2ts", "3gp", "webm", "mpg", "mpeg",
];
const MUSIC: [&str; 9] = [
    "mp3", "wav", "wma", "flac", "m4a", "aac", "ogg", "opus", "aiff",
];

/// What kind of medium arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Medium {
    /// A drive on a device that is plugged in, as a USB stick.
    Removable,
    /// A disc.
    Optical,
    /// A card in a reader that stays.
    Card,
}

impl Medium {
    /// The bit of `NoDriveTypeAutoRun` for drives of this kind.
    fn policy_bit(self) -> u32 {
        match self {
            Self::Removable | Self::Card => 0x4,
            Self::Optical => 0x20,
        }
    }
}

/// What a look at a drive found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contents {
    pub pictures: usize,
    pub videos: usize,
    pub music: usize,
    /// How many entries were looked at in all.
    pub entries: usize,
    /// A `DCIM` folder, as cameras write.
    pub camera: bool,
    /// A `VIDEO_TS` folder, as a DVD has.
    pub dvd: bool,
    /// A `BDMV` folder, as a Blu-ray disc has.
    pub blu_ray: bool,
    /// Tracks of an audio CD (`.cda`).
    pub audio_cd: bool,
}

impl Contents {
    /// Looks at the drive at `root` and the folders right under it, at
    /// most [`LOOKED_AT`] entries. It reads the drive, so it is not done
    /// on the thread of the user interface.
    pub fn read(root: &Path) -> Self {
        let mut contents = Self::default();
        let mut folders = vec![(root.to_owned(), 0)];
        while let Some((folder, depth)) = folders.pop() {
            let Ok(entries) = fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                if contents.entries >= LOOKED_AT {
                    return contents;
                }
                contents.entries += 1;
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                if is_dir {
                    if depth == 0 {
                        contents.note_folder(&name);
                        folders.push((entry.path(), depth + 1));
                    }
                } else {
                    contents.note_file(&name);
                }
            }
        }
        contents
    }

    fn note_folder(&mut self, name: &str) {
        match name.to_ascii_uppercase().as_str() {
            "DCIM" => self.camera = true,
            "VIDEO_TS" => self.dvd = true,
            "BDMV" => self.blu_ray = true,
            _ => {}
        }
    }

    fn note_file(&mut self, name: &str) {
        let Some((_, extension)) = name.rsplit_once('.') else {
            return;
        };
        let extension = extension.to_ascii_lowercase();
        let extension = extension.as_str();
        if PICTURES.contains(&extension) {
            self.pictures += 1;
        } else if VIDEOS.contains(&extension) {
            self.videos += 1;
        } else if MUSIC.contains(&extension) {
            self.music += 1;
        } else if extension == "cda" {
            self.audio_cd = true;
        }
    }

    /// The AutoPlay events for a `medium` holding this, the one that
    /// stands for the medium first, then the one of what it holds.
    pub fn events(&self, medium: Medium) -> Vec<&'static str> {
        let mut events = Vec::new();
        match medium {
            Medium::Optical => {
                if self.audio_cd {
                    events.push("PlayCDAudioOnArrival");
                }
                if self.blu_ray {
                    events.push("PlayBluRayOnArrival");
                }
                if self.dvd {
                    events.push("PlayDVDMovieOnArrival");
                }
                if self.entries == 0 {
                    events.push("HandleCDBurningOnArrival");
                }
            }
            Medium::Card => events.push(r"CameraAlternate\ShowPicturesOnArrival"),
            Medium::Removable => events.push("StorageOnArrival"),
        }
        let kinds = [self.pictures, self.videos, self.music]
            .iter()
            .filter(|&&count| count > 0)
            .count();
        let content = match kinds {
            0 => None,
            1 if self.pictures > 0 => Some("ShowPicturesOnArrival"),
            1 if self.videos > 0 => Some("PlayVideoFilesOnArrival"),
            1 => Some("PlayMusicFilesOnArrival"),
            _ => Some("MixedContentOnArrival"),
        };
        events.extend(content);
        if events.is_empty() {
            events.push("UnknownContentOnArrival");
        }
        events
    }
}

/// How a handler is run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invoke {
    /// The folder of the drive is opened, which the caller does.
    OpenFolder,
    /// Nothing is done.
    Nothing,
    /// The verb `verb` of the class `class`, with the root of the drive.
    Verb { class: String, verb: String },
    /// `IHWEventHandler` of the class `id`, initialized with `init`.
    Handler { id: String, init: String },
}

/// A handler AutoPlay offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoPlayHandler {
    /// Its name in the registry, which the choice of the user names.
    pub name: String,
    /// What it does, as "Import photos and videos".
    pub action: String,
    /// Who does it, as "Photos".
    pub provider: String,
    pub invoke: Invoke,
}

/// What is done by default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chosen {
    /// The user is asked.
    Ask,
    /// The handler of this name is run, for this event.
    Run { handler: String, event: String },
}

/// What AutoPlay offers for a drive that arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoPlayOffer {
    /// The event a choice is kept for: the one that stands for the medium.
    pub event: String,
    /// What the user chose before for this kind of drive.
    pub chosen: Chosen,
    /// The handlers to choose from, those of the first event first.
    pub handlers: Vec<AutoPlayHandler>,
}

impl AutoPlayOffer {
    /// The handler named `name`.
    pub fn handler(&self, name: &str) -> Option<&AutoPlayHandler> {
        self.handlers.iter().find(|handler| handler.name == name)
    }
}

/// The choice made for the first of `events` that has one, as `chosen`
/// reads it; asking where none has.
fn chosen_of(events: &[&str], chosen: impl Fn(&str) -> Option<String>) -> Chosen {
    for event in events {
        match chosen(event) {
            Some(name) if name == PROMPT => return Chosen::Ask,
            Some(name) if !name.trim().is_empty() => {
                return Chosen::Run {
                    handler: name,
                    event: (*event).to_owned(),
                };
            }
            _ => {}
        }
    }
    Chosen::Ask
}

/// The names of `lists` in order, each once, without the one that asks;
/// opening the folder and doing nothing are always among them.
fn handler_names(lists: impl IntoIterator<Item = Vec<String>>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut names: Vec<String> = { lists.into_iter().flatten() }
        .filter(|name| name != PROMPT)
        .filter(|name| seen.insert(name.to_ascii_lowercase()))
        .collect();
    for built_in in [OPEN_FOLDER, NOTHING] {
        if seen.insert(built_in.to_ascii_lowercase()) {
            names.push(built_in.to_owned());
        }
    }
    names
}

/// The last part of an event, as `HandleEvent` takes it.
fn event_type(event: &str) -> &str {
    event.rsplit('\\').next().unwrap_or(event)
}

/// AutoPlay for drives that arrive.
pub struct AutoPlay;

impl AutoPlay {
    /// What AutoPlay offers for the drive at `root`, a `medium`; `None`
    /// where AutoPlay is switched off for the user or by policy. It reads
    /// the drive, so it is not done on the thread of the user interface.
    pub fn offer(root: &Path, medium: Medium) -> Option<AutoPlayOffer> {
        if !Self::allowed(root, medium) {
            return None;
        }
        let events = Contents::read(root).events(medium);
        let chosen = chosen_of(&events, user_choice);
        let lists = events.iter().map(|event| event_handlers(event));
        let handlers = { handler_names(lists).into_iter() }
            .filter_map(|name| read_handler(&name))
            .collect();
        Some(AutoPlayOffer {
            event: events[0].to_owned(),
            chosen,
            handlers,
        })
    }

    /// Whether AutoPlay is on for a `medium` at `root`: not switched off
    /// with `DisableAutoplay`, and not for its kind of drive by the policy
    /// `NoDriveTypeAutoRun`.
    fn allowed(root: &Path, medium: Medium) -> bool {
        if registry_dword(HKEY_CURRENT_USER, AUTOPLAY, "DisableAutoplay") == Some(1) {
            return false;
        }
        let policy = registry_dword(HKEY_CURRENT_USER, POLICY, "NoDriveTypeAutoRun")
            .or_else(|| registry_dword(HKEY_LOCAL_MACHINE, POLICY, "NoDriveTypeAutoRun"));
        let mut bit = medium.policy_bit();
        let wide = HSTRING::from(root.as_os_str());
        // SAFETY: the root is NUL-terminated and outlives the call.
        let kind = unsafe { GetDriveTypeW(&wide) };
        if kind < 32 {
            bit = 1 << kind;
        }
        policy.is_none_or(|policy| policy & bit == 0)
    }

    /// Keeps `handler` as what is done for `event` from now on, as the
    /// choice in Settings > AutoPlay does.
    pub fn remember(event: &str, handler: &str) -> Result<()> {
        let path = format!(r"{AUTOPLAY}\UserChosenExecuteHandlers\{event}");
        let (key, _) = Key::create(HKEY_CURRENT_USER, &path, REG_SAM_FLAGS(0), false)
            .map_err(|failure| failure.into_error("the choice of AutoPlay was not kept"))?;
        key.set_text("", handler)
            .map_err(|failure| failure.into_error("the choice of AutoPlay was not kept"))
    }

    /// Runs `handler` for `event` with the drive at `root`, on a thread of
    /// its own, as a handler may wait; a failure is reported. Opening the
    /// folder and doing nothing are the caller's.
    pub fn run(handler: &AutoPlayHandler, event: &str, root: &Path) {
        let (invoke, name) = (handler.invoke.clone(), handler.action.clone());
        let (event, root) = (event_type(event).to_owned(), root.to_owned());
        ShellThread::run_alone("autoplay", move || {
            let ran = match &invoke {
                Invoke::OpenFolder | Invoke::Nothing => Ok(()),
                Invoke::Verb { class, verb } => run_verb(class, verb, &root),
                Invoke::Handler { id, init } => run_handler(id, init, &event, &root),
            };
            if let Err(error) = ran {
                report!("AutoPlay did not {name}: {error:#}");
            }
        });
    }
}

/// The handler the user chose for `event`, in Settings > AutoPlay or else
/// as the default of the event.
fn user_choice(event: &str) -> Option<String> {
    ["UserChosenExecuteHandlers", "EventHandlersDefaultSelection"]
        .iter()
        .find_map(|choices| {
            let key = format!(r"{AUTOPLAY}\{choices}\{event}");
            registry_text(HKEY_CURRENT_USER, &key, None, TEXT_LENGTH)
                .or_else(|| {
                    let parent = format!(r"{AUTOPLAY}\{choices}");
                    registry_text(HKEY_CURRENT_USER, &parent, Some(event), TEXT_LENGTH)
                })
                .filter(|name| !name.is_empty())
        })
}

/// The names of the handlers registered for `event`, of the user first.
fn event_handlers(event: &str) -> Vec<String> {
    let path = format!(r"{AUTOPLAY}\EventHandlers\{event}");
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .filter_map(|root| Key::read(root, &path, REG_SAM_FLAGS(0)))
        .flat_map(|key| key.values().into_iter().map(|(name, _)| name))
        .filter(|name| !name.is_empty())
        .collect()
}

/// The handler `name` as the registry describes it, of the user first;
/// `None` for one that is not there or cannot be run.
fn read_handler(name: &str) -> Option<AutoPlayHandler> {
    let path = format!(r"{AUTOPLAY}\Handlers\{name}");
    let key = [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .find_map(|root| Key::read(root, &path, REG_SAM_FLAGS(0)));
    let text = |value: &str| {
        key.as_ref()
            .and_then(|key| key.expanded_text(value))
            .filter(|text| !text.trim().is_empty())
    };
    let invoke = match name {
        OPEN_FOLDER => Invoke::OpenFolder,
        NOTHING => Invoke::Nothing,
        _ => match (text("InvokeProgID"), text("CLSID"), text("ProgID")) {
            (Some(class), _, _) => Invoke::Verb {
                class,
                verb: text("InvokeVerb").unwrap_or_else(|| "open".to_owned()),
            },
            (None, Some(id), _) => Invoke::Handler {
                id,
                init: text("InitCmdLine").unwrap_or_default(),
            },
            (None, None, Some(prog_id)) => Invoke::Handler {
                id: class_of_prog_id(&prog_id)?,
                init: text("InitCmdLine").unwrap_or_default(),
            },
            _ => return None,
        },
    };
    let action = text("Action").map(|action| indirect(&action));
    let built_in = matches!(invoke, Invoke::OpenFolder | Invoke::Nothing);
    // A handler of another program without a key is not offered.
    if key.is_none() && !built_in {
        return None;
    }
    Some(AutoPlayHandler {
        name: name.to_owned(),
        action: action.unwrap_or_else(|| name.to_owned()),
        provider: text("Provider")
            .map(|provider| indirect(&provider))
            .unwrap_or_default(),
        invoke,
    })
}

/// The class a `ProgID` names, from its `CLSID` key.
fn class_of_prog_id(prog_id: &str) -> Option<String> {
    let path = format!(r"{prog_id}\CLSID");
    registry_text(HKEY_CLASSES_ROOT, &path, None, TEXT_LENGTH)
}

/// `text` with an indirect string (`@shell32.dll,-17166`,
/// `@{Package?ms-resource://...}`) read from where it points; as it is
/// where it is none or cannot be read.
fn indirect(text: &str) -> String {
    if !text.starts_with('@') {
        return text.to_owned();
    }
    let source = HSTRING::from(text);
    let mut buffer = [0u16; TEXT_LENGTH];
    // SAFETY: the source is NUL-terminated and the buffer carries its
    // length; both outlive the call.
    match unsafe { SHLoadIndirectString(&source, &mut buffer, None) } {
        Ok(()) => text_in(&buffer),
        Err(_) => text.to_owned(),
    }
}

/// Has the Shell do the verb `verb` of the class `class` with `root`.
fn run_verb(class: &str, verb: &str, root: &Path) -> Result<()> {
    let (class, verb) = (HSTRING::from(class), HSTRING::from(verb));
    let file = HSTRING::from(root.as_os_str());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_CLASSNAME,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpClass: PCWSTR(class.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: the structure has its size, and every string it points to is
    // NUL-terminated and outlives the call.
    unsafe { ShellExecuteExW(&raw mut info) }.context("the Shell refused the verb")
}

/// Has the handler of the class `id` handle `event` for the drive at
/// `root`, as Explorer does: the root is both the device and its other
/// name.
fn run_handler(id: &str, init: &str, event: &str, root: &Path) -> Result<()> {
    let id = HSTRING::from(id);
    let (init, event) = (HSTRING::from(init), HSTRING::from(event));
    let root = HSTRING::from(root.as_os_str());
    // SAFETY: COM is initialized on this thread by `run_alone`; every
    // string is NUL-terminated and outlives the calls.
    unsafe {
        let class = CLSIDFromString(&id).context("the handler names no class")?;
        let handler: IHWEventHandler =
            CoCreateInstance(&class, None, CLSCTX_INPROC_SERVER | CLSCTX_LOCAL_SERVER)
                .context("the handler did not start")?;
        handler.Initialize(&init)?;
        handler
            .HandleEvent(&root, &root, &event)
            .context("the handler refused the drive")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contents(pictures: usize, videos: usize, music: usize) -> Contents {
        Contents {
            pictures,
            videos,
            music,
            entries: pictures + videos + music,
            ..Contents::default()
        }
    }

    #[test]
    fn a_drive_is_named_by_its_medium_and_then_by_what_it_holds() {
        assert_eq!(
            contents(3, 0, 0).events(Medium::Removable),
            ["StorageOnArrival", "ShowPicturesOnArrival"]
        );
        assert_eq!(
            contents(0, 2, 1).events(Medium::Removable),
            ["StorageOnArrival", "MixedContentOnArrival"]
        );
        assert_eq!(
            contents(0, 0, 0).events(Medium::Card),
            [r"CameraAlternate\ShowPicturesOnArrival"]
        );
        let disc = Contents {
            dvd: true,
            entries: 2,
            ..Contents::default()
        };
        assert_eq!(disc.events(Medium::Optical), ["PlayDVDMovieOnArrival"]);
        let blank = Contents::default();
        assert_eq!(blank.events(Medium::Optical), ["HandleCDBurningOnArrival"]);
        let data = Contents {
            entries: 5,
            ..Contents::default()
        };
        assert_eq!(data.events(Medium::Optical), ["UnknownContentOnArrival"]);
    }

    #[test]
    fn names_and_folders_tell_what_a_drive_holds() {
        let mut found = Contents::default();
        for name in [
            "a.JPG",
            "b.mp4",
            "c.flac",
            "Track01.cda",
            "notes.txt",
            "bare",
        ] {
            found.note_file(name);
        }
        found.note_folder("dcim");
        assert_eq!((found.pictures, found.videos, found.music), (1, 1, 1));
        assert!(found.audio_cd && found.camera && !found.dvd);
    }

    #[test]
    fn the_first_event_with_a_choice_decides() {
        let events = ["StorageOnArrival", "ShowPicturesOnArrival"];
        let chosen = |event: &str| match event {
            "ShowPicturesOnArrival" => Some("PhotosImport".to_owned()),
            _ => None,
        };
        assert_eq!(
            chosen_of(&events, chosen),
            Chosen::Run {
                handler: "PhotosImport".to_owned(),
                event: "ShowPicturesOnArrival".to_owned()
            }
        );
        let ask = |_: &str| Some(PROMPT.to_owned());
        assert_eq!(chosen_of(&events, ask), Chosen::Ask);
        assert_eq!(chosen_of(&events, |_| None), Chosen::Ask);
    }

    #[test]
    fn every_handler_is_offered_once() {
        let names = handler_names([
            vec!["PhotosImport".to_owned(), PROMPT.to_owned()],
            vec!["photosimport".to_owned(), OPEN_FOLDER.to_owned()],
        ]);
        assert_eq!(names, ["PhotosImport", OPEN_FOLDER, NOTHING]);
    }

    #[test]
    fn a_handler_gets_the_last_part_of_its_event() {
        assert_eq!(
            event_type(r"CameraAlternate\ShowPicturesOnArrival"),
            "ShowPicturesOnArrival"
        );
        assert_eq!(event_type("StorageOnArrival"), "StorageOnArrival");
    }
}
