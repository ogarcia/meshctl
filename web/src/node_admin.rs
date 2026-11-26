// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Another Meshtastic node's settings, read from it over the mesh as its
//! admin: the radio's key has to be one of the node's admin keys. What it
//! has is asked as the drawer opens, which says too whether it takes the
//! key; each section, as it is opened. Only shown, for now.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{ErrorCode, SettingsSection};

/// Where asking the node is.
#[derive(Debug, Clone, PartialEq)]
enum Asked {
    Asking,
    Sections(Vec<String>),
    Failed(Failure),
}

/// The drawer with a node's settings, as its admin.
#[component]
pub fn AdminDrawer(num: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let open = live.administering;
    let asked = RwSignal::new(Asked::Asking);
    let ask = move || {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        asked.set(Asked::Asking);
        spawn_local(async move {
            let answer = match daemon::node_sections(radio, num).await {
                Ok(sections) => Asked::Sections(sections),
                Err(failure) => Asked::Failed(failure),
            };
            let _ = asked.try_set(answer);
        });
    };
    // Asked anew each time it opens: it may have been given the key since.
    Effect::new(move |_| {
        if open.get() {
            ask();
        }
    });
    let title = Signal::derive(move || t_string!(i18n, admin_title).to_owned());
    view! {
        <crate::node::Drawer open title>
            <div class="node-admin">
                {move || match asked.get() {
                    Asked::Asking => view! {
                        <p class="dim admin-waiting">
                            <span class="spinner" aria-hidden="true"></span>
                            {t!(i18n, admin_asking)}
                        </p>
                    }
                    .into_any(),
                    Asked::Sections(sections) => view! {
                        <p class="dim">{t!(i18n, admin_hint)}</p>
                        <div class="admin-sections">
                            {sections
                                .into_iter()
                                .map(|id| view! { <SectionRow num id /> })
                                .collect_view()}
                        </div>
                    }
                    .into_any(),
                    Asked::Failed(Failure::Rejected(ErrorCode::NotAdmin)) => {
                        view! { <NotAdmin /> }.into_any()
                    }
                    Asked::Failed(failure) => view! {
                        <p class="note bad">{failed(i18n, &failure)}</p>
                        <button type="button" on:click=move |_| ask()>
                            {t!(i18n, admin_retry)}
                        </button>
                    }
                    .into_any(),
                }}
            </div>
        </crate::node::Drawer>
    }
}

/// Why the node, or the radio, did not tell, in words.
fn failed(i18n: leptos_i18n::I18nContext<crate::i18n::Locale>, failure: &Failure) -> String {
    match failure {
        Failure::Rejected(ErrorCode::RadioDidNotAnswer) => t_string!(i18n, admin_no_answer),
        Failure::Rejected(ErrorCode::RadioRefused) => t_string!(i18n, admin_refused),
        Failure::Rejected(ErrorCode::RadioNotConnected) => t_string!(i18n, chat_not_connected),
        _ => t_string!(i18n, error_failed),
    }
    .to_owned()
}

/// A node that does not take the radio's key: what its owner has to do,
/// and the key, to give them.
#[component]
fn NotAdmin() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let key = move || {
        let num = live
            .status
            .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num))?;
        live.nodes
            .with(|nodes| nodes.get(&num).and_then(|n| n.public_key.clone()))
    };
    view! {
        <p class="note bad">{t!(i18n, admin_not_admin)}</p>
        <p class="dim">{t!(i18n, admin_not_admin_hint)}</p>
        {move || {
            let Some(key) = key() else {
                return view! { <p class="dim">{t!(i18n, admin_own_key_unknown)}</p> }.into_any();
            };
            {
                let copied = key.clone();
                view! {
                    <span class="copyable admin-key">
                        <span class="mono public-key" title=copied.clone()>{key}</span>
                        <button
                            class="icon-button"
                            title=move || t_string!(i18n, node_key_copy)
                            aria-label=move || t_string!(i18n, node_key_copy)
                            inner_html=COPY
                            on:click=move |_| crate::clipboard::copy(&copied)
                        ></button>
                    </span>
                }
                .into_any()
            }
        }}
    }
}

/// Where asking for a section is.
#[derive(Debug, Clone, PartialEq)]
enum Section {
    Unasked,
    Asking,
    Got(SettingsSection),
    Failed(Failure),
}

/// A section of the node's settings, by its name, read from it as it is
/// opened, and shown as the radio's own are.
#[component]
fn SectionRow(num: u32, id: String) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let open = RwSignal::new(false);
    let read = RwSignal::new(Section::Unasked);
    let name = id
        .split_once('.')
        .map_or(id.as_str(), |(_, n)| n)
        .to_owned();
    let read_it = {
        let id = id.clone();
        move || {
            let Some(radio) = live.selected.get_untracked() else {
                return;
            };
            read.set(Section::Asking);
            let id = id.clone();
            spawn_local(async move {
                let answer = match daemon::node_section(radio, num, &id).await {
                    Ok(section) => Section::Got(section),
                    Err(failure) => Section::Failed(failure),
                };
                let _ = read.try_set(answer);
            });
        }
    };
    let toggle = {
        let read_it = read_it.clone();
        move |_| {
            open.update(|o| *o = !*o);
            if open.get_untracked()
                && matches!(read.get_untracked(), Section::Unasked | Section::Failed(_))
            {
                read_it();
            }
        }
    };
    // On or off, once read, for those that say.
    let state = move || match read.get() {
        Section::Got(section) => crate::sections::enabled(&section),
        _ => None,
    };
    // Nothing to change, for now: what it is changed in is not kept.
    let edits: crate::sections::SectionEdits = RwSignal::new(Default::default());
    view! {
        <div class="admin-section" class:open=move || open.get()>
            <button type="button" class="admin-section-head" on:click=toggle>
                <span class="chevron" aria-hidden="true" inner_html=CHEVRON></span>
                <span class="label">
                    {move || crate::names::section(i18n.get_locale(), &name)}
                </span>
                {move || {
                    state().map(|on| {
                        view! {
                            <span class=if on { "badge ready" } else { "badge disabled" }>
                                {if on {
                                    t!(i18n, section_on).into_any()
                                } else {
                                    t!(i18n, section_off).into_any()
                                }}
                            </span>
                        }
                    })
                }}
            </button>
            <Show when=move || open.get()>
                <div class="admin-section-body">
                    {
                        let read_it = read_it.clone();
                        move || match read.get() {
                            Section::Unasked | Section::Asking => view! {
                                <p class="dim admin-waiting">
                                    <span class="spinner" aria-hidden="true"></span>
                                    {t!(i18n, admin_reading)}
                                </p>
                            }
                            .into_any(),
                            Section::Got(section) => {
                                view! { <crate::sections::SectionForm section edits read_only=true /> }
                                    .into_any()
                            }
                            Section::Failed(failure) => {
                                let read_it = read_it.clone();
                                view! {
                                    <p class="note bad">{failed(i18n, &failure)}</p>
                                    <button type="button" on:click=move |_| read_it()>
                                        {t!(i18n, admin_retry)}
                                    </button>
                                }
                                .into_any()
                            }
                        }
                    }
                </div>
            </Show>
        </div>
    }
}

const COPY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/copy.svg");
const CHEVRON: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-down.svg");
