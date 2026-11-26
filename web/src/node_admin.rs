// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Why another Meshtastic node's settings could not be read from it, as its
//! admin over the mesh: one that does not take the radio's key says what
//! its owner has to do, with the key to give them; one that did not answer,
//! or would not, says so, to try again.

use crate::app::Live;
use crate::daemon::Failure;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use meshctl_api::ErrorCode;

/// Why a node, or the radio, did not tell, in words.
pub fn failed(i18n: leptos_i18n::I18nContext<crate::i18n::Locale>, failure: &Failure) -> String {
    match failure {
        Failure::Rejected(ErrorCode::RadioDidNotAnswer) => t_string!(i18n, admin_no_answer),
        Failure::Rejected(ErrorCode::RadioRefused) => t_string!(i18n, admin_refused),
        Failure::Rejected(ErrorCode::NotAdmin) => t_string!(i18n, admin_not_admin),
        Failure::Rejected(ErrorCode::RadioNotConnected) => t_string!(i18n, chat_not_connected),
        _ => t_string!(i18n, error_failed),
    }
    .to_owned()
}

/// Why a node's settings could not be read, and a way to try again.
#[component]
pub fn Failed(failure: Failure, retry: Callback<()>) -> impl IntoView {
    let i18n = use_i18n();
    let not_admin = failure == Failure::Rejected(ErrorCode::NotAdmin);
    let label = move || t_string!(i18n, admin_retry);
    view! {
        <div class="node-admin">
            <div class="note bad admin-failed">
                <span class="what">{move || failed(i18n, &failure)}</span>
                <button
                    type="button"
                    class="with-icon retry"
                    title=label
                    aria-label=label
                    on:click=move |_| retry.run(())
                >
                    <span class="icon" inner_html=REFRESH></span>
                    <span class="label">{label}</span>
                </button>
            </div>
            {not_admin.then(|| view! { <NotAdmin /> })}
        </div>
    }
}

/// What the owner of a node that does not take the radio's key has to do,
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
        <p class="dim">{t!(i18n, admin_not_admin_hint)}</p>
        {move || {
            let Some(key) = key() else {
                return view! { <p class="dim">{t!(i18n, admin_own_key_unknown)}</p> }.into_any();
            };
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
        }}
    }
}

pub(crate) const REFRESH: &str =
    include_str!("../vendor/tabler-icons-3.51.0/icons/outline/refresh.svg");
const COPY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/copy.svg");
