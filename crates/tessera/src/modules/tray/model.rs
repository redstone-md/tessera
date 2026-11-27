// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The icons of the notification area as every strip and the overflow show
//! them: read once for all of them, with their names, their pictures and
//! what the settings of Windows say of each.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, AsyncApp, Context, Entity, EntityId, EventEmitter, Global, WeakEntity,
};

use crate::icons::{Icon, icon_of_pixels};
use crate::platform::{Tray, TrayIcon, TrayPromotion};
use crate::tray_settings::{
    Arranged, IconKey, Place, TrayConfig, TraySettings, WindowsChoice, windows_choice,
};

/// How often the icons are checked for changes.
const REFRESH_INTERVAL: Duration = Duration::from_millis(250);

/// How often the choices made in the settings of Windows are read again.
const CHOICES_INTERVAL: Duration = Duration::from_secs(30);

/// How soon after they were read the choices are read again for an icon
/// that was not seen before.
const CHOICES_AGAIN: Duration = Duration::from_secs(3);

/// How long after an icon was chosen with the keyboard its application may
/// hand the keyboard back to the strip.
const KEYBOARD_BACK: Duration = Duration::from_secs(60);

/// One icon with what is shown of it.
pub struct ModelIcon {
    pub icon: TrayIcon,
    pub image: Icon,
    pub key: IconKey,
    /// Whether the settings of Windows put it in the taskbar, if they say.
    pub windows: Option<bool>,
}

/// The strip that chose the icon `id` with the keyboard: its application
/// asked for the keyboard to go back there.
pub struct KeyboardBack {
    pub strip: EntityId,
    pub id: u64,
}

/// The icons of the notification area, once for the whole shell.
pub struct TrayModel {
    /// Oldest first.
    icons: Vec<ModelIcon>,
    /// The state of the notification area the icons were taken from.
    generation: Option<u64>,
    choices: Vec<TrayPromotion>,
    /// When the choices were last asked for.
    choices_asked: Option<Instant>,
    /// The names of the icons seen since the choices were last read.
    seen: HashSet<String>,
    config: TrayConfig,
    /// The icon last chosen with the keyboard, by which strip and when.
    keyboard: Option<(u64, EntityId, Instant)>,
}

impl EventEmitter<KeyboardBack> for TrayModel {}

/// The one model, once it is made.
struct Shared(Entity<TrayModel>);

impl Global for Shared {}

impl TrayModel {
    /// The model, made and started with the first strip. `Err` says why
    /// the notification area stays empty.
    pub fn shared(cx: &mut App) -> Result<Entity<Self>, String> {
        if let Some(shared) = cx.try_global::<Shared>() {
            return Ok(shared.0.clone());
        }
        Tray::start()?;
        let model = cx.new(Self::new);
        cx.set_global(Shared(model.clone()));
        Ok(model)
    }

    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                if this.update(cx, Self::refresh).is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH_INTERVAL).await;
            }
        })
        .detach();
        let requests = Tray::focus_requests();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(id) = requests.recv().await {
                if this
                    .update(cx, |this, cx| this.keyboard_back(id, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.observe_global::<TraySettings>(|this, cx| {
            let config = TraySettings::config(cx);
            if this.config != config {
                this.config = config;
                cx.notify();
            }
        })
        .detach();
        Self {
            icons: Vec::new(),
            generation: None,
            choices: Vec::new(),
            choices_asked: None,
            seen: HashSet::new(),
            config: TraySettings::config(cx),
            keyboard: None,
        }
    }

    /// Takes the icons over if they changed, and has the choices of
    /// Windows read again when it is time or an icon is new.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let due = self
            .choices_asked
            .is_none_or(|asked| asked.elapsed() >= CHOICES_INTERVAL);
        if due {
            self.read_choices(cx);
        }
        if self.generation == Some(Tray::generation()) {
            return;
        }
        let (generation, icons) = Tray::icons();
        self.generation = Some(generation);
        let mut kept: Vec<ModelIcon> = Vec::with_capacity(icons.len());
        for icon in icons {
            let known = self.icons.iter().position(|known| known.icon.id == icon.id);
            let known = known.map(|index| self.icons.swap_remove(index));
            let image = match known {
                Some(known) if known.icon.image_version == icon.image_version => known.image,
                _ => icon.pixels.as_deref().and_then(icon_of_pixels),
            };
            let key = IconKey::new(
                icon.guid,
                icon.executable.as_deref().map(|path| path.as_path()),
                icon.owner_class(),
                icon.uid,
            );
            let windows = self.windows_choice(&icon);
            kept.push(ModelIcon {
                icon,
                image,
                key,
                windows,
            });
        }
        self.icons = kept;
        let fresh = self
            .icons
            .iter()
            .any(|icon| !self.seen.contains(icon.key.as_str()));
        let recent = self
            .choices_asked
            .is_some_and(|asked| asked.elapsed() < CHOICES_AGAIN);
        if fresh && !recent {
            self.read_choices(cx);
        }
        cx.notify();
    }

    /// Reads the choices made in the settings of Windows, away from the
    /// thread that draws, and places the icons by them.
    fn read_choices(&mut self, cx: &mut Context<Self>) {
        self.choices_asked = Some(Instant::now());
        self.seen = self
            .icons
            .iter()
            .map(|icon| icon.key.as_str().to_owned())
            .collect();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let choices = cx
                .background_executor()
                .spawn(async { Tray::windows_choices() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.choices == choices {
                    return;
                }
                this.choices = choices;
                for index in 0..this.icons.len() {
                    this.icons[index].windows = this.windows_choice(&this.icons[index].icon);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// What the settings of Windows say of `icon`.
    fn windows_choice(&self, icon: &TrayIcon) -> Option<bool> {
        let choices: Vec<WindowsChoice> = self
            .choices
            .iter()
            .map(|choice| WindowsChoice {
                guid: choice.guid,
                executable: choice.executable.as_deref().map(|path| path.as_path()),
                uid: choice.uid,
                promoted: choice.promoted,
            })
            .collect();
        let executable = icon.executable.as_deref().map(|path| path.as_path());
        windows_choice(&choices, icon.guid, executable, icon.uid)
    }

    /// The icon `id`, while its application keeps it.
    pub fn icon(&self, id: u64) -> Option<&ModelIcon> {
        self.icons.iter().find(|icon| icon.icon.id == id)
    }

    /// The icons a strip shows, in their order: those in its bar and those
    /// in its overflow. Without `bluetooth` the icon Windows shows for
    /// Bluetooth is left out; the bar holds at most `limit`.
    pub fn arranged(&self, bluetooth: bool, limit: Option<usize>) -> (Vec<u64>, Vec<u64>) {
        let icons: Vec<&ModelIcon> = self
            .icons
            .iter()
            .filter(|icon| bluetooth || !icon.icon.is_of_bluetooth())
            .collect();
        let named: Vec<(&IconKey, Option<bool>)> =
            icons.iter().map(|icon| (&icon.key, icon.windows)).collect();
        let Arranged { bar, overflow } = self.config.arrange(&named, limit);
        let ids = |indices: Vec<usize>| {
            indices
                .into_iter()
                .map(|index| icons[index].icon.id)
                .collect()
        };
        (ids(bar), ids(overflow))
    }

    /// The settings with the icon `id` put at `index` of the bar or of the
    /// overflow, as a strip showing `bar` and `overflow` sees them: where
    /// it stands now and the new order. `None` for an icon that is gone.
    pub fn put(
        &self,
        id: u64,
        to: Place,
        index: usize,
        (bar, overflow): (Vec<u64>, Vec<u64>),
    ) -> Option<TrayConfig> {
        let moved = self.icon(id)?;
        let (mut bar, mut overflow) = (bar, overflow);
        bar.retain(|&other| other != id);
        overflow.retain(|&other| other != id);
        let list = match to {
            Place::Bar => &mut bar,
            Place::Overflow => &mut overflow,
        };
        list.insert(index.min(list.len()), id);
        // An icon over the limit stands in the bar as far as the lists go,
        // and its name is written only where it is to stand elsewhere.
        let config = match self.config.place(&moved.key, moved.windows) == to {
            true => self.config.clone(),
            false => self.config.moved(&moved.key, to),
        };
        let order: Vec<&IconKey> = bar
            .iter()
            .chain(&overflow)
            .filter_map(|&id| Some(&self.icon(id)?.key))
            .collect();
        Some(config.reordered(&order))
    }

    /// The settings with the icon `id` moved to the bar or to the
    /// overflow, keeping its place in the order.
    pub fn moved(&self, id: u64, to: Place) -> Option<TrayConfig> {
        Some(self.config.moved(&self.icon(id)?.key, to))
    }

    /// Notes that the strip `strip` chose the icon `id` with the keyboard,
    /// so that the application may hand the keyboard back to it.
    pub fn chosen_with_keyboard(&mut self, id: u64, strip: EntityId) {
        self.keyboard = Some((id, strip, Instant::now()));
    }

    /// The application of the icon `id` hands the keyboard back: the strip
    /// that chose it with the keyboard lately takes it.
    fn keyboard_back(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some((chosen, strip, when)) = self.keyboard
            && chosen == id
            && when.elapsed() < KEYBOARD_BACK
        {
            cx.emit(KeyboardBack { strip, id });
        }
    }
}
