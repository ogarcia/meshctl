// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! meshctl's mark as the icon of a tab, a bookmark or a notification: in the
//! accent the panel is in, so that it is the colour its user picked, and in
//! a red none of them is for the API's reference, to tell the two apart.

use crate::Accent;

/// The mark in two colours: one where the system is light, one where it is
/// dark, as a browser's own bars are.
pub fn icon(light: &str, dark: &str) -> String {
    format!(
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><style>g{{color:{light}}}"#,
            r#"@media (prefers-color-scheme:dark){{g{{color:{dark}}}}}</style>"#,
            r#"<g fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round">"#,
            r#"<path d="M9.27 18.34L6 26M17.99 15.42L26 20M6 26L26 20M18.27 9.11L25 6"/>"#,
            r#"<g fill="currentColor" stroke="none"><circle cx="6" cy="26" r="4.2"/>"#,
            r#"<circle cx="26" cy="20" r="4.2"/><circle cx="25" cy="6" r="4.2"/>"#,
            r#"<path fill-rule="evenodd" d="M10.72 8.21L10.72 6.65L13.28 6.65L13.28 8.21L14.65 9L15.99 8.21L17.27 10.44L15.92 11.21L15.92 12.79L17.27 13.56L15.99 15.79L14.65 15L13.28 15.79L13.28 17.35L10.72 17.35L10.72 15.79L9.35 15L8.01 15.79L6.73 13.56L8.08 12.79L8.08 11.21L6.73 10.44L8.01 8.21L9.35 9ZM13.7 12A1.7 1.7 0 1 0 10.3 12A1.7 1.7 0 1 0 13.7 12Z"/>"#,
            r#"</g></g></svg>"#,
        ),
        light = light,
        dark = dark,
    )
}

/// The mark in an accent: its colour as the interface's style sheet has it,
/// light and dark.
pub fn accent_icon(accent: Accent) -> String {
    let (light, dark) = match accent {
        Accent::Amber => ("#9a5b00", "#f0a63a"),
        Accent::Blue => ("#1864ab", "#4dabf7"),
        Accent::Teal => ("#087f5b", "#38d9a9"),
        Accent::Violet => ("#6741d9", "#9775fa"),
        Accent::Coral => ("#c0401e", "#ff8a6b"),
        Accent::Gray => ("#343a40", "#dee2e6"),
    };
    icon(light, dark)
}

/// The mark of the API's reference: a deep red, no accent's.
pub fn api_icon() -> String {
    icon("#c2002f", "#ff4d6d")
}

/// An icon as a `data:` address, to put in a page as it is.
pub fn data_url(svg: &str) -> String {
    let mut url = String::from("data:image/svg+xml,");
    for c in svg.chars() {
        match c {
            '%' => url.push_str("%25"),
            '#' => url.push_str("%23"),
            '<' => url.push_str("%3C"),
            '>' => url.push_str("%3E"),
            '"' => url.push('\''),
            c => url.push(c),
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_amber_it_is_the_icon_the_interface_ships() {
        let shipped = include_str!("../../web/favicon.svg");
        assert_eq!(accent_icon(Accent::Amber), shipped.trim_end());
    }

    #[test]
    fn as_an_address_it_needs_no_quoting() {
        let url = data_url(&api_icon());
        assert!(url.starts_with("data:image/svg+xml,%3Csvg xmlns='http://"));
        assert!(!url.contains(['"', '#', '<', '>']));
        assert!(url.contains("%23c2002f"));
    }
}
