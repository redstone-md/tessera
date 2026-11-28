// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the editor of the rules reads of a window that was picked or
//! chosen, as KWin's "Detect window properties": its program, its path,
//! its packaged application, its title, its class, its kind, and where it
//! is and how large, in shares of the work area of its display. Each is a
//! suggestion the user ticks; those of the window menu's own rule (the
//! application, or else the program and the class) are ticked at first.
//!
//! Reading asks the window for its title, which waits for its program, so
//! it is done off the thread of the interface.

use crate::platform::{AppWindow, AppWindowId, Display, ScreenRect, WindowKind};
use crate::rules::matching::Named;
use crate::rules::naming;

use super::draft::{Draft, share_value};

/// A window as it was read.
#[derive(Clone, Debug, PartialEq)]
pub struct Detected {
    pub id: AppWindowId,
    pub named: Named,
    /// Where its top left corner is, and how large it is, in shares of the
    /// work area of its display.
    pub position: Option<[f64; 2]>,
    pub size: Option<[f64; 2]>,
}

/// One thing that was read, which can be taken into a rule.
#[derive(Clone, Debug, PartialEq)]
pub enum Suggestion {
    /// A name, by its key, as `exe`.
    Name(&'static str, String),
    Kind(WindowKind),
    Position([f64; 2]),
    Size([f64; 2]),
}

impl Suggestion {
    /// The key of the rule it is written under.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Name(key, _) => key,
            Self::Kind(_) => "type",
            Self::Position(_) => "position",
            Self::Size(_) => "size",
        }
    }

    /// What it says, as the file writes it.
    pub fn text(&self) -> String {
        match self {
            Self::Name(_, name) => name.clone(),
            Self::Kind(kind) => kind.word().to_owned(),
            Self::Position([x, y]) | Self::Size([x, y]) => format!("{x:.3}, {y:.3}"),
        }
    }

    /// Writes it into `draft`: a name to be matched word for word where
    /// the rule says nothing else of how names are matched.
    pub fn take_into(&self, draft: &mut Draft) {
        let pair = |[first, second]: [f64; 2]| {
            toml::Value::Array(vec![share_value(first), share_value(second)])
        };
        match self {
            Self::Name(key, name) => {
                draft.set_text_at(&[], key, name);
                if draft.text("match").is_none() {
                    draft.set("match", Some("equals".into()));
                }
            }
            Self::Kind(kind) => {
                draft.set_text_at(&[], "type", kind.word());
            }
            Self::Position(at) => {
                draft.set("position", Some(pair(*at)));
            }
            Self::Size(size) => {
                draft.set("size", Some(pair(*size)));
            }
        }
    }
}

/// Where the top left corner of `frame` is in `work`, and how large
/// `frame` is, in shares of `work`, each from 0 to 1.
pub fn shares(frame: ScreenRect, work: ScreenRect) -> ([f64; 2], [f64; 2]) {
    let width = f64::from((work.right - work.left).max(1));
    let height = f64::from((work.bottom - work.top).max(1));
    let share = |value: f64| (value.clamp(0., 1.) * 1000.).round() / 1000.;
    let position = [
        share(f64::from(frame.left - work.left) / width),
        share(f64::from(frame.top - work.top) / height),
    ];
    let size = [
        share(f64::from(frame.right - frame.left) / width).max(0.001),
        share(f64::from(frame.bottom - frame.top) / height).max(0.001),
    ];
    (position, size)
}

impl Detected {
    /// Reads the window `id`; `None` for one that is gone. This waits for
    /// the program of the window.
    pub fn read(id: AppWindowId) -> Option<Self> {
        let window = AppWindow::of_id(id)?;
        let named = Named::of(&window);
        let placed = id
            .frame()
            .zip(Display::of_window(id))
            .filter(|_| !id.is_maximized() && !id.is_minimized())
            .map(|(frame, display)| shares(frame, display.work));
        Some(Self {
            id,
            named,
            position: placed.map(|(position, _)| position),
            size: placed.map(|(_, size)| size),
        })
    }

    /// Everything that was read, as suggestions, in the order they are
    /// offered.
    pub fn suggestions(&self) -> Vec<Suggestion> {
        let named = &self.named;
        let names = [
            ("exe", named.exe.clone()),
            ("class", Some(named.class.clone())),
            ("title", Some(named.title.clone())),
            ("path", named.path.clone()),
            ("app", named.app.clone()),
        ];
        let mut found: Vec<Suggestion> = { names.into_iter() }
            .filter_map(|(key, name)| {
                Some(Suggestion::Name(key, name.filter(|name| !name.is_empty())?))
            })
            .collect();
        found.extend(named.kind.map(Suggestion::Kind));
        found.extend(self.position.map(Suggestion::Position));
        found.extend(self.size.map(Suggestion::Size));
        found
    }

    /// Which of [`Self::suggestions`] are ticked at first: those the rule
    /// of the window menu names the window by.
    pub fn ticked_at_first(&self) -> Vec<usize> {
        let first: Vec<&'static str> = naming(&self.named)
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        let suggestions = self.suggestions();
        { suggestions.iter().enumerate() }
            .filter(|(_, suggestion)| {
                matches!(suggestion, Suggestion::Name(key, _) if first.contains(key))
            })
            .map(|(at, _)| at)
            .collect()
    }
}

/// The windows a rule can be made for: those a task switcher lists, each
/// with its title, read off the thread of the interface.
pub fn open_windows() -> Vec<(AppWindowId, String)> {
    { AppWindow::enumerate().into_iter() }
        .filter(|window| !window.is_own())
        .map(|window| (window.id, window.id.title()))
        .filter(|(_, title)| !title.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    fn detected() -> Detected {
        Detected {
            id: AppWindowId::from_raw(1),
            named: Named {
                exe: Some("notepad.exe".to_owned()),
                path: Some(r"C:\Windows\notepad.exe".to_owned()),
                app: None,
                title: "Untitled - Notepad".to_owned(),
                class: "Notepad".to_owned(),
                kind: Some(WindowKind::Normal),
            },
            position: Some([0.25, 0.]),
            size: Some([0.5, 1.]),
        }
    }

    #[test]
    fn a_frame_is_read_in_shares_of_the_work_area() {
        let work = rect(0, 40, 1920, 1080);
        let (position, size) = shares(rect(480, 40, 1440, 1080), work);
        assert_eq!(position, [0.25, 0.]);
        assert_eq!(size, [0.5, 1.]);
        // Off the work area it is kept within the shares.
        let (position, _) = shares(rect(-100, 0, 100, 100), work);
        assert_eq!(position, [0., 0.]);
    }

    #[test]
    fn what_was_read_is_offered_and_the_names_of_the_menu_are_ticked() {
        let window = detected();
        let suggestions = window.suggestions();
        let keys: Vec<&str> = suggestions.iter().map(Suggestion::key).collect();
        assert_eq!(
            keys,
            ["exe", "class", "title", "path", "type", "position", "size"]
        );
        let ticked = window.ticked_at_first();
        let ticked: Vec<&str> = ticked.iter().map(|&at| suggestions[at].key()).collect();
        assert_eq!(ticked, ["exe", "class"]);
    }

    #[test]
    fn a_suggestion_taken_names_the_window_word_for_word() {
        let mut draft = Draft::default();
        for suggestion in detected().suggestions() {
            suggestion.take_into(&mut draft);
        }
        assert_eq!(draft.text("match"), Some("equals"));
        assert_eq!(draft.text("type"), Some("normal"));
        assert_eq!(draft.pair("size"), Some([0.5, 1.]));
        let rule = draft.check().expect("the rule can be used");
        let mut named = detected().named;
        assert!(rule.matches(&named));
        named.title = "Other - Notepad".to_owned();
        assert!(!rule.matches(&named));
    }
}
