// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A wallpaper of Tessera's own: pictures shown behind the icons of the
//! desktop on every display, one fading into the next.
//!
//! The wallpaper of Windows is left as it is and shows again when Tessera
//! exits, because the pictures are drawn in windows of Tessera that sit
//! over it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use gpui::{
    App, AppContext, AsyncApp, Context, Global, IntoElement, ObjectFit, ParentElement,
    PlatformDisplay, Render, RenderImage, Styled, StyledImage, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, black, div,
    img,
};
use image::imageops::FilterType;
use serde::Deserialize;

use crate::motion::Spring;
use crate::platform::NativeSurface;
use crate::theme::Theme;

/// How long a picture stays unless the configuration says, in seconds.
const INTERVAL: u64 = 600;

/// The shortest time a picture stays, in seconds: reading the next one
/// takes time and memory.
const SHORTEST_INTERVAL: u64 = 5;

/// How long after one display the next one goes over to a new picture.
const STAGGER: Duration = Duration::from_millis(400);

/// The kinds of file taken from a directory, by extension.
const KINDS: [&str; 3] = ["jpg", "jpeg", "png"];

/// What the configuration says about the wallpaper.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WallpaperConfig {
    /// Picture files, and directories whose picture files are taken, in
    /// the order of their names. Without any, Tessera draws no wallpaper.
    #[serde(default)]
    images: Vec<PathBuf>,
    /// How many seconds a picture stays before the next one.
    interval: Option<u64>,
}

impl WallpaperConfig {
    fn interval(&self) -> Duration {
        let seconds = self.interval.unwrap_or(INTERVAL);
        if seconds < SHORTEST_INTERVAL {
            report!(
                "`wallpaper.interval` is under {SHORTEST_INTERVAL} seconds, which it is taken as"
            );
        }
        Duration::from_secs(seconds.max(SHORTEST_INTERVAL))
    }

    /// The picture files the configuration names, those of a directory in
    /// the order of their names. What cannot be read is reported.
    fn pictures(&self) -> Vec<PathBuf> {
        let mut pictures = Vec::new();
        for path in &self.images {
            if !path.is_dir() {
                pictures.push(path.clone());
                continue;
            }
            match std::fs::read_dir(path) {
                Ok(entries) => {
                    let mut found: Vec<PathBuf> = entries
                        .filter_map(|entry| Some(entry.ok()?.path()))
                        .filter(|path| is_picture(path))
                        .collect();
                    found.sort();
                    pictures.extend(found);
                }
                Err(error) => report!(
                    "the wallpaper directory {} cannot be read: {error}",
                    path.display()
                ),
            }
        }
        pictures
    }
}

/// Whether the name of `path` ends as a picture file's does.
fn is_picture(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        KINDS
            .iter()
            .any(|kind| extension.eq_ignore_ascii_case(kind))
    })
}

/// The picture in the file `path`, ready to draw and scaled down to the
/// smallest size that still covers `width` by `height` pixels.
fn picture(path: &Path, width: u32, height: u32) -> anyhow::Result<Arc<RenderImage>> {
    let mut image =
        image::open(path).with_context(|| format!("{} cannot be read", path.display()))?;
    let (wide, high) = covering(image.width(), image.height(), width, height);
    if wide < image.width() {
        image = image.resize_exact(wide, high, FilterType::Triangle);
    }
    let mut pixels = image.into_rgba8();
    // The renderer expects blue first.
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}

/// The size a picture of `width` by `height` pixels is scaled to so that
/// it just covers `over_width` by `over_height`, keeping its shape. A
/// picture that is not larger than that keeps its size.
fn covering(width: u32, height: u32, over_width: u32, over_height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (width, height);
    }
    let scale = f64::max(
        f64::from(over_width) / f64::from(width),
        f64::from(over_height) / f64::from(height),
    );
    if scale >= 1. {
        return (width, height);
    }
    let scaled = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
    (scaled(width), scaled(height))
}

/// The wallpaper windows and the pictures they go through.
pub struct Wallpaper {
    config: WallpaperConfig,
    windows: Vec<WindowHandle<Backdrop>>,
    /// Counts configurations, so that the task of an earlier one stops.
    task: usize,
}

impl Global for Wallpaper {}

impl Wallpaper {
    /// Draws the wallpaper `config` asks for, if it asks for one.
    pub fn start(config: &WallpaperConfig, cx: &mut App) {
        cx.set_global(Self {
            config: WallpaperConfig::default(),
            windows: Vec::new(),
            task: 0,
        });
        Self::configure(config, cx);
    }

    /// Takes over a configuration that was read again, if it says
    /// something else about the wallpaper than the one before.
    pub fn configure(config: &WallpaperConfig, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if this.config == *config {
            return;
        }
        this.config = config.clone();
        this.task += 1;
        let task = this.task;
        let pictures = config.pictures();
        if pictures.is_empty() {
            Self::close(cx);
            return;
        }
        if cx.global::<Self>().windows.is_empty() {
            let windows = cx
                .displays()
                .iter()
                .filter_map(|display| {
                    Backdrop::open(display, cx)
                        .inspect_err(|error| report!("a display gets no wallpaper: {error:#}"))
                        .ok()
                })
                .collect();
            cx.global_mut::<Self>().windows = windows;
        }
        let interval = config.interval();
        cx.spawn(async move |cx: &mut AsyncApp| {
            for path in pictures.iter().cycle() {
                // One picture for all displays, large enough for each.
                let Ok(Some((width, height))) = cx.update(|cx| Self::largest(task, cx)) else {
                    return;
                };
                let path = path.clone();
                let read = cx
                    .background_executor()
                    .spawn(async move { picture(&path, width, height) })
                    .await;
                match read {
                    Ok(picture) => {
                        let Ok(Some(windows)) = cx.update(|cx| Self::windows(task, cx)) else {
                            return;
                        };
                        // One display after the other: the renderer takes
                        // a copy of the picture for each window, and for a
                        // while the memory of another to make it.
                        for window in windows {
                            window
                                .update(cx, |backdrop, window, cx| {
                                    backdrop.show(picture.clone(), window, cx);
                                })
                                .ok();
                            cx.background_executor().timer(STAGGER).await;
                        }
                    }
                    Err(error) => report!("a wallpaper picture is left out: {error:#}"),
                }
                if pictures.len() == 1 {
                    return;
                }
                cx.background_executor().timer(interval).await;
            }
        })
        .detach();
    }

    /// The width and the height in pixels of the widest and the highest
    /// wallpaper window, unless `task` is not the current one or no window
    /// is left.
    fn largest(task: usize, cx: &mut App) -> Option<(u32, u32)> {
        let this = cx.global::<Self>();
        if this.task != task {
            return None;
        }
        let sizes: Vec<(u32, u32)> = this
            .windows
            .clone()
            .into_iter()
            .filter_map(|window| {
                window
                    .update(cx, |_, window, _| {
                        let size = window.viewport_size();
                        let scale = window.scale_factor();
                        let pixels = |side: gpui::Pixels| (f32::from(side) * scale).ceil() as u32;
                        (pixels(size.width), pixels(size.height))
                    })
                    .ok()
            })
            .collect();
        let width = sizes.iter().map(|size| size.0).max()?;
        let height = sizes.iter().map(|size| size.1).max()?;
        Some((width, height))
    }

    /// The wallpaper windows, unless `task` is not the current one.
    fn windows(task: usize, cx: &App) -> Option<Vec<WindowHandle<Backdrop>>> {
        let this = cx.global::<Self>();
        (this.task == task).then(|| this.windows.clone())
    }

    fn close(cx: &mut App) {
        for window in std::mem::take(&mut cx.global_mut::<Self>().windows) {
            window
                .update(cx, |backdrop, window, cx| {
                    backdrop.forget(window, cx);
                    window.remove_window();
                })
                .ok();
        }
    }
}

/// The wallpaper of one display: the picture shown and, while that one is
/// still fading in, the one before it underneath.
struct Backdrop {
    shown: Option<Arc<RenderImage>>,
    before: Option<Arc<RenderImage>>,
    /// 0 when the picture shown starts to appear, 1 once it is all there.
    presence: Spring,
}

impl Backdrop {
    /// Opens the wallpaper window of `display`, empty and behind the icons
    /// of the desktop.
    fn open(
        display: &std::rc::Rc<dyn PlatformDisplay>,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<Self>> {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(display.bounds())),
            // As for an edge surface: keeps GPUI from insetting the content.
            titlebar: Some(TitlebarOptions::default()),
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            display_id: Some(display.id()),
            window_background: WindowBackgroundAppearance::Opaque,
            ..Default::default()
        };
        let mut placed = Ok(());
        let window = cx.open_window(options, |window, cx| {
            placed = NativeSurface::of(window).and_then(NativeSurface::put_behind_desktop_icons);
            let effects = cx.global::<Theme>().motion.effects;
            cx.new(|_| Self {
                shown: None,
                before: None,
                presence: Spring::new(effects, 1.),
            })
        })?;
        if let Err(error) = placed {
            // Left where it opened, the window would cover the display.
            window
                .update(cx, |_, window, _| window.remove_window())
                .ok();
            return Err(error);
        }
        Ok(window)
    }

    /// Fades over to `picture` from the picture shown.
    fn show(&mut self, picture: Arc<RenderImage>, window: &mut Window, cx: &mut Context<Self>) {
        self.settle(window, cx);
        self.before = self.shown.replace(picture);
        let effects = cx.global::<Theme>().motion.effects;
        self.presence = Spring::new(effects, 0.);
        self.presence.animate_to(1.);
        cx.notify();
    }

    /// Lets go of the picture underneath, which the renderer keeps a copy
    /// of until told.
    fn settle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(before) = self.before.take() {
            cx.drop_image(before, Some(window));
        }
    }

    /// Lets go of every picture.
    fn forget(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settle(window, cx);
        if let Some(shown) = self.shown.take() {
            cx.drop_image(shown, Some(window));
        }
    }
}

impl Render for Backdrop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let presence = self.presence.value(window).clamp(0., 1.);
        if self.presence.is_settled() && self.before.is_some() {
            self.settle(window, cx);
        }
        let layer = |picture: &Arc<RenderImage>| {
            img(picture.clone())
                .absolute()
                .size_full()
                .object_fit(ObjectFit::Cover)
        };
        div()
            .size_full()
            .relative()
            .bg(black())
            .children(self.before.as_ref().map(layer))
            .children(
                self.shown
                    .as_ref()
                    .map(|picture| layer(picture).opacity(presence)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_larger_picture_is_scaled_to_just_cover() {
        // Wider than the display in shape: the height decides.
        assert_eq!(covering(8000, 2000, 2000, 1000), (4000, 1000));
        // Higher in shape: the width decides.
        assert_eq!(covering(4000, 6000, 2000, 1000), (2000, 3000));
    }

    #[test]
    fn a_picture_that_is_not_larger_keeps_its_size() {
        assert_eq!(covering(1000, 500, 2000, 1000), (1000, 500));
        assert_eq!(covering(4000, 500, 2000, 1000), (4000, 500));
        assert_eq!(covering(0, 500, 2000, 1000), (0, 500));
    }

    #[test]
    fn pictures_are_known_by_their_extension_in_any_case() {
        assert!(is_picture(Path::new(r"C:\Pictures\sea.JPG")));
        assert!(is_picture(Path::new("hill.png")));
        assert!(!is_picture(Path::new("notes.txt")));
        assert!(!is_picture(Path::new("png")));
    }

    #[test]
    fn a_directory_gives_its_pictures_in_the_order_of_their_names() {
        let directory =
            std::env::temp_dir().join(format!("tessera-wallpaper-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        for name in ["b.png", "a.jpg", "c.txt"] {
            std::fs::write(directory.join(name), []).unwrap();
        }
        let single = PathBuf::from(r"C:\elsewhere\one.png");
        let config = WallpaperConfig {
            images: vec![single.clone(), directory.clone()],
            interval: None,
        };
        let pictures = config.pictures();
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(
            pictures,
            [single, directory.join("a.jpg"), directory.join("b.png")]
        );
    }
}
