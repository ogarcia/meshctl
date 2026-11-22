// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Traceroutes: the routes traced to a node, hop by hop.

use crate::app::Live;

use crate::i18n::{t, use_i18n};
use crate::mesh::ago;
use leptos::prelude::*;
use meshctl_api::{Hop, Traceroute, TracerouteStatus};

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

/// How well a hop was heard: `good`, `fair`, `poor`, or `unknown`.
pub fn quality(snr: Option<f32>) -> &'static str {
    match snr {
        Some(s) if s >= 5.0 => "good",
        Some(s) if s >= -5.0 => "fair",
        Some(_) => "poor",
        None => "unknown",
    }
}

#[component]
pub fn TraceCard(traceroute: Traceroute) -> impl IntoView {
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
                let quality = quality(hop.snr);
                view! { <span class=format!("hop {quality}")>"→ " {snr} " →"</span> }
            });
            // Named as the radio knows them now: a name learnt later shows.
            let num = hop.node;
            let name = move || num.map_or_else(|| "?".to_owned(), |num| node_label(live, num));
            let title = move || num.map(|num| node_title(live, num));
            view! {
                {link}
                <span class="short" title=title>{name}</span>
            }
        })
        .collect_view();
    view! { <div class="path">{steps}</div> }
}
