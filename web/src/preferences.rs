// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! How the interface looks and speaks. The daemon keeps it, so every browser
//! that opens meshctl finds it as it was left, and each one open changes with
//! it at once.

use crate::app::Live;
use crate::i18n::{t, use_i18n};
use leptos::prelude::*;
use meshctl_api::{Language, MapTheme, Preferences, Theme};

#[component]
pub fn PreferencesView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let current = move || live.preferences.get();
    // Each option changes one preference, and leaves the rest as they are.
    let choose = move |edit: Box<dyn Fn(&mut Preferences)>| {
        move || {
            let mut preferences = live.preferences.get_untracked();
            edit(&mut preferences);
            live.prefer(preferences);
        }
    };

    let themes = [Theme::System, Theme::Light, Theme::Dark].map(|theme| {
        let label = move || match theme {
            Theme::System => t!(i18n, theme_system).into_any(),
            Theme::Light => t!(i18n, theme_light).into_any(),
            Theme::Dark => t!(i18n, theme_dark).into_any(),
        };
        view! {
            <Choice
                on=Signal::derive(move || current().theme == theme)
                pick=choose(Box::new(move |p| p.theme = theme))
            >
                {label}
            </Choice>
        }
    });
    let maps = [MapTheme::Page, MapTheme::Light, MapTheme::Dark].map(|map| {
        let label = move || match map {
            MapTheme::Page => t!(i18n, map_page).into_any(),
            MapTheme::Light => t!(i18n, map_light).into_any(),
            MapTheme::Dark => t!(i18n, map_dark).into_any(),
        };
        view! {
            <Choice
                on=Signal::derive(move || current().map_theme == map)
                pick=choose(Box::new(move |p| p.map_theme = map))
            >
                {label}
            </Choice>
        }
    });
    let routes = [false, true].map(|on_pick| {
        let label = move || {
            if on_pick {
                t!(i18n, routes_picked).into_any()
            } else {
                t!(i18n, routes_asked).into_any()
            }
        };
        view! {
            <Choice
                on=Signal::derive(move || current().map_routes == on_pick)
                pick=choose(Box::new(move |p| p.map_routes = on_pick))
            >
                {label}
            </Choice>
        }
    });
    let languages = [None, Some(Language::En), Some(Language::Es)].map(|language| {
        let label = move || match language {
            None => t!(i18n, language_browser).into_any(),
            // Each language in its own words.
            Some(Language::En) => "English".into_any(),
            Some(Language::Es) => "Español".into_any(),
        };
        view! {
            <Choice
                on=Signal::derive(move || current().language == language)
                pick=choose(Box::new(move |p| p.language = language))
            >
                {label}
            </Choice>
        }
    });

    view! {
        <section class="card preferences">
            <p class="dim">{t!(i18n, prefs_text)}</p>
            <h2>{t!(i18n, prefs_theme)}</h2>
            <div class="choices" role="radiogroup">{themes.into_iter().collect_view()}</div>
            <h2>{t!(i18n, prefs_map)}</h2>
            <div class="choices" role="radiogroup">{maps.into_iter().collect_view()}</div>
            <h2>{t!(i18n, prefs_routes)}</h2>
            <div class="choices" role="radiogroup">{routes.into_iter().collect_view()}</div>
            <h2>{t!(i18n, prefs_language)}</h2>
            <div class="choices" role="radiogroup">{languages.into_iter().collect_view()}</div>
        </section>
    }
}

/// One of a few options, of which one is chosen.
#[component]
fn Choice(on: Signal<bool>, pick: impl Fn() + 'static, children: Children) -> impl IntoView {
    view! {
        <button
            role="radio"
            aria-checked=move || on.get().to_string()
            class=move || if on.get() { "choice on" } else { "choice" }
            on:click=move |_| pick()
        >
            {children()}
        </button>
    }
}
