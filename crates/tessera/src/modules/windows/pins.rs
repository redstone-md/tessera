// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What an entry of the dock's `pinned` option stands for: an application
//! (a program, a shortcut, a packaged application or a name of the Start
//! menu), a file, a folder, a web address or a link of any registered
//! scheme, or a separator; what each pin is called, which icon it shows,
//! what a click on it opens; and the changes of the list the dock makes
//! when pins are dragged, dropped and removed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::App;

use super::{Docks, pin_name, same_file};
use crate::files::Files;
use crate::i18n::Language;
use crate::icons::{AppIcons, Icon, is_link};
use crate::platform::{AppWindow, Launch, Shortcut, app_id_in, app_path, scheme_handler};

/// The entry of `pinned` that stands for a separator.
pub const SEPARATOR: &str = "separator";

/// The endings of files that are started as programs and grouped with
/// their windows, rather than opened as documents.
const PROGRAMS: [&str; 7] = ["exe", "com", "bat", "cmd", "msc", "lnk", "appref-ms"];

/// The endings whose files are named without them: programs and the
/// shortcuts of the Shell and of the web.
const NAMED_WITHOUT_ENDING: [&str; 8] =
    ["exe", "com", "bat", "cmd", "msc", "lnk", "appref-ms", "url"];

/// What a pin stands for.
#[derive(Clone)]
pub(super) enum Target {
    /// An application, whose windows the pin's button stands for while
    /// they are open.
    App {
        /// What a click opens while the application has no window.
        shortcut: Shortcut,
        /// The program file its windows belong to, when that is known.
        program: Option<PathBuf>,
        /// The packaged application its windows belong to, if it is one.
        app: Option<String>,
    },
    /// A file that is opened with its program.
    File(PathBuf),
    /// A folder, opened in the file manager.
    Folder(PathBuf),
    /// A web address or a link of another scheme, with the program that
    /// opens links of its scheme where Windows names one.
    Address {
        link: String,
        handler: Option<PathBuf>,
    },
    /// A thin line between buttons.
    Separator,
}

/// One entry of `pinned`, with what it stands for.
#[derive(Clone)]
pub(crate) struct Pin {
    /// The entry as `pinned` writes it.
    pub(super) entry: String,
    /// Its place in `pinned`, counting the entries that stand for nothing.
    pub(super) place: usize,
    pub(super) target: Target,
}

/// What an entry names, as far as its text tells, and the disk where it is
/// a path.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Named {
    Separator,
    /// `shell:AppsFolder\` and the identifier of a packaged application.
    Packaged,
    /// A program file or a shortcut.
    Program,
    File,
    Folder,
    /// A link of a scheme, as `https://` or `mailto:`.
    Address,
    /// A path that names nothing on the disk.
    Missing,
    /// What the Start menu calls an application.
    StartMenu,
}

/// What `entry` names; `is_dir` tells, for an absolute path, whether it is
/// a folder (`Some(true)`), a file (`Some(false)`) or nothing.
fn named(entry: &str, is_dir: impl FnOnce(&Path) -> Option<bool>) -> Named {
    if entry.trim().eq_ignore_ascii_case(SEPARATOR) {
        return Named::Separator;
    }
    let path = Path::new(entry);
    if app_id_in(path).is_some() {
        return Named::Packaged;
    }
    if path.is_absolute() {
        return match is_dir(path) {
            None => Named::Missing,
            Some(true) => Named::Folder,
            Some(false) if has_ending(path, &PROGRAMS) => Named::Program,
            Some(false) => Named::File,
        };
    }
    match scheme_of(entry) {
        Some(_) => Named::Address,
        None => Named::StartMenu,
    }
}

/// Whether the file name of `path` ends in one of `endings`, whatever
/// their case.
fn has_ending(path: &Path, endings: &[&str]) -> bool {
    { path.extension().and_then(|ending| ending.to_str()) }.is_some_and(|ending| {
        endings
            .iter()
            .any(|known| known.eq_ignore_ascii_case(ending))
    })
}

/// The scheme of a link, as `https` of `https://example.org`: letters,
/// digits, `+`, `-` and `.` after a first letter, at least two of them so
/// that a drive letter is none, and something after the colon.
pub(super) fn scheme_of(text: &str) -> Option<&str> {
    let (scheme, rest) = text.split_once(':')?;
    let mut letters = scheme.chars();
    let first = letters.next()?;
    let fits = first.is_ascii_alphabetic()
        && scheme.len() >= 2
        && letters.all(|letter| letter.is_ascii_alphanumeric() || "+-.".contains(letter));
    (fits && !rest.trim().is_empty()).then_some(scheme)
}

/// What a link is called: the host of a web address without `www.`, and
/// for other schemes what follows the scheme, as the address of a mail.
fn link_title(link: &str) -> String {
    let Some(scheme) = scheme_of(link) else {
        return link.to_owned();
    };
    let rest = &link[scheme.len() + 1..];
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    let web = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
    let end = match web {
        true => rest.find(['/', '?', '#', ':']),
        false => rest.find(['?', '#']),
    };
    let named = &rest[..end.unwrap_or(rest.len())];
    // A user and a password before the host are not shown.
    let named = match web {
        true => named.rsplit('@').next().unwrap_or(named),
        false => named,
    };
    let named = match web {
        true => named.strip_prefix("www.").unwrap_or(named),
        false => named,
    };
    match named.is_empty() {
        true => link.to_owned(),
        false => named.to_owned(),
    }
}

/// What a packaged application is called while nothing better is known:
/// the name of its package without the publisher, as `WindowsCalculator`
/// of `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App`.
fn packaged_title(id: &str) -> String {
    let family = id.split(['!', '_']).next().unwrap_or(id);
    let name = family.rsplit('.').next().unwrap_or(family);
    match name.is_empty() {
        true => id.to_owned(),
        false => name.to_owned(),
    }
}

/// What a pinned path is called: its file name, without the ending for
/// programs and shortcuts; the path itself for the root of a drive.
fn path_title(path: &Path) -> String {
    let name = match has_ending(path, &NAMED_WITHOUT_ENDING) {
        true => path.file_stem(),
        false => path.file_name(),
    };
    match name {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.to_string_lossy().into_owned(),
    }
}

thread_local! {
    /// The program that opens the links of each scheme, by its name in
    /// lower case, as it was asked for once.
    static HANDLERS: RefCell<HashMap<String, Option<PathBuf>>> = RefCell::default();
}

/// The program that opens links of `scheme`, asked once.
fn handler_of(scheme: &str) -> Option<PathBuf> {
    let scheme = scheme.to_lowercase();
    HANDLERS.with(|handlers| {
        let known = handlers.borrow().get(&scheme).cloned();
        known.unwrap_or_else(|| {
            let found = scheme_handler(&scheme);
            handlers.borrow_mut().insert(scheme, found.clone());
            found
        })
    })
}

/// The arguments that hand `paths` and `links` to a program, each in
/// quotes.
pub(super) fn quoted_arguments(paths: &[PathBuf], links: &[String]) -> String {
    let paths = paths.iter().map(|path| path.to_string_lossy().into_owned());
    let all: Vec<String> = { paths.chain(links.iter().cloned()) }
        .map(|item| format!("\"{}\"", item.replace('"', "")))
        .collect();
    all.join(" ")
}

impl Pin {
    /// The pins `names` stand for, in their order. A name is the path of
    /// a program, a shortcut, a file or a folder, the path that names a
    /// packaged application, a link (`https://…`, `mailto:…`), the word
    /// `separator`, or what the Start menu calls an application; one that
    /// is none of these is reported and skipped.
    pub(super) fn find(names: &[String]) -> Vec<Self> {
        let kinds: Vec<Named> = { names.iter() }
            .map(|name| named(name, |path| path.exists().then(|| path.is_dir())))
            .collect();
        let start_menu = match kinds.contains(&Named::StartMenu) {
            true => Shortcut::start_menu(),
            false => Vec::new(),
        };
        let pins = { names.iter().zip(kinds).enumerate() }.filter_map(|(place, (name, kind))| {
            let target = Self::target(name, kind, &start_menu);
            if target.is_none() {
                report!("nothing to pin is called `{name}`");
            }
            Some(Self {
                entry: name.clone(),
                place,
                target: target?,
            })
        });
        pins.collect()
    }

    /// What `name`, of the kind `kind`, stands for; `None` for nothing.
    fn target(name: &str, kind: Named, start_menu: &[Shortcut]) -> Option<Target> {
        let path = Path::new(name);
        let app = |shortcut: Shortcut| Some(Self::app_of(shortcut));
        match kind {
            Named::Separator => Some(Target::Separator),
            Named::Packaged => app(Shortcut {
                name: name.to_owned(),
                path: app_path(app_id_in(path)?),
            }),
            Named::Program => app(Shortcut {
                name: name.to_owned(),
                path: path.to_owned(),
            }),
            Named::File => Some(Target::File(path.to_owned())),
            Named::Folder => Some(Target::Folder(path.to_owned())),
            Named::Address => Some(Target::Address {
                link: name.to_owned(),
                handler: scheme_of(name).and_then(handler_of),
            }),
            Named::Missing => None,
            Named::StartMenu => {
                let known = |shortcut: &&Shortcut| shortcut.name.eq_ignore_ascii_case(name);
                app(start_menu.iter().find(known).cloned()?)
            }
        }
    }

    fn app_of(shortcut: Shortcut) -> Target {
        let app = app_id_in(&shortcut.path).map(str::to_owned);
        let program = if app.is_some() {
            None
        } else if is_link(&shortcut.path) {
            shortcut.link().target
        } else {
            Some(shortcut.path.clone())
        };
        Target::App {
            shortcut,
            program,
            app,
        }
    }

    /// What the settings call the pin `entry` of `pinned`: the name of a
    /// program or of a shortcut, the name of a folder or a file, the host
    /// of a web address, what the Start menu calls an application, and
    /// for a separator the English text "Separator", which
    /// `Language::translate` gives in the language shown. It reads
    /// nothing from the disk.
    pub(crate) fn title(entry: &str) -> String {
        match named(entry, |_| Some(false)) {
            Named::Separator => "Separator".to_owned(),
            Named::Packaged => packaged_title(app_id_in(Path::new(entry)).unwrap_or(entry)),
            Named::Address => link_title(entry),
            Named::StartMenu => entry.to_owned(),
            Named::Program | Named::File | Named::Folder | Named::Missing => {
                path_title(Path::new(entry))
            }
        }
    }

    /// The icon the dock draws for the pin `entry` of `pinned`; `None`
    /// for a separator, for an application of the Start menu that no
    /// dock shows, and where none is found.
    #[allow(
        dead_code,
        reason = "the Dock card of settings-core draws pins with it"
    )]
    pub(crate) fn icon(entry: &str, cx: &App) -> Icon {
        let kind = named(entry, |path| path.exists().then(|| path.is_dir()));
        if kind == Named::StartMenu {
            return Docks::pin_icon(entry, cx);
        }
        let target = Self::target(entry, kind, &[])?;
        let pin = Self {
            entry: entry.to_owned(),
            place: 0,
            target,
        };
        pin.icon_in(cx.global::<AppIcons>(), false)
    }

    /// What the dock calls the pin, in `language`.
    pub(super) fn name(&self, language: Language) -> String {
        match &self.target {
            Target::Separator => language.say("Separator").to_owned(),
            Target::App { shortcut, .. }
                if named(&self.entry, |_| Some(false)) == Named::StartMenu =>
            {
                shortcut.name.clone()
            }
            _ => Self::title(&self.entry),
        }
    }

    /// The icon of the pin; one of a pack that follows the theme in its
    /// primary color when `active`, as for the application in front.
    pub(super) fn icon_in(&self, icons: &AppIcons, active: bool) -> Icon {
        match &self.target {
            Target::App { shortcut, .. } => icons.of_shortcut_as(shortcut, active),
            Target::File(path) | Target::Folder(path) => icons.of_file(path),
            Target::Address { handler, .. } => {
                let handler = handler.as_deref()?;
                icons.of_executable_as(handler, active)
            }
            Target::Separator => None,
        }
    }

    pub(super) fn is_separator(&self) -> bool {
        matches!(self.target, Target::Separator)
    }

    /// The shortcut an application is started with.
    pub(super) fn shortcut(&self) -> Option<&Shortcut> {
        match &self.target {
            Target::App { shortcut, .. } => Some(shortcut),
            _ => None,
        }
    }

    /// The packaged application of the pin, if it is one.
    pub(super) fn app(&self) -> Option<&str> {
        match &self.target {
            Target::App { app, .. } => app.as_deref(),
            _ => None,
        }
    }

    /// The program file whose windows belong to the pin, when known.
    pub(super) fn program(&self) -> Option<&Path> {
        match &self.target {
            Target::App { program, .. } => program.as_deref(),
            _ => None,
        }
    }

    /// The file or folder Open file location shows: the shortcut or the
    /// program file of an application, a pinned file, or the folder
    /// itself, for which its parent opens with it chosen.
    pub(super) fn location(&self) -> Option<&Path> {
        match &self.target {
            Target::App { shortcut, app, .. } => app.is_none().then_some(&shortcut.path),
            Target::File(path) | Target::Folder(path) => Some(path),
            Target::Address { .. } | Target::Separator => None,
        }
    }

    /// Whether files and links dropped on the pin are opened with it: the
    /// program or shortcut of an application, which takes them as
    /// arguments. A packaged application is started without them, so it
    /// takes none.
    pub(super) fn takes_files(&self) -> bool {
        matches!(&self.target, Target::App { app: None, .. })
    }

    /// Whether `window` belongs to the pinned application.
    pub(super) fn owns(&self, window: &AppWindow) -> bool {
        let Target::App { program, app, .. } = &self.target else {
            return false;
        };
        if let (Some(app), Some(shown)) = (app, &window.app_id) {
            return app.eq_ignore_ascii_case(shown);
        }
        let (Some(program), Some(executable)) = (program, &window.executable) else {
            return false;
        };
        !window.is_own() && same_file(program, executable)
    }

    /// Opens what the pin stands for: starts the application, opens the
    /// file with its program, the folder in the file manager and the link
    /// with the program of its scheme.
    pub(super) fn open(&self, cx: &mut App) {
        match &self.target {
            Target::App { shortcut, .. } => shortcut.launch(),
            Target::File(path) => Launch::new(path)
                .named(format!("`{}`", path.display()))
                .start(),
            Target::Folder(path) => {
                if !Files::open_folder(path, cx) {
                    Launch::new(path)
                        .named(format!("`{}`", path.display()))
                        .start();
                }
            }
            Target::Address { link, .. } => Launch::new(link).named(format!("`{link}`")).start(),
            Target::Separator => {}
        }
    }

    /// Opens `paths` and `links` with the pinned application, as Explorer
    /// does with what is dropped on a pinned button of its taskbar.
    pub(super) fn open_with(&self, paths: &[PathBuf], links: &[String]) {
        let Some(shortcut) = self.shortcut().filter(|_| self.takes_files()) else {
            return report!("`{}` takes nothing dropped on it", self.entry);
        };
        Launch::new(&shortcut.path)
            .arguments(quoted_arguments(paths, links))
            .named(format!("`{}`", shortcut.name))
            .start();
    }
}

/// How `pinned` names a file, a folder or a program dropped on the dock,
/// and a link as it is.
pub(super) fn entry_of(path: &Path) -> String {
    pin_name(path)
}

/// The place of `entry` in `pins`: `at` where it is there, or else the
/// first place where it is, whatever its case.
fn place_of(pins: &[String], at: usize, entry: &str) -> Option<usize> {
    let same = |pin: &String| pin.eq_ignore_ascii_case(entry);
    { pins.get(at).filter(|pin| same(pin)).map(|_| at) }.or_else(|| pins.iter().position(same))
}

/// `pins` with `entry`, last seen at `from`, put at `to`, which counts the
/// places without it; unchanged where it is not there.
pub(super) fn moved(pins: &[String], from: usize, entry: &str, to: usize) -> Vec<String> {
    let mut pins = pins.to_vec();
    let Some(from) = place_of(&pins, from, entry) else {
        return pins;
    };
    let pin = pins.remove(from);
    pins.insert(to.min(pins.len()), pin);
    pins
}

/// `pins` with `entries` put in at `to`, counted before they are put in;
/// one that is pinned already, but a separator, leaves its old place.
pub(super) fn inserted(pins: &[String], to: usize, entries: &[String]) -> Vec<String> {
    let new = |pin: &String| {
        !pin.eq_ignore_ascii_case(SEPARATOR)
            && entries.iter().any(|entry| entry.eq_ignore_ascii_case(pin))
    };
    let before = pins.iter().take(to).filter(|pin| new(pin)).count();
    let mut kept: Vec<String> = pins.iter().filter(|pin| !new(pin)).cloned().collect();
    let at = to.saturating_sub(before).min(kept.len());
    kept.splice(at..at, entries.iter().cloned());
    kept
}

/// `pins` without `entry`, last seen at `at`: that one place only, so that
/// of several separators the one meant goes.
pub(super) fn removed(pins: &[String], at: usize, entry: &str) -> Vec<String> {
    let mut pins = pins.to_vec();
    if let Some(at) = place_of(&pins, at, entry) {
        pins.remove(at);
    }
    pins
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(entry: &str, is_dir: Option<bool>) -> Named {
        named(entry, |_| is_dir)
    }

    #[test]
    fn an_entry_is_told_by_its_text_and_the_disk() {
        assert_eq!(kind("Separator", None), Named::Separator);
        assert_eq!(kind(" separator ", None), Named::Separator);
        assert_eq!(
            kind(
                "shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
                None
            ),
            Named::Packaged
        );
        assert_eq!(kind("C:/Apps/app.exe", Some(false)), Named::Program);
        assert_eq!(kind("C:/Menu/App.LNK", Some(false)), Named::Program);
        assert_eq!(kind("C:/Docs/report.pdf", Some(false)), Named::File);
        assert_eq!(kind("C:/Docs/Node.js", Some(true)), Named::Folder);
        assert_eq!(kind("C:/Gone/app.exe", None), Named::Missing);
        assert_eq!(kind("https://example.org/a", None), Named::Address);
        assert_eq!(kind("mailto:me@example.org", None), Named::Address);
        assert_eq!(kind("ms-settings:display", None), Named::Address);
        assert_eq!(kind("Paint", None), Named::StartMenu);
        assert_eq!(kind("Visual Studio Code", None), Named::StartMenu);
    }

    #[test]
    fn a_scheme_is_no_drive_letter_and_needs_something_after_it() {
        assert_eq!(scheme_of("https://example.org"), Some("https"));
        assert_eq!(scheme_of("ms-settings:display"), Some("ms-settings"));
        assert_eq!(scheme_of("C:/Apps"), None);
        assert_eq!(scheme_of("mailto:"), None);
        assert_eq!(scheme_of("1password:open"), None);
        assert_eq!(scheme_of("Paint"), None);
    }

    #[test]
    fn a_pin_is_called_by_what_it_names() {
        assert_eq!(
            Pin::title("https://www.example.org/path?q=1"),
            "example.org"
        );
        assert_eq!(Pin::title("http://user:secret@host.net:8080/"), "host.net");
        assert_eq!(
            Pin::title("mailto:me@example.org?subject=Hi"),
            "me@example.org"
        );
        assert_eq!(Pin::title("https://"), "https://");
        assert_eq!(Pin::title("C:/Apps/Editor.exe"), "Editor");
        assert_eq!(Pin::title("C:/Menu/Mail.lnk"), "Mail");
        assert_eq!(Pin::title("C:/Links/News.url"), "News");
        assert_eq!(Pin::title("C:/Docs/report.pdf"), "report.pdf");
        assert_eq!(Pin::title("C:/Projects/Node.js"), "Node.js");
        assert_eq!(Pin::title("C:/"), "C:/");
        assert_eq!(Pin::title("separator"), "Separator");
        assert_eq!(Pin::title("Paint"), "Paint");
        assert_eq!(
            Pin::title("shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"),
            "WindowsCalculator"
        );
    }

    #[test]
    fn dropped_items_are_handed_over_in_quotes() {
        let paths = [PathBuf::from(r"C:\My Files\a.txt")];
        let links = ["https://example.org".to_owned()];
        assert_eq!(
            quoted_arguments(&paths, &links),
            r#""C:\My Files\a.txt" "https://example.org""#
        );
        assert_eq!(quoted_arguments(&[], &[]), "");
    }

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn a_dragged_pin_moves_and_the_one_meant_goes() {
        let pins = list(&["a", "separator", "b", "separator", "c"]);
        assert_eq!(
            moved(&pins, 0, "a", 2),
            list(&["separator", "b", "a", "separator", "c"])
        );
        assert_eq!(
            moved(&pins, 4, "C", 0),
            list(&["c", "a", "separator", "b", "separator"])
        );
        assert_eq!(moved(&pins, 9, "z", 0), pins);
        assert_eq!(
            removed(&pins, 3, "separator"),
            list(&["a", "separator", "b", "c"])
        );
        // The list changed meanwhile: the first one of that name goes.
        assert_eq!(
            removed(&pins, 0, "b"),
            list(&["a", "separator", "separator", "c"])
        );
    }

    #[test]
    fn a_dropped_item_is_pinned_once_where_it_is_dropped() {
        let pins = list(&["a", "b", "c"]);
        assert_eq!(
            inserted(&pins, 1, &list(&["x", "y"])),
            list(&["a", "x", "y", "b", "c"])
        );
        assert_eq!(inserted(&pins, 3, &list(&["A"])), list(&["b", "c", "A"]));
        assert_eq!(inserted(&pins, 0, &list(&["c"])), list(&["c", "a", "b"]));
        assert_eq!(inserted(&pins, 99, &list(&["separator"])).len(), 4);
        let separated = list(&["a", "separator"]);
        assert_eq!(
            inserted(&separated, 1, &list(&["separator"])),
            list(&["a", "separator", "separator"])
        );
    }
}
