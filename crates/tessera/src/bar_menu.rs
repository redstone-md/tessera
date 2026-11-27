// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The menu a click with the right button opens on a bar, as the panels of
//! KDE Plasma and the toolbar of Seelen UI have one: on a widget it moves
//! the widget to another slot, takes it out, or adds another; on the bar
//! itself it changes the edge, the thickness, the hiding, the length and
//! the displays of the bar, adds a bar at a free edge or removes this one.
//! Every change is written to the configuration file, which the shell then
//! applies as it applies any change to the file.

use gpui::{App, SharedString, Window};

use crate::config::edit::{ConfigFile, Place, Slot};
use crate::config::{Config, DisplaySelector, Displays, Hiding, SpanAlign, SurfaceConfig};
use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::module::WidgetRegistry;
use crate::panel::{Arranging, rewrite};
use crate::platform::{DisplayName, ScreenEdge};
use crate::popup::PopupAnchor;
use crate::shell::{KnownDisplays, Shell};

const ICON_ADD: &str = "\u{e710}";
const ICON_REMOVE: &str = "\u{e74d}";
const ICON_ARRANGE: &str = "\u{e70f}";
const ICON_MOVE: &str = "\u{e7c2}";
const ICON_BAR: &str = "\u{e8a0}";
const ICON_EDGE: &str = "\u{e740}";
const ICON_THICKNESS: &str = "\u{e9a6}";
const ICON_HIDE: &str = "\u{e7b3}";
const ICON_LENGTH: &str = "\u{e8b9}";
const ICON_DISPLAYS: &str = "\u{e7f4}";
const ICON_SETTINGS: &str = "\u{e713}";
const ICON_NONE: &str = "";

/// The thicknesses offered by name, in logical pixels.
const THICKNESSES: [(&str, f32); 3] = [("Thin", 32.), ("Medium", 48.), ("Thick", 64.)];

/// How much thicker or thinner a step makes a bar, and the least and the
/// most it can be made by steps.
const THICKNESS_STEP: f32 = 4.;
const THICKNESS_RANGE: (f32, f32) = (16., 200.);

/// The shares of its edge a bar can be given from the menu.
const LENGTHS: [f32; 4] = [1., 0.75, 0.5, 0.3];

/// How thick a bar is that the menu adds.
const NEW_THICKNESS: f32 = 40.;

/// The popup the settings are in until they can be opened at a page.
const SETTINGS_POPUP: &str = "settings";

/// What the right button was clicked on.
#[derive(Clone, Debug)]
pub enum BarTarget {
    /// A widget at `place`; `editable` says whether its slot is the one of
    /// the file, which an override for the display may have replaced.
    Widget {
        place: Place,
        name: SharedString,
        editable: bool,
    },
    /// The bar where no widget is; what is added goes to `slot`.
    Bar { slot: Slot },
}

/// The menu of a bar.
pub struct BarMenu;

impl BarMenu {
    /// Opens the menu for `target` on the bar that shows the surface at
    /// `surface` of the configuration, at `edge`, in `window`.
    pub fn open(
        surface: usize,
        edge: ScreenEdge,
        target: BarTarget,
        anchor: PopupAnchor,
        window: &Window,
        cx: &mut App,
    ) {
        let language = Language::of(cx);
        let config = Shell::config(cx).clone();
        let Some(layout) = config.surfaces.get(surface) else {
            return;
        };
        let display = KnownDisplays::of_window(window, cx);
        let bar = Self::bar_entries(surface, edge, layout, &config, display.as_ref(), cx);
        let mut entries = Vec::new();
        match target {
            BarTarget::Widget {
                place,
                name,
                editable,
            } => {
                entries.push(MenuEntry::heading(name));
                if editable {
                    entries.extend(Self::moves(place, language));
                    entries.push(MenuEntry::new(
                        ICON_REMOVE,
                        language.say("Remove widget"),
                        move |cx| later(cx, move |file| file.remove(place)),
                    ));
                    entries.push(MenuEntry::submenu(
                        ICON_ADD,
                        language.say("Add widget"),
                        Self::addable(surface, place.slot, cx),
                    ));
                }
                entries.push(Self::arrange(language));
                entries.push(MenuEntry::submenu(ICON_BAR, language.say("Bar"), bar));
            }
            BarTarget::Bar { slot } => {
                entries.push(MenuEntry::submenu(
                    ICON_ADD,
                    language.say("Add widget"),
                    Self::addable(surface, slot, cx),
                ));
                entries.push(Self::arrange(language));
                entries.extend(bar);
            }
        }
        Menus::open(entries, anchor, cx);
    }

    /// The widgets that can stand on a bar, each of which is added to
    /// `slot` of the surface at `surface` when it is chosen: those the
    /// settings offer, without those made to fill a popup or a card.
    pub fn addable(surface: usize, slot: Slot, cx: &App) -> Vec<MenuEntry> {
        let registry = cx.global::<WidgetRegistry>();
        let names = registry.names().into_iter();
        { names }
            .filter(|name| !registry.is_panel(name))
            .map(|name| {
                let widget = name.clone();
                MenuEntry::new(ICON_ADD, name, move |cx| {
                    let widget = widget.clone();
                    later(cx, move |file| file.add(surface, slot, &widget));
                })
            })
            .collect()
    }

    /// The entries that move the widget at `place` to the end of another
    /// slot.
    fn moves(place: Place, language: Language) -> Vec<MenuEntry> {
        let label = |slot: Slot| match slot {
            Slot::Start => "Move to the start",
            Slot::Center => "Move to the middle",
            Slot::End => "Move to the end",
        };
        Slot::ALL
            .into_iter()
            .filter(|slot| *slot != place.slot)
            .map(|slot| {
                let to = Place {
                    surface: place.surface,
                    slot,
                    index: usize::MAX,
                };
                MenuEntry::new(ICON_MOVE, language.say(label(slot)), move |cx| {
                    later(cx, move |file| file.transfer(place, to));
                })
            })
            .collect()
    }

    fn arrange(language: Language) -> MenuEntry {
        MenuEntry::new(ICON_ARRANGE, language.say("Arrange widgets"), |cx| {
            Arranging::set(true, cx);
        })
    }

    /// The entries about the bar itself.
    fn bar_entries(
        surface: usize,
        edge: ScreenEdge,
        layout: &SurfaceConfig,
        config: &Config,
        display: Option<&DisplayName>,
        cx: &App,
    ) -> Vec<MenuEntry> {
        let language = Language::of(cx);
        let say = |text: &'static str| language.say(text);
        let edges = ScreenEdge::ALL.map(|other| {
            MenuEntry::new(ICON_EDGE, say(edge_label(other)), move |cx| {
                later(cx, move |file| file.set_surface_edge(surface, other));
            })
            .checked(other == edge)
        });
        let mut thickness: Vec<MenuEntry> = THICKNESSES
            .into_iter()
            .map(|(name, pixels)| {
                let label = format!("{} \u{b7} {pixels} px", say(name));
                MenuEntry::new(ICON_THICKNESS, label, move |cx| {
                    later(cx, move |file| file.set_surface_thickness(surface, pixels));
                })
                .checked(layout.thickness == pixels)
            })
            .collect();
        for (name, step) in [("Thicker", THICKNESS_STEP), ("Thinner", -THICKNESS_STEP)] {
            let next = stepped(layout.thickness, step);
            if next != layout.thickness {
                let label = format!("{} \u{b7} {next} px", say(name));
                thickness.push(MenuEntry::new(ICON_THICKNESS, label, move |cx| {
                    later(cx, move |file| file.set_surface_thickness(surface, next));
                }));
            }
        }
        let hiding = Hiding::ALL.map(|hide| {
            MenuEntry::new(ICON_HIDE, say(hide.label()), move |cx| {
                later(cx, move |file| file.set_surface_hide(surface, hide));
            })
            .checked(layout.hide == hide)
        });
        let mut length: Vec<MenuEntry> = LENGTHS
            .into_iter()
            .map(|share| {
                let label = format!("{}%", (share * 100.).round());
                MenuEntry::new(ICON_LENGTH, label, move |cx| {
                    later(cx, move |file| file.set_surface_length(surface, share));
                })
                .checked((layout.length - share).abs() < 0.005)
            })
            .collect();
        if layout.length < 1. {
            length.extend(SpanAlign::ALL.map(|align| {
                let label = say(match align {
                    SpanAlign::Start => Slot::Start.label(),
                    SpanAlign::Center => Slot::Center.label(),
                    SpanAlign::End => Slot::End.label(),
                });
                MenuEntry::new(ICON_LENGTH, label, move |cx| {
                    later(cx, move |file| file.set_surface_align(surface, align));
                })
                .checked(layout.align == align)
            }));
        }
        let mut entries = vec![
            MenuEntry::submenu(ICON_EDGE, say("Edge"), edges.into()),
            MenuEntry::submenu(ICON_THICKNESS, say("Thickness"), thickness),
            MenuEntry::submenu(ICON_HIDE, say("Hiding"), hiding.into()),
            MenuEntry::submenu(ICON_LENGTH, say("Length"), length),
            MenuEntry::submenu(
                ICON_DISPLAYS,
                say("Displays"),
                Self::displays(surface, &layout.displays, KnownDisplays::all(cx), language),
            ),
        ];
        let free = free_edges(config, display);
        let displays = layout.displays.clone();
        let adds: Vec<MenuEntry> = free
            .into_iter()
            .map(|edge| {
                let displays = displays.clone();
                MenuEntry::new(ICON_ADD, say(edge_label(edge)), move |cx| {
                    let displays = displays.clone();
                    later(cx, move |file| {
                        file.add_surface(edge, NEW_THICKNESS, &displays)
                    });
                })
            })
            .collect();
        entries.push(MenuEntry::submenu(ICON_ADD, say("Add bar"), adds));
        if config.surfaces.len() > 1 {
            let asked = vec![
                MenuEntry::heading(say("Remove this bar?")),
                MenuEntry::new(ICON_REMOVE, say("Remove"), move |cx| {
                    later(cx, move |file| file.remove_surface(surface));
                }),
                MenuEntry::new(ICON_NONE, say("Cancel"), |_| {}),
            ];
            entries.push(MenuEntry::submenu(ICON_REMOVE, say("Remove bar"), asked));
        }
        entries.push(MenuEntry::new(ICON_SETTINGS, say("Settings"), |cx| {
            // The menu is still handling the click that chose this.
            cx.defer(|cx| Shell::toggle_popup(SETTINGS_POPUP, cx));
        }));
        entries
    }

    /// The choice of the displays the surface at `surface` appears on:
    /// all, the primary one, the others, or one of `known`.
    fn displays(
        surface: usize,
        displays: &Displays,
        known: &[DisplayName],
        language: Language,
    ) -> Vec<MenuEntry> {
        let choose = |label: SharedString, chosen: Displays, checked: bool| {
            MenuEntry::new(ICON_DISPLAYS, label, move |cx| {
                let chosen = chosen.clone();
                later(cx, move |file| file.set_surface_displays(surface, &chosen));
            })
            .checked(checked)
        };
        let mut entries = vec![
            choose(
                language.say("All displays").into(),
                Displays::All,
                *displays == Displays::All,
            ),
            choose(
                language.say("Primary display").into(),
                Displays::Primary,
                *displays == Displays::Primary,
            ),
            choose(
                language.say("Other displays").into(),
                Displays::Others,
                *displays == Displays::Others,
            ),
        ];
        let mut known = known.to_vec();
        known.sort_by_key(|display| display.position);
        for display in &known {
            let selector = selector_for(display, &known);
            let chosen = Displays::Chosen(vec![selector.clone()]);
            let checked = *displays == chosen;
            let label = format!("{} \u{b7} {}", display.position, display.friendly);
            entries.push(choose(label.into(), chosen, checked));
        }
        entries
    }
}

/// Writes `change` to the configuration once the menu is done with the
/// click that chose it, and has the shell apply it.
fn later(cx: &mut App, change: impl FnOnce(&mut ConfigFile) -> bool + 'static) {
    cx.defer(move |cx| rewrite(cx, change));
}

/// What the menus call an edge.
fn edge_label(edge: ScreenEdge) -> &'static str {
    match edge {
        ScreenEdge::Top => "Top edge",
        ScreenEdge::Bottom => "Bottom edge",
        ScreenEdge::Left => "Left edge",
        ScreenEdge::Right => "Right edge",
    }
}

/// `thickness` a step of `step` thicker, within the range a bar can be
/// made by steps.
fn stepped(thickness: f32, step: f32) -> f32 {
    let (least, most) = THICKNESS_RANGE;
    let next = (thickness + step).clamp(least, most);
    // A bar outside the range is not moved the wrong way into it.
    let backwards = (next - thickness) * step < 0.;
    if backwards { thickness } else { next }
}

/// The edges of `display` no surface of `config` is at: a bar added there
/// keeps clear of the others.
fn free_edges(config: &Config, display: Option<&DisplayName>) -> Vec<ScreenEdge> {
    let primary = display.is_none_or(|display| display.primary);
    ScreenEdge::ALL
        .into_iter()
        .filter(|edge| {
            !config
                .surfaces
                .iter()
                .any(|surface| surface.edge == *edge && surface.displays.includes(display, primary))
        })
        .collect()
}

/// What names `display` among `known` for good: the name of its monitor
/// where no other has the same, and its position otherwise.
fn selector_for(display: &DisplayName, known: &[DisplayName]) -> DisplaySelector {
    let same = known
        .iter()
        .filter(|other| other.friendly.eq_ignore_ascii_case(&display.friendly))
        .count();
    let named = same == 1 && !display.friendly.is_empty() && !display.friendly.contains(['*', '?']);
    let selector = named.then(|| DisplaySelector::parse(&display.friendly).ok());
    match selector.flatten() {
        Some(DisplaySelector::Name(name)) => DisplaySelector::Name(name),
        _ => DisplaySelector::Position(display.position),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(friendly: &str, position: usize, primary: bool) -> DisplayName {
        DisplayName {
            device: format!(r"\\.\DISPLAY{position}"),
            friendly: friendly.to_owned(),
            position,
            primary,
        }
    }

    #[test]
    fn a_bar_is_made_thicker_and_thinner_by_steps_within_its_range() {
        assert_eq!(stepped(40., 4.), 44.);
        assert_eq!(stepped(40., -4.), 36.);
        assert_eq!(stepped(198., 4.), 200.);
        assert_eq!(stepped(16., -4.), 16.);
        // A bar thinner than steps make one stays where it is.
        assert_eq!(stepped(10., -4.), 10.);
    }

    #[test]
    fn a_display_is_named_by_its_monitor_where_that_tells_it_apart() {
        let known = [
            display("DELL U2720Q", 1, true),
            display("LG HDR", 2, false),
            display("LG HDR", 3, false),
            display("2", 4, false),
        ];
        assert_eq!(
            selector_for(&known[0], &known),
            DisplaySelector::Name("DELL U2720Q".into())
        );
        assert_eq!(
            selector_for(&known[2], &known),
            DisplaySelector::Position(3)
        );
        // A name that reads as a position is written as the position.
        assert_eq!(
            selector_for(&known[3], &known),
            DisplaySelector::Position(4)
        );
    }

    #[test]
    fn a_bar_is_added_only_at_an_edge_no_bar_of_the_display_holds() {
        let text = "[[surface]]\nedge = \"top\"\nthickness = 40\n\
                    [[surface]]\nedge = \"bottom\"\nthickness = 40\ndisplays = \"primary\"\n";
        let config: Config = toml::from_str(text).unwrap();
        let main = display("Main", 1, true);
        let side = display("Side", 2, false);
        assert_eq!(
            free_edges(&config, Some(&main)),
            [ScreenEdge::Left, ScreenEdge::Right]
        );
        assert_eq!(
            free_edges(&config, Some(&side)),
            [ScreenEdge::Bottom, ScreenEdge::Left, ScreenEdge::Right]
        );
    }
}
