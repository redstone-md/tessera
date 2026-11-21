// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The sound of the machine. Widgets: `volume`, a compact readout of the
//! default output; `volume-slider` and `microphone-slider`, the controls of
//! the default output and microphone; `volume-mixer`, a slider for each
//! program that plays; and `audio-outputs`, the outputs to choose from.

use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Context, Entity, FontWeight, Global, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, div, px,
};

use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, staggered};
use crate::platform::{AppSound, AudioDevice, AudioOutput};
use crate::theme::Theme;
use crate::ui::{ROW_HEIGHT, Slider, icon, list_row, list_title, one_line, pill, state_layer};

/// How often changes made outside Tessera are picked up.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

const ICON_MUTED: &str = "\u{e74f}";
const ICON_LEVELS: [&str; 4] = ["\u{e992}", "\u{e993}", "\u{e994}", "\u{e995}"];
const ICON_MICROPHONE: &str = "\u{e720}";
const ICON_MICROPHONE_OFF: &str = "\u{ec54}";
const ICON_OUTPUT: &str = "\u{e7f5}";

/// How far below its place a row starts when it arrives.
const ROW_TRAVEL: f32 = 8.;

/// How many outputs are seen at once; the rest are scrolled to. The half
/// row shows that there are more.
const OUTPUTS_SEEN: f32 = 2.5;

/// The size of the name over the slider of a program.
const NAME_SIZE: f32 = 11.;

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
            cx.new(|cx| LevelControl::new(Channel::Output, cx)).into()
        })?;
        widgets.register("microphone-slider", |_, cx| {
            cx.new(|cx| LevelControl::new(Channel::Microphone, cx))
                .into()
        })?;
        widgets.register("volume-mixer", |_, cx| cx.new(VolumeMixer::new).into())?;
        widgets.register("audio-outputs", |_, cx| cx.new(AudioOutputs::new).into())
    }
}

/// What something that sounds is set to.
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
}

/// What only the popup of the sound shows: the microphone, the programs
/// that play and the outputs. It is read while a widget shows it, and not
/// at all otherwise.
#[derive(Default, PartialEq)]
struct Mixer {
    /// `None` while there is no microphone.
    microphone: Option<Level>,
    programs: Vec<AppSound>,
    outputs: Vec<AudioDevice>,
}

struct SharedMixer(WeakEntity<Mixer>);

impl Global for SharedMixer {}

impl Mixer {
    /// The one instance, which lives for as long as a widget keeps it.
    fn shared(cx: &mut App) -> Entity<Self> {
        let shared = cx.try_global::<SharedMixer>();
        if let Some(mixer) = shared.and_then(|shared| shared.0.upgrade()) {
            return mixer;
        }
        let mixer = cx.new(|cx: &mut Context<Self>| {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor().timer(POLL_INTERVAL).await;
                    if this.update(cx, Self::refresh).is_err() {
                        break;
                    }
                }
            })
            .detach();
            Self::read()
        });
        cx.set_global(SharedMixer(mixer.downgrade()));
        mixer
    }

    fn read() -> Self {
        let microphone = AudioOutput::default_input().ok().and_then(|input| {
            Some(Level {
                volume: input.volume().ok()?,
                muted: input.is_muted().ok()?,
            })
        });
        Self {
            microphone,
            programs: AppSound::all().unwrap_or_default(),
            outputs: AudioDevice::outputs().unwrap_or_default(),
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let read = Self::read();
        if read != *self {
            *self = read;
            cx.notify();
        }
    }
}

/// What a slider sets the volume of.
#[derive(Clone, Debug, PartialEq)]
enum Channel {
    /// The default output.
    Output,
    /// The default microphone.
    Microphone,
    /// What the program of this name plays; the sounds of Windows itself
    /// when the name is empty.
    Program(String),
}

impl Channel {
    fn level(&self, sound: &Sound, mixer: Option<&Mixer>) -> Option<Level> {
        match self {
            Self::Output => sound.level,
            Self::Microphone => mixer?.microphone,
            Self::Program(name) => {
                let programs = mixer?.programs.iter();
                let program = programs.into_iter().find(|sound| &sound.program == name)?;
                Some(Level {
                    volume: program.volume,
                    muted: program.muted,
                })
            }
        }
    }

    fn icon(&self, level: Level) -> &'static str {
        match self {
            Self::Microphone if level.muted => ICON_MICROPHONE_OFF,
            Self::Microphone => ICON_MICROPHONE,
            _ => level.icon(),
        }
    }

    /// What stands in place of the slider when there is nothing to set.
    fn absence(&self) -> Option<&'static str> {
        match self {
            Self::Output => Some("No audio output"),
            Self::Microphone => Some("No microphone"),
            Self::Program(_) => None,
        }
    }

    /// The name over the slider, for a channel that needs one.
    fn title(&self, language: Language) -> Option<SharedString> {
        match self {
            Self::Program(name) if name.is_empty() => Some(language.say("System sounds").into()),
            Self::Program(name) => Some(name.clone().into()),
            _ => None,
        }
    }

    /// Sets the volume and lifts a mute, as the system sliders do.
    fn set_volume(&self, volume: f32) -> anyhow::Result<()> {
        match self {
            Self::Output | Self::Microphone => {
                let device = self.device()?;
                device.set_volume(volume)?;
                device.set_muted(false)
            }
            Self::Program(name) => {
                AppSound::set_volume(name, volume)?;
                AppSound::set_muted(name, false)
            }
        }
    }

    fn set_muted(&self, muted: bool) -> anyhow::Result<()> {
        match self {
            Self::Output | Self::Microphone => self.device()?.set_muted(muted),
            Self::Program(name) => AppSound::set_muted(name, muted),
        }
    }

    fn device(&self) -> anyhow::Result<AudioOutput> {
        match self {
            Self::Microphone => AudioOutput::default_input(),
            _ => AudioOutput::default_device(),
        }
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
                .child(Language::of(cx).say("No audio")),
        }
    }
}

/// A mute button, a track to drag or scroll over, and the percentage, for
/// one channel; under its name when it has one.
struct LevelControl {
    channel: Channel,
    sound: Entity<Sound>,
    /// Kept by the channels the mixer reads.
    mixer: Option<Entity<Mixer>>,
    slider: Slider,
    mute: Interaction,
    /// 0 when the control appears, 1 once it is in place.
    arrival: Spring,
    _changes: Vec<Subscription>,
}

impl LevelControl {
    fn new(channel: Channel, cx: &mut Context<Self>) -> Self {
        let theme = *cx.global::<Theme>();
        let sound = Sound::shared(cx);
        let mixer = (channel != Channel::Output).then(|| Mixer::shared(cx));
        let mut changes = vec![cx.observe(&sound, |_, _, cx| cx.notify())];
        if let Some(mixer) = &mixer {
            changes.push(cx.observe(mixer, |_, _, cx| cx.notify()));
        }
        let mut arrival = Spring::new(theme.motion.spatial, 0.);
        arrival.animate_to(1.);
        let mut this = Self {
            channel,
            sound,
            mixer,
            slider: Slider::new(&theme, 0.),
            mute: Interaction::new(&theme.motion),
            arrival,
            _changes: changes,
        };
        let volume = this.level(cx).map_or(0., |level| level.volume);
        this.slider = Slider::new(&theme, volume);
        this
    }

    fn level(&self, cx: &App) -> Option<Level> {
        let mixer = self.mixer.as_ref().map(|mixer| mixer.read(cx));
        self.channel.level(self.sound.read(cx), mixer)
    }

    /// Reports why `change` failed, if it did, and rereads everything.
    fn changed(&mut self, change: anyhow::Result<()>, cx: &mut Context<Self>) {
        if let Err(error) = change {
            report!("{error:#}");
        }
        self.sound.update(cx, Sound::refresh);
        if let Some(mixer) = &self.mixer {
            mixer.update(cx, Mixer::refresh);
        }
    }

    fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        self.changed(self.channel.set_volume(volume), cx);
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let muted = self.level(cx).is_some_and(|level| level.muted);
        self.changed(self.channel.set_muted(!muted), cx);
    }
}

impl Render for LevelControl {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let Some(level) = self.level(cx) else {
            return div()
                .text_color(theme.colors.on_surface_variant)
                .children(self.channel.absence().map(|text| language.say(text)))
                .into_any_element();
        };
        let slider = theme.slider;
        let arrived = self.arrival.value(window).clamp(0., 1.);

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
            .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx)))
            .child(icon(&theme, self.channel.icon(level)));
        let mute = Interaction::track(mute, cx, |this: &mut Self| Some(&mut this.mute));

        self.slider.dim(level.muted);
        let track = self.slider.track(
            "volume-track",
            level.volume,
            window,
            cx,
            |this: &mut Self| &mut this.slider,
            |this, volume, cx| this.set_volume(volume, cx),
        );

        let control = div()
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
            );
        let Some(title) = self.channel.title(language) else {
            return control.into_any_element();
        };
        div()
            .flex_none()
            .relative()
            .top(px(ROW_TRAVEL * (1. - arrived)))
            .opacity(arrived)
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(
                div()
                    .min_w_0()
                    .px(theme.spacing.small)
                    .text_size(px(NAME_SIZE))
                    .text_color(theme.colors.on_surface_variant)
                    .child(one_line(title)),
            )
            .child(control)
            .into_any_element()
    }
}

/// The programs to give a slider each, in the order of `programs`: those
/// of `kept` that still play stay as they are, and `made` adds the rest.
fn reconciled<T>(
    kept: Vec<(String, T)>,
    programs: &[AppSound],
    mut made: impl FnMut(&str) -> T,
) -> Vec<(String, T)> {
    let mut kept: Vec<_> = kept.into_iter().map(Some).collect();
    let rows = programs.iter().map(|sound| {
        let name = sound.program.as_str();
        let same = |row: &&mut Option<(String, T)>| row.as_ref().is_some_and(|row| row.0 == name);
        let found = kept.iter_mut().find(same).and_then(Option::take);
        found.unwrap_or_else(|| (name.to_owned(), made(name)))
    });
    rows.collect()
}

/// A slider for each program that plays through the default output.
struct VolumeMixer {
    mixer: Entity<Mixer>,
    rows: Vec<(String, Entity<LevelControl>)>,
    _changes: Subscription,
}

impl VolumeMixer {
    fn new(cx: &mut Context<Self>) -> Self {
        let mixer = Mixer::shared(cx);
        let mut this = Self {
            _changes: cx.observe(&mixer, |this: &mut Self, _, cx| this.follow(cx)),
            mixer,
            rows: Vec::new(),
        };
        this.follow(cx);
        this
    }

    fn follow(&mut self, cx: &mut Context<Self>) {
        let programs = self.mixer.read(cx).programs.clone();
        let kept = std::mem::take(&mut self.rows);
        self.rows = reconciled(kept, &programs, |name| {
            cx.new(|cx| LevelControl::new(Channel::Program(name.to_owned()), cx))
        });
        cx.notify();
    }
}

impl Render for VolumeMixer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let list = div()
            .id("volume-mixer")
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .overflow_y_scroll()
            .child(list_title(&theme, language.say("Applications")));
        if self.rows.is_empty() {
            return list.child(
                div()
                    .px(theme.spacing.small)
                    .text_color(theme.colors.on_surface_variant)
                    .child(language.say("No application plays sound")),
            );
        }
        list.children(self.rows.iter().map(|(_, row)| row.clone()))
    }
}

/// The audio outputs; a click on one has the sound go through it.
struct AudioOutputs {
    mixer: Entity<Mixer>,
    sound: Entity<Sound>,
    rows: Vec<Interaction>,
    /// 0 when the list appears, 1 once it is in place.
    arrival: Spring,
    _changes: Subscription,
}

impl AudioOutputs {
    fn new(cx: &mut Context<Self>) -> Self {
        let mixer = Mixer::shared(cx);
        let mut arrival = Spring::new(cx.global::<Theme>().motion.spatial, 0.);
        arrival.animate_to(1.);
        Self {
            _changes: cx.observe(&mixer, |_, _, cx| cx.notify()),
            mixer,
            sound: Sound::shared(cx),
            rows: Vec::new(),
            arrival,
        }
    }

    fn choose(&mut self, device: &AudioDevice, cx: &mut Context<Self>) {
        if let Err(error) = device.choose() {
            report!("{error:#}");
        }
        self.sound.update(cx, Sound::refresh);
        self.mixer.update(cx, Mixer::refresh);
    }
}

impl Render for AudioOutputs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let outputs = self.mixer.read(cx).outputs.clone();
        self.rows
            .resize_with(outputs.len(), || Interaction::new(&theme.motion));
        let arrival = self.arrival.value(window).clamp(0., 1.);
        let count = outputs.len();
        let rows = outputs.into_iter().enumerate().map(|(index, device)| {
            let arrived = staggered(arrival, index, count);
            let state = self.rows[index].state(window);
            let note = if device.in_use {
                language.say("In use")
            } else {
                ""
            };
            let row = list_row(
                &theme,
                ICON_OUTPUT,
                device.in_use,
                device.name.clone(),
                note,
            )
            .id(("audio-output", index))
            .relative()
            .top(px(ROW_TRAVEL * (1. - arrived)))
            .opacity(arrived);
            if device.in_use {
                return row.into_any_element();
            }
            let ground = theme.colors.surface_container_highest;
            let row = row.bg(state_layer(&theme, ground, state));
            Interaction::track(row, cx, move |this: &mut Self| this.rows.get_mut(index))
                .on_click(cx.listener(move |this, _, _, cx| this.choose(&device, cx)))
                .into_any_element()
        });
        let rows: Vec<_> = rows.collect();
        let seen = ROW_HEIGHT * OUTPUTS_SEEN + theme.spacing.small * OUTPUTS_SEEN.floor();
        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .child(list_title(&theme, language.say("Output")))
            .child(
                div()
                    .id("audio-outputs")
                    .max_h(seen)
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.small)
                    .overflow_y_scroll()
                    .children(rows),
            )
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

    #[test]
    fn the_microphone_has_its_own_icon() {
        let level = |muted| Level { volume: 0.5, muted };
        assert_eq!(Channel::Microphone.icon(level(false)), ICON_MICROPHONE);
        assert_eq!(Channel::Microphone.icon(level(true)), ICON_MICROPHONE_OFF);
        assert_eq!(Channel::Output.icon(level(true)), ICON_MUTED);
        let program = Channel::Program("player".to_owned());
        assert_eq!(program.icon(level(false)), ICON_LEVELS[2]);
    }

    #[test]
    fn a_channel_reads_its_own_level() {
        let sound = Sound {
            level: Some(Level {
                volume: 0.3,
                muted: false,
            }),
        };
        let mixer = Mixer {
            microphone: Some(Level {
                volume: 0.8,
                muted: true,
            }),
            programs: vec![AppSound {
                program: "player".to_owned(),
                volume: 0.6,
                muted: false,
            }],
            outputs: Vec::new(),
        };
        let volume =
            |channel: Channel, mixer| channel.level(&sound, mixer).map(|level| level.volume);
        assert_eq!(volume(Channel::Output, None), Some(0.3));
        assert_eq!(volume(Channel::Microphone, Some(&mixer)), Some(0.8));
        assert_eq!(volume(Channel::Microphone, None), None);
        let program = |name: &str| Channel::Program(name.to_owned());
        assert_eq!(volume(program("player"), Some(&mixer)), Some(0.6));
        assert_eq!(volume(program("gone"), Some(&mixer)), None);
    }

    #[test]
    fn the_sliders_follow_the_programs_that_play() {
        let playing = |names: &[&str]| -> Vec<AppSound> {
            let sound = |name: &&str| AppSound {
                program: (*name).to_owned(),
                volume: 1.,
                muted: false,
            };
            names.iter().map(sound).collect()
        };
        let mut made = 0;
        let mut make = |_: &str| {
            made += 1;
            made
        };
        let rows = reconciled(Vec::new(), &playing(&["", "game", "player"]), &mut make);
        assert_eq!(
            rows,
            [
                ("".to_owned(), 1),
                ("game".to_owned(), 2),
                ("player".to_owned(), 3)
            ]
        );
        // The game went quiet and a browser started: the two that stay keep
        // their sliders.
        let rows = reconciled(rows, &playing(&["", "browser", "player"]), &mut make);
        assert_eq!(
            rows,
            [
                ("".to_owned(), 1),
                ("browser".to_owned(), 4),
                ("player".to_owned(), 3)
            ]
        );
    }
}
