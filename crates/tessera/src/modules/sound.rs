// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The sound of the machine. Widgets: `volume`, a compact readout of the
//! default output; `volume-slider` and `microphone-slider`, the controls of
//! the default output and microphone; `volume-mixer`, a slider for each
//! program that plays; `audio-outputs` and `audio-inputs`, the outputs and
//! the microphones to choose from, each with its own volume; `spatial-sound`,
//! whether the output renders spatial sound; and `sound-settings`, the way
//! to the sound settings of Windows. Action: `volume`, which turns the
//! default output up or down or mutes it, as the volume keys do.

use std::path::PathBuf;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use gpui::{
    App, AppContext, AsyncApp, Context, Entity, FontWeight, Global, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, div, px,
};

use crate::actions::{Action, Actions};
use crate::flyout::{Flyouts, Reading, volume_icon};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, staggered};
use crate::platform::{AppSound, AudioDevice, AudioOutput, Shortcut, spatial_sound};
use crate::theme::Theme;
use crate::ui::{
    LIST_MIN_HEIGHT, ROW_HEIGHT, Slider, icon, list_row, list_title, one_line, pill, state_layer,
};

/// How often changes made outside Tessera are picked up.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How often it is asked whether the output renders spatial sound, which
/// changes only in the sound settings.
const SPATIAL_INTERVAL: Duration = Duration::from_secs(5);

const ICON_MICROPHONE: &str = "\u{e720}";
const ICON_MICROPHONE_OFF: &str = "\u{ec54}";
const ICON_OUTPUT: &str = "\u{e7f5}";
const ICON_SPATIAL: &str = "\u{e7f6}";
const ICON_SETTINGS: &str = "\u{e713}";
const ICON_OPEN: &str = "\u{e76c}";

/// The page of the Windows settings for sound, where the outputs, the
/// inputs and the spatial sound format of each are chosen.
const SOUND_SETTINGS: &str = "ms-settings:sound";

/// How far below its place a row starts when it arrives.
const ROW_TRAVEL: f32 = 8.;

/// How many devices are seen at once; the rest are scrolled to. The half
/// row shows that there are more.
const DEVICES_SEEN: f32 = 2.5;

/// The size of the name over the slider of a program.
const NAME_SIZE: f32 = 11.;

/// How high a row is that leads to the settings.
const SLIM_ROW: f32 = 40.;

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
        widgets.register_panel("volume-slider", |_, cx| {
            cx.new(|cx| LevelControl::new(Channel::Output, cx)).into()
        })?;
        widgets.register_panel("microphone-slider", |_, cx| {
            cx.new(|cx| LevelControl::new(Channel::Microphone, cx))
                .into()
        })?;
        widgets.register_panel("volume-mixer", |_, cx| cx.new(VolumeMixer::new).into())?;
        widgets.register_panel("audio-outputs", |context, cx| {
            let volumes = context.flag("volumes").unwrap_or(false);
            cx.new(|cx| DeviceList::new(Flow::Output, volumes, cx))
                .into()
        })?;
        widgets.register_panel("audio-inputs", |context, cx| {
            let volumes = context.flag("volumes").unwrap_or(true);
            cx.new(|cx| DeviceList::new(Flow::Input, volumes, cx))
                .into()
        })?;
        widgets.register_panel("spatial-sound", |_, cx| cx.new(SpatialSound::new).into())?;
        widgets.register_panel("sound-settings", |_, cx| {
            let interaction = Interaction::new(&cx.global::<Theme>().motion);
            cx.new(|_| SoundSettings { interaction }).into()
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        let volume = Action::new(|arg, cx| {
            if let Some(change) = VolumeChange::named(arg) {
                change.apply(cx);
            }
        })
        .checking(|arg| {
            VolumeChange::named(arg).map(|_| ()).ok_or_else(|| {
                "`volume` takes `up`, `down`, `mute` or `mute-microphone`".to_owned()
            })
        });
        actions.register("volume", volume)
    }
}

/// Runs the work of the sound widgets one piece after the other, off the
/// thread that draws: Windows answers each call quickly, but not always,
/// and a change and the reading after it must not swap places.
struct AudioWork(Sender<Box<dyn FnOnce() + Send>>);

impl Global for AudioWork {}

impl AudioWork {
    /// Has `work` done after what was handed over before, and gives its
    /// answer once.
    fn call<R: Send + 'static>(
        work: impl FnOnce() -> R + Send + 'static,
        cx: &mut App,
    ) -> Receiver<R> {
        if !cx.has_global::<Self>() {
            let (queue, waiting) = async_channel::unbounded::<Box<dyn FnOnce() + Send>>();
            cx.background_executor()
                .spawn(async move {
                    while let Ok(work) = waiting.recv().await {
                        work();
                    }
                })
                .detach();
            cx.set_global(Self(queue));
        }
        let (answer, answered) = async_channel::bounded(1);
        let work = Box::new(move || {
            answer.try_send(work()).ok();
        });
        // The queue is held by the task for as long as the program runs.
        cx.global::<Self>().0.try_send(work).ok();
        answered
    }
}

/// What the action `volume` does, as the keys of a keyboard do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeChange {
    /// One step up, as Windows counts steps for the output (2 of 100 for
    /// most), and the mute lifted.
    Up,
    Down,
    /// Mutes the output, or lifts the mute.
    Mute,
    /// Mutes the default microphone, or lifts its mute.
    MuteMicrophone,
}

impl VolumeChange {
    /// The change the `arg` of the action names.
    fn named(arg: Option<&str>) -> Option<Self> {
        match arg {
            Some("up") => Some(Self::Up),
            Some("down") => Some(Self::Down),
            Some("mute") => Some(Self::Mute),
            Some("mute-microphone") => Some(Self::MuteMicrophone),
            _ => None,
        }
    }

    /// Makes the change, and tells what the device is set to after it.
    fn make(self) -> anyhow::Result<Reading> {
        if self == Self::MuteMicrophone {
            let input = AudioOutput::default_input()?;
            input.set_muted(!input.is_muted()?)?;
            return Ok(Reading::Microphone {
                muted: input.is_muted()?,
            });
        }
        let output = AudioOutput::default_device()?;
        match self {
            Self::Up => {
                output.step(true)?;
                output.set_muted(false)?;
            }
            Self::Down => output.step(false)?,
            _ => output.set_muted(!output.is_muted()?)?,
        }
        Ok(Reading::Volume {
            level: output.volume()?,
            muted: output.is_muted()?,
        })
    }

    /// Changes the default output or microphone, then shows what it is set
    /// to in the flyout, where the configuration wants it, and in the
    /// sound widgets.
    pub fn apply(self, cx: &mut App) {
        let changed = AudioWork::call(move || self.make(), cx);
        cx.spawn(async move |cx: &mut AsyncApp| {
            let Ok(changed) = changed.recv().await else {
                return;
            };
            let reading = match changed {
                Ok(reading) => reading,
                Err(error) => {
                    report!("the volume was not changed: {error:#}");
                    return;
                }
            };
            cx.update(|cx| {
                if let Reading::Volume { level, muted } = reading
                    && let Some(shared) = cx.try_global::<SharedSound>()
                {
                    let sound = shared.0.clone();
                    let level = Level {
                        volume: level,
                        muted,
                    };
                    sound.update(cx, |sound, cx| sound.take(Some(level), cx));
                }
                if let Some(mixer) = cx.try_global::<SharedMixer>()
                    && let Some(mixer) = mixer.0.upgrade()
                {
                    mixer.update(cx, Mixer::refresh);
                }
                Flyouts::show(reading, cx);
            })
            .ok();
        })
        .detach();
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
    fn of(output: &AudioOutput) -> Option<Self> {
        Some(Self {
            volume: output.volume().ok()?,
            muted: output.is_muted().ok()?,
        })
    }

    fn icon(self) -> &'static str {
        volume_icon(self.volume, self.muted)
    }

    fn percent(self) -> String {
        format!("{:.0}%", self.volume * 100.)
    }
}

/// The level of the default audio output, shared by every sound widget.
struct Sound {
    /// `None` while there is no output to control.
    level: Option<Level>,
    /// Whether the level was read yet; nothing is shown before.
    known: bool,
    /// How many times the level was read at once, as it is after a widget
    /// sets it. A reading asked for before that is older and is dropped.
    asked: usize,
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
                    let Ok(asked) = this.read_with(cx, |sound, _| sound.asked) else {
                        break;
                    };
                    let Ok(level) = cx.update(|cx| AudioWork::call(Self::read, cx)) else {
                        break;
                    };
                    let level = level.recv().await.ok().flatten();
                    let taken = this.update(cx, |sound, cx| {
                        if sound.asked == asked {
                            sound.take(level, cx);
                        }
                    });
                    if taken.is_err() {
                        break;
                    }
                    cx.background_executor().timer(POLL_INTERVAL).await;
                }
            })
            .detach();
            Self {
                level: None,
                known: false,
                asked: 0,
            }
        });
        cx.set_global(SharedSound(sound.clone()));
        sound
    }

    fn read() -> Option<Level> {
        Level::of(&AudioOutput::default_device().ok()?)
    }

    /// Reads the level again, after a change.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.asked += 1;
        let read = AudioWork::call(Self::read, cx);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Ok(level) = read.recv().await {
                this.update(cx, |sound, cx| sound.take(level, cx)).ok();
            }
        })
        .detach();
    }

    fn take(&mut self, level: Option<Level>, cx: &mut Context<Self>) {
        if level != self.level || !self.known {
            self.level = level;
            self.known = true;
            cx.notify();
        }
    }
}

/// What only the popup of the sound shows: the microphone, the programs
/// that play, and the outputs and inputs. It is read while a widget shows
/// it, and not at all otherwise.
#[derive(Default, PartialEq)]
struct Mixer {
    /// `None` while there is no microphone.
    microphone: Option<Level>,
    programs: Vec<AppSound>,
    outputs: Vec<AudioDevice>,
    inputs: Vec<AudioDevice>,
    /// As [`Sound::asked`].
    asked: usize,
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
                    let Ok(asked) = this.read_with(cx, |mixer, _| mixer.asked) else {
                        break;
                    };
                    let Ok(read) = cx.update(|cx| AudioWork::call(Self::read, cx)) else {
                        break;
                    };
                    let Ok(read) = read.recv().await else {
                        break;
                    };
                    let taken = this.update(cx, |mixer, cx| {
                        if mixer.asked == asked {
                            mixer.take(read, cx);
                        }
                    });
                    if taken.is_err() {
                        break;
                    }
                    cx.background_executor().timer(POLL_INTERVAL).await;
                }
            })
            .detach();
            Self::default()
        });
        cx.set_global(SharedMixer(mixer.downgrade()));
        mixer
    }

    fn read() -> Self {
        Self {
            microphone: AudioOutput::default_input()
                .ok()
                .and_then(|input| Level::of(&input)),
            programs: AppSound::all().unwrap_or_default(),
            outputs: AudioDevice::outputs().unwrap_or_default(),
            inputs: AudioDevice::inputs().unwrap_or_default(),
            asked: 0,
        }
    }

    /// Reads everything again, after a change.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.asked += 1;
        let read = AudioWork::call(Self::read, cx);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Ok(read) = read.recv().await {
                this.update(cx, |mixer, cx| mixer.take(read, cx)).ok();
            }
        })
        .detach();
    }

    fn take(&mut self, mut read: Self, cx: &mut Context<Self>) {
        read.asked = self.asked;
        if read != *self {
            *self = read;
            cx.notify();
        }
    }

    fn devices(&self, flow: Flow) -> &[AudioDevice] {
        match flow {
            Flow::Output => &self.outputs,
            Flow::Input => &self.inputs,
        }
    }
}

/// Which way sound goes through a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    /// Played through it: a speaker or headphones.
    Output,
    /// Recorded from it: a microphone.
    Input,
}

impl Flow {
    fn title(self) -> &'static str {
        match self {
            Self::Output => "Output",
            Self::Input => "Input",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Output => ICON_OUTPUT,
            Self::Input => ICON_MICROPHONE,
        }
    }

    /// What the list says while there is no device.
    fn absence(self) -> &'static str {
        match self {
            Self::Output => "No audio output",
            Self::Input => "No microphone",
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
    /// The output or input Windows knows by this identifier.
    Device(Flow, String),
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
            Self::Device(flow, id) => {
                let devices = mixer?.devices(*flow).iter();
                let device = devices.into_iter().find(|device| &device.id == id)?;
                Some(Level {
                    volume: device.volume?,
                    muted: device.muted,
                })
            }
        }
    }

    fn icon(&self, level: Level) -> &'static str {
        match self {
            Self::Microphone | Self::Device(Flow::Input, _) if level.muted => ICON_MICROPHONE_OFF,
            Self::Microphone | Self::Device(Flow::Input, _) => ICON_MICROPHONE,
            _ => level.icon(),
        }
    }

    /// What stands in place of the slider when there is nothing to set.
    fn absence(&self) -> Option<&'static str> {
        match self {
            Self::Output => Some(Flow::Output.absence()),
            Self::Microphone => Some(Flow::Input.absence()),
            Self::Program(_) | Self::Device(..) => None,
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
            Self::Program(name) => {
                AppSound::set_volume(name, volume)?;
                AppSound::set_muted(name, false)
            }
            _ => {
                let device = self.device()?;
                device.set_volume(volume)?;
                device.set_muted(false)
            }
        }
    }

    fn set_muted(&self, muted: bool) -> anyhow::Result<()> {
        match self {
            Self::Program(name) => AppSound::set_muted(name, muted),
            _ => self.device()?.set_muted(muted),
        }
    }

    /// The device whose own volume the channel is; not for a program.
    fn device(&self) -> anyhow::Result<AudioOutput> {
        match self {
            Self::Microphone => AudioOutput::default_input(),
            Self::Device(_, id) => AudioOutput::device(id),
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
        let sound = self.sound.read(cx);
        if !sound.known {
            return div().into_any_element();
        }
        let readout = pill(&theme).font_weight(FontWeight::MEDIUM);
        let readout = match sound.level {
            Some(level) => readout
                .child(icon(&theme, level.icon()))
                .child(level.percent()),
            None => readout
                .text_color(theme.colors.on_surface_variant)
                .child(Language::of(cx).say("No audio")),
        };
        readout.into_any_element()
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
    /// The volume to set once the change before is done: a drag asks for
    /// many in a row, and only the last one counts.
    wanted: Option<f32>,
    /// Whether a change is being made now.
    busy: bool,
    /// 0 when the control appears, 1 once it is in place.
    arrival: Spring,
    _changes: Vec<Subscription>,
}

/// A change to a channel that is kept to be made later.
type Change = Box<dyn FnOnce(&Channel) -> anyhow::Result<()> + Send>;

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
            wanted: None,
            busy: false,
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

    /// Has `change` made to the channel, then the volume that was wanted
    /// meanwhile set, and everything read again once nothing is left.
    fn change(
        &mut self,
        change: impl FnOnce(&Channel) -> anyhow::Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.busy = true;
        let channel = self.channel.clone();
        let done = AudioWork::call(move || change(&channel), cx);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Ok(Err(error)) = done.recv().await {
                report!("{error:#}");
            }
            this.update(cx, |this, cx| {
                this.busy = false;
                match this.wanted.take() {
                    Some(volume) => {
                        // Boxed, for the change made here is of a type
                        // that names the one before it: each would have
                        // a `change` of its own built, without end.
                        let set: Change = Box::new(move |channel| channel.set_volume(volume));
                        this.change(set, cx)
                    }
                    None => {
                        this.sound.update(cx, Sound::refresh);
                        if let Some(mixer) = &this.mixer {
                            mixer.update(cx, Mixer::refresh);
                        }
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        if self.busy {
            self.wanted = Some(volume);
            return;
        }
        self.change(move |channel| channel.set_volume(volume), cx);
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let muted = self.level(cx).is_some_and(|level| level.muted);
        self.change(move |channel| channel.set_muted(!muted), cx);
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

/// The rows to give a slider each, in the order of `keys`: those of `kept`
/// whose key is still there stay as they are, and `made` adds the rest.
fn reconciled<'a, T>(
    kept: Vec<(String, T)>,
    keys: impl IntoIterator<Item = &'a str>,
    mut made: impl FnMut(&str) -> T,
) -> Vec<(String, T)> {
    let mut kept: Vec<_> = kept.into_iter().map(Some).collect();
    let rows = keys.into_iter().map(|key| {
        let same = |row: &&mut Option<(String, T)>| row.as_ref().is_some_and(|row| row.0 == key);
        let found = kept.iter_mut().find(same).and_then(Option::take);
        found.unwrap_or_else(|| (key.to_owned(), made(key)))
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
        let names = programs.iter().map(|sound| sound.program.as_str());
        let kept = std::mem::take(&mut self.rows);
        self.rows = reconciled(kept, names, |name| {
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
            .min_h(LIST_MIN_HEIGHT)
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

/// The audio outputs or inputs; a click on one has the sound go through
/// it, or be recorded from it. With `volumes`, each has its own slider
/// under it.
struct DeviceList {
    flow: Flow,
    mixer: Entity<Mixer>,
    sound: Entity<Sound>,
    rows: Vec<Interaction>,
    /// The slider of each device, by its identifier, while `volumes`.
    sliders: Option<Vec<(String, Entity<LevelControl>)>>,
    /// 0 when the list appears, 1 once it is in place.
    arrival: Spring,
    _changes: Subscription,
}

impl DeviceList {
    fn new(flow: Flow, volumes: bool, cx: &mut Context<Self>) -> Self {
        let mixer = Mixer::shared(cx);
        let mut arrival = Spring::new(cx.global::<Theme>().motion.spatial, 0.);
        arrival.animate_to(1.);
        let mut this = Self {
            _changes: cx.observe(&mixer, |this: &mut Self, _, cx| this.follow(cx)),
            flow,
            mixer,
            sound: Sound::shared(cx),
            rows: Vec::new(),
            sliders: volumes.then(Vec::new),
            arrival,
        };
        this.follow(cx);
        this
    }

    /// Keeps a slider for each device there is.
    fn follow(&mut self, cx: &mut Context<Self>) {
        if let Some(sliders) = self.sliders.take() {
            let flow = self.flow;
            let devices = self.mixer.read(cx).devices(flow).to_vec();
            let ids = devices.iter().map(|device| device.id.as_str());
            self.sliders = Some(reconciled(sliders, ids, |id| {
                cx.new(|cx| LevelControl::new(Channel::Device(flow, id.to_owned()), cx))
            }));
        }
        cx.notify();
    }

    fn choose(&mut self, device: AudioDevice, cx: &mut Context<Self>) {
        let chosen = AudioWork::call(move || device.choose(), cx);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Ok(Err(error)) = chosen.recv().await {
                report!("{error:#}");
            }
            this.update(cx, |this, cx| {
                this.sound.update(cx, Sound::refresh);
                this.mixer.update(cx, Mixer::refresh);
            })
            .ok();
        })
        .detach();
    }

    /// The slider of the device `id`, if the list shows one.
    fn slider(&self, id: &str) -> Option<Entity<LevelControl>> {
        let sliders = self.sliders.as_ref()?;
        let found = sliders.iter().find(|(known, _)| known == id);
        found.map(|(_, slider)| slider.clone())
    }
}

impl Render for DeviceList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let devices = self.mixer.read(cx).devices(self.flow).to_vec();
        self.rows
            .resize_with(devices.len(), || Interaction::new(&theme.motion));
        let arrival = self.arrival.value(window).clamp(0., 1.);
        let count = devices.len();
        let glyph = self.flow.glyph();
        let rows = devices.into_iter().enumerate().map(|(index, device)| {
            let arrived = staggered(arrival, index, count);
            let state = self.rows[index].state(window);
            let note = if device.in_use {
                language.say("In use")
            } else {
                ""
            };
            let slider = self.slider(&device.id);
            let row = list_row(&theme, glyph, device.in_use, device.name.clone(), note)
                .id(("audio-device", index));
            let row = if device.in_use {
                row.into_any_element()
            } else {
                let ground = theme.colors.surface_container_highest;
                let row = row.bg(state_layer(&theme, ground, state));
                Interaction::track(row, cx, move |this: &mut Self| this.rows.get_mut(index))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(device.clone(), cx)))
                    .into_any_element()
            };
            div()
                .flex_none()
                .relative()
                .top(px(ROW_TRAVEL * (1. - arrived)))
                .opacity(arrived)
                .flex()
                .flex_col()
                .gap(theme.spacing.tiny)
                .child(row)
                .children(slider)
        });
        let rows: Vec<_> = rows.collect();
        let title = list_title(&theme, language.say(self.flow.title()));
        let list = div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .child(title);
        if rows.is_empty() {
            return list.child(
                div()
                    .px(theme.spacing.small)
                    .text_color(theme.colors.on_surface_variant)
                    .child(language.say(self.flow.absence())),
            );
        }
        let row_span = if self.sliders.is_some() {
            ROW_HEIGHT + theme.slider.handle_height + theme.spacing.tiny
        } else {
            ROW_HEIGHT
        };
        list.child(
            div()
                .id(match self.flow {
                    Flow::Output => "audio-outputs",
                    Flow::Input => "audio-inputs",
                })
                .max_h(seen_height(row_span, theme.spacing.small))
                .flex()
                .flex_col()
                .gap(theme.spacing.small)
                .overflow_y_scroll()
                .children(rows),
        )
    }
}

/// How high a list is that shows [`DEVICES_SEEN`] rows `span` high with
/// `gap` between them.
fn seen_height(span: Pixels, gap: Pixels) -> Pixels {
    span * DEVICES_SEEN + gap * DEVICES_SEEN.floor()
}

/// Opens the sound settings of Windows.
fn open_sound_settings() {
    Shortcut {
        name: SOUND_SETTINGS.to_owned(),
        path: PathBuf::from(SOUND_SETTINGS),
    }
    .launch();
}

/// A slim row that opens the sound settings of Windows, showing `text`,
/// and `note` beside it when there is one.
fn settings_row(
    theme: &Theme,
    id: &'static str,
    glyph: &'static str,
    text: &'static str,
    note: Option<&'static str>,
    interaction: &mut Interaction,
    window: &Window,
) -> gpui::Stateful<gpui::Div> {
    let state = interaction.state(window);
    div()
        .id(id)
        .h(px(SLIM_ROW))
        .flex_none()
        .px(theme.spacing.medium)
        .flex()
        .items_center()
        .gap(theme.spacing.medium)
        .rounded(theme.shape.medium)
        .bg(state_layer(theme, theme.colors.surface_container, state))
        .text_color(theme.colors.on_surface_variant)
        .child(icon(theme, glyph).flex_none())
        .child(div().flex_1().min_w_0().child(one_line(text)))
        .children(note.map(|note| div().flex_none().child(note)))
        .child(icon(theme, ICON_OPEN).flex_none())
        .on_click(|_, _, _| open_sound_settings())
}

/// The way to the sound settings of Windows.
struct SoundSettings {
    interaction: Interaction,
}

impl Render for SoundSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let text = Language::of(cx).say("Sound settings");
        let row = settings_row(
            &theme,
            "sound-settings",
            ICON_SETTINGS,
            text,
            None,
            &mut self.interaction,
            window,
        );
        Interaction::track(row, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}

/// Whether the default output renders spatial sound, read every few
/// seconds; nothing where it cannot. Windows lets no other program choose
/// the format, so a click opens the sound settings, where it is chosen.
struct SpatialSound {
    /// `None` until it is known, and where the output cannot have it.
    on: Option<bool>,
    interaction: Interaction,
}

impl SpatialSound {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let Ok(read) = cx.update(|cx| AudioWork::call(spatial_sound, cx)) else {
                    break;
                };
                let on = read.recv().await.ok().flatten();
                let taken = this.update(cx, |this, cx| {
                    if this.on != on {
                        this.on = on;
                        cx.notify();
                    }
                });
                if taken.is_err() {
                    break;
                }
                cx.background_executor().timer(SPATIAL_INTERVAL).await;
            }
        })
        .detach();
        Self {
            on: None,
            interaction: Interaction::new(&cx.global::<Theme>().motion),
        }
    }
}

impl Render for SpatialSound {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let Some(on) = self.on else {
            return div().into_any_element();
        };
        let language = Language::of(cx);
        let note = language.say(if on { "On" } else { "Off" });
        let row = settings_row(
            &theme,
            "spatial-sound",
            ICON_SPATIAL,
            language.say("Spatial sound"),
            Some(note),
            &mut self.interaction,
            window,
        );
        Interaction::track(row, cx, |this: &mut Self| Some(&mut this.interaction))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flyout::{ICON_LEVELS, ICON_MUTED};

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
        let input = Channel::Device(Flow::Input, "mic".to_owned());
        assert_eq!(input.icon(level(true)), ICON_MICROPHONE_OFF);
        assert_eq!(Channel::Output.icon(level(true)), ICON_MUTED);
        let output = Channel::Device(Flow::Output, "speakers".to_owned());
        assert_eq!(output.icon(level(false)), ICON_LEVELS[2]);
        let program = Channel::Program("player".to_owned());
        assert_eq!(program.icon(level(false)), ICON_LEVELS[2]);
    }

    fn device(id: &str, volume: Option<f32>, muted: bool) -> AudioDevice {
        AudioDevice {
            id: id.to_owned(),
            name: id.to_owned(),
            in_use: false,
            volume,
            muted,
        }
    }

    #[test]
    fn a_channel_reads_its_own_level() {
        let sound = Sound {
            level: Some(Level {
                volume: 0.3,
                muted: false,
            }),
            known: true,
            asked: 0,
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
            outputs: vec![device("speakers", Some(0.4), false)],
            inputs: vec![device("mic", Some(0.9), true), device("line", None, false)],
            asked: 0,
        };
        let volume =
            |channel: Channel, mixer| channel.level(&sound, mixer).map(|level| level.volume);
        assert_eq!(volume(Channel::Output, None), Some(0.3));
        assert_eq!(volume(Channel::Microphone, Some(&mixer)), Some(0.8));
        assert_eq!(volume(Channel::Microphone, None), None);
        let program = |name: &str| Channel::Program(name.to_owned());
        assert_eq!(volume(program("player"), Some(&mixer)), Some(0.6));
        assert_eq!(volume(program("gone"), Some(&mixer)), None);
        let device = |flow, id: &str| Channel::Device(flow, id.to_owned());
        assert_eq!(
            volume(device(Flow::Output, "speakers"), Some(&mixer)),
            Some(0.4)
        );
        assert_eq!(volume(device(Flow::Input, "mic"), Some(&mixer)), Some(0.9));
        // An output is not found among the inputs, and a device without a
        // volume of its own has no slider.
        assert_eq!(volume(device(Flow::Input, "speakers"), Some(&mixer)), None);
        assert_eq!(volume(device(Flow::Input, "line"), Some(&mixer)), None);
    }

    #[test]
    fn the_sliders_follow_the_programs_that_play() {
        let mut made = 0;
        let mut make = |_: &str| {
            made += 1;
            made
        };
        let rows = reconciled(Vec::new(), ["", "game", "player"], &mut make);
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
        let rows = reconciled(rows, ["", "browser", "player"], &mut make);
        assert_eq!(
            rows,
            [
                ("".to_owned(), 1),
                ("browser".to_owned(), 4),
                ("player".to_owned(), 3)
            ]
        );
    }

    #[test]
    fn the_volume_action_names_its_change() {
        assert_eq!(VolumeChange::named(Some("up")), Some(VolumeChange::Up));
        assert_eq!(VolumeChange::named(Some("down")), Some(VolumeChange::Down));
        assert_eq!(VolumeChange::named(Some("mute")), Some(VolumeChange::Mute));
        assert_eq!(
            VolumeChange::named(Some("mute-microphone")),
            Some(VolumeChange::MuteMicrophone)
        );
        assert_eq!(VolumeChange::named(Some("louder")), None);
        assert_eq!(VolumeChange::named(None), None);
    }

    #[test]
    fn a_list_shows_two_rows_and_a_half() {
        assert_eq!(seen_height(px(56.), px(8.)), px(156.));
    }
}
