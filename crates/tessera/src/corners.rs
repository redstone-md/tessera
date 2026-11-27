// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Hot corners: each `[[corner]]` of the configuration names a corner or
//! an edge of the displays, or of one display, and what is done when the
//! pointer rests there for a moment: an action, as a `[[key]]` names one,
//! a popup to open, or a program to start. It is done once, and again
//! only after the pointer has gone some way off; nothing is done while a
//! button of the pointer is held, as while a window is dragged, or while
//! the window in front fills the display, as a game does.
//!
//! The pointer is looked at from a thread of its own: ten times a second
//! while it is far from every corner in use, and more often near one, so
//! that the wait is kept to; there is no such thread while no corner is
//! configured.

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use gpui::{App, AsyncApp, Global};
use serde::Deserialize;

use crate::actions::Actions;
use crate::platform::{Display, ScreenRect, front_fills, pointer_at, pointer_held};
use crate::shell::Shell;

/// How often the pointer is looked at when it is far from every corner
/// in use, and when it is near one.
const FAR_LOOK: Duration = Duration::from_millis(100);
const NEAR_LOOK: Duration = Duration::from_millis(20);

/// How near a corner or an edge in use the pointer is to be looked at
/// more often, in physical pixels.
const NEAR: i32 = 48;

/// How far the pointer is to be from the corners and edges of a display
/// to be in one, in physical pixels: a corner is a small square, an edge
/// a thin strip.
const CORNER: i32 = 4;
const EDGE: i32 = 2;

/// How far the pointer has to go from where a corner was used before it
/// can be used again.
const REARM: i32 = 40;

/// How long the pointer rests in a corner when the configuration says
/// nothing, and the longest it may say, in milliseconds.
const DEFAULT_DELAY: u32 = 150;
const LONGEST_DELAY: u32 = 5000;

/// How often the arrangement of the displays is read again.
const DISPLAYS_LOOK: Duration = Duration::from_secs(2);

/// A corner or an edge of a display.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Spot {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Top,
    Bottom,
    Left,
    Right,
}

impl Spot {
    /// Whether a corner in use at this spot is used with the pointer at
    /// `hit`: a corner at that corner, an edge anywhere along it.
    fn covers(self, hit: Self) -> bool {
        use Spot::*;
        self == hit
            || matches!(
                (self, hit),
                (Top, TopLeft | TopRight)
                    | (Bottom, BottomLeft | BottomRight)
                    | (Left, TopLeft | BottomLeft)
                    | (Right, TopRight | BottomRight)
            )
    }
}

/// Which corner or edge of the display `frame` the pointer at `(x, y)`
/// is in, if any; a corner is told before the edges it lies on.
fn spot_in(frame: ScreenRect, (x, y): (i32, i32)) -> Option<Spot> {
    let inside = (frame.left..frame.right).contains(&x) && (frame.top..frame.bottom).contains(&y);
    if !inside {
        return None;
    }
    let near = |from: i32, to: i32, within: i32| (from - to).abs() < within;
    let (left, right) = (
        near(x, frame.left, CORNER),
        near(x, frame.right - 1, CORNER),
    );
    let (top, bottom) = (
        near(y, frame.top, CORNER),
        near(y, frame.bottom - 1, CORNER),
    );
    match (left, right, top, bottom) {
        (true, _, true, _) => return Some(Spot::TopLeft),
        (_, true, true, _) => return Some(Spot::TopRight),
        (true, _, _, true) => return Some(Spot::BottomLeft),
        (_, true, _, true) => return Some(Spot::BottomRight),
        _ => {}
    }
    match () {
        () if near(y, frame.top, EDGE) => Some(Spot::Top),
        () if near(y, frame.bottom - 1, EDGE) => Some(Spot::Bottom),
        () if near(x, frame.left, EDGE) => Some(Spot::Left),
        () if near(x, frame.right - 1, EDGE) => Some(Spot::Right),
        () => None,
    }
}

/// The display, of `frames`, and the corner or edge the pointer at
/// `point` is in.
fn hit(point: (i32, i32), frames: &[ScreenRect]) -> Option<(usize, Spot)> {
    frames
        .iter()
        .enumerate()
        .find_map(|(index, &frame)| Some((index, spot_in(frame, point)?)))
}

/// Whether `point` is within [`NEAR`] of the edge of `frame` that `spot`
/// lies on, or of both edges of a corner.
fn nearby(frame: ScreenRect, spot: Spot, (x, y): (i32, i32)) -> bool {
    let inside = (frame.left - NEAR..frame.right + NEAR).contains(&x)
        && (frame.top - NEAR..frame.bottom + NEAR).contains(&y);
    let (left, right) = (
        (x - frame.left).abs() < NEAR,
        (frame.right - x).abs() < NEAR,
    );
    let (top, bottom) = (
        (y - frame.top).abs() < NEAR,
        (frame.bottom - y).abs() < NEAR,
    );
    inside
        && match spot {
            Spot::TopLeft => left && top,
            Spot::TopRight => right && top,
            Spot::BottomLeft => left && bottom,
            Spot::BottomRight => right && bottom,
            Spot::Top => top,
            Spot::Bottom => bottom,
            Spot::Left => left,
            Spot::Right => right,
        }
}

/// What a corner does.
#[derive(Clone, Debug, PartialEq)]
enum Deed {
    Action { name: String, arg: Option<String> },
    Popup(String),
    Run(Vec<String>),
}

impl Deed {
    fn done(&self, cx: &mut App) {
        match self {
            Self::Action { name, arg } => Actions::run(name, arg.as_deref(), cx),
            Self::Popup(name) => Shell::toggle_popup(name, cx),
            Self::Run(command) => {
                let started = Command::new(&command[0])
                    .args(&command[1..])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                if let Err(error) = started {
                    report!("`{}` did not start: {error}", command[0]);
                }
            }
        }
    }
}

/// A `[[corner]]`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CornerConfig {
    /// The corner or the edge.
    at: Spot,
    /// The display, counted from 1 from the left; every display without.
    display: Option<usize>,
    /// How long the pointer rests there first, in milliseconds.
    delay: Option<u32>,
    /// The name of an action, as a `[[key]]` names one.
    action: Option<String>,
    /// What the action is done with.
    arg: Option<String>,
    /// A popup to open, or to close when it is open.
    popup: Option<String>,
    /// A program to start, and its arguments.
    run: Option<Vec<String>>,
}

impl CornerConfig {
    /// What the corner does, or why it does nothing; `actions` are those it
    /// can name.
    fn deed(&self, actions: &Actions) -> Result<Deed, String> {
        let deeds = [
            self.action.as_ref().map(|name| {
                actions.check(name, self.arg.as_deref())?;
                Ok(Deed::Action {
                    name: name.clone(),
                    arg: self.arg.clone(),
                })
            }),
            self.popup.clone().map(|popup| Ok(Deed::Popup(popup))),
            self.run.as_ref().map(|run| match run.is_empty() {
                true => Err("`run` names no program".to_owned()),
                false => Ok(Deed::Run(run.clone())),
            }),
        ];
        if self.arg.is_some() && self.action.is_none() {
            return Err("`arg` is what an `action` is done with, and there is none".to_owned());
        }
        let mut deeds = deeds.into_iter().flatten();
        match (deeds.next(), deeds.next()) {
            (Some(deed), None) => deed,
            _ => Err("it needs one of `action`, `popup` and `run`".to_owned()),
        }
    }

    /// How long the pointer rests there first.
    fn delay(&self) -> Duration {
        let delay = match self.delay {
            Some(delay) if delay > LONGEST_DELAY => {
                report!(
                    "a `[[corner]]` waits {delay} ms, more than {LONGEST_DELAY}, so it waits {DEFAULT_DELAY}"
                );
                DEFAULT_DELAY
            }
            delay => delay.unwrap_or(DEFAULT_DELAY),
        };
        Duration::from_millis(u64::from(delay))
    }
}

/// A corner in use, as the thread that looks at the pointer knows it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Spotted {
    at: Spot,
    /// The display, counted from 0 from the left.
    display: Option<usize>,
    delay: Duration,
}

/// Which of `corners` is used with the pointer at `spot` of the display at
/// `display`: one at that very corner first, then one along an edge, each
/// in the order of the file.
fn chosen(corners: &[Spotted], display: usize, spot: Spot) -> Option<usize> {
    let here = |corner: &&Spotted| corner.display.is_none_or(|only| only == display);
    let exact = corners
        .iter()
        .position(|corner| here(&corner) && corner.at == spot);
    exact.or_else(|| { corners.iter() }.position(|corner| here(&corner) && corner.at.covers(spot)))
}

/// Waits for the pointer to rest in a corner, and for it to go away again.
#[derive(Debug, Default)]
struct Trigger {
    /// The corner the pointer is in and since when.
    resting: Option<(usize, Instant)>,
    /// Where the pointer was when a corner was used last, while it has
    /// not gone far enough away.
    used_at: Option<(i32, i32)>,
}

impl Trigger {
    /// Takes in that the pointer is at `point`, in the corner at `corner`
    /// of those in use if it is in one, at `now`. The corner to use now
    /// is returned, if the pointer has rested there for `delay`.
    fn see(
        &mut self,
        point: (i32, i32),
        corner: Option<usize>,
        delay: impl Fn(usize) -> Duration,
        now: Instant,
    ) -> Option<usize> {
        if let Some((x, y)) = self.used_at {
            if (point.0 - x).abs() < REARM && (point.1 - y).abs() < REARM {
                return None;
            }
            self.used_at = None;
        }
        let Some(corner) = corner else {
            self.resting = None;
            return None;
        };
        let since = match self.resting {
            Some((resting, since)) if resting == corner => since,
            _ => {
                self.resting = Some((corner, now));
                now
            }
        };
        if now.duration_since(since) < delay(corner) {
            return None;
        }
        self.resting = None;
        self.used_at = Some(point);
        Some(corner)
    }

    /// Forgets where the pointer rests, as while a button is held.
    fn reset(&mut self) {
        self.resting = None;
    }
}

/// Counts the threads started to look at the pointer; only the latest
/// goes on.
static LOOKER: AtomicU64 = AtomicU64::new(0);

/// The corners in use.
#[derive(Default)]
pub struct Corners {
    configs: Vec<CornerConfig>,
    deeds: Vec<Deed>,
}

impl Global for Corners {}

impl Corners {
    /// Uses the corners of `configs` from now on; the actions they name
    /// are to be registered by now.
    pub fn start(configs: &[CornerConfig], cx: &mut App) {
        cx.set_global(Self::default());
        Self::configure(configs, cx);
    }

    /// Takes over the corners of a configuration that was read again, if
    /// they changed. One that cannot be used is reported and left out.
    pub fn configure(configs: &[CornerConfig], cx: &mut App) {
        if cx.global::<Self>().configs == configs {
            return;
        }
        let mut deeds = Vec::new();
        let mut spotted = Vec::new();
        for config in configs {
            match config.deed(cx.default_global::<Actions>()) {
                Ok(deed) => {
                    deeds.push(deed);
                    spotted.push(Spotted {
                        at: config.at,
                        display: config.display.map(|number| number.saturating_sub(1)),
                        delay: config.delay(),
                    });
                }
                Err(why) => report!("a `[[corner]]` does nothing: {why}"),
            }
        }
        let this = cx.global_mut::<Self>();
        this.configs = configs.to_vec();
        this.deeds = deeds;
        let looker = LOOKER.fetch_add(1, Ordering::Relaxed) + 1;
        if spotted.is_empty() {
            return;
        }
        let (send, used) = async_channel::bounded(4);
        let corners = Arc::new(spotted);
        let started = thread::Builder::new()
            .name("tessera hot corners".to_owned())
            .spawn(move || look(looker, &corners, &send));
        if let Err(error) = started {
            report!("the hot corners are not used: {error}");
            return;
        }
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(corner) = used.recv().await {
                let done = cx.update(|cx| {
                    let deed = cx.global::<Self>().deeds.get(corner).cloned();
                    if let Some(deed) = deed {
                        deed.done(cx);
                    }
                });
                if done.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}

/// Looks at the pointer until another thread is started in place of this
/// one, the `looker`th, and sends the place of each of `corners` that is
/// used.
fn look(looker: u64, corners: &[Spotted], send: &async_channel::Sender<usize>) {
    let mut trigger = Trigger::default();
    let mut frames: Vec<ScreenRect> = Vec::new();
    let mut read = None::<Instant>;
    let mut pause = FAR_LOOK;
    while LOOKER.load(Ordering::Relaxed) == looker {
        thread::sleep(pause);
        if read.is_none_or(|read| read.elapsed() >= DISPLAYS_LOOK) {
            frames = Display::all()
                .into_iter()
                .map(|display| display.frame)
                .collect();
            read = Some(Instant::now());
        }
        let Some(point) = pointer_at() else {
            continue;
        };
        let near = corners.iter().any(|corner| {
            let displays = frames.iter().enumerate();
            { displays }
                .filter(|&(index, _)| corner.display.is_none_or(|only| only == index))
                .any(|(_, &frame)| nearby(frame, corner.at, point))
        });
        pause = if near { NEAR_LOOK } else { FAR_LOOK };
        if !near || pointer_held() {
            trigger.reset();
            continue;
        }
        let found = hit(point, &frames);
        let corner = found.and_then(|(display, spot)| chosen(corners, display, spot));
        let delay = |corner: usize| corners[corner].delay;
        let Some(used) = trigger.see(point, corner, delay, Instant::now()) else {
            continue;
        };
        let filled = found.is_some_and(|(display, _)| front_fills(frames[display]));
        if !filled && send.try_send(used).is_err() && send.is_closed() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    #[test]
    fn a_corner_is_told_before_its_edges() {
        assert_eq!(spot_in(SCREEN, (0, 0)), Some(Spot::TopLeft));
        assert_eq!(spot_in(SCREEN, (1919, 2)), Some(Spot::TopRight));
        assert_eq!(spot_in(SCREEN, (3, 1079)), Some(Spot::BottomLeft));
        assert_eq!(spot_in(SCREEN, (1919, 1079)), Some(Spot::BottomRight));
        assert_eq!(spot_in(SCREEN, (900, 0)), Some(Spot::Top));
        assert_eq!(spot_in(SCREEN, (900, 1079)), Some(Spot::Bottom));
        assert_eq!(spot_in(SCREEN, (0, 500)), Some(Spot::Left));
        assert_eq!(spot_in(SCREEN, (1919, 500)), Some(Spot::Right));
        assert_eq!(spot_in(SCREEN, (900, 500)), None);
        let right = ScreenRect {
            left: 1920,
            right: 3840,
            ..SCREEN
        };
        assert_eq!(hit((1920, 0), &[SCREEN, right]), Some((1, Spot::TopLeft)));
        assert_eq!(hit((1919, 0), &[SCREEN, right]), Some((0, Spot::TopRight)));
    }

    #[test]
    fn a_corner_of_its_own_comes_before_an_edge() {
        let corner = |at, display| Spotted {
            at,
            display,
            delay: Duration::ZERO,
        };
        let corners = [
            corner(Spot::Top, None),
            corner(Spot::TopLeft, Some(1)),
            corner(Spot::BottomRight, None),
        ];
        assert_eq!(chosen(&corners, 0, Spot::TopLeft), Some(0));
        assert_eq!(chosen(&corners, 1, Spot::TopLeft), Some(1));
        assert_eq!(chosen(&corners, 0, Spot::Top), Some(0));
        assert_eq!(chosen(&corners, 0, Spot::BottomRight), Some(2));
        assert_eq!(chosen(&corners, 0, Spot::Left), None);
        assert!(Spot::Left.covers(Spot::BottomLeft));
        assert!(!Spot::Left.covers(Spot::TopRight));
    }

    #[test]
    fn a_corner_waits_and_is_used_again_once_the_pointer_went_away() {
        let mut trigger = Trigger::default();
        let start = Instant::now();
        let delay = |_| Duration::from_millis(150);
        let at = |millis| start + Duration::from_millis(millis);
        assert_eq!(trigger.see((0, 0), Some(0), delay, at(0)), None);
        assert_eq!(trigger.see((0, 0), Some(0), delay, at(100)), None);
        assert_eq!(trigger.see((0, 0), Some(0), delay, at(160)), Some(0));
        // Resting on does not use it again.
        assert_eq!(trigger.see((1, 1), Some(0), delay, at(400)), None);
        assert_eq!(trigger.see((10, 10), None, delay, at(450)), None);
        // Far enough away, it can be used again.
        assert_eq!(trigger.see((100, 100), None, delay, at(500)), None);
        assert_eq!(trigger.see((0, 0), Some(0), delay, at(600)), None);
        assert_eq!(trigger.see((0, 0), Some(0), delay, at(760)), Some(0));
        // Leaving before the wait is over uses nothing.
        let mut trigger = Trigger::default();
        assert_eq!(trigger.see((0, 0), Some(0), delay, at(0)), None);
        assert_eq!(trigger.see((300, 300), None, delay, at(100)), None);
        assert_eq!(trigger.see((0, 0), Some(0), delay, at(200)), None);
        assert_eq!(trigger.see((0, 0), Some(0), delay, at(300)), None);
    }

    #[test]
    fn a_corner_does_one_thing() {
        let actions = Actions::default();
        let corner = |text: &str| toml::from_str::<CornerConfig>(text).unwrap();
        let popup = corner("at = \"top-left\"\npopup = \"launcher\"");
        assert_eq!(popup.deed(&actions), Ok(Deed::Popup("launcher".to_owned())));
        assert!(corner("at = \"top\"").deed(&actions).is_err());
        let both = corner("at = \"top\"\npopup = \"a\"\nrun = [\"b.exe\"]");
        assert!(both.deed(&actions).is_err());
        assert!(
            corner("at = \"top\"\naction = \"none\"")
                .deed(&actions)
                .is_err()
        );
        assert!(
            corner("at = \"top\"\narg = \"x\"\npopup = \"a\"")
                .deed(&actions)
                .is_err()
        );
        assert_eq!(
            corner("at = \"left\"\nrun = [\"a.exe\"]\ndelay = 0").delay(),
            Duration::ZERO
        );
        assert!(toml::from_str::<CornerConfig>("at = \"middle\"\npopup = \"a\"").is_err());
    }

    #[test]
    fn the_pointer_is_watched_closely_only_near_a_corner_in_use() {
        assert!(nearby(SCREEN, Spot::TopLeft, (10, 20)));
        assert!(!nearby(SCREEN, Spot::TopLeft, (10, 500)));
        assert!(nearby(SCREEN, Spot::Bottom, (900, 1060)));
        assert!(!nearby(SCREEN, Spot::Bottom, (900, 500)));
        assert!(!nearby(SCREEN, Spot::Right, (5000, 500)));
    }
}
