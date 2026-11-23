// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Finds and starts applications from the Start menu, and opens the files
//! of the folders its `folders` option names. Widget: `launcher`.

use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, Entity, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Pixels, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div, img, point, px,
};

use crate::field::TextField;
use crate::i18n::Language;
use crate::icons::AppIcons;
use crate::module::{Module, WidgetRegistry};
use crate::modules::start::Pins;
use crate::motion::Spring;
use crate::platform::Shortcut;
use crate::popup::Popups;
use crate::theme::Theme;
use crate::ui::{icon, one_line};

const ICON_SEARCH: &str = "\u{e721}";
const ICON_PIN: &str = "\u{e718}";
const ICON_PINNED: &str = "\u{e841}";

/// The side of the button that pins the application of a row.
const PIN_BUTTON: Pixels = px(28.);

const DEFAULT_PLACEHOLDER: &str = "Search applications";

/// Rows prepared before the height of the list is known.
const ROWS_BEFORE_LAYOUT: usize = 16;

/// How many letters the search needs before it looks among the files,
/// and how many files it shows at most.
const LETTERS_FOR_FILES: usize = 2;
const FILES_SHOWN: usize = 30;

/// How deep under a folder files are looked for, the folder itself being
/// the first level, and how many are remembered at most.
const FOLDER_DEPTH: usize = 4;
const FILES_KEPT: usize = 20_000;

/// The attributes of a file or folder that is hidden or belongs to the
/// system.
const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;

/// The pages of the Windows settings the search finds, each with what
/// Windows opens it by.
const PAGES: [(&str, &str); 29] = [
    ("Display", "ms-settings:display"),
    ("Night light", "ms-settings:nightlight"),
    ("Sound", "ms-settings:sound"),
    ("Notifications", "ms-settings:notifications"),
    ("Power and battery", "ms-settings:powersleep"),
    ("Storage", "ms-settings:storagesense"),
    ("Bluetooth and devices", "ms-settings:bluetooth"),
    ("Printers and scanners", "ms-settings:printers"),
    ("Mouse", "ms-settings:mousetouchpad"),
    ("Network and internet", "ms-settings:network-status"),
    ("Wi-Fi", "ms-settings:network-wifi"),
    ("VPN", "ms-settings:network-vpn"),
    ("Personalization", "ms-settings:personalization"),
    ("Background", "ms-settings:personalization-background"),
    ("Colors", "ms-settings:colors"),
    ("Themes", "ms-settings:themes"),
    ("Lock screen", "ms-settings:lockscreen"),
    ("Installed apps", "ms-settings:appsfeatures"),
    ("Default apps", "ms-settings:defaultapps"),
    ("Startup apps", "ms-settings:startupapps"),
    ("Your account", "ms-settings:yourinfo"),
    ("Sign-in options", "ms-settings:signinoptions"),
    ("Date and time", "ms-settings:dateandtime"),
    ("Language and region", "ms-settings:regionlanguage"),
    ("Typing", "ms-settings:typing"),
    ("Accessibility", "ms-settings:easeofaccess"),
    ("Privacy and security", "ms-settings:privacy"),
    ("Windows Update", "ms-settings:windowsupdate"),
    ("About this computer", "ms-settings:about"),
];

const ICON_SETTINGS: &str = "\u{e713}";

pub struct LauncherModule;

impl Module for LauncherModule {
    fn name(&self) -> &'static str {
        "launcher"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("launcher", |context, cx| {
            let language = Language::of(cx);
            let placeholder = context.text("placeholder");
            let placeholder = placeholder.unwrap_or(language.say(DEFAULT_PLACEHOLDER));
            let placeholder = SharedString::new(placeholder);
            let folders = context.texts("folders").unwrap_or_default();
            let folders = folders.iter().map(|folder| folder_named(folder)).collect();
            let pages = match context.flag("settings") {
                Some(false) => Vec::new(),
                _ => pages(language),
            };
            cx.new(|cx| Launcher::new(placeholder, pages, folders, cx))
                .into()
        })
    }
}

/// How well `name` answers `query`, which is lowercase; lower is better.
fn rank(name: &str, query: &str) -> Option<u8> {
    let name = name.to_lowercase();
    let at = name.find(query)?;
    Some(if at == 0 {
        0
    } else if name[..at].ends_with(|before: char| !before.is_alphanumeric()) {
        1
    } else {
        2
    })
}

/// The positions in `names` of those containing `query`, ignoring case:
/// names that start with it first, then names with a word that does, then
/// the rest, each group in its given order.
fn search<'a>(names: impl Iterator<Item = &'a str>, query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let mut found: Vec<(u8, usize)> = names
        .enumerate()
        .filter_map(|(index, name)| Some((rank(name, &query)?, index)))
        .collect();
    found.sort_by_key(|&(rank, _)| rank);
    found.into_iter().map(|(_, index)| index).collect()
}

/// The pages of the Windows settings, named in `language`.
fn pages(language: Language) -> Vec<Shortcut> {
    let page = |&(name, address): &(&'static str, &str)| Shortcut {
        name: language.say(name).to_owned(),
        path: PathBuf::from(address),
    };
    PAGES.iter().map(page).collect()
}

/// The positions in `pages`, which are those of [`PAGES`] in a language,
/// of the ones whose name in that language or in English contains
/// `query`, the best first.
fn pages_matching(pages: &[Shortcut], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let mut found: Vec<(u8, usize)> = pages
        .iter()
        .zip(PAGES)
        .enumerate()
        .filter_map(|(index, (page, (english, _)))| {
            let ranks = [page.name.as_str(), english].map(|name| rank(name, &query));
            Some((ranks.into_iter().flatten().min()?, index))
        })
        .collect();
    found.sort_by_key(|&(rank, _)| rank);
    found.into_iter().map(|(_, index)| index).collect()
}

/// What a row of the list opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    App,
    Page,
    File,
}

/// The folder `name` stands for: a `~` it begins with is the folder of
/// the user.
fn folder_named(name: &str) -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    match (name.strip_prefix('~'), home) {
        (Some(rest), Some(home)) => home.join(rest.trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(name),
    }
}

/// The files and folders under `folders`, each with its name, down to
/// [`FOLDER_DEPTH`] and at most [`FILES_KEPT`] of them; hidden ones, those
/// of the system and those whose names begin with a dot are left out with
/// all that is in them. The nearest to a folder come first.
fn files_under(folders: &[PathBuf]) -> Vec<Shortcut> {
    let mut found = Vec::new();
    let mut level: Vec<PathBuf> = folders.to_vec();
    for _ in 0..FOLDER_DEPTH {
        let mut deeper = Vec::new();
        for folder in &level {
            let Ok(entries) = fs::read_dir(folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || metadata.file_attributes() & HIDDEN_OR_SYSTEM != 0 {
                    continue;
                }
                if found.len() == FILES_KEPT {
                    return found;
                }
                let path = entry.path();
                if metadata.is_dir() {
                    deeper.push(path.clone());
                }
                found.push(Shortcut { name, path });
            }
        }
        level = deeper;
    }
    found
}

/// What the row of a file says beside its name: the folder it lies in.
fn folder_of(path: &Path) -> Option<String> {
    let folder = path.parent()?.file_name()?;
    Some(folder.to_string_lossy().into_owned())
}

/// The first of `count` rows of height `row` to draw and how many, for a
/// list scrolled down by `scrolled` that is `viewport` high.
fn visible_rows(count: usize, row: f32, scrolled: f32, viewport: f32) -> (usize, usize) {
    if viewport <= 0. || row <= 0. {
        return (0, count.min(ROWS_BEFORE_LAYOUT));
    }
    let first = ((scrolled.max(0.) / row) as usize).min(count);
    let shown = (viewport / row).ceil() as usize + 1;
    (first, shown.min(count - first))
}

/// A search field over the list of Start menu shortcuts and the files of
/// the folders that are searched.
struct Launcher {
    /// The applications, after them the pages of the Windows settings and
    /// then the files.
    shortcuts: Vec<Shortcut>,
    /// How many of `shortcuts` are applications, and how many pages.
    apps: usize,
    pages: usize,
    /// The field the search is typed into.
    field: Entity<TextField>,
    /// Positions in `shortcuts` of those matching the query, best first.
    matches: Vec<usize>,
    /// The position in `matches` that Enter starts.
    selected: usize,
    /// The row the selection highlight is drawn at; follows `selected`.
    highlight: Spring,
    scroll: ScrollHandle,
    /// Whether the keys were given to the field yet.
    focused: bool,
    /// What is pinned to the Start menu, which a row can change.
    pins: Entity<Pins>,
}

impl Launcher {
    fn new(
        placeholder: SharedString,
        pages: Vec<Shortcut>,
        folders: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut shortcuts = Shortcut::start_menu();
        let apps = shortcuts.len();
        let pages = {
            shortcuts.extend(pages);
            shortcuts.len() - apps
        };
        let field = cx.new(|cx| TextField::new(placeholder, cx).with_glyph(ICON_SEARCH));
        cx.observe(&field, |this, _, cx| this.typed(cx)).detach();
        let pins = Pins::shared(cx);
        cx.observe(&pins, |_, _, cx| cx.notify()).detach();
        if !folders.is_empty() {
            // Folders are read aside: there may be many files in them.
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let files = cx
                    .background_executor()
                    .spawn(async move { files_under(&folders) });
                let files = files.await;
                this.update(cx, |this, cx| {
                    this.shortcuts.truncate(this.apps + this.pages);
                    this.shortcuts.extend(files);
                    this.find(cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        Self {
            pins,
            matches: (0..apps).collect(),
            apps,
            pages,
            shortcuts,
            field,
            selected: 0,
            highlight: Spring::new(cx.global::<Theme>().motion.spatial_fast, 0.),
            scroll: ScrollHandle::new(),
            focused: false,
        }
    }

    /// What the field shows and the list is searched for.
    fn text(&self, cx: &Context<Self>) -> String {
        self.field.read(cx).text()
    }

    /// The field says something else: the list starts over.
    fn typed(&mut self, cx: &mut Context<Self>) {
        self.find(cx);
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.select(0, cx);
        cx.notify();
    }

    /// What the entry of `shortcuts` at `index` is.
    fn kind(&self, index: usize) -> Kind {
        if index < self.apps {
            Kind::App
        } else if index < self.apps + self.pages {
            Kind::Page
        } else {
            Kind::File
        }
    }

    /// Looks for what the field asks for: among the applications always,
    /// and among the pages of the settings and the files once enough
    /// letters are written. The pages come after the applications and
    /// the files last.
    fn find(&mut self, cx: &Context<Self>) {
        let text = self.text(cx);
        let (apps, rest) = self.shortcuts.split_at(self.apps);
        let (pages, files) = rest.split_at(self.pages);
        fn names(shortcuts: &[Shortcut]) -> impl Iterator<Item = &str> {
            shortcuts.iter().map(|shortcut| shortcut.name.as_str())
        }
        let mut matches = search(names(apps), &text);
        if text.trim().chars().count() >= LETTERS_FOR_FILES {
            let pages = pages_matching(pages, &text);
            matches.extend(pages.into_iter().map(|index| index + self.apps));
            let files = search(names(files), &text);
            let files = files.into_iter().take(FILES_SHOWN);
            matches.extend(files.map(|index| index + self.apps + self.pages));
        }
        self.matches = matches;
        if self.selected >= self.matches.len() {
            self.selected = self.matches.len().saturating_sub(1);
        }
    }

    fn select(&mut self, row: usize, cx: &mut Context<Self>) {
        let row = row.min(self.matches.len().saturating_sub(1));
        if row != self.selected {
            self.selected = row;
            cx.notify();
        }
        self.highlight.animate_to(row as f32);
    }

    /// Selects the row `steps` away and scrolls it into view.
    fn step(&mut self, steps: isize, cx: &mut Context<Self>) {
        self.select(self.selected.saturating_add_signed(steps), cx);
        let row = cx.global::<Theme>().launcher.row_height;
        let viewport = self.scroll.bounds().size.height;
        let top = row * self.selected as f32;
        let scrolled = -self.scroll.offset().y;
        let wanted = scrolled.clamp(top + row - viewport, top).max(px(0.));
        self.scroll.set_offset(point(px(0.), -wanted));
        cx.notify();
    }

    /// Starts the shortcut in `row` and closes the popup this is shown in.
    fn launch(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(shortcut) = self.matches.get(row).map(|&index| &self.shortcuts[index]) else {
            return;
        };
        shortcut.launch();
        cx.defer(Popups::close);
    }

    /// Handles the keys that are not text; text goes to the field.
    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => cx.defer(Popups::close),
            "enter" => self.launch(self.selected, cx),
            "down" => self.step(1, cx),
            "up" => self.step(-1, cx),
            _ => {}
        }
    }

    fn render_row(
        &self,
        row: usize,
        theme: &Theme,
        top: Pixels,
        lit: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shortcut = &self.shortcuts[self.matches[row]];
        let kind = self.kind(self.matches[row]);
        // A file says where it lies, and a page that it is one.
        let folder = match kind {
            Kind::App => None,
            Kind::Page => Some(Language::of(cx).say("Settings").to_owned()),
            Kind::File => folder_of(&shortcut.path),
        };
        let folder = folder.map(|folder| {
            div()
                .flex_none()
                .max_w(px(160.))
                .text_size(theme.typography.label_size)
                .text_color(theme.colors.on_surface_variant)
                .child(one_line(SharedString::new(folder)))
        });
        let size = theme.launcher.icon_size;
        let icons = cx.global::<AppIcons>();
        let image = match kind {
            Kind::App => icons.of_shortcut(shortcut),
            Kind::File => icons.of_file(&shortcut.path),
            Kind::Page => None,
        };
        let image = match image {
            Some(image) => img(image).size(size).flex_none().into_any_element(),
            None if kind == Kind::Page => div()
                .size(size)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.colors.primary)
                .child(icon(theme, ICON_SETTINGS))
                .into_any_element(),
            None => div()
                .size(size)
                .flex_none()
                .rounded(theme.shape.small)
                .bg(theme.colors.outline)
                .into_any_element(),
        };
        div()
            .id(("launcher-row", row))
            .absolute()
            .top(top)
            .w_full()
            .h(theme.launcher.row_height)
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .child(image)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(one_line(SharedString::new(shortcut.name.as_str()))),
            )
            .children(folder)
            .children(self.render_pin(row, row == lit, theme, cx))
            .on_mouse_move(cx.listener(move |this, _, _, cx| this.select(row, cx)))
            .on_click(cx.listener(move |this, _, _, cx| this.launch(row, cx)))
            .into_any_element()
    }

    /// The button that pins the application of `row` to the Start menu or
    /// unpins it. It is there on the row the highlight is on (`lit`) and on
    /// pinned ones, and only if something shows the pins.
    fn render_pin(
        &self,
        row: usize,
        lit: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // Only applications are pinned.
        if self.kind(self.matches[row]) != Kind::App {
            return None;
        }
        let name = self.shortcuts[self.matches[row]].name.clone();
        let pins = self.pins.read(cx);
        let pinned = pins.holds(&name);
        if !pins.is_shown() || !(pinned || lit) {
            return None;
        }
        let (glyph, color) = if pinned {
            (ICON_PINNED, theme.colors.primary)
        } else {
            (ICON_PIN, theme.colors.on_surface_variant)
        };
        let button = div()
            .id(("launcher-pin", row))
            .size(PIN_BUTTON)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .text_color(color)
            .hover(|style| style.bg(theme.colors.surface_container_highest))
            .child(icon(theme, glyph))
            .on_click(cx.listener(move |this, _, _, cx| {
                // The row under the button would start the application.
                cx.stop_propagation();
                this.pins.update(cx, |pins, cx| pins.toggle(&name, cx));
            }));
        Some(button.into_any_element())
    }

    fn render_list(
        &mut self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.matches.is_empty() {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.colors.on_surface_variant)
                .child(Language::of(cx).say("Nothing matches"))
                .into_any_element();
        }
        let row = theme.launcher.row_height;
        let (first, shown) = visible_rows(
            self.matches.len(),
            f32::from(row),
            -f32::from(self.scroll.offset().y),
            f32::from(self.scroll.bounds().size.height),
        );
        // The highlight moves to the selected row; what goes with it is
        // drawn on the row it has reached.
        let at = self.highlight.value(window).max(0.);
        let lit = at.round() as usize;
        let highlight = div()
            .absolute()
            .top(row * at)
            .w_full()
            .h(row)
            .rounded(theme.shape.medium)
            .bg(theme.colors.secondary_container);
        let rows: Vec<AnyElement> = (first..first + shown)
            .map(|index| self.render_row(index, theme, row * index as f32, lit, cx))
            .collect();
        div()
            .id("launcher-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(row * self.matches.len() as f32)
                    .child(highlight)
                    .children(rows),
            )
            .into_any_element()
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        if !self.focused {
            self.focused = true;
            self.field.read(cx).focus(window);
        }
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .child(div().flex_none().flex().child(self.field.clone()))
            .child(self.render_list(&theme, window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn better_matches_come_first() {
        let names = ["Paint", "Notepad", "Windows Terminal", "Terminal Preview"];
        let found = search(names.into_iter(), " TE ");
        // Starts with it, then a word starts with it, then merely contains.
        assert_eq!(found, [3, 2, 1]);
        assert_eq!(search(names.into_iter(), "").len(), 4);
        assert!(search(names.into_iter(), "xyz").is_empty());
    }

    #[test]
    fn files_are_found_under_a_folder_without_the_hidden_ones() {
        let folder = std::env::temp_dir().join(format!("tessera-files-{}", std::process::id()));
        let inner = folder.join("one").join("two").join("three").join("four");
        fs::create_dir_all(&inner).unwrap();
        fs::create_dir_all(folder.join(".git")).unwrap();
        fs::write(folder.join("Report.txt"), "").unwrap();
        fs::write(folder.join(".git").join("config"), "").unwrap();
        fs::write(folder.join("one").join("notes.md"), "").unwrap();
        fs::write(inner.join("deep.txt"), "").unwrap();
        let found = files_under(std::slice::from_ref(&folder));
        fs::remove_dir_all(&folder).unwrap();
        let mut names: Vec<&str> = found.iter().map(|file| file.name.as_str()).collect();
        // The first level comes before what lies deeper.
        assert!(names[..2].contains(&"Report.txt") && names[..2].contains(&"one"));
        names.sort_unstable();
        // What is in `four` lies too deep, and `.git` is left out.
        assert_eq!(
            names,
            ["Report.txt", "four", "notes.md", "one", "three", "two"]
        );
        let notes = found.iter().find(|file| file.name == "notes.md").unwrap();
        assert_eq!(folder_of(&notes.path).as_deref(), Some("one"));
    }

    #[test]
    fn a_folder_may_be_named_from_the_folder_of_the_user() {
        assert_eq!(folder_named("D:/music"), PathBuf::from("D:/music"));
        if let Some(home) = std::env::var_os("USERPROFILE") {
            assert_eq!(
                folder_named("~/Documents"),
                PathBuf::from(home).join("Documents")
            );
        }
    }

    #[test]
    fn the_pages_of_the_settings_are_named_and_addressed() {
        let russian = pages(Language::Russian);
        assert_eq!(russian.len(), PAGES.len());
        let sound = russian.iter().find(|page| page.name == "Звук").unwrap();
        assert_eq!(sound.path, PathBuf::from("ms-settings:sound"));
        let update = pages_matching(&russian, "windows up");
        assert_eq!(
            russian[update[0]].path,
            PathBuf::from("ms-settings:windowsupdate")
        );
        let sounds = pages_matching(&russian, "зву");
        assert_eq!(russian[sounds[0]].name, "Звук");
        let english = pages(Language::English);
        assert!(english.iter().all(|page| !page.name.is_empty()));
        let mut names: Vec<&str> = PAGES.iter().map(|page| page.0).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PAGES.len());
    }

    #[test]
    fn only_rows_in_view_are_drawn() {
        assert_eq!(visible_rows(100, 40., 0., 0.), (0, ROWS_BEFORE_LAYOUT));
        assert_eq!(visible_rows(100, 40., 90., 200.), (2, 6));
        assert_eq!(visible_rows(100, 40., 3900., 200.), (97, 3));
        assert_eq!(visible_rows(3, 40., 0., 200.), (0, 3));
    }
}
