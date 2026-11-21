// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! How the interface looks and speaks. The daemon keeps it, so every browser
//! that opens meshctl finds it as it was left, and each one open changes with
//! it at once.

use crate::app::Live;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use meshctl_api::{Accent, Language, MapTheme, Preferences, Theme};

/// The accents to come, as they will look: for now, not to be picked.
const ACCENTS_TO_COME: [&str; 5] = ["#4dabf7", "#38d9a9", "#9775fa", "#f06595", "#ff8a6b"];

#[component]
pub fn PreferencesView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let current = move || live.preferences.get();
    // Each option changes one preference, and leaves the rest as they are.
    let choose = move |edit: Box<dyn Fn(&mut Preferences) + Send + Sync>| {
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
        let pick = choose(Box::new(move |p| p.theme = theme));
        (
            Signal::derive(move || current().theme == theme),
            Box::new(pick) as Box<dyn Fn() + Send + Sync>,
            label.into_any(),
        )
    });
    let maps = [MapTheme::Page, MapTheme::Light, MapTheme::Dark].map(|map| {
        let label = move || match map {
            MapTheme::Page => t!(i18n, map_page).into_any(),
            MapTheme::Light => t!(i18n, map_light).into_any(),
            MapTheme::Dark => t!(i18n, map_dark).into_any(),
        };
        let pick = choose(Box::new(move |p| p.map_theme = map));
        (
            Signal::derive(move || current().map_theme == map),
            Box::new(pick) as Box<dyn Fn() + Send + Sync>,
            label.into_any(),
        )
    });
    // The accent: amber, the only one there is yet; the others, to come,
    // show where they will be.
    let amber = move || current().accent == Accent::Amber;
    let coming = ACCENTS_TO_COME.map(|color| {
        view! {
            <button
                type="button"
                role="radio"
                class="swatch"
                aria-checked="false"
                disabled
                title=move || t_string!(i18n, accent_soon)
                aria-label=move || t_string!(i18n, accent_soon)
                style=format!("--swatch: {color}")
            ></button>
        }
    });

    // A list, for there will be more languages than fit side by side. Each
    // one in its own words.
    let language_value = move || match current().language {
        None => "",
        Some(Language::En) => "en",
        Some(Language::Es) => "es",
    };
    let pick_language = move |ev: leptos::ev::Event| {
        let language = match event_target_value(&ev).as_str() {
            "en" => Some(Language::En),
            "es" => Some(Language::Es),
            _ => None,
        };
        choose(Box::new(move |p| p.language = language))();
    };

    view! {
        <section class="preferences">
            <Row title=t!(i18n, prefs_theme).into_any() hint=t!(i18n, prefs_theme_hint).into_any()>
                {pills(Vec::from(themes))}
            </Row>
            <Row title=t!(i18n, prefs_map).into_any() hint=t!(i18n, prefs_map_hint).into_any()>
                {pills(Vec::from(maps))}
            </Row>
            <Row title=t!(i18n, prefs_accent).into_any() hint=t!(i18n, prefs_accent_hint).into_any()>
                <div class="pref-control accents" role="radiogroup">
                    <button
                        type="button"
                        role="radio"
                        class="swatch"
                        class:on=amber
                        aria-checked=move || amber().to_string()
                        title=move || t_string!(i18n, accent_amber)
                        aria-label=move || t_string!(i18n, accent_amber)
                        style="--swatch: #f0a63a"
                        on:click=move |_| choose(Box::new(|p| p.accent = Accent::Amber))()
                    ></button>
                    {coming.into_iter().collect_view()}
                </div>
            </Row>
            <Row
                title=t!(i18n, prefs_language).into_any()
                hint=t!(i18n, prefs_language_hint).into_any()
            >
                <select class="pref-control" prop:value=language_value on:change=pick_language>
                    <option value="">{t!(i18n, language_browser)}</option>
                    <option value="en">"English"</option>
                    <option value="es">"Español"</option>
                </select>
            </Row>
        </section>
    }
}

/// One preference: what it is, on the left, and its control on the right.
#[component]
fn Row(title: AnyView, hint: AnyView, children: Children) -> impl IntoView {
    view! {
        <div class="pref-row">
            <div class="pref-what">
                <span class="pref-title">{title}</span>
                <span class="dim">{hint}</span>
            </div>
            {children()}
        </div>
    }
}

/// One option: whether it is the one chosen, choosing it, and its name.
pub type Choice = (Signal<bool>, Box<dyn Fn() + Send + Sync>, AnyView);

/// A few options, of which one is chosen, under a pill that slides to it.
pub fn pills(options: Vec<Choice>) -> impl IntoView {
    let n = options.len();
    let ons: Vec<Signal<bool>> = options.iter().map(|(on, _, _)| *on).collect();
    let at = move || ons.iter().position(|on| on.get()).unwrap_or(0);
    let buttons = options
        .into_iter()
        .map(|(on, pick, label)| {
            view! {
                <button
                    type="button"
                    role="radio"
                    class:on=move || on.get()
                    aria-checked=move || on.get().to_string()
                    on:click=move |_| pick()
                >
                    {label}
                </button>
            }
        })
        .collect_view();
    view! {
        <div
            class="segmented pref-control"
            role="radiogroup"
            style=move || format!("--n: {n}; --at: {}", at())
        >
            {buttons}
        </div>
    }
}
