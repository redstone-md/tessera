// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The volume of the default audio output. Widgets: `volume`, a compact
//! readout, and `volume-slider`, the control to change it.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Bounds, Context, Entity, FontWeight, Global, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render,
    ScrollDelta, ScrollWheelEvent, StatefulInteractiveElement, Styled, Subscription, WeakEntity,
    Window, canvas, div, px,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring};
use crate::platform::AudioOutput;
use crate::theme::Theme;
use crate::ui::{icon, pill, state_layer};

/// How often changes made outside Tessera are picked up.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// The share of the range one notch of the mouse wheel moves the volume.
const WHEEL_STEP: f32 = 0.02;

const ICON_MUTED: &str = "\u{e74f}";
const ICON_LEVELS: [&str; 4] = ["\u{e992}", "\u{e993}", "\u{e994}", "\u{e995}"];

pub struct SoundModule;

impl Module for SoundModule {
    fn name(&self) -> &'static str {
        "sound"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("volume", |_, cx| {
            let sound = Sound::shared(cx);
            cx.new(|cx| VolumeReadout {
                _changes: cx.observe(&sound, |_, _, cx| cx.notify()),
                sound,
            })
            .into()
        })?;
        widgets.register("volume-slider", |_, cx| {
            let sound = Sound::shared(cx);
            cx.new(|cx| VolumeSlider::new(sound, cx)).into()
        })
    }
}

/// What the output is set to.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Level {
    /// From 0 to 1.
    volume: f32,
    muted: bool,
}

impl Level {
    fn icon(self) -> &'static str {
        if self.muted {
            return ICON_MUTED;
        }
        let step = (self.volume * 3.).ceil().clamp(0., 3.) as usize;
        ICON_LEVELS[step]
    }

    fn percent(self) -> String {
        format!("{:.0}%", self.volume * 100.)
    }
}

/// The level of the default audio output, shared by every sound widget.
struct Sound {
    /// `None` while there is no output to control.
    level: Option<Level>,
}

struct SharedSound(Entity<Sound>);

impl Global for SharedSound {}

impl Sound {
    /// The one instance, created when the first widget asks for it.
    fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(shared) = cx.try_global::<SharedSound>() {
            return shared.0.clone();
        }
        let sound = cx.new(|cx: &mut Context<Self>| {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor().timer(POLL_INTERVAL).await;
                    if this.update(cx, Self::refresh).is_err() {
                        break;
                    }
                }
            })
            .detach();
            Self {
                level: Self::read(),
            }
        });
        cx.set_global(SharedSound(sound.clone()));
        sound
    }

    fn read() -> Option<Level> {
        let output = AudioOutput::default_device().ok()?;
        Some(Level {
            volume: output.volume().ok()?,
            muted: output.is_muted().ok()?,
        })
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let level = Self::read();
        if level != self.level {
            self.level = level;
            cx.notify();
        }
    }

    /// Applies `change` to the current default output and rereads it.
    fn change(
        &mut self,
        change: impl FnOnce(&AudioOutput) -> anyhow::Result<()>,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = AudioOutput::default_device().and_then(|output| change(&output)) {
            eprintln!("tessera: {error:#}");
        }
        self.refresh(cx);
    }

    /// Sets the volume and lifts a mute, as the system slider does.
    fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        self.change(
            |output| {
                output.set_volume(volume)?;
                output.set_muted(false)
            },
            cx,
        );
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let muted = self.level.is_some_and(|level| level.muted);
        self.change(|output| output.set_muted(!muted), cx);
    }
}

/// Shows the volume as an icon and a percentage.
struct VolumeReadout {
    sound: Entity<Sound>,
    _changes: Subscription,
}

impl Render for VolumeReadout {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let readout = pill(&theme).font_weight(FontWeight::MEDIUM);
        match self.sound.read(cx).level {
            Some(level) => readout
                .child(icon(&theme, level.icon()))
                .child(level.percent()),
            None => readout
                .text_color(theme.colors.on_surface_variant)
                .child("No audio"),
        }
    }
}

/// The volume for a pointer at `x`, on a track that starts at `left` and is
/// `width` wide, with `inset` at each end taken by half the handle.
fn volume_at(x: f32, left: f32, width: f32, inset: f32) -> f32 {
    let travel = width - inset * 2.;
    if travel <= 0. {
        return 0.;
    }
    ((x - left - inset) / travel).clamp(0., 1.)
}

/// A mute button, a track to drag or scroll over, and the percentage.
struct VolumeSlider {
    sound: Entity<Sound>,
    /// The volume as drawn; it follows the real one.
    shown: Spring,
    dragging: bool,
    /// Where the track was painted last, in window coordinates.
    track: Rc<Cell<Bounds<Pixels>>>,
    mute: Interaction,
    _changes: Subscription,
}

impl VolumeSlider {
    fn new(sound: Entity<Sound>, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let volume = sound.read(cx).level.map_or(0., |level| level.volume);
        Self {
            _changes: cx.observe(&sound, |_, _, cx| cx.notify()),
            sound,
            shown: Spring::new(motion.effects, volume),
            dragging: false,
            track: Rc::default(),
            mute: Interaction::new(&motion),
        }
    }

    fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        self.sound
            .update(cx, |sound, cx| sound.set_volume(volume, cx));
    }

    /// Moves the volume to where the pointer is on the track.
    fn follow(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let theme = *cx.global::<Theme>();
        let track = self.track.get();
        let inset = f32::from(theme.slider.handle_width) / 2. + f32::from(theme.slider.handle_gap);
        let volume = volume_at(
            f32::from(x),
            f32::from(track.origin.x),
            f32::from(track.size.width),
            inset,
        );
        self.set_volume(volume, cx);
    }

    fn nudge(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let up = match event.delta {
            ScrollDelta::Lines(lines) => lines.y,
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y),
        };
        let Some(level) = self.sound.read(cx).level else {
            return;
        };
        if up != 0. {
            self.set_volume(level.volume + WHEEL_STEP * up.signum(), cx);
        }
    }
}

impl Render for VolumeSlider {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let Some(level) = self.sound.read(cx).level else {
            return div()
                .text_color(theme.colors.on_surface_variant)
                .child("No audio output")
                .into_any_element();
        };
        self.shown.animate_to(level.volume);
        let shown = self.shown.value(window).clamp(0., 1.);
        let slider = theme.slider;

        let mute_state = self.mute.state(window);
        let mute = div()
            .id("volume-mute")
            .size(slider.handle_height)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(
                &theme,
                theme.colors.surface_container,
                mute_state,
            ))
            .on_click(cx.listener(|this, _, _, cx| {
                this.sound.update(cx, Sound::toggle_mute);
            }))
            .child(icon(&theme, level.icon()));
        let mute = Interaction::track(mute, cx, |this: &mut Self| Some(&mut this.mute));

        let width = self.track.get().size.width;
        let travel = (width - slider.handle_width - slider.handle_gap * 2.).max(px(0.));
        let (active, inactive) = if level.muted {
            (theme.colors.outline, theme.colors.surface_container_highest)
        } else {
            (theme.colors.primary, theme.colors.secondary_container)
        };
        let painted = self.track.clone();
        let track = div()
            .id("volume-track")
            .flex_1()
            .h(slider.handle_height)
            .relative()
            .flex()
            .items_center()
            .gap(slider.handle_gap)
            .child(
                canvas(move |bounds, _, _| painted.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                div()
                    .flex_none()
                    .w(travel * shown)
                    .h(slider.track_height)
                    .rounded_l(theme.shape.full)
                    .rounded_r(slider.inner_radius)
                    .bg(active),
            )
            .child(
                div()
                    .flex_none()
                    .w(slider.handle_width)
                    .h_full()
                    .rounded(theme.shape.full)
                    .bg(active),
            )
            .child(
                div()
                    .flex_none()
                    .w(travel * (1. - shown))
                    .h(slider.track_height)
                    .rounded_l(slider.inner_radius)
                    .rounded_r(theme.shape.full)
                    .bg(inactive),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.dragging = true;
                    this.follow(event.position.x, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if !this.dragging {
                    return;
                }
                if event.pressed_button == Some(MouseButton::Left) {
                    this.follow(event.position.x, cx);
                } else {
                    this.dragging = false;
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .on_scroll_wheel(cx.listener(|this, event, _, cx| this.nudge(event, cx)));

        div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(mute)
            .child(track)
            .child(
                div()
                    .flex_none()
                    .w(slider.value_width)
                    .flex()
                    .justify_end()
                    .font_weight(FontWeight::MEDIUM)
                    .child(level.percent()),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_track_maps_the_pointer_to_a_volume() {
        assert_eq!(volume_at(100., 100., 220., 10.), 0.);
        assert_eq!(volume_at(110., 100., 220., 10.), 0.);
        assert_eq!(volume_at(210., 100., 220., 10.), 0.5);
        assert_eq!(volume_at(310., 100., 220., 10.), 1.);
        assert_eq!(volume_at(900., 100., 220., 10.), 1.);
        assert_eq!(volume_at(5., 100., 0., 10.), 0.);
    }

    #[test]
    fn the_icon_follows_the_level() {
        let level = |volume, muted| Level { volume, muted };
        assert_eq!(level(0.5, true).icon(), ICON_MUTED);
        assert_eq!(level(0., false).icon(), ICON_LEVELS[0]);
        assert_eq!(level(0.2, false).icon(), ICON_LEVELS[1]);
        assert_eq!(level(0.5, false).icon(), ICON_LEVELS[2]);
        assert_eq!(level(1., false).icon(), ICON_LEVELS[3]);
        assert_eq!(level(0.426, false).percent(), "43%");
    }
}
