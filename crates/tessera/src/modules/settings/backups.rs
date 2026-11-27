// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Backups card of General: the settings exported to a folder with the
//! theme files they name, a configuration file imported, a backup of
//! config.toml restored, and the built-in configuration put back. Every
//! write of the file through `ConfigFile::change` keeps a backup, once a
//! minute at most (config/edit.rs). What reads or writes the disk is done
//! away from the thread that draws.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    AnyElement, App, AsyncApp, Context, Global, IntoElement, ParentElement, PathPromptOptions,
    SharedString, Styled, Task, WeakEntity, Window, div,
};

use super::{Card, Control, Face, Section, Settings, card, note, wrap};
use crate::config::edit::{
    BACKUPS_KEPT, ConfigFile, backups_dir, list_backups, replace_config, reset_config,
};
use crate::config::{Config, config_dir, theme_file};
use crate::i18n::Language;
use crate::popup::Popups;
use crate::shell::Shell;
use crate::theme::Theme;

/// How long after the settings give way to a dialog it opens, so that it
/// is not closed with them.
const DIALOG_AFTER: Duration = Duration::from_millis(350);

/// The name the folder of exported settings is offered under.
const EXPORT_NAME: &str = "Tessera settings";

/// The English title of the card, which `open_at` names it by.
const TITLE: &str = "Backups";

/// What a button of the card does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum BackupDeed {
    /// Asks for a folder and writes the settings into it.
    Export,
    /// Asks for a configuration file and puts it in place of config.toml.
    Import,
    /// Shows the backups there are, or hides them.
    List,
    /// Puts the backup at this place of the list in place of config.toml.
    Restore(usize),
    /// Asks whether every setting is to go back to its default.
    AskReset,
    /// Puts the built-in configuration in place of config.toml.
    Reset,
    /// Leaves the settings as they are.
    KeepSettings,
}

/// A backup as the card lists it.
#[derive(Clone, Debug)]
struct Listed {
    path: PathBuf,
    time: chrono::NaiveDateTime,
    /// How many keys it holds otherwise than config.toml; none where it
    /// cannot be read.
    changed: Option<usize>,
}

/// What the card keeps.
#[derive(Default)]
pub(super) struct BackupState {
    /// The backups, once they have been read for the list.
    listed: Option<Vec<Listed>>,
    /// Whether the list is shown.
    open: bool,
    /// Whether it is asked whether to reset every setting.
    asking: bool,
    working: Option<Task<()>>,
}

/// What the last export, import, restore or reset came to, said on the
/// card, which may be shown again after a dialog.
#[derive(Default)]
struct BackupNews(Option<String>);

impl Global for BackupNews {}

/// The keys at any depth of `table`, each by its path, with its value.
fn leaves(table: &toml::Table) -> Vec<(String, &toml::Value)> {
    let mut leaves = Vec::new();
    for (key, value) in table {
        match value {
            toml::Value::Table(inner) => {
                for (path, value) in self::leaves(inner) {
                    leaves.push((format!("{key}.{path}"), value));
                }
            }
            other => leaves.push((key.clone(), other)),
        }
    }
    leaves
}

/// How many keys `one` holds otherwise than `other`: with another value,
/// or that only one of them holds. A list is one key.
pub(super) fn changed_keys(one: &toml::Table, other: &toml::Table) -> usize {
    let (ones, others) = (leaves(one), leaves(other));
    let differs = |(path, value): &(String, &toml::Value), against: &[(String, &toml::Value)]| {
        !against
            .iter()
            .any(|(known, held)| known == path && held == value)
    };
    let changed = ones.iter().filter(|leaf| differs(leaf, &others)).count();
    let added = { others.iter() }
        .filter(|(path, _)| !ones.iter().any(|(known, _)| known == path))
        .count();
    changed + added
}

/// The theme files `config` names, the base and those stacked over it,
/// that are files of the configuration directory.
fn theme_files(config: &Config) -> Vec<String> {
    let names = config.theme.iter().chain(&config.theme_layers);
    names.map(|name| theme_file(name)).collect()
}

/// Writes config.toml and the theme files it names into `folder`, which
/// is made; how many files were written is returned.
fn export_to(folder: &Path) -> Result<usize, String> {
    let source = ConfigFile::location()?;
    let text = fs::read_to_string(&source)
        .map_err(|error| format!("{} cannot be read: {error}", source.display()))?;
    let failed = |error: std::io::Error| format!("{} cannot be written: {error}", folder.display());
    fs::create_dir_all(folder).map_err(failed)?;
    fs::write(folder.join("config.toml"), &text).map_err(failed)?;
    let mut written = 1;
    let config: Config = toml::from_str(&text).map_err(|error| error.to_string())?;
    let directory = config_dir().ok_or("there is no configuration directory")?;
    for file in theme_files(&config) {
        let from = directory.join(&file);
        if !from.is_file() {
            // The colors of Windows and of the wallpaper are no files.
            continue;
        }
        let to = folder.join(&file);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(failed)?;
        }
        fs::copy(&from, &to).map_err(failed)?;
        written += 1;
    }
    Ok(written)
}

/// Puts the configuration file at `path` in place of config.toml, after
/// backing that up, with the theme files it names that stand beside it,
/// as an export writes them. A file the shell could not use is refused.
fn import_from(path: &Path) -> Result<(), String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
    let config: Config = toml::from_str(&text).map_err(|error| {
        format!(
            "{} is no configuration Tessera can use: {error}",
            path.display()
        )
    })?;
    let directory = config_dir().ok_or("there is no configuration directory")?;
    if let Some(beside) = path.parent() {
        for file in theme_files(&config) {
            let from = beside.join(&file);
            if !from.is_file() || beside == directory {
                continue;
            }
            let to = directory.join(&file);
            let failed =
                |error: std::io::Error| format!("{} cannot be written: {error}", to.display());
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).map_err(failed)?;
            }
            fs::copy(&from, &to).map_err(failed)?;
        }
    }
    replace_config(&text)
}

/// The backups there are, each with how many keys it holds otherwise
/// than config.toml.
fn read_backups() -> Vec<Listed> {
    let now = ConfigFile::open().ok().and_then(|file| file.table());
    { list_backups().into_iter() }
        .map(|(path, time)| {
            let held = fs::read_to_string(&path).ok();
            let held = held.and_then(|text| toml::from_str::<toml::Table>(&text).ok());
            let changed = { held.zip(now.as_ref()) }.map(|(held, now)| changed_keys(&held, now));
            Listed {
                path,
                time,
                changed,
            }
        })
        .collect()
}

/// Where the dialogs of export and import open.
fn documents() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    let documents = home.as_ref().map(|home| home.join("Documents"));
    { documents.filter(|path| path.is_dir()) }
        .or(home)
        .or_else(config_dir)
        .unwrap_or_default()
}

/// Says `news` on the card.
fn tell(news: String, cx: &mut App) {
    cx.set_global(BackupNews(Some(news)));
}

impl Settings {
    /// Does what a button of the Backups card does.
    pub(super) fn act_backup(&mut self, deed: BackupDeed, cx: &mut Context<Self>) {
        match deed {
            BackupDeed::Export => self.export(cx),
            BackupDeed::Import => self.import(cx),
            BackupDeed::List => {
                self.backups.open = !self.backups.open;
                if self.backups.open {
                    self.list_backups(cx);
                }
                cx.notify();
            }
            BackupDeed::Restore(index) => {
                let listed = self
                    .backups
                    .listed
                    .as_ref()
                    .and_then(|listed| listed.get(index));
                let Some(path) = listed.map(|listed| listed.path.clone()) else {
                    return;
                };
                self.replace(cx, "The backup was restored.", move || {
                    let text = fs::read_to_string(&path)
                        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
                    replace_config(&text)
                });
            }
            BackupDeed::AskReset => {
                self.backups.asking = true;
                cx.notify();
            }
            BackupDeed::KeepSettings => {
                self.backups.asking = false;
                cx.notify();
            }
            BackupDeed::Reset => {
                self.backups.asking = false;
                self.replace(cx, "Every setting is back to its default.", reset_config);
            }
        }
    }

    /// Reads the backups for the list, away from the thread that draws.
    fn list_backups(&mut self, cx: &mut Context<Self>) {
        let reading = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let listed = cx
                .background_executor()
                .spawn(async { read_backups() })
                .await;
            this.update(cx, |this, cx| {
                this.backups.listed = Some(listed);
                cx.notify();
            })
            .ok();
        });
        self.backups.working = Some(reading);
    }

    /// Has `replace` put another file in place of config.toml, away from
    /// the thread that draws; then the settings read the file again and
    /// say `done`, or why it was not done.
    fn replace(
        &mut self,
        cx: &mut Context<Self>,
        done: &'static str,
        replace: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) {
        let working = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let replaced = cx.background_executor().spawn(async { replace() }).await;
            this.update(cx, |this, cx| {
                let news = match replaced {
                    Ok(()) => Language::of(cx).say(done).to_owned(),
                    Err(error) => {
                        report!("{error}");
                        error
                    }
                };
                tell(news, cx);
                this.reread(cx);
                if this.backups.open {
                    this.list_backups(cx);
                }
            })
            .ok();
        });
        self.backups.working = Some(working);
    }

    /// Asks for a folder, in a dialog the settings give way to, and writes
    /// the settings into it; the settings open again at the card.
    fn export(&mut self, cx: &mut Context<Self>) {
        cx.defer(Popups::close);
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            cx.background_executor().timer(DIALOG_AFTER).await;
            let Ok(chosen) =
                cx.update(|cx| cx.prompt_for_new_path(&documents(), Some(EXPORT_NAME)))
            else {
                return;
            };
            let folder = match chosen.await {
                Ok(Ok(Some(folder))) => Some(folder),
                Ok(Err(error)) => {
                    report!("no folder can be chosen: {error:#}");
                    None
                }
                _ => None,
            };
            if let Some(folder) = folder {
                let target = folder.clone();
                let written = cx
                    .background_executor()
                    .spawn(async move { export_to(&target) })
                    .await;
                cx.update(|cx| {
                    let news = match written {
                        Ok(count) => format!(
                            "{} {} ({count})",
                            Language::of(cx).say("The settings were exported to"),
                            folder.display()
                        ),
                        Err(error) => error,
                    };
                    tell(news, cx);
                })
                .ok();
            }
            cx.update(|cx| Self::open_at(Section::General.title(), Some(TITLE), cx))
                .ok();
        })
        .detach();
    }

    /// Asks for a configuration file, in a dialog the settings give way
    /// to, and puts it in place of config.toml; the settings open again at
    /// the card.
    fn import(&mut self, cx: &mut Context<Self>) {
        let prompt: SharedString = Language::of(cx).say("Import…").into();
        cx.defer(Popups::close);
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            cx.background_executor().timer(DIALOG_AFTER).await;
            let options = PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(prompt),
            };
            let Ok(chosen) = cx.update(|cx| cx.prompt_for_paths(options)) else {
                return;
            };
            let path = match chosen.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(error)) => {
                    report!("no file can be chosen: {error:#}");
                    None
                }
                _ => None,
            };
            if let Some(path) = path {
                let imported = cx
                    .background_executor()
                    .spawn(async move { import_from(&path) })
                    .await;
                cx.update(|cx| {
                    let news = match imported {
                        Ok(()) => {
                            Shell::config_changed(cx);
                            Language::of(cx)
                                .say("The settings were imported.")
                                .to_owned()
                        }
                        Err(error) => error,
                    };
                    tell(news, cx);
                })
                .ok();
            }
            cx.update(|cx| Self::open_at(Section::General.title(), Some(TITLE), cx))
                .ok();
        })
        .detach();
    }

    /// The Backups card.
    pub(super) fn backups_card(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Card {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut chip = |this: &mut Self, deed: BackupDeed, text: &'static str, selected: bool| {
            let face = Face::Chip {
                text: language.say(text).into(),
                selected,
            };
            this.control(Control::Backup(deed), face, window, cx)
        };
        let buttons = vec![
            chip(self, BackupDeed::Export, "Export…", true),
            chip(self, BackupDeed::Import, "Import…", true),
            chip(
                self,
                BackupDeed::List,
                "Restore a backup…",
                self.backups.open,
            ),
            chip(
                self,
                BackupDeed::AskReset,
                "Reset to defaults",
                self.backups.asking,
            ),
        ];
        let about = format!(
            "{} {BACKUPS_KEPT}.",
            language.say(
                "Before a change is written, config.toml is copied to the backups folder, once a minute at most. Backups kept:"
            )
        );
        let mut section = card(&theme, language.say(TITLE))
            .child(note(&theme, about))
            .child(wrap(&theme).children(buttons));
        let mut weight = 4;
        if self.backups.asking {
            let ask = "Every setting goes back to what Tessera comes with; config.toml is backed up first.";
            let reset = chip(self, BackupDeed::Reset, "Reset", true);
            let keep = chip(self, BackupDeed::KeepSettings, "Cancel", false);
            section = section
                .child(note(&theme, language.say(ask)))
                .child(wrap(&theme).child(reset).child(keep));
            weight += 2;
        }
        if self.backups.open {
            section = section.child(self.backup_list(window, cx));
            weight += self.backups.listed.as_ref().map_or(1, Vec::len);
        }
        let news = cx
            .try_global::<BackupNews>()
            .and_then(|news| news.0.clone());
        section = section.children(news.map(|news| note(&theme, news)));
        if let Some(directory) = backups_dir() {
            let folder = format!("{} {}", language.say("Folder:"), directory.display());
            section = section.child(
                note(&theme, folder).text_size(theme.typography.label_size * super::CAPTION_SHARE),
            );
        }
        (weight, section.into_any_element())
    }

    /// The backups there are, each with its time, how many keys it holds
    /// otherwise, and a button that restores it.
    fn backup_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let Some(listed) = self.backups.listed.clone() else {
            return note(&theme, language.say("The backups are being read.")).into_any_element();
        };
        if listed.is_empty() {
            return note(&theme, language.say("No backup has been made yet.")).into_any_element();
        }
        let mut lines = Vec::new();
        for (index, backup) in listed.iter().enumerate() {
            let changed = match backup.changed {
                Some(count) => format!("{} {count}", language.say("Keys that differ:")),
                None => language.say("It cannot be read.").to_owned(),
            };
            let face = Face::Chip {
                text: language.say("Restore").into(),
                selected: false,
            };
            let restore = self.control(
                Control::Backup(BackupDeed::Restore(index)),
                face,
                window,
                cx,
            );
            lines.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.spacing.small)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme.colors.on_surface)
                            .child(format!("{} UTC", backup.time.format("%Y-%m-%d %H:%M:%S"))),
                    )
                    .child(note(&theme, changed))
                    .child(restore),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .children(lines)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn the_keys_that_differ_are_counted_at_any_depth() {
        let one =
            table("contrast = false\n[tiling]\ngap = 8\nborder = true\n[files]\nown = true\n");
        let other = table("contrast = false\n[tiling]\ngap = 4\n[night_light]\nfrom = \"21:00\"\n");
        // tiling.gap differs, tiling.border and files.own are only in
        // one, night_light.from only in the other.
        assert_eq!(changed_keys(&one, &other), 4);
        assert_eq!(changed_keys(&one, &one), 0);
        let lists = table("icon_packs = [\"a\", \"b\"]\n");
        let reordered = table("icon_packs = [\"b\", \"a\"]\n");
        assert_eq!(changed_keys(&lists, &reordered), 1);
    }

    #[test]
    fn an_export_names_the_theme_files_of_the_configuration() {
        let config: Config =
            toml::from_str("theme = \"dusk\"\ntheme_layers = [\"round\"]\n").unwrap();
        assert_eq!(
            theme_files(&config),
            ["themes/dusk.toml", "themes/round.toml"]
        );
    }
}
