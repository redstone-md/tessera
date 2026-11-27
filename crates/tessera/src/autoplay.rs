// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! AutoPlay for drives that arrive, which Explorer does beside Tessera and
//! nothing does without it: by default only where Tessera is the shell
//! (`[files] autoplay`). When a drive arrives, what it holds and the
//! handlers registered for it are read away from the user interface; the
//! choice the user made in Settings > AutoPlay is run at once, and where
//! the user is to be asked, the notice of the drive gets "Choose what to
//! do", which opens the popup `autoplay` with the handlers to choose from.
//! The choice made there is kept for that kind of drive, as Windows keeps
//! it.

use std::path::{Path, PathBuf};

use gpui::{
    App, Context, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    Render, StatefulInteractiveElement, Styled, Window, div, px,
};
use serde::Deserialize;

use crate::banners::{Banners, Notice, NoticeAction};
use crate::files::Files;
use crate::i18n::Language;
use crate::platform::{AutoPlay, AutoPlayHandler, AutoPlayOffer, Chosen, Invoke, Launch, Medium};
use crate::popup::Popups;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{Chooser, Pick, choice_row_of, one_line};

/// The name of the popup and of its widget.
pub const POPUP: &str = "autoplay";

const ICON_FOLDER: &str = "\u{e8b7}";
const ICON_NOTHING: &str = "\u{e711}";
const ICON_PLAY: &str = "\u{e768}";
const ICON_DRIVE: &str = "\u{e88e}";

/// When Tessera does AutoPlay, `[files] autoplay`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoPlayWhen {
    /// Only as the shell of the session: beside Explorer, Explorer does it.
    #[default]
    Shell,
    /// Also beside Explorer, which then does it as well.
    Always,
    /// Never.
    Off,
}

impl AutoPlayWhen {
    /// Whether AutoPlay is done, `shell` saying whether Tessera is the
    /// shell.
    pub fn active(self, shell: bool) -> bool {
        match self {
            Self::Shell => shell,
            Self::Always => true,
            Self::Off => false,
        }
    }
}

/// A drive the user is asked about.
#[derive(Clone, Debug)]
struct Asked {
    root: PathBuf,
    /// The drive as the user knows it, as "Stick (E:)".
    title: String,
    offer: AutoPlayOffer,
}

/// The drive asked about last, which the popup shows.
#[derive(Default)]
struct Asking(Option<Asked>);

impl Global for Asking {}

/// What a drive that arrived is to the AutoPlay of Windows.
pub fn medium_of(optical: bool, on_device: bool) -> Medium {
    match (optical, on_device) {
        (true, _) => Medium::Optical,
        (false, true) => Medium::Removable,
        (false, false) => Medium::Card,
    }
}

/// Does AutoPlay for the drive at `root`, called `title`, a `medium`;
/// `notice` is the notice of its arrival, where drives that arrive are
/// told of. Where AutoPlay is off, the notice is posted as it is; where
/// a choice was made before, it is run and the notice posted; otherwise
/// the notice offers "Choose what to do", and one is made for it if
/// there is none.
pub fn arrived(root: PathBuf, title: String, medium: Medium, notice: Option<Notice>, cx: &mut App) {
    let read = {
        let root = root.clone();
        cx.background_executor()
            .spawn(async move { AutoPlay::offer(&root, medium) })
    };
    cx.spawn(async move |cx| {
        let offer = read.await;
        cx.update(|cx| match offer {
            Some(offer) => take(Asked { root, title, offer }, notice, cx),
            None => {
                if let Some(notice) = notice {
                    Banners::post(notice, cx);
                }
            }
        })
        .ok();
    })
    .detach();
}

/// Runs what was chosen for `asked`, or asks; then posts the notice.
fn take(asked: Asked, notice: Option<Notice>, cx: &mut App) {
    if let Chosen::Run { handler, event } = &asked.offer.chosen
        && let Some(handler) = asked.offer.handler(handler)
    {
        run(handler, event, &asked.root, cx);
        if let Some(notice) = notice {
            Banners::post(notice, cx);
        }
        return;
    }
    let language = Language::of(cx);
    let choose = NoticeAction::new(language.say("Choose what to do"), |cx| {
        Shell::toggle_popup(POPUP, cx);
    });
    let notice = match notice {
        Some(mut notice) => {
            notice.actions.insert(0, choose);
            notice
        }
        None => Notice {
            app: language.say("AutoPlay").to_owned(),
            title: asked.title.clone(),
            body: language
                .say("Choose what happens with this drive.")
                .to_owned(),
            glyph: Some(ICON_DRIVE),
            actions: vec![choose],
            ..Notice::default()
        },
    };
    cx.set_global(Asking(Some(asked)));
    Banners::post(notice, cx);
}

/// Runs `handler` for `event` with the drive at `root`: the folder opens
/// in the file manager of Tessera where it opens folders, and in Explorer
/// otherwise.
fn run(handler: &AutoPlayHandler, event: &str, root: &Path, cx: &mut App) {
    match handler.invoke {
        Invoke::OpenFolder => {
            if !Files::open_folder(root, cx) {
                Launch::new(root).start();
            }
        }
        Invoke::Nothing => {}
        _ => AutoPlay::run(handler, event, root),
    }
}

/// The handlers to choose from for the drive asked about last: the
/// widget `autoplay`, in the popup of the same name.
pub struct AutoPlayList {
    chooser: Chooser,
}

impl AutoPlayList {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe_global::<Asking>(|this, cx| {
            this.chooser.chosen = 0;
            cx.notify();
        })
        .detach();
        Self {
            chooser: Chooser::new(cx),
        }
    }

    fn asked(cx: &App) -> Option<Asked> {
        cx.try_global::<Asking>()?.0.clone()
    }

    /// Takes the handler `index`: keeps it for this kind of drive, runs it
    /// and closes the popup.
    fn take(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(asked) = Self::asked(cx) else {
            return;
        };
        let Some(handler) = asked.offer.handlers.get(index).cloned() else {
            return;
        };
        let event = asked.offer.event.clone();
        let name = handler.name.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = AutoPlay::remember(&event, &name) {
                    report!("{error:#}");
                }
            })
            .detach();
        run(&handler, &asked.offer.event, &asked.root, cx);
        cx.set_global(Asking(None));
        cx.defer(Popups::close);
    }

    fn key_down(&mut self, event: &KeyDownEvent, count: usize, cx: &mut Context<Self>) {
        match Pick::of(event) {
            Some(Pick::Take) => self.take(self.chooser.chosen, cx),
            Some(Pick::Leave) => cx.defer(Popups::close),
            Some(Pick::Letter(_)) | None => return,
            Some(pick) => self.chooser.pick(pick, count),
        }
        cx.notify();
    }
}

/// The glyph of a handler.
fn glyph(handler: &AutoPlayHandler) -> &'static str {
    match handler.invoke {
        Invoke::OpenFolder => ICON_FOLDER,
        Invoke::Nothing => ICON_NOTHING,
        _ => ICON_PLAY,
    }
}

impl Render for AutoPlayList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        self.chooser.take_focus(window);
        let asked = Self::asked(cx);
        let own_folders = Files::opens_folders(cx);
        let handlers = asked
            .as_ref()
            .map(|asked| asked.offer.handlers.clone())
            .unwrap_or_default();
        let count = handlers.len();
        let mut rows = Vec::with_capacity(count);
        for (index, handler) in handlers.iter().enumerate() {
            let state = self.chooser.state(index, &theme, window);
            let chosen = index == self.chooser.chosen;
            let provider = match (&handler.invoke, own_folders) {
                (Invoke::OpenFolder, true) => language.say("Tessera Files").to_owned(),
                _ => handler.provider.clone(),
            };
            let label = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(one_line(handler.action.clone()))
                .children((!provider.is_empty()).then(|| {
                    div()
                        .text_size(theme.typography.label_size)
                        .text_color(theme.colors.on_surface_variant)
                        .child(one_line(provider))
                }));
            let row = choice_row_of(&theme, glyph(handler), label, None, chosen, state)
                .id(("autoplay", index))
                .on_click(cx.listener(move |this, _, _, cx| this.take(index, cx)));
            rows.push(Chooser::track(row, index, cx, |this: &mut Self| {
                &mut this.chooser
            }));
        }
        let small = |text: &'static str| {
            div()
                .flex_none()
                .px(theme.spacing.small)
                .text_size(px(12.))
                .text_color(theme.colors.on_surface_variant)
                .child(language.say(text))
        };
        div()
            .id(POPUP)
            .track_focus(self.chooser.focus())
            .on_key_down(cx.listener(move |this, event, _, cx| this.key_down(event, count, cx)))
            .flex_none()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(
                div()
                    .flex_none()
                    .px(theme.spacing.small)
                    .font_weight(FontWeight::MEDIUM)
                    .child(one_line(asked.as_ref().map_or_else(
                        || language.say("AutoPlay").to_owned(),
                        |asked| asked.title.clone(),
                    ))),
            )
            .child(small(match asked {
                Some(_) => "Choose what to do with this drive.",
                None => "No drive is waiting for a choice.",
            }))
            .children(rows)
            .children((count > 0).then(|| {
                small("The choice is kept for drives of this kind; Settings > AutoPlay changes it.")
                    .pt(px(2.))
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autoplay_is_done_as_the_shell_by_default() {
        assert!(AutoPlayWhen::default().active(true));
        assert!(!AutoPlayWhen::default().active(false));
        assert!(AutoPlayWhen::Always.active(false));
        assert!(!AutoPlayWhen::Off.active(true));
        let read: toml::Table = toml::from_str("autoplay = \"always\"").unwrap();
        let when: AutoPlayWhen = read["autoplay"].clone().try_into().unwrap();
        assert_eq!(when, AutoPlayWhen::Always);
    }

    #[test]
    fn a_medium_is_told_by_its_drive() {
        assert_eq!(medium_of(true, false), Medium::Optical);
        assert_eq!(medium_of(false, true), Medium::Removable);
        assert_eq!(medium_of(false, false), Medium::Card);
    }
}
