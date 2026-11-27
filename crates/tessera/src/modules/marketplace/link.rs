// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! `tessera://` links, as a web page of a registry offers them for a
//! one-click install: `tessera://install?registry=<url>&kind=<kind>&id=<id>`
//! asks to install what that registry offers, and
//! `tessera://open?settings=<section>&item=<id>` opens the settings. The
//! action `install-resource` takes such a link; Windows hands links to it
//! once they are registered in the marketplace. An install is never done
//! by the link itself: the popup `install-resource` shows the registry,
//! what is offered and what it asks for, and waits for a click.

use std::path::PathBuf;

use gpui::{
    App, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div,
};

use super::{Placing, asks, directory, kind_question, place_widget, placing_label, suggested};
use crate::actions::Action;
use crate::extension::registry::{Catalog, Kind, Location, Offer, Package};
use crate::i18n::Language;
use crate::motion::Interaction;
use crate::popup::Popups;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{one_line, state_layer};

use super::updates::Updates;

/// The name of the action, of the popup and of its widget.
pub const NAME: &str = "install-resource";

/// The popup the settings are in until they can be opened at a part.
const SETTINGS_POPUP: &str = "settings";

/// What a `tessera://` link asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// To install the offer `id` of `kind` of the registry at `registry`.
    Install {
        registry: String,
        kind: Kind,
        id: String,
    },
    /// To open the settings at `section`, at `item` within it.
    Open {
        section: String,
        item: Option<String>,
    },
}

impl Link {
    /// What `text` asks for, or why it is no link Tessera follows.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim().trim_matches('"');
        let scheme = "tessera://";
        let rest = { text.get(..scheme.len()) }
            .filter(|start| start.eq_ignore_ascii_case(scheme))
            .map(|_| &text[scheme.len()..])
            .ok_or_else(|| format!("`{text}` is no tessera:// link"))?;
        let (what, query) = rest.split_once('?').unwrap_or((rest, ""));
        let query = query.split('#').next().unwrap_or_default();
        let pairs = pairs(query).ok_or_else(|| format!("`{text}` is not written right"))?;
        let get = |key: &str| {
            let found = pairs.iter().find(|(name, _)| name == key);
            found.map(|(_, value)| value.clone())
        };
        match what.trim_end_matches('/') {
            "install" => {
                let registry = get("registry").ok_or("the link names no registry")?;
                if !registry.starts_with("https://") {
                    return Err(format!("`{registry}` is no https:// address"));
                }
                let kind = get("kind").unwrap_or_else(|| "extension".to_owned());
                let kind = Kind::named(&kind).ok_or_else(|| format!("`{kind}` is no kind"))?;
                let id = get("id").ok_or("the link names nothing to install")?;
                if !crate::extension::registry::is_plain_id(&id) {
                    return Err(format!("`{id}` is not a name made of a-z, 0-9 and `-`"));
                }
                Ok(Self::Install { registry, kind, id })
            }
            "open" => Ok(Self::Open {
                section: get("settings").ok_or("the link names no part of the settings")?,
                item: get("item"),
            }),
            other => Err(format!("tessera://{other} is not something Tessera does")),
        }
    }
}

/// The pairs of a query, decoded; `None` where one is not written right.
fn pairs(query: &str) -> Option<Vec<(String, String)>> {
    let pairs = query.split('&').filter(|pair| !pair.is_empty());
    pairs
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            Some((decoded(name)?, decoded(value)?))
        })
        .collect()
}

/// `text` with its `%xx` escapes and `+` decoded; `None` where an escape
/// is broken or the result is no UTF-8.
fn decoded(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.bytes();
    while let Some(byte) = rest.next() {
        bytes.push(match byte {
            b'+' => b' ',
            b'%' => {
                let high = (rest.next()? as char).to_digit(16)?;
                let low = (rest.next()? as char).to_digit(16)?;
                (high * 16 + low) as u8
            }
            byte => byte,
        });
    }
    String::from_utf8(bytes).ok()
}

/// The action that follows a link.
pub fn action() -> Action {
    Action::new(follow).checking(|arg| match arg {
        Some(_) => Ok(()),
        None => Err(format!("`{NAME}` takes a tessera:// link as `arg`")),
    })
}

/// Follows the link `arg`. One that cannot be followed is reported.
fn follow(arg: Option<&str>, cx: &mut App) {
    let link = match Link::parse(arg.unwrap_or_default()) {
        Ok(link) => link,
        Err(error) => {
            report!("the link is not followed: {error}");
            return;
        }
    };
    match link {
        // The settings cannot be opened at a part yet. A link opens them
        // and never closes them.
        Link::Open { .. } => {
            if !Popups::shows(SETTINGS_POPUP, cx) {
                Shell::toggle_popup(SETTINGS_POPUP, cx);
            }
        }
        Link::Install { .. } => {
            let shown = cx.default_global::<Requested>().shown;
            cx.global_mut::<Requested>().link = Some(link);
            if !shown {
                Shell::toggle_popup(NAME, cx);
            }
        }
    }
}

/// The link to install from, once one was followed.
#[derive(Default)]
struct Requested {
    link: Option<Link>,
    /// Whether the popup shows it.
    shown: bool,
}

impl gpui::Global for Requested {}

/// Where the popup is with the link it shows.
enum Step {
    Reading,
    /// Downloaded and checked; the user is asked.
    Asking(Box<(Offer, Package)>),
    Installing,
    /// Installed, with the next step that can be taken, if any.
    Done(Option<Placing>),
    Failed(SharedString),
}

/// The widget of the popup `install-resource`.
pub struct InstallPrompt {
    link: Option<Link>,
    /// Whether the configuration names the registry of the link.
    configured: bool,
    step: Step,
    buttons: [Interaction; 3],
}

impl InstallPrompt {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        cx.default_global::<Requested>().shown = true;
        cx.on_release(|_, cx| cx.default_global::<Requested>().shown = false)
            .detach();
        cx.observe_global::<Requested>(|this: &mut Self, cx| {
            let link = cx.global::<Requested>().link.clone();
            if link != this.link {
                this.read(cx);
            }
        })
        .detach();
        let mut this = Self {
            link: None,
            configured: false,
            step: Step::Reading,
            buttons: [Interaction::new(&motion); 3],
        };
        this.read(cx);
        this
    }

    /// Reads the registry of the link and downloads what it asks to
    /// install, away from the user interface.
    fn read(&mut self, cx: &mut Context<Self>) {
        self.link = cx.global::<Requested>().link.clone();
        self.step = Step::Reading;
        let Some(Link::Install { registry, kind, id }) = self.link.clone() else {
            self.step = Step::Failed(Language::of(cx).say("No link was followed").into());
            cx.notify();
            return;
        };
        self.configured = Updates::registries(cx).contains(&registry);
        let language = Language::of(cx);
        let wanted = self.link.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let source =
                        Location::parse(&registry).map_err(|error| format!("{error:#}"))?;
                    let catalog = Catalog::browse(std::slice::from_ref(&registry));
                    let offer = { catalog.offers.into_iter() }.find(|offer| {
                        offer.kind() == kind && offer.id == id && offer.source == source
                    });
                    let Some(offer) = offer else {
                        let mut problem =
                            format!("{registry} offers no {} `{id}`", kind.singular());
                        if let Some(first) = catalog.problems.first() {
                            problem = format!("{problem}: {first}");
                        }
                        return Err(problem);
                    };
                    let package = offer
                        .fetch(language)
                        .map_err(|error| format!("{error:#}"))?;
                    Ok((offer, package))
                })
                .await;
            this.update(cx, |this, cx| {
                // Another link may have been followed meanwhile.
                if this.link != wanted {
                    return;
                }
                this.step = match found {
                    Ok(found) => Step::Asking(Box::new(found)),
                    Err(error) => Step::Failed(error.into()),
                };
                this.buttons = [Interaction::new(&cx.global::<Theme>().motion); 3];
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        self.install_placing(false, cx);
    }

    /// Installs, and puts the widget of an extension where its registry
    /// suggests.
    fn install_and_place(&mut self, cx: &mut Context<Self>) {
        self.install_placing(true, cx);
    }

    /// Installs away from the user interface, as an icon pack may be many
    /// files, and then, with `place`, puts the widget where the registry
    /// suggests.
    fn install_placing(&mut self, place: bool, cx: &mut Context<Self>) {
        let Step::Asking(found) = std::mem::replace(&mut self.step, Step::Installing) else {
            return;
        };
        let (offer, package) = *found;
        let kind = offer.kind();
        let directory = directory(kind);
        let wanted = self.link.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let installed = cx
                .background_executor()
                .spawn(async move {
                    let directory = directory
                        .ok_or_else(|| "the configuration directory is unknown".to_owned());
                    let installed = directory.and_then(|directory| {
                        { package.install(&directory) }.map_err(|error| format!("{error:#}"))
                    });
                    (installed, package)
                })
                .await;
            this.update(cx, |this, cx| {
                this.installed(kind, &offer.id, installed, place, &wanted, cx)
            })
            .ok();
        })
        .detach();
        self.buttons = [Interaction::new(&cx.global::<Theme>().motion); 3];
        cx.notify();
    }

    /// Shows how the install went, and places the widget.
    fn installed(
        &mut self,
        kind: Kind,
        id: &str,
        (installed, package): (Result<PathBuf, String>, Package),
        place: bool,
        wanted: &Option<Link>,
        cx: &mut Context<Self>,
    ) {
        let step = match installed {
            Ok(installed) => {
                cx.defer(Updates::check);
                if kind == Kind::Extension {
                    // The extensions are loaded again before the layout
                    // is, so that the widget is known when it is placed.
                    cx.defer(Shell::extensions_changed);
                }
                let placed = { suggested(&package) }
                    .filter(|_| place)
                    .map(|(widget, place)| place_widget(&widget, &place, cx))
                    .unwrap_or(Ok(()));
                match placed {
                    Ok(()) => Step::Done(Placing::after(kind, id, &installed)),
                    Err(error) => Step::Failed(error.into()),
                }
            }
            Err(error) => Step::Failed(error.into()),
        };
        // Another link may have been followed meanwhile; the install is
        // done all the same, but the popup shows that link.
        if self.link == *wanted && matches!(self.step, Step::Installing) {
            self.step = step;
            self.buttons = [Interaction::new(&cx.global::<Theme>().motion); 3];
            cx.notify();
        }
    }

    /// Takes the next step after the install: uses the icon pack, shows
    /// the wallpaper or places the widget.
    fn take_next_step(&mut self, cx: &mut Context<Self>) {
        let Step::Done(Some(placing)) = std::mem::replace(&mut self.step, Step::Done(None)) else {
            return;
        };
        match placing.take(cx) {
            Ok(()) => Popups::close(cx),
            Err(error) => {
                self.step = Step::Failed(error.into());
                cx.notify();
            }
        }
    }
}

impl Render for InstallPrompt {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let say = |text| language.say(text);
        let caption_size = theme.typography.label_size * super::CAPTION_SHARE;
        let caption = |text: SharedString| {
            div()
                .text_size(caption_size)
                .text_color(theme.colors.on_surface_variant)
                .child(text)
        };
        let states = self.buttons.each_mut().map(|button| button.state(window));
        type Press = fn(&mut InstallPrompt, &mut Context<InstallPrompt>);
        let mut button = |slot: usize, label: &'static str, prominent: bool, press: Press| {
            let base = match prominent {
                true => theme.colors.secondary_container,
                false => theme.colors.surface_container_highest,
            };
            let button = div()
                .id((label, slot))
                .flex_none()
                .px(theme.spacing.small + theme.spacing.tiny)
                .py(theme.spacing.tiny)
                .rounded(theme.shape.full)
                .text_color(theme.colors.primary)
                .bg(state_layer(&theme, base, states[slot]))
                .on_click(cx.listener(move |this, _, _, cx| press(this, cx)))
                .child(say(label));
            Interaction::track(button, cx, move |this: &mut Self| {
                Some(&mut this.buttons[slot])
            })
            .into_any_element()
        };
        let close: Press = |_, cx| Popups::close(cx);

        let mut body: Vec<gpui::AnyElement> = Vec::new();
        let mut buttons: Vec<gpui::AnyElement> = Vec::new();
        if let Some(Link::Install { registry, .. }) = &self.link {
            body.push(
                caption(format!("{} {registry}", say("From the registry")).into())
                    .into_any_element(),
            );
            if !self.configured {
                body.push(
                    caption(
                        say("This registry is not among those of the configuration. Install only what you trust.")
                            .into(),
                    )
                    .text_color(theme.colors.primary)
                    .font_weight(FontWeight::MEDIUM)
                    .into_any_element(),
                );
            }
        }
        match &self.step {
            Step::Reading => {
                body.push(caption(say("Reading the registries…").into()).into_any_element())
            }
            Step::Installing => body.push(caption(say("Downloading…").into()).into_any_element()),
            Step::Failed(error) => {
                body.push(caption(error.clone()).into_any_element());
                buttons.push(button(0, "Close", true, close));
            }
            Step::Asking(found) => {
                let (offer, package) = &**found;
                let title = format!(
                    "{} · {} {}",
                    say(offer.kind().label()),
                    offer.name,
                    offer.version
                );
                body.insert(
                    0,
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.colors.on_surface)
                        .child(one_line(title))
                        .into_any_element(),
                );
                if !offer.description.is_empty() {
                    body.push(caption(offer.description.clone().into()).into_any_element());
                }
                let question = match offer.kind() {
                    Kind::Extension => asks(package, language),
                    kind => say(kind_question(kind)).to_owned(),
                };
                body.push(
                    caption(question.into())
                        .text_color(theme.colors.on_surface)
                        .into_any_element(),
                );
                let accept = match offer.kind() {
                    Kind::Extension => "Allow and install",
                    _ => "Install",
                };
                buttons.push(button(2, "Cancel", false, close));
                match package.suggested_widget() {
                    Some((_, place)) => {
                        buttons.push(button(1, accept, false, Self::install));
                        buttons.push(button(
                            0,
                            placing_label(place),
                            true,
                            Self::install_and_place,
                        ));
                    }
                    None => buttons.push(button(0, accept, true, Self::install)),
                }
            }
            Step::Done(next) => {
                body.push(caption(say("Installed").into()).into_any_element());
                buttons.push(button(1, "Close", false, close));
                if let Some(next) = next {
                    buttons.push(button(0, next.label(), true, Self::take_next_step));
                }
            }
        }
        div()
            .flex_1()
            .p(theme.spacing.medium)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .text_size(theme.typography.label_size)
            .child(
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(say("Install from a link")),
            )
            .children(body)
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.spacing.small)
                    .children(buttons),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_asks_to_install_or_to_open_the_settings() {
        assert_eq!(
            Link::parse(
                "tessera://install/?registry=https%3A%2F%2Fexample.org%2Fr.toml&kind=icon-pack&id=line"
            ),
            Ok(Link::Install {
                registry: "https://example.org/r.toml".to_owned(),
                kind: Kind::IconPack,
                id: "line".to_owned(),
            })
        );
        assert_eq!(
            Link::parse("\"TESSERA://install?id=lights&registry=https://example.org/r.toml\""),
            Ok(Link::Install {
                registry: "https://example.org/r.toml".to_owned(),
                kind: Kind::Extension,
                id: "lights".to_owned(),
            })
        );
        assert_eq!(
            Link::parse("tessera://open?settings=extensions&item=weather#top"),
            Ok(Link::Open {
                section: "extensions".to_owned(),
                item: Some("weather".to_owned()),
            })
        );
        for refused in [
            "https://example.org",
            "tessera://install?registry=C%3A%5Cr.toml&id=x",
            "tessera://install?registry=https://e.org/r.toml&id=Bad%20Id",
            "tessera://install?registry=https://e.org/r.toml&id=x&kind=plugin",
            "tessera://install?registry=https://e.org/r.toml",
            "tessera://install?registry=https://e.org/r.toml&id=%zz",
            "tessera://open",
            "tessera://delete?id=x",
        ] {
            assert!(Link::parse(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn escapes_of_a_link_are_decoded() {
        assert_eq!(decoded("a%20b+c%C3%A9").as_deref(), Some("a b cé"));
        assert_eq!(decoded("%4"), None);
        assert_eq!(decoded("%ff"), None);
    }
}
