// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Which windows the switcher offers and in what order: the scope of
//! `[switcher]`, where minimized windows go, the windows of one
//! application, and what is typed while it is open.

use std::path::Path;

use serde::Deserialize;

use crate::platform::ChoiceKey;

/// Which windows the switcher offers, as `scope` of `[switcher]` says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The windows of the workspace that is shown, on every display.
    #[default]
    Workspace,
    /// Those of them on the display the switcher opens on.
    Display,
    /// Those and the windows of the other workspaces of Tessera's own.
    All,
}

/// Where the switcher puts minimized windows, as `minimized` of
/// `[switcher]` says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Minimized {
    /// Where they were last used, like every other window.
    #[default]
    Keep,
    /// After all the windows that are not minimized.
    Last,
    /// Nowhere: they are not offered.
    Hide,
}

impl Minimized {
    /// `windows` in the order the switcher offers them, `minimized`
    /// telling which are: the order stays among the windows that are
    /// minimized and among those that are not.
    pub fn arrange<T>(self, windows: Vec<T>, minimized: impl Fn(&T) -> bool) -> Vec<T> {
        match self {
            Self::Keep => windows,
            Self::Hide => windows.into_iter().filter(|it| !minimized(it)).collect(),
            Self::Last => {
                let (folded, open): (Vec<T>, Vec<T>) = windows.into_iter().partition(&minimized);
                open.into_iter().chain(folded).collect()
            }
        }
    }
}

/// Which windows a step of the switcher goes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Through {
    /// Every window the switcher offers.
    All,
    /// The windows of the application of the chosen one, or, when the
    /// switcher opens, of the window in front.
    App,
}

/// The way and the windows a step of the `switcher` action goes, from
/// its `arg`; `None` for one it does not take.
pub fn step_of(arg: Option<&str>) -> Option<(bool, Through)> {
    Some(match arg {
        None | Some("next") => (false, Through::All),
        Some("previous") => (true, Through::All),
        Some("app-next") => (false, Through::App),
        Some("app-previous") => (true, Through::App),
        _ => return None,
    })
}

/// Whether two windows belong to the same application: both have a
/// program, and it is the same file. Windows does not mind the case of
/// a path.
pub fn same_application(one: Option<&Path>, other: Option<&Path>) -> bool {
    match (one, other) {
        (Some(one), Some(other)) => {
            one.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&other.as_os_str().to_string_lossy())
                || one == other
        }
        _ => false,
    }
}

/// Whether a window titled `title` of the program called `program`
/// answers what was `typed`: every word of it is in the title or in the
/// name of the program, whatever the case. Nothing typed answers every
/// window.
pub fn answers(title: &str, program: &str, typed: &str) -> bool {
    let title = title.to_lowercase();
    let program = program.to_lowercase();
    typed
        .to_lowercase()
        .split_whitespace()
        .all(|word| title.contains(word) || program.contains(word))
}

/// The text a key pressed while the switcher is open types into its
/// filter: what it writes, or the letter on it while Alt is held (Alt
/// keeps the switcher open, and a key held with it writes nothing). A
/// combination with Ctrl but not Alt types nothing; Ctrl with Alt is
/// AltGr, which writes. `None` for a key that writes nothing.
pub fn typed_text(key: &str, written: Option<&str>, control: bool, alt: bool) -> Option<String> {
    let usable = |text: &str| !text.is_empty() && !text.chars().any(char::is_control);
    if control && !alt {
        return None;
    }
    if let Some(written) = written.filter(|written| usable(written)) {
        return Some(written.to_owned());
    }
    match key {
        "space" => Some(" ".to_owned()),
        key if alt && key.chars().count() == 1 && usable(key) => Some(key.to_owned()),
        _ => None,
    }
}

/// The key of a choice that a key GPUI saw is, for when the keyboard
/// hook did not take it first.
pub fn choice_key(key: &str) -> Option<ChoiceKey> {
    Some(match key {
        "escape" => ChoiceKey::Escape,
        "enter" => ChoiceKey::Enter,
        "left" => ChoiceKey::Left,
        "up" => ChoiceKey::Up,
        "right" => ChoiceKey::Right,
        "down" => ChoiceKey::Down,
        _ => return None,
    })
}

/// Where the choice is among `now` after the windows offered changed
/// from `before`, where it was at `chosen`: on the same window where it
/// is still offered, and otherwise at the same place, or the last one.
pub fn rechosen<T: PartialEq>(before: &[T], chosen: usize, now: &[T]) -> usize {
    let same = before
        .get(chosen)
        .and_then(|window| now.iter().position(|other| other == window));
    same.unwrap_or_else(|| chosen.min(now.len().saturating_sub(1)))
}

/// The window chosen when the switcher opens over `count` windows, the
/// one in front being at `front` of them: the one after it, or before it
/// with `back`; the first where none of them is in front.
pub fn first_choice(count: usize, front: Option<usize>, back: bool) -> usize {
    front.map_or(0, |front| stepped(front, count, back))
}

/// The place after `at` among `count`, or before it with `back`; past
/// either end comes the other one.
pub fn stepped(at: usize, count: usize, back: bool) -> usize {
    let count = count.max(1);
    let step = if back { count - 1 } else { 1 };
    (at + step) % count
}

/// How many wheel notches the scrolled `distance` makes with what was
/// scrolled before and not counted yet, `left`, given as lines (pixels
/// are given divided by the height of a line); a notch is one line.
/// Gives the notches, positive for scrolling down, and what is left.
pub fn notches(left: f32, distance: f32) -> (i32, f32) {
    // GPUI gives scrolling up as a positive distance.
    let total = left - distance;
    let whole = total.trunc();
    (whole as i32, total - whole)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn minimized_windows_stay_go_last_or_are_left_out() {
        let windows = vec![(1, false), (2, true), (3, false), (4, true)];
        let folded = |window: &(i32, bool)| window.1;
        let order = |policy: Minimized| -> Vec<i32> {
            { policy.arrange(windows.clone(), folded).into_iter() }
                .map(|window| window.0)
                .collect()
        };
        assert_eq!(order(Minimized::Keep), [1, 2, 3, 4]);
        assert_eq!(order(Minimized::Last), [1, 3, 2, 4]);
        assert_eq!(order(Minimized::Hide), [1, 3]);
    }

    #[test]
    fn the_switcher_is_configured_by_name() {
        #[derive(Deserialize)]
        struct Both {
            scope: Scope,
            minimized: Minimized,
        }
        let both: Both = toml::from_str("scope = \"display\"\nminimized = \"last\"").unwrap();
        assert_eq!(
            (both.scope, both.minimized),
            (Scope::Display, Minimized::Last)
        );
        assert!(toml::from_str::<Both>("scope = \"galaxy\"\nminimized = \"keep\"").is_err());
        assert_eq!(Scope::default(), Scope::Workspace);
        assert_eq!(Minimized::default(), Minimized::Keep);
    }

    #[test]
    fn the_action_goes_through_all_windows_or_those_of_one_application() {
        assert_eq!(step_of(None), Some((false, Through::All)));
        assert_eq!(step_of(Some("next")), Some((false, Through::All)));
        assert_eq!(step_of(Some("previous")), Some((true, Through::All)));
        assert_eq!(step_of(Some("app-next")), Some((false, Through::App)));
        assert_eq!(step_of(Some("app-previous")), Some((true, Through::App)));
        assert_eq!(step_of(Some("sideways")), None);
    }

    #[test]
    fn windows_of_one_program_belong_to_one_application() {
        let notepad = PathBuf::from(r"C:\Windows\notepad.exe");
        let shouted = PathBuf::from(r"C:\WINDOWS\NOTEPAD.EXE");
        let paint = PathBuf::from(r"C:\Windows\mspaint.exe");
        assert!(same_application(Some(&notepad), Some(&shouted)));
        assert!(!same_application(Some(&notepad), Some(&paint)));
        // A window whose program is unknown belongs with none.
        assert!(!same_application(None, None));
        assert!(!same_application(Some(&notepad), None));
    }

    #[test]
    fn every_typed_word_is_in_the_title_or_the_program() {
        assert!(answers("Report.docx - Word", "WINWORD", ""));
        assert!(answers("Report.docx - Word", "WINWORD", "rep"));
        assert!(answers("Report.docx - Word", "WINWORD", "word REP"));
        assert!(answers("Inbox", "thunderbird", "thunder"));
        assert!(!answers("Inbox", "thunderbird", "inbox mail"));
        assert!(answers("Входящие", "почта", "вход"));
    }

    #[test]
    fn keys_type_what_they_write_or_their_letter_with_alt() {
        assert_eq!(typed_text("a", Some("a"), false, false), Some("a".into()));
        assert_eq!(typed_text("a", Some("A"), false, false), Some("A".into()));
        // Held with Alt, a key writes nothing; its letter is taken.
        assert_eq!(typed_text("f", None, false, true), Some("f".into()));
        assert_eq!(typed_text("space", None, false, true), Some(" ".into()));
        // AltGr writes.
        assert_eq!(typed_text("q", Some("@"), true, true), Some("@".into()));
        assert_eq!(typed_text("w", None, true, false), None);
        assert_eq!(typed_text("w", Some("w"), true, false), None);
        assert_eq!(typed_text("f5", None, false, true), None);
        assert_eq!(typed_text("backspace", Some("\u{8}"), false, false), None);
        assert_eq!(choice_key("escape"), Some(ChoiceKey::Escape));
        assert_eq!(choice_key("a"), None);
    }

    #[test]
    fn the_choice_stays_on_its_window_while_the_list_changes() {
        assert_eq!(rechosen(&[1, 2, 3, 4], 2, &[3, 4]), 0);
        assert_eq!(rechosen(&[1, 2, 3, 4], 2, &[1, 2, 4]), 2);
        assert_eq!(rechosen(&[1, 2, 3, 4], 3, &[1, 2]), 1);
        assert_eq!(rechosen(&[1, 2, 3], 1, &[]), 0);
        assert_eq!(rechosen::<i32>(&[], 0, &[5, 6]), 0);
    }

    #[test]
    fn the_switcher_opens_on_the_window_used_before_the_one_in_front() {
        assert_eq!(first_choice(3, Some(0), false), 1);
        assert_eq!(first_choice(3, Some(0), true), 2);
        // With the desktop in front, the window used last is chosen.
        assert_eq!(first_choice(3, None, false), 0);
        assert_eq!(first_choice(3, None, true), 0);
        assert_eq!(first_choice(1, Some(0), false), 0);
    }

    #[test]
    fn stepping_past_an_end_comes_to_the_other_one() {
        assert_eq!(stepped(0, 4, false), 1);
        assert_eq!(stepped(3, 4, false), 0);
        assert_eq!(stepped(0, 4, true), 3);
        assert_eq!(stepped(2, 4, true), 1);
        assert_eq!(stepped(0, 0, false), 0);
    }

    #[test]
    fn the_wheel_counts_whole_notches_and_keeps_the_rest() {
        assert_eq!(notches(0., -1.), (1, 0.));
        assert_eq!(notches(0., 2.), (-2, 0.));
        assert_eq!(notches(0., -0.4), (0, 0.4));
        let (none, left) = notches(0.4, -0.4);
        assert_eq!(none, 0);
        let (one, _) = notches(left, -0.4);
        assert_eq!(one, 1);
    }
}
