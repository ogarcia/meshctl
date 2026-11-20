// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A radio's button: its colour, and what it shows (its initials, one or two
//! characters, an emoji or an icon). The rail, the radios' list and their
//! editor show it the same way, at their own sizes.

use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;
use meshctl_api::{Badge, BadgeContent, BadgeIcon};

/// The colours offered, light enough for dark text and strong enough on
/// both themes' rails. Any other can be picked too.
pub const PALETTE: [&str; 10] = [
    "#f0a63a", "#ff8a6b", "#f06595", "#cc5de8", "#9775fa", "#4dabf7", "#3bc9db", "#38d9a9",
    "#a9e34b", "#adb5bd",
];

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
    badge
        .color
        .as_deref()
        .and_then(|color| Some(format!("--badge: {color}; --badge-ink: {}", ink(color)?)))
        .unwrap_or_default()
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

/// What a button shows inside: the box around it is its holder's.
#[component]
pub fn Face(
    #[prop(into)] name: Signal<String>,
    #[prop(into)] badge: Signal<Badge>,
) -> impl IntoView {
    move || match badge.get().content {
        BadgeContent::Initials => {
            view! { <span class="glyph">{initials(&name.get())}</span> }.into_any()
        }
        BadgeContent::Text { text } => view! { <span class="glyph">{text}</span> }.into_any(),
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
    fn text_reads_on_its_colour() {
        assert_eq!(ink("#f0a63a"), Some("#12161c"));
        assert_eq!(ink("#1c3d8f"), Some("#ffffff"));
        assert_eq!(ink("#000000"), Some("#ffffff"));
        assert_eq!(ink("red"), None);
    }
}
