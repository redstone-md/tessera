// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Offers the extensions, themes, icon packs, wallpapers and tiling
//! layouts of the registries for installing, updating and removing.
//! Widget: `marketplace`, with a tab for each kind it shows. Also the
//! action and the popup `install-resource`, which follow `tessera://`
//! links.

mod link;
mod updates;

use std::mem;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::Context as _;
use gpui::{
    AnyElement, App, AppContext, AsyncApp, Context, EventEmitter, FontWeight, InteractiveElement,
    IntoElement, ObjectFit, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, WeakEntity, Window, div, img, px,
};

pub use self::updates::Updates;
use crate::actions::Actions;
use crate::config::edit::ConfigFile;
use crate::config::{PopupConfig, PopupPlace, WidgetSpec, config_dir};
use crate::extension::extensions_dir;
use crate::extension::registry::{Catalog, Installed, Kind, Offer, Package, Placement};
use crate::i18n::Language;
use crate::icons::{AppIcons, Icon, image_of_bytes};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, InteractionState, Spring};
use crate::platform::UriScheme;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{one_line, state_layer};

/// The size of secondary text, as a share of the label size.
const CAPTION_SHARE: f32 = 0.85;

/// The most room the question below an offer takes.
const QUESTION_ROOM: Pixels = px(160.);

/// How far below its place an offer starts when it appears.
const ENTRANCE_OFFSET: Pixels = px(16.);

/// The round field with the first letter of an offer.
const BADGE_SIZE: Pixels = px(36.);

/// How high the picture of a theme, a wallpaper or an icon pack is drawn,
/// and how many pixels a picture is decoded to at most on its longer
/// side.
const BANNER_HEIGHT: Pixels = px(132.);
const PICTURE_SIZE: u32 = 640;

/// The most columns the offers stand in.
const COLUMNS_LIMIT: f64 = 4.;

/// The option that says what is offered: a kind, a list of kinds, or
/// nothing for every kind.
const SHOW_OPTION: &str = "show";

/// The option that says in how many columns.
const COLUMNS_OPTION: &str = "columns";

/// The size of the popup that follows a link.
const LINK_POPUP_SIZE: (f32, f32) = (420., 320.);

pub struct MarketplaceModule {
    /// Where the registries to read are.
    pub registries: Vec<String>,
}

impl Module for MarketplaceModule {
    fn name(&self) -> &'static str {
        "marketplace"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        let registries = self.registries.clone();
        widgets.register_panel("marketplace", move |context, cx| {
            // The registries of the layout in use are those the shell reads.
            Updates::configure(registries.clone(), cx);
            let kinds = kinds_shown(context.options.get(SHOW_OPTION));
            let columns = context.number(COLUMNS_OPTION).unwrap_or(1.);
            let columns = columns.clamp(1., COLUMNS_LIMIT) as usize;
            cx.new(|cx| Marketplace::new(kinds, columns, cx)).into()
        })?;
        widgets.register_panel(link::NAME, |_, cx| cx.new(link::InstallPrompt::new).into())?;
        let (width, height) = LINK_POPUP_SIZE;
        widgets.register_popup(PopupConfig {
            name: link::NAME.to_owned(),
            place: PopupPlace::Center,
            width,
            height,
            content: vec![WidgetSpec {
                widget: link::NAME.to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        actions.register(link::NAME, link::action())
    }
}

/// The kinds the option `show` names, in the order of the tabs; every
/// kind where it names none. A name that is no kind is reported.
fn kinds_shown(option: Option<&toml::Value>) -> Vec<Kind> {
    let names: Vec<&str> = match option {
        None => return Kind::ALL.to_vec(),
        Some(toml::Value::String(name)) => vec![name.as_str()],
        Some(toml::Value::Array(names)) => names.iter().filter_map(toml::Value::as_str).collect(),
        Some(other) => {
            report!("the marketplace's `show` is a kind or a list of kinds, not `{other}`");
            return Kind::ALL.to_vec();
        }
    };
    let mut kinds = Vec::new();
    for name in names {
        match Kind::named(name) {
            Some(kind) if !kinds.contains(&kind) => kinds.push(kind),
            Some(_) => {}
            None => report!(
                "the marketplace shows \"extensions\", \"themes\", \"icon-packs\", \"wallpapers\" or \"tiling-layouts\", not `{name}`"
            ),
        }
    }
    if kinds.is_empty() {
        kinds.push(Kind::Extension);
    }
    kinds
}

/// Where what is of `kind` is installed: the extensions directory, or
/// the directory of the configuration named after the kind.
fn directory(kind: Kind) -> Option<PathBuf> {
    match kind {
        Kind::Extension => extensions_dir(),
        _ => config_dir().map(|directory| directory.join(kind.name())),
    }
}

/// What of `kind` is installed now.
fn installed(kind: Kind) -> Vec<Installed> {
    directory(kind).map_or_else(Vec::new, |directory| Installed::of(kind, &directory))
}

/// What installing an offer of `kind` that is not an extension does, as
/// the question before it.
fn kind_question(kind: Kind) -> &'static str {
    match kind {
        Kind::Theme => "A theme changes how Tessera looks and nothing else.",
        Kind::IconPack => "An icon pack changes the pictures of icons and nothing else.",
        Kind::Wallpaper => "A wallpaper is a picture and changes nothing else.",
        Kind::TilingLayout | Kind::Extension => {
            "A tiling layout says where windows go and nothing else."
        }
    }
}

/// What removing something of `kind` does, as the question before it.
fn leave_question(kind: Kind) -> &'static str {
    match kind {
        Kind::Extension => "Deletes the extension and what it remembered.",
        Kind::Theme => "Deletes the theme file.",
        Kind::IconPack => "Deletes the icon pack.",
        Kind::Wallpaper => "Deletes the picture.",
        Kind::TilingLayout => "Deletes the layout file.",
    }
}

/// What the marketplace says when the registries offer nothing of `kind`.
fn nothing_offered(kind: Kind) -> &'static str {
    match kind {
        Kind::Extension => "The registries offer no extensions",
        Kind::Theme => "The registries offer no themes",
        Kind::IconPack => "The registries offer no icon packs",
        Kind::Wallpaper => "The registries offer no wallpapers",
        Kind::TilingLayout => "The registries offer no tiling layouts",
    }
}

/// Changes the configuration file with `change` and has the shell apply
/// it once the update under way is over.
fn change_config(cx: &mut App, change: impl FnOnce(&mut ConfigFile) -> bool) -> Result<(), String> {
    let mut file = ConfigFile::open()?;
    if !change(&mut file) {
        return Err("the configuration has no place for it".to_owned());
    }
    file.save()?;
    cx.defer(Shell::config_changed);
    Ok(())
}

/// Puts the icon pack `id` first among those in use, or with `on` false
/// takes it out of them.
fn use_pack(id: &str, on: bool, cx: &mut App) -> Result<(), String> {
    let mut packs = cx.global::<AppIcons>().packs().ids().to_vec();
    packs.retain(|known| known != id);
    if on {
        packs.insert(0, id.to_owned());
    }
    change_config(cx, |file| {
        file.set_icon_packs(&packs);
        true
    })
}

/// Shows the picture `file` as the wallpaper.
fn apply_wallpaper(file: &Path, cx: &mut App) -> Result<(), String> {
    change_config(cx, |file_of_config| file_of_config.apply_wallpaper(file))
}

/// Puts `widget` where `place` says.
fn place_widget(widget: &str, place: &Placement, cx: &mut App) -> Result<(), String> {
    let slot = place.slot().ok_or("the place names no part of a surface")?;
    change_config(cx, |file| file.place_widget(place.in_dock(), slot, widget))
}

/// What can be done right after an install, with one more click.
#[derive(Clone, Debug)]
enum Placing {
    /// Use the icon pack.
    Pack(String),
    /// Show the wallpaper.
    Wallpaper(PathBuf),
    /// Put the widget its registry suggests where it suggests.
    Widget { widget: String, place: Placement },
}

impl Placing {
    /// The step after installing `package`, offered as `id`, to `place`.
    fn after(kind: Kind, id: &str, place: &Path, package: &Package) -> Option<Self> {
        match kind {
            Kind::IconPack => Some(Self::Pack(id.to_owned())),
            Kind::Wallpaper => Some(Self::Wallpaper(place.to_owned())),
            Kind::Extension => {
                let (widget, place) = package.suggested_widget()?;
                Some(Self::Widget {
                    widget: widget.to_owned(),
                    place: place.clone(),
                })
            }
            Kind::Theme | Kind::TilingLayout => None,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Pack(_) => "Use",
            Self::Wallpaper(_) => "Apply",
            Self::Widget { place, .. } if place.in_dock() => "Add to the dock",
            Self::Widget { .. } => "Add to the bar",
        }
    }
}

/// Told to who watches a marketplace when something was installed or
/// removed.
pub struct Changed;

/// What the user is doing with an offer.
enum Stage {
    Idle,
    /// Being downloaded.
    Downloading,
    /// Downloaded and checked; the user is asked to allow what it asks for.
    Offered(Box<Package>),
    /// The user is asked whether to delete it.
    Leaving,
}

struct Row {
    offer: Offer,
    installed: Option<Installed>,
    stage: Stage,
    /// Why the last attempt failed.
    failure: Option<SharedString>,
    /// 0 when the row appears, 1 once it is in place.
    presence: Spring,
    /// How far the question below the row is unfolded, 0 to 1.
    question: Spring,
    buttons: [Interaction; 3],
    /// The picture the registry shows of the offer, once it is here.
    picture: Icon,
    /// Where the registry suggests the widget of the extension just
    /// installed goes, until it is put there or declined.
    suggestion: Option<Placing>,
}

impl Row {
    /// Moves on to `stage`. Its buttons are other ones than those shown
    /// until now, so they start out untouched.
    fn turn(&mut self, stage: Stage, cx: &Context<Marketplace>) -> Stage {
        self.buttons = [Interaction::new(&cx.global::<Theme>().motion); 3];
        mem::replace(&mut self.stage, stage)
    }

    fn refresh(&mut self) {
        self.installed = installed(self.offer.kind())
            .into_iter()
            .find(|installed| installed.id == self.offer.id);
    }

    /// Whether what is installed is from a registry and in another
    /// version than the one offered.
    fn has_update(&self) -> bool {
        let version = self
            .installed
            .as_ref()
            .and_then(|known| known.version.as_ref());
        version.is_some_and(|version| *version != self.offer.version)
    }
}

pub struct Marketplace {
    /// What is offered here, one tab each.
    kinds: Vec<Kind>,
    /// The tab shown.
    kind: Kind,
    /// In how many columns.
    columns: usize,
    /// The reading of the registries the rows were made from.
    shown: Option<Rc<Catalog>>,
    problems: Vec<SharedString>,
    /// The offers of every kind shown.
    rows: Vec<Row>,
    /// What is installed and offered by no registry that was read.
    others: Vec<Installed>,
    /// Whether `tessera://` links are handed to this Tessera; `None`
    /// until that is known.
    links: Option<bool>,
    /// Why the last change in the head of the marketplace failed.
    failure: Option<SharedString>,
    /// Check for updates, update all, and the registration of links.
    controls: [Interaction; 3],
    tabs: Vec<Interaction>,
}

/// What a press of a button of the row at an index does.
type Action = fn(&mut Marketplace, usize, &mut Context<Marketplace>);

/// Something a row lets the user do.
struct Choice {
    label: &'static str,
    /// Whether it is what the row suggests.
    prominent: bool,
    action: Action,
}

impl Choice {
    fn suggested(label: &'static str, action: Action) -> Self {
        Self {
            label,
            prominent: true,
            action,
        }
    }

    fn other(label: &'static str, action: Action) -> Self {
        Self {
            label,
            prominent: false,
            action,
        }
    }
}

impl EventEmitter<Changed> for Marketplace {}

impl Marketplace {
    fn new(kinds: Vec<Kind>, columns: usize, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let mut this = Self {
            kind: kinds[0],
            tabs: kinds.iter().map(|_| Interaction::new(&motion)).collect(),
            kinds,
            columns,
            shown: None,
            problems: Vec::new(),
            rows: Vec::new(),
            others: Vec::new(),
            links: None,
            failure: None,
            controls: [Interaction::new(&motion); 3],
        };
        cx.observe_global::<Updates>(|this: &mut Self, cx| this.follow(cx))
            .detach();
        Updates::check_if_stale(cx);
        this.follow(cx);
        this.ask_about_links(cx);
        this
    }

    /// Shows the latest reading of the registries, if it is new.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let catalog = Updates::catalog(cx);
        let same = match (&catalog, &self.shown) {
            (Some(new), Some(old)) => Rc::ptr_eq(new, old),
            (None, None) => true,
            _ => false,
        };
        if !same && let Some(catalog) = catalog {
            self.show(catalog, cx);
        }
        cx.notify();
    }

    /// Reads the registries again; what they offer replaces the rows.
    fn read(&mut self, _: usize, cx: &mut Context<Self>) {
        Updates::check(cx);
    }

    fn show(&mut self, catalog: Rc<Catalog>, cx: &mut Context<Self>) {
        let motion = cx.global::<Theme>().motion;
        let mut installed: Vec<Installed> = { self.kinds.iter() }
            .flat_map(|&kind| installed(kind))
            .collect();
        self.problems = catalog.problems.iter().cloned().map(Into::into).collect();
        let mut old = mem::take(&mut self.rows);
        let offers = { catalog.offers.iter() }.filter(|offer| self.kinds.contains(&offer.kind()));
        let rows = offers.cloned().map(|offer| {
            let same = |known: &Installed| known.kind() == offer.kind() && known.id == offer.id;
            let known = installed.iter().position(same);
            let installed = known.map(|index| installed.swap_remove(index));
            let earlier = { old.iter() }
                .position(|row| row.offer.kind() == offer.kind() && row.offer.id == offer.id);
            // A row that was there keeps what the user is doing with it.
            if let Some(earlier) = earlier {
                let mut row = old.swap_remove(earlier);
                row.offer = offer;
                row.installed = installed;
                return row;
            }
            let mut presence = Spring::new(motion.spatial, 0.);
            presence.animate_to(1.);
            Row {
                installed,
                offer,
                stage: Stage::Idle,
                failure: None,
                presence,
                question: Spring::new(motion.spatial, 0.),
                buttons: [Interaction::new(&motion); 3],
                picture: None,
                suggestion: None,
            }
        });
        self.rows = rows.collect();
        self.others = installed;
        self.shown = Some(catalog);
        self.fetch_pictures(cx);
        cx.notify();
    }

    /// Downloads the pictures of the offers, away from the user interface,
    /// and shows each as it comes. One that cannot be had is reported and
    /// leaves its offer with the initial of its name.
    fn fetch_pictures(&mut self, cx: &mut Context<Self>) {
        let offers: Vec<Offer> = { self.rows.iter() }
            .filter(|row| row.offer.has_preview() && row.picture.is_none())
            .map(|row| row.offer.clone())
            .collect();
        for offer in offers {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let (kind, id) = (offer.kind(), offer.id.clone());
                let picture = cx
                    .background_executor()
                    .spawn(async move {
                        let bytes = offer.fetch_preview()?;
                        bytes
                            .map(|bytes| image_of_bytes(&bytes, PICTURE_SIZE))
                            .transpose()
                    })
                    .await;
                this.update(cx, |this, cx| match picture {
                    Ok(picture) => {
                        let row = { this.rows.iter_mut() }
                            .find(|row| row.offer.kind() == kind && row.offer.id == id);
                        if let Some(row) = row {
                            row.picture = picture.flatten();
                            cx.notify();
                        }
                    }
                    Err(error) => report!("a picture of the marketplace is missing: {error:#}"),
                })
                .ok();
            })
            .detach();
        }
    }

    /// Learns, away from the user interface, whether links are handed to
    /// this Tessera.
    fn ask_about_links(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let registered = cx
                .background_executor()
                .spawn(async { UriScheme::registered() })
                .await;
            this.update(cx, |this, cx| {
                this.links = Some(registered);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Registers `tessera://` links for this Tessera, or takes them back.
    fn toggle_links(&mut self, _: usize, cx: &mut Context<Self>) {
        let Some(registered) = self.links.take() else {
            return;
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let done = cx
                .background_executor()
                .spawn(async move {
                    match registered {
                        true => UriScheme::unregister(),
                        false => UriScheme::register(),
                    }
                    .map(|()| !registered)
                })
                .await;
            this.update(cx, |this, cx| {
                match done {
                    Ok(now) => {
                        this.links = Some(now);
                        this.failure = None;
                    }
                    Err(error) => {
                        this.links = Some(registered);
                        this.failure = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Downloads every update of what is shown here. An update is
    /// installed as soon as it is here, but for an extension that asks
    /// for other things than the installed version, which asks first.
    fn update_all(&mut self, _: usize, cx: &mut Context<Self>) {
        let waiting: Vec<usize> = { self.rows.iter().enumerate() }
            .filter(|(_, row)| row.has_update() && matches!(row.stage, Stage::Idle))
            .map(|(index, _)| index)
            .collect();
        for index in waiting {
            self.fetch(index, true, cx);
        }
    }

    /// The card of what is installed without a registry that offers it:
    /// its name and, for an extension, its widgets.
    fn render_other(other: &Installed, theme: &Theme, language: Language) -> AnyElement {
        let caption_size = theme.typography.label_size * CAPTION_SHARE;
        let letter = other.id.chars().next().unwrap_or('?');
        let badge = div()
            .size(BADGE_SIZE)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(theme.colors.secondary_container)
            .text_color(theme.colors.primary)
            .font_weight(FontWeight::MEDIUM)
            .child(SharedString::from(letter.to_uppercase().to_string()));
        let mark = match &other.version {
            Some(version) => format!("{} {version}", language.say("Installed")),
            None => language.say("Installed by hand").to_owned(),
        };
        let header = div()
            .flex()
            .items_center()
            .gap(theme.spacing.small + theme.spacing.tiny)
            .child(badge)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.on_surface)
                    .child(one_line(other.id.clone())),
            )
            .child(
                div()
                    .flex_none()
                    .px(theme.spacing.small)
                    .py(theme.spacing.tiny / 2.)
                    .rounded(theme.shape.full)
                    .bg(theme.colors.secondary_container)
                    .text_size(caption_size)
                    .text_color(theme.colors.primary)
                    .child(SharedString::from(mark)),
            );
        let widgets = (!other.widgets.is_empty()).then(|| {
            let said = language.say("Widgets for the layout:");
            div()
                .text_size(caption_size)
                .text_color(theme.colors.on_surface_variant)
                .child(format!("{said} {}", other.widgets.join(", ")))
        });
        let card = div()
            .p(theme.spacing.medium)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .rounded(theme.shape.large)
            .bg(theme.colors.surface_container_highest)
            .child(header)
            .children(widgets);
        div()
            .w_full()
            .flex_none()
            .pb(theme.spacing.medium)
            .child(card)
            .into_any_element()
    }

    /// Downloads the offer of the row, then asks before installing it.
    fn download(&mut self, index: usize, cx: &mut Context<Self>) {
        self.fetch(index, false, cx);
    }

    /// Downloads the offer of the row, then asks before installing it, or
    /// with `then_install` installs it unless it is an extension that asks
    /// for other things than its installed version.
    fn fetch(&mut self, index: usize, then_install: bool, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get_mut(index) else {
            return;
        };
        row.turn(Stage::Downloading, cx);
        row.failure = None;
        let offer = row.offer.clone();
        let language = Language::of(cx);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (kind, id) = (offer.kind(), offer.id.clone());
            let package = cx
                .background_executor()
                .spawn(async move { offer.fetch(language) })
                .await;
            let shown = this.update(cx, |this, cx| {
                // The registries may have been read again meanwhile.
                let waiting = this.rows.iter().position(|row| {
                    row.offer.kind() == kind
                        && row.offer.id == id
                        && matches!(row.stage, Stage::Downloading)
                });
                let Some(index) = waiting else {
                    return;
                };
                let row = &mut this.rows[index];
                match package {
                    Ok(package) => {
                        let installed_wishes =
                            row.installed.as_ref().map(|known| known.wishes(language));
                        let same = installed_wishes.as_ref() == Some(&package.wishes);
                        let at_once = then_install && (kind != Kind::Extension || same);
                        row.turn(Stage::Offered(Box::new(package)), cx);
                        if at_once {
                            this.confirm(index, cx);
                            return;
                        }
                        row.question.animate_to(1.);
                    }
                    Err(error) => {
                        row.turn(Stage::Idle, cx);
                        row.failure = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            });
            shown.ok();
        })
        .detach();
        cx.notify();
    }

    /// Asks whether to delete what the row installed.
    fn ask_to_remove(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get_mut(index) {
            row.turn(Stage::Leaving, cx);
            row.failure = None;
            row.question.animate_to(1.);
            cx.notify();
        }
    }

    /// Does what the row asked about: installs or deletes what it offers.
    /// The shell loads the extensions again after one of them changed.
    fn confirm(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get_mut(index) else {
            return;
        };
        let kind = row.offer.kind();
        let done = match row.turn(Stage::Idle, cx) {
            Stage::Offered(package) => directory(kind)
                .context("the configuration directory is unknown")
                .and_then(|directory| package.install(&directory))
                .map(|place| {
                    let next = Placing::after(kind, &row.offer.id, &place, &package);
                    row.suggestion = next.filter(|next| matches!(next, Placing::Widget { .. }));
                }),

            Stage::Leaving => {
                let removed = row.installed.as_ref().map_or(Ok(()), Installed::remove);
                // An icon pack that is gone is no longer used.
                if removed.is_ok() && kind == Kind::IconPack {
                    let id = row.offer.id.clone();
                    if cx.global::<AppIcons>().packs().ids().contains(&id)
                        && let Err(error) = use_pack(&id, false, cx)
                    {
                        report!("{error}");
                    }
                }
                removed
            }
            stage => {
                row.stage = stage;
                return;
            }
        };
        let Some(row) = self.rows.get_mut(index) else {
            return;
        };
        // The suggestion of a place, if any, is the next question.
        row.question
            .animate_to(if row.suggestion.is_some() { 1. } else { 0. });
        row.failure = done.err().map(|error| format!("{error:#}").into());
        row.refresh();
        if kind == Kind::Extension {
            cx.defer(Shell::extensions_changed);
        }
        cx.emit(Changed);
        cx.notify();
    }

    fn cancel(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get_mut(index) {
            row.turn(Stage::Idle, cx);
            row.question.animate_to(0.);
            row.suggestion = None;
            cx.notify();
        }
    }

    /// Does what [`Placing`] the row holds, or with an icon pack in use
    /// takes it out of use.
    fn take_step(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get_mut(index) else {
            return;
        };
        let id = row.offer.id.clone();
        let in_use = cx.global::<AppIcons>().packs().ids().contains(&id);
        let place = row.installed.as_ref().map(|known| known.place().to_owned());
        let step = match (row.suggestion.take(), row.offer.kind(), place) {
            (Some(step), ..) => step,
            (None, Kind::IconPack, _) => Placing::Pack(id.clone()),
            (None, Kind::Wallpaper, Some(place)) => Placing::Wallpaper(place),
            _ => return,
        };
        let done = match step {
            Placing::Pack(id) => use_pack(&id, !in_use, cx),
            Placing::Wallpaper(file) => apply_wallpaper(&file, cx),
            Placing::Widget { widget, place } => place_widget(&widget, &place, cx),
        };
        if let Some(row) = self.rows.get_mut(index) {
            row.turn(Stage::Idle, cx);
            row.question.animate_to(0.);
            row.failure = done.err().map(Into::into);
        }
        cx.notify();
    }

    /// A button of the row at `index`; `slot` says which of its
    /// interactions it drives.
    fn button(
        index: usize,
        slot: usize,
        choice: Choice,
        state: InteractionState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Choice {
            label,
            prominent,
            action,
        } = choice;
        let button = pill(label, (label, index), prominent, state, theme, cx)
            .on_click(cx.listener(move |this, _, _, cx| action(this, index, cx)));
        Interaction::track(button, cx, move |this: &mut Self| {
            Some(&mut this.rows.get_mut(index)?.buttons[slot])
        })
        .into_any_element()
    }

    /// A button of the head of the marketplace.
    fn control(
        &mut self,
        slot: usize,
        label: &'static str,
        prominent: bool,
        action: Action,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let state = self.controls[slot].state(window);
        let button = pill(label, label, prominent, state, &theme, cx)
            .on_click(cx.listener(move |this, _, _, cx| action(this, slot, cx)));
        Interaction::track(button, cx, move |this: &mut Self| {
            Some(&mut this.controls[slot])
        })
        .into_any_element()
    }

    /// What the row at `index` lets the user do while nothing is under
    /// way.
    fn idle_choices(&self, index: usize, cx: &App) -> Vec<(usize, Choice)> {
        let row = &self.rows[index];
        let mut choices = Vec::new();
        let Some(known) = &row.installed else {
            choices.push((0, Choice::suggested("Install", Self::download)));
            return choices;
        };
        if row.has_update() {
            choices.push((0, Choice::suggested("Update", Self::download)));
        }
        match row.offer.kind() {
            Kind::IconPack => {
                let in_use = cx
                    .global::<AppIcons>()
                    .packs()
                    .ids()
                    .contains(&row.offer.id);
                let label = if in_use { "Stop using" } else { "Use" };
                choices.push((1, Choice::other(label, Self::take_step)));
            }
            Kind::Wallpaper => choices.push((1, Choice::other("Apply", Self::take_step))),
            Kind::Extension | Kind::Theme | Kind::TilingLayout => {}
        }
        if known.version.is_some() {
            choices.push((2, Choice::other("Remove", Self::ask_to_remove)));
        }
        choices
    }

    fn render_row(&mut self, index: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let choices = self.idle_choices(index, cx);
        let in_use = self.rows[index].offer.kind() == Kind::IconPack && {
            cx.global::<AppIcons>().packs().ids()
        }
        .contains(&self.rows[index].offer.id);
        let row = &mut self.rows[index];
        let kind = row.offer.kind();
        let presence = row.presence.value(window);
        let question = row.question.value(window).max(0.);
        let states = row.buttons.each_mut().map(|button| button.state(window));
        let caption_size = theme.typography.label_size * CAPTION_SHARE;
        let mut button = |slot: usize, choice: Choice| {
            Self::button(index, slot, choice, states[slot], &theme, cx)
        };
        let caption = |text: SharedString| {
            div()
                .text_size(caption_size)
                .text_color(theme.colors.on_surface_variant)
                .child(text)
        };

        let mut actions: Vec<AnyElement> = Vec::new();
        match &row.stage {
            Stage::Idle if row.suggestion.is_none() => {
                for (slot, choice) in choices {
                    actions.push(button(slot, choice));
                }
            }
            Stage::Downloading => {
                actions.push(caption(language.say("Downloading…").into()).into_any_element());
            }
            Stage::Idle | Stage::Offered(_) | Stage::Leaving => {}
        }
        let letter = row.offer.name.chars().next().unwrap_or('?');
        let picture = row.picture.clone();
        let banner = picture
            .clone()
            .filter(|_| kind != Kind::Extension)
            .map(|picture| {
                img(picture)
                    .w_full()
                    .h(BANNER_HEIGHT)
                    .rounded(theme.shape.medium)
                    .object_fit(ObjectFit::Cover)
            });
        let icon = picture.filter(|_| kind == Kind::Extension).map(|picture| {
            img(picture)
                .size(BADGE_SIZE)
                .flex_none()
                .rounded(theme.shape.small)
                .into_any_element()
        });
        let badge = div()
            .size(BADGE_SIZE)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(theme.colors.secondary_container)
            .text_color(theme.colors.primary)
            .font_weight(FontWeight::MEDIUM)
            .child(SharedString::from(letter.to_uppercase().to_string()));
        let origin = format!("{} · {}", row.offer.version, row.offer.registry);
        let title = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.on_surface)
                    .child(one_line(row.offer.name.clone())),
            )
            .child(caption(origin.into()));
        let mark = row.installed.as_ref().map(|known| {
            let text = match (&known.version, in_use) {
                (_, true) => language.say("In use").to_owned(),
                (Some(version), false) => format!("{} {version}", language.say("Installed")),
                (None, false) => language.say("Installed by hand").to_owned(),
            };
            div()
                .flex_none()
                .px(theme.spacing.small)
                .py(theme.spacing.tiny / 2.)
                .rounded(theme.shape.full)
                .bg(theme.colors.secondary_container)
                .text_size(caption_size)
                .text_color(theme.colors.primary)
                .child(SharedString::from(text))
        });
        let header = div()
            .flex()
            .items_center()
            .gap(theme.spacing.small + theme.spacing.tiny)
            .child(icon.unwrap_or_else(|| badge.into_any_element()))
            .child(title)
            .children(mark);
        let description = (!row.offer.description.is_empty())
            .then(|| caption(row.offer.description.clone().into()));
        let installed = row.installed.as_ref();
        let widgets = installed.filter(|known| !known.widgets.is_empty());
        let widgets = widgets.map(|known| {
            let said = language.say("Widgets for the layout:");
            caption(format!("{said} {}", known.widgets.join(", ")).into())
        });
        let layout_name = (kind == Kind::TilingLayout && installed.is_some()).then(|| {
            let said = language.say("Its name for the tiling:");
            caption(format!("{said} {}", row.offer.id).into())
        });
        let actions = (!actions.is_empty()).then(|| {
            div()
                .pt(theme.spacing.tiny)
                .flex()
                .flex_wrap()
                .items_center()
                .justify_end()
                .gap(theme.spacing.small)
                .children(actions)
        });

        let asked = match (&row.stage, kind, &row.suggestion) {
            (Stage::Offered(package), Kind::Extension, _) => {
                Some((asks(package, language), "Allow and install"))
            }
            (Stage::Offered(_), kind, _) => {
                Some((language.say(kind_question(kind)).to_owned(), "Install"))
            }
            (Stage::Leaving, kind, _) => {
                Some((language.say(leave_question(kind)).to_owned(), "Remove"))
            }
            (Stage::Idle, _, Some(next)) => Some((
                language
                    .say("Its registry suggests a place for its widget.")
                    .to_owned(),
                next.label(),
            )),
            (Stage::Idle | Stage::Downloading, ..) => None,
        };
        let accept: Action = match row.suggestion {
            Some(_) => Self::take_step,
            None => Self::confirm,
        };
        let decline = match row.suggestion {
            Some(_) => "Not now",
            None => "Cancel",
        };
        let asked = asked.map(|(text, accept_label)| {
            div()
                .pt(theme.spacing.small)
                .flex()
                .flex_col()
                .gap(theme.spacing.small)
                .child(caption(text.into()).text_color(theme.colors.on_surface))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(theme.spacing.small)
                        .child(button(1, Choice::other(decline, Self::cancel)))
                        .child(button(0, Choice::suggested(accept_label, accept))),
                )
        });
        // The question unfolds and folds; while it folds it is already empty.
        let unfolded = question > 0. || asked.is_some();
        let asked = unfolded.then(|| {
            div()
                .max_h(QUESTION_ROOM * question)
                .opacity(question.clamp(0., 1.))
                .overflow_hidden()
                .children(asked)
        });
        let failure = row.failure.clone().map(caption);

        let card = div()
            .p(theme.spacing.medium)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .rounded(theme.shape.large)
            .bg(theme.colors.surface_container_highest)
            .children(banner)
            .child(header)
            .children(description)
            .children(widgets)
            .children(layout_name)
            .children(actions)
            .children(asked)
            .children(failure);
        div()
            .w_full()
            .flex_none()
            .pt(ENTRANCE_OFFSET * (1. - presence).max(0.))
            .pb(theme.spacing.medium)
            .opacity(presence.clamp(0., 1.))
            .child(card)
            .into_any_element()
    }

    /// How many offers of `kind` are updates of what is installed.
    fn updates_of(&self, kind: Kind) -> usize {
        let rows = self.rows.iter();
        rows.filter(|row| row.offer.kind() == kind && row.has_update())
            .count()
    }

    /// The tabs of the kinds, each with the number of its updates.
    fn render_tabs(
        &mut self,
        window: &Window,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = Language::of(cx);
        let kinds = self.kinds.clone();
        let tabs = kinds.into_iter().enumerate().map(|(index, kind)| {
            let state = self.tabs[index].state(window);
            let chosen = kind == self.kind;
            let base = match chosen {
                true => theme.colors.secondary_container,
                false => theme.colors.surface_container,
            };
            let count = self.updates_of(kind);
            let label = match count {
                0 => language.say(kind.label()).to_owned(),
                count => format!("{} · {count}", language.say(kind.label())),
            };
            let tab = div()
                .id(("kind", index))
                .flex_none()
                .px(theme.spacing.small + theme.spacing.tiny)
                .py(theme.spacing.tiny)
                .rounded(theme.shape.full)
                .text_color(match chosen {
                    true => theme.colors.primary,
                    false => theme.colors.on_surface_variant,
                })
                .bg(state_layer(theme, base, state))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kind = kind;
                    cx.notify();
                }))
                .child(label);
            Interaction::track(tab, cx, move |this: &mut Self| this.tabs.get_mut(index))
                .into_any_element()
        });
        let tabs: Vec<AnyElement> = tabs.collect();
        div()
            .flex_none()
            .pb(theme.spacing.small)
            .flex()
            .flex_wrap()
            .gap(theme.spacing.tiny)
            .children(tabs)
            .into_any_element()
    }
}

/// A round button labelled `label`, prominent or not.
fn pill(
    label: &'static str,
    id: impl Into<gpui::ElementId>,
    prominent: bool,
    state: InteractionState,
    theme: &Theme,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let base = if prominent {
        theme.colors.secondary_container
    } else {
        theme.colors.surface_container_highest
    };
    div()
        .id(id)
        .flex_none()
        .px(theme.spacing.small + theme.spacing.tiny)
        .py(theme.spacing.tiny)
        .rounded(theme.shape.full)
        .text_color(theme.colors.primary)
        .bg(state_layer(theme, base, state))
        .child(Language::of(cx).say(label))
}

/// What installing `package` allows it, in a sentence.
fn asks(package: &Package, language: Language) -> String {
    let say = |text| language.say(text);
    if package.wishes.is_empty() {
        say("Asks for nothing beyond showing its widgets.").to_owned()
    } else {
        let joint = format!(" {} ", say("and to"));
        format!("{} {}.", say("Asks to"), package.wishes.join(&joint))
    }
}

impl Render for Marketplace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let caption_size = theme.typography.label_size * CAPTION_SHARE;
        let kind = self.kind;
        let others = self.others.iter().filter(|other| other.kind() == kind);
        let mut cards: Vec<AnyElement> = others
            .map(|other| Self::render_other(other, &theme, language))
            .collect();
        for index in 0..self.rows.len() {
            if self.rows[index].offer.kind() == kind {
                cards.push(self.render_row(index, window, cx));
            }
        }

        let reading = Updates::reading(cx);
        let updates: usize = self.kinds.iter().map(|&kind| self.updates_of(kind)).sum();
        let check =
            (!reading).then(|| self.control(0, "Check for updates", false, Self::read, window, cx));
        let update_all = (updates > 0 && !reading)
            .then(|| self.control(1, "Update all", true, Self::update_all, window, cx));
        let count = (updates > 0).then(|| {
            div()
                .text_size(caption_size)
                .child(format!("{} {updates}", language.say("Updates:")))
        });
        let header = div()
            .min_h(theme.bar.pill_height)
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.spacing.small)
            .text_color(theme.colors.on_surface_variant)
            .child(div().flex_1().child(language.say("From the registries")))
            .children(count)
            .children(update_all)
            .children(check);
        let tabs = (self.kinds.len() > 1).then(|| self.render_tabs(window, &theme, cx));
        let problems = self.problems.iter().chain(&self.failure).map(|problem| {
            div()
                .flex_none()
                .pb(theme.spacing.tiny)
                .text_size(caption_size)
                .text_color(theme.colors.on_surface_variant)
                .child(problem.clone())
        });
        let problems: Vec<_> = problems.collect();
        let links = self.links.map(|registered| {
            let (said, label) = match registered {
                true => ("tessera:// links open here.", "Unregister tessera:// links"),
                false => (
                    "tessera:// links of registries can open here.",
                    "Register tessera:// links",
                ),
            };
            div()
                .flex_none()
                .pt(theme.spacing.small)
                .flex()
                .flex_wrap()
                .items_center()
                .gap(theme.spacing.small)
                .text_size(caption_size)
                .text_color(theme.colors.on_surface_variant)
                .child(div().flex_1().child(language.say(said)))
                .child(self.control(2, label, false, Self::toggle_links, window, cx))
        });

        let note = |text: &'static str| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.colors.on_surface_variant)
                .child(language.say(text))
                .into_any_element()
        };
        let list = match (cards.is_empty(), reading) {
            (true, true) => note("Reading the registries…"),
            (true, false) => note(nothing_offered(kind)),
            (false, _) => {
                // Dealt out like cards, so that the columns stay even.
                let mut columns: Vec<Vec<AnyElement>> = Vec::new();
                columns.resize_with(self.columns.max(1), Vec::new);
                let count = columns.len();
                for (index, card) in cards.into_iter().enumerate() {
                    columns[index % count].push(card);
                }
                let columns = columns.into_iter().map(|cards| {
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .children(cards)
                });
                div()
                    .id("offered")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_start()
                            .gap(theme.spacing.medium)
                            .children(columns),
                    )
                    .into_any_element()
            }
        };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .text_size(theme.typography.label_size)
            .children(tabs)
            .child(header)
            .children(problems)
            .child(list)
            .children(links)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_an_extension_asks_for_is_said_in_full() {
        let package =
            |wishes: &[&str]| Package::asking(wishes.iter().map(|&wish| wish.to_owned()).collect());
        let english = Language::English;
        assert_eq!(
            asks(&package(&[]), english),
            "Asks for nothing beyond showing its widgets."
        );
        assert_eq!(
            asks(
                &package(&["contact a.example, b.example", "read ~/x"]),
                english
            ),
            "Asks to contact a.example, b.example and to read ~/x."
        );
        assert_eq!(
            asks(
                &package(&[
                    "обращаться к a.example",
                    "узнавать об устройствах bluetooth"
                ]),
                Language::Russian
            ),
            "Просит разрешения обращаться к a.example и узнавать об устройствах bluetooth."
        );
    }

    #[test]
    fn the_marketplace_shows_the_kinds_it_is_asked_for() {
        let value = |text: &str| -> toml::Value {
            toml::from_str::<toml::Table>(&format!("show = {text}")).unwrap()["show"].clone()
        };
        assert_eq!(kinds_shown(None), Kind::ALL);
        assert_eq!(kinds_shown(Some(&value("\"themes\""))), [Kind::Theme]);
        assert_eq!(
            kinds_shown(Some(&value(
                "[\"wallpapers\", \"icon-pack\", \"wallpapers\"]"
            ))),
            [Kind::Wallpaper, Kind::IconPack]
        );
        assert_eq!(kinds_shown(Some(&value("\"plugins\""))), [Kind::Extension]);
    }
}
