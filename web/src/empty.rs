// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A page with nothing to show yet, and why: a radio not connected, one
//! switched off, meshctl out of reach. The same look for all of them.

use leptos::prelude::*;

pub const PLUG_OFF: &str =
    include_str!("../vendor/tabler-icons-3.51.0/icons/outline/plug-connected-x.svg");
pub const POWER: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/power.svg");
pub const SERVER_OFF: &str =
    include_str!("../vendor/tabler-icons-3.51.0/icons/outline/server-off.svg");

/// An icon in a circle, what is the matter, a word on it and, if any, what
/// can be done: in the middle of the page, or of its part with `compact`.
#[component]
pub fn EmptyState(
    icon: &'static str,
    title: AnyView,
    text: AnyView,
    #[prop(optional)] compact: bool,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="empty-state" class:compact=compact>
            <span class="empty-icon" inner_html=icon></span>
            <h2>{title}</h2>
            <p class="dim">{text}</p>
            {children.map(|children| view! { <div class="empty-actions">{children()}</div> })}
        </div>
    }
}
