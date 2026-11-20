// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A radio's button: its colour, and what it shows (its initials, one or two
//! characters, an emoji or an icon). The rail, the radios' list and their
//! editor show it the same way, at their own sizes.

use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;
use meshctl_api::{Badge, BadgeContent, BadgeIcon};

/// The palette's colours, by name, as the API names them: the style sheet has
/// a tone of each for each theme (`--mc-badge-<name>`), which reads with its
/// text (`--mc-badge-ink`). Any other colour can be picked too.
pub use meshctl_api::BADGE_COLORS as PALETTE;

/// The colours of the palette as they were once kept, as `#rrggbb`, in its
/// order: they are read as the palette's.
const PALETTE_BEFORE: [&str; 10] = [
    "#f0a63a", "#ff8a6b", "#f06595", "#cc5de8", "#9775fa", "#4dabf7", "#3bc9db", "#38d9a9",
    "#a9e34b", "#adb5bd",
];

/// The palette's colour a colour is, if it is one: by its name, or as it
/// was once kept.
pub fn palette_name(color: &str) -> Option<&'static str> {
    PALETTE
        .iter()
        .zip(PALETTE_BEFORE)
        .find(|(name, before)| **name == color || before.eq_ignore_ascii_case(color))
        .map(|(name, _)| *name)
}

/// Two letters for a radio: its words' first, or its first two.
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let letters: String = match words.as_slice() {
        [first, second, ..] => first
            .chars()
            .take(1)
            .chain(second.chars().take(1))
            .collect(),
        [only] => only.chars().take(2).collect(),
        [] => "?".into(),
    };
    let mut chars = letters.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The text that reads on a colour: dark on the light ones, white on the
/// dark ones.
fn ink(color: &str) -> Option<&'static str> {
    let hex = color.strip_prefix('#').filter(|hex| hex.len() == 6)?;
    let value = u32::from_str_radix(hex, 16).ok()?;
    let channel = |shift: u32| f64::from((value >> shift) & 0xff);
    let light = (channel(16) * 0.299 + channel(8) * 0.587 + channel(0) * 0.114) / 255.0;
    Some(if light > 0.55 { "#12161c" } else { "#ffffff" })
}

/// The style a button takes for its colour: none for the rail's own.
pub fn style(badge: &Badge) -> String {
    let Some(color) = badge.color.as_deref() else {
        return String::new();
    };
    match palette_name(color) {
        // The theme's tones of it, at rest and selected, and their text.
        Some(name) => format!(
            "--badge: var(--mc-badge-{name}); --badge-ink: var(--mc-badge-ink); \
             --badge-on: var(--mc-badge-{name}-on); --badge-on-ink: var(--mc-badge-ink-on)"
        ),
        // Any other: dimmed into the rail at rest, in full selected.
        None => ink(color)
            .map(|ink| {
                format!(
                    "--badge: color-mix(in srgb, {color} 50%, var(--mc-rail)); \
                     --badge-ink: {ink}; --badge-on: {color}; --badge-on-ink: {ink}"
                )
            })
            .unwrap_or_default(),
    }
}

/// The class a button takes: `colored` when it has a colour of its own.
pub fn class(badge: &Badge) -> &'static str {
    if badge.color.is_some() {
        "face colored"
    } else {
        "face"
    }
}

/// An icon's picture, as Tabler draws it: two of them, the radios' own, drawn
/// for meshctl in their spirit. The projects' logos are trademarks.
pub fn svg(icon: BadgeIcon) -> &'static str {
    macro_rules! tabler {
        ($name:literal) => {
            include_str!(concat!(
                "../vendor/tabler-icons-3.51.0/icons/outline/",
                $name,
                ".svg"
            ))
        };
    }
    match icon {
        BadgeIcon::Meshtastic => include_str!("../icons/meshtastic.svg"),
        BadgeIcon::Meshcore => include_str!("../icons/meshcore.svg"),
        BadgeIcon::Home => tabler!("home"),
        BadgeIcon::Car => tabler!("car"),
        BadgeIcon::Bike => tabler!("bike"),
        BadgeIcon::Walk => tabler!("walk"),
        BadgeIcon::Boat => tabler!("sailboat"),
        BadgeIcon::Plane => tabler!("plane"),
        BadgeIcon::Tower => tabler!("building-broadcast-tower"),
        BadgeIcon::Antenna => tabler!("antenna"),
        BadgeIcon::Mountain => tabler!("mountain"),
        BadgeIcon::Tent => tabler!("tent"),
        BadgeIcon::Backpack => tabler!("backpack"),
        BadgeIcon::Briefcase => tabler!("briefcase"),
        BadgeIcon::Building => tabler!("building"),
        BadgeIcon::Flask => tabler!("flask"),
        BadgeIcon::Sun => tabler!("sun"),
        BadgeIcon::Star => tabler!("star"),
        BadgeIcon::Walkie => tabler!("radio"),
        BadgeIcon::Satellite => tabler!("satellite"),
        BadgeIcon::Flag => tabler!("flag"),
        BadgeIcon::Bolt => tabler!("bolt"),
        BadgeIcon::Tree => tabler!("tree"),
        BadgeIcon::Heart => tabler!("heart"),
        BadgeIcon::Truck => tabler!("truck"),
        BadgeIcon::Motorbike => tabler!("motorbike"),
        BadgeIcon::Caravan => tabler!("caravan"),
        BadgeIcon::Lighthouse => tabler!("building-lighthouse"),
        BadgeIcon::SolarPanel => tabler!("solar-panel"),
        BadgeIcon::Compass => tabler!("compass"),
        BadgeIcon::Anchor => tabler!("anchor"),
        BadgeIcon::Router => tabler!("router"),
    }
}

/// An icon's name, for whoever cannot see it.
pub fn icon_name(icon: BadgeIcon) -> String {
    let i18n = use_i18n();
    match icon {
        BadgeIcon::Meshtastic => "Meshtastic".into(),
        BadgeIcon::Meshcore => "MeshCore".into(),
        BadgeIcon::Home => t_string!(i18n, icon_home).to_string(),
        BadgeIcon::Car => t_string!(i18n, icon_car).to_string(),
        BadgeIcon::Bike => t_string!(i18n, icon_bike).to_string(),
        BadgeIcon::Walk => t_string!(i18n, icon_walk).to_string(),
        BadgeIcon::Boat => t_string!(i18n, icon_boat).to_string(),
        BadgeIcon::Plane => t_string!(i18n, icon_plane).to_string(),
        BadgeIcon::Tower => t_string!(i18n, icon_tower).to_string(),
        BadgeIcon::Antenna => t_string!(i18n, icon_antenna).to_string(),
        BadgeIcon::Mountain => t_string!(i18n, icon_mountain).to_string(),
        BadgeIcon::Tent => t_string!(i18n, icon_tent).to_string(),
        BadgeIcon::Backpack => t_string!(i18n, icon_backpack).to_string(),
        BadgeIcon::Briefcase => t_string!(i18n, icon_briefcase).to_string(),
        BadgeIcon::Building => t_string!(i18n, icon_building).to_string(),
        BadgeIcon::Flask => t_string!(i18n, icon_flask).to_string(),
        BadgeIcon::Sun => t_string!(i18n, icon_sun).to_string(),
        BadgeIcon::Star => t_string!(i18n, icon_star).to_string(),
        BadgeIcon::Walkie => t_string!(i18n, icon_walkie).to_string(),
        BadgeIcon::Satellite => t_string!(i18n, icon_satellite).to_string(),
        BadgeIcon::Flag => t_string!(i18n, icon_flag).to_string(),
        BadgeIcon::Bolt => t_string!(i18n, icon_bolt).to_string(),
        BadgeIcon::Tree => t_string!(i18n, icon_tree).to_string(),
        BadgeIcon::Heart => t_string!(i18n, icon_heart).to_string(),
        BadgeIcon::Truck => t_string!(i18n, icon_truck).to_string(),
        BadgeIcon::Motorbike => t_string!(i18n, icon_motorbike).to_string(),
        BadgeIcon::Caravan => t_string!(i18n, icon_caravan).to_string(),
        BadgeIcon::Lighthouse => t_string!(i18n, icon_lighthouse).to_string(),
        BadgeIcon::SolarPanel => t_string!(i18n, icon_solar_panel).to_string(),
        BadgeIcon::Compass => t_string!(i18n, icon_compass).to_string(),
        BadgeIcon::Anchor => t_string!(i18n, icon_anchor).to_string(),
        BadgeIcon::Router => t_string!(i18n, icon_router).to_string(),
    }
}

/// A colour of the palette's name, for whoever cannot see it.
pub fn color_name(name: &str) -> String {
    let i18n = use_i18n();
    match name {
        "amber" => t_string!(i18n, color_amber).to_string(),
        "coral" => t_string!(i18n, color_coral).to_string(),
        "pink" => t_string!(i18n, color_pink).to_string(),
        "grape" => t_string!(i18n, color_grape).to_string(),
        "violet" => t_string!(i18n, color_violet).to_string(),
        "blue" => t_string!(i18n, color_blue).to_string(),
        "cyan" => t_string!(i18n, color_cyan).to_string(),
        "teal" => t_string!(i18n, color_teal).to_string(),
        "lime" => t_string!(i18n, color_lime).to_string(),
        _ => t_string!(i18n, color_gray).to_string(),
    }
}

/// The most a button's text takes, in bytes, as Meshtastic's short names:
/// four letters, fewer with accents, or one emoji.
pub const TEXT_MAX: usize = 4;

/// As much of a text as a button takes, in whole characters.
pub fn fit(text: &str) -> String {
    let mut fitted = String::new();
    for c in text.chars() {
        if fitted.len() + c.len_utf8() > TEXT_MAX {
            break;
        }
        fitted.push(c);
    }
    fitted
}

/// A button's text: an emoji alone at an emoji's size, three or four
/// letters smaller, to fit.
fn text(text: String) -> AnyView {
    let class = if !text.chars().any(char::is_alphanumeric) {
        "emoji"
    } else if text.chars().count() > 2 {
        "glyph long"
    } else {
        "glyph"
    };
    view! { <span class=class>{text}</span> }.into_any()
}

/// What a button shows inside: the box around it is its holder's.
#[component]
pub fn Face(
    #[prop(into)] name: Signal<String>,
    #[prop(into)] badge: Signal<Badge>,
    /// Its node's short name, if it has one: shown when nothing was picked.
    #[prop(into, optional)]
    short: MaybeProp<String>,
) -> impl IntoView {
    move || match badge.get().content {
        BadgeContent::Initials => match short.get() {
            Some(short) => text(short),
            None => view! { <span class="glyph">{initials(&name.get())}</span> }.into_any(),
        },
        BadgeContent::Text { text: chosen } => text(chosen),
        BadgeContent::Emoji { emoji } => view! { <span class="emoji">{emoji}</span> }.into_any(),
        BadgeContent::Icon { icon } => {
            view! { <span class="pict" aria-hidden="true" inner_html=svg(icon)></span> }.into_any()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{initials, ink};

    #[test]
    fn initials_are_two_letters() {
        assert_eq!(initials("casa"), "Ca");
        assert_eq!(initials("Casa de campo"), "Cd");
        assert_eq!(initials("Ñu"), "Ñu");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn the_palette_is_known_by_name_and_as_it_was() {
        assert_eq!(super::palette_name("teal"), Some("teal"));
        assert_eq!(super::palette_name("#F0A63A"), Some("amber"));
        assert_eq!(super::palette_name("#123456"), None);
        let badge = |color: &str| meshctl_api::Badge {
            color: Some(color.into()),
            ..Default::default()
        };
        assert_eq!(
            super::style(&badge("#4dabf7")),
            "--badge: var(--mc-badge-blue); --badge-ink: var(--mc-badge-ink); \
             --badge-on: var(--mc-badge-blue-on); --badge-on-ink: var(--mc-badge-ink-on)"
        );
        assert_eq!(
            super::style(&badge("#1c3d8f")),
            "--badge: color-mix(in srgb, #1c3d8f 50%, var(--mc-rail)); \
             --badge-ink: #ffffff; --badge-on: #1c3d8f; --badge-on-ink: #ffffff"
        );
    }

    #[test]
    fn a_text_fits_in_four_bytes() {
        use super::fit;
        assert_eq!(fit("G3L0 M0V1L"), "G3L0");
        assert_eq!(fit("ÑANDÚ"), "ÑAN");
        assert_eq!(fit("🦊M"), "🦊");
        assert_eq!(fit("M🦊"), "M");
    }

    #[test]
    fn text_reads_on_its_colour() {
        assert_eq!(ink("#f0a63a"), Some("#12161c"));
        assert_eq!(ink("#1c3d8f"), Some("#ffffff"));
        assert_eq!(ink("#000000"), Some("#ffffff"));
        assert_eq!(ink("red"), None);
    }
}
