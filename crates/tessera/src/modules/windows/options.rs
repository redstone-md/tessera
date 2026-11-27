// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the options of the widget `windows` choose, and the choices the
//! dock makes from them: which windows it shows, how it groups and orders
//! them, and which way it is laid out.

use crate::module::WidgetContext;
use crate::ui::Pick;

/// Whether the windows of one application share a button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Grouping {
    #[default]
    Always,
    /// One button for each window.
    Never,
}

/// Which windows have a button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Showing {
    /// Those on the display of the dock.
    #[default]
    Display,
    /// Those of every display.
    All,
    /// Those of the workspace shown, on every display.
    Workspace,
    /// Only the minimized ones of the display of the dock.
    Minimized,
}

/// In which order the buttons stand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sorting {
    /// The pinned applications first, then the others as they opened.
    #[default]
    Pinned,
    /// Every application as it opened its first window, then the pinned
    /// ones that do not run.
    Opened,
    /// By the name of the application.
    Name,
}

/// What a click with the middle button on a button does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Middle {
    /// Starts another window of the application.
    #[default]
    New,
    /// Closes the windows of the button.
    Close,
}

/// When a dock that hides is shown because a window asks for attention.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reveal {
    Never,
    /// While the window flashes.
    #[default]
    WhileFlashing,
    /// Until the window is activated.
    UntilActivated,
}

/// The options of one dock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DockOptions {
    pub group: Grouping,
    /// Whether each button shows the title of its window beside its icon.
    pub labels: bool,
    /// Whether the number of windows is shown instead of dots.
    pub counter: bool,
    pub show: Showing,
    pub sort: Sorting,
    pub middle: Middle,
    /// Whether the wheel over a button goes through its windows.
    pub wheel: bool,
    pub reveal: Reveal,
    /// Whether the buttons count the waiting notifications.
    pub badges: bool,
}

impl DockOptions {
    /// The options `context` gives; one with a value that is not a choice
    /// is reported and keeps its default.
    pub fn of(context: &WidgetContext) -> Self {
        let all_displays = context.flag("all_displays") == Some(true);
        let show_default = match all_displays {
            true => Showing::All,
            false => Showing::Display,
        };
        Self {
            group: choice(
                context,
                "group",
                &[("always", Grouping::Always), ("never", Grouping::Never)],
                Grouping::Always,
            ),
            labels: context.flag("labels").unwrap_or(false),
            counter: context.flag("counter").unwrap_or(false),
            show: choice(
                context,
                "show",
                &[
                    ("display", Showing::Display),
                    ("all", Showing::All),
                    ("workspace", Showing::Workspace),
                    ("minimized", Showing::Minimized),
                ],
                show_default,
            ),
            sort: choice(
                context,
                "sort",
                &[
                    ("pinned", Sorting::Pinned),
                    ("opened", Sorting::Opened),
                    ("name", Sorting::Name),
                ],
                Sorting::Pinned,
            ),
            middle: choice(
                context,
                "middle",
                &[("new", Middle::New), ("close", Middle::Close)],
                Middle::New,
            ),
            wheel: context.flag("wheel").unwrap_or(false),
            reveal: choice(
                context,
                "reveal_on_attention",
                &[
                    ("never", Reveal::Never),
                    ("while_flashing", Reveal::WhileFlashing),
                    ("until_activated", Reveal::UntilActivated),
                ],
                Reveal::WhileFlashing,
            ),
            badges: context.flag("badges").unwrap_or(true),
        }
    }
}

/// The choice the text option `key` names among `choices`, or `default`
/// where it names none, which is reported.
fn choice<T: Copy>(context: &WidgetContext, key: &str, choices: &[(&str, T)], default: T) -> T {
    let Some(text) = context.text(key) else {
        return default;
    };
    match choose(text, choices) {
        Some(chosen) => chosen,
        None => {
            let names: Vec<&str> = choices.iter().map(|(name, _)| *name).collect();
            report!(
                "the widget option `{key}` is one of `{}`, `{text}` is ignored",
                names.join("`, `")
            );
            default
        }
    }
}

/// The choice `text` names, whatever its case.
fn choose<T: Copy>(text: &str, choices: &[(&str, T)]) -> Option<T> {
    let text = text.trim();
    let named = choices
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(text));
    named.map(|(_, chosen)| *chosen)
}

/// What is known of a window when it is decided whether it has a button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// Whether it is on the display of the dock.
    pub here: bool,
    pub minimized: bool,
    /// Whether it is on the workspace shown; `None` where that is not
    /// known, and every window that is listed is.
    pub in_workspace: Option<bool>,
    /// Whether a rule leaves it out of the dock.
    pub skipped: bool,
    /// Whether its application removed its button (`DeleteTab`).
    pub removed: bool,
}

/// Whether a window has a button where the dock shows `show`.
pub fn is_shown(show: Showing, window: Candidate) -> bool {
    if window.skipped || window.removed {
        return false;
    }
    match show {
        Showing::Display => window.here,
        Showing::All => true,
        Showing::Workspace => window.in_workspace.unwrap_or(true),
        Showing::Minimized => window.here && window.minimized,
    }
}

/// What the buttons are ordered by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortKey {
    /// The place among the pinned applications, for a pinned one.
    pub pinned: Option<usize>,
    /// When the button got its first window, counted up; `None` for a
    /// pinned application that does not run.
    pub opened: Option<u64>,
    /// The name of the application, in lower case.
    pub name: String,
}

/// The indices of `keys` in the order `sorting` puts their buttons in.
/// The order is stable: buttons that compare the same keep theirs.
pub fn order(keys: &[SortKey], sorting: Sorting) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..keys.len()).collect();
    match sorting {
        Sorting::Pinned => indices.sort_by_key(|&index| {
            let key = &keys[index];
            (key.pinned.is_none(), key.pinned, key.opened)
        }),
        Sorting::Opened => indices.sort_by_key(|&index| {
            let key = &keys[index];
            (key.opened.is_none(), key.opened, key.pinned)
        }),
        Sorting::Name => indices.sort_by(|&one, &other| keys[one].name.cmp(&keys[other].name)),
    }
    indices
}

/// Which of `windows`, the one used last first, the wheel brings forward
/// after `front`, going `forward` to those used longer ago or back: the
/// next of them, round, or the first where `front` is none of them.
pub fn cycled<T: PartialEq>(windows: &[T], front: Option<&T>, forward: bool) -> Option<usize> {
    let count = windows.len();
    if count == 0 {
        return None;
    }
    let Some(at) = front.and_then(|front| windows.iter().position(|window| window == front)) else {
        return Some(0);
    };
    Some(match forward {
        true => (at + 1) % count,
        false => (at + count - 1) % count,
    })
}

/// The button the keyboard goes to from the one at `index` of `count`
/// with `pick`: the arrows go round at both ends, Home and End go to the
/// first and the last.
pub fn stepped(index: usize, count: usize, pick: Pick) -> usize {
    if count == 0 {
        return 0;
    }
    let index = index.min(count - 1);
    match pick {
        Pick::Move(by) => (index as isize + by).rem_euclid(count as isize) as usize,
        Pick::First => 0,
        Pick::Last => count - 1,
        Pick::Take | Pick::Leave | Pick::Letter(_) => index,
    }
}

/// Which way the buttons are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// In a row; the marks are under the icons, toward the edge the dock
    /// is at the bottom of, or over them at the top.
    Row,
    /// In a column, with the marks on the side `toward`.
    Column { toward: Side },
}

/// A side of a button in a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Layout {
    /// The layout of a dock `width` by `height` that lies `left` and
    /// `right` away from the sides of its display: a column where it is
    /// taller than wide, with the marks on the side where the display has
    /// more room, which is the side away from the edge it stands at.
    pub fn of(width: f32, height: f32, left: f32, right: f32) -> Self {
        if height <= width {
            return Self::Row;
        }
        let toward = match left <= right {
            true => Side::Right,
            false => Side::Left,
        };
        Self::Column { toward }
    }
}

/// How urgent a state of progress is, for the one button that shows the
/// progress of several windows.
pub fn urgency(state: crate::platform::ProgressState) -> u8 {
    use crate::platform::ProgressState;
    match state {
        ProgressState::Error => 3,
        ProgressState::Paused => 2,
        ProgressState::Normal => 1,
        ProgressState::Indeterminate => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(options: &str) -> WidgetContext {
        WidgetContext {
            display: None,
            options: options.parse().unwrap(),
        }
    }

    #[test]
    fn the_options_choose_and_a_wrong_one_keeps_its_default() {
        let options = DockOptions::of(&context(
            "group = \"never\"\nlabels = true\nshow = \"Minimized\"\nsort = \"name\"\n\
             middle = \"close\"\nwheel = true\nreveal_on_attention = \"until_activated\"\n\
             counter = true",
        ));
        assert_eq!(options.group, Grouping::Never);
        assert!(options.labels && options.wheel && options.counter && options.badges);
        assert_eq!(options.show, Showing::Minimized);
        assert_eq!(options.sort, Sorting::Name);
        assert_eq!(options.middle, Middle::Close);
        assert_eq!(options.reveal, Reveal::UntilActivated);
        let wrong = DockOptions::of(&context("group = \"sometimes\"\nshow = 3"));
        assert_eq!(wrong.group, Grouping::Always);
        assert_eq!(wrong.show, Showing::Display);
        assert_eq!(DockOptions::of(&context("")), DockOptions::of(&context("")));
    }

    #[test]
    fn all_displays_is_what_show_all_says() {
        assert_eq!(
            DockOptions::of(&context("all_displays = true")).show,
            Showing::All
        );
        let both = DockOptions::of(&context("all_displays = true\nshow = \"minimized\""));
        assert_eq!(both.show, Showing::Minimized);
    }

    fn candidate(here: bool, minimized: bool) -> Candidate {
        Candidate {
            here,
            minimized,
            in_workspace: None,
            skipped: false,
            removed: false,
        }
    }

    #[test]
    fn a_window_is_shown_as_the_dock_says() {
        let there = candidate(false, false);
        let here_small = candidate(true, true);
        assert!(!is_shown(Showing::Display, there));
        assert!(is_shown(Showing::All, there));
        assert!(is_shown(Showing::Minimized, here_small));
        assert!(!is_shown(Showing::Minimized, candidate(true, false)));
        let elsewhere = Candidate {
            in_workspace: Some(false),
            ..there
        };
        assert!(!is_shown(Showing::Workspace, elsewhere));
        assert!(is_shown(Showing::Workspace, there));
        let skipped = Candidate {
            skipped: true,
            ..here_small
        };
        assert!(!is_shown(Showing::All, skipped));
        let removed = Candidate {
            removed: true,
            ..here_small
        };
        assert!(!is_shown(Showing::All, removed));
    }

    #[test]
    fn buttons_are_ordered_as_asked_and_stably() {
        let key = |pinned, opened, name: &str| SortKey {
            pinned,
            opened,
            name: name.to_owned(),
        };
        let keys = [
            key(None, Some(3), "editor"),
            key(Some(1), None, "browser"),
            key(Some(0), Some(5), "mail"),
            key(None, Some(1), "terminal"),
        ];
        assert_eq!(order(&keys, Sorting::Pinned), [2, 1, 3, 0]);
        assert_eq!(order(&keys, Sorting::Opened), [3, 0, 2, 1]);
        assert_eq!(order(&keys, Sorting::Name), [1, 0, 2, 3]);
        let same = [key(None, None, "a"), key(None, None, "a")];
        assert_eq!(order(&same, Sorting::Name), [0, 1]);
    }

    #[test]
    fn the_wheel_goes_round_the_windows() {
        let windows = [10, 20, 30];
        assert_eq!(cycled(&windows, Some(&10), true), Some(1));
        assert_eq!(cycled(&windows, Some(&30), true), Some(0));
        assert_eq!(cycled(&windows, Some(&10), false), Some(2));
        assert_eq!(cycled(&windows, Some(&99), true), Some(0));
        assert_eq!(cycled(&windows, None, false), Some(0));
        assert_eq!(cycled::<i32>(&[], None, true), None);
    }

    #[test]
    fn the_keyboard_goes_round_the_buttons() {
        assert_eq!(stepped(0, 3, Pick::Move(-1)), 2);
        assert_eq!(stepped(2, 3, Pick::Move(1)), 0);
        assert_eq!(stepped(1, 3, Pick::Last), 2);
        assert_eq!(stepped(5, 3, Pick::Take), 2);
        assert_eq!(stepped(0, 0, Pick::First), 0);
    }

    #[test]
    fn a_tall_dock_is_a_column_with_its_marks_toward_the_room() {
        assert_eq!(Layout::of(600., 56., 100., 100.), Layout::Row);
        assert_eq!(
            Layout::of(56., 600., 0., 1800.),
            Layout::Column {
                toward: Side::Right
            }
        );
        assert_eq!(
            Layout::of(56., 600., 1800., 0.),
            Layout::Column { toward: Side::Left }
        );
    }

    #[test]
    fn a_failure_is_more_urgent_than_a_pause() {
        use crate::platform::ProgressState;
        assert!(urgency(ProgressState::Error) > urgency(ProgressState::Paused));
        assert!(urgency(ProgressState::Paused) > urgency(ProgressState::Normal));
        assert!(urgency(ProgressState::Normal) > urgency(ProgressState::Indeterminate));
    }
}
