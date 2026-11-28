// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Recording a key combination: the keys pressed next anywhere are taken
//! by [`Hotkeys::record`], while the keys held so far are shown, on the
//! Shortcuts page or in a tip at the pointer, as the window menu records
//! a shortcut for a window.

use std::time::Duration;

use gpui::{App, AsyncApp, Context, Task, WeakEntity};

use crate::banners::{Banners, Notice};
use crate::config::KeyConfig;
use crate::config::edit::ConfigFile;
use crate::i18n::Language;
use crate::keys::{Gesture, HELD};
use crate::platform::{Display, Hotkeys, Modifiers, pointer_at};
use crate::shell::Shell;
use crate::snap::anchor_at;
use crate::tip::{TipKey, Tips};

/// How often the keys held are looked at while recording.
const LOOK_EVERY: Duration = Duration::from_millis(50);

/// What the tip of a recording is known by.
const RECORDING_TIP: TipKey = ("key-recording", 0);

/// What a recording says while it waits.
pub const WAITING: &str = "Press the keys…";

/// The keys held, as they are shown: `Ctrl + Alt + …`, or nothing.
pub fn held_shown(held: Modifiers) -> String {
    let down = [held.control, held.alt, held.shift, held.windows];
    let names: Vec<&str> = { HELD.into_iter().zip(down) }
        .filter(|(_, down)| *down)
        .map(|(name, _)| name)
        .collect();
    match names.is_empty() {
        true => String::new(),
        false => format!("{} + \u{2026}", names.join(" + ")),
    }
}

/// Records a combination for the view of `cx`: `held` hears of the keys
/// held whenever they change, and `done` of the combination, or of none
/// where the recording was called off. Dropping the task stops listening;
/// the recording itself ends by itself.
pub fn record<V: 'static>(
    cx: &mut Context<V>,
    held: impl Fn(&mut V, Modifiers, &mut Context<V>) + 'static,
    done: impl FnOnce(&mut V, Option<String>, &mut Context<V>) + 'static,
) -> Task<()> {
    let answer = Hotkeys::record();
    cx.spawn(async move |this: WeakEntity<V>, cx: &mut AsyncApp| {
        let mut before = Modifiers::default();
        loop {
            match answer.try_recv() {
                Ok(recorded) => {
                    this.update(cx, |this, cx| done(this, recorded, cx)).ok();
                    return;
                }
                Err(async_channel::TryRecvError::Closed) => {
                    this.update(cx, |this, cx| done(this, None, cx)).ok();
                    return;
                }
                Err(async_channel::TryRecvError::Empty) => {}
            }
            let now = Modifiers::down_now();
            if now != before {
                before = now;
                if this.update(cx, |this, cx| held(this, now, cx)).is_err() {
                    return;
                }
            }
            cx.background_executor().timer(LOOK_EVERY).await;
        }
    })
}

/// Records a combination with a tip at the pointer of what is held so
/// far, and gives it to `done`, unless it is called off.
pub fn record_with_tip(cx: &mut App, done: impl FnOnce(String, &mut App) + 'static) {
    let answer = Hotkeys::record();
    let point = pointer_at();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut before = None;
        let recorded = loop {
            match answer.try_recv() {
                Ok(recorded) => break recorded,
                Err(async_channel::TryRecvError::Closed) => break None,
                Err(async_channel::TryRecvError::Empty) => {}
            }
            let now = Modifiers::down_now();
            if before != Some(now) {
                before = Some(now);
                let shown = cx.update(|cx| show_tip(now, point, cx));
                if shown.is_err() {
                    return;
                }
            }
            cx.background_executor().timer(LOOK_EVERY).await;
        };
        cx.update(|cx| {
            Tips::leave(RECORDING_TIP, cx);
            if let Some(recorded) = recorded {
                done(recorded, cx);
            }
        })
        .ok();
    })
    .detach();
}

/// Shows the tip of a recording at `point` with the keys `held`.
fn show_tip(held: Modifiers, point: Option<(i32, i32)>, cx: &mut App) {
    let Some(point) = point.or_else(pointer_at) else {
        return;
    };
    let Some(display) = Display::at(point) else {
        return;
    };
    let language = Language::of(cx);
    let text = format!("{} {}", language.say(WAITING), held_shown(held));
    let anchor = anchor_at(&display, point, cx);
    Tips::request(RECORDING_TIP, text.trim_end().to_owned().into(), anchor, cx);
}

/// Writes a change of the `[[key]]` entries away from the thread that
/// draws, and takes the configuration over once it is written; a change
/// that fails is reported, and told in a notice where `tell` is given.
pub fn write_keys(
    change: impl FnOnce(&mut ConfigFile) -> bool + Send + 'static,
    cx: &mut App,
    done: impl FnOnce(Result<(), String>, &mut App) + 'static,
) {
    let written = cx.background_executor().spawn(async move {
        ConfigFile::change(|file| match change(file) {
            true => Ok(()),
            false => Err("the configuration has changed in the meantime".to_owned()),
        })
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        let written = written.await;
        cx.update(|cx| {
            match &written {
                Ok(()) => Shell::config_changed(cx),
                Err(error) => report!("the keys were not written: {error}"),
            }
            done(written, cx);
        })
        .ok();
    })
    .detach();
}

/// Tells in a notice that `key` was written, or why it was not.
pub fn tell_written(key: &KeyConfig, title: &str, written: Result<(), String>, cx: &mut App) {
    let language = Language::of(cx);
    let shown = Gesture::parse(&key.press)
        .map_or_else(|| key.press.clone(), |gesture| gesture.shown(language));
    let (headline, body) = match written {
        Ok(()) => (language.say("Shortcut set"), format!("{shown}: {title}")),
        Err(error) => (language.say("The shortcut was not set"), error),
    };
    let notice = Notice {
        app: language.say("Shortcuts").to_owned(),
        title: headline.to_owned(),
        body,
        glyph: Some("\u{e765}"),
        ..Notice::default()
    };
    Banners::post(notice, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_held_are_shown_before_the_key() {
        assert_eq!(held_shown(Modifiers::default()), "");
        let held = Modifiers::parse("win+ctrl").unwrap();
        assert_eq!(held_shown(held), "Ctrl + Win + \u{2026}");
    }
}
