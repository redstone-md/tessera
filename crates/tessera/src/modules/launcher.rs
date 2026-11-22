// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Finds and starts applications from the Start menu, and opens the files
//! of the folders its `folders` option names. Widget: `launcher`.

use std::fs;
use std::ops::Range;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AppContext, AsyncApp, Bounds, Context, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    Pixels, Point, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    UTF16Selection, WeakEntity, Window, canvas, div, img, point, px,
};

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

pub struct LauncherModule;

impl Module for LauncherModule {
    fn name(&self) -> &'static str {
        "launcher"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("launcher", |context, cx| {
            let placeholder = context.text("placeholder").unwrap_or(DEFAULT_PLACEHOLDER);
            let placeholder = SharedString::new(placeholder);
            let folders = context.texts("folders").unwrap_or_default();
            let folders = folders.iter().map(|folder| folder_named(folder)).collect();
            cx.new(|cx| Launcher::new(placeholder, folders, cx)).into()
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
    /// The applications, and after them the files.
    shortcuts: Vec<Shortcut>,
    /// How many of `shortcuts` are applications.
    apps: usize,
    query: String,
    /// Text an input method is still composing; it follows the query.
    composing: String,
    /// Positions in `shortcuts` of those matching the query, best first.
    matches: Vec<usize>,
    /// The position in `matches` that Enter starts.
    selected: usize,
    /// The row the selection highlight is drawn at; follows `selected`.
    highlight: Spring,
    scroll: ScrollHandle,
    focus: FocusHandle,
    focused: bool,
    placeholder: SharedString,
    /// What is pinned to the Start menu, which a row can change.
    pins: Entity<Pins>,
}

impl Launcher {
    fn new(placeholder: SharedString, folders: Vec<PathBuf>, cx: &mut Context<Self>) -> Self {
        let shortcuts = Shortcut::start_menu();
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
                    this.shortcuts.truncate(this.apps);
                    this.shortcuts.extend(files);
                    this.find();
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        Self {
            pins,
            matches: (0..shortcuts.len()).collect(),
            apps: shortcuts.len(),
            shortcuts,
            query: String::new(),
            composing: String::new(),
            selected: 0,
            highlight: Spring::new(cx.global::<Theme>().motion.spatial_fast, 0.),
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            focused: false,
            placeholder,
        }
    }

    /// What the field shows and the list is searched for.
    fn text(&self) -> String {
        format!("{}{}", self.query, self.composing)
    }

    fn set_query(&mut self, query: String, composing: &str, cx: &mut Context<Self>) {
        self.query = query;
        composing.clone_into(&mut self.composing);
        self.find();
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.select(0, cx);
        cx.notify();
    }

    /// Looks for what the field asks for: among the applications always,
    /// and among the files once enough letters are written. The files
    /// come after the applications.
    fn find(&mut self) {
        let text = self.text();
        let (apps, files) = self.shortcuts.split_at(self.apps);
        fn names(shortcuts: &[Shortcut]) -> impl Iterator<Item = &str> {
            shortcuts.iter().map(|shortcut| shortcut.name.as_str())
        }
        let mut matches = search(names(apps), &text);
        if text.trim().chars().count() >= LETTERS_FOR_FILES {
            let files = search(names(files), &text);
            let files = files.into_iter().take(FILES_SHOWN);
            matches.extend(files.map(|index| index + self.apps));
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

    /// Handles the keys that are not text; text arrives through
    /// `EntityInputHandler`.
    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => cx.defer(Popups::close),
            "enter" => self.launch(self.selected, cx),
            "down" => self.step(1, cx),
            "up" => self.step(-1, cx),
            "backspace" if self.composing.is_empty() => {
                let mut query = self.query.clone();
                query.pop();
                self.set_query(query, "", cx);
            }
            _ => {}
        }
    }

    fn render_field(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let shown = self.text();
        let is_empty = shown.is_empty();
        let text = if is_empty {
            div()
                .text_color(theme.colors.on_surface_variant)
                .child(self.placeholder.clone())
        } else {
            div().child(SharedString::new(shown))
        };
        // Typed text is delivered to whoever handles input while painting.
        let (focus, this) = (self.focus.clone(), cx.entity());
        let input = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                window.handle_input(&focus, ElementInputHandler::new(bounds, this), cx);
            },
        )
        .absolute()
        .size_full();
        let caret = div()
            .w(px(2.))
            .h(theme.typography.icon_size)
            .rounded(theme.shape.full)
            .bg(theme.colors.primary);
        let field = div()
            .h(theme.launcher.field_height)
            .flex_none()
            .relative()
            .child(input)
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .rounded(theme.shape.full)
            .bg(theme.colors.surface_container_highest)
            .child(icon(theme, ICON_SEARCH).text_color(theme.colors.on_surface_variant));
        // The caret stands before the placeholder and after typed text.
        if is_empty {
            field.child(caret).child(text)
        } else {
            field.child(text).child(caret)
        }
        .into_any_element()
    }

    fn render_row(
        &self,
        row: usize,
        theme: &Theme,
        top: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shortcut = &self.shortcuts[self.matches[row]];
        // A file says where it lies.
        let folder = (self.matches[row] >= self.apps)
            .then(|| folder_of(&shortcut.path))
            .flatten();
        let folder = folder.map(|folder| {
            div()
                .flex_none()
                .max_w(px(160.))
                .text_size(theme.typography.label_size)
                .text_color(theme.colors.on_surface_variant)
                .child(one_line(SharedString::new(folder)))
        });
        let size = theme.launcher.icon_size;
        let image = match cx.global::<AppIcons>().of_shortcut(shortcut) {
            Some(image) => img(image).size(size).flex_none().into_any_element(),
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
            .children(self.render_pin(row, theme, cx))
            .on_mouse_move(cx.listener(move |this, _, _, cx| this.select(row, cx)))
            .on_click(cx.listener(move |this, _, _, cx| this.launch(row, cx)))
            .into_any_element()
    }

    /// The button that pins the application of `row` to the Start menu or
    /// unpins it. It is there on the selected row and on pinned ones, and
    /// only if something shows the pins.
    fn render_pin(&self, row: usize, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        // Only applications are pinned.
        if self.matches[row] >= self.apps {
            return None;
        }
        let name = self.shortcuts[self.matches[row]].name.clone();
        let pins = self.pins.read(cx);
        let pinned = pins.holds(&name);
        if !pins.is_shown() || !(pinned || row == self.selected) {
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
                .child("No application matches")
                .into_any_element();
        }
        let row = theme.launcher.row_height;
        let (first, shown) = visible_rows(
            self.matches.len(),
            f32::from(row),
            -f32::from(self.scroll.offset().y),
            f32::from(self.scroll.bounds().size.height),
        );
        let highlight = div()
            .absolute()
            .top(row * self.highlight.value(window).max(0.))
            .w_full()
            .h(row)
            .rounded(theme.shape.medium)
            .bg(theme.colors.secondary_container);
        let rows: Vec<AnyElement> = (first..first + shown)
            .map(|index| self.render_row(index, theme, row * index as f32, cx))
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

/// Text input, with the caret always at the end. Positions count UTF-16
/// code units.
impl EntityInputHandler for Launcher {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let units: Vec<u16> = self.text().encode_utf16().collect();
        let end = range.end.min(units.len());
        Some(String::from_utf16_lossy(&units[range.start.min(end)..end]))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.text().encode_utf16().count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let start = self.query.encode_utf16().count();
        let length = self.composing.encode_utf16().count();
        (length > 0).then(|| start..start + length)
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_query(self.text(), "", cx);
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let typed: String = text.chars().filter(|typed| !typed.is_control()).collect();
        self.set_query(format!("{}{typed}", self.query), "", cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_query(self.query.clone(), new_text, cx);
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus);
        }
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .child(self.render_field(&theme, cx))
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
    fn only_rows_in_view_are_drawn() {
        assert_eq!(visible_rows(100, 40., 0., 0.), (0, ROWS_BEFORE_LAYOUT));
        assert_eq!(visible_rows(100, 40., 90., 200.), (2, 6));
        assert_eq!(visible_rows(100, 40., 3900., 200.), (97, 3));
        assert_eq!(visible_rows(3, 40., 0., 200.), (0, 3));
    }
}
