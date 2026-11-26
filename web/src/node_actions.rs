// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What a Meshtastic node can be told to do, as its admin: the radio
//! itself, or a node administered over the mesh. Reboot, shut down, forget
//! the nodes it knows, or go back to its factory settings; each asked
//! first, the last more firmly, and said once the node took it.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::NodeAction;

/// The section with what the node can be told to do.
#[component]
pub fn ActionsPanel() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let node = crate::settings::node_of();
    // The one on its way, and how the last went.
    let sending = RwSignal::new(None::<NodeAction>);
    let outcome = RwSignal::new(None::<Result<NodeAction, Failure>>);
    let act = move |action: NodeAction| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        let (what, hint) = untrack(|| words(i18n, node, action));
        let mut asked = format!("{what}: {hint}");
        if node.is_some() && action == NodeAction::FactoryReset {
            asked.push_str("\n\n");
            asked.push_str(t_string!(i18n, action_factory_remote));
        }
        asked.push_str("\n\n");
        asked.push_str(t_string!(i18n, action_confirm));
        let sure = web_sys::window()
            .and_then(|w| w.confirm_with_message(&asked).ok())
            .unwrap_or(false);
        if !sure {
            return;
        }
        sending.set(Some(action));
        outcome.set(None);
        spawn_local(async move {
            let done = daemon::act(radio, node, action).await;
            let _ = outcome.try_set(Some(done.map(|()| action)));
            let _ = sending.try_set(None);
        });
    };
    let row = move |action: NodeAction| {
        // In the language shown, as it changes.
        let what = move || words(i18n, node, action).0;
        let hint = move || words(i18n, node, action).1;
        let danger = action == NodeAction::FactoryReset;
        view! {
            <div class="pref-row">
                <div class="pref-what">
                    <span class="pref-title">{what}</span>
                    <span class="dim">{hint}</span>
                </div>
                <button
                    type="button"
                    class:danger-outline=danger
                    disabled=move || sending.get().is_some()
                    on:click=move |_| act(action)
                >
                    {what}
                </button>
            </div>
        }
    };
    view! {
        <div class="section-head">
            <h2>{move || crate::names::section(i18n.get_locale(), "actions")}</h2>
        </div>
        <div class="preferences node-actions">
            {row(NodeAction::Reboot)}
            {row(NodeAction::Shutdown)}
            {row(NodeAction::ForgetNodes)}
            {row(NodeAction::FactoryReset)}
        </div>
        <p class="dim admin-waiting" class:gone=move || sending.get().is_none()>
            <span class="spinner" aria-hidden="true"></span>
            {move || {
                if node.is_some() {
                    t!(i18n, admin_sending).into_any()
                } else {
                    t!(i18n, settings_sending).into_any()
                }
            }}
        </p>
        {move || {
            outcome
                .get()
                .map(|done| match done {
                    Ok(_) => view! { <p class="note">{t!(i18n, action_done)}</p> }.into_any(),
                    Err(failure) => view! {
                        <p class="note bad">{move || crate::node_admin::failed(i18n, &failure)}</p>
                    }
                    .into_any(),
                })
        }}
    }
}

/// An action's name, and what it does, said of the radio or of a node.
fn words(
    i18n: leptos_i18n::I18nContext<crate::i18n::Locale>,
    node: Option<u32>,
    action: NodeAction,
) -> (&'static str, &'static str) {
    match action {
        NodeAction::Reboot => (
            t_string!(i18n, action_reboot),
            t_string!(i18n, action_reboot_hint),
        ),
        NodeAction::Shutdown => (
            t_string!(i18n, action_shutdown),
            if node.is_some() {
                t_string!(i18n, action_shutdown_remote_hint)
            } else {
                t_string!(i18n, action_shutdown_hint)
            },
        ),
        NodeAction::ForgetNodes => (
            t_string!(i18n, action_forget),
            t_string!(i18n, action_forget_hint),
        ),
        NodeAction::FactoryReset => (
            t_string!(i18n, action_factory),
            t_string!(i18n, action_factory_hint),
        ),
    }
}
