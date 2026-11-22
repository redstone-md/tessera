// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `update` widget: tells of a release of Tessera newer than the one
//! running, and a click opens its page, or, when asked to install, puts
//! the program of that release in the place of the one running and
//! starts it.

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, WeakEntity, Window, div,
};

use sha2::{Digest, Sha256};

use crate::extension::http::{download, download_into};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring};
use crate::platform::Shortcut;
use crate::theme::Theme;
use crate::ui::{icon, pill, state_layer};

const ICON_UPDATE: &str = "\u{e896}";

/// Where Tessera is released, unless the widget names another place.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// How long after the start the releases are first asked for, so that the
/// start itself is not held up, and the hours between two checks.
const FIRST_CHECK: Duration = Duration::from_secs(20);
const HOURS_BETWEEN_CHECKS: f64 = 24.;

/// The most of the list of releases that is read.
const FEED_LIMIT: u64 = 1 << 20;

/// What comes before the name of a release in the list of releases.
const TAG_MARK: &str = "/releases/tag/";

/// The file of a release that is the program, the most of it that is
/// read, and the most of the file beside it that says its checksum.
const PROGRAM: &str = "tessera.exe";
const PROGRAM_LIMIT: u64 = 256 << 20;
const CHECKSUM_LIMIT: u64 = 4096;

/// How much of itself the widget shows while the program is fetched.
const BUSY_OPACITY: f32 = 0.6;

pub struct UpdateModule;

impl Module for UpdateModule {
    fn name(&self) -> &'static str {
        "update"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("update", |context, cx| {
            let repository = context.text("repository").unwrap_or(REPOSITORY);
            let asked = Asked {
                repository: repository.trim_end_matches('/').to_owned(),
                hours: context
                    .number("hours")
                    .unwrap_or(HOURS_BETWEEN_CHECKS)
                    .max(1.),
                previews: context.flag("previews").unwrap_or(false),
                install: context.flag("install").unwrap_or(false),
            };
            cx.new(|cx| Update::new(asked, cx)).into()
        })
    }
}

/// What the layout asks of the widget.
#[derive(Clone)]
struct Asked {
    /// The address of the repository on GitHub.
    repository: String,
    /// The hours between two checks.
    hours: f64,
    /// Whether a release that is a preview counts.
    previews: bool,
    /// Whether a click installs the release instead of opening its page.
    install: bool,
}

/// The checksum a file of checksums names first: 64 digits to the base
/// of 16, in small letters.
fn checksum_named(text: &str) -> Option<String> {
    let word = text.split_whitespace().next()?;
    let usable = word.len() == 64 && word.bytes().all(|byte| byte.is_ascii_hexdigit());
    usable.then(|| word.to_ascii_lowercase())
}

/// The file beside `executable` whose name ends in `word` as well.
fn beside(executable: &Path, word: &str) -> PathBuf {
    let mut name = executable.as_os_str().to_owned();
    name.push(".");
    name.push(word);
    name.into()
}

/// Writes to a file and sums up what was written.
struct Summed {
    file: File,
    sum: Sha256,
}

impl Write for Summed {
    fn write(&mut self, part: &[u8]) -> io::Result<usize> {
        let written = self.file.write(part)?;
        self.sum.update(&part[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Puts `fresh` in the place of `executable`, which may be running, and
/// keeps what was there beside it with `.old` at its name.
fn swap(executable: &Path, fresh: &Path) -> io::Result<()> {
    let old = beside(executable, "old");
    // One from an earlier update may be left.
    let _ = fs::remove_file(&old);
    fs::rename(executable, &old)?;
    fs::rename(fresh, executable).inspect_err(|_| {
        let _ = fs::rename(&old, executable);
    })
}

/// Fetches the program of `release` from `repository` and puts it in the
/// place of `executable`, if it is the file the release says it is.
fn fetch(repository: &str, release: &str, executable: &Path) -> Result<(), String> {
    let address = format!("{repository}/releases/download/{release}/{PROGRAM}");
    let said = download(&format!("{address}.sha256"), CHECKSUM_LIMIT)?;
    let wanted = checksum_named(&String::from_utf8_lossy(&said))
        .ok_or("the release names no checksum for the program")?;
    let fresh = beside(executable, "new");
    let fetched = File::create(&fresh)
        .map_err(|error| error.to_string())
        .and_then(|file| {
            let mut summed = Summed {
                file,
                sum: Sha256::new(),
            };
            download_into(&address, PROGRAM_LIMIT, &mut summed)?;
            summed.flush().map_err(|error| error.to_string())?;
            let sum = summed.sum.finalize();
            let got: String = sum.iter().map(|byte| format!("{byte:02x}")).collect();
            match got == wanted {
                true => Ok(()),
                false => Err("the program is not the one the release names".to_owned()),
            }
        })
        .and_then(|()| swap(executable, &fresh).map_err(|error| error.to_string()));
    if fetched.is_err() {
        let _ = fs::remove_file(&fresh);
    }
    fetched
}

/// One part of what follows the hyphen of a version.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    /// Numbers come before words.
    Number(u64),
    Word(String),
}

/// A version as releases are named: three numbers, and for a preview a
/// hyphen and parts between dots.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Version {
    numbers: [u64; 3],
    preview: Vec<Part>,
}

impl Version {
    /// Reads `name`, with or without a `v` before it; what follows a `+`
    /// does not count.
    fn parse(name: &str) -> Option<Self> {
        let name = name.strip_prefix('v').unwrap_or(name);
        let plain = |letter: char| letter.is_ascii_alphanumeric() || ".-+".contains(letter);
        if !name.chars().all(plain) {
            return None;
        }
        let name = name.split('+').next()?;
        let (numbers, preview) = match name.split_once('-') {
            Some((numbers, preview)) => (numbers, Some(preview)),
            None => (name, None),
        };
        let mut read = numbers.split('.').map(str::parse::<u64>);
        let numbers = [read.next()?.ok()?, read.next()?.ok()?, read.next()?.ok()?];
        if read.next().is_some() {
            return None;
        }
        let part = |part: &str| match part.parse() {
            Ok(number) => Part::Number(number),
            Err(_) => Part::Word(part.to_owned()),
        };
        let preview: Vec<Part> = preview
            .map(|preview| preview.split('.').map(part).collect())
            .unwrap_or_default();
        Some(Self { numbers, preview })
    }

    fn is_preview(&self) -> bool {
        !self.preview.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let numbers = self.numbers.cmp(&other.numbers);
        // A preview comes before the release it leads to.
        numbers.then_with(|| match (self.is_preview(), other.is_preview()) {
            (false, false) => Ordering::Equal,
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (true, true) => self.preview.cmp(&other.preview),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The name of the newest release in `feed`, the list of releases as
/// GitHub gives it, that is newer than `running`; previews count only
/// with `previews`. Names that are no versions are passed over.
fn newer_release(feed: &str, running: &Version, previews: bool) -> Option<String> {
    let names = feed.split(TAG_MARK).skip(1).filter_map(|rest| {
        let name = rest.split(['"', '<', '?', '#', '&', '/']).next()?;
        let version = Version::parse(name)?;
        (previews || !version.is_preview()).then_some((version, name))
    });
    let (version, name) = names.max_by(|one, other| one.0.cmp(&other.0))?;
    (version > *running).then(|| name.to_owned())
}

/// Shows the newer release that was last found, and nothing without one.
struct Update {
    asked: Asked,
    /// The name of the release that is newer than the one running.
    found: Option<String>,
    interaction: Interaction,
    /// 0 without a newer release, 1 once one is shown.
    shown: Spring,
    /// Whether the program of the release is being fetched.
    installing: bool,
    /// 1 while it is.
    busy: Spring,
    /// Whether that went wrong, so that a click opens the page.
    failed: bool,
}

impl Update {
    fn new(asked: Asked, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let every = Duration::from_secs_f64(asked.hours * 3600.);
        let feed = format!("{}/releases.atom", asked.repository);
        let previews = asked.previews;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(FIRST_CHECK).await;
            loop {
                let feed = feed.clone();
                let found = cx.background_executor().spawn(async move {
                    let running = Version::parse(env!("CARGO_PKG_VERSION"))?;
                    let feed = download(&feed, FEED_LIMIT)
                        .inspect_err(|error| report!("the releases were not read: {error}"))
                        .ok()?;
                    newer_release(&String::from_utf8_lossy(&feed), &running, previews)
                });
                let found = found.await;
                if this.update(cx, |this, cx| this.take(found, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(every).await;
            }
        })
        .detach();
        Self {
            asked,
            found: None,
            interaction: Interaction::new(&motion),
            shown: Spring::new(motion.effects, 0.),
            installing: false,
            busy: Spring::new(motion.effects, 0.),
            failed: false,
        }
    }

    /// Opens the page of `release` in the browser.
    fn open(&self, release: &str) {
        let page = format!("{}{TAG_MARK}{release}", self.asked.repository);
        let page = Shortcut {
            name: page.clone(),
            path: PathBuf::from(&page),
        };
        page.launch();
    }

    /// What a click does: opens the page of `release`, or installs it.
    fn act(&mut self, release: String, cx: &mut Context<Self>) {
        if !self.asked.install || self.failed {
            return self.open(&release);
        }
        if self.installing {
            return;
        }
        self.installing = true;
        self.busy.animate_to(1.);
        cx.notify();
        let repository = self.asked.repository.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let fetched = cx.background_executor().spawn(async move {
                let executable = std::env::current_exe().map_err(|error| error.to_string())?;
                fetch(&repository, &release, &executable)
            });
            let fetched = fetched.await;
            let told = this.update(cx, |this, cx| match fetched {
                Ok(()) => crate::shell::restart(cx),
                Err(why) => {
                    report!("the update was not installed: {why}");
                    this.installing = false;
                    this.failed = true;
                    this.busy.animate_to(0.);
                    cx.notify();
                }
            });
            told.ok();
        })
        .detach();
    }

    /// Takes in what a check found; one that found nothing keeps what an
    /// earlier one found, since the newer release is still there.
    fn take(&mut self, found: Option<String>, cx: &mut Context<Self>) {
        if found.is_none() || found == self.found {
            return;
        }
        self.found = found;
        self.shown.animate_to(1.);
        cx.notify();
    }
}

impl Render for Update {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let Some(found) = self.found.clone() else {
            return div().into_any_element();
        };
        let shown = self.shown.value(window).clamp(0., 1.);
        let state = self.interaction.state(window);
        let busy = self.busy.value(window).clamp(0., 1.);
        let release = found.clone();
        let said = match self.installing {
            true => Language::of(cx).say("Updating").into(),
            false => found,
        };
        let button = pill(&theme)
            .id("update")
            .opacity(shown * (1. - (1. - BUSY_OPACITY) * busy))
            .bg(state_layer(&theme, theme.colors.secondary_container, state))
            .text_color(theme.colors.on_surface)
            .on_click(cx.listener(move |this, _, _, cx| this.act(release.clone(), cx)))
            .child(icon(&theme, ICON_UPDATE))
            .child(div().font_weight(FontWeight::MEDIUM).child(said));
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(name: &str) -> Version {
        Version::parse(name).unwrap()
    }

    #[test]
    fn versions_are_read_and_ordered() {
        assert_eq!(version("v1.2.3").numbers, [1, 2, 3]);
        assert_eq!(version("1.2.3+build"), version("v1.2.3"));
        assert!(Version::parse("1.2").is_none());
        assert!(Version::parse("1.2.3.4").is_none());
        assert!(Version::parse("nightly").is_none());
        assert!(Version::parse("1.2.3-rc 1").is_none());
        assert!(version("0.2.0") > version("0.2.0-dev"));
        assert!(version("0.2.0-dev") > version("0.1.0-alpha.22"));
        assert!(version("0.1.0-alpha.22") > version("0.1.0-alpha.9"));
        assert!(version("0.1.0-beta") > version("0.1.0-alpha.22"));
        assert!(version("0.1.0-alpha.1") > version("0.1.0-alpha"));
        assert!(version("0.10.0") > version("0.9.9"));
    }

    #[test]
    fn a_checksum_is_read_from_its_file() {
        let sum = "AB".repeat(32);
        let named = checksum_named(&format!("{sum}  tessera.exe\n"));
        assert_eq!(named, Some("ab".repeat(32)));
        assert_eq!(checksum_named(""), None);
        assert_eq!(checksum_named("abc tessera.exe"), None);
        assert_eq!(checksum_named(&"zz".repeat(32)), None);
    }

    #[test]
    fn the_new_program_takes_the_place_of_the_old_one() {
        let folder = std::env::temp_dir().join(format!("tessera-swap-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let program = folder.join("tessera.exe");
        let fresh = beside(&program, "new");
        fs::write(&program, "old").unwrap();
        fs::write(beside(&program, "old"), "older").unwrap();
        fs::write(&fresh, "new").unwrap();
        swap(&program, &fresh).unwrap();
        assert_eq!(fs::read_to_string(&program).unwrap(), "new");
        assert_eq!(fs::read_to_string(beside(&program, "old")).unwrap(), "old");
        assert!(!fresh.exists());
        // Without a new one the old one stays where it is.
        assert!(swap(&program, &fresh).is_err());
        assert_eq!(fs::read_to_string(&program).unwrap(), "new");
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn what_is_written_is_summed_up() {
        let folder = std::env::temp_dir().join(format!("tessera-sum-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let mut summed = Summed {
            file: File::create(folder.join("file")).unwrap(),
            sum: Sha256::new(),
        };
        summed.write_all(b"abc").unwrap();
        let sum = summed.sum.finalize();
        let sum: String = { sum.iter() }.map(|byte| format!("{byte:02x}")).collect();
        assert!(sum.starts_with("ba7816bf8f01cfea"));
        assert_eq!(fs::read(folder.join("file")).unwrap(), b"abc");
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn the_running_version_is_one() {
        assert!(Version::parse(env!("CARGO_PKG_VERSION")).is_some());
    }

    #[test]
    fn the_newest_release_is_told_only_when_it_is_newer() {
        let feed = r#"<feed>
            <entry><link href="https://github.com/a/b/releases/tag/v0.3.0-rc.1"/></entry>
            <entry><link href="https://github.com/a/b/releases/tag/v0.2.1"/></entry>
            <entry><link href="https://github.com/a/b/releases/tag/nightly"/></entry>
            <content>&lt;a href=&quot;/a/b/releases/tag/v0.2.1-rc.2&quot;&gt;v9.9.9&lt;/a&gt;</content>
            <entry><link href="https://github.com/a/b/releases/tag/v0.2.0"/></entry>
        </feed>"#;
        let found = |running: &str, previews| newer_release(feed, &version(running), previews);
        assert_eq!(found("0.2.0", false).as_deref(), Some("v0.2.1"));
        assert_eq!(found("0.2.0", true).as_deref(), Some("v0.3.0-rc.1"));
        assert_eq!(found("0.2.1", false), None);
        assert_eq!(found("0.3.0", true), None);
        assert_eq!(
            newer_release("<feed></feed>", &version("0.1.0"), true),
            None
        );
    }

    /// Asks GitHub for the releases of Tessera; run by hand.
    #[test]
    #[ignore = "asks github.com"]
    fn the_releases_of_tessera_are_read() {
        let feed = download(&format!("{REPOSITORY}/releases.atom"), FEED_LIMIT).unwrap();
        let feed = String::from_utf8_lossy(&feed);
        let found = newer_release(&feed, &version("0.0.1"), true);
        println!("found: {found:?}");
        assert!(found.is_some_and(|name| Version::parse(&name).is_some()));
    }
}
