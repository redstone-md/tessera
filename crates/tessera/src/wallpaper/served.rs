// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The wallpaper as Explorer answers for it, in shell mode: programs that
//! ask COM for `IDesktopWallpaper` get Tessera's server
//! ([`WallpaperServer`]), what they set is written to the configuration
//! ([`edits`]) and taken over at once, and what they read is what Tessera
//! shows ([`Served::shown`]).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use gpui::{App, AsyncApp};

use super::compose::Fit;
use super::edit::{self, Edit, paths_value};
use super::looks::{Plan, WindowsNow};
use super::{SHORTEST_INTERVAL, Wallpaper, WallpaperConfig, numbers};
use crate::platform::{
    ScreenRect, WallpaperAnswers, WallpaperRequest, WallpaperServer, shell_mode,
};
use crate::shell::Shell;

/// The server, and what it answers.
pub struct Served {
    answers: Arc<Mutex<WallpaperAnswers>>,
    _server: WallpaperServer,
}

impl Served {
    /// Starts the server in shell mode, away from the thread of the
    /// interface, and takes over what programs ask of it from then on.
    pub fn start(config: &WallpaperConfig, cx: &mut App) {
        if !shell_mode() {
            return;
        }
        let answers = Arc::new(Mutex::new(WallpaperAnswers {
            enabled: true,
            ..WallpaperAnswers::default()
        }));
        Self::configure_answers(&answers, config);
        let (requests, asked) = async_channel::unbounded();
        cx.spawn(async move |cx: &mut AsyncApp| {
            let kept = answers.clone();
            let server = cx
                .background_executor()
                .spawn(async move { WallpaperServer::start(kept, requests) })
                .await;
            let server = match server {
                Ok(server) => server,
                Err(error) => {
                    report!("programs cannot set the wallpaper through Tessera: {error:#}");
                    return;
                }
            };
            let taken = cx.update(|cx| {
                cx.global_mut::<Wallpaper>().served = Some(Self {
                    answers,
                    _server: server,
                });
            });
            if taken.is_err() {
                return;
            }
            while let Ok(request) = asked.recv().await {
                if Self::take(request, cx).await.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Does what a program asked: goes on to another picture, or writes
    /// the change to the configuration, off the thread of the interface,
    /// and has the shell take it over.
    async fn take(request: WallpaperRequest, cx: &mut AsyncApp) -> anyhow::Result<()> {
        if let WallpaperRequest::Advance { back } = request {
            return cx.update(|cx| Wallpaper::go_on(if back { -1 } else { 1 }, cx));
        }
        let frames = cx.update(|cx| cx.global::<Wallpaper>().frames.clone())?;
        let written = cx
            .background_executor()
            .spawn(async move { edit::write(edits(&request, &frames)) })
            .await;
        match written {
            Ok(_) => cx.update(Shell::config_changed),
            Err(error) => {
                report!("the wallpaper a program set is not kept: {error}");
                Ok(())
            }
        }
    }

    /// Has the server answer what the configuration says of pictures that
    /// take turns.
    pub fn configure(config: &WallpaperConfig, cx: &App) {
        if let Some(served) = &cx.global::<Wallpaper>().served {
            Self::configure_answers(&served.answers, config);
        }
    }

    fn configure_answers(answers: &Mutex<WallpaperAnswers>, config: &WallpaperConfig) {
        let (slideshow, shuffle, interval) = config.slideshow();
        let mut answers = answers.lock().unwrap_or_else(PoisonError::into_inner);
        answers.turning = slideshow.len() > 1;
        answers.slideshow = slideshow;
        answers.shuffle = shuffle;
        answers.interval_ms = u32::try_from(interval.as_millis()).unwrap_or(u32::MAX);
        answers.enabled = !config.shows_color_alone();
    }

    /// Has the server answer that the displays at `frames` show what
    /// `plans` say, the wallpaper of Windows being `windows` where it was
    /// read.
    pub fn shown(
        frames: &[ScreenRect],
        plans: &[Plan],
        windows: Option<&WindowsNow>,
        cx: &mut AsyncApp,
    ) -> anyhow::Result<()> {
        let shown = answers_of(frames, plans, windows);
        cx.update(|cx| {
            if let Some(served) = &cx.global::<Wallpaper>().served {
                let mut answers = served
                    .answers
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                answers.displays = shown.displays;
                answers.position = shown.position;
                answers.color = shown.color;
            }
        })
    }
}

/// The displays with their pictures, the position and the color that
/// `plans` show on the displays at `frames`.
fn answers_of(
    frames: &[ScreenRect],
    plans: &[Plan],
    windows: Option<&WindowsNow>,
) -> WallpaperAnswers {
    let mut answers = WallpaperAnswers::default();
    let mut first = true;
    for (frame, plan) in frames.iter().zip(plans) {
        let picture = match plan {
            Plan::Picture {
                file, fit, ground, ..
            } => {
                if first {
                    answers.position = fit.to_windows();
                    answers.color = ground.middle();
                    first = false;
                }
                Some(file.clone())
            }
            Plan::Fill(ground) => {
                if first {
                    answers.color = ground.middle();
                    first = false;
                }
                None
            }
            // What the desktop window draws: the wallpaper of Windows.
            Plan::Nothing => windows.and_then(|now| {
                if first {
                    answers.position = now.wallpaper.position;
                    answers.color = now.wallpaper.color;
                    first = false;
                }
                now.wallpaper.picture_of(*frame).map(PathBuf::from)
            }),
            Plan::Video(_) => None,
        };
        answers.displays.push((*frame, picture));
    }
    answers
}

/// What is written to the configuration for `request`, the displays
/// being at `frames`: a picture for one display is written to its entry,
/// a picture or slideshow for all to the wallpaper, in place of the
/// pictures, videos and collection shown before anywhere, so that it is
/// what is seen.
pub fn edits(request: &WallpaperRequest, frames: &[ScreenRect]) -> Vec<Edit> {
    let files = || Edit::Key("source", Some("files".into()));
    match request {
        WallpaperRequest::Picture {
            display: Some(display),
            file,
        } => {
            let Some(at) = frames.iter().position(|frame| frame == display) else {
                return Vec::new();
            };
            let number = numbers(frames)[at];
            let mut wanted = vec![
                Edit::Display(number, "collection", None),
                Edit::Display(number, "video", None),
                Edit::Display(number, "videos", None),
            ];
            wanted.push(match file {
                Some(file) => Edit::Display(
                    number,
                    "images",
                    Some(paths_value(std::slice::from_ref(file))),
                ),
                None => Edit::Display(number, "images", None),
            });
            wanted
        }
        WallpaperRequest::Picture {
            display: None,
            file,
        } => {
            let mut wanted = replace_shown();
            match file {
                Some(file) => wanted.extend([
                    files(),
                    Edit::Key("images", Some(paths_value(std::slice::from_ref(file)))),
                ]),
                None => wanted.push(Edit::Key("source", Some("color".into()))),
            }
            wanted
        }
        WallpaperRequest::Position(position) => {
            let fit = Fit::of_windows(*position);
            vec![
                Edit::Key("fit", Some(fit.name().into())),
                Edit::EveryPlace("fit"),
            ]
        }
        // The color alone would have the wallpaper be the color where
        // nothing else names what it is.
        WallpaperRequest::Color([red, green, blue]) => vec![
            Edit::KeepSource,
            Edit::Key("gradient", None),
            Edit::Key(
                "color",
                Some(format!("#{red:02x}{green:02x}{blue:02x}").into()),
            ),
        ],
        WallpaperRequest::Slideshow(paths) => {
            let mut wanted = replace_shown();
            wanted.extend([files(), Edit::Key("images", Some(paths_value(paths)))]);
            wanted
        }
        WallpaperRequest::SlideshowOptions {
            shuffle,
            interval_ms,
        } => {
            let seconds = (u64::from(*interval_ms) / 1000).max(SHORTEST_INTERVAL);
            vec![
                Edit::Key("shuffle", Some((*shuffle).into())),
                Edit::Key(
                    "interval",
                    Some(i64::try_from(seconds).unwrap_or(i64::MAX).into()),
                ),
            ]
        }
        WallpaperRequest::Enable(true) => vec![Edit::Key("source", None)],
        WallpaperRequest::Enable(false) => vec![Edit::Key("source", Some("color".into()))],
        WallpaperRequest::Advance { .. } => Vec::new(),
    }
}

/// Takes away the pictures, videos and collection of the wallpaper, of
/// every display and of every workspace, so that what is written then is
/// shown everywhere.
fn replace_shown() -> Vec<Edit> {
    let shown = ["collection", "video", "videos", "images"];
    let mut wanted: Vec<Edit> = { shown.into_iter() }
        .flat_map(|key| [Edit::Key(key, None), Edit::EveryPlace(key)])
        .collect();
    wanted.extend([
        Edit::Key("images_light", None),
        Edit::Key("images_dark", None),
    ]);
    wanted
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml_edit::Value;

    use crate::platform::WallpaperPosition;

    fn frame(left: i32) -> ScreenRect {
        ScreenRect {
            left,
            top: 0,
            right: left + 100,
            bottom: 100,
        }
    }

    fn texts(edits: &[Edit]) -> Vec<String> {
        { edits.iter() }
            .map(|edit| match edit {
                Edit::Key(key, value) => format!("{key}={}", shown(value)),
                Edit::Display(number, key, value) => format!("{number}.{key}={}", shown(value)),
                Edit::EveryPlace(key) => format!("*.{key}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    fn shown(value: &Option<Value>) -> String {
        value
            .as_ref()
            .map_or("-".to_owned(), |value| value.to_string().trim().to_owned())
    }

    #[test]
    fn a_picture_of_one_display_goes_to_its_entry() {
        let frames = [frame(100), frame(0)];
        let request = WallpaperRequest::Picture {
            display: Some(frame(100)),
            file: Some(PathBuf::from("sea.jpg")),
        };
        assert_eq!(
            texts(&edits(&request, &frames)),
            [
                "2.collection=-",
                "2.video=-",
                "2.videos=-",
                "2.images=[\"sea.jpg\"]"
            ]
        );
        let elsewhere = WallpaperRequest::Picture {
            display: Some(frame(500)),
            file: None,
        };
        assert!(edits(&elsewhere, &frames).is_empty());
    }

    #[test]
    fn a_picture_of_all_displays_replaces_what_they_showed() {
        let request = WallpaperRequest::Picture {
            display: None,
            file: Some(PathBuf::from("hill.png")),
        };
        let slideshow = WallpaperRequest::Slideshow(vec![PathBuf::from("hill.png")]);
        for request in [request, slideshow] {
            let wanted = texts(&edits(&request, &[frame(0)]));
            for key in ["collection", "video", "videos", "images"] {
                assert!(wanted.contains(&format!("{key}=-")));
                assert!(wanted.contains(&format!("*.{key}")));
            }
            assert!(wanted.contains(&"images_light=-".to_owned()));
            assert!(wanted.contains(&"images_dark=-".to_owned()));
            assert!(wanted.ends_with(&[
                "source=\"files\"".to_owned(),
                "images=[\"hill.png\"]".to_owned()
            ]));
        }
        let none = WallpaperRequest::Picture {
            display: None,
            file: None,
        };
        assert!(texts(&edits(&none, &[])).contains(&"source=\"color\"".to_owned()));
    }

    #[test]
    fn options_of_the_slideshow_keep_the_shortest_interval() {
        let request = WallpaperRequest::SlideshowOptions {
            shuffle: true,
            interval_ms: 1200,
        };
        assert_eq!(texts(&edits(&request, &[])), ["shuffle=true", "interval=5"]);
        let color = WallpaperRequest::Color([16, 32, 255]);
        assert_eq!(
            texts(&edits(&color, &[])),
            ["KeepSource", "gradient=-", "color=\"#1020ff\""]
        );
        let position = WallpaperRequest::Position(WallpaperPosition::Tile);
        assert_eq!(texts(&edits(&position, &[])), ["fit=\"tile\"", "*.fit"]);
        assert_eq!(Fit::Tile.to_windows(), WallpaperPosition::Tile);
    }

    #[test]
    fn the_answers_tell_what_each_display_shows() {
        let plans = [
            Plan::Picture {
                file: PathBuf::from("a.jpg"),
                fit: Fit::Center,
                ground: super::super::compose::Ground::Solid([1, 2, 3]),
                written: None,
                own: true,
                adjust: super::super::adjust::Adjustments::default(),
            },
            Plan::Nothing,
        ];
        let answers = answers_of(&[frame(0), frame(100)], &plans, None);
        assert_eq!(
            answers.displays,
            [(frame(0), Some(PathBuf::from("a.jpg"))), (frame(100), None)]
        );
        assert_eq!(answers.position, WallpaperPosition::Center);
        assert_eq!(answers.color, [1, 2, 3]);
    }
}
