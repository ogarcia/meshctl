// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What the selected radio knows: itself, and the nodes it hears.

use crate::app::Live;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use meshctl_api::{LinkState, Node};
use std::collections::BTreeMap;

#[component]
pub fn MeshView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let disabled = move || {
        live.status
            .get()
            .is_some_and(|status| status.link == LinkState::Disabled)
    };
    view! {
        <Show
            when=move || !disabled()
            fallback=move || view! { <p class="card dim">{t!(i18n, radio_disabled_note)}</p> }
        >
            <LocalCard />
            <NodeList />
        </Show>
    }
}

/// The radio itself.
#[component]
fn LocalCard() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let local = move || live.status.get().and_then(|s| s.local);
    let name = move || {
        let local = local()?;
        let me = live.nodes.with(|nodes| nodes.get(&local.num).cloned());
        Some(me.and_then(|n| n.long_name).unwrap_or(local.id))
    };

    view! {
        <Show when=move || local().is_some()>
            <section class="card local">
                <h2>{name}</h2>
                <dl>
                    <dt>{t!(i18n, local_id)}</dt>
                    <dd class="mono">{move || local().map(|l| l.id)}</dd>
                    <dt>{t!(i18n, local_hardware)}</dt>
                    <dd>{move || local().and_then(|l| l.hw_model)}</dd>
                    <dt>{t!(i18n, local_firmware)}</dt>
                    <dd class="mono">{move || local().and_then(|l| l.firmware_version)}</dd>
                    <dt>{t!(i18n, local_role)}</dt>
                    <dd>{move || local().and_then(|l| l.role)}</dd>
                    <dt>{t!(i18n, local_radio)}</dt>
                    <dd>
                        {move || {
                            local()
                                .map(|l| {
                                    [l.region, l.modem_preset]
                                        .into_iter()
                                        .flatten()
                                        .collect::<Vec<_>>()
                                        .join(" · ")
                                })
                        }}
                    </dd>
                </dl>
            </section>
        </Show>
    }
}

#[component]
fn NodeList() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let filter = RwSignal::new(String::new());

    let shown = move || {
        let needle = filter.get().to_lowercase();
        let mut nodes: Vec<Node> = live.nodes.with(|nodes| {
            nodes
                .values()
                .filter(|node| needle.is_empty() || matches(node, &needle))
                .cloned()
                .collect()
        });
        nodes.sort_by(|a, b| b.last_heard.cmp(&a.last_heard).then(a.num.cmp(&b.num)));
        nodes
    };
    let total = move || live.nodes.with(BTreeMap::len);

    view! {
        <section class="card nodes">
            <div class="toolbar">
                <h2>{t!(i18n, nodes_title)} " " <span class="count">{total}</span></h2>
                <input
                    type="search"
                    placeholder=move || t_string!(i18n, search)
                    prop:value=move || filter.get()
                    on:input=move |ev| filter.set(event_target_value(&ev))
                />
            </div>
            <div class="scroll">
                <table>
                    <thead>
                        <tr>
                            <th>{t!(i18n, nodes_name)}</th>
                            <th>{t!(i18n, nodes_id)}</th>
                            <th class="num">{t!(i18n, nodes_heard)}</th>
                            <th class="num">{t!(i18n, nodes_hops)}</th>
                            <th class="num">"SNR"</th>
                            <th class="num">{t!(i18n, nodes_battery)}</th>
                            <th>{t!(i18n, nodes_hardware)}</th>
                        </tr>
                    </thead>
                    <tbody>
                        <For each=shown key=|node| (node.num, node.last_heard) let:node>
                            <NodeRow node now=live.now />
                        </For>
                    </tbody>
                </table>
            </div>
        </section>
    }
}

fn matches(node: &Node, needle: &str) -> bool {
    [
        node.long_name.as_deref(),
        node.short_name.as_deref(),
        Some(&node.id),
    ]
    .into_iter()
    .flatten()
    .any(|text| text.to_lowercase().contains(needle))
}

#[component]
fn NodeRow(node: Node, now: RwSignal<u32>) -> impl IntoView {
    let heard = node.last_heard;
    let hops = if node.via_mqtt {
        "MQTT".to_owned()
    } else {
        node.hops_away.map(|h| h.to_string()).unwrap_or_default()
    };
    // Heard through others, the SNR is the last relay's, not this node's.
    let snr = node
        .snr
        .filter(|_| node.hops_away == Some(0) && !node.via_mqtt)
        .map(|s| format!("{s:.1} dB"))
        .unwrap_or_default();
    let battery = match node.battery_level {
        Some(level) if level > 100 => "⚡".to_owned(),
        Some(level) => format!("{level} %"),
        None => String::new(),
    };

    view! {
        <tr>
            <td>
                <span class="short">{node.short_name.clone()}</span>
                " "
                {node.long_name.clone()}
            </td>
            <td class="mono">{node.id.clone()}</td>
            <td class="num">{move || heard.map(|t| ago(now.get(), t)).unwrap_or_default()}</td>
            <td class="num">{hops}</td>
            <td class="num">{snr}</td>
            <td class="num">{battery}</td>
            <td class="dim">{node.hw_model.clone()}</td>
        </tr>
    }
}

/// How long ago, in the largest unit that fits. The units are the same in
/// every language the interface speaks.
fn ago(now: u32, then: u32) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3_600 => format!("{} min", seconds / 60),
        3_600..86_400 => format!("{} h", seconds / 3_600),
        _ => format!("{} d", seconds / 86_400),
    }
}
