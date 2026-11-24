// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What meshctl is, which version, where it comes from and who makes it: a
//! window opened from the foot of meshctl's settings, or of a phone's
//! "More".

use crate::app::Live;
use crate::daemon;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

const REPOSITORY: &str = "https://github.com/ogarcia/meshctl";
const CLOSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/x.svg");
const WEBSITE: &str = "https://meshctl.ogarcia.me";
const WORLD: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/world.svg");
const INFO: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/info-circle.svg");
const GITHUB: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/brand-github.svg");
const BUG: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/bug.svg");
const FLAG: &str = include_str!("../icons/flag-eu.svg");

/// meshctl's logo: the mesh, its main node a gear, as assets/icons/meshctl.svg
/// draws it, in the colour of the text around.
#[component]
pub fn Logo(#[prop(optional)] class: &'static str) -> impl IntoView {
    view! {
        <svg class=class viewBox="0 0 32 32" aria-hidden="true">
            <path
                stroke-width="2.2"
                d="M9.31 18.25L6 26M17.9 15.37L26 20M6 26L26 20M18.17 9.15L25 6"
            ></path>
            <g fill="currentColor" stroke="none">
                <circle cx="6" cy="26" r="4.1"></circle>
                <circle cx="26" cy="20" r="4.1"></circle>
                <circle cx="25" cy="6" r="4.1"></circle>
                <path
                    fill-rule="evenodd"
                    d="M11.07 8.01L11.11 6.67L12.89 6.67L12.93 8.01L14.17 8.52L15.14 7.61L16.39 8.86L15.48 9.83L15.99 11.07L17.33 11.11L17.33 12.89L15.99 12.93L15.48 14.17L16.39 15.14L15.14 16.39L14.17 15.48L12.93 15.99L12.89 17.33L11.11 17.33L11.07 15.99L9.83 15.48L8.86 16.39L7.61 15.14L8.52 14.17L8.01 12.93L6.67 12.89L6.67 11.11L8.01 11.07L8.52 9.83L7.61 8.86L8.86 7.61L9.83 8.52ZM13.6 12A1.6 1.6 0 1 0 10.4 12A1.6 1.6 0 1 0 13.6 12Z"
                ></path>
            </g>
        </svg>
    }
}

/// Asks the daemon for meshctl's version, and the commit it was built from,
/// once: it knows them, not the page.
fn ask_version(live: Live) {
    if live.version.get_untracked().is_some() {
        return;
    }
    spawn_local(async move {
        if let Ok(read) = daemon::version().await {
            let shown = match read.revision {
                Some(revision) => format!("{} ({revision})", read.version),
                None => read.version,
            };
            live.version.set(Some(shown));
        }
    });
}

/// The foot of meshctl's settings, and of a phone's "More": its logo, its
/// version, and the way to the about window.
#[component]
pub fn AboutFoot() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    ask_version(live);
    view! {
        <footer class="local-foot about-foot">
            <button
                class="own-node"
                title=move || t_string!(i18n, about_title)
                on:click=move |_| live.about.set(true)
            >
                <Logo class="foot-logo" />
                <span class="who">
                    <b>"meshctl"</b>
                    <span class="mono dim">{move || live.version.get().unwrap_or_default()}</span>
                </span>
                <span class="info" aria-hidden="true" inner_html=INFO></span>
            </button>
        </footer>
    }
}

/// The about window, while it is open: Escape, its ✕ or a click off it
/// close it.
#[component]
pub fn AboutDialog() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    Effect::new(move |_| {
        if live.about.get() {
            ask_version(live);
        }
    });
    let version = live.version;
    let close = move || live.about.set(false);
    let escaping = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" && live.about.get_untracked() {
            close();
        }
    });
    on_cleanup(move || escaping.remove());

    move || {
        live.about.get().then(|| {
            view! {
                <div class="modal-back" on:click=move |_| close()>
                    <div
                        class="card about"
                        role="dialog"
                        aria-modal="true"
                        aria-labelledby="about-title"
                        on:click=|ev| ev.stop_propagation()
                    >
                        <button
                            class="icon-button close"
                            title=move || t_string!(i18n, drawer_close)
                            aria-label=move || t_string!(i18n, drawer_close)
                            inner_html=CLOSE
                            on:click=move |_| close()
                        ></button>
                        <Logo class="about-logo" />
                        <h2 id="about-title">"meshctl"</h2>
                        <p class="about-version">{move || version.get().unwrap_or_default()}</p>
                        <p>{t!(i18n, about_text)}</p>
                        <nav class="about-links" aria-label=move || t_string!(i18n, about_title)>
                            <a class="button" href=WEBSITE target="_blank" rel="noopener">
                                <span class="icon" inner_html=WORLD></span>
                                {t!(i18n, about_website)}
                            </a>
                            <a class="button" href=REPOSITORY target="_blank" rel="noopener">
                                <span class="icon" inner_html=GITHUB></span>
                                "GitHub"
                            </a>
                            <a
                                class="button"
                                href=format!("{REPOSITORY}/issues")
                                target="_blank"
                                rel="noopener"
                            >
                                <span class="icon" inner_html=BUG></span>
                                {t!(i18n, about_issues)}
                            </a>
                        </nav>
                        <p class="about-small">
                            <a
                                href=format!("{REPOSITORY}/blob/master/LICENSE")
                                target="_blank"
                                rel="noopener"
                            >
                                {t!(i18n, about_license)}
                            </a>
                        </p>
                        <p class="about-small">
                            {t!(i18n, about_made_with)} " "
                            <span
                                class="about-heart"
                                role="img"
                                aria-label=move || t_string!(i18n, about_love)
                            >
                                "♥"
                            </span>
                            " " {t!(i18n, about_made_in)} " "
                            <span class="about-flag" aria-hidden="true" inner_html=FLAG></span>
                            " " {t!(i18n, about_by)} " "
                            <span class="about-name">"Óscar García Amor"</span>
                        </p>
                    </div>
                </div>
            }
        })
    }
}
