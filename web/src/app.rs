// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use crate::i18n::{I18nContextProvider, Locale, t, use_i18n};
use gloo_net::http::Request;
use leptos::prelude::*;
use meshctl_api::{API_V1, Version};

#[component]
pub fn App() -> impl IntoView {
    view! {
        <I18nContextProvider>
            <Shell />
        </I18nContextProvider>
    }
}

#[component]
fn Shell() -> impl IntoView {
    let i18n = use_i18n();
    let version = LocalResource::new(fetch_version);

    let switch_language = move |_| {
        let next = match i18n.get_locale() {
            Locale::en => Locale::es,
            Locale::es => Locale::en,
        };
        i18n.set_locale(next);
    };

    view! {
        <header>
            <h1>"meshctl"</h1>
            <button on:click=switch_language>{t!(i18n, other_language)}</button>
        </header>
        <main>
            <p>{t!(i18n, tagline)}</p>
            <Suspense fallback=move || view! { <p>{t!(i18n, connecting)}</p> }>
                {move || {
                    version
                        .get()
                        .map(|answer| match answer {
                            Some(Version { name, version }) => {
                                view! { <p>{t!(i18n, served_by, name, version)}</p> }.into_any()
                            }
                            None => view! { <p>{t!(i18n, unreachable)}</p> }.into_any(),
                        })
                }}
            </Suspense>
        </main>
    }
}

/// What the daemon says it is, or `None` if it does not answer sensibly.
async fn fetch_version() -> Option<Version> {
    let response = Request::get(&format!("{API_V1}/version"))
        .send()
        .await
        .ok()?;
    if !response.ok() {
        return None;
    }
    response.json().await.ok()
}
