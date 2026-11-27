// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Renaming many files at once: a pattern with `#` for a number ("Holiday
//! #" names them "Holiday 1", "Holiday 2" and so on, "##" with two
//! digits), a text to find and one to put in its place, each name keeping
//! its extension. A dialog over the pane shows every new name before
//! anything is renamed, and refuses names that cannot be or that would
//! meet.

use std::collections::HashSet;

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px,
};

use super::model::is_valid_name;
use crate::field::TextField;
use crate::i18n::Language;
use crate::platform::rename_new;
use crate::theme::Theme;
use crate::ui::one_line;

/// How many new names the dialog shows; the rest are renamed alike.
const PREVIEW_ROWS: usize = 200;

/// What begins the names files take for a moment, where new names swap.
const PASSING: &str = ".tessera-renaming";

/// Why new names cannot be given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// The name cannot be the name of a file.
    Invalid(String),
    /// Two files would get the name.
    Twice(String),
    /// Something that is not renamed has the name already.
    Taken(String),
}

impl Conflict {
    /// What the dialog says of it, in the language of the user.
    fn said(&self, language: Language) -> String {
        let (text, name) = match self {
            Self::Invalid(name) => ("cannot be the name of a file", name),
            Self::Twice(name) => ("would be the name of two files", name),
            Self::Taken(name) => ("is the name of something else here", name),
        };
        format!("“{name}” {}", language.say(text))
    }
}

/// The stem and the extension of `name`, the dot with the extension; a
/// folder, or a name without an extension, is a stem alone.
fn stem_and_extension(name: &str, folder: bool) -> (&str, &str) {
    if folder {
        return (name, "");
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    }
}

/// `pattern` with each run of `#` in it replaced by `number`, with as
/// many digits at least as the run is long; without `#`, `pattern` and
/// the number in brackets, as Explorer names many files, unless `alone`.
fn numbered(pattern: &str, number: usize, alone: bool) -> String {
    if !pattern.contains('#') {
        return match alone {
            true => pattern.to_owned(),
            false => format!("{pattern} ({number})"),
        };
    }
    let mut named = String::new();
    let mut run = 0;
    for letter in pattern.chars().chain(['\0']) {
        if letter == '#' {
            run += 1;
            continue;
        }
        if run > 0 {
            named.push_str(&format!("{number:0run$}"));
            run = 0;
        }
        if letter != '\0' {
            named.push(letter);
        }
    }
    named
}

/// The new names of the files at `paths`, numbered in their order, as
/// `(old, new)` for each that changes: the stem is `pattern` numbered
/// where it is given, then `find` is replaced by `replace` in it, and the
/// extension stays. `taken` holds the names in the folders that are not
/// renamed, each by its folder, as `parent_key` gives them. `folders` tells which paths are folders.
pub fn plan(
    paths: &[PathBuf],
    folders: &HashSet<PathBuf>,
    pattern: &str,
    find: &str,
    replace: &str,
    taken: &HashSet<(PathBuf, String)>,
) -> Result<Vec<(PathBuf, PathBuf)>, Conflict> {
    let pattern = pattern.trim();
    let mut given = HashSet::new();
    let mut plan = Vec::new();
    let alone = paths.len() == 1;
    for (index, path) in paths.iter().enumerate() {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (stem, extension) = stem_and_extension(&name, folders.contains(path));
        let mut stem = match pattern.is_empty() {
            true => stem.to_owned(),
            false => numbered(pattern, index + 1, alone),
        };
        if !find.is_empty() {
            stem = stem.replace(find, replace);
        }
        let new = format!("{stem}{extension}");
        if !is_valid_name(&new) {
            return Err(Conflict::Invalid(new));
        }
        let key = parent_key(path, &new);
        let taken = taken.contains(&key) && !new.eq_ignore_ascii_case(&name);
        if !given.insert(key) {
            return Err(Conflict::Twice(new));
        }
        if taken {
            return Err(Conflict::Taken(new));
        }
        if new != name {
            plan.push((path.clone(), path.with_file_name(new)));
        }
    }
    Ok(plan)
}

/// What tells a name in a folder from the others there, in any case.
pub fn parent_key(path: &Path, name: &str) -> (PathBuf, String) {
    let parent = path.parent().map(Path::to_owned).unwrap_or_default();
    (parent, name.to_lowercase())
}

/// Whether some new name of `plan` is the old name of another file of
/// it, so that the files must pass through names of their own first.
pub fn swaps(plan: &[(PathBuf, PathBuf)]) -> bool {
    let old: HashSet<(PathBuf, String)> = { plan.iter() }
        .map(|(old, _)| parent_key(old, &old.file_name().unwrap_or_default().to_string_lossy()))
        .collect();
    plan.iter().any(|(from, to)| {
        let name = to.file_name().unwrap_or_default().to_string_lossy();
        from != to && old.contains(&parent_key(to, &name)) && !same_but_case(from, to)
    })
}

/// Whether two paths differ in case alone, as a rename that only changes
/// capitals does.
fn same_but_case(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// Renames the files of `plan`, through names of their own first where
/// new names swap. Nothing is put over a file that is there: such a
/// file, and each rename that fails, is told in what is given back. A
/// file whose rename fails gets its old name back where that is free, or
/// else is told with the name it was left at.
/// Blocks on the disk; it is done away from the user interface.
pub fn apply(plan: &[(PathBuf, PathBuf)]) -> Vec<String> {
    let mut problems = Vec::new();
    let passing = swaps(plan);
    // Each file with the name it has now and the one it gets.
    let mut steps: Vec<(&PathBuf, PathBuf, &PathBuf)> = Vec::new();
    for (index, (from, to)) in plan.iter().enumerate() {
        let via = match passing {
            true => from.with_file_name(format!("{PASSING}-{}-{index}", std::process::id())),
            false => from.clone(),
        };
        if passing && let Err(error) = rename_new(from, &via) {
            problems.push(format!("{}: {error}", from.display()));
            continue;
        }
        steps.push((from, via, to));
    }
    for (from, via, to) in steps {
        let Err(error) = rename_new(&via, to) else {
            continue;
        };
        problems.push(match to.exists() && !same_but_case(from, to) {
            true => format!("{} is there already", to.display()),
            false => format!("{}: {error}", from.display()),
        });
        // Another file of the plan, or of another program, may have
        // taken the old name meanwhile; it is not put over.
        if &via != from
            && let Err(error) = rename_new(&via, from)
        {
            problems.push(format!(
                "{} was left as {}: {error}",
                from.display(),
                via.display()
            ));
        }
    }
    problems
}

/// What the dialog tells the pane it stands over.
#[derive(Clone, Debug, PartialEq)]
pub enum RenameEvent {
    /// To rename the files as planned.
    Apply(Vec<(PathBuf, PathBuf)>),
    Cancel,
}

/// The dialog that renames many files at once.
pub struct BatchRename {
    paths: Vec<PathBuf>,
    folders: HashSet<PathBuf>,
    taken: HashSet<(PathBuf, String)>,
    pattern: Entity<TextField>,
    find: Entity<TextField>,
    replace: Entity<TextField>,
    planned: Result<Vec<(PathBuf, PathBuf)>, Conflict>,
}

impl EventEmitter<RenameEvent> for BatchRename {}

impl BatchRename {
    /// The dialog for the files at `paths`, of which `folders` are
    /// folders, among the names `taken` of what is not renamed.
    pub fn new(
        paths: Vec<PathBuf>,
        folders: HashSet<PathBuf>,
        taken: HashSet<(PathBuf, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language = Language::of(cx);
        let field = |placeholder: &'static str, cx: &mut Context<Self>| {
            let field = cx.new(|cx| TextField::new(language.say(placeholder).into(), cx));
            cx.observe(&field, |this, _, cx| this.replan(cx)).detach();
            field
        };
        let pattern = field("New name, # for the number", cx);
        let find = field("Find", cx);
        let replace = field("Replace with", cx);
        pattern.read(cx).focus(window);
        let mut dialog = Self {
            paths,
            folders,
            taken,
            pattern,
            find,
            replace,
            planned: Ok(Vec::new()),
        };
        dialog.replan(cx);
        dialog
    }

    fn replan(&mut self, cx: &mut Context<Self>) {
        let (pattern, find, replace) = (
            self.pattern.read(cx).text(),
            self.find.read(cx).text(),
            self.replace.read(cx).text(),
        );
        self.planned = plan(
            &self.paths,
            &self.folders,
            &pattern,
            &find,
            &replace,
            &self.taken,
        );
        cx.notify();
    }

    /// Renames as planned, where that can be done.
    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        if let Ok(plan) = &self.planned
            && !plan.is_empty()
        {
            cx.emit(RenameEvent::Apply(plan.clone()));
        }
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        cx.emit(RenameEvent::Cancel);
    }

    /// A button of the dialog.
    fn button(
        id: &'static str,
        label: &'static str,
        strong: bool,
        enabled: bool,
        theme: &Theme,
        language: Language,
    ) -> gpui::Stateful<gpui::Div> {
        let (back, fore) = match strong {
            true => (theme.colors.primary, theme.colors.surface_container),
            false => (
                theme.colors.surface_container_highest,
                theme.colors.on_surface,
            ),
        };
        div()
            .id(id)
            .h(px(32.))
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .rounded(theme.shape.full)
            .bg(back)
            .text_color(fore)
            .text_size(theme.typography.label_size)
            .when(!enabled, |button| button.opacity(0.5))
            .when(enabled, |button| {
                button.cursor_pointer().hover(|style| style.opacity(0.9))
            })
            .child(language.say(label))
    }

    /// The new names, old beside new, as many as the dialog shows.
    fn render_preview(&self, theme: &Theme, language: Language) -> AnyElement {
        let rows: Vec<AnyElement> = match &self.planned {
            Err(conflict) => vec![
                div()
                    .text_color(gpui::rgb(0xe5534b))
                    .child(conflict.said(language))
                    .into_any_element(),
            ],
            Ok(plan) if plan.is_empty() => vec![
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(language.say("No name changes"))
                    .into_any_element(),
            ],
            Ok(plan) => { plan.iter().take(PREVIEW_ROWS) }
                .map(|(old, new)| {
                    let name = |path: &Path| -> SharedString {
                        path.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                            .into()
                    };
                    div()
                        .flex()
                        .gap(theme.spacing.small)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(theme.colors.on_surface_variant)
                                .child(one_line(name(old))),
                        )
                        .child("\u{2192}")
                        .child(div().flex_1().min_w_0().child(one_line(name(new))))
                        .into_any_element()
                })
                .collect(),
        };
        div()
            .id("files-rename-preview")
            .h(px(220.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(theme.spacing.small)
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container_highest.opacity(0.5))
            .overflow_y_scroll()
            .text_size(theme.typography.label_size)
            .children(rows)
            .into_any_element()
    }
}

impl Render for BatchRename {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let ready = matches!(&self.planned, Ok(plan) if !plan.is_empty());
        let count = self.paths.len();
        let label = |text: &'static str| {
            div()
                .text_size(theme.typography.label_size * 0.85)
                .text_color(theme.colors.on_surface_variant)
                .child(language.say(text))
        };
        let card = div()
            .id("files-rename")
            .w(px(520.))
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .p(theme.spacing.medium)
            .rounded(theme.shape.large)
            .bg(theme.colors.surface_container)
            .border_1()
            .border_color(theme.colors.outline.opacity(0.3))
            .shadow_lg()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(theme.typography.label_size * 1.25)
                    .child(format!("{} ({count})", language.say("Rename"))),
            )
            .child(label("New name, # for the number"))
            .child(self.pattern.clone())
            .child(
                div()
                    .flex()
                    .gap(theme.spacing.small)
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(theme.spacing.tiny)
                            .child(label("Find"))
                            .child(self.find.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(theme.spacing.tiny)
                            .child(label("Replace with"))
                            .child(self.replace.clone()),
                    ),
            )
            .child(self.render_preview(&theme, language))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.spacing.small)
                    .child(
                        Self::button(
                            "files-rename-cancel",
                            "Cancel",
                            false,
                            true,
                            &theme,
                            language,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                    )
                    .child(
                        Self::button(
                            "files-rename-apply",
                            "Rename",
                            true,
                            ready,
                            &theme,
                            language,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                    ),
            );
        div()
            .id("files-rename-scrim")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.3))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| this.cancel(cx)),
            )
            .child(card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| Path::new("C:\\photos").join(name))
            .collect()
    }

    fn no_folders() -> HashSet<PathBuf> {
        HashSet::new()
    }

    fn nothing_taken() -> HashSet<(PathBuf, String)> {
        HashSet::new()
    }

    fn new_names(plan: &[(PathBuf, PathBuf)]) -> Vec<String> {
        { plan.iter() }
            .map(|(_, new)| new.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_pattern_numbers_the_files_and_keeps_their_extensions() {
        let paths = files(&["a.jpg", "b.JPG", "c.png"]);
        let plan = plan(&paths, &no_folders(), "Holiday #", "", "", &nothing_taken())
            .expect("names can be given");
        assert_eq!(
            new_names(&plan),
            ["Holiday 1.jpg", "Holiday 2.JPG", "Holiday 3.png"]
        );
        let plan = super::plan(&paths, &no_folders(), "IMG_###", "", "", &nothing_taken()).unwrap();
        assert_eq!(new_names(&plan)[1], "IMG_002.JPG");
        let plan = super::plan(&paths, &no_folders(), "Trip", "", "", &nothing_taken()).unwrap();
        assert_eq!(new_names(&plan)[0], "Trip (1).jpg");
    }

    #[test]
    fn find_and_replace_changes_the_stem_alone() {
        let paths = files(&["draft report.docx", "draft.draft.txt", "final.txt"]);
        let plan = plan(
            &paths,
            &no_folders(),
            "",
            "draft",
            "final",
            &nothing_taken(),
        )
        .unwrap();
        assert_eq!(new_names(&plan), ["final report.docx", "final.final.txt"]);
    }

    #[test]
    fn names_that_meet_or_cannot_be_are_refused() {
        let paths = files(&["a.txt", "b.txt"]);
        let twice = plan(&paths, &no_folders(), "", "a", "b", &nothing_taken());
        assert_eq!(twice, Err(Conflict::Twice("b.txt".to_owned())));
        let invalid = plan(&paths, &no_folders(), "x:#", "", "", &nothing_taken());
        assert_eq!(invalid, Err(Conflict::Invalid("x:1.txt".to_owned())));
        let taken = HashSet::from([parent_key(&paths[0], "Photo 1.txt")]);
        let met = plan(&paths, &no_folders(), "Photo #", "", "", &taken);
        assert_eq!(met, Err(Conflict::Taken("Photo 1.txt".to_owned())));
        // A name taken in another folder, as among the results of a
        // search, is no matter.
        let elsewhere = HashSet::from([parent_key(Path::new("D:\\other\\x"), "Photo 1.txt")]);
        assert!(plan(&paths, &no_folders(), "Photo #", "", "", &elsewhere).is_ok());
    }

    #[test]
    fn a_folder_keeps_the_dots_of_its_name() {
        let paths = files(&["v1.2"]);
        let folders = paths.iter().cloned().collect();
        let plan = plan(&paths, &folders, "", "1.2", "2.0", &HashSet::new()).unwrap();
        assert_eq!(new_names(&plan), ["v2.0"]);
    }

    #[test]
    fn names_that_swap_pass_through_names_of_their_own() {
        let paths = files(&["1.txt", "2.txt"]);
        let swapped = vec![
            (paths[0].clone(), paths[1].clone()),
            (paths[1].clone(), paths[0].clone()),
        ];
        assert!(swaps(&swapped));
        let plan = plan(&paths, &no_folders(), "Note #", "", "", &nothing_taken()).unwrap();
        assert!(!swaps(&plan));
        let capitals = vec![(paths[0].clone(), Path::new("C:\\photos").join("1.TXT"))];
        assert!(!swaps(&capitals));
    }
}
