// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Traceroutes: tracing the route to a node, and the routes traced.

use crate::app::Live;
use crate::daemon::Failure;
use crate::i18n::{t, use_i18n};
use crate::mesh::ago;
use leptos::prelude::*;
use meshctl_api::{ErrorCode, Hop, LinkState, Traceroute, TracerouteStatus};

fn node_label(live: Live, num: u32) -> String {
    live.nodes.with(|nodes| {
        nodes
            .get(&num)
            .and_then(|n| n.short_name.clone().or_else(|| n.long_name.clone()))
            .unwrap_or_else(|| format!("!{num:08x}"))
    })
}

fn node_title(live: Live, num: u32) -> String {
    live.nodes.with(|nodes| {
        let id = format!("!{num:08x}");
        match nodes.get(&num).and_then(|n| n.long_name.clone()) {
            Some(name) => format!("{name} ({id})"),
            None => id,
        }
    })
}

#[component]
pub fn TracesView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let supported = move || live.selected_kind() != Some(meshctl_api::RadioKind::Meshcore);
    view! {
        <Show
            when=supported
            fallback=move || view! { <p class="card dim">{t!(i18n, meshcore_not_yet)}</p> }
        >
            <TraceForm />
        </Show>
        <section class="card traces">
            <For
                each=move || live.traceroutes.get()
                key=|t| (t.id, t.status)
                let:traceroute
            >
                <TraceCard traceroute />
            </For>
        </section>
    }
}

#[component]
fn TraceForm() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let chosen = RwSignal::new(None::<u32>);
    let local = move || {
        live.status
            .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num))
    };
    // The nodes heard, by name; a route can only be traced to someone else.
    let options = move || {
        let me = local();
        let mut nodes: Vec<(u32, String)> = live.nodes.with(|nodes| {
            nodes
                .values()
                .filter(|n| Some(n.num) != me)
                .map(|n| (n.num, node_title(live, n.num)))
                .collect()
        });
        nodes.sort_by_key(|(_, name)| name.to_lowercase());
        nodes
    };
    let connected = move || {
        live.status
            .with(|s| s.as_ref().is_some_and(|s| s.link == LinkState::Ready))
    };
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if let Some(node) = chosen.get_untracked() {
            live.trace(node);
        }
    };
    let problem = move || {
        if !connected() {
            return Some(t!(i18n, trace_not_connected).into_any());
        }
        live.trace_error.get().map(|failure| match failure {
            Failure::Rejected(ErrorCode::TooSoon) => t!(i18n, trace_too_soon).into_any(),
            Failure::Rejected(ErrorCode::RadioNotConnected) => {
                t!(i18n, trace_not_connected).into_any()
            }
            _ => t!(i18n, error_failed).into_any(),
        })
    };

    view! {
        <form class="card trace-form" on:submit=submit>
            <h2>{t!(i18n, trace_title)}</h2>
            <p class="dim">{t!(i18n, trace_text)}</p>
            <div class="row">
                <select on:change=move |ev| chosen.set(event_target_value(&ev).parse().ok())>
                    <option value="" selected=move || chosen.get().is_none()>
                        {t!(i18n, trace_pick)}
                    </option>
                    <For each=options key=|(num, name)| (*num, name.clone()) let:option>
                        <option
                            value=option.0.to_string()
                            selected=move || chosen.get() == Some(option.0)
                        >
                            {option.1.clone()}
                        </option>
                    </For>
                </select>
                <button
                    type="submit"
                    class="primary"
                    disabled=move || chosen.get().is_none() || !connected()
                >
                    {t!(i18n, trace_send)}
                </button>
            </div>
            <p class="error">{problem}</p>
        </form>
    }
}

#[component]
fn TraceCard(traceroute: Traceroute) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let at = u32::try_from(traceroute.at).unwrap_or_default();
    let target = traceroute.target;
    let status = match traceroute.status {
        TracerouteStatus::Pending => view! {
            <span class="badge syncing">{t!(i18n, trace_pending)}</span>
        }
        .into_any(),
        TracerouteStatus::Done => view! {
            <span class="badge ready">{t!(i18n, trace_done)}</span>
        }
        .into_any(),
        TracerouteStatus::Failed => {
            let why = match traceroute.error.as_deref() {
                Some("TIMEOUT") => t!(i18n, trace_timeout).into_any(),
                Some(code) => code.to_owned().into_any(),
                None => ().into_any(),
            };
            view! {
                <span class="badge radio-down">{t!(i18n, trace_failed)} " · " {why}</span>
            }
            .into_any()
        }
    };
    let routes = (traceroute.status == TracerouteStatus::Done).then(|| {
        let towards = traceroute.towards.clone();
        let back = traceroute.back.clone();
        let hops_there = towards.len().saturating_sub(2);
        let hops_back = back.len().saturating_sub(2);
        view! {
            <div class="route">
                <span class="label dim">{t!(i18n, trace_towards)} " (" {hops_there} ")"</span>
                <Route hops=towards />
            </div>
            <div class="route">
                <span class="label dim">{t!(i18n, trace_back)} " (" {hops_back} ")"</span>
                <Route hops=back />
            </div>
        }
    });

    view! {
        <article class="trace">
            <header>
                <strong>{move || node_title(live, target)}</strong>
                {status}
                <span class="dim when">{move || ago(live.now.get(), at)}</span>
            </header>
            {routes}
        </article>
    }
}

/// A route, node by node, with the SNR each hop was heard at between them.
#[component]
fn Route(hops: Vec<Hop>) -> impl IntoView {
    let live = expect_context::<Live>();
    let steps = hops
        .into_iter()
        .enumerate()
        .map(|(i, hop)| {
            let link = (i > 0).then(|| {
                let snr = hop
                    .snr
                    .map_or_else(|| "?".to_owned(), |s| format!("{s:.1} dB"));
                let quality = match hop.snr {
                    Some(s) if s >= 5.0 => "good",
                    Some(s) if s >= -5.0 => "fair",
                    Some(_) => "poor",
                    None => "unknown",
                };
                view! { <span class=format!("hop {quality}")>"→ " {snr} " →"</span> }
            });
            let name = hop
                .node
                .map_or_else(|| "?".to_owned(), |num| node_label(live, num));
            let title = hop.node.map(|num| node_title(live, num));
            view! {
                {link}
                <span class="short" title=title>{name}</span>
            }
        })
        .collect_view();
    view! { <div class="path">{steps}</div> }
}
