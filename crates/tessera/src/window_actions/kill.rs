// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The action `kill-window`, as xkill of KDE: every display is covered by
//! a veil with a crosshair, the window under the pointer is outlined, and
//! a click on it asks whether its program is to be ended at once, as End
//! task of Task Manager does; Esc or the right button leave it. The
//! window menu asks the same of the window it is for.

use gpui::{
    App, AsyncApp, Bounds, Context, CursorStyle, FocusHandle, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels,
    Point, Render, StatefulInteractiveElement, Styled, Window, WindowHandle, div, point, px, size,
};

use crate::i18n::Language;
use crate::motion::Interaction;
use crate::platform::{
    AppWindow, AppWindowId, Display, NativeSurface, RunningProcess, ScreenRect, pointer_at,
};
use crate::popup::{open_floating, retire_floating};
use crate::snap::anchor_at;
use crate::theme::Theme;
use crate::ui::{choice_button, icon, one_line, pill, release_hover};

const ICON_KILL: &str = "\u{e7ba}";

/// How much the veil darkens the displays, and how strongly the window
/// under the pointer is filled.
const VEIL: f32 = 0.35;
const OUTLINED: f32 = 0.18;

/// How wide the question is, in logical pixels.
const QUESTION_WIDTH: f32 = 380.;

/// A window whose program may be ended.
#[derive(Clone, Debug, PartialEq)]
struct Candidate {
    id: AppWindowId,
    frame: ScreenRect,
    /// The file name of its program, or the packaged application.
    program: String,
    title: String,
    /// The process that shows what is in the window, which is ended; 0
    /// where it is not found, and nothing is ended.
    process: u32,
}

impl Candidate {
    /// What is known of `window`, unless it is minimized or shows nothing
    /// to click.
    fn of(window: &AppWindow) -> Option<Self> {
        if window.id.is_minimized() {
            return None;
        }
        let file = { window.executable.as_deref() }
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned());
        let process = window.id.content_process().unwrap_or(0);
        // A window of the frame host of packaged applications is named by
        // the application it shows, whose process is ended, not the host.
        let program = match process == window.id.process() {
            true => file.or_else(|| window.app_id.clone()),
            false => window.app_id.clone().or(file),
        };
        Some(Self {
            id: window.id,
            frame: window.id.frame()?,
            program: program.unwrap_or_default(),
            title: window.id.title(),
            process,
        })
    }

    /// What the program is called where it is told that it is not ended.
    fn named(&self) -> String {
        match self.program.is_empty() {
            true => format!("the process {}", self.process),
            false => self.program.clone(),
        }
    }

    /// Whether the window is still there and shown by the same process,
    /// so that a number Windows gave another process since is not ended.
    fn still_there(&self) -> bool {
        self.id.content_process() == Some(self.process)
    }

    /// The windows there are, in their order on the screen, the one in
    /// front first. It looks into their processes: not for the thread of
    /// GPUI.
    fn all() -> Vec<Self> {
        { AppWindow::enumerate().into_iter() }
            .filter_map(|window| Self::of(&window))
            .collect()
    }
}

/// The first of `candidates` that holds `point`, as the windows lie on
/// the screen.
fn under(candidates: &[Candidate], (x, y): (i32, i32)) -> Option<usize> {
    candidates.iter().position(|candidate| {
        let frame = candidate.frame;
        (frame.left..frame.right).contains(&x) && (frame.top..frame.bottom).contains(&y)
    })
}

/// The veils that are open and what they show.
#[derive(Default)]
struct Killing {
    veils: Vec<WindowHandle<KillView>>,
    candidates: Vec<Candidate>,
    hovered: Option<usize>,
    /// The window whose program is ended once that is confirmed.
    asking: Option<Candidate>,
}

impl gpui::Global for Killing {}

impl Killing {
    /// Closes every veil.
    fn close(cx: &mut App) {
        let killing = cx.default_global::<Self>();
        let veils = std::mem::take(&mut killing.veils);
        killing.candidates.clear();
        killing.hovered = None;
        killing.asking = None;
        for veil in veils {
            // A veil that is gone already is fine.
            veil.update(cx, |_, window, cx| retire_floating(window, cx))
                .ok();
        }
    }

    /// Has every veil be drawn again.
    fn redraw(cx: &mut App) {
        let veils = cx.global::<Self>().veils.clone();
        for veil in veils {
            veil.update(cx, |_, _, cx| cx.notify()).ok();
        }
    }

    /// Outlines the window at `point` of the screen.
    fn hover(point: (i32, i32), cx: &mut App) {
        let killing = cx.global_mut::<Self>();
        if killing.asking.is_some() {
            return;
        }
        let hovered = under(&killing.candidates, point);
        if hovered != killing.hovered {
            killing.hovered = hovered;
            Self::redraw(cx);
        }
    }

    /// Asks whether the program of the window at `point` is to be ended,
    /// on `veil`; the other veils close.
    fn choose(point: (i32, i32), veil: WindowHandle<KillView>, cx: &mut App) {
        let killing = cx.global_mut::<Self>();
        let Some(chosen) = under(&killing.candidates, point) else {
            return;
        };
        let Some(asked) = killing.candidates.get(chosen).cloned() else {
            return;
        };
        if asked.process == 0 {
            report!("the program of the window is not found, so nothing is ended");
            return Self::close(cx);
        }
        killing.asking = Some(asked);
        let others: Vec<_> = { killing.veils.iter() }
            .copied()
            .filter(|other| *other != veil)
            .collect();
        killing.veils.retain(|other| *other == veil);
        for other in others {
            other
                .update(cx, |_, window, cx| retire_floating(window, cx))
                .ok();
        }
        // The veil clicked may not be the one that had the keyboard, and
        // the question is answered with Enter and Esc.
        veil.update(cx, |_, window, cx| {
            if let Ok(native) = NativeSurface::of(window) {
                cx.spawn(async move |_, _| native.take_keyboard()).detach();
            }
        })
        .ok();
        Self::redraw(cx);
    }

    /// Ends the program that was asked about, and closes the veils.
    fn confirm(cx: &mut App) {
        let Some(asked) = cx.global_mut::<Self>().asking.take() else {
            return;
        };
        Self::close(cx);
        cx.background_executor()
            .spawn(async move {
                if !asked.still_there() {
                    report!("{} is not ended: its window is gone", asked.named());
                } else if let Err(error) = RunningProcess::kill(asked.process) {
                    report!("{} is not ended: {error:#}", asked.named());
                }
            })
            .detach();
    }
}

/// Covers every display with a veil on which a click on a window asks to
/// end its program; asked again while the veils are open, it closes them.
pub fn pick(cx: &mut App) {
    if !cx.default_global::<Killing>().veils.is_empty() {
        return Killing::close(cx);
    }
    let pointer = pointer_at();
    let under_pointer = pointer.and_then(Display::at);
    let mut veils = Vec::new();
    for display in Display::all() {
        let keyboard = under_pointer.is_none_or(|under| under.key == display.key);
        match open_veil(display, keyboard, cx) {
            Ok(veil) => veils.push(veil),
            Err(error) => report!("a display is not covered to choose a window: {error:#}"),
        }
    }
    cx.global_mut::<Killing>().veils = veils;
    let read = cx.background_executor().spawn(async { Candidate::all() });
    cx.spawn(async move |cx: &mut AsyncApp| {
        let candidates = read.await;
        cx.update(|cx| {
            let killing = cx.global_mut::<Killing>();
            if killing.veils.is_empty() {
                return;
            }
            killing.hovered = pointer.and_then(|pointer| under(&candidates, pointer));
            killing.candidates = candidates;
            Killing::redraw(cx);
        })
        .ok();
    })
    .detach();
}

/// Asks whether the program of the window `id` is to be ended, over the
/// display the window is on.
pub fn ask_to_kill(id: AppWindowId, cx: &mut App) {
    Killing::close(cx);
    let Some(display) = Display::of_window(id) else {
        return;
    };
    let read = cx.background_executor().spawn(async move {
        let listed = { AppWindow::enumerate().into_iter() }
            .filter(|window| window.id == id)
            .find_map(|window| Candidate::of(&window));
        listed.or_else(|| {
            Some(Candidate {
                id,
                frame: id.frame()?,
                program: String::new(),
                title: id.title(),
                process: id.content_process().unwrap_or(0),
            })
        })
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        let Some(candidate) = read.await else {
            return;
        };
        if candidate.process == 0 {
            report!("the program of the window is not found, so nothing is ended");
            return;
        }
        cx.update(|cx| match open_veil(display, true, cx) {
            Ok(veil) => {
                let killing = cx.default_global::<Killing>();
                killing.veils = vec![veil];
                killing.asking = Some(candidate);
                Killing::redraw(cx);
            }
            Err(error) => report!("the program of a window is not asked about: {error:#}"),
        })
        .ok();
    })
    .detach();
}

/// Opens a veil over all of `display`, which takes the keyboard where
/// `keyboard`.
fn open_veil(
    display: Display,
    keyboard: bool,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<KillView>> {
    let frame = display.frame;
    let scale = display.scale.max(0.1);
    let rough = Bounds {
        origin: point(px(frame.left as f32 / scale), px(frame.top as f32 / scale)),
        size: size(
            px((frame.right - frame.left) as f32 / scale),
            px((frame.bottom - frame.top) as f32 / scale),
        ),
    };
    let display_id = anchor_at(&display, (frame.left, frame.top), cx).display_id;
    open_floating(rough, display_id, keyboard, cx, move |window, cx| {
        match NativeSurface::of(window) {
            // The window takes its place only while nothing of the user
            // interface is being worked on, as it is right after this.
            Ok(native) => cx
                .spawn(async move |_, _| {
                    if let Err(error) = settle(native, frame, keyboard) {
                        report!("a veil to choose a window is not where it belongs: {error:#}");
                    }
                })
                .detach(),
            Err(error) => report!("a veil to choose a window is not where it belongs: {error:#}"),
        }
        KillView::new(display, cx)
    })
}

/// Gives the veil `native` its styles, places it over `frame` and shows
/// it, taking the keyboard where `keyboard`. Every veil can take the
/// keyboard, as the one a window is chosen on asks the question.
fn settle(native: NativeSurface, frame: ScreenRect, keyboard: bool) -> anyhow::Result<()> {
    native.adopt_as_popup()?;
    native.place(frame)?;
    native.reveal(false);
    if keyboard {
        native.take_keyboard();
    }
    Ok(())
}

/// The root view of a veil over one display.
pub struct KillView {
    display: Display,
    focus: FocusHandle,
    focused: bool,
    /// The pointer feedback of Cancel and of the button that ends the
    /// program.
    buttons: [Interaction; 2],
}

impl KillView {
    fn new(display: Display, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        Self {
            display,
            focus: cx.focus_handle(),
            focused: false,
            buttons: [Interaction::new(&motion), Interaction::new(&motion)],
        }
    }

    /// `position` of the window, on the screen.
    fn on_screen(&self, position: Point<Pixels>) -> (i32, i32) {
        let scale = self.display.scale;
        (
            self.display.frame.left + (position.x / px(1.) * scale).round() as i32,
            self.display.frame.top + (position.y / px(1.) * scale).round() as i32,
        )
    }

    /// `rect` of the screen in logical pixels of the window: its left,
    /// top, width and height.
    fn logical(&self, rect: ScreenRect) -> [Pixels; 4] {
        let scale = self.display.scale.max(0.1);
        let frame = self.display.frame;
        [
            px((rect.left - frame.left) as f32 / scale),
            px((rect.top - frame.top) as f32 / scale),
            px((rect.right - rect.left) as f32 / scale),
            px((rect.bottom - rect.top) as f32 / scale),
        ]
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => cx.defer(Killing::close),
            "enter" => cx.defer(Killing::confirm),
            _ => {}
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &Window, cx: &mut Context<Self>) {
        let asking = cx.global::<Killing>().asking.is_some();
        match event.button {
            MouseButton::Left if !asking => {
                let point = self.on_screen(event.position);
                let veil = window.window_handle().downcast::<Self>();
                if let Some(veil) = veil {
                    cx.defer(move |cx| Killing::choose(point, veil, cx));
                }
            }
            MouseButton::Right => cx.defer(Killing::close),
            _ => {}
        }
    }

    /// The outline of the window `rect`, where it lies on this display.
    fn outline(&self, rect: ScreenRect, theme: &Theme) -> Option<gpui::Div> {
        let frame = self.display.frame;
        let apart = rect.right <= frame.left
            || rect.left >= frame.right
            || rect.bottom <= frame.top
            || rect.top >= frame.bottom;
        if apart {
            return None;
        }
        let [left, top, width, height] = self.logical(rect);
        Some(
            div()
                .absolute()
                .left(left)
                .top(top)
                .w(width)
                .h(height)
                .rounded(theme.shape.small)
                .border_2()
                .border_color(theme.colors.primary)
                .bg(theme.colors.primary.opacity(OUTLINED)),
        )
    }

    /// What is said at the top of the display while a window is chosen.
    fn hint(theme: &Theme, language: Language) -> gpui::Div {
        div()
            .absolute()
            .top(theme.spacing.medium * 4.)
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                pill(theme)
                    .child(icon(theme, ICON_KILL))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(language.say("Kill window")),
                    )
                    .child(
                        div()
                            .text_color(theme.colors.on_surface_variant)
                            .child(language.say("Click a window to end its program. Esc cancels.")),
                    ),
            )
    }

    /// The question whether the program of `asked` is to be ended.
    fn question(
        &mut self,
        asked: &Candidate,
        theme: &Theme,
        language: Language,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let states = self.buttons.each_mut().map(|button| button.state(window));
        let cancel = choice_button(theme, language.say("Cancel"), false, states[0])
            .id("kill-cancel")
            .on_click(|_, _, cx| cx.defer(Killing::close));
        let kill = choice_button(theme, language.say("Kill process"), true, states[1])
            .id("kill-confirm")
            .on_click(|_, _, cx| cx.defer(Killing::confirm));
        let cancel = Interaction::track(cancel, cx, |this: &mut Self| Some(&mut this.buttons[0]));
        let kill = Interaction::track(kill, cx, |this: &mut Self| Some(&mut this.buttons[1]));
        let program = match asked.program.is_empty() {
            true => format!("{} {}", language.say("Process"), asked.process),
            false => format!(
                "{} ({} {})",
                asked.program,
                language.say("Process"),
                asked.process
            ),
        };
        let card = div()
            .w(px(QUESTION_WIDTH))
            .p(theme.spacing.medium)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .rounded(theme.shape.large)
            .bg(theme.colors.surface_container)
            .text_color(theme.colors.on_surface)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .font_weight(FontWeight::MEDIUM)
                    .child(icon(theme, ICON_KILL))
                    .child(one_line(language.say("Kill process"))),
            )
            .child(one_line(program))
            .child(
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(one_line(asked.title.clone())),
            )
            .child(
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(language.say("It ends at once: what it has not saved is lost.")),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.spacing.small)
                    .child(cancel)
                    .child(kill),
            );
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(card)
    }
}

impl Render for KillView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        if !self.focused {
            self.focused = true;
            self.focus.focus(window);
        }
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let killing = cx.global::<Killing>();
        let asking = killing.asking.clone();
        let outlined = match &asking {
            Some(asked) => Some(asked.frame),
            None => { killing.hovered }
                .and_then(|hovered| killing.candidates.get(hovered))
                .map(|candidate| candidate.frame),
        };
        let mut veil = div()
            .id("kill-window")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .size_full()
            .relative()
            .bg(theme.colors.surface_container.opacity(VEIL))
            .children(outlined.and_then(|rect| self.outline(rect, &theme)));
        match &asking {
            Some(asked) => veil = veil.child(self.question(asked, &theme, language, window, cx)),
            None => {
                veil = veil
                    .cursor(CursorStyle::Crosshair)
                    .child(Self::hint(&theme, language))
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        let point = this.on_screen(event.position);
                        cx.defer(move |cx| Killing::hover(point, cx));
                    }));
            }
        }
        veil.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event, window, cx| this.mouse_down(event, window, cx)),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|this, event, window, cx| this.mouse_down(event, window, cx)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(raw: isize, left: i32, top: i32, right: i32, bottom: i32) -> Candidate {
        Candidate {
            id: AppWindowId::from_raw(raw),
            frame: ScreenRect {
                left,
                top,
                right,
                bottom,
            },
            program: String::new(),
            title: String::new(),
            process: 1,
        }
    }

    #[test]
    fn the_window_in_front_is_the_one_under_the_pointer() {
        let windows = [
            candidate(1, 100, 100, 300, 300),
            candidate(2, 0, 0, 1000, 1000),
        ];
        assert_eq!(under(&windows, (150, 150)), Some(0));
        assert_eq!(under(&windows, (500, 500)), Some(1));
        assert_eq!(under(&windows, (300, 150)), Some(1));
        assert_eq!(under(&windows, (1500, 150)), None);
    }
}
