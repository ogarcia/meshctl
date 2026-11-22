// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Traceroutes: the routes traced to a node, hop by hop.

use crate::app::Live;

use crate::i18n::{Locale, t, t_string, use_i18n};
use crate::mesh::ago;
use leptos::prelude::*;
use leptos_i18n::I18nContext;
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

    // An answered one, as text to paste anywhere.
    let copy = (traceroute.status == TracerouteStatus::Done).then(|| {
        let traceroute = traceroute.clone();
        view! {
            <button
                class="icon-button copy"
                title=move || t_string!(i18n, trace_copy)
                aria-label=move || t_string!(i18n, trace_copy)
                inner_html=COPY
                on:click=move |_| crate::clipboard::copy(&as_text(live, i18n, &traceroute))
            ></button>
        }
    });

    view! {
        <article class="trace">
            <header>
                <strong>{move || node_title(live, target)}</strong>
                {status}
                <span class="dim when">{move || ago(live.now.get(), at)}</span>
                {copy}
            </header>
            {routes}
        </article>
    }
}

const COPY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/copy.svg");

/// A traced route as text: whom to, when, and each way node by node, each
/// with the SNR it heard the one before at.
///
/// ```text
/// Traza a KMesh7 (!98127f33) · 22/11/2026 12:48
/// Ida (2): ART1 → stlo (11.2 dB) → AERO (12.0 dB) → KMs7 (-6.8 dB)
/// Vuelta (3): KMs7 → z4p4 (-7.2 dB) → AERO (0.5 dB) → stlo (11.8 dB) → ART1 (12.2 dB)
/// ```
fn as_text(live: Live, i18n: I18nContext<Locale>, traceroute: &Traceroute) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(
        traceroute.at as f64 * 1000.0,
    ));
    let when = format!(
        "{:02}/{:02}/{} {:02}:{:02}",
        date.get_date(),
        date.get_month() + 1,
        date.get_full_year(),
        date.get_hours(),
        date.get_minutes()
    );
    let way = |hops: &[Hop]| {
        hops.iter()
            .enumerate()
            .map(|(i, hop)| {
                let name = hop
                    .node
                    .map_or_else(|| "?".to_owned(), |num| node_label(live, num));
                match hop.snr {
                    Some(snr) if i > 0 => format!("{name} ({snr:.1} dB)"),
                    None if i > 0 => format!("{name} (?)"),
                    _ => name,
                }
            })
            .collect::<Vec<_>>()
            .join(" → ")
    };
    let title =
        t_string!(i18n, trace_text_title).replace("{name}", &node_title(live, traceroute.target));
    format!(
        "{title} · {when}\n{} ({}): {}\n{} ({}): {}",
        t_string!(i18n, trace_towards),
        traceroute.towards.len().saturating_sub(2),
        way(&traceroute.towards),
        t_string!(i18n, trace_back),
        traceroute.back.len().saturating_sub(2),
        way(&traceroute.back),
    )
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
