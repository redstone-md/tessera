// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How the launcher shows the applications while nothing is typed, as
//! Kickoff of KDE Plasma and the applications menu of Seelen do: all of
//! them, the favorites (those pinned to Start), or by category beside a
//! rail of the categories. The categories are the folders of the Start
//! menu; a shortcut that lies in none stands with the others that lie in
//! none, and the packaged applications stand together, as Windows gives
//! no category for them.

use std::path::{Component, Path};

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Pixels,
    StatefulInteractiveElement, Styled, div, px,
};

use crate::i18n::Language;
use crate::platform::{Shortcut, app_id_in};
use crate::theme::Theme;
use crate::ui::one_line;

/// How wide the rail of the categories is, and how high a tab is.
pub const RAIL_WIDTH: Pixels = px(176.);
const TAB_HEIGHT: Pixels = px(32.);
const RAIL_ROW: Pixels = px(32.);

/// What the launcher shows while nothing is typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    All,
    Favorites,
    Categories,
}

impl View {
    pub const ALL: [Self; 3] = [Self::Favorites, Self::All, Self::Categories];

    /// The view the option `view` names and whether its applications
    /// stand as tiles: `all` (or `tiles`, or nothing) as tiles, `list` all
    /// of them one under another, `favorites` as tiles, `categories` as a
    /// list beside the rail. Another name is reported.
    pub fn named(name: Option<&str>) -> (Self, bool) {
        match name {
            None | Some("all" | "tiles") => (Self::All, true),
            Some("list") => (Self::All, false),
            Some("favorites") => (Self::Favorites, true),
            Some("categories") => (Self::Categories, false),
            Some(other) => {
                report!(
                    "the launcher has no view `{other}`; there are `all`, `list`, `favorites` and `categories`"
                );
                (Self::All, true)
            }
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "All apps",
            Self::Favorites => "Favorites",
            Self::Categories => "Categories",
        }
    }
}

/// A category of the applications.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    /// A folder of the Start menu, by its name.
    Folder(String),
    /// The shortcuts that lie in no folder.
    Loose,
    /// The packaged applications.
    Packaged,
}

impl Group {
    /// What the rail calls it.
    pub fn label(&self, language: Language) -> String {
        match self {
            Self::Folder(name) => name.clone(),
            Self::Loose => language.say("Not in a folder").to_owned(),
            Self::Packaged => language.say("Packaged apps").to_owned(),
        }
    }
}

/// The category of the application at `path`: the first folder below
/// `Start Menu\Programs` it lies in, of the user's or of the machine's.
pub fn group_of(path: &Path) -> Group {
    if app_id_in(path).is_some() {
        return Group::Packaged;
    }
    let parts: Vec<String> = { path.components() }
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let programs = { parts.windows(2) }.position(|pair| {
        pair[0].eq_ignore_ascii_case("Start Menu") && pair[1].eq_ignore_ascii_case("Programs")
    });
    match programs.map(|at| &parts[at + 2..]) {
        // A folder, and the shortcut in it or deeper.
        Some([folder, _, ..]) => Group::Folder(folder.clone()),
        _ => Group::Loose,
    }
}

/// The categories of a list of applications and which one each is in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Groups {
    /// The categories, the folders first by name, whatever their case.
    pub names: Vec<Group>,
    /// The place in `names` of the category of each application.
    of_app: Vec<usize>,
}

impl Groups {
    pub fn of(apps: &[Shortcut]) -> Self {
        let each: Vec<Group> = apps.iter().map(|app| group_of(&app.path)).collect();
        let mut names = each.clone();
        names.sort_by_cached_key(|group| match group {
            Group::Folder(name) => (0, name.to_lowercase()),
            Group::Loose => (1, String::new()),
            Group::Packaged => (2, String::new()),
        });
        // Two folders of one name, of the user's and of the machine's, are
        // one category.
        names.dedup_by(|one, other| match (one, other) {
            (Group::Folder(one), Group::Folder(other)) => one.eq_ignore_ascii_case(other),
            (one, other) => one == other,
        });
        let place = |group: &Group| {
            { names.iter() }
                .position(|name| match (name, group) {
                    (Group::Folder(name), Group::Folder(group)) => name.eq_ignore_ascii_case(group),
                    (name, group) => name == group,
                })
                .unwrap_or(0)
        };
        let of_app = each.iter().map(place).collect();
        Self { names, of_app }
    }

    /// The places of the applications in the category `group`.
    pub fn members(&self, group: usize) -> Vec<usize> {
        { self.of_app.iter().enumerate() }
            .filter(|&(_, &of)| of == group)
            .map(|(app, _)| app)
            .collect()
    }
}

/// The places in `apps` of the applications `pinned` names, in their
/// order; a name with no application is left out.
pub fn favorites(apps: &[Shortcut], pinned: &[String]) -> Vec<usize> {
    { pinned.iter() }
        .filter_map(|name| {
            let name = name.to_lowercase();
            apps.iter().position(|app| app.name.to_lowercase() == name)
        })
        .collect()
}

/// The row of tabs that switches the views, with `shown` lit; a click on
/// one has `choose` take it.
pub fn render_tabs<V: 'static>(
    shown: View,
    theme: &Theme,
    cx: &mut Context<V>,
    choose: fn(&mut V, View, &mut Context<V>),
) -> AnyElement {
    let language = Language::of(cx);
    let tabs: Vec<AnyElement> = { View::ALL.into_iter().enumerate() }
        .map(|(index, view)| {
            let lit = view == shown;
            let (ground, text) = match lit {
                true => (theme.colors.secondary_container, theme.colors.on_surface),
                false => (
                    theme.colors.surface_container,
                    theme.colors.on_surface_variant,
                ),
            };
            div()
                .id(("launcher-tab", index))
                .h(TAB_HEIGHT)
                .px(theme.spacing.medium)
                .flex()
                .items_center()
                .rounded(theme.shape.full)
                .bg(ground)
                .text_color(text)
                .text_size(theme.typography.label_size)
                .hover(|style| style.bg(theme.colors.surface_container_highest))
                .child(language.say(view.label()))
                .on_click(cx.listener(move |this, _, _, cx| choose(this, view, cx)))
                .into_any_element()
        })
        .collect();
    div()
        .flex_none()
        .flex()
        .gap(theme.spacing.small)
        .children(tabs)
        .into_any_element()
}

/// The rail of the categories `groups`, with `chosen` lit; a click on one
/// has `choose` take it.
pub fn render_rail<V: 'static>(
    groups: &Groups,
    chosen: usize,
    theme: &Theme,
    cx: &mut Context<V>,
    choose: fn(&mut V, usize, &mut Context<V>),
) -> AnyElement {
    let language = Language::of(cx);
    let rows: Vec<AnyElement> = { groups.names.iter().enumerate() }
        .map(|(index, group)| {
            let lit = index == chosen;
            let ground = match lit {
                true => theme.colors.secondary_container,
                false => theme.colors.surface_container,
            };
            div()
                .id(("launcher-category", index))
                .flex_none()
                .h(RAIL_ROW)
                .px(theme.spacing.medium)
                .flex()
                .items_center()
                .rounded(theme.shape.medium)
                .bg(ground)
                .hover(|style| style.bg(theme.colors.surface_container_highest))
                .child(one_line(group.label(language)))
                .on_click(cx.listener(move |this, _, _, cx| choose(this, index, cx)))
                .into_any_element()
        })
        .collect();
    div()
        .id("launcher-rail")
        .flex_none()
        .w(RAIL_WIDTH)
        .h_full()
        .flex()
        .flex_col()
        .gap(theme.spacing.tiny)
        .overflow_y_scroll()
        .children(rows)
        .into_any_element()
}

/// What the list says in a view that has nothing to show.
pub fn empty_text(view: View, language: Language) -> &'static str {
    match view {
        View::Favorites => language.say("Pin applications to Start to find them here"),
        _ => language.say("Nothing matches"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn app(name: &str, path: &str) -> Shortcut {
        Shortcut {
            name: name.to_owned(),
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn the_option_names_the_view() {
        assert_eq!(View::named(None), (View::All, true));
        assert_eq!(View::named(Some("list")), (View::All, false));
        assert_eq!(View::named(Some("favorites")), (View::Favorites, true));
        assert_eq!(View::named(Some("categories")), (View::Categories, false));
    }

    #[test]
    fn a_category_is_the_first_folder_of_the_start_menu() {
        let tools =
            r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Windows Tools\Sub\Disk.lnk";
        assert_eq!(
            group_of(Path::new(tools)),
            Group::Folder("Windows Tools".to_owned())
        );
        let loose = r"C:\Users\me\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Paint.lnk";
        assert_eq!(group_of(Path::new(loose)), Group::Loose);
        let packaged = r"shell:AppsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App";
        assert_eq!(group_of(Path::new(packaged)), Group::Packaged);
        assert_eq!(group_of(Path::new(r"D:\Tools\tool.exe")), Group::Loose);
    }

    #[test]
    fn the_categories_are_sorted_and_one_per_name() {
        let apps = [
            app(
                "Word",
                r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Office\Word.lnk",
            ),
            app(
                "Calc",
                r"shell:AppsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
            ),
            app(
                "Zip",
                r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\7-Zip\Zip.lnk",
            ),
            app(
                "Paint",
                r"C:\Users\me\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Paint.lnk",
            ),
            app(
                "Excel",
                r"C:\Users\me\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\office\Excel.lnk",
            ),
        ];
        let groups = Groups::of(&apps);
        assert_eq!(
            groups.names,
            [
                Group::Folder("7-Zip".to_owned()),
                Group::Folder("Office".to_owned()),
                Group::Loose,
                Group::Packaged,
            ]
        );
        assert_eq!(groups.members(1), [0, 4]);
        assert_eq!(groups.members(3), [1]);
    }

    #[test]
    fn the_favorites_are_the_pinned_applications_in_their_order() {
        let apps = [
            app("Paint", "p.lnk"),
            app("Notepad", "n.lnk"),
            app("Edge", "e.lnk"),
        ];
        let pinned = ["edge".to_owned(), "Gone".to_owned(), "Paint".to_owned()];
        assert_eq!(favorites(&apps, &pinned), [2, 0]);
    }
}
