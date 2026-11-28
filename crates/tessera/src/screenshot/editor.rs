// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The editor of a screenshot: a window with the picture, to draw on it
//! and to send it on.
//!
//! The tools draw a rectangle, an ellipse, an arrow, a line, a stroke of
//! the pen or of the highlighter, a text and numbered stamps, in one of
//! eight colours and three widths; these stay marks over the picture until
//! it leaves the editor. Blur and pixelate change the pixels of the part
//! dragged over, with the image crate, so that what was there cannot be
//! read again; crop keeps only the part dragged over, with what was drawn
//! on it. Every change can be undone and done again. The picture is shown
//! to fit the window, or larger or smaller with the buttons of zoom or
//! Ctrl and the wheel.
//!
//! From the editor the picture is copied (Ctrl+C), saved into the folder
//! of screenshots (Ctrl+S), saved as a file chosen in the dialog of
//! Windows (PNG, JPEG or WebP by its extension), opened with a program
//! Windows offers, shared, or dragged out as a file: the editor keeps a
//! file of what it shows in the folder of temporary files for the last
//! three, written away from the user interface after each change.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use chrono::{DateTime, Local};
use gpui::{
    AnyElement, App, AppContext, AsyncApp, Bounds, Context, CursorStyle, Entity, FocusHandle,
    FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement, PathBuilder, Pixels, Point, Render, RenderImage,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, TitlebarOptions, Window,
    WindowBounds, WindowKind, WindowOptions, canvas, div, img, point, px, rgba, size,
};
use image::RgbaImage;

use super::marks::{self, Area, Color, HIGHLIGHT, Mark, Spot};
use super::{ICON_SCREENSHOT, Screenshots, Shot, away, render_image};
use crate::field::TextField;
use crate::files::Files;
use crate::i18n::Language;
use crate::platform::{DragOut, choose_app, encode_picture, render_text};
use crate::theme::Theme;
use crate::ui::icon;

/// The colours offered, as red, green, blue and alpha.
const COLORS: [Color; 8] = [
    [229, 57, 53, 255],
    [251, 140, 0, 255],
    [253, 216, 53, 255],
    [67, 160, 71, 255],
    [30, 136, 229, 255],
    [142, 36, 170, 255],
    [0, 0, 0, 255],
    [255, 255, 255, 255],
];

/// The widths of lines offered, in pixels of the picture, and the sizes of
/// texts and stamps that go with each.
const WIDTHS: [f32; 3] = [3., 6., 12.];
const TEXT_SIZES: [f32; 3] = [20., 32., 52.];
const STAMP_SIZES: [f32; 3] = [28., 40., 60.];

/// How strongly blur blurs, and how large the squares of pixelate are, in
/// pixels of the picture, for each width.
const BLUR: [f32; 3] = [6., 12., 24.];
const SQUARES: [u32; 3] = [8, 14, 24];

/// The steps of zoom, as parts of the size of the picture on the display.
const ZOOMS: [f32; 9] = [0.125, 0.25, 0.5, 0.75, 1., 1.5, 2., 4., 8.];

/// How many files of what the editor shows are kept for dragging out.
const EXPORTS_KEPT: usize = 3;

/// The height of the bars above and below the picture.
const BAR: f32 = 44.;

const ICON_RECTANGLE: &str = "\u{e739}";
const ICON_ELLIPSE: &str = "\u{ea3a}";
const ICON_ARROW: &str = "\u{e72a}";
const ICON_LINE: &str = "\u{e738}";
const ICON_PEN: &str = "\u{e70f}";
const ICON_HIGHLIGHTER: &str = "\u{e7e6}";
const ICON_TEXT: &str = "\u{e8d2}";
const ICON_STAMP: &str = "\u{ea3b}";
const ICON_BLUR: &str = "\u{e81e}";
const ICON_PIXELATE: &str = "\u{e80a}";
const ICON_CROP: &str = "\u{e7a8}";
const ICON_UNDO: &str = "\u{e7a7}";
const ICON_REDO: &str = "\u{e7a6}";
const ICON_ZOOM_IN: &str = "\u{e8a3}";
const ICON_ZOOM_OUT: &str = "\u{e71f}";
const ICON_FIT: &str = "\u{e740}";
const ICON_DRAG: &str = "\u{e7c2}";

/// A tool of the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Rectangle,
    Ellipse,
    Arrow,
    Line,
    Pen,
    Highlighter,
    Text,
    Stamp,
    Blur,
    Pixelate,
    Crop,
}

impl Tool {
    const ALL: [Self; 11] = [
        Self::Rectangle,
        Self::Ellipse,
        Self::Arrow,
        Self::Line,
        Self::Pen,
        Self::Highlighter,
        Self::Text,
        Self::Stamp,
        Self::Blur,
        Self::Pixelate,
        Self::Crop,
    ];

    fn glyph(self) -> &'static str {
        match self {
            Self::Rectangle => ICON_RECTANGLE,
            Self::Ellipse => ICON_ELLIPSE,
            Self::Arrow => ICON_ARROW,
            Self::Line => ICON_LINE,
            Self::Pen => ICON_PEN,
            Self::Highlighter => ICON_HIGHLIGHTER,
            Self::Text => ICON_TEXT,
            Self::Stamp => ICON_STAMP,
            Self::Blur => ICON_BLUR,
            Self::Pixelate => ICON_PIXELATE,
            Self::Crop => ICON_CROP,
        }
    }

    /// Its English name, a text of the RUSSIAN table.
    fn title(self) -> &'static str {
        match self {
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Arrow => "Arrow",
            Self::Line => "Line",
            Self::Pen => "Pen",
            Self::Highlighter => "Highlighter",
            Self::Text => "Text",
            Self::Stamp => "Numbered stamp",
            Self::Blur => "Blur",
            Self::Pixelate => "Pixelate",
            Self::Crop => "Crop",
        }
    }

    /// Whether it draws a rectangle of the picture to change, rather than
    /// a mark.
    fn takes_area(self) -> bool {
        matches!(self, Self::Blur | Self::Pixelate | Self::Crop)
    }
}

/// The picture and what is drawn on it, as one step of undo.
#[derive(Clone)]
struct Doc {
    picture: Arc<RgbaImage>,
    /// The picture as GPUI draws it.
    shown: Arc<RenderImage>,
    marks: Vec<Mark>,
}

/// What a press of the left button is drawing.
#[derive(Clone, Debug)]
enum Drawing {
    /// A shape, or a rectangle to change, from `from` to `to`.
    Span { from: Spot, to: Spot },
    /// A stroke through its points.
    Stroke(Vec<Spot>),
}

/// A file of what the editor shows, for dragging out, and the version it
/// shows.
struct Export {
    version: u64,
    path: PathBuf,
}

/// The editor of one screenshot.
pub struct Editor {
    doc: Doc,
    undone: Vec<Doc>,
    redone: Vec<Doc>,
    tool: Tool,
    color: Color,
    /// Which of the widths is chosen.
    width: usize,
    /// The zoom, as a part of the size of the picture on the display; fit
    /// to the window without one.
    zoom: Option<f32>,
    drawing: Option<Drawing>,
    /// A text being typed and where it goes.
    typing: Option<(Spot, Entity<TextField>)>,
    taken: DateTime<Local>,
    /// Where the picture was drawn last, in the window.
    drawn: Rc<Cell<Bounds<Pixels>>>,
    /// What the bar at the bottom says.
    status: Option<SharedString>,
    /// The tool under the pointer, which the bar at the bottom names.
    hovered: Option<Tool>,
    /// Whether a change of the pixels is being worked on.
    busy: bool,
    /// Counts the changes, for the files kept for dragging out.
    version: u64,
    exports: Vec<Export>,
    focus: FocusHandle,
    focused: bool,
}

impl Editor {
    /// Opens the editor with `shot`.
    pub fn open(shot: Shot, cx: &mut App) {
        let pixels = shot.pixels.clone();
        let shown = away("screenshot editor", move || render_image(&pixels));
        cx.spawn(async move |cx: &mut AsyncApp| {
            let Ok(shown) = shown.recv().await else {
                return report!("the editor of the screenshot did not open");
            };
            cx.update(|cx| Self::open_window(shot, shown, cx)).ok();
        })
        .detach();
    }

    fn open_window(shot: Shot, shown: Arc<RenderImage>, cx: &mut App) {
        let (width, height) = shot.pixels.dimensions();
        // The picture at its size where it fits; the window is made fit.
        let display = cx.primary_display();
        let room = display
            .as_ref()
            .map_or(size(px(1280.), px(800.)), |display| {
                let bounds = display.bounds();
                size(bounds.size.width * 0.85, bounds.size.height * 0.85)
            });
        let wanted = size(
            px(width as f32).max(px(640.)).min(room.width),
            (px(height as f32) + px(BAR * 2.))
                .max(px(420.))
                .min(room.height),
        );
        let bounds = Bounds::centered(None, wanted, cx);
        let title = Language::of(cx).say("Screenshot");
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            is_movable: true,
            is_resizable: true,
            is_minimizable: true,
            window_min_size: Some(size(px(520.), px(360.))),
            ..Default::default()
        };
        let opened = cx.open_window(options, |_, cx| {
            cx.new(|cx| {
                let mut editor = Self {
                    doc: Doc {
                        picture: shot.pixels,
                        shown,
                        marks: Vec::new(),
                    },
                    undone: Vec::new(),
                    redone: Vec::new(),
                    tool: Tool::Rectangle,
                    color: COLORS[0],
                    width: 1,
                    zoom: None,
                    drawing: None,
                    typing: None,
                    taken: shot.taken,
                    drawn: Rc::default(),
                    status: None,
                    hovered: None,
                    busy: false,
                    version: 0,
                    exports: Vec::new(),
                    focus: cx.focus_handle(),
                    focused: false,
                };
                editor.export(cx);
                editor
            })
        });
        if let Err(error) = opened {
            report!("the editor of the screenshot did not open: {error:#}");
        }
    }

    /// The doc as it is now, made the one to go back to, and the one done
    /// again forgotten.
    fn keep(&mut self) {
        self.undone.push(self.doc.clone());
        self.redone.clear();
    }

    /// The doc changed: what is shown and the file to drag out follow.
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.version += 1;
        self.status = None;
        self.export(cx);
        cx.notify();
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(doc) = self.undone.pop() {
            self.redone.push(std::mem::replace(&mut self.doc, doc));
            self.changed(cx);
        }
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(doc) = self.redone.pop() {
            self.undone.push(std::mem::replace(&mut self.doc, doc));
            self.changed(cx);
        }
    }

    /// The logical pixels a pixel of the picture takes now, for a window
    /// with `scale` physical pixels a logical one and `room` to show it.
    fn shown_scale(&self, scale: f32, room: gpui::Size<Pixels>) -> f32 {
        let (width, height) = self.doc.picture.dimensions();
        let zoom = self.zoom.unwrap_or_else(|| {
            let across = room.width / px(1.) * scale / width.max(1) as f32;
            let down = room.height / px(1.) * scale / height.max(1) as f32;
            across.min(down).min(1.)
        });
        zoom / scale
    }

    /// Zooms one step in or out from what is shown.
    fn zoom_step(&mut self, larger: bool, window: &Window, cx: &mut Context<Self>) {
        let scale = window.scale_factor();
        let now = self.shown_scale(scale, self.room(window)) * scale;
        let next = match larger {
            true => ZOOMS.into_iter().find(|zoom| *zoom > now + 0.001),
            false => ZOOMS.into_iter().rev().find(|zoom| *zoom < now - 0.001),
        };
        if let Some(next) = next {
            self.zoom = Some(next);
            cx.notify();
        }
    }

    /// The room the picture has in the window.
    fn room(&self, window: &Window) -> gpui::Size<Pixels> {
        let viewport = window.viewport_size();
        size(
            viewport.width - px(24.),
            viewport.height - px(BAR * 2. + 24.),
        )
    }

    /// Where on the picture the window's `position` is.
    fn spot(&self, position: Point<Pixels>, window: &Window) -> Spot {
        let drawn = self.drawn.get();
        let scale = self.shown_scale(window.scale_factor(), self.room(window));
        let local = position - drawn.origin;
        Spot::new(local.x / px(1.) / scale, local.y / px(1.) / scale)
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.commit_text(cx);
        let spot = self.spot(event.position, window);
        match self.tool {
            Tool::Text => {
                let placeholder = Language::of(cx).say("Type, then Enter");
                let field = cx.new(|cx| TextField::new(placeholder.into(), cx));
                field.read(cx).focus(window);
                self.typing = Some((spot, field));
            }
            Tool::Stamp => {
                self.keep();
                let number = Mark::next_stamp(&self.doc.marks);
                self.doc.marks.push(Mark::Stamp {
                    at: spot,
                    number,
                    color: self.color,
                    size: STAMP_SIZES[self.width],
                });
                self.changed(cx);
            }
            Tool::Pen | Tool::Highlighter => self.drawing = Some(Drawing::Stroke(vec![spot])),
            _ => {
                self.drawing = Some(Drawing::Span {
                    from: spot,
                    to: spot,
                })
            }
        }
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let spot = self.spot(event.position, window);
        match &mut self.drawing {
            Some(Drawing::Span { to, .. }) => *to = spot,
            Some(Drawing::Stroke(points)) => {
                if points
                    .last()
                    .is_none_or(|last| (last.x - spot.x).abs() + (last.y - spot.y).abs() >= 1.)
                {
                    points.push(spot);
                }
            }
            None => return,
        }
        cx.notify();
    }

    fn mouse_up(&mut self, cx: &mut Context<Self>) {
        let Some(drawing) = self.drawing.take() else {
            return;
        };
        let (color, width) = (self.color, WIDTHS[self.width]);
        let mark = match (self.tool, drawing) {
            (Tool::Rectangle, Drawing::Span { from, to }) => Some(Mark::Rectangle {
                from,
                to,
                color,
                width,
            }),
            (Tool::Ellipse, Drawing::Span { from, to }) => Some(Mark::Ellipse {
                from,
                to,
                color,
                width,
            }),
            (Tool::Arrow, Drawing::Span { from, to }) => Some(Mark::Arrow {
                from,
                to,
                color,
                width,
            }),
            (Tool::Line, Drawing::Span { from, to }) => Some(Mark::Line {
                from,
                to,
                color,
                width,
            }),
            (Tool::Pen, Drawing::Stroke(points)) => Some(Mark::Pen {
                points,
                color,
                width,
            }),
            (Tool::Highlighter, Drawing::Stroke(points)) => Some(Mark::Highlighter {
                points,
                color,
                width: width * 3.,
            }),
            (tool, Drawing::Span { from, to }) if tool.takes_area() => {
                self.change_area(tool, from, to, cx);
                None
            }
            _ => None,
        };
        if let Some(mark) = mark.filter(|mark| !is_dot(mark)) {
            self.keep();
            self.doc.marks.push(mark);
            self.changed(cx);
        }
        cx.notify();
    }

    /// Blurs, pixelates or crops the part between `from` and `to`, away
    /// from the user interface.
    fn change_area(&mut self, tool: Tool, from: Spot, to: Spot, cx: &mut Context<Self>) {
        let Some(area) = Area::between(from, to, self.doc.picture.dimensions()) else {
            return;
        };
        self.busy = true;
        let (picture, drawn) = (self.doc.picture.clone(), self.doc.marks.clone());
        let width = self.width;
        let changed = away("screenshot change", move || {
            let picture = match tool {
                Tool::Blur => marks::blur(&picture, area, BLUR[width]),
                Tool::Pixelate => marks::pixelate(&picture, area, SQUARES[width]),
                // What was drawn goes with the part that is kept.
                _ => marks::crop(&marks::flatten(&picture, &drawn, &render_text), area),
            };
            let shown = render_image(&picture);
            (Arc::new(picture), shown)
        });
        cx.spawn(async move |this, cx| {
            let changed = changed.recv().await;
            this.update(cx, |this, cx| {
                this.busy = false;
                if let Ok((picture, shown)) = changed {
                    this.keep();
                    let marks = match tool {
                        Tool::Crop => Vec::new(),
                        _ => this.doc.marks.clone(),
                    };
                    this.doc = Doc {
                        picture,
                        shown,
                        marks,
                    };
                    if tool == Tool::Crop {
                        this.zoom = None;
                    }
                    this.changed(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Makes the text being typed a mark.
    fn commit_text(&mut self, cx: &mut Context<Self>) {
        let Some((at, field)) = self.typing.take() else {
            return;
        };
        let text = field.read(cx).text();
        if text.trim().is_empty() {
            return;
        }
        self.keep();
        self.doc.marks.push(Mark::Text {
            at,
            text,
            color: self.color,
            size: TEXT_SIZES[self.width],
        });
        self.changed(cx);
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let control = keys.modifiers.control;
        if self.typing.is_some() {
            match keys.key.as_str() {
                "enter" => {
                    self.commit_text(cx);
                    window.focus(&self.focus);
                }
                "escape" => {
                    self.typing = None;
                    window.focus(&self.focus);
                }
                _ => return,
            }
            cx.notify();
            return;
        }
        match (control, keys.modifiers.shift, keys.key.as_str()) {
            (true, false, "z") => self.undo(cx),
            (true, true, "z") | (true, false, "y") => self.redo(cx),
            (true, false, "c") => self.copy(cx),
            (true, false, "s") => self.save(cx),
            (true, true, "s") => self.save_as(cx),
            (true, _, "=" | "+") => self.zoom_step(true, window, cx),
            (true, _, "-") => self.zoom_step(false, window, cx),
            (true, _, "0") => {
                self.zoom = None;
                cx.notify();
            }
            (false, _, "escape") => {
                self.drawing = None;
                cx.notify();
            }
            _ => {}
        }
    }

    fn scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.modifiers.control {
            return;
        }
        let delta = event.delta.pixel_delta(px(16.));
        if delta.y != px(0.) {
            self.zoom_step(delta.y > px(0.), window, cx);
            cx.stop_propagation();
        }
    }

    /// The picture with what is drawn on it, made away from the user
    /// interface.
    fn flattened(&self) -> async_channel::Receiver<Arc<RgbaImage>> {
        let (picture, drawn) = (self.doc.picture.clone(), self.doc.marks.clone());
        away("screenshot flatten", move || match drawn.is_empty() {
            true => picture,
            false => Arc::new(marks::flatten(&picture, &drawn, &render_text)),
        })
    }

    /// Says `text` in the bar at the bottom.
    fn tell(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = Some(text.into());
        cx.notify();
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let flattened = self.flattened();
        cx.spawn(async move |this, cx| {
            let Ok(picture) = flattened.recv().await else {
                return;
            };
            this.update(cx, |this, cx| {
                Screenshots::copy(picture, false, cx);
                let copied = Language::of(cx).say("Copied to the clipboard");
                this.tell(copied, cx);
            })
            .ok();
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let flattened = self.flattened();
        let taken = self.taken;
        cx.spawn(async move |this, cx| {
            let Ok(picture) = flattened.recv().await else {
                return;
            };
            let Ok(saved) = cx.update(|cx| Screenshots::save(picture, taken, cx)) else {
                return;
            };
            let saved = saved.recv().await;
            this.update(cx, |this, cx| this.saved(saved, cx)).ok();
        })
        .detach();
    }

    fn save_as(&mut self, cx: &mut Context<Self>) {
        let config = Screenshots::config(cx);
        let folder = Screenshots::folder(cx).unwrap_or_else(std::env::temp_dir);
        let name = super::file_name(&config.name, self.taken, 1, config.format.extension());
        let chosen = cx.prompt_for_new_path(&folder, Some(&name));
        let flattened = self.flattened();
        cx.spawn(async move |this, cx| {
            let path = match chosen.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                Ok(Err(error)) => {
                    return report!("no file was chosen to save the screenshot as: {error:#}");
                }
                Err(_) => return,
            };
            let Ok(picture) = flattened.recv().await else {
                return;
            };
            let saved = Screenshots::save_as(picture, path).recv().await;
            this.update(cx, |this, cx| this.saved(saved, cx)).ok();
        })
        .detach();
    }

    /// A save ended with `saved`.
    fn saved(
        &mut self,
        saved: Result<anyhow::Result<PathBuf>, async_channel::RecvError>,
        cx: &mut Context<Self>,
    ) {
        let language = Language::of(cx);
        match saved {
            Ok(Ok(path)) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned());
                let text = format!("{} {}", language.say("Saved as"), name.unwrap_or_default());
                self.tell(text, cx);
            }
            Ok(Err(error)) => {
                report!("the screenshot was not saved: {error:#}");
                self.tell(language.say("The screenshot was not saved"), cx);
            }
            Err(_) => self.tell(language.say("The screenshot was not saved"), cx),
        }
    }

    /// Writes a file of what the editor shows into the folder of
    /// temporary files, away from the user interface, for dragging out,
    /// opening with a program and sharing.
    fn export(&mut self, cx: &mut Context<Self>) {
        let version = self.version;
        let flattened = self.flattened();
        let stamp = self.taken.format("%Y-%m-%d %H%M%S").to_string();
        cx.spawn(async move |this, cx| {
            let Ok(picture) = flattened.recv().await else {
                return;
            };
            let written = away("screenshot export", move || {
                let folder = std::env::temp_dir().join("Tessera");
                std::fs::create_dir_all(&folder).ok();
                let path = folder.join(format!("Screenshot {stamp} ({version}).png"));
                let bytes = encode_picture(&picture, image::ImageFormat::Png)?;
                std::fs::write(&path, bytes)?;
                anyhow::Ok(path)
            })
            .recv()
            .await;
            this.update(cx, |this, cx| match written {
                Ok(Ok(path)) => {
                    this.exports.push(Export { version, path });
                    while this.exports.len() > EXPORTS_KEPT {
                        let old = this.exports.remove(0);
                        std::fs::remove_file(old.path).ok();
                    }
                    cx.notify();
                }
                Ok(Err(error)) => {
                    report!("the screenshot is not ready to be dragged out: {error:#}")
                }
                Err(_) => {}
            })
            .ok();
        })
        .detach();
    }

    /// The file of what the editor shows now, once it is written.
    fn exported(&self) -> Option<PathBuf> {
        { self.exports.iter() }
            .find(|export| export.version == self.version)
            .map(|export| export.path.clone())
    }

    fn open_with(&mut self, cx: &mut Context<Self>) {
        match self.exported() {
            Some(path) => choose_app(&path),
            None => {
                let wait = Language::of(cx).say("A moment, the picture is being written");
                self.tell(wait, cx);
            }
        }
    }

    fn share(&mut self, cx: &mut Context<Self>) {
        match self.exported() {
            Some(path) => Files::share(vec![path], cx),
            None => {
                let wait = Language::of(cx).say("A moment, the picture is being written");
                self.tell(wait, cx);
            }
        }
    }

    fn drag_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.exported() else {
            let wait = Language::of(cx).say("A moment, the picture is being written");
            return self.tell(wait, cx);
        };
        let dragged = DragOut::new(vec![path]).start(window, cx);
        cx.spawn(async move |_, _| {
            if let Err(error) = dragged.await {
                report!("the screenshot was not dragged out: {error}");
            }
        })
        .detach();
    }

    fn tool_button(&self, tool: Tool, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let chosen = self.tool == tool;
        square_button(("tool", tool as usize), tool.glyph(), chosen, theme)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hovered = match hovered {
                    true => Some(tool),
                    false => this.hovered.filter(|other| *other != tool),
                };
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.commit_text(cx);
                this.tool = tool;
                cx.notify();
            }))
            .into_any_element()
    }

    fn toolbar(&self, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let mut tools: Vec<AnyElement> = { Tool::ALL.into_iter() }
            .map(|tool| self.tool_button(tool, theme, cx))
            .collect();
        tools.push(gap(theme));
        for (index, color) in COLORS.into_iter().enumerate() {
            let chosen = self.color == color;
            tools.push(
                div()
                    .id(("color", index))
                    .size(px(22.))
                    .flex_none()
                    .rounded(theme.shape.full)
                    .bg(hsla_of(color))
                    .border_2()
                    .border_color(match chosen {
                        true => theme.colors.primary,
                        false => theme.colors.outline,
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.color = color;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        tools.push(gap(theme));
        for (index, width) in WIDTHS.into_iter().enumerate() {
            let chosen = self.width == index;
            let line = div()
                .w(px(16.))
                .h(px(width / 2. + 1.))
                .rounded(theme.shape.full)
                .bg(theme.colors.on_surface);
            tools.push(
                div()
                    .id(("width", index))
                    .size(px(32.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme.shape.small)
                    .when_chosen(chosen, theme)
                    .cursor_pointer()
                    .child(line)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.width = index;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        tools.push(gap(theme));
        let undo = square_button("undo", ICON_UNDO, false, theme)
            .on_click(cx.listener(|this, _, _, cx| this.undo(cx)));
        let redo = square_button("redo", ICON_REDO, false, theme)
            .on_click(cx.listener(|this, _, _, cx| this.redo(cx)));
        tools.push(undo.into_any_element());
        tools.push(redo.into_any_element());
        tools.push(gap(theme));
        let out = square_button("zoom-out", ICON_ZOOM_OUT, false, theme)
            .on_click(cx.listener(|this, _, window, cx| this.zoom_step(false, window, cx)));
        let shown =
            self.shown_scale(window.scale_factor(), self.room(window)) * window.scale_factor();
        let percent = div()
            .w(px(48.))
            .flex_none()
            .flex()
            .justify_center()
            .text_color(theme.colors.on_surface_variant)
            .child(format!("{}%", (shown * 100.).round()));
        let zoom_in = square_button("zoom-in", ICON_ZOOM_IN, false, theme)
            .on_click(cx.listener(|this, _, window, cx| this.zoom_step(true, window, cx)));
        let fit = square_button("zoom-fit", ICON_FIT, self.zoom.is_none(), theme).on_click(
            cx.listener(|this, _, _, cx| {
                this.zoom = None;
                cx.notify();
            }),
        );
        tools.extend([
            out.into_any_element(),
            percent.into_any_element(),
            zoom_in.into_any_element(),
            fit.into_any_element(),
        ]);
        div()
            .h(px(BAR))
            .flex_none()
            .px(theme.spacing.small)
            .flex()
            .items_center()
            .gap(px(2.))
            .overflow_hidden()
            .bg(theme.colors.surface_container)
            .children(tools)
            .into_any_element()
    }

    fn bottom_bar(&self, theme: &Theme, language: Language, cx: &mut Context<Self>) -> AnyElement {
        let (width, height) = self.doc.picture.dimensions();
        let said = match (&self.status, self.hovered) {
            (_, Some(tool)) => SharedString::from(language.say(tool.title())),
            (Some(status), None) => status.clone(),
            (None, None) => format!("{width} × {height} {}", language.say("pixels")).into(),
        };
        let action = |id: &'static str,
                      label: &'static str,
                      cx: &mut Context<Self>,
                      press: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            div()
                .id(id)
                .flex_none()
                .px(theme.spacing.medium)
                .py(theme.spacing.tiny)
                .rounded(theme.shape.full)
                .text_color(theme.colors.primary)
                .hover(|style| style.bg(theme.colors.surface_container_highest))
                .cursor_pointer()
                .child(language.say(label))
                .on_click(cx.listener(move |this, _, window, cx| press(this, window, cx)))
                .into_any_element()
        };
        let ready = self.exported().is_some();
        let drag = div()
            .id("drag-out")
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .px(theme.spacing.medium)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.full)
            .text_color(match ready {
                true => theme.colors.primary,
                false => theme.colors.on_surface_variant,
            })
            .cursor(CursorStyle::OpenHand)
            .child(icon(theme, ICON_DRAG))
            .child(language.say("Drag out"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.drag_out(window, cx)),
            );
        div()
            .h(px(BAR))
            .flex_none()
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .bg(theme.colors.surface_container)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(theme.colors.on_surface_variant)
                    .child(crate::ui::one_line(said)),
            )
            .child(drag)
            .child(action("open-with", "Open with", cx, |this, _, cx| {
                this.open_with(cx)
            }))
            .child(action("share", "Share", cx, |this, _, cx| this.share(cx)))
            .child(action("save-as", "Save as…", cx, |this, _, cx| {
                this.save_as(cx)
            }))
            .child(action("copy", "Copy", cx, |this, _, cx| this.copy(cx)))
            .child(action("save", "Save", cx, |this, _, cx| this.save(cx)))
            .into_any_element()
    }

    /// The picture with its marks and what is being drawn, `scale` logical
    /// pixels a pixel of the picture.
    fn picture(&self, scale: f32, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (width, height) = self.doc.picture.dimensions();
        let (shown_width, shown_height) = (px(width as f32 * scale), px(height as f32 * scale));
        let drawn = self.drawn.clone();
        let mut painted: Vec<Mark> = { self.doc.marks.iter() }
            .filter(|mark| !matches!(mark, Mark::Text { .. } | Mark::Stamp { .. }))
            .cloned()
            .collect();
        let (color, width_now) = (self.color, WIDTHS[self.width]);
        let mut area = None;
        match (&self.drawing, self.tool) {
            (Some(Drawing::Span { from, to }), tool) if tool.takes_area() => {
                area = Some((*from, *to))
            }
            (Some(Drawing::Span { from, to }), tool) => {
                let (from, to) = (*from, *to);
                painted.push(match tool {
                    Tool::Ellipse => Mark::Ellipse {
                        from,
                        to,
                        color,
                        width: width_now,
                    },
                    Tool::Arrow => Mark::Arrow {
                        from,
                        to,
                        color,
                        width: width_now,
                    },
                    Tool::Line => Mark::Line {
                        from,
                        to,
                        color,
                        width: width_now,
                    },
                    _ => Mark::Rectangle {
                        from,
                        to,
                        color,
                        width: width_now,
                    },
                });
            }
            (Some(Drawing::Stroke(points)), Tool::Highlighter) => painted.push(Mark::Highlighter {
                points: points.clone(),
                color,
                width: width_now * 3.,
            }),
            (Some(Drawing::Stroke(points)), _) => painted.push(Mark::Pen {
                points: points.clone(),
                color,
                width: width_now,
            }),
            (None, _) => {}
        }
        let outline = theme.colors.primary;
        let marks_layer = canvas(
            move |bounds, _, _| drawn.set(bounds),
            move |bounds, (), window, _| {
                for mark in &painted {
                    paint_mark(window, mark, bounds.origin, scale);
                }
                if let Some((from, to)) = area {
                    let corners = [from, Spot::new(to.x, from.y), to, Spot::new(from.x, to.y)];
                    let points: Vec<Point<Pixels>> = { corners.iter() }
                        .map(|spot| at(bounds.origin, *spot, scale))
                        .collect();
                    let mut path = PathBuilder::stroke(px(1.5));
                    path.add_polygon(&points, true);
                    if let Ok(path) = path.build() {
                        window.paint_path(path, outline);
                    }
                }
            },
        )
        .absolute()
        .size_full();
        let mut layers = vec![
            img(self.doc.shown.clone())
                .absolute()
                .size_full()
                .into_any_element(),
            marks_layer.into_any_element(),
        ];
        for mark in &self.doc.marks {
            match mark {
                Mark::Text {
                    at: spot,
                    text,
                    color,
                    size,
                } => layers.push(
                    div()
                        .absolute()
                        .left(px(spot.x * scale))
                        .top(px(spot.y * scale))
                        .font_family("Segoe UI")
                        .text_size(px(size * scale))
                        .line_height(px(size * scale * 1.33))
                        .text_color(hsla_of(*color))
                        .whitespace_nowrap()
                        .child(text.clone())
                        .into_any_element(),
                ),
                Mark::Stamp {
                    at: spot,
                    number,
                    color,
                    size,
                } => {
                    let side = px(size * scale);
                    layers.push(
                        div()
                            .absolute()
                            .left(px(spot.x * scale) - side / 2.)
                            .top(px(spot.y * scale) - side / 2.)
                            .size(side)
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(side)
                            .bg(hsla_of(*color))
                            .font_family("Segoe UI")
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(side * 0.6)
                            .text_color(hsla_of(marks::contrast(*color)))
                            .child(number.to_string())
                            .into_any_element(),
                    );
                }
                _ => {}
            }
        }
        if let Some((spot, field)) = &self.typing {
            layers.push(
                div()
                    .absolute()
                    .left(px(spot.x * scale))
                    .top(px(spot.y * scale))
                    .w(px(280.))
                    .flex()
                    .child(field.clone())
                    .into_any_element(),
            );
        }
        div()
            .id("editor-picture")
            .relative()
            .flex_none()
            .w(shown_width)
            .h(shown_height)
            .cursor(CursorStyle::Crosshair)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.mouse_down(event, window, cx)),
            )
            .children(layers)
            .into_any_element()
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus);
        }
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let scale = self.shown_scale(window.scale_factor(), self.room(window));
        let toolbar = self.toolbar(&theme, window, cx);
        let picture = self.picture(scale, &theme, cx);
        let bottom = self.bottom_bar(&theme, language, cx);
        let busy = self.busy.then(|| {
            div()
                .absolute()
                .top(px(BAR + 8.))
                .right(px(16.))
                .px(theme.spacing.small)
                .rounded(theme.shape.small)
                .bg(theme.colors.surface_container)
                .text_color(theme.colors.on_surface_variant)
                .child(icon(&theme, ICON_SCREENSHOT))
        });
        div()
            .id("screenshot-editor")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme.colors.surface_container_highest)
            .text_color(theme.colors.on_surface)
            .text_size(theme.typography.label_size)
            .font_family(theme.typography.family)
            .on_key_down(cx.listener(|this, event, window, cx| this.key_down(event, window, cx)))
            // What is being drawn follows the pointer, and ends where the
            // button is let go, inside the picture or not.
            .on_mouse_move(
                cx.listener(|this, event, window, cx| this.mouse_move(event, window, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.mouse_up(cx)),
            )
            .on_scroll_wheel(
                cx.listener(|this, event, window, cx| this.scroll_wheel(event, window, cx)),
            )
            .child(toolbar)
            .child(
                div()
                    .id("editor-canvas")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_scroll()
                    .flex()
                    .p(px(12.))
                    .child(div().m_auto().child(picture)),
            )
            .child(bottom)
            .children(busy)
    }
}

/// Whether `mark` is too small to be meant: a shape without extent.
fn is_dot(mark: &Mark) -> bool {
    let near = |from: &Spot, to: &Spot| (from.x - to.x).abs() < 2. && (from.y - to.y).abs() < 2.;
    match mark {
        Mark::Rectangle { from, to, .. }
        | Mark::Ellipse { from, to, .. }
        | Mark::Arrow { from, to, .. }
        | Mark::Line { from, to, .. } => near(from, to),
        _ => false,
    }
}

/// Where the picture's `spot` is drawn, the picture's top left corner
/// being at `origin` and `scale` logical pixels a pixel of it.
fn at(origin: Point<Pixels>, spot: Spot, scale: f32) -> Point<Pixels> {
    origin + point(px(spot.x * scale), px(spot.y * scale))
}

/// Draws `mark` over the picture as GPUI paints paths.
fn paint_mark(window: &mut Window, mark: &Mark, origin: Point<Pixels>, scale: f32) {
    let line = |window: &mut Window, points: &[Spot], width: f32, color: Hsla, closed: bool| {
        let points: Vec<Point<Pixels>> =
            points.iter().map(|spot| at(origin, *spot, scale)).collect();
        let mut path = PathBuilder::stroke(px((width * scale).max(1.)));
        match points.as_slice() {
            [] => return,
            [one] => {
                path.move_to(*one);
                path.line_to(*one + point(px(0.1), px(0.1)));
            }
            _ => path.add_polygon(&points, closed),
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    };
    match mark {
        Mark::Rectangle {
            from,
            to,
            color,
            width,
        } => {
            let corners = [*from, Spot::new(to.x, from.y), *to, Spot::new(from.x, to.y)];
            line(window, &corners, *width, hsla_of(*color), true);
        }
        Mark::Ellipse {
            from,
            to,
            color,
            width,
        } => {
            let middle = Spot::new((from.x + to.x) / 2., (from.y + to.y) / 2.);
            let (rx, ry) = ((to.x - from.x).abs() / 2., (to.y - from.y).abs() / 2.);
            let points: Vec<Spot> = (0..72)
                .map(|step| {
                    let angle = step as f32 / 72. * std::f32::consts::TAU;
                    Spot::new(middle.x + rx * angle.cos(), middle.y + ry * angle.sin())
                })
                .collect();
            line(window, &points, *width, hsla_of(*color), true);
        }
        Mark::Line {
            from,
            to,
            color,
            width,
        } => line(window, &[*from, *to], *width, hsla_of(*color), false),
        Mark::Arrow {
            from,
            to,
            color,
            width,
        } => {
            let end = Mark::arrow_shaft_end(*from, *to, *width);
            line(window, &[*from, end], *width, hsla_of(*color), false);
            let corners = Mark::arrow_head(*from, *to, *width);
            let head: Vec<Point<Pixels>> = { corners.iter() }
                .map(|spot| at(origin, *spot, scale))
                .collect();
            let mut path = PathBuilder::fill();
            path.add_polygon(&head, true);
            if let Ok(path) = path.build() {
                window.paint_path(path, hsla_of(*color));
            }
        }
        Mark::Pen {
            points,
            color,
            width,
        } => line(window, points, *width, hsla_of(*color), false),
        Mark::Highlighter {
            points,
            color,
            width,
        } => {
            let color = [color[0], color[1], color[2], HIGHLIGHT];
            line(window, points, *width, hsla_of(color), false);
        }
        Mark::Text { .. } | Mark::Stamp { .. } => {}
    }
}

/// `color` as GPUI takes colours.
fn hsla_of(color: Color) -> Hsla {
    rgba(u32::from_be_bytes(color)).into()
}

/// A square button of a bar with `glyph`, marked where `chosen`.
fn square_button(
    id: impl Into<gpui::ElementId>,
    glyph: &'static str,
    chosen: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(32.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.shape.small)
        .when_chosen(chosen, theme)
        .hover(|style| style.bg(theme.colors.surface_container_highest))
        .cursor_pointer()
        .child(icon(theme, glyph))
}

/// Room between groups of buttons.
fn gap(theme: &Theme) -> AnyElement {
    div()
        .w(theme.spacing.small)
        .h(px(20.))
        .flex_none()
        .border_l_1()
        .border_color(theme.colors.outline.opacity(0.4))
        .ml(theme.spacing.small)
        .into_any_element()
}

/// The look of a button that is chosen.
trait Chosen {
    fn when_chosen(self, chosen: bool, theme: &Theme) -> Self;
}

impl<E: Styled> Chosen for E {
    fn when_chosen(self, chosen: bool, theme: &Theme) -> Self {
        match chosen {
            true => self
                .bg(theme.colors.secondary_container)
                .text_color(theme.colors.primary),
            false => self,
        }
    }
}
