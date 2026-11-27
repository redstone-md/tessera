// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the Start menu of Windows 11 recommends: the files opened lately,
//! from the Recent folder, and the applications installed lately, those
//! whose shortcuts in the Start menu were made in the last days. Each can
//! be taken away with its menu: a file by its shortcut in the Recent
//! folder, as Windows does, an application by remembering it in
//! `recommended.toml` in the configuration directory. Widget:
//! `recommended`.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Context, Entity, FontWeight, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div, img, px, relative,
};
use serde::{Deserialize, Serialize};

use super::{AppMenu, ItemKind, MenuSlot, Remover};
use crate::config::{config_dir, read_user_file};
use crate::files::Files;
use crate::i18n::Language;
use crate::icons::AppIcons;
use crate::module::WidgetContext;
use crate::motion::Interaction;
use crate::platform::{ShellThread, Shortcut, recent_files, recent_folder};
use crate::popup::Popups;
use crate::theme::Theme;
use crate::ui::{one_line, state_layer};

/// The file that remembers the applications taken away, in the
/// configuration directory.
const FILE: &str = "recommended.toml";

/// How many entries are shown unless the layout says, how many stand in
/// a row, and for how many days an application counts as new.
const DEFAULT_SHOWN: usize = 6;
const DEFAULT_COLUMNS: usize = 2;
const DEFAULT_DAYS: u64 = 7;

/// The side of the icon of an entry, and how high an entry is.
const ICON: Pixels = px(32.);
const ENTRY_HEIGHT: Pixels = px(52.);

/// How many icons are looked for while one frame is drawn.
const ICONS_PER_FRAME: usize = 2;

/// What an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    File,
    App,
}

/// Something recommended.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    kind: EntryKind,
    /// The file, or the shortcut of the application.
    shortcut: Shortcut,
}

impl Entry {
    /// What the file remembers an application taken away by.
    fn key(&self) -> String {
        format!("app:{}", self.shortcut.name.to_lowercase())
    }
}

/// What `recommended.toml` holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Remembered {
    /// The applications taken away, by [`Entry::key`].
    #[serde(default)]
    hidden: Vec<String>,
}

/// What the layout asks of the widget.
#[derive(Clone, Copy, Debug)]
struct Options {
    shown: usize,
    columns: usize,
    days: u64,
    files: bool,
    apps: bool,
}

impl Options {
    fn of(context: &WidgetContext) -> Self {
        let number = |key: &str, default: usize, most: usize| {
            context
                .number(key)
                .map_or(default, |number| (number as usize).clamp(1, most))
        };
        Self {
            shown: number("shown", DEFAULT_SHOWN, 24),
            columns: number("columns", DEFAULT_COLUMNS, 4),
            days: number("days", DEFAULT_DAYS as usize, 90) as u64,
            files: context.flag("files").unwrap_or(true),
            apps: context.flag("apps").unwrap_or(true),
        }
    }
}

/// The applications of `shortcuts` whose shortcut files were made since
/// `since`, the newest first, without those `hidden` names.
fn new_apps(shortcuts: Vec<Shortcut>, since: SystemTime, hidden: &[String]) -> Vec<Shortcut> {
    let mut found: Vec<(SystemTime, Shortcut)> = { shortcuts.into_iter() }
        .filter_map(|shortcut| {
            let made = fs::metadata(&shortcut.path).ok()?.created().ok()?;
            Some((made, shortcut))
        })
        .filter(|(made, shortcut)| {
            let key = format!("app:{}", shortcut.name.to_lowercase());
            *made >= since && !hidden.contains(&key)
        })
        .collect();
    found.sort_by_key(|(made, _)| std::cmp::Reverse(*made));
    found.into_iter().map(|(_, shortcut)| shortcut).collect()
}

/// `files` and `apps` as they are shown, at most `shown`: the new
/// applications first, as Windows puts them, then the files.
fn arranged(files: Vec<Shortcut>, apps: Vec<Shortcut>, shown: usize) -> Vec<Entry> {
    let apps = apps.into_iter().map(|shortcut| Entry {
        kind: EntryKind::App,
        shortcut,
    });
    let files = files.into_iter().map(|shortcut| Entry {
        kind: EntryKind::File,
        shortcut,
    });
    apps.chain(files).take(shown).collect()
}

/// The shortcut of the Recent folder that records the file `name`.
fn recent_link(name: &str) -> Option<PathBuf> {
    Some(recent_folder()?.join(format!("{name}.lnk")))
}

/// The recommended entries as a grid.
pub struct Recommended {
    options: Options,
    entries: Vec<Entry>,
    /// Whether they were read yet.
    read: bool,
    hidden: Vec<String>,
    interactions: Vec<Interaction>,
    menu: MenuSlot,
    icons_left: Cell<usize>,
    icons_owed: Cell<bool>,
}

impl Recommended {
    pub fn view(context: &WidgetContext, cx: &mut App) -> Entity<Self> {
        let options = Options::of(context);
        cx.new(|cx| {
            let mut this = Self {
                options,
                entries: Vec::new(),
                read: false,
                hidden: Vec::new(),
                interactions: Vec::new(),
                menu: MenuSlot::default(),
                icons_left: Cell::new(ICONS_PER_FRAME),
                icons_owed: Cell::new(false),
            };
            this.load(cx);
            this
        })
    }

    /// Reads the files opened lately and the applications installed
    /// lately, aside.
    fn load(&mut self, cx: &mut Context<Self>) {
        let options = self.options;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let remembered = cx
                .background_executor()
                .spawn(async { read_user_file::<Remembered>(FILE).unwrap_or_default() })
                .await;
            let files = match options.files {
                true => recent_files(options.shown).await,
                false => Vec::new(),
            };
            let files: Vec<Shortcut> = { files.into_iter() }
                .map(|place| Shortcut {
                    name: place.name,
                    path: place.path,
                })
                .collect();
            let apps = match options.apps {
                true => {
                    let hidden = remembered.hidden.clone();
                    let since = SystemTime::now() - Duration::from_secs(options.days * 86_400);
                    // The Start menu lists packaged applications through the
                    // Shell, which wants a thread of its own.
                    let listed =
                        ShellThread::call(move || new_apps(Shortcut::start_menu(), since, &hidden));
                    listed.recv().await.unwrap_or_default()
                }
                false => Vec::new(),
            };
            this.update(cx, |this, cx| {
                this.hidden = remembered.hidden;
                this.entries = arranged(files, apps, options.shown);
                this.read = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Takes the entry `entry` away, for good.
    fn remove(&mut self, entry: &Entry, cx: &mut Context<Self>) {
        self.entries.retain(|shown| shown != entry);
        cx.notify();
        match entry.kind {
            EntryKind::File => {
                let name = entry.shortcut.name.clone();
                cx.background_executor()
                    .spawn(async move {
                        let removed = recent_link(&name).map(fs::remove_file);
                        if let Some(Err(error)) = removed {
                            report!("`{name}` stays among the files opened lately: {error}");
                        }
                    })
                    .detach();
            }
            EntryKind::App => {
                self.hidden.push(entry.key());
                let remembered = Remembered {
                    hidden: self.hidden.clone(),
                };
                cx.background_executor()
                    .spawn(async move {
                        if let Err(error) = write(&remembered) {
                            report!("{FILE} was not written: {error}");
                        }
                    })
                    .detach();
            }
        }
    }

    fn open(&self, entry: &Entry, cx: &mut Context<Self>) {
        if !Files::open_folder(&entry.shortcut.path, cx) {
            entry.shortcut.launch();
        }
        cx.defer(Popups::close);
    }

    fn open_menu(&mut self, index: usize, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(index).cloned() else {
            return;
        };
        let kind = match entry.kind {
            EntryKind::File => ItemKind::File,
            EntryKind::App => ItemKind::App,
        };
        let this = cx.entity().downgrade();
        let shortcut = entry.shortcut.clone();
        let remove: Remover = Rc::new(move |cx: &mut App| {
            this.update(cx, |this, cx| this.remove(&entry, cx)).ok();
        });
        let make = move |cx: &mut Context<AppMenu>| AppMenu::new(shortcut, kind, Some(remove), cx);
        self.menu
            .open(event.position, make, |this: &mut Self| &mut this.menu, cx);
    }

    /// The icon of `entry`, or a blank square where it has none or it is
    /// to be looked for in a later frame.
    fn icon(&self, entry: &Entry, theme: &Theme, cx: &App) -> AnyElement {
        let icons = cx.global::<AppIcons>();
        let path = &entry.shortcut.path;
        let known = match entry.kind {
            EntryKind::App => icons.has(path),
            EntryKind::File => icons.has_of_file(path),
        };
        let may = known || self.icons_left.get() > 0;
        if !known && may {
            self.icons_left.set(self.icons_left.get() - 1);
        }
        self.icons_owed.set(self.icons_owed.get() || !may);
        let image = may
            .then(|| match entry.kind {
                EntryKind::App => icons.of_shortcut(&entry.shortcut),
                EntryKind::File => icons.of_file(path),
            })
            .flatten();
        match image {
            Some(image) => img(image).size(ICON).flex_none().into_any_element(),
            None => div()
                .size(ICON)
                .flex_none()
                .rounded(theme.shape.small)
                .bg(theme.colors.outline)
                .into_any_element(),
        }
    }

    fn tile(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        while self.interactions.len() <= index {
            self.interactions.push(Interaction::new(&theme.motion));
        }
        let state = self.interactions[index].state(window);
        let entry = self.entries[index].clone();
        let detail = match entry.kind {
            EntryKind::App => language.say("Recently added").to_owned(),
            EntryKind::File => folder_of(&entry.shortcut.path).unwrap_or_default(),
        };
        let image = self.icon(&entry, &theme, cx);
        let tile = div()
            .id(("recommended", index))
            .w(relative(1. / self.options.columns as f32))
            .h(ENTRY_HEIGHT)
            .px(theme.spacing.small)
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .rounded(theme.shape.medium)
            .bg(state_layer(&theme, theme.colors.surface_container, state))
            .child(image)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(one_line(SharedString::new(entry.shortcut.name.clone())))
                    .child(
                        div()
                            .text_size(theme.typography.label_size)
                            .text_color(theme.colors.on_surface_variant)
                            .child(one_line(SharedString::new(detail))),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.open(&entry, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_menu(index, event, cx);
                }),
            );
        Interaction::track(tile, cx, move |this: &mut Self| {
            this.interactions.get_mut(index)
        })
        .into_any_element()
    }
}

/// What an entry of a file says under its name: the folder it lies in.
fn folder_of(path: &Path) -> Option<String> {
    let folder = path.parent()?.file_name()?;
    Some(folder.to_string_lossy().into_owned())
}

/// Writes what is remembered to its file.
fn write(remembered: &Remembered) -> Result<(), String> {
    let directory = config_dir().ok_or("there is no configuration directory")?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let text = toml::to_string(remembered).map_err(|error| error.to_string())?;
    fs::write(directory.join(FILE), text).map_err(|error| error.to_string())
}

impl Render for Recommended {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        self.icons_left.set(ICONS_PER_FRAME);
        let tiles: Vec<AnyElement> = { 0..self.entries.len() }
            .map(|index| self.tile(index, window, cx))
            .collect();
        if self.icons_owed.replace(false) {
            window.request_animation_frame();
        }
        let body = match (self.read, tiles.is_empty()) {
            (true, true) => div()
                .py(theme.spacing.small)
                .text_color(theme.colors.on_surface_variant)
                .child(
                    language.say("The files you open and the applications you install show here"),
                )
                .into_any_element(),
            _ => div().flex().flex_wrap().children(tiles).into_any_element(),
        };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(language.say("Recommended")),
            )
            .child(body)
            .children(self.menu.render())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shortcut(name: &str) -> Shortcut {
        Shortcut {
            name: name.to_owned(),
            path: PathBuf::from(format!("{name}.lnk")),
        }
    }

    #[test]
    fn the_new_applications_come_before_the_files() {
        let files = vec![shortcut("report.docx"), shortcut("photo.jpg")];
        let apps = vec![shortcut("Paint")];
        let entries = arranged(files, apps, 2);
        let names: Vec<&str> = { entries.iter() }
            .map(|entry| entry.shortcut.name.as_str())
            .collect();
        assert_eq!(names, ["Paint", "report.docx"]);
        assert_eq!(entries[0].kind, EntryKind::App);
        assert_eq!(entries[0].key(), "app:paint");
    }

    #[test]
    fn only_shortcuts_made_lately_and_not_taken_away_are_new() {
        let folder = std::env::temp_dir().join(format!("tessera-new-apps-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let made = |name: &str| {
            let path = folder.join(format!("{name}.lnk"));
            fs::write(&path, "").unwrap();
            Shortcut {
                name: name.to_owned(),
                path,
            }
        };
        let shortcuts = vec![made("Fresh"), made("Hidden")];
        let missing = shortcut("Gone");
        let past = SystemTime::now() - Duration::from_secs(60);
        let hidden = ["app:hidden".to_owned()];
        let found = new_apps(
            vec![shortcuts[0].clone(), shortcuts[1].clone(), missing],
            past,
            &hidden,
        );
        let future = SystemTime::now() + Duration::from_secs(60);
        let none = new_apps(shortcuts, future, &[]);
        fs::remove_dir_all(&folder).unwrap();
        let names: Vec<&str> = found
            .iter()
            .map(|shortcut| shortcut.name.as_str())
            .collect();
        assert_eq!(names, ["Fresh"]);
        assert!(none.is_empty());
    }

    #[test]
    fn what_is_remembered_is_read_back() {
        let remembered = Remembered {
            hidden: vec!["app:paint".to_owned()],
        };
        let text = toml::to_string(&remembered).unwrap();
        assert_eq!(toml::from_str::<Remembered>(&text).unwrap(), remembered);
        assert_eq!(
            toml::from_str::<Remembered>("").unwrap(),
            Remembered::default()
        );
    }
}
