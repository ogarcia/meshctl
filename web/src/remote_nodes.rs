// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Another Meshtastic node's own list of nodes, administered over the mesh:
//! which it keeps as contacts and favourites, which it ignores, which it
//! forgets. It does not let its list be read, so what it has is not shown:
//! the nodes the radio knows are offered, each with what can be done with
//! it there, and what came of the last thing sent, as the node says it has
//! it, not what it made of it.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{ErrorCode, Node, NodeDbAction, NodeDbChange};
use std::collections::BTreeMap;

/// How many nodes are offered at once: the rest, by searching.
const SHOWN: usize = 30;

/// What came of the last thing sent about a node, or a link: by its number,
/// or 0 for the link.
#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    Sending,
    Done,
    /// Sent, and no acknowledgement came: it may have landed still.
    Unconfirmed,
    Failed(Failure),
}

/// Whether a node's key can be trusted to go as a contact: one there, of
/// 32 bytes, not all zeros, which says two of them were told apart.
fn trusted_key(node: &Node) -> bool {
    node.public_key.as_deref().is_some_and(|key| {
        let key = key.trim_end_matches('=');
        key.len() == 43 && key.chars().any(|c| c != 'A')
    })
}

/// The section of another node's settings with its own list of nodes.
#[component]
pub fn RemoteNodesPanel() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let node = crate::settings::node_of();
    let search = RwSignal::new(String::new());
    let link = RwSignal::new(String::new());
    let outcomes = RwSignal::new(BTreeMap::<u32, Outcome>::new());
    // The one whose actions are shown.
    let open = RwSignal::new(None::<u32>);
    let busy = move || outcomes.with(|o| o.values().any(|o| *o == Outcome::Sending));

    // The radio's own first, then its favourites, then the latest heard;
    // never the node administered itself.
    let candidates = Memo::new(move |_| {
        let needle = search.get().to_lowercase();
        let own = live
            .status
            .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
        let mut nodes: Vec<Node> = live.nodes.with(|nodes| {
            nodes
                .values()
                .filter(|n| Some(n.num) != node)
                .filter(|n| {
                    needle.is_empty()
                        || [n.long_name.as_deref(), n.short_name.as_deref(), Some(&n.id)]
                            .into_iter()
                            .flatten()
                            .any(|text| text.to_lowercase().contains(&needle))
                })
                .cloned()
                .collect()
        });
        nodes.sort_by(|a, b| {
            (Some(b.num) == own)
                .cmp(&(Some(a.num) == own))
                .then(b.favorite.cmp(&a.favorite))
                .then(b.last_heard.cmp(&a.last_heard))
                .then(a.num.cmp(&b.num))
        });
        (own, nodes)
    });

    let send = move |key: u32, change: NodeDbChange| {
        let (Some(radio), Some(node)) = (live.selected.get_untracked(), node) else {
            return;
        };
        outcomes.update(|o| _ = o.insert(key, Outcome::Sending));
        spawn_local(async move {
            let outcome = match daemon::change_node_db(radio, node, &change).await {
                Ok(()) => Outcome::Done,
                Err(Failure::Rejected(ErrorCode::RadioDidNotAnswer)) => Outcome::Unconfirmed,
                Err(failure) => Outcome::Failed(failure),
            };
            let _ = outcomes.try_update(|o| _ = o.insert(key, outcome));
        });
    };
    // Those that cannot be undone, or not as easily, asked first.
    let act = move |target: &Node, action: NodeDbAction| {
        let name = target
            .long_name
            .clone()
            .unwrap_or_else(|| target.id.clone());
        let ask = match action {
            NodeDbAction::Ignore => Some(t_string!(i18n, remote_nodes_ignore_sure)),
            NodeDbAction::Remove => Some(t_string!(i18n, remote_nodes_remove_sure)),
            _ => None,
        };
        if let Some(ask) = ask {
            let asked = ask.replace("{name}", &name);
            let sure = web_sys::window()
                .and_then(|w| w.confirm_with_message(&asked).ok())
                .unwrap_or(false);
            if !sure {
                return;
            }
        }
        send(
            target.num,
            NodeDbChange {
                action,
                node: Some(target.num),
                link: None,
            },
        );
    };

    let said = move |outcome: Outcome| match outcome {
        Outcome::Sending => (t_string!(i18n, remote_nodes_sending).to_owned(), ""),
        Outcome::Done => (t_string!(i18n, remote_nodes_done).to_owned(), "good"),
        Outcome::Unconfirmed => (t_string!(i18n, remote_nodes_unconfirmed).to_owned(), "warn"),
        Outcome::Failed(Failure::Rejected(ErrorCode::InvalidSettings)) => {
            (t_string!(i18n, remote_nodes_invalid).to_owned(), "bad")
        }
        Outcome::Failed(failure) => (crate::node_admin::failed(i18n, &failure), "bad"),
    };
    let outcome_of = move |key: u32| {
        move || {
            outcomes.with(|o| o.get(&key).cloned()).map(|outcome| {
                let (text, class) = said(outcome);
                view! { <p class=format!("remote-outcome {class}")>{text}</p> }
            })
        }
    };

    let row = move |target: Node, own: bool| {
        let num = target.num;
        let keyed = trusted_key(&target);
        let name = target
            .long_name
            .clone()
            .unwrap_or_else(|| target.id.clone());
        let shown = move || open.get() == Some(num);
        let button = {
            let target = target.clone();
            move |action: NodeDbAction, label: &'static str, danger: bool| {
                let target = target.clone();
                view! {
                    <button
                        type="button"
                        class:danger-outline=danger
                        disabled=move || busy() || (action == NodeDbAction::Contact && !keyed)
                        on:click=move |_| act(&target, action)
                    >
                        {label}
                    </button>
                }
            }
        };
        view! {
            <li class="remote-node" class:open=shown>
                <button
                    type="button"
                    class="remote-node-head"
                    aria-expanded=move || shown().to_string()
                    on:click=move |_| open.update(|o| *o = if *o == Some(num) { None } else { Some(num) })
                >
                    <span class="name">{name}</span>
                    <span class="id">{target.id.clone()}</span>
                    {own.then(|| view! { <span class="tag">{t!(i18n, remote_nodes_own)}</span> })}
                    {target.favorite.then(|| view! { <span class="tag">"★"</span> })}
                </button>
                <Show when=shown>
                    <div class="remote-node-actions">
                        {button(NodeDbAction::Contact, t_string!(i18n, remote_nodes_contact), false)}
                        {button(NodeDbAction::Unfavorite, t_string!(i18n, remote_nodes_unfavorite), false)}
                        {button(NodeDbAction::Ignore, t_string!(i18n, remote_nodes_ignore), true)}
                        {button(NodeDbAction::Unignore, t_string!(i18n, remote_nodes_unignore), false)}
                        {button(NodeDbAction::Remove, t_string!(i18n, remote_nodes_remove), true)}
                    </div>
                    {(!keyed).then(|| view! { <p class="dim remote-no-key">{t!(i18n, remote_nodes_no_key)}</p> })}
                </Show>
                {outcome_of(num)}
            </li>
        }
    };

    let send_link = move |_| {
        let url = link.get_untracked().trim().to_owned();
        if url.is_empty() {
            return;
        }
        send(
            0,
            NodeDbChange {
                action: NodeDbAction::Contact,
                node: None,
                link: Some(url),
            },
        );
    };

    view! {
        <div class="section-head">
            <h2>{move || crate::names::section(i18n.get_locale(), "nodes")}</h2>
        </div>
        <div class="remote-nodes">
            <p class="dim remote-intro">{t!(i18n, remote_nodes_intro)}</p>
            <div class="remote-link">
                <input
                    type="text"
                    placeholder=move || t_string!(i18n, remote_nodes_link)
                    prop:value=move || link.get()
                    on:input=move |ev| link.set(event_target_value(&ev))
                />
                <button
                    type="button"
                    disabled=move || busy() || link.with(|l| l.trim().is_empty())
                    on:click=send_link
                >
                    {t!(i18n, remote_nodes_link_send)}
                </button>
            </div>
            {outcome_of(0)}
            <input
                type="search"
                class="remote-search"
                placeholder=move || t_string!(i18n, search)
                prop:value=move || search.get()
                on:input=move |ev| search.set(event_target_value(&ev))
            />
            <ul class="remote-node-list">
                {move || {
                    let (own, nodes) = candidates.get();
                    nodes
                        .into_iter()
                        .take(SHOWN)
                        .map(|n| {
                            let is_own = Some(n.num) == own;
                            row(n, is_own)
                        })
                        .collect_view()
                }}
            </ul>
            {move || {
                let more = candidates.with(|(_, nodes)| nodes.len().saturating_sub(SHOWN));
                (more > 0)
                    .then(|| {
                        view! {
                            <p class="dim">
                                {t_string!(i18n, remote_nodes_more).replace("{n}", &more.to_string())}
                            </p>
                        }
                    })
            }}
        </div>
    }
}
