// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Performance mode: how much the shell spends on looks. Off moves
//! everything as the theme says; Minimal has what slides arrive at once
//! and fades take half the time; Extreme moves nothing and asks what
//! plays by itself (live previews, video wallpapers) to pause.
//!
//! `[performance]` chooses the mode by the power source and energy saver:
//! `default`, `on_battery` and `on_energy_saver` (battery saver on older
//! Windows). The action `performance` and the tile of the same name choose
//! one by hand, which holds until the power source or the saver changes,
//! or `performance auto` gives the choice back to the configuration.
//!
//! Other parts ask [`Performance::minimal`] and [`Performance::extreme`]
//! where they decide whether to animate or play.

use gpui::{App, AppContext, AsyncApp, Context, Entity, Global};
use serde::Deserialize;

use crate::motion::Calm;
use crate::platform::{PowerChange, PowerState};

/// How much the shell spends on looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PerformanceMode {
    /// Everything moves as the theme says.
    #[default]
    Off,
    /// Slides arrive at once and fades are quicker.
    Minimal,
    /// Nothing moves, and what plays by itself pauses.
    Extreme,
}

impl PerformanceMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Minimal, Self::Extreme];

    /// What the configuration and an `arg` call the mode.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Extreme => "extreme",
        }
    }

    /// The mode called `name`.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.name() == name)
    }

    /// What the tile says of it, in English.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Minimal => "Minimal",
            Self::Extreme => "Extreme",
        }
    }

    /// The mode after this one, the first after the last.
    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// How much the springs move in this mode.
    fn calm(self) -> Calm {
        match self {
            Self::Off => Calm::Full,
            Self::Minimal => Calm::Reduced,
            Self::Extreme => Calm::Still,
        }
    }
}

/// `[performance]`: the mode by power source and energy saver.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceConfig {
    /// The mode on mains power, and wherever no other line applies.
    #[serde(default)]
    pub default: PerformanceMode,
    /// The mode on battery; `default` where absent.
    pub on_battery: Option<PerformanceMode>,
    /// The mode while energy saver (or battery saver) is on, whatever the
    /// power source; the mode for the source where absent.
    pub on_energy_saver: Option<PerformanceMode>,
}

impl PerformanceConfig {
    /// The mode the configuration chooses for the power in `state`.
    pub fn mode_for(&self, state: PowerState) -> PerformanceMode {
        let by_source = match state.on_battery {
            true => self.on_battery.unwrap_or(self.default),
            false => self.default,
        };
        match state.saver {
            true => self.on_energy_saver.unwrap_or(by_source),
            false => by_source,
        }
    }
}

/// The power as performance mode follows it: the source and both savers,
/// as Windows told them last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Power {
    on_battery: bool,
    battery_saver: bool,
    energy_saver: bool,
}

impl Power {
    fn of(state: PowerState) -> Self {
        Self {
            on_battery: state.on_battery,
            battery_saver: state.saver,
            energy_saver: false,
        }
    }

    fn state(self) -> PowerState {
        PowerState {
            on_battery: self.on_battery,
            saver: self.battery_saver || self.energy_saver,
        }
    }

    /// The power after `change`.
    fn after(mut self, change: PowerChange) -> Self {
        match change {
            PowerChange::OnBattery(on) => self.on_battery = on,
            PowerChange::BatterySaver(on) => self.battery_saver = on,
            PowerChange::EnergySaver(on) => self.energy_saver = on,
        }
        self
    }
}

/// The performance mode in force, and what it follows.
pub struct Performance {
    config: PerformanceConfig,
    power: Power,
    /// The mode chosen by hand, until the power changes.
    chosen: Option<PerformanceMode>,
}

/// The one [`Performance`] of the shell.
struct Shared(Entity<Performance>);

impl Global for Shared {}

impl Performance {
    /// Applies `config` and starts following the power source and the
    /// savers. Called once, before the surfaces open.
    pub fn start(config: &PerformanceConfig, cx: &mut App) {
        let performance = cx.new(|_| Self {
            config: config.clone(),
            power: Power::of(PowerState::read()),
            chosen: None,
        });
        cx.set_global(Shared(performance.clone()));
        Self::apply(cx);
        let Some(changes) = PowerState::watch() else {
            return;
        };
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(change) = changes.recv().await {
                let followed = performance.update(cx, |this, cx| this.power_changed(change, cx));
                if followed.is_err() || cx.update(Self::apply).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Takes a new `[performance]`. A mode chosen by hand stays.
    pub fn configure(config: &PerformanceConfig, cx: &mut App) {
        let Some(performance) = Self::entity(cx) else {
            return;
        };
        performance.update(cx, |this, cx| {
            if this.config != *config {
                this.config = config.clone();
                cx.notify();
            }
        });
        Self::apply(cx);
    }

    /// The one performance of the shell, to observe; `None` before the
    /// shell started.
    pub fn entity(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<Shared>().map(|shared| shared.0.clone())
    }

    /// The one performance of the shell; one that follows nothing and
    /// keeps the default mode where the shell did not start it.
    pub fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(performance) = Self::entity(cx) {
            return performance;
        }
        let performance = cx.new(|_| Self {
            config: PerformanceConfig::default(),
            power: Power::default(),
            chosen: None,
        });
        cx.set_global(Shared(performance.clone()));
        performance
    }

    /// The mode in force; Off before the shell started.
    pub fn mode(cx: &App) -> PerformanceMode {
        Self::entity(cx).map_or(PerformanceMode::Off, |this| this.read(cx).in_force())
    }

    /// Whether the shell saves at least as much as Minimal says: slides
    /// are to arrive at once, and live thumbnails and blur are to be
    /// skipped.
    #[allow(
        dead_code,
        reason = "for the previews, the live thumbnails and the blur, which skip it"
    )]
    pub fn minimal(cx: &App) -> bool {
        Self::mode(cx) >= PerformanceMode::Minimal
    }

    /// Whether the shell saves all it can: nothing is to move, and live
    /// previews and video wallpapers are to pause.
    #[allow(
        dead_code,
        reason = "for the live previews and video wallpapers, which pause in it"
    )]
    pub fn extreme(cx: &App) -> bool {
        Self::mode(cx) == PerformanceMode::Extreme
    }

    /// Puts `mode` in force by hand, or gives the choice back to the
    /// configuration with `None`.
    pub fn choose(mode: Option<PerformanceMode>, cx: &mut App) {
        let Some(performance) = Self::entity(cx) else {
            return;
        };
        performance.update(cx, |this, cx| {
            this.chosen = mode;
            cx.notify();
        });
        Self::apply(cx);
    }

    /// The mode in force.
    pub fn in_force(&self) -> PerformanceMode {
        { self.chosen }.unwrap_or_else(|| self.config.mode_for(self.power.state()))
    }

    /// Whether the mode in force was chosen by hand.
    pub fn by_hand(&self) -> bool {
        self.chosen.is_some()
    }

    fn power_changed(&mut self, change: PowerChange, cx: &mut Context<Self>) {
        let power = self.power.after(change);
        if power == self.power {
            return;
        }
        // A change of the power is when the configuration decides again.
        if power.state() != self.power.state() {
            self.chosen = None;
        }
        self.power = power;
        cx.notify();
    }

    /// Has the springs follow the mode in force.
    fn apply(cx: &mut App) {
        Calm::set(Self::mode(cx).calm());
        cx.refresh_windows();
    }

    /// Does what the action `performance` does with `arg`: no `arg` puts
    /// the next mode in force, `auto` gives the choice back to the
    /// configuration, and the name of a mode puts it in force.
    pub fn perform(arg: Option<&str>, cx: &mut App) {
        match arg {
            None => Self::choose(Some(Self::mode(cx).next()), cx),
            Some("auto") => Self::choose(None, cx),
            Some(name) => Self::choose(PerformanceMode::named(name), cx),
        }
    }

    /// Whether the action `performance` can be done with `arg`, and why
    /// not.
    pub fn check(arg: Option<&str>) -> Result<(), String> {
        match arg {
            None | Some("auto") => Ok(()),
            Some(name) if PerformanceMode::named(name).is_some() => Ok(()),
            Some(_) => Err(
                "`performance` takes `off`, `minimal`, `extreme`, `auto` or no `arg`".to_owned(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(on_battery: bool, saver: bool) -> PowerState {
        PowerState { on_battery, saver }
    }

    #[test]
    fn the_mode_follows_the_power_source_and_the_saver() {
        let config = PerformanceConfig {
            default: PerformanceMode::Off,
            on_battery: Some(PerformanceMode::Minimal),
            on_energy_saver: Some(PerformanceMode::Extreme),
        };
        assert_eq!(config.mode_for(state(false, false)), PerformanceMode::Off);
        assert_eq!(
            config.mode_for(state(true, false)),
            PerformanceMode::Minimal
        );
        assert_eq!(config.mode_for(state(true, true)), PerformanceMode::Extreme);
        assert_eq!(
            config.mode_for(state(false, true)),
            PerformanceMode::Extreme
        );
    }

    #[test]
    fn an_absent_line_falls_back_to_the_one_before() {
        let config = PerformanceConfig {
            default: PerformanceMode::Minimal,
            on_battery: None,
            on_energy_saver: None,
        };
        for (on_battery, saver) in [(false, false), (true, false), (true, true)] {
            assert_eq!(
                config.mode_for(state(on_battery, saver)),
                PerformanceMode::Minimal
            );
        }
    }

    #[test]
    fn either_saver_counts_as_saving() {
        let power = Power::default().after(PowerChange::EnergySaver(true));
        assert!(power.state().saver);
        let power = power.after(PowerChange::EnergySaver(false));
        assert!(!power.state().saver);
        let power = power.after(PowerChange::BatterySaver(true));
        assert!(power.state().saver);
        assert!(power.after(PowerChange::OnBattery(true)).state().on_battery);
    }

    #[test]
    fn the_modes_have_names_and_go_round() {
        for mode in PerformanceMode::ALL {
            assert_eq!(PerformanceMode::named(mode.name()), Some(mode));
        }
        assert_eq!(PerformanceMode::Extreme.next(), PerformanceMode::Off);
        assert!(PerformanceMode::Extreme > PerformanceMode::Minimal);
    }

    #[test]
    fn the_configuration_reads_its_lines() {
        let config: PerformanceConfig =
            toml::from_str("default = \"minimal\"\non_battery = \"extreme\"").unwrap();
        assert_eq!(config.default, PerformanceMode::Minimal);
        assert_eq!(config.on_battery, Some(PerformanceMode::Extreme));
        assert_eq!(config.on_energy_saver, None);
        assert!(toml::from_str::<PerformanceConfig>("speed = 1").is_err());
    }

    #[test]
    fn the_action_takes_the_names_of_the_modes() {
        for arg in [
            None,
            Some("auto"),
            Some("off"),
            Some("minimal"),
            Some("extreme"),
        ] {
            assert_eq!(Performance::check(arg), Ok(()));
        }
        assert!(Performance::check(Some("fast")).is_err());
    }
}
