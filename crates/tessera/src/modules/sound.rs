// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The volume of the default audio output. Widgets: `volume`, a compact
//! readout, and `volume-slider`, the control to change it.

use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Context, Entity, FontWeight, Global, InteractiveElement,
    IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled, Subscription,
    WeakEntity, Window, div,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::Interaction;
use crate::platform::AudioOutput;
use crate::theme::Theme;
use crate::ui::{Slider, icon, pill, state_layer};

/// How often changes made outside Tessera are picked up.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

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
            report!("{error:#}");
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

/// A mute button, a track to drag or scroll over, and the percentage.
struct VolumeSlider {
    sound: Entity<Sound>,
    slider: Slider,
    mute: Interaction,
    _changes: Subscription,
}

impl VolumeSlider {
    fn new(sound: Entity<Sound>, cx: &mut Context<Self>) -> Self {
        let theme = *cx.global::<Theme>();
        let volume = sound.read(cx).level.map_or(0., |level| level.volume);
        Self {
            _changes: cx.observe(&sound, |_, _, cx| cx.notify()),
            sound,
            slider: Slider::new(&theme, volume),
            mute: Interaction::new(&theme.motion),
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

        self.slider.dim(level.muted);
        let track = self.slider.track(
            "volume-track",
            level.volume,
            window,
            cx,
            |this: &mut Self| &mut this.slider,
            |this, volume, cx| {
                this.sound
                    .update(cx, |sound, cx| sound.set_volume(volume, cx));
            },
        );

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
